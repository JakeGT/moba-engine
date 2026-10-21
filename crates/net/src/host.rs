//! Multi-match hosting: many independent [`NetServer`]s in one process.
//!
//! Each match keeps its own UDP socket (one port per match), so the wire protocol and the
//! client are unchanged: a client joins a match by connecting to its port. Matches are split
//! across worker threads; each worker runs the fixed 30 Hz loop for its matches (poll all
//! sockets, then tick every match that is due) and records how long the work took.
//!
//! Workers are plain long-lived threads rather than `moba-jobs` tasks: the loop sleeps
//! between ticks and polls non-blocking sockets, which does not fit a fork-join pool.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use moba_game::GameContent;

use crate::server::{NetServer, SERVER_TICK_RATE, ServerConfig, ServerStats};

/// Width of one tick-duration histogram bucket.
const BUCKET_NANOS: u64 = 10_000;
/// Histogram buckets (10 µs each, so 0–20 ms; slower ticks land in the last bucket).
const BUCKETS: usize = 2_000;

/// Fixed-size histogram of per-match tick durations; recording never allocates.
#[derive(Clone)]
pub struct TickHistogram {
    counts: Box<[u32; BUCKETS]>,
    total: u64,
    max_nanos: u64,
}

impl core::fmt::Debug for TickHistogram {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TickHistogram")
            .field("total", &self.total)
            .field("p50_nanos", &self.percentile_nanos(50.0))
            .field("p99_nanos", &self.percentile_nanos(99.0))
            .field("max_nanos", &self.max_nanos)
            .finish_non_exhaustive()
    }
}

impl Default for TickHistogram {
    fn default() -> Self {
        Self {
            counts: Box::new([0; BUCKETS]),
            total: 0,
            max_nanos: 0,
        }
    }
}

impl TickHistogram {
    /// Records one tick duration.
    pub fn record(&mut self, duration: Duration) {
        let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        let bucket = usize::try_from(nanos / BUCKET_NANOS)
            .unwrap_or(usize::MAX)
            .min(BUCKETS - 1);
        self.counts[bucket] = self.counts[bucket].saturating_add(1);
        self.total += 1;
        self.max_nanos = self.max_nanos.max(nanos);
    }

    /// Adds another histogram's samples.
    pub fn merge(&mut self, other: &Self) {
        for (count, other) in self.counts.iter_mut().zip(other.counts.iter()) {
            *count = count.saturating_add(*other);
        }
        self.total += other.total;
        self.max_nanos = self.max_nanos.max(other.max_nanos);
    }

    /// Number of recorded ticks.
    #[must_use]
    pub const fn count(&self) -> u64 {
        self.total
    }

    /// Slowest recorded tick.
    #[must_use]
    pub const fn max_nanos(&self) -> u64 {
        self.max_nanos
    }

    /// Upper bound of the bucket holding the given percentile (0–100), or 0 when empty.
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // Rank math.
    #[allow(clippy::cast_sign_loss)] // `percentile` is clamped to 0..=100.
    pub fn percentile_nanos(&self, percentile: f64) -> u64 {
        if self.total == 0 {
            return 0;
        }
        let rank = ((percentile.clamp(0.0, 100.0) / 100.0) * self.total as f64).ceil() as u64;
        let rank = rank.max(1);
        let mut seen = 0u64;
        for (index, count) in self.counts.iter().enumerate() {
            seen += u64::from(*count);
            if seen >= rank {
                if index == BUCKETS - 1 {
                    // Overflow bucket: its only honest bound is the recorded maximum.
                    return self.max_nanos;
                }
                return ((index as u64 + 1) * BUCKET_NANOS).min(self.max_nanos);
            }
        }
        self.max_nanos
    }
}

/// Timing for one worker thread.
#[derive(Clone, Debug, Default)]
pub struct WorkerStats {
    /// Matches owned by this worker.
    pub matches: usize,
    /// 30 Hz frames executed (each ticks every owned match once).
    pub frames: u64,
    /// Frames missed because the worker fell behind the 30 Hz schedule (a frame that
    /// finishes `n` whole periods late counts `n + 1`).
    pub overruns: u64,
    /// Worst lateness of a frame start relative to its schedule.
    pub max_lateness_nanos: u64,
    /// Time spent polling and ticking (excluding sleep).
    pub busy_nanos: u64,
    /// Wall time the worker ran.
    pub elapsed_nanos: u64,
    /// Socket errors returned by `poll`/`tick` (the match keeps running).
    pub errors: u64,
    /// Per-match cost per tick (tick plus that match's polling since its previous tick).
    pub ticks: TickHistogram,
}

