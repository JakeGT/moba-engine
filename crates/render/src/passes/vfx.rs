//! GPU particle passes (ADR-0013): `vfx_simulate` (compute), `vfx_draw` (particles into the
//! HDR target, depth-tested read-only against the scene depth and soft-faded by sampling
//! it) and, where the tier enables weighted blended OIT, `vfx_resolve`.
//!
//! **Buffers.** Every buffer is allocated once, at creation, from the tier's
//! [`VfxBudget`]:
//!
//! - `particles`: `max_particles` × [`GpuParticle`] (96 bytes), device local, persistent
//!   across frames. A slot with `lifetime <= 0` is free.
//! - `alive`: the compacted draw lists, rebuilt every frame: sprites in `[0, capacity)`,
//!   then four mesh-slot regions of `capacity / 8` each, then the heat-haze (distortion)
//!   list of `capacity / 8` (ADR-0019).
//! - `args`: 40 words: one `VkDrawIndirectCommand` (sprites), four
//!   `VkDrawIndexedIndirectCommand`s (mesh slots), five raw list counters, three stats
//!   (projected pixels, live particles, spawns), then the distortion list's
//!   `VkDrawIndirectCommand` and counter. The layout is `include/vfx.glsl`'s.
//!
//! **Distortion** (ADR-0019, High and up): `distortion` draws the distortion list with its
//! own pipeline layout (the frame set plus the HDR copy, the scene depth and the unit ID
//! target), refracting the copy the `distortion_copy` pass made. When no distortion particle
//! can be live (the CPU knows every lifetime), the renderer records neither pass.
//! - per frame in flight: the uniform block, the spawn upload (`max_spawns_per_frame`
//!   records, host-visible) and a 128-byte host-readable copy of `args` for [`VfxStats`].
//!
//! **Allocation.** The CPU hands each spawn a free slot: it keeps every slot's expiry time
//! on its own clock (the sum of the frames' `dt`, as the GPU ages particles) and scans at
//! most [`SLOT_SCAN`] slots from a ring cursor. Particles are spawned roughly in time order,
//! so the scan finds a free slot at once until the buffer is nearly full; past that the spawn
//! is dropped and counted, which is the hard particle cap.
//!
//! **Simulation.** Three dispatches with compute → compute barriers between them (inside the
//! pass; the render graph orders the pass against everything else): copy spawns into their
//! slots and reset the counters; age, integrate, kill and append every live slot to its
//! draw list; write the indirect arguments clamped to each list's capacity. The pass then
//! copies `args` to this frame's readback buffer.

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};
use gpu_allocator::MemoryLocation;
use moba_core::math::{Mat4, Vec3, Vec4};

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::mesh::Vertex;
use crate::passes::scene::ScenePass;
use crate::resources::{
    CommandPool, DescriptorPool, DescriptorSetLayout, GpuBuffer, GpuImage, GraphicsPipeline,
    PipelineLayout, Sampler,
};
use crate::scene::{MeshId, Scene};
use crate::shaders::ShaderSource;
use crate::vfx::{
    ParticleBlend, ParticleRole, ParticleShape, ParticleSpawn, VFX_MESH_SLOTS, VfxBudget, VfxStats,
};

const MODE_SPAWN: u32 = 0;
const MODE_SIMULATE: u32 = 1;
const MODE_FINALIZE: u32 = 2;
const WORKGROUP: u32 = 64;
/// Words in the args buffer (see the module docs and `include/vfx.glsl`).
const ARGS_WORDS: usize = 40;
/// The distortion list's `VkDrawIndirectCommand` (ADR-0019).
const ARGS_DISTORT: usize = 32;
const ARGS_BYTES: u64 = (ARGS_WORDS * 4) as u64;
const ARGS_MESH: usize = 4;
const STAT_PIXELS: usize = 29;
const STAT_ALIVE: usize = 30;
const STAT_SPAWNED: usize = 31;
/// Slots the allocator inspects per spawn before it drops the spawn.
pub(crate) const SLOT_SCAN: usize = 64;
/// Extra seconds a slot stays reserved after its particle's lifetime (GPU and CPU sum the
/// frame times in different precision).
const EXPIRY_MARGIN: f64 = 0.1;
/// Longest frame step the simulation integrates, seconds (a hitch does not teleport).
const MAX_DT: f32 = 0.1;
/// Soft-particle fade distance, metres.
const SOFT_DISTANCE: f32 = 0.6;
/// Atlas cell drawn for mesh particles where meshes are unavailable (the `shard` sprite).
const MESH_FALLBACK_CELL: u8 = 5;
/// The particle flag of a heat-haze particle (`VFX_FLAG_DISTORT`).
const FLAG_DISTORT: u32 = 256;
const ACCUM_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
const REVEAL_FORMAT: vk::Format = vk::Format::R16_SFLOAT;

/// One particle on the GPU (std430, 96 bytes). Must match `Particle` in `include/vfx.glsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuParticle {
    pos_age: [f32; 4],
    vel_life: [f32; 4],
    seg_rot: [f32; 4],
    size_spin: [f32; 4],
    colors: [u32; 4],
    misc: [u32; 4],
}

/// The per-frame block (std140, 192 bytes). Must match `VfxFrame` in `include/vfx.glsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct VfxUniform {
    view_proj: [[f32; 4]; 4],
    camera_right: [f32; 4],
    camera_up: [f32; 4],
    eye: [f32; 4],
    viewport: [f32; 4],
    depth: [f32; 4],
    light: [f32; 4],
    counts: [u32; 4],
    mesh_indices: [u32; 4],
}

/// Push constants of the compute and mesh shaders (8 bytes, compute + vertex).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct VfxPush {
    mode: u32,
    slot: u32,
}
const PUSH_SIZE: u32 = core::mem::size_of::<VfxPush>() as u32;

/// Push constants of the distortion pipeline (48 bytes, vertex + fragment). Must match
/// `include/distortion.glsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct DistortionPush {
    /// Last pixel of the render sub-rectangle (x, y), the offset cap (pixels), 1 = unit mask.
    rect: [f32; 4],
    /// Soft distance (m), view-distance fade start and end (m), unused.
    fade: [f32; 4],
    /// First index of the distortion list in `alive`.
    list: [u32; 4],
}
const DISTORTION_PUSH_SIZE: u32 = core::mem::size_of::<DistortionPush>() as u32;
const DISTORTION_PUSH_STAGES: vk::ShaderStageFlags = vk::ShaderStageFlags::from_raw(
    vk::ShaderStageFlags::VERTEX.as_raw() | vk::ShaderStageFlags::FRAGMENT.as_raw(),
);
const PUSH_STAGES: vk::ShaderStageFlags = vk::ShaderStageFlags::from_raw(
    vk::ShaderStageFlags::COMPUTE.as_raw() | vk::ShaderStageFlags::VERTEX.as_raw(),
);

/// IEEE half-precision bits of `value` (round to nearest, saturating at ±65504).
fn f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let magnitude = value.abs();
    if magnitude.is_nan() {
        return sign | 0x7e00;
    }
    if magnitude >= 65_504.0 {
        return sign | 0x7bff;
    }
    if magnitude < 6.103_515_6e-5 {
        // Subnormal half: a multiple of 2^-24.
        return sign | (magnitude * 16_777_216.0).round() as u16;
    }
    // Normal here: the biased f32 exponent is at least 113, rebiased from 127 to 15.
    let exponent = (magnitude.to_bits() >> 23) - 112;
    let mantissa = magnitude.to_bits() & 0x007f_ffff;
    let mut half = (exponent << 10) | (mantissa >> 13);
    if mantissa & 0x1000 != 0 {
        half += 1; // A carry into the exponent is still the correctly rounded value.
    }
    sign | half.min(0x7bff) as u16
}

/// `packHalf2x16(vec2(low, high))`.
fn pack_half2(low: f32, high: f32) -> u32 {
    u32::from(f16_bits(low)) | (u32::from(f16_bits(high)) << 16)
}

