//! Hero abilities.
//!
//! Abilities are static definitions with an id. The simulation stores cooldowns and casts
//! them; the definition decides what a cast does. M8 replaces the static table with loaded
//! content, but the simulation-facing shape stays the same.

use moba_core::Fix;

use crate::combat::DamageType;

/// What happens when an ability is cast.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AbilityKind {
    /// Fires a projectile in the direction of the target point.
    Skillshot {
        /// Travel speed in world units per second.
        speed_per_sec: Fix,
        /// Maximum travel distance.
        range: Fix,
        /// Collision radius around the projectile path.
        radius: Fix,
        /// Damage on hit.
        damage: i32,
        /// Mitigation type.
        damage_type: DamageType,
    },
    /// Instantly moves the caster toward the target point by at most `range`.
    Dash {
        /// Maximum dash distance.
        range: Fix,
    },
    /// Instantly heals the caster.
    Heal {
        /// Health restored.
        amount: i32,
        /// Visual/UI radius (unused by the simulation maths).
        radius: Fix,
    },
    /// Grants the caster a temporary damage-absorbing shield.
    Shield {
        /// Damage absorbed before health.
        amount: i32,
        /// Duration in simulation ticks.
        duration_ticks: u32,
    },
    /// Slows enemies around the target point for a duration.
    Slow {
        /// Effect radius.
        radius: Fix,
        /// Movement multiplier (0..1).
        factor: Fix,
        /// Duration in simulation ticks.
        duration_ticks: u32,
    },
    /// Instantly damages enemies around the caster.
    Burst {
        /// Effect radius.
        radius: Fix,
        /// Damage dealt.
        damage: i32,
        /// Mitigation type.
        damage_type: DamageType,
    },
}

/// A castable ability.
#[derive(Copy, Clone, Debug)]
pub struct AbilityDef {
    /// Name shown in UI and logs.
    pub name: &'static str,
    /// Cooldown in simulation ticks.
    pub cooldown_ticks: u32,
    /// Mana cost.
    pub mana_cost: i32,
    /// Maximum cast distance from the caster.
    pub cast_range: Fix,
    /// Effect.
    pub kind: AbilityKind,
}

const fn fx(value: i32) -> Fix {
    Fix::from_i32(value)
}

