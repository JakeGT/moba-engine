//! The renderer: device and swapchain lifecycle, frame synchronisation and the frame graph.
//!
//! A frame is a precompiled render graph (ADR-0008, [`crate::frame_graph`]): the scene pass
//! renders into an HDR target, the composite pass tonemaps it into the swapchain image and
//! the UI pass draws the HUD on top. The pass code lives in [`crate::passes`]; this module
//! owns acquisition, submission, presentation, resize and the capture readback.

use core::ffi::CStr;

use ash::{khr, vk};
use gpu_allocator::MemoryLocation;
use moba_platform::Window;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::frame_graph::{self, Binding, Bindings, FramePlan, PassKind};
use crate::instance::{InstanceContext, ValidationMode};
use crate::passes::composite::CompositePass;
use crate::passes::overlay::UiPass;
use crate::passes::scene::{ScenePass, SceneTargets};
use crate::profiler::{GpuProfiler, GpuTimings};
use crate::quality::QualityTier;
use crate::resources::{CommandPool, Fence, GpuBuffer, GpuImage, Semaphore};
use crate::scene::{MeshId, Scene};
use crate::swapchain::Swapchain;
use crate::targets::{self, TransientPool};
use crate::ui::UiDrawList;

const FRAMES_IN_FLIGHT: usize = 2;
/// Default per-frame bone budget: 16 skinned instances × 64 joints (48 KiB per frame).
pub const DEFAULT_MAX_BONES: usize = 16 * 64;

/// Renderer configuration.
#[derive(Clone, Debug)]
#[non_exhaustive]
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
}

/// Per-frame-in-flight command recording and synchronisation.
struct FrameSync {
    command_pool: CommandPool,
    command_buffer: vk::CommandBuffer,
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
    profiler: GpuProfiler,
    frames: Vec<FrameSync>,
    capture_buffer: Option<GpuBuffer>,
    /// The normal frame and the capture variant, compiled once at creation.
    plan: FramePlan,
    capture_plan: FramePlan,
    capture_pending: bool,
    capture_ready: bool,
    capture_extent: Option<vk::Extent2D>,
    capture_format: Option<vk::Format>,
    images_in_flight: Vec<Option<usize>>,
    frame_index: usize,
    hdr_format: vk::Format,
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

        let device = DeviceContext::new(&instance, surface_handle)?;
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
        let graph_error =
            |error: crate::graph::GraphError| RenderError::RenderGraph(error.to_string());
        let plan = frame_graph::build(hdr_format, depth_format, depth_aspect, false)
            .map_err(graph_error)?;
        let capture_plan = frame_graph::build(hdr_format, depth_format, depth_aspect, true)
            .map_err(graph_error)?;
        if plan.graph.slots() != capture_plan.graph.slots() {
            return Err(RenderError::RenderGraph(String::from(
                "the capture variant disagrees on transient slots",
            )));
        }
        if !plan.graph.culled().is_empty() {
            eprintln!("[render] graph culled passes: {:?}", plan.graph.culled());
        }
        let transients = TransientPool::new(&device, plan.graph.slots(), swapchain.extent)?;

        let scene = ScenePass::new(
            &device,
            FRAMES_IN_FLIGHT,
            hdr_format,
            depth_format,
            config.max_instances,
            config.max_bones,
            config.skinning,
        )?;
        config.max_bones = scene.max_bones();
        let composite = CompositePass::new(&device, swapchain.format, config.exposure)?;
        let ui = UiPass::new(
            &device,
            FRAMES_IN_FLIGHT,
            swapchain.format,
            config.max_ui_vertices,
            config.ui_scale,
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
            let command_buffer = command_pool.allocate_one()?;
            frames.push(FrameSync {
                command_pool,
                command_buffer,
                image_available: Semaphore::new(device.device.clone())?,
                fence: Fence::new(device.device.clone(), true)?,
            });
        }
        let images_in_flight = vec![None; swapchain.image_count()];

        let renderer = Self {
            window: window.clone(),
            swapchain,
            transients,
            scene,
            composite,
            ui,
            profiler,
            frames,
            capture_buffer: None,
            plan,
            capture_plan,
            capture_pending: false,
            capture_ready: false,
            capture_extent: None,
            capture_format: None,
            images_in_flight,
            frame_index: 0,
            hdr_format,
            device,
            surface,
            instance,
            config,
        };
        renderer.bind_transients();
        Ok(renderer)
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
    /// the 32-byte skinned vertex layout) and returns its id. Draw it with a
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

