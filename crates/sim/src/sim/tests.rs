//! Unit tests for the simulation systems.

use std::any::TypeId;

use moba_game::{AreaEffect, EMPTY_ITEM, Effect, EffectList, ModifierKind, Targeting};

use super::*;
use crate::components::{
    AttackDamageType, AttackStats, AttackTarget, BaseMoveSpeed, HeroIdentity, HeroSlot,
    ItemCooldowns, LaneWalker, LastHitBy, Mana, Modifier, Modifiers, MoveSpeed, MoveTarget, Regen,
    Resistances, VisionRange,
};

fn idle(picks: [u8; MAX_PLAYERS]) -> Simulation {
    Simulation::new(
        SimConfig::default()
            .with_bot_mask(0)
            .with_spawn_waves(false)
            .with_hero_picks(picks),
    )
}

fn area_status(
    modifier: u16,
    radius: Fix,
    duration_ticks: u32,
    factor: Fix,
    amount: i32,
    damage_type: DamageType,
) -> EffectList {
    EffectList::one(Effect::Area {
        radius,
        effects: AreaEffectList::one(AreaEffect::ApplyModifier {
            modifier,
            duration_ticks,
            factor,
            amount,
            damage_type,
        }),
    })
}

fn self_modifier(
    modifier: u16,
    duration_ticks: u32,
    factor: Fix,
    amount: i32,
    damage_type: DamageType,
) -> EffectList {
    EffectList::one(Effect::ApplyModifier {
        modifier,
        duration_ticks,
        factor,
        amount,
        damage_type,
    })
}

fn has_modifier(sim: &Simulation, entity: Entity, kind: ModifierKind) -> bool {
    sim.world
        .get::<Modifiers>(entity)
        .is_some_and(|modifiers| modifiers.has(kind))
}

fn shield_total(sim: &Simulation, entity: Entity) -> i32 {
    sim.world
        .get::<Modifiers>(entity)
        .map_or(0, Modifiers::shield_total)
}

fn find_unit(sim: &Simulation, kind: UnitKind, team: Team) -> Entity {
    sim.world
        .entities()
        .find(|entity| {
            sim.world.get::<UnitKind>(*entity) == Some(&kind)
                && sim.world.get::<Team>(*entity) == Some(&team)
        })
        .expect("unit exists")
}

#[test]
fn destroying_a_nexus_ends_the_match() {
    let mut sim = idle(SimConfig::default().hero_picks);
    let nexus = find_unit(&sim, UnitKind::Nexus, Team::Red);
    sim.world.get_mut::<Health>(nexus).expect("health").current = 0;
    sim.tick(&TickInputs::new());
    assert_eq!(sim.outcome(), Some(MatchOutcome::Winner(Team::Blue)));
    assert_eq!(match_ended_events(&sim), [Some(Team::Blue)]);
    assert!(!sim.world.entities().any(|entity| entity == nexus));
}

fn match_ended_events(sim: &Simulation) -> Vec<Option<Team>> {
    sim.events()
        .iter()
        .filter_map(|event| match event {
            SimEvent::MatchEnded { winner } => Some(*winner),
            _ => None,
        })
        .collect()
}

fn score_limit_one() -> Simulation {
    let mut content = GameContent::default();
    content.rules.score_limit = 1;
    Simulation::new_with_content(
        SimConfig::default()
            .with_bot_mask(0)
            .with_spawn_waves(false),
        Arc::new(content),
    )
}

fn kill_with_credit(sim: &mut Simulation, victim: Entity, killer: Entity, killer_team: Team) {
    sim.world.get_mut::<Health>(victim).expect("health").current = 0;
    let _ = sim.world.insert(
        victim,
        LastHitBy {
            team: killer_team,
            entity: killer,
        },
    );
}

#[test]
fn both_nexuses_falling_in_one_tick_is_a_draw() {
    // Before logic v8 the nexus processed first decided the match (#290).
    let mut sim = idle(SimConfig::default().hero_picks);
    for team in [Team::Blue, Team::Red] {
        let nexus = find_unit(&sim, UnitKind::Nexus, team);
        sim.world.get_mut::<Health>(nexus).expect("health").current = 0;
    }
    sim.tick(&TickInputs::new());
    assert_eq!(sim.outcome(), Some(MatchOutcome::Draw));
    assert_eq!(match_ended_events(&sim), [None]);
    let hash = sim.state_hash();
    sim.tick(&TickInputs::new());
    assert_eq!(sim.state_hash(), hash, "a drawn match stops ticking");
}

#[test]
fn both_teams_reaching_the_score_limit_in_one_tick_is_a_draw() {
    let mut sim = score_limit_one();
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    kill_with_credit(&mut sim, blue, red, Team::Red);
    kill_with_credit(&mut sim, red, blue, Team::Blue);
    sim.tick(&TickInputs::new());
    assert_eq!(sim.score(), (1, 1));
    assert_eq!(sim.outcome(), Some(MatchOutcome::Draw));
    assert_eq!(match_ended_events(&sim), [None]);
}

#[test]
fn one_team_reaching_the_score_limit_wins() {
    let mut sim = score_limit_one();
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    kill_with_credit(&mut sim, red, blue, Team::Blue);
    sim.tick(&TickInputs::new());
    assert_eq!(sim.outcome(), Some(MatchOutcome::Winner(Team::Blue)));
    assert_eq!(match_ended_events(&sim), [Some(Team::Blue)]);
}

#[test]
fn both_teams_surrendering_in_one_tick_is_a_draw() {
    let mut sim = idle(SimConfig::default().hero_picks);
    let mut inputs = TickInputs::new();
    inputs.set(0, PlayerCommand::Surrender);
    inputs.set(5, PlayerCommand::Surrender);
    sim.tick(&inputs);
    assert_eq!(sim.outcome(), Some(MatchOutcome::Draw));
}

#[test]
fn wave_formations_are_x_mirrored_between_teams() {
    // Waves must mirror like hero spawns (x -> extent - x). Before logic v8 red's formation
    // was the point reflection of blue's, so in the top lane blue's ranged minion spawned
    // ahead of the lane start and red's behind it (#290).
    let mut sim = idle(SimConfig::default().hero_picks);
    sim.spawn_wave();
    let extent = sim.content.map.extent;
    let mut units: [Vec<(u8, u8, FixVec2)>; 2] = [Vec::new(), Vec::new()];
    for (_, (kind, team, walker, position)) in sim
        .world
        .query::<(&UnitKind, &Team, &LaneWalker, &Position)>()
    {
        units[team.index()].push((walker.lane, *kind as u8, position.0));
    }
    let [mut blue, mut red] = units;
    // Sort by lane, kind and the side-independent y coordinate.
    let key = |unit: &(u8, u8, FixVec2)| (unit.0, unit.1, unit.2.y.to_bits());
    blue.sort_by_key(key);
    red.sort_by_key(key);
    assert_eq!(blue.len(), 12);
    assert_eq!(red.len(), 12);
    // `normalize_or_zero` rounds toward negative infinity, so mirrored lane directions can
    // differ by a few ULP; allow a hundredth of a world unit.
    let tolerance = Fix::from_i32(1) / Fix::from_i32(100);
    for (blue, red) in blue.iter().zip(&red) {
        let mirrored = FixVec2::new(extent - red.2.x, red.2.y);
        assert_eq!((blue.0, blue.1), (red.0, red.1));
        assert!(
            (blue.2.x - mirrored.x).abs() <= tolerance
                && (blue.2.y - mirrored.y).abs() <= tolerance,
            "lane {} kind {}: blue {:?} vs mirrored red {:?}",
            blue.0,
            blue.1,
            blue.2,
            mirrored
        );
    }
}

#[test]
fn nexus_win_rule_can_be_disabled() {
    let mut content = GameContent::default();
    content.rules.nexus_win = false;
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let nexus = find_unit(&sim, UnitKind::Nexus, Team::Red);
    sim.world.get_mut::<Health>(nexus).expect("health").current = 0;
    sim.tick(&TickInputs::new());
    assert!(sim.outcome().is_none());
}

#[test]
fn towers_can_be_destroyed() {
    let mut sim = idle(SimConfig::default().hero_picks);
    let tower = find_unit(&sim, UnitKind::Tower, Team::Blue);
    sim.world.get_mut::<Health>(tower).expect("health").current = 0;
    sim.tick(&TickInputs::new());
    assert!(!sim.world.entities().any(|entity| entity == tower));
    assert!(sim.events().iter().any(|event| matches!(
        event,
        SimEvent::StructureDestroyed {
            team: Team::Blue,
            kind: UnitKind::Tower,
            ..
        }
    )));
    assert!(sim.outcome().is_none());
}

