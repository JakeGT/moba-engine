//! Physical images behind the render graph's transient slots, and target format choice.

use ash::vk;

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::graph::{ImageSize, PhysicalImage};
use crate::quality::QualityTier;
use crate::resources::{CommandPool, GpuImage};

/// The two window-dependent extents transient slots are sized from (ADR-0015).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TargetExtents {
    /// The swapchain image size.
    pub(crate) swapchain: vk::Extent2D,
    /// The render allocation: the swapchain size times the maximum render scale. Dynamic
    /// resolution renders a sub-rectangle of it.
    pub(crate) render: vk::Extent2D,
}

impl TargetExtents {
    /// The size of a slot of `size`.
    pub(crate) fn of(self, size: ImageSize) -> vk::Extent2D {
        match size {
            ImageSize::Swapchain => self.swapchain,
            ImageSize::Render { shift } => shifted(self.render, shift),
            ImageSize::Fixed { width, height } => vk::Extent2D { width, height },
        }
    }
}

/// `extent >> shift` per side, at least 1.
pub(crate) fn shifted(extent: vk::Extent2D, shift: u8) -> vk::Extent2D {
    let shift = u32::from(shift.min(31));
    vk::Extent2D {
        width: (extent.width >> shift).max(1),
        height: (extent.height >> shift).max(1),
    }
}

/// One `GpuImage` per transient slot, sized to the swapchain or the render allocation.
/// Recreated only on resize.
pub(crate) struct TransientPool {
    images: Vec<GpuImage>,
}

impl TransientPool {
    pub(crate) fn new(
        device: &DeviceContext,
        slots: &[PhysicalImage],
        extents: TargetExtents,
    ) -> RenderResult<Self> {
        let mut images = Vec::with_capacity(slots.len());
        for (index, slot) in slots.iter().enumerate() {
            images.push(create_slot(device, index, slot, extents)?);
        }
        Ok(Self { images })
    }

    /// Reallocates the window-sized slots for `extents` (a resize) and keeps the fixed-size
    /// ones (the shadow map: up to 64 MiB that a resize must not churn). Every new image is
    /// created before any is replaced, so a failed allocation changes nothing.
    pub(crate) fn resize(
        &mut self,
        device: &DeviceContext,
        slots: &[PhysicalImage],
        extents: TargetExtents,
    ) -> RenderResult<()> {
        let mut fresh = Vec::with_capacity(slots.len());
        for (index, slot) in slots.iter().enumerate() {
            fresh.push(match slot.desc.size {
                ImageSize::Swapchain | ImageSize::Render { .. } => {
                    Some(create_slot(device, index, slot, extents)?)
                }
                ImageSize::Fixed { .. } => None,
            });
        }
        for (image, fresh) in self.images.iter_mut().zip(fresh) {
            if let Some(fresh) = fresh {
                *image = fresh;
            }
        }
        Ok(())
    }

    pub(crate) fn image(&self, slot: usize) -> &GpuImage {
        &self.images[slot]
    }
}

fn create_slot(
    device: &DeviceContext,
    index: usize,
    slot: &PhysicalImage,
    extents: TargetExtents,
) -> RenderResult<GpuImage> {
    GpuImage::with_samples(
        device.device.clone(),
        device.allocator(),
        &format!("graph-transient-{index}"),
        extents.of(slot.desc.size),
        slot.desc.format,
        slot.usage,
        slot.desc.aspect,
        slot.desc.samples,
    )
}

/// A fixed offscreen image (R7b): an HDR colour target that its graph pass renders into and
/// the UI samples, plus a depth buffer for that pass.
///
/// Created once with the renderer at a fixed size (never on resize or per frame). Creation
/// clears the colour image to black and leaves it in `SHADER_READ_ONLY_OPTIMAL`, the state
/// the frame graph imports it in (`ImportState::persistent`), so the UI can sample it before
/// its first render.
pub(crate) struct OffscreenTarget {
    color: GpuImage,
    depth: GpuImage,
    extent: vk::Extent2D,
}

