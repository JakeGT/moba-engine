//! Latency reduction through device extensions (ADR-0020 §6a): `VK_NV_low_latency2` (the
//! Vulkan face of Reflex) and `VK_AMD_anti_lag`. No SDK code is used, so there is no feature:
//! every build detects them. N1 drives them from the frame loop.

use crate::caps::DeviceCapabilities;

/// The latency-reduction extension a device offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LatencyMode {
    /// `VK_NV_low_latency2`.
    Reflex,
    /// `VK_AMD_anti_lag`.
    AntiLag,
    /// Neither extension is advertised.
    None,
}

impl LatencyMode {
    /// The name used in logs.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Reflex => "reflex",
            Self::AntiLag => "anti-lag",
            Self::None => "none",
        }
    }
}

/// The extension `capabilities` advertises, Reflex first.
pub(crate) fn detect(capabilities: &DeviceCapabilities) -> LatencyMode {
    if capabilities.has_extension("VK_NV_low_latency2") {
        LatencyMode::Reflex
    } else if capabilities.has_extension("VK_AMD_anti_lag") {
        LatencyMode::AntiLag
    } else {
        LatencyMode::None
    }
}
