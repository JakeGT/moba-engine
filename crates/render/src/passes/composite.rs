//! Composite pass: HDR scene colour → exposure → `AgX` tonemap → swapchain (ADR-0008).

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};

use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::pipeline;
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GraphicsPipeline, PipelineLayout, Sampler,
};
use crate::shaders::ShaderSource;

/// Push-constant block of `composite.frag` (16 bytes, fragment stage).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct CompositePush {
    exposure: f32,
    saturation: f32,
    srgb_target: u32,
    reserved: u32,
}
const COMPOSITE_PUSH_SIZE: u32 = core::mem::size_of::<CompositePush>() as u32;
/// `AgX` look saturation (the "punchy" preset's 1.4 with identity slope/offset/power): keeps
/// team colours as saturated as the pre-tonemap image. Colour grading replaces it in R6.
const TONEMAP_SATURATION: f32 = 1.4;

/// Fullscreen tonemap pipeline and the descriptor set that names the HDR target.
pub(crate) struct CompositePass {
    set: vk::DescriptorSet,
    _descriptor_pool: DescriptorPool,
    _set_layout: DescriptorSetLayout,
    sampler: Sampler,
    pipeline_layout: PipelineLayout,
    pipeline: GraphicsPipeline,
    swapchain_format: vk::Format,
    push: CompositePush,
}

impl CompositePass {
    pub(crate) fn new(
        device: &DeviceContext,
        shaders: &ShaderSource,
        swapchain_format: vk::Format,
        exposure: f32,
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
        // `texelFetch` ignores filtering; a sampler is still required for `sampler2D`.
        let sampler = Sampler::new(
            device.device.clone(),
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::NEAREST)
                .min_filter(vk::Filter::NEAREST)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
        )?;
        let push_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(COMPOSITE_PUSH_SIZE)];
        let pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[set_layout.handle()],
            &push_range,
        )?;
        let pipeline = build_pipeline(device, shaders, pipeline_layout.handle(), swapchain_format)?;
        let exposure = if exposure.is_finite() && exposure > 0.0 {
            exposure
        } else {
            1.0
        };
        Ok(Self {
            set,
            _descriptor_pool: descriptor_pool,
            _set_layout: set_layout,
            sampler,
            pipeline_layout,
            pipeline,
            swapchain_format,
            push: CompositePush {
                exposure,
                saturation: TONEMAP_SATURATION,
                srgb_target: u32::from(is_srgb(swapchain_format)),
                reserved: 0,
            },
        })
    }

    /// Rebuilds the pipeline from `shaders` (dev shader reload); the device must be idle.
    pub(crate) fn rebuild_pipeline(
        &mut self,
        device: &DeviceContext,
        shaders: &ShaderSource,
    ) -> RenderResult<()> {
        self.pipeline = build_pipeline(
            device,
            shaders,
            self.pipeline_layout.handle(),
            self.swapchain_format,
        )?;
        Ok(())
    }

    /// Points the descriptor set at the HDR target. Only call while no submitted frame
    /// uses the set (at creation and after the resize's device-idle wait).
    pub(crate) fn bind_hdr(&self, device: &DeviceContext, view: vk::ImageView) {
        let image_info = [vk::DescriptorImageInfo::default()
            .sampler(self.sampler.handle())
            .image_view(view)
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

    /// Records the fullscreen tonemap into `target`, which every fragment overwrites.
    pub(crate) fn record(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
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
        // SAFETY: the set matches set 0 of the layout and names the live HDR view, which the
        // graph transitioned to SHADER_READ_ONLY_OPTIMAL.
        unsafe {
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout.handle(),
                0,
                &[self.set],
                &[],
            );
        }
        // SAFETY: the layout declares exactly this 16-byte fragment-stage range at offset 0.
        unsafe {
            device.device.cmd_push_constants(
                command_buffer,
                self.pipeline_layout.handle(),
                vk::ShaderStageFlags::FRAGMENT,
                0,
                bytes_of(&self.push),
            );
        }
        // SAFETY: the fullscreen vertex shader reads no vertex buffers; three vertices form
        // the covering triangle.
        unsafe { device.device.cmd_draw(command_buffer, 3, 1, 0, 0) };
        device.cmd_end_rendering(command_buffer);
    }
}

fn build_pipeline(
    device: &DeviceContext,
    shaders: &ShaderSource,
    layout: vk::PipelineLayout,
    swapchain_format: vk::Format,
) -> RenderResult<GraphicsPipeline> {
    let vertex = shaders.module(device, "fullscreen.vert")?;
    let fragment = shaders.module(device, "composite.frag")?;
    let handle = pipeline::create_fullscreen(
        device,
        layout,
        vertex.handle(),
        fragment.handle(),
        swapchain_format,
    )?;
    Ok(GraphicsPipeline::from_raw(device.device.clone(), handle))
}

/// Whether stores to `format` are sRGB-encoded by the hardware.
fn is_srgb(format: vk::Format) -> bool {
    matches!(
        format,
        vk::Format::B8G8R8A8_SRGB
            | vk::Format::R8G8B8A8_SRGB
            | vk::Format::A8B8G8R8_SRGB_PACK32
            | vk::Format::B8G8R8_SRGB
            | vk::Format::R8G8B8_SRGB
    )
}

#[cfg(test)]
mod tests {
    use super::{CompositePush, is_srgb};
    use ash::vk;

    #[test]
    fn composite_push_matches_the_shader_block() {
        assert_eq!(core::mem::size_of::<CompositePush>(), 16);
        assert!(is_srgb(vk::Format::B8G8R8A8_SRGB));
        assert!(!is_srgb(vk::Format::B8G8R8A8_UNORM));
    }
}
