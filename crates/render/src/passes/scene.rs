//! Scene and shadow passes: terrain, instanced units and GPU-skinned meshes into the HDR
//! target, and the main view's units (static and skinned) into the directional shadow map
//! before it.
//!
//! Every pipeline, each material permutation and the shadow casters included, is built in
//! [`ScenePass::new`] (or [`ScenePass::build_pipelines`] on a dev shader reload), never while
//! drawing. A mesh with a textured material ([`ScenePass::set_mesh_material`]) draws with the
//! `MATERIAL_TEXTURED` permutation and its material's descriptor set (set 2); every other
//! mesh keeps the vertex-colour look. Set 3 is a scene lighting set (shadow map, IBL, BRDF
//! LUT, ramp; `crate::lighting`), bound once per view: the main view's names the shadow map,
//! the offscreen views' a default image (their camera uniform also turns shadows off).
//!
//! **VAT meshes** (R4b, ADR-0012): minions registered with [`ScenePass::register_vat_mesh`]
//! draw instanced, one draw per mesh after the skinned draws, through `vat.vert` (and
//! `vat_shadow.vert` into the shadow map). Their pipelines use a second layout whose set 2 is
//! the mesh's VAT texture set; sets 0/1 stay compatible and set 3 is re-bound with it.
//!
//! The pass draws one or more **views** (R7b): view 0 is the frame's main scene; the others
//! are the offscreen images (portraits, the minimap) rendered with the same pipelines and
//! meshes into their own targets. Each view owns its per-frame camera, instance and bone
//! buffers and its bucketing scratch, all sized at creation, so preparing and recording
//! several views in one frame allocates nothing.
//!
//! **Culling and LODs** (R8, ADR-0018): each view culls its units against its camera frustum
//! (outlined units are kept: the hull outline reaches past the silhouette) and picks each
//! unit's mesh level by projected error; the main view also builds a separate **caster list**
//! for the shadow pass, culled against the fitted shadow box's sides. A list buckets its
//! instances by mesh and level, in scene order inside a bucket, so a culled list draws a
//! subset of the unculled one in the same order. The terrain is drawn as the runs of its
//! visible chunks. `CullMode::Off` keeps every unit and chunk.
//!
//! **Foliage and water** (R3b, ADR-0017): drawn last in the scene pass with their own layouts
//! (`super::environment`): the main view's visible foliage chunks, then the simple water
//! (alpha-blended over the carved bed) in every terrain view. The full water is a pass of
//! its own after the scene ([`ScenePass::record_water_pass`]).

use core::sync::atomic::{AtomicU64, Ordering};

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};
use gpu_allocator::MemoryLocation;
use moba_assets::JointMatrix;

use moba_core::math::Mat4;

use crate::cull::{CullMode, CullStats, Frustum, LodConfig, LodMetric, Planes};
use crate::device::DeviceContext;
use crate::environment::EnvironmentConfig;
use crate::error::{RenderError, RenderResult};
use crate::lighting::FrameLighting;
use crate::mesh::{self, GpuMesh, MAX_LODS};
use crate::motion::{DrawnState, MotionHistory};
use crate::pipeline::{
    self, ColorTarget, DepthOnly, PipelineSpec, SceneBlend, TargetBlend, TargetsSpec, VertexLayout,
    VertexlessDepth,
};
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuBuffer, GraphicsPipeline, PipelineLayout,
};
use crate::scene::{
    BoneRange, CameraUniform, ClearRect, GpuInstance, MeshId, Scene, TemporalCamera,
};
use crate::shaders::ShaderSource;
use crate::terrain::{MAX_TERRAIN_CHUNKS, TerrainChunk};
use crate::vat::{VatMeshes, VatPush};
use crate::world::WorldFrame;

use super::environment::EnvironmentResources;

/// Mesh slots, including the built-in cube in slot 0. Bucketing scratch is sized by this.
pub(crate) const MAX_MESHES: usize = 64;
/// Draw buckets per list: one per mesh slot and level (R8).
const BUCKETS: usize = MAX_MESHES * MAX_LODS;
/// A unit's per-frame decision bits ([`SceneView::decisions`]): in the camera list, in the
/// caster list, and its mesh level in bits 2..4.
const IN_CAMERA: u8 = 1;
const IN_CASTERS: u8 = 2;
const LOD_SHIFT: u8 = 2;
/// Most backdrop rectangles one view clears ([`SceneTargets::clear_rects`]); later ones are
/// ignored.
pub(crate) const MAX_CLEAR_RECTS: usize = 32;
/// Bytes per joint in the bone buffer: three `vec4` rows.
const BONE_STRIDE: u64 = core::mem::size_of::<JointMatrix>() as u64;
const CAMERA_UNIFORM_SIZE: u64 = core::mem::size_of::<CameraUniform>() as u64;
static BONES_TRUNCATED: AtomicU64 = AtomicU64::new(0);
static SKINNED_DROPPED: AtomicU64 = AtomicU64::new(0);

/// Joint matrices from [`Scene::bones`] dropped from the most recent frame because they
/// exceeded [`RenderConfig::max_bones`](crate::RenderConfig::max_bones).
#[must_use]
pub fn bones_truncated() -> u64 {
    BONES_TRUNCATED.load(Ordering::Relaxed)
}

/// Skinned-mesh instances not drawn in the most recent frame: no bone range, a range past
/// the uploaded bones, or more skinned draws than the per-frame budget.
#[must_use]
pub fn skinned_instances_dropped() -> u64 {
    SKINNED_DROPPED.load(Ordering::Relaxed)
}

/// Push-constant block of `skinned.vert`: the draw's bone range (16 bytes, vertex stage).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct SkinPush {
    first_bone: u32,
    joint_count: u32,
    reserved: [u32; 2],
}
const SKIN_PUSH_SIZE: u32 = core::mem::size_of::<SkinPush>() as u32;

/// One entry of the previous-instance buffer (ADR-0021 §2c; `PrevInstance` in
/// `include/motion.glsl`), indexed like the camera list's instances: the previous model
/// matrix as three rows and the previous VAT pose. 64 bytes; only the temporal prepass reads
/// it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct PrevInstance {
    rows: [[f32; 4]; 3],
    /// x from frame | next << 16, y to frame | next << 16, z unorm16 (from t, to t), w unorm16
    /// blend | [`PREV_VALID`].
    pose: [u32; 4],
}

/// `PrevInstance::pose[3]` bit: the instance has a previous state (`MOBA_PREV_VALID`).
pub(crate) const PREV_VALID: u32 = 0x1_0000;

impl PrevInstance {
    /// The entry of an instance drawn as `state` last frame; `None`: no previous state (the
    /// shader then uses the current one: camera motion only).
    pub(crate) fn new(state: Option<&DrawnState>) -> Self {
        let Some(state) = state else {
            return Self::zeroed();
        };
        let m = state.transform.to_cols_array_2d();
        let row = |r: usize| [m[0][r], m[1][r], m[2][r], m[3][r]];
        let frame = |value: u32| value.min(0xFFFF);
        let pose = state.vat;
        Self {
            rows: [row(0), row(1), row(2)],
            pose: [
                frame(pose.from.frame) | (frame(pose.from.next) << 16),
                frame(pose.to.frame) | (frame(pose.to.next) << 16),
                unorm16(pose.from.t) | (unorm16(pose.to.t) << 16),
                unorm16(pose.blend) | PREV_VALID,
            ],
        }
    }
}

/// `packUnorm2x16`'s quantisation of one value (non-finite as 0).
pub(crate) fn unorm16(value: f32) -> u32 {
    let value = if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    };
    // Exact: the product is in 0..=65535.
    (value * 65_535.0).round() as u32
}

/// One skinned instance: drawn alone so its push constant can select its bones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SkinnedDraw {
    slot: u32,
    instance: u32,
    first_bone: u32,
    joint_count: u32,
    /// The mesh level drawn (R8).
    lod: u8,
    /// The previous palette's first bone in this frame's bone buffer (ADR-0021 §2c), when
    /// [`Self::has_previous`].
    previous_bone: u32,
    has_previous: bool,
}

/// Per-frame-in-flight scene buffers and their descriptor sets.
struct SceneFrame {
    camera_buffer: GpuBuffer,
    instance_buffer: GpuBuffer,
    /// Joint matrices for this frame's skinned draws (set 1, binding 1).
    bone_buffer: GpuBuffer,
    camera_set: vk::DescriptorSet,
    instance_set: vk::DescriptorSet,
    /// The previous-instance buffer (set 1, binding 2; ADR-0021 §2c): the main view with
    /// temporal AA only. Without it binding 2 names the instance buffer, which nothing reads.
    previous: Option<GpuBuffer>,
    /// The overlay prepass's camera (ADR-0021 §5c: output resolution, unjittered): the main
    /// view with the overlay only.
    overlay_camera: Option<(GpuBuffer, vk::DescriptorSet)>,
}

/// Single-sample images an MSAA scene resolves into (ADR-0015).
#[derive(Clone, Copy, Debug)]
pub(crate) struct SceneResolve {
    pub(crate) color: vk::ImageView,
    /// The depth resolve target (`SAMPLE_ZERO`); `None` when the prepass already resolved it.
    pub(crate) depth: Option<vk::ImageView>,
}

/// Attachments the scene pass renders into, and how one view is drawn.
#[allow(clippy::struct_excessive_bools)] // Independent per-view switches.
pub(crate) struct SceneTargets<'a> {
    /// The colour attachment (the multisampled one with MSAA).
    pub(crate) color: vk::ImageView,
    /// The depth attachment (the multisampled one with MSAA).
    pub(crate) depth: vk::ImageView,
    /// The render area and viewport: this frame's rendered sub-rectangle (dynamic
    /// resolution) from the top-left corner.
    pub(crate) extent: vk::Extent2D,
    pub(crate) clear_color: [f32; 4],
    /// Draw the terrain plane (the main view and the minimap; portraits do not).
    pub(crate) terrain: bool,
    /// Draw the sky behind the scene (the main view, when the world config has it).
    pub(crate) sky: bool,
    /// Rectangles cleared to their own colour after the full clear (portrait backdrops).
    pub(crate) clear_rects: &'a [ClearRect],
    /// The view's scene lighting set (set 3).
    pub(crate) lighting: vk::DescriptorSet,
    /// MSAA resolve targets (main view only).
    pub(crate) resolve: Option<SceneResolve>,
    /// Load the depth the prepass wrote instead of clearing it (main view with AO).
    pub(crate) load_depth: bool,
    /// Keep the depth contents for a later pass (soft particles); otherwise `DONT_CARE`.
    pub(crate) store_depth: bool,
    /// Draw the inverted-hull outlines of outlined units (the main view, `hull` mode, R2b).
    pub(crate) hull_outlines: bool,
}

/// The depth prepass's attachment (R6, ADR-0015).
pub(crate) struct PrepassTarget {
    /// The depth attachment (the multisampled one with MSAA), stored for the scene.
    pub(crate) depth: vk::ImageView,
    /// The single-sample depth the MSAA depth resolves into, for the AO pass.
    pub(crate) resolve: Option<vk::ImageView>,
    /// R2b's `R8_UINT` outline ID target (cleared to 0), with screen-space outlines.
    pub(crate) outline_ids: Option<OutlineIdTarget>,
    /// The temporal prepass's motion target (ADR-0021 §2b; cleared to 0), first attachment.
    pub(crate) motion: Option<vk::ImageView>,
    /// The main view's lighting set (the temporal prepass's foliage binds it).
    pub(crate) lighting: vk::DescriptorSet,
    pub(crate) extent: vk::Extent2D,
}

/// The `tc_mask` pass's targets (ADR-0021 §2d).
pub(crate) struct TcMaskTarget {
    pub(crate) mask: vk::ImageView,
    /// Read-only (tested and sampled-capable).
    pub(crate) depth: vk::ImageView,
    pub(crate) lighting: vk::DescriptorSet,
    pub(crate) extent: vk::Extent2D,
}

/// The overlay prepass's targets (ADR-0021 §5c), at output resolution.
pub(crate) struct OverlayPrepassTarget {
    pub(crate) depth: vk::ImageView,
    pub(crate) ids: vk::ImageView,
    pub(crate) lighting: vk::DescriptorSet,
    pub(crate) extent: vk::Extent2D,
}

/// What [`ScenePass::prepare`] needs for the main view on the Tiered path (ADR-0021).
#[derive(Clone, Copy, Debug)]
pub(crate) struct MainFrame {
    /// The jittered, unjittered and previous matrices; `None` without temporal AA.
    pub(crate) temporal: Option<TemporalCamera>,
    /// Forget the motion history (`Scene::history_reset`, a cut, a resize).
    pub(crate) history_reset: bool,
    /// The output (swapchain) extent, for the overlay prepass's camera.
    pub(crate) output: vk::Extent2D,
}

/// The prepass's outline ID attachment (R2b, ADR-0016).
#[derive(Clone, Copy, Debug)]
pub(crate) struct OutlineIdTarget {
    /// The attachment (the multisampled one with MSAA).
    pub(crate) view: vk::ImageView,
    /// The single-sample ID image the MSAA one resolves into (`SAMPLE_ZERO`, the integer
    /// resolve mode, so the IDs match the resolved depth's sample), for the outline pass.
    pub(crate) resolve: Option<vk::ImageView>,
}

/// The shadow map the shadow pass renders into.
pub(crate) struct ShadowTarget {
    pub(crate) depth: vk::ImageView,
    pub(crate) size: u32,
}

/// Slope-scaled depth bias of the shadow casters (in units of the depth format's minimum
/// resolvable difference, and of the polygon's depth slope); the shader adds a normal
/// offset of 1.5 texels on top (ADR-0011).
const SHADOW_BIAS_CONSTANT: f32 = 1.25;
const SHADOW_BIAS_SLOPE: f32 = 1.75;

/// Per-view budget: the instance and joint-matrix capacity of its per-frame buffers.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ViewBudget {
    pub(crate) max_instances: usize,
    pub(crate) max_bones: usize,
    /// The view also builds a shadow caster list (the main view with shadows, R8): its
    /// instance buffer holds `2 × max_instances`.
    pub(crate) casters: bool,
    /// The view keeps a motion history and writes previous instances and palettes (the main
    /// view with temporal AA, ADR-0021 §2c): its bone buffer holds `2 × max_bones`.
    pub(crate) temporal: bool,
    /// The view has the overlay prepass's camera (the main view on Tiered, ADR-0021 §5c).
    pub(crate) overlay: bool,
}

/// How a scene pass culls and selects levels (R8, ADR-0018), fixed at creation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CullSettings {
    pub(crate) mode: CullMode,
    pub(crate) lod: LodConfig,
    /// The tier's per-model triangle budget: a mesh over it draws its first level within it
    /// as its finest.
    pub(crate) model_triangles: usize,
}

/// One list of instances, bucketed by mesh and level: the camera list (prepass, scene,
/// hulls) or the main view's caster list (the shadow pass).
struct DrawList {
    /// The list's first instance slot in its view's instance buffer.
    base: u32,
    /// Per bucket (`slot * MAX_LODS + level`): instances, outlined instances (which lead the
    /// bucket, so the hulls draw exactly them), first instance slot (absolute) and the write
    /// cursors of the outlined and the plain instances.
    counts: [u32; BUCKETS],
    outlined: [u32; BUCKETS],
    starts: [u32; BUCKETS],
    cursors: [u32; BUCKETS],
    plain_cursors: [u32; BUCKETS],
    /// Per-frame skinned draws; capacity fixed at creation so pushing never reallocates.
    skinned: Vec<SkinnedDraw>,
    /// Instances in the list this frame.
    len: u32,
}

impl DrawList {
    fn new(base: u32, skinned_capacity: usize) -> Self {
        Self {
            base,
            counts: [0; BUCKETS],
            outlined: [0; BUCKETS],
            starts: [0; BUCKETS],
            cursors: [0; BUCKETS],
            plain_cursors: [0; BUCKETS],
            skinned: Vec::with_capacity(skinned_capacity),
            len: 0,
        }
    }

