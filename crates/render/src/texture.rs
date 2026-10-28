//! One-shot texture uploads (UI atlas, material textures) with full mip chains.

use ash::vk;
use gpu_allocator::MemoryLocation;
use moba_assets::{TextureData, TextureFormat};

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::resources::{CommandPool, GpuBuffer, GpuImage};

/// The Vulkan format of a [`TextureFormat`] (the KTX2 `vkFormat`).
pub(crate) fn vk_format(format: TextureFormat) -> vk::Format {
    match format {
        TextureFormat::R8Unorm => vk::Format::R8_UNORM,
        TextureFormat::Rg8Unorm => vk::Format::R8G8_UNORM,
        TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        TextureFormat::Rgba8Srgb => vk::Format::R8G8B8A8_SRGB,
        TextureFormat::Bc4Unorm => vk::Format::BC4_UNORM_BLOCK,
        TextureFormat::Bc5Unorm => vk::Format::BC5_UNORM_BLOCK,
        TextureFormat::Bc7Unorm => vk::Format::BC7_UNORM_BLOCK,
        TextureFormat::Bc7Srgb => vk::Format::BC7_SRGB_BLOCK,
        // `TextureFormat` is non-exhaustive across crates; a new format must be mapped here.
        _ => vk::Format::UNDEFINED,
    }
}

/// Whether `format` can be sampled with linear filtering from optimal-tiling images.
pub(crate) fn sampleable(device: &DeviceContext, format: vk::Format) -> bool {
    if format == vk::Format::UNDEFINED {
        return false;
    }
    // SAFETY: the physical device belongs to the instance the device context was made from.
    let properties = unsafe {
        device
            .instance
            .get_physical_device_format_properties(device.physical, format)
    };
    properties.optimal_tiling_features.contains(
        vk::FormatFeatureFlags::SAMPLED_IMAGE | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR,
    )
}

/// Uploads tightly-packed RGBA8 pixels into a sampled image (one level, `R8G8B8A8_UNORM`).
pub(crate) fn upload_rgba(
    device: &DeviceContext,
    name: &str,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> RenderResult<GpuImage> {
    let texture = TextureData::new(TextureFormat::Rgba8Unorm, width, height, &[pixels.to_vec()])
        .map_err(|_| RenderError::InvalidTexture)?;
    upload(device, name, &texture)
}

/// Uploads every mip level of `texture` into a sampled image in
/// `SHADER_READ_ONLY_OPTIMAL`. Load-time only: it waits for the graphics queue.
#[allow(clippy::too_many_lines)] // Upload/barrier/copy sequence is intentionally linear.
pub(crate) fn upload(
    device: &DeviceContext,
    name: &str,
    texture: &TextureData,
) -> RenderResult<GpuImage> {
    let format = vk_format(texture.format());
    if format == vk::Format::UNDEFINED {
        return Err(RenderError::InvalidTexture);
    }
    let levels = texture.level_count();
    let bytes = texture.data();
    let mut staging = GpuBuffer::new(
        device.device.clone(),
        device.allocator(),
        "texture-staging",
        bytes.len() as u64,
        vk::BufferUsageFlags::TRANSFER_SRC,
        MemoryLocation::CpuToGpu,
    )?;
    staging.write_bytes(0, bytes);
    let image = GpuImage::with_mips(
        device.device.clone(),
        device.allocator(),
        name,
        vk::Extent2D {
            width: texture.width(),
            height: texture.height(),
        },
        format,
        vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        vk::ImageAspectFlags::COLOR,
        levels,
    )?;
    let all_levels = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: levels,
        base_array_layer: 0,
        layer_count: 1,
    };

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
        .subresource_range(all_levels);
    let dependency =
        vk::DependencyInfo::default().image_memory_barriers(core::slice::from_ref(&to_transfer));
    device.cmd_pipeline_barrier2(command, &dependency);

    // One region per level; `moba_assets::TextureData` guarantees each level holds exactly
    // its size's bytes at `level_offset`, so no copy reads past the staging buffer.
    let mut regions = Vec::with_capacity(levels as usize);
    for level in 0..levels {
        let (width, height, _) = texture.level(level).ok_or(RenderError::InvalidTexture)?;
        let offset = texture
            .level_offset(level)
            .ok_or(RenderError::InvalidTexture)?;
        regions.push(
            vk::BufferImageCopy::default()
                .buffer_offset(offset as u64)
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: level,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                }),
        );
    }
    // SAFETY: the staging buffer holds every level and the image was created for transfer
    // with `levels` mip levels.
    unsafe {
        device.device.cmd_copy_buffer_to_image(
            command,
            staging.handle(),
            image.image(),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &regions,
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
        .subresource_range(all_levels);
    let dependency =
        vk::DependencyInfo::default().image_memory_barriers(core::slice::from_ref(&to_shader));
    device.cmd_pipeline_barrier2(command, &dependency);

    // SAFETY: the command buffer is recording and is ended once.
    unsafe { device.device.end_command_buffer(command) }?;
    let commands = [command];
    let submit = vk::SubmitInfo::default().command_buffers(&commands);
    // SAFETY: the command buffer is executable on the graphics queue; submits to the queue
    // happen only on the renderer's thread (see `mesh::upload_bytes`).
    unsafe {
        device
            .device
            .queue_submit(device.graphics_queue, &[submit], vk::Fence::null())
    }?;
    // SAFETY: waiting for our own load-time submission is valid.
    if let Err(error) = unsafe { device.device.queue_wait_idle(device.graphics_queue) } {
        // The copy may still be running: freeing the staging buffer, pool or image now could
        // be a use-after-free. Wait for the device instead, or leak them.
        // SAFETY: the device is live; waiting has no other preconditions.
        if unsafe { device.device.device_wait_idle() }.is_err() {
            core::mem::forget(pool);
            core::mem::forget(staging);
            core::mem::forget(image);
        }
        return Err(error.into());
    }
    drop(staging);
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::vk_format;
    use moba_assets::TextureFormat;

    #[test]
    fn vk_formats_match_the_ktx2_values() {
        for &format in TextureFormat::ALL {
            assert_eq!(
                u32::try_from(vk_format(format).as_raw()).expect("positive"),
                format.vk_format(),
                "{format}"
            );
        }
    }
}