impl OffscreenTarget {
    /// Creates the images and runs the initial clear and transition (a queue wait: creation
    /// only).
    pub(crate) fn new(
        device: &DeviceContext,
        name: &str,
        extent: vk::Extent2D,
        color_format: vk::Format,
        (depth_format, depth_aspect): (vk::Format, vk::ImageAspectFlags),
    ) -> RenderResult<Self> {
        let extent = vk::Extent2D {
            width: extent.width.max(1),
            height: extent.height.max(1),
        };
        let color = GpuImage::new(
            device.device.clone(),
            device.allocator(),
            name,
            extent,
            color_format,
            vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_DST,
            vk::ImageAspectFlags::COLOR,
        )?;
        let depth = GpuImage::new(
            device.device.clone(),
            device.allocator(),
            &format!("{name}-depth"),
            extent,
            depth_format,
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
            depth_aspect,
        )?;
        if let Err((error, in_use)) = initialise_sampled(device, color.image()) {
            if in_use {
                // The clear may still be running on the GPU: leak the images rather than
                // free memory it may still write.
                core::mem::forget(color);
                core::mem::forget(depth);
            }
            return Err(error);
        }
        Ok(Self {
            color,
            depth,
            extent,
        })
    }

    pub(crate) fn color(&self) -> &GpuImage {
        &self.color
    }

    pub(crate) fn depth(&self) -> &GpuImage {
        &self.depth
    }

    pub(crate) fn extent(&self) -> vk::Extent2D {
        self.extent
    }
}

/// Clears `image` (one colour level) to opaque black and transitions it to
/// `SHADER_READ_ONLY_OPTIMAL`, waiting for the graphics queue. On error, the flag says
/// whether the submitted clear may still be running (the caller must then not free `image`).
pub(crate) fn initialise_sampled(
    device: &DeviceContext,
    image: vk::Image,
) -> Result<(), (RenderError, bool)> {
    let idle = |error: vk::Result| (RenderError::from(error), false);
    let range = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    };
    let pool = CommandPool::new(device.device.clone(), device.graphics_family)
        .map_err(|error| (error, false))?;
    let command = pool.allocate_one().map_err(|error| (error, false))?;
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: the command buffer is allocated and not recording.
    unsafe { device.device.begin_command_buffer(command, &begin) }.map_err(idle)?;
    let to_transfer = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::NONE)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(vk::PipelineStageFlags2::CLEAR)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(range);
    device.cmd_pipeline_barrier2(
        command,
        &vk::DependencyInfo::default().image_memory_barriers(core::slice::from_ref(&to_transfer)),
    );
    let black = vk::ClearColorValue {
        float32: [0.0, 0.0, 0.0, 1.0],
    };
    // SAFETY: the image was created with TRANSFER_DST usage and is in TRANSFER_DST_OPTIMAL.
    unsafe {
        device.device.cmd_clear_color_image(
            command,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &black,
            core::slice::from_ref(&range),
        );
    }
    let to_sampled = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::CLEAR)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
        .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(range);
    device.cmd_pipeline_barrier2(
        command,
        &vk::DependencyInfo::default().image_memory_barriers(core::slice::from_ref(&to_sampled)),
    );
    // SAFETY: the command buffer is recording and is ended once.
    unsafe { device.device.end_command_buffer(command) }.map_err(idle)?;
    let commands = [command];
    let submit = vk::SubmitInfo::default().command_buffers(&commands);
    // SAFETY: the command buffer is executable on the graphics queue; submits happen only on
    // the renderer's thread.
    unsafe {
        device
            .device
            .queue_submit(device.graphics_queue, &[submit], vk::Fence::null())
    }
    .map_err(idle)?;
    // SAFETY: waiting for our own creation-time submission is valid.
    if let Err(error) = unsafe { device.device.queue_wait_idle(device.graphics_queue) } {
        // The clear may still be running: freeing the pool or the image now could be a
        // use-after-free.
        // SAFETY: the device is live; waiting has no other preconditions.
        let in_use = unsafe { device.device.device_wait_idle() }.is_err();
        if in_use {
            core::mem::forget(pool);
        }
        return Err((error.into(), in_use));
    }
    Ok(())
}

pub(crate) fn optimal_features(
    device: &DeviceContext,
    format: vk::Format,
) -> vk::FormatFeatureFlags {
    // SAFETY: the physical device belongs to the instance the device context was made from.
    let properties = unsafe {
        device
            .instance
            .get_physical_device_format_properties(device.physical, format)
    };
    properties.optimal_tiling_features
}

/// Usage of the multisampled scene colour (R6 MSAA 4×): only ever an attachment, resolved
/// at the end of the scene pass and never stored, so tilers may keep it on chip.
pub(crate) const MSAA_COLOR_USAGE: vk::ImageUsageFlags = vk::ImageUsageFlags::from_raw(
    vk::ImageUsageFlags::COLOR_ATTACHMENT.as_raw()
        | vk::ImageUsageFlags::TRANSIENT_ATTACHMENT.as_raw(),
);

