//! Drive the simulation with explicit intents instead of bots.
//!
//! Run with: `cargo run -p moba-tools --example scripted_orders`
#![allow(clippy::print_stdout)]

use moba_core::FixVec2;
use moba_sim::{PlayerCommand, SimConfig, Simulation, TickInputs};

fn main() {
    let mut simulation = Simulation::new(SimConfig {
        bot_mask: 0,
        spawn_waves: false,
        ..SimConfig::default()
    });
    let start = simulation.hero_position(0).expect("blue hero");
    let target = FixVec2::from_ints(100, 128);

    for tick in 0..600u32 {
        let mut inputs = TickInputs::default();
        inputs.set(0, PlayerCommand::MoveTo(target));
        simulation.tick(&inputs);
        if tick % 120 == 0
            && let Some(position) = simulation.hero_position(0)
        {
            println!(
                "tick {tick:>3}  hero 0 at ({:.2}, {:.2})",
                position.x.to_f32(),
                position.y.to_f32()
            );
        }
    }

    let end = simulation.hero_position(0).expect("blue hero");
    println!(
        "start ({:.2}, {:.2}) -> end ({:.2}, {:.2})",
        start.x.to_f32(),
        start.y.to_f32(),
        end.x.to_f32(),
        end.y.to_f32()
    );
    println!("state hash {:#018x}", simulation.state_hash());
}
