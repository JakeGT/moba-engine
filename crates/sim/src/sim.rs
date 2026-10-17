//! Headless fixed-tick MOBA simulation.

use core::fmt;

use moba_core::{Fix, FixVec2, StableHasher, Tick};
use moba_ecs::{Entity, With, Without, World};
use moba_game::{
    DamageType, HEROES, HeroDef, HeroId, MAP, MELEE_MINION, NEXUS, RANGED_MINION, TOWER, Team,
    mitigate_typed,
};
use moba_nav::SpatialGrid;

use crate::components::{
    AttackDamageType, AttackStats, AttackTarget, Cooldowns, Dead, Health, HeroIdentity, HeroSlot,
    LaneWalker, Mana, MoveSpeed, MoveTarget, Position, Regen, Resistances, UnitKind, VisionRange,
};

/// Number of player slots (5v5).
pub const MAX_PLAYERS: usize = 10;
/// Hard cap on live entities. Waves stop when the cap is reached.
pub const MAX_UNITS: usize = 512;
/// Hard cap on in-flight projectiles.
pub const MAX_PROJECTILES: usize = 64;
/// Hard cap on queued events; events beyond this are dropped, never allocated.
pub const MAX_EVENTS: usize = 256;
/// Ticks between minion waves (30 seconds at 30 Hz).
pub const WAVE_INTERVAL_TICKS: u32 = 900;
/// First wave delay (1 second).
pub const WAVE_DELAY_TICKS: u32 = 30;
/// Hero respawn delay (10 seconds).
pub const HERO_RESPAWN_TICKS: u32 = 300;
/// Map extent in world units.
pub const WORLD_EXTENT: i32 = 256;

/// Fixed-point duration of one tick (`1/30` second).
const ONE_TICK: Fix = Fix::from_bits(143_165_576);
const MINION_AGGRO: Fix = Fix::from_i32(6);
const MINION_AGGRO_SQ: Fix = Fix::from_bits(
    MINION_AGGRO
        .to_bits()
        .saturating_mul(MINION_AGGRO.to_bits()),
);
const WAYPOINT_REACH: Fix = Fix::from_i32(3);

/// Simulation configuration.
#[derive(Clone, Copy, Debug)]
pub struct SimConfig {
    /// Seed mixed into the deterministic state hash.
    pub seed: u64,
    /// Bitmask of player slots controlled by the deterministic bot AI.
    pub bot_mask: u16,
    /// Hero id per player slot (`0..5`; larger values wrap).
    pub hero_picks: [u8; MAX_PLAYERS],
    /// Whether to spawn minion waves. Tests can disable this for cheap tick loops.
    pub spawn_waves: bool,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            seed: 0x5EED_1234,
            bot_mask: 0x03FF,
            hero_picks: [0, 1, 2, 3, 4, 4, 3, 2, 1, 0],
            spawn_waves: true,
        }
    }
}

/// A command from one player for the current tick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlayerCommand {
    /// Clears movement and attack orders.
    Stop,
    /// Walks to a world position.
    MoveTo(FixVec2),
    /// Basic-attacks an entity while it is in range.
    Attack(Entity),
    /// Casts ability slot `0..3` at a world position.
    CastAbility {
        /// Q/W/E slot.
        slot: u8,
        /// Target point.
        target: FixVec2,
    },
}

/// One tick of input for every player.
#[derive(Copy, Clone, Debug, Default)]
pub struct TickInputs {
    /// Slot-indexed commands; `None` keeps the current order.
    pub players: [Option<PlayerCommand>; MAX_PLAYERS],
}

impl TickInputs {
    /// Creates empty inputs.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            players: [None; MAX_PLAYERS],
        }
    }

    /// Sets one player's command, ignoring out-of-range slots.
    pub fn set(&mut self, slot: usize, command: PlayerCommand) {
        if slot < MAX_PLAYERS {
            self.players[slot] = Some(command);
        }
    }
}

/// Something observable that happened during a tick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SimEvent {
    /// A minion wave spawned.
    WaveSpawned {
        /// Wave number, starting at one.
        wave: u32,
    },
    /// A unit died.
    UnitDied {
        /// Dead entity.
        entity: Entity,
        /// Unit type.
        kind: UnitKind,
        /// Owning team.
        team: Team,
    },
    /// A tower or nexus was destroyed.
    StructureDestroyed {
        /// Owning team.
        team: Team,
        /// Structure type.
        kind: UnitKind,
    },
    /// A hero respawned.
    HeroRespawned {
        /// Respawned entity.
        entity: Entity,
    },
    /// A nexus fell and the match ended.
    MatchEnded {
        /// Winning team.
        winner: Team,
    },
}

#[derive(Copy, Clone, Debug)]
struct TargetRef {
    entity: Entity,
    team: Team,
    kind: UnitKind,
    position: FixVec2,
}

#[derive(Copy, Clone, Debug)]
struct Projectile {
    owner: Entity,
    team: Team,
    position: FixVec2,
    direction: FixVec2,
    speed_per_tick: Fix,
    remaining: Fix,
    radius: Fix,
    damage: i32,
    damage_type: DamageType,
}

#[derive(Copy, Clone, Debug)]
struct HeroView {
    slot: u8,
    team: Team,
    position: FixVec2,
    health: i32,
    mana: i32,
    cooldowns: Cooldowns,
    attack_range_sq: Fix,
    def: &'static HeroDef,
}

#[derive(Copy, Clone, Debug)]
struct AttackerSnapshot {
    attacker: Entity,
    target: Entity,
    damage: i32,
    range_sq: Fix,
    interval: u32,
}