impl GpuParticle {
    /// Encodes `spawn` for `slot`. `mesh_ok` says whether its mesh slot can draw a mesh.
    pub(crate) fn encode(spawn: &ParticleSpawn, slot: u32, mesh_ok: bool) -> Self {
        let finite = |value: f32, fallback: f32| if value.is_finite() { value } else { fallback };
        let finite3 = |value: Vec3| {
            if value.is_finite() { value } else { Vec3::ZERO }
        };
        let color = |value: Vec4| {
            let value = if value.is_finite() { value } else { Vec4::ZERO };
            value.clamp(Vec4::ZERO, Vec4::new(1_024.0, 1_024.0, 1_024.0, 1.0))
        };
        let (mut shape, mut cell, mut frames) = (spawn.shape, spawn.cell.min(63), spawn.frames);
        let mesh_slot = u32::from(spawn.mesh_slot.min(VFX_MESH_SLOTS as u8 - 1));
        let distortion = spawn.blend == ParticleBlend::Distortion;
        if distortion {
            // Heat haze is always an upright billboard (ADR-0019).
            shape = ParticleShape::Billboard;
        } else if shape == ParticleShape::Mesh && !mesh_ok {
            shape = ParticleShape::Billboard;
            cell = MESH_FALLBACK_CELL;
            frames = 1;
        }
        let shape_bits = match shape {
            ParticleShape::Segment => 1,
            ParticleShape::Ground => 2,
            ParticleShape::Mesh => 3,
            // `ParticleShape` is non-exhaustive in other crates; this crate knows them all.
            _ => 0,
        };
        let blend_bits = match spawn.blend {
            ParticleBlend::Alpha => 4,
            ParticleBlend::Distortion => FLAG_DISTORT,
            _ => 0,
        };
        let role_bits = match spawn.role {
            ParticleRole::Ally => 1 << 3,
            ParticleRole::EnemyTelegraph => 2 << 3,
            _ => 0,
        };
        let flags = shape_bits | blend_bits | role_bits | (mesh_slot << 5);
        let frames = u32::from(frames.clamp(1, 64 - cell));
        let fps = (finite(spawn.fps, 0.0).clamp(0.0, 1_000.0) * 8.0) as u32;
        let sprite = u32::from(cell) | (frames << 8) | (fps.min(0xffff) << 16);
        let position = finite3(spawn.position);
        let (start, end) = (color(spawn.color), color(spawn.color_end));
        Self {
            pos_age: [position.x, position.y, position.z, 0.0],
            vel_life: {
                let velocity = finite3(spawn.velocity).clamp_length_max(1_000.0);
                [
                    velocity.x,
                    velocity.y,
                    velocity.z,
                    finite(spawn.lifetime, 0.01).clamp(0.01, 10.0),
                ]
            },
            seg_rot: {
                let segment = finite3(spawn.segment).clamp_length_max(200.0);
                [segment.x, segment.y, segment.z, finite(spawn.rotation, 0.0)]
            },
            size_spin: [
                finite(spawn.size, 0.0).clamp(0.0, 200.0),
                finite(spawn.size_end, 0.0).clamp(0.0, 200.0),
                finite(spawn.spin, 0.0).clamp(-100.0, 100.0),
                finite(spawn.drag, 0.0).clamp(0.0, 100.0),
            ],
            colors: [
                pack_half2(start.x, start.y),
                pack_half2(start.z, start.w),
                pack_half2(end.x, end.y),
                pack_half2(end.z, end.w),
            ],
            misc: [
                flags,
                sprite,
                finite(spawn.gravity, 0.0).clamp(-500.0, 500.0).to_bits(),
                slot,
            ],
        }
    }

    /// Seconds the encoded particle lives.
    pub(crate) fn lifetime(&self) -> f32 {
        self.vel_life[3]
    }
}

/// `(P22, P23)` of the perspective projection inside `view_proj`, so that linear view depth
/// is `P23 / (d + P22)` for a depth-buffer value `d`; `None` for a non-perspective matrix.
pub(crate) fn depth_params(view_proj: Mat4) -> Option<(f32, f32)> {
    let r2 = view_proj.row(2);
    let r3 = view_proj.row(3);
    // w_clip = -z_view, so row 3 is minus the view's z row; row 2 is -P22 · row 3 + P23 · e_w.
    let axes = [r3.x, r3.y, r3.z];
    let (axis, &largest) = axes
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))?;
    if largest.abs() < 1e-6 {
        return None;
    }
    let p22 = -[r2.x, r2.y, r2.z][axis] / largest;
    let p23 = r2.w + p22 * r3.w;
    (p22.is_finite() && p23.is_finite() && p23.abs() > 1e-9).then_some((p22, p23))
}

/// The camera's world-space right and up vectors (screen right, screen up) from `view_proj`.
pub(crate) fn camera_axes(view_proj: Mat4) -> (Vec3, Vec3) {
    let inverse = view_proj.inverse();
    let unproject = |x: f32, y: f32| {
        let point = inverse * Vec4::new(x, y, 0.5, 1.0);
        point.truncate() / point.w
    };
    let centre = unproject(0.0, 0.0);
    let right = (unproject(0.5, 0.0) - centre).normalize_or_zero();
    let up = (unproject(0.0, 0.5) - centre).normalize_or_zero();
    if right == Vec3::ZERO || up == Vec3::ZERO {
        (Vec3::X, Vec3::Y)
    } else {
        (right, up)
    }
}

/// CPU-side slot allocator (see the module docs). Pure logic, unit-tested headless.
///
/// A frame's changes (the clock step and its allocations) are a transaction: [`Self::begin`]
/// opens one, [`Self::commit`] keeps it once the frame is submitted, and the next `begin`
/// rolls back one that was never committed (a frame that failed after `prepare`), so the
/// CPU clock never runs ahead of the GPU's and a slot is never held for a particle that was
/// not uploaded.
pub(crate) struct SlotAllocator {
    expiry: Vec<f64>,
    clock: f64,
    cursor: usize,
    /// `(slot, previous expiry)` of this frame's allocations (preallocated).
    undo: Vec<(u32, f64)>,
    /// Clock and cursor before the open transaction.
    saved: Option<(f64, usize)>,
}

impl SlotAllocator {
    /// `capacity` slots; at most `per_frame` allocations per transaction are undoable.
    pub(crate) fn new(capacity: usize, per_frame: usize) -> Self {
        Self {
            expiry: vec![f64::NEG_INFINITY; capacity],
            clock: 0.0,
            cursor: 0,
            undo: Vec::with_capacity(per_frame),
            saved: None,
        }
    }

    /// Opens this frame's transaction, rolling back an uncommitted previous one.
    pub(crate) fn begin(&mut self) {
        self.rollback();
        self.saved = Some((self.clock, self.cursor));
    }

    /// Keeps the open transaction (the frame was submitted).
    pub(crate) fn commit(&mut self) {
        self.undo.clear();
        self.saved = None;
    }

    fn rollback(&mut self) {
        if let Some((clock, cursor)) = self.saved.take() {
            for &(slot, previous) in self.undo.iter().rev() {
                self.expiry[slot as usize] = previous;
            }
            self.clock = clock;
            self.cursor = cursor;
        }
        self.undo.clear();
    }

    /// Advances the clock by one frame's step.
    pub(crate) fn advance(&mut self, dt: f32) {
        self.clock += f64::from(dt);
    }

    /// A free slot for a particle living `lifetime` seconds, or `None` when the next
    /// [`SLOT_SCAN`] slots are all live.
    pub(crate) fn allocate(&mut self, lifetime: f32) -> Option<u32> {
        let capacity = self.expiry.len();
        for _ in 0..SLOT_SCAN.min(capacity) {
            let slot = self.cursor;
            self.cursor = (self.cursor + 1) % capacity;
            if self.expiry[slot] <= self.clock {
                if self.undo.len() >= self.undo.capacity() {
                    return None; // More than one frame's worth: never reallocate.
                }
                let id = u32::try_from(slot).ok()?;
                self.undo.push((id, self.expiry[slot]));
                self.expiry[slot] = self.clock + f64::from(lifetime) + EXPIRY_MARGIN;
                return Some(id);
            }
        }
        None
    }
}

/// Views and region the distortion pass renders into this frame (ADR-0019).
pub(crate) struct DistortionTargets {
    pub(crate) hdr: vk::ImageView,
    pub(crate) depth: vk::ImageView,
    /// The render sub-rectangle (top-left corner of the render-sized targets).
    pub(crate) extent: vk::Extent2D,
}

/// The distortion pipeline's own layout and set (ADR-0019).
struct DistortionResources {
    pipeline_layout: PipelineLayout,
    set: vk::DescriptorSet,
    _layout: DescriptorSetLayout,
    /// Bilinear, clamped: the HDR copy is sampled at sub-pixel offsets.
    linear_sampler: Sampler,
}

/// Views the VFX passes render into or sample this frame.
pub(crate) struct VfxTargets {
    pub(crate) hdr: vk::ImageView,
    pub(crate) depth: vk::ImageView,
    pub(crate) accum: Option<vk::ImageView>,
    pub(crate) reveal: Option<vk::ImageView>,
    pub(crate) extent: vk::Extent2D,
}

