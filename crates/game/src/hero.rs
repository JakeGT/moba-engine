//! Hero definitions.

use moba_core::Fix;

use crate::ability::{ABILITIES, AbilityDef};

/// Index of a hero in [`HEROES`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct HeroId(pub u8);

impl HeroId {
    /// Returns the definition for this hero.
    #[inline]
    #[must_use]
    pub fn def(self) -> &'static HeroDef {
        &HEROES[self.0 as usize]
    }
}

/// Static stats and abilities of a hero.
///
/// The vertical slice has no levelling: every hero uses its level-one stats for the whole
/// match. Growth fields can be added without changing the simulation interface.
#[derive(Copy, Clone, Debug)]
pub struct HeroDef {
    /// Display name.
    pub name: &'static str,
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
    /// Armour (physical mitigation).
    pub armor: i32,
    /// Magic resistance.
    pub magic_resist: i32,
    /// Vision radius (used by fog of war from M5).
    pub vision_range: Fix,
    /// Ability ids into [`ABILITIES`], in Q/W/E order.
    pub abilities: [u8; 3],
}

impl HeroDef {
    /// Returns the definition of one ability slot.
    #[inline]
    #[must_use]
    pub fn ability(self, slot: usize) -> Option<&'static AbilityDef> {
        self.abilities
            .get(slot)
            .and_then(|&id| ABILITIES.get(id as usize))
    }
}

const fn fx(value: i32) -> Fix {
    Fix::from_i32(value)
}

/// Every hero in stable id order.
pub const HEROES: &[HeroDef] = &[
    HeroDef {
        name: "Blade",
        max_health: 760,
        max_mana: 280,
        health_regen_per_tick: 2,
        mana_regen_per_tick: 4,
        attack_damage: 62,
        attack_range: fx(2),
        attack_interval_ticks: 32,
        move_speed_per_sec: fx(7),
        armor: 28,
        magic_resist: 22,
        vision_range: fx(14),
        abilities: [0, 1, 2],
    },
    HeroDef {
        name: "Ranger",
        max_health: 540,
        max_mana: 320,
        health_regen_per_tick: 1,
        mana_regen_per_tick: 4,
        attack_damage: 54,
        attack_range: fx(9),
        attack_interval_ticks: 30,
        move_speed_per_sec: fx(7),
        armor: 18,
        magic_resist: 18,
        vision_range: fx(15),
        abilities: [3, 4, 5],
    },
    HeroDef {
        name: "Mage",
        max_health: 500,
        max_mana: 420,
        health_regen_per_tick: 1,
        mana_regen_per_tick: 5,
        attack_damage: 42,
        attack_range: fx(8),
        attack_interval_ticks: 34,
        move_speed_per_sec: fx(6),
        armor: 14,
        magic_resist: 24,
        vision_range: fx(14),
        abilities: [6, 7, 8],
    },
    HeroDef {
        name: "Guardian",
        max_health: 900,
        max_mana: 300,
        health_regen_per_tick: 2,
        mana_regen_per_tick: 3,
        attack_damage: 46,
        attack_range: fx(2),
        attack_interval_ticks: 36,
        move_speed_per_sec: fx(6),
        armor: 36,
        magic_resist: 30,
        vision_range: fx(13),
        abilities: [9, 10, 11],
    },
    HeroDef {
        name: "Assassin",
        max_health: 560,
        max_mana: 300,
        health_regen_per_tick: 1,
        mana_regen_per_tick: 4,
        attack_damage: 68,
        attack_range: fx(2),
        attack_interval_ticks: 28,
        move_speed_per_sec: fx(8),
        armor: 22,
        magic_resist: 18,
        vision_range: fx(13),
        abilities: [12, 13, 14],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hero_table_is_well_formed() {
        for (index, hero) in HEROES.iter().enumerate() {
            assert_ne!(hero.name, "");
            assert!(hero.max_health > 0);
            assert!(hero.max_mana > 0);
            assert!(hero.attack_damage > 0);
            assert!(hero.attack_range > Fix::ZERO);
            assert!(hero.move_speed_per_sec > Fix::ZERO);
            assert!(hero.attack_interval_ticks > 0);
            for slot in 0..3 {
                assert!(hero.ability(slot).is_some(), "hero {index} slot {slot}");
            }
        }
    }
}
