//! Rendering quality tiers (AAA graphics roadmap §3: every feature is tiered).

use ash::vk;

/// Rendering quality tier. Every renderer feature keys off this; a feature without a `Low`
/// path or an off switch does not merge.
///
/// Today it selects only the HDR colour format: `Low` uses the 32-bit packed
/// `B10G11R11_UFLOAT_PACK32` (half the bandwidth of the 64-bit target, no alpha), the others
/// `R16G16B16A16_SFLOAT`. Later phases (shadows, post-processing, VFX budgets) add to it.
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
        assert_eq!(
            QualityTier::Low.hdr_format(),
            vk::Format::B10G11R11_UFLOAT_PACK32
        );
        assert_eq!(
            QualityTier::High.hdr_format(),
            vk::Format::R16G16B16A16_SFLOAT
        );
    }
}