    /// Whether [`Self::register_skinned_mesh`] can succeed (the skinned pipeline exists).
    #[must_use]
    pub fn skinning_available(&self) -> bool {
        self.scene.skinning_available()
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

    /// Draws one frame.
    ///
    /// Returns `Ok(())` when the frame was presented or when the window is minimised. A
    /// stale swapchain is recreated internally and the frame is skipped.
    pub fn render(&mut self, scene: &Scene<'_>) -> RenderResult<()> {
        self.render_internal(scene, None)
    }

    /// Draws a frame with a screen-space UI overlay.
    ///
    /// # Errors
    /// Returns Vulkan errors from resource creation, presentation or the frame submission.
    pub fn render_with_ui(&mut self, scene: &Scene<'_>, ui: &UiDrawList) -> RenderResult<()> {
        self.render_internal(scene, Some(ui))
    }

    /// Points passes that sample transients at the current pool images.
    fn bind_transients(&self) {
        let hdr = self
            .plan
            .graph
            .slot_of(self.plan.resources.hdr)
            .expect("the composite keeps the HDR target live");
        self.composite
            .bind_hdr(&self.device, self.transients.image(hdr).view());
    }

    fn transient(&self, resource: crate::graph::ResourceId) -> &GpuImage {
        let slot = self
            .plan
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

    fn render_internal(&mut self, scene: &Scene<'_>, ui: Option<&UiDrawList>) -> RenderResult<()> {
        if !self.sync_extent()? {
            return Ok(());
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

        let image_available = self.frames[frame_index].image_available.handle();
        let (image_index, suboptimal) = match self.swapchain.acquire(image_available) {
            Ok(result) => result,
            Err(RenderError::SwapchainOutOfDate) => {
                self.recreate_swapchain()?;
                return Ok(());
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

        self.scene.prepare(frame_index, scene)?;
        self.ui.prepare(frame_index, ui)?;

        let command_buffer = self.frames[frame_index].command_buffer;
        let pool = self.frames[frame_index].command_pool.handle();
        // SAFETY: the pool belongs to this device and no command buffer from it is pending
        // (the frame fence was just waited on).
        unsafe {
            self.device
                .device
                .reset_command_pool(pool, vk::CommandPoolResetFlags::empty())
        }?;
        let begin = vk::CommandBufferBeginInfo::default();
        // SAFETY: the command buffer is allocated from the reset pool and not recording.
        unsafe {
            self.device
                .device
                .begin_command_buffer(command_buffer, &begin)
        }?;
        self.record_frame(command_buffer, frame_index, image_index)?;
        // SAFETY: the command buffer is recording and is ended exactly once.
        unsafe { self.device.device.end_command_buffer(command_buffer) }?;

        let render_finished = self.swapchain.render_finished[image_index as usize].handle();
        let wait_semaphores = [image_available];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let signal_semaphores = [render_finished];
        let command_buffers = [command_buffer];
        let submit = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(&command_buffers)
            .signal_semaphores(&signal_semaphores);
        // Reset as late as possible: every fallible recording step is complete, so a
        // failure before this point cannot leave an unsignalled fence for the next wait.
        // SAFETY: the fence was waited on above and is not in use by a submit.
        unsafe { self.device.device.reset_fences(&[fence]) }?;
        // SAFETY: the command buffer is executable and the semaphores are live.
        unsafe {
            self.device.device.queue_submit(
                self.device.graphics_queue,
                core::slice::from_ref(&submit),
                fence,
            )
        }?;
        self.profiler.submitted(frame_index);
        if self.capture_pending {
            self.capture_pending = false;
            self.capture_ready = true;
        }

        let present_result =
            self.swapchain
                .present(self.device.present_queue, image_index, render_finished);
        self.frame_index = (frame_index + 1) % FRAMES_IN_FLIGHT;

        match present_result {
            Ok(true) if !suboptimal => Ok(()),
            Ok(_) | Err(RenderError::SwapchainSuboptimal | RenderError::SwapchainOutOfDate) => {
                self.recreate_swapchain()?;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Records the compiled frame graph: per pass, its barriers, a timestamp pair around the
    /// pass's commands, then the final transitions.
    fn record_frame(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame_index: usize,
        image_index: u32,
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
        let plan = if capturing {
            &self.capture_plan
        } else {
            &self.plan
        };
        let resources = plan.resources;
        let hdr_view = self.transient(resources.hdr).view();
        let depth_view = self.transient(resources.depth).view();
        let bindings = self.bindings(plan, frame_index, swapchain_image, capture_buffer);
        self.profiler.begin_frame(command_buffer, frame_index);
        for pass in plan.graph.passes() {
            frame_graph::record_barriers(&self.device, command_buffer, &pass.barriers, &bindings);
            self.profiler
                .begin_pass(command_buffer, frame_index, pass.name);
            match pass.tag {
                PassKind::Scene => self.scene.record(
                    &self.device,
                    command_buffer,
                    frame_index,
                    &SceneTargets {
                        color: hdr_view,
                        depth: depth_view,
                        extent,
                        clear_color: self.config.clear_color,
                    },
                ),
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
            command_buffer,
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
        bindings.set(
            resources.bones,
            Binding::Buffer(self.scene.bone_buffer(frame_index)),
        );
        if let (Some(resource), Some(buffer)) = (resources.capture, capture_buffer) {
            bindings.set(resource, Binding::Buffer(buffer));
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
        self.swapchain = swapchain;
        self.transients =
            TransientPool::new(&self.device, self.plan.graph.slots(), self.swapchain.extent)?;
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
        self.capture_frame_internal(scene, None)
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
        self.capture_frame_internal(scene, Some(ui))
    }

    fn capture_frame_internal(
        &mut self,
        scene: &Scene<'_>,
        ui: Option<&UiDrawList>,
    ) -> RenderResult<CapturedFrame> {
        // The capture plan imports the readback buffer as idle: every capture waits for the
        // device below before reading it, so no earlier submission still touches it.
        self.capture_ready = false;
        self.capture_pending = true;
        self.render_internal(scene, ui)?;
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
            .finish_non_exhaustive()
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: no other thread submits work; waiting here makes the subsequent field
        // drops safe.
        let _ = unsafe { self.device.device.device_wait_idle() };
    }
}
