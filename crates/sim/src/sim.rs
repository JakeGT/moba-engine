//! Headless fixed-tick MOBA simulation.

use core::fmt;
use std::sync::Arc;

use moba_core::{Fix, FixVec2, Rng, StableHasher, Tick};
use moba_ecs::{Entity, With, Without, World};
use moba_game::{
    AreaEffect, AreaEffectList, DamageType, EMPTY_ITEM, Effect, GameContent, HeroId, LEVEL_GROWTH,
    ModifierKind, Targeting, Team, mitigate_typed,
};
use moba_nav::SpatialGrid;

use crate::components::{
    AttackDamageType, AttackStats, AttackTarget, BaseMoveSpeed, Cooldowns, Dead, Health,
    HeroIdentity, HeroSlot, ItemCooldowns, LaneWalker, LastHitBy, MAX_MODIFIERS, Mana, Modifier,
    Modifiers, MoveSpeed, MoveTarget, Position, Progression, Regen, Resistances, UnitKind,
    VisionRange,
};

/// Number of player slots (5v5).
pub const MAX_PLAYERS: usize = 10;
/// Hard cap on live entities. Waves stop when the cap is reached.
pub const MAX_UNITS: usize = 512;
const HASH_POSITION: u32 = 1;
const HASH_HEALTH: u32 = 2;
const HASH_MANA: u32 = 3;
const HASH_ATTACK_STATS: u32 = 4;
const HASH_ATTACK_TARGET: u32 = 5;
const HASH_MOVE_TARGET: u32 = 6;
const HASH_COOLDOWNS: u32 = 7;
const HASH_LANE_WALKER: u32 = 8;
const HASH_DEAD: u32 = 9;
const HASH_HERO_IDENTITY: u32 = 10;
const HASH_HERO_SLOT: u32 = 11;
const HASH_PROGRESSION: u32 = 12;
const HASH_MODIFIERS: u32 = 13;
const HASH_TEAM: u32 = 19;
const HASH_KIND: u32 = 20;
const HASH_REGEN: u32 = 21;
const HASH_RESISTANCES: u32 = 22;
const HASH_VISION: u32 = 23;
const HASH_MOVE_SPEED: u32 = 24;
const HASH_BASE_MOVE_SPEED: u32 = 25;
const HASH_LAST_HIT_BY: u32 = 26;
const HASH_ATTACK_DAMAGE_TYPE: u32 = 27;
const HASH_PROJECTILE: u32 = 28;
const HASH_ZONE: u32 = 29;
const HASH_ITEM_COOLDOWNS: u32 = 30;

/// Hard cap on in-flight projectiles.
pub const MAX_PROJECTILES: usize = 64;
/// Hard cap on active zones.
pub const MAX_ZONES: usize = 16;
/// Hard cap on queued events; events beyond this are dropped, never allocated.
pub const MAX_EVENTS: usize = 256;
/// Ticks between minion waves (30 seconds at 30 Hz).
pub const WAVE_INTERVAL_TICKS: u32 = 900;
/// First wave delay (1 second).
pub const WAVE_DELAY_TICKS: u32 = 30;
/// Hero respawn delay (10 seconds).
pub const HERO_RESPAWN_TICKS: u32 = 300;

/// Fixed-point duration of one tick (`1/30` second).
pub const TICK_DURATION: Fix = Fix::from_bits(143_165_576);
const ONE_TICK: Fix = TICK_DURATION;
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
            hero_picks: [0, 1, 2, 3, 4, 0, 1, 2, 3, 4],
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
    /// Purchases an item from the catalogue (must be near the team nexus).
    BuyItem(u8),
    /// Uses the active on inventory slot `0..6`.
    UseItem(u8),
    /// Concedes the match for the sender's team.
    Surrender,
}

/// One tick of input for every player.
#[derive(Copy, Clone, Debug, Default)]
pub struct TickInputs {
    /// Slot-indexed commands; `None` keeps the current order.
    pub players: [Option<PlayerCommand>; MAX_PLAYERS],
    /// Slot-indexed purchases. Separate from `players` so a move and a buy can happen in
    /// the same tick and both are recorded in replays.
    pub purchases: [Option<u8>; MAX_PLAYERS],
}

impl TickInputs {
    /// Creates empty inputs.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            players: [None; MAX_PLAYERS],
            purchases: [None; MAX_PLAYERS],
        }
    }

    /// Sets one player's command, ignoring out-of-range slots.
    pub fn set(&mut self, slot: usize, command: PlayerCommand) {
        if slot < MAX_PLAYERS {
            self.players[slot] = Some(command);
        }
    }

    /// Queues a purchase for one player alongside any movement/cast command.
    pub fn set_purchase(&mut self, slot: usize, item: u8) {
        if slot < MAX_PLAYERS {
            self.purchases[slot] = Some(item);
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
        /// Death position.
        position: FixVec2,
        /// Last hitter, when known.
        killer: Option<Entity>,
        /// Victim hero slot, when applicable.
        hero_slot: Option<u8>,
        /// Killer hero slot, when applicable.
        killer_slot: Option<u8>,
    },
    /// A tower or nexus was destroyed.
    StructureDestroyed {
        /// Owning team.
        team: Team,
        /// Structure type.
        kind: UnitKind,
        /// Structure position.
        position: FixVec2,
    },
    /// A hero respawned.
    HeroRespawned {
        /// Respawned entity.
        entity: Entity,
    },
    /// An ability passed validation and is being resolved.
    AbilityCast {
        /// Casting entity.
        caster: Entity,
        /// Casting team.
        team: Team,
        /// Ability slot (0..=2).
        ability: u8,
        /// Resolved target point.
        target: FixVec2,
    },
    /// A projectile was spawned.
    ProjectileSpawned {
        /// Owning entity.
        owner: Entity,
        /// Owning team.
        team: Team,
        /// Spawn position.
        position: FixVec2,
        /// Unit direction.
        direction: FixVec2,
        /// Travel speed per second.
        speed_per_sec: Fix,
        /// Collision radius.
        radius: Fix,
        /// Visual arc height.
        arc: Fix,
        /// Maximum travel distance.
        range: Fix,
    },
    /// A projectile hit an enemy.
    ProjectileHit {
        /// Owning entity.
        owner: Entity,
        /// Owning team.
        team: Team,
        /// Hit entity.
        target: Entity,
        /// Impact position.
        position: FixVec2,
    },
    /// A periodic zone was spawned.
    ZoneSpawned {
        /// Owning entity.
        owner: Entity,
        /// Owning team.
        team: Team,
        /// Zone centre.
        position: FixVec2,
        /// Zone radius.
        radius: Fix,
        /// Lifetime in ticks.
        duration_ticks: u32,
    },
    /// A nexus fell and the match ended.
    MatchEnded {
        /// Winning team.
        winner: Team,
    },
}

#[derive(Copy, Clone, Debug)]
struct VisionSource {
    position: FixVec2,
    range: Fix,
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
    arc: Fix,
    on_hit: AreaEffectList,
}

#[derive(Copy, Clone, Debug)]
struct Zone {
    owner: Entity,
    team: Team,
    position: FixVec2,
    radius: Fix,
    remaining: u32,
    period_ticks: u32,
    cooldown: u32,
    effects: AreaEffectList,
}

#[derive(Copy, Clone, Debug)]
struct ObservedCandidate {
    entity: Entity,
    team: Team,
    kind: UnitKind,
    position: FixVec2,
    health: i32,
    max_health: i32,
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
    max_health: i32,
    ability_mana: [i32; 3],
    ability_range: [Fix; 3],
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
#[allow(clippy::struct_excessive_bools)] // Independent per-entity status flags.
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
    /// Hero definition index for heroes (`0xFF` otherwise).
    pub hero_pick: u8,
    /// Remaining Q/W/E cooldowns for heroes.
    pub cooldowns: [u32; 3],
    /// Hero level (zero for non-heroes).
    pub level: u8,
    /// Experience towards the next level.
    pub xp: u32,
    /// Current gold (only meaningful for the viewing player's hero).
    pub gold: u32,
    /// Inventory item ids (`0xFF` empty).
    pub items: [u8; 6],
    /// Whether a temporary shield is active.
    pub shielded: bool,
    /// Whether a movement slow is active.
    pub slowed: bool,
    /// Whether the unit is rooted.
    pub rooted: bool,
    /// Whether the unit is silenced.
    pub silenced: bool,
    /// Whether the unit is burning.
    pub burning: bool,
    /// Whether a movement haste is active.
    pub hasted: bool,
    /// Generic replicated status bits from active modifiers.
    pub status_bits: u16,
}

/// Strategy that produces bot commands each tick.
///
/// Policies run deterministically inside the authoritative simulation and receive mutable
/// world access. Implementations must not consult wall-clock time or external state.
pub trait BotPolicy: Send + Sync {
    /// Produces one tick of inputs. Slots left `None` keep their current order.
    fn inputs(&mut self, simulation: &mut Simulation) -> TickInputs;
}

/// Built-in bot policy: lane assignment, farming, fighting and shopping.
#[derive(Copy, Clone, Debug, Default)]
pub struct DefaultBotPolicy;

impl BotPolicy for DefaultBotPolicy {
    fn inputs(&mut self, simulation: &mut Simulation) -> TickInputs {
        simulation.default_bot_inputs()
    }
}

/// The authoritative MOBA simulation.
pub struct Simulation {
    world: World,
    tick: Tick,
    config: SimConfig,
    content: Arc<GameContent>,
    hero_entities: [Entity; MAX_PLAYERS],
    wave: u32,
    next_wave_tick: u32,
    outcome: Option<Team>,
    blue_score: u32,
    red_score: u32,
    projectiles: Vec<Projectile>,
    zones: Vec<Zone>,
    events: Vec<SimEvent>,
    grid: SpatialGrid<TargetRef>,
    vision_grid: Option<SpatialGrid<VisionSource>>,
    hero_views: Vec<HeroView>,
    attackers: Vec<AttackerSnapshot>,
    deaths: Vec<(Entity, Team, UnitKind, FixVec2)>,
    respawns: Vec<Entity>,
    damage_hits: Vec<Entity>,
    modifier_entities: Vec<Entity>,
    burn_hits: Vec<(Entity, i32, DamageType, Entity)>,
    observed_candidates: Vec<ObservedCandidate>,
    bot_observed: [Vec<ObservedUnit>; 2],
    bot_sources: Vec<(FixVec2, Fix)>,
    bot_policy: Option<Box<dyn BotPolicy>>,
    bot_rng: [Rng; MAX_PLAYERS],
}

