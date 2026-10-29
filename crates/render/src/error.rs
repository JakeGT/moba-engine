//! Renderer error types.

use core::fmt;

/// Errors raised while initialising or driving the renderer.
#[derive(Debug)]
#[non_exhaustive]
pub enum RenderError {
    /// The Vulkan loader could not be loaded.
    Loader(ash::LoadingError),
    /// A Vulkan call returned an error.
    Vulkan(ash::vk::Result),
    /// No physical device supports Vulkan 1.3 and the required features.
    NoSuitableDevice(String),
    /// The loader does not support Vulkan 1.3.
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
    /// The device was lost and must be recreated.
    DeviceLost,
    /// Texture dimensions overflowed or did not match the pixel buffer.
    InvalidTexture,
    /// [`Renderer::register_mesh`](crate::Renderer::register_mesh) was called with every
    /// mesh slot in use.
    TooManyMeshes,
    /// A mesh passed to [`Renderer::register_mesh`](crate::Renderer::register_mesh) is not a
    /// non-empty triangle list with in-range indices (or, for
    /// [`Renderer::register_skinned_mesh`](crate::Renderer::register_skinned_mesh), has no
    /// valid skin).
    InvalidMesh,
    /// GPU skinning is unavailable: disabled in [`RenderConfig`](crate::RenderConfig) or
    /// the skinned pipeline failed to build. Draw the static mesh instead.
    SkinningUnavailable,
    /// The frame's render graph failed to compile (an internal invariant; ADR-0008).
    RenderGraph(String),
    /// [`Renderer::register_texture`](crate::Renderer::register_texture) was called with
    /// every texture slot ([`MAX_TEXTURES`](crate::MAX_TEXTURES)) in use.
    TooManyTextures,
    /// [`Renderer::register_materials`](crate::Renderer::register_materials) would exceed
    /// [`MAX_MATERIALS`](crate::MAX_MATERIALS).
    TooManyMaterials,
    /// A material is inconsistent (unknown texture or mesh, a texture format its slot does
    /// not allow, a textured material without a base colour or on a mesh without UVs); the
    /// text says which.
    InvalidMaterial(&'static str),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Loader(error) => write!(f, "cannot load the Vulkan loader: {error}"),
            Self::Vulkan(result) => write!(f, "Vulkan error: {result:?}"),
            Self::NoSuitableDevice(reason) => write!(f, "no suitable GPU found: {reason}"),
            Self::UnsupportedApiVersion(version) => {
                write!(f, "Vulkan 1.3 is required, loader provides {version:#x}")
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
            Self::DeviceLost => f.write_str("device lost"),
            Self::TooManyTextures => f.write_str("texture limit reached"),
            Self::TooManyMaterials => f.write_str("material limit reached"),
            Self::InvalidMaterial(reason) => write!(f, "invalid material: {reason}"),
            Self::InvalidTexture => f.write_str("invalid texture dimensions"),
            Self::TooManyMeshes => write!(
                f,
                "mesh limit reached ({} including the built-in cube)",
                crate::passes::scene::MAX_MESHES
            ),
            Self::InvalidMesh => {
                f.write_str("invalid mesh (empty, not triangles or index out of range)")
            }
            Self::SkinningUnavailable => f.write_str("GPU skinning is unavailable"),
            Self::RenderGraph(reason) => write!(f, "render graph error: {reason}"),
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
        if result == ash::vk::Result::ERROR_DEVICE_LOST {
            Self::DeviceLost
        } else {
            Self::Vulkan(result)
        }
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
