//! `gbuffer_smoke` (ADR-0023 verification gate 1): the G-buffer encoding on a device,
//! checked against the Rust twin (`crate::gbuffer`) byte for byte.
//!
//! 4096 deterministic surfaces ([`surfaces`]) cover every shading model, every flag
//! combination, every rim slot (and out-of-range ones), normals over the whole sphere (the six
//! axes, the lower hemisphere, non-unit and zero vectors), roughness below both floors, and
//! metallic, AO, base colour and model data outside `[0, 1]` (an anisotropy angle outside
//! `[0, π]` and π exactly). They are uploaded as floats, so the device sees the CPU's exact
//! inputs.
//!
//! 1. **Raster (GLSL):** `gbuffer_smoke.frag` runs `encode_gbuffer` (`include/gbuffer.glsl`,
//!    what the G-buffer prepass's fragment stages call) for each pixel's surface into the
//!    three targets in their real formats. Every read-back texel must equal
//!    [`crate::gbuffer::encode`]'s (`gb1` as its packed `A2B10G10R10` word).
//! 2. **Compute (Slang):** `gbuffer_smoke.slang` runs `lib/gbuffer.slang`'s `encode_gbuffer`
//!    on the same surfaces (each channel's stored integer must equal the twin's) and
//!    `decode_gbuffer` on the texels pass 1 wrote, which must match
//!    [`crate::gbuffer::decode`]: model, flags and rim slot exactly, every float within
//!    [`DECODE_TOLERANCE`] or [`DECODE_ULPS`] (`pow`, `sqrt` and division differ by an ulp or
//!    two between devices; thin-film thicknesses are hundreds of nanometres).
//!
//! **No texel is ambiguous.** A shader and the twin agree on a stored integer unless the value
//! before rounding, `x · (2^b − 1) + 0.5`, lies within a device's float error of an integer
//! (`pow`, `sqrt` and division are not correctly rounded on every device). [`surfaces`]
//! re-draws any surface with such a value within [`MARGIN`] of a step of an integer (counted
//! in a note), so every texel is compared, none excluded. Values that are integers exactly by
//! exact arithmetic (a clamp to 0 or 1, an octahedral coordinate of exactly ½ from a zero
//! component) stay: every device computes them exactly.

use ash::vk;
use ash::vk::TaggedStructure as _;
use bytemuck::{Pod, Zeroable};
use gpu_allocator::MemoryLocation;

use super::bindless::{copy_to_buffer, host_read_barrier};
use super::{Headless, SmokeReport};
use crate::bindless::{color_barrier, submit_one_shot};
use crate::error::{RenderError, RenderResult};
use crate::gbuffer::{
    FLAG_NO_FOG_OF_WAR, FLAG_RESERVED, FLAG_UNIT, GB0_FORMAT, GB1_FORMAT, GB2_FORMAT,
    GBufferTexels, ShadingModel, SurfaceParams, decode, encode, model_channels, octahedral_encode,
    srgb_encode, stored_roughness,
};
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuBuffer, GpuImage, GraphicsPipeline, PipelineLayout,
    ShaderModule,
};

/// The frame's side, in pixels: one surface per pixel.
pub(crate) const SIZE: u32 = 64;
/// Surfaces.
const COUNT: usize = (SIZE * SIZE) as usize;
/// A value before rounding this close (in steps) to an integer is re-drawn.
pub(crate) const MARGIN: f32 = 0.01;
/// Largest allowed difference between the Slang decode and the Rust one, per float.
pub(crate) const DECODE_TOLERANCE: f32 = 1e-5;
/// ... or this many units in the last place, for large values (film thickness).
pub(crate) const DECODE_ULPS: u32 = 4;

/// `SmokeSurface` in `gbuffer_smoke.frag` and `gbuffer_smoke.slang` (std430 / structured
/// buffer, 64 bytes): the upload, and the Slang decode's output.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct SmokeSurface {
    /// rgb linear base colour, a metallic.
    pub(crate) base_metallic: [f32; 4],
    /// xyz normal, w perceptual roughness.
    pub(crate) normal_roughness: [f32; 4],
    /// xyz model data (physical units), w material AO.
    pub(crate) model_data_ao: [f32; 4],
    /// x model, y flags, z rim slot, w unused.
    pub(crate) model_flags_rim: [u32; 4],
}

