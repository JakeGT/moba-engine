//! Contact-shadow pass (R2b, ADR-0016): a fullscreen ray march over the depth pre-pass into
//! an `R8_UNORM` visibility target that the scene's `shade()` multiplies into its shadow
//! term. High and up; it reads the main view's camera uniform (set 0) for the matrices.

use ash::vk;

use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::frame_graph::CONTACT_FORMAT;
use crate::pipeline;
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GraphicsPipeline, PipelineLayout, Sampler,
};
use crate::shaders::ShaderSource;

/// The fullscreen pipeline and the set naming the depth target.
pub(crate) struct ContactPass {
    set: vk::DescriptorSet,
    _descriptor_pool: DescriptorPool,
    _set_layout: DescriptorSetLayout,
    sampler: Sampler,
    pipeline_layout: PipelineLayout,
    pipeline: GraphicsPipeline,
}

impl ContactPass {
    /// `camera_layout` is the scene's camera set layout (set 0).
    pub(crate) fn new(
        device: &DeviceContext,
        shaders: &ShaderSource,
        camera_layout: vk::DescriptorSetLayout,
    ) -> RenderResult<Self> {
        let binding = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let set_layout = DescriptorSetLayout::new(device.device.clone(), &binding)?;
        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)];
        let descriptor_pool = DescriptorPool::new(device.device.clone(), &pool_sizes, 1)?;
        let set = descriptor_pool.allocate(&device.device, &[set_layout.handle()])?[0];
        // `texelFetch` only; a sampler is still required for `sampler2D`.
        let sampler = Sampler::new(
            device.device.clone(),
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::NEAREST)
                .min_filter(vk::Filter::NEAREST)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
        )?;
        let pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[camera_layout, set_layout.handle()],
            &[],
        )?;
        let pipeline = build_pipeline(device, shaders, pipeline_layout.handle())?;
        Ok(Self {
            set,
            _descriptor_pool: descriptor_pool,
            _set_layout: set_layout,
            sampler,
            pipeline_layout,
            pipeline,
        })
    }

    /// Builds a replacement pipeline (dev shader reload) without installing it.
    pub(crate) fn build_pipeline(
        &self,
        device: &DeviceContext,
        shaders: &ShaderSource,
    ) -> RenderResult<GraphicsPipeline> {
        build_pipeline(device, shaders, self.pipeline_layout.handle())
    }

    /// Installs a pipeline from [`Self::build_pipeline`]. The device must be idle.
    pub(crate) fn install_pipeline(&mut self, pipeline: GraphicsPipeline) {
        self.pipeline = pipeline;
    }

    /// Points the set at the depth target. Only while no submitted frame uses the set
    /// (creation, after the resize's device-idle wait).
    pub(crate) fn bind_depth(&self, device: &DeviceContext, depth: vk::ImageView) {
        let image_info = [vk::DescriptorImageInfo::default()
            .sampler(self.sampler.handle())
            .image_view(depth)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info);
        // SAFETY: the set, sampler and view are live and no pending command buffer uses the
        // set (see the method contract).
        unsafe {
            device
                .device
                .update_descriptor_sets(core::slice::from_ref(&write), &[]);
        }
    }

    /// Records the ray march into `target` (every fragment is written), reading this frame's
    /// main-view camera set.
    pub(crate) fn record(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        camera_set: vk::DescriptorSet,
        target: vk::ImageView,
        extent: vk::Extent2D,
    ) {
        super::begin_color_only(
            device,
            command_buffer,
            target,
            extent,
            vk::AttachmentLoadOp::DONT_CARE,
            super::full_viewport(extent),
        );
        // SAFETY: the pipeline is live and was built for this attachment format.
        unsafe {
            device.device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline.handle(),
            );
        }
        // SAFETY: set 0 is a camera set allocated with the layout the pipeline layout names
        // at 0 (its uniform was written this frame, before submission); set 1 names the
        // depth target, which the graph transitioned to SHADER_READ_ONLY_OPTIMAL.
        unsafe {
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout.handle(),
                0,
                &[camera_set, self.set],
                &[],
            );
        }
        // SAFETY: the fullscreen vertex shader reads no vertex buffers.
        unsafe { device.device.cmd_draw(command_buffer, 3, 1, 0, 0) };
        device.cmd_end_rendering(command_buffer);
    }
}

fn build_pipeline(
    device: &DeviceContext,
    shaders: &ShaderSource,
    layout: vk::PipelineLayout,
) -> RenderResult<GraphicsPipeline> {
    let vertex = shaders.module(device, "fullscreen.vert")?;
    let fragment = shaders.module(device, "contact_shadows.frag")?;
    let handle = pipeline::create_fullscreen(
        device,
        layout,
        vertex.handle(),
        fragment.handle(),
        CONTACT_FORMAT,
    )?;
    Ok(GraphicsPipeline::from_raw(device.device.clone(), handle))
}
