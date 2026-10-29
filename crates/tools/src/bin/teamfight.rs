//! Records the scripted 5v5 teamfight replay: the R5 VFX evidence scene (ADR-0013).
//!
//! ```text
//! teamfight [--out FILE] [--ticks N]     # default tests/replays/teamfight-5v5.mbr, 1800
//! ```
//!
//! Ten scripted heroes (no bots, no minion waves, default content) march from their bases to
//! a rally point north of mid lane, out of tower range, and fight: every hero casts its
//! Q/W/E in rotation at the nearest enemy hero whenever the cooldown allows and basic-attacks
//! in between, respawns and walks back. The script reads only simulation state, so the
//! recording is deterministic; the tool replays it before writing and prints what the fight
//! contains (casts per ability, projectiles, zones, deaths, attacks). `moba-tools replay`
//! verifies the file, and `moba-client` plays it with `MOBA_REPLAY`.
#![allow(clippy::print_stdout)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use moba_core::{Fix, FixVec2};
use moba_sim::{PlayerCommand, Replay, SimConfig, SimEvent, Simulation, TickInputs};

/// Ticks between two cast attempts of one hero.
const CAST_EVERY: u32 = 9;
/// Ticks between re-issued move orders while marching.
const MOVE_EVERY: u32 = 45;
/// An enemy hero this close (world units) starts the fight.
const ENGAGE: i32 = 16;
/// Casts aim at most this far (world units): inside the shortest directional range (5).
const AIM: i32 = 4;
/// State-hash checkpoints every this many ticks.
const CHECKPOINT_EVERY: u32 = 300;

fn rally(team_blue: bool, slot: usize) -> FixVec2 {
    // Five abreast on each side of the rally point (128, 152), 10 units apart.
    let lateral = i32::try_from(slot % 5).unwrap_or(0) * 3 - 6;
    let x = if team_blue { 125 } else { 131 };
    FixVec2::from_ints(x, 152 + lateral)
}

/// The scripted inputs for one tick.
fn script(simulation: &Simulation) -> TickInputs {
    let tick = simulation.current_tick().get();
    let mut inputs = TickInputs::new();
    let alive = |slot: usize| {
        simulation
            .hero_health(slot)
            .is_some_and(|health| health.current > 0)
    };
    for slot in 0..moba_sim::MAX_PLAYERS {
        if !alive(slot) {
            continue;
        }
        let Some(position) = simulation.hero_position(slot) else {
            continue;
        };
        let blue = slot < 5;
        let enemies = if blue { 5..10 } else { 0..5 };
        let nearest = enemies
            .filter(|&enemy| alive(enemy))
            .filter_map(|enemy| {
                simulation
                    .hero_position(enemy)
                    .map(|at| (enemy, at, position.distance(at)))
            })
            .min_by_key(|&(enemy, _, distance)| (distance, enemy));
        let phase = u32::try_from(slot).unwrap_or(0);
        match nearest {
            Some((enemy, at, distance)) if distance <= Fix::from_i32(ENGAGE) => {
                let range = simulation
                    .hero_entity(slot)
                    .and_then(|entity| simulation.world().get::<moba_sim::AttackStats>(entity))
                    .map_or(Fix::from_i32(2), |stats| stats.range);
                if (tick + phase).is_multiple_of(CAST_EVERY) {
                    // Q/W/E in rotation, aimed at the nearest enemy hero. The point is at
                    // most `AIM` away, inside every ability's range: skillshots fly their
                    // full range along the direction anyway, and dashes stay short.
                    let ability = ((tick + phase) / CAST_EVERY) % 3;
                    let aim = Fix::from_i32(AIM).min(distance);
                    let target = position + (at - position).normalize_or_zero() * aim;
                    inputs.set(
                        slot,
                        PlayerCommand::CastAbility {
                            slot: u8::try_from(ability).unwrap_or(0),
                            target,
                        },
                    );
                } else if distance <= range {
                    if let Some(entity) = simulation.hero_entity(enemy) {
                        inputs.set(slot, PlayerCommand::Attack(entity));
                    }
                } else {
                    inputs.set(slot, PlayerCommand::MoveTo(at));
                }
            }
            _ => {
                if (tick + phase).is_multiple_of(MOVE_EVERY) {
                    inputs.set(slot, PlayerCommand::MoveTo(rally(blue, slot)));
                }
            }
        }
    }
    inputs
}