/// A unit as seen by one team's fog-of-war snapshot.
#[derive(Copy, Clone, Debug)]
pub struct ObservedUnit {
    /// Stable entity handle.
    pub entity: Entity,
    /// Unit type.
    pub kind: UnitKind,
    /// Owning team.
    pub team: Team,
    /// Ground position.
    pub position: FixVec2,
    /// Current health (`0` while dead).
    pub health: i32,
    /// Maximum health.
    pub max_health: i32,
    /// Current mana (zero for non-heroes).
    pub mana: i32,
    /// Maximum mana (zero for non-heroes).
    pub max_mana: i32,
    /// Whether the unit is a dead hero waiting to respawn.
    pub dead: bool,
    /// Player slot for heroes, `None` otherwise.
    pub hero_slot: Option<u8>,
    /// Remaining Q/W/E cooldowns for heroes.
    pub cooldowns: [u32; 3],
}

/// The authoritative MOBA simulation.
pub struct Simulation {
    world: World,
    tick: Tick,
    config: SimConfig,
    hero_entities: [Entity; MAX_PLAYERS],
    wave: u32,
    next_wave_tick: u32,
    outcome: Option<Team>,
    projectiles: Vec<Projectile>,
    events: Vec<SimEvent>,
    grid: SpatialGrid<TargetRef>,
    hero_views: Vec<HeroView>,
    attackers: Vec<AttackerSnapshot>,
    deaths: Vec<(Entity, Team, UnitKind)>,
    respawns: Vec<Entity>,
    damage_hits: Vec<Entity>,
}

impl Simulation {
    /// Creates a fresh match.
    #[must_use]
    pub fn new(config: SimConfig) -> Self {
        let mut simulation = Self {
            world: World::with_capacity(MAX_UNITS),
            tick: Tick::ZERO,
            config,
            hero_entities: [Entity::PLACEHOLDER; MAX_PLAYERS],
            wave: 0,
            next_wave_tick: WAVE_DELAY_TICKS,
            outcome: None,
            projectiles: Vec::with_capacity(MAX_PROJECTILES),
            events: Vec::with_capacity(MAX_EVENTS),
            grid: SpatialGrid::new(
                FixVec2::ZERO,
                FixVec2::splat(Fix::from_i32(WORLD_EXTENT)),
                Fix::from_i32(8),
            ),
            hero_views: Vec::with_capacity(MAX_PLAYERS),
            attackers: Vec::with_capacity(MAX_UNITS),
            deaths: Vec::with_capacity(MAX_UNITS),
            respawns: Vec::with_capacity(MAX_PLAYERS),
            damage_hits: Vec::with_capacity(MAX_UNITS),
        };
        simulation.spawn_structures();
        for slot in 0..MAX_PLAYERS {
            let entity = simulation.spawn_hero(slot);
            simulation.hero_entities[slot] = entity;
        }
        simulation.refresh_spatial();
        simulation
    }

    /// Current tick (the next tick to be simulated).
    #[inline]
    #[must_use]
    pub const fn current_tick(&self) -> Tick {
        self.tick
    }

    /// Winning team once a nexus falls.
    #[inline]
    #[must_use]
    pub const fn outcome(&self) -> Option<Team> {
        self.outcome
    }

    /// Number of waves spawned so far.
    #[inline]
    #[must_use]
    pub const fn wave(&self) -> u32 {
        self.wave
    }

    /// Number of live entities.
    #[inline]
    #[must_use]
    pub fn unit_count(&self) -> usize {
        self.world.entity_count()
    }

    /// Read-only access to the ECS world (renderer/tools/snapshots).
    #[inline]
    #[must_use]
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Simulation configuration.
    #[inline]
    #[must_use]
    pub const fn config(&self) -> SimConfig {
        self.config
    }

    /// Entity for a player slot.
    #[inline]
    #[must_use]
    pub const fn hero_entity(&self, slot: usize) -> Option<Entity> {
        if slot < MAX_PLAYERS {
            Some(self.hero_entities[slot])
        } else {
            None
        }
    }

    /// Hero position, if the slot exists.
    #[must_use]
    pub fn hero_position(&self, slot: usize) -> Option<FixVec2> {
        self.hero_entity(slot)
            .and_then(|entity| self.world.get::<Position>(entity))
            .map(|position| position.0)
    }

    /// Hero health, if the slot exists.
    #[must_use]
    pub fn hero_health(&self, slot: usize) -> Option<Health> {
        self.hero_entity(slot)
            .and_then(|entity| self.world.get::<Health>(entity))
            .copied()
    }

    /// Sets which player slots are driven by the server bot AI.
    ///
    /// The network server clears the bit for connected humans and leaves bot bits set for
    /// empty slots. The match seed and picks do not change.
    pub fn set_bot_mask(&mut self, bot_mask: u16) {
        self.config.bot_mask = bot_mask;
    }

