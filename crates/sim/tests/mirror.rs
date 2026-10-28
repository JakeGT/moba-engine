//! Side-swap tolerance mirror (WP-2.1, #290).
//!
//! Two matches run on the default map with waves off. Run A uses `seed`; run B uses
//! `seed ^ 1`, which flips the alternating command/attack phase so the team that resolves
//! first in A resolves first (mirrored) in B. A scripted [`BotPolicy`] gives slot `i` in A
//! the same commands as slot `(i + 5) % 10` in B, with explicit coordinates reflected in x
//! (`x -> extent - x`) and `Attack` targets remapped through the hero slot -> entity
//! tables ([`Simulation::hero_entity`]).
//!
//! Every tick the test compares each A slot with its mirrored B slot:
//! - hero positions: x reflected and y equal, within [`POSITION_TOLERANCE_ULPS`];
//! - health, mana, dead state, ability cooldowns, level, xp, gold and the team score:
//!   exactly;
//! - per-tick events, normalised to slots and teams (positions dropped), as an exact
//!   multiset, so kill credit is compared by slot.
//!
//! An exact mirror is not a goal (finish plan, Phase 2: the maintainer accepted an
//! asymmetric mirror). The script is shaped around the known asymmetries instead of
//! hiding them in tolerances:
//! - `Fix` multiplication and division floor toward negative infinity, so
//!   `floor(-x) != -floor(x)` and a reflected walk drifts by a few hundred ULPs until the
//!   hero reaches its destination and snaps to it. Positions are the only tolerance.
//! - The alternating phase reverses the *whole* slot list, so within a team the order is
//!   reversed between the runs. Two same-team attackers finishing one target in the same
//!   tick therefore swap kill credit (and gold, thorns, attack cooldown). Heroes fight in
//!   five separate 1v1 duels (blue slot `k` against red slot `5 + (k + 1) % 5`), so a
//!   target only ever takes damage from one enemy and same-team ordering is unobservable.
//! - Deaths are resolved in ECS order, and a hero that dies earlier in the same pass no
//!   longer earns experience from a later death. A mutual kill therefore hands the
//!   experience to whichever hero the ECS lists second. Each duel alternates an aggressor
//!   and a defender that only holds position, with a 30-tick truce between windows for
//!   projectiles in flight, so a duel cannot end in a mutual kill. Arenas are more than
//!   the 30-unit experience radius apart, so deaths in different duels never interact.
//! - Tower targeting breaks ties by raw entity id. Duel arenas are at least 24 units from
//!   every tower (tower range is 10), so structures never engage.
//! - Waves are x-mirrored since sim logic v8, but the remaining entity-id tie-breaks and
//!   processing-order asymmetries still split the runs within a few waves (first at tick
//!   2,514 for the first seed pair when measured, against tick 738 before v8), so waves
//!   stay disabled.
//! - Bot RNG streams are per slot; the scripted policy draws no simulation randomness.

use moba_core::{Fix, FixVec2};
use moba_ecs::Entity;
use moba_game::Team;
use moba_sim::{
    BotPolicy, Cooldowns, Dead, Health, Mana, MatchOutcome, PlayerCommand, Position, Progression,
    SimConfig, SimEvent, Simulation, TickInputs,
};

/// Ticks per run.
const TICKS: u32 = 3_000;
/// Maximum allowed difference, in Q32.32 ULPs (2^-32 world units), between a hero's
/// coordinate in run A and the reflected coordinate of its mirror in run B. The observed
/// maximum is a few hundred ULPs (about 1e-7 units); see the module docs.
const POSITION_TOLERANCE_ULPS: i64 = 2_048;
/// Player slots in a 5v5 match.
const SLOTS: usize = 10;
/// Duel arena centre per pair (indexed by the blue slot). Arenas are 44+ units apart, 24+
/// units from every tower, and off the walking lines to the other arenas.
const ARENAS: [(i32, i32); 5] = [(128, 32), (128, 80), (128, 176), (128, 224), (84, 184)];
/// Each duel alternates its aggressor every window.
const WINDOW_TICKS: u32 = 600;
/// Start of each window in which neither duellist attacks.
const TRUCE_TICKS: u32 = 30;
/// A hero further than this from its arena point walks back before fighting.
const ARENA_RADIUS: i32 = 3;
/// Items bought at tick 0, chosen per slot from these (Ironblade, Mana Crystal, Swift
/// Boots). Chain Vest is excluded: its thorns would let a defender deal damage and so
/// reopen same-tick mutual kills.
const ITEMS: [u8; 3] = [0, 2, 3];