#[test]
fn basic_attacks_apply_mitigated_damage() {
    let mut sim = idle(SimConfig::default().hero_picks);
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    sim.world.get_mut::<Position>(blue).unwrap().0 = FixVec2::from_ints(100, 100);
    sim.world.get_mut::<Position>(red).unwrap().0 = FixVec2::from_ints(101, 100);
    let start = sim.world.get::<Health>(red).unwrap().current;
    let mut inputs = TickInputs::new();
    inputs.set(0, PlayerCommand::Attack(red));
    sim.tick(&inputs);
    let after = sim.world.get::<Health>(red).unwrap().current;
    assert!(
        after < start,
        "attack should deal damage ({start} -> {after})"
    );
}

#[test]
fn skillshots_travel_and_hit() {
    let picks = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1];
    let mut sim = idle(picks);
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    sim.world.get_mut::<Position>(blue).unwrap().0 = FixVec2::from_ints(100, 100);
    sim.world.get_mut::<Position>(red).unwrap().0 = FixVec2::from_ints(108, 100);
    let start = sim.world.get::<Health>(red).unwrap().current;
    let mut inputs = TickInputs::new();
    inputs.set(
        0,
        PlayerCommand::CastAbility {
            slot: 0,
            target: FixVec2::from_ints(108, 100),
        },
    );
    sim.tick(&inputs);
    for _ in 0..20 {
        sim.tick(&TickInputs::new());
    }
    let after = sim.world.get::<Health>(red).unwrap().current;
    assert!(after < start, "skillshot should hit ({start} -> {after})");
}

#[test]
fn bot_visibility_masks_hide_distant_enemies() {
    let mut sim = idle([0; MAX_PLAYERS]);
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    sim.world.get_mut::<Position>(blue).unwrap().0 = FixVec2::from_ints(10, 10);
    sim.world.get_mut::<Position>(red).unwrap().0 = FixVec2::from_ints(250, 250);
    let masks = sim.bot_visibility_masks();
    assert!(!Simulation::visible_to(&masks, Team::Blue, red));
    sim.world.get_mut::<Position>(red).unwrap().0 = FixVec2::from_ints(12, 10);
    let masks = sim.bot_visibility_masks();
    assert!(Simulation::visible_to(&masks, Team::Blue, red));
}

#[test]
fn custom_minion_stats_affect_spawned_minions() {
    let mut content = moba_game::GameContent::default();
    content.rules.melee_minion.max_health = 77;
    let mut sim = Simulation::new_with_content(
        SimConfig::default().with_bot_mask(0).with_spawn_waves(true),
        std::sync::Arc::new(content),
    );
    for _ in 0..32 {
        sim.tick(&TickInputs::new());
    }
    let health = sim
        .world
        .entities()
        .find(|entity| sim.world.get::<UnitKind>(*entity) == Some(&UnitKind::MeleeMinion))
        .and_then(|entity| sim.world.get::<Health>(entity))
        .expect("melee minion spawned");
    assert_eq!(health.max, 77);
}

#[test]
fn team_size_one_spawns_only_two_heroes() {
    let mut content = moba_game::GameContent::default();
    content.rules.team_size = 1;
    content.rules.bot_fill = false;
    let mut sim = Simulation::new_with_content(
        SimConfig::default()
            .with_bot_mask(0x3)
            .with_spawn_waves(false),
        std::sync::Arc::new(content),
    );
    let heroes = sim
        .world
        .entities()
        .filter(|entity| sim.world.get::<UnitKind>(*entity) == Some(&UnitKind::Hero))
        .count();
    assert_eq!(heroes, 2);
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[1];
    assert_eq!(sim.world.get::<Team>(blue).copied(), Some(Team::Blue));
    assert_eq!(sim.world.get::<Team>(red).copied(), Some(Team::Red));
    sim.tick_with_bots();
    assert!(sim.state_hash() != 0);
}

#[test]
fn custom_respawn_rule_controls_death_timer() {
    let mut content = moba_game::GameContent::default();
    content.rules.hero_respawn_ticks = 5;
    content.rules.respawn_per_level = 0;
    let mut sim = Simulation::new_with_content(
        SimConfig::default()
            .with_bot_mask(0)
            .with_spawn_waves(false),
        std::sync::Arc::new(content),
    );
    let hero = sim.hero_entities[0];
    sim.world.get_mut::<Health>(hero).unwrap().current = 0;
    sim.tick(&TickInputs::new());
    assert!(sim.world.has::<Dead>(hero));
    for _ in 0..6 {
        sim.tick(&TickInputs::new());
    }
    assert!(!sim.world.has::<Dead>(hero));
    assert!(sim.world.get::<Health>(hero).unwrap().current > 0);
}

#[test]
fn segment_distance_supports_fast_projectile_sweeps() {
    let start = FixVec2::from_ints(0, 0);
    let end = FixVec2::from_ints(10, 0);
    assert_eq!(
        super::combat::segment_distance_sq(FixVec2::from_ints(5, 0), start, end),
        Fix::ZERO
    );
    assert_eq!(
        super::combat::segment_distance_sq(FixVec2::from_ints(5, 2), start, end),
        Fix::from_i32(4)
    );
    assert_eq!(
        super::combat::segment_distance_sq(FixVec2::from_ints(2, 3), start, start),
        Fix::from_i32(13)
    );
}

#[test]
fn arcing_projectile_event_carries_arc() {
    let mut content = moba_game::GameContent::default();
    content.abilities[2].effects.effects[0] = moba_game::Effect::Projectile {
        speed_per_sec: Fix::from_i32(26),
        range: Fix::from_i32(11),
        radius: Fix::ONE,
        arc: Fix::from_i32(3),
        on_hit: moba_game::AreaEffectList::one(moba_game::AreaEffect::Damage {
            amount: 10,
            damage_type: moba_game::DamageType::Physical,
        }),
    };
    let picks = [0; 10];
    let mut sim = Simulation::new_with_content(
        SimConfig::default()
            .with_hero_picks(picks)
            .with_bot_mask(0)
            .with_spawn_waves(false),
        std::sync::Arc::new(content),
    );
    let blue = sim.hero_entities[0];
    sim.world.get_mut::<Position>(blue).unwrap().0 = FixVec2::from_ints(100, 100);
    let mut inputs = TickInputs::new();
    inputs.set(
        0,
        PlayerCommand::CastAbility {
            slot: 2,
            target: FixVec2::from_ints(106, 100),
        },
    );
    sim.tick(&inputs);
    let arc = sim
        .events()
        .iter()
        .find_map(|event| match *event {
            SimEvent::ProjectileSpawned { arc, .. } => Some(arc),
            _ => None,
        })
        .expect("projectile spawned");
    assert_eq!(arc, Fix::from_i32(3));
}

#[test]
fn a_cast_emits_exactly_one_cast_event() {
    let mut content = moba_game::GameContent::default();
    content.abilities[2].effects.effects[0] = moba_game::Effect::Projectile {
        speed_per_sec: Fix::from_i32(26),
        range: Fix::from_i32(11),
        radius: Fix::ONE,
        arc: Fix::ZERO,
        on_hit: moba_game::AreaEffectList::one(moba_game::AreaEffect::Damage {
            amount: 10,
            damage_type: moba_game::DamageType::Physical,
        }),
    };
    let mut sim = Simulation::new_with_content(
        SimConfig::default()
            .with_hero_picks([0; 10])
            .with_bot_mask(0)
            .with_spawn_waves(false),
        std::sync::Arc::new(content),
    );
    let blue = sim.hero_entities[0];
    sim.world.get_mut::<Position>(blue).unwrap().0 = FixVec2::from_ints(100, 100);
    let mut inputs = TickInputs::new();
    inputs.set(
        0,
        PlayerCommand::CastAbility {
            slot: 2,
            target: FixVec2::from_ints(106, 100),
        },
    );
    sim.tick(&inputs);
    let casts = sim
        .events()
        .iter()
        .filter(|event| matches!(event, SimEvent::AbilityCast { .. }))
        .count();
    assert_eq!(casts, 1);
}

#[test]
fn bot_mask_is_not_part_of_the_state_hash() {
    let config = SimConfig::default()
        .with_hero_picks([0; 10])
        .with_bot_mask(0)
        .with_spawn_waves(false);
    let mut sim = Simulation::new(config);
    let before = sim.state_hash();
    sim.set_bot_mask(0x3FF);
    assert_eq!(sim.state_hash(), before);
}