impl SmokeSurface {
    fn from_params(s: &SurfaceParams) -> Self {
        Self {
            base_metallic: [
                s.base_color[0],
                s.base_color[1],
                s.base_color[2],
                s.metallic,
            ],
            normal_roughness: [s.normal[0], s.normal[1], s.normal[2], s.roughness],
            model_data_ao: [s.model_data[0], s.model_data[1], s.model_data[2], s.ao],
            model_flags_rim: [s.model as u32, s.flags, s.rim_slot, 0],
        }
    }
}

/// `SmokeEncoded` in `gbuffer_smoke.slang` (48 bytes): each channel's stored integer.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub(crate) struct SmokeEncoded {
    pub(crate) gb0: [u32; 4],
    /// The 10/10/10/2 fields.
    pub(crate) gb1: [u32; 4],
    pub(crate) gb2: [u32; 4],
}

impl SmokeEncoded {
    fn of(texels: GBufferTexels) -> Self {
        Self {
            gb0: texels.gb0.map(u32::from),
            gb1: texels.gb1_fields(),
            gb2: texels.gb2.map(u32::from),
        }
    }
}

/// `SmokePush` (8 bytes; the fragment stage reads only `width`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct SmokePush {
    width: u32,
    height: u32,
}

/// `SplitMix64`: a small deterministic generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[low, high)`.
    fn range(&mut self, low: f32, high: f32) -> f32 {
        let unit = (self.next() >> 40) as f32 / (1u64 << 24) as f32;
        low + (high - low) * unit
    }
}

/// How far (in steps) `x · steps + 0.5` lies from the nearest integer after the twin's clamp,
/// or `None` when it is exact by exact arithmetic (a clamp to 0 or 1).
fn distance(x: f32, steps: f32) -> Option<f32> {
    if x.is_nan() || x <= 0.0 || x >= 1.0 {
        return None;
    }
    let v = x * steps + 0.5;
    let fraction = v - v.floor();
    Some(fraction.min(1.0 - fraction))
}

/// The smallest distance to an integer of every value `surface` quantises (see [`distance`]).
pub(crate) fn margin(surface: &SurfaceParams) -> f32 {
    let mut values = Vec::with_capacity(12);
    for channel in surface.base_color {
        values.push((srgb_encode(channel), 255.0));
    }
    values.push((surface.metallic, 255.0));
    let [u, v] = octahedral_encode(surface.normal);
    // A coordinate of exactly ½ comes from an exact zero (0 / sum · 0.5 + 0.5).
    for coordinate in [u, v] {
        if coordinate.to_bits() != 0.5f32.to_bits() {
            values.push((coordinate, 1023.0));
        }
    }
    values.push((stored_roughness(surface.roughness, surface.flags), 1023.0));
    for channel in model_channels(surface.model, surface.model_data, surface.ao) {
        values.push((channel, 255.0));
    }
    values
        .into_iter()
        .filter_map(|(x, steps)| distance(x, steps))
        .fold(f32::INFINITY, f32::min)
}

/// A normal for surface `index`: the six axes and the zero vector first, then a Fibonacci
/// sphere (both hemispheres), every fifth one scaled off unit length.
fn normal(index: usize, rng: &mut Rng) -> [f32; 3] {
    const SPECIAL: [[f32; 3]; 7] = [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
        [0.0, 0.0, 0.0],
    ];
    if let Some(&special) = SPECIAL.get(index) {
        return special;
    }
    let golden = core::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    let jitter = rng.range(0.0, 1.0);
    let y = 1.0 - 2.0 * ((index as f32 + jitter) / COUNT as f32);
    let radius = (1.0 - y * y).max(0.0).sqrt();
    let theta = golden * index as f32 + rng.range(0.0, 0.01);
    let scale = if index.is_multiple_of(5) {
        rng.range(0.25, 3.0)
    } else {
        1.0
    };
    [
        radius * theta.cos() * scale,
        y * scale,
        radius * theta.sin() * scale,
    ]
}

