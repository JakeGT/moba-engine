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
        TextureFormat::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
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

/// Uploads every mip level (and, for a cube map, every face) of `texture` into a sampled
/// image in `SHADER_READ_ONLY_OPTIMAL`. Load-time only: it waits for the graphics queue.
pub(crate) fn upload(
    device: &DeviceContext,
    name: &str,
    texture: &TextureData,
) -> RenderResult<GpuImage> {
    let format = vk_format(texture.format());
    if format == vk::Format::UNDEFINED {
        return Err(RenderError::InvalidTexture);
    }
    let mut levels = [(0usize, 0usize); moba_assets::MAX_MIP_LEVELS as usize];
    let count = texture.level_count() as usize;
    for (level, slot) in levels.iter_mut().enumerate().take(count) {
        let level = u32::try_from(level).map_err(|_| RenderError::InvalidTexture)?;
        let (_, _, bytes) = texture.level(level).ok_or(RenderError::InvalidTexture)?;
        let offset = texture
            .level_offset(level)
            .ok_or(RenderError::InvalidTexture)?;
        *slot = (offset, bytes.len());
    }
    upload_raw(
        device,
        name,
        &RawImage {
            format,
            extent: vk::Extent2D {
                width: texture.width(),
                height: texture.height(),
            },
            layers: if texture.is_cube() {
                RawLayers::Cube
            } else {
                RawLayers::Plain
            },
            levels: &levels[..count],
            bytes: texture.data(),
        },
    )
}

/// Uploads `layers` (2D textures of one format, size and level count) as one `2D_ARRAY`
/// image, layer `i` from `layers[i]`, decoding block-compressed layers on the CPU unless
/// `compressed` (and the device samples the format). Load-time only.
///
/// # Errors
/// [`RenderError::InvalidTexture`] for an empty list, a cube, layers that disagree on
/// format, size or levels, or a format the device cannot sample; upload errors.
pub(crate) fn upload_array(
    device: &DeviceContext,
    name: &str,
    layers: &[&TextureData],
    compressed: bool,
) -> RenderResult<GpuImage> {
    let first = layers.first().ok_or(RenderError::InvalidTexture)?;
    if layers.iter().any(|layer| {
        layer.is_cube()
            || layer.format() != first.format()
            || layer.width() != first.width()
            || layer.height() != first.height()
            || layer.level_count() != first.level_count()
    }) {
        return Err(RenderError::InvalidTexture);
    }
    let native = !first.format().is_block_compressed()
        || (compressed && sampleable(device, vk_format(first.format())));
    let decoded: Vec<TextureData> = if native {
        Vec::new()
    } else {
        layers
            .iter()
            .map(|layer| layer.decode_to_uncompressed())
            .collect::<Result<_, _>>()
            .map_err(|_| RenderError::InvalidTexture)?
    };
    let sources: Vec<&TextureData> = if native {
        layers.to_vec()
    } else {
        decoded.iter().collect()
    };
    let format = vk_format(sources[0].format());
    if !sampleable(device, format) {
        return Err(RenderError::InvalidTexture);
    }
    let count = sources[0].level_count() as usize;
    let mut levels = [(0usize, 0usize); moba_assets::MAX_MIP_LEVELS as usize];
    let mut bytes = Vec::new();
    for (level, slot) in levels.iter_mut().enumerate().take(count) {
        let level = u32::try_from(level).map_err(|_| RenderError::InvalidTexture)?;
        let start = bytes.len();
        for source in &sources {
            let (_, _, texels) = source.level(level).ok_or(RenderError::InvalidTexture)?;
            bytes.extend_from_slice(texels);
        }
        *slot = (start, bytes.len() - start);
    }
    upload_raw(
        device,
        name,
        &RawImage {
            format,
            extent: vk::Extent2D {
                width: sources[0].width(),
                height: sources[0].height(),
            },
            layers: RawLayers::Array(
                u32::try_from(sources.len()).map_err(|_| RenderError::InvalidTexture)?,
            ),
            levels: &levels[..count],
            bytes: &bytes,
        },
    )
}

