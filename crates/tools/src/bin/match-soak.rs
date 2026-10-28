//! Run many deterministic bot matches headlessly and verify replayability.
//!
//! Usage: `cargo run --release -p moba-tools --bin match-soak -- [--matches N] [--ticks N]`
//!
//! Fixed mirrors: `--matches 300 --pick 2 --fixed-picks`. The seed drives per-slot bot RNG
//! streams and the initial scheduling phase, so each seed is an independent match.
//!
//! Balance sweep: `--matches 5000 --ticks 60000 --no-verify --threads 8`. Matches that hit
//! the tick cap without a winner, and matches drawn because both teams met a win condition
//! in the same tick (sim logic v8), count as draws; the blue win rate and its Wilson 95 %
//! interval are computed over decided matches only. `peak_units` is the largest live entity
//! count seen after any tick, to show whether waves ever reach `MAX_UNITS`. `--threads` spreads seeds across
//! worker threads; results are merged in seed order, so the report does not depend on
//! the thread count (apart from timings, which share cores).
#![allow(clippy::print_stdout)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use moba_game::{GameContent, Team};
use moba_sim::{MatchOutcome, SimConfig, Simulation, TickInputs};

/// Two-sided 95 % normal quantile.
const Z_95: f64 = 1.959_963_984_540_054;

struct MatchResult {
    /// Seed-independent state hash, so distinct seeds that play out identically collide.
    hash: u64,
    outcome: Option<MatchOutcome>,
    ticks: u32,
    peak_units: usize,
    tick_nanos: u64,
    max_tick_nanos: u64,
    max_tick_at: u32,
}

#[derive(Clone)]
struct Settings {
    matches: u32,
    max_ticks: u32,
    seed: u64,
    verify: bool,
    bot_mask: u16,
    spawn_waves: bool,
    pick: Option<u8>,
    vary_picks: bool,
    content: Option<Arc<GameContent>>,
}

impl Settings {
    fn picks(&self, index: u32) -> [u8; moba_sim::MAX_PLAYERS] {
        self.pick.map_or_else(
            || {
                if self.vary_picks {
                    let mut team = [0u8, 1, 2, 3, 4];
                    let rotation = (index as usize) % team.len().max(1);
                    team.rotate_left(rotation);
                    [
                        team[0], team[1], team[2], team[3], team[4], team[0], team[1], team[2],
                        team[3], team[4],
                    ]
                } else {
                    SimConfig::default().hero_picks
                }
            },
            |pick| [pick; moba_sim::MAX_PLAYERS],
        )
    }

    fn run_match(&self, seed: u64, hero_picks: [u8; moba_sim::MAX_PLAYERS]) -> MatchResult {
        let config = SimConfig::default()
            .with_seed(seed)
            .with_bot_mask(self.bot_mask)
            .with_spawn_waves(self.spawn_waves)
            .with_hero_picks(hero_picks);
        let mut simulation = self.content.as_ref().map_or_else(
            || Simulation::new(config),
            |content| Simulation::new_with_content(config, Arc::clone(content)),
        );
        let mut tick_nanos = 0u64;
        let mut max_tick_nanos = 0u64;
        let mut max_tick_at = 0u32;
        let mut ticks = 0u32;
        let mut peak_units = simulation.unit_count();
        for _ in 0..self.max_ticks {
            let inputs: TickInputs = simulation.bot_inputs();
            let tick_start = Instant::now();
            simulation.tick(&inputs);
            simulation.clear_events();
            let elapsed = u64::try_from(tick_start.elapsed().as_nanos()).unwrap_or(u64::MAX);
            tick_nanos = tick_nanos.saturating_add(elapsed);
            if elapsed > max_tick_nanos {
                max_tick_nanos = elapsed;
                max_tick_at = ticks;
            }
            ticks += 1;
            peak_units = peak_units.max(simulation.unit_count());
            if simulation.outcome().is_some() {
                break;
            }
        }
        MatchResult {
            hash: simulation.state_hash_normalized(),
            outcome: simulation.outcome(),
            ticks,
            peak_units,
            tick_nanos,
            max_tick_nanos,
            max_tick_at,
        }
    }

