//! Multi-match hosting: many independent [`NetServer`]s in one process.
//!
//! Each match keeps its own UDP socket (one port per match), so the wire protocol and the
//! client are unchanged: a client joins a match by connecting to its port. Matches are split
//! across worker threads; each worker runs the fixed 30 Hz loop for its matches (poll all
//! sockets, then tick every match that is due) and records how long the work took.
//!
//! Workers are plain long-lived threads rather than `moba-jobs` tasks: the loop sleeps
//! between ticks and polls non-blocking sockets, which does not fit a fork-join pool.
//!
//! **Flood isolation.** A worker polls its sockets round-robin (the starting match rotates
//! every pass) and caps each match's receive work per pass at [`DEFAULT_POLL_DATAGRAMS`]
//! datagrams or [`DEFAULT_POLL_TIME`], whichever comes first (see
//! [`MatchHost::set_poll_budget`]). Unread datagrams stay in that socket's kernel queue, so a
//! flood on one port costs its neighbours at most one capped slice per pass and the excess is
//! shed by the kernel. Every datagram is charged against the per-address, per-match and
//! optional host-wide ([`HostBudget`]) budgets before it is decoded.
//!
//! **Shared content reload.** [`MatchHost::watch_content`] loads and trust-checks a watched
//! pack once, on a host thread, and hands the same `Arc<GameContent>` to every match (each
//! applies it in its own Lobby/Results phase). Replace the file atomically: write a
//! temporary file in the same directory, then `rename` it over the watched path.

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use moba_game::GameContent;

use crate::server::{NetServer, SERVER_TICK_RATE, ServerConfig, ServerStats};

/// Default per-match cap on datagrams read in one worker pass.
pub const DEFAULT_POLL_DATAGRAMS: u32 = 64;
/// Default per-match cap on receive time in one worker pass.
pub const DEFAULT_POLL_TIME: Duration = Duration::from_micros(500);

/// Host-wide datagram budget shared by every match on every worker.
///
/// A lock-free token bucket (`packets_per_second` refill, burst of one second). Each match
/// charges it after its own per-address and per-match budgets admit a datagram and before
/// the datagram is decoded, so it caps the total decode/decrypt work a spoofed-source flood
/// can cause across the whole process. Legitimate packets compete for the same budget, so
/// size it above the expected aggregate (≈ 50 pps per connected client) with headroom.
#[derive(Debug)]
pub struct HostBudget {
    rate: u64,
    burst: i64,
    epoch: Instant,
    tokens: AtomicI64,
    /// Time since `epoch` (µs) up to which tokens have been credited.
    credited_micros: AtomicU64,
    rejected: AtomicU64,
}

impl HostBudget {
    /// Creates a full bucket; `0` admits everything.
    #[must_use]
    pub fn new(packets_per_second: u32) -> Self {
        let burst = i64::from(packets_per_second.max(1));
        Self {
            rate: u64::from(packets_per_second),
            burst,
            epoch: Instant::now(),
            tokens: AtomicI64::new(burst),
            credited_micros: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
        }
    }

    /// Configured refill rate (packets per second, 0 = unlimited).
    #[must_use]
    pub const fn packets_per_second(&self) -> u64 {
        self.rate
    }

    /// Datagrams refused so far.
    #[must_use]
    pub fn rejected(&self) -> u64 {
        self.rejected.load(Ordering::Relaxed)
    }

    /// Takes one token at `now`; returns `false` (and counts a rejection) when empty.
    pub fn try_take(&self, now: Instant) -> bool {
        if self.rate == 0 {
            return true;
        }
        self.refill(now);
        if self.tokens.fetch_sub(1, Ordering::AcqRel) >= 1 {
            return true;
        }
        self.tokens.fetch_add(1, Ordering::AcqRel);
        self.rejected.fetch_add(1, Ordering::Relaxed);
        false
    }