    /// Builds the list of units visible to `viewer` for a snapshot.
    ///
    /// Teammates are always visible (including dead heroes so the HUD can show respawn
    /// timers). Enemy structures are treated as permanently known map features; enemy
    /// heroes and minions are visible only while inside the vision radius of a living
    /// teammate. The output vector is cleared first and must be owned by the caller so the
    /// hot path allocates nothing after warm-up.
    pub fn observed_units(&self, viewer: Team, out: &mut Vec<ObservedUnit>) {
        out.clear();
        let mut sources = [(FixVec2::ZERO, Fix::ZERO); MAX_UNITS];
        let mut source_count = 0usize;
        for entity in self.world.entities() {
            let Some(team) = self.world.get::<Team>(entity) else {
                continue;
            };
            if *team != viewer {
                continue;
            }
            let Some(health) = self.world.get::<Health>(entity) else {
                continue;
            };
            if health.current <= 0 {
                continue;
            }
            let Some(position) = self.world.get::<Position>(entity) else {
                continue;
            };
            let Some(vision) = self.world.get::<VisionRange>(entity) else {
                continue;
            };
            if source_count < MAX_UNITS {
                sources[source_count] = (position.0, vision.0);
                source_count += 1;
            }
        }

        for entity in self.world.entities() {
            let Some(team) = self.world.get::<Team>(entity) else {
                continue;
            };
            let Some(kind) = self.world.get::<UnitKind>(entity) else {
                continue;
            };
            let Some(position) = self.world.get::<Position>(entity) else {
                continue;
            };
            let Some(health) = self.world.get::<Health>(entity) else {
                continue;
            };
            let dead = health.current <= 0;
            let allied = *team == viewer;
            let visible = allied
                || kind.is_structure()
                || (!dead && unit_in_vision(position.0, &sources[..source_count]));
            if !visible {
                continue;
            }
            let mana = self
                .world
                .get::<Mana>(entity)
                .map_or((0, 0), |mana| (mana.current, mana.max));
            let hero_slot = self.world.get::<HeroSlot>(entity).map(|slot| slot.0);
            let cooldowns = self
                .world
                .get::<Cooldowns>(entity)
                .map_or([0; 3], |cooldowns| {
                    [
                        u32::from(u16::try_from(cooldowns.0[0]).unwrap_or(u16::MAX)),
                        u32::from(u16::try_from(cooldowns.0[1]).unwrap_or(u16::MAX)),
                        u32::from(u16::try_from(cooldowns.0[2]).unwrap_or(u16::MAX)),
                    ]
                });
            out.push(ObservedUnit {
                entity,
                kind: *kind,
                team: *team,
                position: position.0,
                health: health.current,
                max_health: health.max,
                mana: mana.0,
                max_mana: mana.1,
                dead: dead && hero_slot.is_some(),
                hero_slot,
                cooldowns,
            });
        }
    }

    /// Events produced since the queue was last cleared.
    #[inline]
    #[must_use]
    pub fn events(&self) -> &[SimEvent] {
        &self.events
    }

    /// Clears the event queue.
    pub fn clear_events(&mut self) {
        self.events.clear();
    }

    /// Advances the simulation by one tick using the supplied inputs.
    pub fn tick(&mut self, inputs: &TickInputs) {
        if self.outcome.is_some() {
            return;
        }
        self.tick_timers();
        self.apply_inputs(inputs);
        self.refresh_spatial();
        self.update_minion_targets();
        self.update_structure_targets();
        self.move_units();
        self.refresh_spatial();
        self.resolve_attacks();
        self.update_projectiles();
        self.resolve_deaths();
        if self.tick.get() >= self.next_wave_tick && self.config.spawn_waves {
            self.spawn_wave();
            self.next_wave_tick = self.next_wave_tick.saturating_add(WAVE_INTERVAL_TICKS);
        }
        self.tick = self.tick.wrapping_add(1);
    }

    /// Generates deterministic bot commands and advances one tick.
    pub fn tick_with_bots(&mut self) {
        let inputs = self.bot_inputs();
        self.tick(&inputs);
    }

    /// Deterministic hash of the complete authoritative state.
    ///
    /// Used by replay/reconciliation tests. Iteration follows ECS archetype order, which is
    /// deterministic for the same command sequence.
    #[must_use]
    pub fn state_hash(&self) -> u64 {
        let mut hasher = StableHasher::new();
        hasher.write_u64(self.config.seed);
        hasher.write_u32(self.tick.get());
        hasher.write_u32(self.wave);
        hasher.write_u32(self.next_wave_tick);
        match self.outcome {
            Some(team) => hasher.write_u32(1 + team.index() as u32),
            None => hasher.write_u32(0),
        }

        for projectile in &self.projectiles {
            hasher.write_u64(projectile.owner.to_bits());
            hasher.write_u32(projectile.team.index() as u32);
            hasher.write_u64(projectile.position.x.to_bits() as u64);
            hasher.write_u64(projectile.position.y.to_bits() as u64);
            hasher.write_u64(projectile.remaining.to_bits() as u64);
            hasher.write_u32(projectile.damage as u32);
        }

        for entity in self.world.entities() {
            hasher.write_u64(entity.to_bits());
            hash_team(&self.world, entity, &mut hasher);
            hash_kind(&self.world, entity, &mut hasher);
            if let Some(position) = self.world.get::<Position>(entity) {
                hasher.write_u32(1);
                hasher.write_u64(position.0.x.to_bits() as u64);
                hasher.write_u64(position.0.y.to_bits() as u64);
            } else {
                hasher.write_u32(0);
            }
            if let Some(health) = self.world.get::<Health>(entity) {
                hasher.write_u32(2);
                hasher.write_u32(health.current as u32);
            }
            if let Some(mana) = self.world.get::<Mana>(entity) {
                hasher.write_u32(3);
                hasher.write_u32(mana.current as u32);
            }
            if let Some(stats) = self.world.get::<AttackStats>(entity) {
                hasher.write_u32(4);
                hasher.write_u32(stats.cooldown);
            }
            if let Some(target) = self.world.get::<AttackTarget>(entity) {
                hasher.write_u32(5);
                hasher.write_u64(target.0.map_or(u64::MAX, Entity::to_bits));
            }
            if let Some(target) = self.world.get::<MoveTarget>(entity) {
                hasher.write_u32(6);
                match target.0 {
                    Some(position) => {
                        hasher.write_u32(1);
                        hasher.write_u64(position.x.to_bits() as u64);
                        hasher.write_u64(position.y.to_bits() as u64);
                    }
                    None => hasher.write_u32(0),
                }
            }
            if let Some(cooldowns) = self.world.get::<Cooldowns>(entity) {
                hasher.write_u32(7);
                for cooldown in cooldowns.0 {
                    hasher.write_u32(cooldown);
                }
            }
            if let Some(walker) = self.world.get::<LaneWalker>(entity) {
                hasher.write_u32(8);
                hasher.write_u32(u32::from(walker.lane));
                hasher.write_u32(u32::from(walker.waypoint));
            }
            if let Some(dead) = self.world.get::<Dead>(entity) {
                hasher.write_u32(9);
                hasher.write_u32(dead.respawn_in);
            }
            if let Some(identity) = self.world.get::<HeroIdentity>(entity) {
                hasher.write_u32(10);
                hasher.write_u32(u32::from(identity.0.0));
            }
        }
        hasher.finish64()
    }

