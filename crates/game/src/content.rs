//! Runtime content definitions shared by the simulation and server.
//!
//! The default catalogue is derived from the compile-time [`ITEMS`] table, so
//! existing games and replays keep working. Servers can load a different item catalogue and
//! publish it to clients in the handshake; clients render and shop against the received
//! catalogue. The catalogue is deliberately small and fixed-capacity on the wire.

use std::path::Path;

use moba_core::{Fix, FixVec2};
use serde::Deserialize;

use crate::{
    ABILITIES, AbilityDef, AreaEffect, AreaEffectList, DEFAULT_MAP_EXTENT, DamageType, Effect,
    EffectList, ITEMS, ItemDef, MAP, MapDef, StructureKind, Targeting, Team,
};

/// Maximum length of a manifest field.
pub const MAX_MANIFEST_FIELD: usize = 32;

/// Human-readable identity of a content pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentManifest {
    /// Pack name.
    pub name: String,
    /// Pack version string.
    pub version: String,
    /// Author/maintainer.
    pub author: String,
}

impl Default for ContentManifest {
    fn default() -> Self {
        Self {
            name: String::from("Core"),
            version: String::from(env!("CARGO_PKG_VERSION")),
            author: String::from("MOBA Engine"),
        }
    }
}

/// Optional Ed25519 attestation of a pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackAttestation {
    /// Publisher Ed25519 public key.
    pub publisher: [u8; 32],
    /// Signature over [`GameContent::signing_message`].
    pub signature: [u8; 64],
}

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
/// Upper bound for armour/magic resistance.
pub const MAX_RESIST: i32 = 100_000;

/// Fixed status-modifier behaviour kinds.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ModifierKind {
    /// Temporary damage absorption.
    Shield = 0,
    /// Movement multiplier below one.
    Slow = 1,
    /// Movement multiplier above one.
    Haste = 2,
    /// Prevents movement orders.
    Root = 3,
    /// Prevents ability casts.
    Silence = 4,
    /// Damage over time.
    Burn = 5,
}

/// An owned modifier definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModifierDef {
    /// Display name (1..=16 ASCII bytes).
    pub name: String,
    /// Behaviour kind.
    pub kind: ModifierKind,
}

/// An owned, runtime item definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemSpec {
    /// Display name (1..=16 ASCII bytes).
    pub name: String,
    /// Cost in gold.
    pub cost: u32,
    /// Bonus maximum/current health.
    pub health: i32,
    /// Bonus maximum/current mana.
    pub mana: i32,
    /// Bonus attack damage.
    pub attack_damage: i32,
    /// Bonus armour.
    pub armor: i32,
    /// Bonus magic resistance.
    pub magic_resist: i32,
    /// Bonus movement speed in world units per second.
    pub move_speed_per_sec: Fix,
}

/// An owned lane path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneSpec {
    /// Display name (1..=16 ASCII bytes).
    pub name: String,
    /// Waypoints from blue base to red base.
    pub path: Vec<FixVec2>,
}

impl LaneSpec {
    /// Number of waypoints.
    #[must_use]
    pub fn len(&self) -> usize {
        self.path.len()
    }

    /// Returns `true` when the lane has no waypoints.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.path.is_empty()
    }

    /// Waypoint at `index` measured from `team`'s own base.
    #[must_use]
    pub fn point(&self, team: Team, index: usize) -> FixVec2 {
        match team {
            Team::Blue => self.path[index.min(self.path.len() - 1)],
            Team::Red => self.path[self.path.len() - 1 - index.min(self.path.len() - 1)],
        }
    }
}

/// An owned placed structure.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct StructureSpec {
    /// Owning team.
    pub team: Team,
    /// World position.
    pub position: FixVec2,
    /// Structure type.
    pub kind: StructureKind,
    /// Optional lane index.
    pub lane: Option<u8>,
}

/// A vision-blocking wall segment (does not block movement).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WallSpec {
    /// First endpoint.
    pub a: FixVec2,
    /// Second endpoint.
    pub b: FixVec2,
}

/// Runtime minion stats.
#[allow(missing_docs)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MinionStatsSpec {
    pub max_health: i32,
    pub attack_damage: i32,
    pub attack_range: Fix,
    pub attack_interval_ticks: u32,
    pub move_speed_per_sec: Fix,
    pub armor: i32,
    pub magic_resist: i32,
    pub vision_range: Fix,
}

/// Runtime tower/nexus stats.
#[allow(missing_docs)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct StructureStatsSpec {
    pub max_health: i32,
    pub attack_damage: i32,
    pub attack_range: Fix,
    pub attack_interval_ticks: u32,
    pub armor: i32,
    pub magic_resist: i32,
    pub vision_range: Fix,
}

/// Tunable match rules shared by simulation and server.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RulesSpec {
    /// Melee minion stats.
    pub melee_minion: MinionStatsSpec,
    /// Ranged minion stats.
    pub ranged_minion: MinionStatsSpec,
    /// Tower stats.
    pub tower: StructureStatsSpec,
    /// Nexus stats.
    pub nexus: StructureStatsSpec,
    /// Maximum hero level.
    pub level_cap: u8,
    /// Base XP for the next level.
    pub xp_base: u32,
    /// Additional XP per level.
    pub xp_per_level: u32,
    /// Vision radius for melee minions.
    pub melee_vision: Fix,
    /// Vision radius for ranged minions.
    pub ranged_vision: Fix,
    /// Vision radius for towers.
    pub tower_vision: Fix,
    /// Vision radius for nexuses.
    pub nexus_vision: Fix,
    /// Heroes per team (1..=5).
    pub team_size: u8,
    /// Fill empty player slots with bots.
    pub bot_fill: bool,
    /// Hero respawn delay at level 1, in ticks.
    pub hero_respawn_ticks: u32,
    /// Extra respawn ticks per hero level.
    pub respawn_per_level: u32,
    /// Ticks between minion waves.
    pub wave_interval_ticks: u32,
    /// Delay before the first wave, in ticks.
    pub first_wave_ticks: u32,
}

impl Default for RulesSpec {
    fn default() -> Self {
        Self {
            melee_minion: MinionStatsSpec {
                max_health: 320,
                attack_damage: 14,
                attack_range: Fix::from_i32(2),
                attack_interval_ticks: 30,
                move_speed_per_sec: Fix::from_i32(4),
                armor: 10,
                magic_resist: 5,
                vision_range: Fix::from_i32(9),
            },
            ranged_minion: MinionStatsSpec {
                max_health: 220,
                attack_damage: 20,
                attack_range: Fix::from_i32(7),
                attack_interval_ticks: 32,
                move_speed_per_sec: Fix::from_i32(4),
                armor: 5,
                magic_resist: 5,
                vision_range: Fix::from_i32(10),
            },
            tower: StructureStatsSpec {
                max_health: 2200,
                attack_damage: 105,
                attack_range: Fix::from_i32(10),
                attack_interval_ticks: 30,
                armor: 40,
                magic_resist: 40,
                vision_range: Fix::from_i32(13),
            },
            nexus: StructureStatsSpec {
                max_health: 4800,
                attack_damage: 85,
                attack_range: Fix::from_i32(9),
                attack_interval_ticks: 30,
                armor: 30,
                magic_resist: 30,
                vision_range: Fix::from_i32(14),
            },
            level_cap: 15,
            xp_base: 180,
            xp_per_level: 100,
            melee_vision: Fix::from_i32(9),
            ranged_vision: Fix::from_i32(10),
            tower_vision: Fix::from_i32(13),
            nexus_vision: Fix::from_i32(14),
            team_size: 5,
            bot_fill: true,
            hero_respawn_ticks: 300,
            respawn_per_level: 0,
            wave_interval_ticks: 900,
            first_wave_ticks: 30,
        }
    }
}

/// An owned runtime map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapSpec {
    /// Display name.
    pub name: String,
    /// Square world extent in world units.
    pub extent: Fix,
    /// Lanes in stable order.
    pub lanes: Vec<LaneSpec>,
    /// Structures in stable order.
    pub structures: Vec<StructureSpec>,
    /// Vision-blocking wall segments.
    pub walls: Vec<WallSpec>,
    /// Blue spawn.
    pub blue_spawn: FixVec2,
    /// Red spawn.
    pub red_spawn: FixVec2,
}

impl From<&MapDef> for MapSpec {
    fn from(map: &MapDef) -> Self {
        Self {
            name: String::from("Map"),
            extent: Fix::from_i32(DEFAULT_MAP_EXTENT),
            lanes: map
                .lanes
                .iter()
                .map(|lane| LaneSpec {
                    name: lane.name.to_owned(),
                    path: lane.path.to_vec(),
                })
                .collect(),
            structures: map
                .structures
                .iter()
                .map(|structure| StructureSpec {
                    team: structure.team,
                    position: structure.position,
                    kind: structure.kind,
                    lane: structure.lane,
                })
                .collect(),
            walls: Vec::new(),
            blue_spawn: map.blue_spawn,
            red_spawn: map.red_spawn,
        }
    }
}

