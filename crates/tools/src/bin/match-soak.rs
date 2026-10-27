//! Run many deterministic bot matches headlessly and verify replayability.
//!
//! Usage: `cargo run --release -p moba-tools --bin match-soak -- [--matches N] [--ticks N]`
//!
//! Fixed mirrors: `--matches 300 --pick 2 --fixed-picks`. The seed now drives per-slot bot
//! RNG streams and the initial scheduling phase, so each seed is an independent match.
#![allow(clippy::print_stdout)]

use std::time::Instant;

use std::sync::Arc;

use moba_game::GameContent;
use moba_sim::{MatchOutcome, SimConfig, Simulation, TickInputs};

struct MatchResult {
    hash: u64,
    outcome: Option<MatchOutcome>,
    ticks: u32,
    tick_nanos: u64,
    max_tick_nanos: u64,
    max_tick_at: u32,
}

fn run_match(
    seed: u64,
    max_ticks: u32,
    bot_mask: u16,
    spawn_waves: bool,
    hero_picks: [u8; moba_sim::MAX_PLAYERS],
    content: Option<&Arc<GameContent>>,
) -> MatchResult {
    let config = SimConfig {
        seed,
        bot_mask,
        spawn_waves,
        hero_picks,
    };
    let mut simulation = content.map_or_else(
        || Simulation::new(config),
        |content| Simulation::new_with_content(config, Arc::clone(content)),
    );
    let mut tick_nanos = 0u64;
    let mut max_tick_nanos = 0u64;
    let mut max_tick_at = 0u32;
    let mut ticks = 0u32;
    for _ in 0..max_ticks {
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
        if simulation.outcome().is_some() {
            break;
        }
    }
    MatchResult {
        hash: simulation.state_hash(),
        outcome: simulation.outcome(),
        ticks,
        tick_nanos,
        max_tick_nanos,
        max_tick_at,
    }
}

#[allow(clippy::too_many_lines)] // CLI report is linear.
fn main() {
    let mut matches = 50u32;
    let mut max_ticks = 36_000u32;
    let mut seed = 0x5EED_1234_ABCD_0001u64;
    let mut verify = true;
    let mut bot_mask = 0x03FFu16;
    let mut spawn_waves = true;
    let mut pick: Option<u8> = None;
    let mut vary_picks = true;
    let mut content: Option<Arc<GameContent>> = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--matches" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    matches = value;
                }
            }
            "--content" => {
                let Some(path) = args.next() else {
                    eprintln!("--content needs a TOML path");
                    std::process::exit(2);
                };
                match moba_game::load_content_file(std::path::Path::new(&path)) {
                    Ok(loaded) => content = Some(Arc::new(loaded)),
                    Err(error) => {
                        eprintln!("cannot load content: {error}");
                        std::process::exit(2);
                    }
                }
            }
            "--ticks" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    max_ticks = value;
                }
            }
            "--seed" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    seed = value;
                }
            }
            "--bot-mask" => {
                if let Some(value) = args.next() {
                    let text = value.trim_start_matches("0x");
                    bot_mask = u16::from_str_radix(text, 16).unwrap_or(bot_mask);
                }
            }
            "--no-verify" => verify = false,
            "--no-waves" => spawn_waves = false,
            "--pick" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    pick = Some(value);
                }
            }
            "--fixed-picks" => vary_picks = false,
            "--help" => {
                eprintln!(
                    "usage: match-soak [--matches N] [--ticks N] [--seed N] [--bot-mask HEX] [--pick N] [--fixed-picks] [--no-waves] [--no-verify]"
                );
                return;
            }
            other => eprintln!("unknown argument: {other}"),
        }
    }
    eprintln!(
        "match-soak: {matches} matches, max {max_ticks} ticks, bot_mask={bot_mask:#x}, verify={verify}, seed={seed:#x}"
    );
    let start = Instant::now();
    let mut outcomes = [0u32; 3];
    let mut hashes = Vec::with_capacity(matches as usize);
    let mut total_ticks = 0u64;
    let mut total_tick_nanos = 0u64;
    let mut max_tick_nanos = 0u64;
    let mut max_tick_at = 0u32;
    let mut mismatches = 0u32;
    for index in 0..matches {
        let match_seed = seed.wrapping_add(u64::from(index));
        let picks = pick.map_or_else(
            || {
                if vary_picks {
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
        );
        let first = run_match(
            match_seed,
            max_ticks,
            bot_mask,
            spawn_waves,
            picks,
            content.as_ref(),
        );
        if verify {
            let second = run_match(
                match_seed,
                max_ticks,
                bot_mask,
                spawn_waves,
                picks,
                content.as_ref(),
            );
            if first.hash != second.hash
                || first.outcome != second.outcome
                || first.ticks != second.ticks
            {
                mismatches += 1;
                eprintln!(
                    "nondeterministic match seed={match_seed:#x}: {:#x}/{} vs {:#x}/{}",
                    first.hash, first.ticks, second.hash, second.ticks
                );
            }
        }
        let outcome_index = match first.outcome {
            // Unresolved and same-tick (drawn) matches both count as no winner.
            None | Some(MatchOutcome::Draw) => 0,
            Some(MatchOutcome::Winner(moba_game::Team::Blue)) => 1,
            Some(MatchOutcome::Winner(moba_game::Team::Red)) => 2,
        };
        outcomes[outcome_index] += 1;
        hashes.push(first.hash);
        total_ticks += u64::from(first.ticks);
        total_tick_nanos = total_tick_nanos.saturating_add(first.tick_nanos);
        if first.max_tick_nanos > max_tick_nanos {
            max_tick_nanos = first.max_tick_nanos;
            max_tick_at = first.max_tick_at;
        }
        if (index + 1) % 10 == 0 {
            eprintln!(
                "  {}/{} matches, {} ticks, worst tick {} us",
                index + 1,
                matches,
                total_ticks,
                max_tick_nanos / 1_000
            );
        }
    }
    hashes.sort_unstable();
    let unique = hashes.windows(2).filter(|pair| pair[0] != pair[1]).count() + 1;
    println!(
        "matches={} ticks={} avg_tick_us={} max_tick_us={} max_tick_at={} wall_s={:.2} unique_hashes={} outcome_none={} blue={} red={} determinism={}",
        matches,
        total_ticks,
        total_tick_nanos
            .checked_div(total_ticks.max(1))
            .unwrap_or(0)
            / 1_000,
        max_tick_nanos / 1_000,
        max_tick_at,
        start.elapsed().as_secs_f64(),
        unique,
        outcomes[0],
        outcomes[1],
        outcomes[2],
        if mismatches == 0 { "ok" } else { "FAILED" }
    );
    if mismatches > 0 {
        std::process::exit(1);
    }
}