    fn spawn_structures(&mut self) {
        for structure in MAP.structures {
            let entity = self.world.spawn();
            let (stats, kind) = match structure.kind {
                moba_game::StructureKind::Tower => (&TOWER, UnitKind::Tower),
                moba_game::StructureKind::Nexus => (&NEXUS, UnitKind::Nexus),
            };
            let _ = self.world.insert(entity, Position(structure.position));
            let _ = self.world.insert(entity, structure.team);
            let _ = self.world.insert(entity, kind);
            let _ = self.world.insert(
                entity,
                Health {
                    current: stats.max_health,
                    max: stats.max_health,
                },
            );
            let _ = self.world.insert(
                entity,
                AttackStats::new(
                    stats.attack_damage,
                    stats.attack_range,
                    stats.attack_interval_ticks,
                ),
            );
            let _ = self.world.insert(
                entity,
                Resistances {
                    armor: stats.armor,
                    magic_resist: stats.magic_resist,
                },
            );
            let _ = self.world.insert(entity, AttackTarget::default());
            let _ = self.world.insert(entity, VisionRange(stats.vision_range));
            let _ = self
                .world
                .insert(entity, AttackDamageType(DamageType::Physical));
            let _ = self.world.insert(entity, MoveTarget::default());
        }
    }

    fn spawn_hero(&mut self, slot: usize) -> Entity {
        let team = if slot < 5 { Team::Blue } else { Team::Red };
        let pick = usize::from(self.config.hero_picks[slot]) % HEROES.len();
        let hero_id = HeroId(u8::try_from(pick).expect("hero index fits u8"));
        let def = hero_id.def();
        let entity = self.world.spawn();
        let _ = self.world.insert(
            entity,
            Position(MAP.hero_spawn(team, u8::try_from(slot % 5).expect("slot fits u8"))),
        );
        let _ = self.world.insert(entity, team);
        let _ = self.world.insert(entity, UnitKind::Hero);
        let _ = self.world.insert(
            entity,
            Health {
                current: def.max_health,
                max: def.max_health,
            },
        );
        let _ = self.world.insert(
            entity,
            Mana {
                current: def.max_mana,
                max: def.max_mana,
            },
        );
        let _ = self.world.insert(
            entity,
            AttackStats::new(
                def.attack_damage,
                def.attack_range,
                def.attack_interval_ticks,
            ),
        );
        let _ = self.world.insert(
            entity,
            Resistances {
                armor: def.armor,
                magic_resist: def.magic_resist,
            },
        );
        let _ = self
            .world
            .insert(entity, MoveSpeed(def.move_speed_per_sec * ONE_TICK));
        let _ = self.world.insert(entity, MoveTarget::default());
        let _ = self.world.insert(entity, AttackTarget::default());
        let _ = self
            .world
            .insert(entity, HeroSlot(u8::try_from(slot).expect("slot fits u8")));
        let _ = self.world.insert(entity, HeroIdentity(hero_id));
        let _ = self.world.insert(entity, Cooldowns::default());
        let _ = self.world.insert(
            entity,
            Regen {
                health: def.health_regen_per_tick,
                mana: def.mana_regen_per_tick,
            },
        );
        let _ = self.world.insert(entity, VisionRange(def.vision_range));
        let _ = self
            .world
            .insert(entity, AttackDamageType(DamageType::Physical));
        entity
    }

    fn spawn_minion(
        &mut self,
        team: Team,
        lane: usize,
        kind: UnitKind,
        position: FixVec2,
        waypoint: u16,
    ) -> Option<Entity> {
        if self.world.entity_count() >= MAX_UNITS {
            return None;
        }
        let def = if kind == UnitKind::MeleeMinion {
            &MELEE_MINION
        } else {
            &RANGED_MINION
        };
        let entity = self.world.spawn();
        let _ = self.world.insert(entity, Position(position));
        let _ = self.world.insert(entity, team);
        let _ = self.world.insert(entity, kind);
        let _ = self.world.insert(
            entity,
            Health {
                current: def.max_health,
                max: def.max_health,
            },
        );
        let _ = self.world.insert(
            entity,
            AttackStats::new(
                def.attack_damage,
                def.attack_range,
                def.attack_interval_ticks,
            ),
        );
        let _ = self.world.insert(
            entity,
            Resistances {
                armor: def.armor,
                magic_resist: def.magic_resist,
            },
        );
        let _ = self
            .world
            .insert(entity, MoveSpeed(def.move_speed_per_sec * ONE_TICK));
        let _ = self.world.insert(entity, MoveTarget::default());
        let _ = self.world.insert(entity, AttackTarget::default());
        let _ = self.world.insert(
            entity,
            LaneWalker {
                lane: u8::try_from(lane).expect("lane fits u8"),
                waypoint,
            },
        );
        let _ = self.world.insert(entity, VisionRange(def.vision_range));
        let _ = self
            .world
            .insert(entity, AttackDamageType(DamageType::Physical));
        Some(entity)
    }