impl MapSpec {
    /// Team spawn point.
    #[must_use]
    pub const fn spawn(&self, team: Team) -> FixVec2 {
        match team {
            Team::Blue => self.blue_spawn,
            Team::Red => self.red_spawn,
        }
    }

    /// A hero's spawn position for a slot in `0..5`.
    #[must_use]
    pub fn hero_spawn(&self, team: Team, slot: u8) -> FixVec2 {
        let base = self.spawn(team);
        let offsets = [(0, 0), (4, -7), (4, 7), (-4, -12), (-4, 12)];
        let (dx, dy) = offsets[slot as usize % offsets.len()];
        let dx = if team == Team::Blue { dx } else { -dx };
        FixVec2::new(base.x + Fix::from_i32(dx), base.y + Fix::from_i32(dy))
    }

    /// Nexus position for a team.
    #[must_use]
    pub fn nexus_position(&self, team: Team) -> FixVec2 {
        self.structures
            .iter()
            .find(|structure| structure.team == team && structure.kind == StructureKind::Nexus)
            .map_or(self.spawn(team), |structure| structure.position)
    }
}

/// An owned, runtime ability definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbilitySpec {
    /// Display name.
    pub name: String,
    /// Cooldown in simulation ticks.
    pub cooldown_ticks: u32,
    /// Mana cost.
    pub mana_cost: i32,
    /// Targeting mode.
    pub targeting: Targeting,
    /// Ordered effects.
    pub effects: EffectList,
}

impl AbilitySpec {
    /// Maximum cast range (`0` for self-cast).
    #[must_use]
    pub const fn cast_range(&self) -> Fix {
        self.targeting.range()
    }

    /// Largest display radius across area/zone/projectile effects.
    #[must_use]
    pub fn display_radius(&self) -> Fix {
        let mut radius = Fix::ZERO;
        for effect in self.effects.iter() {
            let candidate = match effect {
                Effect::Projectile { radius, .. }
                | Effect::Area { radius, .. }
                | Effect::Zone { radius, .. } => *radius,
                _ => Fix::ZERO,
            };
            if candidate > radius {
                radius = candidate;
            }
        }
        radius
    }
}

impl From<&AbilityDef> for AbilitySpec {
    fn from(ability: &AbilityDef) -> Self {
        Self {
            name: ability.name.to_owned(),
            cooldown_ticks: ability.cooldown_ticks,
            mana_cost: ability.mana_cost,
            targeting: ability.targeting,
            effects: ability.effects,
        }
    }
}

/// An owned, runtime hero stat definition.
///
/// Ability behaviour still comes from the engine ability library; `abilities` are stable
/// ability ids in Q/W/E order so packs can recombine/tune existing kits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeroSpec {
    /// Display name.
    pub name: String,
    /// Maximum health.
    pub max_health: i32,
    /// Maximum mana.
    pub max_mana: i32,
    /// Health regenerated per tick.
    pub health_regen_per_tick: i32,
    /// Mana regenerated per tick.
    pub mana_regen_per_tick: i32,
    /// Basic attack damage.
    pub attack_damage: i32,
    /// Basic attack range.
    pub attack_range: Fix,
    /// Basic attack interval in ticks.
    pub attack_interval_ticks: u32,
    /// Movement speed in world units per second.
    pub move_speed_per_sec: Fix,
    /// Armour.
    pub armor: i32,
    /// Magic resistance.
    pub magic_resist: i32,
    /// Vision radius.
    pub vision_range: Fix,
    /// Ability ids for Q/W/E.
    pub abilities: [u8; 3],
    /// Health gained per level.
    pub health_per_level: i32,
    /// Mana gained per level.
    pub mana_per_level: i32,
    /// Attack damage gained per level.
    pub attack_damage_per_level: i32,
    /// Armour gained per level.
    pub armor_per_level: i32,
    /// Magic resistance gained per level.
    pub magic_resist_per_level: i32,
}

impl From<&crate::HeroDef> for HeroSpec {
    fn from(hero: &crate::HeroDef) -> Self {
        Self {
            name: hero.name.to_owned(),
            max_health: hero.max_health,
            max_mana: hero.max_mana,
            health_regen_per_tick: hero.health_regen_per_tick,
            mana_regen_per_tick: hero.mana_regen_per_tick,
            attack_damage: hero.attack_damage,
            attack_range: hero.attack_range,
            attack_interval_ticks: hero.attack_interval_ticks,
            move_speed_per_sec: hero.move_speed_per_sec,
            armor: hero.armor,
            magic_resist: hero.magic_resist,
            vision_range: hero.vision_range,
            abilities: hero.abilities,
            health_per_level: crate::LEVEL_GROWTH.health,
            mana_per_level: crate::LEVEL_GROWTH.mana,
            attack_damage_per_level: crate::LEVEL_GROWTH.attack_damage,
            armor_per_level: crate::LEVEL_GROWTH.armor,
            magic_resist_per_level: crate::LEVEL_GROWTH.magic_resist,
        }
    }
}

impl From<&ItemDef> for ItemSpec {
    fn from(item: &ItemDef) -> Self {
        Self {
            name: item.name.to_owned(),
            cost: item.cost,
            health: item.health,
            mana: item.mana,
            attack_damage: item.attack_damage,
            armor: item.armor,
            magic_resist: item.magic_resist,
            move_speed_per_sec: item.move_speed_per_sec,
        }
    }
}

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
        }
    }
}

/// Content validation errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentError {
    /// No items were supplied.
    NoItems,
    /// Too many items for the wire catalogue.
    TooManyItems,
    /// An item name is empty, too long or non-ASCII.
    BadName,
    /// Two items share a name.
    DuplicateName,
    /// An item costs zero gold.
    FreeItem,
    /// An item has an out-of-range stat.
    BadItemStats,
    /// No heroes were supplied.
    NoHeroes,
    /// Too many heroes for the wire catalogue.
    TooManyHeroes,
    /// A hero name is empty, too long or non-ASCII.
    BadHeroName,
    /// Two heroes share a name.
    DuplicateHeroName,
    /// A hero ability id is outside the ability library.
    BadAbility,
    /// A hero has non-positive health or an invalid attack interval.
    BadHeroStats,
    /// No abilities were supplied.
    NoAbilities,
    /// Too many abilities for the wire catalogue.
    TooManyAbilities,
    /// An ability name is empty, too long or non-ASCII.
    BadAbilityName,
    /// Two abilities share a name.
    DuplicateAbilityName,
    /// Ability cooldown mana or cast range is invalid.
    BadAbilityStats,
    /// Modifier definitions are invalid.
    BadModifiers,
    /// Map has no lanes or too many lanes/points/structures.
    BadMap,
    /// Map lane names or structure layout are invalid.
    BadMapData,
    /// Wall count/endpoints are invalid.
    BadWalls,
    /// Manifest fields are empty, too long or non-ASCII.
    BadManifest,
}

impl core::fmt::Display for ContentError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoItems => f.write_str("content must define at least one item"),
            Self::TooManyItems => write!(f, "content has more than {MAX_CONTENT_ITEMS} items"),
            Self::BadName => f.write_str("item names must be 1..=16 ASCII bytes"),
            Self::DuplicateName => f.write_str("item names must be unique"),
            Self::FreeItem => f.write_str("item costs must be greater than zero"),
            Self::BadItemStats => f.write_str("item stats are out of range"),
            Self::NoHeroes => f.write_str("content must define at least one hero"),
            Self::TooManyHeroes => write!(f, "content has more than {MAX_CONTENT_HEROES} heroes"),
            Self::BadHeroName => f.write_str("hero names must be 1..=16 ASCII bytes"),
            Self::DuplicateHeroName => f.write_str("hero names must be unique"),
            Self::BadAbility => f.write_str("hero ability ids must exist"),
            Self::BadHeroStats => f.write_str("hero stats must be positive"),
            Self::NoAbilities => f.write_str("content must define at least one ability"),
            Self::TooManyAbilities => {
                write!(f, "content has more than {MAX_CONTENT_ABILITIES} abilities")
            }
            Self::BadAbilityName => f.write_str("ability names must be 1..=16 ASCII bytes"),
            Self::DuplicateAbilityName => f.write_str("ability names must be unique"),
            Self::BadAbilityStats => {
                f.write_str("ability cooldown must be positive and mana/cast range non-negative")
            }
            Self::BadModifiers => f.write_str("modifier names/kinds are invalid or duplicated"),
            Self::BadMap => f.write_str("map limits are invalid"),
            Self::BadMapData => f.write_str("map lanes/structures are invalid"),
            Self::BadWalls => f.write_str("map walls are invalid"),
            Self::BadManifest => f.write_str("manifest fields must be 1..=32 ASCII bytes"),
        }
    }
}

impl std::error::Error for ContentError {}

