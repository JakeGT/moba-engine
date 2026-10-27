//! Vulkan renderer: device setup, swapchain, pipelines and the render loop.
//!
//! The renderer requires Vulkan 1.3 and uses core dynamic rendering and synchronization2
//! (ADR-0002). On macOS it runs through `MoltenVK` or `KosmicKrisp` with
//! `VK_KHR_portability_subset`. It owns no winit types; the surface is created from the
//! platform's [`moba_platform::Window`].

#![allow(clippy::module_name_repetitions)]

mod device;
mod error;
mod font_data;
mod instance;
mod mesh;
mod pipeline;
mod renderer;
mod resources;
mod scene;
mod swapchain;
mod texture;
mod ui;

pub use error::{RenderError, RenderResult};
pub use instance::{ValidationMode, VulkanProbe, probe_vulkan};
pub use renderer::{
    CapturedFrame, DEFAULT_MAX_BONES, RenderConfig, Renderer, bones_truncated,
    skinned_instances_dropped, ui_vertices_truncated,
};
pub use scene::{
    BoneRange, Camera, MeshId, Scene, UnitInstance, VISION_CIRCLES, VISION_WALLS, VisionCircles,
};

/// One joint's skinning matrix (three `vec4` rows) as stored in [`Scene::bones`].
pub use moba_assets::JointMatrix;
/// Validated mesh accepted by [`Renderer::register_mesh`] and, when it carries a skin,
/// [`Renderer::register_skinned_mesh`].
pub use moba_assets::MeshData;
pub use ui::{ATLAS_HEIGHT, ATLAS_WIDTH, UiDrawList, UiVertex, build_atlas};
