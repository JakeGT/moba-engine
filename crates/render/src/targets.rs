//! Physical images behind the render graph's transient slots, and target format choice.

use ash::vk;

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::graph::{ImageSize, PhysicalImage};
use crate::quality::QualityTier;
use crate::resources::GpuImage;

/// One `GpuImage` per transient slot, sized to the swapchain. Recreated only on resize.
pub(crate) struct TransientPool {
    images: Vec<GpuImage>,
}

impl TransientPool {
    pub(crate) fn new(
        device: &DeviceContext,
        slots: &[PhysicalImage],
        extent: vk::Extent2D,
    ) -> RenderResult<Self> {
        let mut images = Vec::with_capacity(slots.len());
        for (index, slot) in slots.iter().enumerate() {
            let size = match slot.desc.size {
                ImageSize::Swapchain => extent,
            };
            images.push(GpuImage::new(
                device.device.clone(),
                device.allocator(),
                &format!("graph-transient-{index}"),
                size,
                slot.desc.format,
                slot.usage,
                slot.desc.aspect,
            )?);
        }
        Ok(Self { images })
    }

    pub(crate) fn image(&self, slot: usize) -> &GpuImage {
        &self.images[slot]
    }
}

fn optimal_features(device: &DeviceContext, format: vk::Format) -> vk::FormatFeatureFlags {
    // SAFETY: the physical device belongs to the instance the device context was made from.
    let properties = unsafe {
        device
            .instance
            .get_physical_device_format_properties(device.physical, format)
    };
    properties.optimal_tiling_features
}

/// The tier's HDR colour format if it can be rendered to, blended and sampled; otherwise
/// `R16G16B16A16_SFLOAT`, whose support for all three is mandatory.
pub(crate) fn choose_hdr_format(device: &DeviceContext, tier: QualityTier) -> vk::Format {
    let required = vk::FormatFeatureFlags::COLOR_ATTACHMENT
        | vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND
        | vk::FormatFeatureFlags::SAMPLED_IMAGE;
    let preferred = tier.hdr_format();
    if optimal_features(device, preferred).contains(required) {
        preferred
    } else {
        vk::Format::R16G16B16A16_SFLOAT
    }
}

/// A depth format usable as an attachment and sampled image.
pub(crate) fn choose_depth_format(
    device: &DeviceContext,
) -> RenderResult<(vk::Format, vk::ImageAspectFlags)> {
    let candidates = [
        (vk::Format::D32_SFLOAT, vk::ImageAspectFlags::DEPTH),
        (vk::Format::D16_UNORM, vk::ImageAspectFlags::DEPTH),
    ];
    let required =
        vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT | vk::FormatFeatureFlags::SAMPLED_IMAGE;
    candidates
        .into_iter()
        .find(|(format, _)| optimal_features(device, *format).contains(required))
        .ok_or_else(|| {
            // D16_UNORM depth attachment + sampled support is mandatory.
            RenderError::NoSuitableDevice(String::from(
                "no sampled depth format (D32_SFLOAT/D16_UNORM)",
            ))
        })
}