fn area_effect_valid(effect: &AreaEffect, modifiers: &[ModifierDef]) -> bool {
    match effect {
        AreaEffect::None => true,
        AreaEffect::Damage { amount, .. } => (0..=MAX_STAT).contains(amount),
        AreaEffect::ApplyModifier {
            modifier,
            duration_ticks,
            factor,
            amount,
            ..
        } => {
            let Some(definition) = modifiers.get(usize::from(*modifier)) else {
                return false;
            };
            if !(1..=MAX_DURATION_TICKS).contains(duration_ticks) {
                return false;
            }
            match definition.kind {
                ModifierKind::Shield => (0..=MAX_STAT).contains(amount),
                ModifierKind::Slow => *factor > Fix::ZERO && *factor < Fix::ONE,
                ModifierKind::Haste => *factor > Fix::ONE,
                ModifierKind::Burn => (1..=MAX_STAT).contains(amount),
                ModifierKind::Root | ModifierKind::Silence => true,
            }
        }
    }
}

fn area_effects_valid(effects: &AreaEffectList, modifiers: &[ModifierDef]) -> bool {
    !effects.is_empty()
        && effects
            .iter()
            .all(|effect| area_effect_valid(effect, modifiers))
}

fn effect_valid(effect: &Effect, modifiers: &[ModifierDef]) -> bool {
    let in_range = |value: Fix| value > Fix::ZERO && value <= Fix::from_i32(MAX_RANGE);
    match effect {
        Effect::None => false,
        Effect::Damage { amount, .. } | Effect::Heal { amount } => (0..=MAX_STAT).contains(amount),
        Effect::ApplyModifier {
            modifier,
            duration_ticks,
            factor,
            amount,
            ..
        } => area_effect_valid(
            &AreaEffect::ApplyModifier {
                modifier: *modifier,
                duration_ticks: *duration_ticks,
                factor: *factor,
                amount: *amount,
                damage_type: DamageType::Physical,
            },
            modifiers,
        ),
        Effect::Displace { range } => in_range(*range),
        Effect::Projectile {
            speed_per_sec,
            range,
            radius,
            arc,
            on_hit,
        } => {
            in_range(*speed_per_sec)
                && in_range(*range)
                && in_range(*radius)
                && *arc >= Fix::ZERO
                && *arc <= Fix::from_i32(MAX_RANGE)
                && area_effects_valid(on_hit, modifiers)
        }
        Effect::Area { radius, effects } => {
            in_range(*radius) && area_effects_valid(effects, modifiers)
        }
        Effect::Zone {
            radius,
            duration_ticks,
            period_ticks,
            effects,
        } => {
            in_range(*radius)
                && (1..=MAX_DURATION_TICKS).contains(duration_ticks)
                && (1..=MAX_DURATION_TICKS).contains(period_ticks)
                && area_effects_valid(effects, modifiers)
        }
    }
}

fn hash_area_effects(feed: &mut dyn FnMut(&[u8]), effects: &AreaEffectList) {
    feed(&[u8::try_from(effects.iter().count()).unwrap_or(u8::MAX)]);
    for effect in effects.iter() {
        match effect {
            AreaEffect::None => {}
            AreaEffect::Damage {
                amount,
                damage_type,
            } => {
                feed(&[0]);
                feed(&amount.to_le_bytes());
                feed(&[*damage_type as u8]);
            }
            AreaEffect::ApplyModifier {
                modifier,
                duration_ticks,
                factor,
                amount,
                damage_type,
            } => {
                feed(&[1]);
                feed(&modifier.to_le_bytes());
                feed(&duration_ticks.to_le_bytes());
                feed(&factor.to_bits().to_le_bytes());
                feed(&amount.to_le_bytes());
                feed(&[*damage_type as u8]);
            }
        }
    }
}

fn hash_effects(feed: &mut dyn FnMut(&[u8]), effects: &EffectList) {
    feed(&[u8::try_from(effects.iter().count()).unwrap_or(u8::MAX)]);
    for effect in effects.iter() {
        match effect {
            Effect::None => {}
            Effect::Damage {
                amount,
                damage_type,
            } => {
                feed(&[0]);
                feed(&amount.to_le_bytes());
                feed(&[*damage_type as u8]);
            }
            Effect::Heal { amount } => {
                feed(&[1]);
                feed(&amount.to_le_bytes());
            }
            Effect::ApplyModifier {
                modifier,
                duration_ticks,
                factor,
                amount,
                damage_type,
            } => {
                feed(&[2]);
                feed(&modifier.to_le_bytes());
                feed(&duration_ticks.to_le_bytes());
                feed(&factor.to_bits().to_le_bytes());
                feed(&amount.to_le_bytes());
                feed(&[*damage_type as u8]);
            }
            Effect::Displace { range } => {
                feed(&[3]);
                feed(&range.to_bits().to_le_bytes());
            }
            Effect::Projectile {
                speed_per_sec,
                range,
                radius,
                arc,
                on_hit,
            } => {
                feed(&[4]);
                feed(&speed_per_sec.to_bits().to_le_bytes());
                feed(&range.to_bits().to_le_bytes());
                feed(&radius.to_bits().to_le_bytes());
                feed(&arc.to_bits().to_le_bytes());
                hash_area_effects(feed, on_hit);
            }
            Effect::Area { radius, effects } => {
                feed(&[5]);
                feed(&radius.to_bits().to_le_bytes());
                hash_area_effects(feed, effects);
            }
            Effect::Zone {
                radius,
                duration_ticks,
                period_ticks,
                effects,
            } => {
                feed(&[6]);
                feed(&radius.to_bits().to_le_bytes());
                feed(&duration_ticks.to_le_bytes());
                feed(&period_ticks.to_le_bytes());
                hash_area_effects(feed, effects);
            }
        }
    }
}

