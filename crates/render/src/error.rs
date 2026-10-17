//! Renderer error types.

use core::fmt;

/// Errors raised while initialising or driving the renderer.
#[derive(Debug)]
pub enum RenderError {
    /// The Vulkan loader could not be loaded.
    Loader(ash::LoadingError),
    /// A Vulkan call returned an error.
    Vulkan(ash::vk::Result),
    /// No physical device supports Vulkan 1.2 and the required features.
    NoSuitableDevice(String),
    /// The loader does not support Vulkan 1.2.
    UnsupportedApiVersion(u32),
    /// A required instance extension is missing.
    MissingInstanceExtension(String),
    /// A required device extension is missing.
    MissingDeviceExtension(String),
    /// A required layer is missing.
    MissingLayer(String),
    /// The window could not provide a native handle.
    WindowHandle(raw_window_handle::HandleError),
    /// GPU memory allocation failed.
    Allocation(gpu_allocator::AllocationError),
    /// An I/O operation (shader loading, screenshots) failed.
    Io(std::io::Error),
    /// A shader module was rejected.
    Shader(&'static str),
    /// The selected device has no graphics queue family.
    NoGraphicsQueue,
    /// The selected device has no queue family that can present to the surface.
    NoPresentQueue,
    /// The swapchain is out of date and the frame must be retried.
    SwapchainOutOfDate,
    /// The swapchain is suboptimal but still usable.
    SwapchainSuboptimal,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Loader(error) => write!(f, "cannot load the Vulkan loader: {error}"),
            Self::Vulkan(result) => write!(f, "Vulkan error: {result:?}"),
            Self::NoSuitableDevice(reason) => write!(f, "no suitable GPU found: {reason}"),
            Self::UnsupportedApiVersion(version) => {
                write!(f, "Vulkan 1.2 is required, loader provides {version:#x}")
            }
            Self::MissingInstanceExtension(name) => {
                write!(f, "missing instance extension {name}")
            }
            Self::MissingDeviceExtension(name) => write!(f, "missing device extension {name}"),
            Self::MissingLayer(name) => write!(f, "missing Vulkan layer {name}"),
            Self::WindowHandle(error) => write!(f, "window handle error: {error}"),
            Self::Allocation(error) => write!(f, "GPU allocation error: {error}"),
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Shader(name) => write!(f, "failed to create shader module {name}"),
            Self::NoGraphicsQueue => f.write_str("no graphics queue family"),
            Self::NoPresentQueue => f.write_str("no present queue family"),
            Self::SwapchainOutOfDate => f.write_str("swapchain out of date"),
            Self::SwapchainSuboptimal => f.write_str("swapchain suboptimal"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Loader(error) => Some(error),
            Self::WindowHandle(error) => Some(error),
            Self::Allocation(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ash::vk::Result> for RenderError {
    fn from(result: ash::vk::Result) -> Self {
        Self::Vulkan(result)
    }
}

impl From<gpu_allocator::AllocationError> for RenderError {
    fn from(error: gpu_allocator::AllocationError) -> Self {
        Self::Allocation(error)
    }
}

impl From<raw_window_handle::HandleError> for RenderError {
    fn from(error: raw_window_handle::HandleError) -> Self {
        Self::WindowHandle(error)
    }
}

impl From<std::io::Error> for RenderError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Renderer result alias.
pub type RenderResult<T> = core::result::Result<T, RenderError>;