/// Small deterministic generator for the script (test-only; not simulation RNG).
struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u32) -> u32 {
        u32::try_from(self.next() % u64::from(bound)).expect("bounded")
    }

    fn range(&mut self, low: i32, high: i32) -> i32 {
        let span = u32::try_from(high - low + 1).expect("positive span");
        low + i32::try_from(self.below(span)).expect("small")
    }

    /// Random `Fix` bits in `[-0.25, 0.25)`.
    fn quarter(&mut self) -> i64 {
        i64::try_from(self.next() >> 33).expect("31 bits") - (1 << 30)
    }
}

/// A scripted cast, in run A's frame.
#[derive(Copy, Clone, Debug)]
enum Cast {
    /// At the duel partner's current position.
    AtPartner(u8),
    /// At the caster's position plus an integer offset.
    Offset { ability: u8, dx: i32, dy: i32 },
}

/// The script in run A's frame. Run B's policy reflects it.
#[derive(Clone)]
struct Script {
    /// Duel point per A slot, as raw `Fix` bits.
    arena: [(i64, i64); SLOTS],
    /// Item bought at tick 0 per A slot.
    buy: [u8; SLOTS],
    /// `casts[tick][slot]`.
    casts: Vec<[Option<Cast>; SLOTS]>,
}

fn build_script(seed: u64) -> Script {
    let mut rng = SplitMix(seed);
    let arena = core::array::from_fn(|slot| {
        // Partners stand about 1.5 units apart (blue west), inside melee range. Random
        // fractional bits keep the duel off integer lattices, where exact Pythagorean
        // distances (3-4-5) can sit exactly on an ability radius in run A and a few ULPs
        // either side of it in run B.
        let (side, pair): (i64, usize) = if slot < 5 {
            (-3, slot)
        } else {
            (3, partner(slot))
        };
        let (cx, cy) = ARENAS[pair];
        let x = (i64::from(cx) << 32) + side * (1 << 30) + rng.quarter();
        let y = (i64::from(cy) << 32) + rng.quarter();
        (x, y)
    });
    let buy = core::array::from_fn(|_| ITEMS[rng.below(3) as usize]);
    let casts = (0..TICKS)
        .map(|_| {
            core::array::from_fn(|_| {
                if rng.below(20) != 0 {
                    return None;
                }
                let ability = u8::try_from(rng.below(3)).expect("small");
                Some(if rng.below(4) == 0 {
                    Cast::Offset {
                        ability,
                        dx: rng.range(-2, 2),
                        dy: rng.range(1, 2),
                    }
                } else {
                    Cast::AtPartner(ability)
                })
            })
        })
        .collect();
    Script { arena, buy, casts }
}

/// The slot that plays run-A slot `slot` in run B.
const fn mirror_slot(slot: usize) -> usize {
    (slot + 5) % SLOTS
}

/// Duel partner of a slot: blue `k` against red `5 + (k + 1) % 5`, so the duellists play
/// different heroes.
const fn partner(slot: usize) -> usize {
    if slot < 5 {
        5 + (slot + 1) % 5
    } else {
        (slot + 3) % 5
    }
}

