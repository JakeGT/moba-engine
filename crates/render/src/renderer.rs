//! The renderer: swapchain lifecycle, frame synchronisation and scene drawing.

use core::ffi::CStr;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ash::{Device, khr, vk};
use bytemuck::bytes_of;
use moba_platform::Window;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::instance::{InstanceContext, ValidationMode};
use crate::mesh::{self, GpuMesh};
use crate::pipeline::{self, PipelineSpec};
use crate::resources::{
    CommandPool, DescriptorPool, DescriptorSetLayout, Fence, GpuBuffer, GpuImage, GraphicsPipeline,
    PipelineLayout, Semaphore, ShaderModule,
};
use crate::scene::{CameraUniform, GpuInstance, Scene};
use crate::swapchain::Swapchain;
use crate::texture;
use crate::ui::{UiDrawList, UiVertex};
use gpu_allocator::MemoryLocation;

const FRAMES_IN_FLIGHT: usize = 2;
const UI_ATLAS: &[u8] = include_bytes!("../assets/ui-atlas.rgba");
static UI_VERTICES_TRUNCATED: AtomicU64 = AtomicU64::new(0);
static UI_TRUNCATION_LOGGED: AtomicBool = AtomicBool::new(false);

/// UI vertices dropped from the most recent submitted frame.
#[must_use]
pub fn ui_vertices_truncated() -> u64 {
    UI_VERTICES_TRUNCATED.load(Ordering::Relaxed)
}
const CAMERA_UNIFORM_SIZE: u64 = core::mem::size_of::<CameraUniform>() as u64;

/// Renderer configuration.
#[derive(Clone, Debug)]
pub struct RenderConfig {
    /// Validation layer policy.
    pub validation: ValidationMode,
    /// Maximum number of units drawn per frame. The instance buffer is pre-sized.
    pub max_instances: usize,
    /// Request vertical sync (FIFO). Disable to allow mailbox/immediate.
    pub vsync: bool,
    /// Linear RGBA clear colour.
    pub clear_color: [f32; 4],
    /// Maximum number of UI vertices per frame.
    pub max_ui_vertices: usize,
    /// HUD scale factor applied when uploading UI vertices.
    pub ui_scale: f32,
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
        }
    }
}

