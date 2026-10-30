//! In-memory, validated content types: rules, rewards, map, ability, hero and item specs.
//!
//! These are the runtime shapes the simulation reads. TOML parsing lives in `schema.rs`/
//! `load.rs`, range checks in `validate.rs` and the canonical hash feed in `hash.rs`.

use moba_core::{Fix, FixVec2};

use crate::{
    AbilityDef, DEFAULT_MAP_EXTENT, Effect, EffectList, ItemDef, MapDef, StructureKind, Targeting,
    Team,
};

/// Built-in visual themes a pack may reference.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum MapTheme {
    /// Temperate green.
    #[default]
    Grass,
    /// Grey stone.
    Stone,
    /// Cold snow.
    Snow,
    /// Warm desert.
    Desert,
    /// Arcane violet.
    Arcane,
}

impl MapTheme {
    /// Stable asset-manifest key for this map theme.
    #[must_use]
    pub const fn asset_name(self) -> &'static str {
        match self {
            Self::Grass => "map-grass",
            Self::Stone => "map-stone",
            Self::Snow => "map-snow",
            Self::Desert => "map-desert",
            Self::Arcane => "map-arcane",
        }
    }

    /// Parses a theme id.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "grass" => Self::Grass,
            "stone" => Self::Stone,
            "snow" => Self::Snow,
            "desert" => Self::Desert,
            "arcane" => Self::Arcane,
            _ => return None,
        })
    }

    /// Stable id used on the wire/hash.
    #[must_use]
    pub const fn id(self) -> u8 {
        match self {
            Self::Grass => 0,
            Self::Stone => 1,
            Self::Snow => 2,
            Self::Desert => 3,
            Self::Arcane => 4,
        }
    }

    /// Decodes a wire id.
    #[must_use]
    pub const fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0 => Self::Grass,
            1 => Self::Stone,
            2 => Self::Snow,
            3 => Self::Desert,
            4 => Self::Arcane,
            _ => return None,
        })
    }
}

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
    /// Prevents movement and casts.
    Stun = 6,
}

/// XP and gold awarded for a kill.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitRewardsSpec {
    /// XP granted to nearby allies.
    pub xp: u32,
    /// Gold granted to the last hitter.
    pub gold: u32,
}

/// Per-unit-kind rewards.
#[allow(missing_docs)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RewardsSpec {
    pub melee_minion: UnitRewardsSpec,
    pub ranged_minion: UnitRewardsSpec,
    pub tower: UnitRewardsSpec,
    pub nexus: UnitRewardsSpec,
    pub hero: UnitRewardsSpec,
}

impl Default for RewardsSpec {
    fn default() -> Self {
        Self {
            melee_minion: UnitRewardsSpec { xp: 32, gold: 20 },
            ranged_minion: UnitRewardsSpec { xp: 45, gold: 25 },
            tower: UnitRewardsSpec { xp: 220, gold: 250 },
            nexus: UnitRewardsSpec { xp: 300, gold: 500 },
            hero: UnitRewardsSpec { xp: 180, gold: 300 },
        }
    }
}

/// An owned modifier definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModifierDef {
    /// Display name (1..=16 ASCII bytes).
    pub name: String,
    /// Behaviour kind.
    pub kind: ModifierKind,
    /// Flat armour bonus while active.
    pub bonus_armor: i32,
    /// Flat attack-damage bonus while active.
    pub bonus_attack_damage: i32,
    /// Damage dealt when the modifier expires.
    pub on_expire_damage: i32,
    /// Damage reflected to attackers while active.
    pub on_damaged_damage: i32,
    /// Damage dealt each tick while active.
    pub on_tick_damage: i32,
    /// Replicated status bits while active.
    pub status_bits: u16,
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
    /// Bonus on-hit damage.
    pub on_hit_damage: u16,
    /// Damage reflected to attackers.
    pub on_damaged_damage: u16,
    /// Self-heal applied by the active (0 = no active).
    pub active_heal: i32,
    /// Active cooldown in ticks (0 = no active).
    pub active_cooldown_ticks: u16,
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

