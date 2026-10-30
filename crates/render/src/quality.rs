//! Rendering quality tiers (AAA graphics roadmap §3: every feature is tiered).

use ash::vk;

use crate::vfx::VfxBudget;

/// Rendering quality tier. Every renderer feature keys off this; a feature without a `Low`
/// path or an off switch does not merge.
///
/// It selects the HDR colour format (`Low` uses the 32-bit packed `B10G11R11_UFLOAT_PACK32`,
/// half the bandwidth of the 64-bit target, no alpha; the others `R16G16B16A16_SFLOAT`), the
/// material sampler's anisotropy, the shadow filter, shadow-map size and specular IBL
/// ([`LightingConfig::for_tier`](crate::LightingConfig::for_tier), ADR-0011) and the VFX
/// budgets ([`Self::vfx_budget`], ADR-0013). Later phases (post-processing) add to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum QualityTier {
    /// Min-spec (Intel UHD 620 / Apple M1 class) at 1080p/60.
    Low,
    /// GTX 1060 / RX 580 class.
    Medium,
    /// RTX 3060 class at 1440p/144. The default.
    #[default]
    High,
    /// RTX 4070+ class at 4K.
    Ultra,
}

impl QualityTier {
    /// Every tier, lowest first. A slice rather than an array so a later tier is not a
    /// breaking change for this `#[non_exhaustive]` enum.
    pub const ALL: &'static [Self] = &[Self::Low, Self::Medium, Self::High, Self::Ultra];

    /// Lower-case name, as accepted by [`Self::from_name`] (`"low"`, `"medium"`, ...).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Ultra => "ultra",
        }
    }

    /// Parses a tier name, ignoring ASCII case and surrounding whitespace.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .iter()
            .copied()
            .find(|tier| tier.name().eq_ignore_ascii_case(name))
    }

    /// Maximum sampler anisotropy for material textures (`None` = off), clamped to the
    /// device limit; `Low` keeps plain trilinear filtering for min-spec bandwidth.
    pub(crate) const fn max_anisotropy(self) -> Option<f32> {
        match self {
            Self::Low => None,
            Self::Medium => Some(4.0),
            Self::High => Some(8.0),
            Self::Ultra => Some(16.0),
        }
    }

    /// GPU particle budgets (ADR-0013). `Low` has no soft particles, no OIT targets and no
    /// mesh particles; every tier keeps enemy telegraphs at least 35 % opaque.
    #[must_use]
    pub const fn vfx_budget(self) -> VfxBudget {
        let (max_particles, max_spawns_per_frame, max_overdraw, max_distance) = match self {
            Self::Low => (4_096, 512, 1.5, 90.0),
            Self::Medium => (12_288, 1_024, 3.0, 130.0),
            Self::High => (24_576, 2_048, 5.0, 170.0),
            Self::Ultra => (49_152, 4_096, 8.0, 240.0),
        };
        let full = !matches!(self, Self::Low);
        VfxBudget {
            max_particles,
            max_spawns_per_frame,
            max_overdraw,
            soft_particles: full,
            oit: full,
            mesh_particles: full,
            max_distance,
            telegraph_floor: 0.35,
        }
    }

    /// The tier's asset and memory budgets (R8, ADR-0018 §4; `moba_assets::TIER_BUDGETS`):
    /// VRAM, texture bytes, per-model triangles/joints/texture bytes and the mesh LOD
    /// tolerance.
    #[must_use]
    pub fn budget(self) -> &'static moba_assets::TierBudget {
        let index = match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::Ultra => 3,
        };
        &moba_assets::TIER_BUDGETS[index]
    }

    /// Preferred HDR scene colour format for this tier.
    pub(crate) const fn hdr_format(self) -> vk::Format {
        match self {
            Self::Low => vk::Format::B10G11R11_UFLOAT_PACK32,
            Self::Medium | Self::High | Self::Ultra => vk::Format::R16G16B16A16_SFLOAT,
        }
    }
}

impl core::fmt::Display for QualityTier {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::QualityTier;
    use ash::vk;

    #[test]
    fn tiers_round_trip_by_name_and_pick_hdr_formats() {
        for &tier in QualityTier::ALL {
            assert_eq!(QualityTier::from_name(tier.name()), Some(tier));
        }
        assert_eq!(QualityTier::from_name(" ULTRA "), Some(QualityTier::Ultra));
        assert_eq!(QualityTier::from_name("epic"), None);
        assert_eq!(QualityTier::default(), QualityTier::High);
        for &tier in QualityTier::ALL {
            assert_eq!(
                tier.budget().name,
                tier.name(),
                "budgets are indexed by tier"
            );
        }
        assert_eq!(
            QualityTier::Low.hdr_format(),
            vk::Format::B10G11R11_UFLOAT_PACK32
        );
        assert_eq!(
            QualityTier::High.hdr_format(),
            vk::Format::R16G16B16A16_SFLOAT
        );
    }

    #[test]
    fn vfx_budgets_grow_with_the_tier_and_low_drops_the_extra_targets() {
        let low = QualityTier::Low.vfx_budget();
        assert!(!low.oit && !low.soft_particles && !low.mesh_particles);
        let mut previous = 0;
        for &tier in QualityTier::ALL {
            let budget = tier.vfx_budget();
            assert!(budget.max_particles > previous);
            assert!(budget.max_spawns_per_frame <= budget.max_particles / 8);
            assert_eq!(budget.max_particles % 64, 0, "whole compute workgroups");
            assert!(budget.telegraph_floor > 0.0);
            previous = budget.max_particles;
        }
        assert!(QualityTier::High.vfx_budget().oit);
    }
}