    fn spawn_wave(&mut self) {
        self.wave = self.wave.saturating_add(1);
        for team in Team::ALL {
            for (lane_index, lane) in MAP.lanes.iter().enumerate() {
                let start = lane.point(team, 0);
                let direction = (lane.point(team, 1) - start).normalize_or_zero();
                let perpendicular = direction.perpendicular();
                for index in 0..3i32 {
                    let offset = Fix::from_i32(index - 1);
                    let position = clamp_position(
                        start
                            + perpendicular * offset * Fix::from_i32(2)
                            + direction * Fix::from_i32(-1),
                    );
                    self.spawn_minion(team, lane_index, UnitKind::MeleeMinion, position, 1);
                }
                let position = clamp_position(start + perpendicular * Fix::from_i32(6));
                self.spawn_minion(team, lane_index, UnitKind::RangedMinion, position, 1);
            }
        }
        self.push_event(SimEvent::WaveSpawned { wave: self.wave });
    }

    fn tick_timers(&mut self) {
        for (_, cooldowns) in self.world.query::<&mut Cooldowns>() {
            for cooldown in &mut cooldowns.0 {
                *cooldown = cooldown.saturating_sub(1);
            }
        }
        for (_, stats) in self.world.query::<&mut AttackStats>() {
            stats.cooldown = stats.cooldown.saturating_sub(1);
        }
        for (_, (health, mana, regen)) in self
            .world
            .query_filtered::<(&mut Health, &mut Mana, &Regen), Without<Dead>>()
        {
            // A hero that reached zero this tick is already dead; regeneration must not
            // revive it before the death pass runs.
            if health.current > 0 {
                health.current = (health.current + regen.health).min(health.max);
                mana.current = (mana.current + regen.mana).min(mana.max);
            }
        }

        self.respawns.clear();
        for (entity, dead) in self.world.query::<&mut Dead>() {
            dead.respawn_in = dead.respawn_in.saturating_sub(1);
            if dead.respawn_in == 0 {
                self.respawns.push(entity);
            }
        }
        let mut respawns = core::mem::take(&mut self.respawns);
        for entity in respawns.drain(..) {
            self.respawn_hero(entity);
        }
        self.respawns = respawns;
    }

    fn respawn_hero(&mut self, entity: Entity) {
        let Some(team) = self.world.get::<Team>(entity).copied() else {
            return;
        };
        let Some(slot) = self.world.get::<HeroSlot>(entity).map(|slot| slot.0) else {
            return;
        };
        let _ = self.world.remove::<Dead>(entity);
        if let Some(position) = self.world.get_mut::<Position>(entity) {
            position.0 = MAP.hero_spawn(team, slot % 5);
        }
        if let Some(health) = self.world.get_mut::<Health>(entity) {
            health.current = health.max;
        }
        if let Some(mana) = self.world.get_mut::<Mana>(entity) {
            mana.current = mana.max;
        }
        if let Some(cooldowns) = self.world.get_mut::<Cooldowns>(entity) {
            cooldowns.0 = [0; 3];
        }
        if let Some(stats) = self.world.get_mut::<AttackStats>(entity) {
            stats.cooldown = 0;
        }
        if let Some(target) = self.world.get_mut::<MoveTarget>(entity) {
            target.0 = None;
        }
        if let Some(target) = self.world.get_mut::<AttackTarget>(entity) {
            target.0 = None;
        }
        self.push_event(SimEvent::HeroRespawned { entity });
    }

    fn apply_inputs(&mut self, inputs: &TickInputs) {
        for (slot, command) in inputs.players.iter().enumerate() {
            let Some(command) = command else {
                continue;
            };
            let entity = self.hero_entities[slot];
            if self.world.has::<Dead>(entity) {
                continue;
            }
            match *command {
                PlayerCommand::Stop => {
                    if let Some(target) = self.world.get_mut::<MoveTarget>(entity) {
                        target.0 = None;
                    }
                    if let Some(target) = self.world.get_mut::<AttackTarget>(entity) {
                        target.0 = None;
                    }
                }
                PlayerCommand::MoveTo(position) => {
                    let destination = clamp_position(position);
                    if let Some(target) = self.world.get_mut::<MoveTarget>(entity) {
                        target.0 = Some(destination);
                    }
                    if let Some(target) = self.world.get_mut::<AttackTarget>(entity) {
                        target.0 = None;
                    }
                }
                PlayerCommand::Attack(target) => {
                    if let Some(order) = self.world.get_mut::<AttackTarget>(entity) {
                        order.0 = Some(target);
                    }
                    if let Some(order) = self.world.get_mut::<MoveTarget>(entity) {
                        order.0 = None;
                    }
                }
                PlayerCommand::CastAbility {
                    slot: ability,
                    target,
                } => {
                    self.cast_ability(entity, ability, target);
                }
            }
        }
    }

