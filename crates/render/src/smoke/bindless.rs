//! `bindless_smoke` (ADR-0020 gate 3): divergent bindless sampling, checked exactly.
//!
//! 212 textures (2×2, four distinct texels each) and a 2×2 depth texture are registered; 12
//! textures are released during frame 1, which is first reported in flight (nothing recycles)
//! and then retired (their slots point at the magenta fallback again and return to the free
//! list); 10 new textures reuse those slots, lowest first. A 256-entry handle list in a
//! device-address buffer then holds 209 live handles (one live texture is left out, see
//! below), the 2 recycled-and-unused slots, `MISSING`, `WHITE` and `FLAT_NORMAL`.
//!
//! Every pixel of a 64×64 frame samples the entry [`slot`] picks with the palette sampler
//! [`sampler`] picks, at [`UV`], once in a fragment shader (into a colour attachment) and once
//! in a compute shader (into a bindless storage image). [`UV`] is chosen so that the result
//! depends on the sampler: wrap and clamp, point and linear give four different values for
//! every 2×2 texture ([`sample`]), exactly. In the compute pass the alpha channel is a depth
//! comparison of the depth texture through the comparison sampler [`compare_sampler`] picks,
//! which also differs per comparison entry. (The fragment pass checks no comparison: lavapipe,
//! Mesa 26.2.5, crashes in the JIT'd fragment shader when it takes a second sample after a
//! `nonuniformEXT` one; the compute shader is unaffected.) The 1×1 fallbacks return their
//! texel under every sampler, so the 2 recycled slots and `MISSING` read magenta. (No frame
//! may index a released handle; the smoke indexes those two free slots directly, to check the
//! rewrite at retirement, and writes nothing to them while the frame is pending.)
//!
//! **Update after bind.** While the frame is pending (submitted, not yet waited for), the
//! smoke releases and retires the live texture the list leaves out, registers a new texture
//! into that slot, and registers, releases and retires a second storage image: four descriptor
//! writes into the bound set, to slots the frame does not index. Without `UPDATE_AFTER_BIND`
//! and `UPDATE_UNUSED_WHILE_PENDING` validation reports them
//! (VUID-vkUpdateDescriptorSets-None-03047), which fails the run.

use ash::vk;
use gpu_allocator::MemoryLocation;