    /// Counting-sort bucketing of the `units` whose decision has `bit`, by mesh and level,
    /// without allocating. On return `counts[b]` is bucket `b`'s instances, `starts[b]` its
    /// first slot (from `base`), `cursors[b] == starts[b]` and `plain_cursors[b]` the first
    /// slot after its outlined instances. Buckets are in (mesh slot, level) order and keep
    /// scene order inside, so a scene that only uses the cube at LOD0 is laid out as before.
    fn bucket(
        &mut self,
        units: &[crate::UnitInstance],
        decisions: &[(u32, u8)],
        bit: u8,
        mesh_count: usize,
    ) {
        self.counts.fill(0);
        self.outlined.fill(0);
        self.skinned.clear();
        for &(index, decision) in decisions {
            if decision & bit == 0 {
                continue;
            }
            let unit = &units[index as usize];
            let bucket = bucket_of(mesh_slot(unit.mesh, mesh_count), decision);
            self.counts[bucket] += 1;
            self.outlined[bucket] += u32::from(is_outlined(unit));
        }
        let mut next = self.base;
        for bucket in 0..BUCKETS {
            self.starts[bucket] = next;
            self.plain_cursors[bucket] = next + self.outlined[bucket];
            next += self.counts[bucket];
        }
        self.cursors = self.starts;
        self.len = next - self.base;
    }

    /// The slot of the next instance of `unit` in `bucket` (outlined first), advancing its
    /// cursor. Only for units counted by [`Self::bucket`].
    fn place(&mut self, unit: &crate::UnitInstance, bucket: usize) -> u32 {
        let cursor = if is_outlined(unit) {
            &mut self.cursors[bucket]
        } else {
            &mut self.plain_cursors[bucket]
        };
        let instance = *cursor;
        *cursor += 1;
        instance
    }
}

/// Keeps, in scene order, the `(unit index, decision)` of every unit `decide` puts in a list
/// (camera or casters), up to `capacity` (R8 review: the instance cap applies after culling,
/// so off-screen units never push visible ones out at the cap). Returns how many units a
/// list wanted that did not fit. `out` is cleared first and never grows past `capacity`.
fn keep_drawn(
    units: &[crate::UnitInstance],
    capacity: usize,
    out: &mut Vec<(u32, u8)>,
    mut decide: impl FnMut(&crate::UnitInstance) -> u8,
) -> u32 {
    out.clear();
    let mut dropped = 0u32;
    for (index, unit) in units.iter().enumerate() {
        let decision = decide(unit);
        if decision & (IN_CAMERA | IN_CASTERS) == 0 {
            continue;
        }
        match u32::try_from(index) {
            Ok(index) if out.len() < capacity.min(out.capacity()) => out.push((index, decision)),
            _ => dropped = dropped.saturating_add(1),
        }
    }
    dropped
}

/// The bucket of a unit of mesh `slot` with `decision`'s level.
fn bucket_of(slot: usize, decision: u8) -> usize {
    slot * MAX_LODS + usize::from(decision >> LOD_SHIFT) % MAX_LODS
}

/// One view's per-frame buffers and its bucketing scratch.
struct SceneView {
    frames: Vec<SceneFrame>,
    /// The camera list (prepass, scene, hulls).
    camera: DrawList,
    /// The shadow caster list (the main view with shadows, R8), from slot `max_instances`.
    casters: Option<DrawList>,
    /// Per unit this frame: [`IN_CAMERA`], [`IN_CASTERS`] and the level; capacity
    /// `max_instances`, never grown.
    /// `(unit index, decision bits)` of the units some list draws, in scene order, at most
    /// `max_instances` (capacity fixed at creation).
    decisions: Vec<(u32, u8)>,
    /// The terrain's visible index runs this frame; capacity [`MAX_TERRAIN_CHUNKS`].
    terrain_runs: Vec<(u32, u32)>,
    /// Units handed in for the frame being recorded (after the cap).
    unit_count: usize,
    max_instances: usize,
    max_bones: usize,
}

/// Every scene pipeline: the vertex-colour set and the `MATERIAL_TEXTURED` permutations.
pub(crate) struct ScenePipelines {
    unit: GraphicsPipeline,
    unit_textured: GraphicsPipeline,
    terrain: GraphicsPipeline,
    /// `None` when skinning is disabled or the pipeline failed to build.
    skinned: Option<GraphicsPipeline>,
    /// `None` with `skinned`, or if the textured skinned permutation failed to build (then
    /// textured skinned meshes draw with `skinned`).
    skinned_textured: Option<GraphicsPipeline>,
    /// Depth-only static caster; `None` when shadows are off.
    unit_shadow: Option<GraphicsPipeline>,
    /// Depth-only skinned caster; `None` when shadows or skinning are off (then skinned
    /// meshes cast no shadow).
    skinned_shadow: Option<GraphicsPipeline>,
    /// Vertex-animation-texture pipeline (R4b); `None` if it failed to build (then
    /// [`ScenePass::register_vat_mesh`] fails and minions keep their fallback).
    vat: Option<GraphicsPipeline>,
    /// Depth-only VAT caster; `None` when shadows are off or `vat` is.
    vat_shadow: Option<GraphicsPipeline>,
    /// The far-plane sky (R3a): depth test only, no vertex input; built with the set's
    /// samples, so the MSAA set has its own (R6).
    sky: GraphicsPipeline,
    /// The depth prepass (R6); `None` without one (AO, contact shadows or screen-space
    /// outlines ask for it).
    prepass: Option<PrepassPipelines>,
    /// Inverted-hull outlines (R2b): static, skinned and VAT; `None` unless this set draws
    /// the main view with `hull` outlines (the skinned and VAT ones also without skinning or
    /// VAT).
    hull_unit: Option<GraphicsPipeline>,
    hull_skinned: Option<GraphicsPipeline>,
    hull_vat: Option<GraphicsPipeline>,
    /// Procedural foliage (R3b): no vertex input, the foliage layout, the set's samples.
    foliage: GraphicsPipeline,
    /// Simple water: premultiplied alpha, depth tested only, the water layout, the set's
    /// samples (R3b).
    water: GraphicsPipeline,
    /// Full water with refraction (R3b), single-sampled after any resolve; `None` unless the
    /// full water passes run (built in the single-sample set only).
    water_full: Option<GraphicsPipeline>,
    /// Water surfaces into the transparency-and-composition mask (ADR-0021 §2d); temporal
    /// only.
    tc_mask: Option<GraphicsPipeline>,
    /// The overlay prepass (ADR-0021 §5c); Tiered only.
    overlay: Option<OverlayPipelines>,
}

/// Pipelines of the depth prepass: depth plus, with screen-space outlines, the units' outline
/// slots into the ID target (R2b). The `*_prepass.vert` shaders compute the same `invariant
/// gl_Position` as the scene's, so the scene pass reproduces the depth exactly.
pub(crate) struct PrepassPipelines {
    terrain: GraphicsPipeline,
    unit: GraphicsPipeline,
    /// `None` without skinning, or if it failed to build (skinned meshes are then drawn by
    /// the scene pass alone: correct, without early rejection, AO depth or an outline ID).
    skinned: Option<GraphicsPipeline>,
    /// `None` with [`ScenePipelines::vat`] (uses the VAT layout).
    vat: Option<GraphicsPipeline>,
    /// The foliage (temporal prepass only, ADR-0021 §2b: brush joins the prepass).
    foliage: Option<GraphicsPipeline>,
    /// The background's camera motion at the far plane (temporal prepass only).
    sky: Option<GraphicsPipeline>,
}

/// The overlay prepass (ADR-0021 §5c): output resolution, unjittered, depth + IDs (terrain
/// and water `GROUND`, units their slot or `OCCLUDER`).
pub(crate) struct OverlayPipelines {
    terrain: GraphicsPipeline,
    unit: GraphicsPipeline,
    skinned: Option<GraphicsPipeline>,
    vat: Option<GraphicsPipeline>,
    water: GraphicsPipeline,
}

/// Layouts and mode the R3b pipelines are built for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EnvironmentLayouts {
    pub(crate) foliage: vk::PipelineLayout,
    pub(crate) water: vk::PipelineLayout,
    pub(crate) full_water: bool,
    /// Decals are on (`EnvironmentConfig::decals`); off, the terrain uses
    /// `terrain_nodecals.frag`, its decal code compiled out.
    pub(crate) decals: bool,
}

/// Attachments of the full water pass (R3b).
pub(crate) struct WaterTargets {
    pub(crate) hdr: vk::ImageView,
    pub(crate) depth: vk::ImageView,
    pub(crate) extent: vk::Extent2D,
    pub(crate) lighting: vk::DescriptorSet,
}

/// What one [`ScenePipelines`] set is built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SceneSetSpec {
    pub(crate) samples: vk::SampleCountFlags,
    /// Build the shadow casters (the single-sample set only).
    pub(crate) shadow: Option<ShadowPipelineSpec>,
    /// Build the prepass pipelines (the main view's set, with AO, contact shadows or
    /// screen-space outlines).
    pub(crate) prepass: bool,
    /// The prepass also writes the outline ID target (R2b's screen-space outlines).
    pub(crate) outline_ids: bool,
    /// Build the inverted-hull outline pipelines (the main view's set, R2b's `hull` mode).
    pub(crate) hull: bool,
    /// The prepass writes motion (ADR-0021 §2b) and the `tc_mask` pipeline is built.
    pub(crate) temporal: bool,
    /// Tiered (ADR-0021 §5): glow decals leave the scene shaders and the overlay prepass is
    /// built.
    pub(crate) overlay: bool,
}

