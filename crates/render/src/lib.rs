//! Vulkan renderer: device setup, swapchain, pipelines and the render loop.
//!
//! The renderer targets Vulkan 1.2 with `VK_KHR_dynamic_rendering`,
//! `VK_KHR_synchronization2` and descriptor indexing, falling back to promoted core
//! features on Vulkan 1.3+ devices. It owns no winit types; the surface is created from
//! the platform's [`moba_platform::Window`].

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
mod ui;

pub use error::{RenderError, RenderResult};
pub use instance::ValidationMode;
pub use renderer::{CapturedFrame, RenderConfig, Renderer};
pub use scene::{Camera, Scene, UnitInstance, VISION_CIRCLES, VisionCircles};
pub use ui::{UiDrawList, UiVertex};
