//! The renderer: device and swapchain lifecycle, frame synchronisation and the frame graph.
//!
//! A frame is a precompiled render graph (ADR-0008, [`crate::frame_graph`]): the shadow pass
//! renders the directional shadow map (when shadows are on), the scene pass renders into an
//! HDR target with stylised PBR (ADR-0011), the composite pass tonemaps it into the swapchain
//! image and the UI pass draws the HUD on top. Frames that re-render an offscreen image (R7b:
//! hero portraits, the baked minimap) run that image's pass first, with the scene pipelines,
//! shadows off and the environment kept. The
//! pass code lives in [`crate::passes`]; this module owns acquisition, submission,
//! presentation, resize and the capture readback.

use core::ffi::CStr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ash::{khr, vk};
use gpu_allocator::MemoryLocation;
use moba_platform::Window;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::frame_graph::{
    self, Binding, Bindings, DepthFeatures, FramePlan, OFFSCREEN_VARIANTS, PassKind, ShadowSpec,
    VfxMode,
};
use crate::instance::{InstanceContext, ValidationMode};
use crate::lighting::{
    self, BakedAo, EnvironmentState, FrameLighting, LightingConfig, LightingResources, OutlineMode,
    ShadowFilter,
};
use crate::lights::LightStats;
use crate::material::{MaterialData, MaterialId, MaterialStats, Materials, TextureId};
use crate::passes::composite::CompositePass;
use crate::passes::contact::ContactPass;
use crate::passes::outline::OutlinePass;
use crate::passes::overlay::{UiImages, UiPass};
use crate::passes::scene::{
    PrepassPipelineSpec, PrepassTarget, ScenePass, SceneTargets, ShadowPipelineSpec, ShadowTarget,
    ViewBudget,
};
use crate::passes::vfx::{VfxPass, VfxTargets};
use crate::pipeline_cache::PipelineCacheState;
use crate::profiler::{GpuProfiler, GpuTimings};
use crate::quality::QualityTier;
use crate::resources::{CommandPool, Fence, GpuBuffer, GpuImage, Semaphore};
use crate::scene::OutlinePalette;
use crate::scene::{MeshId, OFFSCREEN_MAX_BONES, OFFSCREEN_MAX_INSTANCES, OffscreenRender, Scene};
use crate::shaders::ShaderSource;
use crate::swapchain::Swapchain;
use crate::targets::{self, OffscreenTarget, TransientPool};
use crate::ui::{UI_IMAGE_SLOTS, UiDrawList, UiImage};
use crate::vfx::{VFX_MESH_SLOTS, VfxBudget, VfxStats};

const FRAMES_IN_FLIGHT: usize = 2;
/// Default per-frame bone budget: 16 skinned instances × 64 joints (48 KiB per frame).
pub const DEFAULT_MAX_BONES: usize = 16 * 64;
/// Largest side of an offscreen [`UiImage`] (the guaranteed `maxImageDimension2D`).
pub const MAX_UI_IMAGE_SIZE: u32 = 4096;
/// Default [`RenderConfig::ui_image_sizes`]: a 2048x128 portrait strip (sixteen 128-pixel
/// cells) and a 512x512 minimap.
pub const DEFAULT_UI_IMAGE_SIZES: [[u32; 2]; UI_IMAGE_SLOTS] = [[2048, 128], [512, 512]];

/// Renderer configuration.
#[derive(Clone, Debug)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)] // Independent feature switches.
pub struct RenderConfig {
    /// Validation layer policy.
    pub validation: ValidationMode,
    /// Maximum number of units drawn per frame, shared by every mesh. The instance buffer
    /// is pre-sized; units past the cap are dropped in scene order.
    pub max_instances: usize,
    /// Request vertical sync (FIFO). Disable to allow mailbox/immediate.
    pub vsync: bool,
    /// Linear RGBA clear colour of the HDR scene target (tonemapped like the scene).
    pub clear_color: [f32; 4],
    /// Maximum number of UI vertices per frame.
    pub max_ui_vertices: usize,
    /// HUD scale factor applied when uploading UI vertices.
    pub ui_scale: f32,
    /// Joint matrices uploaded per frame for skinned meshes; [`Scene::bones`] past this are
    /// dropped and counted ([`bones_truncated`](crate::bones_truncated)). Default
    /// [`DEFAULT_MAX_BONES`].
    pub max_bones: usize,
    /// Build the GPU skinning pipeline. When `false` (or if the pipeline fails to build),
    /// [`Renderer::register_skinned_mesh`] returns [`RenderError::SkinningUnavailable`].
    pub skinning: bool,
    /// Quality tier every tiered feature keys off. Today it picks the HDR target format
    /// (`Low`: `B10G11R11_UFLOAT_PACK32`, otherwise `R16G16B16A16_SFLOAT`). Default `High`.
    pub quality_tier: QualityTier,
    /// Linear exposure multiplier applied before the tonemap. Default `1.0`; a non-finite or
    /// non-positive value is treated as `1.0`.
    pub exposure: f32,
    /// File the `VkPipelineCache` is loaded from at creation and saved to on drop. `None`
    /// (the default) builds every pipeline from scratch and saves nothing. A missing, corrupt
    /// or foreign file is a cold start, never an error ([`PipelineStats::cache`]).
    pub pipeline_cache: Option<PathBuf>,
    /// Debug builds (`debug_assertions`) only: a directory of SPIR-V overrides
    /// (`<module>.spv`) used instead of the built-in modules, and re-read by
    /// [`Renderer::reload_shaders`]. Release builds ignore it, because an override that does
    /// not match its pipeline's interface is invalid Vulkan usage. Default `None`.
    pub shader_dir: Option<PathBuf>,
    /// Upload BC7/BC5/BC4 textures as is when the device supports them. When `false`, or on
    /// devices without `textureCompressionBC`, they are decoded on the CPU at load. Default
    /// `true`.
    pub compressed_textures: bool,
    /// Build the GPU particle passes (ADR-0013) with the tier's [`VfxBudget`]. When `false`,
    /// or on a device whose graphics queue has no compute, the frame has no VFX passes and
    /// [`Scene::vfx`] spawns are ignored. Default `true`.
    pub vfx: bool,
    /// Size of each offscreen [`UiImage`] (R7b), indexed by [`UiImage::index`]; default
    /// [`DEFAULT_UI_IMAGE_SIZES`]. Each side is clamped to `1..=`[`MAX_UI_IMAGE_SIZE`]; a
    /// 1x1 image is the cheapest way to switch a slot off. The images are allocated once
    /// with the renderer in the HDR format (with a depth buffer each) and never resized.
    pub ui_image_sizes: [[u32; 2]; UI_IMAGE_SLOTS],
    /// Shadow and image-based-lighting features (ADR-0011). `None` (the default) uses
    /// [`LightingConfig::for_tier`] of [`RenderConfig::quality_tier`]; set it to override a
    /// tier default or turn a feature off.
    pub lighting: Option<LightingConfig>,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            validation: ValidationMode::IfAvailable,
            max_instances: 4096,
            vsync: true,
            clear_color: [0.028, 0.035, 0.055, 1.0],
            max_ui_vertices: 65_536,
            ui_scale: 1.0,
            max_bones: DEFAULT_MAX_BONES,
            skinning: true,
            quality_tier: QualityTier::High,
            exposure: 1.0,
            pipeline_cache: None,
            shader_dir: None,
            compressed_textures: true,
            vfx: true,
            ui_image_sizes: DEFAULT_UI_IMAGE_SIZES,
            lighting: None,
        }
    }
}

impl RenderConfig {
    /// Returns this config with [`RenderConfig::validation`] set.
    #[must_use]
    pub const fn with_validation(mut self, validation: ValidationMode) -> Self {
        self.validation = validation;
        self
    }

    /// Returns this config with [`RenderConfig::max_instances`] set.
    #[must_use]
    pub const fn with_max_instances(mut self, max_instances: usize) -> Self {
        self.max_instances = max_instances;
        self
    }

    /// Returns this config with [`RenderConfig::vsync`] set.
    #[must_use]
    pub const fn with_vsync(mut self, vsync: bool) -> Self {
        self.vsync = vsync;
        self
    }

    /// Returns this config with [`RenderConfig::clear_color`] set.
    #[must_use]
    pub const fn with_clear_color(mut self, clear_color: [f32; 4]) -> Self {
        self.clear_color = clear_color;
        self
    }

    /// Returns this config with [`RenderConfig::max_ui_vertices`] set.
    #[must_use]
    pub const fn with_max_ui_vertices(mut self, max_ui_vertices: usize) -> Self {
        self.max_ui_vertices = max_ui_vertices;
        self
    }

    /// Returns this config with [`RenderConfig::ui_scale`] set.
    #[must_use]
    pub const fn with_ui_scale(mut self, ui_scale: f32) -> Self {
        self.ui_scale = ui_scale;
        self
    }

    /// Returns this config with [`RenderConfig::max_bones`] set.
    #[must_use]
    pub const fn with_max_bones(mut self, max_bones: usize) -> Self {
        self.max_bones = max_bones;
        self
    }

    /// Returns this config with [`RenderConfig::skinning`] set.
    #[must_use]
    pub const fn with_skinning(mut self, skinning: bool) -> Self {
        self.skinning = skinning;
        self
    }