impl WorkerStats {
    /// Fraction of wall time spent working (0–1).
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // Reporting only.
    pub fn utilisation(&self) -> f64 {
        if self.elapsed_nanos == 0 {
            0.0
        } else {
            self.busy_nanos as f64 / self.elapsed_nanos as f64
        }
    }
}

/// Result of a host run.
#[derive(Clone, Debug, Default)]
pub struct HostReport {
    /// One entry per worker thread.
    pub workers: Vec<WorkerStats>,
    /// One entry per match, in match order.
    pub matches: Vec<ServerStats>,
}

impl HostReport {
    /// Tick-duration histogram across every match.
    #[must_use]
    pub fn ticks(&self) -> TickHistogram {
        let mut total = TickHistogram::default();
        for worker in &self.workers {
            total.merge(&worker.ticks);
        }
        total
    }

    /// Total overrun frames across workers.
    #[must_use]
    pub fn overruns(&self) -> u64 {
        self.workers.iter().map(|worker| worker.overruns).sum()
    }

    /// Total frames across workers.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.workers.iter().map(|worker| worker.frames).sum()
    }
}

/// Hosts many matches in one process.
#[derive(Debug)]
pub struct MatchHost {
    matches: Vec<NetServer>,
    workers: usize,
}

impl MatchHost {
    /// Wraps already-configured servers. `workers` is clamped to `1..=matches`, so the
    /// reported count is the number of threads `run_until` actually starts.
    #[must_use]
    pub fn new(matches: Vec<NetServer>, workers: usize) -> Self {
        let workers = workers.clamp(1, matches.len().max(1));
        Self { matches, workers }
    }

