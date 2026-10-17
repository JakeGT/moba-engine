//! Run many deterministic bot matches headlessly and verify replayability.
//!
//! Usage: `cargo run --release -p moba-tools --bin match-soak -- [--matches N] [--ticks N]`
#![allow(clippy::print_stdout)]

use std::time::Instant;

use moba_sim::{SimConfig, Simulation, TickInputs};

struct MatchResult {
    hash: u64,
    outcome: Option<moba_game::Team>,
    ticks: u32,
    tick_nanos: u64,
    max_tick_nanos: u64,
}

fn run_match(
    seed: u64,
    max_ticks: u32,
    bot_mask: u16,
    spawn_waves: bool,
    pick: Option<u8>,
) -> MatchResult {
    let config = SimConfig {
        seed,
        bot_mask,
        spawn_waves,
        hero_picks: pick.map_or_else(
            || SimConfig::default().hero_picks,
            |pick| [pick; moba_sim::MAX_PLAYERS],
        ),
    };
    let mut simulation = Simulation::new(config);
    let mut tick_nanos = 0u64;
    let mut max_tick_nanos = 0u64;
    let mut ticks = 0u32;
    for _ in 0..max_ticks {
        let inputs: TickInputs = simulation.bot_inputs();
        let tick_start = Instant::now();
        simulation.tick(&inputs);
        simulation.clear_events();
        let elapsed = u64::try_from(tick_start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        tick_nanos = tick_nanos.saturating_add(elapsed);
        max_tick_nanos = max_tick_nanos.max(elapsed);
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
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--matches" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    matches = value;
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
            "--help" => {
                eprintln!(
                    "usage: match-soak [--matches N] [--ticks N] [--seed N] [--bot-mask HEX] [--pick N] [--no-waves] [--no-verify]"
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
    let mut mismatches = 0u32;
    for index in 0..matches {
        let match_seed = seed.wrapping_add(u64::from(index));
        let first = run_match(match_seed, max_ticks, bot_mask, spawn_waves, pick);
        if verify {
            let second = run_match(match_seed, max_ticks, bot_mask, spawn_waves, pick);
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
            None => 0,
            Some(moba_game::Team::Blue) => 1,
            Some(moba_game::Team::Red) => 2,
        };
        outcomes[outcome_index] += 1;
        hashes.push(first.hash);
        total_ticks += u64::from(first.ticks);
        total_tick_nanos = total_tick_nanos.saturating_add(first.tick_nanos);
        max_tick_nanos = max_tick_nanos.max(first.max_tick_nanos);
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
        "matches={} ticks={} avg_tick_us={} max_tick_us={} wall_s={:.2} unique_hashes={} outcome_none={} blue={} red={} determinism={}",
        matches,
        total_ticks,
        total_tick_nanos
            .checked_div(total_ticks.max(1))
            .unwrap_or(0)
            / 1_000,
        max_tick_nanos / 1_000,
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