/// How the layers of a [`RawImage`] are arranged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawLayers {
    /// One layer (a `2D` view).
    Plain,
    /// Six cube faces (a `CUBE` view).
    Cube,
    /// `n` layers (a `2D_ARRAY` view, even for one layer).
    Array(u32),
    /// A 3D image of `n` slices (a square `extent` × `n` volume, one level; R6's grading LUTs).
    Volume(u32),
}

impl RawLayers {
    const fn count(self) -> u32 {
        match self {
            Self::Plain | Self::Volume(_) => 1,
            Self::Cube => 6,
            Self::Array(layers) => layers,
        }
    }

    /// Slices of a 3D image; 1 otherwise.
    const fn depth(self) -> u32 {
        match self {
            Self::Volume(depth) => depth,
            _ => 1,
        }
    }
}

/// Texel data for [`upload_raw`]: every level's `(offset, length)` in `bytes`, largest level
/// first; a cube level holds its six faces consecutively (+X, −X, +Y, −Y, +Z, −Z), an array
/// level its layers in order.
pub(crate) struct RawImage<'a> {
    pub(crate) format: vk::Format,
    pub(crate) extent: vk::Extent2D,
    pub(crate) layers: RawLayers,
    pub(crate) levels: &'a [(usize, usize)],
    pub(crate) bytes: &'a [u8],
}

/// Uploads `raw` into a sampled image in `SHADER_READ_ONLY_OPTIMAL`. The caller guarantees
/// each level range lies inside `bytes` and holds exactly that level's texels (every face
/// for a cube); [`upload`] gets that from `TextureData`'s invariants, and the renderer's own
/// images are built with exact sizes. Load-time only: it waits for the graphics queue.
#[allow(clippy::too_many_lines)] // Upload/barrier/copy sequence is intentionally linear.
pub(crate) fn upload_raw(
    device: &DeviceContext,
    name: &str,
    raw: &RawImage<'_>,
) -> RenderResult<GpuImage> {
    let levels = u32::try_from(raw.levels.len()).map_err(|_| RenderError::InvalidTexture)?;
    if levels == 0
        || raw.bytes.is_empty()
        || raw.levels.iter().any(|&(offset, len)| {
            offset
                .checked_add(len)
                .is_none_or(|end| end > raw.bytes.len())
        })
    {
        return Err(RenderError::InvalidTexture);
    }
    let layers = raw.layers.count();
    if layers == 0 {
        return Err(RenderError::InvalidTexture);
    }
    let mut staging = GpuBuffer::new(
        device.device.clone(),
        device.allocator(),
        "texture-staging",
        raw.bytes.len() as u64,
        vk::BufferUsageFlags::TRANSFER_SRC,
        MemoryLocation::CpuToGpu,
    )?;
    staging.write_bytes(0, raw.bytes);
    let usage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST;
    let image = match raw.layers {
        RawLayers::Volume(_) => GpuImage::volume(
            device.device.clone(),
            device.allocator(),
            name,
            raw.extent.width,
            raw.format,
            usage,
        )?,
        RawLayers::Cube => GpuImage::cube_with_mips(
            device.device.clone(),
            device.allocator(),
            name,
            raw.extent.width,
            raw.format,
            usage,
            levels,
        )?,
        RawLayers::Plain => GpuImage::with_mips(
            device.device.clone(),
            device.allocator(),
            name,
            raw.extent,
            raw.format,
            usage,
            vk::ImageAspectFlags::COLOR,
            levels,
        )?,
        RawLayers::Array(count) => GpuImage::array_with_mips(
            device.device.clone(),
            device.allocator(),
            name,
            raw.extent,
            raw.format,
            usage,
            count,
            levels,
        )?,
    };
    let all_levels = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: levels,
        base_array_layer: 0,
        layer_count: layers,
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

    // One region per level covering every layer: with zero row length and image height the
    // faces are read tightly packed, one after another, from the level's offset.
    let mut regions = Vec::with_capacity(raw.levels.len());
    for (level, &(offset, _)) in (0u32..).zip(raw.levels) {
        let width = (raw.extent.width >> level).max(1);
        let height = (raw.extent.height >> level).max(1);
        regions.push(
            vk::BufferImageCopy::default()
                .buffer_offset(offset as u64)
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: level,
                    base_array_layer: 0,
                    layer_count: layers,
                })
                .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: raw.layers.depth().max(1),
                }),
        );
    }
    // SAFETY: the staging buffer holds every level (checked above; exact sizes are the
    // caller's contract) and the image was created for transfer with `levels` mip levels and
    // `layers` layers.
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
    if let Err(failure) = submit_and_wait(device, command) {
        if !failure.idle {
            core::mem::forget(pool);
            core::mem::forget(staging);
            core::mem::forget(image);
        }
        return Err(failure.error);
    }
    drop(staging);
    Ok(image)
}