    #[allow(clippy::too_many_lines)] // Ability execution is one validated match.
    fn cast_ability(&mut self, caster: Entity, slot: u8, target: FixVec2) {
        let Some(identity) = self.world.get::<HeroIdentity>(caster).copied() else {
            return;
        };
        let def = identity.0.def();
        let ability_index = usize::from(slot);
        let Some(ability) = def.ability(ability_index) else {
            return;
        };
        let Some(cooldowns) = self.world.get::<Cooldowns>(caster).copied() else {
            return;
        };
        if cooldowns.0[ability_index] > 0 {
            return;
        }
        let Some(mana) = self.world.get::<Mana>(caster).copied() else {
            return;
        };
        if mana.current < ability.mana_cost {
            return;
        }
        let Some(position) = self
            .world
            .get::<Position>(caster)
            .map(|position| position.0)
        else {
            return;
        };
        let Some(team) = self.world.get::<Team>(caster).copied() else {
            return;
        };

        let is_directed = matches!(
            ability.kind,
            moba_game::AbilityKind::Skillshot { .. } | moba_game::AbilityKind::Dash { .. }
        );
        if is_directed {
            let distance = position.distance(target);
            if distance == Fix::ZERO || distance > ability.cast_range {
                return;
            }
        }

        match ability.kind {
            moba_game::AbilityKind::Skillshot {
                speed_per_sec,
                range,
                radius,
                damage,
                damage_type,
            } => {
                let direction = (target - position).normalize_or_zero();
                if direction == FixVec2::ZERO {
                    return;
                }
                if self.projectiles.len() >= MAX_PROJECTILES {
                    return;
                }
                self.projectiles.push(Projectile {
                    owner: caster,
                    team,
                    position,
                    direction,
                    speed_per_tick: speed_per_sec * ONE_TICK,
                    remaining: range,
                    radius,
                    damage,
                    damage_type,
                });
            }
            moba_game::AbilityKind::Dash { range } => {
                let delta = target - position;
                let distance = delta.length().min(range);
                if distance > Fix::ZERO {
                    let destination =
                        clamp_position(position + delta.normalize_or_zero() * distance);
                    if let Some(current) = self.world.get_mut::<Position>(caster) {
                        current.0 = destination;
                    }
                }
            }
            moba_game::AbilityKind::Heal { amount, .. } => {
                if let Some(health) = self.world.get_mut::<Health>(caster) {
                    health.current = (health.current + amount).min(health.max);
                }
            }
            moba_game::AbilityKind::Burst {
                radius,
                damage,
                damage_type,
            } => {
                self.damage_hits.clear();
                for (entity, (other_position, other_team)) in
                    self.world.query::<(&Position, &Team)>()
                {
                    if *other_team == team
                        || other_position.0.distance_sq(position) > radius * radius
                    {
                        continue;
                    }
                    self.damage_hits.push(entity);
                }
                let mut hits = core::mem::take(&mut self.damage_hits);
                for target in hits.drain(..) {
                    self.damage(target, damage, damage_type);
                }
                self.damage_hits = hits;
            }
        }

        if let Some(mana) = self.world.get_mut::<Mana>(caster) {
            mana.current = (mana.current - ability.mana_cost).max(0);
        }
        if let Some(cooldowns) = self.world.get_mut::<Cooldowns>(caster) {
            cooldowns.0[ability_index] = ability.cooldown_ticks;
        }
    }

    fn update_minion_targets(&mut self) {
        let grid = &self.grid;
        let map = MAP;
        for (_, (position, team, stats, attack_target, move_target, walker)) in
            self.world.query_filtered::<(
                &Position,
                &Team,
                &AttackStats,
                &mut AttackTarget,
                &mut MoveTarget,
                &mut LaneWalker,
            ), (With<LaneWalker>, Without<Dead>)>()
        {
            let position = position.0;
            let team = *team;
            let mut best_unit: Option<(Fix, Entity)> = None;
            let mut best_structure: Option<(Fix, Entity)> = None;
            let search_radius = stats.range.max(MINION_AGGRO);
            grid.for_each_in_circle(position, search_radius, |candidate| {
                if candidate.team == team {
                    return;
                }
                let distance = candidate.position.distance_sq(position);
                if candidate.kind.is_structure() {
                    if distance <= stats.range_sq
                        && best_structure.is_none_or(|(best, _)| distance < best)
                    {
                        best_structure = Some((distance, candidate.entity));
                    }
                } else if distance <= MINION_AGGRO_SQ
                    && best_unit.is_none_or(|(best, _)| distance < best)
                {
                    best_unit = Some((distance, candidate.entity));
                }
            });

            if let Some((_, target)) = best_unit {
                attack_target.0 = Some(target);
                move_target.0 = None;
                continue;
            }
            if let Some((_, target)) = best_structure {
                attack_target.0 = Some(target);
                move_target.0 = None;
                continue;
            }

            attack_target.0 = None;
            let lane = map.lanes[usize::from(walker.lane).min(map.lanes.len() - 1)];
            let mut waypoint = usize::from(walker.waypoint).min(lane.len() - 1);
            while waypoint < lane.len() - 1
                && position.distance(lane.point(team, waypoint)) <= WAYPOINT_REACH
            {
                waypoint += 1;
            }
            walker.waypoint = u16::try_from(waypoint).expect("waypoint fits u16");
            let destination = if waypoint >= lane.len() - 1 {
                map.nexus_position(team.enemy())
            } else {
                lane.point(team, waypoint)
            };
            move_target.0 = Some(destination);
        }
    }

    fn update_structure_targets(&mut self) {
        let grid = &self.grid;
        for (_, (position, team, stats, kind, attack_target)) in
            self.world
                .query::<(&Position, &Team, &AttackStats, &UnitKind, &mut AttackTarget)>()
        {
            if !kind.is_structure() {
                continue;
            }
            let position = position.0;
            let team = *team;
            let mut best_minion: Option<(Fix, Entity)> = None;
            let mut best_hero: Option<(Fix, Entity)> = None;
            grid.for_each_in_circle(position, stats.range, |candidate| {
                if candidate.team == team {
                    return;
                }
                let distance = candidate.position.distance_sq(position);
                if distance > stats.range_sq {
                    return;
                }
                if candidate.kind.is_minion() {
                    if best_minion.is_none_or(|(best, _)| distance < best) {
                        best_minion = Some((distance, candidate.entity));
                    }
                } else if candidate.kind.is_hero()
                    && best_hero.is_none_or(|(best, _)| distance < best)
                {
                    best_hero = Some((distance, candidate.entity));
                }
            });
            attack_target.0 = best_minion.or(best_hero).map(|(_, entity)| entity);
        }
    }

