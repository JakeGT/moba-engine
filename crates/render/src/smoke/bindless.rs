//! `bindless_smoke` (ADR-0020 gate 3): divergent bindless sampling, checked exactly.
//!
//! 212 solid-colour 1×1 textures are registered; 12 are released during frame 1, which is
//! first reported in flight (nothing recycles) and then retired (their slots point at the
//! magenta fallback again and return to the free list); 10 new textures reuse those slots,
//! lowest first. A 256-entry handle list in a device-address buffer then holds the 210 live
//! handles, the 2 released-and-unused ("stale") handles, `MISSING`, `WHITE` and `FLAT_NORMAL`.
//! Every pixel of a 64×64 frame samples the entry [`slot`] picks with the palette sampler
//! [`sampler`] picks, once in a fragment shader (into a colour attachment) and once in a compute
//! shader (into a bindless storage image). A 1×1 texture returns its texel exactly under every
//! filter, so the expectation is the entry's colour: stale and missing entries read magenta.

use ash::vk;
use gpu_allocator::MemoryLocation;

use super::{Headless, SmokeReport};
use crate::bindless::{
    BindlessTable, NON_COMPARE_SAMPLERS, TextureHandle, color_barrier, color_layers,
    submit_one_shot, upload_solid,
};
use crate::error::{RenderError, RenderResult};
use crate::pipeline::{FullscreenBlend, create_fullscreen};
use crate::resources::{GpuBuffer, GpuImage, GraphicsPipeline, PipelineLayout, ShaderModule};

/// The frame's side, in pixels.
pub(crate) const SIZE: u32 = 64;
/// Handle-list entries (a power of two: the shader masks with `LIST_LEN - 1`).
const LIST_LEN: usize = 256;
/// Textures registered first.
const TEXTURES: usize = 212;
/// Of those, the indices released during frame 1.
const RELEASED: [usize; 12] = [3, 20, 37, 54, 71, 88, 105, 122, 139, 156, 173, 190];
/// New textures registered after the release retires; they reuse the lowest released slots.
const REPLACEMENTS: usize = 10;
/// The frame the releases happen in.
const RELEASE_FRAME: u64 = 1;
const FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const FLAT_NORMAL: [u8; 4] = [128, 128, 255, 255];

/// Texture `index`'s colour (originals `0..212`, replacements `1000..1010`): distinct from each
/// other and from the fallbacks (a unit test checks it).
pub(crate) fn texel(index: u32) -> [u8; 4] {
    [
        (index.wrapping_mul(7).wrapping_add(11) & 0xff) as u8,
        (index.wrapping_mul(53).wrapping_add(29) & 0xff) as u8,
        (index.wrapping_mul(101).wrapping_add(3) & 0x7f) as u8,
        255,
    ]
}

/// The handle-list entry pixel `(x, y)` samples (mirrors `smoke_slot` in the shaders).
pub(crate) fn slot(x: u32, y: u32) -> usize {
    (((x.wrapping_mul(93)) ^ (y.wrapping_mul(159))) as usize) & (LIST_LEN - 1)
}

/// The palette sampler pixel `(x, y)` uses (mirrors `smoke_sampler`).
pub(crate) fn sampler(x: u32, y: u32) -> u32 {
    (x + 3 * y) % NON_COMPARE_SAMPLERS
}