impl ScenePipelines {
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One pipeline per permutation.
    fn build(
        device: &DeviceContext,
        shaders: &ShaderSource,
        layout: vk::PipelineLayout,
        vat_layout: vk::PipelineLayout,
        color_format: vk::Format,
        depth_format: vk::Format,
        set: SceneSetSpec,
        skinning: bool,
        environment: EnvironmentLayouts,
    ) -> RenderResult<Self> {
        let SceneSetSpec {
            samples,
            shadow,
            prepass,
            outline_ids,
            hull,
            temporal,
            overlay,
        } = set;
        let module = |name| shaders.module(device, name);
        let unit_vertex = module("unit.vert")?;
        let unit_fragment = module("unit.frag")?;
        let textured_vertex = module("unit_textured.vert")?;
        let textured_fragment = module("unit_textured.frag")?;
        let terrain_vertex = module("terrain.vert")?;
        // ADR-0021 §5b: on Tiered the glow decals draw in the overlay, so the scene's terrain,
        // foliage and water compile them out.
        let terrain_fragment = module(match (environment.decals, overlay) {
            (false, _) => "terrain_nodecals.frag",
            (true, false) => "terrain.frag",
            (true, true) => "terrain_tiered.frag",
        })?;
        let build = |vertex_layout, vertex: vk::ShaderModule, fragment: vk::ShaderModule| {
            pipeline::create(
                device,
                &PipelineSpec {
                    layout,
                    vertex_layout,
                    vertex,
                    fragment,
                    color_format,
                    depth_format,
                    cull_mode: vk::CullModeFlags::NONE,
                    samples,
                    depth_write: true,
                },
            )
            .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))
        };
        let unit = build(
            VertexLayout::Static,
            unit_vertex.handle(),
            unit_fragment.handle(),
        )?;
        let unit_textured = build(
            VertexLayout::StaticTextured,
            textured_vertex.handle(),
            textured_fragment.handle(),
        )?;
        let terrain = build(
            VertexLayout::Terrain,
            terrain_vertex.handle(),
            terrain_fragment.handle(),
        )?;
        // `RenderConfig::pipeline_stats`: the terrain shader carries the decal code and sits
        // near a register step on some GPUs (ADR-0017 §1), so its compiled statistics are
        // logged for `scripts/terrain-decal-check.sh`.
        for line in device.pipeline_statistics(terrain.handle()) {
            eprintln!(
                "[render] pipeline-stats terrain samples={} {line}",
                samples.as_raw()
            );
        }
        let sky_vertex = module("sky.vert")?;
        let sky_fragment = module("sky.frag")?;
        let sky = pipeline::create_vertexless(
            device,
            layout,
            sky_vertex.handle(),
            sky_fragment.handle(),
            color_format,
            false,
            VertexlessDepth::TestOnly(depth_format, samples),
        )
        .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))?;
        let (skinned, skinned_textured) = if skinning {
            let skinned = module("skinned.vert").and_then(|vertex| {
                build(
                    VertexLayout::Skinned,
                    vertex.handle(),
                    unit_fragment.handle(),
                )
            });
            match skinned {
                Ok(skinned) => {
                    let textured = module("skinned_textured.vert").and_then(|vertex| {
                        build(
                            VertexLayout::SkinnedTextured,
                            vertex.handle(),
                            textured_fragment.handle(),
                        )
                    });
                    let textured = textured
                        .inspect_err(|error| {
                            eprintln!(
                                "[render] textured skinned pipeline unavailable ({error}); \
                                 textured skinned meshes use the vertex-colour look"
                            );
                        })
                        .ok();
                    (Some(skinned), textured)
                }
                Err(error) => {
                    eprintln!(
                        "[render] skinned pipeline unavailable ({error}); using static meshes"
                    );
                    (None, None)
                }
            }
        } else {
            (None, None)
        };
        let shadow_caster = |name, vertex_layout, spec: ShadowPipelineSpec| {
            module(name).and_then(|vertex| {
                pipeline::create_depth_only(
                    device,
                    layout,
                    vertex_layout,
                    vertex.handle(),
                    spec.format,
                    DepthOnly::shadow(spec.depth_clamp),
                )
                .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))
            })
        };
        let vat = module("vat.vert")
            .and_then(|vertex| {
                pipeline::create(
                    device,
                    &PipelineSpec {
                        layout: vat_layout,
                        vertex_layout: VertexLayout::Static,
                        vertex: vertex.handle(),
                        fragment: unit_fragment.handle(),
                        color_format,
                        depth_format,
                        cull_mode: vk::CullModeFlags::NONE,
                        samples,
                        depth_write: true,
                    },
                )
                .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))
            })
            .inspect_err(|error| {
                eprintln!(
                    "[render] VAT pipeline unavailable ({error}); minions use their fallback"
                );
            })
            .ok();
        let vat_shadow = match (shadow, &vat) {
            (Some(spec), Some(_)) => module("vat_shadow.vert")
                .and_then(|vertex| {
                    pipeline::create_depth_only(
                        device,
                        vat_layout,
                        VertexLayout::StaticShadow,
                        vertex.handle(),
                        spec.format,
                        DepthOnly::shadow(spec.depth_clamp),
                    )
                    .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))
                })
                .inspect_err(|error| {
                    eprintln!(
                        "[render] VAT shadow pipeline unavailable ({error}); VAT meshes cast no shadow"
                    );
                })
                .ok(),
            _ => None,
        };
        // R3b foliage and water.
        // With temporal AA the foliage is in the prepass, so its position is `invariant` to
        // reproduce that depth (ADR-0021 §2b).
        let foliage_vertex = module(if temporal {
            "foliage_tiered.vert"
        } else {
            "foliage.vert"
        })?;
        let foliage_fragment = module(if overlay {
            "foliage_tiered.frag"
        } else {
            "foliage.frag"
        })?;
        let foliage = pipeline::create_vertexless(
            device,
            environment.foliage,
            foliage_vertex.handle(),
            foliage_fragment.handle(),
            color_format,
            false,
            VertexlessDepth::ReadWrite(depth_format, samples),
        )
        .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))?;
        let water_vertex = module("water.vert")?;
        let water_spec = |fragment: vk::ShaderModule, samples| PipelineSpec {
            layout: environment.water,
            vertex_layout: VertexLayout::Water,
            vertex: water_vertex.handle(),
            fragment,
            color_format,
            depth_format,
            cull_mode: vk::CullModeFlags::NONE,
            samples,
            // Water never writes depth (the particles and decals see the bed through it).
            depth_write: false,
        };
        let water_fragment = module(if overlay {
            "water_tiered.frag"
        } else {
            "water.frag"
        })?;
        let water = pipeline::create_blended(
            device,
            &water_spec(water_fragment.handle(), samples),
            SceneBlend::Premultiplied,
        )
        .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))?;
        // The full water draws after any MSAA resolve: single-sampled, in the base set only.
        let water_full = if environment.full_water && samples == vk::SampleCountFlags::TYPE_1 {
            let fragment = module(if overlay {
                "water_refraction_tiered.frag"
            } else {
                "water_refraction.frag"
            })?;
            Some(
                pipeline::create_blended(
                    device,
                    &water_spec(fragment.handle(), vk::SampleCountFlags::TYPE_1),
                    SceneBlend::OpaqueTestOnly,
                )
                .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))?,
            )
        } else {
            None
        };
        let (unit_shadow, skinned_shadow) = match shadow {
            Some(spec) => {
                let unit_shadow =
                    shadow_caster("unit_shadow.vert", VertexLayout::StaticShadow, spec)?;
                let skinned_shadow = if skinned.is_some() {
                    shadow_caster("skinned_shadow.vert", VertexLayout::SkinnedShadow, spec)
                        .inspect_err(|error| {
                            eprintln!(
                                "[render] skinned shadow pipeline unavailable ({error}); \
                                 skinned meshes cast no shadow"
                            );
                        })
                        .ok()
                } else {
                    None
                };
                (Some(unit_shadow), skinned_shadow)
            }
            None => (None, None),
        };
        let prepass = if !prepass {
            None
        } else if temporal {
            Some(Self::build_motion_prepass(
                device,
                shaders,
                layout,
                vat_layout,
                environment.foliage,
                depth_format,
                samples,
                outline_ids,
                skinned.is_some(),
                vat.is_some(),
            )?)
        } else {
            let id_format = outline_ids.then_some(crate::frame_graph::OUTLINE_ID_FORMAT);
            let id_fragment = if outline_ids {
                Some(module("prepass.frag")?)
            } else {
                None
            };
            let fragment = id_fragment
                .as_ref()
                .map(crate::resources::ShaderModule::handle);
            let prepass_with = |layout, vertex: vk::ShaderModule, vertex_layout, fragment| {
                pipeline::create_prepass(
                    device,
                    layout,
                    vertex_layout,
                    vertex,
                    fragment,
                    id_format,
                    depth_format,
                    samples,
                )
                .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))
            };
            // The terrain has no outline: depth only, the ID target keeps its clear (0).
            let terrain =
                prepass_with(layout, terrain_vertex.handle(), VertexLayout::Terrain, None)?;
            let unit_prepass = module("unit_prepass.vert")?;
            let unit = prepass_with(
                layout,
                unit_prepass.handle(),
                VertexLayout::StaticShadow,
                fragment,
            )?;
            let skinned = if skinned.is_some() {
                module("skinned_prepass.vert")
                    .and_then(|vertex| {
                        prepass_with(
                            layout,
                            vertex.handle(),
                            VertexLayout::SkinnedShadow,
                            fragment,
                        )
                    })
                    .inspect_err(|error| {
                        eprintln!("[render] skinned prepass pipeline unavailable ({error})");
                    })
                    .ok()
            } else {
                None
            };
            // Minions (R4b) are in the prepass too: their depth feeds GTAO, contact shadows
            // and the particles, occludes outlines, and their slot reaches the ID target.
            let vat_prepass = match &vat {
                Some(_) => Some(prepass_with(
                    vat_layout,
                    module("vat_prepass.vert")?.handle(),
                    VertexLayout::StaticShadow,
                    fragment,
                )?),
                None => None,
            };
            Some(PrepassPipelines {
                terrain,
                unit,
                skinned,
                vat: vat_prepass,
                foliage: None,
                sky: None,
            })
        };
        // ADR-0021 §2d: water surfaces, depth-tested, into the T&C mask (MAX).
        let tc_mask = if temporal && samples == vk::SampleCountFlags::TYPE_1 {
            let fragment = module("tc_mask.frag")?;
            let target = [ColorTarget {
                format: crate::frame_graph::MASK_FORMAT,
                write_mask: vk::ColorComponentFlags::R,
                blend: TargetBlend::Max,
            }];
            Some(
                pipeline::create_targets(
                    device,
                    &TargetsSpec {
                        layout: environment.water,
                        vertex_layout: Some(VertexLayout::Water),
                        vertex: water_vertex.handle(),
                        fragment: Some(fragment.handle()),
                        colors: &target,
                        depth_format,
                        depth_write: false,
                        samples,
                    },
                )
                .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))?,
            )
        } else {
            None
        };
        let overlay = if overlay && samples == vk::SampleCountFlags::TYPE_1 {
            Some(Self::build_overlay(
                device,
                shaders,
                layout,
                vat_layout,
                environment.water,
                skinned.is_some(),
                vat.is_some(),
            )?)
        } else {
            None
        };
        let (hull_unit, hull_skinned, hull_vat) = if hull {
            let fragment = module("outline_hull.frag")?;
            let hull_pipeline = |layout, vertex_layout, vertex: vk::ShaderModule| {
                pipeline::create(
                    device,
                    &PipelineSpec {
                        layout,
                        vertex_layout,
                        vertex,
                        fragment: fragment.handle(),
                        color_format,
                        depth_format,
                        cull_mode: vk::CullModeFlags::NONE,
                        samples,
                        depth_write: false,
                    },
                )
                .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))
            };
            let unit_vertex = module("unit_outline.vert")?;
            let unit = hull_pipeline(layout, VertexLayout::Static, unit_vertex.handle())?;
            let skinned = if skinned.is_some() {
                module("skinned_outline.vert")
                    .and_then(|vertex| {
                        hull_pipeline(layout, VertexLayout::Skinned, vertex.handle())
                    })
                    .inspect_err(|error| {
                        eprintln!(
                            "[render] skinned outline pipeline unavailable ({error}); \
                             skinned meshes are not outlined"
                        );
                    })
                    .ok()
            } else {
                None
            };
            let vat_hull = if vat.is_some() {
                module("vat_outline.vert")
                    .and_then(|vertex| {
                        hull_pipeline(vat_layout, VertexLayout::Static, vertex.handle())
                    })
                    .inspect_err(|error| {
                        eprintln!(
                            "[render] VAT outline pipeline unavailable ({error}); \
                             VAT meshes are not outlined"
                        );
                    })
                    .ok()
            } else {
                None
            };
            (Some(unit), skinned, vat_hull)
        } else {
            (None, None, None)
        };
        Ok(Self {
            unit,
            unit_textured,
            terrain,
            skinned,
            skinned_textured,
            unit_shadow,
            skinned_shadow,
            vat,
            vat_shadow,
            sky,
            prepass,
            hull_unit,
            hull_skinned,
            hull_vat,
            foliage,
            water,
            water_full,
            tc_mask,
            overlay,
        })
    }

    /// The temporal prepass (ADR-0021 §2b): every opaque main-view draw writes depth and its
    /// motion (attachment 0, `R16G16_SFLOAT`), the units also their slot into the unit mask
    /// (attachment 1) when `ids` (the heat haze keeps it). The terrain, the foliage and the
    /// background leave the mask alone (an empty write mask; `independentBlend`).
    #[allow(clippy::too_many_arguments)] // One set of pipelines.
    fn build_motion_prepass(
        device: &DeviceContext,
        shaders: &ShaderSource,
        layout: vk::PipelineLayout,
        vat_layout: vk::PipelineLayout,
        foliage_layout: vk::PipelineLayout,
        depth_format: vk::Format,
        samples: vk::SampleCountFlags,
        ids: bool,
        skinned: bool,
        vat: bool,
    ) -> RenderResult<PrepassPipelines> {
        let module = |name| shaders.module(device, name);
        let motion_fragment = module("prepass_motion.frag")?;
        let ids_fragment = module(if ids {
            "prepass_motion_ids.frag"
        } else {
            "prepass_motion.frag"
        })?;
        let motion = ColorTarget {
            format: crate::frame_graph::MOTION_FORMAT,
            write_mask: vk::ColorComponentFlags::R | vk::ColorComponentFlags::G,
            blend: TargetBlend::None,
        };
        let id = |write: bool| ColorTarget {
            format: crate::frame_graph::OUTLINE_ID_FORMAT,
            write_mask: if write {
                vk::ColorComponentFlags::R
            } else {
                vk::ColorComponentFlags::empty()
            },
            blend: TargetBlend::None,
        };
        let count = 1 + usize::from(ids);
        let plain = [motion, id(false)];
        let with_ids = [motion, id(true)];
        let build = |layout,
                     vertex_layout,
                     vertex: vk::ShaderModule,
                     fragment: vk::ShaderModule,
                     colors: &[ColorTarget],
                     depth_write| {
            pipeline::create_targets(
                device,
                &TargetsSpec {
                    layout,
                    vertex_layout,
                    vertex,
                    fragment: Some(fragment),
                    colors,
                    depth_format,
                    depth_write,
                    samples,
                },
            )
            .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))
        };
        let terrain = build(
            layout,
            Some(VertexLayout::Terrain),
            module("terrain_motion.vert")?.handle(),
            motion_fragment.handle(),
            &plain[..count],
            true,
        )?;
        let unit = build(
            layout,
            Some(VertexLayout::StaticShadow),
            module("unit_motion.vert")?.handle(),
            ids_fragment.handle(),
            &with_ids[..count],
            true,
        )?;
        let skinned = if skinned {
            module("skinned_motion.vert")
                .and_then(|vertex| {
                    build(
                        layout,
                        Some(VertexLayout::SkinnedShadow),
                        vertex.handle(),
                        ids_fragment.handle(),
                        &with_ids[..count],
                        true,
                    )
                })
                .inspect_err(|error| {
                    eprintln!("[render] skinned motion prepass pipeline unavailable ({error})");
                })
                .ok()
        } else {
            None
        };
        let vat = if vat {
            Some(build(
                vat_layout,
                Some(VertexLayout::StaticShadow),
                module("vat_motion.vert")?.handle(),
                ids_fragment.handle(),
                &with_ids[..count],
                true,
            )?)
        } else {
            None
        };
        let foliage = build(
            foliage_layout,
            None,
            module("foliage_motion.vert")?.handle(),
            motion_fragment.handle(),
            &plain[..count],
            true,
        )?;
        // On the far plane, tested only: where nothing was drawn.
        let sky = build(
            layout,
            None,
            module("sky.vert")?.handle(),
            module("sky_motion.frag")?.handle(),
            &plain[..count],
            false,
        )?;
        Ok(PrepassPipelines {
            terrain,
            unit,
            skinned,
            vat,
            foliage: Some(foliage),
            sky: Some(sky),
        })
    }

    /// The overlay prepass (ADR-0021 §5c): the `*_prepass.vert` shaders through the overlay
    /// camera; the units write their slot (or `OCCLUDER`, 8) with `prepass.frag`, the terrain
    /// and the water `GROUND` (9) with `overlay_ground.frag`.
    fn build_overlay(
        device: &DeviceContext,
        shaders: &ShaderSource,
        layout: vk::PipelineLayout,
        vat_layout: vk::PipelineLayout,
        water_layout: vk::PipelineLayout,
        skinned: bool,
        vat: bool,
    ) -> RenderResult<OverlayPipelines> {
        let module = |name| shaders.module(device, name);
        let ground = module("overlay_ground.frag")?;
        let unit_fragment = module("prepass.frag")?;
        let ids = [ColorTarget {
            format: crate::frame_graph::OUTLINE_ID_FORMAT,
            write_mask: vk::ColorComponentFlags::R,
            blend: TargetBlend::None,
        }];
        let build =
            |layout, vertex_layout, vertex: vk::ShaderModule, fragment: vk::ShaderModule| {
                pipeline::create_targets(
                    device,
                    &TargetsSpec {
                        layout,
                        vertex_layout: Some(vertex_layout),
                        vertex,
                        fragment: Some(fragment),
                        colors: &ids,
                        depth_format: crate::frame_graph::OVERLAY_DEPTH_FORMAT,
                        depth_write: true,
                        samples: vk::SampleCountFlags::TYPE_1,
                    },
                )
                .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))
            };
        let terrain = build(
            layout,
            VertexLayout::Terrain,
            module("terrain.vert")?.handle(),
            ground.handle(),
        )?;
        let unit = build(
            layout,
            VertexLayout::StaticShadow,
            module("unit_prepass.vert")?.handle(),
            unit_fragment.handle(),
        )?;
        let skinned = if skinned {
            Some(build(
                layout,
                VertexLayout::SkinnedShadow,
                module("skinned_prepass.vert")?.handle(),
                unit_fragment.handle(),
            )?)
        } else {
            None
        };
        let vat = if vat {
            Some(build(
                vat_layout,
                VertexLayout::StaticShadow,
                module("vat_prepass.vert")?.handle(),
                unit_fragment.handle(),
            )?)
        } else {
            None
        };
        let water = build(
            water_layout,
            VertexLayout::Water,
            module("water.vert")?.handle(),
            ground.handle(),
        )?;
        Ok(OverlayPipelines {
            terrain,
            unit,
            skinned,
            vat,
            water,
        })
    }
}

/// How the shadow casters are built: the shadow map's format and whether depth clamping
/// ("pancaking") is enabled, so casters nearer the light than the map's near plane still
/// write depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShadowPipelineSpec {
    pub(crate) format: vk::Format,
    pub(crate) depth_clamp: bool,
}

/// Pipelines, meshes and per-frame buffers of the scene pass.
pub(crate) struct ScenePass {
    /// The R2a plane, drawn without a map terrain.
    terrain_mesh: GpuMesh,
    /// The map's heightfield mesh (R3a, `Renderer::set_terrain`), drawn instead of the plane.
    map_terrain: Option<GpuMesh>,
    /// Unit meshes: slot 0 is the cube, the rest come from `register_mesh`. Never shrinks,
    /// so a `MeshId` stays valid and no frame in flight can reference a freed mesh.
    meshes: Vec<GpuMesh>,
    /// Per mesh slot: its material's descriptor set, or null for the vertex-colour look.
    mesh_material_sets: [vk::DescriptorSet; MAX_MESHES],
    /// View 0 is the main scene; the rest are offscreen views (R7b).
    views: Vec<SceneView>,
    _descriptor_pool: DescriptorPool,
    _camera_layout: DescriptorSetLayout,
    _instance_layout: DescriptorSetLayout,
    /// Per-frame-in-flight camera uniform layout (set 0), for passes that read the main
    /// view's camera (R2b's contact shadows).
    camera_layout_handle: vk::DescriptorSetLayout,
    pipeline_layout: PipelineLayout,
    /// Single-sample pipelines: the offscreen views, the shadow casters and, without MSAA,
    /// the main view (and its prepass).
    pipelines: ScenePipelines,
    /// The main view's MSAA pipelines (and its prepass), when MSAA is on (ADR-0015).
    msaa_pipelines: Option<ScenePipelines>,
    color_format: vk::Format,
    depth_format: vk::Format,
    /// The shadow casters' spec; `None` when shadows are off.
    shadow: Option<ShadowPipelineSpec>,
    /// The main view's sample count and whether it has a prepass.
    main: MainViewSpec,
    skinning: bool,
    /// VAT textures and sets (R4b).
    vat: VatMeshes,
    /// Layout of the VAT pipelines: sets 0/1/3 as `pipeline_layout`, set 2 the VAT set.
    vat_pipeline_layout: PipelineLayout,
    /// Per mesh slot: its index in `vat`, or `u8::MAX` for a mesh without a VAT.
    mesh_vat: [u8; MAX_MESHES],
    /// Per mesh slot: the finest level this tier draws (its triangle budget, R8).
    mesh_finest: [u8; MAX_MESHES],
    /// The map terrain's culling chunks (R8); empty for the R2a plane.
    terrain_chunks: Vec<TerrainChunk>,
    /// Culling and level selection (R8).
    culling: CullSettings,
    /// What the main view culled in the latest frame.
    stats: CullStats,
    /// Foliage and water (R3b).
    environment: EnvironmentResources,
    /// What each keyed instance drew last frame (ADR-0021 §2c); the main view with temporal
    /// AA only.
    motion: Option<MotionHistory>,
}