struct VfxFrameBuffers {
    uniform: GpuBuffer,
    spawns: GpuBuffer,
    stats: GpuBuffer,
    set: vk::DescriptorSet,
    spawn_count: u32,
    dropped: u32,
    pixels_on_screen: u64,
    submitted: bool,
    sequence: u64,
}

/// Every VFX pipeline (rebuilt together on a dev shader reload).
pub(crate) struct VfxPipelines {
    simulate: GraphicsPipeline,
    sprite: GraphicsPipeline,
    mesh: Option<GraphicsPipeline>,
    resolve: Option<GraphicsPipeline>,
    /// The heat-haze pipeline (ADR-0019); `None` without distortion.
    distortion: Option<GraphicsPipeline>,
}

/// The VFX passes' buffers, descriptor sets and pipelines.
pub(crate) struct VfxPass {
    frames: Vec<VfxFrameBuffers>,
    particles: GpuBuffer,
    alive: GpuBuffer,
    args: GpuBuffer,
    default_atlas: GpuImage,
    texture_set: vk::DescriptorSet,
    resolve_set: Option<vk::DescriptorSet>,
    distortion: Option<DistortionResources>,
    /// Allocator-clock time until which a distortion particle may be live (ADR-0019).
    distortion_until: f64,
    pipelines: VfxPipelines,
    pipeline_layout: PipelineLayout,
    resolve_pipeline_layout: PipelineLayout,
    _pool: DescriptorPool,
    _frame_layout: DescriptorSetLayout,
    _texture_layout: DescriptorSetLayout,
    _resolve_layout: DescriptorSetLayout,
    atlas_sampler: Sampler,
    point_sampler: Sampler,
    atlas_view: vk::ImageView,
    depth_view: vk::ImageView,
    allocator: SlotAllocator,
    mesh_slots: [Option<(MeshId, u32)>; VFX_MESH_SLOTS],
    budget: VfxBudget,
    oit: bool,
    capacity: u32,
    mesh_region: u32,
    hdr_format: vk::Format,
    depth_format: vk::Format,
    latest: Option<VfxStats>,
    next_sequence: u64,
}

fn layout_binding(
    binding: u32,
    kind: vk::DescriptorType,
    stages: vk::ShaderStageFlags,
) -> vk::DescriptorSetLayoutBinding<'static> {
    vk::DescriptorSetLayoutBinding::default()
        .binding(binding)
        .descriptor_type(kind)
        .descriptor_count(1)
        .stage_flags(stages)
}

