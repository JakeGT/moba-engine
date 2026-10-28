//! Headless fixed-tick MOBA simulation.

use core::fmt;
use std::sync::Arc;

use moba_core::{Fix, FixVec2, Rng, Tick};
use moba_ecs::{Entity, World};
use moba_game::{AreaEffectList, DamageType, GameContent, Team};
use moba_nav::SpatialGrid;

use crate::components::{Cooldowns, Dead, Health, Position, Progression, UnitKind};

mod abilities;
mod bots;
mod combat;
mod deaths;
mod hashing;
mod inputs;
mod items;
mod movement;
mod spawn;
mod status;
mod vision;

/// Number of player slots (5v5).
pub const MAX_PLAYERS: usize = 10;
/// Hard cap on live entities. Waves stop when the cap is reached.
pub const MAX_UNITS: usize = 512;

/// Hard cap on in-flight projectiles.
pub const MAX_PROJECTILES: usize = 64;
/// Hard cap on active zones.
pub const MAX_ZONES: usize = 16;
/// Hard cap on queued events; events beyond this are dropped, never allocated.
pub const MAX_EVENTS: usize = 256;
/// Most [`SimEvent::BasicAttack`] events in the queue at once. Basic attacks are frequent
/// presentation-only events, so they may fill at most half of [`MAX_EVENTS`]; the rest is
/// kept for casts, deaths and the other events, which a busy tick never crowds out.
pub const MAX_ATTACK_EVENTS: usize = MAX_EVENTS / 2;
/// Ticks between minion waves (30 seconds at 30 Hz).
pub const WAVE_INTERVAL_TICKS: u32 = 900;
/// First wave delay (1 second).
pub const WAVE_DELAY_TICKS: u32 = 30;
/// Hero respawn delay (10 seconds).
pub const HERO_RESPAWN_TICKS: u32 = 300;

/// Fixed-point duration of one tick (`1/30` second).
pub const TICK_DURATION: Fix = Fix::from_bits(143_165_576);
const ONE_TICK: Fix = TICK_DURATION;

/// Simulation configuration.
///
/// `#[non_exhaustive]`: start from [`SimConfig::default()`] and adjust it with the
/// `with_*` builders (or assign the public fields), so new options can be added without
/// breaking callers:
///
/// ```
/// use moba_sim::SimConfig;
///
/// let config = SimConfig::default().with_seed(7).with_bot_mask(0).with_spawn_waves(false);
/// assert_eq!(config.seed, 7);
/// ```
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
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

impl SimConfig {
    /// Returns this config with [`SimConfig::seed`] set.
    #[must_use]
    pub const fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Returns this config with [`SimConfig::bot_mask`] set.
    #[must_use]
    pub const fn with_bot_mask(mut self, bot_mask: u16) -> Self {
        self.bot_mask = bot_mask;
        self
    }

    /// Returns this config with [`SimConfig::hero_picks`] set.
    #[must_use]
    pub const fn with_hero_picks(mut self, hero_picks: [u8; MAX_PLAYERS]) -> Self {
        self.hero_picks = hero_picks;
        self
    }

    /// Returns this config with [`SimConfig::spawn_waves`] set.
    #[must_use]
    pub const fn with_spawn_waves(mut self, spawn_waves: bool) -> Self {
        self.spawn_waves = spawn_waves;
        self
    }
}

/// A command from one player for the current tick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
#[non_exhaustive]
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

/// How a finished match ended.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MatchOutcome {
    /// One team won.
    Winner(Team),
    /// Both teams met a win condition in the same tick (for example both nexuses fell, or
    /// both teams reached the score limit), so neither side wins.
    Draw,
}

impl MatchOutcome {
    /// The winning team, or `None` for a draw.
    #[inline]
    #[must_use]
    pub const fn winner(self) -> Option<Team> {
        match self {
            Self::Winner(team) => Some(team),
            Self::Draw => None,
        }
    }
}

