//! UI pass: the HUD batch blended over the tonemapped swapchain image (never tonemapped).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ash::vk;
use bytemuck::bytes_of;
use gpu_allocator::MemoryLocation;

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::pipeline;
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuBuffer, GpuImage, GraphicsPipeline, PipelineLayout,
    Sampler, ShaderModule,
};
use crate::texture;
use crate::ui::{UiDrawList, UiVertex};

const UI_ATLAS: &[u8] = include_bytes!("../../assets/ui-atlas.rgba");
static UI_VERTICES_TRUNCATED: AtomicU64 = AtomicU64::new(0);
static UI_TRUNCATION_LOGGED: AtomicBool = AtomicBool::new(false);

/// UI vertices dropped from the most recent submitted frame.
#[must_use]
pub fn ui_vertices_truncated() -> u64 {
    UI_VERTICES_TRUNCATED.load(Ordering::Relaxed)
}

/// Atlas, pipeline and per-frame vertex buffers of the UI pass.
pub(crate) struct UiPass {
    buffers: Vec<GpuBuffer>,
    sets: Vec<vk::DescriptorSet>,
    _descriptor_pool: DescriptorPool,
    _set_layout: DescriptorSetLayout,
    _atlas: GpuImage,
    _sampler: Sampler,
    pipeline_layout: PipelineLayout,
    pipeline: GraphicsPipeline,
    max_vertices: usize,
    scale: f32,
    /// Vertices uploaded for the frame being recorded.
    count: u32,
}

impl UiPass {
    pub(crate) fn new(
        device: &DeviceContext,
        frames: usize,
        color_format: vk::Format,
        max_vertices: usize,
        scale: f32,
    ) -> RenderResult<Self> {
        let vertex = ShaderModule::new(device.device.clone(), moba_shaders::UI_VERT)?;
        let fragment = ShaderModule::new(device.device.clone(), moba_shaders::UI_FRAG)?;
        let binding = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let set_layout = DescriptorSetLayout::new(device.device.clone(), &binding)?;
        let atlas = texture::upload_rgba(
            device,
            "ui-atlas",
            crate::ui::ATLAS_WIDTH,
            crate::ui::ATLAS_HEIGHT,
            UI_ATLAS,
        )?;
        let sampler = Sampler::new(
            device.device.clone(),
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::NEAREST)
                .min_filter(vk::Filter::NEAREST)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
        )?;
        let frame_count = u32::try_from(frames).expect("frame count fits u32");
        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(frame_count)];
        let descriptor_pool = DescriptorPool::new(device.device.clone(), &pool_sizes, frame_count)?;
        let set_layouts = vec![set_layout.handle(); frames];
        let sets = descriptor_pool.allocate(&device.device, &set_layouts)?;
        let image_info = [vk::DescriptorImageInfo::default()
            .sampler(sampler.handle())
            .image_view(atlas.view())
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let writes: Vec<vk::WriteDescriptorSet<'_>> = sets
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

        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(8)];
        let pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[set_layout.handle()],
            &push,
        )?;
        let pipeline = GraphicsPipeline::from_raw(
            device.device.clone(),
            pipeline::create_ui(
                &device.device,
                pipeline_layout.handle(),
                vertex.handle(),
                fragment.handle(),
                color_format,
            )?,
        );
        let buffer_size = u64::try_from(core::mem::size_of::<UiVertex>())
            .expect("UI vertex size fits u64")
            * u64::try_from(max_vertices).expect("UI vertex count fits u64");
        let mut buffers = Vec::with_capacity(frames);
        for _ in 0..frames {
            buffers.push(GpuBuffer::new(
                device.device.clone(),
                device.allocator(),
                "ui-vertices",
                buffer_size,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                MemoryLocation::CpuToGpu,
            )?);
        }
        Ok(Self {
            buffers,
            sets,
            _descriptor_pool: descriptor_pool,
            _set_layout: set_layout,
            _atlas: atlas,
            _sampler: sampler,
            pipeline_layout,
            pipeline,
            max_vertices,
            scale,
            count: 0,
        })
    }

    /// Uploads this frame's vertices, scaled by the HUD scale. The frame's fence must have
    /// been waited on.
    pub(crate) fn prepare(&mut self, frame: usize, ui: Option<&UiDrawList>) -> RenderResult<()> {
        let total = ui.map_or(0, |list| list.vertices().len());
        let count = total.min(self.max_vertices);
        let truncated = u64::try_from(total - count).unwrap_or(u64::MAX);
        UI_VERTICES_TRUNCATED.store(truncated, Ordering::Relaxed);
        if truncated > 0 {
            if !UI_TRUNCATION_LOGGED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "[ui] dropped {truncated} UI vertices (max {})",
                    self.max_vertices
                );
            }
        } else {
            UI_TRUNCATION_LOGGED.store(false, Ordering::Relaxed);
        }
        self.count = 0;
        let Some(source) = ui.filter(|_| count > 0) else {
            return Ok(());
        };
        let mapped = self.buffers[frame]
            .mapped_slice_mut::<UiVertex>(count)
            .ok_or_else(|| {
                RenderError::NoSuitableDevice(String::from("UI vertex buffer is not host visible"))
            })?;
        // Scale a local copy and store it once: `mapped` is write-combined host memory,
        // where every read-back (`target.position[0] *= ...`) is an uncached load. With a few
        // thousand HUD vertices that cost ~7 ms per frame.
        for (target, vertex) in mapped.iter_mut().zip(&source.vertices()[..count]) {
            *target = scaled_ui_vertex(*vertex, self.scale);
        }
        self.count = u32::try_from(count).expect("bounded by max_ui_vertices");
        Ok(())
    }

    /// Records the HUD over `target` (loaded, blended). With no vertices nothing is
    /// recorded; the graph's barriers still run.
    pub(crate) fn record(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        target: vk::ImageView,
        extent: vk::Extent2D,
    ) {
        if self.count == 0 {
            return;
        }
        super::begin_color_only(
            device,
            command_buffer,
            target,
            extent,
            vk::AttachmentLoadOp::LOAD,
            super::flipped_viewport(extent),
        );
        // SAFETY: pipeline, buffer and push constant range are live and compatible.
        unsafe {
            device.device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline.handle(),
            );
        }
        // SAFETY: the UI buffer was written for this frame and is large enough.
        unsafe {
            device.device.cmd_bind_vertex_buffers(
                command_buffer,
                0,
                &[self.buffers[frame].handle()],
                &[0],
            );
        }
        // SAFETY: the UI set was written for this frame index and matches the layout.
        unsafe {
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout.handle(),
                0,
                &[self.sets[frame]],
                &[],
            );
        }
        let viewport_size = [extent.width as f32, extent.height as f32];
        // SAFETY: the UI pipeline layout declares exactly this 8-byte vertex-stage push
        // constant range.
        unsafe {
            device.device.cmd_push_constants(
                command_buffer,
                self.pipeline_layout.handle(),
                vk::ShaderStageFlags::VERTEX,
                0,
                bytes_of(&viewport_size),
            );
        }
        // SAFETY: `count` was bounded by the vertex buffer capacity in `prepare`.
        unsafe { device.device.cmd_draw(command_buffer, self.count, 1, 0, 0) };
        device.cmd_end_rendering(command_buffer);
    }
}

/// A UI vertex with its position scaled by the HUD scale, built off the mapped buffer.
fn scaled_ui_vertex(mut vertex: UiVertex, scale: f32) -> UiVertex {
    vertex.position[0] *= scale;
    vertex.position[1] *= scale;
    vertex
}