/// How the main view is drawn (ADR-0015).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MainViewSpec {
    pub(crate) samples: vk::SampleCountFlags,
    pub(crate) prepass: bool,
    /// The prepass writes R2b's outline IDs (screen-space outlines).
    pub(crate) outline_ids: bool,
    /// The scene draws R2b's inverted-hull outlines.
    pub(crate) hull: bool,
    /// Temporal AA (ADR-0021): the prepass writes motion. Never with MSAA (Tiered refuses
    /// it, §7).
    pub(crate) temporal: bool,
    /// The Tiered path's overlay (ADR-0021 §5). Never with MSAA.
    pub(crate) overlay: bool,
}

impl MainViewSpec {
    fn sets(self, shadow: Option<ShadowPipelineSpec>) -> (SceneSetSpec, Option<SceneSetSpec>) {
        let msaa = self.samples != vk::SampleCountFlags::TYPE_1;
        let base = SceneSetSpec {
            samples: vk::SampleCountFlags::TYPE_1,
            shadow,
            prepass: self.prepass && !msaa,
            outline_ids: self.outline_ids && !msaa,
            hull: self.hull && !msaa,
            temporal: self.temporal && !msaa,
            overlay: self.overlay && !msaa,
        };
        let main = msaa.then_some(SceneSetSpec {
            samples: self.samples,
            shadow: None,
            prepass: self.prepass,
            outline_ids: self.outline_ids,
            hull: self.hull,
            temporal: false,
            overlay: false,
        });
        (base, main)
    }
}

/// Bindings of the scene's set 0 (the camera block, vertex + fragment) and set 1 (binding 0
/// instances, binding 1 joint matrices read only by `skinned.vert`, binding 2 the previous
/// instances read only by the temporal prepass, ADR-0021 §2c; vertex).
pub(crate) fn buffer_set_layout_bindings() -> (
    [vk::DescriptorSetLayoutBinding<'static>; 1],
    [vk::DescriptorSetLayoutBinding<'static>; 3],
) {
    let camera = [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)];
    let storage = |binding| {
        vk::DescriptorSetLayoutBinding::default()
            .binding(binding)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::VERTEX)
    };
    (camera, [storage(0), storage(1), storage(2)])
}

impl ScenePass {
    /// Builds the scene pipelines for `color_format`/`depth_format` and, per view in `views`
    /// (view 0 is the main scene), `frames` sets of per-frame buffers. Each view's
    /// `max_bones` is clamped to the storage-buffer range; the main view's is returned
    /// through the pass (see [`Self::max_bones`]). `material_layout` and `lighting_layout` are
    /// sets 2 and 3 of every scene pipeline; `shadow` builds the shadow casters.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)] // Wiring belongs together.
    pub(crate) fn new(
        device: &DeviceContext,
        shaders: &ShaderSource,
        frames: usize,
        color_format: vk::Format,
        depth_format: vk::Format,
        shadow: Option<ShadowPipelineSpec>,
        material_layout: vk::DescriptorSetLayout,
        lighting_layout: vk::DescriptorSetLayout,
        views: &[ViewBudget],
        main: MainViewSpec,
        skinning: bool,
        culling: CullSettings,
        environment: EnvironmentConfig,
    ) -> RenderResult<Self> {
        let (camera_binding, instance_binding) = buffer_set_layout_bindings();
        // Every scene pipeline shares one layout, so its per-stage descriptor totals must fit
        // the device (R3a review: later set-3 bindings count for every scene pipeline).
        // So does the VAT layout (R4b), which swaps set 2 for the VAT texture.
        let limits = crate::descriptor_budget::StageTotals {
            samplers: device.max_per_stage_samplers,
            sampled_images: device.max_per_stage_sampled_images,
            storage_buffers: device.max_per_stage_storage_buffers,
        };
        let lighting_bindings = crate::lighting::set_layout_bindings();
        crate::descriptor_budget::check(
            "scene",
            &[
                &camera_binding,
                &instance_binding,
                &crate::material::set_layout_bindings(),
                &lighting_bindings,
            ],
            limits,
        )?;
        crate::descriptor_budget::check(
            "VAT",
            &[
                &camera_binding,
                &instance_binding,
                &crate::vat::set_layout_bindings(),
                &lighting_bindings,
            ],
            limits,
        )?;
        // R3b: the foliage and water layouts swap set 2 likewise.
        crate::descriptor_budget::check(
            "foliage",
            &[
                &camera_binding,
                &instance_binding,
                &super::environment::foliage_set_bindings(),
                &lighting_bindings,
            ],
            limits,
        )?;
        crate::descriptor_budget::check(
            "water",
            &[
                &camera_binding,
                &instance_binding,
                &super::environment::water_set_bindings(),
                &lighting_bindings,
            ],
            limits,
        )?;
        let camera_layout = DescriptorSetLayout::new(device.device.clone(), &camera_binding)?;
        let instance_layout = DescriptorSetLayout::new(device.device.clone(), &instance_binding)?;
        let frame_count = u32::try_from(frames * views.len()).expect("frame count fits u32");
        // ADR-0021 §5c: the overlay prepass's camera, one more uniform set per frame.
        let overlay_sets = u32::try_from(frames).expect("frame count fits u32")
            * u32::from(views.iter().any(|view| view.overlay));
        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(frame_count + overlay_sets),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(3 * frame_count),
        ];
        let descriptor_pool = DescriptorPool::new(
            device.device.clone(),
            &pool_sizes,
            2 * frame_count + overlay_sets,
        )?;

