//! Latency reduction through device extensions (ADR-0020 §6a): `VK_NV_low_latency2` (the
//! Vulkan face of Reflex) and `VK_AMD_anti_lag`. No SDK code is used, so there is no feature:
//! every build detects them. N1 drives them from the frame loop.

/// The latency-reduction extension enabled on the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LatencyMode {
    /// `VK_NV_low_latency2`.
    Reflex,
    /// `VK_AMD_anti_lag`.
    AntiLag,
    /// Neither extension is enabled on the device.
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

/// The extension the device was created with, Reflex first. This reads the enabled set, not
/// what the device advertises: the tiered path enables `VK_NV_low_latency2` only with
/// `presentId`, and the compat path enables neither extension (ADR-0020 §1e).
pub(crate) fn detect(enabled: &[String]) -> LatencyMode {
    let has = |name: &str| enabled.iter().any(|enabled| enabled == name);
    if has("VK_NV_low_latency2") {
        LatencyMode::Reflex
    } else if has("VK_AMD_anti_lag") {
        LatencyMode::AntiLag
    } else {
        LatencyMode::None
    }
}
