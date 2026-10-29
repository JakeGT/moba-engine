//! Renderer bone-count LOD for skinned heroes (R4b, ADR-0012 amendment).
//!
//! On top of the update-rate LOD (`MOBA_ANIM_LOD`), heroes far from the camera eye or off
//! screen draw reduced skins built at load (`moba_assets::SkinLod`): **LOD1** at most 2
//! influences and 75 % of the joints (spring-chain joints dropped first), **LOD2** 1 influence
//! and at most 4 joints. Off-screen heroes, which still cast shadows, use LOD2.
//!
//! The distances are per quality tier ([`BoneLod::for_tier`]); `MOBA_BONE_LOD=off` keeps every
//! hero on the full skin and `MOBA_BONE_LOD=<near>,<far>` overrides the distances. A pinned
//! capture always draws LOD0.

use moba_render::QualityTier;

/// Environment variable for the bone LOD (`off`, or `<near>,<far>` metres).
pub(crate) const BONE_LOD_ENV: &str = "MOBA_BONE_LOD";
/// Reduced skin levels below the full one.
pub(crate) const LOD_LEVELS: usize = 2;
/// LOD1 keeps this share of the joints (rounded up).
const LOD1_JOINT_SHARE: f32 = 0.75;
/// LOD2 keeps at most this many joints.
const LOD2_JOINTS: usize = 4;

/// The joint and influence budget of reduced level `level` (1 or 2) of a `joints`-joint
/// skin.
pub(crate) fn lod_budget(level: usize, joints: usize) -> (usize, usize) {
    if level <= 1 {
        let kept = (joints as f32 * LOD1_JOINT_SHARE).ceil() as usize;
        (kept.clamp(1, joints.max(1)), 2)
    } else {
        (LOD2_JOINTS.min(joints).max(1), 1)
    }
}

/// Bone-count LOD distances.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BoneLod {
    /// `None` when disabled; otherwise LOD1 beyond the first distance, LOD2 beyond the second.
    bands: Option<(f32, f32)>,
}

impl BoneLod {
    /// The tier's distances: `Low` 40/80 m, `Medium` 60/110 m, `High` 80/140 m, `Ultra`
    /// 110/180 m.
    pub(crate) fn for_tier(tier: QualityTier) -> Self {
        let bands = match tier {
            QualityTier::Low => (40.0, 80.0),
            QualityTier::Medium => (60.0, 110.0),
            QualityTier::Ultra => (110.0, 180.0),
            _ => (80.0, 140.0),
        };
        Self { bands: Some(bands) }
    }

    /// No reduction.
    pub(crate) const OFF: Self = Self { bands: None };

    /// Parses `off` or `<near>,<far>`; anything else keeps the tier default (and is logged).
    pub(crate) fn parse(spec: &str, tier: QualityTier) -> Self {
        let spec = spec.trim();
        if spec.eq_ignore_ascii_case("off") || spec == "0" {
            return Self::OFF;
        }
        let parsed = spec.split_once(',').and_then(|(near, far)| {
            let near: f32 = near.trim().parse().ok()?;
            let far: f32 = far.trim().parse().ok()?;
            (near.is_finite() && far.is_finite() && near >= 0.0 && far >= near)
                .then_some((near, far))
        });
        parsed.map_or_else(
            || {
                eprintln!(
                    "[anim] {BONE_LOD_ENV}={spec} is not `off` or `<near>,<far>`; using the tier default"
                );
                Self::for_tier(tier)
            },
            |bands| Self { bands: Some(bands) },
        )
    }

    /// From [`BONE_LOD_ENV`], else the tier default.
    pub(crate) fn from_env(tier: QualityTier) -> Self {
        std::env::var(BONE_LOD_ENV)
            .map_or_else(|_| Self::for_tier(tier), |spec| Self::parse(&spec, tier))
    }

    /// Whether reduced skins are used at all (they are only built when they are).
    pub(crate) fn enabled(self) -> bool {
        self.bands.is_some()
    }

    /// The skin level (0 full, 1, 2) for a hero `distance` metres from the eye.
    pub(crate) fn level(self, distance: f32, on_screen: bool) -> usize {
        let Some((near, far)) = self.bands else {
            return 0;
        };
        if !on_screen || !distance.is_finite() || distance > far {
            2
        } else {
            usize::from(distance > near)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BoneLod, lod_budget};
    use moba_render::QualityTier;

    #[test]
    fn levels_follow_distance_visibility_and_tier() {
        let high = BoneLod::for_tier(QualityTier::High);
        assert_eq!(high.level(46.0, true), 0);
        assert_eq!(high.level(100.0, true), 1);
        assert_eq!(high.level(200.0, true), 2);
        assert_eq!(high.level(10.0, false), 2, "off-screen");
        assert_eq!(BoneLod::for_tier(QualityTier::Low).level(60.0, true), 1);
        assert_eq!(BoneLod::for_tier(QualityTier::Ultra).level(100.0, true), 0);
        let off = BoneLod::parse("off", QualityTier::High);
        assert!(!off.enabled());
        assert_eq!(off.level(500.0, false), 0);
        assert_eq!(
            BoneLod::parse("10,20", QualityTier::High).level(15.0, true),
            1
        );
        assert_eq!(
            BoneLod::parse("nope", QualityTier::Low),
            BoneLod::for_tier(QualityTier::Low)
        );
    }

    #[test]
    fn budgets_shrink_the_sample_rig() {
        assert_eq!(lod_budget(1, 10), (8, 2));
        assert_eq!(lod_budget(2, 10), (4, 1));
        assert_eq!(lod_budget(2, 2), (2, 1));
        assert_eq!(lod_budget(1, 1), (1, 2));
    }
}