/// Model data for `model` in physical units, out of range about one time in five.
fn model_data(model: ShadingModel, index: usize, rng: &mut Rng) -> [f32; 3] {
    let wide = index.is_multiple_of(5);
    let unit = |rng: &mut Rng| {
        if wide {
            rng.range(-0.3, 1.3)
        } else {
            rng.range(0.0, 1.0)
        }
    };
    match model {
        ShadingModel::Unlit => [unit(rng), unit(rng), unit(rng)],
        ShadingModel::Standard | ShadingModel::TransmissiveFallback => {
            [rng.range(-0.01, 0.2), 0.0, 0.0]
        }
        ShadingModel::Clearcoat | ShadingModel::Sheen | ShadingModel::Foliage => {
            [unit(rng), unit(rng), unit(rng)]
        }
        ShadingModel::Anisotropic => {
            let angle = match index % 7 {
                0 => core::f32::consts::PI,
                1 => 0.0,
                2 => rng.range(-7.0, 0.0),
                3 => rng.range(3.2, 10.0),
                _ => rng.range(0.0, core::f32::consts::PI),
            };
            [unit(rng), angle, 0.0]
        }
        ShadingModel::ThinFilm => [
            rng.range(
                if wide { 50.0 } else { 100.0 },
                if wide { 450.0 } else { 400.0 },
            ),
            rng.range(if wide { 0.8 } else { 1.0 }, if wide { 2.3 } else { 2.0 }),
            0.0,
        ],
    }
}

/// Draws surface `index` with `rng`.
fn draw(index: usize, rng: &mut Rng) -> SurfaceParams {
    let model = ShadingModel::ALL[index % ShadingModel::ALL.len()];
    let flag_bits = (index / 8) % 8;
    let flags = [FLAG_UNIT, FLAG_RESERVED, FLAG_NO_FOG_OF_WAR]
        .iter()
        .enumerate()
        .filter(|(bit, _)| flag_bits & (1 << bit) != 0)
        .fold(0, |flags, (_, flag)| flags | flag);
    // Slots 0–7, and 8–15 one time in eleven (only bits 0–2 are stored).
    let rim_slot = ((index / 64) % 8 + if index.is_multiple_of(11) { 8 } else { 0 }) as u32;
    let wide = index.is_multiple_of(3);
    let color = |rng: &mut Rng| {
        if wide {
            rng.range(-0.1, 1.2)
        } else {
            // Squared: most of the range near black, where sRGB spends its codes.
            let x = rng.range(0.0, 1.0);
            x * x * x
        }
    };
    SurfaceParams {
        base_color: [color(rng), color(rng), color(rng)],
        metallic: if wide {
            rng.range(-0.2, 1.2)
        } else {
            rng.range(0.0, 1.0)
        },
        normal: normal(index, rng),
        roughness: match index % 13 {
            // Below both floors (0.045, and 0.08 on units).
            0 => rng.range(-0.1, 0.045),
            1 => rng.range(0.045, 0.08),
            2 => rng.range(1.0, 1.5),
            _ => rng.range(0.0, 1.0),
        },
        model,
        model_data: model_data(model, index, rng),
        ao: if wide {
            rng.range(-0.2, 1.2)
        } else {
            rng.range(0.0, 1.0)
        },
        flags,
        rim_slot,
    }
}

/// The smoke's surfaces, and how many were re-drawn because a value fell within [`MARGIN`]
/// of a rounding boundary.
pub(crate) fn surfaces() -> (Vec<SurfaceParams>, usize) {
    let mut rng = Rng(0x6275_6666_6572_0023);
    let mut redrawn = 0;
    let surfaces = (0..COUNT)
        .map(|index| {
            loop {
                let surface = draw(index, &mut rng);
                if margin(&surface) >= MARGIN {
                    break surface;
                }
                redrawn += 1;
            }
        })
        .collect();
    (surfaces, redrawn)
}

/// Units in the last place between `a` and `b` (`u32::MAX` across signs or for NaN).
fn ulps(a: f32, b: f32) -> u32 {
    if a.to_bits() == b.to_bits() || (a == 0.0 && b == 0.0) {
        0
    } else if a.is_nan() || b.is_nan() || a.is_sign_negative() != b.is_sign_negative() {
        u32::MAX
    } else {
        a.to_bits().abs_diff(b.to_bits())
    }
}

/// Whether two decoded floats agree within [`DECODE_TOLERANCE`] or [`DECODE_ULPS`].
fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= DECODE_TOLERANCE || ulps(a, b) <= DECODE_ULPS
}

