//! Hero abilities.
//!
//! Abilities are static definitions with an id. The simulation stores cooldowns and casts
//! them; the definition decides what a cast does. M8 replaces the static table with loaded
//! content, but the simulation-facing shape stays the same.

use moba_core::Fix;

use crate::combat::DamageType;

/// Maximum top-level effects per ability.
pub const MAX_ABILITY_EFFECTS: usize = 3;
/// Maximum area/on-hit effects per list.
pub const MAX_AREA_EFFECTS: usize = 3;

/// How an ability chooses its target point.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Targeting {
    /// Applies at the caster's position.
    SelfCast,
    /// A ground point within `range` (zero means self-centred).
    Point {
        /// Maximum cast distance.
        range: Fix,
    },
    /// A direction/point at distance `0..=range`.
    Direction {
        /// Maximum cast distance.
        range: Fix,
    },
}

impl Targeting {
    /// Maximum cast range (`0` for self-cast).
    #[must_use]
    pub const fn range(self) -> Fix {
        match self {
            Self::SelfCast => Fix::ZERO,
            Self::Point { range } | Self::Direction { range } => range,
        }
    }
}

/// One effect applied to every valid target in an area or on projectile hit.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AreaEffect {
    /// No effect (empty slot).
    None,
    /// Damage with mitigation type.
    Damage {
        /// Raw damage.
        amount: i32,
        /// Mitigation type.
        damage_type: DamageType,
    },
    /// Apply a content modifier definition.
    ApplyModifier {
        /// Modifier definition id.
        modifier: u16,
        /// Duration in ticks.
        duration_ticks: u32,
        /// Movement factor for slow/haste.
        factor: Fix,
        /// Shield absorption or per-tick burn damage.
        amount: i32,
        /// Burn mitigation type.
        damage_type: DamageType,
    },
}

/// Fixed-capacity area effect list.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AreaEffectList {
    /// Effect slots.
    pub effects: [AreaEffect; MAX_AREA_EFFECTS],
}

impl AreaEffectList {
    /// Empty list.
    pub const EMPTY: Self = Self {
        effects: [AreaEffect::None; MAX_AREA_EFFECTS],
    };

    /// List with one effect.
    #[must_use]
    pub const fn one(effect: AreaEffect) -> Self {
        let mut list = Self::EMPTY;
        list.effects[0] = effect;
        list
    }

    /// Iterates populated effects.
    pub fn iter(&self) -> impl Iterator<Item = &AreaEffect> {
        self.effects
            .iter()
            .filter(|effect| !matches!(effect, AreaEffect::None))
    }

    /// Whether the list has no effects.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }
}

/// One top-level ability effect.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// No effect (empty slot).
    None,
    /// Immediate damage to enemies at the target.
    Damage {
        /// Raw damage.
        amount: i32,
        /// Mitigation type.
        damage_type: DamageType,
    },
    /// Heal the caster.
    Heal {
        /// Health restored.
        amount: i32,
    },
    /// Apply a content modifier to the caster.
    ApplyModifier {
        /// Modifier definition id.
        modifier: u16,
        /// Duration in ticks.
        duration_ticks: u32,
        /// Movement factor (slow/haste).
        factor: Fix,
        /// Shield absorption or per-tick burn damage.
        amount: i32,
        /// Burn mitigation type.
        damage_type: DamageType,
    },
    /// Displace the caster toward the target point.
    Displace {
        /// Maximum displacement.
        range: Fix,
    },
    /// Spawn a projectile with on-hit area effects.
    Projectile {
        /// Travel speed per second.
        speed_per_sec: Fix,
        /// Maximum travel distance.
        range: Fix,
        /// Collision radius.
        radius: Fix,
        /// Effects applied to the first enemy hit.
        on_hit: AreaEffectList,
    },
    /// Apply effects to enemies around the target point.
    Area {
        /// Effect radius.
        radius: Fix,
        /// Effects applied to each enemy.
        effects: AreaEffectList,
    },
    /// Spawn a persistent periodic zone.
    Zone {
        /// Zone radius.
        radius: Fix,
        /// Lifetime in ticks.
        duration_ticks: u32,
        /// Ticks between applications.
        period_ticks: u32,
        /// Effects applied per period.
        effects: AreaEffectList,
    },
}

