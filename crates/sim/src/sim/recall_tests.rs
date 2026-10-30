//! Recall: channel, teleport, every cancellation cause and repeat behaviour.

use moba_core::{Fix, FixVec2};
use moba_ecs::Entity;
use moba_game::{DamageType, GameContent, ModifierDef, ModifierKind, Team};

use super::{PlayerCommand, RecallCancel, RecallEnd, SimConfig, SimEvent, Simulation, TickInputs};
use crate::components::{Cooldowns, Dead, Health, MoveTarget, Position};

/// A quiet spot away from every structure (tower range is 10).
const RECALL_SPOT: FixVec2 = FixVec2::from_ints(64, 100);

/// An idle match with blue slot 0 standing at [`RECALL_SPOT`] and red slot 5 one unit away
/// (within melee range).
fn recall_sim() -> Simulation {
    recall_sim_with(GameContent::default())
}

fn recall_sim_with(content: GameContent) -> Simulation {
    let mut sim = Simulation::new_with_content(
        SimConfig::default()
            .with_bot_mask(0)
            .with_spawn_waves(false)
            .with_hero_picks([0, 1, 2, 3, 4, 0, 1, 2, 3, 4]),
        std::sync::Arc::new(content),
    );
    let blue = sim.hero_entities[0];
    let red = sim.hero_entities[5];
    sim.world.get_mut::<Position>(blue).expect("blue").0 = RECALL_SPOT;
    sim.world.get_mut::<Position>(red).expect("red").0 = RECALL_SPOT + FixVec2::from_ints(1, 0);
    sim
}

fn command(slot: usize, command: PlayerCommand) -> TickInputs {
    let mut inputs = TickInputs::new();
    inputs.set(slot, command);
    inputs
}

fn recall_events(sim: &Simulation) -> Vec<SimEvent> {
    sim.events()
        .iter()
        .copied()
        .filter(|event| {
            matches!(
                event,
                SimEvent::RecallStarted { .. } | SimEvent::RecallEnded { .. }
            )
        })
        .collect()
}

/// Starts slot 0's recall and runs `ticks` more idle ticks; returns the hero.
fn start_recall_and_wait(sim: &mut Simulation, ticks: u32) -> Entity {
    let hero = sim.hero_entities[0];
    sim.tick(&command(0, PlayerCommand::Recall));
    assert_eq!(
        recall_events(sim),
        vec![SimEvent::RecallStarted {
            hero,
            team: Team::Blue,
            position: RECALL_SPOT,
            duration_ticks: 240,
        }]
    );
    sim.clear_events();
    for _ in 0..ticks {
        sim.tick(&TickInputs::new());
        sim.clear_events();
    }
    hero
}

fn assert_cancelled(sim: &Simulation, hero: Entity, cause: RecallCancel) {
    assert_eq!(
        sim.recall_remaining(0),
        Some(0),
        "{cause:?} cancels the recall"
    );
    assert_eq!(
        recall_events(sim),
        vec![SimEvent::RecallEnded {
            hero,
            team: Team::Blue,
            position: RECALL_SPOT,
            end: RecallEnd::Cancelled(cause),
        }]
    );
}

fn stun_content() -> (GameContent, u16) {
    let mut content = GameContent::default();
    content.modifiers.push(ModifierDef {
        name: String::from("Stun"),
        kind: ModifierKind::Stun,
        bonus_armor: 0,
        bonus_attack_damage: 0,
        on_expire_damage: 0,
        on_damaged_damage: 0,
        on_tick_damage: 0,
        status_bits: 0,
    });
    let stun = u16::try_from(content.modifiers.len() - 1).expect("id");
    content.validate().expect("valid");
    (content, stun)
}

#[test]
fn recall_channels_for_eight_seconds_then_teleports_to_the_spawn() {
    let mut sim = recall_sim();
    let hero = sim.hero_entities[0];
    let health = sim.hero_health(0).expect("health");
    // A walking hero stops when it starts recalling.
    sim.tick(&command(
        0,
        PlayerCommand::MoveTo(FixVec2::from_ints(64, 140)),
    ));
    sim.world.get_mut::<Position>(hero).expect("hero").0 = RECALL_SPOT;
    start_recall_and_wait(&mut sim, 0);
    assert_eq!(sim.recall_remaining(0), Some(239));
    assert_eq!(sim.world.get::<MoveTarget>(hero), Some(&MoveTarget(None)));
    let mut observed = Vec::new();
    sim.observed_units(Team::Blue, &mut observed);
    assert!(
        observed
            .iter()
            .any(|unit| unit.entity == hero && unit.recalling),
        "the channel is observable"
    );
    for _ in 0..238 {
        sim.tick(&TickInputs::new());
        assert_eq!(recall_events(&sim), [] as [SimEvent; 0]);
        sim.clear_events();
    }
    assert_eq!(sim.recall_remaining(0), Some(1));
    assert_eq!(sim.hero_position(0), Some(RECALL_SPOT), "it stands still");
    sim.tick(&TickInputs::new());
    let spawn = sim.content().map.hero_spawn(Team::Blue, 0);
    assert_eq!(sim.hero_position(0), Some(spawn));
    assert_eq!(sim.recall_remaining(0), Some(0));
    assert_eq!(
        recall_events(&sim),
        vec![SimEvent::RecallEnded {
            hero,
            team: Team::Blue,
            position: RECALL_SPOT,
            end: RecallEnd::Completed { destination: spawn },
        }]
    );
    assert_eq!(sim.hero_health(0), Some(health), "nothing hit it");
    sim.observed_units(Team::Blue, &mut observed);
    assert!(
        observed
            .iter()
            .any(|unit| unit.entity == hero && !unit.recalling)
    );
}