    /// Returns this config with [`RenderConfig::quality_tier`] set.
    #[must_use]
    pub const fn with_quality_tier(mut self, quality_tier: QualityTier) -> Self {
        self.quality_tier = quality_tier;
        self
    }

    /// Returns this config with [`RenderConfig::exposure`] set.
    #[must_use]
    pub const fn with_exposure(mut self, exposure: f32) -> Self {
        self.exposure = exposure;
        self
    }

    /// Returns this config with [`RenderConfig::pipeline_cache`] set.
    #[must_use]
    pub fn with_pipeline_cache(mut self, path: Option<PathBuf>) -> Self {
        self.pipeline_cache = path;
        self
    }

    /// Returns this config with [`RenderConfig::shader_dir`] set.
    #[must_use]
    pub fn with_shader_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.shader_dir = dir;
        self
    }

    /// Returns this config with [`RenderConfig::compressed_textures`] set.
    #[must_use]
    pub const fn with_compressed_textures(mut self, compressed: bool) -> Self {
        self.compressed_textures = compressed;
        self
    }

    /// Returns this config with [`RenderConfig::lighting`] set.
    #[must_use]
    pub const fn with_lighting(mut self, lighting: Option<LightingConfig>) -> Self {
        self.lighting = lighting;
        self
    }

    /// The lighting features this config asks for: [`RenderConfig::lighting`], else the
    /// tier's defaults.
    #[must_use]
    pub const fn effective_lighting(&self) -> LightingConfig {
        match self.lighting {
            Some(lighting) => lighting,
            None => LightingConfig::for_tier(self.quality_tier),
        }
    }

    /// Returns this config with [`RenderConfig::vfx`] set.
    #[must_use]
    pub const fn with_vfx(mut self, vfx: bool) -> Self {
        self.vfx = vfx;
        self
    }

    /// Returns this config with the size of offscreen `image` set
    /// ([`RenderConfig::ui_image_sizes`]).
    #[must_use]
    pub const fn with_ui_image_size(mut self, image: UiImage, size: [u32; 2]) -> Self {
        self.ui_image_sizes[image.index()] = size;
        self
    }
}

/// How the renderer's pipelines were built ([`Renderer::pipeline_stats`]).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct PipelineStats {
    /// Graphics pipelines built at creation (every pass and permutation).
    pub pipelines: u32,
    /// Driver time spent in `vkCreateGraphicsPipelines`, milliseconds.
    pub build_ms: f32,
    /// Wall time of [`Renderer::new`], milliseconds (device, swapchain, pipelines, meshes).
    pub startup_ms: f32,
    /// Whether the pipeline cache started warm, cold or rejected.
    pub cache: PipelineCacheState,
}

/// Per-frame-in-flight command recording and synchronisation.
///
/// Two command buffers per frame: `[0]` holds the passes that do not touch the swapchain
/// (the scene) and is submitted without waiting on the acquire semaphore, so its GPU time
/// never includes that wait; `[1]` holds the rest and waits on it. Both go in one
/// `vkQueueSubmit`, so the frame fence covers both.
struct FrameSync {
    command_pool: CommandPool,
    command_buffers: [vk::CommandBuffer; 2],
    image_available: Semaphore,
    fence: Fence,
}

struct Surface {
    loader: khr::surface::Instance,
    handle: vk::SurfaceKHR,
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: the surface belongs to this instance and is destroyed once. The device
        // is already gone, which is allowed: surfaces only depend on the instance.
        unsafe { self.loader.destroy_surface(self.handle, None) };
    }
}

/// Vulkan renderer owning the swapchain, the frame graph and its passes.
///
/// Field order is load-bearing: GPU resources are declared before the device and the device
/// before the surface/instance so that Rust drops them children-first.
pub struct Renderer {
    swapchain: Swapchain,
    transients: TransientPool,
    scene: ScenePass,
    composite: CompositePass,
    ui: UiPass,
    lighting: LightingResources,
    /// GPU particles; `None` when disabled or unsupported (the frame then has no VFX passes).
    vfx: Option<VfxPass>,
    /// Screen-space contact shadows (R2b); `None` when off.
    contact: Option<ContactPass>,
    /// Screen-space outlines (R2b); `None` unless outlines are `screen`.
    outline: Option<OutlinePass>,
    /// This frame's outline palette, for the outline pass.
    frame_outlines: OutlinePalette,
    materials: Materials,
    shaders: ShaderSource,
    pipeline_stats: PipelineStats,
    profiler: GpuProfiler,
    frames: Vec<FrameSync>,
    capture_buffer: Option<GpuBuffer>,
    /// Offscreen images (R7b), indexed by [`UiImage::index`]; fixed size.
    offscreen: Vec<OffscreenTarget>,
    /// Every frame variant, compiled once at creation: index
    /// `capture * OFFSCREEN_VARIANTS + offscreen mask` (see [`Self::plan_index`]).
    plans: Vec<FramePlan>,
    capture_pending: bool,
    capture_ready: bool,
    capture_extent: Option<vk::Extent2D>,
    capture_format: Option<vk::Format>,
    images_in_flight: Vec<Option<usize>>,
    frame_index: usize,
    /// Whether the latest frame reached `vkQueueSubmit` ([`Self::last_frame_submitted`]).
    last_submitted: bool,
    hdr_format: vk::Format,
    depth_aspect: vk::ImageAspectFlags,
    /// The lighting features in use (shadows off when the device has no shadow format).
    lighting_config: LightingConfig,
    /// The shadow map, when shadows are on.
    shadow: Option<ShadowSpec>,
    device: DeviceContext,
    surface: Surface,
    instance: InstanceContext,
    config: RenderConfig,
    // Declared last so the window outlives the surface/instance during drop.
    window: Window,
}

