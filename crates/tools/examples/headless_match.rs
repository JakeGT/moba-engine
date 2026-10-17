//! Embed the authoritative simulation in a tool or custom server.
//!
//! Run with:
//! `cargo run -p moba-tools --example headless_match -- --ticks 3000`
#![allow(clippy::print_stdout)]

use moba_sim::{SimConfig, Simulation};

fn main() {
    let mut ticks = 3_000u32;
    let mut seed = SimConfig::default().seed;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--ticks" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    ticks = value;
                }
            }
            "--seed" => {
                if let Some(value) = args.next().and_then(|value| value.parse().ok()) {
                    seed = value;
                }
            }
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }

    let config = SimConfig {
        seed,
        ..SimConfig::default()
    };
    let mut simulation = Simulation::new(config);
    let mut deaths = 0usize;
    for _ in 0..ticks {
        simulation.tick_with_bots();
        deaths += simulation
            .events()
            .iter()
            .filter(|event| matches!(event, moba_sim::SimEvent::UnitDied { .. }))
            .count();
        simulation.clear_events();
    }

    println!("seed        {}", config.seed);
    println!("ticks       {}", simulation.current_tick().get());
    println!("waves       {}", simulation.wave());
    println!("units       {}", simulation.unit_count());
    println!(
        "score       blue {} / red {}",
        simulation.score().0,
        simulation.score().1
    );
    println!("deaths      {deaths}");
    println!("outcome     {:?}", simulation.outcome());
    println!("state hash  {:#018x}", simulation.state_hash());
}

// SimConfig/Simulation are intentionally sufficient to embed the match: no window, renderer,
// network or clock is required, and the same seed + inputs produce the same state hash.
