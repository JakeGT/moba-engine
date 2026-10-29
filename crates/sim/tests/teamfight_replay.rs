//! The checked-in R5 teamfight replay (the VFX evidence scene, ADR-0013) must verify with
//! the current simulation, and must keep casting every built-in ability. A deliberate
//! simulation logic change regenerates it with
//! `cargo run --release -p moba-tools --bin teamfight` (and records the new numbers in
//! `docs/perf/0016-vfx.md`).

use std::collections::BTreeMap;
use std::sync::Arc;

use moba_game::GameContent;
use moba_sim::{HeroIdentity, Replay, SimEvent, Simulation};

const TEAMFIGHT: &[u8] = include_bytes!("../../../tests/replays/teamfight-5v5.mbr");

#[test]
fn checked_in_teamfight_replay_verifies() {
    let replay = Replay::decode(TEAMFIGHT).expect("the teamfight replay decodes");
    assert_eq!(replay.frames.len(), 1_800);
    assert_eq!(replay.checkpoints.len(), 6);
    assert_eq!(replay.config.bot_mask, 0, "scripted, not bots");
    let result = replay.play().expect("the teamfight replay verifies");
    assert_eq!(Some(result.state_hash), replay.expected_hash);
    assert_eq!(result.ticks, 1_800);
}

#[test]
fn teamfight_replay_casts_every_built_in_ability() {
    let replay = Replay::decode(TEAMFIGHT).expect("the teamfight replay decodes");
    let content = GameContent::default();
    let mut simulation = Simulation::new_with_content(replay.config, Arc::new(content.clone()));
    let mut casts: BTreeMap<String, u32> = BTreeMap::new();
    let (mut projectiles, mut deaths) = (0, 0);
    for frame in &replay.frames {
        simulation.tick(frame);
        for event in simulation.events() {
            match *event {
                SimEvent::AbilityCast {
                    caster, ability, ..
                } => {
                    let hero = simulation
                        .world()
                        .get::<HeroIdentity>(caster)
                        .expect("casters are heroes")
                        .0
                        .0;
                    let id = content.heroes[usize::from(hero)].abilities[usize::from(ability)];
                    let name = content.abilities[usize::from(id)].name.clone();
                    *casts.entry(name).or_default() += 1;
                }
                SimEvent::ProjectileSpawned { .. } => projectiles += 1,
                SimEvent::UnitDied { .. } => deaths += 1,
                _ => {}
            }
        }
        simulation.clear_events();
    }
    for ability in &content.abilities {
        assert!(
            casts.get(&ability.name).copied().unwrap_or(0) > 0,
            "the teamfight never casts {} ({casts:?})",
            ability.name
        );
    }
    assert_eq!(
        casts.len(),
        content.abilities.len(),
        "all 15 built-in abilities"
    );
    assert!(projectiles > 0 && deaths > 0);
}