impl Simulation {
    /// Creates a fresh match.
    #[must_use]
    pub fn new(config: SimConfig) -> Self {
        Self::new_with_content(config, Arc::new(GameContent::default()))
    }

    /// Creates a fresh match with a validated runtime content pack.
    ///
    /// The caller is responsible for [`GameContent::validate`]; the server loads/validates
    /// content before creating the match.
    #[must_use]
    pub fn new_with_content(config: SimConfig, content: Arc<GameContent>) -> Self {
        let bot_seed = config.seed;
        let extent = content.map.extent;
        let rules = content.rules;
        let mut simulation = Self {
            world: World::with_capacity(MAX_UNITS),
            tick: Tick::ZERO,
            config,
            content,
            hero_entities: [Entity::PLACEHOLDER; MAX_PLAYERS],
            wave: 0,
            next_wave_tick: rules.first_wave_ticks,
            outcome: None,
            blue_score: 0,
            red_score: 0,
            projectiles: Vec::with_capacity(MAX_PROJECTILES),
            zones: Vec::with_capacity(MAX_ZONES),
            events: Vec::with_capacity(MAX_EVENTS),
            grid: SpatialGrid::new(FixVec2::ZERO, FixVec2::splat(extent), Fix::from_i32(8)),
            vision_grid: Some(SpatialGrid::new(
                FixVec2::ZERO,
                FixVec2::splat(extent),
                Fix::from_i32(8),
            )),
            hero_views: Vec::with_capacity(MAX_PLAYERS),
            attackers: Vec::with_capacity(MAX_UNITS),
            deaths: Vec::with_capacity(MAX_UNITS),
            respawns: Vec::with_capacity(MAX_PLAYERS),
            damage_hits: Vec::with_capacity(MAX_UNITS),
            modifier_entities: Vec::with_capacity(MAX_UNITS),
            burn_hits: Vec::with_capacity(MAX_UNITS),
            observed_candidates: Vec::with_capacity(MAX_UNITS),
            bot_observed: [Vec::with_capacity(MAX_UNITS), Vec::with_capacity(MAX_UNITS)],
            bot_sources: Vec::with_capacity(MAX_UNITS * 2),
            bot_policy: None,
            bot_rng: core::array::from_fn(|index| {
                Rng::from_seed(
                    bot_seed ^ (0xB0B0_0000_0000_0000 | index.wrapping_mul(0x9E37) as u64),
                )
            }),
        };
        simulation.spawn_structures();
        let active_players = usize::from(rules.team_size) * 2;
        for slot in 0..active_players.min(MAX_PLAYERS) {
            let entity = simulation.spawn_hero(slot);
            simulation.hero_entities[slot] = entity;
        }
        simulation.refresh_spatial();
        simulation
    }