/// Whether A slot `slot` may deal damage on `tick`.
const fn aggressor(slot: usize, tick: u32) -> bool {
    let pair = if slot < 5 { slot } else { partner(slot) };
    let blue_turn = (tick / WINDOW_TICKS + pair as u32).is_multiple_of(2);
    tick % WINDOW_TICKS >= TRUCE_TICKS && blue_turn == (slot < 5)
}

/// Replays the script; `mirrored` selects run B's reflected frame.
struct DuelPolicy {
    script: Script,
    mirrored: bool,
}

impl DuelPolicy {
    fn slot(&self, slot: usize) -> usize {
        if self.mirrored {
            mirror_slot(slot)
        } else {
            slot
        }
    }

    fn point(&self, extent: Fix, (x, y): (i64, i64)) -> FixVec2 {
        let x = Fix::from_bits(x);
        let x = if self.mirrored { extent - x } else { x };
        FixVec2::new(x, Fix::from_bits(y))
    }
}

impl BotPolicy for DuelPolicy {
    fn inputs(&mut self, simulation: &mut Simulation) -> TickInputs {
        let mut inputs = TickInputs::new();
        let tick = simulation.current_tick().get();
        let Some(casts) = self.script.casts.get(tick as usize) else {
            return inputs;
        };
        let extent = simulation.content().map.extent;
        let radius_sq = Fix::from_i32(ARENA_RADIUS * ARENA_RADIUS);
        for (a_slot, cast) in casts.iter().enumerate() {
            let slot = self.slot(a_slot);
            // Entity map: run-A partner slot -> this run's slot -> this run's entity.
            let opponent = simulation
                .hero_entity(self.slot(partner(a_slot)))
                .expect("partner exists");
            let hero = simulation.hero_entity(slot).expect("hero exists");
            if tick == 0 {
                inputs.set_purchase(slot, self.script.buy[a_slot]);
            }
            if simulation.world().has::<Dead>(hero) {
                continue;
            }
            let own = simulation.hero_position(slot).expect("hero position");
            let arena = self.point(extent, self.script.arena[a_slot]);
            if !aggressor(a_slot, tick) {
                // Defenders and truces hold the arena point; `MoveTo` also clears any
                // attack order.
                inputs.set(slot, PlayerCommand::MoveTo(arena));
                continue;
            }
            let command = match *cast {
                Some(Cast::AtPartner(ability)) => {
                    simulation.world().get::<Position>(opponent).map(|target| {
                        PlayerCommand::CastAbility {
                            slot: ability,
                            target: target.0,
                        }
                    })
                }
                Some(Cast::Offset { ability, dx, dy }) => {
                    let dx = if self.mirrored { -dx } else { dx };
                    Some(PlayerCommand::CastAbility {
                        slot: ability,
                        target: own + FixVec2::from_ints(dx, dy),
                    })
                }
                None => Some(if own.distance_sq(arena) > radius_sq {
                    PlayerCommand::MoveTo(arena)
                } else {
                    PlayerCommand::Attack(opponent)
                }),
            };
            if let Some(command) = command {
                inputs.set(slot, command);
            }
        }
        inputs
    }
}

/// An event in run A's frame (slots and teams), without positions.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Died {
        victim: Option<u8>,
        killer: Option<u8>,
        team: u8,
    },
    StructureDestroyed {
        team: u8,
    },
    Respawned(Option<u8>),
    Cast {
        caster: Option<u8>,
        ability: u8,
    },
    ProjectileSpawned(Option<u8>),
    ProjectileHit {
        owner: Option<u8>,
        target: Option<u8>,
    },
    ZoneSpawned(Option<u8>),
    Attack {
        attacker: Option<u8>,
        target: Option<u8>,
        team: u8,
        interval_ticks: u32,
    },
    MatchEnded(Option<u8>),
    WaveSpawned,
}

/// Reads one run in run A's frame.
struct Frame<'a> {
    simulation: &'a Simulation,
    mirrored: bool,
}