struct FrameData {
    command_pool: CommandPool,
    command_buffer: vk::CommandBuffer,
    image_available: Semaphore,
    fence: Fence,
    camera_buffer: GpuBuffer,
    instance_buffer: GpuBuffer,
    camera_set: vk::DescriptorSet,
    instance_set: vk::DescriptorSet,
    ui_buffer: GpuBuffer,
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

/// The Vulkan renderer.
///
/// `Renderer` owns the device and the swapchain. Field order is load-bearing: GPU
/// resources are declared before the device and the device before the surface/instance so
/// that Rust drops them children-first.
pub struct Renderer {
    swapchain: Swapchain,
    terrain_mesh: GpuMesh,
    unit_mesh: GpuMesh,
    _camera_layout: DescriptorSetLayout,
    _instance_layout: DescriptorSetLayout,
    _ui_layout: DescriptorSetLayout,
    _ui_atlas: GpuImage,
    ui_sampler: vk::Sampler,
    ui_sets: Vec<vk::DescriptorSet>,
    _descriptor_pool: DescriptorPool,
    pipeline_layout: PipelineLayout,
    unit_pipeline: GraphicsPipeline,
    terrain_pipeline: GraphicsPipeline,
    ui_pipeline_layout: PipelineLayout,
    ui_pipeline: GraphicsPipeline,
    frames: Vec<FrameData>,
    capture_buffer: Option<GpuBuffer>,
    capture_pending: bool,
    capture_ready: bool,
    images_in_flight: Vec<Option<usize>>,
    frame_index: usize,
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
    #[allow(clippy::too_many_lines)] // Pipeline/descriptor/resource wiring belongs together.
    pub fn new(window: &Window, config: RenderConfig) -> RenderResult<Self> {
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

        let unit_vertex = ShaderModule::new(device.device.clone(), moba_shaders::UNIT_VERT)?;
        let unit_fragment = ShaderModule::new(device.device.clone(), moba_shaders::UNIT_FRAG)?;
        let terrain_vertex = ShaderModule::new(device.device.clone(), moba_shaders::TERRAIN_VERT)?;
        let terrain_fragment =
            ShaderModule::new(device.device.clone(), moba_shaders::TERRAIN_FRAG)?;
        let ui_vertex = ShaderModule::new(device.device.clone(), moba_shaders::UI_VERT)?;
        let ui_fragment = ShaderModule::new(device.device.clone(), moba_shaders::UI_FRAG)?;

        let camera_binding = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)];
        let camera_layout = DescriptorSetLayout::new(device.device.clone(), &camera_binding)?;
        let instance_binding = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::VERTEX)];
        let instance_layout = DescriptorSetLayout::new(device.device.clone(), &instance_binding)?;
        let ui_binding = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let ui_layout = DescriptorSetLayout::new(device.device.clone(), &ui_binding)?;
        let ui_atlas = texture::upload_rgba(
            &device,
            "ui-atlas",
            crate::ui::ATLAS_WIDTH,
            crate::ui::ATLAS_HEIGHT,
            UI_ATLAS,
        )?;
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
        // SAFETY: the device is live and the sampler info is fully initialised.
        let ui_sampler = unsafe { device.device.create_sampler(&sampler_info, None) }?;

        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(u32::try_from(FRAMES_IN_FLIGHT).expect("frame count fits u32")),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(u32::try_from(FRAMES_IN_FLIGHT).expect("frame count fits u32")),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(u32::try_from(FRAMES_IN_FLIGHT).expect("frame count fits u32")),
        ];
        let descriptor_pool = DescriptorPool::new(
            device.device.clone(),
            &pool_sizes,
            u32::try_from(FRAMES_IN_FLIGHT * 3).expect("set count fits u32"),
        )?;

        let layouts = [camera_layout.handle(), instance_layout.handle()];
        let pipeline_layout = PipelineLayout::new(device.device.clone(), &layouts)?;

        let unit_pipeline = GraphicsPipeline::from_raw(
            device.device.clone(),
            pipeline::create(
                &device.device,
                &PipelineSpec {
                    layout: pipeline_layout.handle(),
                    vertex: unit_vertex.handle(),
                    fragment: unit_fragment.handle(),
                    color_format: swapchain.format,
                    depth_format: swapchain.depth_format,
                    cull_mode: vk::CullModeFlags::NONE,
                },
            )?,
        );
        let terrain_pipeline = GraphicsPipeline::from_raw(
            device.device.clone(),
            pipeline::create(
                &device.device,
                &PipelineSpec {
                    layout: pipeline_layout.handle(),
                    vertex: terrain_vertex.handle(),
                    fragment: terrain_fragment.handle(),
                    color_format: swapchain.format,
                    depth_format: swapchain.depth_format,
                    cull_mode: vk::CullModeFlags::NONE,
                },
            )?,
        );

        let ui_push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(8)];
        let ui_pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[ui_layout.handle()],
            &ui_push,
        )?;
        let ui_pipeline = GraphicsPipeline::from_raw(
            device.device.clone(),
            pipeline::create_ui(
                &device.device,
                ui_pipeline_layout.handle(),
                ui_vertex.handle(),
                ui_fragment.handle(),
                swapchain.format,
                swapchain.depth_format,
            )?,
        );

        let terrain_mesh = mesh::upload(&device, &mesh::MeshData::plane(300.0))?;
        let unit_mesh = mesh::upload(&device, &mesh::MeshData::cube())?;

        let descriptor_layouts: Vec<vk::DescriptorSetLayout> = (0..FRAMES_IN_FLIGHT)
            .flat_map(|_| [camera_layout.handle(), instance_layout.handle()])
            .collect();
        let sets = descriptor_pool.allocate(&device.device, &descriptor_layouts)?;
        let ui_set_layouts: Vec<vk::DescriptorSetLayout> =
            (0..FRAMES_IN_FLIGHT).map(|_| ui_layout.handle()).collect();
        let ui_sets = descriptor_pool.allocate(&device.device, &ui_set_layouts)?;
        let image_info = [vk::DescriptorImageInfo::default()
            .sampler(ui_sampler)
            .image_view(ui_atlas.view())
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let writes: Vec<vk::WriteDescriptorSet<'_>> = ui_sets
            .iter()
            .map(|set| {
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&image_info)
            })
            .collect();
        // SAFETY: the sets, sampler and image view are live and the write describes them.
        unsafe { device.device.update_descriptor_sets(&writes, &[]) };
        let instance_buffer_size = u64::try_from(core::mem::size_of::<GpuInstance>())
            .expect("instance size fits u64")
            * u64::try_from(config.max_instances).expect("instance count fits u64");
        let mut frames = Vec::with_capacity(FRAMES_IN_FLIGHT);
        for frame_index in 0..FRAMES_IN_FLIGHT {
            let command_pool = CommandPool::new(device.device.clone(), device.graphics_family)?;
            let command_buffer = command_pool.allocate_one()?;
            let camera_buffer = GpuBuffer::new(
                device.device.clone(),
                device.allocator(),
                "camera-uniform",
                CAMERA_UNIFORM_SIZE,
                vk::BufferUsageFlags::UNIFORM_BUFFER,
                MemoryLocation::CpuToGpu,
            )?;
            let instance_buffer = GpuBuffer::new(
                device.device.clone(),
                device.allocator(),
                "unit-instances",
                instance_buffer_size,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                MemoryLocation::CpuToGpu,
            )?;
            let ui_buffer_size = u64::try_from(core::mem::size_of::<UiVertex>())
                .expect("UI vertex size fits u64")
                * u64::try_from(config.max_ui_vertices).expect("UI vertex count fits u64");
            let ui_buffer = GpuBuffer::new(
                device.device.clone(),
                device.allocator(),
                "ui-vertices",
                ui_buffer_size,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                MemoryLocation::CpuToGpu,
            )?;
            let camera_set = sets[frame_index * 2];
            let instance_set = sets[frame_index * 2 + 1];
            write_descriptors(
                &device.device,
                camera_set,
                instance_set,
                &camera_buffer,
                &instance_buffer,
            );
            frames.push(FrameData {
                command_pool,
                command_buffer,
                image_available: Semaphore::new(device.device.clone())?,
                fence: Fence::new(device.device.clone(), true)?,
                camera_buffer,
                instance_buffer,
                camera_set,
                instance_set,
                ui_buffer,
            });
        }

        let images_in_flight = vec![None; swapchain.image_count()];

        Ok(Self {
            window: window.clone(),
            swapchain,
            terrain_mesh,
            unit_mesh,
            _camera_layout: camera_layout,
            _instance_layout: instance_layout,
            _ui_layout: ui_layout,
            _ui_atlas: ui_atlas,
            ui_sampler,
            ui_sets,
            _descriptor_pool: descriptor_pool,
            pipeline_layout,
            unit_pipeline,
            terrain_pipeline,
            ui_pipeline_layout,
            ui_pipeline,
            frames,
            capture_buffer: None,
            capture_pending: false,
            capture_ready: false,
            images_in_flight,
            frame_index: 0,
            device,
            surface,
            instance,
            config,
        })
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

    #[allow(clippy::too_many_lines)] // The frame is one explicit, ordered command stream.
    fn render_internal(&mut self, scene: &Scene<'_>, ui: Option<&UiDrawList>) -> RenderResult<()> {
        let [width, height] = self.window.size();
        if width == 0 || height == 0 {
            return Ok(());
        }
        if width != self.swapchain.extent.width || height != self.swapchain.extent.height {
            // A fixed surface extent (common on Wayland/HiDPI) can legitimately differ from
            // the logical window size forever; only recreate when the chosen extent changes.
            // SAFETY: the surface and physical device belong to this instance.
            let capabilities = unsafe {
                self.device
                    .surface
                    .get_physical_device_surface_capabilities(
                        self.device.physical,
                        self.surface.handle,
                    )
            }?;
            let desired =
                crate::swapchain::choose_extent(capabilities, vk::Extent2D { width, height });
            if desired != self.swapchain.extent {
                self.recreate_swapchain()?;
            }
        }
        if self.capture_pending {
            if !self.swapchain.transfer_src {
                return Err(RenderError::NoSuitableDevice(String::from(
                    "swapchain does not support TRANSFER_SRC for capture",
                )));
            }
            self.ensure_capture_buffer()?;
        }

        let frame_index = self.frame_index;
        let fence = self.frames[frame_index].fence.handle();
        // SAFETY: the fence belongs to this device and is waited on with an effectively
        // infinite timeout. It is signalled or will be signalled by the frame's submit.
        unsafe { self.device.device.wait_for_fences(&[fence], true, u64::MAX) }?;

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

        let unit_count = scene.units.len().min(self.config.max_instances);
        {
            let frame = &mut self.frames[frame_index];
            let uniform = CameraUniform::new(scene.camera, &scene.vision, scene.terrain_tint);
            frame.camera_buffer.write_bytes(0, bytes_of(&uniform));
            if unit_count > 0 {
                let mapped = frame
                    .instance_buffer
                    .mapped_slice_mut::<GpuInstance>(unit_count)
                    .ok_or_else(|| {
                        RenderError::NoSuitableDevice(String::from(
                            "instance buffer is not host visible",
                        ))
                    })?;
                for (destination, source) in mapped.iter_mut().zip(&scene.units[..unit_count]) {
                    *destination = GpuInstance::from(*source);
                }
            }
            let ui_total = ui.map_or(0, |list| list.vertices().len());
            let ui_count = ui_total.min(self.config.max_ui_vertices);
            let truncated = u64::try_from(ui_total.saturating_sub(ui_count)).unwrap_or(u64::MAX);
            UI_VERTICES_TRUNCATED.store(truncated, Ordering::Relaxed);
            if truncated > 0 {
                if !UI_TRUNCATION_LOGGED.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "[ui] dropped {truncated} UI vertices (max {})",
                        self.config.max_ui_vertices
                    );
                }
            } else {
                UI_TRUNCATION_LOGGED.store(false, Ordering::Relaxed);
            }
            if ui_count > 0 {
                let source = ui.expect("UI list exists when UI count is non-zero");
                let mapped = frame
                    .ui_buffer
                    .mapped_slice_mut::<UiVertex>(ui_count)
                    .ok_or_else(|| {
                        RenderError::NoSuitableDevice(String::from(
                            "UI vertex buffer is not host visible",
                        ))
                    })?;
                let scale = self.config.ui_scale;
                for (target, source) in mapped.iter_mut().zip(&source.vertices()[..ui_count]) {
                    *target = *source;
                    target.position[0] *= scale;
                    target.position[1] *= scale;
                }
            }
        }

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

        let color_layout = self.swapchain.color_layouts[image_index as usize];
        let depth_layout = self.swapchain.depth_layouts[image_index as usize];
        transition_color(
            &self.device,
            command_buffer,
            self.swapchain.images[image_index as usize],
            color_layout,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        );
        if depth_layout != vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL {
            transition_depth(
                &self.device,
                command_buffer,
                self.swapchain.depth_images[image_index as usize].image(),
                depth_layout,
                self.swapchain.depth_aspect,
            );
        }
        self.swapchain.color_layouts[image_index as usize] =
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL;
        self.swapchain.depth_layouts[image_index as usize] =
            vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL;

        let extent = self.swapchain.extent;
        let clear_color = vk::ClearValue {
            color: vk::ClearColorValue {
                float32: self.config.clear_color,
            },
        };
        let color_attachment = vk::RenderingAttachmentInfo::default()
            .image_view(self.swapchain.views[image_index as usize])
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(clear_color);
        let depth_clear = vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue {
                depth: 1.0,
                stencil: 0,
            },
        };
        let depth_attachment = vk::RenderingAttachmentInfo::default()
            .image_view(self.swapchain.depth_images[image_index as usize].view())
            .image_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .clear_value(depth_clear);
        let render_info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            })
            .layer_count(1)
            .color_attachments(core::slice::from_ref(&color_attachment))
            .depth_attachment(&depth_attachment);
        self.device
            .cmd_begin_rendering(command_buffer, &render_info);

        let viewport = vk::Viewport {
            x: 0.0,
            y: extent.height as f32,
            width: extent.width as f32,
            height: -(extent.height as f32),
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let scissor = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent,
        };
        // SAFETY: the command buffer is recording inside a render pass and the viewport
        // and scissor are valid for this extent.
        unsafe {
            self.device
                .device
                .cmd_set_viewport(command_buffer, 0, &[viewport]);
        }
        // SAFETY: same as above.
        unsafe {
            self.device
                .device
                .cmd_set_scissor(command_buffer, 0, &[scissor]);
        }

        let descriptor_sets = [
            self.frames[frame_index].camera_set,
            self.frames[frame_index].instance_set,
        ];
        // SAFETY: both sets were allocated from this frame's layout and the pipeline layout
        // declares them at indices 0 and 1.
        unsafe {
            self.device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout.handle(),
                0,
                &descriptor_sets,
                &[],
            );
        }

        // Terrain.
        // SAFETY: pipeline, buffers and descriptor sets are live and compatible.
        unsafe {
            self.device.device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.terrain_pipeline.handle(),
            );
        }
        // SAFETY: same as above.
        unsafe {
            self.device.device.cmd_bind_vertex_buffers(
                command_buffer,
                0,
                &[self.terrain_mesh.vertices.handle()],
                &[0],
            );
        }
        // SAFETY: the index buffer is a 32-bit index buffer.
        unsafe {
            self.device.device.cmd_bind_index_buffer(
                command_buffer,
                self.terrain_mesh.indices.handle(),
                0,
                vk::IndexType::UINT32,
            );
        }
        // SAFETY: the mesh is uploaded and drawable.
        unsafe {
            self.device.device.cmd_draw_indexed(
                command_buffer,
                self.terrain_mesh.index_count,
                1,
                0,
                0,
                0,
            );
        }

        if unit_count > 0 {
            // SAFETY: pipeline, buffers and descriptor sets are live and compatible.
            unsafe {
                self.device.device.cmd_bind_pipeline(
                    command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.unit_pipeline.handle(),
                );
            }
            // SAFETY: same as above.
            unsafe {
                self.device.device.cmd_bind_vertex_buffers(
                    command_buffer,
                    0,
                    &[self.unit_mesh.vertices.handle()],
                    &[0],
                );
            }
            // SAFETY: the index buffer is a 32-bit index buffer.
            unsafe {
                self.device.device.cmd_bind_index_buffer(
                    command_buffer,
                    self.unit_mesh.indices.handle(),
                    0,
                    vk::IndexType::UINT32,
                );
            }
            // SAFETY: `unit_count` was bounded by the instance buffer capacity.
            unsafe {
                self.device.device.cmd_draw_indexed(
                    command_buffer,
                    self.unit_mesh.index_count,
                    u32::try_from(unit_count).expect("unit count fits u32"),
                    0,
                    0,
                    0,
                );
            }
        }

        let ui_count = ui.map_or(0, |list| {
            list.vertices().len().min(self.config.max_ui_vertices)
        });
        if ui_count > 0 {
            // SAFETY: pipeline, buffer and push constant range are live and compatible.
            unsafe {
                self.device.device.cmd_bind_pipeline(
                    command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.ui_pipeline.handle(),
                );
            }
            // SAFETY: the UI buffer was written for this frame and is large enough.
            unsafe {
                self.device.device.cmd_bind_vertex_buffers(
                    command_buffer,
                    0,
                    &[self.frames[frame_index].ui_buffer.handle()],
                    &[0],
                );
            }
            // SAFETY: the UI set was written for this frame index and matches the layout.
            unsafe {
                self.device.device.cmd_bind_descriptor_sets(
                    command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.ui_pipeline_layout.handle(),
                    0,
                    &[self.ui_sets[frame_index]],
                    &[],
                );
            }
            let viewport_size = [extent.width as f32, extent.height as f32];
            // SAFETY: the UI pipeline layout declares exactly this 8-byte vertex-stage push
            // constant range.
            unsafe {
                self.device.device.cmd_push_constants(
                    command_buffer,
                    self.ui_pipeline_layout.handle(),
                    vk::ShaderStageFlags::VERTEX,
                    0,
                    bytes_of(&viewport_size),
                );
            }
            // SAFETY: `ui_count` was bounded by the vertex buffer capacity.
            unsafe {
                self.device.device.cmd_draw(
                    command_buffer,
                    u32::try_from(ui_count).expect("UI vertex count fits u32"),
                    1,
                    0,
                    0,
                );
            }
        }

        self.device.cmd_end_rendering(command_buffer);

        let capturing = self.capture_pending;
        let color_image = self.swapchain.images[image_index as usize];
        if capturing {
            transition_image(
                &self.device,
                command_buffer,
                color_image,
                vk::ImageAspectFlags::COLOR,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                vk::PipelineStageFlags2::TRANSFER,
                vk::AccessFlags2::TRANSFER_READ,
            );
            let capture = self.capture_buffer.as_mut().ok_or_else(|| {
                RenderError::NoSuitableDevice(String::from(
                    "capture buffer missing while capture is pending",
                ))
            })?;
            let required = u64::from(self.swapchain.extent.width)
                * u64::from(self.swapchain.extent.height)
                * 4;
            if capture.size() < required {
                return Err(RenderError::NoSuitableDevice(format!(
                    "capture buffer too small: {} < {required}",
                    capture.size()
                )));
            }
            let region = vk::BufferImageCopy::default()
                .buffer_offset(0)
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(0)
                        .base_array_layer(0)
                        .layer_count(1),
                )
                .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
                .image_extent(vk::Extent3D {
                    width: self.swapchain.extent.width,
                    height: self.swapchain.extent.height,
                    depth: 1,
                });
            // SAFETY: the image is in TRANSFER_SRC_OPTIMAL, the buffer is large enough and
            // usable as a copy destination.
            unsafe {
                self.device.device.cmd_copy_image_to_buffer(
                    command_buffer,
                    color_image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    capture.handle(),
                    core::slice::from_ref(&region),
                );
            }
            let barrier = vk::BufferMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                .dst_access_mask(vk::AccessFlags2::HOST_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(capture.handle())
                .offset(0)
                .size(vk::WHOLE_SIZE);
            let dependency = vk::DependencyInfo::default()
                .buffer_memory_barriers(core::slice::from_ref(&barrier));
            self.device
                .cmd_pipeline_barrier2(command_buffer, &dependency);
            transition_image(
                &self.device,
                command_buffer,
                color_image,
                vk::ImageAspectFlags::COLOR,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::ImageLayout::PRESENT_SRC_KHR,
                vk::PipelineStageFlags2::TRANSFER,
                vk::AccessFlags2::TRANSFER_READ,
                vk::PipelineStageFlags2::NONE,
                vk::AccessFlags2::NONE,
            );
        } else {
            transition_color(
                &self.device,
                command_buffer,
                color_image,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::PRESENT_SRC_KHR,
            );
        }
        self.swapchain.color_layouts[image_index as usize] = vk::ImageLayout::PRESENT_SRC_KHR;

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
        if capturing {
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

    fn recreate_swapchain(&mut self) -> RenderResult<()> {
        let [width, height] = self.window.size();
        if width == 0 || height == 0 {
            return Ok(());
        }
        // SAFETY: waiting for the device to idle guarantees no frame is still using the old
        // swapchain before it is replaced.
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
        self.images_in_flight = vec![None; self.swapchain.image_count()];
        self.frame_index = 0;
        Ok(())
    }
}