#[allow(clippy::too_many_lines)] // Set-up, one recorded frame and the comparison, in order.
pub(super) fn run(gpu: &Headless, report: &mut SmokeReport) -> RenderResult<()> {
    let device = &gpu.device;
    let mut table = BindlessTable::new(device)?;
    let created_writes = table.writes();

    // Register, release (frame 1), retire, reuse.
    let originals: Vec<[u8; 4]> = (0..TEXTURES as u32).map(texel).collect();
    let mut images: Vec<Option<GpuImage>> = upload_solid(device, "smoke-texture", &originals)?
        .into_iter()
        .map(Some)
        .collect();
    let mut handles = Vec::with_capacity(TEXTURES);
    for image in images.iter().flatten() {
        handles.push(table.register_texture(image.view())?);
    }
    for &index in &RELEASED {
        if !table.release_texture(handles[index], RELEASE_FRAME) {
            return Err(RenderError::Shader(
                "a live bindless handle was not released",
            ));
        }
    }
    table.retire(RELEASE_FRAME - 1);
    report.check(
        || String::from("released handles pending while frame 1 is in flight"),
        table.pending().0,
        RELEASED.len(),
    );
    table.retire(RELEASE_FRAME);
    report.check(
        || String::from("released handles pending after frame 1 retired"),
        table.pending().0,
        0,
    );
    // The descriptors now point at the fallback, so the images can go.
    for &index in &RELEASED {
        images[index] = None;
    }
    let replacement_colors: Vec<[u8; 4]> = (1000..1000 + REPLACEMENTS as u32).map(texel).collect();
    let replacements = upload_solid(device, "smoke-replacement", &replacement_colors)?;
    let mut replacement_handles = Vec::with_capacity(REPLACEMENTS);
    for image in &replacements {
        replacement_handles.push(table.register_texture(image.view())?);
    }
    let mut released_handles: Vec<TextureHandle> =
        RELEASED.iter().map(|&index| handles[index]).collect();
    released_handles.sort_unstable();
    report.check(
        || String::from("replacement handles (the lowest released slots, in order)"),
        replacement_handles.clone(),
        released_handles[..REPLACEMENTS].to_vec(),
    );
    let stale = &released_handles[REPLACEMENTS..];

    // The handle list and what each entry must read.
    let mut list: Vec<(TextureHandle, [u8; 4])> = Vec::with_capacity(LIST_LEN);
    for (index, &handle) in handles.iter().enumerate() {
        if !RELEASED.contains(&index) {
            list.push((handle, originals[index]));
        }
    }
    for (&handle, &color) in replacement_handles.iter().zip(&replacement_colors) {
        list.push((handle, color));
    }
    for &handle in stale {
        list.push((handle, MAGENTA));
    }
    while list.len() < LIST_LEN - 16 {
        list.push((TextureHandle::MISSING, MAGENTA));
    }
    while list.len() < LIST_LEN - 8 {
        list.push((TextureHandle::WHITE, WHITE));
    }
    while list.len() < LIST_LEN {
        list.push((TextureHandle::FLAT_NORMAL, FLAT_NORMAL));
    }
    let list_bytes: Vec<u8> = list
        .iter()
        .flat_map(|(handle, _)| handle.index().to_le_bytes())
        .collect();
    let mut list_buffer = GpuBuffer::new(
        device.device.clone(),
        device.allocator(),
        "smoke-handle-list",
        list_bytes.len() as u64,
        vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
        MemoryLocation::CpuToGpu,
    )?;
    list_buffer.write_bytes(0, &list_bytes);
    report.notes.push(format!(
        "{} textures live ({} registered, {} released in frame {RELEASE_FRAME} and retired, \
         {} reused their slots {:?}, stale {:?} read the fallback)",
        table.live_textures(),
        TEXTURES + REPLACEMENTS,
        RELEASED.len(),
        REPLACEMENTS,
        replacement_handles
            .iter()
            .map(|handle| handle.index())
            .collect::<Vec<_>>(),
        stale
            .iter()
            .map(|handle| handle.index())
            .collect::<Vec<_>>(),
    ));

    // Targets, pipelines and readbacks.
    let extent = vk::Extent2D {
        width: SIZE,
        height: SIZE,
    };
    let color = GpuImage::new(
        device.device.clone(),
        device.allocator(),
        "smoke-color",
        extent,
        FORMAT,
        vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        vk::ImageAspectFlags::COLOR,
    )?;
    let storage = GpuImage::new(
        device.device.clone(),
        device.allocator(),
        "smoke-storage",
        extent,
        FORMAT,
        vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC,
        vk::ImageAspectFlags::COLOR,
    )?;
    let storage_handle = table.register_storage_image(storage.view())?;
    let bindings = crate::bindless::set_layout_bindings();
    crate::descriptor_budget::check_tiered(
        "bindless_smoke",
        &[&bindings],
        device.update_after_bind,
    )?;
    let push_range = vk::PushConstantRange {
        stage_flags: vk::ShaderStageFlags::FRAGMENT | vk::ShaderStageFlags::COMPUTE,
        offset: 0,
        size: 24,
    };
    let layout = PipelineLayout::with_push_constants(
        device.device.clone(),
        &[table.layout()],
        core::slice::from_ref(&push_range),
    )?;
    let vertex = ShaderModule::new(device.device.clone(), moba_shaders::FULLSCREEN_VERT)?;
    let fragment = ShaderModule::new(device.device.clone(), moba_shaders::BINDLESS_SMOKE_FRAG)?;
    let compute = ShaderModule::new(device.device.clone(), moba_shaders::BINDLESS_SMOKE_COMP)?;
    let graphics = GraphicsPipeline::from_raw(
        device.device.clone(),
        create_fullscreen(
            device,
            layout.handle(),
            vertex.handle(),
            fragment.handle(),
            FORMAT,
            FullscreenBlend::None,
        )?,
    );
    let stage = vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(compute.handle())
        .name(c"main");
    let compute_pipeline = GraphicsPipeline::from_raw(
        device.device.clone(),
        device.create_compute_pipeline(
            &vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(layout.handle()),
        )?,
    );
    let bytes = (SIZE * SIZE * 4) as usize;
    let readback = |name| {
        GpuBuffer::new(
            device.device.clone(),
            device.allocator(),
            name,
            bytes as u64,
            vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuToCpu,
        )
    };
    let color_readback = readback("smoke-color-readback")?;
    let storage_readback = readback("smoke-storage-readback")?;

    let mut push = [0u8; 24];
    push[..8].copy_from_slice(&super::address(&list_buffer).to_le_bytes());
    push[8..12].copy_from_slice(&(LIST_LEN as u32 - 1).to_le_bytes());
    push[12..16].copy_from_slice(&NON_COMPARE_SAMPLERS.to_le_bytes());
    push[16..20].copy_from_slice(&storage_handle.index().to_le_bytes());
    push[20..24].copy_from_slice(&SIZE.to_le_bytes());
    let writes_before_frame = table.writes();

    // The frame: draw, dispatch, copy both results out.
    submit_one_shot(device, |command| {
        let to_targets = [
            color_barrier(
                color.image(),
                (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                (
                    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                ),
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            ),
            color_barrier(
                storage.image(),
                (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                (
                    vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::SHADER_STORAGE_WRITE,
                ),
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::GENERAL,
            ),
        ];
        device.cmd_pipeline_barrier2(
            command,
            &vk::DependencyInfo::default().image_memory_barriers(&to_targets),
        );
        let attachment = vk::RenderingAttachmentInfo::default()
            .image_view(color.view())
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                color: vk::ClearColorValue { float32: [0.0; 4] },
            });
        let rendering = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent,
            })
            .layer_count(1)
            .color_attachments(core::slice::from_ref(&attachment));
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: SIZE as f32,
            height: SIZE as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let scissor = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent,
        };
        let sets = [table.set()];
        device.cmd_begin_rendering(command, &rendering);
        let d = &device.device;
        // SAFETY: the command buffer is recording inside dynamic rendering and the pipeline
        // is live.
        unsafe { d.cmd_bind_pipeline(command, vk::PipelineBindPoint::GRAPHICS, graphics.handle()) };
        // SAFETY: the viewport is dynamic in this pipeline.
        unsafe { d.cmd_set_viewport(command, 0, core::slice::from_ref(&viewport)) };
        // SAFETY: the scissor is dynamic in this pipeline.
        unsafe { d.cmd_set_scissor(command, 0, core::slice::from_ref(&scissor)) };
        // SAFETY: the set is live and matches set 0 of the layout.
        unsafe {
            d.cmd_bind_descriptor_sets(
                command,
                vk::PipelineBindPoint::GRAPHICS,
                layout.handle(),
                0,
                &sets,
                &[],
            );
        }
        // SAFETY: the layout's push range covers these 24 bytes for both stages.
        unsafe { d.cmd_push_constants(command, layout.handle(), push_range.stage_flags, 0, &push) };
        // SAFETY: a fullscreen triangle with no vertex input, inside the rendering.
        unsafe { d.cmd_draw(command, 3, 1, 0, 0) };
        device.cmd_end_rendering(command);
        // SAFETY: the compute pipeline is live.
        unsafe {
            d.cmd_bind_pipeline(
                command,
                vk::PipelineBindPoint::COMPUTE,
                compute_pipeline.handle(),
            );
        }
        // SAFETY: as for the graphics bind point.
        unsafe {
            d.cmd_bind_descriptor_sets(
                command,
                vk::PipelineBindPoint::COMPUTE,
                layout.handle(),
                0,
                &sets,
                &[],
            );
        }
        // SAFETY: as above.
        unsafe { d.cmd_push_constants(command, layout.handle(), push_range.stage_flags, 0, &push) };
        // SAFETY: the storage image is in `GENERAL` and the dispatch covers exactly the 64×64
        // image in 8×8 groups.
        unsafe { d.cmd_dispatch(command, SIZE / 8, SIZE / 8, 1) };
        let to_copy = [
            color_barrier(
                color.image(),
                (
                    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                ),
                (
                    vk::PipelineStageFlags2::COPY,
                    vk::AccessFlags2::TRANSFER_READ,
                ),
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            ),
            color_barrier(
                storage.image(),
                (
                    vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::SHADER_STORAGE_WRITE,
                ),
                (
                    vk::PipelineStageFlags2::COPY,
                    vk::AccessFlags2::TRANSFER_READ,
                ),
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            ),
        ];
        device.cmd_pipeline_barrier2(
            command,
            &vk::DependencyInfo::default().image_memory_barriers(&to_copy),
        );
        copy_to_buffer(device, command, &color, &color_readback, extent);
        copy_to_buffer(device, command, &storage, &storage_readback, extent);
        host_read_barrier(device, command);
    })?;
    report.check(
        || String::from("descriptor writes while recording and running the frame"),
        table.writes() - writes_before_frame,
        0,
    );
    // The frame retired (its fence signalled): the output's slot can be released and recycled.
    let frame = RELEASE_FRAME + 1;
    let released = table.release_storage_image(storage_handle, frame);
    table.retire(frame);
    report.check(
        || String::from("storage image released and retired after the frame"),
        (released, table.pending().1),
        (true, 0),
    );
    report.notes.push(format!(
        "descriptor writes: {created_writes} at creation (the fallback in every slot), {} at \
         load (registrations and retirement), 0 in the frame",
        writes_before_frame - created_writes
    ));

    let mut color_pixels = vec![0u8; bytes];
    let mut storage_pixels = vec![0u8; bytes];
    if !color_readback.read_bytes(0, &mut color_pixels)
        || !storage_readback.read_bytes(0, &mut storage_pixels)
    {
        return Err(RenderError::Shader("smoke readback is not host visible"));
    }
    for (name, pixels) in [("fragment", &color_pixels), ("compute", &storage_pixels)] {
        for y in 0..SIZE {
            for x in 0..SIZE {
                let at = ((y * SIZE + x) * 4) as usize;
                let actual: [u8; 4] = pixels[at..at + 4].try_into().expect("four bytes");
                let entry = slot(x, y);
                let (handle, expected) = list[entry];
                report.check(
                    || {
                        format!(
                            "{name} pixel ({x}, {y}) list[{entry}] texture {} sampler {}",
                            handle.index(),
                            sampler(x, y)
                        )
                    },
                    actual,
                    expected,
                );
            }
        }
    }
    let list = &list;
    let distinct: std::collections::BTreeSet<u32> = (0..SIZE)
        .flat_map(|y| (0..SIZE).map(move |x| list[slot(x, y)].0.index()))
        .collect();
    report.notes.push(format!(
        "{} distinct texture handles and {NON_COMPARE_SAMPLERS} samplers sampled across \
         {SIZE}x{SIZE} pixels, twice (fragment, compute)",
        distinct.len()
    ));
    Ok(())
}

