//! Screen-space outline pass (R2b, ADR-0016): after the tonemap and before the UI, dilates
//! the depth pre-pass's `R8_UINT` outline IDs by each palette slot's width and alpha-blends
//! the slot colours over the swapchain image, so the palette (standard or colour-blind)
//! reaches the screen exactly.

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};

use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::pipeline;
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GraphicsPipeline, PipelineLayout, Sampler,
};
use crate::scene::{OUTLINE_SLOTS, OutlinePalette};
use crate::shaders::ShaderSource;

/// Push-constant block of `outline.frag`: the palette (128 bytes, fragment stage). Slot 0
/// carries the widest width and whether the target encodes sRGB on store.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct OutlinePush {
    palette: [[f32; 4]; OUTLINE_SLOTS],
}
const OUTLINE_PUSH_SIZE: u32 = core::mem::size_of::<OutlinePush>() as u32;

/// The blended fullscreen pipeline and the set naming the ID target.
pub(crate) struct OutlinePass {
    set: vk::DescriptorSet,
    _descriptor_pool: DescriptorPool,
    _set_layout: DescriptorSetLayout,
    sampler: Sampler,
    pipeline_layout: PipelineLayout,
    pipeline: GraphicsPipeline,
    swapchain_format: vk::Format,
}

impl OutlinePass {
    pub(crate) fn new(
        device: &DeviceContext,
        shaders: &ShaderSource,
        swapchain_format: vk::Format,
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
        // Integer IDs: nearest only (`texelFetch` ignores it anyway).
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
            .size(OUTLINE_PUSH_SIZE)];
        let pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[set_layout.handle()],
            &push_range,
        )?;
        let pipeline = build_pipeline(device, shaders, pipeline_layout.handle(), swapchain_format)?;
        Ok(Self {
            set,
            _descriptor_pool: descriptor_pool,
            _set_layout: set_layout,
            sampler,
            pipeline_layout,
            pipeline,
            swapchain_format,
        })
    }

    /// Builds a replacement pipeline (dev shader reload) without installing it.
    pub(crate) fn build_pipeline(
        &self,
        device: &DeviceContext,
        shaders: &ShaderSource,
    ) -> RenderResult<GraphicsPipeline> {
        build_pipeline(
            device,
            shaders,
            self.pipeline_layout.handle(),
            self.swapchain_format,
        )
    }

    /// Installs a pipeline from [`Self::build_pipeline`]. The device must be idle.
    pub(crate) fn install_pipeline(&mut self, pipeline: GraphicsPipeline) {
        self.pipeline = pipeline;
    }

    /// Points the set at the ID target. Only while no submitted frame uses the set.
    pub(crate) fn bind_ids(&self, device: &DeviceContext, ids: vk::ImageView) {
        let image_info = [vk::DescriptorImageInfo::default()
            .sampler(self.sampler.handle())
            .image_view(ids)
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

    /// Records the outlines over `target` (loaded), or nothing when every slot is off.
    pub(crate) fn record(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        target: vk::ImageView,
        extent: vk::Extent2D,
        palette: &OutlinePalette,
    ) {
        let push = push_block(palette, super::composite::is_srgb(self.swapchain_format));
        if push.palette[0][0] <= 0.0 {
            return;
        }
        super::begin_color_only(
            device,
            command_buffer,
            target,
            extent,
            vk::AttachmentLoadOp::LOAD,
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
        // SAFETY: the set matches set 0 of the layout and names the live ID target, which the
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
        // SAFETY: the layout declares exactly this 128-byte fragment-stage range at offset 0.
        unsafe {
            device.device.cmd_push_constants(
                command_buffer,
                self.pipeline_layout.handle(),
                vk::ShaderStageFlags::FRAGMENT,
                0,
                bytes_of(&push),
            );
        }
        // SAFETY: the fullscreen vertex shader reads no vertex buffers.
        unsafe { device.device.cmd_draw(command_buffer, 3, 1, 0, 0) };
        device.cmd_end_rendering(command_buffer);
    }
}

/// The push block for `palette`: the sanitised slots, slot 0 = (widest width, sRGB flag).
fn push_block(palette: &OutlinePalette, srgb: bool) -> OutlinePush {
    let mut slots = palette.sanitized();
    slots[0] = [palette.max_width(), f32::from(u8::from(srgb)), 0.0, 0.0];
    OutlinePush { palette: slots }
}

fn build_pipeline(
    device: &DeviceContext,
    shaders: &ShaderSource,
    layout: vk::PipelineLayout,
    swapchain_format: vk::Format,
) -> RenderResult<GraphicsPipeline> {
    let vertex = shaders.module(device, "fullscreen.vert")?;
    let fragment = shaders.module(device, "outline.frag")?;
    let handle = pipeline::create_fullscreen_blend(
        device,
        layout,
        vertex.handle(),
        fragment.handle(),
        swapchain_format,
        true,
    )?;
    Ok(GraphicsPipeline::from_raw(device.device.clone(), handle))
}

#[cfg(test)]
mod tests {
    use super::{OutlinePush, push_block};
    use crate::scene::OutlinePalette;
    use moba_core::math::Vec3;

    #[test]
    fn the_push_block_is_the_shader_palette() {
        assert_eq!(core::mem::size_of::<OutlinePush>(), 128);
        let palette = OutlinePalette::NONE
            .with_slot(1, Vec3::new(0.1, 0.2, 1.0), 1.5)
            .with_slot(6, Vec3::ONE, 3.0);
        let push = push_block(&palette, true);
        assert_eq!(push.palette[0], [3.0, 1.0, 0.0, 0.0]);
        assert_eq!(push.palette[1], [0.1, 0.2, 1.0, 1.5]);
        assert_eq!(push_block(&OutlinePalette::NONE, false).palette[0][0], 0.0);
    }
}