    /// Credits whole tokens for the time since the last credit. One thread wins the
    /// compare-exchange per interval; the fractional remainder carries over.
    fn refill(&self, now: Instant) {
        let now_micros = u64::try_from(now.saturating_duration_since(self.epoch).as_micros())
            .unwrap_or(u64::MAX);
        let credited = self.credited_micros.load(Ordering::Acquire);
        let elapsed = now_micros.saturating_sub(credited);
        let earned = u128::from(elapsed) * u128::from(self.rate) / 1_000_000;
        if earned == 0 {
            return;
        }
        let burst = u128::try_from(self.burst).unwrap_or(0);
        let (credit, advance) = if earned >= burst {
            (self.burst, elapsed)
        } else {
            // `earned * 1e6 / rate <= elapsed` because `earned` was rounded down.
            let advance =
                u64::try_from(earned * 1_000_000 / u128::from(self.rate)).unwrap_or(elapsed);
            (i64::try_from(earned).unwrap_or(self.burst), advance)
        };
        if self
            .credited_micros
            .compare_exchange(
                credited,
                credited + advance,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .is_err()
        {
            return;
        }
        let before = self.tokens.fetch_add(credit, Ordering::AcqRel);
        if before.saturating_add(credit) > self.burst {
            self.tokens.fetch_min(self.burst, Ordering::AcqRel);
        }
    }
}

/// A content pack watched once for a whole host and shared by every match.
///
/// [`ContentWatch::poll`] stats the file, and when its modification time changes loads,
/// validates and trust-checks it **once**, then publishes it under a new generation. Matches
/// that follow the watch ([`NetServer::follow_content`]) adopt the same `Arc<GameContent>`
/// in their next Lobby/Results phase, so every match serves the same pack.
///
/// Replace the watched file atomically (write a temporary file in the same directory, then
/// `rename` it over the path). A half-written file fails to load, is logged and is not
/// published; the next modification is tried again.
#[derive(Debug)]
pub struct ContentWatch {
    path: PathBuf,
    require_attestation: bool,
    publisher: Option<[u8; 32]>,
    modified: Mutex<Option<SystemTime>>,
    generation: AtomicU64,
    latest: Mutex<(u64, Option<Arc<GameContent>>)>,
}

impl ContentWatch {
    /// Watches `path`, applying the same trust policy as
    /// [`NetServer::set_content_trust`]. The file's current contents are assumed to be
    /// loaded already; only later modifications are published.
    #[must_use]
    pub fn new(path: PathBuf, require_attestation: bool, publisher: Option<[u8; 32]>) -> Self {
        let modified = std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok();
        Self {
            path,
            require_attestation: require_attestation || publisher.is_some(),
            publisher,
            modified: Mutex::new(modified),
            generation: AtomicU64::new(0),
            latest: Mutex::new((0, None)),
        }
    }

    /// Watched path.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Generation of the latest published pack (0 before the first reload).
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Latest published pack and its generation.
    #[must_use]
    pub fn latest(&self) -> (u64, Option<Arc<GameContent>>) {
        let latest = self.latest.lock().unwrap_or_else(PoisonError::into_inner);
        (latest.0, latest.1.clone())
    }

    /// Checks the file and publishes a new generation when it changed and loaded cleanly.
    /// Returns whether a new pack was published.
    pub fn poll(&self) -> bool {
        let Ok(modified) = std::fs::metadata(&self.path).and_then(|metadata| metadata.modified())
        else {
            return false;
        };
        {
            let mut seen = self.modified.lock().unwrap_or_else(PoisonError::into_inner);
            if *seen == Some(modified) {
                return false;
            }
            *seen = Some(modified);
        }
        let content = match moba_game::load_content_file(&self.path) {
            Ok(content) => content,
            Err(error) => {
                eprintln!("content reload failed for {}: {error}", self.path.display());
                return false;
            }
        };
        if !crate::server::content_trusted(&content, self.require_attestation, self.publisher) {
            eprintln!("content reload rejected: pack trust policy failed");
            return false;
        }
        let hash = content.hash();
        let mut latest = self.latest.lock().unwrap_or_else(PoisonError::into_inner);
        let generation = latest.0 + 1;
        *latest = (generation, Some(Arc::new(content)));
        self.generation.store(generation, Ordering::Release);
        eprintln!(
            "content reload: published {} (generation {generation}, hash {hash:#018x})",
            self.path.display()
        );
        true
    }
}

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
#[non_exhaustive]
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
    /// Worker passes in which a match hit its per-pass poll cap (datagrams left queued).
    pub poll_capped: u64,
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
#[non_exhaustive]
pub struct HostReport {
    /// One entry per worker thread.
    pub workers: Vec<WorkerStats>,
    /// One entry per match, in match order.
    pub matches: Vec<ServerStats>,
    /// Per-match tick-cost histogram, in match order.
    pub match_ticks: Vec<TickHistogram>,
    /// Worker index owning each match, in match order.
    pub match_workers: Vec<usize>,
    /// Datagrams refused by the host-wide budget (0 when none is configured).
    pub host_budget_rejected: u64,
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
    poll_datagrams: u32,
    poll_time: Duration,
    budget: Option<Arc<HostBudget>>,
    content_watch: Option<Arc<ContentWatch>>,
}

impl MatchHost {
    /// Wraps already-configured servers. `workers` is clamped to `1..=matches`, so the
    /// reported count is the number of threads `run_until` actually starts.
    ///
    /// This is the production entry point: configure each `NetServer` (keys, identity,
    /// trust, results log, ...) before handing it over.
    #[must_use]
    pub fn new(matches: Vec<NetServer>, workers: usize) -> Self {
        let workers = workers.clamp(1, matches.len().max(1));
        Self {
            matches,
            workers,
            poll_datagrams: DEFAULT_POLL_DATAGRAMS,
            poll_time: DEFAULT_POLL_TIME,
            budget: None,
            content_watch: None,
        }
    }

