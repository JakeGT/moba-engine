//! Data-driven MOBA rules: teams, heroes, abilities, units, map and combat maths.
//!
//! This crate contains **data and pure functions only**. It does not own a simulation or
//! know about networking, rendering or wall clocks. Every numeric gameplay value is either
//! an integer or a [`moba_core::Fix`], so the simulation is reproducible bit-for-bit.
//!
//! Content is a static table today; the asset/content milestone (M8) will load these
//! definitions from data files without changing the simulation.

#![forbid(unsafe_code)]

pub mod ability;
pub mod combat;
pub mod content;
pub mod hero;
pub mod item;
pub mod map;
pub mod team;
pub mod units;

pub use ability::{ABILITIES, AbilityDef, AbilityKind};
pub use combat::{DamageType, mitigate, mitigate_typed};
pub use content::{
    ContentError, GameContent, HeroSpec, ItemSpec, MAX_CONTENT_HEROES, MAX_CONTENT_ITEMS,
    MAX_HERO_NAME, MAX_ITEM_NAME,
};
pub use hero::{HEROES, HeroDef, HeroId};
pub use item::{
    EMPTY_ITEM, GOLD_PER_INTERVAL, GOLD_TICK_INTERVAL, ITEM_SLOTS, ITEMS, ItemDef, LEVEL_CAP,
    LEVEL_GROWTH, LevelGrowth, SHOP_RADIUS, STARTING_GOLD, XP_RADIUS, xp_to_next,
};
pub use map::{LANES, LaneDef, MAP, MapDef, STRUCTURES, StructureDef, StructureKind};
pub use team::Team;
pub use units::{MELEE_MINION, MinionDef, NEXUS, RANGED_MINION, StructureStats, TOWER};