impl Renderer {
    /// Creates a renderer for `window`.
    ///
    /// Enables the Khronos validation layer according to `config.validation`; pass
    /// [`ValidationMode::Required`] in CI so a missing layer is a hard failure.
    #[allow(clippy::too_many_lines)] // Device, graph and pass wiring belongs together.
    pub fn new(window: &Window, mut config: RenderConfig) -> RenderResult<Self> {
        assert!(config.max_instances > 0, "max_instances must be positive");
        let started = Instant::now();
        let display_handle = window.display_handle()?.as_raw();
        let window_handle = window.window_handle()?.as_raw();
        // SAFETY: ash-window returns the static list of surface extensions for this
        // platform's display handle.
        let extensions = ash_window::enumerate_required_extensions(display_handle)?;
        let required: Vec<&CStr> = extensions
            .iter()
            .map(|pointer| {
                // SAFETY: the pointers are static NUL-terminated extension names provided
                // by ash-window.
                unsafe { CStr::from_ptr(*pointer) }
            })
            .collect();

        let instance = InstanceContext::new(&required, config.validation)?;
        let loader = khr::surface::Instance::new(&instance.entry, &instance.instance);
        // SAFETY: the instance was created with exactly these surface extensions for this
        // display/window handle pair.
        let surface_handle = unsafe {
            ash_window::create_surface(
                &instance.entry,
                &instance.instance,
                display_handle,
                window_handle,
                None,
            )
        }?;
        let surface = Surface {
            loader,
            handle: surface_handle,
        };

        let mut device = DeviceContext::new(&instance, surface_handle)?;
        // Every pipeline below is built through this cache, at creation only.
        let cache = device.init_pipeline_cache(config.pipeline_cache.as_deref())?;
        let shaders = ShaderSource::new(config.shader_dir.clone());
        let size = window.size();
        if size[0] == 0 || size[1] == 0 {
            return Err(RenderError::SwapchainOutOfDate);
        }
        let extent = vk::Extent2D {
            width: size[0],
            height: size[1],
        };
        let swapchain = Swapchain::new(
            &instance,
            &device,
            surface_handle,
            extent,
            config.vsync,
            None,
        )?;

        let hdr_format = targets::choose_hdr_format(&device, config.quality_tier);
        let (depth_format, depth_aspect) = targets::choose_depth_format(&device)?;
        let mut lighting_config = config.effective_lighting();
        let shadow_format = targets::choose_shadow_format(&device);
        if shadow_format.is_none() && lighting_config.shadows != ShadowFilter::Off {
            eprintln!(
                "[render] no depth format usable for comparison-sampled shadow maps \
                 (or no mutableComparisonSamplers); shadows off"
            );
            lighting_config = lighting_config.with_shadows(ShadowFilter::Off);
        }
        lighting_config = lighting_config.with_shadow_resolution(
            lighting_config.clamped_resolution(device.max_image_dimension_2d),
        );
        let shadow = shadow_format
            .filter(|_| lighting_config.shadows != ShadowFilter::Off)
            .map(|(format, _)| ShadowSpec {
                format,
                size: lighting_config.shadow_resolution,
            });
        if !device.depth_clamp && shadow.is_some() {
            eprintln!(
                "[render] depthClamp unsupported: casters above the shadow map's near plane \
                 are clipped (the fit's lift covers 32 m casters)"
            );
        }
        let graph_error =
            |error: crate::graph::GraphError| RenderError::RenderGraph(error.to_string());
        let mut budget = config.quality_tier.vfx_budget();
        // MoltenVK: no sampling of a depth image that is also the (read-only) depth
        // attachment of the same rendering; the particles are depth-tested only.
        if device.portability_subset {
            budget.soft_particles = false;
        }
        let vfx = if !config.vfx {
            None
        } else if !device.compute_supported {
            eprintln!("[render] VFX off: the graphics queue has no compute");
            None
        } else {
            match VfxPass::new(
                &device,
                &shaders,
                FRAMES_IN_FLIGHT,
                hdr_format,
                depth_format,
                budget,
                budget.oit,
            ) {
                Ok(pass) => Some(pass),
                Err(error) => {
                    eprintln!("[render] VFX unavailable ({error}); drawing without particles");
                    None
                }
            }
        };
        let vfx_mode = match &vfx {
            None => VfxMode::Off,
            Some(_) if budget.oit => VfxMode::Oit,
            Some(_) => VfxMode::Unsorted,
        };
        let depth_features = DepthFeatures {
            outline_ids: lighting_config.outlines == OutlineMode::Screen,
            contact: lighting_config.contact_steps() > 0,
        };
        let mut plans = Vec::with_capacity(2 * OFFSCREEN_VARIANTS);
        for capture in [false, true] {
            for offscreen in 0..OFFSCREEN_VARIANTS {
                plans.push(
                    frame_graph::build(
                        hdr_format,
                        depth_format,
                        depth_aspect,
                        shadow,
                        capture,
                        offscreen,
                        vfx_mode,
                        depth_features,
                    )
                    .map_err(graph_error)?,
                );
            }
        }
        // The variants alternate on the same physical transients, so each slot must also
        // start from the same (stage, access) in all of them: a slot's first barrier orders
        // against whichever variant ran in the previous frame.
        if plans
            .iter()
            .any(|variant| !variant.graph.shares_transients_with(&plans[0].graph))
        {
            return Err(RenderError::RenderGraph(String::from(
                "a frame variant disagrees on transient slots or their start state",
            )));
        }
        if !plans[0].graph.culled().is_empty() {
            eprintln!(
                "[render] graph culled passes: {:?}",
                plans[0].graph.culled()
            );
        }
        let transients = TransientPool::new(&device, plans[0].graph.slots(), swapchain.extent)?;
        let mut offscreen = Vec::with_capacity(UI_IMAGE_SLOTS);
        for (slot, size) in config.ui_image_sizes.into_iter().enumerate() {
            let [width, height] = size.map(|side| side.clamp(1, MAX_UI_IMAGE_SIZE));
            config.ui_image_sizes[slot] = [width, height];
            offscreen.push(OffscreenTarget::new(
                &device,
                frame_graph::OFFSCREEN_PASS_NAMES[slot],
                vk::Extent2D { width, height },
                hdr_format,
                (depth_format, depth_aspect),
            )?);
        }

        let materials = Materials::new(
            &device,
            config.compressed_textures,
            config.quality_tier.max_anisotropy(),
        )?;
        // View 0 is the main scene; view `1 + slot` renders offscreen image `slot`.
        let mut views = [ViewBudget {
            max_instances: OFFSCREEN_MAX_INSTANCES,
            max_bones: OFFSCREEN_MAX_BONES,
        }; 1 + UI_IMAGE_SLOTS];
        views[0] = ViewBudget {
            max_instances: config.max_instances,
            max_bones: config.max_bones,
        };
        let (default_shadow_format, shadow_linear) = shadow_format.unwrap_or((depth_format, false));
        let lighting = LightingResources::new(
            &device,
            FRAMES_IN_FLIGHT,
            default_shadow_format,
            shadow_linear,
        )?;
        let scene = ScenePass::new(
            &device,
            &shaders,
            FRAMES_IN_FLIGHT,
            hdr_format,
            depth_format,
            shadow.map(|spec| ShadowPipelineSpec {
                format: spec.format,
                depth_clamp: device.depth_clamp,
            }),
            materials.set_layout(),
            lighting.set_layout(),
            &views,
            config.skinning,
            depth_features.prepass().then_some(PrepassPipelineSpec {
                outline_ids: depth_features.outline_ids,
            }),
            lighting_config.outlines == OutlineMode::Hull,
        )?;
        config.max_bones = scene.max_bones();
        let contact = depth_features
            .contact
            .then(|| ContactPass::new(&device, &shaders, scene.camera_set_layout()))
            .transpose()?;
        let outline = depth_features
            .outline_ids
            .then(|| OutlinePass::new(&device, &shaders, swapchain.format))
            .transpose()?;
        let composite = CompositePass::new(&device, &shaders, swapchain.format, config.exposure)?;
        let ui = UiPass::new(
            &device,
            FRAMES_IN_FLIGHT,
            swapchain.format,
            config.max_ui_vertices,
            config.ui_scale,
            &UiImages {
                views: core::array::from_fn(|slot| offscreen[slot].color().view()),
                exposure: composite.exposure(),
            },
        )?;
        let profiler = GpuProfiler::new(
            device.device.clone(),
            FRAMES_IN_FLIGHT,
            device.timestamp_valid_bits,
            device.timestamp_period,
        )?;

        let mut frames = Vec::with_capacity(FRAMES_IN_FLIGHT);
        for _ in 0..FRAMES_IN_FLIGHT {
            let command_pool = CommandPool::new(device.device.clone(), device.graphics_family)?;
            let command_buffers = [command_pool.allocate_one()?, command_pool.allocate_one()?];
            frames.push(FrameSync {
                command_pool,
                command_buffers,
                image_available: Semaphore::new(device.device.clone())?,
                fence: Fence::new(device.device.clone(), true)?,
            });
        }
        let images_in_flight = vec![None; swapchain.image_count()];

        let (pipelines, build_time) = device.pipeline_builds();
        let pipeline_stats = PipelineStats {
            pipelines,
            build_ms: duration_ms(build_time),
            startup_ms: duration_ms(started.elapsed()),
            cache,
        };
        // Save a cold or rejected cache at once, so a later crash still leaves a warm one.
        if let Some(path) = &config.pipeline_cache
            && cache != PipelineCacheState::Warm
            && let Err(error) = device.save_pipeline_cache(path)
        {
            eprintln!(
                "[render] pipeline cache {}: not saved ({error})",
                path.display()
            );
        }

        let mut renderer = Self {
            window: window.clone(),
            swapchain,
            transients,
            scene,
            composite,
            ui,
            lighting,
            vfx,
            contact,
            outline,
            frame_outlines: OutlinePalette::NONE,
            materials,
            shaders,
            pipeline_stats,
            profiler,
            frames,
            capture_buffer: None,
            offscreen,
            plans,
            capture_pending: false,
            capture_ready: false,
            capture_extent: None,
            capture_format: None,
            images_in_flight,
            frame_index: 0,
            last_submitted: false,
            hdr_format,
            depth_aspect,
            lighting_config,
            shadow,
            device,
            surface,
            instance,
            config,
        };
        renderer.bind_transients();
        Ok(renderer)
    }

    /// The lighting features in use: the configured ones, with the shadow resolution clamped
    /// to the device and shadows off when the device has no comparison-sampled depth format.
    #[must_use]
    pub fn lighting_config(&self) -> LightingConfig {
        self.lighting_config
    }

    /// Sets the map's image-based lighting (ADR-0011): `desc`'s SH-L2 diffuse irradiance and
    /// intensity, and `specular`, its prefiltered specular cube (linear, e.g. `rgba16f` from
    /// `moba-tools env-bake`; mip `m` of `M` holds roughness `m / (M - 1)`). The cube is
    /// skipped when [`LightingConfig::specular_ibl`] is off (the `Low` tier keeps only the
    /// SH). A cube whose level count differs from `desc.specular_mips` is used with its own
    /// count and logged. Replaces the previous environment. Load-time only: it waits for the
    /// device to idle and uploads through a staging buffer.
    ///
    /// # Errors
    /// [`RenderError::InvalidEnvironment`] for a 2D or sRGB specular map or a format the
    /// device cannot sample with linear filtering; allocation or Vulkan errors from the
    /// upload. On error the previous environment stays.
    pub fn set_environment(
        &mut self,
        desc: &moba_assets::EnvironmentDesc,
        specular: Option<&crate::TextureData>,
    ) -> RenderResult<()> {
        let specular = specular.filter(|_| self.lighting_config.specular_ibl);
        if let Some(data) = specular {
            lighting::check_environment_cube(data)?;
            if !crate::texture::sampleable(&self.device, crate::texture::vk_format(data.format())) {
                return Err(RenderError::InvalidEnvironment(
                    "the device cannot sample the specular map's format",
                ));
            }
            if data.level_count() != desc.specular_mips {
                eprintln!(
                    "[render] environment: the specular cube has {} levels but the descriptor \
                     says specular_mips = {}; using the cube's",
                    data.level_count(),
                    desc.specular_mips
                );
            }
        }
        // SAFETY: waiting for the device to idle guarantees no submitted frame still uses the
        // lighting sets or the environment image being replaced.
        unsafe { self.device.device.device_wait_idle() }?;
        let image = specular
            .map(|data| crate::texture::upload(&self.device, "environment-specular", data))
            .transpose()?;
        let state = EnvironmentState {
            sh: desc.sh,
            intensity: lighting::sanitize(desc.intensity, 0.0, 16.0),
            max_lod: specular.map_or(0.0, |data| data.level_count().saturating_sub(1) as f32),
            specular: image.is_some(),
        };
        self.lighting.set_environment(&self.device, image, state);
        Ok(())
    }

