//! One-shot RGBA texture upload.

use ash::vk;
use gpu_allocator::MemoryLocation;

use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::resources::{CommandPool, GpuBuffer, GpuImage};

/// Uploads tightly-packed RGBA8 pixels into a sampled image.
#[allow(clippy::too_many_lines)] // Upload/barrier/copy sequence is intentionally linear.
pub(crate) fn upload_rgba(
    device: &DeviceContext,
    name: &str,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> RenderResult<GpuImage> {
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or(crate::error::RenderError::InvalidTexture)?;
    if pixels.len() != expected {
        return Err(crate::error::RenderError::InvalidTexture);
    }
    let mut staging = GpuBuffer::new(
        device.device.clone(),
        device.allocator(),
        "ui-atlas-staging",
        pixels.len() as u64,
        vk::BufferUsageFlags::TRANSFER_SRC,
        MemoryLocation::CpuToGpu,
    )?;
    staging.write_bytes(0, pixels);
    let image = GpuImage::new(
        device.device.clone(),
        device.allocator(),
        name,
        vk::Extent2D { width, height },
        vk::Format::R8G8B8A8_UNORM,
        vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        vk::ImageAspectFlags::COLOR,
    )?;

    let pool = CommandPool::new(device.device.clone(), device.graphics_family)?;
    let command = pool.allocate_one()?;
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: the command buffer is allocated and not recording.
    unsafe { device.device.begin_command_buffer(command, &begin) }?;

    let to_transfer = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::TOP_OF_PIPE)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image.image())
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    let dependency =
        vk::DependencyInfo::default().image_memory_barriers(core::slice::from_ref(&to_transfer));
    device.cmd_pipeline_barrier2(command, &dependency);

    let region = vk::BufferImageCopy::default()
        .buffer_offset(0)
        .buffer_row_length(0)
        .buffer_image_height(0)
        .image_subresource(vk::ImageSubresourceLayers {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
        })
        .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
        .image_extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        });
    // SAFETY: the staging buffer is large enough and the image was created for transfer.
    unsafe {
        device.device.cmd_copy_buffer_to_image(
            command,
            staging.handle(),
            image.image(),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            core::slice::from_ref(&region),
        );
    }

    let to_shader = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
        .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image.image())
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    let dependency =
        vk::DependencyInfo::default().image_memory_barriers(core::slice::from_ref(&to_shader));
    device.cmd_pipeline_barrier2(command, &dependency);

    // SAFETY: the command buffer is recording and is ended once.
    unsafe { device.device.end_command_buffer(command) }?;
    let commands = [command];
    let submit = vk::SubmitInfo::default().command_buffers(&commands);
    // SAFETY: the command buffer is executable on the graphics queue.
    unsafe {
        device
            .device
            .queue_submit(device.graphics_queue, &[submit], vk::Fence::null())
    }?;
    // SAFETY: waiting for our own init submission is valid.
    unsafe { device.device.queue_wait_idle(device.graphics_queue) }?;
    drop(staging);
    Ok(image)
}