    /// Plays match `index` (and its verification replay). Returns the result and whether
    /// the replay diverged.
    fn play(&self, index: u32) -> (MatchResult, bool) {
        let match_seed = self.seed.wrapping_add(u64::from(index));
        let picks = self.picks(index);
        let first = self.run_match(match_seed, picks);
        let mut mismatch = false;
        if self.verify {
            let second = self.run_match(match_seed, picks);
            if first.hash != second.hash
                || first.outcome != second.outcome
                || first.ticks != second.ticks
            {
                mismatch = true;
                eprintln!(
                    "nondeterministic match seed={match_seed:#x}: {:#x}/{} vs {:#x}/{}",
                    first.hash, first.ticks, second.hash, second.ticks
                );
            }
        }
        (first, mismatch)
    }
}

/// Wilson score interval for `successes / trials` at 95 %, as `(low, high)` fractions.
fn wilson_95(successes: u32, trials: u32) -> Option<(f64, f64)> {
    if trials == 0 {
        return None;
    }
    let n = f64::from(trials);
    let p = f64::from(successes) / n;
    let z2 = Z_95 * Z_95;
    let denominator = 1.0 + z2 / n;
    let centre = (p + z2 / (2.0 * n)) / denominator;
    let half = Z_95 * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt() / denominator;
    Some((centre - half, centre + half))
}

/// Nearest-rank percentile of a sorted slice.
fn percentile(sorted: &[u32], percent: usize) -> u32 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (percent * sorted.len()).div_ceil(100).max(1);
    sorted[rank.min(sorted.len()) - 1]
}