/// A captured frame, in tightly packed RGBA8.
#[derive(Clone, Debug)]
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
        let extent = self.swapchain.extent;
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
        match self.swapchain.format {
            vk::Format::B8G8R8A8_SRGB | vk::Format::B8G8R8A8_UNORM => {
                for pixel in rgba.chunks_exact_mut(4) {
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
            .finish_non_exhaustive()
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: no other thread submits work; waiting here makes the subsequent field
        // drops safe.
        let _ = unsafe { self.device.device.device_wait_idle() };
        // SAFETY: the device is idle and this sampler is owned by the renderer.
        unsafe { self.device.device.destroy_sampler(self.ui_sampler, None) };
    }
}

fn write_descriptors(
    device: &Device,
    camera_set: vk::DescriptorSet,
    instance_set: vk::DescriptorSet,
    camera_buffer: &GpuBuffer,
    instance_buffer: &GpuBuffer,
) {
    let camera_info = [vk::DescriptorBufferInfo::default()
        .buffer(camera_buffer.handle())
        .offset(0)
        .range(CAMERA_UNIFORM_SIZE)];
    let instance_info = [vk::DescriptorBufferInfo::default()
        .buffer(instance_buffer.handle())
        .offset(0)
        .range(vk::WHOLE_SIZE)];
    let writes = [
        vk::WriteDescriptorSet::default()
            .dst_set(camera_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .buffer_info(&camera_info),
        vk::WriteDescriptorSet::default()
            .dst_set(instance_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .buffer_info(&instance_info),
    ];
    // SAFETY: both descriptor sets were allocated from a compatible pool and the buffer
    // info entries reference live buffers.
    unsafe { device.update_descriptor_sets(&writes, &[]) };
}

#[allow(clippy::too_many_arguments)]
fn transition_image(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    aspect: vk::ImageAspectFlags,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    src_stage: vk::PipelineStageFlags2,
    src_access: vk::AccessFlags2,
    dst_stage: vk::PipelineStageFlags2,
    dst_access: vk::AccessFlags2,
) {
    let barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(src_stage)
        .src_access_mask(src_access)
        .dst_stage_mask(dst_stage)
        .dst_access_mask(dst_access)
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(aspect)
                .base_mip_level(0)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(1),
        );
    let dependency =
        vk::DependencyInfo::default().image_memory_barriers(core::slice::from_ref(&barrier));
    device.cmd_pipeline_barrier2(command_buffer, &dependency);
}

fn transition_color(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
) {
    let (src_stage, src_access, dst_stage, dst_access) = if old_layout == vk::ImageLayout::UNDEFINED
    {
        (
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags2::NONE,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        )
    } else {
        (
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        )
    };
    let barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(src_stage)
        .src_access_mask(src_access)
        .dst_stage_mask(dst_stage)
        .dst_access_mask(dst_access)
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(0)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(1),
        );
    let dependency =
        vk::DependencyInfo::default().image_memory_barriers(core::slice::from_ref(&barrier));
    device.cmd_pipeline_barrier2(command_buffer, &dependency);
}

fn transition_depth(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    old_layout: vk::ImageLayout,
    aspect: vk::ImageAspectFlags,
) {
    let (src_stage, src_access) = if old_layout == vk::ImageLayout::UNDEFINED {
        (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE)
    } else {
        (
            vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
            vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
        )
    };
    let barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(src_stage)
        .src_access_mask(src_access)
        .dst_stage_mask(
            vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
        )
        .dst_access_mask(vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE)
        .old_layout(old_layout)
        .new_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(aspect)
                .base_mip_level(0)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(1),
        );
    let dependency =
        vk::DependencyInfo::default().image_memory_barriers(core::slice::from_ref(&barrier));
    device.cmd_pipeline_barrier2(command_buffer, &dependency);
}
