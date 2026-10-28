//! Window creation and the platform error type.

use core::fmt;
use std::sync::Arc;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};
use winit::dpi::LogicalSize;
use winit::event_loop::ActiveEventLoop;

/// Configuration for the main game window.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct WindowConfig {
    /// Window title.
    pub title: String,
    /// Logical width in pixels.
    pub width: u32,
    /// Logical height in pixels.
    pub height: u32,
    /// Whether the user may resize the window.
    pub resizable: bool,
    /// Authoritative simulation ticks per second.
    pub tick_rate: u32,
    /// Upper bound on rendered frames per second. `0` means uncapped (not recommended).
    pub max_frame_rate: u32,
}

impl WindowConfig {
    /// Creates a config with the given title and logical size.
    #[must_use]
    pub fn new(title: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            title: title.into(),
            width,
            height,
            ..Self::default()
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: String::from("MOBA"),
            width: 1280,
            height: 720,
            resizable: true,
            tick_rate: 30,
            max_frame_rate: 240,
        }
    }
}

impl WindowConfig {
    /// Returns this config with [`WindowConfig::title`] set.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Returns this config with [`WindowConfig::width`] set.
    #[must_use]
    pub const fn with_width(mut self, width: u32) -> Self {
        self.width = width;
        self
    }

    /// Returns this config with [`WindowConfig::height`] set.
    #[must_use]
    pub const fn with_height(mut self, height: u32) -> Self {
        self.height = height;
        self
    }

    /// Returns this config with [`WindowConfig::resizable`] set.
    #[must_use]
    pub const fn with_resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Returns this config with [`WindowConfig::tick_rate`] set.
    #[must_use]
    pub const fn with_tick_rate(mut self, tick_rate: u32) -> Self {
        self.tick_rate = tick_rate;
        self
    }

    /// Returns this config with [`WindowConfig::max_frame_rate`] set.
    #[must_use]
    pub const fn with_max_frame_rate(mut self, max_frame_rate: u32) -> Self {
        self.max_frame_rate = max_frame_rate;
        self
    }
}

/// Errors raised while creating or running the platform layer.
#[derive(Debug)]
#[non_exhaustive]
pub enum PlatformError {
    /// The windowing event loop could not be created or run.
    EventLoop(winit::error::EventLoopError),
    /// A native window could not be created.
    Window(winit::error::OsError),
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventLoop(error) => write!(f, "event loop error: {error}"),
            Self::Window(error) => write!(f, "window creation error: {error}"),
        }
    }
}

impl std::error::Error for PlatformError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::EventLoop(error) => Some(error),
            Self::Window(error) => Some(error),
        }
    }
}

/// A handle to the native game window.
///
/// Cloning is cheap (the winit window is internally reference counted). The renderer uses
/// its [`HasWindowHandle`] and [`HasDisplayHandle`] implementations to create a Vulkan
/// surface without depending on winit types.
#[derive(Clone)]
pub struct Window {
    inner: Arc<winit::window::Window>,
}

impl Window {
    pub(crate) fn new(inner: winit::window::Window) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }

    pub(crate) fn attributes(config: &WindowConfig) -> winit::window::WindowAttributes {
        winit::window::Window::default_attributes()
            .with_title(config.title.clone())
            .with_inner_size(LogicalSize::new(
                f64::from(config.width),
                f64::from(config.height),
            ))
            .with_resizable(config.resizable)
    }

    pub(crate) fn create(
        event_loop: &ActiveEventLoop,
        config: &WindowConfig,
    ) -> Result<Self, PlatformError> {
        event_loop
            .create_window(Self::attributes(config))
            .map(Self::new)
            .map_err(PlatformError::Window)
    }

    pub(crate) fn id(&self) -> winit::window::WindowId {
        self.inner.id()
    }

    /// Current inner size in physical pixels.
    #[must_use]
    pub fn size(&self) -> [u32; 2] {
        let size = self.inner.inner_size();
        [size.width, size.height]
    }

    /// DPI scale factor of the window.
    #[must_use]
    pub fn scale_factor(&self) -> f64 {
        self.inner.scale_factor()
    }

    /// Requests one frame from the event loop.
    pub fn request_redraw(&self) {
        self.inner.request_redraw();
    }

    /// Changes the window title.
    pub fn set_title(&self, title: &str) {
        self.inner.set_title(title);
    }

    /// Shows or hides the OS cursor.
    pub fn set_cursor_visible(&self, visible: bool) {
        self.inner.set_cursor_visible(visible);
    }
}

impl fmt::Debug for Window {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Window")
            .field("size", &self.size())
            .field("scale_factor", &self.scale_factor())
            .finish_non_exhaustive()
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.inner.window_handle()
    }
}

impl HasDisplayHandle for Window {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.inner.display_handle()
    }
}