        // One layout for every scene pipeline keeps the bound descriptor sets compatible
        // across them: sets 0/1 (camera, instances + bones) and 3 (lighting) are bound once per
        // view and set 2 (the material) per textured mesh. The shadow casters use only sets
        // 0/1. Only `skinned*.vert` reads the push constant.
        let layouts = [
            camera_layout.handle(),
            instance_layout.handle(),
            material_layout,
            lighting_layout,
        ];
        let skin_push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(SKIN_PUSH_SIZE)];
        let pipeline_layout =
            PipelineLayout::with_push_constants(device.device.clone(), &layouts, &skin_push)?;
        // The VAT pipelines differ only in set 2 (the VAT texture); the push range is the
        // same 16 bytes (`VatPush`), so sets 0/1 bound with either layout stay compatible.
        let vat = VatMeshes::new(device)?;
        let vat_layouts = [
            camera_layout.handle(),
            instance_layout.handle(),
            vat.set_layout(),
            lighting_layout,
        ];
        let vat_pipeline_layout =
            PipelineLayout::with_push_constants(device.device.clone(), &vat_layouts, &skin_push)?;
        let environment = EnvironmentResources::new(
            device,
            frames,
            camera_layout.handle(),
            instance_layout.handle(),
            lighting_layout,
            &skin_push,
            environment,
        )?;
        let (base_set, main_set) = main.sets(shadow);
        let layouts = environment.layouts();
        let pipelines = ScenePipelines::build(
            device,
            shaders,
            pipeline_layout.handle(),
            vat_pipeline_layout.handle(),
            color_format,
            depth_format,
            base_set,
            skinning,
            layouts,
        )?;
        let msaa_pipelines = main_set
            .map(|set| {
                ScenePipelines::build(
                    device,
                    shaders,
                    pipeline_layout.handle(),
                    vat_pipeline_layout.handle(),
                    color_format,
                    depth_format,
                    set,
                    skinning,
                    layouts,
                )
            })
            .transpose()?;

        let terrain_mesh = mesh::upload(device, &mesh::CpuMesh::plane(300.0))?;
        let mut meshes = Vec::with_capacity(MAX_MESHES);
        meshes.push(mesh::upload(device, &mesh::CpuMesh::cube())?);

        let set_layouts: Vec<vk::DescriptorSetLayout> = (0..frames * views.len())
            .flat_map(|_| [camera_layout.handle(), instance_layout.handle()])
            .collect();
        let sets = descriptor_pool.allocate(&device.device, &set_layouts)?;
        let mut scene_views = Vec::with_capacity(views.len());
        for (view, budget) in views.iter().enumerate() {
            let first_set = view * frames * 2;
            let overlay_sets = if budget.overlay {
                let layouts = vec![camera_layout.handle(); frames];
                descriptor_pool.allocate(&device.device, &layouts)?
            } else {
                Vec::new()
            };
            scene_views.push(SceneView::new(
                device,
                *budget,
                &sets[first_set..first_set + frames * 2],
                &overlay_sets,
            )?);
        }
        let max_joints = views
            .first()
            .filter(|view| view.temporal)
            .map_or(0, |view| {
                scene_views
                    .first()
                    .map_or(view.max_bones, |state| state.max_bones)
            });
        let motion = (main.temporal && views.first().is_some_and(|view| view.temporal))
            .then(|| MotionHistory::new(max_joints));

        Ok(Self {
            terrain_mesh,
            map_terrain: None,
            meshes,
            mesh_material_sets: [vk::DescriptorSet::null(); MAX_MESHES],
            views: scene_views,
            _descriptor_pool: descriptor_pool,
            camera_layout_handle: camera_layout.handle(),
            _camera_layout: camera_layout,
            _instance_layout: instance_layout,
            pipeline_layout,
            pipelines,
            msaa_pipelines,
            color_format,
            depth_format,
            shadow,
            main,
            skinning,
            vat,
            vat_pipeline_layout,
            mesh_vat: [u8::MAX; MAX_MESHES],
            mesh_finest: [0; MAX_MESHES],
            terrain_chunks: Vec::with_capacity(MAX_TERRAIN_CHUNKS),
            culling,
            stats: CullStats {
                mode: culling.mode,
                ..CullStats::default()
            },
            environment,
            motion,
        })
    }

    /// What the main view culled and selected in the latest frame.
    pub(crate) fn cull_stats(&self) -> CullStats {
        self.stats
    }

    /// Records the finest level the tier draws for the mesh just pushed (its triangle
    /// budget): LOD0 when it fits, else the first level that does, else the coarsest.
    fn push_mesh(&mut self, gpu: GpuMesh) {
        let budget = self.culling.model_triangles;
        let count = usize::from(gpu.lods.count.max(1));
        let finest = (0..count)
            .find(|&level| gpu.lods.levels[level].index_count as usize / 3 <= budget)
            .unwrap_or(count - 1);
        self.mesh_finest[self.meshes.len()] = u8::try_from(finest).unwrap_or(0);
        self.meshes.push(gpu);
    }

    /// The camera uniform's set layout (set 0 of every scene pipeline).
    pub(crate) fn camera_set_layout(&self) -> vk::DescriptorSetLayout {
        self.camera_layout_handle
    }

    /// The main view's camera set for `frame` (written by [`Self::prepare`]).
    pub(crate) fn camera_set(&self, frame: usize) -> vk::DescriptorSet {
        self.views[0].frames[frame].camera_set
    }

    /// Builds a replacement for every pipeline from `shaders` (dev shader reload) without
    /// installing it, so the caller can swap all passes at once or none. A set that lost a
    /// pipeline the current one has (a skinned permutation that no longer builds) is an
    /// error, so a reload never silently drops skinning.
    pub(crate) fn build_pipelines(
        &self,
        device: &DeviceContext,
        shaders: &ShaderSource,
    ) -> RenderResult<(ScenePipelines, Option<ScenePipelines>)> {
        let (base_set, main_set) = self.main.sets(self.shadow);
        let build = |set| {
            ScenePipelines::build(
                device,
                shaders,
                self.pipeline_layout.handle(),
                self.vat_pipeline_layout.handle(),
                self.color_format,
                self.depth_format,
                set,
                self.skinning,
                self.environment.layouts(),
            )
        };
        let pipelines = build(base_set)?;
        let msaa = main_set.map(build).transpose()?;
        let lost = |new: &ScenePipelines, old: &ScenePipelines| {
            new.skinned.is_some() != old.skinned.is_some()
                || new.skinned_textured.is_some() != old.skinned_textured.is_some()
                || new.skinned_shadow.is_some() != old.skinned_shadow.is_some()
                || new.hull_skinned.is_some() != old.hull_skinned.is_some()
                || new
                    .prepass
                    .as_ref()
                    .map(|prepass| prepass.skinned.is_some())
                    != old
                        .prepass
                        .as_ref()
                        .map(|prepass| prepass.skinned.is_some())
        };
        let lost_vat = |new: &ScenePipelines, old: &ScenePipelines| {
            new.vat.is_some() != old.vat.is_some()
                || new.vat_shadow.is_some() != old.vat_shadow.is_some()
        };
        let msaa_pair = msaa.as_ref().zip(self.msaa_pipelines.as_ref());
        if lost(&pipelines, &self.pipelines) || msaa_pair.is_some_and(|(new, old)| lost(new, old)) {
            return Err(RenderError::Shader("a skinned pipeline failed to rebuild"));
        }
        if lost_vat(&pipelines, &self.pipelines)
            || msaa_pair.is_some_and(|(new, old)| lost_vat(new, old))
        {
            return Err(RenderError::Shader("a VAT pipeline failed to rebuild"));
        }
        Ok((pipelines, msaa))
    }

    /// The foliage and water resources (R3b).
    pub(crate) fn environment(&self) -> &EnvironmentResources {
        &self.environment
    }

    /// The foliage and water resources (R3b), mutably (uploads; the device must be idle).
    pub(crate) fn environment_mut(&mut self) -> &mut EnvironmentResources {
        &mut self.environment
    }

    /// Installs pipelines from [`Self::build_pipelines`]. The device must be idle.
    pub(crate) fn install_pipelines(
        &mut self,
        (pipelines, msaa): (ScenePipelines, Option<ScenePipelines>),
    ) {
        self.pipelines = pipelines;
        self.msaa_pipelines = msaa;
    }

    /// The pipeline set `view` draws with: the MSAA set for the main view when MSAA is on.
    fn view_pipelines(&self, view: usize) -> &ScenePipelines {
        match (&self.msaa_pipelines, view) {
            (Some(msaa), 0) => msaa,
            _ => &self.pipelines,
        }
    }

    /// Draws `mesh` with the material whose descriptor set is `set` (null for the
    /// vertex-colour look). Load time only.
    ///
    /// # Errors
    /// [`RenderError::InvalidMaterial`] for an unknown mesh, or a textured material on a mesh
    /// without texture coordinates.
    pub(crate) fn set_mesh_material(
        &mut self,
        mesh: MeshId,
        set: vk::DescriptorSet,
    ) -> RenderResult<()> {
        let gpu = self
            .meshes
            .get(mesh.index())
            .ok_or(RenderError::InvalidMaterial("unknown mesh id"))?;
        if set != vk::DescriptorSet::null() && !gpu.has_uvs {
            return Err(RenderError::InvalidMaterial(
                "a textured material needs a mesh with texture coordinates",
            ));
        }
        self.mesh_material_sets[mesh.index()] = set;
        Ok(())
    }

    /// Replaces the map's terrain mesh and its culling chunks (`None`: the R2a plane). The
    /// device must be idle.
    pub(crate) fn set_terrain_mesh(&mut self, terrain: Option<(GpuMesh, Vec<TerrainChunk>)>) {
        self.terrain_chunks.clear();
        self.map_terrain = terrain.map(|(mesh, chunks)| {
            // At most MAX_TERRAIN_CHUNKS by construction; never grow the scratch.
            let room = self.terrain_chunks.capacity();
            self.terrain_chunks.extend(chunks.into_iter().take(room));
            mesh
        });
    }

    /// The main view's bone budget after clamping to the device's storage-buffer range.
    pub(crate) fn max_bones(&self) -> usize {
        self.views[0].max_bones
    }

    /// Whether the skinned pipeline exists.
    pub(crate) fn skinning_available(&self) -> bool {
        self.pipelines.skinned.is_some()
    }

    /// Number of registered meshes, including the built-in cube.
    pub(crate) fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    /// A registered static (unskinned) mesh, for mesh particles.
    pub(crate) fn mesh(&self, mesh: MeshId) -> Option<&GpuMesh> {
        self.meshes
            .get(mesh.index())
            .filter(|gpu| gpu.joint_count == 0)
    }

    /// A view's bone buffer for this frame, for the render graph's host-write → vertex-read
    /// barrier.
    pub(crate) fn bone_buffer(&self, view: usize, frame: usize) -> vk::Buffer {
        self.views[view].frames[frame].bone_buffer.handle()
    }

    /// Uploads a static mesh into the next slot (load time only).
    pub(crate) fn register_mesh(
        &mut self,
        device: &DeviceContext,
        data: &crate::MeshData,
    ) -> RenderResult<MeshId> {
        let id = self.next_mesh_id()?;
        let cpu = mesh::CpuMesh::from_asset(data).ok_or(RenderError::InvalidMesh)?;
        let gpu = mesh::upload(device, &cpu)?;
        self.push_mesh(gpu);
        Ok(id)
    }

    /// Uploads a skinned mesh into the next slot (load time only).
    pub(crate) fn register_skinned_mesh(
        &mut self,
        device: &DeviceContext,
        data: &crate::MeshData,
    ) -> RenderResult<MeshId> {
        if self.pipelines.skinned.is_none() {
            return Err(RenderError::SkinningUnavailable);
        }
        let id = self.next_mesh_id()?;
        let cpu = mesh::CpuSkinnedMesh::from_asset(data).ok_or(RenderError::InvalidMesh)?;
        let gpu = mesh::upload_skinned(device, &cpu)?;
        self.push_mesh(gpu);
        Ok(id)
    }

    /// Uploads a static mesh with its vertex-animation texture into the next slot (load time
    /// only). The mesh draws with the VAT pipelines and each instance's
    /// [`UnitInstance::vat`](crate::UnitInstance::vat) pose.
    pub(crate) fn register_vat_mesh(
        &mut self,
        device: &DeviceContext,
        data: &crate::MeshData,
        texture: &crate::TextureData,
        descriptor: &moba_assets::VatDescriptor,
    ) -> RenderResult<MeshId> {
        if !self.vat_available() {
            return Err(RenderError::InvalidVat("the VAT pipeline is unavailable"));
        }
        let id = self.next_mesh_id()?;
        let cpu = mesh::CpuMesh::from_asset(data).ok_or(RenderError::InvalidMesh)?;
        if cpu.vertices.len() != descriptor.vertex_count as usize {
            return Err(RenderError::InvalidVat(
                "the mesh does not match its descriptor",
            ));
        }
        // Every check first, then the mesh, then the VAT entry: a failure at any step leaves
        // no VAT slot or mesh slot taken (the uploaded mesh is dropped).
        crate::vat::validate(self.vat.len(), data, texture, descriptor)?;
        // The culling sphere covers every baked frame (R8): the rest pose's would be too
        // small for an attack swing or a death fall.
        let bounds = descriptor
            .position_bounds(texture)
            .ok_or(RenderError::InvalidVat(
                "the texture holds non-finite positions",
            ))?;
        let mut gpu = mesh::upload(device, &cpu)?;
        gpu.bounds = bounds;
        let entry = self.vat.register(device, data, texture, descriptor)?;
        let entry = u8::try_from(entry).map_err(|_| RenderError::InvalidVat("too many VATs"))?;
        self.push_mesh(gpu);
        self.mesh_vat[id.index()] = entry;
        Ok(id)
    }

    /// Whether VAT meshes can be registered: the VAT pipeline exists in every set (and its
    /// prepass twin wherever there is a prepass).
    pub(crate) fn vat_available(&self) -> bool {
        let complete = |set: &ScenePipelines| {
            set.vat.is_some()
                && set
                    .prepass
                    .as_ref()
                    .is_none_or(|prepass| prepass.vat.is_some())
        };
        complete(&self.pipelines) && self.msaa_pipelines.as_ref().is_none_or(complete)
    }

    /// Registered VAT meshes.
    pub(crate) fn vat_mesh_count(&self) -> usize {
        self.vat.len()
    }

    fn next_mesh_id(&self) -> RenderResult<MeshId> {
        if self.meshes.len() >= MAX_MESHES {
            return Err(RenderError::TooManyMeshes);
        }
        let id = u16::try_from(self.meshes.len()).map_err(|_| RenderError::TooManyMeshes)?;
        Ok(MeshId::from_index(id))
    }

    /// Writes this frame's camera (with `lighting`), instances and bones of `view` and
    /// buckets its draws: culls each unit against the view's camera (and, for the main view
    /// with shadows, against the shadow box for the caster list), picks its mesh level and
    /// computes the terrain's visible runs (R8). The frame's fence must have been waited on.
    /// The truncation counters and [`Self::cull_stats`] report the main view (0) only.
    #[allow(clippy::too_many_lines)] // Cull, bucket and scatter belong together.
    pub(crate) fn prepare(
        &mut self,
        view: usize,
        frame: usize,
        scene: &Scene<'_>,
        lighting: &FrameLighting,
        world: &WorldFrame,
        extent: vk::Extent2D,
        lod_height: u32,
        main: Option<&MainFrame>,
    ) -> RenderResult<()> {
        let mesh_count = self.meshes.len();
        let culling = self.culling;
        let Self {
            views,
            meshes,
            mesh_finest,
            map_terrain,
            terrain_chunks,
            terrain_mesh,
            stats,
            motion,
            ..
        } = self;
        // ADR-0021 §2c: the main view's history moves on one frame (and forgets on a reset).
        let history = match (view, motion.as_mut(), main) {
            (0, Some(history), Some(main)) => {
                history.begin_frame(main.history_reset);
                Some(history)
            }
            _ => None,
        };
        let state = &mut views[view];
        let unit_count = scene.units.len();
        let bone_count = scene.bones.len().min(state.max_bones);
        if view == 0 {
            BONES_TRUNCATED.store(
                u64::try_from(scene.bones.len() - bone_count).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
        }
        state.unit_count = unit_count;
        let mut skinned_dropped = 0u64;
        let target = &mut state.frames[frame];
        let uniform = CameraUniform::new(
            scene.camera,
            &scene.vision,
            scene.terrain_tint,
            lighting,
            world,
            &scene.outlines,
            [extent.width, extent.height],
        );
        // ADR-0021 §1: the jittered matrix and the motion members (Compat: none of it).
        let uniform = match main.and_then(|main| main.temporal.as_ref()) {
            Some(temporal) => uniform.with_temporal(temporal),
            None => uniform,
        };
        target.camera_buffer.write_bytes(0, bytes_of(&uniform));
        // ADR-0021 §5c: the overlay prepass's camera, unjittered at output resolution.
        if let (Some(main), Some((buffer, _))) = (main, target.overlay_camera.as_mut()) {
            let overlay = CameraUniform::new(
                scene.camera,
                &scene.vision,
                scene.terrain_tint,
                lighting,
                world,
                &scene.outlines,
                [main.output.width, main.output.height],
            );
            buffer.write_bytes(0, bytes_of(&overlay));
        }
        if bone_count > 0 {
            let mapped = target
                .bone_buffer
                .mapped_slice_mut::<JointMatrix>(bone_count)
                .ok_or_else(|| {
                    RenderError::NoSuitableDevice(String::from("bone buffer is not host visible"))
                })?;
            mapped.copy_from_slice(&scene.bones[..bone_count]);
        }
        // `bone_count <= max_bones`, which the bone buffer was sized from.
        let uploaded_bones = u32::try_from(bone_count).unwrap_or(u32::MAX);

        // Culling volumes and the level metric (R8): `Off` keeps everything.
        let culled = culling.mode != CullMode::Off;
        let camera_frustum = if culled {
            Frustum::new(scene.camera.view_proj, Planes::All)
        } else {
            Frustum::EVERYTHING
        };
        let caster_frustum = if culled {
            Frustum::new(
                Mat4::from_cols_array_2d(&lighting.light_view_proj),
                Planes::Sides,
            )
        } else {
            Frustum::EVERYTHING
        };
        // `lod_height` is the view's height at its configured render scale, not this frame's
        // dynamic-resolution extent, so dynres never flips a unit between levels.
        let metric = LodMetric::new(scene.camera.view_proj, lod_height, culling.lod);
        let casters = state.casters.is_some();

        // The terrain's visible runs: every chunk of the map's heightfield, or the plane.
        match map_terrain.as_ref() {
            Some(_) if !terrain_chunks.is_empty() => {
                let visible = crate::cull::terrain_runs(
                    terrain_chunks,
                    &camera_frustum,
                    &mut state.terrain_runs,
                );
                if view == 0 {
                    stats.terrain_chunks = u32::try_from(terrain_chunks.len()).unwrap_or(u32::MAX);
                    stats.terrain_chunks_drawn = visible;
                }
            }
            terrain => {
                let mesh = terrain.unwrap_or(terrain_mesh);
                state.terrain_runs.clear();
                state.terrain_runs.push((0, mesh.index_count));
                if view == 0 {
                    stats.terrain_chunks = 1;
                    stats.terrain_chunks_drawn = 1;
                }
            }
        }

        let units = scene.units;
        // Every unit is tested; the instance cap applies to the ones a list draws.
        let dropped = keep_drawn(units, state.max_instances, &mut state.decisions, |unit| {
            let slot = mesh_slot(unit.mesh, mesh_count);
            let mesh = &meshes[slot];
            let (center, radius) = crate::cull::world_sphere(&unit.transform, mesh.bounds);
            let in_camera =
                !culled || is_outlined(unit) || camera_frustum.sphere_visible(center, radius);
            let in_casters = casters && (!culled || caster_frustum.sphere_visible(center, radius));
            if !in_camera && !in_casters {
                return 0;
            }
            let scale = crate::cull::max_axis_scale(&unit.transform);
            let lod = metric.select(&mesh.lods, mesh_finest[slot], center, scale);
            (u8::from(in_camera) * IN_CAMERA)
                | (u8::from(in_casters) * IN_CASTERS)
                | (lod << LOD_SHIFT)
        });
        let mut lod_instances = [0u32; MAX_LODS];
        for &(_, decision) in state.decisions.iter().filter(|(_, d)| d & IN_CAMERA != 0) {
            lod_instances[usize::from(decision >> LOD_SHIFT) % MAX_LODS] += 1;
        }
        state
            .camera
            .bucket(units, &state.decisions, IN_CAMERA, mesh_count);
        if let Some(list) = state.casters.as_mut() {
            list.bucket(units, &state.decisions, IN_CASTERS, mesh_count);
        }
        if view == 0 {
            *stats = CullStats {
                mode: culling.mode,
                candidates: u32::try_from(unit_count).unwrap_or(u32::MAX),
                drawn: state.camera.len,
                casters: state.casters.as_ref().map_or(0, |list| list.len),
                dropped,
                lod_instances,
                ..*stats
            };
        }
        let lists = state.casters.as_ref().map_or(1, |_| 2);
        // ADR-0021 §2c: the camera list's previous instances, and the previous palettes after
        // this frame's bones (the second half of the bone buffer).
        let max_bones = state.max_bones;
        let mut previous = match (&history, target.previous.as_mut()) {
            (Some(_), Some(buffer)) => {
                let instances = buffer
                    .mapped_slice_mut::<PrevInstance>(state.max_instances)
                    .ok_or_else(|| {
                        RenderError::NoSuitableDevice(String::from(
                            "previous-instance buffer is not host visible",
                        ))
                    })?;
                let bones = target
                    .bone_buffer
                    .mapped_slice_mut::<JointMatrix>(2 * max_bones)
                    .ok_or_else(|| {
                        RenderError::NoSuitableDevice(String::from(
                            "bone buffer is not host visible",
                        ))
                    })?;
                Some((instances, &mut bones[max_bones..], 0usize))
            }
            _ => None,
        };
        if !state.decisions.is_empty() {
            let mapped = target
                .instance_buffer
                .mapped_slice_mut::<GpuInstance>(state.max_instances * lists)
                .ok_or_else(|| {
                    RenderError::NoSuitableDevice(String::from(
                        "instance buffer is not host visible",
                    ))
                })?;
            // Each bucket's instances land contiguously at its `starts` slot, so one draw per
            // bucket reads them through `firstInstance` (gl_InstanceIndex); the outlined ones
            // first, so the hulls draw only them.
            for &(index, decision) in &state.decisions {
                let unit = &units[index as usize];
                let slot = mesh_slot(unit.mesh, mesh_count);
                let bucket = bucket_of(slot, decision);
                let joints = meshes[slot].joint_count;
                let lod = decision >> LOD_SHIFT;
                let gpu = GpuInstance::from(*unit);
                for (bit, list) in [
                    (IN_CAMERA, Some(&mut state.camera)),
                    (IN_CASTERS, state.casters.as_mut()),
                ] {
                    let Some(list) = list.filter(|_| decision & bit != 0) else {
                        continue;
                    };
                    let instance = list.place(unit, bucket);
                    mapped[instance as usize] = gpu;
                    // The camera list's previous state (ADR-0021 §2c), and where its previous
                    // palette went.
                    let mut previous_bone = None;
                    if bit == IN_CAMERA
                        && let (Some(history), Some((instances, bones, cursor))) =
                            (history.as_deref(), previous.as_mut())
                    {
                        let drawn =
                            history.previous(unit.motion_key, unit.motion_reset, &unit.transform);
                        instances[instance as usize] =
                            PrevInstance::new(drawn.as_ref().map(|(state, _)| state));
                        let count = unit.bones.count.min(joints) as usize;
                        if let Some((_, palette)) = drawn
                            && joints > 0
                            && count > 0
                            && palette.len() >= count
                            && *cursor + count <= bones.len()
                        {
                            bones[*cursor..*cursor + count].copy_from_slice(&palette[..count]);
                            previous_bone = u32::try_from(max_bones + *cursor).ok();
                            *cursor += count;
                        }
                    }
                    if joints == 0 {
                        continue;
                    }
                    // A skinned mesh is only drawn with a bone range inside this frame's
                    // upload; the capacity check keeps `push` from reallocating.
                    match skinned_draw(unit.bones, joints, uploaded_bones, slot, instance) {
                        Some(draw) if list.skinned.len() < list.skinned.capacity() => {
                            list.skinned.push(SkinnedDraw {
                                lod,
                                previous_bone: previous_bone.unwrap_or(0),
                                has_previous: previous_bone.is_some(),
                                ..draw
                            });
                        }
                        // The caster list does not count drops twice.
                        _ if bit == IN_CASTERS => {}
                        _ => skinned_dropped += 1,
                    }
                }
            }
        }
        if view == 0 {
            SKINNED_DROPPED.store(skinned_dropped, Ordering::Relaxed);
        }
        // ADR-0021 §2c: what every keyed unit drew this frame, for the next one (all of
        // them, so a unit entering the view next frame has its history).
        if let Some(history) = history {
            for unit in units {
                if unit.motion_key == 0 {
                    continue;
                }
                let first = unit.bones.first as usize;
                let joints = scene
                    .bones
                    .get(first..first.saturating_add(unit.bones.count as usize))
                    .filter(|_| first + (unit.bones.count as usize) <= bone_count)
                    .unwrap_or(&[]);
                history.record(unit.motion_key, unit.transform, unit.vat, joints);
            }
        }
        Ok(())
    }

    /// Records `view`: clear (plus any backdrop rectangles), terrain (when asked), static
    /// meshes (one instanced draw per mesh: vertex-colour meshes, then textured meshes with
    /// their material sets), then one draw per skinned instance (vertex-colour, then
    /// textured). The graph has already transitioned the targets.
    #[allow(clippy::too_many_lines)] // One explicit, ordered draw stream.
    pub(crate) fn record(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        view: usize,
        frame: usize,
        targets: &SceneTargets<'_>,
    ) {
        let state = &self.views[view];
        let pipelines = self.view_pipelines(view);
        let extent = targets.extent;
        let mut color_attachment = vk::RenderingAttachmentInfo::default()
            .image_view(targets.color)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: targets.clear_color,
                },
            });
        // The VFX pass tests and samples the main view's depth after the scene (ADR-0013).
        let depth_store = if targets.store_depth {
            vk::AttachmentStoreOp::STORE
        } else {
            vk::AttachmentStoreOp::DONT_CARE
        };
        let depth_load = if targets.load_depth {
            vk::AttachmentLoadOp::LOAD
        } else {
            vk::AttachmentLoadOp::CLEAR
        };
        let mut depth_attachment = vk::RenderingAttachmentInfo::default()
            .image_view(targets.depth)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .load_op(depth_load)
            .store_op(depth_store)
            .clear_value(vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            });
        if let Some(resolve) = targets.resolve {
            // MSAA: only the resolved images live on; the multisampled colour is discarded.
            color_attachment = color_attachment
                .store_op(vk::AttachmentStoreOp::DONT_CARE)
                .resolve_mode(vk::ResolveModeFlags::AVERAGE)
                .resolve_image_view(resolve.color)
                .resolve_image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
            if let Some(depth) = resolve.depth {
                depth_attachment = depth_attachment
                    .resolve_mode(vk::ResolveModeFlags::SAMPLE_ZERO)
                    .resolve_image_view(depth)
                    .resolve_image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
            }
        }
        let render_info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            })
            .layer_count(1)
            .color_attachments(core::slice::from_ref(&color_attachment))
            .depth_attachment(&depth_attachment);
        device.cmd_begin_rendering(command_buffer, &render_info);
        super::set_viewport_scissor(
            device,
            command_buffer,
            super::flipped_viewport(extent),
            extent,
        );
        clear_rects(device, command_buffer, targets);

        self.bind_view_sets(device, command_buffer, state, frame);
        // SAFETY: the lighting set was allocated with the layout the pipeline layout declares
        // at index 3; its views live as long as the renderer and it is only rewritten while
        // the device is idle.
        unsafe {
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout.handle(),
                3,
                &[targets.lighting],
                &[],
            );
        }

        if targets.terrain {
            self.draw_terrain(device, command_buffer, state, &pipelines.terrain);
        }

        let list = &state.camera;
        if list.len > 0 {
            for textured in [false, true] {
                let pipeline = if textured {
                    &pipelines.unit_textured
                } else {
                    &pipelines.unit
                };
                self.draw_static(device, command_buffer, list, pipeline, textured, true);
            }
        }

        if !list.skinned.is_empty()
            && let Some(skinned) = &pipelines.skinned
        {
            // Textured skinned meshes fall back to the vertex-colour pipeline if their
            // permutation is missing.
            let textured = pipelines.skinned_textured.as_ref();
            self.record_skinned(device, command_buffer, list, skinned, false);
            self.record_skinned(
                device,
                command_buffer,
                list,
                textured.unwrap_or(skinned),
                true,
            );
        }

        if targets.sky {
            // On the far plane: only pixels nothing else covered pass the depth test. Before
            // the VAT draws, which re-bind sets 2/3 with their own layout.
            bind_pipeline(device, command_buffer, &pipelines.sky);
            // SAFETY: the sky reads no vertex buffers; three vertices form the covering
            // triangle, and its sets (camera, lighting) are bound with the shared layout.
            unsafe { device.device.cmd_draw(command_buffer, 3, 1, 0, 0) };
        }
        // Then the VAT minions (R4b): their layout re-binds sets 2/3, which would invalidate
        // set 3 for any scene-layout draw after them. The hulls below use only sets 0/1.
        if list.len > 0
            && let Some(vat) = &pipelines.vat
        {
            self.draw_vat(
                device,
                command_buffer,
                list,
                vat,
                Some(targets.lighting),
                false,
            );
        }

        // Inverted-hull outlines last (R2b): depth-tested against everything above (minions
        // included), never written, so each hull shows only as its unit's silhouette ring
        // (ADR-0016). Only the outlined instances are drawn.
        if targets.hull_outlines && list.len > 0 {
            if let Some(pipeline) = &pipelines.hull_unit {
                for textured in [false, true] {
                    self.draw_static_with(
                        device,
                        command_buffer,
                        list,
                        pipeline,
                        textured,
                        false,
                        true,
                    );
                }
            }
            if let Some(pipeline) = &pipelines.hull_skinned {
                for textured in [false, true] {
                    self.draw_skinned(
                        device,
                        command_buffer,
                        list,
                        pipeline,
                        textured,
                        false,
                        true,
                    );
                }
            }
            if let Some(pipeline) = &pipelines.hull_vat {
                self.draw_vat(device, command_buffer, list, pipeline, None, true);
            }
        }
        // R3b, after every scene-layout draw (they re-bind sets 2/3 with their own layouts):
        // the main view's foliage, then the simple water over the carved bed.
        if view == 0 {
            self.environment.record_foliage(
                device,
                command_buffer,
                frame,
                pipelines.foliage.handle(),
                targets.lighting,
            );
        }
        if targets.terrain && (view != 0 || self.pipelines.water_full.is_none()) {
            self.environment.record_water(
                device,
                command_buffer,
                frame,
                pipelines.water.handle(),
                targets.lighting,
                false,
            );
        }

        device.cmd_end_rendering(command_buffer);
    }

    /// Records the full water pass (R3b): the main view's water over the HDR target, depth
    /// tested read-only and sampled, refracting the copy the `water_copy` pass made.
    pub(crate) fn record_water_pass(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        targets: &WaterTargets,
    ) {
        let Some(pipeline) = &self.pipelines.water_full else {
            return;
        };
        if !self.environment.has_water() {
            return;
        }
        let color = vk::RenderingAttachmentInfo::default()
            .image_view(targets.hdr)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE);
        // Read-only depth: tested, never written, and sampled in the same pass.
        let depth = vk::RenderingAttachmentInfo::default()
            .image_view(targets.depth)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::NONE);
        let render_info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: targets.extent,
            })
            .layer_count(1)
            .color_attachments(core::slice::from_ref(&color))
            .depth_attachment(&depth);
        device.cmd_begin_rendering(command_buffer, &render_info);
        super::set_viewport_scissor(
            device,
            command_buffer,
            super::flipped_viewport(targets.extent),
            targets.extent,
        );
        self.bind_view_sets(device, command_buffer, &self.views[0], frame);
        self.environment.record_water(
            device,
            command_buffer,
            frame,
            pipeline.handle(),
            targets.lighting,
            true,
        );
        device.cmd_end_rendering(command_buffer);
    }

    /// Binds `state`'s camera and instance sets (0 and 1) for this frame.
    fn bind_view_sets(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        state: &SceneView,
        frame: usize,
    ) {
        let scene_frame = &state.frames[frame];
        let descriptor_sets = [scene_frame.camera_set, scene_frame.instance_set];
        // SAFETY: both sets were allocated from this frame's layout and the pipeline layout
        // declares them at indices 0 and 1.
        unsafe {
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout.handle(),
                0,
                &descriptor_sets,
                &[],
            );
        }
    }

    /// Records the shadow pass: the **main view's caster list** (static meshes instanced,
    /// skinned meshes one draw each, from view 0's caster buckets and bones; R8: culled
    /// against the shadow box's sides) depth-only through the light's view-projection into
    /// `target`, cleared to 1. Offscreen views never cast into it. The terrain receives but
    /// never casts.
    pub(crate) fn record_shadow(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        target: &ShadowTarget,
    ) {
        let state = &self.views[0];
        let extent = vk::Extent2D {
            width: target.size,
            height: target.size,
        };
        let depth_attachment = vk::RenderingAttachmentInfo::default()
            .image_view(target.depth)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            });
        let render_info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            })
            .layer_count(1)
            .depth_attachment(&depth_attachment);
        device.cmd_begin_rendering(command_buffer, &render_info);
        super::set_viewport_scissor(device, command_buffer, super::full_viewport(extent), extent);
        // SAFETY: the bound shadow pipelines declare DEPTH_BIAS dynamic and the rasterizer's
        // depth bias is enabled; a zero clamp disables clamping.
        unsafe {
            device.device.cmd_set_depth_bias(
                command_buffer,
                SHADOW_BIAS_CONSTANT,
                0.0,
                SHADOW_BIAS_SLOPE,
            );
        }
        if let Some(list) = state.casters.as_ref().filter(|list| list.len > 0) {
            self.bind_view_sets(device, command_buffer, state, frame);
            if let Some(pipeline) = &self.pipelines.unit_shadow {
                for textured in [false, true] {
                    self.draw_static(device, command_buffer, list, pipeline, textured, false);
                }
            }
            if let Some(pipeline) = &self.pipelines.skinned_shadow {
                for textured in [false, true] {
                    self.draw_skinned(
                        device,
                        command_buffer,
                        list,
                        pipeline,
                        textured,
                        false,
                        false,
                    );
                }
            }
            if let Some(pipeline) = &self.pipelines.vat_shadow {
                self.draw_vat(device, command_buffer, list, pipeline, None, false);
            }
        }
        device.cmd_end_rendering(command_buffer);
    }

    /// Records the main view's depth prepass (R6, ADR-0015; R2b, ADR-0016): terrain, static,
    /// skinned and VAT meshes into `target` (cleared to 1 and stored for the scene, which
    /// loads it), resolving sample 0 into the single-sample depth with MSAA, plus the units'
    /// outline slots into the ID target (cleared to 0, resolved the same way). **Unbiased**:
    /// every scene vertex shader's `gl_Position` is `invariant`, so the scene pass reproduces
    /// these depths and its `LESS_OR_EQUAL` test passes where the prepass drew; a bias would
    /// make the ID target's depth test disagree with the scene's at grazing occluders,
    /// leaking a hidden unit's outline over a wall. Uses the main view's camera, instances
    /// and bones prepared for the scene; materials are not bound.
    pub(crate) fn record_prepass(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        target: &PrepassTarget,
    ) {
        let Some(prepass) = self.view_pipelines(0).prepass.as_ref() else {
            return;
        };
        let state = &self.views[0];
        let mut depth_attachment = vk::RenderingAttachmentInfo::default()
            .image_view(target.depth)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            });
        if let Some(resolve) = target.resolve {
            depth_attachment = depth_attachment
                .resolve_mode(vk::ResolveModeFlags::SAMPLE_ZERO)
                .resolve_image_view(resolve)
                .resolve_image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        }
        let id_attachment = target.outline_ids.map(|ids| {
            let attachment = vk::RenderingAttachmentInfo::default()
                .image_view(ids.view)
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .clear_value(vk::ClearValue {
                    color: vk::ClearColorValue { uint32: [0; 4] },
                });
            match ids.resolve {
                Some(resolve) => attachment
                    .resolve_mode(vk::ResolveModeFlags::SAMPLE_ZERO)
                    .resolve_image_view(resolve)
                    .resolve_image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL),
                None => attachment,
            }
        });
        // ADR-0021 §2b: motion first, cleared to 0 (no motion), then the unit mask.
        let motion_attachment = target.motion.map(|view| {
            vk::RenderingAttachmentInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .clear_value(vk::ClearValue {
                    color: vk::ClearColorValue { float32: [0.0; 4] },
                })
        });
        let mut color_attachments = [vk::RenderingAttachmentInfo::default(); 2];
        let mut color_count = 0;
        for attachment in motion_attachment.into_iter().chain(id_attachment) {
            color_attachments[color_count] = attachment;
            color_count += 1;
        }
        let render_info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: target.extent,
            })
            .layer_count(1)
            .color_attachments(&color_attachments[..color_count])
            .depth_attachment(&depth_attachment);
        device.cmd_begin_rendering(command_buffer, &render_info);
        super::set_viewport_scissor(
            device,
            command_buffer,
            super::flipped_viewport(target.extent),
            target.extent,
        );
        self.bind_view_sets(device, command_buffer, state, frame);
        // The same terrain runs as `record` (the map's heightfield, or the R2a plane).
        self.draw_terrain(device, command_buffer, state, &prepass.terrain);
        // The position-only prepass shaders read no material, so one pipeline draws both
        // the vertex-colour and the textured meshes. The camera list, as the scene's.
        let list = &state.camera;
        if list.len > 0 {
            for textured in [false, true] {
                self.draw_static(device, command_buffer, list, &prepass.unit, textured, false);
            }
        }
        if !list.skinned.is_empty()
            && let Some(skinned) = &prepass.skinned
        {
            for textured in [false, true] {
                self.draw_skinned(
                    device,
                    command_buffer,
                    list,
                    skinned,
                    textured,
                    false,
                    false,
                );
            }
        }
        // VAT minions (R4b), so GTAO, contact shadows, outlines and the particles' depth test
        // see them; last, as in `record` (the VAT layout re-binds set 2).
        if list.len > 0
            && let Some(vat) = &prepass.vat
        {
            self.draw_vat(device, command_buffer, list, vat, None, false);
        }
        // ADR-0021 §2b: the temporal prepass also draws the foliage (with its own layout's
        // sets 2/3) and, last, the background's camera motion on the far plane.
        if let Some(foliage) = &prepass.foliage {
            self.environment.record_foliage(
                device,
                command_buffer,
                frame,
                foliage.handle(),
                target.lighting,
            );
        }
        if let Some(sky) = &prepass.sky {
            bind_pipeline(device, command_buffer, sky);
            // SAFETY: no vertex input; three vertices form the covering triangle, and its set
            // (the camera) is bound with the shared layout.
            unsafe { device.device.cmd_draw(command_buffer, 3, 1, 0, 0) };
        }
        device.cmd_end_rendering(command_buffer);
    }

    /// Records `tc_mask` (ADR-0021 §2d): the water surfaces, depth-tested against the scene,
    /// into the transparency-and-composition mask with `MAX` (the mask was cleared by
    /// `mask_clear`).
    pub(crate) fn record_tc_mask(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        target: &TcMaskTarget,
    ) {
        let Some(pipeline) = &self.pipelines.tc_mask else {
            return;
        };
        let color = vk::RenderingAttachmentInfo::default()
            .image_view(target.mask)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE);
        let depth = vk::RenderingAttachmentInfo::default()
            .image_view(target.depth)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::NONE);
        let render_info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: target.extent,
            })
            .layer_count(1)
            .color_attachments(core::slice::from_ref(&color))
            .depth_attachment(&depth);
        device.cmd_begin_rendering(command_buffer, &render_info);
        super::set_viewport_scissor(
            device,
            command_buffer,
            super::flipped_viewport(target.extent),
            target.extent,
        );
        self.bind_view_sets(device, command_buffer, &self.views[0], frame);
        self.environment.record_water(
            device,
            command_buffer,
            frame,
            pipeline.handle(),
            target.lighting,
            false,
        );
        device.cmd_end_rendering(command_buffer);
    }

    /// Records the overlay prepass (ADR-0021 §5c): at output resolution through the overlay
    /// camera (unjittered), the terrain and the water as `GROUND`, every camera-list unit as
    /// its outline slot or `OCCLUDER`, into the overlay depth (cleared to 1) and IDs (cleared
    /// to 0). The foliage, the particles and the sky are left out.
    pub(crate) fn record_overlay_prepass(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        target: &OverlayPrepassTarget,
    ) {
        let Some(overlay) = &self.pipelines.overlay else {
            return;
        };
        let state = &self.views[0];
        let Some((_, camera_set)) = &state.frames[frame].overlay_camera else {
            return;
        };
        let ids = vk::RenderingAttachmentInfo::default()
            .image_view(target.ids)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                color: vk::ClearColorValue { uint32: [0; 4] },
            });
        let depth = vk::RenderingAttachmentInfo::default()
            .image_view(target.depth)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            });
        let render_info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: target.extent,
            })
            .layer_count(1)
            .color_attachments(core::slice::from_ref(&ids))
            .depth_attachment(&depth);
        device.cmd_begin_rendering(command_buffer, &render_info);
        super::set_viewport_scissor(
            device,
            command_buffer,
            super::flipped_viewport(target.extent),
            target.extent,
        );
        let sets = [*camera_set, state.frames[frame].instance_set];
        // SAFETY: the overlay camera set was allocated with the camera layout, the instance
        // set with the instance layout; the shared pipeline layout declares them at 0 and 1.
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
        self.draw_terrain(device, command_buffer, state, &overlay.terrain);
        let list = &state.camera;
        if list.len > 0 {
            for textured in [false, true] {
                self.draw_static(device, command_buffer, list, &overlay.unit, textured, false);
            }
        }
        if !list.skinned.is_empty()
            && let Some(skinned) = &overlay.skinned
        {
            for textured in [false, true] {
                self.draw_skinned(
                    device,
                    command_buffer,
                    list,
                    skinned,
                    textured,
                    false,
                    false,
                );
            }
        }
        if list.len > 0
            && let Some(vat) = &overlay.vat
        {
            self.draw_vat(device, command_buffer, list, vat, None, false);
        }
        // The water surfaces are ground too (a telegraph lands on the surface), last: the
        // water layout re-binds sets 2/3.
        self.environment.record_water(
            device,
            command_buffer,
            frame,
            overlay.water.handle(),
            target.lighting,
            false,
        );
        device.cmd_end_rendering(command_buffer);
    }

    /// The terrain's visible runs of `state` (computed in [`Self::prepare`]) with `pipeline`:
    /// the map's heightfield, or the R2a plane.
    fn draw_terrain(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        state: &SceneView,
        pipeline: &GraphicsPipeline,
    ) {
        let terrain = self.map_terrain.as_ref().unwrap_or(&self.terrain_mesh);
        if state.terrain_runs.is_empty() {
            return;
        }
        bind_pipeline(device, command_buffer, pipeline);
        bind_mesh(device, command_buffer, terrain);
        for &(first, count) in &state.terrain_runs {
            // SAFETY: each run lies inside the terrain's index buffer (the chunks tile it,
            // `terrain::grid_mesh`; the plane's run is its whole buffer), and every index is
            // in range for its vertices by construction.
            unsafe {
                device
                    .device
                    .cmd_draw_indexed(command_buffer, count, 1, first, 0, 0);
            }
        }
    }

    /// One instanced draw with `pipeline` per static mesh and level whose material is
    /// (`textured`) or is not textured, binding each material set when `bind_materials`.
    fn draw_static(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        list: &DrawList,
        pipeline: &GraphicsPipeline,
        textured: bool,
        bind_materials: bool,
    ) {
        self.draw_static_with(
            device,
            command_buffer,
            list,
            pipeline,
            textured,
            bind_materials,
            false,
        );
    }

    /// [`Self::draw_static`]; with `hull` (the outline pipeline) each draw first pushes
    /// whether its mesh is the built-in cube, which extrudes from its centre.
    #[allow(clippy::too_many_arguments)] // The draw loop's switches.
    fn draw_static_with(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        list: &DrawList,
        pipeline: &GraphicsPipeline,
        textured: bool,
        bind_materials: bool,
        hull: bool,
    ) {
        let mut bound = false;
        for (slot, mesh) in self.meshes.iter().enumerate() {
            let set = self.mesh_material_sets[slot];
            // Skinned meshes are drawn per instance below, and VAT meshes instanced, each
            // with their own pipeline.
            if mesh.joint_count > 0
                || self.mesh_vat[slot] != u8::MAX
                || (set != vk::DescriptorSet::null()) != textured
            {
                continue;
            }
            let mut mesh_bound = false;
            for level in 0..usize::from(mesh.lods.count.max(1)) {
                let bucket = slot * MAX_LODS + level;
                // The hull draws only the outlined instances, which lead the bucket.
                let count = if hull {
                    list.outlined[bucket]
                } else {
                    list.counts[bucket]
                };
                if count == 0 {
                    continue;
                }
                if !bound {
                    bind_pipeline(device, command_buffer, pipeline);
                    bound = true;
                }
                if !mesh_bound {
                    if textured && bind_materials {
                        bind_material(device, command_buffer, self.pipeline_layout.handle(), set);
                    }
                    bind_mesh(device, command_buffer, mesh);
                    if hull {
                        let push = SkinPush {
                            first_bone: 0,
                            joint_count: 0,
                            reserved: [u32::from(slot == 0), 0],
                        };
                        // SAFETY: the shared pipeline layout declares exactly this 16-byte
                        // vertex-stage range at offset 0.
                        unsafe {
                            device.device.cmd_push_constants(
                                command_buffer,
                                self.pipeline_layout.handle(),
                                vk::ShaderStageFlags::VERTEX,
                                0,
                                bytes_of(&push),
                            );
                        }
                    }
                    mesh_bound = true;
                }
                let range = mesh.lods.level(level);
                // SAFETY: indices: the level's range lies inside the index buffer (LOD0 then
                // the levels, back to back, `mesh::with_levels`), and every index is below the
                // vertex count (cube by construction; registered meshes re-checked in
                // `CpuMesh::from_asset`), so no vertex fetch leaves the vertex buffer.
                // Instances: `starts[bucket] + count <= base + len <= the list's capacity`,
                // inside the view's instance buffer, by construction in `DrawList::bucket`.
                unsafe {
                    device.device.cmd_draw_indexed(
                        command_buffer,
                        range.index_count,
                        count,
                        range.first_index,
                        0,
                        list.starts[bucket],
                    );
                }
            }
        }
    }

    /// One instanced draw per VAT mesh and level with `pipeline` (`vat` or `vat_shadow`),
    /// binding the mesh's VAT set at 2 and, for the colour pass, `lighting` at 3. Both are
    /// bound with the VAT layout, which differs from the scene layout in set 2 only, so sets
    /// 0/1 stay valid; binding 2 and 3 in one call re-validates 3 after 2 changed layout.
    fn draw_vat(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        list: &DrawList,
        pipeline: &GraphicsPipeline,
        lighting: Option<vk::DescriptorSet>,
        hull: bool,
    ) {
        let layout = self.vat_pipeline_layout.handle();
        let mut bound = false;
        for (slot, mesh) in self.meshes.iter().enumerate() {
            let entry = self.mesh_vat[slot];
            if entry == u8::MAX {
                continue;
            }
            let Some((set, push)) = self.vat.entry(usize::from(entry)) else {
                continue;
            };
            let mut mesh_bound = false;
            for level in 0..usize::from(mesh.lods.count.max(1)) {
                let bucket = slot * MAX_LODS + level;
                // The hull draws only the outlined instances, which lead each bucket.
                let count = if hull {
                    list.outlined[bucket]
                } else {
                    list.counts[bucket]
                };
                if count == 0 {
                    continue;
                }
                if !bound {
                    bind_pipeline(device, command_buffer, pipeline);
                    bound = true;
                }
                if !mesh_bound {
                    let lighting_set = [set, lighting.unwrap_or(vk::DescriptorSet::null())];
                    let sets = if lighting.is_some() {
                        &lighting_set[..]
                    } else {
                        &lighting_set[..1]
                    };
                    // SAFETY: the VAT set was allocated with the VAT set layout, which the VAT
                    // pipeline layout declares at 2, and the lighting set with the lighting
                    // layout it declares at 3; both outlive the renderer's frames and are not
                    // updated after creation (the lighting set only while the device is idle).
                    unsafe {
                        device.device.cmd_bind_descriptor_sets(
                            command_buffer,
                            vk::PipelineBindPoint::GRAPHICS,
                            layout,
                            2,
                            sets,
                            &[],
                        );
                    }
                    // SAFETY: the VAT layout declares exactly this 16-byte vertex-stage range
                    // at 0.
                    unsafe {
                        device.device.cmd_push_constants(
                            command_buffer,
                            layout,
                            vk::ShaderStageFlags::VERTEX,
                            0,
                            bytes_of::<VatPush>(&push),
                        );
                    }
                    bind_mesh(device, command_buffer, mesh);
                    mesh_bound = true;
                }
                let range = mesh.lods.level(level);
                // SAFETY: the level's indices lie inside the index buffer and are in range for
                // the vertex buffer (`CpuMesh::from_asset`); `starts[bucket] + count` stays
                // inside the list (`DrawList::bucket`). Texture reads: the mesh has exactly
                // `push.vertex_count` vertices (every level indexes the same vertices) and the
                // shader clamps every frame below `push.frame_count`, and
                // `VatMeshes::register` checked that `2 * frames * vertices` texels fit.
                unsafe {
                    device.device.cmd_draw_indexed(
                        command_buffer,
                        range.index_count,
                        count,
                        range.first_index,
                        0,
                        list.starts[bucket],
                    );
                }
            }
        }
    }

    /// One draw per skinned instance whose mesh's material is (`textured`) or is not
    /// textured, with `pipeline`.
    fn record_skinned(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        list: &DrawList,
        pipeline: &GraphicsPipeline,
        textured: bool,
    ) {
        self.draw_skinned(
            device,
            command_buffer,
            list,
            pipeline,
            textured,
            true,
            false,
        );
    }

    /// One draw per skinned instance whose mesh's material is (`textured`) or is not
    /// textured, with `pipeline`, binding each material set when `bind_materials`; with
    /// `outlined_only` (the hull) only the outlined instances.
    #[allow(clippy::too_many_arguments)] // The draw loop's switches.
    fn draw_skinned(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        list: &DrawList,
        pipeline: &GraphicsPipeline,
        textured: bool,
        bind_materials: bool,
        outlined_only: bool,
    ) {
        let mut bound_pipeline = false;
        let mut bound_slot = u32::MAX;
        for draw in &list.skinned {
            let mesh = &self.meshes[draw.slot as usize];
            let set = self.mesh_material_sets[draw.slot as usize];
            if (set != vk::DescriptorSet::null()) != textured {
                continue;
            }
            let bucket = draw.slot as usize * MAX_LODS + usize::from(draw.lod) % MAX_LODS;
            if outlined_only && draw.instance >= list.starts[bucket] + list.outlined[bucket] {
                continue;
            }
            if !bound_pipeline {
                // The pipeline was created with the shared layout the descriptor sets
                // are bound with, so sets 0/1 stay valid for it.
                bind_pipeline(device, command_buffer, pipeline);
                bound_pipeline = true;
            }
            if draw.slot != bound_slot {
                bound_slot = draw.slot;
                if textured && bind_materials {
                    bind_material(device, command_buffer, self.pipeline_layout.handle(), set);
                }
                // Skinned meshes (joint_count > 0) hold `SkinnedVertex` data, the layout
                // of the bound skinned pipeline.
                bind_mesh(device, command_buffer, mesh);
            }
            // ADR-0021 §2c: the previous palette for the temporal prepass (unused elsewhere).
            let push = SkinPush {
                first_bone: draw.first_bone,
                joint_count: draw.joint_count,
                reserved: [draw.previous_bone, u32::from(draw.has_previous)],
            };
            // SAFETY: the shared pipeline layout declares exactly this 16-byte
            // vertex-stage range at offset 0.
            unsafe {
                device.device.cmd_push_constants(
                    command_buffer,
                    self.pipeline_layout.handle(),
                    vk::ShaderStageFlags::VERTEX,
                    0,
                    bytes_of(&push),
                );
            }
            let range = mesh.lods.level(usize::from(draw.lod));
            // SAFETY: the level's indices lie inside the index buffer and are in range for the
            // vertex buffer (re-checked in `CpuSkinnedMesh::from_asset`). `draw.instance` is a
            // bucketed instance slot of the list, inside the view's instance buffer. Bones:
            // `skinned_draw` guarantees `joint_count >= 1` and `first_bone + joint_count <=
            // bone_count <= max_bones`, the bone buffer's capacity, and the shader clamps each
            // joint index below `joint_count`, so every bone read stays inside this frame's
            // upload.
            unsafe {
                device.device.cmd_draw_indexed(
                    command_buffer,
                    range.index_count,
                    1,
                    range.first_index,
                    0,
                    draw.instance,
                );
            }
        }
    }
}