#[test]
fn recall_length_comes_from_content_and_zero_disables_it() {
    let mut content = GameContent::default();
    content.rules.recall_ticks = 3;
    let mut sim = recall_sim_with(content);
    sim.tick(&command(0, PlayerCommand::Recall));
    sim.tick(&TickInputs::new());
    assert_eq!(sim.hero_position(0), Some(RECALL_SPOT));
    sim.tick(&TickInputs::new());
    assert_eq!(
        sim.hero_position(0),
        Some(sim.content().map.hero_spawn(Team::Blue, 0))
    );

    let mut content = GameContent::default();
    content.rules.recall_ticks = 0;
    let mut sim = recall_sim_with(content);
    sim.tick(&command(0, PlayerCommand::Recall));
    assert_eq!(sim.recall_remaining(0), Some(0));
    assert_eq!(recall_events(&sim), [] as [SimEvent; 0]);
}

#[test]
fn red_heroes_recall_to_their_own_spawn_slot() {
    let mut sim = recall_sim();
    let red = sim.hero_entities[7];
    sim.world.get_mut::<Position>(red).expect("red").0 = FixVec2::from_ints(190, 156);
    sim.tick(&command(7, PlayerCommand::Recall));
    for _ in 0..239 {
        sim.tick(&TickInputs::new());
    }
    assert_eq!(
        sim.hero_position(7),
        Some(sim.content().map.hero_spawn(Team::Red, 2))
    );
}

#[test]
fn move_attack_and_stop_orders_cancel_a_recall() {
    let red = recall_sim().hero_entities[5];
    for order in [
        PlayerCommand::MoveTo(FixVec2::from_ints(64, 110)),
        PlayerCommand::Attack(red),
        PlayerCommand::Stop,
    ] {
        let mut sim = recall_sim();
        let hero = start_recall_and_wait(&mut sim, 30);
        sim.tick(&command(0, order));
        assert_cancelled(&sim, hero, RecallCancel::Order);
    }
}

#[test]
fn a_rooted_hero_ignores_a_move_order_and_keeps_recalling() {
    let mut sim = recall_sim();
    let red = sim.hero_entities[5];
    let hero = start_recall_and_wait(&mut sim, 5);
    sim.apply_modifier(
        hero,
        moba_game::MODIFIER_ROOT,
        30,
        Fix::ONE,
        0,
        red,
        DamageType::Physical,
    );
    sim.tick(&command(
        0,
        PlayerCommand::MoveTo(FixVec2::from_ints(64, 110)),
    ));
    assert_eq!(recall_events(&sim), [] as [SimEvent; 0]);
    assert_eq!(sim.recall_remaining(0), Some(233));
}

#[test]
fn a_successful_cast_cancels_a_recall_but_a_refused_one_does_not() {
    let mut sim = recall_sim();
    let hero = start_recall_and_wait(&mut sim, 10);
    // Second Wind on cooldown: refused, so the channel goes on.
    sim.world.get_mut::<Cooldowns>(hero).expect("cooldowns").0[1] = 100;
    let cast = PlayerCommand::CastAbility {
        slot: 1,
        target: RECALL_SPOT,
    };
    sim.tick(&command(0, cast));
    assert_eq!(recall_events(&sim), [] as [SimEvent; 0]);
    assert_eq!(sim.recall_remaining(0), Some(228));
    sim.clear_events();
    sim.world.get_mut::<Cooldowns>(hero).expect("cooldowns").0[1] = 0;
    sim.tick(&command(0, cast));
    assert_cancelled(&sim, hero, RecallCancel::Cast);
    assert!(
        sim.events()
            .iter()
            .any(|event| matches!(event, SimEvent::AbilityCast { .. }))
    );
}

#[test]
fn damage_cancels_a_recall() {
    let mut sim = recall_sim();
    let hero = start_recall_and_wait(&mut sim, 30);
    // A real basic attack from the adjacent enemy hero.
    sim.tick(&command(5, PlayerCommand::Attack(hero)));
    assert_cancelled(&sim, hero, RecallCancel::Damage);
    let health = sim.hero_health(0).expect("health");
    assert!(health.current < health.max);
}