#[allow(clippy::too_many_lines)] // CLI parsing and report are linear.
fn main() {
    let mut settings = Settings {
        matches: 50,
        max_ticks: 36_000,
        seed: 0x5EED_1234_ABCD_0001,
        verify: true,
        bot_mask: 0x03FF,
        spawn_waves: true,
        pick: None,
        vary_picks: true,
        content: None,
    };
    let mut threads = 1u32;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--matches" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    settings.matches = value;
                }
            }
            "--content" => {
                let Some(path) = args.next() else {
                    eprintln!("--content needs a TOML path");
                    std::process::exit(2);
                };
                match moba_game::load_content_file(std::path::Path::new(&path)) {
                    Ok(loaded) => settings.content = Some(Arc::new(loaded)),
                    Err(error) => {
                        eprintln!("cannot load content: {error}");
                        std::process::exit(2);
                    }
                }
            }
            "--ticks" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    settings.max_ticks = value;
                }
            }
            "--seed" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    settings.seed = value;
                }
            }
            "--bot-mask" => {
                if let Some(value) = args.next() {
                    let text = value.trim_start_matches("0x");
                    settings.bot_mask = u16::from_str_radix(text, 16).unwrap_or(settings.bot_mask);
                }
            }
            "--threads" => {
                if let Some(value) = args.next().and_then(|value| value.parse::<u32>().ok()) {
                    threads = value.max(1);
                }
            }
            "--no-verify" => settings.verify = false,
            "--no-waves" => settings.spawn_waves = false,
            "--pick" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    settings.pick = Some(value);
                }
            }
            "--fixed-picks" => settings.vary_picks = false,
            "--help" => {
                eprintln!(
                    "usage: match-soak [--matches N] [--ticks N] [--seed N] [--bot-mask HEX] [--pick N] [--fixed-picks] [--no-waves] [--no-verify] [--threads N]"
                );
                return;
            }
            other => eprintln!("unknown argument: {other}"),
        }
    }
    let matches = settings.matches;
    let threads = threads.min(matches.max(1));
    eprintln!(
        "match-soak: {matches} matches, max {} ticks, bot_mask={:#x}, verify={}, seed={:#x}, threads={threads}",
        settings.max_ticks, settings.bot_mask, settings.verify, settings.seed
    );
    let start = Instant::now();
    let next = AtomicU32::new(0);
    let done = AtomicU32::new(0);
    let progress_every = (matches / 50).max(10);
    let mut results: Vec<(u32, MatchResult, bool)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        if index >= matches {
                            break;
                        }
                        let (result, mismatch) = settings.play(index);
                        local.push((index, result, mismatch));
                        let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if finished.is_multiple_of(progress_every) {
                            eprintln!(
                                "  {finished}/{matches} matches, {:.0} s",
                                start.elapsed().as_secs_f64()
                            );
                        }
                    }
                    local
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("match-soak worker panicked"))
            .collect()
    });
    results.sort_unstable_by_key(|(index, _, _)| *index);

    // Unresolved at the tick cap, blue, red, same-tick draw.
    let mut outcomes = [0u32; 4];
    let mut peak_units = 0usize;
    let mut hashes = Vec::with_capacity(results.len());
    let mut resolve_ticks = Vec::with_capacity(results.len());
    let mut total_ticks = 0u64;
    let mut total_tick_nanos = 0u64;
    let mut max_tick_nanos = 0u64;
    let mut max_tick_at = 0u32;
    let mut mismatches = 0u32;
    for (_, result, mismatch) in &results {
        mismatches += u32::from(*mismatch);
        peak_units = peak_units.max(result.peak_units);
        let outcome_index = match result.outcome {
            Some(MatchOutcome::Winner(Team::Blue)) => 1,
            Some(MatchOutcome::Winner(Team::Red)) => 2,
            Some(MatchOutcome::Draw) => 3,
            // Unresolved. `MatchOutcome` is `#[non_exhaustive]`; an unknown outcome counts here.
            _ => 0,
        };
        outcomes[outcome_index] += 1;
        if result.outcome.is_some() {
            resolve_ticks.push(result.ticks);
        }
        hashes.push(result.hash);
        total_ticks += u64::from(result.ticks);
        total_tick_nanos = total_tick_nanos.saturating_add(result.tick_nanos);
        if result.max_tick_nanos > max_tick_nanos {
            max_tick_nanos = result.max_tick_nanos;
            max_tick_at = result.max_tick_at;
        }
    }
    hashes.sort_unstable();
    hashes.dedup();
    resolve_ticks.sort_unstable();
    println!(
        "matches={} ticks={} avg_tick_us={} max_tick_us={} max_tick_at={} wall_s={:.2} unique_hashes={} outcome_none={} blue={} red={} draw={} peak_units={} determinism={}",
        matches,
        total_ticks,
        total_tick_nanos
            .checked_div(total_ticks.max(1))
            .unwrap_or(0)
            / 1_000,
        max_tick_nanos / 1_000,
        max_tick_at,
        start.elapsed().as_secs_f64(),
        hashes.len(),
        outcomes[0],
        outcomes[1],
        outcomes[2],
        outcomes[3],
        peak_units,
        if mismatches == 0 { "ok" } else { "FAILED" }
    );
    let decided = outcomes[1] + outcomes[2];
    let (rate, interval) = wilson_95(outcomes[1], decided).map_or_else(
        || ("n/a".to_owned(), "n/a".to_owned()),
        |(low, high)| {
            (
                format!(
                    "{:.2}%",
                    100.0 * f64::from(outcomes[1]) / f64::from(decided)
                ),
                format!("{:.2}%..{:.2}%", 100.0 * low, 100.0 * high),
            )
        },
    );
    println!(
        "balance: decided={decided} draws={} cap_draws={} same_tick_draws={} blue_win_rate={rate} wilson95={interval} resolve_tick_p10={} median={} p90={} max={}",
        outcomes[0] + outcomes[3],
        outcomes[0],
        outcomes[3],
        percentile(&resolve_ticks, 10),
        percentile(&resolve_ticks, 50),
        percentile(&resolve_ticks, 90),
        resolve_ticks.last().copied().unwrap_or(0),
    );
    if mismatches > 0 {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{percentile, wilson_95};

    #[test]
    fn report_statistics_match_reference_values() {
        // 157 / 300 -> 46.7 %..57.9 % (the 2026-10-18 table in docs/perf/0008-balance.md).
        let (low, high) = wilson_95(157, 300).expect("trials");
        assert!((low - 0.4669).abs() < 1e-4, "{low}");
        assert!((high - 0.5792).abs() < 1e-4, "{high}");
        let (low, high) = wilson_95(0, 10).expect("trials");
        assert!(low.abs() < 1e-12);
        assert!((high - 0.2775).abs() < 1e-4, "{high}");
        assert!(wilson_95(0, 0).is_none());

        let sorted = [10, 20, 30, 40, 50];
        assert_eq!(percentile(&sorted, 10), 10);
        assert_eq!(percentile(&sorted, 50), 30);
        assert_eq!(percentile(&sorted, 90), 50);
        assert_eq!(percentile(&[], 50), 0);
    }
}