impl Frame<'_> {
    fn slot(&self, slot: u8) -> u8 {
        if self.mirrored {
            u8::try_from(mirror_slot(usize::from(slot))).expect("slot fits u8")
        } else {
            slot
        }
    }

    fn entity(&self, entity: Entity) -> Option<u8> {
        (0..SLOTS)
            .find(|&slot| self.simulation.hero_entity(slot) == Some(entity))
            .map(|slot| self.slot(u8::try_from(slot).expect("slot fits u8")))
    }

    fn team(&self, team: Team) -> u8 {
        let team = if self.mirrored { team.enemy() } else { team };
        match team {
            Team::Blue => 0,
            Team::Red => 1,
        }
    }

    fn key(&self, event: &SimEvent) -> Key {
        match *event {
            SimEvent::WaveSpawned { .. } => Key::WaveSpawned,
            SimEvent::UnitDied {
                team,
                hero_slot,
                killer_slot,
                ..
            } => Key::Died {
                victim: hero_slot.map(|slot| self.slot(slot)),
                killer: killer_slot.map(|slot| self.slot(slot)),
                team: self.team(team),
            },
            SimEvent::StructureDestroyed { team, .. } => Key::StructureDestroyed {
                team: self.team(team),
            },
            SimEvent::HeroRespawned { entity } => Key::Respawned(self.entity(entity)),
            SimEvent::AbilityCast {
                caster, ability, ..
            } => Key::Cast {
                caster: self.entity(caster),
                ability,
            },
            SimEvent::ProjectileSpawned { owner, .. } => Key::ProjectileSpawned(self.entity(owner)),
            SimEvent::ProjectileHit { owner, target, .. } => Key::ProjectileHit {
                owner: self.entity(owner),
                target: self.entity(target),
            },
            SimEvent::ZoneSpawned { owner, .. } => Key::ZoneSpawned(self.entity(owner)),
            SimEvent::BasicAttack {
                attacker,
                team,
                target,
                interval_ticks,
            } => Key::Attack {
                attacker: self.entity(attacker),
                target: self.entity(target),
                team: self.team(team),
                interval_ticks,
            },
            SimEvent::MatchEnded { winner } => {
                Key::MatchEnded(winner.map(|winner| self.team(winner)))
            }
            _ => unreachable!("mirror test has no key for {event:?}"),
        }
    }

    /// This tick's events as a sorted multiset.
    fn events(&self) -> Vec<Key> {
        let mut keys: Vec<Key> = self
            .simulation
            .events()
            .iter()
            .map(|event| self.key(event))
            .collect();
        keys.sort_unstable();
        keys
    }

    /// `(blue, red)` score in run A's frame.
    fn score(&self) -> (u32, u32) {
        let (blue, red) = self.simulation.score();
        if self.mirrored {
            (red, blue)
        } else {
            (blue, red)
        }
    }
}

/// Exact per-hero state compared between the runs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Exact {
    health: Health,
    mana: Mana,
    dead: bool,
    cooldowns: [u32; 3],
    level: u8,
    xp: u32,
    gold: u32,
}

fn exact(simulation: &Simulation, slot: usize) -> Exact {
    let entity = simulation.hero_entity(slot).expect("slot exists");
    let world = simulation.world();
    let progress = world.get::<Progression>(entity).copied().expect("hero");
    Exact {
        health: world.get::<Health>(entity).copied().expect("hero health"),
        mana: world.get::<Mana>(entity).copied().expect("hero mana"),
        dead: world.has::<Dead>(entity),
        cooldowns: world.get::<Cooldowns>(entity).expect("hero cooldowns").0,
        level: progress.level,
        xp: progress.xp,
        gold: progress.gold,
    }
}

/// Totals over a mirrored run, to prove the script exercised combat.
#[derive(Default, Debug)]
struct Coverage {
    max_ulps: i64,
    deaths: usize,
    casts: usize,
    projectile_hits: usize,
    damaged_ticks: usize,
}

