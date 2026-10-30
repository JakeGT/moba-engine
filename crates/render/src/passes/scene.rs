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
//! **Foliage and water** (R3b, ADR-0017): drawn last in the scene pass with their own layouts
//! (`super::environment`): the main view's visible foliage chunks, then the simple water
//! (alpha-blended over the carved bed) in every terrain view. The full water is a pass of
//! its own after the scene ([`ScenePass::record_water_pass`]).

use core::sync::atomic::{AtomicU64, Ordering};

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};
use gpu_allocator::MemoryLocation;
use moba_assets::JointMatrix;

use crate::device::DeviceContext;
use crate::environment::EnvironmentConfig;
use crate::error::{RenderError, RenderResult};
use crate::lighting::FrameLighting;
use crate::mesh::{self, GpuMesh};
use crate::pipeline::{self, DepthOnly, PipelineSpec, SceneBlend, VertexLayout, VertexlessDepth};
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuBuffer, GraphicsPipeline, PipelineLayout,
};
use crate::scene::{BoneRange, CameraUniform, ClearRect, GpuInstance, MeshId, Scene};
use crate::shaders::ShaderSource;
use crate::vat::{VatMeshes, VatPush};
use crate::world::WorldFrame;

use super::environment::EnvironmentResources;

/// Mesh slots, including the built-in cube in slot 0. Bucketing scratch is sized by this.
pub(crate) const MAX_MESHES: usize = 64;
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

/// One skinned instance: drawn alone so its push constant can select its bones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SkinnedDraw {
    slot: u32,
    instance: u32,
    first_bone: u32,
    joint_count: u32,
}

/// Per-frame-in-flight scene buffers and their descriptor sets.
struct SceneFrame {
    camera_buffer: GpuBuffer,
    instance_buffer: GpuBuffer,
    /// Joint matrices for this frame's skinned draws (set 1, binding 1).
    bone_buffer: GpuBuffer,
    camera_set: vk::DescriptorSet,
    instance_set: vk::DescriptorSet,
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
    pub(crate) extent: vk::Extent2D,
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
}

/// One view's per-frame buffers and its bucketing scratch.
struct SceneView {
    frames: Vec<SceneFrame>,
    /// Per-frame bucketing scratch (instances per mesh, first instance, write cursor).
    mesh_counts: [u32; MAX_MESHES],
    mesh_starts: [u32; MAX_MESHES],
    mesh_cursors: [u32; MAX_MESHES],
    /// Outlined instances per mesh (R2b): they lead the mesh's range, so the hull pipelines
    /// draw exactly `mesh_outlined[slot]` instances from `mesh_starts[slot]`.
    mesh_outlined: [u32; MAX_MESHES],
    /// Write cursor of the non-outlined instances (after the outlined ones).
    mesh_plain_cursors: [u32; MAX_MESHES],
    /// Per-frame skinned draws; capacity fixed at creation so pushing never reallocates.
    skinned_draws: Vec<SkinnedDraw>,
    /// Units uploaded for the frame being recorded.
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
}

/// Layouts and mode the R3b pipelines are built for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EnvironmentLayouts {
    pub(crate) foliage: vk::PipelineLayout,
    pub(crate) water: vk::PipelineLayout,
    pub(crate) full_water: bool,
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
        } = set;
        let module = |name| shaders.module(device, name);
        let unit_vertex = module("unit.vert")?;
        let unit_fragment = module("unit.frag")?;
        let textured_vertex = module("unit_textured.vert")?;
        let textured_fragment = module("unit_textured.frag")?;
        let terrain_vertex = module("terrain.vert")?;
        let terrain_fragment = module("terrain.frag")?;
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
        let foliage_vertex = module("foliage.vert")?;
        let foliage_fragment = module("foliage.frag")?;
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
        let water_fragment = module("water.frag")?;
        let water = pipeline::create_blended(
            device,
            &water_spec(water_fragment.handle(), samples),
            SceneBlend::Premultiplied,
        )
        .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))?;
        // The full water draws after any MSAA resolve: single-sampled, in the base set only.
        let water_full = if environment.full_water && samples == vk::SampleCountFlags::TYPE_1 {
            let fragment = module("water_refraction.frag")?;
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
        let prepass = if prepass {
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
            })
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
    /// Foliage and water (R3b).
    environment: EnvironmentResources,
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
        };
        let main = msaa.then_some(SceneSetSpec {
            samples: self.samples,
            shadow: None,
            prepass: self.prepass,
            outline_ids: self.outline_ids,
            hull: self.hull,
        });
        (base, main)
    }
}