/// Copies a whole colour image in `TRANSFER_SRC_OPTIMAL` into `buffer`, tightly packed.
pub(super) fn copy_to_buffer(
    device: &crate::device::DeviceContext,
    command: vk::CommandBuffer,
    image: &GpuImage,
    buffer: &GpuBuffer,
    extent: vk::Extent2D,
) {
    let region = vk::BufferImageCopy::default()
        .image_subresource(color_layers())
        .image_extent(vk::Extent3D {
            width: extent.width,
            height: extent.height,
            depth: 1,
        });
    // SAFETY: the image was created with `TRANSFER_SRC` and is in `TRANSFER_SRC_OPTIMAL`; the
    // buffer was created with `TRANSFER_DST` and holds the whole image tightly packed.
    unsafe {
        device.device.cmd_copy_image_to_buffer(
            command,
            image.image(),
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            buffer.handle(),
            core::slice::from_ref(&region),
        );
    }
}

/// Makes the copies' writes visible to the host's reads after the fence.
pub(super) fn host_read_barrier(device: &crate::device::DeviceContext, command: vk::CommandBuffer) {
    let barrier = vk::MemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COPY)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::HOST)
        .dst_access_mask(vk::AccessFlags2::HOST_READ);
    device.cmd_pipeline_barrier2(
        command,
        &vk::DependencyInfo::default().memory_barriers(core::slice::from_ref(&barrier)),
    );
}

