//! Shared tonemap facts. The composite pass itself (HDR → bloom, exposure, `AgX`, grading,
//! screen effects) is part of the post chain in [`super::post`] (ADR-0015).

use ash::vk;

/// `AgX` look saturation shared by the composite and the UI's offscreen images (R7b).
pub(crate) use super::post::TONEMAP_SATURATION;

/// Whether stores to `format` are sRGB-encoded by the hardware.
pub(crate) fn is_srgb(format: vk::Format) -> bool {
    matches!(
        format,
        vk::Format::B8G8R8A8_SRGB
            | vk::Format::R8G8B8A8_SRGB
            | vk::Format::A8B8G8R8_SRGB_PACK32
            | vk::Format::B8G8R8_SRGB
            | vk::Format::R8G8B8_SRGB
    )
}

#[cfg(test)]
mod tests {
    use super::is_srgb;
    use ash::vk;

    #[test]
    fn srgb_formats_are_recognised() {
        assert!(is_srgb(vk::Format::B8G8R8A8_SRGB));
        assert!(!is_srgb(vk::Format::B8G8R8A8_UNORM));
    }
}
