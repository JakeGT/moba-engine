//! Print a fixed deterministic simulation state hash for cross-platform CI comparison.
#![allow(clippy::print_stdout)]

use moba_core::FixVec2;
use moba_sim::{PlayerCommand, SimConfig, Simulation, TickInputs};

fn main() {
    let config = SimConfig {
        seed: 0x51A7_E5EED,
        bot_mask: 0,
        spawn_waves: false,
        hero_picks: [0, 1, 2, 3, 4, 0, 1, 2, 3, 4],
    };
    let mut simulation = Simulation::new(config);
    for tick in 0..600u32 {
        let mut inputs = TickInputs::new();
        if tick == 0 {
            for slot in 0..10u8 {
                let x = 64i32 + i32::from(slot) * 8;
                inputs.set(
                    slot as usize,
                    PlayerCommand::MoveTo(FixVec2::from_ints(x, 128)),
                );
            }
        }
        if tick == 120 || tick == 360 {
            for slot in 0..10usize {
                inputs.set(
                    slot,
                    PlayerCommand::CastAbility {
                        slot: (tick / 120 % 3) as u8,
                        target: FixVec2::from_ints(128, 128),
                    },
                );
            }
        }
        simulation.tick(&inputs);
        simulation.clear_events();
    }
    println!("{:#018x}", simulation.state_hash());
}