#[cfg(test)]
mod tests {
    use super::{FLAT_NORMAL, LIST_LEN, MAGENTA, REPLACEMENTS, SIZE, TEXTURES, WHITE, slot, texel};

    #[test]
    fn texture_colours_are_distinct_and_never_a_fallback() {
        let colors: Vec<_> = (0..TEXTURES as u32)
            .chain(1000..1000 + REPLACEMENTS as u32)
            .map(texel)
            .collect();
        let unique: std::collections::BTreeSet<_> = colors.iter().collect();
        assert_eq!(unique.len(), colors.len());
        for fallback in [MAGENTA, WHITE, FLAT_NORMAL] {
            assert!(!unique.contains(&fallback));
        }
    }

    /// Neighbouring pixels read different entries (the handles diverge in every quad), and the
    /// frame reaches every entry.
    #[test]
    fn the_slot_pattern_diverges_and_covers_the_list() {
        let mut seen = vec![false; LIST_LEN];
        for y in 0..SIZE {
            for x in 0..SIZE {
                seen[slot(x, y)] = true;
                if x + 1 < SIZE {
                    assert_ne!(slot(x, y), slot(x + 1, y));
                }
                if y + 1 < SIZE {
                    assert_ne!(slot(x, y), slot(x, y + 1));
                }
            }
        }
        assert!(seen.iter().all(|&seen| seen));
    }
}
