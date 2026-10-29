//! Physical images behind the render graph's transient slots, and target format choice.

use ash::vk;

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::graph::{ImageSize, PhysicalImage};
use crate::quality::QualityTier;
use crate::resources::{CommandPool, GpuImage};

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
fn initialise_sampled(device: &DeviceContext, image: vk::Image) -> Result<(), (RenderError, bool)> {
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
