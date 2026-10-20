//! Items, shop rules and progression constants.
//!
//! Item bonuses are flat integers and are applied once when purchased; the simulation
//! stays free of percentage/float stat aggregation. Experience uses the League-style
//! rising requirement curve (280 XP to level 2, +100 per level) with a level-15 cap for
//! the three-ability slice.

use moba_core::Fix;

/// Inventory slots per hero.
pub const ITEM_SLOTS: usize = 6;
/// Empty inventory slot sentinel on the wire and in [`crate::ITEM_SLOTS`] arrays.
pub const EMPTY_ITEM: u8 = 0xFF;
/// Starting gold.
pub const STARTING_GOLD: u32 = 500;
/// Gold granted every [`GOLD_TICK_INTERVAL`] ticks to living heroes.
pub const GOLD_PER_INTERVAL: u32 = 1;
/// Ticks between passive gold ticks (2 gold/second at 30 Hz).
pub const GOLD_TICK_INTERVAL: u32 = 15;
/// Distance from the team nexus within which purchases are allowed.
pub const SHOP_RADIUS: Fix = Fix::from_i32(30);
/// Level cap for this slice (Q/W/E only, no ultimate).
pub const LEVEL_CAP: u8 = 15;
/// Radius in world units that shares minion/structure experience.
pub const XP_RADIUS: Fix = Fix::from_i32(30);
/// Per-level stat growth. Kept in the content table so balance changes never touch the
/// simulation loop.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LevelGrowth {
    /// Maximum/current health gained.
    pub health: i32,
    /// Maximum/current mana gained.
    pub mana: i32,
    /// Attack damage gained.
    pub attack_damage: i32,
    /// Armour gained.
    pub armor: i32,
    /// Magic resistance gained.
    pub magic_resist: i32,
}

/// Default hero growth per level.
pub const LEVEL_GROWTH: LevelGrowth = LevelGrowth {
    health: 70,
    mana: 30,
    attack_damage: 4,
    armor: 1,
    magic_resist: 1,
};

/// Experience required to go from `level` to `level + 1`.
#[must_use]
pub const fn xp_to_next(level: u8) -> u32 {
    180 + 100 * level as u32
}

/// A purchasable item.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ItemDef {
    /// Display name.
    pub name: &'static str,
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
    /// Self-heal applied by the active (0 = no active).
    pub active_heal: i32,
    /// Active cooldown in ticks (0 = no active).
    pub active_cooldown_ticks: u16,
}

/// Item catalogue in stable id order.
pub const ITEMS: &[ItemDef] = &[
    ItemDef {
        name: "Ironblade",
        cost: 350,
        health: 0,
        mana: 0,
        attack_damage: 12,
        armor: 0,
        magic_resist: 0,
        move_speed_per_sec: Fix::ZERO,
        active_heal: 0,
        active_cooldown_ticks: 0,
    },
    ItemDef {
        name: "Chain Vest",
        cost: 400,
        health: 150,
        mana: 0,
        attack_damage: 0,
        armor: 8,
        magic_resist: 0,
        move_speed_per_sec: Fix::ZERO,
        active_heal: 0,
        active_cooldown_ticks: 0,
    },
    ItemDef {
        name: "Mana Crystal",
        cost: 350,
        health: 0,
        mana: 120,
        attack_damage: 0,
        armor: 0,
        magic_resist: 6,
        move_speed_per_sec: Fix::ZERO,
        active_heal: 0,
        active_cooldown_ticks: 0,
    },
    ItemDef {
        name: "Swift Boots",
        cost: 500,
        health: 0,
        mana: 0,
        attack_damage: 0,
        armor: 0,
        magic_resist: 0,
        move_speed_per_sec: Fix::from_bits(5_153_960_755),
        active_heal: 0,
        active_cooldown_ticks: 0,
    },
    ItemDef {
        name: "Ogre Heart",
        cost: 900,
        health: 350,
        mana: 0,
        attack_damage: 0,
        armor: 0,
        magic_resist: 0,
        move_speed_per_sec: Fix::ZERO,
        active_heal: 0,
        active_cooldown_ticks: 0,
    },
    ItemDef {
        name: "Warhammer",
        cost: 1_100,
        health: 0,
        mana: 0,
        attack_damage: 35,
        armor: 10,
        magic_resist: 0,
        move_speed_per_sec: Fix::ZERO,
        active_heal: 0,
        active_cooldown_ticks: 0,
    },
    ItemDef {
        name: "Spellguard",
        cost: 950,
        health: 0,
        mana: 250,
        attack_damage: 0,
        armor: 0,
        magic_resist: 25,
        move_speed_per_sec: Fix::ZERO,
        active_heal: 0,
        active_cooldown_ticks: 0,
    },
    ItemDef {
        name: "Bastion Plate",
        cost: 1_400,
        health: 500,
        mana: 0,
        attack_damage: 0,
        armor: 30,
        magic_resist: 0,
        move_speed_per_sec: Fix::ZERO,
        active_heal: 0,
        active_cooldown_ticks: 0,
    },
];
