//! Deterministic replay checks for the shipped alternative mode packs.

use std::sync::Arc;

use moba_game::{Team, load_content_toml};
use moba_sim::{SimConfig, Simulation, TickInputs, UnitKind};

fn run(pack: &str, ticks: u32) -> (u64, usize, usize, Option<Team>) {
    let content = Arc::new(load_content_toml(pack).expect("valid pack"));
    let mut simulation = Simulation::new_with_content(SimConfig::default(), Arc::clone(&content));
    for _ in 0..ticks {
        let inputs = simulation.bot_inputs();
        simulation.tick(&inputs);
        simulation.clear_events();
    }
    let heroes = simulation
        .world()
        .entities()
        .filter(|entity| simulation.world().get::<UnitKind>(*entity) == Some(&UnitKind::Hero))
        .count();
    let blue = simulation
        .world()
        .entities()
        .filter(|entity| simulation.world().get::<Team>(*entity) == Some(&Team::Blue))
        .count();
    (simulation.state_hash(), heroes, blue, simulation.outcome())
}

#[test]
fn one_v_one_mid_pack_replays_deterministically() {
    let pack = include_str!("../../../content/1v1-mid.toml");
    let first = run(pack, 3_000);
    let second = run(pack, 3_000);
    assert_eq!(first, second);
    assert_eq!(first.1, 2, "1v1 spawns exactly two heroes");
}

#[test]
fn aram_pack_replays_deterministically() {
    let pack = include_str!("../../../content/aram.toml");
    let first = run(pack, 3_000);
    let second = run(pack, 3_000);
    assert_eq!(first, second);
    assert_eq!(first.1, 10, "ARAM keeps 5v5 heroes");
}

#[test]
fn empty_inputs_do_not_advance_a_team_size_one_match_past_its_heroes() {
    let pack = include_str!("../../../content/1v1-mid.toml");
    let content = Arc::new(load_content_toml(pack).expect("valid pack"));
    let mut simulation = Simulation::new_with_content(SimConfig::default(), content);
    for _ in 0..120 {
        simulation.tick(&TickInputs::new());
    }
    assert_eq!(
        simulation
            .world()
            .entities()
            .filter(|entity| simulation.world().get::<UnitKind>(*entity) == Some(&UnitKind::Hero))
            .count(),
        2
    );
}