impl GameContent {
    /// Validates names, counts and costs.
    ///
    /// # Errors
    /// Returns the first [`ContentError`] found.
    #[allow(clippy::too_many_lines)] // Flat validation table is intentionally exhaustive.
    pub fn validate(&self) -> Result<(), ContentError> {
        for field in [
            &self.manifest.name,
            &self.manifest.version,
            &self.manifest.author,
        ] {
            if field.is_empty() || field.len() > MAX_MANIFEST_FIELD || !field.is_ascii() {
                return Err(ContentError::BadManifest);
            }
        }
        if self.items.is_empty() {
            return Err(ContentError::NoItems);
        }
        if self.items.len() > MAX_CONTENT_ITEMS {
            return Err(ContentError::TooManyItems);
        }
        for (index, item) in self.items.iter().enumerate() {
            if item.name.is_empty() || item.name.len() > MAX_ITEM_NAME || !item.name.is_ascii() {
                return Err(ContentError::BadName);
            }
            if item.cost == 0 || item.cost > MAX_STAT as u32 {
                return Err(ContentError::FreeItem);
            }
            for value in [
                item.health,
                item.mana,
                item.attack_damage,
                item.armor,
                item.magic_resist,
            ] {
                if value.unsigned_abs() > MAX_ITEM_STAT as u32 {
                    return Err(ContentError::BadItemStats);
                }
            }
            if item.move_speed_per_sec < Fix::ZERO
                || item.move_speed_per_sec > Fix::from_i32(MAX_SPEED_PER_SEC)
            {
                return Err(ContentError::BadItemStats);
            }
            if self.items[..index]
                .iter()
                .any(|other| other.name == item.name)
            {
                return Err(ContentError::DuplicateName);
            }
        }
        if self.abilities.is_empty() {
            return Err(ContentError::NoAbilities);
        }
        if self.abilities.len() > MAX_CONTENT_ABILITIES {
            return Err(ContentError::TooManyAbilities);
        }
        for (index, ability) in self.abilities.iter().enumerate() {
            if ability.name.is_empty()
                || ability.name.len() > MAX_ABILITY_NAME
                || !ability.name.is_ascii()
            {
                return Err(ContentError::BadAbilityName);
            }
            if self.abilities[..index]
                .iter()
                .any(|other| other.name == ability.name)
            {
                return Err(ContentError::DuplicateAbilityName);
            }
            if ability.cooldown_ticks == 0
                || ability.cooldown_ticks > MAX_COOLDOWN_TICKS
                || ability.mana_cost < 0
                || ability.mana_cost > MAX_STAT
                || ability.targeting.range() < Fix::ZERO
                || ability.targeting.range() > Fix::from_i32(MAX_RANGE)
                || ability.effects.is_empty()
                || !ability
                    .effects
                    .iter()
                    .all(|effect| effect_valid(effect, &self.modifiers))
            {
                return Err(ContentError::BadAbilityStats);
            }
        }
        if self.modifiers.is_empty() || self.modifiers.len() > MAX_CONTENT_MODIFIERS {
            return Err(ContentError::BadModifiers);
        }
        for (index, modifier) in self.modifiers.iter().enumerate() {
            if modifier.name.is_empty()
                || modifier.name.len() > MAX_ITEM_NAME
                || !modifier.name.is_ascii()
                || self.modifiers[..index]
                    .iter()
                    .any(|other| other.name == modifier.name || other.kind == modifier.kind)
            {
                return Err(ContentError::BadModifiers);
            }
        }
        for kind in [
            ModifierKind::Shield,
            ModifierKind::Slow,
            ModifierKind::Haste,
            ModifierKind::Root,
            ModifierKind::Silence,
            ModifierKind::Burn,
        ] {
            if !self.modifiers.iter().any(|modifier| modifier.kind == kind) {
                return Err(ContentError::BadModifiers);
            }
        }
        if self.map.name.is_empty()
            || self.map.name.len() > MAX_MANIFEST_FIELD
            || !self.map.name.is_ascii()
        {
            return Err(ContentError::BadMap);
        }
        if self.map.lanes.is_empty()
            || self.map.lanes.len() > MAX_CONTENT_LANES
            || self.map.structures.len() > MAX_CONTENT_STRUCTURES
        {
            return Err(ContentError::BadMap);
        }
        for (index, lane) in self.map.lanes.iter().enumerate() {
            if lane.name.is_empty()
                || lane.name.len() > MAX_HERO_NAME
                || !lane.name.is_ascii()
                || lane.path.len() < 2
                || lane.path.len() > MAX_LANE_POINTS
            {
                return Err(ContentError::BadMapData);
            }
            if self.map.lanes[..index]
                .iter()
                .any(|other| other.name == lane.name)
            {
                return Err(ContentError::BadMapData);
            }
        }
        for team in Team::ALL {
            if !self
                .map
                .structures
                .iter()
                .any(|structure| structure.team == team && structure.kind == StructureKind::Nexus)
            {
                return Err(ContentError::BadMapData);
            }
        }
        if !(1..=5).contains(&self.rules.team_size)
            || !(1..=99).contains(&self.rules.level_cap)
            || self.rules.melee_minion.max_health <= 0
            || self.rules.ranged_minion.max_health <= 0
            || self.rules.tower.max_health <= 0
            || self.rules.nexus.max_health <= 0
            || self.rules.melee_minion.attack_interval_ticks == 0
            || self.rules.ranged_minion.attack_interval_ticks == 0
            || self.rules.tower.attack_interval_ticks == 0
            || self.rules.nexus.attack_interval_ticks == 0
            || self.rules.xp_base == 0
            || self.rules.xp_per_level == 0
            || self.rules.hero_respawn_ticks == 0
            || self.rules.wave_interval_ticks == 0
            || self.rules.first_wave_ticks == 0
            || self.rules.respawn_per_level > 1_000
        {
            return Err(ContentError::BadMap);
        }
        if self.map.extent <= Fix::ZERO || self.map.extent > Fix::from_i32(MAX_MAP_COORD) {
            return Err(ContentError::BadMap);
        }
        let extent = self.map.extent;
        let in_world = |point: FixVec2| {
            point.x >= Fix::ZERO && point.x <= extent && point.y >= Fix::ZERO && point.y <= extent
        };
        if !in_world(self.map.blue_spawn)
            || !in_world(self.map.red_spawn)
            || self
                .map
                .lanes
                .iter()
                .any(|lane| lane.path.iter().any(|point| !in_world(*point)))
            || self
                .map
                .structures
                .iter()
                .any(|structure| !in_world(structure.position))
        {
            return Err(ContentError::BadMapData);
        }
        if self.map.walls.len() > MAX_CONTENT_WALLS
            || self
                .map
                .walls
                .iter()
                .any(|wall| wall.a == wall.b || !in_world(wall.a) || !in_world(wall.b))
        {
            return Err(ContentError::BadWalls);
        }
        if self.map.structures.iter().any(|structure| {
            structure
                .lane
                .is_some_and(|lane| usize::from(lane) >= self.map.lanes.len())
        }) {
            return Err(ContentError::BadMapData);
        }
        if self.heroes.is_empty() {
            return Err(ContentError::NoHeroes);
        }
        if self.heroes.len() > MAX_CONTENT_HEROES {
            return Err(ContentError::TooManyHeroes);
        }
        for (index, hero) in self.heroes.iter().enumerate() {
            if hero.name.is_empty() || hero.name.len() > MAX_HERO_NAME || !hero.name.is_ascii() {
                return Err(ContentError::BadHeroName);
            }
            if self.heroes[..index]
                .iter()
                .any(|other| other.name == hero.name)
            {
                return Err(ContentError::DuplicateHeroName);
            }
            if hero
                .abilities
                .iter()
                .any(|&id| usize::from(id) >= self.abilities.len())
            {
                return Err(ContentError::BadAbility);
            }
            if hero.health_per_level < 0
                || hero.mana_per_level < 0
                || hero.attack_damage_per_level < 0
                || hero.armor_per_level < 0
                || hero.magic_resist_per_level < 0
            {
                return Err(ContentError::BadHeroStats);
            }
            if hero.max_health <= 0
                || hero.max_health > MAX_STAT
                || hero.max_mana < 0
                || hero.max_mana > MAX_STAT
                || hero.health_regen_per_tick.unsigned_abs() > MAX_ITEM_STAT as u32
                || hero.mana_regen_per_tick.unsigned_abs() > MAX_ITEM_STAT as u32
                || hero.attack_damage.unsigned_abs() > MAX_ITEM_STAT as u32
                || hero.armor.unsigned_abs() > MAX_RESIST as u32
                || hero.magic_resist.unsigned_abs() > MAX_RESIST as u32
                || hero.attack_interval_ticks == 0
                || hero.attack_interval_ticks > MAX_COOLDOWN_TICKS
                || hero.attack_range < Fix::ZERO
                || hero.attack_range > Fix::from_i32(MAX_RANGE)
                || hero.move_speed_per_sec <= Fix::ZERO
                || hero.move_speed_per_sec > Fix::from_i32(MAX_SPEED_PER_SEC)
                || hero.vision_range <= Fix::ZERO
                || hero.vision_range > Fix::from_i32(MAX_RANGE)
            {
                return Err(ContentError::BadHeroStats);
            }
            for growth in [
                hero.health_per_level,
                hero.mana_per_level,
                hero.attack_damage_per_level,
                hero.armor_per_level,
                hero.magic_resist_per_level,
            ] {
                if growth.unsigned_abs() > MAX_ITEM_STAT as u32 {
                    return Err(ContentError::BadHeroStats);
                }
            }
        }
        Ok(())
    }

    /// Stable bytes signed by a publisher attestation.
    #[must_use]
    pub fn signing_message(&self) -> Vec<u8> {
        let mut message = Vec::with_capacity(96);
        message.extend_from_slice(b"moba-pack-v1\n");
        for field in [
            &self.manifest.name,
            &self.manifest.version,
            &self.manifest.author,
        ] {
            message.extend_from_slice(field.as_bytes());
            message.push(b'\n');
        }
        message.extend_from_slice(&self.hash().to_le_bytes());
        message
    }

