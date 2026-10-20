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
pub use renderer::{CapturedFrame, RenderConfig, Renderer, ui_vertices_truncated};
pub use scene::{Camera, Scene, UnitInstance, VISION_CIRCLES, VISION_WALLS, VisionCircles};
pub use ui::{ATLAS_HEIGHT, ATLAS_WIDTH, UiDrawList, UiVertex, build_atlas};