/// A 1×1 depth image cleared to 1.0 (fully lit) in `SHADER_READ_ONLY_OPTIMAL`: the shadow
/// map bound when shadows are off. Load-time only.
pub(crate) fn default_depth(device: &DeviceContext, format: vk::Format) -> RenderResult<GpuImage> {
    let image = GpuImage::new(
        device.device.clone(),
        device.allocator(),
        "default-shadow-map",
        vk::Extent2D {
            width: 1,
            height: 1,
        },
        format,
        vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        vk::ImageAspectFlags::DEPTH,
    )?;
    let range = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::DEPTH,
        base_mip_level: 0,
        level_count: 1,
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
        .dst_stage_mask(vk::PipelineStageFlags2::CLEAR)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image.image())
        .subresource_range(range);
    device.cmd_pipeline_barrier2(
        command,
        &vk::DependencyInfo::default().image_memory_barriers(core::slice::from_ref(&to_transfer)),
    );
    let clear = vk::ClearDepthStencilValue {
        depth: 1.0,
        stencil: 0,
    };
    // SAFETY: the image was created with TRANSFER_DST and is in TRANSFER_DST_OPTIMAL; the
    // range names its only level and layer of the depth aspect.
    unsafe {
        device.device.cmd_clear_depth_stencil_image(
            command,
            image.image(),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &clear,
            core::slice::from_ref(&range),
        );
    }
    let to_shader = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::CLEAR)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
        .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image.image())
        .subresource_range(range);
    device.cmd_pipeline_barrier2(
        command,
        &vk::DependencyInfo::default().image_memory_barriers(core::slice::from_ref(&to_shader)),
    );
    // SAFETY: the command buffer is recording and is ended once.
    unsafe { device.device.end_command_buffer(command) }?;
    if let Err(failure) = submit_and_wait(device, command) {
        if !failure.idle {
            core::mem::forget(pool);
            core::mem::forget(image);
        }
        return Err(failure.error);
    }
    Ok(image)
}

/// A failed load-time submission: the error, and whether the device is known idle (if not,
/// the caller must leak every resource the commands used rather than free it).
struct WaitFailure {
    error: RenderError,
    idle: bool,
}

/// Submits a one-time command buffer and waits for the graphics queue to idle.
fn submit_and_wait(device: &DeviceContext, command: vk::CommandBuffer) -> Result<(), WaitFailure> {
    let commands = [command];
    let submit = vk::SubmitInfo::default().command_buffers(&commands);
    // SAFETY: the command buffer is executable on the graphics queue; submits to the queue
    // happen only on the renderer's thread (see `mesh::upload_bytes`).
    if let Err(error) = unsafe {
        device
            .device
            .queue_submit(device.graphics_queue, &[submit], vk::Fence::null())
    } {
        // Nothing was submitted, so nothing is in use.
        return Err(WaitFailure {
            error: error.into(),
            idle: true,
        });
    }
    // SAFETY: waiting for our own load-time submission is valid.
    if let Err(error) = unsafe { device.device.queue_wait_idle(device.graphics_queue) } {
        // The commands may still be running: freeing the pool or the resources they use now
        // could be a use-after-free. Wait for the whole device instead; if that fails too,
        // the caller leaks them.
        // SAFETY: the device is live; waiting has no other preconditions.
        let idle = unsafe { device.device.device_wait_idle() }.is_ok();
        return Err(WaitFailure {
            error: error.into(),
            idle,
        });
    }
    Ok(())
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