impl VfxPass {
    /// Allocates every buffer for `budget`, uploads the built-in atlas and builds the
    /// pipelines. `oit` selects the weighted blended OIT draw (and the resolve pass);
    /// `budget.distortion` the heat-haze pipeline and its set (ADR-0019), whose layout is
    /// checked against the device's per-stage descriptor limits.
    #[allow(clippy::too_many_lines)] // One linear set-up sequence.
    pub(crate) fn new(
        device: &DeviceContext,
        shaders: &ShaderSource,
        frames: usize,
        hdr_format: vk::Format,
        depth_format: vk::Format,
        budget: VfxBudget,
        oit: bool,
    ) -> RenderResult<Self> {
        let capacity = budget.max_particles.max(WORKGROUP);
        let mesh_region = (capacity / 8).max(1);
        let max_spawns = budget.max_spawns_per_frame.clamp(1, capacity);
        let frame_bindings = frame_set_bindings();
        let frame_layout = DescriptorSetLayout::new(device.device.clone(), &frame_bindings)?;
        let sampled = vk::DescriptorType::COMBINED_IMAGE_SAMPLER;
        let fragment = vk::ShaderStageFlags::FRAGMENT;
        let texture_layout = DescriptorSetLayout::new(
            device.device.clone(),
            &[
                layout_binding(0, sampled, fragment),
                layout_binding(1, sampled, fragment),
            ],
        )?;
        let resolve_layout = DescriptorSetLayout::new(
            device.device.clone(),
            &[
                layout_binding(0, sampled, fragment),
                layout_binding(1, sampled, fragment),
            ],
        )?;
        let distortion_bindings = distortion_set_bindings();
        if budget.distortion {
            crate::descriptor_budget::check(
                "distortion",
                &[&frame_bindings, &distortion_bindings],
                crate::descriptor_budget::StageTotals {
                    samplers: device.max_per_stage_samplers,
                    sampled_images: device.max_per_stage_sampled_images,
                    storage_buffers: device.max_per_stage_storage_buffers,
                },
            )?;
        }
        let distortion_layout = budget
            .distortion
            .then(|| DescriptorSetLayout::new(device.device.clone(), &distortion_bindings))
            .transpose()?;
        let frame_count = u32::try_from(frames).expect("frame count fits u32");
        let pool = DescriptorPool::new(
            device.device.clone(),
            &[
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::UNIFORM_BUFFER)
                    .descriptor_count(frame_count),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(4 * frame_count),
                vk::DescriptorPoolSize::default()
                    .ty(sampled)
                    .descriptor_count(7),
            ],
            frame_count + 3,
        )?;
        let push = [vk::PushConstantRange::default()
            .stage_flags(PUSH_STAGES)
            .offset(0)
            .size(PUSH_SIZE)];
        let pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[frame_layout.handle(), texture_layout.handle()],
            &push,
        )?;
        let resolve_pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[resolve_layout.handle()],
            &[],
        )?;
        let distortion_push = [vk::PushConstantRange::default()
            .stage_flags(DISTORTION_PUSH_STAGES)
            .offset(0)
            .size(DISTORTION_PUSH_SIZE)];
        let distortion_pipeline_layout = distortion_layout
            .as_ref()
            .map(|layout| {
                PipelineLayout::with_push_constants(
                    device.device.clone(),
                    &[frame_layout.handle(), layout.handle()],
                    &distortion_push,
                )
            })
            .transpose()?;

        let buffer = |name: &str, size: u64, usage, location| {
            GpuBuffer::new(
                device.device.clone(),
                device.allocator(),
                name,
                size,
                usage,
                location,
            )
        };
        let particle_bytes = core::mem::size_of::<GpuParticle>() as u64;
        let range_limit = u64::from(device.max_storage_buffer_range);
        let particles_size = particle_bytes * u64::from(capacity);
        // Sprites, the four mesh-slot regions, then the distortion list (ADR-0019).
        let alive_size =
            4 * (u64::from(capacity) + u64::from(mesh_region) * (VFX_MESH_SLOTS as u64 + 1));
        if particles_size > range_limit || alive_size > range_limit {
            return Err(RenderError::Vfx(
                "the particle budget exceeds the storage-buffer range",
            ));
        }
        let particles = buffer(
            "vfx-particles",
            particles_size,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuOnly,
        )?;
        let alive = buffer(
            "vfx-alive",
            alive_size,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuOnly,
        )?;
        let args = buffer(
            "vfx-args",
            ARGS_BYTES,
            vk::BufferUsageFlags::STORAGE_BUFFER
                | vk::BufferUsageFlags::INDIRECT_BUFFER
                | vk::BufferUsageFlags::TRANSFER_SRC
                | vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuOnly,
        )?;
        clear_buffers(device, &[&particles, &alive, &args])?;

        let sets_layouts: Vec<vk::DescriptorSetLayout> = (0..frames)
            .map(|_| frame_layout.handle())
            .chain([texture_layout.handle()])
            .chain(oit.then(|| resolve_layout.handle()))
            .chain(distortion_layout.as_ref().map(DescriptorSetLayout::handle))
            .collect();
        let sets = pool.allocate(&device.device, &sets_layouts)?;
        let mut frame_buffers = Vec::with_capacity(frames);
        for (frame, &set) in sets.iter().take(frames).enumerate() {
            let _ = frame;
            let resources = VfxFrameBuffers {
                uniform: buffer(
                    "vfx-uniform",
                    core::mem::size_of::<VfxUniform>() as u64,
                    vk::BufferUsageFlags::UNIFORM_BUFFER,
                    MemoryLocation::CpuToGpu,
                )?,
                spawns: buffer(
                    "vfx-spawns",
                    particle_bytes * u64::from(max_spawns),
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                    MemoryLocation::CpuToGpu,
                )?,
                stats: buffer(
                    "vfx-stats",
                    ARGS_BYTES,
                    vk::BufferUsageFlags::TRANSFER_DST,
                    MemoryLocation::GpuToCpu,
                )?,
                set,
                spawn_count: 0,
                dropped: 0,
                pixels_on_screen: 1,
                submitted: false,
                sequence: 0,
            };
            write_frame_set(&device.device, &resources, &particles, &alive, &args);
            frame_buffers.push(resources);
        }
        let texture_set = sets[frames];
        let resolve_set = oit.then(|| sets[frames + 1]);
        let distortion_set = budget
            .distortion
            .then(|| sets[frames + 1 + usize::from(oit)]);

        let atlas_sampler = Sampler::new(
            device.device.clone(),
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::LINEAR)
                .min_filter(vk::Filter::LINEAR)
                .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .min_lod(0.0)
                .max_lod(vk::LOD_CLAMP_NONE),
        )?;
        // `texelFetch` ignores filtering; the depth and OIT targets still need a sampler.
        let point_sampler = Sampler::new(
            device.device.clone(),
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::NEAREST)
                .min_filter(vk::Filter::NEAREST)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
        )?;
        let default_atlas =
            crate::texture::upload(device, "vfx-atlas", &moba_assets::procedural_atlas())?;
        let pipelines = build_pipelines(
            device,
            shaders,
            &PipelineLayouts {
                particles: pipeline_layout.handle(),
                resolve: resolve_pipeline_layout.handle(),
                distortion: distortion_pipeline_layout
                    .as_ref()
                    .map(PipelineLayout::handle),
            },
            hdr_format,
            depth_format,
            oit,
            budget.mesh_particles,
        )?;
        let distortion = match (
            distortion_layout,
            distortion_pipeline_layout,
            distortion_set,
        ) {
            (Some(layout), Some(pipeline_layout), Some(set)) => Some(DistortionResources {
                pipeline_layout,
                set,
                _layout: layout,
                linear_sampler: Sampler::new(
                    device.device.clone(),
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::LINEAR)
                        .min_filter(vk::Filter::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                )?,
            }),
            _ => None,
        };
        let pass = Self {
            frames: frame_buffers,
            particles,
            alive,
            args,
            atlas_view: default_atlas.view(),
            default_atlas,
            texture_set,
            resolve_set,
            distortion,
            distortion_until: f64::NEG_INFINITY,
            pipelines,
            pipeline_layout,
            resolve_pipeline_layout,
            _pool: pool,
            _frame_layout: frame_layout,
            _texture_layout: texture_layout,
            _resolve_layout: resolve_layout,
            atlas_sampler,
            point_sampler,
            depth_view: vk::ImageView::null(),
            allocator: SlotAllocator::new(capacity as usize, max_spawns as usize),
            mesh_slots: [None; VFX_MESH_SLOTS],
            budget,
            oit,
            capacity,
            mesh_region,
            hdr_format,
            depth_format,
            latest: None,
            next_sequence: 0,
        };
        pass.write_atlas(device);
        Ok(pass)
    }

    /// The tier budget the pass was built for.
    pub(crate) fn budget(&self) -> VfxBudget {
        self.budget
    }

    /// Builds replacement pipelines (dev shader reload) without installing them.
    pub(crate) fn build_pipelines(
        &self,
        device: &DeviceContext,
        shaders: &ShaderSource,
    ) -> RenderResult<VfxPipelines> {
        build_pipelines(
            device,
            shaders,
            &PipelineLayouts {
                particles: self.pipeline_layout.handle(),
                resolve: self.resolve_pipeline_layout.handle(),
                distortion: self
                    .distortion
                    .as_ref()
                    .map(|distortion| distortion.pipeline_layout.handle()),
            },
            self.hdr_format,
            self.depth_format,
            self.oit,
            self.budget.mesh_particles,
        )
    }

    /// Installs pipelines from [`Self::build_pipelines`]. The device must be idle.
    pub(crate) fn install_pipelines(&mut self, pipelines: VfxPipelines) {
        self.pipelines = pipelines;
    }

    /// Replaces the atlas with `view` (`None` restores the built-in one). Only call while no
    /// submitted frame uses the texture set (the renderer waits for the device first).
    pub(crate) fn set_atlas(&mut self, device: &DeviceContext, view: Option<vk::ImageView>) {
        self.atlas_view = view.unwrap_or_else(|| self.default_atlas.view());
        self.write_atlas(device);
    }

    /// Binds a registered mesh to a mesh-particle slot (`None` unbinds it). Load time only.
    pub(crate) fn set_mesh(
        &mut self,
        slot: usize,
        mesh: Option<(MeshId, u32)>,
    ) -> RenderResult<()> {
        let entry = self
            .mesh_slots
            .get_mut(slot)
            .ok_or(RenderError::Vfx("mesh slot out of range"))?;
        *entry = mesh;
        Ok(())
    }

    fn write_atlas(&self, device: &DeviceContext) {
        let info = [vk::DescriptorImageInfo::default()
            .sampler(self.atlas_sampler.handle())
            .image_view(self.atlas_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.texture_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&info);
        // SAFETY: the set, sampler and view are live, and no pending command buffer uses the
        // set (creation, or after the caller's device-idle wait).
        unsafe {
            device
                .device
                .update_descriptor_sets(core::slice::from_ref(&write), &[]);
        }
    }

    /// Points the descriptor sets at this swapchain size's depth, OIT targets and (with
    /// distortion) the HDR copy and the unit ID target. Only call while no submitted frame
    /// uses them (at creation and after the resize's idle wait).
    pub(crate) fn bind_targets(
        &mut self,
        device: &DeviceContext,
        depth: vk::ImageView,
        accum: Option<vk::ImageView>,
        reveal: Option<vk::ImageView>,
        distortion: Option<(vk::ImageView, vk::ImageView)>,
    ) {
        self.depth_view = depth;
        // Without soft particles (Low, portability-subset devices) the shader never samples
        // the depth, and the slot names the atlas instead, so the depth attachment is never
        // also bound as a sampled image.
        let (depth_view, depth_layout) = if self.budget.soft_particles {
            (depth, vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
        } else {
            (self.atlas_view, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        };
        let depth_info = [vk::DescriptorImageInfo::default()
            .sampler(self.point_sampler.handle())
            .image_view(depth_view)
            .image_layout(depth_layout)];
        let mut writes = vec![
            vk::WriteDescriptorSet::default()
                .dst_set(self.texture_set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&depth_info),
        ];
        let accum_info;
        let reveal_info;
        if let (Some(set), Some(accum), Some(reveal)) = (self.resolve_set, accum, reveal) {
            accum_info = [vk::DescriptorImageInfo::default()
                .sampler(self.point_sampler.handle())
                .image_view(accum)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            reveal_info = [vk::DescriptorImageInfo::default()
                .sampler(self.point_sampler.handle())
                .image_view(reveal)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            for (binding, info) in [(0, &accum_info), (1, &reveal_info)] {
                writes.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(binding)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(info),
                );
            }
        }
        // ADR-0019: the copy (bilinear), the depth (read-only attachment of the same pass,
        // like the particles' soft depth) and the unit IDs (point).
        let copy_info;
        let distortion_depth_info;
        let ids_info;
        if let (Some(resources), Some((copy, ids))) = (&self.distortion, distortion) {
            copy_info = [vk::DescriptorImageInfo::default()
                .sampler(resources.linear_sampler.handle())
                .image_view(copy)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            distortion_depth_info = [vk::DescriptorImageInfo::default()
                .sampler(self.point_sampler.handle())
                .image_view(depth)
                .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)];
            ids_info = [vk::DescriptorImageInfo::default()
                .sampler(self.point_sampler.handle())
                .image_view(ids)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            for (binding, info) in [(0, &copy_info), (1, &distortion_depth_info), (2, &ids_info)] {
                writes.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(resources.set)
                        .dst_binding(binding)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(info),
                );
            }
        }
        // SAFETY: every set, sampler and view is live and no pending command buffer uses the
        // sets (see the method contract).
        unsafe { device.device.update_descriptor_sets(&writes, &[]) };
    }

    /// Whether the heat-haze pipeline exists (ADR-0019).
    pub(crate) fn has_distortion(&self) -> bool {
        self.distortion.is_some() && self.pipelines.distortion.is_some()
    }

    /// Whether a distortion particle may be live this frame (after [`Self::prepare`]): the
    /// renderer records the copy and the draw only then (ADR-0019).
    pub(crate) fn distortion_live(&self) -> bool {
        self.has_distortion() && self.allocator.clock < self.distortion_until
    }

    /// The particle state, compacted lists and indirect arguments (render-graph imports).
    pub(crate) fn buffers(&self) -> [vk::Buffer; 3] {
        [
            self.particles.handle(),
            self.alive.handle(),
            self.args.handle(),
        ]
    }

    /// This frame's spawn upload and stats readback buffers.
    pub(crate) fn frame_buffers(&self, frame: usize) -> [vk::Buffer; 2] {
        let buffers = &self.frames[frame];
        [buffers.spawns.handle(), buffers.stats.handle()]
    }

    /// Decodes the stats of `frame`'s previous submission. Call after its fence wait.
    pub(crate) fn collect(&mut self, frame: usize) {
        let buffers = &mut self.frames[frame];
        if !core::mem::take(&mut buffers.submitted) {
            return;
        }
        let Some(words) = buffers.stats.mapped_slice_mut::<u32>(ARGS_WORDS) else {
            return;
        };
        let meshes = (0..VFX_MESH_SLOTS)
            .map(|slot| words[ARGS_MESH + slot * 5 + 1])
            .sum();
        let distortion = words[ARGS_DISTORT + 1];
        let pixels = u64::from(words[STAT_PIXELS]);
        self.latest = Some(VfxStats {
            frame: buffers.sequence,
            alive: words[STAT_ALIVE],
            sprites: words[1],
            meshes,
            distortion,
            spawned: words[STAT_SPAWNED],
            dropped: buffers.dropped,
            pixels,
            overdraw: pixels as f32 / buffers.pixels_on_screen.max(1) as f32,
        });
    }

    /// The latest decoded stats.
    pub(crate) fn latest(&self) -> Option<VfxStats> {
        self.latest
    }

    /// Marks `frame` as submitted, so the next [`Self::collect`] reads its stats.
    pub(crate) fn submitted(&mut self, frame: usize) {
        self.allocator.commit();
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        if let Some(buffers) = self.frames.get_mut(frame) {
            buffers.submitted = true;
            buffers.sequence = sequence;
        }
    }

    /// Allocates slots for this frame's spawns and writes the spawn upload and the uniform
    /// block. The frame's fence must have been waited on. Allocation-free.
    pub(crate) fn prepare(&mut self, frame: usize, scene: &Scene<'_>, extent: vk::Extent2D) {
        let vfx = scene.vfx;
        let dt = if vfx.dt.is_finite() {
            vfx.dt.clamp(0.0, MAX_DT)
        } else {
            0.0
        };
        self.allocator.begin();
        self.allocator.advance(dt);
        let mesh_ok: [bool; VFX_MESH_SLOTS] = core::array::from_fn(|slot| {
            self.budget.mesh_particles
                && self.pipelines.mesh.is_some()
                && self.mesh_slots[slot].is_some()
        });
        let buffers = &mut self.frames[frame];
        let limit = (self.budget.max_spawns_per_frame.max(1) as usize).min(self.capacity as usize);
        let wanted = vfx.spawns.len();
        let mut dropped = wanted.saturating_sub(limit);
        let mut count = 0usize;
        let distortion = self.distortion.is_some() && self.pipelines.distortion.is_some();
        if let Some(upload) = buffers.spawns.mapped_slice_mut::<GpuParticle>(limit) {
            for spawn in &vfx.spawns[..wanted.min(limit)] {
                let haze = spawn.blend == ParticleBlend::Distortion;
                if haze && !distortion {
                    // Heat haze where distortion is off: skipped, not dropped (ADR-0019).
                    continue;
                }
                let slot_ok = mesh_ok[usize::from(spawn.mesh_slot).min(VFX_MESH_SLOTS - 1)];
                let encoded = GpuParticle::encode(spawn, 0, slot_ok);
                let Some(slot) = self.allocator.allocate(encoded.lifetime()) else {
                    dropped += 1;
                    continue;
                };
                if haze {
                    let until =
                        self.allocator.clock + f64::from(encoded.lifetime()) + EXPIRY_MARGIN;
                    self.distortion_until = self.distortion_until.max(until);
                }
                upload[count] = encoded;
                upload[count].misc[3] = slot;
                count += 1;
            }
        } else {
            dropped = wanted;
        }
        buffers.spawn_count = u32::try_from(count).unwrap_or(u32::MAX);
        buffers.dropped = u32::try_from(dropped).unwrap_or(u32::MAX);
        buffers.pixels_on_screen = u64::from(extent.width) * u64::from(extent.height);

        let camera = scene.camera;
        let (right, up) = camera_axes(camera.view_proj);
        let soft = if self.budget.soft_particles {
            depth_params(camera.view_proj)
        } else {
            None
        };
        let (p22, p23) = soft.unwrap_or((0.0, 1.0));
        let ally_opacity = if vfx.ally_opacity.is_finite() {
            vfx.ally_opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let mesh_indices: [u32; VFX_MESH_SLOTS] = core::array::from_fn(|slot| {
            if mesh_ok[slot] {
                self.mesh_slots[slot].map_or(0, |(_, indices)| indices)
            } else {
                0
            }
        });
        let width = extent.width.max(1) as f32;
        let height = extent.height.max(1) as f32;
        let uniform = VfxUniform {
            view_proj: camera.view_proj.to_cols_array_2d(),
            camera_right: right.extend(dt).to_array(),
            camera_up: up.extend(self.allocator.clock as f32).to_array(),
            eye: camera
                .position
                .extend(if soft.is_some() { SOFT_DISTANCE } else { 0.0 })
                .to_array(),
            viewport: [width, height, 1.0 / width, 1.0 / height],
            depth: [p22, p23, ally_opacity, self.budget.telegraph_floor],
            light: camera
                .light_direction
                .normalize_or_zero()
                .extend(0.4)
                .to_array(),
            counts: [self.capacity, buffers.spawn_count, self.mesh_region, 0],
            mesh_indices,
        };
        buffers.uniform.write_bytes(0, bytes_of(&uniform));
    }

    /// Records the simulation (three dispatches) and the stats copy.
    pub(crate) fn record_simulate(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
    ) {
        let buffers = &self.frames[frame];
        // SAFETY: the compute pipeline is live and was built with `pipeline_layout`.
        unsafe {
            device.device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                self.pipelines.simulate.handle(),
            );
        }
        // SAFETY: the set matches set 0 of the layout and names live buffers.
        unsafe {
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_layout.handle(),
                0,
                &[buffers.set],
                &[],
            );
        }
        let spawn_groups = buffers.spawn_count.max(1).div_ceil(WORKGROUP);
        let groups = [
            (MODE_SPAWN, spawn_groups),
            (MODE_SIMULATE, self.capacity.div_ceil(WORKGROUP)),
            (MODE_FINALIZE, 1),
        ];
        for (index, (mode, count)) in groups.into_iter().enumerate() {
            if index > 0 {
                memory_barrier(
                    device,
                    command_buffer,
                    vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::SHADER_STORAGE_WRITE,
                    vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE,
                );
            }
            self.push(device, command_buffer, mode, 0);
            // SAFETY: the pipeline, set and push constants are bound; the shader bounds every
            // index by the capacities in the uniform block, which match the buffer sizes.
            unsafe { device.device.cmd_dispatch(command_buffer, count, 1, 1) };
        }
        memory_barrier(
            device,
            command_buffer,
            vk::PipelineStageFlags2::COMPUTE_SHADER,
            vk::AccessFlags2::SHADER_STORAGE_WRITE,
            vk::PipelineStageFlags2::COPY,
            vk::AccessFlags2::TRANSFER_READ,
        );
        let region = vk::BufferCopy::default().size(ARGS_BYTES);
        // SAFETY: both buffers hold `ARGS_BYTES`; args was created with TRANSFER_SRC and the
        // stats buffer with TRANSFER_DST, and the graph ordered the stats buffer's previous
        // host read before this frame.
        unsafe {
            device.device.cmd_copy_buffer(
                command_buffer,
                self.args.handle(),
                buffers.stats.handle(),
                core::slice::from_ref(&region),
            );
        }
    }

    fn push(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        mode: u32,
        slot: u32,
    ) {
        // SAFETY: the layout declares exactly this 8-byte compute + vertex range at offset 0.
        unsafe {
            device.device.cmd_push_constants(
                command_buffer,
                self.pipeline_layout.handle(),
                PUSH_STAGES,
                0,
                bytes_of(&VfxPush { mode, slot }),
            );
        }
    }

    /// Records the particle draw into the HDR target (and the OIT targets).
    pub(crate) fn record_draw(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        targets: &VfxTargets,
        scene: &ScenePass,
    ) {
        let extent = targets.extent;
        let mut colors = [vk::RenderingAttachmentInfo::default()
            .image_view(targets.hdr)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE); 3];
        let mut count = 1;
        if self.oit
            && let (Some(accum), Some(reveal)) = (targets.accum, targets.reveal)
        {
            for view in [accum, reveal] {
                colors[count] = vk::RenderingAttachmentInfo::default()
                    .image_view(view)
                    .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .load_op(vk::AttachmentLoadOp::CLEAR)
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .clear_value(vk::ClearValue {
                        color: vk::ClearColorValue { float32: [0.0; 4] },
                    });
                count += 1;
            }
        }
        // Read-only depth: tested, never written (STORE_OP_NONE), and sampled for soft
        // particles in the same pass.
        let depth = vk::RenderingAttachmentInfo::default()
            .image_view(targets.depth)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::NONE);
        let render_info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            })
            .layer_count(1)
            .color_attachments(&colors[..count])
            .depth_attachment(&depth);
        device.cmd_begin_rendering(command_buffer, &render_info);
        super::set_viewport_scissor(
            device,
            command_buffer,
            super::flipped_viewport(extent),
            extent,
        );
        let sets = [self.frames[frame].set, self.texture_set];
        // SAFETY: both sets match the layout (sets 0 and 1) and name live resources; the
        // depth view was transitioned to DEPTH_STENCIL_READ_ONLY_OPTIMAL by the graph.
        unsafe {
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout.handle(),
                0,
                &sets,
                &[],
            );
        }
        bind_graphics(device, command_buffer, &self.pipelines.sprite);
        self.push(device, command_buffer, 0, 0);
        // SAFETY: the args buffer holds a VkDrawIndirectCommand at offset 0 whose instance
        // count the simulation clamped to the sprite list's capacity, and was created with
        // INDIRECT_BUFFER usage.
        unsafe {
            device
                .device
                .cmd_draw_indirect(command_buffer, self.args.handle(), 0, 1, 16);
        }
        if let Some(pipeline) = &self.pipelines.mesh {
            let mut bound = false;
            for (slot, entry) in self.mesh_slots.iter().enumerate() {
                let Some(mesh) = entry.and_then(|(id, _)| scene.mesh(id)) else {
                    continue;
                };
                if !bound {
                    bind_graphics(device, command_buffer, pipeline);
                    bound = true;
                }
                // SAFETY: the mesh buffers live as long as the renderer and hold `Vertex`
                // data (static meshes only; `set_vfx_mesh` rejects skinned ones) and `u32`
                // indices.
                unsafe {
                    device.device.cmd_bind_vertex_buffers(
                        command_buffer,
                        0,
                        &[mesh.vertices.handle()],
                        &[0],
                    );
                }
                // SAFETY: as above; the index buffer holds `index_count` in-range indices.
                unsafe {
                    device.device.cmd_bind_index_buffer(
                        command_buffer,
                        mesh.indices.handle(),
                        0,
                        vk::IndexType::UINT32,
                    );
                }
                self.push(device, command_buffer, 0, slot as u32);
                let offset = ((ARGS_MESH + slot * 5) * 4) as u64;
                // SAFETY: the args buffer holds this slot's VkDrawIndexedIndirectCommand at
                // `offset` (4-byte aligned), with the mesh's index count and an instance count
                // clamped to the slot region; the shader bounds its list reads the same way.
                unsafe {
                    device.device.cmd_draw_indexed_indirect(
                        command_buffer,
                        self.args.handle(),
                        offset,
                        1,
                        20,
                    );
                }
            }
        }
        device.cmd_end_rendering(command_buffer);
    }

    /// Records the OIT resolve over the HDR target (OIT tiers only).
    pub(crate) fn record_resolve(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        hdr: vk::ImageView,
        extent: vk::Extent2D,
    ) {
        let (Some(pipeline), Some(set)) = (&self.pipelines.resolve, self.resolve_set) else {
            return;
        };
        super::begin_color_only(
            device,
            command_buffer,
            hdr,
            extent,
            vk::AttachmentLoadOp::LOAD,
            super::full_viewport(extent),
        );
        bind_graphics(device, command_buffer, pipeline);
        // SAFETY: the set matches the resolve layout and names the OIT targets, which the
        // graph transitioned to SHADER_READ_ONLY_OPTIMAL.
        unsafe {
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.resolve_pipeline_layout.handle(),
                0,
                &[set],
                &[],
            );
        }
        // SAFETY: the fullscreen vertex shader reads no vertex buffers.
        unsafe { device.device.cmd_draw(command_buffer, 3, 1, 0, 0) };
        device.cmd_end_rendering(command_buffer);
    }
}