/// Every ability in stable id order. Hero definitions reference these by index.
pub const ABILITIES: &[AbilityDef] = &[
    // Blade: dash, self-heal, heavy skillshot.
    AbilityDef {
        name: "Blade Dash",
        cooldown_ticks: 240,
        mana_cost: 40,
        cast_range: fx(7),
        kind: AbilityKind::Dash { range: fx(7) },
    },
    AbilityDef {
        name: "Second Wind",
        cooldown_ticks: 480,
        mana_cost: 30,
        cast_range: fx(0),
        kind: AbilityKind::Heal {
            amount: 140,
            radius: fx(1),
        },
    },
    AbilityDef {
        name: "Cleaving Throw",
        cooldown_ticks: 300,
        mana_cost: 50,
        cast_range: fx(11),
        kind: AbilityKind::Skillshot {
            speed_per_sec: fx(26),
            range: fx(11),
            radius: fx(1),
            damage: 110,
            damage_type: DamageType::Physical,
        },
    },
    // Ranger: long physical skillshot, heal, short dash.
    AbilityDef {
        name: "Piercing Arrow",
        cooldown_ticks: 300,
        mana_cost: 45,
        cast_range: fx(14),
        kind: AbilityKind::Skillshot {
            speed_per_sec: fx(30),
            range: fx(14),
            radius: fx(1),
            damage: 125,
            damage_type: DamageType::Physical,
        },
    },
    AbilityDef {
        name: "Field Dressing",
        cooldown_ticks: 480,
        mana_cost: 35,
        cast_range: fx(0),
        kind: AbilityKind::Heal {
            amount: 100,
            radius: fx(1),
        },
    },
    AbilityDef {
        name: "Evade",
        cooldown_ticks: 360,
        mana_cost: 40,
        cast_range: fx(6),
        kind: AbilityKind::Dash { range: fx(6) },
    },
    // Mage: magic skillshot, point-blank nova, short blink.
    AbilityDef {
        name: "Arc Bolt",
        cooldown_ticks: 240,
        mana_cost: 50,
        cast_range: fx(13),
        kind: AbilityKind::Skillshot {
            speed_per_sec: fx(24),
            range: fx(13),
            radius: fx(1),
            damage: 135,
            damage_type: DamageType::Magic,
        },
    },
    AbilityDef {
        name: "Nova",
        cooldown_ticks: 420,
        mana_cost: 70,
        cast_range: fx(0),
        kind: AbilityKind::Burst {
            radius: fx(5),
            damage: 120,
            damage_type: DamageType::Magic,
        },
    },
    AbilityDef {
        name: "Blink Step",
        cooldown_ticks: 360,
        mana_cost: 40,
        cast_range: fx(5),
        kind: AbilityKind::Dash { range: fx(5) },
    },
    // Guardian: shield bash, large heal, magic skillshot.
    AbilityDef {
        name: "Shield Bash",
        cooldown_ticks: 360,
        mana_cost: 50,
        cast_range: fx(0),
        kind: AbilityKind::Burst {
            radius: fx(4),
            damage: 90,
            damage_type: DamageType::Physical,
        },
    },
    AbilityDef {
        name: "Bulwark",
        cooldown_ticks: 480,
        mana_cost: 50,
        cast_range: fx(0),
        kind: AbilityKind::Heal {
            amount: 180,
            radius: fx(2),
        },
    },
    AbilityDef {
        name: "Judgement",
        cooldown_ticks: 300,
        mana_cost: 45,
        cast_range: fx(10),
        kind: AbilityKind::Skillshot {
            speed_per_sec: fx(22),
            range: fx(10),
            radius: fx(1),
            damage: 95,
            damage_type: DamageType::Magic,
        },
    },
    // Assassin: dash, heal, high-damage skillshot.
    AbilityDef {
        name: "Shadow Step",
        cooldown_ticks: 300,
        mana_cost: 45,
        cast_range: fx(8),
        kind: AbilityKind::Dash { range: fx(8) },
    },
    AbilityDef {
        name: "Blood Rite",
        cooldown_ticks: 420,
        mana_cost: 35,
        cast_range: fx(0),
        kind: AbilityKind::Heal {
            amount: 90,
            radius: fx(1),
        },
    },
    AbilityDef {
        name: "Venom Dart",
        cooldown_ticks: 240,
        mana_cost: 55,
        cast_range: fx(12),
        kind: AbilityKind::Skillshot {
            speed_per_sec: fx(28),
            range: fx(12),
            radius: fx(1),
            damage: 140,
            damage_type: DamageType::Physical,
        },
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ability_table_is_complete_and_deterministic() {
        assert_eq!(ABILITIES.len(), 15);
        for ability in ABILITIES {
            assert!(ability.cooldown_ticks > 0);
            assert!(ability.mana_cost >= 0);
            match ability.kind {
                AbilityKind::Skillshot { damage, .. } | AbilityKind::Burst { damage, .. } => {
                    assert!(damage > 0);
                }
                AbilityKind::Heal { amount, .. } => assert!(amount > 0),
                AbilityKind::Dash { range } => assert!(range > Fix::ZERO),
                AbilityKind::Shield {
                    amount,
                    duration_ticks,
                } => {
                    assert!(amount > 0);
                    assert!(duration_ticks > 0);
                }
                AbilityKind::Slow {
                    radius,
                    factor,
                    duration_ticks,
                } => {
                    assert!(radius > Fix::ZERO);
                    assert!(factor > Fix::ZERO && factor < Fix::ONE);
                    assert!(duration_ticks > 0);
                }
            }
        }
    }
}