/// The largest absolute difference between two decoded surfaces' floats, whether every float
/// is [`close`], and whether their integers (model, flags, rim slot) match.
fn decode_difference(a: &SmokeSurface, b: &SmokeSurface) -> (f32, bool, bool) {
    let floats = |s: &SmokeSurface| {
        let mut all = [0.0f32; 12];
        all[..4].copy_from_slice(&s.base_metallic);
        all[4..8].copy_from_slice(&s.normal_roughness);
        all[8..].copy_from_slice(&s.model_data_ao);
        all
    };
    let difference = floats(a)
        .iter()
        .zip(floats(b))
        .map(|(x, y)| {
            if x.to_bits() == y.to_bits() {
                0.0
            } else {
                (x - y).abs()
            }
        })
        .fold(0.0f32, |worst, d| {
            if d.is_nan() {
                f32::INFINITY
            } else {
                worst.max(d)
            }
        });
    let all_close = floats(a).iter().zip(floats(b)).all(|(&x, y)| close(x, y));
    (
        difference,
        all_close,
        a.model_flags_rim == b.model_flags_rim,
    )
}

fn buffer_write(
    set: vk::DescriptorSet,
    binding: u32,
    info: &[vk::DescriptorBufferInfo; 1],
) -> vk::WriteDescriptorSet<'_> {
    vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(binding)
        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
        .buffer_info(info)
}

fn image_write(
    set: vk::DescriptorSet,
    binding: u32,
    info: &[vk::DescriptorImageInfo; 1],
) -> vk::WriteDescriptorSet<'_> {
    vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(binding)
        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
        .image_info(info)
}

fn buffer(
    gpu: &Headless,
    name: &str,
    size: usize,
    usage: vk::BufferUsageFlags,
    location: MemoryLocation,
) -> RenderResult<GpuBuffer> {
    GpuBuffer::new(
        gpu.device.device.clone(),
        gpu.device.allocator(),
        name,
        size as u64,
        usage,
        location,
    )
}

fn read<T: Pod + Zeroable + Clone>(buffer: &GpuBuffer, count: usize) -> RenderResult<Vec<T>> {
    let mut values = vec![T::zeroed(); count];
    if !buffer.read_bytes(0, bytemuck::cast_slice_mut(&mut values)) {
        return Err(RenderError::Shader("smoke readback is not host visible"));
    }
    Ok(values)
}

/// The raster pipeline: `fullscreen.vert` + `gbuffer_smoke.frag` into the three formats.
fn raster_pipeline(
    gpu: &Headless,
    layout: vk::PipelineLayout,
    vertex: vk::ShaderModule,
    fragment: vk::ShaderModule,
) -> RenderResult<GraphicsPipeline> {
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vertex)
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fragment)
            .name(c"main"),
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let attachment = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA);
    let attachments = [attachment; 3];
    let color_blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    let formats = [GB0_FORMAT, GB1_FORMAT, GB2_FORMAT];
    let mut rendering =
        vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&formats);
    let create_info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .color_blend_state(&color_blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push(&mut rendering);
    Ok(GraphicsPipeline::from_raw(
        gpu.device.device.clone(),
        gpu.device.create_graphics_pipeline(&create_info)?,
    ))
}

