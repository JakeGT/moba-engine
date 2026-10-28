//! Extend the simulation ECS from outside the engine.
//!
//! Custom components are ignored by the built-in gameplay systems but can be queried by your
//! own tools, modes or server logic. Run with:
//! `cargo run -p moba-tools --example custom_component`
#![allow(clippy::print_stdout)]

use moba_sim::{SimConfig, Simulation, TickInputs};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct AssistTracker {
    ticks_alive: u32,
}

fn main() {
    let mut simulation = Simulation::new(
        SimConfig::default()
            .with_bot_mask(0)
            .with_spawn_waves(false),
    );
    let hero = simulation.hero_entity(0).expect("blue hero");
    let _ = simulation
        .world_mut()
        .insert(hero, AssistTracker { ticks_alive: 0 });

    for _ in 0..120 {
        simulation.tick(&TickInputs::default());
        if let Some(tracker) = simulation.world_mut().get_mut::<AssistTracker>(hero) {
            tracker.ticks_alive += 1;
        }
    }

    let tracker = *simulation
        .world()
        .get::<AssistTracker>(hero)
        .expect("custom component survived ticks");
    println!("hero {hero:?} tracked for {} ticks", tracker.ticks_alive);
    println!("gameplay hash {:#018x}", simulation.state_hash());
}
