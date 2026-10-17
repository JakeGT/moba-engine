#![allow(clippy::print_stdout)] // Example output.

//! Load a TOML content pack and run a short bot match with it.
//!
//! ```bash
//! cargo run -p moba-engine --example custom_content
//! ```

use std::sync::Arc;

use moba_engine::game::load_content_file;
use moba_engine::prelude::{SimConfig, Simulation, TickInputs};

fn main() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/example-pack.toml");
    let content = load_content_file(&path).expect("valid content pack");
    println!(
        "pack={} {} items={} heroes={} hash={:#018x}",
        content.manifest.name,
        content.manifest.version,
        content.items.len(),
        content.heroes.len(),
        content.hash()
    );
    let mut simulation = Simulation::new_with_content(SimConfig::default(), Arc::new(content));
    for _ in 0..300 {
        simulation.tick(&TickInputs::new());
        simulation.clear_events();
    }
    println!(
        "tick={} outcome={:?} state={:#018x}",
        simulation.current_tick().get(),
        simulation.outcome(),
        simulation.state_hash()
    );
}