impl SceneView {
    /// Creates `budget`'s per-frame buffers and writes `sets` (camera, instances: two per
    /// frame) for them.
    fn new(
        device: &DeviceContext,
        budget: ViewBudget,
        sets: &[vk::DescriptorSet],
        overlay_sets: &[vk::DescriptorSet],
    ) -> RenderResult<Self> {
        let ViewBudget {
            max_instances,
            max_bones,
            casters,
            temporal,
            overlay: _,
        } = budget;
        // The caster list (R8) follows the camera list in the same buffer.
        let lists = if casters { 2 } else { 1 };
        // Both SSBOs are bound with `WHOLE_SIZE`, so each must fit one storage-buffer range.
        let range_limit = u64::from(device.max_storage_buffer_range);
        let instance_buffer_size = u64::try_from(core::mem::size_of::<GpuInstance>())
            .ok()
            .zip(u64::try_from(max_instances.max(1) * lists).ok())
            .and_then(|(size, count)| size.checked_mul(count))
            .filter(|&bytes| bytes <= range_limit)
            .ok_or_else(|| {
                RenderError::NoSuitableDevice(format!(
                    "max_instances {max_instances} exceeds the device's storage-buffer range ({range_limit} bytes)"
                ))
            })?;
        // Clamping the bone budget to the range also keeps `(first_bone + joint) * 3` in
        // skinned.vert far below `u32::MAX`.
        let bones_that_fit = usize::try_from(range_limit / BONE_STRIDE).unwrap_or(usize::MAX);
        let max_bones = if max_bones > bones_that_fit {
            eprintln!(
                "[render] max_bones {max_bones} exceeds the storage-buffer range; using {bones_that_fit}"
            );
            bones_that_fit
        } else {
            max_bones
        };
        // ADR-0021 §2c: with temporal AA the previous palettes follow this frame's, so the
        // buffer holds twice the budget (each half within the range).
        let max_bones = if temporal {
            max_bones.min(bones_that_fit / 2)
        } else {
            max_bones
        };
        let palettes = if temporal { 2 } else { 1 };
        // At least one joint so the descriptor always names a non-empty buffer.
        let bone_buffer_size = BONE_STRIDE
            * u64::try_from(max_bones.max(1) * palettes).expect("bounded by the range limit");
        let frames = sets.len() / 2;
        let mut scene_frames = Vec::with_capacity(frames);
        for frame in 0..frames {
            let buffer = |name, size, usage| {
                GpuBuffer::new(
                    device.device.clone(),
                    device.allocator(),
                    name,
                    size,
                    usage,
                    MemoryLocation::CpuToGpu,
                )
            };
            let scene_frame = SceneFrame {
                camera_buffer: buffer(
                    "camera-uniform",
                    CAMERA_UNIFORM_SIZE,
                    vk::BufferUsageFlags::UNIFORM_BUFFER,
                )?,
                instance_buffer: buffer(
                    "unit-instances",
                    instance_buffer_size,
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                )?,
                bone_buffer: buffer(
                    "skin-bones",
                    bone_buffer_size,
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                )?,
                camera_set: sets[frame * 2],
                instance_set: sets[frame * 2 + 1],
                previous: if temporal {
                    Some(buffer(
                        "previous-instances",
                        (core::mem::size_of::<PrevInstance>() * max_instances.max(1)) as u64,
                        vk::BufferUsageFlags::STORAGE_BUFFER,
                    )?)
                } else {
                    None
                },
                overlay_camera: match overlay_sets.get(frame) {
                    Some(&set) => Some((
                        buffer(
                            "overlay-camera",
                            CAMERA_UNIFORM_SIZE,
                            vk::BufferUsageFlags::UNIFORM_BUFFER,
                        )?,
                        set,
                    )),
                    None => None,
                },
            };
            write_descriptors(&device.device, &scene_frame);
            scene_frames.push(scene_frame);
        }
        let skinned_capacity = max_instances.min(max_bones);
        let caster_base = u32::try_from(max_instances).map_err(|_| {
            RenderError::NoSuitableDevice(String::from("max_instances exceeds u32"))
        })?;
        Ok(Self {
            frames: scene_frames,
            camera: DrawList::new(0, skinned_capacity),
            casters: casters.then(|| DrawList::new(caster_base, skinned_capacity)),
            decisions: Vec::with_capacity(max_instances),
            terrain_runs: Vec::with_capacity(MAX_TERRAIN_CHUNKS),
            unit_count: 0,
            max_instances,
            max_bones,
        })
    }
}