impl VfxPass {
    /// Records the heat-haze draw (ADR-0019) into the HDR target: the distortion list, depth
    /// tested read-only against the scene, refracting the copy within the render
    /// sub-rectangle by at most `cap` pixels. `mask` uses the unit ID target. The caller
    /// checks [`Self::distortion_live`].
    pub(crate) fn record_distortion(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        targets: &DistortionTargets,
        cap: f32,
    ) {
        let (Some(resources), Some(pipeline)) = (&self.distortion, &self.pipelines.distortion)
        else {
            return;
        };
        let extent = targets.extent;
        let colors = [vk::RenderingAttachmentInfo::default()
            .image_view(targets.hdr)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE)];
        // Read-only depth: tested, never written, and sampled in the same pass.
        let depth = vk::RenderingAttachmentInfo::default()
            .image_view(targets.depth)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::NONE);
        let render_info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            })
            .layer_count(1)
            .color_attachments(&colors)
            .depth_attachment(&depth);
        device.cmd_begin_rendering(command_buffer, &render_info);
        super::set_viewport_scissor(
            device,
            command_buffer,
            super::flipped_viewport(extent),
            extent,
        );
        bind_graphics(device, command_buffer, pipeline);
        let sets = [self.frames[frame].set, resources.set];
        // SAFETY: both sets match the distortion layout (sets 0 and 1) and name live
        // resources; the graph transitioned the copy and the IDs to SHADER_READ_ONLY_OPTIMAL
        // and the depth to DEPTH_STENCIL_READ_ONLY_OPTIMAL.
        unsafe {
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                resources.pipeline_layout.handle(),
                0,
                &sets,
                &[],
            );
        }
        let push = DistortionPush {
            rect: [
                extent.width.saturating_sub(1) as f32,
                extent.height.saturating_sub(1) as f32,
                cap,
                1.0,
            ],
            fade: [
                crate::distortion::SOFT_DISTANCE,
                crate::distortion::FADE_START,
                crate::distortion::FADE_END,
                0.0,
            ],
            list: [
                self.capacity + self.mesh_region * VFX_MESH_SLOTS as u32,
                0,
                0,
                0,
            ],
        };
        // SAFETY: the layout declares exactly this 48-byte vertex + fragment range at 0.
        unsafe {
            device.device.cmd_push_constants(
                command_buffer,
                resources.pipeline_layout.handle(),
                DISTORTION_PUSH_STAGES,
                0,
                bytes_of(&push),
            );
        }
        let offset = (ARGS_DISTORT * 4) as u64;
        // SAFETY: the args buffer holds the distortion list's VkDrawIndirectCommand at
        // `offset`, whose instance count the simulation clamped to the list's region (and
        // the shader reads `alive` from the list's base within the buffer).
        unsafe {
            device
                .device
                .cmd_draw_indirect(command_buffer, self.args.handle(), offset, 1, 16);
        }
        device.cmd_end_rendering(command_buffer);
    }
}

