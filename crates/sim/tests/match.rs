//! End-to-end simulation tests: waves, combat, resets and determinism.

use moba_sim::{SimConfig, Simulation, TickInputs, UnitKind};

fn idle_sim() -> Simulation {
    Simulation::new(
        SimConfig::default()
            .with_bot_mask(0)
            .with_spawn_waves(false),
    )
}

#[test]
fn a_match_starts_with_ten_heroes_and_sixteen_structures() {
    let sim = idle_sim();
    assert_eq!(sim.unit_count(), 26);
    assert_eq!(sim.current_tick().get(), 0);
    assert!(sim.outcome().is_none());
    for slot in 0..10 {
        let health = sim.hero_health(slot).expect("hero exists");
        assert!(health.current > 0);
        assert_eq!(health.current, health.max);
        let position = sim.hero_position(slot).expect("hero position");
        if slot < 5 {
            assert!(position.x < moba_core::Fix::from_i32(80));
        } else {
            assert!(position.x > moba_core::Fix::from_i32(176));
        }
    }
}

#[test]
fn minion_waves_spawn_on_schedule_and_follow_lanes() {
    let mut sim = Simulation::new(SimConfig::default());
    let start_units = sim.unit_count();
    for _ in 0..31 {
        sim.tick(&TickInputs::new());
    }
    assert_eq!(sim.wave(), 1);
    assert_eq!(
        sim.unit_count(),
        start_units + 24,
        "3 lanes × 2 teams × 4 minions"
    );

    let mut minions = 0;
    let mut lanes = [0u32; 3];
    for entity in sim.world().entities() {
        let Some(kind) = sim.world().get::<UnitKind>(entity) else {
            continue;
        };
        let Some(walker) = sim.world().get::<moba_sim::LaneWalker>(entity) else {
            continue;
        };
        if kind.is_minion() {
            minions += 1;
            lanes[usize::from(walker.lane)] += 1;
        }
    }
    assert_eq!(minions, 24);
    assert_eq!(lanes, [8, 8, 8]);

    // Minions move away from their spawn point; the lane walker advances eventually.
    let before = sim.state_hash();
    for _ in 0..120 {
        sim.tick(&TickInputs::new());
    }
    assert_ne!(before, sim.state_hash());
}

#[test]
fn idle_ticks_are_deterministic_for_ten_thousand_ticks() {
    let mut a = idle_sim();
    let mut b = idle_sim();
    for _ in 0..10_000 {
        a.tick(&TickInputs::new());
        b.tick(&TickInputs::new());
        assert_eq!(a.state_hash(), b.state_hash());
    }
    assert_eq!(a.current_tick(), b.current_tick());
}

#[test]
fn full_bot_match_is_deterministic() {
    let mut a = Simulation::new(SimConfig::default());
    let mut b = Simulation::new(SimConfig::default());
    for tick in 0..10_000 {
        a.tick_with_bots();
        b.tick_with_bots();
        if tick % 100 == 0 || tick + 1 == 10_000 {
            assert_eq!(
                a.state_hash(),
                b.state_hash(),
                "desync at tick {} (event queues differ but state must match)",
                a.current_tick().get()
            );
        }
    }
    assert_eq!(a.outcome(), b.outcome());
}

#[test]
fn bots_fight_and_produce_events() {
    let mut sim = Simulation::new(SimConfig::default());
    let mut saw_damage = false;
    let mut saw_death = false;
    let mut saw_wave = false;
    for _ in 0..1_800 {
        sim.tick_with_bots();
        for event in sim.events() {
            match event {
                moba_sim::SimEvent::WaveSpawned { .. } => saw_wave = true,
                moba_sim::SimEvent::UnitDied { .. } => saw_death = true,
                _ => {}
            }
        }
        if sim
            .hero_health(0)
            .is_some_and(|health| health.current < health.max)
        {
            saw_damage = true;
        }
        sim.clear_events();
    }
    assert!(saw_wave, "waves should spawn");
    assert!(
        saw_damage || saw_death,
        "bots should reach and damage each other"
    );
}

#[test]
fn heroes_respawn_after_death() {
    let mut sim = Simulation::new(SimConfig::default());
    let mut dead = None;
    for _ in 0..6_000 {
        sim.tick_with_bots();
        for event in sim.events() {
            if let moba_sim::SimEvent::UnitDied {
                entity,
                kind: UnitKind::Hero,
                ..
            } = *event
            {
                dead = Some(entity);
            }
        }
        sim.clear_events();
        if dead.is_some() {
            break;
        }
    }
    let entity = dead.expect("a hero should die within six thousand ticks");
    for _ in 0..(moba_sim::HERO_RESPAWN_TICKS + 10) {
        sim.tick_with_bots();
    }
    let health = sim
        .world()
        .get::<moba_sim::Health>(entity)
        .copied()
        .expect("dead hero entity still exists");
    assert!(health.current > 0, "hero should have respawned");
    assert!(
        sim.world().get::<moba_sim::Dead>(entity).is_none(),
        "respawn must clear the Dead marker"
    );
}

#[test]
fn match_ends_only_when_a_nexus_falls() {
    let mut sim = Simulation::new(SimConfig::default());
    let mut ended = false;
    for _ in 0..20_000 {
        sim.tick_with_bots();
        if sim.outcome().is_some() {
            ended = true;
            break;
        }
    }
    let nexuses = sim
        .world()
        .entities()
        .filter(|entity| {
            sim.world()
                .get::<UnitKind>(*entity)
                .is_some_and(|kind| *kind == UnitKind::Nexus)
        })
        .count();
    if ended {
        assert_eq!(
            nexuses, 1,
            "the match must end with exactly one nexus alive"
        );
        assert!(
            sim.events()
                .iter()
                .any(|event| matches!(event, moba_sim::SimEvent::MatchEnded { .. }))
        );
    } else {
        assert_eq!(nexuses, 2, "without a winner both nexuses must stand");
        assert_eq!(sim.current_tick().get(), 20_000);
    }
}