#[test]
fn visibility_masks_reject_out_of_range_entity_indices() {
    let masks = [u64::MAX; 16];
    assert!(!Simulation::visible_to(
        &masks,
        Team::Blue,
        Entity::from_parts(MAX_UNITS as u32, 1),
    ));
    assert!(Simulation::visible_to(
        &masks,
        Team::Red,
        Entity::from_parts((MAX_UNITS - 1) as u32, 1),
    ));
}

#[test]
fn bot_masks_match_fog_visibility() {
    let mut sim = idle(SimConfig::default().hero_picks);
    let masks = sim.bot_visibility_masks();
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    assert!(Simulation::visible_to(&masks, Team::Blue, blue));
    assert!(!Simulation::visible_to(&masks, Team::Blue, red));
    sim.world.get_mut::<Position>(red).unwrap().0 =
        sim.world.get::<Position>(blue).unwrap().0 + FixVec2::from_ints(2, 0);
    let masks = sim.bot_visibility_masks();
    assert!(Simulation::visible_to(&masks, Team::Blue, red));
}

#[test]
fn visibility_query_rejects_hidden_enemy_targets() {
    let mut sim = idle(SimConfig::default().hero_picks);
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    assert!(!sim.is_visible_to(Team::Blue, red));
    sim.world.get_mut::<Position>(red).unwrap().0 =
        sim.world.get::<Position>(blue).unwrap().0 + FixVec2::from_ints(2, 0);
    assert!(sim.is_visible_to(Team::Blue, red));
}

#[test]
fn enemy_units_are_only_visible_inside_team_vision() {
    let mut sim = idle(SimConfig::default().hero_picks);
    let mut observed = Vec::new();
    sim.observed_units(Team::Blue, &mut observed);
    assert!(
        !observed
            .iter()
            .any(|unit| unit.team == Team::Red && unit.kind.is_hero())
    );
    assert_eq!(
        observed
            .iter()
            .filter(|unit| unit.kind.is_structure())
            .count(),
        16,
        "structures are permanently known"
    );

    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    sim.world.get_mut::<Position>(blue).unwrap().0 = FixVec2::from_ints(200, 128);
    sim.world.get_mut::<Position>(red).unwrap().0 = FixVec2::from_ints(204, 128);
    sim.observed_units(Team::Blue, &mut observed);
    assert!(
        observed
            .iter()
            .any(|unit| unit.team == Team::Red && unit.kind.is_hero())
    );
}

#[test]
fn hero_kills_credit_the_attacking_team() {
    let mut sim = idle(SimConfig::default().hero_picks);
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    sim.damage(red, 10_000, DamageType::Physical, blue);
    sim.damage(blue, 10_000, DamageType::Physical, red);
    sim.tick(&TickInputs::default());
    assert_eq!(sim.score(), (1, 1));
    assert_eq!(
        sim.world.get::<Dead>(red).map(|dead| dead.respawn_in),
        Some(HERO_RESPAWN_TICKS)
    );
}

#[test]
fn progression_awards_xp_gold_and_items() {
    use moba_game::{STARTING_GOLD, Team};

    let mut sim = idle(SimConfig::default().hero_picks);
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    let red_position = sim.world.get::<Position>(red).expect("position").0;
    sim.world.get_mut::<Position>(blue).expect("position").0 = red_position;
    sim.damage(red, 10_000, DamageType::Physical, blue);
    sim.tick(&TickInputs::default());

    let progress = *sim.world.get::<Progression>(blue).expect("progression");
    assert_eq!(progress.level, 1);
    assert_eq!(progress.xp, 234, "solo hero kill shares 130% of 180 XP");
    assert!(progress.gold >= STARTING_GOLD + 300);

    let health_before = sim.world.get::<Health>(blue).expect("health").max;
    sim.grant_xp(blue, 1_000);
    let progress = *sim.world.get::<Progression>(blue).expect("progression");
    assert_eq!(progress.level, 4);
    assert_eq!(progress.xp, 94);
    assert_eq!(
        sim.world.get::<Health>(blue).expect("health").max,
        health_before + 210
    );

    let damage_before = sim.world.get::<AttackStats>(blue).expect("stats").damage;
    sim.world.get_mut::<Position>(blue).expect("position").0 =
        sim.content.map.nexus_position(Team::Blue);
    let gold_before = sim.world.get::<Progression>(blue).expect("progress").gold;
    sim.buy_item(blue, 0);
    let progress = *sim.world.get::<Progression>(blue).expect("progress");
    assert_eq!(progress.item_count, 1);
    assert_eq!(progress.gold, gold_before - 350);
    assert_eq!(
        sim.world.get::<AttackStats>(blue).expect("stats").damage,
        damage_before + 12
    );
}

#[test]
fn custom_content_purchase_updates_observation() {
    use moba_game::GameContent;
    let content = GameContent {
        attestation: None,
        rules: moba_game::RulesSpec::default(),
        manifest: GameContent::default().manifest,
        items: vec![moba_game::ItemSpec {
            name: String::from("Test Blade"),
            cost: 10,
            attack_damage: 5,
            ..moba_game::ItemSpec::from(&moba_game::ITEMS[0])
        }],
        heroes: GameContent::default().heroes,
        abilities: GameContent::default().abilities,
        modifiers: GameContent::default().modifiers,
        map: GameContent::default().map,
        assets: moba_game::AssetManifest::default(),
    };
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    assert!(sim.purchase(0, 0));
    let mut units = Vec::new();
    sim.observed_units(Team::Blue, &mut units);
    let own = units
        .iter()
        .find(|unit| unit.hero_slot == Some(0))
        .expect("own hero");
    assert_eq!(own.items[0], 0);
    assert!(own.gold < 500);
}

#[test]
fn walls_block_line_of_sight() {
    use moba_game::{GameContent, WallSpec};
    let mut content = GameContent::default();
    content.map.walls.push(WallSpec {
        a: FixVec2::from_ints(105, 90),
        b: FixVec2::from_ints(105, 170),
    });
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    sim.world.get_mut::<Position>(blue).unwrap().0 = FixVec2::from_ints(100, 128);
    sim.world.get_mut::<Position>(red).unwrap().0 = FixVec2::from_ints(110, 128);
    let mut observed = Vec::new();
    sim.observed_units(Team::Blue, &mut observed);
    assert!(
        observed.iter().all(|unit| unit.hero_slot != Some(5)),
        "wall blocks the enemy hero"
    );
}

#[test]
fn hero_growth_comes_from_content() {
    use moba_game::GameContent;
    let mut content = GameContent::default();
    content.heroes[0].health_per_level = 0;
    content.heroes[0].mana_per_level = 0;
    content.heroes[0].attack_damage_per_level = 0;
    content.heroes[0].armor_per_level = 0;
    content.heroes[0].magic_resist_per_level = 0;
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let blue = sim.hero_entities[0];
    let health = sim.world.get::<Health>(blue).expect("health").max;
    sim.grant_xp(blue, 2_000);
    assert!(sim.world.get::<Progression>(blue).expect("progress").level > 1);
    assert_eq!(sim.world.get::<Health>(blue).expect("health").max, health);
}