/// Fixed-capacity top-level effect list.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct EffectList {
    /// Effect slots.
    pub effects: [Effect; MAX_ABILITY_EFFECTS],
}

impl EffectList {
    /// Empty list.
    pub const EMPTY: Self = Self {
        effects: [Effect::None; MAX_ABILITY_EFFECTS],
    };

    /// List with one effect.
    #[must_use]
    pub const fn one(effect: Effect) -> Self {
        let mut list = Self::EMPTY;
        list.effects[0] = effect;
        list
    }

    /// Iterates populated effects.
    pub fn iter(&self) -> impl Iterator<Item = &Effect> {
        self.effects
            .iter()
            .filter(|effect| !matches!(effect, Effect::None))
    }

    /// Whether the list has no effects.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }
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
    /// Targeting mode.
    pub targeting: Targeting,
    /// Ordered effects.
    pub effects: EffectList,
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
        targeting: Targeting::Direction { range: fx(7) },
        effects: EffectList::one(Effect::Displace { range: fx(7) }),
    },
    AbilityDef {
        name: "Second Wind",
        cooldown_ticks: 480,
        mana_cost: 30,
        targeting: Targeting::SelfCast,
        effects: EffectList::one(Effect::Heal { amount: 140 }),
    },
    AbilityDef {
        name: "Cleaving Throw",
        cooldown_ticks: 300,
        mana_cost: 50,
        targeting: Targeting::Direction { range: fx(11) },
        effects: EffectList::one(Effect::Projectile {
            speed_per_sec: fx(26),
            range: fx(11),
            radius: fx(1),
            on_hit: AreaEffectList::one(AreaEffect::Damage {
                amount: 110,
                damage_type: DamageType::Physical,
            }),
        }),
    },
    // Ranger: long physical skillshot, heal, short dash.
    AbilityDef {
        name: "Piercing Arrow",
        cooldown_ticks: 300,
        mana_cost: 45,
        targeting: Targeting::Direction { range: fx(14) },
        effects: EffectList::one(Effect::Projectile {
            speed_per_sec: fx(30),
            range: fx(14),
            radius: fx(1),
            on_hit: AreaEffectList::one(AreaEffect::Damage {
                amount: 125,
                damage_type: DamageType::Physical,
            }),
        }),
    },
    AbilityDef {
        name: "Field Dressing",
        cooldown_ticks: 480,
        mana_cost: 35,
        targeting: Targeting::SelfCast,
        effects: EffectList::one(Effect::Heal { amount: 100 }),
    },
    AbilityDef {
        name: "Evade",
        cooldown_ticks: 360,
        mana_cost: 40,
        targeting: Targeting::Direction { range: fx(6) },
        effects: EffectList::one(Effect::Displace { range: fx(6) }),
    },
    // Mage: magic skillshot, point-blank nova, short blink.
    AbilityDef {
        name: "Arc Bolt",
        cooldown_ticks: 240,
        mana_cost: 50,
        targeting: Targeting::Direction { range: fx(13) },
        effects: EffectList::one(Effect::Projectile {
            speed_per_sec: fx(24),
            range: fx(13),
            radius: fx(1),
            on_hit: AreaEffectList::one(AreaEffect::Damage {
                amount: 135,
                damage_type: DamageType::Magic,
            }),
        }),
    },
    AbilityDef {
        name: "Nova",
        cooldown_ticks: 420,
        mana_cost: 70,
        targeting: Targeting::Point { range: fx(0) },
        effects: EffectList::one(Effect::Area {
            radius: fx(5),
            effects: AreaEffectList::one(AreaEffect::Damage {
                amount: 120,
                damage_type: DamageType::Magic,
            }),
        }),
    },
    AbilityDef {
        name: "Blink Step",
        cooldown_ticks: 360,
        mana_cost: 40,
        targeting: Targeting::Direction { range: fx(5) },
        effects: EffectList::one(Effect::Displace { range: fx(5) }),
    },
    // Guardian: shield bash, large heal, magic skillshot.
    AbilityDef {
        name: "Shield Bash",
        cooldown_ticks: 360,
        mana_cost: 50,
        targeting: Targeting::Point { range: fx(0) },
        effects: EffectList::one(Effect::Area {
            radius: fx(4),
            effects: AreaEffectList::one(AreaEffect::Damage {
                amount: 90,
                damage_type: DamageType::Physical,
            }),
        }),
    },
    AbilityDef {
        name: "Bulwark",
        cooldown_ticks: 480,
        mana_cost: 50,
        targeting: Targeting::SelfCast,
        effects: EffectList::one(Effect::Heal { amount: 180 }),
    },
    AbilityDef {
        name: "Judgement",
        cooldown_ticks: 300,
        mana_cost: 45,
        targeting: Targeting::Direction { range: fx(10) },
        effects: EffectList::one(Effect::Projectile {
            speed_per_sec: fx(22),
            range: fx(10),
            radius: fx(1),
            on_hit: AreaEffectList::one(AreaEffect::Damage {
                amount: 95,
                damage_type: DamageType::Magic,
            }),
        }),
    },
    // Assassin: dash, heal, high-damage skillshot.
    AbilityDef {
        name: "Shadow Step",
        cooldown_ticks: 300,
        mana_cost: 45,
        targeting: Targeting::Direction { range: fx(8) },
        effects: EffectList::one(Effect::Displace { range: fx(8) }),
    },
    AbilityDef {
        name: "Blood Rite",
        cooldown_ticks: 420,
        mana_cost: 35,
        targeting: Targeting::SelfCast,
        effects: EffectList::one(Effect::Heal { amount: 90 }),
    },
    AbilityDef {
        name: "Venom Dart",
        cooldown_ticks: 240,
        mana_cost: 55,
        targeting: Targeting::Direction { range: fx(12) },
        effects: EffectList::one(Effect::Projectile {
            speed_per_sec: fx(28),
            range: fx(12),
            radius: fx(1),
            on_hit: AreaEffectList::one(AreaEffect::Damage {
                amount: 140,
                damage_type: DamageType::Physical,
            }),
        }),
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
            assert!(!ability.effects.is_empty());
            for effect in ability.effects.iter() {
                match effect {
                    Effect::None => panic!("empty effect slot in default table"),
                    Effect::Damage { amount, .. } | Effect::Heal { amount } => {
                        assert!(*amount > 0);
                    }
                    Effect::ApplyModifier { duration_ticks, .. } => {
                        assert!(*duration_ticks > 0);
                    }
                    Effect::Displace { range } => assert!(*range > Fix::ZERO),
                    Effect::Projectile {
                        speed_per_sec,
                        range,
                        radius,
                        on_hit,
                    } => {
                        assert!(*speed_per_sec > Fix::ZERO);
                        assert!(*range > Fix::ZERO);
                        assert!(*radius > Fix::ZERO);
                        assert!(!on_hit.is_empty());
                    }
                    Effect::Area { radius, effects } => {
                        assert!(*radius > Fix::ZERO);
                        assert!(!effects.is_empty());
                    }
                    Effect::Zone {
                        radius,
                        duration_ticks,
                        period_ticks,
                        effects,
                    } => {
                        assert!(*radius > Fix::ZERO);
                        assert!(*duration_ticks > 0);
                        assert!(*period_ticks > 0);
                        assert!(!effects.is_empty());
                    }
                }
            }
        }
    }
}