/// The VFX frame set (set 0 of the particle and distortion layouts): the uniform block, the
/// particles, the spawn upload, the compacted lists and the arguments.
pub(crate) fn frame_set_bindings() -> [vk::DescriptorSetLayoutBinding<'static>; 5] {
    let storage = vk::DescriptorType::STORAGE_BUFFER;
    let compute_vertex = vk::ShaderStageFlags::COMPUTE | vk::ShaderStageFlags::VERTEX;
    [
        layout_binding(
            0,
            vk::DescriptorType::UNIFORM_BUFFER,
            compute_vertex | vk::ShaderStageFlags::FRAGMENT,
        ),
        layout_binding(1, storage, compute_vertex),
        layout_binding(2, storage, vk::ShaderStageFlags::COMPUTE),
        layout_binding(3, storage, compute_vertex),
        layout_binding(4, storage, vk::ShaderStageFlags::COMPUTE),
    ]
}

/// Set 1 of the distortion layout (ADR-0019): the HDR copy, the scene depth and the unit ID
/// target, all fragment-stage combined image samplers. Not part of the scene layout.
pub(crate) fn distortion_set_bindings() -> [vk::DescriptorSetLayoutBinding<'static>; 3] {
    let sampled = vk::DescriptorType::COMBINED_IMAGE_SAMPLER;
    let fragment = vk::ShaderStageFlags::FRAGMENT;
    [
        layout_binding(0, sampled, fragment),
        layout_binding(1, sampled, fragment),
        layout_binding(2, sampled, fragment),
    ]
}

/// The OIT target formats (accumulation, revealage).
pub(crate) const fn oit_formats() -> (vk::Format, vk::Format) {
    (ACCUM_FORMAT, REVEAL_FORMAT)
}