#[allow(clippy::too_many_lines)] // One linear setup, record and check.
pub(super) fn run(gpu: &Headless, report: &mut SmokeReport) -> RenderResult<()> {
    let device = &gpu.device;
    let d = &device.device;
    let (params, redrawn) = surfaces();
    let upload: Vec<SmokeSurface> = params.iter().map(SmokeSurface::from_params).collect();
    let expected: Vec<GBufferTexels> = params.iter().map(encode).collect();

    // Buffers.
    let storage = vk::BufferUsageFlags::STORAGE_BUFFER;
    let mut input = buffer(
        gpu,
        "gbuffer-smoke-surfaces",
        COUNT * size_of::<SmokeSurface>(),
        storage,
        MemoryLocation::CpuToGpu,
    )?;
    input.write_bytes(0, bytemuck::cast_slice(&upload));
    let encoded = buffer(
        gpu,
        "gbuffer-smoke-encoded",
        COUNT * size_of::<SmokeEncoded>(),
        storage,
        MemoryLocation::GpuToCpu,
    )?;
    let decoded = buffer(
        gpu,
        "gbuffer-smoke-decoded",
        COUNT * size_of::<SmokeSurface>(),
        storage,
        MemoryLocation::GpuToCpu,
    )?;
    let readbacks = [
        buffer(
            gpu,
            "gbuffer-smoke-gb0",
            COUNT * 4,
            vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuToCpu,
        )?,
        buffer(
            gpu,
            "gbuffer-smoke-gb1",
            COUNT * 4,
            vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuToCpu,
        )?,
        buffer(
            gpu,
            "gbuffer-smoke-gb2",
            COUNT * 4,
            vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuToCpu,
        )?,
    ];

    // The three targets.
    let extent = vk::Extent2D {
        width: SIZE,
        height: SIZE,
    };
    let target = |name, format| {
        GpuImage::new(
            d.clone(),
            device.allocator(),
            name,
            extent,
            format,
            vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC,
            vk::ImageAspectFlags::COLOR,
        )
    };
    let targets = [
        target("gbuffer-smoke-gb0", GB0_FORMAT)?,
        target("gbuffer-smoke-gb1", GB1_FORMAT)?,
        target("gbuffer-smoke-gb2", GB2_FORMAT)?,
    ];

    // Descriptor sets: set 0 of the raster pass (the surfaces), set 0 of the compute pass.
    let binding = |index: u32, ty: vk::DescriptorType, stage: vk::ShaderStageFlags| {
        vk::DescriptorSetLayoutBinding::default()
            .binding(index)
            .descriptor_type(ty)
            .descriptor_count(1)
            .stage_flags(stage)
    };
    let fragment_stage = vk::ShaderStageFlags::FRAGMENT;
    let compute_stage = vk::ShaderStageFlags::COMPUTE;
    let storage_ty = vk::DescriptorType::STORAGE_BUFFER;
    let sampled_ty = vk::DescriptorType::SAMPLED_IMAGE;
    let raster_layout =
        DescriptorSetLayout::new(d.clone(), &[binding(0, storage_ty, fragment_stage)])?;
    let compute_layout = DescriptorSetLayout::new(
        d.clone(),
        &[
            binding(0, storage_ty, compute_stage),
            binding(1, storage_ty, compute_stage),
            binding(2, sampled_ty, compute_stage),
            binding(3, sampled_ty, compute_stage),
            binding(4, sampled_ty, compute_stage),
            binding(5, storage_ty, compute_stage),
        ],
    )?;
    let pool = DescriptorPool::new(
        d.clone(),
        &[
            vk::DescriptorPoolSize {
                ty: storage_ty,
                descriptor_count: 4,
            },
            vk::DescriptorPoolSize {
                ty: sampled_ty,
                descriptor_count: 3,
            },
        ],
        2,
    )?;
    let sets = pool.allocate(d, &[raster_layout.handle(), compute_layout.handle()])?;
    let (raster_set, compute_set) = (sets[0], sets[1]);
    let whole = |buffer: &GpuBuffer| {
        [vk::DescriptorBufferInfo::default()
            .buffer(buffer.handle())
            .range(vk::WHOLE_SIZE)]
    };
    let (input_info, encoded_info, decoded_info) =
        (whole(&input), whole(&encoded), whole(&decoded));
    let image_infos = targets.each_ref().map(|image| {
        [vk::DescriptorImageInfo::default()
            .image_view(image.view())
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
    });
    let writes = [
        buffer_write(raster_set, 0, &input_info),
        buffer_write(compute_set, 0, &input_info),
        buffer_write(compute_set, 1, &encoded_info),
        image_write(compute_set, 2, &image_infos[0]),
        image_write(compute_set, 3, &image_infos[1]),
        image_write(compute_set, 4, &image_infos[2]),
        buffer_write(compute_set, 5, &decoded_info),
    ];
    // SAFETY: both sets are live and unused; every write names a live buffer or view.
    unsafe { d.update_descriptor_sets(&writes, &[]) };

    // Pipelines.
    let push = SmokePush {
        width: SIZE,
        height: SIZE,
    };
    let push_bytes = u32::try_from(size_of::<SmokePush>()).expect("small");
    let raster_push = vk::PushConstantRange {
        stage_flags: fragment_stage,
        offset: 0,
        size: 4,
    };
    let compute_push = vk::PushConstantRange {
        stage_flags: compute_stage,
        offset: 0,
        size: push_bytes,
    };
    let raster_pipeline_layout = PipelineLayout::with_push_constants(
        d.clone(),
        &[raster_layout.handle()],
        core::slice::from_ref(&raster_push),
    )?;
    let compute_pipeline_layout = PipelineLayout::with_push_constants(
        d.clone(),
        &[compute_layout.handle()],
        core::slice::from_ref(&compute_push),
    )?;
    let vertex = ShaderModule::new(d.clone(), moba_shaders::FULLSCREEN_VERT)?;
    let fragment = ShaderModule::new(d.clone(), moba_shaders::GBUFFER_SMOKE_FRAG)?;
    let compute = ShaderModule::new(d.clone(), moba_shaders::GBUFFER_SMOKE_SLANG)?;
    let raster = raster_pipeline(
        gpu,
        raster_pipeline_layout.handle(),
        vertex.handle(),
        fragment.handle(),
    )?;
    let stage = vk::PipelineShaderStageCreateInfo::default()
        .stage(compute_stage)
        .module(compute.handle())
        .name(c"main");
    let compute_pipeline = GraphicsPipeline::from_raw(
        d.clone(),
        device.create_compute_pipeline(
            &vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(compute_pipeline_layout.handle()),
        )?,
    );

    submit_one_shot(device, |command| {
        let to_targets = targets.each_ref().map(|image| {
            color_barrier(
                image.image(),
                (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                (
                    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                ),
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            )
        });
        device.cmd_pipeline_barrier2(
            command,
            &vk::DependencyInfo::default().image_memory_barriers(&to_targets),
        );
        let attachments = targets.each_ref().map(|image| {
            vk::RenderingAttachmentInfo::default()
                .image_view(image.view())
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .clear_value(vk::ClearValue {
                    color: vk::ClearColorValue { float32: [0.0; 4] },
                })
        });
        let area = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent,
        };
        let rendering = vk::RenderingInfo::default()
            .render_area(area)
            .layer_count(1)
            .color_attachments(&attachments);
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: SIZE as f32,
            height: SIZE as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        device.cmd_begin_rendering(command, &rendering);
        // SAFETY: recording inside dynamic rendering; the pipeline is live, its viewport and
        // scissor are dynamic, the set matches set 0 of its layout and the push range is the
        // fragment stage's first 4 bytes (the blocks below rely on the same).
        unsafe { d.cmd_bind_pipeline(command, vk::PipelineBindPoint::GRAPHICS, raster.handle()) };
        // SAFETY: as above.
        unsafe { d.cmd_set_viewport(command, 0, core::slice::from_ref(&viewport)) };
        // SAFETY: as above.
        unsafe { d.cmd_set_scissor(command, 0, core::slice::from_ref(&area)) };
        // SAFETY: as above.
        unsafe {
            d.cmd_bind_descriptor_sets(
                command,
                vk::PipelineBindPoint::GRAPHICS,
                raster_pipeline_layout.handle(),
                0,
                &[raster_set],
                &[],
            );
        }
        // SAFETY: as above.
        unsafe {
            d.cmd_push_constants(
                command,
                raster_pipeline_layout.handle(),
                fragment_stage,
                0,
                &bytemuck::bytes_of(&push)[..4],
            );
        }
        // SAFETY: a fullscreen triangle with no vertex input: one fragment per surface.
        unsafe { d.cmd_draw(command, 3, 1, 0, 0) };
        device.cmd_end_rendering(command);
        let to_sampled = targets.each_ref().map(|image| {
            color_barrier(
                image.image(),
                (
                    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                ),
                (
                    vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::SHADER_SAMPLED_READ,
                ),
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            )
        });
        device.cmd_pipeline_barrier2(
            command,
            &vk::DependencyInfo::default().image_memory_barriers(&to_sampled),
        );
        // SAFETY: the compute pipeline is live, the set matches set 0 of its
        // layout (the targets now sampled read-only) and the push range is the compute
        // stage's 8 bytes.
        unsafe {
            d.cmd_bind_pipeline(
                command,
                vk::PipelineBindPoint::COMPUTE,
                compute_pipeline.handle(),
            );
        }
        // SAFETY: as above.
        unsafe {
            d.cmd_bind_descriptor_sets(
                command,
                vk::PipelineBindPoint::COMPUTE,
                compute_pipeline_layout.handle(),
                0,
                &[compute_set],
                &[],
            );
        }
        // SAFETY: as above.
        unsafe {
            d.cmd_push_constants(
                command,
                compute_pipeline_layout.handle(),
                compute_stage,
                0,
                bytemuck::bytes_of(&push),
            );
        }
        // SAFETY: the dispatch covers the 64×64 surfaces in 8×8 groups.
        unsafe { d.cmd_dispatch(command, SIZE / 8, SIZE / 8, 1) };
        let to_copy = targets.each_ref().map(|image| {
            color_barrier(
                image.image(),
                (
                    vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::SHADER_SAMPLED_READ,
                ),
                (
                    vk::PipelineStageFlags2::COPY,
                    vk::AccessFlags2::TRANSFER_READ,
                ),
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            )
        });
        // The compute pass's buffer writes, for the host after the fence.
        let to_host = vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
            .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ);
        device.cmd_pipeline_barrier2(
            command,
            &vk::DependencyInfo::default()
                .memory_barriers(core::slice::from_ref(&to_host))
                .image_memory_barriers(&to_copy),
        );
        for (image, readback) in targets.iter().zip(&readbacks) {
            copy_to_buffer(device, command, image, readback, extent);
        }
        host_read_barrier(device, command);
    })?;

    // 1. The GLSL raster encode, byte for byte.
    let gb0: Vec<[u8; 4]> = read(&readbacks[0], COUNT)?;
    let gb1: Vec<u32> = read(&readbacks[1], COUNT)?;
    let gb2: Vec<[u8; 4]> = read(&readbacks[2], COUNT)?;
    let stored: Vec<GBufferTexels> = (0..COUNT)
        .map(|index| GBufferTexels {
            gb0: gb0[index],
            gb1: gb1[index],
            gb2: gb2[index],
        })
        .collect();
    for (index, (actual, want)) in stored.iter().zip(&expected).enumerate() {
        report.check(
            || format!("GLSL encode, surface {index} ({:?})", params[index]),
            *actual,
            *want,
        );
    }

    // 2. The Slang encode, each channel's integer, and the Slang decode of the stored texels.
    let slang_encoded: Vec<SmokeEncoded> = read(&encoded, COUNT)?;
    for (index, (actual, want)) in slang_encoded.iter().zip(&expected).enumerate() {
        report.check(
            || format!("Slang encode, surface {index} ({:?})", params[index]),
            *actual,
            SmokeEncoded::of(*want),
        );
    }
    let slang_decoded: Vec<SmokeSurface> = read(&decoded, COUNT)?;
    let mut worst = 0.0f32;
    for (index, (actual, texels)) in slang_decoded.iter().zip(&stored).enumerate() {
        let Some(want) = decode(*texels) else {
            report.check(
                || format!("surface {index} stored a reserved model"),
                true,
                false,
            );
            continue;
        };
        let want = SmokeSurface::from_params(&want);
        let (difference, floats_close, integers) = decode_difference(actual, &want);
        worst = worst.max(difference);
        report.check(
            || format!("Slang decode, surface {index}: {actual:?} vs {want:?}"),
            integers && floats_close,
            true,
        );
    }

    let models = ShadingModel::ALL.len();
    report.notes.push(format!(
        "{COUNT} surfaces over {models} models, 8 flag combinations and rim slots 0-15; \
         {redrawn} re-drawn for a value within {MARGIN} of a step of a rounding boundary"
    ));
    report.notes.push(format!(
        "GLSL raster encode ({GB0_FORMAT:?}, {GB1_FORMAT:?}, {GB2_FORMAT:?}) and Slang encode \
         compared byte for byte; Slang decode within {DECODE_TOLERANCE} or {DECODE_ULPS} ulps \
         (largest absolute difference {worst:e})"
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_floats_are_close_within_an_absolute_or_ulp_bound() {
        assert!(close(0.5, 0.5 + 0.9e-5));
        assert!(!close(0.5, 0.5 + 2e-5));
        assert!(close(354.117_65, 354.117_68)); // one ulp at 256-512 is about 3e-5
        assert!(!close(354.117_65, 354.118));
        assert!(!close(f32::NAN, 0.0));
        assert_eq!(ulps(-0.0, 0.0), 0);
    }

    /// The surfaces cover every model, flag combination and rim slot, the six axes and both
    /// hemispheres, and inputs outside every range; none is near a rounding boundary.
    #[test]
    fn the_surfaces_cover_the_encoding_and_are_unambiguous() {
        let (surfaces, redrawn) = surfaces();
        assert_eq!(surfaces.len(), COUNT);
        assert!(redrawn < COUNT / 4, "{redrawn} re-drawn");
        for model in ShadingModel::ALL {
            for flags in 0..8 {
                assert!(
                    surfaces
                        .iter()
                        .any(|s| s.model == model && s.flags == flags),
                    "{model:?} with flags {flags}"
                );
            }
        }
        for slot in 0..16 {
            assert!(
                surfaces.iter().any(|s| s.rim_slot == slot),
                "rim slot {slot}"
            );
        }
        assert!(surfaces.iter().any(|s| s.normal[1] < -0.9));
        assert!(
            surfaces
                .iter()
                .any(|s| s.normal.iter().all(|c| c.to_bits() == 0))
        );
        assert!(surfaces.iter().any(|s| s.roughness < 0.045));
        assert!(
            surfaces
                .iter()
                .any(|s| s.metallic < 0.0 || s.metallic > 1.0)
        );
        assert!(
            surfaces
                .iter()
                .any(|s| s.base_color.iter().any(|&c| c < 0.0))
        );
        let angles: Vec<f32> = surfaces
            .iter()
            .filter(|s| s.model == ShadingModel::Anisotropic)
            .map(|s| s.model_data[1])
            .collect();
        assert!(angles.contains(&core::f32::consts::PI));
        assert!(angles.iter().any(|&a| a < 0.0));
        assert!(angles.iter().any(|&a| a > core::f32::consts::PI));
        for surface in &surfaces {
            assert!(margin(surface) >= MARGIN, "{surface:?}");
        }
        // Every model's encoding is decodable, and differs between surfaces.
        let texels: std::collections::BTreeSet<_> = surfaces
            .iter()
            .map(|s| {
                let t = encode(s);
                assert!(decode(t).is_some());
                (t.gb0, t.gb1, t.gb2)
            })
            .collect();
        assert!(texels.len() > COUNT * 99 / 100, "{} distinct", texels.len());
    }

    /// The Rust mirrors match the Slang structures' reflected layout field for field.
    #[test]
    fn mirrors_match_the_slang_layout() {
        let json = moba_shaders::reflection("gbuffer_smoke.slang").expect("smoke reflection");
        let reflection: serde_json::Value =
            serde_json::from_str(json).expect("slangc reflection JSON");
        let parameter = |name: &str| {
            reflection["parameters"]
                .as_array()
                .expect("parameters")
                .iter()
                .find(|parameter| parameter["name"] == name)
                .unwrap_or_else(|| panic!("`{name}` is reflected"))
                .clone()
        };
        let fields = |ty: &serde_json::Value| -> Vec<(String, u64, u64)> {
            ty["fields"]
                .as_array()
                .expect("fields")
                .iter()
                .map(|field| {
                    (
                        field["name"].as_str().expect("name").to_owned(),
                        field["binding"]["offset"].as_u64().expect("offset"),
                        field["binding"]["size"].as_u64().expect("size"),
                    )
                })
                .collect()
        };
        let surface = vec![
            (
                "base_metallic".to_owned(),
                core::mem::offset_of!(SmokeSurface, base_metallic) as u64,
                16,
            ),
            (
                "normal_roughness".to_owned(),
                core::mem::offset_of!(SmokeSurface, normal_roughness) as u64,
                16,
            ),
            (
                "model_data_ao".to_owned(),
                core::mem::offset_of!(SmokeSurface, model_data_ao) as u64,
                16,
            ),
            (
                "model_flags_rim".to_owned(),
                core::mem::offset_of!(SmokeSurface, model_flags_rim) as u64,
                16,
            ),
        ];
        assert_eq!(size_of::<SmokeSurface>(), 64);
        for name in ["surfaces", "decoded"] {
            let ty = &parameter(name)["type"]["resultType"];
            assert_eq!(ty["name"], "SmokeSurface");
            assert_eq!(fields(ty), surface, "{name}");
        }
        let encoded = vec![
            (
                "gb0".to_owned(),
                core::mem::offset_of!(SmokeEncoded, gb0) as u64,
                16,
            ),
            (
                "gb1".to_owned(),
                core::mem::offset_of!(SmokeEncoded, gb1) as u64,
                16,
            ),
            (
                "gb2".to_owned(),
                core::mem::offset_of!(SmokeEncoded, gb2) as u64,
                16,
            ),
        ];
        assert_eq!(size_of::<SmokeEncoded>(), 48);
        assert_eq!(fields(&parameter("encoded")["type"]["resultType"]), encoded);
        let push = &parameter("push")["type"]["elementType"];
        assert_eq!(
            fields(push),
            vec![
                (
                    "width".to_owned(),
                    core::mem::offset_of!(SmokePush, width) as u64,
                    4
                ),
                (
                    "height".to_owned(),
                    core::mem::offset_of!(SmokePush, height) as u64,
                    4
                ),
            ]
        );
    }
}