#[test]
#[allow(clippy::too_many_lines)] // Explicitly inserts one value of every component.
fn every_registered_component_is_hashed() {
    let mut sim = idle([0, 1, 2, 3, 4, 0, 1, 2, 3, 4]);
    let entity = sim.world_mut().spawn();
    let world = sim.world_mut();
    let _ = world.insert(entity, Position(FixVec2::ZERO));
    let _ = world.insert(entity, Team::Blue);
    let _ = world.insert(entity, UnitKind::Hero);
    let _ = world.insert(entity, Health { current: 1, max: 1 });
    let _ = world.insert(entity, Mana { current: 1, max: 1 });
    let _ = world.insert(entity, AttackStats::new(1, Fix::ONE, 30));
    let _ = world.insert(entity, AttackTarget::default());
    let _ = world.insert(entity, MoveTarget::default());
    let _ = world.insert(entity, Cooldowns::default());
    let _ = world.insert(
        entity,
        LaneWalker {
            lane: 0,
            waypoint: 0,
        },
    );
    let _ = world.insert(entity, Dead { respawn_in: 1 });
    let _ = world.insert(entity, HeroSlot(0));
    let _ = world.insert(
        entity,
        Progression {
            level: 1,
            xp: 0,
            gold: 0,
            items: [EMPTY_ITEM; moba_game::ITEM_SLOTS],
            item_count: 0,
        },
    );
    let mut modifiers = Modifiers::default();
    modifiers.entries[0] = Modifier {
        def_id: moba_game::MODIFIER_SHIELD,
        kind: ModifierKind::Shield,
        remaining: 1,
        factor: Fix::ONE,
        amount: 1,
        damage_type: DamageType::Magic,
        source: entity,
    };
    modifiers.len = 1;
    let _ = world.insert(entity, modifiers);
    let _ = world.insert(entity, Regen { health: 0, mana: 0 });
    let _ = world.insert(
        entity,
        Resistances {
            armor: 0,
            magic_resist: 0,
        },
    );
    let _ = world.insert(entity, VisionRange(Fix::ONE));
    let _ = world.insert(entity, MoveSpeed(Fix::ONE));
    let _ = world.insert(entity, BaseMoveSpeed(Fix::ONE));
    let _ = world.insert(
        entity,
        LastHitBy {
            team: Team::Blue,
            entity,
        },
    );
    let _ = world.insert(entity, AttackDamageType(DamageType::Physical));
    let _ = world.insert(entity, ItemCooldowns::default());

    let mut actual = sim.world().registered_component_types();
    actual.sort_unstable();
    let mut expected = vec![
        TypeId::of::<Position>(),
        TypeId::of::<Health>(),
        TypeId::of::<Mana>(),
        TypeId::of::<AttackStats>(),
        TypeId::of::<AttackTarget>(),
        TypeId::of::<MoveTarget>(),
        TypeId::of::<Cooldowns>(),
        TypeId::of::<ItemCooldowns>(),
        TypeId::of::<LaneWalker>(),
        TypeId::of::<Dead>(),
        TypeId::of::<HeroIdentity>(),
        TypeId::of::<HeroSlot>(),
        TypeId::of::<Progression>(),
        TypeId::of::<Modifiers>(),
        TypeId::of::<Team>(),
        TypeId::of::<UnitKind>(),
        TypeId::of::<Regen>(),
        TypeId::of::<Resistances>(),
        TypeId::of::<VisionRange>(),
        TypeId::of::<MoveSpeed>(),
        TypeId::of::<BaseMoveSpeed>(),
        TypeId::of::<LastHitBy>(),
        TypeId::of::<AttackDamageType>(),
    ];
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

#[test]
fn modifier_status_bits_replicate() {
    let mut content = GameContent::default();
    content.modifiers[0].status_bits = 0x0100;
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let hero = sim.hero_entities[0];
    let mut modifiers = Modifiers::default();
    modifiers.entries[0] = Modifier {
        def_id: 0,
        kind: ModifierKind::Shield,
        remaining: 10,
        factor: Fix::ONE,
        amount: 1,
        damage_type: DamageType::Physical,
        source: hero,
    };
    modifiers.len = 1;
    let _ = sim.world.insert(hero, modifiers);
    let mut observed = Vec::new();
    sim.observed_all(&mut observed);
    let unit = observed
        .iter()
        .find(|unit| unit.entity == hero)
        .expect("hero observed");
    assert_ne!(unit.status_bits & 0x0100, 0);
}

#[test]
fn modifier_on_tick_deals_configured_damage() {
    let mut content = GameContent::default();
    content.modifiers[0].on_tick_damage = 7;
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let hero = sim.hero_entities[0];
    let _ = sim.world.insert(hero, Regen { health: 0, mana: 0 });
    let mut modifiers = Modifiers::default();
    modifiers.entries[0] = Modifier {
        def_id: 0,
        kind: ModifierKind::Shield,
        remaining: 10,
        factor: Fix::ONE,
        amount: 1,
        damage_type: DamageType::Physical,
        source: hero,
    };
    modifiers.len = 1;
    let _ = sim.world.insert(hero, modifiers);
    let before = sim.world.get::<Health>(hero).expect("health").current;
    sim.tick(&TickInputs::new());
    let after = sim.world.get::<Health>(hero).expect("health").current;
    assert!(
        after < before,
        "on-tick should damage ({before} -> {after})"
    );
}

#[test]
fn modifier_on_damaged_bonus_applies() {
    let mut content = GameContent::default();
    content.modifiers[0].on_damaged_damage = 12;
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let hero = sim.hero_entities[0];
    assert_eq!(sim.modifier_on_damaged(hero), 0);
    let mut modifiers = Modifiers::default();
    modifiers.entries[0] = Modifier {
        def_id: 0,
        kind: ModifierKind::Shield,
        remaining: 10,
        factor: Fix::ONE,
        amount: 1,
        damage_type: DamageType::Physical,
        source: hero,
    };
    modifiers.len = 1;
    let _ = sim.world.insert(hero, modifiers);
    assert_eq!(sim.modifier_on_damaged(hero), 12);
}

#[test]
fn modifier_expiry_deals_configured_damage() {
    let mut content = GameContent::default();
    content.modifiers[0].on_expire_damage = 30;
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let hero = sim.hero_entities[0];
    let _ = sim.world.insert(hero, Regen { health: 0, mana: 0 });
    let mut modifiers = Modifiers::default();
    modifiers.entries[0] = Modifier {
        def_id: 0,
        kind: ModifierKind::Shield,
        remaining: 1,
        factor: Fix::ONE,
        amount: 1,
        damage_type: DamageType::Physical,
        source: hero,
    };
    modifiers.len = 1;
    let _ = sim.world.insert(hero, modifiers);
    let before = sim.world.get::<Health>(hero).expect("health").current;
    sim.tick(&TickInputs::new());
    let after = sim.world.get::<Health>(hero).expect("health").current;
    assert!(after < before, "expiry should damage ({before} -> {after})");
}

#[test]
fn modifier_stat_bonuses_apply() {
    let mut content = GameContent::default();
    content.modifiers[0].bonus_armor = 40;
    content.modifiers[0].bonus_attack_damage = 25;
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let hero = sim.hero_entities[0];
    assert_eq!(sim.modifier_armor(hero), 0);
    let mut modifiers = Modifiers::default();
    modifiers.entries[0] = Modifier {
        def_id: 0,
        kind: ModifierKind::Shield,
        remaining: 10,
        factor: Fix::ONE,
        amount: 1,
        damage_type: DamageType::Physical,
        source: hero,
    };
    modifiers.len = 1;
    let _ = sim.world.insert(hero, modifiers);
    assert_eq!(sim.modifier_armor(hero), 40);
    assert_eq!(sim.modifier_attack_damage(hero), 25);
}

#[test]
fn item_on_damaged_bonus_counts_inventory() {
    let mut content = GameContent::default();
    content.items[0].on_damaged_damage = 15;
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let hero = sim.hero_entities[0];
    assert_eq!(sim.on_damaged_bonus(hero), 0);
    let mut progress = *sim.world.get::<Progression>(hero).expect("progress");
    progress.items[0] = 0;
    progress.item_count = 1;
    let _ = sim.world.insert(hero, progress);
    assert_eq!(sim.on_damaged_bonus(hero), 15);
}

#[test]
fn item_on_hit_bonus_counts_inventory() {
    let mut content = GameContent::default();
    content.items[0].on_hit_damage = 20;
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let hero = sim.hero_entities[0];
    assert_eq!(sim.on_hit_bonus(hero), 0);
    let mut progress = *sim.world.get::<Progression>(hero).expect("progress");
    progress.items[0] = 0;
    progress.item_count = 1;
    let _ = sim.world.insert(hero, progress);
    assert_eq!(sim.on_hit_bonus(hero), 20);
}

#[test]
fn item_active_heals_and_respects_cooldown() {
    let mut content = GameContent::default();
    content.items[0].active_heal = 100;
    content.items[0].active_cooldown_ticks = 2;
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let hero = sim.hero_entities[0];
    let mut progress = *sim.world.get::<Progression>(hero).expect("progress");
    progress.items[0] = 0;
    progress.item_count = 1;
    let _ = sim.world.insert(hero, progress);
    let _ = sim.world.insert(hero, Regen { health: 0, mana: 0 });
    sim.world.get_mut::<Health>(hero).expect("health").current = 1;
    let mut inputs = TickInputs::new();
    inputs.set(0, PlayerCommand::UseItem(0));
    sim.tick(&inputs);
    assert_eq!(sim.world.get::<Health>(hero).expect("health").current, 101);
    sim.tick(&inputs);
    assert_eq!(sim.world.get::<Health>(hero).expect("health").current, 101);
    sim.tick(&TickInputs::new());
    sim.tick(&inputs);
    assert_eq!(sim.world.get::<Health>(hero).expect("health").current, 201);
}

#[test]
fn normalized_hash_ignores_seed() {
    let a = idle([0, 1, 2, 3, 4, 0, 1, 2, 3, 4]);
    let mut b = idle([0, 1, 2, 3, 4, 0, 1, 2, 3, 4]);
    b.config.seed = a.config.seed.wrapping_add(1);
    assert_ne!(a.state_hash(), b.state_hash());
    assert_eq!(a.state_hash_normalized(), b.state_hash_normalized());
}

#[test]
fn bot_mask_does_not_change_state_hash() {
    let mut sim = idle([0, 1, 2, 3, 4, 0, 1, 2, 3, 4]);
    let baseline = sim.state_hash();
    sim.config.bot_mask ^= 0x03FF;
    assert_eq!(sim.state_hash(), baseline);
}

#[test]
fn economy_rules_drive_starting_and_passive_gold() {
    let mut content = GameContent::default();
    content.rules.starting_gold = 123;
    content.rules.gold_per_interval = 7;
    content.rules.gold_tick_interval = 1;
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let hero = sim.hero_entities[0];
    assert_eq!(
        sim.world.get::<Progression>(hero).expect("progress").gold,
        123
    );
    let inputs = sim.bot_inputs();
    sim.tick(&inputs);
    assert_eq!(
        sim.world.get::<Progression>(hero).expect("progress").gold,
        130
    );
}

#[test]
fn state_hash_covers_combat_and_movement_fields() {
    let mut sim = idle([0, 1, 2, 3, 4, 0, 1, 2, 3, 4]);
    let hero = sim.hero_entities[0];
    let baseline = sim.state_hash();
    sim.world.get_mut::<Health>(hero).expect("health").max += 1;
    assert_ne!(sim.state_hash(), baseline);
    sim.world.get_mut::<Health>(hero).expect("health").max -= 1;
    sim.world.get_mut::<Mana>(hero).expect("mana").max += 1;
    assert_ne!(sim.state_hash(), baseline);
    sim.world.get_mut::<Mana>(hero).expect("mana").max -= 1;
    sim.world
        .get_mut::<AttackStats>(hero)
        .expect("attack")
        .damage += 1;
    assert_ne!(sim.state_hash(), baseline);
    sim.world
        .get_mut::<AttackStats>(hero)
        .expect("attack")
        .damage -= 1;
    sim.world
        .get_mut::<Resistances>(hero)
        .expect("resistances")
        .armor += 1;
    assert_ne!(sim.state_hash(), baseline);
    sim.world
        .get_mut::<Resistances>(hero)
        .expect("resistances")
        .armor -= 1;
    sim.world.get_mut::<Regen>(hero).expect("regen").health += 1;
    assert_ne!(sim.state_hash(), baseline);
    sim.world.get_mut::<Regen>(hero).expect("regen").health -= 1;
    sim.world.get_mut::<VisionRange>(hero).expect("vision").0 += Fix::ONE;
    assert_ne!(sim.state_hash(), baseline);
    sim.world.get_mut::<VisionRange>(hero).expect("vision").0 -= Fix::ONE;
    sim.world.get_mut::<MoveSpeed>(hero).expect("speed").0 += Fix::ONE;
    assert_ne!(sim.state_hash(), baseline);
    sim.world.get_mut::<MoveSpeed>(hero).expect("speed").0 -= Fix::ONE;
    sim.world
        .get_mut::<BaseMoveSpeed>(hero)
        .expect("base speed")
        .0 += Fix::ONE;
    assert_ne!(sim.state_hash(), baseline);
    sim.world
        .get_mut::<BaseMoveSpeed>(hero)
        .expect("base speed")
        .0 -= Fix::ONE;
    assert_eq!(sim.state_hash(), baseline);
}

#[test]
fn seed_drives_bot_variation_and_scheduling_phase() {
    let run = |seed: u64| {
        let mut sim = Simulation::new(SimConfig::default().with_seed(seed).with_spawn_waves(false));
        let phase = sim.blue_first();
        let commands = format!("{:?}", sim.bot_inputs());
        (phase, commands)
    };
    let (even_phase, even_commands) = run(0);
    let (odd_phase, odd_commands) = run(1);
    assert!(even_phase, "even seed starts blue-first");
    assert!(!odd_phase, "odd seed starts red-first");
    assert_ne!(even_commands, odd_commands, "seeded bot streams differ");
    assert_eq!(
        run(0),
        (even_phase, even_commands),
        "same seed is deterministic"
    );
}

#[test]
fn data_only_root_on_hit_skillshot_applies_root() {
    let content =
        moba_game::load_content_toml(include_str!("../../../../content/ability-pack.toml"))
            .expect("ability pack");
    let index = content
        .abilities
        .iter()
        .position(|ability| ability.name == "Rooting Shot")
        .expect("rooting shot");
    let mut content = content;
    content.heroes[0].abilities = [u8::try_from(index).expect("id"), 1, 2];
    let mut sim = Simulation::new_with_content(
        SimConfig::default()
            .with_bot_mask(0)
            .with_spawn_waves(false)
            .with_hero_picks([0; MAX_PLAYERS]),
        std::sync::Arc::new(content),
    );
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    let blue_position = sim.world.get::<Position>(blue).expect("position").0;
    sim.world.get_mut::<Position>(red).expect("position").0 =
        blue_position + FixVec2::from_ints(5, 0);
    let target = sim.world.get::<Position>(red).expect("position").0;
    let health = sim.world.get::<Health>(red).expect("health").current;
    let mut cast = TickInputs::new();
    cast.set(0, PlayerCommand::CastAbility { slot: 0, target });
    sim.tick(&cast);
    for _ in 0..30 {
        sim.tick(&TickInputs::new());
    }
    assert!(has_modifier(&sim, red, ModifierKind::Root));
    assert!(sim.world.get::<Health>(red).expect("health").current < health);
}

#[test]
fn data_only_burn_zone_periodically_damages() {
    let content =
        moba_game::load_content_toml(include_str!("../../../../content/ability-pack.toml"))
            .expect("ability pack");
    let index = content
        .abilities
        .iter()
        .position(|ability| ability.name == "Cinder Zone")
        .expect("cinder zone");
    let mut content = content;
    content.heroes[0].abilities = [u8::try_from(index).expect("id"), 1, 2];
    let mut sim = Simulation::new_with_content(
        SimConfig::default()
            .with_bot_mask(0)
            .with_spawn_waves(false)
            .with_hero_picks([0; MAX_PLAYERS]),
        std::sync::Arc::new(content),
    );
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    let blue_position = sim.world.get::<Position>(blue).expect("position").0;
    sim.world.get_mut::<Position>(red).expect("position").0 =
        blue_position + FixVec2::from_ints(3, 0);
    let target = sim.world.get::<Position>(red).expect("position").0;
    let health = sim.world.get::<Health>(red).expect("health").current;
    let mut cast = TickInputs::new();
    cast.set(0, PlayerCommand::CastAbility { slot: 0, target });
    sim.tick(&cast);
    for _ in 0..75 {
        sim.tick(&TickInputs::new());
    }
    assert!(has_modifier(&sim, red, ModifierKind::Burn));
    assert!(sim.world.get::<Health>(red).expect("health").current < health);
}

#[test]
fn every_unit_has_an_always_present_modifier_list() {
    let sim = idle([0, 1, 2, 3, 4, 0, 1, 2, 3, 4]);
    for entity in sim.world().entities() {
        assert!(
            sim.world().get::<Modifiers>(entity).is_some(),
            "every spawned unit must own Modifiers"
        );
    }
}

#[test]
fn state_hash_covers_modifiers() {
    let mut sim = idle([0, 1, 2, 3, 4, 0, 1, 2, 3, 4]);
    let hero = sim.hero_entities[0];
    let baseline = sim.state_hash();
    let modifiers = sim.world.get_mut::<Modifiers>(hero).expect("modifiers");
    modifiers.entries[0] = Modifier {
        def_id: moba_game::MODIFIER_SHIELD,
        kind: ModifierKind::Shield,
        remaining: 5,
        factor: Fix::ONE,
        amount: 10,
        damage_type: DamageType::Physical,
        source: hero,
    };
    modifiers.len = 1;
    assert_ne!(sim.state_hash(), baseline);
    let modifiers = sim.world.get_mut::<Modifiers>(hero).expect("modifiers");
    modifiers.entries[0].kind = ModifierKind::Slow;
    modifiers.entries[0].factor = moba_core::Fix::from_f32(0.5);
    assert_ne!(sim.state_hash(), baseline);
    let modifiers = sim.world.get_mut::<Modifiers>(hero).expect("modifiers");
    modifiers.entries[0].kind = ModifierKind::Root;
    assert_ne!(sim.state_hash(), baseline);
    sim.world
        .get_mut::<Modifiers>(hero)
        .expect("modifiers")
        .clear();
    assert_eq!(sim.state_hash(), baseline);
}

#[test]
fn state_hash_tracks_burn_source_and_type() {
    use moba_game::GameContent;
    let mut sim = Simulation::new_with_content(
        SimConfig::default().with_bot_mask(0),
        std::sync::Arc::new(GameContent::default()),
    );
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    let modifiers = sim.world.get_mut::<Modifiers>(red).expect("modifiers");
    modifiers.entries[0] = Modifier {
        def_id: moba_game::MODIFIER_BURN,
        kind: ModifierKind::Burn,
        remaining: 10,
        factor: Fix::ONE,
        amount: 20,
        damage_type: DamageType::Magic,
        source: blue,
    };
    modifiers.len = 1;
    let magic = sim.state_hash();
    let modifiers = sim.world.get_mut::<Modifiers>(red).expect("modifiers");
    modifiers.entries[0].damage_type = DamageType::Physical;
    let physical = sim.state_hash();
    assert_ne!(magic, physical);
}

#[test]
fn burn_ability_deals_damage_over_time_and_expires() {
    use moba_game::{AbilitySpec, GameContent};
    let mut content = GameContent::default();
    content.abilities[0] = AbilitySpec {
        name: String::from("Test Burn"),
        cooldown_ticks: 120,
        mana_cost: 5,
        targeting: Targeting::Point { range: Fix::ZERO },
        effects: area_status(
            moba_game::MODIFIER_BURN,
            1_000.into(),
            10,
            Fix::ONE,
            20,
            DamageType::Magic,
        ),
    };
    let mut sim = Simulation::new_with_content(
        SimConfig::default().with_bot_mask(0),
        std::sync::Arc::new(content),
    );
    let red = sim.hero_entities[5];
    let before = sim.world.get::<Health>(red).expect("health").current;
    let mut cast = TickInputs::new();
    let target = sim.world.get::<Position>(red).expect("position").0;
    cast.set(0, PlayerCommand::CastAbility { slot: 0, target });
    sim.tick(&cast);
    assert!(has_modifier(&sim, red, ModifierKind::Burn));
    for _ in 0..10 {
        sim.tick(&TickInputs::new());
    }
    assert!(!has_modifier(&sim, red, ModifierKind::Burn));
    assert!(
        sim.world.get::<Health>(red).expect("health").current < before - 100,
        "burn ticks damage over its duration"
    );
}

#[test]
fn silence_ability_blocks_casting_and_expires() {
    use moba_game::{AbilitySpec, GameContent};
    let mut content = GameContent::default();
    content.abilities[0] = AbilitySpec {
        name: String::from("Test Silence"),
        cooldown_ticks: 120,
        mana_cost: 5,
        targeting: Targeting::Point { range: Fix::ZERO },
        effects: area_status(
            moba_game::MODIFIER_SILENCE,
            1_000.into(),
            60,
            Fix::ONE,
            0,
            DamageType::Physical,
        ),
    };
    let mut sim = Simulation::new_with_content(
        SimConfig::default().with_bot_mask(0),
        std::sync::Arc::new(content),
    );
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    let red_position = sim.world.get::<Position>(red).expect("position").0;
    let mut cast = TickInputs::new();
    cast.set(
        0,
        PlayerCommand::CastAbility {
            slot: 0,
            target: red_position,
        },
    );
    sim.tick(&cast);
    assert!(has_modifier(&sim, red, ModifierKind::Silence));
    let mana_before = sim.world.get::<Mana>(red).expect("mana").current;
    let blue_position = sim.world.get::<Position>(blue).expect("position").0;
    let mut red_cast = TickInputs::new();
    red_cast.set(
        5,
        PlayerCommand::CastAbility {
            slot: 0,
            target: blue_position,
        },
    );
    sim.tick(&red_cast);
    assert_eq!(
        sim.world.get::<Mana>(red).expect("mana").current,
        mana_before,
        "silenced cast spends no mana"
    );
    for _ in 0..61 {
        sim.tick(&TickInputs::new());
    }
    assert!(!has_modifier(&sim, red, ModifierKind::Silence));
    sim.tick(&red_cast);
    assert!(
        sim.world.get::<Mana>(red).expect("mana").current < mana_before,
        "cast works after silence expires"
    );
}

#[test]
fn root_blocks_dash_but_not_after_expiry() {
    use moba_game::{AbilitySpec, GameContent};
    let mut content = GameContent::default();
    content.abilities[0] = AbilitySpec {
        name: String::from("Test Root"),
        cooldown_ticks: 120,
        mana_cost: 5,
        targeting: Targeting::Point { range: Fix::ZERO },
        effects: area_status(
            moba_game::MODIFIER_ROOT,
            1_000.into(),
            60,
            Fix::ONE,
            0,
            DamageType::Physical,
        ),
    };
    content.abilities[1] = AbilitySpec {
        name: String::from("Test Dash"),
        cooldown_ticks: 60,
        mana_cost: 0,
        targeting: Targeting::Direction {
            range: Fix::from_i32(50),
        },
        effects: EffectList::one(Effect::Displace {
            range: Fix::from_i32(50),
        }),
    };
    content.heroes[0].abilities = [0, 1, 2];
    let mut sim = Simulation::new_with_content(
        SimConfig::default().with_bot_mask(0),
        std::sync::Arc::new(content),
    );
    let red = sim.hero_entities[5];
    let red_position = sim.world.get::<Position>(red).expect("position").0;
    let mut root = TickInputs::new();
    root.set(
        0,
        PlayerCommand::CastAbility {
            slot: 0,
            target: red_position,
        },
    );
    sim.tick(&root);
    let mut dash = TickInputs::new();
    dash.set(
        5,
        PlayerCommand::CastAbility {
            slot: 1,
            target: red_position + FixVec2::from_ints(40, 0),
        },
    );
    sim.tick(&dash);
    assert_eq!(
        sim.world.get::<Position>(red).expect("position").0,
        red_position,
        "rooted hero cannot dash"
    );
    assert_eq!(
        sim.world.get::<Cooldowns>(red).expect("cooldowns").0[1],
        0,
        "blocked dash does not start cooldown"
    );
    for _ in 0..61 {
        sim.tick(&TickInputs::new());
    }
    sim.tick(&dash);
    assert_ne!(
        sim.world.get::<Position>(red).expect("position").0,
        red_position,
        "dash works after root expires"
    );
}

#[test]
fn root_ability_blocks_movement_and_expires() {
    use moba_game::{AbilitySpec, GameContent};
    let mut content = GameContent::default();
    content.abilities[0] = AbilitySpec {
        name: String::from("Test Root"),
        cooldown_ticks: 120,
        mana_cost: 5,
        targeting: Targeting::Point { range: Fix::ZERO },
        effects: area_status(
            moba_game::MODIFIER_ROOT,
            1_000.into(),
            60,
            Fix::ONE,
            0,
            DamageType::Physical,
        ),
    };
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let red = sim.hero_entities[5];
    let red_position = sim.world.get::<Position>(red).expect("position").0;
    let mut cast = TickInputs::new();
    cast.set(
        0,
        PlayerCommand::CastAbility {
            slot: 0,
            target: red_position,
        },
    );
    sim.tick(&cast);
    assert!(has_modifier(&sim, red, ModifierKind::Root));
    assert!(
        sim.world
            .get::<MoveTarget>(red)
            .expect("target")
            .0
            .is_none()
    );
    let mut move_red = TickInputs::new();
    move_red.set(5, PlayerCommand::MoveTo(FixVec2::from_ints(20, 20)));
    sim.tick(&move_red);
    assert!(
        sim.world
            .get::<MoveTarget>(red)
            .expect("target")
            .0
            .is_none()
    );
    for _ in 0..61 {
        sim.tick(&TickInputs::new());
    }
    assert!(!has_modifier(&sim, red, ModifierKind::Root));
    sim.tick(&move_red);
    assert!(
        sim.world
            .get::<MoveTarget>(red)
            .expect("target")
            .0
            .is_some()
    );
}

fn speed_fixture(haste_ticks: u32, slow_ticks: u32) -> (Simulation, Entity, Entity, Fix) {
    use moba_game::{AbilitySpec, GameContent};
    let mut content = GameContent::default();
    content.abilities[0] = AbilitySpec {
        name: String::from("Fixture Haste"),
        cooldown_ticks: 30,
        mana_cost: 0,
        targeting: Targeting::SelfCast,
        effects: self_modifier(
            moba_game::MODIFIER_HASTE,
            haste_ticks,
            Fix::from_f32(1.5),
            0,
            DamageType::Physical,
        ),
    };
    content.abilities[1] = AbilitySpec {
        name: String::from("Fixture Slow"),
        cooldown_ticks: 30,
        mana_cost: 0,
        targeting: Targeting::Point { range: Fix::ZERO },
        effects: area_status(
            moba_game::MODIFIER_SLOW,
            Fix::from_i32(10_000),
            slow_ticks,
            Fix::from_f32(0.5),
            0,
            DamageType::Physical,
        ),
    };
    for hero in &mut content.heroes {
        hero.abilities = [0, 1, 2];
    }
    let mut sim = Simulation::new_with_content(
        SimConfig::default()
            .with_bot_mask(0)
            .with_spawn_waves(false)
            .with_hero_picks([0; MAX_PLAYERS]),
        std::sync::Arc::new(content),
    );
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    let base = sim.world.get::<BaseMoveSpeed>(blue).expect("base speed").0;
    let blue_position = sim.world.get::<Position>(blue).expect("position").0;
    let mut inputs = TickInputs::new();
    inputs.set(
        0,
        PlayerCommand::CastAbility {
            slot: 0,
            target: FixVec2::ZERO,
        },
    );
    inputs.set(
        5,
        PlayerCommand::CastAbility {
            slot: 1,
            target: blue_position,
        },
    );
    sim.tick(&inputs);
    (sim, blue, red, base)
}

#[test]
fn haste_and_slow_expire_cleanly_in_either_order() {
    let (mut sim, blue, _red, base) = speed_fixture(30, 60);
    assert_eq!(
        sim.world.get::<MoveSpeed>(blue).expect("speed").0,
        base * Fix::from_f32(0.75)
    );
    for _ in 0..35 {
        sim.tick(&TickInputs::new());
    }
    assert_eq!(
        sim.world.get::<MoveSpeed>(blue).expect("speed").0,
        base * Fix::from_f32(0.5)
    );
    for _ in 0..30 {
        sim.tick(&TickInputs::new());
    }
    assert_eq!(sim.world.get::<MoveSpeed>(blue).expect("speed").0, base);

    let (mut sim, blue, _red, base) = speed_fixture(60, 30);
    for _ in 0..35 {
        sim.tick(&TickInputs::new());
    }
    assert_eq!(
        sim.world.get::<MoveSpeed>(blue).expect("speed").0,
        base * Fix::from_f32(1.5)
    );
    for _ in 0..30 {
        sim.tick(&TickInputs::new());
    }
    assert_eq!(sim.world.get::<MoveSpeed>(blue).expect("speed").0, base);
}

#[test]
fn item_purchase_while_slowed_updates_base_speed() {
    let (mut sim, blue, _red, base) = speed_fixture(60, 60);
    let nexus = sim.content.map.nexus_position(Team::Blue);
    sim.world.get_mut::<Position>(blue).expect("position").0 = nexus;
    sim.world
        .get_mut::<Progression>(blue)
        .expect("progress")
        .gold = 5_000;
    assert!(sim.purchase(0, 3), "Swift Boots purchase succeeds");
    let bonus = sim.content.items[3].move_speed_per_sec * ONE_TICK;
    let expected = (base + bonus) * Fix::from_f32(0.75);
    assert_eq!(sim.world.get::<MoveSpeed>(blue).expect("speed").0, expected);
    for _ in 0..65 {
        sim.tick(&TickInputs::new());
    }
    assert_eq!(
        sim.world.get::<MoveSpeed>(blue).expect("speed").0,
        base + bonus
    );
}

#[test]
fn haste_ability_boosts_speed_and_expires() {
    use moba_game::{AbilitySpec, GameContent};
    let mut content = GameContent::default();
    content.abilities[0] = AbilitySpec {
        name: String::from("Test Haste"),
        cooldown_ticks: 120,
        mana_cost: 5,
        targeting: Targeting::SelfCast,
        effects: self_modifier(
            moba_game::MODIFIER_HASTE,
            60,
            Fix::from_f32(1.5),
            0,
            DamageType::Physical,
        ),
    };
    let mut sim = Simulation::new_with_content(
        SimConfig::default().with_bot_mask(0),
        std::sync::Arc::new(content),
    );
    let blue = sim.hero_entities[0];
    let before = sim.world.get::<MoveSpeed>(blue).expect("speed").0;
    let mut cast = TickInputs::new();
    cast.set(
        0,
        PlayerCommand::CastAbility {
            slot: 0,
            target: FixVec2::ZERO,
        },
    );
    sim.tick(&cast);
    assert!(has_modifier(&sim, blue, ModifierKind::Haste));
    assert!(sim.world.get::<MoveSpeed>(blue).expect("speed").0 > before);
    for _ in 0..62 {
        sim.tick(&TickInputs::new());
    }
    assert!(!has_modifier(&sim, blue, ModifierKind::Haste));
    assert_eq!(sim.world.get::<MoveSpeed>(blue).expect("speed").0, before);
}

#[test]
fn slow_ability_reduces_speed_and_expires() {
    use moba_game::{AbilitySpec, GameContent};
    let mut content = GameContent::default();
    content.abilities[0] = AbilitySpec {
        name: String::from("Test Slow"),
        cooldown_ticks: 120,
        mana_cost: 5,
        targeting: Targeting::Point { range: Fix::ZERO },
        effects: area_status(
            moba_game::MODIFIER_SLOW,
            1_000.into(),
            60,
            Fix::from_f32(0.5),
            0,
            DamageType::Physical,
        ),
    };
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let red = sim.hero_entities[5];
    let red_position = sim.world.get::<Position>(red).expect("position").0;
    let before = sim.world.get::<MoveSpeed>(red).expect("speed").0;
    let mut inputs = TickInputs::new();
    inputs.set(
        0,
        PlayerCommand::CastAbility {
            slot: 0,
            target: red_position,
        },
    );
    sim.tick(&inputs);
    let slowed = sim.world.get::<MoveSpeed>(red).expect("speed").0;
    assert!(slowed < before, "slow reduces movement speed");
    assert!(has_modifier(&sim, red, ModifierKind::Slow));
    for _ in 0..62 {
        sim.tick(&TickInputs::new());
    }
    assert!(!has_modifier(&sim, red, ModifierKind::Slow));
    assert_eq!(sim.world.get::<MoveSpeed>(red).expect("speed").0, before);
}

#[test]
fn shield_ability_absorbs_damage_and_expires() {
    use moba_game::{AbilitySpec, GameContent};
    let mut content = GameContent::default();
    content.abilities[0] = AbilitySpec {
        name: String::from("Test Shield"),
        cooldown_ticks: 120,
        mana_cost: 10,
        targeting: Targeting::SelfCast,
        effects: self_modifier(
            moba_game::MODIFIER_SHIELD,
            60,
            Fix::ONE,
            100,
            DamageType::Physical,
        ),
    };
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    let mut inputs = TickInputs::new();
    inputs.set(
        0,
        PlayerCommand::CastAbility {
            slot: 0,
            target: FixVec2::ZERO,
        },
    );
    sim.tick(&inputs);
    assert_eq!(shield_total(&sim, blue), 100);
    let before = sim.world.get::<Health>(blue).expect("health").current;
    sim.damage(blue, 10, DamageType::Physical, red);
    assert_eq!(
        sim.world.get::<Health>(blue).expect("health").current,
        before,
        "shield absorbs the hit"
    );
    assert!(shield_total(&sim, blue) < 100);
    for _ in 0..62 {
        sim.tick(&TickInputs::new());
    }
    assert!(!has_modifier(&sim, blue, ModifierKind::Shield));
}

#[test]
fn custom_map_content_places_spawns_and_structures() {
    let content = moba_game::load_content_toml(include_str!("../../../../content/map-pack.toml"))
        .expect("map pack");
    let sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    assert_eq!(sim.unit_count(), 12, "10 heroes + two nexuses");
    let blue = sim
        .world
        .get::<Position>(sim.hero_entities[0])
        .expect("position");
    assert!(blue.0.x < Fix::from_i32(40));
    assert_eq!(
        sim.content.map.nexus_position(Team::Blue),
        FixVec2::from_ints(20, 128)
    );
}

#[test]
fn custom_bot_policy_replaces_default_inputs() {
    struct StopPolicy;

    impl BotPolicy for StopPolicy {
        fn inputs(&mut self, _simulation: &mut Simulation) -> TickInputs {
            let mut inputs = TickInputs::new();
            inputs.set(0, PlayerCommand::Stop);
            inputs
        }
    }

    let mut simulation = Simulation::new(SimConfig::default());
    simulation.set_bot_policy(Some(Box::new(StopPolicy)));
    let inputs = simulation.bot_inputs();
    assert_eq!(inputs.players[0], Some(PlayerCommand::Stop));
    assert!(inputs.players[1..].iter().all(Option::is_none));
    simulation.set_bot_policy(None);
    let default = simulation.bot_inputs();
    assert!(default.players.iter().any(Option::is_some));
}

#[test]
#[allow(clippy::too_many_lines)] // One table covers every point-target kind.
fn point_abilities_reject_out_of_range_targets() {
    use moba_game::{AbilitySpec, GameContent};
    let cases = [
        (
            area_status(
                moba_game::MODIFIER_SLOW,
                Fix::from_i32(1_000),
                10,
                Fix::from_f32(0.5),
                0,
                DamageType::Physical,
            ),
            Some(ModifierKind::Slow),
        ),
        (
            area_status(
                moba_game::MODIFIER_ROOT,
                Fix::from_i32(1_000),
                10,
                Fix::ONE,
                0,
                DamageType::Physical,
            ),
            Some(ModifierKind::Root),
        ),
        (
            area_status(
                moba_game::MODIFIER_SILENCE,
                Fix::from_i32(1_000),
                10,
                Fix::ONE,
                0,
                DamageType::Physical,
            ),
            Some(ModifierKind::Silence),
        ),
        (
            area_status(
                moba_game::MODIFIER_BURN,
                Fix::from_i32(1_000),
                10,
                Fix::ONE,
                10,
                DamageType::Magic,
            ),
            Some(ModifierKind::Burn),
        ),
        (
            EffectList::one(Effect::Area {
                radius: Fix::from_i32(1_000),
                effects: AreaEffectList::one(AreaEffect::Damage {
                    amount: 10,
                    damage_type: DamageType::Magic,
                }),
            }),
            None,
        ),
    ];
    for (effects, expected_status) in cases {
        let mut content = GameContent::default();
        content.abilities[0] = AbilitySpec {
            name: String::from("Range Probe"),
            cooldown_ticks: 60,
            mana_cost: 10,
            targeting: Targeting::Point {
                range: Fix::from_i32(5),
            },
            effects,
        };
        for hero in &mut content.heroes {
            hero.abilities = [0, 1, 2];
        }
        let mut sim = Simulation::new_with_content(
            SimConfig::default()
                .with_bot_mask(0)
                .with_spawn_waves(false)
                .with_hero_picks([0; MAX_PLAYERS]),
            std::sync::Arc::new(content),
        );
        let blue = sim.hero_entities[0];
        let red = sim.hero_entities[5];
        let blue_position = sim.world.get::<Position>(blue).expect("position").0;
        let target = blue_position + FixVec2::from_ints(100, 0);
        sim.world.get_mut::<Position>(red).expect("position").0 = target;
        let mana_before = sim.world.get::<Mana>(blue).expect("mana").current;
        let red_health = sim.world.get::<Health>(red).expect("health").current;
        let mut inputs = TickInputs::new();
        inputs.set(0, PlayerCommand::CastAbility { slot: 0, target });
        sim.tick(&inputs);
        assert_eq!(
            sim.world.get::<Mana>(blue).expect("mana").current,
            mana_before,
            "out-of-range cast spends no mana"
        );
        assert_eq!(
            sim.world.get::<Cooldowns>(blue).expect("cooldowns").0[0],
            0,
            "out-of-range cast starts no cooldown"
        );
        if let Some(kind) = expected_status {
            assert!(
                !has_modifier(&sim, red, kind),
                "out-of-range point effect was not applied"
            );
        }
        assert_eq!(
            sim.world.get::<Health>(red).expect("health").current,
            red_health,
            "out-of-range effect deals no damage"
        );
    }
}

#[test]
fn burst_is_centred_on_the_validated_target_point() {
    use moba_game::{AbilitySpec, GameContent};
    let mut content = GameContent::default();
    content.abilities[0] = AbilitySpec {
        name: String::from("Targeted Burst"),
        cooldown_ticks: 60,
        mana_cost: 0,
        targeting: Targeting::Point {
            range: Fix::from_i32(20),
        },
        effects: EffectList::one(Effect::Area {
            radius: Fix::from_i32(5),
            effects: AreaEffectList::one(AreaEffect::Damage {
                amount: 50,
                damage_type: DamageType::Magic,
            }),
        }),
    };
    for hero in &mut content.heroes {
        hero.abilities = [0, 1, 2];
    }
    let mut sim = Simulation::new_with_content(
        SimConfig::default()
            .with_bot_mask(0)
            .with_spawn_waves(false)
            .with_hero_picks([0; MAX_PLAYERS]),
        std::sync::Arc::new(content),
    );
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    let blue_position = sim.world.get::<Position>(blue).expect("position").0;
    let target = blue_position + FixVec2::from_ints(10, 0);
    sim.world.get_mut::<Position>(red).expect("position").0 = target;
    let blue_health = sim.world.get::<Health>(blue).expect("health").current;
    let red_health = sim.world.get::<Health>(red).expect("health").current;
    let mut inputs = TickInputs::new();
    inputs.set(0, PlayerCommand::CastAbility { slot: 0, target });
    sim.tick(&inputs);
    assert!(
        sim.world.get::<Health>(red).expect("health").current < red_health,
        "target-centred burst damages the enemy at the target"
    );
    assert_eq!(
        sim.world.get::<Health>(blue).expect("health").current,
        blue_health,
        "caster is outside the target-centred area"
    );
}

#[test]
fn custom_ability_content_drives_casting() {
    use moba_game::{AbilitySpec, GameContent};
    let mut content = GameContent::default();
    content.abilities[0] = AbilitySpec {
        name: String::from("Test Zap"),
        cooldown_ticks: 60,
        mana_cost: 10,
        targeting: Targeting::Point {
            range: Fix::from_i32(12),
        },
        effects: EffectList::one(Effect::Area {
            radius: Fix::from_i32(6),
            effects: AreaEffectList::one(AreaEffect::Damage {
                amount: 100,
                damage_type: DamageType::Magic,
            }),
        }),
    };
    let mut sim = Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    let red_position = sim.world.get::<Position>(red).expect("position").0;
    sim.world.get_mut::<Position>(blue).expect("position").0 = red_position;
    let before = sim.world.get::<Health>(red).expect("health").current;
    let mut inputs = TickInputs::new();
    inputs.set(
        0,
        PlayerCommand::CastAbility {
            slot: 0,
            target: red_position,
        },
    );
    sim.tick(&inputs);
    assert!(
        sim.world.get::<Health>(red).expect("health").current < before,
        "custom burst ability damaged the target"
    );
}

#[test]
fn surrender_ends_the_match_for_the_enemy_team() {
    use moba_game::Team;
    let mut sim = idle(SimConfig::default().hero_picks);
    let mut inputs = TickInputs::new();
    inputs.set(0, PlayerCommand::Surrender);
    sim.tick(&inputs);
    assert_eq!(sim.outcome(), Some(MatchOutcome::Winner(Team::Red)));
}

#[test]
fn dead_heroes_respawn_at_their_base() {
    let mut sim = idle(SimConfig::default().hero_picks);
    let blue = sim.hero_entities[0];
    sim.world.get_mut::<Health>(blue).unwrap().current = 0;
    sim.tick(&TickInputs::new());
    assert!(sim.world.get::<Dead>(blue).is_some());
    for _ in 0..(HERO_RESPAWN_TICKS + 2) {
        sim.tick(&TickInputs::new());
    }
    assert!(sim.world.get::<Dead>(blue).is_none());
    let health = sim.world.get::<Health>(blue).unwrap();
    assert_eq!(health.current, health.max);
    let position = sim.world.get::<Position>(blue).unwrap().0;
    assert!(position.x < Fix::from_i32(80));
}