#[test]
fn a_stun_cancels_a_recall_and_blocks_starting_one() {
    let (content, stun) = stun_content();
    let mut sim = recall_sim_with(content.clone());
    let red = sim.hero_entities[5];
    let hero = start_recall_and_wait(&mut sim, 30);
    sim.apply_modifier(hero, stun, 30, Fix::ONE, 0, red, DamageType::Physical);
    assert_cancelled(&sim, hero, RecallCancel::Stun);
    sim.clear_events();
    sim.tick(&command(0, PlayerCommand::Recall));
    assert_eq!(
        sim.recall_remaining(0),
        Some(0),
        "cannot recall while stunned"
    );
    assert_eq!(recall_events(&sim), [] as [SimEvent; 0]);

    // A slow or silence does not cancel a recall either (a root: the test above).
    for modifier in [moba_game::MODIFIER_SLOW, moba_game::MODIFIER_SILENCE] {
        let mut sim = recall_sim_with(content.clone());
        let hero = start_recall_and_wait(&mut sim, 5);
        sim.apply_modifier(hero, modifier, 30, Fix::ONE, 0, red, DamageType::Physical);
        sim.tick(&TickInputs::new());
        assert_eq!(recall_events(&sim), [] as [SimEvent; 0]);
        assert_eq!(sim.recall_remaining(0), Some(233));
    }
}

#[test]
fn dying_cancels_a_recall_and_the_hero_respawns_without_one() {
    let mut sim = recall_sim();
    let hero = start_recall_and_wait(&mut sim, 30);
    // Health reaching zero without damage (an embedder's direct write) still cancels it
    // through the death pass.
    sim.world.get_mut::<Health>(hero).expect("health").current = 0;
    sim.tick(&TickInputs::new());
    assert_cancelled(&sim, hero, RecallCancel::Death);
    assert!(sim.world.has::<Dead>(hero));
    for _ in 0..400 {
        sim.tick(&TickInputs::new());
    }
    assert!(!sim.world.has::<Dead>(hero), "respawned");
    assert_eq!(sim.recall_remaining(0), Some(0));

    // A killing blow cancels it as damage, before the death.
    let mut sim = recall_sim();
    let red = sim.hero_entities[5];
    let hero = start_recall_and_wait(&mut sim, 30);
    sim.damage(hero, 1_000_000, DamageType::Magic, red);
    assert_cancelled(&sim, hero, RecallCancel::Damage);
    sim.clear_events();
    sim.tick(&TickInputs::new());
    assert!(sim.world.has::<Dead>(hero));
    assert!(recall_events(&sim).is_empty(), "cancelled once");
    // A dead hero cannot recall.
    sim.tick(&command(0, PlayerCommand::Recall));
    assert_eq!(sim.recall_remaining(0), Some(0));
    assert_eq!(recall_events(&sim), [] as [SimEvent; 0]);
}

#[test]
fn recalling_again_while_channelling_has_no_effect() {
    let mut sim = recall_sim();
    start_recall_and_wait(&mut sim, 100);
    assert_eq!(sim.recall_remaining(0), Some(139));
    sim.tick(&command(0, PlayerCommand::Recall));
    assert!(recall_events(&sim).is_empty(), "no restart, no cancel");
    assert_eq!(sim.recall_remaining(0), Some(138));
    for _ in 0..137 {
        sim.tick(&TickInputs::new());
    }
    assert_eq!(sim.hero_position(0), Some(RECALL_SPOT));
    sim.tick(&TickInputs::new());
    assert_eq!(
        sim.hero_position(0),
        Some(sim.content().map.hero_spawn(Team::Blue, 0)),
        "completes on the original schedule"
    );
}

#[test]
fn buying_and_using_items_do_not_cancel_a_recall() {
    let mut sim = recall_sim();
    start_recall_and_wait(&mut sim, 5);
    let mut inputs = command(0, PlayerCommand::UseItem(0));
    inputs.set_purchase(0, 0);
    sim.tick(&inputs);
    assert_eq!(sim.recall_remaining(0), Some(233));
    sim.tick(&command(0, PlayerCommand::BuyItem(0)));
    assert_eq!(sim.recall_remaining(0), Some(232));
    assert_eq!(recall_events(&sim), [] as [SimEvent; 0]);
}

#[test]
fn the_recall_channel_is_part_of_the_state_hash() {
    let mut a = recall_sim();
    let mut b = recall_sim();
    a.tick(&command(0, PlayerCommand::Recall));
    b.tick(&TickInputs::new());
    assert_eq!(a.hero_position(0), b.hero_position(0));
    assert_ne!(a.state_hash(), b.state_hash());
}