/// Binds a scene pipeline.
fn bind_pipeline(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    pipeline: &GraphicsPipeline,
) {
    // SAFETY: the pipeline is live and was created with the scene pass's shared layout, so
    // the descriptor sets bound with that layout stay valid for it.
    unsafe {
        device.device.cmd_bind_pipeline(
            command_buffer,
            vk::PipelineBindPoint::GRAPHICS,
            pipeline.handle(),
        );
    }
}

/// Binds a material's descriptor set at set 2.
fn bind_material(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    layout: vk::PipelineLayout,
    set: vk::DescriptorSet,
) {
    // SAFETY: `set` was allocated at load time with the material layout, which the shared
    // pipeline layout declares at index 2; its buffer and views live as long as the
    // renderer, and it is never updated after creation.
    unsafe {
        device.device.cmd_bind_descriptor_sets(
            command_buffer,
            vk::PipelineBindPoint::GRAPHICS,
            layout,
            2,
            &[set],
            &[],
        );
    }
}

/// Binds a mesh's vertex and `u32` index buffers.
fn bind_mesh(device: &DeviceContext, command_buffer: vk::CommandBuffer, mesh: &GpuMesh) {
    // SAFETY: the mesh buffers live as long as the renderer and match the bound pipeline's
    // vertex layout (callers pair static meshes with static pipelines, skinned with skinned).
    unsafe {
        device
            .device
            .cmd_bind_vertex_buffers(command_buffer, 0, &[mesh.vertices.handle()], &[0]);
    }
    // SAFETY: the buffer is live, was created with INDEX_BUFFER usage and holds `u32` indices.
    unsafe {
        device.device.cmd_bind_index_buffer(
            command_buffer,
            mesh.indices.handle(),
            0,
            vk::IndexType::UINT32,
        );
    }
}