#[allow(clippy::too_many_lines)] // CLI, recording, verification and report.
fn main() {
    let mut out = PathBuf::from("tests/replays/teamfight-5v5.mbr");
    let mut ticks: u32 = 1_800;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--out" => {
                let Some(path) = args.next() else {
                    eprintln!("--out needs a path");
                    std::process::exit(2);
                };
                out = PathBuf::from(path);
            }
            "--ticks" => {
                let Some(value) = args.next().and_then(|value| value.parse().ok()) else {
                    eprintln!("--ticks needs a number");
                    std::process::exit(2);
                };
                ticks = value;
            }
            "--help" => {
                eprintln!("usage: teamfight [--out FILE] [--ticks N]");
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let config = SimConfig::default()
        .with_seed(0x7EA3_F16E_0005_0005)
        .with_bot_mask(0)
        .with_spawn_waves(false);
    let mut simulation = Simulation::new(config);
    let mut replay = Replay::new(config);
    let mut casts: BTreeMap<String, u32> = BTreeMap::new();
    let mut counts: BTreeMap<&'static str, u32> = BTreeMap::new();
    let mut first_fight: Option<u32> = None;
    for _ in 0..ticks {
        let inputs = script(&simulation);
        simulation.tick(&inputs);
        replay.push(inputs);
        for event in simulation.events() {
            let kind = match event {
                SimEvent::AbilityCast {
                    caster, ability, ..
                } => {
                    first_fight.get_or_insert(simulation.current_tick().get());
                    let content = simulation.content();
                    let name = simulation
                        .world()
                        .get::<moba_sim::HeroIdentity>(*caster)
                        .and_then(|hero| content.heroes.get(usize::from(hero.0.0)))
                        .and_then(|hero| hero.abilities.get(usize::from(*ability)))
                        .and_then(|&id| content.abilities.get(usize::from(id)))
                        .map_or_else(|| String::from("?"), |spec| spec.name.clone());
                    *casts.entry(name).or_default() += 1;
                    "casts"
                }
                SimEvent::ProjectileSpawned { .. } => "projectiles",
                SimEvent::ProjectileHit { .. } => "projectile hits",
                SimEvent::ZoneSpawned { .. } => "zones",
                SimEvent::UnitDied { .. } => "deaths",
                SimEvent::BasicAttack { .. } => "basic attacks",
                SimEvent::HeroRespawned { .. } => "respawns",
                _ => "other",
            };
            *counts.entry(kind).or_default() += 1;
        }
        simulation.clear_events();
        let completed = simulation.current_tick().get();
        if completed.is_multiple_of(CHECKPOINT_EVERY) {
            replay
                .checkpoints
                .push((completed, simulation.state_hash()));
        }
    }
    replay.expected_hash = Some(simulation.state_hash());
    let bytes = replay.encode();
    let decoded = Replay::decode(&bytes).unwrap_or_else(|error| {
        eprintln!("the recording does not decode: {error}");
        std::process::exit(1);
    });
    let result = decoded.play().unwrap_or_else(|error| {
        eprintln!("the recording does not verify: {error}");
        std::process::exit(1);
    });
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).unwrap_or_else(|error| {
            eprintln!("cannot create {}: {error}", parent.display());
            std::process::exit(1);
        });
    }
    std::fs::write(&out, &bytes).unwrap_or_else(|error| {
        eprintln!("cannot write {}: {error}", out.display());
        std::process::exit(1);
    });
    println!("file        {}", out.display());
    println!("bytes       {}", bytes.len());
    println!("ticks       {}", result.ticks);
    println!("state hash  {:#018x}", result.state_hash);
    println!("checkpoints {}", replay.checkpoints.len());
    println!(
        "first cast  tick {}",
        first_fight.map_or_else(|| String::from("none"), |tick| tick.to_string())
    );
    for (kind, count) in &counts {
        println!("{kind:<15} {count}");
    }
    for (name, count) in &casts {
        println!("  cast {name:<16} {count}");
    }
}