    /// Binds `count` matches from one template.
    ///
    /// Match `i` binds `template.bind` with its port increased by `i`; a template port of
    /// 0 gives every match an ephemeral port. Each match gets a distinct simulation seed
    /// so concurrent matches do not play identically.
    ///
    /// # Errors
    /// Returns the first bind error.
    pub fn bind(
        template: &ServerConfig,
        content: &Arc<GameContent>,
        count: usize,
        workers: usize,
    ) -> io::Result<Self> {
        let mut matches = Vec::with_capacity(count);
        for index in 0..count {
            let mut config = template.clone();
            config.bind = match_addr(template.bind, index)?;
            config.sim.seed = template
                .sim
                .seed
                .wrapping_add((index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            config.record = template
                .record
                .as_ref()
                .map(|path| suffixed_path(path, index));
            matches.push(NetServer::bind_with_content(config, Arc::clone(content))?);
        }
        Ok(Self::new(matches, workers))
    }

    /// Number of matches.
    #[must_use]
    pub fn len(&self) -> usize {
        self.matches.len()
    }

    /// Whether the host has no matches.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// Number of worker threads `run_until` will use.
    #[must_use]
    pub const fn workers(&self) -> usize {
        self.workers
    }

    /// Bound address of every match, in match order.
    ///
    /// # Errors
    /// Returns the OS error if an address cannot be queried.
    pub fn local_addrs(&self) -> io::Result<Vec<SocketAddr>> {
        self.matches.iter().map(NetServer::local_addr).collect()
    }

    /// Mutable access to every match, e.g. to install keys or systems before running.
    pub fn matches_mut(&mut self) -> &mut [NetServer] {
        &mut self.matches
    }

    /// Runs every match at 30 Hz until `duration` elapses or `shutdown` is set, then
    /// flushes in-progress recordings.
    ///
    /// Matches are split as evenly as possible across the workers, and each worker's 30 Hz
    /// schedule is offset by `tick × index / workers` so snapshot bursts do not align. A
    /// socket error in one match is counted (and logged at a bounded rate); it never stops
    /// other matches.
    pub fn run_until(&mut self, duration: Duration, shutdown: &AtomicBool) -> HostReport {
        let tick = Duration::from_secs_f64(1.0 / f64::from(SERVER_TICK_RATE));
        let workers = self.workers.min(self.matches.len()).max(1);
        let base = self.matches.len() / workers;
        let extra = self.matches.len() % workers;
        let start = Instant::now();
        let reports: Vec<WorkerStats> = std::thread::scope(|scope| {
            let mut rest: &mut [NetServer] = &mut self.matches;
            let mut handles = Vec::with_capacity(workers);
            for index in 0..workers {
                let take = base + usize::from(index < extra);
                let (chunk, tail) = rest.split_at_mut(take);
                rest = tail;
                let phase = tick.mul_f64(index as f64 / workers as f64);
                handles.push(
                    std::thread::Builder::new()
                        .name(format!("match-worker-{index}"))
                        .spawn_scoped(scope, move || {
                            run_worker(chunk, start + phase, duration, shutdown)
                        })
                        .expect("spawn match worker"),
                );
            }
            handles
                .into_iter()
                .map(|handle| handle.join().expect("match worker panicked"))
                .collect()
        });
        for server in &mut self.matches {
            server.finish_recording();
        }
        HostReport {
            workers: reports,
            matches: self.matches.iter().map(NetServer::stats).collect(),
        }
    }
}

/// Runs one worker's matches on a fixed 30 Hz schedule starting at `first_frame`.
///
/// Each match's poll time since its previous tick is added to that tick's duration, so the
/// histogram reflects the full per-match cost (receive, decrypt, handshake and tick).
fn run_worker(
    matches: &mut [NetServer],
    first_frame: Instant,
    duration: Duration,
    shutdown: &AtomicBool,
) -> WorkerStats {
    let tick = Duration::from_secs_f64(1.0 / f64::from(SERVER_TICK_RATE));
    let tick_nanos = tick.as_nanos();
    let mut stats = WorkerStats {
        matches: matches.len(),
        ..WorkerStats::default()
    };
    // Allocated once per worker; the loop itself never allocates.
    let mut poll_nanos = vec![0u64; matches.len()];
    let mut errors_logged = Instant::now().checked_sub(Duration::from_secs(1));
    let start = Instant::now();
    let mut next_frame = first_frame;
    while start.elapsed() < duration && !shutdown.load(Ordering::Relaxed) {
        let busy_start = Instant::now();
        for (server, polled) in matches.iter_mut().zip(poll_nanos.iter_mut()) {
            let poll_start = Instant::now();
            if let Err(error) = server.poll() {
                note_error(&mut stats, &mut errors_logged, "poll", &error);
            }
            *polled += nanos(poll_start.elapsed());
        }
        let now = Instant::now();
        if now >= next_frame {
            stats.max_lateness_nanos = stats.max_lateness_nanos.max(nanos(now - next_frame));
            for (server, polled) in matches.iter_mut().zip(poll_nanos.iter_mut()) {
                let tick_start = Instant::now();
                if let Err(error) = server.tick() {
                    note_error(&mut stats, &mut errors_logged, "tick", &error);
                }
                let cost = nanos(tick_start.elapsed()) + core::mem::take(polled);
                stats.ticks.record(Duration::from_nanos(cost));
            }
            stats.frames += 1;
            next_frame += tick;
            let after = Instant::now();
            if next_frame < after {
                // Every whole period we fell behind is a dropped frame. Reschedule rather
                // than burst to catch up.
                stats.overruns += dropped_frames(after - next_frame, tick_nanos);
                next_frame = after + tick;
            }
        }
        stats.busy_nanos += nanos(busy_start.elapsed());
        let now = Instant::now();
        if next_frame > now {
            std::thread::sleep((next_frame - now).min(Duration::from_millis(1)));
        }
    }
    stats.elapsed_nanos = nanos(start.elapsed());
    stats
}

/// Frames lost when the next frame is already `behind` overdue.
fn dropped_frames(behind: Duration, tick_nanos: u128) -> u64 {
    u64::try_from(behind.as_nanos() / tick_nanos.max(1)).unwrap_or(u64::MAX) + 1
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// Counts an error and logs at most one line per second per worker.
fn note_error(
    stats: &mut WorkerStats,
    last_logged: &mut Option<Instant>,
    what: &str,
    error: &io::Error,
) {
    stats.errors += 1;
    if last_logged.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
        *last_logged = Some(Instant::now());
        eprintln!(
            "match host: {what} error: {error} ({} so far on this worker)",
            stats.errors
        );
    }
}

fn match_addr(base: SocketAddr, index: usize) -> io::Result<SocketAddr> {
    if base.port() == 0 {
        return Ok(base);
    }
    let port = u16::try_from(usize::from(base.port()) + index).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "match port range exceeds 65535",
        )
    })?;
    let mut addr = base;
    addr.set_port(port);
    Ok(addr)
}

