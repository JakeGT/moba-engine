//! Vulkan renderer: device setup, swapchain, pipelines and the render loop.
//!
//! The renderer requires Vulkan 1.3 and uses core dynamic rendering and synchronization2
//! (ADR-0002). Each frame is a small render graph (ADR-0008): a scene pass into an HDR
//! target, a post chain (ADR-0015: AO, bloom, exposure, grading, anti-aliasing, resolution
//! scaling) into the swapchain and a UI pass on top, with barriers
//! derived from the passes' declared accesses and per-pass GPU timestamps
//! ([`Renderer::gpu_timings`]). On macOS it runs through `MoltenVK` or `KosmicKrisp` with
//! `VK_KHR_portability_subset`. It owns no winit types; the surface is created from the
//! platform's [`moba_platform::Window`].

#![allow(clippy::module_name_repetitions)]

mod device;
mod dynres;
mod error;
mod font;
mod font_data;
mod font_metrics;
mod frame_graph;
mod graph;
mod instance;
mod lighting;
mod material;
mod mesh;
mod passes;
mod pipeline;
mod pipeline_cache;
mod post;
mod profiler;
mod quality;
mod renderer;
mod resources;
mod scene;
mod shaders;
mod swapchain;
mod targets;
mod texture;
mod ui;
mod vat;
mod vfx;

pub use error::{RenderError, RenderResult};
pub use instance::{ValidationMode, VulkanProbe, probe_vulkan};
pub use lighting::{
    LightingConfig, MAX_SHADOW_RESOLUTION, MIN_SHADOW_RESOLUTION, SceneLighting, ShadowFilter,
};
pub use material::{
    MAX_MATERIALS, MAX_TEXTURES, MaterialData, MaterialId, MaterialStats, TextureId,
};
pub use passes::overlay::ui_vertices_truncated;
pub use passes::scene::{bones_truncated, skinned_instances_dropped};
pub use pipeline_cache::{PipelineCacheRejection, PipelineCacheState};
pub use post::{
    AntiAliasing, MAX_RENDER_SCALE, MIN_RENDER_SCALE, PostConfig, PostLook, ScreenEffects,
    UpscaleFilter,
};
pub use profiler::{GpuTimings, PassTiming};
pub use quality::QualityTier;
pub use renderer::{
    CapturedFrame, DEFAULT_MAX_BONES, DEFAULT_UI_IMAGE_SIZES, MAX_UI_IMAGE_SIZE, PipelineStats,
    RenderConfig, Renderer,
};
pub use scene::{
    BoneRange, Camera, ClearRect, MeshId, OFFSCREEN_MAX_BONES, OFFSCREEN_MAX_INSTANCES,
    OffscreenRender, Scene, UnitInstance, VISION_CIRCLES, VISION_WALLS, VisionCircles,
};

/// Environment descriptor (SH-L2 diffuse, intensity) accepted by [`Renderer::set_environment`].
pub use moba_assets::EnvironmentDesc;
/// One joint's skinning matrix (three `vec4` rows) as stored in [`Scene::bones`].
pub use moba_assets::JointMatrix;
/// Validated mesh accepted by [`Renderer::register_mesh`] and, when it carries a skin,
/// [`Renderer::register_skinned_mesh`].
pub use moba_assets::MeshData;
/// Material shader permutation and texture slots ([`MaterialData`]).
pub use moba_assets::{MaterialShader, TextureSlot};
/// Validated texture accepted by [`Renderer::register_texture`] (and, as a cube map, by
/// [`Renderer::set_environment`]).
pub use moba_assets::{TextureData, TextureFormat};
/// Vertex-animation-texture descriptor and per-instance pose for
/// [`Renderer::register_vat_mesh`] and [`UnitInstance::with_vat`] (R4b).
pub use moba_assets::{VatDescriptor, VatPose, VatSample};
pub use ui::{
    ATLAS_HEIGHT, ATLAS_WIDTH, TextStyle, UI_IMAGE_SLOTS, UiDrawList, UiImage, UiVertex,
    build_atlas, pack_rgba8,
};
pub use vat::MAX_VAT_MESHES;
pub use vfx::{
    ParticleBlend, ParticleRole, ParticleShape, ParticleSpawn, VFX_MESH_SLOTS, VfxBudget, VfxFrame,
    VfxStats,
};
