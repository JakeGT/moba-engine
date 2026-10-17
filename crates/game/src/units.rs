//! Non-hero unit and structure stats.

use moba_core::Fix;

/// Stats for a lane minion type.
#[derive(Copy, Clone, Debug)]
pub struct MinionDef {
    /// Display name.
    pub name: &'static str,
    /// Maximum health.
    pub max_health: i32,
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
}

/// Stats shared by towers and nexuses.
#[derive(Copy, Clone, Debug)]
pub struct StructureStats {
    /// Display name.
    pub name: &'static str,
    /// Maximum health.
    pub max_health: i32,
    /// Attack damage.
    pub attack_damage: i32,
    /// Attack range.
    pub attack_range: Fix,
    /// Attack interval in ticks.
    pub attack_interval_ticks: u32,
    /// Armour.
    pub armor: i32,
    /// Magic resistance.
    pub magic_resist: i32,
    /// Vision radius.
    pub vision_range: Fix,
}

const fn fx(value: i32) -> Fix {
    Fix::from_i32(value)
}

/// Melee lane minion.
pub const MELEE_MINION: MinionDef = MinionDef {
    name: "Melee Minion",
    max_health: 320,
    attack_damage: 14,
    attack_range: fx(2),
    attack_interval_ticks: 30,
    move_speed_per_sec: fx(4),
    armor: 10,
    magic_resist: 5,
    vision_range: fx(9),
};

/// Ranged lane minion.
pub const RANGED_MINION: MinionDef = MinionDef {
    name: "Ranged Minion",
    max_health: 220,
    attack_damage: 20,
    attack_range: fx(7),
    attack_interval_ticks: 32,
    move_speed_per_sec: fx(4),
    armor: 5,
    magic_resist: 5,
    vision_range: fx(10),
};

/// Lane tower.
pub const TOWER: StructureStats = StructureStats {
    name: "Tower",
    max_health: 2_200,
    attack_damage: 105,
    attack_range: fx(10),
    attack_interval_ticks: 30,
    armor: 40,
    magic_resist: 40,
    vision_range: fx(13),
};

/// Team nexus. Destroying it wins the match.
pub const NEXUS: StructureStats = StructureStats {
    name: "Nexus",
    max_health: 4_800,
    attack_damage: 85,
    attack_range: fx(9),
    attack_interval_ticks: 30,
    armor: 30,
    magic_resist: 30,
    vision_range: fx(14),
};