    /// Removes the environment: the hemispheric ambient lights the scene again. Waits for
    /// the device to idle.
    ///
    /// # Errors
    /// Vulkan errors from the idle wait.
    pub fn clear_environment(&mut self) -> RenderResult<()> {
        // SAFETY: as in `set_environment`.
        unsafe { self.device.device.device_wait_idle() }?;
        self.lighting
            .set_environment(&self.device, None, EnvironmentState::NONE);
        Ok(())
    }

    /// Whether an environment is set (image-based lighting rather than hemispheric ambient,
    /// unless [`LightingConfig::ibl`] is off).
    #[must_use]
    pub fn has_environment(&self) -> bool {
        self.lighting.state().intensity > 0.0
    }

    /// Sets the map's baked ambient occlusion (ADR-0016): `texture`, a 2D single-channel map
    /// (`R8_UNORM` or BC4 from `moba-tools ao-bake`, 1 = open), placed over the world by
    /// `placement`. It multiplies the ambient line of every lit surface unless
    /// [`LightingConfig::baked_ao`] is off. Replaces the previous one. Load-time only: it
    /// waits for the device to idle and uploads through a staging buffer.
    ///
    /// # Errors
    /// [`RenderError::InvalidTexture`] for a cube, a format other than `R8Unorm`/`Bc4Unorm`
    /// (or one the device cannot sample) or an unusable placement; allocation or Vulkan
    /// errors from the upload. On error the previous AO stays.
    pub fn set_baked_ao(
        &mut self,
        texture: &crate::TextureData,
        placement: BakedAo,
    ) -> RenderResult<()> {
        let single_channel = matches!(
            texture.format(),
            crate::TextureFormat::R8Unorm | crate::TextureFormat::Bc4Unorm
        );
        if texture.is_cube()
            || !single_channel
            || placement.uniform().is_none()
            || !crate::texture::sampleable(
                &self.device,
                crate::texture::vk_format(texture.format()),
            )
        {
            return Err(RenderError::InvalidTexture);
        }
        // SAFETY: waiting for the device to idle guarantees no submitted frame still uses the
        // lighting sets or the AO image being replaced.
        unsafe { self.device.device.device_wait_idle() }?;
        let image = crate::texture::upload(&self.device, "baked-ao", texture)?;
        self.lighting
            .set_baked_ao(&self.device, Some((image, placement)));
        Ok(())
    }

    /// Removes the baked AO. Waits for the device to idle.
    ///
    /// # Errors
    /// Vulkan errors from the idle wait.
    pub fn clear_baked_ao(&mut self) -> RenderResult<()> {
        // SAFETY: as in `set_baked_ao`.
        unsafe { self.device.device.device_wait_idle() }?;
        self.lighting.set_baked_ao(&self.device, None);
        Ok(())
    }

    /// Whether a baked AO map is set (it is applied unless [`LightingConfig::baked_ao`] is
    /// off).
    #[must_use]
    pub fn has_baked_ao(&self) -> bool {
        self.lighting.baked_ao().is_some()
    }

    /// What happened to the most recent frame's [`Scene::lights`]: uploaded, dropped by the
    /// tier's caps, culled (ADR-0016).
    #[must_use]
    pub fn light_stats(&self) -> LightStats {
        self.lighting.light_stats()
    }

    /// Binds a registered 2D colour texture as the stylisation ramp (`None` removes it), used
    /// with [`SceneLighting::ramp`](crate::SceneLighting) > 0. Waits for the device to idle.
    ///
    /// # Errors
    /// [`RenderError::InvalidMaterial`] for an unknown id or a texture that is not a 2D
    /// RGBA colour texture; Vulkan errors from the idle wait.
    pub fn set_lighting_ramp(&mut self, texture: Option<TextureId>) -> RenderResult<()> {
        let view = texture
            .map(|texture| self.materials.ramp_view(texture))
            .transpose()?;
        // SAFETY: as in `set_environment`.
        unsafe { self.device.device.device_wait_idle() }?;
        self.lighting.set_ramp(&self.device, view);
        Ok(())
    }

    /// Human-readable name of the selected GPU.
    #[must_use]
    pub fn device_name(&self) -> &str {
        &self.device.name
    }

    /// Whether the Khronos validation layer is active.
    #[must_use]
    pub fn validation_enabled(&self) -> bool {
        self.instance.validation_enabled()
    }

    /// Number of Vulkan validation ERROR messages seen by the process.
    #[must_use]
    pub fn validation_error_count(&self) -> u64 {
        InstanceContext::validation_error_count()
    }

    /// The configured quality tier.
    #[must_use]
    pub fn quality_tier(&self) -> QualityTier {
        self.config.quality_tier
    }

