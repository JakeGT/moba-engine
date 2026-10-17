//! Damage mitigation maths.

/// How an ability or attack deals damage.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DamageType {
    /// Reduced by armour.
    Physical,
    /// Reduced by magic resistance.
    Magic,
}

impl DamageType {
    /// The resistance that mitigates this damage type.
    #[inline]
    #[must_use]
    pub const fn resistance(self, armor: i32, magic_resist: i32) -> i32 {
        match self {
            Self::Physical => armor,
            Self::Magic => magic_resist,
        }
    }
}

/// Applies the classic `100 / (100 + resist)` mitigation curve using integer maths.
///
/// Negative resistances are clamped to zero (the vertical slice does not model shred), and
/// any positive damage deals at least one point. The result is deterministic: no floats,
/// no platform rounding.
#[inline]
#[must_use]
pub const fn mitigate(damage: i32, resist: i32) -> i32 {
    if damage <= 0 {
        return 0;
    }
    let resist = if resist < 0 { 0 } else { resist };
    let mitigated = damage * 100 / (100 + resist);
    if mitigated < 1 { 1 } else { mitigated }
}

/// Damage after applying the defender's relevant resistance.
#[inline]
#[must_use]
pub const fn mitigate_typed(
    damage: i32,
    damage_type: DamageType,
    armor: i32,
    magic_resist: i32,
) -> i32 {
    mitigate(damage, damage_type.resistance(armor, magic_resist))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mitigation_curve_is_monotonic_and_deterministic() {
        assert_eq!(mitigate(100, 0), 100);
        assert_eq!(mitigate(100, 100), 50);
        assert_eq!(mitigate(100, 300), 25);
        assert_eq!(
            mitigate(1, 10_000),
            1,
            "positive damage never rounds to zero"
        );
        assert_eq!(mitigate(0, 0), 0);
        assert_eq!(mitigate(-5, 10), 0);
        assert_eq!(mitigate(100, -50), 100, "negative resist clamps to zero");
        assert_eq!(mitigate_typed(100, DamageType::Magic, 1_000, 100), 50);
    }
}