/// Whether 4× multisampled images of the actual scene formats can be created: `hdr` with
/// [`MSAA_COLOR_USAGE`] and `depth` as a depth attachment (the device-wide
/// `framebuffer*SampleCounts` limits do not cover every format).
pub(crate) fn supports_msaa4(device: &DeviceContext, hdr: vk::Format, depth: vk::Format) -> bool {
    let samples = |format, usage| {
        // SAFETY: the physical device belongs to the instance the device context was made
        // from; the call only reads properties.
        unsafe {
            device.instance.get_physical_device_image_format_properties(
                device.physical,
                format,
                vk::ImageType::TYPE_2D,
                vk::ImageTiling::OPTIMAL,
                usage,
                vk::ImageCreateFlags::empty(),
            )
        }
        .is_ok_and(|properties| {
            properties
                .sample_counts
                .contains(vk::SampleCountFlags::TYPE_4)
        })
    };
    samples(hdr, MSAA_COLOR_USAGE) && samples(depth, vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
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

/// The bloom mip format (R6): the packed `B10G11R11_UFLOAT_PACK32` (half the bandwidth of
/// the 64-bit format, no alpha, positive values only) where it can be rendered to, blended
/// into and sampled with linear filtering; otherwise `R16G16B16A16_SFLOAT`.
pub(crate) fn choose_bloom_format(device: &DeviceContext) -> vk::Format {
    let required = vk::FormatFeatureFlags::COLOR_ATTACHMENT
        | vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND
        | vk::FormatFeatureFlags::SAMPLED_IMAGE
        | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR;
    let packed = vk::Format::B10G11R11_UFLOAT_PACK32;
    if optimal_features(device, packed).contains(required) {
        packed
    } else {
        vk::Format::R16G16B16A16_SFLOAT
    }
}

/// The adapted-exposure format (R6): `R32_SFLOAT` where it can be rendered to, blended into
/// and sampled, so a slow adaptation at a high frame rate never stalls on half-float
/// precision; otherwise `R16_SFLOAT` (blendable everywhere), which
/// [`crate::post::adaptation_step`] then feeds in steps no smaller than
/// [`crate::post::MIN_HALF_ADAPTATION_STEP`].
pub(crate) fn choose_exposure_format(device: &DeviceContext) -> vk::Format {
    let required = vk::FormatFeatureFlags::COLOR_ATTACHMENT
        | vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND
        | vk::FormatFeatureFlags::SAMPLED_IMAGE;
    let full = vk::Format::R32_SFLOAT;
    if optimal_features(device, full).contains(required) {
        full
    } else {
        vk::Format::R16_SFLOAT
    }
}

/// `VkFormatProperties3::optimalTilingFeatures` (Vulkan 1.3 core), which also reports depth
/// comparison support.
fn optimal_features_2(device: &DeviceContext, format: vk::Format) -> vk::FormatFeatureFlags2 {
    let mut properties_3 = vk::FormatProperties3::default();
    let mut properties = vk::FormatProperties2::default().push_next(&mut properties_3);
    // SAFETY: the physical device belongs to the instance the device context was made from,
    // and the output chain is valid for the call.
    unsafe {
        device.instance.get_physical_device_format_properties2(
            device.physical,
            format,
            &mut properties,
        );
    }
    properties_3.optimal_tiling_features
}

/// The shadow-map format and whether it filters linearly (hardware 2×2 PCF), or `None` when
/// the device cannot do depth-compare shadow maps at all (shadows then turn off, logged).
///
/// The first of `D32_SFLOAT`, `D16_UNORM` that is an attachment, sampled, usable with a
/// comparison sampler (`SAMPLED_IMAGE_DEPTH_COMPARISON`) and linearly filterable wins; else
/// the first without linear filtering (PCF taps are then nearest). A portability-subset
/// device without `mutableComparisonSamplers` gets `None`.
pub(crate) fn choose_shadow_format(device: &DeviceContext) -> Option<(vk::Format, bool)> {
    use vk::FormatFeatureFlags2 as F;
    if !device.comparison_samplers {
        return None;
    }
    let base = F::DEPTH_STENCIL_ATTACHMENT | F::SAMPLED_IMAGE | F::SAMPLED_IMAGE_DEPTH_COMPARISON;
    let candidates = [vk::Format::D32_SFLOAT, vk::Format::D16_UNORM];
    let supports = |format, wanted| optimal_features_2(device, format).contains(wanted);
    candidates
        .iter()
        .copied()
        .find(|&format| supports(format, base | F::SAMPLED_IMAGE_FILTER_LINEAR))
        .map(|format| (format, true))
        .or_else(|| {
            candidates
                .iter()
                .copied()
                .find(|&format| supports(format, base))
                .map(|format| (format, false))
        })
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