    fn move_units(&mut self) {
        for (_, (position, target, speed)) in self
            .world
            .query_filtered::<(&mut Position, &MoveTarget, &MoveSpeed), Without<Dead>>()
        {
            let Some(destination) = target.0 else {
                continue;
            };
            let next = position.0.move_towards(destination, speed.0);
            position.0 = clamp_position(next);
        }
    }

    fn resolve_attacks(&mut self) {
        self.attackers.clear();
        for (attacker, (stats, target)) in self.world.query::<(&AttackStats, &AttackTarget)>() {
            if stats.cooldown > 0 {
                continue;
            }
            let Some(target) = target.0 else {
                continue;
            };
            self.attackers.push(AttackerSnapshot {
                attacker,
                target,
                damage: stats.damage,
                range_sq: stats.range_sq,
                interval: stats.interval,
            });
        }
        let attackers = core::mem::take(&mut self.attackers);
        for snapshot in attackers.iter().copied() {
            if self.world.has::<Dead>(snapshot.attacker) {
                continue;
            }
            let Some(attacker_position) = self
                .world
                .get::<Position>(snapshot.attacker)
                .map(|position| position.0)
            else {
                continue;
            };
            let Some(target_position) = self
                .world
                .get::<Position>(snapshot.target)
                .map(|position| position.0)
            else {
                continue;
            };
            let Some(target_health) = self.world.get::<Health>(snapshot.target) else {
                continue;
            };
            if target_health.current <= 0 {
                continue;
            }
            let attacker_team = self.world.get::<Team>(snapshot.attacker).copied();
            let target_team = self.world.get::<Team>(snapshot.target).copied();
            if attacker_team == target_team {
                continue;
            }
            if attacker_position.distance_sq(target_position) > snapshot.range_sq {
                continue;
            }
            self.damage(snapshot.target, snapshot.damage, DamageType::Physical);
            if let Some(stats) = self.world.get_mut::<AttackStats>(snapshot.attacker) {
                stats.cooldown = snapshot.interval;
            }
        }
        self.attackers = attackers;
    }

    fn update_projectiles(&mut self) {
        let mut index = 0;
        while index < self.projectiles.len() {
            let mut projectile = self.projectiles[index];
            projectile.position += projectile.direction * projectile.speed_per_tick;
            projectile.remaining -= projectile.speed_per_tick;
            if projectile.remaining <= Fix::ZERO {
                self.projectiles.swap_remove(index);
                continue;
            }
            let mut hit: Option<(Fix, Entity)> = None;
            self.grid
                .for_each_in_circle(projectile.position, projectile.radius, |candidate| {
                    if candidate.team == projectile.team {
                        return;
                    }
                    let distance = candidate.position.distance_sq(projectile.position);
                    if hit.is_none_or(|(best, entity)| {
                        distance < best || (distance == best && candidate.entity < entity)
                    }) {
                        hit = Some((distance, candidate.entity));
                    }
                });
            if let Some((_, target)) = hit {
                self.damage(target, projectile.damage, projectile.damage_type);
                self.projectiles.swap_remove(index);
            } else {
                self.projectiles[index] = projectile;
                index += 1;
            }
        }
    }

    fn resolve_deaths(&mut self) {
        let mut deaths = core::mem::take(&mut self.deaths);
        deaths.clear();
        for (entity, (health, team, kind)) in self.world.query::<(&Health, &Team, &UnitKind)>() {
            if health.current <= 0 {
                deaths.push((entity, *team, *kind));
            }
        }

        for (entity, team, kind) in deaths.iter().copied() {
            if kind.is_hero() {
                if self.world.has::<Dead>(entity) {
                    continue;
                }
                let _ = self.world.insert(
                    entity,
                    Dead {
                        respawn_in: HERO_RESPAWN_TICKS,
                    },
                );
                if let Some(target) = self.world.get_mut::<MoveTarget>(entity) {
                    target.0 = None;
                }
                if let Some(target) = self.world.get_mut::<AttackTarget>(entity) {
                    target.0 = None;
                }
                self.push_event(SimEvent::UnitDied { entity, kind, team });
                continue;
            }

            self.world.despawn(entity);
            self.push_event(SimEvent::UnitDied { entity, kind, team });
            if kind.is_structure() {
                self.push_event(SimEvent::StructureDestroyed { team, kind });
                if kind == UnitKind::Nexus && self.outcome.is_none() {
                    self.outcome = Some(team.enemy());
                    self.push_event(SimEvent::MatchEnded {
                        winner: team.enemy(),
                    });
                }
            }
        }
        self.deaths = deaths;
    }

    fn damage(&mut self, target: Entity, amount: i32, damage_type: DamageType) {
        let Some(health) = self.world.get::<Health>(target).copied() else {
            return;
        };
        if health.current <= 0 {
            return;
        }
        let resistances = self
            .world
            .get::<Resistances>(target)
            .copied()
            .unwrap_or(Resistances {
                armor: 0,
                magic_resist: 0,
            });
        let dealt = mitigate_typed(
            amount,
            damage_type,
            resistances.armor,
            resistances.magic_resist,
        );
        if let Some(health) = self.world.get_mut::<Health>(target) {
            health.current = (health.current - dealt).max(0);
        }
    }

    fn refresh_spatial(&mut self) {
        let grid = &mut self.grid;
        grid.clear();
        for (entity, (position, team, health, kind)) in
            self.world.query::<(&Position, &Team, &Health, &UnitKind)>()
        {
            if health.current <= 0 {
                continue;
            }
            grid.insert(
                position.0,
                TargetRef {
                    entity,
                    team: *team,
                    kind: *kind,
                    position: position.0,
                },
            );
        }
    }

    fn push_event(&mut self, event: SimEvent) {
        if self.events.len() < MAX_EVENTS {
            self.events.push(event);
        }
    }

