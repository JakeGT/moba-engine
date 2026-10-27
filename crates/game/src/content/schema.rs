//! Serde TOML schema: the on-disk `ContentFile` and its `*Entry` tables.
//!
//! Every table is `deny_unknown_fields`; defaults mirror the built-in [`RulesSpec`]. The
//! entries are converted into specs by `load.rs`. New TOML tables/fields belong here.

use serde::Deserialize;

use crate::DEFAULT_MAP_EXTENT;

use super::{MinionStatsSpec, RewardsSpec, RulesSpec, StructureStatsSpec};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModifierEntry {
    pub(super) name: String,
    pub(super) kind: String,
    #[serde(default)]
    pub(super) bonus_armor: i32,
    #[serde(default)]
    pub(super) bonus_attack_damage: i32,
    #[serde(default)]
    pub(super) on_expire_damage: i32,
    #[serde(default)]
    pub(super) on_damaged_damage: i32,
    #[serde(default)]
    pub(super) on_tick_damage: i32,
    #[serde(default)]
    pub(super) status_bits: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ContentFile {
    pub(super) name: Option<String>,
    pub(super) publisher: Option<String>,
    pub(super) signature: Option<String>,
    pub(super) version: Option<String>,
    pub(super) author: Option<String>,
    #[serde(default)]
    pub(super) items: Vec<ItemEntry>,
    pub(super) heroes: Option<Vec<HeroEntry>>,
    pub(super) abilities: Option<Vec<AbilityEntry>>,
    pub(super) modifiers: Option<Vec<ModifierEntry>>,
    pub(super) rules: Option<RulesEntry>,
    pub(super) map: Option<MapEntry>,
}

#[derive(Copy, Clone, Debug)]
pub(super) struct MinionStatsEntry {
    pub(super) max_health: i32,
    pub(super) attack_damage: i32,
    pub(super) attack_range: f32,
    pub(super) attack_interval_ticks: u32,
    pub(super) move_speed_per_sec: f32,
    pub(super) armor: i32,
    pub(super) magic_resist: i32,
    pub(super) vision_range: f32,
}

#[derive(Copy, Clone, Debug)]
pub(super) struct StructureStatsEntry {
    pub(super) max_health: i32,
    pub(super) attack_damage: i32,
    pub(super) attack_range: f32,
    pub(super) attack_interval_ticks: u32,
    pub(super) armor: i32,
    pub(super) magic_resist: i32,
    pub(super) vision_range: f32,
}

macro_rules! minion_fields {
    ($name:ident, $default:expr) => {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, default)]
        pub(super) struct $name {
            pub(super) max_health: i32,
            pub(super) attack_damage: i32,
            pub(super) attack_range: f32,
            pub(super) attack_interval_ticks: u32,
            pub(super) move_speed_per_sec: f32,
            pub(super) armor: i32,
            pub(super) magic_resist: i32,
            pub(super) vision_range: f32,
        }

        impl Default for $name {
            fn default() -> Self {
                let stats: MinionStatsSpec = $default;
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

        impl From<$name> for MinionStatsEntry {
            fn from(entry: $name) -> Self {
                Self {
                    max_health: entry.max_health,
                    attack_damage: entry.attack_damage,
                    attack_range: entry.attack_range,
                    attack_interval_ticks: entry.attack_interval_ticks,
                    move_speed_per_sec: entry.move_speed_per_sec,
                    armor: entry.armor,
                    magic_resist: entry.magic_resist,
                    vision_range: entry.vision_range,
                }
            }
        }
    };
}

minion_fields!(MeleeMinionEntry, RulesSpec::default().melee_minion);
minion_fields!(RangedMinionEntry, RulesSpec::default().ranged_minion);

macro_rules! structure_fields {
    ($name:ident, $default:expr) => {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, default)]
        pub(super) struct $name {
            pub(super) max_health: i32,
            pub(super) attack_damage: i32,
            pub(super) attack_range: f32,
            pub(super) attack_interval_ticks: u32,
            pub(super) armor: i32,
            pub(super) magic_resist: i32,
            pub(super) vision_range: f32,
        }

        impl Default for $name {
            fn default() -> Self {
                let stats: StructureStatsSpec = $default;
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

        impl From<$name> for StructureStatsEntry {
            fn from(entry: $name) -> Self {
                Self {
                    max_health: entry.max_health,
                    attack_damage: entry.attack_damage,
                    attack_range: entry.attack_range,
                    attack_interval_ticks: entry.attack_interval_ticks,
                    armor: entry.armor,
                    magic_resist: entry.magic_resist,
                    vision_range: entry.vision_range,
                }
            }
        }
    };
}

structure_fields!(TowerStatsEntry, RulesSpec::default().tower);
structure_fields!(NexusStatsEntry, RulesSpec::default().nexus);

#[derive(Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(super) struct RulesEntry {
    pub(super) melee_minion: MeleeMinionEntry,
    pub(super) ranged_minion: RangedMinionEntry,
    pub(super) tower: TowerStatsEntry,
    pub(super) nexus: NexusStatsEntry,
    pub(super) level_cap: u8,
    pub(super) xp_base: u32,
    pub(super) xp_per_level: u32,
    pub(super) melee_vision: f32,
    pub(super) ranged_vision: f32,
    pub(super) tower_vision: f32,
    pub(super) nexus_vision: f32,
    pub(super) team_size: u8,
    pub(super) bot_fill: bool,
    pub(super) hero_respawn_ticks: u32,
    pub(super) respawn_per_level: u32,
    pub(super) wave_interval_ticks: u32,
    pub(super) first_wave_ticks: u32,
    pub(super) starting_gold: u32,
    pub(super) gold_per_interval: u32,
    pub(super) gold_tick_interval: u32,
    pub(super) xp_radius: f32,
    pub(super) shop_radius: f32,
    pub(super) shared_xp_percent: u32,
    pub(super) rewards: RewardsEntry,
    pub(super) structure_heroes_first: bool,
    pub(super) nexus_win: bool,
    pub(super) score_limit: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(super) struct RewardsEntry {
    pub(super) melee_minion: UnitRewardsEntry,
    pub(super) ranged_minion: UnitRewardsEntry,
    pub(super) tower: UnitRewardsEntry,
    pub(super) nexus: UnitRewardsEntry,
    pub(super) hero: UnitRewardsEntry,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub(super) struct UnitRewardsEntry {
    pub(super) xp: u32,
    pub(super) gold: u32,
}

impl Default for RewardsEntry {
    fn default() -> Self {
        let rewards = RewardsSpec::default();
        Self {
            melee_minion: UnitRewardsEntry {
                xp: rewards.melee_minion.xp,
                gold: rewards.melee_minion.gold,
            },
            ranged_minion: UnitRewardsEntry {
                xp: rewards.ranged_minion.xp,
                gold: rewards.ranged_minion.gold,
            },
            tower: UnitRewardsEntry {
                xp: rewards.tower.xp,
                gold: rewards.tower.gold,
            },
            nexus: UnitRewardsEntry {
                xp: rewards.nexus.xp,
                gold: rewards.nexus.gold,
            },
            hero: UnitRewardsEntry {
                xp: rewards.hero.xp,
                gold: rewards.hero.gold,
            },
        }
    }
}

impl Default for RulesEntry {
    fn default() -> Self {
        let rules = RulesSpec::default();
        Self {
            melee_minion: MeleeMinionEntry::default(),
            ranged_minion: RangedMinionEntry::default(),
            tower: TowerStatsEntry::default(),
            nexus: NexusStatsEntry::default(),
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
            starting_gold: rules.starting_gold,
            gold_per_interval: rules.gold_per_interval,
            gold_tick_interval: rules.gold_tick_interval,
            xp_radius: rules.xp_radius.to_f32(),
            shop_radius: rules.shop_radius.to_f32(),
            shared_xp_percent: rules.shared_xp_percent,
            rewards: RewardsEntry::default(),
            structure_heroes_first: rules.structure_heroes_first,
            nexus_win: rules.nexus_win,
            score_limit: rules.score_limit,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MapEntry {
    #[serde(default = "default_map_name")]
    pub(super) name: String,
    #[serde(default = "default_map_theme")]
    pub(super) theme: String,
    #[serde(default = "default_map_extent")]
    pub(super) extent: i32,
    pub(super) blue_spawn: [i32; 2],
    pub(super) red_spawn: [i32; 2],
    pub(super) lanes: Vec<LaneEntry>,
    #[serde(default)]
    pub(super) structures: Vec<StructureEntry>,
    #[serde(default)]
    pub(super) walls: Vec<WallEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WallEntry {
    pub(super) a: [i32; 2],
    pub(super) b: [i32; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LaneEntry {
    pub(super) name: String,
    pub(super) path: Vec<[i32; 2]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StructureEntry {
    pub(super) team: String,
    pub(super) kind: String,
    pub(super) position: [i32; 2],
    #[serde(default)]
    pub(super) lane: Option<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ItemEntry {
    pub(super) name: String,
    pub(super) cost: u32,
    #[serde(default)]
    pub(super) health: i32,
    #[serde(default)]
    pub(super) mana: i32,
    #[serde(default)]
    pub(super) attack_damage: i32,
    #[serde(default)]
    pub(super) armor: i32,
    #[serde(default)]
    pub(super) magic_resist: i32,
    #[serde(default)]
    pub(super) move_speed: f32,
    #[serde(default)]
    pub(super) on_hit_damage: u16,
    #[serde(default)]
    pub(super) on_damaged_damage: u16,
    #[serde(default)]
    pub(super) active_heal: i32,
    #[serde(default)]
    pub(super) active_cooldown_ticks: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AbilityEntry {
    pub(super) name: String,
    pub(super) cooldown_ticks: u32,
    #[serde(default)]
    pub(super) mana_cost: i32,
    #[serde(default)]
    pub(super) cast_range: f32,
    pub(super) kind: String,
    #[serde(default)]
    pub(super) speed: f32,
    #[serde(default)]
    pub(super) range: f32,
    #[serde(default)]
    pub(super) radius: f32,
    /// Visual projectile arc height at mid-flight.
    #[serde(default)]
    pub(super) arc: f32,
    #[serde(default)]
    pub(super) damage: i32,
    #[serde(default)]
    pub(super) amount: i32,
    #[serde(default)]
    pub(super) duration_ticks: u32,
    #[serde(default)]
    pub(super) factor: f32,
    #[serde(default = "default_damage_type")]
    pub(super) damage_type: String,
    /// Optional status kind applied on a skillshot hit (e.g. `root`).
    #[serde(default)]
    pub(super) on_hit: Option<String>,
    /// Duration for the on-hit status (falls back to `duration_ticks`).
    #[serde(default)]
    pub(super) on_hit_duration_ticks: u32,
    /// Period between zone applications (defaults to 30).
    #[serde(default)]
    pub(super) period_ticks: u32,
}

fn default_map_name() -> String {
    String::from("Map")
}

fn default_map_theme() -> String {
    String::from("grass")
}

const fn default_map_extent() -> i32 {
    DEFAULT_MAP_EXTENT
}

fn default_damage_type() -> String {
    String::from("physical")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HeroEntry {
    pub(super) name: String,
    pub(super) max_health: i32,
    pub(super) max_mana: i32,
    #[serde(default)]
    pub(super) health_regen_per_tick: i32,
    #[serde(default)]
    pub(super) mana_regen_per_tick: i32,
    pub(super) attack_damage: i32,
    pub(super) attack_range: f32,
    pub(super) attack_interval_ticks: u32,
    pub(super) move_speed: f32,
    #[serde(default)]
    pub(super) armor: i32,
    #[serde(default)]
    pub(super) magic_resist: i32,
    pub(super) vision_range: f32,
    pub(super) abilities: [u8; 3],
    pub(super) health_per_level: Option<i32>,
    pub(super) mana_per_level: Option<i32>,
    pub(super) attack_damage_per_level: Option<i32>,
    pub(super) armor_per_level: Option<i32>,
    pub(super) magic_resist_per_level: Option<i32>,
}