    /// Stable FNV-1a hash of the catalogue, used for diagnostics and mismatch checks.
    #[must_use]
    #[allow(clippy::too_many_lines)] // Canonical field feed mirrors the content schema.
    pub fn hash(&self) -> u64 {
        let mut hash = 0xCBF2_9CE4_8422_2325u64;
        let mut feed = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
            }
        };
        for field in [
            &self.manifest.name,
            &self.manifest.version,
            &self.manifest.author,
        ] {
            feed(field.as_bytes());
            feed(&[0]);
        }
        for item in &self.items {
            feed(item.name.as_bytes());
            feed(&[0]);
            for value in [
                item.cost,
                item.health as u32,
                item.mana as u32,
                item.attack_damage as u32,
                item.armor as u32,
                item.magic_resist as u32,
            ] {
                feed(&value.to_le_bytes());
            }
            feed(&item.move_speed_per_sec.to_bits().to_le_bytes());
        }
        for ability in &self.abilities {
            feed(ability.name.as_bytes());
            feed(&[0]);
            feed(&ability.cooldown_ticks.to_le_bytes());
            feed(&(ability.mana_cost as u32).to_le_bytes());
            match ability.targeting {
                Targeting::SelfCast => feed(&[0]),
                Targeting::Point { range } => {
                    feed(&[1]);
                    feed(&range.to_bits().to_le_bytes());
                }
                Targeting::Direction { range } => {
                    feed(&[2]);
                    feed(&range.to_bits().to_le_bytes());
                }
            }
            hash_effects(&mut feed, &ability.effects);
        }
        for hero in &self.heroes {
            feed(hero.name.as_bytes());
            feed(&[0]);
            for value in [
                hero.max_health as u32,
                hero.max_mana as u32,
                hero.health_regen_per_tick as u32,
                hero.mana_regen_per_tick as u32,
                hero.attack_damage as u32,
                hero.attack_interval_ticks,
                hero.armor as u32,
                hero.magic_resist as u32,
            ] {
                feed(&value.to_le_bytes());
            }
            for value in [
                hero.attack_range,
                hero.move_speed_per_sec,
                hero.vision_range,
            ] {
                feed(&value.to_bits().to_le_bytes());
            }
            feed(&hero.abilities);
            for value in [
                hero.health_per_level as u32,
                hero.mana_per_level as u32,
                hero.attack_damage_per_level as u32,
                hero.armor_per_level as u32,
                hero.magic_resist_per_level as u32,
            ] {
                feed(&value.to_le_bytes());
            }
        }
        feed(&[
            self.rules.team_size,
            u8::from(self.rules.bot_fill),
            self.rules.level_cap,
        ]);
        for value in [self.rules.xp_base, self.rules.xp_per_level] {
            feed(&value.to_le_bytes());
        }
        for value in [
            self.rules.melee_vision,
            self.rules.ranged_vision,
            self.rules.tower_vision,
            self.rules.nexus_vision,
        ] {
            feed(&value.to_bits().to_le_bytes());
        }
        for minion in [self.rules.melee_minion, self.rules.ranged_minion] {
            feed(&minion.max_health.to_le_bytes());
            feed(&minion.attack_damage.to_le_bytes());
            feed(&minion.attack_range.to_bits().to_le_bytes());
            feed(&minion.attack_interval_ticks.to_le_bytes());
            feed(&minion.move_speed_per_sec.to_bits().to_le_bytes());
            feed(&minion.armor.to_le_bytes());
            feed(&minion.magic_resist.to_le_bytes());
        }
        for structure in [self.rules.tower, self.rules.nexus] {
            feed(&structure.max_health.to_le_bytes());
            feed(&structure.attack_damage.to_le_bytes());
            feed(&structure.attack_range.to_bits().to_le_bytes());
            feed(&structure.attack_interval_ticks.to_le_bytes());
            feed(&structure.armor.to_le_bytes());
            feed(&structure.magic_resist.to_le_bytes());
        }
        for value in [
            self.rules.hero_respawn_ticks,
            self.rules.respawn_per_level,
            self.rules.wave_interval_ticks,
            self.rules.first_wave_ticks,
        ] {
            feed(&value.to_le_bytes());
        }
        feed(self.map.name.as_bytes());
        feed(&[0]);
        feed(&self.map.extent.to_bits().to_le_bytes());
        feed(&self.map.blue_spawn.x.to_bits().to_le_bytes());
        feed(&self.map.blue_spawn.y.to_bits().to_le_bytes());
        feed(&self.map.red_spawn.x.to_bits().to_le_bytes());
        feed(&self.map.red_spawn.y.to_bits().to_le_bytes());
        for modifier in &self.modifiers {
            feed(modifier.name.as_bytes());
            feed(&[0]);
            feed(&[modifier.kind as u8]);
        }
        for lane in &self.map.lanes {
            feed(lane.name.as_bytes());
            feed(&[0]);
            for point in &lane.path {
                feed(&point.x.to_bits().to_le_bytes());
                feed(&point.y.to_bits().to_le_bytes());
            }
        }
        for structure in &self.map.structures {
            feed(&[structure.team.index() as u8, structure.kind as u8]);
            feed(&structure.position.x.to_bits().to_le_bytes());
            feed(&structure.position.y.to_bits().to_le_bytes());
            feed(&[structure.lane.unwrap_or(0xFF)]);
        }
        for wall in &self.map.walls {
            for point in [wall.a, wall.b] {
                feed(&point.x.to_bits().to_le_bytes());
                feed(&point.y.to_bits().to_le_bytes());
            }
        }
        hash
    }
}

/// Errors loading a content pack from TOML.
#[derive(Debug)]
pub enum ContentLoadError {
    /// File could not be read.
    Io(std::io::Error),
    /// TOML could not be parsed.
    Toml(toml::de::Error),
    /// Content failed semantic validation.
    Invalid(ContentError),
    /// A field has an unsupported value.
    InvalidField {
        /// Dotted field name.
        field: &'static str,
        /// Rejected value.
        value: String,
    },
}

impl core::fmt::Display for ContentLoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "cannot read content: {error}"),
            Self::Toml(error) => write!(f, "invalid TOML: {error}"),
            Self::Invalid(error) => write!(f, "invalid content: {error}"),
            Self::InvalidField { field, value } => {
                write!(f, "invalid value {value:?} for {field}")
            }
        }
    }
}

impl std::error::Error for ContentLoadError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModifierEntry {
    name: String,
    kind: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContentFile {
    name: Option<String>,
    publisher: Option<String>,
    signature: Option<String>,
    version: Option<String>,
    author: Option<String>,
    #[serde(default)]
    items: Vec<ItemEntry>,
    heroes: Option<Vec<HeroEntry>>,
    abilities: Option<Vec<AbilityEntry>>,
    modifiers: Option<Vec<ModifierEntry>>,
    rules: Option<RulesEntry>,
    map: Option<MapEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, default)]
struct MinionStatsEntry {
    max_health: i32,
    attack_damage: i32,
    attack_range: f32,
    attack_interval_ticks: u32,
    move_speed_per_sec: f32,
    armor: i32,
    magic_resist: i32,
    vision_range: f32,
}