    /// Binds `count` matches from one template (**test and load-tool helper**).
    ///
    /// Match `i` binds `template.bind` with its port increased by `i`; a template port of
    /// 0 gives every match an ephemeral port. Each match gets a distinct simulation seed
    /// so concurrent matches do not play identically.
    ///
    /// Only [`ServerConfig`] is applied: no pre-shared client tokens, client identity
    /// allowlist, server identity key, content trust policy or results log. Configure
    /// those through [`MatchHost::matches_mut`] before [`MatchHost::run_until`], or build
    /// the servers yourself and use [`MatchHost::new`] as `moba-server` does.
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

    /// Caps each match's receive work per worker pass (defaults:
    /// [`DEFAULT_POLL_DATAGRAMS`], [`DEFAULT_POLL_TIME`]); at least one datagram is read.
    pub fn set_poll_budget(&mut self, max_datagrams: u32, max_time: Duration) {
        self.poll_datagrams = max_datagrams.max(1);
        self.poll_time = max_time;
    }

    /// Installs a host-wide datagram budget shared by every match (`0` removes it) and
    /// returns it so callers can read its rejection counter.
    pub fn set_host_packet_budget(&mut self, packets_per_second: u32) -> Option<Arc<HostBudget>> {
        self.budget =
            (packets_per_second > 0).then(|| Arc::new(HostBudget::new(packets_per_second)));
        for server in &mut self.matches {
            server.set_host_budget(self.budget.clone());
        }
        self.budget.clone()
    }

    /// Watches one content pack for every match: `run_until` polls it once per second on a
    /// host thread and each match adopts the shared pack in its next Lobby/Results phase.
    /// See [`ContentWatch`] for the atomic-replace requirement.
    pub fn watch_content(&mut self, watch: Arc<ContentWatch>) {
        for server in &mut self.matches {
            server.follow_content(Arc::clone(&watch));
        }
        self.content_watch = Some(watch);
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
        let budget = PollBudget {
            datagrams: self.poll_datagrams,
            time: self.poll_time,
        };
        let watch = self.content_watch.clone();
        let finished = AtomicBool::new(false);
        let reports: Vec<(WorkerStats, Vec<TickHistogram>)> = std::thread::scope(|scope| {
            if let Some(watch) = watch.as_deref() {
                let finished = &finished;
                std::thread::Builder::new()
                    .name(String::from("content-watch"))
                    .spawn_scoped(scope, move || {
                        watch_loop(watch, finished, shutdown);
                    })
                    .expect("spawn content watcher");
            }
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
                            run_worker(chunk, start + phase, duration, shutdown, budget)
                        })
                        .expect("spawn match worker"),
                );
            }
            let reports = handles
                .into_iter()
                .map(|handle| handle.join().expect("match worker panicked"))
                .collect();
            finished.store(true, Ordering::Relaxed);
            reports
        });
        for server in &mut self.matches {
            server.finish_recording();
        }
        let mut report = HostReport {
            workers: Vec::with_capacity(reports.len()),
            matches: self.matches.iter().map(NetServer::stats).collect(),
            match_ticks: Vec::with_capacity(self.matches.len()),
            match_workers: Vec::with_capacity(self.matches.len()),
            host_budget_rejected: self.budget.as_deref().map_or(0, HostBudget::rejected),
        };
        for (index, (worker, ticks)) in reports.into_iter().enumerate() {
            report
                .match_workers
                .extend(core::iter::repeat_n(index, ticks.len()));
            report.match_ticks.extend(ticks);
            report.workers.push(worker);
        }
        report
    }
}

/// Per-match receive cap for one worker pass.
#[derive(Copy, Clone, Debug)]
struct PollBudget {
    datagrams: u32,
    time: Duration,
}