/// Default recall channel length: 240 ticks (8 s at 30 Hz).
pub(crate) const DEFAULT_RECALL_TICKS: u32 = 240;

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
    /// Starting gold per hero.
    pub starting_gold: u32,
    /// Passive gold granted per interval.
    pub gold_per_interval: u32,
    /// Ticks between passive gold grants.
    pub gold_tick_interval: u32,
    /// Shared-XP radius around a kill.
    pub xp_radius: Fix,
    /// Shop interaction radius around the nexus.
    pub shop_radius: Fix,
    /// Shared minion XP percentage (100 = full).
    pub shared_xp_percent: u32,
    /// Kill rewards per unit kind.
    pub rewards: RewardsSpec,
    /// Whether structures prioritize heroes over minions.
    pub structure_heroes_first: bool,
    /// Whether destroying a nexus ends the match.
    pub nexus_win: bool,
    /// Score limit that ends the match (0 disables).
    pub score_limit: u16,
    /// Recall channel length in ticks (0 disables recall). The default is 240 (8 s).
    pub recall_ticks: u32,
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
            starting_gold: 500,
            gold_per_interval: 1,
            gold_tick_interval: 15,
            xp_radius: Fix::from_i32(30),
            shop_radius: Fix::from_i32(30),
            shared_xp_percent: 130,
            rewards: RewardsSpec::default(),
            structure_heroes_first: false,
            nexus_win: true,
            score_limit: 0,
            recall_ticks: DEFAULT_RECALL_TICKS,
        }
    }
}

/// An owned runtime map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapSpec {
    /// Display name.
    pub name: String,
    /// Visual theme id.
    pub theme: MapTheme,
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
    /// Explicit per-map visuals name (`map.visuals`), resolved by the client against its
    /// local asset root. `None` means [`MapTheme::asset_name`]; see [`MapSpec::visuals_name`].
    /// Presentation only, but hashed and carried on the wire like `theme`.
    pub visuals: Option<String>,
}

impl From<&MapDef> for MapSpec {
    fn from(map: &MapDef) -> Self {
        Self {
            name: String::from("Map"),
            theme: MapTheme::Grass,
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
            visuals: None,
        }
    }
}

/// `value` as a whole number of world units, or `None` when it has a fractional part.
const fn whole_units(value: Fix) -> Option<i64> {
    if value.fract().to_bits() == 0 {
        Some(value.to_i64())
    } else {
        None
    }
}

impl MapSpec {
    /// The visuals name a client resolves: the explicit `visuals`, else the theme's
    /// [`MapTheme::asset_name`].
    #[must_use]
    pub fn visuals_name(&self) -> &str {
        self.visuals
            .as_deref()
            .unwrap_or_else(|| self.theme.asset_name())
    }

    /// The exact `[x, y]` the network catalogue carries for a map point (lane waypoint,
    /// spawn, structure position or wall endpoint), or `None` when either axis is fractional
    /// or outside `i16`.
    ///
    /// Map geometry travels as whole world units. [`GameContent::validate`] rejects every
    /// point this returns `None` for, so the wire conversion is lossless for validated
    /// content and the client reconstructs the same content hash.
    ///
    /// [`GameContent::validate`]: super::GameContent::validate
    #[must_use]
    pub fn wire_point(point: FixVec2) -> Option<[i16; 2]> {
        let axis = |value: Fix| whole_units(value).and_then(|units| i16::try_from(units).ok());
        Some([axis(point.x)?, axis(point.y)?])
    }

    /// The exact value the network catalogue carries for [`MapSpec::extent`], or `None` when
    /// it is fractional or outside `u16`. See [`MapSpec::wire_point`].
    #[must_use]
    pub fn wire_extent(extent: Fix) -> Option<u16> {
        whole_units(extent).and_then(|units| u16::try_from(units).ok())
    }

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
    /// Optional model name (`heroes.model`), resolved by the client against its local asset
    /// root; `None` draws the procedural rig. Presentation only, but hashed and carried on
    /// the wire so every client asks for the same name.
    pub model: Option<String>,
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
            model: None,
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
            on_hit_damage: item.on_hit_damage,
            on_damaged_damage: item.on_damaged_damage,
            active_heal: item.active_heal,
            active_cooldown_ticks: item.active_cooldown_ticks,
        }
    }
}