impl Default for MinionStatsEntry {
    fn default() -> Self {
        let stats = RulesSpec::default().melee_minion;
        Self {
            max_health: stats.max_health,
            attack_damage: stats.attack_damage,
            attack_range: stats.attack_range.to_f32(),
            attack_interval_ticks: stats.attack_interval_ticks,
            move_speed_per_sec: stats.move_speed_per_sec.to_f32(),
            armor: stats.armor,
            magic_resist: stats.magic_resist,
            vision_range: stats.vision_range.to_f32(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, default)]
struct StructureStatsEntry {
    max_health: i32,
    attack_damage: i32,
    attack_range: f32,
    attack_interval_ticks: u32,
    armor: i32,
    magic_resist: i32,
    vision_range: f32,
}

impl Default for StructureStatsEntry {
    fn default() -> Self {
        let stats = RulesSpec::default().tower;
        Self {
            max_health: stats.max_health,
            attack_damage: stats.attack_damage,
            attack_range: stats.attack_range.to_f32(),
            attack_interval_ticks: stats.attack_interval_ticks,
            armor: stats.armor,
            magic_resist: stats.magic_resist,
            vision_range: stats.vision_range.to_f32(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RulesEntry {
    melee_minion: MinionStatsEntry,
    ranged_minion: MinionStatsEntry,
    tower: StructureStatsEntry,
    nexus: StructureStatsEntry,
    level_cap: u8,
    xp_base: u32,
    xp_per_level: u32,
    melee_vision: f32,
    ranged_vision: f32,
    tower_vision: f32,
    nexus_vision: f32,
    team_size: u8,
    bot_fill: bool,
    hero_respawn_ticks: u32,
    respawn_per_level: u32,
    wave_interval_ticks: u32,
    first_wave_ticks: u32,
}

impl Default for RulesEntry {
    fn default() -> Self {
        let rules = RulesSpec::default();
        Self {
            melee_minion: MinionStatsEntry::default(),
            ranged_minion: {
                let stats = RulesSpec::default().ranged_minion;
                MinionStatsEntry {
                    max_health: stats.max_health,
                    attack_damage: stats.attack_damage,
                    attack_range: stats.attack_range.to_f32(),
                    attack_interval_ticks: stats.attack_interval_ticks,
                    move_speed_per_sec: stats.move_speed_per_sec.to_f32(),
                    armor: stats.armor,
                    magic_resist: stats.magic_resist,
                    vision_range: stats.vision_range.to_f32(),
                }
            },
            tower: StructureStatsEntry::default(),
            nexus: {
                let stats = RulesSpec::default().nexus;
                StructureStatsEntry {
                    max_health: stats.max_health,
                    attack_damage: stats.attack_damage,
                    attack_range: stats.attack_range.to_f32(),
                    attack_interval_ticks: stats.attack_interval_ticks,
                    armor: stats.armor,
                    magic_resist: stats.magic_resist,
                    vision_range: stats.vision_range.to_f32(),
                }
            },
            level_cap: rules.level_cap,
            xp_base: rules.xp_base,
            xp_per_level: rules.xp_per_level,
            melee_vision: rules.melee_vision.to_f32(),
            ranged_vision: rules.ranged_vision.to_f32(),
            tower_vision: rules.tower_vision.to_f32(),
            nexus_vision: rules.nexus_vision.to_f32(),
            team_size: rules.team_size,
            bot_fill: rules.bot_fill,
            hero_respawn_ticks: rules.hero_respawn_ticks,
            respawn_per_level: rules.respawn_per_level,
            wave_interval_ticks: rules.wave_interval_ticks,
            first_wave_ticks: rules.first_wave_ticks,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MapEntry {
    #[serde(default = "default_map_name")]
    name: String,
    #[serde(default = "default_map_extent")]
    extent: i32,
    blue_spawn: [i32; 2],
    red_spawn: [i32; 2],
    lanes: Vec<LaneEntry>,
    #[serde(default)]
    structures: Vec<StructureEntry>,
    #[serde(default)]
    walls: Vec<WallEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WallEntry {
    a: [i32; 2],
    b: [i32; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaneEntry {
    name: String,
    path: Vec<[i32; 2]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StructureEntry {
    team: String,
    kind: String,
    position: [i32; 2],
    #[serde(default)]
    lane: Option<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemEntry {
    name: String,
    cost: u32,
    #[serde(default)]
    health: i32,
    #[serde(default)]
    mana: i32,
    #[serde(default)]
    attack_damage: i32,
    #[serde(default)]
    armor: i32,
    #[serde(default)]
    magic_resist: i32,
    #[serde(default)]
    move_speed: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AbilityEntry {
    name: String,
    cooldown_ticks: u32,
    #[serde(default)]
    mana_cost: i32,
    #[serde(default)]
    cast_range: f32,
    kind: String,
    #[serde(default)]
    speed: f32,
    #[serde(default)]
    range: f32,
    #[serde(default)]
    radius: f32,
    /// Visual projectile arc height at mid-flight.
    #[serde(default)]
    arc: f32,
    #[serde(default)]
    damage: i32,
    #[serde(default)]
    amount: i32,
    #[serde(default)]
    duration_ticks: u32,
    #[serde(default)]
    factor: f32,
    #[serde(default = "default_damage_type")]
    damage_type: String,
    /// Optional status kind applied on a skillshot hit (e.g. `root`).
    #[serde(default)]
    on_hit: Option<String>,
    /// Duration for the on-hit status (falls back to `duration_ticks`).
    #[serde(default)]
    on_hit_duration_ticks: u32,
    /// Period between zone applications (defaults to 30).
    #[serde(default)]
    period_ticks: u32,
}

fn default_map_name() -> String {
    String::from("Map")
}

const fn default_map_extent() -> i32 {
    DEFAULT_MAP_EXTENT
}

fn default_damage_type() -> String {
    String::from("physical")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HeroEntry {
    name: String,
    max_health: i32,
    max_mana: i32,
    #[serde(default)]
    health_regen_per_tick: i32,
    #[serde(default)]
    mana_regen_per_tick: i32,
    attack_damage: i32,
    attack_range: f32,
    attack_interval_ticks: u32,
    move_speed: f32,
    #[serde(default)]
    armor: i32,
    #[serde(default)]
    magic_resist: i32,
    vision_range: f32,
    abilities: [u8; 3],
    health_per_level: Option<i32>,
    mana_per_level: Option<i32>,
    attack_damage_per_level: Option<i32>,
    armor_per_level: Option<i32>,
    magic_resist_per_level: Option<i32>,
}

fn invalid_field(field: &'static str, value: impl Into<String>) -> ContentLoadError {
    ContentLoadError::InvalidField {
        field,
        value: value.into(),
    }
}

fn fix_param(value: f32, field: &'static str) -> Result<Fix, ContentLoadError> {
    if value.is_finite() {
        Ok(Fix::from_f32(value))
    } else {
        Err(invalid_field(field, value.to_string()))
    }
}

fn parse_modifier_kind(value: &str) -> Result<ModifierKind, ContentLoadError> {
    match value {
        "shield" => Ok(ModifierKind::Shield),
        "slow" => Ok(ModifierKind::Slow),
        "haste" => Ok(ModifierKind::Haste),
        "root" => Ok(ModifierKind::Root),
        "silence" => Ok(ModifierKind::Silence),
        "burn" => Ok(ModifierKind::Burn),
        other => Err(invalid_field("modifiers.kind", other)),
    }
}

fn parse_hex_array<const N: usize>(value: &str) -> Result<[u8; N], ()> {
    if value.len() != N * 2 {
        return Err(());
    }
    let mut output = [0u8; N];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).map_err(|_| ())?;
    }
    Ok(output)
}

fn parse_damage_type(value: &str) -> Result<DamageType, ContentLoadError> {
    match value {
        "physical" => Ok(DamageType::Physical),
        "magic" => Ok(DamageType::Magic),
        other => Err(invalid_field("abilities.damage_type", other)),
    }
}

fn parse_team(value: &str) -> Result<Team, ContentLoadError> {
    match value {
        "blue" => Ok(Team::Blue),
        "red" => Ok(Team::Red),
        other => Err(invalid_field("map.structures.team", other)),
    }
}

fn parse_structure_kind(value: &str) -> Result<StructureKind, ContentLoadError> {
    match value {
        "nexus" => Ok(StructureKind::Nexus),
        "tower" => Ok(StructureKind::Tower),
        other => Err(invalid_field("map.structures.kind", other)),
    }
}

/// Parses and validates a content pack from TOML.
///
/// # Errors
/// Returns [`ContentLoadError`] for syntax or validation failures.
#[allow(clippy::too_many_lines)] // One declarative conversion for every content table.
pub fn load_toml(text: &str) -> Result<GameContent, ContentLoadError> {
    let file: ContentFile = toml::from_str(text).map_err(ContentLoadError::Toml)?;
    let items = file
        .items
        .into_iter()
        .map(|item| {
            Ok::<ItemSpec, ContentLoadError>(ItemSpec {
                name: item.name,
                cost: item.cost,
                health: item.health,
                mana: item.mana,
                attack_damage: item.attack_damage,
                armor: item.armor,
                magic_resist: item.magic_resist,
                move_speed_per_sec: fix_param(item.move_speed, "items.move_speed")?,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let items = if items.is_empty() {
        GameContent::default().items
    } else {
        items
    };
    let heroes = match file.heroes {
        Some(heroes) => heroes
            .into_iter()
            .map(|hero| {
                Ok::<HeroSpec, ContentLoadError>(HeroSpec {
                    name: hero.name,
                    max_health: hero.max_health,
                    max_mana: hero.max_mana,
                    health_regen_per_tick: hero.health_regen_per_tick,
                    mana_regen_per_tick: hero.mana_regen_per_tick,
                    attack_damage: hero.attack_damage,
                    attack_range: fix_param(hero.attack_range, "heroes.attack_range")?,
                    attack_interval_ticks: hero.attack_interval_ticks,
                    move_speed_per_sec: fix_param(hero.move_speed, "heroes.move_speed")?,
                    armor: hero.armor,
                    magic_resist: hero.magic_resist,
                    vision_range: fix_param(hero.vision_range, "heroes.vision_range")?,
                    abilities: hero.abilities,
                    health_per_level: hero.health_per_level.unwrap_or(crate::LEVEL_GROWTH.health),
                    mana_per_level: hero.mana_per_level.unwrap_or(crate::LEVEL_GROWTH.mana),
                    attack_damage_per_level: hero
                        .attack_damage_per_level
                        .unwrap_or(crate::LEVEL_GROWTH.attack_damage),
                    armor_per_level: hero.armor_per_level.unwrap_or(crate::LEVEL_GROWTH.armor),
                    magic_resist_per_level: hero
                        .magic_resist_per_level
                        .unwrap_or(crate::LEVEL_GROWTH.magic_resist),
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => GameContent::default().heroes,
    };
    let modifiers = match file.modifiers {
        Some(modifiers) => modifiers
            .into_iter()
            .map(|modifier| {
                Ok(ModifierDef {
                    name: modifier.name,
                    kind: parse_modifier_kind(&modifier.kind)?,
                })
            })
            .collect::<Result<Vec<_>, ContentLoadError>>()?,
        None => GameContent::default().modifiers,
    };
    let abilities = match file.abilities {
        Some(abilities) => abilities
            .into_iter()
            .map(|ability| {
                let damage_type = parse_damage_type(&ability.damage_type)?;
                let cast_range = fix_param(ability.cast_range, "abilities.cast_range")?;
                let radius = fix_param(ability.radius, "abilities.radius")?;
                let factor = fix_param(ability.factor, "abilities.factor")?;
                let modifier_id = |kind: ModifierKind| {
                    modifiers
                        .iter()
                        .position(|definition| definition.kind == kind)
                        .and_then(|index| u16::try_from(index).ok())
                        .unwrap_or(0)
                };
                let apply = |kind: ModifierKind, duration_ticks: u32, amount: i32| {
                    AreaEffect::ApplyModifier {
                        modifier: modifier_id(kind),
                        duration_ticks,
                        factor,
                        amount,
                        damage_type,
                    }
                };
                let (targeting, effects) = match ability.kind.as_str() {
                    "skillshot" => {
                        let mut on_hit = AreaEffectList::one(AreaEffect::Damage {
                            amount: ability.damage,
                            damage_type,
                        });
                        if let Some(status) = ability.on_hit.as_deref() {
                            let kind = parse_modifier_kind(status)?;
                            let duration = if ability.on_hit_duration_ticks == 0 {
                                ability.duration_ticks
                            } else {
                                ability.on_hit_duration_ticks
                            };
                            on_hit.effects[1] = AreaEffect::ApplyModifier {
                                modifier: modifier_id(kind),
                                duration_ticks: duration,
                                factor: Fix::ONE,
                                amount: 0,
                                damage_type,
                            };
                        }
                        (
                            Targeting::Direction { range: cast_range },
                            EffectList::one(Effect::Projectile {
                                speed_per_sec: fix_param(ability.speed, "abilities.speed")?,
                                range: fix_param(ability.range, "abilities.range")?,
                                radius,
                                arc: fix_param(ability.arc, "abilities.arc")?,
                                on_hit,
                            }),
                        )
                    }
                    "dash" => (
                        Targeting::Direction { range: cast_range },
                        EffectList::one(Effect::Displace {
                            range: fix_param(ability.range, "abilities.range")?,
                        }),
                    ),
                    "heal" => (
                        Targeting::SelfCast,
                        EffectList::one(Effect::Heal {
                            amount: ability.amount,
                        }),
                    ),
                    "haste" => (
                        Targeting::SelfCast,
                        EffectList::one(Effect::ApplyModifier {
                            modifier: modifier_id(ModifierKind::Haste),
                            duration_ticks: ability.duration_ticks,
                            factor,
                            amount: 0,
                            damage_type,
                        }),
                    ),
                    "burn" => (
                        Targeting::Point { range: cast_range },
                        EffectList::one(Effect::Area {
                            radius,
                            effects: AreaEffectList::one(apply(
                                ModifierKind::Burn,
                                ability.duration_ticks,
                                ability.damage,
                            )),
                        }),
                    ),
                    "silence" => (
                        Targeting::Point { range: cast_range },
                        EffectList::one(Effect::Area {
                            radius,
                            effects: AreaEffectList::one(apply(
                                ModifierKind::Silence,
                                ability.duration_ticks,
                                0,
                            )),
                        }),
                    ),
                    "root" => (
                        Targeting::Point { range: cast_range },
                        EffectList::one(Effect::Area {
                            radius,
                            effects: AreaEffectList::one(apply(
                                ModifierKind::Root,
                                ability.duration_ticks,
                                0,
                            )),
                        }),
                    ),
                    "slow" => (
                        Targeting::Point { range: cast_range },
                        EffectList::one(Effect::Area {
                            radius,
                            effects: AreaEffectList::one(apply(
                                ModifierKind::Slow,
                                ability.duration_ticks,
                                0,
                            )),
                        }),
                    ),
                    "shield" => (
                        Targeting::SelfCast,
                        EffectList::one(Effect::ApplyModifier {
                            modifier: modifier_id(ModifierKind::Shield),
                            duration_ticks: ability.duration_ticks,
                            factor: Fix::ONE,
                            amount: ability.amount,
                            damage_type,
                        }),
                    ),
                    "burn_zone" => (
                        Targeting::Point { range: cast_range },
                        EffectList::one(Effect::Zone {
                            radius,
                            duration_ticks: ability.duration_ticks,
                            period_ticks: if ability.period_ticks == 0 {
                                30
                            } else {
                                ability.period_ticks
                            },
                            effects: AreaEffectList::one(apply(
                                ModifierKind::Burn,
                                ability.duration_ticks,
                                ability.damage,
                            )),
                        }),
                    ),
                    "burst" => (
                        Targeting::Point { range: cast_range },
                        EffectList::one(Effect::Area {
                            radius,
                            effects: AreaEffectList::one(AreaEffect::Damage {
                                amount: ability.damage,
                                damage_type,
                            }),
                        }),
                    ),
                    other => return Err(invalid_field("abilities.kind", other)),
                };
                Ok::<AbilitySpec, ContentLoadError>(AbilitySpec {
                    name: ability.name,
                    cooldown_ticks: ability.cooldown_ticks,
                    mana_cost: ability.mana_cost,
                    targeting,
                    effects,
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => GameContent::default().abilities,
    };
    let map = match file.map {
        Some(map) => {
            let MapEntry {
                name,
                blue_spawn,
                red_spawn,
                lanes,
                structures,
                walls,
                extent,
            } = map;
            let structures = structures
                .into_iter()
                .map(|structure| {
                    Ok::<StructureSpec, ContentLoadError>(StructureSpec {
                        team: parse_team(&structure.team)?,
                        position: FixVec2::from_ints(structure.position[0], structure.position[1]),
                        kind: parse_structure_kind(&structure.kind)?,
                        lane: structure.lane,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            MapSpec {
                name,
                extent: Fix::from_i32(extent),
                blue_spawn: FixVec2::from_ints(blue_spawn[0], blue_spawn[1]),
                red_spawn: FixVec2::from_ints(red_spawn[0], red_spawn[1]),
                lanes: lanes
                    .into_iter()
                    .map(|lane| LaneSpec {
                        name: lane.name,
                        path: lane
                            .path
                            .into_iter()
                            .map(|point| FixVec2::from_ints(point[0], point[1]))
                            .collect(),
                    })
                    .collect(),
                structures,
                walls: walls
                    .into_iter()
                    .map(|wall| WallSpec {
                        a: FixVec2::from_ints(wall.a[0], wall.a[1]),
                        b: FixVec2::from_ints(wall.b[0], wall.b[1]),
                    })
                    .collect(),
            }
        }
        None => GameContent::default().map,
    };
    let rules = file
        .rules
        .map_or_else(RulesSpec::default, |rules| RulesSpec {
            melee_minion: MinionStatsSpec {
                max_health: rules.melee_minion.max_health,
                attack_damage: rules.melee_minion.attack_damage,
                attack_range: Fix::from_f32(rules.melee_minion.attack_range),
                attack_interval_ticks: rules.melee_minion.attack_interval_ticks,
                move_speed_per_sec: Fix::from_f32(rules.melee_minion.move_speed_per_sec),
                armor: rules.melee_minion.armor,
                magic_resist: rules.melee_minion.magic_resist,
                vision_range: Fix::from_f32(rules.melee_minion.vision_range),
            },
            ranged_minion: MinionStatsSpec {
                max_health: rules.ranged_minion.max_health,
                attack_damage: rules.ranged_minion.attack_damage,
                attack_range: Fix::from_f32(rules.ranged_minion.attack_range),
                attack_interval_ticks: rules.ranged_minion.attack_interval_ticks,
                move_speed_per_sec: Fix::from_f32(rules.ranged_minion.move_speed_per_sec),
                armor: rules.ranged_minion.armor,
                magic_resist: rules.ranged_minion.magic_resist,
                vision_range: Fix::from_f32(rules.ranged_minion.vision_range),
            },
            tower: StructureStatsSpec {
                max_health: rules.tower.max_health,
                attack_damage: rules.tower.attack_damage,
                attack_range: Fix::from_f32(rules.tower.attack_range),
                attack_interval_ticks: rules.tower.attack_interval_ticks,
                armor: rules.tower.armor,
                magic_resist: rules.tower.magic_resist,
                vision_range: Fix::from_f32(rules.tower.vision_range),
            },
            nexus: StructureStatsSpec {
                max_health: rules.nexus.max_health,
                attack_damage: rules.nexus.attack_damage,
                attack_range: Fix::from_f32(rules.nexus.attack_range),
                attack_interval_ticks: rules.nexus.attack_interval_ticks,
                armor: rules.nexus.armor,
                magic_resist: rules.nexus.magic_resist,
                vision_range: Fix::from_f32(rules.nexus.vision_range),
            },
            level_cap: rules.level_cap,
            xp_base: rules.xp_base,
            xp_per_level: rules.xp_per_level,
            melee_vision: Fix::from_f32(rules.melee_vision),
            ranged_vision: Fix::from_f32(rules.ranged_vision),
            tower_vision: Fix::from_f32(rules.tower_vision),
            nexus_vision: Fix::from_f32(rules.nexus_vision),
            team_size: rules.team_size,
            bot_fill: rules.bot_fill,
            hero_respawn_ticks: rules.hero_respawn_ticks,
            respawn_per_level: rules.respawn_per_level,
            wave_interval_ticks: rules.wave_interval_ticks,
            first_wave_ticks: rules.first_wave_ticks,
        });
    let attestation = match (file.publisher.as_deref(), file.signature.as_deref()) {
        (None, None) => None,
        (Some(publisher), Some(signature)) => Some(PackAttestation {
            publisher: parse_hex_array(publisher)
                .map_err(|()| invalid_field("publisher", publisher))?,
            signature: parse_hex_array(signature)
                .map_err(|()| invalid_field("signature", signature))?,
        }),
        _ => {
            return Err(invalid_field(
                "attestation",
                "publisher and signature are both required",
            ));
        }
    };
    let defaults = ContentManifest::default();
    let manifest = ContentManifest {
        name: file.name.unwrap_or(defaults.name),
        version: file.version.unwrap_or(defaults.version),
        author: file.author.unwrap_or(defaults.author),
    };
    let content = GameContent {
        manifest,
        items,
        heroes,
        abilities,
        modifiers,
        rules,
        map,
        attestation,
    };
    content.validate().map_err(ContentLoadError::Invalid)?;
    Ok(content)
}

/// Reads, parses and validates a content pack.
///
/// # Errors
/// Returns [`ContentLoadError`] for I/O, syntax or validation failures.
pub fn load_file(path: &Path) -> Result<GameContent, ContentLoadError> {
    let text = std::fs::read_to_string(path).map_err(ContentLoadError::Io)?;
    load_toml(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_content_is_valid_and_stable() {
        let content = GameContent::default();
        assert_eq!(content.items.len(), ITEMS.len());
        assert_eq!(content.validate(), Ok(()));
        assert_eq!(content.hash(), GameContent::default().hash());
    }

    #[test]
    fn map_only_pack_overlays_defaults() {
        let text = r#"
[map]
blue_spawn = [32, 128]
red_spawn = [224, 128]

[[map.lanes]]
name = "Mid"
path = [[32, 128], [128, 128], [224, 128]]

[[map.structures]]
team = "blue"
kind = "nexus"
position = [20, 128]

[[map.structures]]
team = "red"
kind = "nexus"
position = [236, 128]

[[map.walls]]
a = [80, 96]
b = [80, 160]
"#;
        let content = load_toml(text).expect("map pack");
        assert_eq!(content.items.len(), ITEMS.len());
        assert_eq!(content.map.lanes.len(), 1);
        assert_eq!(content.map.walls.len(), 1);
        assert_eq!(content.map.nexus_position(Team::Blue).x.to_i64(), 20);
    }

    #[test]
    fn example_mode_packs_validate() {
        for text in [
            include_str!("../../../content/1v1-mid.toml"),
            include_str!("../../../content/aram.toml"),
        ] {
            let content = load_toml(text).expect("parse mode pack");
            content.validate().expect("valid mode pack");
        }
    }

    #[test]
    fn attestation_fields_parse_without_changing_the_hash() {
        let plain = "name = \"Pack\"\nversion = \"1\"\nauthor = \"a\"\n";
        let signed = format!(
            "{plain}publisher = \"{}\"\nsignature = \"{}\"\n",
            "11".repeat(32),
            "22".repeat(64)
        );
        let unsigned = load_toml(plain).expect("plain");
        let signed = load_toml(&signed).expect("signed");
        let attestation = signed.attestation.as_ref().expect("attestation");
        assert_eq!(attestation.publisher, [0x11; 32]);
        assert_eq!(attestation.signature, [0x22; 64]);
        assert_eq!(signed.hash(), unsigned.hash());
    }

    #[test]
    fn map_extent_parses_and_scales_positions() {
        let text = r#"
[rules]
team_size = 1
bot_fill = false
hero_respawn_ticks = 120
respawn_per_level = 10
wave_interval_ticks = 600
first_wave_ticks = 60

[map]
extent = 128
blue_spawn = [10, 10]
red_spawn = [118, 118]

[[map.lanes]]
name = "Mid"
path = [[10, 10], [64, 64], [118, 118]]

[[map.structures]]
team = "blue"
kind = "nexus"
position = [10, 10]

[[map.structures]]
team = "red"
kind = "nexus"
position = [118, 118]
"#;
        let content = load_toml(text).expect("parse");
        assert_eq!(content.map.extent, Fix::from_i32(128));
        assert_eq!(content.rules.team_size, 1);
        assert!(!content.rules.bot_fill);
        assert_eq!(content.rules.hero_respawn_ticks, 120);
        assert_eq!(content.rules.respawn_per_level, 10);
        assert_eq!(content.rules.wave_interval_ticks, 600);
        assert_eq!(content.rules.first_wave_ticks, 60);
        content.validate().expect("valid 128 map");
    }

    #[test]
    fn example_pack_parses() {
        let content = load_toml(include_str!("../../../content/example-pack.toml"))
            .expect("example pack is valid");
        assert_eq!(content.items.len(), 8);
        assert_eq!(content.heroes.len(), 1);
        assert_eq!(content.heroes[0].name, "Vanguard");
        assert_eq!(content.manifest.name, "Example Pack");
        assert_eq!(content.manifest.version, "1.0.0");
    }

    #[test]
    fn ability_pack_parses_and_heroes_reference_it() {
        let text = r#"
[[items]]
name = "Stick"
cost = 1

[[abilities]]
name = "Zap"
cooldown_ticks = 60
mana_cost = 10
cast_range = 5.0
kind = "burst"
radius = 3.0
damage = 50
damage_type = "magic"

[[heroes]]
name = "Spark"
max_health = 500
max_mana = 300
attack_damage = 40
attack_range = 8.0
attack_interval_ticks = 30
move_speed = 6.0
vision_range = 14.0
abilities = [0, 0, 0]
"#;
        let content = load_toml(text).expect("ability pack");
        assert_eq!(content.abilities.len(), 1);
        assert_eq!(content.abilities[0].name, "Zap");
        assert!(matches!(
            content.abilities[0].targeting,
            Targeting::Point { .. }
        ));
        assert!(content.abilities[0].effects.iter().any(|effect| matches!(
            effect,
            Effect::Area { effects, .. }
                if effects.iter().any(|effect| matches!(
                    effect,
                    AreaEffect::Damage { amount: 50, .. }
                ))
        )));
    }

    #[test]
    fn manifest_validation_rejects_empty_fields() {
        let mut content = GameContent::default();
        content.manifest.author.clear();
        assert_eq!(content.validate(), Err(ContentError::BadManifest));
    }

    #[test]
    fn hero_validation_rejects_bad_ability_ids() {
        let mut content = GameContent::default();
        content.heroes[0].abilities = [255, 0, 0];
        assert_eq!(content.validate(), Err(ContentError::BadAbility));
        let mut content = GameContent::default();
        content.heroes[0].max_health = 0;
        assert_eq!(content.validate(), Err(ContentError::BadHeroStats));
    }

    #[test]
    fn strict_schema_rejects_unknown_fields_and_enums() {
        assert!(matches!(
            load_toml("bogus = 1"),
            Err(ContentLoadError::Toml(_))
        ));
        assert!(matches!(
            load_toml("[[items]]\nname = \"X\"\ncost = 1\nextra = 2\n"),
            Err(ContentLoadError::Toml(_))
        ));
        let bad_kind = r#"
[[abilities]]
name = "Bad"
cooldown_ticks = 1
kind = "teleport"
"#;
        assert!(matches!(
            load_toml(bad_kind),
            Err(ContentLoadError::InvalidField {
                field: "abilities.kind",
                ..
            })
        ));
        let bad_damage = r#"
[[abilities]]
name = "Bad"
cooldown_ticks = 1
kind = "burst"
radius = 1.0
damage = 1
damage_type = "holy"
"#;
        assert!(matches!(
            load_toml(bad_damage),
            Err(ContentLoadError::InvalidField {
                field: "abilities.damage_type",
                ..
            })
        ));
        let bad_team = r#"
[map]
blue_spawn = [32, 128]
red_spawn = [224, 128]
[[map.lanes]]
name = "Mid"
path = [[32, 128], [224, 128]]
[[map.structures]]
team = "green"
kind = "nexus"
position = [20, 128]
"#;
        assert!(matches!(
            load_toml(bad_team),
            Err(ContentLoadError::InvalidField {
                field: "map.structures.team",
                ..
            })
        ));
        let bad_structure = bad_team.replace("green", "blue").replace("nexus", "spire");
        assert!(matches!(
            load_toml(&bad_structure),
            Err(ContentLoadError::InvalidField {
                field: "map.structures.kind",
                ..
            })
        ));
        let non_finite = r#"
[[items]]
name = "X"
cost = 1
move_speed = nan
"#;
        assert!(matches!(
            load_toml(non_finite),
            Err(ContentLoadError::InvalidField {
                field: "items.move_speed",
                ..
            })
        ));
    }

    #[test]
    fn modifier_definitions_are_validated_and_hashed() {
        let mut content = GameContent::default();
        let baseline = content.hash();
        content.modifiers[0].name = String::from("Barrier");
        assert_ne!(content.hash(), baseline);
        assert_eq!(content.validate(), Ok(()));

        let mut content = GameContent::default();
        content
            .modifiers
            .retain(|modifier| modifier.kind != ModifierKind::Burn);
        assert_eq!(content.validate(), Err(ContentError::BadModifiers));

        let unknown = r#"
[[modifiers]]
name = "Odd"
kind = "charm"
"#;
        assert!(matches!(
            load_toml(unknown),
            Err(ContentLoadError::InvalidField {
                field: "modifiers.kind",
                ..
            })
        ));
    }

    #[test]
    fn validation_rejects_out_of_range_stats_and_map_coordinates() {
        let mut content = GameContent::default();
        content.items[0].health = MAX_ITEM_STAT + 1;
        assert_eq!(content.validate(), Err(ContentError::BadItemStats));
        let mut content = GameContent::default();
        content.heroes[0].max_health = MAX_STAT + 1;
        assert_eq!(content.validate(), Err(ContentError::BadHeroStats));
        let mut content = GameContent::default();
        content.map.blue_spawn = FixVec2::from_ints(-1, 128);
        assert_eq!(content.validate(), Err(ContentError::BadMapData));
        let mut content = GameContent::default();
        content.abilities[0].cooldown_ticks = MAX_COOLDOWN_TICKS + 1;
        assert_eq!(content.validate(), Err(ContentError::BadAbilityStats));
    }

    #[test]
    fn validation_rejects_bad_names_and_duplicates() {
        let mut content = GameContent::default();
        content.items[0].name.clear();
        assert_eq!(content.validate(), Err(ContentError::BadName));
        let mut content = GameContent::default();
        content.items[1].name = content.items[0].name.clone();
        assert_eq!(content.validate(), Err(ContentError::DuplicateName));
        let mut content = GameContent::default();
        content.items[0].cost = 0;
        assert_eq!(content.validate(), Err(ContentError::FreeItem));
    }
}
