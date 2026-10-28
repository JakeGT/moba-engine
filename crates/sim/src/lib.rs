//! Headless, deterministic, fixed-tick (30 Hz) MOBA simulation.
//!
//! The simulation is authoritative: it consumes [`TickInputs`] (intent only), never reads a
//! wall clock, never uses a randomly-seeded hash map, and stores every gameplay value in
//! integers or Q32.32 fixed-point. The same `(seed, inputs)` sequence therefore produces the
//! same state on every platform.
//!
//! Milestone M4 scope: 5v5 heroes, three-lane minion waves, towers and nexuses, basic
//! attacks, three abilities per hero, respawns, deterministic bots, and a state hash for
//! replay/desync checks.

#![forbid(unsafe_code)]

mod components;
mod replay;
mod sim;

pub use components::{
    AttackDamageType, AttackStats, AttackTarget, BaseMoveSpeed, Cooldowns, Dead, Health,
    HeroIdentity, HeroSlot, ItemCooldowns, LaneWalker, LastHitBy, MAX_MODIFIERS, Mana, Modifier,
    Modifiers, MoveSpeed, MoveTarget, Position, Progression, Regen, Resistances, UnitKind,
    VisionRange,
};
pub use moba_game::ModifierKind;
pub use replay::{Replay, ReplayError, ReplayResult, SIM_LOGIC_VERSION};
pub use sim::{
    BotPolicy, DefaultBotPolicy, HERO_RESPAWN_TICKS, MAX_EVENTS, MAX_PLAYERS, MAX_PROJECTILES,
    MAX_UNITS, MAX_ZONES, MatchOutcome, ObservedUnit, PlayerCommand, SimConfig, SimEvent,
    Simulation, TICK_DURATION, TickInputs, WAVE_DELAY_TICKS, WAVE_INTERVAL_TICKS,
};
