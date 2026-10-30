//! Regenerate the checked-in v9 replay fixture.
#![allow(clippy::print_stdout)]

use moba_sim::{Replay, SimConfig, Simulation};

fn main() {
    let config = SimConfig::default();
    let mut simulation = Simulation::new(config);
    let mut replay = Replay::new(config);
    for _ in 0..300 {
        let inputs = simulation.bot_inputs();
        replay.push(inputs);
        simulation.tick(&inputs);
    }
    replay.expected_hash = Some(simulation.state_hash());
    replay
        .checkpoints
        .push((simulation.current_tick().get(), simulation.state_hash()));
    let bytes = replay.encode();
    std::fs::write("crates/sim/tests/fixtures/seed-v9.mbr", bytes).expect("write fixture");
    println!("wrote fixture: {:#018x}", simulation.state_hash());
}