    /// Deterministic bot AI. Returns one command per bot-controlled slot.
    #[must_use]
    pub fn bot_inputs(&mut self) -> TickInputs {
        self.refresh_spatial();
        self.hero_views.clear();
        for (_entity, (slot, team, position, health, mana, cooldowns, identity, stats)) in
            self.world.query_filtered::<(
                &HeroSlot,
                &Team,
                &Position,
                &Health,
                &Mana,
                &Cooldowns,
                &HeroIdentity,
                &AttackStats,
            ), Without<Dead>>()
        {
            self.hero_views.push(HeroView {
                slot: slot.0,
                team: *team,
                position: position.0,
                health: health.current,
                mana: mana.current,
                cooldowns: *cooldowns,
                attack_range_sq: stats.range_sq,
                def: identity.0.def(),
            });
        }

        let mut inputs = TickInputs::new();
        let view_count = self.hero_views.len();
        for index in 0..view_count {
            let view = self.hero_views[index];
            let slot = usize::from(view.slot);
            if self.config.bot_mask & (1 << slot) == 0 {
                continue;
            }
            let enemy_nexus = MAP.nexus_position(view.team.enemy());
            let hero_target =
                self.nearest_enemy(view.position, view.team, Fix::from_i32(13), |kind| {
                    kind.is_hero()
                });
            let minion_target =
                self.nearest_enemy(view.position, view.team, Fix::from_i32(10), |kind| {
                    kind.is_minion()
                });
            let structure_target =
                self.nearest_enemy(view.position, view.team, Fix::from_i32(200), |kind| {
                    kind.is_structure()
                });
            let target = hero_target.or(minion_target).or(structure_target);

            let command = if let Some(target) = target {
                let distance_sq = view.position.distance_sq(target.position);
                let defensive = view.health * 100 <= view.def.max_health * 45;
                if defensive && view.cooldowns.0[1] == 0 {
                    if let Some(ability) = view.def.ability(1) {
                        if view.mana >= ability.mana_cost {
                            Some(PlayerCommand::CastAbility {
                                slot: 1,
                                target: view.position,
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else if view.cooldowns.0[0] == 0
                    && view.def.ability(0).is_some_and(|ability| {
                        view.mana >= ability.mana_cost
                            && distance_sq <= ability.cast_range * ability.cast_range
                    })
                {
                    Some(PlayerCommand::CastAbility {
                        slot: 0,
                        target: target.position,
                    })
                } else if distance_sq > view.attack_range_sq
                    && distance_sq <= Fix::from_i32(12) * Fix::from_i32(12)
                    && view.cooldowns.0[2] == 0
                    && view.def.ability(2).is_some_and(|ability| {
                        view.mana >= ability.mana_cost
                            && distance_sq <= ability.cast_range * ability.cast_range
                    })
                {
                    Some(PlayerCommand::CastAbility {
                        slot: 2,
                        target: target.position,
                    })
                } else if distance_sq <= view.attack_range_sq {
                    Some(PlayerCommand::Attack(target.entity))
                } else {
                    Some(PlayerCommand::MoveTo(target.position))
                }
            } else {
                Some(PlayerCommand::MoveTo(enemy_nexus))
            };
            inputs.set(slot, command.unwrap_or(PlayerCommand::Stop));
        }
        inputs
    }

    fn nearest_enemy(
        &self,
        position: FixVec2,
        team: Team,
        radius: Fix,
        filter: impl Fn(UnitKind) -> bool,
    ) -> Option<TargetRef> {
        let mut best: Option<(Fix, TargetRef)> = None;
        self.grid.for_each_in_circle(position, radius, |candidate| {
            if candidate.team == team || !filter(candidate.kind) {
                return;
            }
            let distance = candidate.position.distance_sq(position);
            if best.as_ref().is_none_or(|(best_distance, best_ref)| {
                distance < *best_distance
                    || (distance == *best_distance && candidate.entity < best_ref.entity)
            }) {
                best = Some((distance, candidate));
            }
        });
        best.map(|(_, target)| target)
    }
}

fn unit_in_vision(position: FixVec2, sources: &[(FixVec2, Fix)]) -> bool {
    sources
        .iter()
        .any(|&(source, range)| position.distance_sq(source) <= range * range)
}

fn clamp_position(position: FixVec2) -> FixVec2 {
    let min = Fix::from_i32(2);
    let max = Fix::from_i32(WORLD_EXTENT - 2);
    FixVec2::new(position.x.clamp(min, max), position.y.clamp(min, max))
}

fn hash_team(world: &World, entity: Entity, hasher: &mut StableHasher) {
    if let Some(team) = world.get::<Team>(entity) {
        hasher.write_u32(11);
        hasher.write_u32(team.index() as u32);
    }
}

fn hash_kind(world: &World, entity: Entity, hasher: &mut StableHasher) {
    if let Some(kind) = world.get::<UnitKind>(entity) {
        hasher.write_u32(12);
        hasher.write_u32(*kind as u32);
    }
}

impl fmt::Debug for Simulation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Simulation")
            .field("tick", &self.tick)
            .field("wave", &self.wave)
            .field("units", &self.world.entity_count())
            .field("projectiles", &self.projectiles.len())
            .field("outcome", &self.outcome)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle(picks: [u8; MAX_PLAYERS]) -> Simulation {
        Simulation::new(SimConfig {
            bot_mask: 0,
            spawn_waves: false,
            hero_picks: picks,
            ..SimConfig::default()
        })
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
        assert_eq!(sim.outcome(), Some(Team::Blue));
        assert!(
            sim.events()
                .iter()
                .any(|event| matches!(event, SimEvent::MatchEnded { winner: Team::Blue }))
        );
        assert!(!sim.world.entities().any(|entity| entity == nexus));
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
                kind: UnitKind::Tower
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
}