/// `match.mbr` → `match-m03.mbr` (directories are left unchanged).
#[must_use]
pub fn suffixed_path(path: &std::path::Path, index: usize) -> std::path::PathBuf {
    let stem = path
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    let name = match path.extension() {
        Some(extension) => format!("{stem}-m{index:02}.{}", extension.to_string_lossy()),
        None => format!("{stem}-m{index:02}"),
    };
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_percentiles_and_merge() {
        let mut histogram = TickHistogram::default();
        for micros in 1..=100u64 {
            histogram.record(Duration::from_micros(micros));
        }
        assert_eq!(histogram.count(), 100);
        // The 50th sample (50 µs) is in the [50, 60) µs bucket; percentiles report the
        // bucket's upper bound, so they never understate a tick.
        assert_eq!(histogram.percentile_nanos(50.0), 60_000);
        assert_eq!(histogram.percentile_nanos(99.0), 100_000);
        assert_eq!(histogram.max_nanos(), 100_000);
        let mut other = TickHistogram::default();
        other.record(Duration::from_millis(50)); // overflow bucket
        histogram.merge(&other);
        assert_eq!(histogram.count(), 101);
        assert_eq!(histogram.max_nanos(), 50_000_000);
        assert_eq!(histogram.percentile_nanos(100.0), 50_000_000);
        assert_eq!(TickHistogram::default().percentile_nanos(99.0), 0);
    }

    #[test]
    fn ports_and_paths_are_per_match() {
        let base: SocketAddr = "127.0.0.1:7777".parse().unwrap();
        assert_eq!(match_addr(base, 3).unwrap().port(), 7780);
        let ephemeral: SocketAddr = "127.0.0.1:0".parse().unwrap();
        assert_eq!(match_addr(ephemeral, 9).unwrap().port(), 0);
        let high: SocketAddr = "127.0.0.1:65535".parse().unwrap();
        assert!(match_addr(high, 1).is_err());
        assert_eq!(
            suffixed_path(std::path::Path::new("/tmp/match.mbr"), 3),
            std::path::PathBuf::from("/tmp/match-m03.mbr")
        );
        assert_eq!(
            suffixed_path(std::path::Path::new("results"), 12),
            std::path::PathBuf::from("results-m12")
        );
    }

    #[test]
    fn host_runs_matches_concurrently_and_reports() {
        let template = ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            countdown_ticks: 0,
            ..ServerConfig::default()
        };
        let content = Arc::new(GameContent::default());
        let mut host = MatchHost::bind(&template, &content, 4, 2).unwrap();
        assert_eq!(host.len(), 4);
        assert_eq!(host.workers(), 2);
        let addrs = host.local_addrs().unwrap();
        let mut ports: Vec<u16> = addrs.iter().map(SocketAddr::port).collect();
        ports.sort_unstable();
        ports.dedup();
        assert_eq!(ports.len(), 4, "every match has its own socket");
        let shutdown = AtomicBool::new(false);
        let report = host.run_until(Duration::from_millis(400), &shutdown);
        assert_eq!(report.workers.len(), 2);
        assert_eq!(report.matches.len(), 4);
        assert_eq!(
            report
                .workers
                .iter()
                .map(|worker| worker.matches)
                .sum::<usize>(),
            4
        );
        // ~12 frames in 400 ms; allow generous slack for loaded CI machines.
        assert!(report.frames() >= 4, "frames={}", report.frames());
        assert_eq!(report.ticks().count(), report.frames() * 2);
        assert!(report.workers.iter().all(|worker| worker.errors == 0));
    }

    #[test]
    fn stalls_count_every_dropped_frame() {
        let tick = Duration::from_secs_f64(1.0 / 30.0).as_nanos();
        assert_eq!(dropped_frames(Duration::from_millis(1), tick), 1);
        // A 200 ms stall drops six 33 ms frames plus the late one.
        assert_eq!(dropped_frames(Duration::from_millis(200), tick), 7);
    }

    #[test]
    fn matches_are_split_evenly_across_workers() {
        let template = ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            ..ServerConfig::default()
        };
        let content = Arc::new(GameContent::default());
        let mut host = MatchHost::bind(&template, &content, 5, 2).unwrap();
        let shutdown = AtomicBool::new(false);
        let report = host.run_until(Duration::from_millis(100), &shutdown);
        let mut split: Vec<usize> = report.workers.iter().map(|worker| worker.matches).collect();
        split.sort_unstable();
        assert_eq!(split, vec![2, 3]);
    }

    #[test]
    fn shutdown_flag_stops_the_host() {
        let template = ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            ..ServerConfig::default()
        };
        let content = Arc::new(GameContent::default());
        let mut host = MatchHost::bind(&template, &content, 2, 8).unwrap();
        assert_eq!(host.workers(), 2, "workers clamp to the match count");
        let shutdown = AtomicBool::new(true);
        let started = Instant::now();
        let report = host.run_until(Duration::from_secs(30), &shutdown);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(report.frames(), 0);
    }
}