    /// Content pack used by this match.
    #[inline]
    #[must_use]
    pub fn content(&self) -> &GameContent {
        &self.content
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

    /// Team hero-kill score as `(blue, red)`.
    #[inline]
    #[must_use]
    pub const fn score(&self) -> (u32, u32) {
        (self.blue_score, self.red_score)
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

    /// Mutable ECS access for embedding tools, custom modes and debug inspectors.
    ///
    /// Mutating simulation-owned components bypasses gameplay invariants and state hashing;
    /// prefer adding custom components that your own systems read. Change tracking still
    /// works, and the next tick will iterate whatever archetypes exist.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    fn clamp_position(&self, position: FixVec2) -> FixVec2 {
        clamp_to_extent(position, self.content.map.extent)
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
    pub fn observed_units(&mut self, viewer: Team, out: &mut Vec<ObservedUnit>) {
        self.observe(Some(viewer), out);
    }

    /// Whether `entity` is currently visible to `viewer` under the fog rules.
    pub fn is_visible_to(&mut self, viewer: Team, entity: Entity) -> bool {
        let mut observed = core::mem::take(&mut self.bot_observed[viewer.index()]);
        self.observe(Some(viewer), &mut observed);
        let visible = observed.iter().any(|unit| unit.entity == entity);
        self.bot_observed[viewer.index()] = observed;
        visible
    }

    /// Builds the complete unit list for a spectator (no fog filtering).
    pub fn observed_all(&mut self, out: &mut Vec<ObservedUnit>) {
        self.observe(None, out);
    }

    #[allow(clippy::too_many_lines)] // One fog pass keeps the visibility rules together.
    fn observe(&mut self, viewer: Option<Team>, out: &mut Vec<ObservedUnit>) {
        out.clear();
        let mut sources = [(FixVec2::ZERO, Fix::ZERO); MAX_UNITS];
        let mut source_count = 0usize;
        let mut max_vision = Fix::ZERO;
        if let Some(viewer) = viewer {
            for (_entity, (team, health, position, vision)) in
                self.world
                    .query::<(&Team, &Health, &Position, &VisionRange)>()
            {
                if *team != viewer || health.current <= 0 {
                    continue;
                }
                max_vision = max_vision.max(vision.0);
                if source_count < MAX_UNITS {
                    sources[source_count] = (position.0, vision.0);
                    source_count += 1;
                }
            }
        }

        let mut vision_grid = self.vision_grid.take().expect("vision grid");
        vision_grid.clear();
        for &(position, range) in &sources[..source_count] {
            vision_grid.insert(position, VisionSource { position, range });
        }
        self.observed_candidates.clear();
        for (entity, (team, kind, position, health)) in
            self.world.query::<(&Team, &UnitKind, &Position, &Health)>()
        {
            self.observed_candidates.push(ObservedCandidate {
                entity,
                team: *team,
                kind: *kind,
                position: position.0,
                health: health.current,
                max_health: health.max,
            });
        }
        let candidates = core::mem::take(&mut self.observed_candidates);
        for candidate in &candidates {
            let entity = candidate.entity;
            let team = candidate.team;
            let kind = candidate.kind;
            let position = candidate.position;
            let dead = candidate.health <= 0;
            let allied = viewer.is_some_and(|viewer| team == viewer);
            let visible = viewer.is_none()
                || allied
                || kind.is_structure()
                || (!dead
                    && vision_grid.any_in_circle(position, max_vision, |source| {
                        source.position.distance_sq(position) <= source.range * source.range
                            && self.content.map.walls.iter().all(|wall| {
                                !segments_intersect(source.position, position, wall.a, wall.b)
                            })
                    }));
            if !visible {
                continue;
            }
            let mana = self
                .world
                .get::<Mana>(entity)
                .map_or((0, 0), |mana| (mana.current, mana.max));
            let hero_slot = self.world.get::<HeroSlot>(entity).map(|slot| slot.0);
            let hero_pick = self
                .world
                .get::<HeroIdentity>(entity)
                .map_or(0xFF, |identity| identity.0.0);
            let progression = self.world.get::<Progression>(entity).copied();
            let modifiers = self.world.get::<Modifiers>(entity);
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
                kind,
                team,
                position,
                health: candidate.health,
                max_health: candidate.max_health,
                mana: mana.0,
                max_mana: mana.1,
                dead: dead && hero_slot.is_some(),
                hero_slot,
                hero_pick,
                cooldowns,
                level: progression.map_or(0, |progress| progress.level),
                xp: progression.map_or(0, |progress| progress.xp),
                gold: progression.map_or(0, |progress| progress.gold),
                items: progression.map_or([EMPTY_ITEM; 6], |progress| progress.items),
                shielded: modifiers.is_some_and(|m| m.shield_total() > 0),
                slowed: modifiers.is_some_and(|m| m.has(ModifierKind::Slow)),
                rooted: modifiers.is_some_and(|m| m.has(ModifierKind::Root)),
                silenced: modifiers.is_some_and(|m| m.has(ModifierKind::Silence)),
                burning: modifiers.is_some_and(|m| m.has(ModifierKind::Burn)),
                hasted: modifiers.is_some_and(|m| m.has(ModifierKind::Haste)),
                status_bits: modifiers.map_or(0, |modifiers| {
                    modifiers
                        .entries()
                        .iter()
                        .filter_map(|modifier| {
                            self.content.modifiers.get(usize::from(modifier.def_id))
                        })
                        .fold(0, |bits, definition| bits | definition.status_bits)
                }),
            });
        }
        self.observed_candidates = candidates;
        self.vision_grid = Some(vision_grid);
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
        if self.tick.get() % self.content.rules.gold_tick_interval == 0 {
            for hero in self.hero_entities {
                if self.world.has::<Dead>(hero) {
                    continue;
                }
                if let Some(progress) = self.world.get_mut::<Progression>(hero) {
                    progress.gold = progress
                        .gold
                        .saturating_add(self.content.rules.gold_per_interval);
                }
            }
        }
        self.tick_timers();
        // Refresh before casts so area queries see the current unit positions.
        self.refresh_spatial();
        self.apply_inputs(inputs);
        self.update_minion_targets();
        self.update_structure_targets();
        self.move_units();
        self.refresh_spatial();
        self.resolve_attacks();
        self.update_projectiles();
        self.tick_zones();
        self.resolve_deaths();
        if self.tick.get() >= self.next_wave_tick && self.config.spawn_waves {
            self.spawn_wave();
            self.next_wave_tick = self
                .next_wave_tick
                .saturating_add(self.content.rules.wave_interval_ticks);
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
    #[allow(clippy::too_many_lines)] // Deterministic field coverage is intentionally flat.
    pub fn state_hash(&self) -> u64 {
        let mut hasher = StableHasher::new();
        hasher.write_u64(self.config.seed);
        // `bot_mask` is a server-side input source, not simulation state. Recording it
        // only at the end of a replay made mid-match disconnects diverge; generated inputs
        // are already resolved and recorded per tick.
        hasher.write_u32(u32::from(self.config.spawn_waves));
        for pick in self.config.hero_picks {
            hasher.write_u32(u32::from(pick));
        }
        hasher.write_u32(self.tick.get());
        hasher.write_u32(self.wave);
        hasher.write_u32(self.next_wave_tick);
        hasher.write_u32(self.blue_score);
        hasher.write_u32(self.red_score);
        match self.outcome {
            Some(team) => hasher.write_u32(1 + team.index() as u32),
            None => hasher.write_u32(0),
        }

        for projectile in &self.projectiles {
            hasher.write_u32(HASH_PROJECTILE);
            hasher.write_u64(projectile.owner.to_bits());
            hasher.write_u32(projectile.team.index() as u32);
            hasher.write_u64(projectile.position.x.to_bits() as u64);
            hasher.write_u64(projectile.position.y.to_bits() as u64);
            hasher.write_u64(projectile.direction.x.to_bits() as u64);
            hasher.write_u64(projectile.direction.y.to_bits() as u64);
            hasher.write_u64(projectile.speed_per_tick.to_bits() as u64);
            hasher.write_u64(projectile.remaining.to_bits() as u64);
            hasher.write_u64(projectile.radius.to_bits() as u64);
            hasher.write_u64(projectile.arc.to_bits() as u64);
            hash_runtime_area_effects(&mut hasher, &projectile.on_hit);
        }
        for zone in &self.zones {
            hasher.write_u32(HASH_ZONE);
            hasher.write_u64(zone.owner.to_bits());
            hasher.write_u32(zone.team.index() as u32);
            hasher.write_u64(zone.position.x.to_bits() as u64);
            hasher.write_u64(zone.position.y.to_bits() as u64);
            hasher.write_u64(zone.radius.to_bits() as u64);
            hasher.write_u32(zone.remaining);
            hasher.write_u32(zone.period_ticks);
            hasher.write_u32(zone.cooldown);
            hash_runtime_area_effects(&mut hasher, &zone.effects);
        }

        for entity in self.world.entities() {
            hasher.write_u64(entity.to_bits());
            hash_team(&self.world, entity, &mut hasher);
            hash_kind(&self.world, entity, &mut hasher);
            if let Some(position) = self.world.get::<Position>(entity) {
                hasher.write_u32(HASH_POSITION);
                hasher.write_u64(position.0.x.to_bits() as u64);
                hasher.write_u64(position.0.y.to_bits() as u64);
            }
            if let Some(health) = self.world.get::<Health>(entity) {
                hasher.write_u32(HASH_HEALTH);
                hasher.write_u32(health.current as u32);
                hasher.write_u32(health.max as u32);
            }
            if let Some(mana) = self.world.get::<Mana>(entity) {
                hasher.write_u32(HASH_MANA);
                hasher.write_u32(mana.current as u32);
                hasher.write_u32(mana.max as u32);
            }
            if let Some(stats) = self.world.get::<AttackStats>(entity) {
                hasher.write_u32(HASH_ATTACK_STATS);
                hasher.write_u32(stats.damage as u32);
                hasher.write_u64(stats.range.to_bits() as u64);
                hasher.write_u64(stats.range_sq.to_bits() as u64);
                hasher.write_u32(stats.interval);
                hasher.write_u32(stats.cooldown);
            }
            if let Some(target) = self.world.get::<AttackTarget>(entity) {
                hasher.write_u32(HASH_ATTACK_TARGET);
                hasher.write_u64(target.0.map_or(u64::MAX, Entity::to_bits));
            }
            if let Some(target) = self.world.get::<MoveTarget>(entity) {
                hasher.write_u32(HASH_MOVE_TARGET);
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
                hasher.write_u32(HASH_COOLDOWNS);
                for cooldown in cooldowns.0 {
                    hasher.write_u32(cooldown);
                }
            }
            if let Some(cooldowns) = self.world.get::<ItemCooldowns>(entity) {
                hasher.write_u32(HASH_ITEM_COOLDOWNS);
                for cooldown in cooldowns.0 {
                    hasher.write_u32(u32::from(cooldown));
                }
            }
            if let Some(walker) = self.world.get::<LaneWalker>(entity) {
                hasher.write_u32(HASH_LANE_WALKER);
                hasher.write_u32(u32::from(walker.lane));
                hasher.write_u32(u32::from(walker.waypoint));
            }
            if let Some(dead) = self.world.get::<Dead>(entity) {
                hasher.write_u32(HASH_DEAD);
                hasher.write_u32(dead.respawn_in);
            }
            if let Some(identity) = self.world.get::<HeroIdentity>(entity) {
                hasher.write_u32(HASH_HERO_IDENTITY);
                hasher.write_u32(u32::from(identity.0.0));
            }
            if let Some(slot) = self.world.get::<HeroSlot>(entity) {
                hasher.write_u32(HASH_HERO_SLOT);
                hasher.write_u32(u32::from(slot.0));
            }
            if let Some(progress) = self.world.get::<Progression>(entity) {
                hasher.write_u32(HASH_PROGRESSION);
                hasher.write_u32(u32::from(progress.level));
                hasher.write_u32(progress.xp);
                hasher.write_u32(progress.gold);
                hasher.write_u32(u32::from(progress.item_count));
                for item in progress.items {
                    hasher.write_u32(u32::from(item));
                }
            }
            if let Some(modifiers) = self.world.get::<Modifiers>(entity) {
                hasher.write_u32(HASH_MODIFIERS);
                hasher.write_u32(u32::from(modifiers.len));
                for modifier in modifiers.entries() {
                    hasher.write_u32(u32::from(modifier.def_id));
                    hasher.write_u32(modifier.kind as u32);
                    hasher.write_u32(modifier.remaining);
                    hasher.write_u64(modifier.factor.to_bits() as u64);
                    hasher.write_u32(modifier.amount as u32);
                    hasher.write_u32(modifier.damage_type as u32);
                    hasher.write_u64(modifier.source.to_bits());
                }
            }
            if let Some(regen) = self.world.get::<Regen>(entity) {
                hasher.write_u32(HASH_REGEN);
                hasher.write_u32(regen.health as u32);
                hasher.write_u32(regen.mana as u32);
            }
            if let Some(resistances) = self.world.get::<Resistances>(entity) {
                hasher.write_u32(HASH_RESISTANCES);
                hasher.write_u32(resistances.armor as u32);
                hasher.write_u32(resistances.magic_resist as u32);
            }
            if let Some(vision) = self.world.get::<VisionRange>(entity) {
                hasher.write_u32(HASH_VISION);
                hasher.write_u64(vision.0.to_bits() as u64);
            }
            if let Some(speed) = self.world.get::<MoveSpeed>(entity) {
                hasher.write_u32(HASH_MOVE_SPEED);
                hasher.write_u64(speed.0.to_bits() as u64);
            }
            if let Some(speed) = self.world.get::<BaseMoveSpeed>(entity) {
                hasher.write_u32(HASH_BASE_MOVE_SPEED);
                hasher.write_u64(speed.0.to_bits() as u64);
            }
            if let Some(last_hit) = self.world.get::<LastHitBy>(entity) {
                hasher.write_u32(HASH_LAST_HIT_BY);
                hasher.write_u32(last_hit.team.index() as u32);
                hasher.write_u64(last_hit.entity.to_bits());
            }
            if let Some(damage_type) = self.world.get::<AttackDamageType>(entity) {
                hasher.write_u32(HASH_ATTACK_DAMAGE_TYPE);
                hasher.write_u32(damage_type.0 as u32);
            }
        }
        hasher.finish64()
    }

    fn spawn_structures(&mut self) {
        for structure in self.content.map.structures.clone() {
            let entity = self.world.spawn();
            let (stats, kind) = match structure.kind {
                moba_game::StructureKind::Tower => (&self.content.rules.tower, UnitKind::Tower),
                moba_game::StructureKind::Nexus => (&self.content.rules.nexus, UnitKind::Nexus),
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
            let _ = self.world.insert(entity, Modifiers::default());
            let _ = self.world.insert(entity, MoveTarget::default());
        }
    }

    fn spawn_hero(&mut self, slot: usize) -> Entity {
        let team_size = usize::from(self.content.rules.team_size);
        let team = if slot < team_size {
            Team::Blue
        } else {
            Team::Red
        };
        let pick = usize::from(self.config.hero_picks[slot]) % self.content.heroes.len();
        let hero_id = HeroId(u8::try_from(pick).expect("hero index fits u8"));
        let def = &self.content.heroes[pick];
        let entity = self.world.spawn();
        let _ = self.world.insert(
            entity,
            Position(
                self.content
                    .map
                    .hero_spawn(team, u8::try_from(slot % team_size).expect("slot fits u8")),
            ),
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
        let base_speed = def.move_speed_per_sec * ONE_TICK;
        let _ = self.world.insert(entity, MoveSpeed(base_speed));
        let _ = self.world.insert(entity, BaseMoveSpeed(base_speed));
        let _ = self.world.insert(entity, MoveTarget::default());
        let _ = self.world.insert(entity, AttackTarget::default());
        let _ = self
            .world
            .insert(entity, HeroSlot(u8::try_from(slot).expect("slot fits u8")));
        let _ = self.world.insert(entity, HeroIdentity(hero_id));
        let _ = self.world.insert(entity, Cooldowns::default());
        let _ = self.world.insert(entity, ItemCooldowns::default());
        let _ = self.world.insert(
            entity,
            Progression {
                level: 1,
                xp: 0,
                gold: self.content.rules.starting_gold,
                items: [EMPTY_ITEM; 6],
                item_count: 0,
            },
        );
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
        let _ = self.world.insert(entity, Modifiers::default());
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
            &self.content.rules.melee_minion
        } else {
            &self.content.rules.ranged_minion
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
        let base_speed = def.move_speed_per_sec * ONE_TICK;
        let _ = self.world.insert(entity, MoveSpeed(base_speed));
        let _ = self.world.insert(entity, BaseMoveSpeed(base_speed));
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
        let _ = self.world.insert(entity, Modifiers::default());
        Some(entity)
    }

    fn spawn_wave(&mut self) {
        self.wave = self.wave.saturating_add(1);
        // Alternate spawn order so neither team systematically receives the first tick of
        // wave movement/entity ordering.
        let order = if self.wave % 2 == 0 {
            [Team::Red, Team::Blue]
        } else {
            [Team::Blue, Team::Red]
        };
        for team in order {
            let lanes: Vec<(FixVec2, FixVec2)> = self
                .content
                .map
                .lanes
                .iter()
                .map(|lane| (lane.point(team, 0), lane.point(team, 1)))
                .collect();
            for (lane_index, (start, next)) in lanes.into_iter().enumerate() {
                let direction = (next - start).normalize_or_zero();
                let perpendicular = direction.perpendicular();
                for index in 0..3i32 {
                    let offset = Fix::from_i32(index - 1);
                    let position = self.clamp_position(
                        start
                            + perpendicular * offset * Fix::from_i32(2)
                            + direction * Fix::from_i32(-1),
                    );
                    self.spawn_minion(team, lane_index, UnitKind::MeleeMinion, position, 1);
                }
                let position = self.clamp_position(start + perpendicular * Fix::from_i32(6));
                self.spawn_minion(team, lane_index, UnitKind::RangedMinion, position, 1);
            }
        }
        self.push_event(SimEvent::WaveSpawned { wave: self.wave });
    }

    fn recompute_move_speed(&mut self, entity: Entity) {
        let base = self
            .world
            .get::<BaseMoveSpeed>(entity)
            .map(|base| base.0)
            .or_else(|| self.world.get::<MoveSpeed>(entity).map(|speed| speed.0));
        let Some(base) = base else {
            return;
        };
        let factor = self
            .world
            .get::<Modifiers>(entity)
            .map_or(Fix::ONE, Modifiers::move_factor);
        if let Some(speed) = self.world.get_mut::<MoveSpeed>(entity) {
            speed.0 = base * factor;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_modifier(
        &mut self,
        entity: Entity,
        def_id: u16,
        remaining: u32,
        factor: Fix,
        amount: i32,
        source: Entity,
        damage_type: DamageType,
    ) {
        let Some(kind) = self
            .content
            .modifiers
            .get(usize::from(def_id))
            .map(|definition| definition.kind)
        else {
            return;
        };
        let Some(modifiers) = self.world.get_mut::<Modifiers>(entity) else {
            return;
        };
        let mut existing = None;
        for index in 0..usize::from(modifiers.len) {
            if modifiers.entries[index].kind == kind {
                existing = Some(index);
                break;
            }
        }
        if let Some(index) = existing {
            let modifier = &mut modifiers.entries[index];
            modifier.def_id = def_id;
            modifier.remaining = modifier.remaining.max(remaining);
            match kind {
                ModifierKind::Shield => {
                    modifier.amount = modifier.amount.saturating_add(amount);
                }
                ModifierKind::Slow => {
                    modifier.factor = modifier.factor.min(factor);
                }
                ModifierKind::Haste => {
                    modifier.factor = modifier.factor.max(factor);
                }
                ModifierKind::Burn => {
                    modifier.amount = amount;
                    modifier.damage_type = damage_type;
                    modifier.source = source;
                }
                ModifierKind::Root | ModifierKind::Silence | ModifierKind::Stun => {}
            }
        } else if usize::from(modifiers.len) < MAX_MODIFIERS {
            let index = usize::from(modifiers.len);
            modifiers.entries[index] = Modifier {
                def_id,
                kind,
                remaining,
                factor,
                amount,
                damage_type,
                source,
            };
            modifiers.len += 1;
        } else {
            return;
        }
        if matches!(kind, ModifierKind::Slow | ModifierKind::Haste) {
            self.recompute_move_speed(entity);
        }
    }

    fn gather_enemies(&mut self, team: Team, target: FixVec2, radius: Fix) {
        self.damage_hits.clear();
        self.grid.for_each_in_circle(target, radius, |candidate| {
            if candidate.team == team {
                return;
            }
            if self
                .world
                .get::<Health>(candidate.entity)
                .is_some_and(|health| health.current > 0)
            {
                self.damage_hits.push(candidate.entity);
            }
        });
    }

    #[allow(clippy::too_many_lines)] // One ordered status/timer pass.
    fn tick_timers(&mut self) {
        for (_, cooldowns) in self.world.query::<&mut Cooldowns>() {
            for cooldown in &mut cooldowns.0 {
                *cooldown = cooldown.saturating_sub(1);
            }
        }
        for (_, stats) in self.world.query::<&mut AttackStats>() {
            stats.cooldown = stats.cooldown.saturating_sub(1);
        }
        for (_, cooldowns) in self.world.query::<&mut ItemCooldowns>() {
            for cooldown in &mut cooldowns.0 {
                *cooldown = cooldown.saturating_sub(1);
            }
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

        self.modifier_entities.clear();
        for (entity, _) in self.world.query::<&Modifiers>() {
            self.modifier_entities.push(entity);
        }
        let mut burn_hits = core::mem::take(&mut self.burn_hits);
        burn_hits.clear();
        for index in 0..self.modifier_entities.len() {
            let entity = self.modifier_entities[index];
            let Some(modifiers) = self.world.get_mut::<Modifiers>(entity) else {
                continue;
            };
            let mut position = 0;
            let mut recompute = false;
            while position < usize::from(modifiers.len) {
                let modifier = &mut modifiers.entries[position];
                modifier.remaining = modifier.remaining.saturating_sub(1);
                if modifier.kind == ModifierKind::Burn {
                    burn_hits.push((
                        entity,
                        modifier.amount,
                        modifier.damage_type,
                        modifier.source,
                    ));
                } else if let Some(definition) =
                    self.content.modifiers.get(usize::from(modifier.def_id))
                    && definition.on_tick_damage > 0
                {
                    burn_hits.push((
                        entity,
                        definition.on_tick_damage,
                        DamageType::Magic,
                        modifier.source,
                    ));
                }
                let expired = modifier.remaining == 0
                    || (modifier.kind == ModifierKind::Shield && modifier.amount <= 0);
                if expired {
                    if let Some(definition) =
                        self.content.modifiers.get(usize::from(modifier.def_id))
                        && definition.on_expire_damage > 0
                    {
                        burn_hits.push((
                            entity,
                            definition.on_expire_damage,
                            DamageType::Magic,
                            modifier.source,
                        ));
                    }
                    recompute |= matches!(modifier.kind, ModifierKind::Slow | ModifierKind::Haste);
                    modifiers
                        .entries
                        .swap(position, usize::from(modifiers.len) - 1);
                    modifiers.len -= 1;
                } else {
                    position += 1;
                }
            }
            if recompute {
                self.recompute_move_speed(entity);
            }
        }
        for (target, damage, damage_type, source) in burn_hits.drain(..) {
            self.damage(target, damage, damage_type, source);
        }
        self.burn_hits = burn_hits;
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
        let _ = self.world.remove::<LastHitBy>(entity);
        if let Some(modifiers) = self.world.get_mut::<Modifiers>(entity) {
            modifiers.clear();
        }
        self.recompute_move_speed(entity);
        if let Some(position) = self.world.get_mut::<Position>(entity) {
            position.0 = self
                .content
                .map
                .hero_spawn(team, slot % self.content.rules.team_size);
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

    /// Deterministic scheduling phase: half the seeds resolve blue first, half red first.
    fn blue_first(&self) -> bool {
        (self.tick.get().wrapping_add((self.config.seed & 1) as u32)) % 2 == 0
    }

    fn apply_inputs(&mut self, inputs: &TickInputs) {
        // Alternate command resolution order so neither team systematically resolves casts,
        // purchases or attack orders first.
        let order: [usize; MAX_PLAYERS] = if self.blue_first() {
            core::array::from_fn(|index| index)
        } else {
            core::array::from_fn(|index| MAX_PLAYERS - 1 - index)
        };
        for slot in order {
            let entity = self.hero_entities[slot];
            if self.world.has::<Dead>(entity) {
                continue;
            }
            if let Some(item) = inputs.purchases[slot] {
                self.buy_item(entity, item);
            }
            let Some(command) = inputs.players[slot] else {
                continue;
            };
            match command {
                PlayerCommand::Stop => {
                    if let Some(target) = self.world.get_mut::<MoveTarget>(entity) {
                        target.0 = None;
                    }
                    if let Some(target) = self.world.get_mut::<AttackTarget>(entity) {
                        target.0 = None;
                    }
                }
                PlayerCommand::MoveTo(position) => {
                    if self
                        .world
                        .get::<Modifiers>(entity)
                        .is_none_or(|m| !m.has(ModifierKind::Root) && !m.has(ModifierKind::Stun))
                    {
                        let destination = self.clamp_position(position);
                        if let Some(target) = self.world.get_mut::<MoveTarget>(entity) {
                            target.0 = Some(destination);
                        }
                        if let Some(target) = self.world.get_mut::<AttackTarget>(entity) {
                            target.0 = None;
                        }
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
                PlayerCommand::BuyItem(item) => {
                    self.buy_item(entity, item);
                }
                PlayerCommand::UseItem(slot) => {
                    self.use_item(entity, slot);
                }
                PlayerCommand::Surrender => {
                    if self.outcome.is_none() {
                        if let Some(team) = self.world.get::<Team>(entity).copied() {
                            let winner = team.enemy();
                            self.outcome = Some(winner);
                            self.push_event(SimEvent::MatchEnded { winner });
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_lines)] // One validated targeting/effect pass.
    fn cast_ability(&mut self, caster: Entity, slot: u8, mut target: FixVec2) {
        let Some(identity) = self.world.get::<HeroIdentity>(caster).copied() else {
            return;
        };
        let Some(def) = self.content.heroes.get(usize::from(identity.0.0)) else {
            return;
        };
        let ability_index = usize::from(slot);
        let Some(ability) = def
            .abilities
            .get(ability_index)
            .and_then(|&id| self.content.abilities.get(usize::from(id)))
        else {
            return;
        };
        let cooldown_ticks = ability.cooldown_ticks;
        let mana_cost = ability.mana_cost;
        let targeting = ability.targeting;
        let effects = ability.effects;
        let Some(cooldowns) = self.world.get::<Cooldowns>(caster).copied() else {
            return;
        };
        if cooldowns.0[ability_index] > 0 {
            return;
        }
        let Some(mana) = self.world.get::<Mana>(caster).copied() else {
            return;
        };
        if mana.current < mana_cost {
            return;
        }
        let Some(position) = self.world.get::<Position>(caster).map(|p| p.0) else {
            return;
        };
        let Some(team) = self.world.get::<Team>(caster).copied() else {
            return;
        };
        if self
            .world
            .get::<Modifiers>(caster)
            .is_some_and(|m| m.has(ModifierKind::Silence) || m.has(ModifierKind::Stun))
        {
            return;
        }
        if self
            .world
            .get::<Modifiers>(caster)
            .is_some_and(|m| m.has(ModifierKind::Root) || m.has(ModifierKind::Stun))
            && effects
                .iter()
                .any(|effect| matches!(effect, Effect::Displace { .. }))
        {
            return;
        }
        match targeting {
            Targeting::SelfCast => target = position,
            Targeting::Point { range } => {
                if range == Fix::ZERO {
                    target = position;
                } else if position.distance(target) > range {
                    return;
                }
            }
            Targeting::Direction { range } => {
                let distance = position.distance(target);
                if distance == Fix::ZERO || distance > range {
                    return;
                }
            }
        }
        self.push_event(SimEvent::AbilityCast {
            caster,
            team,
            ability: slot,
            target,
        });
        for effect in effects.iter() {
            match effect {
                Effect::None => {}
                Effect::Damage {
                    amount,
                    damage_type,
                } => {
                    self.gather_enemies(team, target, Fix::ONE);
                    let mut hits = core::mem::take(&mut self.damage_hits);
                    for target_entity in hits.drain(..) {
                        self.damage(target_entity, *amount, *damage_type, caster);
                    }
                    self.damage_hits = hits;
                }
                Effect::Heal { amount } => {
                    if let Some(health) = self.world.get_mut::<Health>(caster) {
                        health.current = (health.current + amount).min(health.max);
                    }
                }
                Effect::ApplyModifier {
                    modifier,
                    duration_ticks,
                    factor,
                    amount,
                    damage_type,
                } => {
                    self.apply_modifier(
                        caster,
                        *modifier,
                        *duration_ticks,
                        *factor,
                        *amount,
                        caster,
                        *damage_type,
                    );
                }
                Effect::Displace { range } => {
                    let delta = target - position;
                    let distance = delta.length().min(*range);
                    if distance > Fix::ZERO {
                        let destination =
                            self.clamp_position(position + delta.normalize_or_zero() * distance);
                        if let Some(current) = self.world.get_mut::<Position>(caster) {
                            current.0 = destination;
                        }
                    }
                }
                Effect::Projectile {
                    speed_per_sec,
                    range,
                    radius,
                    arc,
                    on_hit,
                } => {
                    let direction = (target - position).normalize_or_zero();
                    if direction == FixVec2::ZERO || self.projectiles.len() >= MAX_PROJECTILES {
                        continue;
                    }
                    self.projectiles.push(Projectile {
                        owner: caster,
                        team,
                        position,
                        direction,
                        speed_per_tick: *speed_per_sec * ONE_TICK,
                        remaining: *range,
                        radius: *radius,
                        arc: *arc,
                        on_hit: *on_hit,
                    });
                    self.push_event(SimEvent::ProjectileSpawned {
                        owner: caster,
                        team,
                        position,
                        direction,
                        speed_per_sec: *speed_per_sec,
                        radius: *radius,
                        arc: *arc,
                        range: *range,
                    });
                }
                Effect::Area { radius, effects } => {
                    self.apply_area_effects(caster, team, target, *radius, *effects);
                }
                Effect::Zone {
                    radius,
                    duration_ticks,
                    period_ticks,
                    effects,
                } => {
                    if self.zones.len() < MAX_ZONES {
                        self.zones.push(Zone {
                            owner: caster,
                            team,
                            position: target,
                            radius: *radius,
                            remaining: *duration_ticks,
                            period_ticks: *period_ticks,
                            cooldown: 0,
                            effects: *effects,
                        });
                        self.push_event(SimEvent::ZoneSpawned {
                            owner: caster,
                            team,
                            position: target,
                            radius: *radius,
                            duration_ticks: *duration_ticks,
                        });
                    }
                }
            }
        }

        if let Some(mana) = self.world.get_mut::<Mana>(caster) {
            mana.current = (mana.current - mana_cost).max(0);
        }
        if let Some(cooldowns) = self.world.get_mut::<Cooldowns>(caster) {
            cooldowns.0[ability_index] = cooldown_ticks;
        }
    }

    fn apply_area_effects(
        &mut self,
        caster: Entity,
        team: Team,
        center: FixVec2,
        radius: Fix,
        effects: AreaEffectList,
    ) {
        self.gather_enemies(team, center, radius);
        let mut hits = core::mem::take(&mut self.damage_hits);
        for target in hits.drain(..) {
            self.apply_area_effects_to(caster, target, effects);
        }
        self.damage_hits = hits;
    }

    fn apply_area_effects_to(&mut self, caster: Entity, target: Entity, effects: AreaEffectList) {
        for effect in effects.iter() {
            match effect {
                AreaEffect::None => {}
                AreaEffect::Damage {
                    amount,
                    damage_type,
                } => self.damage(target, *amount, *damage_type, caster),
                AreaEffect::ApplyModifier {
                    modifier,
                    duration_ticks,
                    factor,
                    amount,
                    damage_type,
                } => self.apply_modifier(
                    target,
                    *modifier,
                    *duration_ticks,
                    *factor,
                    *amount,
                    caster,
                    *damage_type,
                ),
            }
        }
    }

    fn tick_zones(&mut self) {
        let mut index = 0;
        while index < self.zones.len() {
            let mut zone = self.zones[index];
            if zone.cooldown == 0 {
                self.apply_area_effects(
                    zone.owner,
                    zone.team,
                    zone.position,
                    zone.radius,
                    zone.effects,
                );
                zone.cooldown = zone.period_ticks;
            }
            zone.cooldown = zone.cooldown.saturating_sub(1);
            zone.remaining = zone.remaining.saturating_sub(1);
            if zone.remaining == 0 {
                self.zones.swap_remove(index);
            } else {
                self.zones[index] = zone;
                index += 1;
            }
        }
    }

    fn update_minion_targets(&mut self) {
        let grid = &self.grid;
        let map = &self.content.map;
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
                        && best_structure.is_none_or(|(best, entity)| {
                            distance < best || (distance == best && candidate.entity < entity)
                        })
                    {
                        best_structure = Some((distance, candidate.entity));
                    }
                } else if distance <= MINION_AGGRO_SQ
                    && best_unit.is_none_or(|(best, entity)| {
                        distance < best || (distance == best && candidate.entity < entity)
                    })
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
            let lane = &map.lanes[usize::from(walker.lane).min(map.lanes.len() - 1)];
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
                    if best_minion.is_none_or(|(best, entity)| {
                        distance < best || (distance == best && candidate.entity < entity)
                    }) {
                        best_minion = Some((distance, candidate.entity));
                    }
                } else if candidate.kind.is_hero()
                    && best_hero.is_none_or(|(best, entity)| {
                        distance < best || (distance == best && candidate.entity < entity)
                    })
                {
                    best_hero = Some((distance, candidate.entity));
                }
            });
            attack_target.0 = if self.content.rules.structure_heroes_first {
                best_hero.or(best_minion)
            } else {
                best_minion.or(best_hero)
            }
            .map(|(_, entity)| entity);
        }
    }

    fn move_units(&mut self) {
        let extent = self.content.map.extent;
        for (_, (position, target, speed)) in self
            .world
            .query_filtered::<(&mut Position, &MoveTarget, &MoveSpeed), Without<Dead>>()
        {
            let Some(destination) = target.0 else {
                continue;
            };
            let next = position.0.move_towards(destination, speed.0);
            position.0 = clamp_to_extent(next, extent);
        }
    }

    fn modifier_armor(&self, entity: Entity) -> i32 {
        self.world.get::<Modifiers>(entity).map_or(0, |modifiers| {
            modifiers
                .entries()
                .iter()
                .filter_map(|modifier| self.content.modifiers.get(usize::from(modifier.def_id)))
                .map(|definition| definition.bonus_armor)
                .sum()
        })
    }

    fn modifier_attack_damage(&self, entity: Entity) -> i32 {
        self.world.get::<Modifiers>(entity).map_or(0, |modifiers| {
            modifiers
                .entries()
                .iter()
                .filter_map(|modifier| self.content.modifiers.get(usize::from(modifier.def_id)))
                .map(|definition| definition.bonus_attack_damage)
                .sum()
        })
    }

    fn modifier_on_damaged(&self, target: Entity) -> i32 {
        self.world.get::<Modifiers>(target).map_or(0, |modifiers| {
            modifiers
                .entries()
                .iter()
                .filter_map(|modifier| self.content.modifiers.get(usize::from(modifier.def_id)))
                .map(|definition| definition.on_damaged_damage)
                .sum()
        })
    }

    fn on_damaged_bonus(&self, target: Entity) -> i32 {
        if self.world.get::<HeroSlot>(target).is_none() {
            return 0;
        }
        self.world.get::<Progression>(target).map_or(0, |progress| {
            progress.items[..usize::from(progress.item_count)]
                .iter()
                .filter_map(|item| self.content.items.get(usize::from(*item)))
                .map(|item| i32::from(item.on_damaged_damage))
                .sum()
        })
    }

    fn on_hit_bonus(&self, attacker: Entity) -> i32 {
        if self.world.get::<HeroSlot>(attacker).is_none() {
            return 0;
        }
        self.world
            .get::<Progression>(attacker)
            .map_or(0, |progress| {
                progress.items[..usize::from(progress.item_count)]
                    .iter()
                    .filter_map(|item| self.content.items.get(usize::from(*item)))
                    .map(|item| i32::from(item.on_hit_damage))
                    .sum()
            })
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
        let alternate = !self.blue_first();
        for index in 0..attackers.len() {
            let snapshot = if alternate {
                attackers[attackers.len() - 1 - index]
            } else {
                attackers[index]
            };
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
            let Some(attacker_team) = self.world.get::<Team>(snapshot.attacker).copied() else {
                continue;
            };
            let target_team = self.world.get::<Team>(snapshot.target).copied();
            if Some(attacker_team) == target_team {
                continue;
            }
            if attacker_position.distance_sq(target_position) > snapshot.range_sq {
                continue;
            }
            let bonus = self.on_hit_bonus(snapshot.attacker);
            let modifier_damage = self.modifier_attack_damage(snapshot.attacker);
            self.damage(
                snapshot.target,
                snapshot
                    .damage
                    .saturating_add(bonus)
                    .saturating_add(modifier_damage),
                DamageType::Physical,
                snapshot.attacker,
            );
            let thorns = self
                .on_damaged_bonus(snapshot.target)
                .saturating_add(self.modifier_on_damaged(snapshot.target));
            if thorns > 0 && self.world.has::<Health>(snapshot.attacker) {
                self.damage(
                    snapshot.attacker,
                    thorns,
                    DamageType::Physical,
                    snapshot.target,
                );
            }
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
                self.push_event(SimEvent::ProjectileHit {
                    owner: projectile.owner,
                    team: projectile.team,
                    target,
                    position: projectile.position,
                });
                self.apply_area_effects_to(projectile.owner, target, projectile.on_hit);
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
        for (entity, (health, team, kind, position)) in
            self.world.query::<(&Health, &Team, &UnitKind, &Position)>()
        {
            if health.current <= 0 {
                deaths.push((entity, *team, *kind, position.0));
            }
        }

        for (entity, team, kind, position) in deaths.iter().copied() {
            if kind.is_hero() {
                if self.world.has::<Dead>(entity) {
                    continue;
                }
                let level = self
                    .world
                    .get::<Progression>(entity)
                    .map_or(1, |progress| progress.level);
                let respawn_in = self.content.rules.hero_respawn_ticks.saturating_add(
                    self.content
                        .rules
                        .respawn_per_level
                        .saturating_mul(u32::from(level.saturating_sub(1))),
                );
                let _ = self.world.insert(entity, Dead { respawn_in });
                if let Some(target) = self.world.get_mut::<MoveTarget>(entity) {
                    target.0 = None;
                }
                if let Some(target) = self.world.get_mut::<AttackTarget>(entity) {
                    target.0 = None;
                }
                let credit = self.world.get::<LastHitBy>(entity).copied();
                let hero_slot = self.world.get::<HeroSlot>(entity).map(|slot| slot.0);
                let killer_slot = credit.and_then(|credit| {
                    self.world.get::<HeroSlot>(credit.entity).map(|slot| slot.0)
                });
                if let Some(credit) = credit {
                    match credit.team {
                        Team::Blue => self.blue_score = self.blue_score.saturating_add(1),
                        Team::Red => self.red_score = self.red_score.saturating_add(1),
                    }
                }
                let limit = u32::from(self.content.rules.score_limit);
                if limit > 0 && self.outcome.is_none() {
                    if self.blue_score >= limit {
                        self.outcome = Some(Team::Blue);
                        self.push_event(SimEvent::MatchEnded { winner: Team::Blue });
                    } else if self.red_score >= limit {
                        self.outcome = Some(Team::Red);
                        self.push_event(SimEvent::MatchEnded { winner: Team::Red });
                    }
                }
                self.award_bounty(team, kind, position, credit.map(|credit| credit.entity));
                self.push_event(SimEvent::UnitDied {
                    entity,
                    kind,
                    team,
                    position,
                    killer: credit.map(|credit| credit.entity),
                    hero_slot,
                    killer_slot,
                });
                continue;
            }

            let credit = self.world.get::<LastHitBy>(entity).copied();
            let hero_slot = self.world.get::<HeroSlot>(entity).map(|slot| slot.0);
            let killer_slot = credit
                .and_then(|credit| self.world.get::<HeroSlot>(credit.entity).map(|slot| slot.0));
            self.award_bounty(team, kind, position, credit.map(|credit| credit.entity));
            self.world.despawn(entity);
            self.push_event(SimEvent::UnitDied {
                entity,
                kind,
                team,
                position,
                killer: credit.map(|credit| credit.entity),
                hero_slot,
                killer_slot,
            });
            if kind.is_structure() {
                self.push_event(SimEvent::StructureDestroyed {
                    team,
                    kind,
                    position,
                });
                if kind == UnitKind::Nexus && self.content.rules.nexus_win && self.outcome.is_none()
                {
                    self.outcome = Some(team.enemy());
                    self.push_event(SimEvent::MatchEnded {
                        winner: team.enemy(),
                    });
                }
            }
        }
        self.deaths = deaths;
    }

    fn damage(&mut self, target: Entity, amount: i32, damage_type: DamageType, source: Entity) {
        let Some(source_team) = self.world.get::<Team>(source).copied() else {
            return;
        };
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
        let bonus_armor = self.modifier_armor(target);
        let dealt = mitigate_typed(
            amount,
            damage_type,
            resistances.armor.saturating_add(bonus_armor),
            resistances.magic_resist,
        );
        let mut damage = dealt;
        if let Some(modifiers) = self.world.get_mut::<Modifiers>(target) {
            let mut index = 0;
            while index < usize::from(modifiers.len) {
                let modifier = &mut modifiers.entries[index];
                if modifier.kind == ModifierKind::Shield && modifier.amount > 0 {
                    let absorbed = modifier.amount.min(damage);
                    modifier.amount -= absorbed;
                    damage -= absorbed;
                }
                if modifier.kind == ModifierKind::Shield && modifier.amount <= 0 {
                    modifiers
                        .entries
                        .swap(index, usize::from(modifiers.len) - 1);
                    modifiers.len -= 1;
                } else {
                    index += 1;
                }
            }
        }
        let remaining = if let Some(health) = self.world.get_mut::<Health>(target) {
            health.current = (health.current - damage).max(0);
            health.current
        } else {
            return;
        };
        if remaining <= 0 {
            let target_team = self.world.get::<Team>(target).copied();
            if target_team.is_some_and(|team| team != source_team) {
                let _ = self.world.insert(
                    target,
                    LastHitBy {
                        team: source_team,
                        entity: source,
                    },
                );
            }
        }
    }

    fn award_bounty(
        &mut self,
        dead_team: Team,
        kind: UnitKind,
        position: FixVec2,
        last_hit: Option<Entity>,
    ) {
        let enemy = dead_team.enemy();
        let reward = self.unit_rewards(kind);
        let mut beneficiaries = [Entity::PLACEHOLDER; MAX_PLAYERS];
        let mut count = 0usize;
        for hero in self.hero_entities {
            if self.world.get::<Team>(hero).copied() != Some(enemy) || self.world.has::<Dead>(hero)
            {
                continue;
            }
            let Some(hero_position) = self.world.get::<Position>(hero).map(|p| p.0) else {
                continue;
            };
            if hero_position.distance_sq(position)
                <= self.content.rules.xp_radius * self.content.rules.xp_radius
            {
                beneficiaries[count] = hero;
                count += 1;
            }
        }
        if count > 0 {
            // League-style shared minion experience: 130% split among nearby allies.
            let shared = (reward.0 * self.content.rules.shared_xp_percent / 100)
                / u32::try_from(count).unwrap_or(1);
            let share = shared.max(1);
            for hero in &beneficiaries[..count] {
                self.grant_xp(*hero, share);
            }
        }
        let Some(killer) = last_hit else {
            return;
        };
        if self.world.get::<Team>(killer).copied() != Some(enemy)
            || self.world.get::<HeroSlot>(killer).is_none()
        {
            return;
        }
        if let Some(progress) = self.world.get_mut::<Progression>(killer) {
            progress.gold = progress.gold.saturating_add(reward.1);
        }
    }

    fn grant_xp(&mut self, hero: Entity, amount: u32) {
        let Some(mut progress) = self.world.get::<Progression>(hero).copied() else {
            return;
        };
        let cap = self.content.rules.level_cap;
        let xp_base = self.content.rules.xp_base;
        let xp_per_level = self.content.rules.xp_per_level;
        let xp_needed =
            |level: u8| xp_base.saturating_add(xp_per_level.saturating_mul(u32::from(level)));
        if progress.level >= cap {
            progress.xp = 0;
            let _ = self.world.insert(hero, progress);
            return;
        }
        progress.xp = progress.xp.saturating_add(amount);
        while progress.level < cap {
            let needed = xp_needed(progress.level);
            if progress.xp < needed {
                break;
            }
            progress.xp -= needed;
            progress.level += 1;
            self.level_up(hero);
        }
        if progress.level >= cap {
            progress.xp = 0;
        }
        let _ = self.world.insert(hero, progress);
    }

    fn level_up(&mut self, hero: Entity) {
        let growth = self
            .world
            .get::<HeroIdentity>(hero)
            .and_then(|identity| self.content.heroes.get(usize::from(identity.0.0)))
            .map_or(LEVEL_GROWTH, |hero| moba_game::LevelGrowth {
                health: hero.health_per_level,
                mana: hero.mana_per_level,
                attack_damage: hero.attack_damage_per_level,
                armor: hero.armor_per_level,
                magic_resist: hero.magic_resist_per_level,
            });
        if let Some(health) = self.world.get_mut::<Health>(hero) {
            health.max = health.max.saturating_add(growth.health);
            health.current = (health.current + growth.health).min(health.max);
        }
        if let Some(mana) = self.world.get_mut::<Mana>(hero) {
            mana.max = mana.max.saturating_add(growth.mana);
            mana.current = (mana.current + growth.mana).min(mana.max);
        }
        if let Some(stats) = self.world.get_mut::<AttackStats>(hero) {
            stats.damage = stats.damage.saturating_add(growth.attack_damage);
        }
        if let Some(resistances) = self.world.get_mut::<Resistances>(hero) {
            resistances.armor = resistances.armor.saturating_add(growth.armor);
            resistances.magic_resist = resistances.magic_resist.saturating_add(growth.magic_resist);
        }
    }

    /// Purchases an item for `slot`'s hero. Used by the network server for shop inputs,
    /// which are applied immediately rather than queued as movement orders.
    ///
    /// Returns whether the purchase succeeded (near own nexus, enough gold, free slot).
    pub fn purchase(&mut self, slot: usize, item_id: u8) -> bool {
        let Some(&hero) = self.hero_entities.get(slot) else {
            return false;
        };
        if self.world.has::<Dead>(hero) {
            return false;
        }
        self.buy_item(hero, item_id)
    }

    fn use_item(&mut self, hero: Entity, slot: u8) {
        if self.world.has::<Dead>(hero) {
            return;
        }
        let Some(progress) = self.world.get::<Progression>(hero).copied() else {
            return;
        };
        if slot >= progress.item_count {
            return;
        }
        let item_id = progress.items[usize::from(slot)];
        let Some(item) = self.content.items.get(usize::from(item_id)) else {
            return;
        };
        if item.active_heal <= 0 || item.active_cooldown_ticks == 0 {
            return;
        }
        let Some(mut cooldowns) = self.world.get::<ItemCooldowns>(hero).copied() else {
            return;
        };
        if cooldowns.0[usize::from(slot)] != 0 {
            return;
        }
        if let Some(health) = self.world.get_mut::<Health>(hero) {
            health.current = (health.current + item.active_heal).min(health.max);
        }
        cooldowns.0[usize::from(slot)] = item.active_cooldown_ticks;
        let _ = self.world.insert(hero, cooldowns);
    }

    #[allow(clippy::too_many_lines)] // Flat stat application is clearer than a trait tower.
    fn buy_item(&mut self, hero: Entity, item_id: u8) -> bool {
        let Some(item) = self.content.items.get(usize::from(item_id)) else {
            return false;
        };
        let cost = item.cost;
        let item_health = item.health;
        let item_mana = item.mana;
        let item_attack_damage = item.attack_damage;
        let item_armor = item.armor;
        let item_magic_resist = item.magic_resist;
        let item_move_speed = item.move_speed_per_sec;
        let Some(team) = self.world.get::<Team>(hero).copied() else {
            return false;
        };
        let Some(position) = self.world.get::<Position>(hero).map(|p| p.0) else {
            return false;
        };
        if position.distance_sq(self.content.map.nexus_position(team))
            > self.content.rules.shop_radius * self.content.rules.shop_radius
        {
            return false;
        }
        let Some(mut progress) = self.world.get::<Progression>(hero).copied() else {
            return false;
        };
        if progress.gold < cost || usize::from(progress.item_count) >= moba_game::ITEM_SLOTS {
            return false;
        }
        progress.gold -= cost;
        progress.items[usize::from(progress.item_count)] = item_id;
        progress.item_count += 1;
        let _ = self.world.insert(hero, progress);
        if let Some(health) = self.world.get_mut::<Health>(hero) {
            health.max = health.max.saturating_add(item_health);
            health.current = health.current.saturating_add(item_health);
        }
        if let Some(mana) = self.world.get_mut::<Mana>(hero) {
            mana.max = mana.max.saturating_add(item_mana);
            mana.current = mana.current.saturating_add(item_mana);
        }
        if let Some(stats) = self.world.get_mut::<AttackStats>(hero) {
            stats.damage = stats.damage.saturating_add(item_attack_damage);
        }
        if let Some(resistances) = self.world.get_mut::<Resistances>(hero) {
            resistances.armor = resistances.armor.saturating_add(item_armor);
            resistances.magic_resist = resistances.magic_resist.saturating_add(item_magic_resist);
        }
        let bonus = item_move_speed * ONE_TICK;
        if let Some(base) = self.world.get_mut::<BaseMoveSpeed>(hero) {
            base.0 += bonus;
        } else if let Some(speed) = self.world.get_mut::<MoveSpeed>(hero) {
            speed.0 += bonus;
        }
        self.recompute_move_speed(hero);
        true
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
        } else if matches!(event, SimEvent::MatchEnded { .. }) {
            // Match end is a terminal event; it must not be lost when the bounded queue is
            // full of routine combat events.
            self.events[MAX_EVENTS - 1] = event;
        }
    }

    /// Installs a custom bot policy (or restores the default with `None`).
    pub fn set_bot_policy(&mut self, policy: Option<Box<dyn BotPolicy>>) {
        self.bot_policy = policy;
    }

    /// Deterministic bot AI. Returns one command per bot-controlled slot.
    #[must_use]
    pub fn bot_inputs(&mut self) -> TickInputs {
        if let Some(mut policy) = self.bot_policy.take() {
            let inputs = policy.inputs(self);
            self.bot_policy = Some(policy);
            inputs
        } else {
            self.default_bot_inputs()
        }
    }

    #[must_use]
    #[allow(clippy::too_many_lines)] // Explicit priority ladder is easier to audit.
    fn default_bot_inputs(&mut self) -> TickInputs {
        self.refresh_spatial();
        let visibility = self.bot_visibility_masks();
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
                max_health: self.content.heroes[usize::from(identity.0.0)].max_health,
                ability_mana: {
                    let mut mana = [0; 3];
                    for (slot, id) in self.content.heroes[usize::from(identity.0.0)]
                        .abilities
                        .iter()
                        .enumerate()
                    {
                        mana[slot] = self
                            .content
                            .abilities
                            .get(usize::from(*id))
                            .map_or(0, |ability| ability.mana_cost);
                    }
                    mana
                },
                ability_range: {
                    let mut range = [Fix::ZERO; 3];
                    for (slot, id) in self.content.heroes[usize::from(identity.0.0)]
                        .abilities
                        .iter()
                        .enumerate()
                    {
                        range[slot] = self
                            .content
                            .abilities
                            .get(usize::from(*id))
                            .map_or(Fix::ZERO, moba_game::AbilitySpec::cast_range);
                    }
                    range
                },
            });
        }

        let mut inputs = TickInputs::new();
        let view_count = self.hero_views.len();
        for index in 0..view_count {
            let view = self.hero_views[index];
            let slot = usize::from(view.slot);
            let (cast_roll, jitter_x, jitter_y, buy_roll) = {
                let rng = &mut self.bot_rng[slot % MAX_PLAYERS];
                (
                    rng.gen_range_u32(100),
                    i32::try_from(rng.gen_range_u32(5)).unwrap_or(0) - 2,
                    i32::try_from(rng.gen_range_u32(5)).unwrap_or(0) - 2,
                    rng.gen_range_u32(100),
                )
            };
            if self.config.bot_mask & (1 << slot) == 0 {
                continue;
            }
            if let Some(hero) = self.hero_entities.get(slot).copied() {
                if let Some(progress) = self.world.get::<Progression>(hero).copied()
                    && let Some(cooldowns) = self.world.get::<ItemCooldowns>(hero).copied()
                    && view.health * 2 <= view.max_health
                {
                    let mut used = false;
                    for item_slot in 0..usize::from(progress.item_count) {
                        let item_id = progress.items[item_slot];
                        if cooldowns.0[item_slot] == 0
                            && self
                                .content
                                .items
                                .get(usize::from(item_id))
                                .is_some_and(|item| {
                                    item.active_heal > 0 && item.active_cooldown_ticks > 0
                                })
                        {
                            inputs.set(
                                slot,
                                PlayerCommand::UseItem(u8::try_from(item_slot).unwrap_or(0)),
                            );
                            used = true;
                            break;
                        }
                    }
                    if used {
                        continue;
                    }
                }
                let near_shop = self.world.get::<Position>(hero).is_some_and(|position| {
                    position
                        .0
                        .distance_sq(self.content.map.nexus_position(view.team))
                        <= self.content.rules.shop_radius * self.content.rules.shop_radius
                });
                if near_shop {
                    if let Some(progress) = self.world.get::<Progression>(hero) {
                        if usize::from(progress.item_count) < moba_game::ITEM_SLOTS {
                            let affordable: Vec<u8> = self
                                .content
                                .items
                                .iter()
                                .enumerate()
                                .filter(|(_, item)| item.cost <= progress.gold)
                                .map(|(id, _)| u8::try_from(id).unwrap_or(0))
                                .collect();
                            if !affordable.is_empty() {
                                let pick =
                                    usize::try_from(buy_roll).unwrap_or(0) % affordable.len();
                                inputs.set(slot, PlayerCommand::BuyItem(affordable[pick]));
                                continue;
                            }
                        }
                    }
                }
            }
            let enemy_nexus = self.content.map.nexus_position(view.team.enemy());
            let hero_target = self.nearest_enemy(
                view.position,
                view.team,
                Fix::from_i32(13),
                &visibility,
                UnitKind::is_hero,
            );
            let minion_target = self.nearest_enemy(
                view.position,
                view.team,
                Fix::from_i32(10),
                &visibility,
                UnitKind::is_minion,
            );
            let structure_target = self.nearest_enemy(
                view.position,
                view.team,
                Fix::from_i32(200),
                &visibility,
                UnitKind::is_structure,
            );
            let target = hero_target.or(minion_target).or(structure_target);

            let command = if let Some(target) = target {
                let distance_sq = view.position.distance_sq(target.position);
                let defensive = i64::from(view.health) * 100 <= i64::from(view.max_health) * 45;
                if defensive && view.cooldowns.0[1] == 0 && cast_roll < 80 {
                    if view.mana >= view.ability_mana[1] {
                        Some(PlayerCommand::CastAbility {
                            slot: 1,
                            target: view.position,
                        })
                    } else {
                        None
                    }
                } else if view.cooldowns.0[0] == 0
                    && view.mana >= view.ability_mana[0]
                    && distance_sq <= view.ability_range[0] * view.ability_range[0]
                    && cast_roll < 85
                {
                    Some(PlayerCommand::CastAbility {
                        slot: 0,
                        target: target.position
                            + FixVec2::new(Fix::from_i32(jitter_x), Fix::from_i32(jitter_y)),
                    })
                } else if distance_sq > view.attack_range_sq
                    && distance_sq <= Fix::from_i32(12) * Fix::from_i32(12)
                    && view.cooldowns.0[2] == 0
                    && view.mana >= view.ability_mana[2]
                    && distance_sq <= view.ability_range[2] * view.ability_range[2]
                    && cast_roll < 75
                {
                    Some(PlayerCommand::CastAbility {
                        slot: 2,
                        target: target.position
                            + FixVec2::new(Fix::from_i32(jitter_x), Fix::from_i32(jitter_y)),
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

    fn bot_visibility_masks(&mut self) -> [u64; 16] {
        let mut sources = core::mem::take(&mut self.bot_sources);
        sources.clear();
        sources.resize(MAX_UNITS * 2, (FixVec2::ZERO, Fix::ZERO));
        let mut source_count = [0usize; 2];
        for (_entity, (team, health, position, vision)) in
            self.world
                .query::<(&Team, &Health, &Position, &VisionRange)>()
        {
            if health.current <= 0 {
                continue;
            }
            let index = team.index();
            // Store both teams in one fixed array, offset by team, so both fog masks are
            // built from a single world query and candidate pass.
            if source_count[index] < MAX_UNITS {
                let slot = index * MAX_UNITS + source_count[index];
                sources[slot] = (position.0, vision.0);
                source_count[index] += 1;
            }
        }
        let mut team_masks = [0u64; 16];
        for (entity, (team, kind, position, health)) in
            self.world.query::<(&Team, &UnitKind, &Position, &Health)>()
        {
            let slot = entity.index() as usize;
            if slot >= MAX_UNITS {
                continue;
            }
            let index = team.index();
            let allied = |viewer: usize| viewer == index;
            let visible_from =
                |viewer: usize| {
                    let start = viewer * MAX_UNITS;
                    let count = source_count[viewer];
                    sources[start..start + count]
                        .iter()
                        .any(|&(source, range)| {
                            source.distance_sq(position.0) <= range * range
                                && self.content.map.walls.iter().all(|wall| {
                                    !segments_intersect(source, position.0, wall.a, wall.b)
                                })
                        })
                };
            let dead = health.current <= 0;
            for viewer in 0..2 {
                if allied(viewer) || kind.is_structure() || (!dead && visible_from(viewer)) {
                    team_masks[viewer * 8 + slot / 64] |= 1 << (slot % 64);
                }
            }
        }
        self.bot_sources = sources;
        team_masks
    }

    fn visible_to(masks: &[u64; 16], team: Team, entity: Entity) -> bool {
        let slot = entity.index() as usize;
        slot < MAX_UNITS && masks[team.index() * 8 + slot / 64] & (1 << (slot % 64)) != 0
    }

    fn nearest_enemy(
        &self,
        position: FixVec2,
        team: Team,
        radius: Fix,
        masks: &[u64; 16],
        filter: impl Fn(UnitKind) -> bool,
    ) -> Option<TargetRef> {
        let mut best: Option<(Fix, TargetRef)> = None;
        self.grid.for_each_in_circle(position, radius, |candidate| {
            if candidate.team == team
                || !filter(candidate.kind)
                || !Self::visible_to(masks, team, candidate.entity)
            {
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

    fn unit_rewards(&self, kind: UnitKind) -> (u32, u32) {
        let reward = match kind {
            UnitKind::MeleeMinion => self.content.rules.rewards.melee_minion,
            UnitKind::RangedMinion => self.content.rules.rewards.ranged_minion,
            UnitKind::Tower => self.content.rules.rewards.tower,
            UnitKind::Nexus => self.content.rules.rewards.nexus,
            UnitKind::Hero => self.content.rules.rewards.hero,
        };
        (reward.xp, reward.gold)
    }
}

fn cross(a: FixVec2, b: FixVec2) -> Fix {
    a.x * b.y - a.y * b.x
}

fn segments_intersect(a: FixVec2, b: FixVec2, c: FixVec2, d: FixVec2) -> bool {
    let ab = b - a;
    let cd = d - c;
    let ac = c - a;
    let ad = d - a;
    let ca = a - c;
    let cb = b - c;
    let d1 = cross(ab, ac);
    let d2 = cross(ab, ad);
    let d3 = cross(cd, ca);
    let d4 = cross(cd, cb);
    ((d1 > Fix::ZERO) != (d2 > Fix::ZERO)) && ((d3 > Fix::ZERO) != (d4 > Fix::ZERO))
}

fn clamp_to_extent(position: FixVec2, extent: Fix) -> FixVec2 {
    let min = Fix::from_i32(2);
    let max = extent - Fix::from_i32(2);
    FixVec2::new(position.x.clamp(min, max), position.y.clamp(min, max))
}

fn hash_team(world: &World, entity: Entity, hasher: &mut StableHasher) {
    if let Some(team) = world.get::<Team>(entity) {
        hasher.write_u32(HASH_TEAM);
        hasher.write_u32(team.index() as u32);
    }
}

fn hash_runtime_area_effects(hasher: &mut StableHasher, effects: &AreaEffectList) {
    hasher.write_u32(u32::try_from(effects.iter().count()).unwrap_or(u32::MAX));
    for effect in effects.iter() {
        match effect {
            AreaEffect::None => {}
            AreaEffect::Damage {
                amount,
                damage_type,
            } => {
                hasher.write_u32(0);
                hasher.write_u32(*amount as u32);
                hasher.write_u32(*damage_type as u32);
            }
            AreaEffect::ApplyModifier {
                modifier,
                duration_ticks,
                factor,
                amount,
                damage_type,
            } => {
                hasher.write_u32(1);
                hasher.write_u32(u32::from(*modifier));
                hasher.write_u32(*duration_ticks);
                hasher.write_u64(factor.to_bits() as u64);
                hasher.write_u32(*amount as u32);
                hasher.write_u32(*damage_type as u32);
            }
        }
    }
}

fn hash_kind(world: &World, entity: Entity, hasher: &mut StableHasher) {
    if let Some(kind) = world.get::<UnitKind>(entity) {
        hasher.write_u32(HASH_KIND);
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
    use moba_game::EffectList;
    use std::any::TypeId;

    fn idle(picks: [u8; MAX_PLAYERS]) -> Simulation {
        Simulation::new(SimConfig {
            bot_mask: 0,
            spawn_waves: false,
            hero_picks: picks,
            ..SimConfig::default()
        })
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
        assert_eq!(sim.outcome(), Some(Team::Blue));
        assert!(
            sim.events()
                .iter()
                .any(|event| matches!(event, SimEvent::MatchEnded { winner: Team::Blue }))
        );
        assert!(!sim.world.entities().any(|entity| entity == nexus));
    }

    #[test]
    fn nexus_win_rule_can_be_disabled() {
        let mut content = GameContent::default();
        content.rules.nexus_win = false;
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
            SimConfig {
                bot_mask: 0,
                spawn_waves: true,
                ..SimConfig::default()
            },
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
            SimConfig {
                bot_mask: 0x3,
                spawn_waves: false,
                ..SimConfig::default()
            },
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
            SimConfig {
                bot_mask: 0,
                spawn_waves: false,
                ..SimConfig::default()
            },
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
            SimConfig {
                hero_picks: picks,
                bot_mask: 0,
                spawn_waves: false,
                ..SimConfig::default()
            },
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
            SimConfig {
                hero_picks: [0; 10],
                bot_mask: 0,
                spawn_waves: false,
                ..SimConfig::default()
            },
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
        let config = SimConfig {
            hero_picks: [0; 10],
            bot_mask: 0,
            spawn_waves: false,
            ..SimConfig::default()
        };
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
        };
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
            let mut sim = Simulation::new(SimConfig {
                seed,
                spawn_waves: false,
                ..SimConfig::default()
            });
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
            moba_game::load_content_toml(include_str!("../../../content/ability-pack.toml"))
                .expect("ability pack");
        let index = content
            .abilities
            .iter()
            .position(|ability| ability.name == "Rooting Shot")
            .expect("rooting shot");
        let mut content = content;
        content.heroes[0].abilities = [u8::try_from(index).expect("id"), 1, 2];
        let mut sim = Simulation::new_with_content(
            SimConfig {
                bot_mask: 0,
                spawn_waves: false,
                hero_picks: [0; MAX_PLAYERS],
                ..SimConfig::default()
            },
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
            moba_game::load_content_toml(include_str!("../../../content/ability-pack.toml"))
                .expect("ability pack");
        let index = content
            .abilities
            .iter()
            .position(|ability| ability.name == "Cinder Zone")
            .expect("cinder zone");
        let mut content = content;
        content.heroes[0].abilities = [u8::try_from(index).expect("id"), 1, 2];
        let mut sim = Simulation::new_with_content(
            SimConfig {
                bot_mask: 0,
                spawn_waves: false,
                hero_picks: [0; MAX_PLAYERS],
                ..SimConfig::default()
            },
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
            SimConfig {
                bot_mask: 0,
                ..SimConfig::default()
            },
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
            SimConfig {
                bot_mask: 0,
                ..SimConfig::default()
            },
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
            SimConfig {
                bot_mask: 0,
                ..SimConfig::default()
            },
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
            SimConfig {
                bot_mask: 0,
                ..SimConfig::default()
            },
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
            SimConfig {
                bot_mask: 0,
                spawn_waves: false,
                hero_picks: [0; MAX_PLAYERS],
                ..SimConfig::default()
            },
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
            SimConfig {
                bot_mask: 0,
                ..SimConfig::default()
            },
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
        let content = moba_game::load_content_toml(include_str!("../../../content/map-pack.toml"))
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
                SimConfig {
                    bot_mask: 0,
                    spawn_waves: false,
                    hero_picks: [0; MAX_PLAYERS],
                    ..SimConfig::default()
                },
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
            SimConfig {
                bot_mask: 0,
                spawn_waves: false,
                hero_picks: [0; MAX_PLAYERS],
                ..SimConfig::default()
            },
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
        let mut sim =
            Simulation::new_with_content(SimConfig::default(), std::sync::Arc::new(content));
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
        assert_eq!(sim.outcome(), Some(Team::Red));
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