/// Clears each of `targets.clear_rects` (clamped to the target; empty ones skipped) to its
/// colour inside the rendering scope begun by `record`.
fn clear_rects(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    targets: &SceneTargets<'_>,
) {
    for clear in targets.clear_rects.iter().take(MAX_CLEAR_RECTS) {
        let Some(rect) = clamp_rect(clear.rect, targets.extent) else {
            continue;
        };
        let attachment = vk::ClearAttachment {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            color_attachment: 0,
            clear_value: vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: clear.color,
                },
            },
        };
        let rect = vk::ClearRect {
            rect,
            base_array_layer: 0,
            layer_count: 1,
        };
        // SAFETY: recording inside a rendering scope with one colour attachment; `rect` lies
        // inside the render area (the whole target) and names layer 0 of a single-layer
        // attachment.
        unsafe {
            device
                .device
                .cmd_clear_attachments(command_buffer, &[attachment], &[rect]);
        }
    }
}

/// `[x, y, width, height]` clamped to `extent`, or `None` when nothing is left.
fn clamp_rect([x, y, width, height]: [u32; 4], extent: vk::Extent2D) -> Option<vk::Rect2D> {
    let x = x.min(extent.width);
    let y = y.min(extent.height);
    let width = width.min(extent.width - x);
    let height = height.min(extent.height - y);
    if width == 0 || height == 0 {
        return None;
    }
    Some(vk::Rect2D {
        offset: vk::Offset2D {
            x: i32::try_from(x).ok()?,
            y: i32::try_from(y).ok()?,
        },
        extent: vk::Extent2D { width, height },
    })
}

fn write_descriptors(device: &ash::Device, frame: &SceneFrame) {
    let camera_info = [vk::DescriptorBufferInfo::default()
        .buffer(frame.camera_buffer.handle())
        .offset(0)
        .range(CAMERA_UNIFORM_SIZE)];
    let instance_info = [vk::DescriptorBufferInfo::default()
        .buffer(frame.instance_buffer.handle())
        .offset(0)
        .range(vk::WHOLE_SIZE)];
    let bone_info = [vk::DescriptorBufferInfo::default()
        .buffer(frame.bone_buffer.handle())
        .offset(0)
        .range(vk::WHOLE_SIZE)];
    // Without a previous-instance buffer binding 2 names the instances (nothing reads it).
    let previous_info = [vk::DescriptorBufferInfo::default()
        .buffer(
            frame
                .previous
                .as_ref()
                .unwrap_or(&frame.instance_buffer)
                .handle(),
        )
        .offset(0)
        .range(vk::WHOLE_SIZE)];
    let overlay_info = frame.overlay_camera.as_ref().map(|(buffer, _)| {
        [vk::DescriptorBufferInfo::default()
            .buffer(buffer.handle())
            .offset(0)
            .range(CAMERA_UNIFORM_SIZE)]
    });
    let mut writes = vec![
        vk::WriteDescriptorSet::default()
            .dst_set(frame.camera_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .buffer_info(&camera_info),
        vk::WriteDescriptorSet::default()
            .dst_set(frame.instance_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .buffer_info(&instance_info),
        vk::WriteDescriptorSet::default()
            .dst_set(frame.instance_set)
            .dst_binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .buffer_info(&bone_info),
        vk::WriteDescriptorSet::default()
            .dst_set(frame.instance_set)
            .dst_binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .buffer_info(&previous_info),
    ];
    if let (Some((_, set)), Some(info)) = (&frame.overlay_camera, &overlay_info) {
        writes.push(
            vk::WriteDescriptorSet::default()
                .dst_set(*set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .buffer_info(info),
        );
    }
    // SAFETY: both descriptor sets were allocated from a compatible pool and the buffer info
    // entries reference live buffers.
    unsafe { device.update_descriptor_sets(&writes, &[]) };
}

/// Maps an instance's mesh to a live slot; unknown ids draw as the cube.
fn mesh_slot(mesh: MeshId, mesh_count: usize) -> usize {
    let index = mesh.index();
    if index < mesh_count { index } else { 0 }
}

/// The draw for one skinned instance, or `None` if its bone range is empty or reaches past
/// the `uploaded` joint matrices. The pushed joint count is at most the mesh's.
fn skinned_draw(
    bones: BoneRange,
    mesh_joints: u32,
    uploaded: u32,
    slot: usize,
    instance: u32,
) -> Option<SkinnedDraw> {
    if bones.count == 0 || mesh_joints == 0 {
        return None;
    }
    if bones.first.checked_add(bones.count)? > uploaded {
        return None;
    }
    Some(SkinnedDraw {
        slot: u32::try_from(slot).ok()?,
        instance,
        first_bone: bones.first,
        joint_count: bones.count.min(mesh_joints),
        lod: 0,
        previous_bone: 0,
        has_previous: false,
    })
}

/// Whether `unit` has a drawable outline slot (`1..OUTLINE_SLOTS`, ADR-0016).
fn is_outlined(unit: &crate::UnitInstance) -> bool {
    (1..crate::scene::OUTLINE_SLOTS).contains(&usize::from(unit.outline))
}

#[cfg(test)]
mod tests {
    use super::{
        BUCKETS, DrawList, IN_CAMERA, IN_CASTERS, LOD_SHIFT, MAX_LODS, SkinPush, SkinnedDraw,
        clamp_rect, keep_drawn, skinned_draw,
    };
    use crate::{BoneRange, MeshId, UnitInstance};
    use moba_core::math::{Mat4, Vec4};

    #[test]
    fn clear_rects_are_clamped_to_the_target() {
        let extent = ash::vk::Extent2D {
            width: 256,
            height: 128,
        };
        let inside = clamp_rect([128, 0, 128, 128], extent).expect("inside");
        assert_eq!((inside.offset.x, inside.offset.y), (128, 0));
        assert_eq!((inside.extent.width, inside.extent.height), (128, 128));
        let clipped = clamp_rect([200, 100, 128, 128], extent).expect("partly inside");
        assert_eq!((clipped.extent.width, clipped.extent.height), (56, 28));
        assert!(clamp_rect([256, 0, 16, 16], extent).is_none());
        assert!(clamp_rect([0, 0, 0, 16], extent).is_none());
    }

    #[test]
    fn skin_push_is_16_bytes() {
        assert_eq!(core::mem::size_of::<SkinPush>(), 16);
    }

    #[test]
    fn skinned_draws_stay_inside_the_uploaded_bones() {
        let draw = skinned_draw(BoneRange::new(64, 64), 7, 128, 3, 9).expect("in range");
        assert_eq!(
            draw,
            SkinnedDraw {
                slot: 3,
                instance: 9,
                first_bone: 64,
                // Never more than the mesh's joints.
                joint_count: 7,
                lod: 0,
                previous_bone: 0,
                has_previous: false,
            }
        );
        assert_eq!(skinned_draw(BoneRange::NONE, 7, 128, 3, 9), None);
        assert_eq!(skinned_draw(BoneRange::new(122, 7), 7, 128, 3, 9), None);
        assert_eq!(
            skinned_draw(BoneRange::new(u32::MAX, 2), 7, 128, 3, 9),
            None
        );
        assert_eq!(skinned_draw(BoneRange::new(0, 7), 0, 128, 3, 9), None);
        let partial = skinned_draw(BoneRange::new(0, 3), 7, 3, 1, 0).expect("short range");
        assert_eq!(partial.joint_count, 3);
    }

    fn unit(mesh: u16) -> UnitInstance {
        UnitInstance::new(Mat4::IDENTITY, Vec4::ONE).with_mesh(MeshId::from_index(mesh))
    }

    /// Buckets the units with every decision in the camera list at LOD0.
    fn camera_list(units: &[UnitInstance], mesh_count: usize) -> DrawList {
        let mut list = DrawList::new(0, 4);
        let decisions: Vec<(u32, u8)> = (0..units.len())
            .map(|index| (u32::try_from(index).expect("small"), IN_CAMERA))
            .collect();
        list.bucket(units, &decisions, IN_CAMERA, mesh_count);
        list
    }

    /// R8 review: the instance cap applies after culling. With room for 3, a scene of 3
    /// off-screen units followed by 4 visible ones draws the first 3 visible ones (not 0), in
    /// scene order, and reports the 4th as dropped; the decisions never grow past the cap.
    #[test]
    fn the_instance_cap_applies_after_culling() {
        let units = [
            unit(0),
            unit(0),
            unit(0),
            unit(1),
            unit(1),
            unit(1),
            unit(1),
        ];
        let mut decisions = Vec::with_capacity(3);
        let visible = |unit: &UnitInstance| {
            if unit.mesh == MeshId::from_index(1) {
                IN_CAMERA
            } else {
                0
            }
        };
        let dropped = keep_drawn(&units, 3, &mut decisions, visible);
        assert_eq!(decisions, [(3, IN_CAMERA), (4, IN_CAMERA), (5, IN_CAMERA)]);
        assert_eq!(dropped, 1);
        assert_eq!(decisions.capacity(), 3, "no growth");
        // A caster-only unit takes a slot too (the shadow pass draws it).
        let caster = |unit: &UnitInstance| {
            if unit.mesh == MeshId::from_index(0) {
                IN_CASTERS
            } else {
                IN_CAMERA
            }
        };
        let dropped = keep_drawn(&units, 3, &mut decisions, caster);
        assert_eq!(decisions.len(), 3);
        assert_eq!(dropped, 4);
        let none = keep_drawn(&units, 3, &mut decisions, |_| 0);
        assert_eq!((decisions.len(), none), (0, 0));
    }

    #[test]
    fn buckets_are_contiguous_and_unknown_meshes_draw_as_cubes() {
        let units = [unit(2), unit(0), unit(1), unit(2), unit(9), unit(0)];
        let list = camera_list(&units, 3);
        // Mesh 9 is not registered (3 meshes), so it joins the cube bucket.
        let lod0 = |slot: usize| slot * MAX_LODS;
        assert_eq!(
            [0, 1, 2, 3].map(|slot| list.counts[lod0(slot)]),
            [3, 1, 2, 0]
        );
        assert_eq!(
            [0, 1, 2, 3].map(|slot| list.starts[lod0(slot)]),
            [0, 3, 4, 6]
        );
        assert_eq!(list.cursors, list.starts);
        assert_eq!(list.starts[BUCKETS - 1], 6);
        assert_eq!(list.len, 6);
    }

    #[test]
    fn cube_only_scenes_keep_one_run_in_scene_order() {
        let list = camera_list(&[unit(0); 5], 1);
        assert_eq!(list.counts[0], 5);
        assert_eq!(list.starts[0], 0);
        assert!(list.counts[1..].iter().all(|&count| count == 0));
    }

    /// R8: culled units leave their bucket without reordering the rest; each list counts only
    /// its own units, from its own base; a unit's level picks its bucket inside the mesh.
    #[test]
    fn lists_bucket_their_own_units_by_mesh_and_level() {
        let units = [unit(1), unit(1), unit(0), unit(1), unit(1)];
        let both = IN_CAMERA | IN_CASTERS;
        let decisions = [
            (0, both),
            (1, IN_CASTERS),
            (2, IN_CAMERA),
            (3, both | (2 << LOD_SHIFT)),
            (4, IN_CAMERA),
        ];
        let mut camera = DrawList::new(0, 4);
        camera.bucket(&units, &decisions, IN_CAMERA, 2);
        let mut casters = DrawList::new(100, 4);
        casters.bucket(&units, &decisions, IN_CASTERS, 2);
        assert_eq!(camera.len, 4);
        assert_eq!(casters.len, 3);
        let mesh1 = MAX_LODS;
        assert_eq!(
            (
                camera.counts[0],
                camera.counts[mesh1],
                camera.counts[mesh1 + 2]
            ),
            (1, 2, 1)
        );
        assert_eq!((camera.starts[mesh1], camera.starts[mesh1 + 2]), (1, 3));
        assert_eq!((casters.counts[mesh1], casters.counts[mesh1 + 2]), (2, 1));
        assert_eq!(casters.starts[0], 100, "the caster list starts at its base");
        assert_eq!(casters.starts[mesh1 + 2], 102);
        // Placing keeps scene order inside a bucket.
        assert_eq!(camera.place(&units[0], mesh1), 1);
        assert_eq!(camera.place(&units[4], mesh1), 2);
        assert_eq!(casters.place(&units[1], mesh1), 100);
    }

    #[test]
    fn outlined_instances_lead_their_mesh_bucket() {
        // Mesh 1: plain, outlined, plain, outlined (slot 9 is out of range: not outlined).
        let units = [
            unit(1),
            unit(1).with_outline(2),
            unit(0).with_outline(9),
            unit(1),
            unit(1).with_outline(7),
        ];
        let list = camera_list(&units, 2);
        let mesh1 = MAX_LODS;
        assert_eq!((list.counts[0], list.counts[mesh1]), (1, 4));
        assert_eq!((list.outlined[0], list.outlined[mesh1]), (0, 2));
        // Mesh 1's outlined instances go to [1, 3), its plain ones to [3, 5).
        assert_eq!((list.cursors[mesh1], list.plain_cursors[mesh1]), (1, 3));
        assert_eq!((list.cursors[0], list.plain_cursors[0]), (0, 0));
    }
}