fn run_mirror(match_seed: u64, script_seed: u64) -> Coverage {
    let config = SimConfig::default()
        .with_seed(match_seed)
        .with_bot_mask(0x03FF)
        .with_spawn_waves(false);
    // B slot i + 5 plays A slot i, so it also plays A slot i's hero.
    let picks = config.hero_picks;
    let mut a = Simulation::new(config);
    let mut b = Simulation::new(
        config
            .with_seed(match_seed ^ 1)
            .with_hero_picks(core::array::from_fn(|slot| picks[mirror_slot(slot)])),
    );
    let script = build_script(script_seed);
    a.set_bot_policy(Some(Box::new(DuelPolicy {
        script: script.clone(),
        mirrored: false,
    })));
    b.set_bot_policy(Some(Box::new(DuelPolicy {
        script,
        mirrored: true,
    })));
    let extent = a.content().map.extent;
    let context = format!("seed {match_seed:#x}, script {script_seed:#x}");
    let mut coverage = Coverage::default();
    for tick in 0..TICKS {
        a.tick_with_bots();
        b.tick_with_bots();
        let frame_a = Frame {
            simulation: &a,
            mirrored: false,
        };
        let frame_b = Frame {
            simulation: &b,
            mirrored: true,
        };
        let events = frame_a.events();
        assert_eq!(
            events,
            frame_b.events(),
            "{context}: events diverge at tick {tick}"
        );
        for key in &events {
            match key {
                Key::Died { .. } => coverage.deaths += 1,
                Key::Cast { .. } => coverage.casts += 1,
                Key::ProjectileHit { .. } => coverage.projectile_hits += 1,
                _ => {}
            }
        }
        let mut damaged = false;
        for slot in 0..SLOTS {
            let other = mirror_slot(slot);
            let pa = a.hero_position(slot).expect("A hero");
            let pb = b.hero_position(other).expect("B hero");
            let dx = (pa.x.to_bits() - (extent - pb.x).to_bits()).abs();
            let dy = (pa.y.to_bits() - pb.y.to_bits()).abs();
            coverage.max_ulps = coverage.max_ulps.max(dx).max(dy);
            assert!(
                dx <= POSITION_TOLERANCE_ULPS && dy <= POSITION_TOLERANCE_ULPS,
                "{context}: slot {slot} position off by ({dx}, {dy}) ULPs at tick {tick}: \
                 {pa:?} vs reflected {pb:?}"
            );
            let state = exact(&a, slot);
            assert_eq!(
                state,
                exact(&b, other),
                "{context}: slot {slot} state diverges at tick {tick}"
            );
            damaged |= state.health.current < state.health.max;
        }
        coverage.damaged_ticks += usize::from(damaged);
        assert_eq!(
            frame_a.score(),
            frame_b.score(),
            "{context}: score diverges at tick {tick}"
        );
        assert_eq!(
            a.outcome(),
            b.outcome().map(|outcome| match outcome {
                MatchOutcome::Winner(team) => MatchOutcome::Winner(team.enemy()),
                MatchOutcome::Draw => MatchOutcome::Draw,
                other => unreachable!("mirror test cannot mirror {other:?}"),
            }),
            "{context}: outcome diverges at tick {tick}"
        );
        a.clear_events();
        b.clear_events();
    }
    coverage
}

#[test]
fn side_swap_mirror_matches_within_tolerance() {
    // A runs both scheduling phases (even and odd seed); B is always the other phase.
    for match_seed in [0x5EED_1234_u64, 0x5EED_1235] {
        for script_seed in (0..8u64).map(|i| 0x0290_0000 + i) {
            let coverage = run_mirror(match_seed, script_seed);
            eprintln!("seed {match_seed:#x} script {script_seed:#x}: {coverage:?}");
            // The script must actually produce fights, or the mirror proves nothing.
            assert!(coverage.casts > 100, "too few casts: {coverage:?}");
            assert!(coverage.deaths > 0, "no hero died: {coverage:?}");
            assert!(
                coverage.damaged_ticks > 1_000,
                "too little combat: {coverage:?}"
            );
        }
    }
}