/// Something observable that happened during a tick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
    /// A basic attack landed. Presentation only (animation hit frames, combat text): it
    /// never feeds the simulation, the state hash or replays.
    ///
    /// Basic attacks resolve instantly in the tick their attacker's cooldown reaches zero,
    /// so this tick is both the release and the hit; the damage is visible in the state
    /// after this tick (`current_tick()` once [`Simulation::tick`] returns). The attacker
    /// cannot attack again for `interval_ticks`. At most [`MAX_ATTACK_EVENTS`] are queued
    /// between clears.
    BasicAttack {
        /// Attacking entity.
        attacker: Entity,
        /// Attacking team.
        team: Team,
        /// Attacked entity.
        target: Entity,
        /// Ticks until the attacker's next attack (its attack interval).
        interval_ticks: u32,
    },
    /// The match ended (nexus destroyed, score limit reached or surrender).
    MatchEnded {
        /// Winning team, or `None` when the match ended in a draw.
        winner: Option<Team>,
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
#[non_exhaustive]
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

impl ObservedUnit {
    /// A unit with no health, mana, progression, items or status: `hero_slot` is `None`,
    /// `hero_pick` and every item slot are `0xFF`, and every flag is clear.
    ///
    /// The simulation fills every field in [`Simulation::observed_units`]; this constructor
    /// is for tests and custom snapshot producers, which then set the public fields they
    /// need. `ObservedUnit` is `#[non_exhaustive]`, so it cannot be built with a struct
    /// literal outside this crate.
    #[must_use]
    pub const fn new(entity: Entity, kind: UnitKind, team: Team, position: FixVec2) -> Self {
        Self {
            entity,
            kind,
            team,
            position,
            health: 0,
            max_health: 0,
            mana: 0,
            max_mana: 0,
            dead: false,
            hero_slot: None,
            hero_pick: 0xFF,
            cooldowns: [0; 3],
            level: 0,
            xp: 0,
            gold: 0,
            items: [0xFF; 6],
            shielded: false,
            slowed: false,
            rooted: false,
            silenced: false,
            burning: false,
            hasted: false,
            status_bits: 0,
        }
    }
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
    outcome: Option<MatchOutcome>,
    /// Win conditions met during the current tick, indexed by the team they favour. They are
    /// resolved together at the end of the tick so the result never depends on processing
    /// order; always `[false; 2]` between ticks, so it is not part of the state hash.
    pending_wins: [bool; 2],
    blue_score: u32,
    red_score: u32,
    projectiles: Vec<Projectile>,
    zones: Vec<Zone>,
    events: Vec<SimEvent>,
    /// [`SimEvent::BasicAttack`] entries in `events` (bounded by [`MAX_ATTACK_EVENTS`]).
    attack_events: usize,
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
            pending_wins: [false; 2],
            blue_score: 0,
            red_score: 0,
            projectiles: Vec::with_capacity(MAX_PROJECTILES),
            zones: Vec::with_capacity(MAX_ZONES),
            events: Vec::with_capacity(MAX_EVENTS),
            attack_events: 0,
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

    /// How the match ended, once it has (`None` while it is still running).
    #[inline]
    #[must_use]
    pub const fn outcome(&self) -> Option<MatchOutcome> {
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

    /// Events produced since the queue was last cleared.
    #[inline]
    #[must_use]
    pub fn events(&self) -> &[SimEvent] {
        &self.events
    }

    /// Clears the event queue.
    pub fn clear_events(&mut self) {
        self.events.clear();
        self.attack_events = 0;
    }

    /// Advances the simulation by one tick using the supplied inputs.
    pub fn tick(&mut self, inputs: &TickInputs) {
        if self.outcome.is_some() {
            return;
        }
        if self
            .tick
            .get()
            .is_multiple_of(self.content.rules.gold_tick_interval)
        {
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
        self.resolve_outcome();
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

    /// Records that `team` met a win condition this tick.
    fn queue_win(&mut self, team: Team) {
        self.pending_wins[team.index()] = true;
    }

    /// Resolves the win conditions met this tick. If both teams met one in the same tick
    /// the match is a draw, so the result does not depend on processing order.
    fn resolve_outcome(&mut self) {
        let [blue, red] = core::mem::take(&mut self.pending_wins);
        if self.outcome.is_some() {
            return;
        }
        let outcome = match (blue, red) {
            (true, true) => MatchOutcome::Draw,
            (true, false) => MatchOutcome::Winner(Team::Blue),
            (false, true) => MatchOutcome::Winner(Team::Red),
            (false, false) => return,
        };
        self.outcome = Some(outcome);
        self.push_event(SimEvent::MatchEnded {
            winner: outcome.winner(),
        });
    }

    fn push_event(&mut self, event: SimEvent) {
        if matches!(event, SimEvent::BasicAttack { .. }) {
            // Low priority: attacks never take the slots kept for the other events.
            if self.attack_events < MAX_ATTACK_EVENTS && self.events.len() < MAX_EVENTS {
                self.events.push(event);
                self.attack_events += 1;
            }
            return;
        }
        if self.events.len() < MAX_EVENTS {
            self.events.push(event);
        } else if matches!(event, SimEvent::MatchEnded { .. }) {
            // Match end is a terminal event; it must not be lost when the bounded queue is
            // full of routine combat events.
            if matches!(self.events[MAX_EVENTS - 1], SimEvent::BasicAttack { .. }) {
                self.attack_events -= 1;
            }
            self.events[MAX_EVENTS - 1] = event;
        }
    }
}

fn clamp_to_extent(position: FixVec2, extent: Fix) -> FixVec2 {
    let min = Fix::from_i32(2);
    let max = extent - Fix::from_i32(2);
    FixVec2::new(position.x.clamp(min, max), position.y.clamp(min, max))
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
mod tests;