use super::{Headless, SmokeReport};
use crate::bindless::{
    BindlessTable, NON_COMPARE_SAMPLERS, PALETTE_LEN, SamplerAddress, SamplerFilter, SamplerKey,
    TextureData, TextureHandle, color_barrier, color_layers, submit, upload_textures,
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
/// The original the handle list leaves out: released, retired and its slot reused while the
/// frame is pending. Index 0, the lowest slot, so the reuse lands on it.
const UNLISTED: usize = 0;
/// The frame the releases happen in.
const RELEASE_FRAME: u64 = 1;
/// The frame the smoke records and submits.
const SMOKE_FRAME: u64 = 2;
/// Descriptor writes the smoke makes while the frame is pending.
const PENDING_WRITES: u64 = 4;
const FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const FLAT_NORMAL: [u8; 4] = [128, 128, 255, 255];

/// Every texture is 2×2.
const TEXTURE_SIDE: u32 = 2;
/// Where every pixel samples (mirrors `SMOKE_UV`). With 2×2 textures, `u` lies in the texel
/// left of the image (wrap picks the right column, clamp the left one) and `v` inside the
/// bottom row, a quarter texel from the centre line, so point and linear differ too. The
/// bilinear weights are multiples of 1/16, so with `subTexelPrecisionBits` ≥ 4 and the texel
/// offsets multiples of 16 the results are whole numbers; the spec only says UNORM
/// conversion *should* round to nearest, which lavapipe, RADV and NVIDIA do.
pub(crate) const UV: [f64; 2] = [-0.125, 0.625];
/// Per-channel offsets of a texture's texels from its base colour, in row-major order
/// (x0y0, x1y0, x0y1, x1y1). Multiples of 16, so every 1/16-weighted sum is an integer.
const TEXEL_OFFSETS: [u8; 4] = [0, 64, 32, 96];
/// The depth texture's texels, row-major, and the comparison reference (`LESS_OR_EQUAL`):
/// only x0y1 passes, so point/wrap, point/clamp, linear/wrap and linear/clamp give 0, 1, 3/16
/// and 3/4.
const DEPTHS: [f32; 4] = [0.25, 0.25, 0.75, 0.25];
pub(crate) const REFERENCE: f64 = 0.5;
/// The comparison result is written to alpha as `result * COMPARE_SCALE / 255` (mirrors
/// `SMOKE_COMPARE_SCALE`): 240 makes 3/16 and 3/4 whole numbers.
const COMPARE_SCALE: f64 = 240.0;

/// Texture `id`'s four texels (originals `0..212`, replacements `212..222`, the texture
/// registered while the frame is pending `222`): a base colour, distinct per id, plus
/// [`TEXEL_OFFSETS`]. Alpha is opaque.
pub(crate) fn texels(id: u32) -> [[u8; 4]; 4] {
    // A bijection of 0..512 (37 is odd), split into three channels of multiples of 16 below
    // 128, so base + offset stays below 256.
    let scrambled = id.wrapping_mul(37) % 512;
    let base = [
        (scrambled & 7) as u8 * 16,
        ((scrambled >> 3) & 7) as u8 * 16,
        ((scrambled >> 6) & 7) as u8 * 16,
    ];
    TEXEL_OFFSETS.map(|offset| [base[0] + offset, base[1] + offset, base[2] + offset, 255])
}

/// The handle-list entry pixel `(x, y)` samples (mirrors `smoke_slot` in the shaders).
pub(crate) fn slot(x: u32, y: u32) -> usize {
    (((x.wrapping_mul(93)) ^ (y.wrapping_mul(159))) as usize) & (LIST_LEN - 1)
}

/// The non-comparison palette sampler pixel `(x, y)` uses (mirrors `smoke_sampler`).
pub(crate) fn sampler(x: u32, y: u32) -> u32 {
    (x + 3 * y) % NON_COMPARE_SAMPLERS
}

/// The comparison palette sampler pixel `(x, y)` uses for alpha (mirrors
/// `smoke_compare_sampler`).
pub(crate) fn compare_sampler(x: u32, y: u32) -> u32 {
    NON_COMPARE_SAMPLERS + (2 * x + y) % (PALETTE_LEN - NON_COMPARE_SAMPLERS)
}

/// Samples a `side`×`side` texture at [`UV`] with `key`'s address mode and filter, as the
/// Vulkan spec defines it (`fetch` returns one texel, normalised). Anisotropic filtering is
/// plain linear here: every pixel samples the same coordinate, so the derivatives are zero (the
/// fragment shader) or the LOD is explicit (compute).
pub(crate) fn sample<const C: usize>(
    side: u32,
    key: SamplerKey,
    fetch: impl Fn(u32, u32) -> [f64; C],
) -> [f64; C] {
    let texels = i64::from(side);
    let address = |index: i64| -> u32 {
        let index = match key.address {
            SamplerAddress::Wrap => index.rem_euclid(texels),
            SamplerAddress::Clamp => index.clamp(0, texels - 1),
        };
        u32::try_from(index).expect("in range")
    };
    // Texel-space coordinates.
    let [s, t] = UV.map(|coordinate| coordinate * f64::from(side));
    match key.filter {
        SamplerFilter::Point => fetch(address(s.floor() as i64), address(t.floor() as i64)),
        SamplerFilter::Linear | SamplerFilter::Anisotropic => {
            let (left, top) = ((s - 0.5).floor(), (t - 0.5).floor());
            let (alpha, beta) = (s - 0.5 - left, t - 0.5 - top);
            let (i0, j0) = (left as i64, top as i64);
            let taps = [
                (
                    (1.0 - alpha) * (1.0 - beta),
                    fetch(address(i0), address(j0)),
                ),
                (alpha * (1.0 - beta), fetch(address(i0 + 1), address(j0))),
                ((1.0 - alpha) * beta, fetch(address(i0), address(j0 + 1))),
                (alpha * beta, fetch(address(i0 + 1), address(j0 + 1))),
            ];
            let mut out = [0.0; C];
            for (weight, texel) in taps {
                for (out, value) in out.iter_mut().zip(texel) {
                    *out += weight * value;
                }
            }
            out
        }
    }
}

/// A handle-list entry: a 2×2 smoke texture (its id) or a 1×1 fallback (its texel).
#[derive(Clone, Copy, Debug)]
enum Entry {
    Texture(u32),
    Solid([u8; 4]),
}

impl Entry {
    /// What sampling it through palette entry `sampler` writes to an `R8G8B8A8_UNORM` target.
    fn expected(self, sampler: u32) -> [u8; 4] {
        match self {
            // A 1×1 texture returns its texel under every address mode and filter.
            Self::Solid(texel) => texel,
            Self::Texture(id) => {
                let texels = texels(id);
                let value = sample(
                    TEXTURE_SIDE,
                    SamplerKey::palette()[sampler as usize],
                    |x, y| texels[(y * TEXTURE_SIDE + x) as usize].map(f64::from),
                );
                value.map(|channel| channel.round() as u8)
            }
        }
    }
}

/// The depth comparison through palette entry `sampler`, scaled as the shader writes it.
fn expected_compare(sampler: u32) -> u8 {
    let [value] = sample(
        TEXTURE_SIDE,
        SamplerKey::palette()[sampler as usize],
        |x, y| {
            let depth = f64::from(DEPTHS[(y * TEXTURE_SIDE + x) as usize]);
            [if REFERENCE <= depth { 1.0 } else { 0.0 }]
        },
    );
    (value * COMPARE_SCALE).round() as u8
}

/// Uploads 2×2 smoke textures for `ids`, and the depth texture when `depth` is set (last).
fn upload_smoke(
    gpu: &Headless,
    name: &str,
    ids: impl Iterator<Item = u32>,
    depth: bool,
) -> RenderResult<Vec<GpuImage>> {
    let extent = vk::Extent2D {
        width: TEXTURE_SIDE,
        height: TEXTURE_SIDE,
    };
    let mut bytes: Vec<Vec<u8>> = ids
        .map(|id| texels(id).iter().flatten().copied().collect())
        .collect();
    let colors = bytes.len();
    if depth {
        bytes.push(
            DEPTHS
                .iter()
                .flat_map(|depth| depth.to_le_bytes())
                .collect(),
        );
    }
    let data: Vec<TextureData> = bytes
        .iter()
        .enumerate()
        .map(|(index, bytes)| TextureData {
            extent,
            format: if index < colors {
                FORMAT
            } else {
                vk::Format::D32_SFLOAT
            },
            bytes,
        })
        .collect();
    upload_textures(&gpu.device, name, &data, |_| {})
}

#[allow(clippy::too_many_lines)] // Set-up, one recorded frame and the comparison, in order.
pub(super) fn run(gpu: &Headless, report: &mut SmokeReport) -> RenderResult<()> {
    let device = &gpu.device;
    let mut table = BindlessTable::new(device)?;
    let created_writes = table.writes();
    // Comparison entries are checked when the palette holds them (every tiered device today)
    // and `D32_SFLOAT` filters comparisons linearly (else the filtered value is the driver's).
    let depth_features = crate::targets::optimal_features_2(device, vk::Format::D32_SFLOAT);
    let compare = device.comparison_samplers
        && depth_features.contains(
            vk::FormatFeatureFlags2::SAMPLED_IMAGE
                | vk::FormatFeatureFlags2::SAMPLED_IMAGE_DEPTH_COMPARISON
                | vk::FormatFeatureFlags2::SAMPLED_IMAGE_FILTER_LINEAR,
        );

    // Register (the originals, then the depth texture), release (frame 1), retire, reuse.
    let mut uploaded = upload_smoke(gpu, "smoke-texture", 0..TEXTURES as u32, compare)?;
    let depth_image = if compare { uploaded.pop() } else { None };
    let mut images: Vec<Option<GpuImage>> = uploaded.into_iter().map(Some).collect();
    let mut handles = Vec::with_capacity(TEXTURES);
    for image in images.iter().flatten() {
        handles.push(table.register_texture(image.view())?);
    }
    let depth_handle = match &depth_image {
        Some(image) => table.register_texture(image.view())?,
        None => TextureHandle::MISSING,
    };
    for &index in &RELEASED {
        // SAFETY: no frame has been recorded yet, so frame 1 is the last that may index these
        // handles, and the list built below leaves them out.
        let released = unsafe { table.release_texture(handles[index], RELEASE_FRAME) };
        if !released {
            return Err(RenderError::Shader(
                "a live bindless handle was not released",
            ));
        }
    }
    // SAFETY: nothing has been submitted, so every frame up to 0 has retired; frame 1's
    // releases stay pending.
    unsafe { table.retire(RELEASE_FRAME - 1) };
    report.check(
        || String::from("released handles pending while frame 1 is in flight"),
        table.pending().0,
        RELEASED.len(),
    );
    // SAFETY: frame 1 recorded no GPU work, so it has retired, and its releases kept the
    // contract (above).
    unsafe { table.retire(RELEASE_FRAME) };
    report.check(
        || String::from("released handles pending after frame 1 retired"),
        table.pending().0,
        0,
    );
    // The descriptors now point at the fallback, so the images can go.
    for &index in &RELEASED {
        images[index] = None;
    }
    // The replacements and the texture registered while the frame is pending, uploaded now:
    // a one-shot wait during the frame would retire it in the validation layer's view.
    let late_id = (TEXTURES + REPLACEMENTS) as u32;
    let mut later = upload_smoke(gpu, "smoke-replacement", TEXTURES as u32..=late_id, false)?;
    let late_image = later.pop().expect("uploaded");
    let replacements = later;
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
    let recycled = &released_handles[REPLACEMENTS..];

    // The handle list and what each entry must read.
    let mut list: Vec<(TextureHandle, Entry)> = Vec::with_capacity(LIST_LEN);
    for (index, &handle) in handles.iter().enumerate() {
        if !RELEASED.contains(&index) && index != UNLISTED {
            list.push((handle, Entry::Texture(index as u32)));
        }
    }
    for (offset, &handle) in replacement_handles.iter().enumerate() {
        list.push((handle, Entry::Texture((TEXTURES + offset) as u32)));
    }
    for &handle in recycled {
        list.push((handle, Entry::Solid(MAGENTA)));
    }
    while list.len() < LIST_LEN - 16 {
        list.push((TextureHandle::MISSING, Entry::Solid(MAGENTA)));
    }
    while list.len() < LIST_LEN - 8 {
        list.push((TextureHandle::WHITE, Entry::Solid(WHITE)));
    }
    while list.len() < LIST_LEN {
        list.push((TextureHandle::FLAT_NORMAL, Entry::Solid(FLAT_NORMAL)));
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
         {} reused their slots {:?}, recycled slots {:?} read the fallback)",
        table.live_textures(),
        TEXTURES + REPLACEMENTS + usize::from(compare),
        RELEASED.len(),
        REPLACEMENTS,
        replacement_handles
            .iter()
            .map(|handle| handle.index())
            .collect::<Vec<_>>(),
        recycled
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
    // Registered and released while the frame is pending; never accessed, so it stays in
    // `UNDEFINED`.
    let spare_storage = GpuImage::new(
        device.device.clone(),
        device.allocator(),
        "smoke-spare-storage",
        vk::Extent2D {
            width: 1,
            height: 1,
        },
        FORMAT,
        vk::ImageUsageFlags::STORAGE,
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
        size: 32,
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

    let compare_count = if compare {
        PALETTE_LEN - NON_COMPARE_SAMPLERS
    } else {
        0
    };
    let mut push = [0u8; 32];
    push[..8].copy_from_slice(&super::address(&list_buffer).to_le_bytes());
    push[8..12].copy_from_slice(&(LIST_LEN as u32 - 1).to_le_bytes());
    push[12..16].copy_from_slice(&NON_COMPARE_SAMPLERS.to_le_bytes());
    push[16..20].copy_from_slice(&storage_handle.index().to_le_bytes());
    push[20..24].copy_from_slice(&SIZE.to_le_bytes());
    push[24..28].copy_from_slice(&depth_handle.index().to_le_bytes());
    push[28..32].copy_from_slice(&compare_count.to_le_bytes());

    // The frame (number 2): draw, dispatch, copy both results out. Submitted, not waited for.
    let pending = submit(device, |command| {
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
        // SAFETY: the layout's push range covers these 32 bytes for both stages.
        unsafe { d.cmd_push_constants(command, layout.handle(), push_range.stage_flags, 0, &push) };
        // SAFETY: a fullscreen triangle with no vertex input, inside the rendering. The handle
        // list holds live handles, the fallbacks, the two free slots in `RELEASED` (left
        // unwritten while the frame is pending: the late registration takes the lower,
        // recycled slot, as asserted there) and the depth handle, read only through the
        // comparison macro.
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
        // image in 8×8 groups. It indexes the same handle list as the draw.
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

    // While frame 2 is pending: update after bind, on slots it does not index. Nothing here
    // queries the fence, so the validation layer still treats the frame as pending.
    let writes_before_pending = table.writes();
    let unlisted = handles[UNLISTED];
    // SAFETY: no frame indexes the unlisted texture (the list leaves it out), so frame 1 is
    // the last that may, and frame 2 (pending) does not.
    let released = unsafe { table.release_texture(unlisted, RELEASE_FRAME) };
    // SAFETY: frame 1 has retired (it recorded no GPU work), and every release so far kept
    // the contract; frame 2 indexes no recycled slot.
    unsafe { table.retire(RELEASE_FRAME) };
    // Frame 2 also indexes the free slots in `RELEASED`; the late texture must land in the
    // slot just recycled, which is lower (allocation takes the lowest free slot).
    // Checked in every build (the smokes run in release) and before the write.
    if table.next_texture_slot() != Some(unlisted.index()) {
        return Err(RenderError::Shader(
            "bindless smoke: the late texture would not take the recycled slot",
        ));
    }
    let late_handle = table.register_texture(late_image.view())?;
    let spare_handle = table.register_storage_image(spare_storage.view())?;
    // SAFETY: no frame indexes the spare storage image (frame 2 writes only `storage_handle`).
    let spare_released = unsafe { table.release_storage_image(spare_handle, RELEASE_FRAME) };
    // SAFETY: as the retirement above; frame 2 does not index the spare's slot.
    unsafe { table.retire(RELEASE_FRAME) };
    let pending_writes = table.writes() - writes_before_pending;
    pending.wait()?;
    // The image the unlisted handle named is no longer referenced by any descriptor.
    images[UNLISTED] = None;
    report.check(
        || String::from("texture registered while the frame was pending reuses the retired slot"),
        (released, late_handle),
        (true, unlisted),
    );
    report.check(
        || String::from("descriptor writes while the frame was pending (update after bind)"),
        (spare_released, pending_writes),
        (true, PENDING_WRITES),
    );
    // Frame 2 retired (its fence signalled): the output's slot can be released and recycled.
    // SAFETY: frame 2 is the last frame that indexes the output, and no frame is recorded
    // after it.
    let released = unsafe { table.release_storage_image(storage_handle, SMOKE_FRAME) };
    // SAFETY: frame 2's fence was waited for just above.
    unsafe { table.retire(SMOKE_FRAME) };
    report.check(
        || String::from("storage image released and retired after the frame"),
        (released, table.pending().1),
        (true, 0),
    );
    report.notes.push(format!(
        "descriptor writes: {created_writes} at creation (the fallback in every slot), {} \
         before the frame (registrations and retirement), {pending_writes} while it was \
         pending (texture slot {} retired and reused, storage slot {} registered, released and \
         retired)",
        writes_before_pending - created_writes,
        unlisted.index(),
        spare_handle.index(),
    ));

    let mut color_pixels = vec![0u8; bytes];
    let mut storage_pixels = vec![0u8; bytes];
    if !color_readback.read_bytes(0, &mut color_pixels)
        || !storage_readback.read_bytes(0, &mut storage_pixels)
    {
        return Err(RenderError::Shader("smoke readback is not host visible"));
    }
    // Only the compute half writes the comparison into alpha (see `bindless_smoke.frag`).
    for (name, pixels, compares) in [
        ("fragment", &color_pixels, false),
        ("compute", &storage_pixels, compare),
    ] {
        for y in 0..SIZE {
            for x in 0..SIZE {
                let at = ((y * SIZE + x) * 4) as usize;
                let actual: [u8; 4] = pixels[at..at + 4].try_into().expect("four bytes");
                let entry = slot(x, y);
                let (handle, content) = list[entry];
                let mut expected = content.expected(sampler(x, y));
                if compares {
                    expected[3] = expected_compare(compare_sampler(x, y));
                }
                report.check(
                    || {
                        format!(
                            "{name} pixel ({x}, {y}) list[{entry}] texture {} sampler {} \
                             compare sampler {}",
                            handle.index(),
                            sampler(x, y),
                            if compares {
                                compare_sampler(x, y).to_string()
                            } else {
                                String::from("-")
                            }
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
        "{} distinct texture handles with {NON_COMPARE_SAMPLERS} samplers sampled across \
         {SIZE}x{SIZE} pixels at uv ({}, {}), twice (fragment, compute); {}",
        distinct.len(),
        UV[0],
        UV[1],
        if compare {
            format!(
                "compute alpha: the depth texture (handle {}) through the {} comparison \
                 samplers",
                depth_handle.index(),
                PALETTE_LEN - NON_COMPARE_SAMPLERS
            )
        } else {
            String::from("comparison samplers not checked (no linear-filtered depth comparison)")
        }
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
    use super::{
        COMPARE_SCALE, DEPTHS, Entry, FLAT_NORMAL, LIST_LEN, MAGENTA, REFERENCE, REPLACEMENTS,
        SIZE, TEXTURE_SIDE, TEXTURES, WHITE, compare_sampler, expected_compare, sample, sampler,
        slot, texels,
    };
    use crate::bindless::{NON_COMPARE_SAMPLERS, PALETTE_LEN, SamplerKey};

    /// Every texture id the smoke uploads.
    fn ids() -> core::ops::RangeInclusive<u32> {
        0..=(TEXTURES + REPLACEMENTS) as u32
    }

    /// Through every non-comparison sampler, every texture reads a colour no other texture
    /// and no fallback reads, so a wrong handle cannot pass.
    #[test]
    fn texture_results_are_distinct_and_never_a_fallback() {
        for sampler in 0..NON_COMPARE_SAMPLERS {
            let results: Vec<_> = ids()
                .map(|id| Entry::Texture(id).expected(sampler))
                .collect();
            let unique: std::collections::BTreeSet<_> = results.iter().collect();
            assert_eq!(unique.len(), results.len(), "sampler {sampler}");
            for fallback in [MAGENTA, WHITE, FLAT_NORMAL] {
                assert!(!unique.contains(&fallback));
            }
        }
    }

    /// The result depends on the sampler: for every texture, the four address/filter
    /// combinations read four different colours (so a swapped or missing palette entry
    /// fails), anisotropic equals linear, and every value is a whole number before conversion
    /// (exact given round-to-nearest UNORM conversion; see [`UV`]). The comparison entries also give four
    /// distinct, exact values.
    #[test]
    fn every_sampler_reads_a_different_exact_value() {
        let palette = SamplerKey::palette();
        for id in ids() {
            let texels = texels(id);
            let mut distinct = std::collections::BTreeSet::new();
            for (slot, key) in palette[..NON_COMPARE_SAMPLERS as usize].iter().enumerate() {
                let raw = sample(TEXTURE_SIDE, *key, |x, y| {
                    texels[(y * TEXTURE_SIDE + x) as usize].map(f64::from)
                });
                for channel in raw {
                    assert!((channel - channel.round()).abs() < 1e-9, "texture {id}");
                }
                distinct.insert((key.address as u8, key.filter as u8 % 2));
                let result = Entry::Texture(id).expected(slot as u32);
                for (other_slot, other) in
                    palette[..NON_COMPARE_SAMPLERS as usize].iter().enumerate()
                {
                    let same_class = other.address == key.address
                        && (other.filter as u8 % 2) == (key.filter as u8 % 2);
                    let other_result = Entry::Texture(id).expected(other_slot as u32);
                    assert_eq!(
                        result == other_result,
                        same_class,
                        "texture {id}, samplers {slot} and {other_slot}"
                    );
                }
            }
            assert_eq!(distinct.len(), 4);
        }
        let compares: Vec<_> = (NON_COMPARE_SAMPLERS..PALETTE_LEN)
            .map(|slot| {
                let [raw] = sample(TEXTURE_SIDE, palette[slot as usize], |x, y| {
                    let depth = f64::from(DEPTHS[(y * TEXTURE_SIDE + x) as usize]);
                    [if REFERENCE <= depth { 1.0 } else { 0.0 }]
                });
                let scaled = raw * COMPARE_SCALE;
                assert!((scaled - scaled.round()).abs() < 1e-9);
                expected_compare(slot)
            })
            .collect();
        // Linear/wrap, linear/clamp, point/wrap, point/clamp, then anisotropic as linear.
        assert_eq!(compares, [45, 180, 0, 240, 45, 180]);
    }

    /// Neighbouring pixels read different entries and samplers (the handles diverge in every
    /// quad), and the frame reaches every entry and every palette entry.
    #[test]
    fn the_pixel_pattern_diverges_and_covers_the_list_and_palette() {
        let mut seen = vec![false; LIST_LEN];
        let mut samplers = std::collections::BTreeSet::new();
        for y in 0..SIZE {
            for x in 0..SIZE {
                seen[slot(x, y)] = true;
                samplers.insert(sampler(x, y));
                samplers.insert(compare_sampler(x, y));
                if x + 1 < SIZE {
                    assert_ne!(slot(x, y), slot(x + 1, y));
                    assert_ne!(sampler(x, y), sampler(x + 1, y));
                    assert_ne!(compare_sampler(x, y), compare_sampler(x + 1, y));
                }
                if y + 1 < SIZE {
                    assert_ne!(slot(x, y), slot(x, y + 1));
                    assert_ne!(sampler(x, y), sampler(x, y + 1));
                    assert_ne!(compare_sampler(x, y), compare_sampler(x, y + 1));
                }
            }
        }
        assert!(seen.iter().all(|&seen| seen));
        assert_eq!(samplers, (0..PALETTE_LEN).collect());
    }
}
