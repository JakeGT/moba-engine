#![allow(clippy::print_stdout)] // Example output.

//! Run a deterministic bot match without a window or network.
//!
//! ```bash
//! cargo run -p moba-engine --example headless_match
//! ```

use moba_engine::prelude::{FixVec2, PlayerCommand, SimConfig, Simulation, TickInputs};

fn main() {
    let mut simulation = Simulation::new(SimConfig {
        seed: 7,
        bot_mask: 0x03FF,
        ..SimConfig::default()
    });
    // Slot 0 is human-driven; every other slot is a bot.
    let mut inputs = TickInputs::new();
    inputs.set(0, PlayerCommand::MoveTo(FixVec2::from_ints(80, 128)));
    for _ in 0..900 {
        simulation.tick(&inputs);
        simulation.clear_events();
    }
    println!(
        "tick={} outcome={:?} state={:#018x}",
        simulation.current_tick().get(),
        simulation.outcome(),
        simulation.state_hash()
    );
    for slot in 0..10 {
        if let Some(position) = simulation.hero_position(slot) {
            println!(
                "slot {slot}: x={} y={}",
                position.x.to_i32(),
                position.y.to_i32()
            );
        }
    }
}