/// Bindings of the scene's set 0 (the camera block, vertex + fragment) and set 1 (binding 0
/// instances, binding 1 joint matrices read only by `skinned.vert`; vertex).
pub(crate) fn buffer_set_layout_bindings() -> (
    [vk::DescriptorSetLayoutBinding<'static>; 1],
    [vk::DescriptorSetLayoutBinding<'static>; 2],
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
    (camera, [storage(0), storage(1)])
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
        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(frame_count),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(2 * frame_count),
        ];
        let descriptor_pool =
            DescriptorPool::new(device.device.clone(), &pool_sizes, 2 * frame_count)?;

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
            scene_views.push(SceneView::new(
                device,
                *budget,
                &sets[first_set..first_set + frames * 2],
            )?);
        }

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
            environment,
        })
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

    /// Replaces the map's terrain mesh (`None`: the R2a plane). The device must be idle.
    pub(crate) fn set_terrain_mesh(&mut self, mesh: Option<GpuMesh>) {
        self.map_terrain = mesh;
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
        self.meshes.push(mesh::upload(device, &cpu)?);
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
        self.meshes.push(mesh::upload_skinned(device, &cpu)?);
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
        let gpu = mesh::upload(device, &cpu)?;
        let entry = self.vat.register(device, data, texture, descriptor)?;
        let entry = u8::try_from(entry).map_err(|_| RenderError::InvalidVat("too many VATs"))?;
        self.meshes.push(gpu);
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
    /// buckets its draws. The frame's fence must have been waited on. The truncation counters
    /// report the main view (0) only.
    pub(crate) fn prepare(
        &mut self,
        view: usize,
        frame: usize,
        scene: &Scene<'_>,
        lighting: &FrameLighting,
        world: &WorldFrame,
        extent: vk::Extent2D,
    ) -> RenderResult<()> {
        let mesh_count = self.meshes.len();
        let Self { views, meshes, .. } = self;
        let state = &mut views[view];
        let unit_count = scene.units.len().min(state.max_instances);
        let bone_count = scene.bones.len().min(state.max_bones);
        if view == 0 {
            BONES_TRUNCATED.store(
                u64::try_from(scene.bones.len() - bone_count).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
        }
        state.skinned_draws.clear();
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
        target.camera_buffer.write_bytes(0, bytes_of(&uniform));
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
        if unit_count > 0 {
            let units = &scene.units[..unit_count];
            bucket_by_mesh(
                units,
                mesh_count,
                &mut state.mesh_counts,
                &mut state.mesh_outlined,
                &mut state.mesh_starts,
                &mut state.mesh_cursors,
                &mut state.mesh_plain_cursors,
            );
            let mapped = target
                .instance_buffer
                .mapped_slice_mut::<GpuInstance>(unit_count)
                .ok_or_else(|| {
                    RenderError::NoSuitableDevice(String::from(
                        "instance buffer is not host visible",
                    ))
                })?;
            // Each mesh's instances land contiguously at its `mesh_starts` slot, so one draw
            // per mesh reads them through `firstInstance` (gl_InstanceIndex); the outlined
            // ones first, so the hulls draw only them.
            for unit in units {
                let slot = mesh_slot(unit.mesh, mesh_count);
                let cursor = if is_outlined(unit) {
                    &mut state.mesh_cursors[slot]
                } else {
                    &mut state.mesh_plain_cursors[slot]
                };
                let instance = *cursor;
                mapped[instance as usize] = GpuInstance::from(*unit);
                *cursor += 1;
                let joints = meshes[slot].joint_count;
                if joints == 0 {
                    continue;
                }
                // A skinned mesh is only drawn with a bone range inside this frame's upload;
                // the capacity check keeps `push` from reallocating.
                match skinned_draw(unit.bones, joints, uploaded_bones, slot, instance) {
                    Some(draw) if state.skinned_draws.len() < state.skinned_draws.capacity() => {
                        state.skinned_draws.push(draw);
                    }
                    _ => skinned_dropped += 1,
                }
            }
        }
        if view == 0 {
            SKINNED_DROPPED.store(skinned_dropped, Ordering::Relaxed);
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
            let terrain = self.map_terrain.as_ref().unwrap_or(&self.terrain_mesh);
            bind_pipeline(device, command_buffer, &pipelines.terrain);
            bind_mesh(device, command_buffer, terrain);
            // SAFETY: the mesh is uploaded and drawable; its index buffer holds `index_count`
            // in-range `u32` indices (the grid is built with exact indices in
            // `terrain::grid_mesh`).
            unsafe {
                device
                    .device
                    .cmd_draw_indexed(command_buffer, terrain.index_count, 1, 0, 0, 0);
            }
        }

        if state.unit_count > 0 {
            for textured in [false, true] {
                let pipeline = if textured {
                    &pipelines.unit_textured
                } else {
                    &pipelines.unit
                };
                self.draw_static(device, command_buffer, state, pipeline, textured, true);
            }
        }

        if !state.skinned_draws.is_empty()
            && let Some(skinned) = &pipelines.skinned
        {
            // Textured skinned meshes fall back to the vertex-colour pipeline if their
            // permutation is missing.
            let textured = pipelines.skinned_textured.as_ref();
            self.record_skinned(device, command_buffer, state, skinned, false);
            self.record_skinned(
                device,
                command_buffer,
                state,
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
        if state.unit_count > 0
            && let Some(vat) = &pipelines.vat
        {
            self.draw_vat(
                device,
                command_buffer,
                state,
                vat,
                Some(targets.lighting),
                false,
            );
        }

        // Inverted-hull outlines last (R2b): depth-tested against everything above (minions
        // included), never written, so each hull shows only as its unit's silhouette ring
        // (ADR-0016). Only the outlined instances are drawn.
        if targets.hull_outlines && state.unit_count > 0 {
            if let Some(pipeline) = &pipelines.hull_unit {
                for textured in [false, true] {
                    self.draw_static_with(
                        device,
                        command_buffer,
                        state,
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
                        state,
                        pipeline,
                        textured,
                        false,
                        true,
                    );
                }
            }
            if let Some(pipeline) = &pipelines.hull_vat {
                self.draw_vat(device, command_buffer, state, pipeline, None, true);
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

    /// Records the shadow pass: the **main view's** units (static meshes instanced, skinned
    /// meshes one draw each, from view 0's buckets and bones) depth-only through the light's
    /// view-projection into `target`, cleared to 1. Offscreen views never cast into it. The
    /// terrain receives but never casts.
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
        if state.unit_count > 0 {
            self.bind_view_sets(device, command_buffer, state, frame);
            if let Some(pipeline) = &self.pipelines.unit_shadow {
                for textured in [false, true] {
                    self.draw_static(device, command_buffer, state, pipeline, textured, false);
                }
            }
            if let Some(pipeline) = &self.pipelines.skinned_shadow {
                for textured in [false, true] {
                    self.draw_skinned(
                        device,
                        command_buffer,
                        state,
                        pipeline,
                        textured,
                        false,
                        false,
                    );
                }
            }
            if let Some(pipeline) = &self.pipelines.vat_shadow {
                self.draw_vat(device, command_buffer, state, pipeline, None, false);
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
        let render_info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: target.extent,
            })
            .layer_count(1)
            .color_attachments(id_attachment.as_slice())
            .depth_attachment(&depth_attachment);
        device.cmd_begin_rendering(command_buffer, &render_info);
        super::set_viewport_scissor(
            device,
            command_buffer,
            super::flipped_viewport(target.extent),
            target.extent,
        );
        self.bind_view_sets(device, command_buffer, state, frame);
        // The same terrain mesh as `record` (the map's heightfield, or the R2a plane).
        let terrain = self.map_terrain.as_ref().unwrap_or(&self.terrain_mesh);
        bind_pipeline(device, command_buffer, &prepass.terrain);
        bind_mesh(device, command_buffer, terrain);
        // SAFETY: as in `record`: the terrain mesh holds `index_count` in-range indices.
        unsafe {
            device
                .device
                .cmd_draw_indexed(command_buffer, terrain.index_count, 1, 0, 0, 0);
        }
        // The position-only prepass shaders read no material, so one pipeline draws both
        // the vertex-colour and the textured meshes.
        if state.unit_count > 0 {
            for textured in [false, true] {
                self.draw_static(
                    device,
                    command_buffer,
                    state,
                    &prepass.unit,
                    textured,
                    false,
                );
            }
        }
        if !state.skinned_draws.is_empty()
            && let Some(skinned) = &prepass.skinned
        {
            for textured in [false, true] {
                self.draw_skinned(
                    device,
                    command_buffer,
                    state,
                    skinned,
                    textured,
                    false,
                    false,
                );
            }
        }
        // VAT minions (R4b), so GTAO, contact shadows, outlines and the particles' depth test
        // see them; last, as in `record` (the VAT layout re-binds set 2).
        if state.unit_count > 0
            && let Some(vat) = &prepass.vat
        {
            self.draw_vat(device, command_buffer, state, vat, None, false);
        }
        device.cmd_end_rendering(command_buffer);
    }

    /// One instanced draw with `pipeline` per static mesh whose material is (`textured`) or is
    /// not textured, binding each material set when `bind_materials`.
    fn draw_static(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        state: &SceneView,
        pipeline: &GraphicsPipeline,
        textured: bool,
        bind_materials: bool,
    ) {
        self.draw_static_with(
            device,
            command_buffer,
            state,
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
        state: &SceneView,
        pipeline: &GraphicsPipeline,
        textured: bool,
        bind_materials: bool,
        hull: bool,
    ) {
        let mut bound = false;
        {
            for (slot, mesh) in self.meshes.iter().enumerate() {
                // The hull draws only the outlined instances, which lead the mesh's range.
                let count = if hull {
                    state.mesh_outlined[slot]
                } else {
                    state.mesh_counts[slot]
                };
                let set = self.mesh_material_sets[slot];
                // Skinned meshes are drawn per instance below, and VAT meshes instanced, each
                // with their own pipeline.
                if count == 0
                    || mesh.joint_count > 0
                    || self.mesh_vat[slot] != u8::MAX
                    || (set != vk::DescriptorSet::null()) != textured
                {
                    continue;
                }
                if !bound {
                    bind_pipeline(device, command_buffer, pipeline);
                    bound = true;
                }
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
                // SAFETY: indices: `index_count == indices.len()`, so firstIndex 0 +
                // indexCount fits the index buffer, and every index is below the vertex
                // count (cube by construction; registered meshes re-checked in
                // `CpuMesh::from_asset`), so no vertex fetch leaves the vertex buffer.
                // Instances: `mesh_starts[slot] + count <= unit_count <= max_instances`, the
                // view's instance buffer's element capacity, by construction in
                // `bucket_by_mesh`.
                unsafe {
                    device.device.cmd_draw_indexed(
                        command_buffer,
                        mesh.index_count,
                        count,
                        0,
                        0,
                        state.mesh_starts[slot],
                    );
                }
            }
        }
    }

    /// One instanced draw per VAT mesh with `pipeline` (`vat` or `vat_shadow`), binding the
    /// mesh's VAT set at 2 and, for the colour pass, `lighting` at 3. Both are bound with the
    /// VAT layout, which differs from the scene layout in set 2 only, so sets 0/1 stay valid;
    /// binding 2 and 3 in one call re-validates 3 after 2 changed layout.
    fn draw_vat(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        state: &SceneView,
        pipeline: &GraphicsPipeline,
        lighting: Option<vk::DescriptorSet>,
        hull: bool,
    ) {
        let layout = self.vat_pipeline_layout.handle();
        let mut bound = false;
        for (slot, mesh) in self.meshes.iter().enumerate() {
            // The hull draws only the outlined instances, which lead each mesh's range.
            let count = if hull {
                state.mesh_outlined[slot]
            } else {
                state.mesh_counts[slot]
            };
            let entry = self.mesh_vat[slot];
            if count == 0 || entry == u8::MAX {
                continue;
            }
            let Some((set, push)) = self.vat.entry(usize::from(entry)) else {
                continue;
            };
            if !bound {
                bind_pipeline(device, command_buffer, pipeline);
                bound = true;
            }
            let lighting_set = [set, lighting.unwrap_or(vk::DescriptorSet::null())];
            let sets = if lighting.is_some() {
                &lighting_set[..]
            } else {
                &lighting_set[..1]
            };
            // SAFETY: the VAT set was allocated with the VAT set layout, which the VAT pipeline
            // layout declares at 2, and the lighting set with the lighting layout it declares
            // at 3; both outlive the renderer's frames and are not updated after creation
            // (the lighting set only while the device is idle).
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
            // SAFETY: the VAT layout declares exactly this 16-byte vertex-stage range at 0.
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
            // SAFETY: indices are in range for the vertex buffer (`CpuMesh::from_asset`);
            // `mesh_starts[slot] + count <= unit_count <= max_instances` (`bucket_by_mesh`).
            // Texture reads: the mesh has exactly `push.vertex_count` vertices and the shader
            // clamps every frame below `push.frame_count`, and `VatMeshes::register` checked
            // that `2 * frames * vertices` texels fit the texture.
            unsafe {
                device.device.cmd_draw_indexed(
                    command_buffer,
                    mesh.index_count,
                    count,
                    0,
                    0,
                    state.mesh_starts[slot],
                );
            }
        }
    }

    /// One draw per skinned instance whose mesh's material is (`textured`) or is not
    /// textured, with `pipeline`.
    fn record_skinned(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        state: &SceneView,
        pipeline: &GraphicsPipeline,
        textured: bool,
    ) {
        self.draw_skinned(
            device,
            command_buffer,
            state,
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
        state: &SceneView,
        pipeline: &GraphicsPipeline,
        textured: bool,
        bind_materials: bool,
        outlined_only: bool,
    ) {
        {
            let mut bound_pipeline = false;
            let mut bound_slot = u32::MAX;
            for draw in &state.skinned_draws {
                let mesh = &self.meshes[draw.slot as usize];
                let set = self.mesh_material_sets[draw.slot as usize];
                if (set != vk::DescriptorSet::null()) != textured {
                    continue;
                }
                let slot = draw.slot as usize;
                if outlined_only
                    && draw.instance >= state.mesh_starts[slot] + state.mesh_outlined[slot]
                {
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
                let push = SkinPush {
                    first_bone: draw.first_bone,
                    joint_count: draw.joint_count,
                    reserved: [0; 2],
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
                // SAFETY: indices are in range for the vertex buffer (re-checked in
                // `CpuSkinnedMesh::from_asset`). `draw.instance < unit_count <= max_instances`
                // (it is a bucketed instance slot). Bones: `skinned_draw` guarantees
                // `joint_count >= 1` and `first_bone + joint_count <= bone_count <=
                // max_bones`, the bone buffer's capacity, and the shader clamps each joint
                // index below `joint_count`, so every bone read stays inside this frame's
                // upload.
                unsafe {
                    device.device.cmd_draw_indexed(
                        command_buffer,
                        mesh.index_count,
                        1,
                        0,
                        0,
                        draw.instance,
                    );
                }
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
    ) -> RenderResult<Self> {
        let ViewBudget {
            max_instances,
            max_bones,
        } = budget;
        // Both SSBOs are bound with `WHOLE_SIZE`, so each must fit one storage-buffer range.
        let range_limit = u64::from(device.max_storage_buffer_range);
        let instance_buffer_size = u64::try_from(core::mem::size_of::<GpuInstance>())
            .ok()
            .zip(u64::try_from(max_instances.max(1)).ok())
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
        // At least one joint so the descriptor always names a non-empty buffer.
        let bone_buffer_size =
            BONE_STRIDE * u64::try_from(max_bones.max(1)).expect("bounded by the range limit");
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
            };
            write_descriptors(&device.device, &scene_frame);
            scene_frames.push(scene_frame);
        }
        Ok(Self {
            frames: scene_frames,
            mesh_counts: [0; MAX_MESHES],
            mesh_starts: [0; MAX_MESHES],
            mesh_cursors: [0; MAX_MESHES],
            mesh_outlined: [0; MAX_MESHES],
            mesh_plain_cursors: [0; MAX_MESHES],
            skinned_draws: Vec::with_capacity(max_instances.min(max_bones)),
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
    let writes = [
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
    ];
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
    })
}

/// Counting-sort bucketing of `units` by mesh, without allocating.
///
/// On return `counts[m]` is the number of instances of mesh `m`, `starts[m]` the first
/// instance slot of that mesh's contiguous run, and `cursors[m] == starts[m]` (the write
/// cursor the caller advances while scattering). Runs are in mesh-slot order and keep scene
/// order within a mesh, so a scene that only uses the cube is laid out exactly as before.
fn bucket_by_mesh(
    units: &[crate::UnitInstance],
    mesh_count: usize,
    counts: &mut [u32; MAX_MESHES],
    outlined: &mut [u32; MAX_MESHES],
    starts: &mut [u32; MAX_MESHES],
    cursors: &mut [u32; MAX_MESHES],
    plain_cursors: &mut [u32; MAX_MESHES],
) {
    counts.fill(0);
    outlined.fill(0);
    for unit in units {
        let slot = mesh_slot(unit.mesh, mesh_count);
        counts[slot] += 1;
        outlined[slot] += u32::from(is_outlined(unit));
    }
    let mut next = 0u32;
    for slot in 0..MAX_MESHES {
        starts[slot] = next;
        plain_cursors[slot] = next + outlined[slot];
        next += counts[slot];
    }
    *cursors = *starts;
}

/// Whether `unit` has a drawable outline slot (`1..OUTLINE_SLOTS`, ADR-0016).
fn is_outlined(unit: &crate::UnitInstance) -> bool {
    (1..crate::scene::OUTLINE_SLOTS).contains(&usize::from(unit.outline))
}

#[cfg(test)]
mod tests {
    use super::{MAX_MESHES, SkinPush, SkinnedDraw, bucket_by_mesh, clamp_rect, skinned_draw};
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

    #[test]
    fn buckets_are_contiguous_and_unknown_meshes_draw_as_cubes() {
        let units = [unit(2), unit(0), unit(1), unit(2), unit(9), unit(0)];
        let mut counts = [0; MAX_MESHES];
        let mut starts = [0; MAX_MESHES];
        let mut cursors = [0; MAX_MESHES];
        let (mut outlined, mut plain) = ([0; MAX_MESHES], [0; MAX_MESHES]);
        bucket_by_mesh(
            &units,
            3,
            &mut counts,
            &mut outlined,
            &mut starts,
            &mut cursors,
            &mut plain,
        );
        // Mesh 9 is not registered (3 meshes), so it joins the cube bucket.
        assert_eq!(&counts[..4], &[3, 1, 2, 0]);
        assert_eq!(&starts[..4], &[0, 3, 4, 6]);
        assert_eq!(cursors, starts);
        assert_eq!(starts[MAX_MESHES - 1], 6);
    }

    #[test]
    fn cube_only_scenes_keep_one_run_in_scene_order() {
        let units = [unit(0); 5];
        let mut counts = [7; MAX_MESHES];
        let mut starts = [7; MAX_MESHES];
        let mut cursors = [7; MAX_MESHES];
        let (mut outlined, mut plain) = ([0; MAX_MESHES], [0; MAX_MESHES]);
        bucket_by_mesh(
            &units,
            1,
            &mut counts,
            &mut outlined,
            &mut starts,
            &mut cursors,
            &mut plain,
        );
        assert_eq!(counts[0], 5);
        assert_eq!(starts[0], 0);
        assert!(counts[1..].iter().all(|&count| count == 0));
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
        let mut counts = [0; MAX_MESHES];
        let mut starts = [0; MAX_MESHES];
        let mut cursors = [0; MAX_MESHES];
        let (mut outlined, mut plain) = ([0; MAX_MESHES], [0; MAX_MESHES]);
        bucket_by_mesh(
            &units,
            2,
            &mut counts,
            &mut outlined,
            &mut starts,
            &mut cursors,
            &mut plain,
        );
        assert_eq!(&counts[..2], &[1, 4]);
        assert_eq!(&outlined[..2], &[0, 2]);
        // Mesh 1's outlined instances go to [1, 3), its plain ones to [3, 5).
        assert_eq!((cursors[1], plain[1]), (1, 3));
        assert_eq!((cursors[0], plain[0]), (0, 0));
    }
}