    /// Name of the HDR scene target's format (`"R16G16B16A16_SFLOAT"` or
    /// `"B10G11R11_UFLOAT_PACK32"`), for logs. `Low` falls back to the 64-bit format when
    /// the device cannot render to the packed one.
    #[must_use]
    pub fn hdr_format_name(&self) -> &'static str {
        match self.hdr_format {
            vk::Format::B10G11R11_UFLOAT_PACK32 => "B10G11R11_UFLOAT_PACK32",
            _ => "R16G16B16A16_SFLOAT",
        }
    }

    /// Per-pass GPU timings of the latest frame whose timestamp queries completed (they lag
    /// the current frame by the frames in flight). `None` until the first frame completes,
    /// and always on devices whose graphics queue has no timestamp support.
    #[must_use]
    pub fn gpu_timings(&self) -> Option<GpuTimings> {
        self.profiler.latest()
    }

    /// Whether the GPU particle passes exist ([`RenderConfig::vfx`] and compute support).
    #[must_use]
    pub fn vfx_available(&self) -> bool {
        self.vfx.is_some()
    }

    /// The VFX budget in use: the configured tier's ([`QualityTier::vfx_budget`]), with soft
    /// particles off on portability-subset devices (`MoltenVK`).
    #[must_use]
    pub fn vfx_budget(&self) -> VfxBudget {
        self.vfx
            .as_ref()
            .map_or_else(|| self.config.quality_tier.vfx_budget(), VfxPass::budget)
    }

    /// Particle statistics of the latest completed frame (lagging like [`Self::gpu_timings`]):
    /// live particles, spawns, drops and the projected-pixel overdraw estimate. `None` until
    /// the first frame completes, and when VFX are unavailable.
    #[must_use]
    pub fn vfx_stats(&self) -> Option<VfxStats> {
        self.vfx.as_ref().and_then(VfxPass::latest)
    }

    /// Uses a registered texture as the particle atlas (an 8×8 grid of equal cells in the
    /// [`moba_assets::VFX_SPRITES`] layout; R is coverage, G the hot core), or restores the
    /// built-in procedural atlas with `None`. Load time only: waits for the device to idle.
    ///
    /// # Errors
    /// [`RenderError::Vfx`] when VFX are unavailable or the texture id is unknown.
    pub fn set_vfx_atlas(&mut self, texture: Option<TextureId>) -> RenderResult<()> {
        let view = match texture {
            Some(texture) => Some(
                self.materials
                    .texture_view(texture)
                    .ok_or(RenderError::Vfx("unknown atlas texture id"))?,
            ),
            None => None,
        };
        let Some(vfx) = self.vfx.as_mut() else {
            return Err(RenderError::Vfx("VFX are unavailable"));
        };
        // SAFETY: waiting for the device to idle guarantees no submitted frame still uses
        // the descriptor set being rewritten.
        unsafe { self.device.device.device_wait_idle() }?;
        vfx.set_atlas(&self.device, view);
        Ok(())
    }

    /// Binds a registered static mesh to mesh-particle `slot` (`0..VFX_MESH_SLOTS`), or
    /// unbinds it with `None` (its particles then draw as billboards). Load time only.
    ///
    /// # Errors
    /// [`RenderError::Vfx`] when VFX are unavailable, the slot is out of range or the mesh is
    /// unknown or skinned.
    pub fn set_vfx_mesh(&mut self, slot: usize, mesh: Option<MeshId>) -> RenderResult<()> {
        if slot >= VFX_MESH_SLOTS {
            return Err(RenderError::Vfx("mesh slot out of range"));
        }
        let binding = match mesh {
            Some(id) => {
                let gpu = self
                    .scene
                    .mesh(id)
                    .ok_or(RenderError::Vfx("unknown or skinned mesh"))?;
                Some((id, gpu.index_count))
            }
            None => None,
        };
        let Some(vfx) = self.vfx.as_mut() else {
            return Err(RenderError::Vfx("VFX are unavailable"));
        };
        vfx.set_mesh(slot, binding)
    }

    /// Whether this device records per-pass GPU timestamps.
    #[must_use]
    pub fn gpu_timing_supported(&self) -> bool {
        self.profiler.enabled()
    }

    /// Uploads a static mesh and returns its id for [`UnitInstance::mesh`](crate::UnitInstance).
    ///
    /// Load-time only: the upload goes through a staging buffer and a transfer → vertex-input
    /// `synchronization2` barrier, then waits for the graphics queue to idle. Do not call it
    /// per frame. Meshes live as long as the renderer; ids are never reused. Up to 64 meshes
    /// (including the built-in cube) can be registered.
    ///
    /// # Errors
    /// [`RenderError::TooManyMeshes`] once every slot is used, [`RenderError::InvalidMesh`]
    /// if the mesh fails the renderer's own index-range check, otherwise allocation or Vulkan
    /// errors from the upload.
    pub fn register_mesh(&mut self, data: &crate::MeshData) -> RenderResult<MeshId> {
        self.scene.register_mesh(&self.device, data)
    }

    /// Uploads a skinned mesh (its [`MeshSkin`](moba_assets::MeshSkin) joints and weights in
    /// the 56-byte skinned vertex layout) and returns its id. Draw it with a
    /// [`UnitInstance`](crate::UnitInstance) whose [`BoneRange`](crate::BoneRange) names one
    /// joint matrix per joint in [`Scene::bones`]; instances without a valid range are
    /// skipped and counted.
    ///
    /// Load-time only, like [`Self::register_mesh`], and shares its 64 slots.
    ///
    /// # Errors
    /// [`RenderError::SkinningUnavailable`] when skinning is disabled or its pipeline failed
    /// to build (register the static mesh instead), [`RenderError::InvalidMesh`] when the mesh
    /// has no valid skin, otherwise as [`Self::register_mesh`].
    pub fn register_skinned_mesh(&mut self, data: &crate::MeshData) -> RenderResult<MeshId> {
        self.scene.register_skinned_mesh(&self.device, data)
    }

    /// Uploads a static mesh with its vertex-animation texture (R4b, ADR-0012) and returns
    /// its id. Instances of it play [`UnitInstance::vat`](crate::UnitInstance::vat)'s frames
    /// from `texture` (`RGBA16F`, laid out as `descriptor` says, from `moba-tools vat-bake`)
    /// in the vertex shader, instanced, without a bone buffer, and cast shadows. Vertex
    /// colour only (no material). Up to [`MAX_VAT_MESHES`](crate::MAX_VAT_MESHES); shares
    /// the 64 mesh slots. Load-time only, like [`Self::register_mesh`].
    ///
    /// # Errors
    /// [`RenderError::InvalidVat`] when the VAT pipeline is unavailable, every VAT slot is
    /// used, or `texture` or `data` do not match `descriptor`; otherwise as
    /// [`Self::register_mesh`].
    pub fn register_vat_mesh(
        &mut self,
        data: &crate::MeshData,
        texture: &crate::TextureData,
        descriptor: &crate::VatDescriptor,
    ) -> RenderResult<MeshId> {
        self.scene
            .register_vat_mesh(&self.device, data, texture, descriptor)
    }

    /// Whether [`Self::register_vat_mesh`] can succeed (the VAT pipeline built).
    #[must_use]
    pub fn vat_available(&self) -> bool {
        self.scene.vat_available()
    }

    /// Registered VAT meshes.
    #[must_use]
    pub fn vat_mesh_count(&self) -> usize {
        self.scene.vat_mesh_count()
    }

    /// Uploads a texture and returns its id for [`MaterialData::with_texture`].
    ///
    /// Load-time only (staging upload and a queue wait). BC textures are decoded to their
    /// 8-bit equivalent on the CPU when the device cannot sample them or
    /// [`RenderConfig::compressed_textures`] is off. Up to [`MAX_TEXTURES`](crate::MAX_TEXTURES).
    ///
    /// # Errors
    /// [`RenderError::TooManyTextures`], [`RenderError::InvalidTexture`] when the format cannot
    /// be sampled even after decoding, otherwise allocation or Vulkan errors.
    pub fn register_texture(&mut self, data: &crate::TextureData) -> RenderResult<TextureId> {
        self.materials.register_texture(&self.device, data)
    }

    /// Creates materials. The textured ones get a descriptor set each, from one descriptor
    /// pool sized for exactly this call, and their parameter blocks one uniform buffer: call
    /// it once per content load with every material, not once per material. Load-time only.
    /// All-or-nothing: on error no material is created.
    ///
    /// # Errors
    /// [`RenderError::TooManyMaterials`] past [`MAX_MATERIALS`](crate::MAX_MATERIALS),
    /// [`RenderError::InvalidMaterial`] for an unknown texture, a texture format its slot
    /// does not allow (sRGB colour, linear data) or a textured material without a base
    /// colour, otherwise allocation or Vulkan errors.
    pub fn register_materials(
        &mut self,
        materials: &[MaterialData],
    ) -> RenderResult<Vec<MaterialId>> {
        self.materials.register_materials(&self.device, materials)
    }

    /// Draws every instance of `mesh` with `material`. A textured material switches the mesh
    /// to the `MATERIAL_TEXTURED` permutation; a vertex-colour one restores the default look.
    /// Load-time only.
    ///
    /// # Errors
    /// [`RenderError::InvalidMaterial`] for an unknown mesh or material, or a textured
    /// material on a mesh without texture coordinates.
    pub fn set_mesh_material(&mut self, mesh: MeshId, material: MaterialId) -> RenderResult<()> {
        if !self.materials.contains(material) {
            return Err(RenderError::InvalidMaterial("unknown material id"));
        }
        let set = self.materials.set(material);
        self.scene.set_mesh_material(mesh, set)
    }

    /// Texture and material counts.
    #[must_use]
    pub fn material_stats(&self) -> MaterialStats {
        self.materials.stats()
    }

    /// Whether BC textures upload without a CPU decode (device support and config).
    #[must_use]
    pub fn compressed_textures_supported(&self) -> bool {
        self.materials.compressed()
    }

    /// How the pipelines were built: count, driver time and the cache state.
    #[must_use]
    pub fn pipeline_stats(&self) -> PipelineStats {
        self.pipeline_stats
    }

    /// Dev builds: rebuilds the scene and composite pipelines (every permutation) from
    /// [`RenderConfig::shader_dir`] overrides, falling back to built-in modules per file.
    /// Waits for the device to idle first; a stall, so call it on a file change, not per
    /// frame. The UI pipeline is not reloaded. Returns `Ok(false)` when no shader directory
    /// is configured.
    ///
    /// # Errors
    /// A pipeline that fails to build (or a skinned permutation that no longer builds); then
    /// every pass keeps its previous pipelines.
    pub fn reload_shaders(&mut self) -> RenderResult<bool> {
        if !self.shaders.has_overrides() {
            return Ok(false);
        }
        // SAFETY: waiting for the device to idle guarantees no submitted frame still uses
        // the pipelines being replaced.
        unsafe { self.device.device.device_wait_idle() }?;
        // Build everything first and swap only when every pass succeeded, so a failed reload
        // never mixes new scene pipelines with an old composite (or drops one).
        let scene = self.scene.build_pipelines(&self.device, &self.shaders)?;
        let composite = self.composite.build_pipeline(&self.device, &self.shaders)?;
        let vfx = self
            .vfx
            .as_ref()
            .map(|vfx| vfx.build_pipelines(&self.device, &self.shaders))
            .transpose()?;
        let contact = self
            .contact
            .as_ref()
            .map(|pass| pass.build_pipeline(&self.device, &self.shaders))
            .transpose()?;
        let outline = self
            .outline
            .as_ref()
            .map(|pass| pass.build_pipeline(&self.device, &self.shaders))
            .transpose()?;
        self.scene.install_pipelines(scene);
        self.composite.install_pipeline(composite);
        if let (Some(pass), Some(pipeline)) = (self.contact.as_mut(), contact) {
            pass.install_pipeline(pipeline);
        }
        if let (Some(pass), Some(pipeline)) = (self.outline.as_mut(), outline) {
            pass.install_pipeline(pipeline);
        }
        if let (Some(pass), Some(pipelines)) = (self.vfx.as_mut(), vfx) {
            pass.install_pipelines(pipelines);
        }
        Ok(true)
    }

    /// Whether [`Self::register_skinned_mesh`] can succeed (the skinned pipeline exists).
    #[must_use]
    pub fn skinning_available(&self) -> bool {
        self.scene.skinning_available()
    }

    /// Mesh slots in total, including the built-in cube: [`Self::register_mesh`],
    /// [`Self::register_skinned_mesh`] and [`Self::register_vat_mesh`] fail with
    /// [`RenderError::TooManyMeshes`] once [`Self::mesh_count`] reaches it.
    #[must_use]
    pub fn mesh_capacity(&self) -> usize {
        crate::passes::scene::MAX_MESHES
    }

    /// Number of registered meshes, including the built-in cube.
    #[must_use]
    pub fn mesh_count(&self) -> usize {
        self.scene.mesh_count()
    }

    /// Current swapchain extent in physical pixels.
    #[must_use]
    pub fn extent(&self) -> [u32; 2] {
        [self.swapchain.extent.width, self.swapchain.extent.height]
    }

    /// Size in pixels of offscreen `image` ([`RenderConfig::ui_image_sizes`], after
    /// clamping).
    #[must_use]
    pub fn ui_image_extent(&self, image: UiImage) -> [u32; 2] {
        let extent = self.offscreen[image.index()].extent();
        [extent.width, extent.height]
    }

    /// Draws one frame.
    ///
    /// Returns `Ok(())` when the frame was presented or when the window is minimised. A
    /// stale swapchain is recreated internally and the frame is skipped.
    pub fn render(&mut self, scene: &Scene<'_>) -> RenderResult<()> {
        self.render_internal(scene, None, &[]).map(drop)
    }

    /// Draws a frame with a screen-space UI overlay.
    ///
    /// # Errors
    /// Returns Vulkan errors from resource creation, presentation or the frame submission.
    pub fn render_with_ui(&mut self, scene: &Scene<'_>, ui: &UiDrawList) -> RenderResult<()> {
        self.render_internal(scene, Some(ui), &[]).map(drop)
    }

    /// Draws a frame that first re-renders the offscreen images in `offscreen` (R7b), each
    /// through its own render-graph pass, then the scene and the UI, which can sample every
    /// offscreen image ([`UiDrawList::image`]). Images not named keep their last contents.
    /// At most one render per image is used (the first); `offscreen` may be empty.
    ///
    /// Offscreen renders cost GPU time in the frame that carries them, so re-render an
    /// image only when it changes (a map load for the minimap, a few times a second for
    /// animated portraits).
    ///
    /// Returns `Ok(true)` when the frame was submitted, so the offscreen images now hold the
    /// new renders, and `Ok(false)` when it was skipped (a minimised window or a stale
    /// swapchain, which is recreated): then nothing was rendered and the caller should
    /// re-send the renders with a later frame.
    ///
    /// # Errors
    /// As [`Self::render_with_ui`]. An error does not mean nothing ran: presentation, or the
    /// swapchain recreation after it, can fail after the frame was submitted, and then the
    /// offscreen renders have landed. [`Self::last_frame_submitted`] tells the two apart.
    pub fn render_with_offscreen(
        &mut self,
        scene: &Scene<'_>,
        ui: &UiDrawList,
        offscreen: &[OffscreenRender<'_>],
    ) -> RenderResult<bool> {
        self.render_internal(scene, Some(ui), offscreen)
    }

    /// Whether the most recent `render*`/`capture_frame*` call submitted its frame (and so its
    /// offscreen renders), including when it then returned an error from presentation or
    /// swapchain recreation. `false` before the first frame and after a skipped one.
    #[must_use]
    pub fn last_frame_submitted(&self) -> bool {
        self.last_submitted
    }

    /// The plan variant for a capture flag and an offscreen mask.
    const fn plan_index(capture: bool, offscreen: usize) -> usize {
        (capture as usize) * OFFSCREEN_VARIANTS + offscreen
    }

    /// Points passes that sample transients at the current pool images.
    fn bind_transients(&mut self) {
        let plan = &self.plans[0];
        let hdr = plan
            .graph
            .slot_of(plan.resources.hdr)
            .expect("the composite keeps the HDR target live");
        self.composite
            .bind_hdr(&self.device, self.transients.image(hdr).view());
        let transient_view = |resource: Option<crate::graph::ResourceId>| {
            resource
                .and_then(|resource| plan.graph.slot_of(resource))
                .map(|slot| self.transients.image(slot).view())
        };
        let shadow = transient_view(plan.resources.shadow);
        let contact = transient_view(plan.resources.contact);
        let ids = transient_view(plan.resources.outline_ids);
        let depth = transient_view(Some(plan.resources.depth));
        self.lighting.bind_transients(&self.device, shadow, contact);
        if let (Some(pass), Some(depth)) = (&self.contact, depth) {
            pass.bind_depth(&self.device, depth);
        }
        if let (Some(pass), Some(ids)) = (&self.outline, ids) {
            pass.bind_ids(&self.device, ids);
        }
        if let (Some(vfx), Some(resources)) = (self.vfx.as_mut(), plan.resources.vfx) {
            let transients = &self.transients;
            let view = |resource: Option<crate::graph::ResourceId>| {
                resource
                    .and_then(|resource| plan.graph.slot_of(resource))
                    .map(|slot| transients.image(slot).view())
            };
            let depth = view(Some(plan.resources.depth)).expect("depth is live");
            vfx.bind_targets(
                &self.device,
                depth,
                view(resources.accum),
                view(resources.reveal),
            );
        }
    }

    /// The main view's lighting uniform values: the shadow map fitted to the camera (when
    /// shadows are on), the scene's art direction and the environment.
    fn frame_lighting(&self, scene: &Scene<'_>) -> FrameLighting {
        let fit = self.shadow.and_then(|spec| {
            lighting::fit_shadow(
                scene.camera.view_proj,
                scene.camera.light_direction,
                spec.size,
            )
        });
        FrameLighting::new(
            &scene.lighting,
            self.lighting_config,
            self.lighting_config.shadow_resolution,
            fit,
            self.lighting.state(),
        )
    }

    fn transient(&self, resource: crate::graph::ResourceId) -> &GpuImage {
        let slot = self.plans[0]
            .graph
            .slot_of(resource)
            .expect("frame transients are live in both variants");
        self.transients.image(slot)
    }

    /// Recreates the swapchain if the window's chosen extent changed. Returns `false` when
    /// the frame must be skipped (minimised window or a racing resize).
    fn sync_extent(&mut self) -> RenderResult<bool> {
        let [width, height] = self.window.size();
        if width == 0 || height == 0 {
            return Ok(false);
        }
        if width == self.swapchain.extent.width && height == self.swapchain.extent.height {
            return Ok(true);
        }
        // A fixed surface extent (common on Wayland/HiDPI) can legitimately differ from the
        // logical window size forever; only recreate when the chosen extent changes.
        // SAFETY: the surface and physical device belong to this instance.
        let capabilities = unsafe {
            self.device
                .surface
                .get_physical_device_surface_capabilities(self.device.physical, self.surface.handle)
        }?;
        let desired = crate::swapchain::choose_extent(capabilities, vk::Extent2D { width, height });
        if desired == self.swapchain.extent {
            return Ok(true);
        }
        match self.recreate_swapchain() {
            Ok(()) => Ok(true),
            // A 0x0/minimised surface or a racing resize is a skipped frame, not fatal.
            Err(RenderError::SwapchainOutOfDate | RenderError::SwapchainSuboptimal) => Ok(false),
            Err(error) => Err(error),
        }
    }

    #[allow(clippy::too_many_lines)] // Fence, acquire, prepare, record, submit, present.
    fn render_internal(
        &mut self,
        scene: &Scene<'_>,
        ui: Option<&UiDrawList>,
        offscreen: &[OffscreenRender<'_>],
    ) -> RenderResult<bool> {
        self.last_submitted = false;
        if !self.sync_extent()? {
            return Ok(false);
        }
        if self.capture_pending {
            if !self.swapchain.transfer_src {
                return Err(RenderError::NoSuitableDevice(String::from(
                    "swapchain does not support TRANSFER_SRC for capture",
                )));
            }
            self.ensure_capture_buffer()?;
            self.capture_extent = Some(self.swapchain.extent);
            self.capture_format = Some(self.swapchain.format);
        }

        let frame_index = self.frame_index;
        let fence = self.frames[frame_index].fence.handle();
        // SAFETY: the fence belongs to this device and is waited on with an effectively
        // infinite timeout. It is signalled or will be signalled by the frame's submit.
        unsafe { self.device.device.wait_for_fences(&[fence], true, u64::MAX) }?;
        self.profiler.collect(frame_index);
        if let Some(vfx) = self.vfx.as_mut() {
            vfx.collect(frame_index);
        }

        let image_available = self.frames[frame_index].image_available.handle();
        let (image_index, suboptimal) = match self.swapchain.acquire(image_available) {
            Ok(result) => result,
            Err(RenderError::SwapchainOutOfDate) => {
                self.recreate_swapchain()?;
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        if let Some(previous) = self.images_in_flight[image_index as usize] {
            let previous_fence = self.frames[previous].fence.handle();
            // SAFETY: the fence belongs to this device and protects the image we are about
            // to use, so waiting is mandatory.
            unsafe {
                self.device
                    .device
                    .wait_for_fences(&[previous_fence], true, u64::MAX)
            }?;
        }
        self.images_in_flight[image_index as usize] = Some(frame_index);

        // Lights are binned into this frame slot's buffers (its fence was waited on above).
        let grid = self.lighting.upload_lights(
            frame_index,
            scene.lights,
            scene.camera.view_proj,
            self.lighting_config,
        );
        let mut frame_lighting = self
            .frame_lighting(scene)
            .with_lights(grid)
            .with_baked_ao(self.lighting.baked_ao(), self.lighting_config);
        if self.contact.is_some() {
            frame_lighting = frame_lighting.with_contact_shadows(self.lighting_config);
        }
        self.frame_outlines = scene.outlines;
        self.scene.prepare(
            0,
            frame_index,
            scene,
            &frame_lighting,
            self.swapchain.extent,
        )?;
        let mut offscreen_mask = 0usize;
        for render in offscreen {
            let slot = render.image.index();
            if offscreen_mask & (1 << slot) != 0 {
                continue; // One render per image and frame: the first wins.
            }
            offscreen_mask |= 1 << slot;
            // Offscreen views never sample the shadow map (it is the main view's, and their
            // pass runs before the shadow pass): shadows off, the environment kept.
            // No dynamic lights or contact shadows; the map's baked AO only where the map is
            // drawn (the minimap), not in the portrait studio.
            let view_lighting = FrameLighting::offscreen(
                &render.scene.lighting,
                self.lighting_config,
                self.lighting.state(),
            )
            .with_baked_ao(
                self.lighting.baked_ao().filter(|_| render.terrain),
                self.lighting_config,
            );
            let extent = self.offscreen[slot].extent();
            self.scene
                .prepare(1 + slot, frame_index, &render.scene, &view_lighting, extent)?;
        }
        self.ui.prepare(frame_index, ui)?;
        // Last of the uploads: its slot allocations are committed only when the frame is
        // submitted (`VfxPass::submitted`), and rolled back if anything below fails.
        if let Some(vfx) = self.vfx.as_mut() {
            vfx.prepare(frame_index, scene, self.swapchain.extent);
        }

        let command_buffers = self.frames[frame_index].command_buffers;
        let pool = self.frames[frame_index].command_pool.handle();
        // SAFETY: the pool belongs to this device and no command buffer from it is pending
        // (the frame fence was just waited on).
        unsafe {
            self.device
                .device
                .reset_command_pool(pool, vk::CommandPoolResetFlags::empty())
        }?;
        let begin = vk::CommandBufferBeginInfo::default();
        for &command_buffer in &command_buffers {
            // SAFETY: the command buffer is allocated from the reset pool and not recording.
            unsafe {
                self.device
                    .device
                    .begin_command_buffer(command_buffer, &begin)
            }?;
        }
        self.record_frame(
            command_buffers,
            frame_index,
            image_index,
            (offscreen_mask, offscreen),
        )?;
        for &command_buffer in &command_buffers {
            // SAFETY: the command buffer is recording and is ended exactly once.
            unsafe { self.device.device.end_command_buffer(command_buffer) }?;
        }

        let render_finished = self.swapchain.render_finished[image_index as usize].handle();
        let wait_semaphores = [image_available];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let signal_semaphores = [render_finished];
        // Batch 1: passes before the first swapchain access, with no wait. Batch 2: the rest,
        // waiting for the acquired image at the stage its first barrier chains from. Graph
        // barriers order the batches (submission order on one queue).
        let submits = [
            vk::SubmitInfo::default().command_buffers(&command_buffers[..1]),
            vk::SubmitInfo::default()
                .wait_semaphores(&wait_semaphores)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&command_buffers[1..])
                .signal_semaphores(&signal_semaphores),
        ];
        // Reset as late as possible: every fallible recording step is complete, so a
        // failure before this point cannot leave an unsignalled fence for the next wait.
        // SAFETY: the fence was waited on above and is not in use by a submit.
        unsafe { self.device.device.reset_fences(&[fence]) }?;
        // SAFETY: both command buffers are executable and the semaphores are live. One
        // submission command, so the fence signal covers both batches.
        unsafe {
            self.device
                .device
                .queue_submit(self.device.graphics_queue, &submits, fence)
        }?;
        self.profiler.submitted(frame_index);
        self.last_submitted = true;
        if let Some(vfx) = self.vfx.as_mut() {
            vfx.submitted(frame_index);
        }
        if self.capture_pending {
            self.capture_pending = false;
            self.capture_ready = true;
        }

        let present_result =
            self.swapchain
                .present(self.device.present_queue, image_index, render_finished);
        self.frame_index = (frame_index + 1) % FRAMES_IN_FLIGHT;

        // The frame was submitted: its offscreen renders ran even if presenting did not.
        match present_result {
            Ok(true) if !suboptimal => Ok(true),
            Ok(_) | Err(RenderError::SwapchainSuboptimal | RenderError::SwapchainOutOfDate) => {
                self.recreate_swapchain()?;
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }

    /// Records the compiled frame graph: per pass, its barriers, a timestamp pair around the
    /// pass's commands, then the final transitions. Passes before the plan's acquire split go
    /// into `command_buffers[0]`, the rest (and the final transitions) into `[1]`.
    #[allow(clippy::too_many_lines)] // One dispatch over the compiled passes.
    fn record_frame(
        &mut self,
        command_buffers: [vk::CommandBuffer; 2],
        frame_index: usize,
        image_index: u32,
        (offscreen_mask, offscreen): (usize, &[OffscreenRender<'_>]),
    ) -> RenderResult<()> {
        let capturing = self.capture_pending;
        let extent = self.swapchain.extent;
        let swapchain_image = self.swapchain.images[image_index as usize];
        let swapchain_view = self.swapchain.views[image_index as usize];
        let capture_buffer = if capturing {
            let capture = self.capture_buffer.as_ref().ok_or_else(|| {
                RenderError::NoSuitableDevice(String::from(
                    "capture buffer missing while capture is pending",
                ))
            })?;
            let required = u64::from(extent.width) * u64::from(extent.height) * 4;
            if capture.size() < required {
                return Err(RenderError::NoSuitableDevice(format!(
                    "capture buffer too small: {} < {required}",
                    capture.size()
                )));
            }
            Some(capture.handle())
        } else {
            None
        };
        let plan = &self.plans[Self::plan_index(capturing, offscreen_mask)];
        let resources = plan.resources;
        let hdr_view = self.transient(resources.hdr).view();
        let depth_view = self.transient(resources.depth).view();
        let shadow_view = resources.shadow.map(|shadow| self.transient(shadow).view());
        let contact_view = resources
            .contact
            .map(|contact| self.transient(contact).view());
        let ids_view = resources.outline_ids.map(|ids| self.transient(ids).view());
        let hull_outlines = self.lighting_config.outlines == OutlineMode::Hull;
        let bindings = self.bindings(plan, frame_index, swapchain_image, capture_buffer);
        // Query resets run first in submission order, ahead of every timestamp write.
        self.profiler.begin_frame(command_buffers[0], frame_index);
        for (index, pass) in plan.graph.passes().iter().enumerate() {
            let command_buffer = command_buffers[usize::from(index >= plan.acquire_split)];
            frame_graph::record_barriers(&self.device, command_buffer, &pass.barriers, &bindings);
            self.profiler
                .begin_pass(command_buffer, frame_index, pass.name);
            match pass.tag {
                PassKind::Shadow => {
                    if let (Some(depth), Some(spec)) = (shadow_view, self.shadow) {
                        self.scene.record_shadow(
                            &self.device,
                            command_buffer,
                            frame_index,
                            &ShadowTarget {
                                depth,
                                size: spec.size,
                            },
                        );
                    }
                }
                PassKind::Scene => self.scene.record(
                    &self.device,
                    command_buffer,
                    0,
                    frame_index,
                    &SceneTargets {
                        color: hdr_view,
                        depth: depth_view,
                        extent,
                        clear_color: self.config.clear_color,
                        terrain: true,
                        clear_rects: &[],
                        lighting: self.lighting.set(frame_index),
                        store_depth: resources.vfx.is_some(),
                        load_depth: resources.prepass,
                        hull_outlines,
                    },
                ),
                PassKind::DepthPrepass => self.scene.record_prepass(
                    &self.device,
                    command_buffer,
                    frame_index,
                    &PrepassTarget {
                        depth: depth_view,
                        outline_ids: ids_view,
                        extent,
                    },
                ),
                PassKind::ContactShadows => {
                    if let (Some(pass), Some(target)) = (&self.contact, contact_view) {
                        pass.record(
                            &self.device,
                            command_buffer,
                            self.scene.camera_set(frame_index),
                            target,
                            extent,
                        );
                    }
                }
                PassKind::Outline => {
                    if let Some(pass) = &self.outline {
                        pass.record(
                            &self.device,
                            command_buffer,
                            swapchain_view,
                            extent,
                            &self.frame_outlines,
                        );
                    }
                }
                PassKind::VfxSimulate => {
                    if let Some(vfx) = &self.vfx {
                        vfx.record_simulate(&self.device, command_buffer, frame_index);
                    }
                }
                PassKind::VfxDraw => {
                    if let (Some(vfx), Some(ids)) = (&self.vfx, resources.vfx) {
                        let view = |resource: Option<crate::graph::ResourceId>| {
                            resource.map(|resource| self.transient(resource).view())
                        };
                        vfx.record_draw(
                            &self.device,
                            command_buffer,
                            frame_index,
                            &VfxTargets {
                                hdr: hdr_view,
                                depth: depth_view,
                                accum: view(ids.accum),
                                reveal: view(ids.reveal),
                                extent,
                            },
                            &self.scene,
                        );
                    }
                }
                PassKind::VfxResolve => {
                    if let Some(vfx) = &self.vfx {
                        vfx.record_resolve(&self.device, command_buffer, hdr_view, extent);
                    }
                }
                PassKind::Offscreen(slot) => {
                    let slot = usize::from(slot);
                    // The plan only has this pass when a render named the image.
                    if let Some(render) =
                        offscreen.iter().find(|render| render.image.index() == slot)
                    {
                        let target = &self.offscreen[slot];
                        self.scene.record(
                            &self.device,
                            command_buffer,
                            1 + slot,
                            frame_index,
                            &SceneTargets {
                                color: target.color().view(),
                                depth: target.depth().view(),
                                extent: target.extent(),
                                clear_color: render.clear_color,
                                terrain: render.terrain,
                                clear_rects: render.backdrops,
                                lighting: self.lighting.offscreen_set(frame_index),
                                store_depth: false,
                                load_depth: false,
                                hull_outlines: false,
                            },
                        );
                    }
                }
                PassKind::Composite => {
                    self.composite
                        .record(&self.device, command_buffer, swapchain_view, extent);
                }
                PassKind::Ui => {
                    self.ui.record(
                        &self.device,
                        command_buffer,
                        frame_index,
                        swapchain_view,
                        extent,
                    );
                }
                PassKind::Capture => {
                    if let Some(buffer) = capture_buffer {
                        crate::passes::capture::record(
                            &self.device,
                            command_buffer,
                            swapchain_image,
                            extent,
                            buffer,
                        );
                    }
                }
            }
            self.profiler.end_pass(command_buffer, frame_index);
        }
        frame_graph::record_barriers(
            &self.device,
            command_buffers[1],
            plan.graph.final_barriers(),
            &bindings,
        );
        Ok(())
    }

    /// This frame's handle for every resource of `plan`.
    fn bindings(
        &self,
        plan: &FramePlan,
        frame_index: usize,
        swapchain_image: vk::Image,
        capture_buffer: Option<vk::Buffer>,
    ) -> Bindings {
        let resources = plan.resources;
        let image = |resource| {
            let slot = plan
                .graph
                .slot_of(resource)
                .expect("frame transients are live in both variants");
            Binding::Image {
                image: self.transients.image(slot).image(),
                aspect: plan.graph.slots()[slot].desc.aspect,
            }
        };
        let mut bindings = Bindings::new();
        bindings.set(
            resources.swapchain,
            Binding::Image {
                image: swapchain_image,
                aspect: vk::ImageAspectFlags::COLOR,
            },
        );
        bindings.set(resources.hdr, image(resources.hdr));
        bindings.set(resources.depth, image(resources.depth));
        for resource in [resources.shadow, resources.contact, resources.outline_ids]
            .into_iter()
            .flatten()
        {
            bindings.set(resource, image(resource));
        }
        bindings.set(
            resources.bones,
            Binding::Buffer(self.scene.bone_buffer(0, frame_index)),
        );
        for (slot, offscreen) in resources.offscreen.iter().enumerate() {
            let target = &self.offscreen[slot];
            bindings.set(
                offscreen.color,
                Binding::Image {
                    image: target.color().image(),
                    aspect: vk::ImageAspectFlags::COLOR,
                },
            );
            bindings.set(
                offscreen.depth,
                Binding::Image {
                    image: target.depth().image(),
                    aspect: self.depth_aspect,
                },
            );
            bindings.set(
                offscreen.bones,
                Binding::Buffer(self.scene.bone_buffer(1 + slot, frame_index)),
            );
        }
        if let (Some(resource), Some(buffer)) = (resources.capture, capture_buffer) {
            bindings.set(resource, Binding::Buffer(buffer));
        }
        if let (Some(ids), Some(vfx)) = (resources.vfx, &self.vfx) {
            let [particles, alive, args] = vfx.buffers();
            let [spawns, stats] = vfx.frame_buffers(frame_index);
            for (resource, buffer) in [
                (ids.particles, particles),
                (ids.alive, alive),
                (ids.args, args),
                (ids.spawns, spawns),
                (ids.stats, stats),
            ] {
                bindings.set(resource, Binding::Buffer(buffer));
            }
            for resource in [ids.accum, ids.reveal].into_iter().flatten() {
                bindings.set(resource, image(resource));
            }
        }
        debug_assert!(plan.graph.resource_count() <= bindings.0.len());
        bindings
    }

    fn recreate_swapchain(&mut self) -> RenderResult<()> {
        let [width, height] = self.window.size();
        if width == 0 || height == 0 {
            return Ok(());
        }
        // SAFETY: waiting for the device to idle guarantees no frame is still using the old
        // swapchain or transient images before they are replaced.
        unsafe { self.device.device.device_wait_idle() }?;
        let extent = vk::Extent2D { width, height };
        let swapchain = Swapchain::new(
            &self.instance,
            &self.device,
            self.surface.handle,
            extent,
            self.config.vsync,
            Some(self.swapchain.handle()),
        )?;
        // Build the new swapchain-sized targets before replacing anything, so a failed
        // allocation cannot pair the new extent with old-size transients. Fixed-size slots
        // (the shadow map) are kept.
        self.transients
            .resize(&self.device, self.plans[0].graph.slots(), swapchain.extent)?;
        self.swapchain = swapchain;
        self.bind_transients();
        self.images_in_flight = vec![None; self.swapchain.image_count()];
        self.frame_index = 0;
        Ok(())
    }
}

/// A captured frame, in tightly packed RGBA8.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CapturedFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// RGBA bytes, row-major, no padding.
    pub rgba: Vec<u8>,
}

impl Renderer {
    /// Renders `scene` and reads the colour buffer back to the CPU.
    ///
    /// Debug/test path: it waits for the device to go idle. Used for golden images and by
    /// the `MOBA_CAPTURE` client mode.
    pub fn capture_frame(&mut self, scene: &Scene<'_>) -> RenderResult<CapturedFrame> {
        self.capture_frame_internal(scene, None, &[])
    }

    /// Renders with a UI overlay and reads the frame back.
    ///
    /// # Errors
    /// Returns Vulkan errors from rendering or the readback.
    pub fn capture_frame_with_ui(
        &mut self,
        scene: &Scene<'_>,
        ui: &UiDrawList,
    ) -> RenderResult<CapturedFrame> {
        self.capture_frame_internal(scene, Some(ui), &[])
    }

    /// [`Self::render_with_offscreen`], then reads the frame back.
    ///
    /// # Errors
    /// Returns Vulkan errors from rendering or the readback.
    pub fn capture_frame_with_offscreen(
        &mut self,
        scene: &Scene<'_>,
        ui: &UiDrawList,
        offscreen: &[OffscreenRender<'_>],
    ) -> RenderResult<CapturedFrame> {
        self.capture_frame_internal(scene, Some(ui), offscreen)
    }

    fn capture_frame_internal(
        &mut self,
        scene: &Scene<'_>,
        ui: Option<&UiDrawList>,
        offscreen: &[OffscreenRender<'_>],
    ) -> RenderResult<CapturedFrame> {
        // The capture plan imports the readback buffer as idle: every capture waits for the
        // device below before reading it, so no earlier submission still touches it.
        self.capture_ready = false;
        self.capture_pending = true;
        if let Err(error) = self.render_internal(scene, ui, offscreen) {
            // A capture copy may already be submitted (for example when present fails after
            // the submit); keep the "idle" import true for the next capture.
            self.capture_pending = false;
            // SAFETY: waiting for the device to idle has no preconditions beyond a live
            // device; its own error is secondary to the one being returned.
            let _ = unsafe { self.device.device.device_wait_idle() };
            return Err(error);
        }
        if !self.capture_ready {
            self.capture_pending = false;
            return Err(RenderError::NoSuitableDevice(String::from(
                "capture frame was skipped (swapchain out of date)",
            )));
        }
        // SAFETY: waiting for idle guarantees the copy-to-buffer finished before mapping.
        unsafe { self.device.device.device_wait_idle() }?;
        let extent = self.capture_extent.take().unwrap_or(self.swapchain.extent);
        let format = self.capture_format.take().unwrap_or(self.swapchain.format);
        let count = usize::try_from(u64::from(extent.width) * u64::from(extent.height) * 4)
            .expect("frame size fits usize");
        let buffer = self
            .capture_buffer
            .as_mut()
            .expect("capture buffer was ensured above");
        let mut rgba = buffer
            .mapped_slice_mut::<u8>(count)
            .ok_or_else(|| {
                RenderError::NoSuitableDevice(String::from("capture buffer is not host visible"))
            })?
            .to_vec();
        // Expose conventional tightly packed RGBA to callers.
        match format {
            vk::Format::B8G8R8A8_SRGB | vk::Format::B8G8R8A8_UNORM => {
                for pixel in rgba.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
            }
            vk::Format::R8G8B8A8_SRGB | vk::Format::R8G8B8A8_UNORM => {}
            format => {
                return Err(RenderError::NoSuitableDevice(format!(
                    "unsupported capture format {format:?}"
                )));
            }
        }
        if InstanceContext::validation_error_count() > 0 {
            return Err(RenderError::NoSuitableDevice(format!(
                "Vulkan validation reported {} errors",
                InstanceContext::validation_error_count()
            )));
        }
        self.capture_ready = false;
        Ok(CapturedFrame {
            width: extent.width,
            height: extent.height,
            rgba,
        })
    }

    fn ensure_capture_buffer(&mut self) -> RenderResult<()> {
        let extent = self.swapchain.extent;
        let size = u64::from(extent.width) * u64::from(extent.height) * 4;
        if self
            .capture_buffer
            .as_ref()
            .is_some_and(|buffer| buffer.size() == size)
        {
            return Ok(());
        }
        self.capture_buffer = Some(GpuBuffer::new(
            self.device.device.clone(),
            self.device.allocator(),
            "frame-capture",
            size,
            vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuToCpu,
        )?);
        Ok(())
    }
}

impl core::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Renderer")
            .field("device", &self.device.name)
            .field("extent", &self.extent())
            .field("validation", &self.validation_enabled())
            .field("quality_tier", &self.config.quality_tier)
            .field("lighting", &self.lighting_config)
            .finish_non_exhaustive()
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: no other thread submits work; waiting here makes the subsequent field
        // drops safe.
        let _ = unsafe { self.device.device.device_wait_idle() };
        if let Some(path) = &self.config.pipeline_cache
            && let Err(error) = self.device.save_pipeline_cache(path)
        {
            eprintln!(
                "[render] pipeline cache {}: not saved ({error})",
                path.display()
            );
        }
    }
}

fn duration_ms(duration: Duration) -> f32 {
    duration.as_secs_f32() * 1_000.0
}
