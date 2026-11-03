//! `exposure_linear` (ADR-0021 §4a): one compute invocation writing the 1×1 `R32_SFLOAT`
//! linear exposure multiplier every upscaler reads, `exposure × 2^(adapted EV)`, the value the
//! composite applies (`exposure_linear.slang`).
//!
//! Without auto exposure there is no adapted-EV image; binding 0 then names a 1×1 image this
//! pass owns, kept in `SHADER_READ_ONLY_OPTIMAL` and never read (the push says so).

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};

use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuImage, GraphicsPipeline, PipelineLayout,
};
use crate::shaders::ShaderSource;

/// Descriptor sets in the ring (as TAAU's: longer than the frames in flight, asserted in the
/// renderer).
pub(crate) const SET_RING: usize = 3;

/// `ExposurePush` in `exposure_linear.slang`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ExposurePush {
    exposure: f32,
    auto_exposure: u32,
}

/// The `exposure_linear` pass.
pub(crate) struct ExposureLinearPass {
    device: ash::Device,
    pipeline: GraphicsPipeline,
    layout: PipelineLayout,
    _set_layout: DescriptorSetLayout,
    _pool: DescriptorPool,
    sets: [vk::DescriptorSet; SET_RING],
    next: usize,
    /// Binding 0 without auto exposure.
    placeholder: GpuImage,
    placeholder_ready: bool,
}

impl ExposureLinearPass {
    pub(crate) fn new(device: &DeviceContext, shaders: &ShaderSource) -> RenderResult<Self> {
        let compute = vk::ShaderStageFlags::COMPUTE;
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(compute),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(compute),
        ];
        let set_layout = DescriptorSetLayout::new(device.device.clone(), &bindings)?;
        let ring = u32::try_from(SET_RING).expect("small");
        let pool = DescriptorPool::new(
            device.device.clone(),
            &[
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(ring),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_IMAGE)
                    .descriptor_count(ring),
            ],
            ring,
        )?;
        let allocated = pool.allocate(&device.device, &[set_layout.handle(); SET_RING])?;
        let sets = core::array::from_fn(|index| allocated[index]);
        let push = [vk::PushConstantRange::default()
            .stage_flags(compute)
            .offset(0)
            .size(u32::try_from(core::mem::size_of::<ExposurePush>()).expect("small"))];
        let layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[set_layout.handle()],
            &push,
        )?;
        let pipeline = Self::build(device, shaders, layout.handle())?;
        let placeholder = GpuImage::new(
            device.device.clone(),
            device.allocator(),
            "exposure-linear-placeholder",
            vk::Extent2D {
                width: 1,
                height: 1,
            },
            vk::Format::R32_SFLOAT,
            vk::ImageUsageFlags::SAMPLED,
            vk::ImageAspectFlags::COLOR,
        )?;
        Ok(Self {
            device: device.device.clone(),
            pipeline,
            layout,
            _set_layout: set_layout,
            _pool: pool,
            sets,
            next: 0,
            placeholder,
            placeholder_ready: false,
        })
    }

    fn build(
        device: &DeviceContext,
        shaders: &ShaderSource,
        layout: vk::PipelineLayout,
    ) -> RenderResult<GraphicsPipeline> {
        let module = shaders.module(device, "exposure_linear.slang")?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module.handle())
            .name(c"main");
        let handle = device.create_compute_pipeline(
            &vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(layout),
        )?;
        Ok(GraphicsPipeline::from_raw(device.device.clone(), handle))
    }

    /// Debug builds' shader hot reload: a new pipeline from `shaders` (installed with
    /// [`Self::install_pipeline`]).
    pub(crate) fn build_pipeline(
        &self,
        device: &DeviceContext,
        shaders: &ShaderSource,
    ) -> RenderResult<GraphicsPipeline> {
        Self::build(device, shaders, self.layout.handle())
    }

    /// Replaces the pipeline (the device is idle).
    pub(crate) fn install_pipeline(&mut self, pipeline: GraphicsPipeline) {
        self.pipeline = pipeline;
    }

    /// A recorded frame was never submitted: the placeholder's first-use transition may not
    /// have run, so the next record transitions it again (UNDEFINED → read-only is always
    /// valid for an image never written).
    pub(crate) fn forget(&mut self) {
        self.placeholder_ready = false;
    }

    /// Records the pass: `adapted` is the persistent adapted-EV image's view when auto
    /// exposure is on (the graph moved it to `SHADER_READ_ONLY_OPTIMAL`), `target` the
    /// `exposure_linear` transient (in `GENERAL`).
    pub(crate) fn record(
        &mut self,
        cmd: vk::CommandBuffer,
        adapted: Option<vk::ImageView>,
        target: vk::ImageView,
        exposure: f32,
    ) {
        if adapted.is_none() && !self.placeholder_ready {
            let barrier = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::NONE)
                .src_access_mask(vk::AccessFlags2::NONE)
                .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(self.placeholder.image())
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                )];
            // SAFETY: the placeholder is live and owned here; recording into `cmd`.
            unsafe {
                self.device.cmd_pipeline_barrier2(
                    cmd,
                    &vk::DependencyInfo::default().image_memory_barriers(&barrier),
                );
            }
            self.placeholder_ready = true;
        }
        let set = self.sets[self.next];
        self.next = (self.next + 1) % SET_RING;
        let source = [vk::DescriptorImageInfo::default()
            .image_view(adapted.unwrap_or_else(|| self.placeholder.view()))
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let output = [vk::DescriptorImageInfo::default()
            .image_view(target)
            .image_layout(vk::ImageLayout::GENERAL)];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&source),
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&output),
        ];
        let push = ExposurePush {
            exposure,
            auto_exposure: u32::from(adapted.is_some()),
        };
        // SAFETY: no submitted frame reads the set (the ring is longer than the frames in
        // flight) and both views are live in the layouts named.
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
        // SAFETY: the pipeline is live; recording into `cmd`.
        unsafe {
            self.device.cmd_bind_pipeline(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline.handle(),
            );
        }
        // SAFETY: the set matches the layout's only set.
        unsafe {
            self.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.layout.handle(),
                0,
                &[set],
                &[],
            );
        }
        // SAFETY: the layout's push range is the compute stage's 8 bytes at offset 0.
        unsafe {
            self.device.cmd_push_constants(
                cmd,
                self.layout.handle(),
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytes_of(&push),
            );
        }
        // SAFETY: one invocation with the pipeline, set and push constants bound above.
        unsafe { self.device.cmd_dispatch(cmd, 1, 1, 1) };
    }
}