/// Polls a shared content watch about once per second until the workers finish.
fn watch_loop(watch: &ContentWatch, finished: &AtomicBool, shutdown: &AtomicBool) {
    let step = Duration::from_millis(100);
    let mut since_poll = Duration::ZERO;
    while !finished.load(Ordering::Relaxed) && !shutdown.load(Ordering::Relaxed) {
        std::thread::sleep(step);
        since_poll += step;
        if since_poll >= Duration::from_secs(1) {
            since_poll = Duration::ZERO;
            watch.poll();
        }
    }
}

/// Runs one worker's matches on a fixed 30 Hz schedule starting at `first_frame`.
///
/// Each pass polls every match once, starting one match later than the previous pass, and
/// each poll reads at most `budget` (datagrams or time), so a flooded socket cannot starve
/// the others. Each match's poll time since its previous tick is added to that tick's
/// duration, so the histograms reflect the full per-match cost (receive, decrypt, handshake
/// and tick). Returns the worker stats and one histogram per match.
fn run_worker(
    matches: &mut [NetServer],
    first_frame: Instant,
    duration: Duration,
    shutdown: &AtomicBool,
    budget: PollBudget,
) -> (WorkerStats, Vec<TickHistogram>) {
    let tick = Duration::from_secs_f64(1.0 / f64::from(SERVER_TICK_RATE));
    let tick_nanos = tick.as_nanos();
    let mut stats = WorkerStats {
        matches: matches.len(),
        ..WorkerStats::default()
    };
    // Allocated once per worker; the loop itself never allocates.
    let mut poll_nanos = vec![0u64; matches.len()];
    let mut match_ticks = vec![TickHistogram::default(); matches.len()];
    let mut errors_logged = Instant::now().checked_sub(Duration::from_secs(1));
    let start = Instant::now();
    let mut next_frame = first_frame;
    let mut first_polled = 0usize;
    while start.elapsed() < duration && !shutdown.load(Ordering::Relaxed) {
        let busy_start = Instant::now();
        for offset in 0..matches.len() {
            let index = (first_polled + offset) % matches.len();
            let poll_start = Instant::now();
            match matches[index].poll_budgeted(budget.datagrams, budget.time) {
                Ok(true) => stats.poll_capped += 1,
                Ok(false) => {}
                Err(error) => note_error(&mut stats, &mut errors_logged, "poll", &error),
            }
            poll_nanos[index] += nanos(poll_start.elapsed());
        }
        first_polled = (first_polled + 1) % matches.len().max(1);
        let now = Instant::now();
        if now >= next_frame {
            stats.max_lateness_nanos = stats.max_lateness_nanos.max(nanos(now - next_frame));
            for ((server, polled), histogram) in matches
                .iter_mut()
                .zip(poll_nanos.iter_mut())
                .zip(match_ticks.iter_mut())
            {
                let tick_start = Instant::now();
                if let Err(error) = server.tick() {
                    note_error(&mut stats, &mut errors_logged, "tick", &error);
                }
                let cost =
                    Duration::from_nanos(nanos(tick_start.elapsed()) + core::mem::take(polled));
                stats.ticks.record(cost);
                histogram.record(cost);
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
    (stats, match_ticks)
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
        let template = ServerConfig::default()
            .with_bind("127.0.0.1:0".parse().unwrap())
            .with_countdown_ticks(0);
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
        let template = ServerConfig::default().with_bind("127.0.0.1:0".parse().unwrap());
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
        let template = ServerConfig::default().with_bind("127.0.0.1:0".parse().unwrap());
        let content = Arc::new(GameContent::default());
        let mut host = MatchHost::bind(&template, &content, 2, 8).unwrap();
        assert_eq!(host.workers(), 2, "workers clamp to the match count");
        let shutdown = AtomicBool::new(true);
        let started = Instant::now();
        let report = host.run_until(Duration::from_secs(30), &shutdown);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(report.frames(), 0);
    }

    #[test]
    fn host_budget_refills_and_caps_concurrent_takers() {
        let budget = HostBudget::new(100);
        let now = Instant::now();
        assert_eq!((0..150).filter(|_| budget.try_take(now)).count(), 100);
        assert_eq!(budget.rejected(), 50);
        // 50 ms at 100 pps earns five whole tokens.
        let later = now + Duration::from_millis(50);
        assert_eq!((0..20).filter(|_| budget.try_take(later)).count(), 5);
        // A long idle period refills to the burst, never beyond it.
        let idle = later + Duration::from_secs(3_600);
        assert_eq!((0..500).filter(|_| budget.try_take(idle)).count(), 100);

        // Many threads racing on the same instant never take more than the burst.
        let shared = Arc::new(HostBudget::new(1_000));
        let at = Instant::now();
        let taken: usize = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let shared = Arc::clone(&shared);
                    scope.spawn(move || (0..1_000).filter(|_| shared.try_take(at)).count())
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .sum()
        });
        assert_eq!(taken, 1_000);
        assert!(HostBudget::new(0).try_take(at), "0 is unlimited");
    }

    #[test]
    fn flooded_match_is_capped_and_neighbours_keep_running() {
        // Small budgets, so junk is charged even where the OS delivers only a fraction of the
        // flood (macOS's default UDP receive buffer drops most of it).
        let template = ServerConfig::default()
            .with_bind("127.0.0.1:0".parse().unwrap())
            .with_countdown_ticks(0)
            .with_max_packets_per_second(10)
            .with_max_total_packets_per_second(40);
        let content = Arc::new(GameContent::default());
        let mut host = MatchHost::bind(&template, &content, 2, 1).unwrap();
        host.set_poll_budget(8, Duration::from_micros(200));
        let budget = host
            .set_host_packet_budget(1_000)
            .expect("budget installed");
        let target = host.local_addrs().unwrap()[0];
        let stop = Arc::new(AtomicBool::new(false));
        let flood_stop = Arc::clone(&stop);
        let flooder = std::thread::spawn(move || {
            // Several sources, so junk gets past the per-address bucket and reaches the
            // per-match and host-wide budgets.
            let sockets: Vec<std::net::UdpSocket> = (0..8)
                .map(|_| std::net::UdpSocket::bind("127.0.0.1:0").unwrap())
                .collect();
            let junk = [0xA5u8; 96];
            let mut sent = 0u64;
            while !flood_stop.load(Ordering::Relaxed) {
                for socket in &sockets {
                    for _ in 0..16 {
                        let _ = socket.send_to(&junk, target);
                        sent += 1;
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            sent
        });
        let shutdown = AtomicBool::new(false);
        let report = host.run_until(Duration::from_millis(600), &shutdown);
        stop.store(true, Ordering::Relaxed);
        let sent = flooder.join().unwrap();
        assert!(sent > 1_000, "flood sent {sent}");
        let flooded = &report.matches[0];
        assert!(flooded.rate_limited > 0, "junk is charged: {flooded:?}");
        assert!(
            report.workers[0].poll_capped > 0,
            "the per-pass cap engaged"
        );
        assert_eq!(report.matches[1].rate_limited, 0);
        assert_eq!(report.match_ticks.len(), 2);
        assert_eq!(report.match_workers, vec![0, 0]);
        // Both matches ticked every frame.
        assert_eq!(report.match_ticks[1].count(), report.frames());
        assert_eq!(report.host_budget_rejected, budget.rejected());
    }

    #[test]
    fn shared_content_watch_loads_once_for_every_match() {
        let dir = std::env::temp_dir().join(format!(
            "moba-shared-reload-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pack.toml");
        std::fs::write(&path, "[[items]]\nname = \"First\"\ncost = 1\n").unwrap();
        let template = ServerConfig::default().with_bind("127.0.0.1:0".parse().unwrap());
        let content = Arc::new(GameContent::default());
        let mut host = MatchHost::bind(&template, &content, 3, 1).unwrap();
        let watch = Arc::new(ContentWatch::new(path.clone(), false, None));
        host.watch_content(Arc::clone(&watch));
        assert!(!watch.poll(), "unchanged file is not republished");

        // A torn write fails to load and is not published.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&path, "[[items]]\nname = \"Tor").unwrap();
        assert!(!watch.poll());
        assert_eq!(watch.generation(), 0);

        // Atomic replace: write a temporary file, then rename it over the watched path.
        std::thread::sleep(Duration::from_millis(20));
        let staging = dir.join("pack.toml.tmp");
        std::fs::write(&staging, "[[items]]\nname = \"Shared\"\ncost = 2\n").unwrap();
        std::fs::rename(&staging, &path).unwrap();
        assert!(watch.poll());
        assert_eq!(watch.generation(), 1);
        for server in host.matches_mut() {
            for _ in 0..30 {
                server.tick().unwrap();
            }
        }
        let matches = host.matches_mut();
        assert_eq!(matches[0].simulation().content().items[0].name, "Shared");
        for server in &matches[1..] {
            assert!(
                std::ptr::eq(
                    server.simulation().content(),
                    matches[0].simulation().content()
                ),
                "every match serves the same Arc<GameContent>"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