fn bind_graphics(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    pipeline: &GraphicsPipeline,
) {
    // SAFETY: the pipeline is live and was built with the layout its sets are bound with.
    unsafe {
        device.device.cmd_bind_pipeline(
            command_buffer,
            vk::PipelineBindPoint::GRAPHICS,
            pipeline.handle(),
        );
    }
}

fn memory_barrier(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    src_stage: vk::PipelineStageFlags2,
    src_access: vk::AccessFlags2,
    dst_stage: vk::PipelineStageFlags2,
    dst_access: vk::AccessFlags2,
) {
    let barrier = vk::MemoryBarrier2::default()
        .src_stage_mask(src_stage)
        .src_access_mask(src_access)
        .dst_stage_mask(dst_stage)
        .dst_access_mask(dst_access);
    let dependency = vk::DependencyInfo::default().memory_barriers(core::slice::from_ref(&barrier));
    device.cmd_pipeline_barrier2(command_buffer, &dependency);
}

fn write_frame_set(
    device: &ash::Device,
    frame: &VfxFrameBuffers,
    particles: &GpuBuffer,
    alive: &GpuBuffer,
    args: &GpuBuffer,
) {
    let info = |buffer: vk::Buffer| {
        [vk::DescriptorBufferInfo::default()
            .buffer(buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE)]
    };
    let uniform = info(frame.uniform.handle());
    let infos = [
        info(particles.handle()),
        info(frame.spawns.handle()),
        info(alive.handle()),
        info(args.handle()),
    ];
    let mut writes = vec![
        vk::WriteDescriptorSet::default()
            .dst_set(frame.set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .buffer_info(&uniform),
    ];
    for (binding, info) in (1u32..).zip(&infos) {
        writes.push(
            vk::WriteDescriptorSet::default()
                .dst_set(frame.set)
                .dst_binding(binding)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(info),
        );
    }
    // SAFETY: the set was allocated with the frame layout and every buffer is live.
    unsafe { device.update_descriptor_sets(&writes, &[]) };
}

/// Zero-fills `buffers` (every slot free) with a one-shot submission. Creation only.
fn clear_buffers(device: &DeviceContext, buffers: &[&GpuBuffer]) -> RenderResult<()> {
    let pool = CommandPool::new(device.device.clone(), device.graphics_family)?;
    let command = pool.allocate_one()?;
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: the command buffer is allocated and not recording.
    unsafe { device.device.begin_command_buffer(command, &begin) }?;
    for buffer in buffers {
        // SAFETY: each buffer was created with TRANSFER_DST and is not in use.
        unsafe {
            device
                .device
                .cmd_fill_buffer(command, buffer.handle(), 0, vk::WHOLE_SIZE, 0);
        }
    }
    memory_barrier(
        device,
        command,
        vk::PipelineStageFlags2::CLEAR,
        vk::AccessFlags2::TRANSFER_WRITE,
        vk::PipelineStageFlags2::COMPUTE_SHADER | vk::PipelineStageFlags2::VERTEX_SHADER,
        vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE,
    );
    // SAFETY: the command buffer is recording and is ended once.
    unsafe { device.device.end_command_buffer(command) }?;
    let commands = [command];
    let submit = vk::SubmitInfo::default().command_buffers(&commands);
    // SAFETY: the command buffer is executable on the graphics queue; load-time submits
    // happen only on the renderer's thread.
    unsafe {
        device
            .device
            .queue_submit(device.graphics_queue, &[submit], vk::Fence::null())
    }?;
    // SAFETY: waiting for our own load-time submission is valid. On failure the pool is
    // kept alive until the device is idle (or leaked) so the fill cannot outlive it.
    if let Err(error) = unsafe { device.device.queue_wait_idle(device.graphics_queue) } {
        // SAFETY: the device is live; waiting has no other preconditions.
        if unsafe { device.device.device_wait_idle() }.is_err() {
            core::mem::forget(pool);
        }
        return Err(error.into());
    }
    Ok(())
}

/// The pipeline layouts of the VFX pipelines.
struct PipelineLayouts {
    particles: vk::PipelineLayout,
    resolve: vk::PipelineLayout,
    /// `None` without distortion (ADR-0019).
    distortion: Option<vk::PipelineLayout>,
}

fn build_pipelines(
    device: &DeviceContext,
    shaders: &ShaderSource,
    layouts: &PipelineLayouts,
    hdr_format: vk::Format,
    depth_format: vk::Format,
    oit: bool,
    meshes: bool,
) -> RenderResult<VfxPipelines> {
    let (layout, resolve_layout) = (layouts.particles, layouts.resolve);
    let module = |name| shaders.module(device, name);
    let compute = module("vfx_simulate.comp")?;
    let stage = vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(compute.handle())
        .name(c"main");
    let simulate = device.create_compute_pipeline(
        &vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(layout),
    )?;
    let simulate = GraphicsPipeline::from_raw(device.device.clone(), simulate);
    let fragment = module(if oit { "vfx_oit.frag" } else { "vfx.frag" })?;
    let (accum, reveal) = oit_formats();
    let formats = [hdr_format, accum, reveal];
    let formats = if oit { &formats[..] } else { &formats[..1] };
    let blend = if oit {
        Blend::Add
    } else {
        Blend::Premultiplied
    };
    let sprite_vertex = module("vfx.vert")?;
    let sprite = particle_pipeline(
        device,
        &ParticleSpec {
            layout,
            vertex: sprite_vertex.handle(),
            fragment: fragment.handle(),
            formats,
            depth_format,
            blend,
            mesh_input: false,
        },
    )?;
    let mesh = if meshes {
        let vertex = module("vfx_mesh.vert")?;
        Some(particle_pipeline(
            device,
            &ParticleSpec {
                layout,
                vertex: vertex.handle(),
                fragment: fragment.handle(),
                formats,
                depth_format,
                blend,
                mesh_input: true,
            },
        )?)
    } else {
        None
    };
    let resolve = if oit {
        let vertex = module("fullscreen.vert")?;
        let fragment = module("vfx_resolve.frag")?;
        Some(particle_pipeline(
            device,
            &ParticleSpec {
                layout: resolve_layout,
                vertex: vertex.handle(),
                fragment: fragment.handle(),
                formats: &[hdr_format],
                depth_format: vk::Format::UNDEFINED,
                blend: Blend::Over,
                mesh_input: false,
            },
        )?)
    } else {
        None
    };
    let distortion = match layouts.distortion {
        Some(layout) => {
            let vertex = module("vfx_distort.vert")?;
            let fragment = module("vfx_distort.frag")?;
            Some(particle_pipeline(
                device,
                &ParticleSpec {
                    layout,
                    vertex: vertex.handle(),
                    fragment: fragment.handle(),
                    formats: &[hdr_format],
                    depth_format,
                    blend: Blend::Refract,
                    mesh_input: false,
                },
            )?)
        }
        None => None,
    };
    Ok(VfxPipelines {
        simulate,
        sprite,
        mesh,
        resolve,
        distortion,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Blend {
    /// `(ONE, ONE)` on every attachment: additive HDR plus the OIT sums.
    Add,
    /// `(ONE, ONE_MINUS_SRC_ALPHA)`: premultiplied; additive particles write alpha 0.
    Premultiplied,
    /// `(SRC_ALPHA, ONE_MINUS_SRC_ALPHA)`: the OIT resolve over the scene.
    Over,
    /// `(SRC_ALPHA, ONE_MINUS_SRC_ALPHA)` on colour, destination alpha kept: the heat haze
    /// cross-fades overlapping sprites (ADR-0019).
    Refract,
}

struct ParticleSpec<'a> {
    layout: vk::PipelineLayout,
    vertex: vk::ShaderModule,
    fragment: vk::ShaderModule,
    formats: &'a [vk::Format],
    /// `UNDEFINED` for no depth attachment (the resolve).
    depth_format: vk::Format,
    blend: Blend,
    /// Static mesh vertices (position, normal) at binding 0.
    mesh_input: bool,
}

fn particle_pipeline(
    device: &DeviceContext,
    spec: &ParticleSpec<'_>,
) -> RenderResult<GraphicsPipeline> {
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(spec.vertex)
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(spec.fragment)
            .name(c"main"),
    ];
    let bindings = [vk::VertexInputBindingDescription::default()
        .binding(0)
        .stride(core::mem::size_of::<Vertex>() as u32)
        .input_rate(vk::VertexInputRate::VERTEX)];
    let attributes = [
        vk::VertexInputAttributeDescription::default()
            .location(0)
            .binding(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(0),
        vk::VertexInputAttributeDescription::default()
            .location(1)
            .binding(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(12),
    ];
    let vertex_input = if spec.mesh_input {
        vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attributes)
    } else {
        vk::PipelineVertexInputStateCreateInfo::default()
    };
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
        .primitive_restart_enable(false);
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
    let depth_test = spec.depth_format != vk::Format::UNDEFINED;
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(depth_test)
        .depth_write_enable(false)
        .depth_compare_op(if depth_test {
            vk::CompareOp::LESS_OR_EQUAL
        } else {
            vk::CompareOp::ALWAYS
        });
    let (src, dst) = match spec.blend {
        Blend::Add => (vk::BlendFactor::ONE, vk::BlendFactor::ONE),
        Blend::Premultiplied => (vk::BlendFactor::ONE, vk::BlendFactor::ONE_MINUS_SRC_ALPHA),
        Blend::Over | Blend::Refract => (
            vk::BlendFactor::SRC_ALPHA,
            vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        ),
    };
    let (src_alpha, dst_alpha) = if spec.blend == Blend::Refract {
        (vk::BlendFactor::ZERO, vk::BlendFactor::ONE)
    } else {
        (src, dst)
    };
    // Every attachment uses the same state, so `independentBlend` is not required.
    let attachment = vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(true)
        .src_color_blend_factor(src)
        .dst_color_blend_factor(dst)
        .color_blend_op(vk::BlendOp::ADD)
        .src_alpha_blend_factor(src_alpha)
        .dst_alpha_blend_factor(dst_alpha)
        .alpha_blend_op(vk::BlendOp::ADD)
        .color_write_mask(vk::ColorComponentFlags::RGBA);
    let attachments = [attachment; 3];
    let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
        .logic_op_enable(false)
        .attachments(&attachments[..spec.formats.len()]);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    let mut rendering = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(spec.formats)
        .depth_attachment_format(spec.depth_format);
    let create_info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&color_blend)
        .dynamic_state(&dynamic)
        .layout(spec.layout)
        .push_next(&mut rendering);
    // Every referenced struct outlives the call; the layout matches the shaders.
    let handle = device.create_graphics_pipeline(&create_info)?;
    Ok(GraphicsPipeline::from_raw(device.device.clone(), handle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_core::math::Mat4;

    #[test]
    fn gpu_layouts_match_the_shader_blocks() {
        assert_eq!(core::mem::size_of::<GpuParticle>(), 96);
        assert_eq!(core::mem::size_of::<VfxUniform>(), 192);
        assert_eq!(core::mem::size_of::<VfxPush>(), 8);
        assert_eq!(core::mem::size_of::<DistortionPush>(), 48);
        assert_eq!(ARGS_WORDS, 40);
        // The distortion command follows the sprite, mesh, counter and stat words.
        assert_eq!(ARGS_DISTORT, STAT_SPAWNED + 1);
        assert_eq!(
            ARGS_DISTORT % 4,
            0,
            "16-byte aligned like every indirect command"
        );
    }

    #[test]
    fn half_floats_round_trip_the_common_range() {
        assert_eq!(f16_bits(0.0), 0);
        assert_eq!(f16_bits(1.0), 0x3c00);
        assert_eq!(f16_bits(-2.0), 0xc000);
        assert_eq!(f16_bits(0.5), 0x3800);
        assert_eq!(f16_bits(65_504.0), 0x7bff);
        assert_eq!(f16_bits(1e9), 0x7bff);
        assert_eq!(f16_bits(f32::NAN) & 0x7c00, 0x7c00);
        assert_eq!(f16_bits(6.0e-8), 1);
        assert_eq!(pack_half2(1.0, 0.5), 0x3800_3c00);
    }

    #[test]
    fn spawns_encode_flags_sprites_and_fall_back_without_meshes() {
        let spawn = ParticleSpawn::new(Vec3::new(1.0, 2.0, 3.0), 0.5, 1.5, Vec4::ONE)
            .with_blend(ParticleBlend::Alpha)
            .with_role(ParticleRole::EnemyTelegraph)
            .with_sprite(8, 16, 24.0)
            .with_shape(ParticleShape::Ground);
        let gpu = GpuParticle::encode(&spawn, 7, true);
        assert_eq!(gpu.misc[0], 2 | 4 | (2 << 3));
        assert_eq!(gpu.misc[1], 8 | (16 << 8) | (192 << 16));
        assert_eq!(gpu.misc[3], 7);
        assert_eq!(gpu.vel_life[3], 0.5);
        let mesh = ParticleSpawn::new(Vec3::ZERO, 20.0, 1.0, Vec4::ONE).with_mesh(9);
        let with = GpuParticle::encode(&mesh, 0, true);
        assert_eq!(with.misc[0], 3 | (3 << 5), "slot clamped to 3");
        assert_eq!(with.lifetime(), 10.0, "lifetime clamped");
        let without = GpuParticle::encode(&mesh, 0, false);
        assert_eq!(without.misc[0] & 3, 0, "billboard fallback");
        assert_eq!(without.misc[1] & 255, u32::from(MESH_FALLBACK_CELL));
        // Heat haze (ADR-0019): its own flag, always an upright billboard.
        let haze = ParticleSpawn::new(Vec3::ZERO, 1.0, 2.0, Vec4::new(0.6, 2.0, 1.0, 1.0))
            .with_blend(ParticleBlend::Distortion)
            .with_shape(ParticleShape::Ground)
            .with_role(ParticleRole::Ally);
        let gpu = GpuParticle::encode(&haze, 3, true);
        assert_eq!(gpu.misc[0], FLAG_DISTORT | (1 << 3));
        assert_eq!(gpu.misc[0] & 3, 0, "billboard");
        // Hostile values never reach the GPU as NaN.
        let bad = ParticleSpawn::new(Vec3::splat(f32::NAN), f32::NAN, f32::INFINITY, Vec4::NAN);
        let bad = GpuParticle::encode(&bad, 0, false);
        assert!(
            bad.pos_age
                .iter()
                .chain(&bad.vel_life)
                .chain(&bad.size_spin)
                .all(|v| v.is_finite())
        );
    }

    #[test]
    fn allocator_reuses_expired_slots_and_drops_when_full() {
        let mut slots = SlotAllocator::new(4, 8);
        let first: Vec<_> = (0..4).map(|_| slots.allocate(1.0)).collect();
        assert_eq!(first, [Some(0), Some(1), Some(2), Some(3)]);
        assert_eq!(slots.allocate(1.0), None, "every slot is live");
        slots.advance(0.5);
        assert_eq!(slots.allocate(1.0), None);
        slots.advance(0.61);
        assert_eq!(slots.allocate(0.2), Some(0), "expired after life + margin");
    }

    #[test]
    fn allocator_rolls_back_a_frame_that_was_not_submitted() {
        let mut slots = SlotAllocator::new(4, 4);
        slots.begin();
        slots.advance(0.5);
        assert_eq!(slots.allocate(5.0), Some(0));
        slots.commit();
        // A frame that fails after `prepare`: its step and allocations are undone.
        slots.begin();
        slots.advance(0.5);
        assert_eq!(slots.allocate(5.0), Some(1));
        assert_eq!(slots.allocate(5.0), Some(2));
        slots.begin();
        assert!(
            (slots.clock - 0.5).abs() < 1e-12,
            "clock back to the committed frame"
        );
        assert_eq!(slots.allocate(1.0), Some(1), "slot 1 is free again");
        assert_eq!(slots.allocate(1.0), Some(2));
        assert_eq!(slots.allocate(1.0), Some(3));
        assert_eq!(slots.allocate(1.0), None, "slot 0 is still held");
        slots.commit();
        assert_eq!(slots.allocate(1.0), None, "slot 0 still live");
    }

    #[test]
    fn depth_parameters_linearise_the_engine_projection() {
        let camera = crate::Camera::perspective(
            Vec3::new(0.0, 40.0, 30.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            16.0 / 9.0,
            0.1,
            1_000.0,
        );
        let (p22, p23) = depth_params(camera.view_proj).expect("perspective");
        for distance in [1.0f32, 25.0, 300.0] {
            let eye_forward = (Vec3::ZERO - camera.position).normalize();
            let point = camera.position + eye_forward * distance;
            let clip = camera.view_proj * point.extend(1.0);
            let depth = clip.z / clip.w;
            let linear = p23 / (depth + p22);
            assert!(
                (linear - distance).abs() < distance * 1e-3,
                "{linear} vs {distance}"
            );
            assert!((clip.w - distance).abs() < distance * 1e-3);
        }
        assert!(depth_params(Mat4::IDENTITY).is_none());
        let (right, up) = camera_axes(camera.view_proj);
        assert!(right.dot(up).abs() < 1e-4);
        assert!(up.y > 0.0, "screen up points upwards");
        assert!(right.x > 0.9, "looking down -z: screen right is +x");
    }
}
