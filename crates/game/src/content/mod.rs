//! Runtime content definitions shared by the simulation and server.
//!
//! The default catalogue is derived from the compile-time [`ITEMS`] table, so
//! existing games and replays keep working. Servers can load a different item catalogue and
//! publish it to clients in the handshake; clients render and shop against the received
//! catalogue. The catalogue is deliberately small and fixed-capacity on the wire.

mod error;
mod hash;
mod load;
mod manifest;
mod schema;
mod spec;
mod validate;

#[cfg(test)]
mod tests;

pub use error::{ContentError, ContentLoadError};
pub use load::{load_file, load_toml};
pub use manifest::{BUILTIN_CONTENT_VERSION, ContentManifest, MAX_MANIFEST_FIELD, PackAttestation};
pub use spec::{
    AbilitySpec, HeroSpec, ItemSpec, LaneSpec, MapSpec, MapTheme, MinionStatsSpec, ModifierDef,
    ModifierKind, RewardsSpec, RulesSpec, StructureSpec, StructureStatsSpec, UnitRewardsSpec,
    WallSpec,
};
pub use validate::valid_visual_name;

pub use moba_assets::{AssetEntry, AssetError, AssetManifest};

use crate::{ABILITIES, ITEMS, MAP};

/// Maximum length of a visual asset name (`heroes.model`, `map.visuals`) in bytes.
///
/// Names are 1..=16 of `[A-Za-z0-9_-]`: they cross the wire and a client resolves them
/// against its own asset root, so they can never contain a path separator or drive colon.
pub const MAX_VISUAL_NAME: usize = 16;

/// Maximum number of items in a content pack (matches the wire catalogue).
pub const MAX_CONTENT_ITEMS: usize = 16;
/// Maximum item display-name length in bytes.
pub const MAX_ITEM_NAME: usize = 16;
/// Maximum heroes in a content pack.
pub const MAX_CONTENT_HEROES: usize = 8;
/// Maximum hero display-name length in bytes.
pub const MAX_HERO_NAME: usize = 16;
/// Maximum abilities in a content pack.
pub const MAX_CONTENT_ABILITIES: usize = 24;
/// Maximum ability display-name length in bytes.
pub const MAX_ABILITY_NAME: usize = 16;
/// Maximum lanes in a content pack.
pub const MAX_CONTENT_LANES: usize = 4;
/// Maximum waypoints per lane.
pub const MAX_LANE_POINTS: usize = 16;
/// Maximum lane display-name length in bytes.
pub const MAX_LANE_NAME: usize = 16;
/// Maximum structures in a content pack.
pub const MAX_CONTENT_STRUCTURES: usize = 32;
/// Maximum vision-blocking wall segments.
pub const MAX_CONTENT_WALLS: usize = 32;
/// Maximum modifier definitions in a content pack.
pub const MAX_CONTENT_MODIFIERS: usize = 16;
/// Default modifier ids (stable content order).
pub const MODIFIER_SHIELD: u16 = 0;
/// Default slow modifier id.
pub const MODIFIER_SLOW: u16 = 1;
/// Default haste modifier id.
pub const MODIFIER_HASTE: u16 = 2;
/// Default root modifier id.
pub const MODIFIER_ROOT: u16 = 3;
/// Default silence modifier id.
pub const MODIFIER_SILENCE: u16 = 4;
/// Default burn modifier id.
pub const MODIFIER_BURN: u16 = 5;
/// Valid map coordinates are `0..=MAX_MAP_COORD` on both axes.
pub const MAX_MAP_COORD: i32 = 256;
/// Upper bound for health/damage/gold-like integer stats.
pub const MAX_STAT: i32 = 10_000_000;
/// Upper bound for item bonus stats.
pub const MAX_ITEM_STAT: i32 = 1_000_000;
/// Upper bound for cooldowns and attack intervals.
pub const MAX_COOLDOWN_TICKS: u32 = 1_000_000;
/// Upper bound for status durations.
pub const MAX_DURATION_TICKS: u32 = 1_000_000;
/// Upper bound for ability ranges/radii/speeds in world units.
pub const MAX_RANGE: i32 = 1_000;
/// Upper bound for movement speed in world units per second.
pub const MAX_SPEED_PER_SEC: i32 = 100;
/// Largest accepted haste multiplier.
pub const MAX_HASTE_FACTOR: i32 = 3;
/// Upper bound for armour/magic resistance.
pub const MAX_RESIST: i32 = 100_000;

/// A validated content pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameContent {
    /// Pack identity/version metadata.
    pub manifest: ContentManifest,
    /// Item catalogue in stable id order.
    pub items: Vec<ItemSpec>,
    /// Hero catalogue in stable id order.
    pub heroes: Vec<HeroSpec>,
    /// Ability catalogue in stable id order.
    pub abilities: Vec<AbilitySpec>,
    /// Modifier definitions in stable id order.
    pub modifiers: Vec<ModifierDef>,
    /// Tunable match rules.
    pub rules: RulesSpec,
    /// Map geometry.
    pub map: MapSpec,
    /// Optional publisher attestation (not part of the content hash).
    pub attestation: Option<PackAttestation>,
    /// The pack's `[assets]` manifest: names to pack-relative files. Pack-local authoring
    /// data, like `attestation` it is **not** hashed, signed or sent on the wire (clients
    /// resolve the hashed `HeroSpec::model` / `MapSpec::visuals` *names* against their own
    /// asset root). Empty for built-in and wire-reconstructed content.
    pub assets: AssetManifest,
}

fn default_modifiers() -> Vec<ModifierDef> {
    [
        ("Shield", ModifierKind::Shield),
        ("Slow", ModifierKind::Slow),
        ("Haste", ModifierKind::Haste),
        ("Root", ModifierKind::Root),
        ("Silence", ModifierKind::Silence),
        ("Burn", ModifierKind::Burn),
    ]
    .into_iter()
    .map(|(name, kind)| ModifierDef {
        name: String::from(name),
        kind,
        bonus_armor: 0,
        bonus_attack_damage: 0,
        on_expire_damage: 0,
        on_damaged_damage: 0,
        on_tick_damage: 0,
        status_bits: 0,
    })
    .collect()
}

impl Default for GameContent {
    fn default() -> Self {
        Self {
            manifest: ContentManifest::default(),
            items: ITEMS.iter().map(ItemSpec::from).collect(),
            heroes: crate::HEROES.iter().map(HeroSpec::from).collect(),
            abilities: ABILITIES.iter().map(AbilitySpec::from).collect(),
            modifiers: default_modifiers(),
            rules: RulesSpec::default(),
            map: MapSpec::from(&MAP),
            attestation: None,
            assets: AssetManifest::default(),
        }
    }
}
