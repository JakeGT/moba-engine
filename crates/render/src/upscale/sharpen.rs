//! The renderer's `sharpen` pass (ADR-0021 §3, amendment 2 item 8): RCAS after a temporal back
//! end that does not sharpen itself (DLSS, whose own sharpening is deprecated, and XeSS).
//!
//! It wraps the back end as one more [`TemporalUpscaler`], so the frame graph keeps one
//! `temporal` pass whichever back end runs (amendment 5): the back end writes an intermediate
//! image this wrapper owns (`RGBA16F` at output resolution, in `GENERAL` from its first use),
//! then `sharpen.slang` reads it and writes `hdr_out`. The renderer wraps a back end only when
//! the sharpness is above 0 (the default for DLSS and XeSS is 0, so by default nothing is
//! added: the back end writes `hdr_out` directly).
//!
//! **Barriers** on the intermediate, the only resource this wrapper owns:
//! - first use, or the first record after a history reset: `UNDEFINED` → `GENERAL`, waiting on
//!   every earlier compute dispatch (a pending frame's `sharpen` may still be reading it; WAR,
//!   amendment 2 item 28), and on all earlier commands, since a vendor SDK may write it by any
//!   means;
//! - every other frame: the previous frame's `sharpen` read before this frame's back-end write
//!   (WAR);
//! - between the back end and `sharpen`: the back end's writes (any stage) before the compute
//!   read (RAW).
//!
//! **Descriptors:** one set per frame-in-flight slot, as TAAU's (`SET_RING`, asserted in the
//! renderer).

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};

use super::{
    Extent, TemporalUpscaler, UpscaleImage, UpscaleInputs, UpscaleMode, UpscaleOutput, UpscalerKind,
};
use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuImage, GraphicsPipeline, PipelineLayout,
};
use crate::shaders::ShaderSource;

/// Descriptor sets: one per frame-in-flight slot (asserted against `FRAMES_IN_FLIGHT` in the
/// renderer).
pub(crate) const SET_RING: usize = 2;
/// The intermediate's format: `hdr_out`'s.
const FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
/// The pass's workgroup edge.
const TILE: u32 = 16;

/// `SharpenPush` in `sharpen.slang` (16 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct SharpenPush {
    output_size: [u32; 2],
    sharpness: f32,
    pad: u32,
}

/// A back end followed by the `sharpen` pass.
pub(crate) struct Sharpened {
    inner: Box<dyn TemporalUpscaler>,
    device: ash::Device,
    pipeline: GraphicsPipeline,
    layout: PipelineLayout,
    _set_layout: DescriptorSetLayout,
    _pool: DescriptorPool,
    sets: [vk::DescriptorSet; SET_RING],
    /// The back end's output, `sharpen`'s input.
    intermediate: GpuImage,
    extent: Extent,
    /// The intermediate is `UNDEFINED` (first use, or after a reset or a resize).
    fresh: bool,
}

impl Sharpened {
    /// Wraps `inner` at `output`.
    ///
    /// # Errors
    /// A Vulkan or allocation error (the caller keeps `inner` unsharpened, or falls back).
    pub(crate) fn new(
        device: &DeviceContext,
        shaders: &ShaderSource,
        inner: Box<dyn TemporalUpscaler>,
        output: Extent,
    ) -> Result<Self, (Box<dyn TemporalUpscaler>, crate::error::RenderError)> {
        match Self::resources(device, shaders, output) {
            Ok((pipeline, layout, set_layout, pool, sets, intermediate)) => Ok(Self {
                inner,
                device: device.device.clone(),
                pipeline,
                layout,
                _set_layout: set_layout,
                _pool: pool,
                sets,
                intermediate,
                extent: output,
                fresh: true,
            }),
            Err(error) => Err((inner, error)),
        }
    }

    #[allow(clippy::type_complexity)] // The pieces `new` assembles.
    fn resources(
        device: &DeviceContext,
        shaders: &ShaderSource,
        output: Extent,
    ) -> RenderResult<(
        GraphicsPipeline,
        PipelineLayout,
        DescriptorSetLayout,
        DescriptorPool,
        [vk::DescriptorSet; SET_RING],
        GpuImage,
    )> {
        let compute = vk::ShaderStageFlags::COMPUTE;
        let binding = |index, ty| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(index)
                .descriptor_type(ty)
                .descriptor_count(1)
                .stage_flags(compute)
        };
        let bindings = [
            binding(0, vk::DescriptorType::SAMPLED_IMAGE),
            binding(1, vk::DescriptorType::SAMPLED_IMAGE),
            binding(2, vk::DescriptorType::STORAGE_IMAGE),
        ];
        let set_layout = DescriptorSetLayout::new(device.device.clone(), &bindings)?;
        let ring = u32::try_from(SET_RING).expect("small");
        let pool = DescriptorPool::new(
            device.device.clone(),
            &[
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(2 * ring),
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
            .size(u32::try_from(core::mem::size_of::<SharpenPush>()).expect("small"))];
        let layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[set_layout.handle()],
            &push,
        )?;
        let pipeline = build_pipeline(device, shaders, layout.handle())?;
        let intermediate = intermediate(device, output)?;
        Ok((pipeline, layout, set_layout, pool, sets, intermediate))
    }

    fn write_set(&self, set: vk::DescriptorSet, exposure: vk::ImageView, out: vk::ImageView) {
        let source = [vk::DescriptorImageInfo::default()
            .image_view(self.intermediate.view())
            .image_layout(vk::ImageLayout::GENERAL)];
        let exposure = [vk::DescriptorImageInfo::default()
            .image_view(exposure)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let output = [vk::DescriptorImageInfo::default()
            .image_view(out)
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
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&exposure),
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(2)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&output),
        ];
        // SAFETY: no submitted frame reads the set (the slot's fence was waited on, the
        // `record` contract) and the views are live in the layouts named.
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
    }
}

impl TemporalUpscaler for Sharpened {
    fn kind(&self) -> UpscalerKind {
        self.inner.kind()
    }

    fn render_size_range(&self, output: Extent, mode: UpscaleMode) -> (Extent, Extent) {
        self.inner.render_size_range(output, mode)
    }

    fn jitter_phases(&self, render: Extent, output: Extent) -> u32 {
        self.inner.jitter_phases(render, output)
    }

    fn has_sharpening(&self) -> bool {
        true
    }

    unsafe fn record(
        &mut self,
        cmd: vk::CommandBuffer,
        inputs: &UpscaleInputs,
        out: &UpscaleOutput,
    ) {
        let range = color_range();
        let before = if self.fresh {
            fresh_barrier(self.intermediate.image(), range)
        } else {
            reuse_barrier(self.intermediate.image(), range)
        };
        // SAFETY: the intermediate is owned here and live; recording into `cmd`.
        unsafe {
            self.device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&[before]),
            );
        }
        self.fresh = false;
        let staged = UpscaleOutput {
            image: UpscaleImage {
                image: self.intermediate.image(),
                view: self.intermediate.view(),
                format: FORMAT,
                extent: self.extent,
            },
        };
        // SAFETY: the caller's contract is forwarded unchanged (same command buffer, inputs,
        // slot and submission rules); the intermediate stays alive with `self` and is in
        // GENERAL, as an upscaler's output must be.
        unsafe { self.inner.record(cmd, inputs, &staged) };
        let after = written_barrier(self.intermediate.image(), range);
        // SAFETY: as above.
        unsafe {
            self.device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&[after]),
            );
        }
        let set = self.sets[inputs.frame_slot % SET_RING];
        self.write_set(set, inputs.exposure.view, out.image.view);
        let size = out.image.extent;
        let push = SharpenPush {
            output_size: [size.width, size.height],
            sharpness: inputs.sharpness.clamp(0.0, 1.0),
            pad: 0,
        };
        // SAFETY: the pipeline is live; recording into `cmd`.
        unsafe {
            self.device.cmd_bind_pipeline(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline.handle(),
            );
        }
        // SAFETY: the set matches the layout's only set; the graph moved the exposure to
        // SHADER_READ_ONLY_OPTIMAL and the output to GENERAL, and the barrier above the
        // intermediate.
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
        // SAFETY: the layout's push range is the compute stage's 16 bytes at offset 0.
        unsafe {
            self.device.cmd_push_constants(
                cmd,
                self.layout.handle(),
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytes_of(&push),
            );
        }
        // SAFETY: the dispatch covers the output with the state bound above.
        unsafe {
            self.device.cmd_dispatch(
                cmd,
                size.width.div_ceil(TILE),
                size.height.div_ceil(TILE),
                1,
            );
        }
    }

    fn reset_history(&mut self) {
        // A record that was never submitted may have left the first-use transition unrun.
        self.fresh = true;
        self.inner.reset_history();
    }

    fn resize(
        &mut self,
        device: &DeviceContext,
        output: Extent,
        max_render: Extent,
    ) -> RenderResult<()> {
        // Built before replacing anything, so a failed allocation keeps the old image.
        let image = intermediate(device, output)?;
        self.inner.resize(device, output, max_render)?;
        self.intermediate = image;
        self.extent = output;
        self.fresh = true;
        Ok(())
    }

    fn vram_bytes(&self) -> u64 {
        self.inner.vram_bytes() + u64::from(self.extent.width) * u64::from(self.extent.height) * 8
    }

    fn sdk_bytes(&self) -> u64 {
        self.inner.sdk_bytes()
    }

    fn reload_shaders(
        &mut self,
        device: &DeviceContext,
        shaders: &ShaderSource,
    ) -> RenderResult<()> {
        self.pipeline = build_pipeline(device, shaders, self.layout.handle())?;
        self.inner.reload_shaders(device, shaders)
    }
}

fn build_pipeline(
    device: &DeviceContext,
    shaders: &ShaderSource,
    layout: vk::PipelineLayout,
) -> RenderResult<GraphicsPipeline> {
    let module = shaders.module(device, "sharpen.slang")?;
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

fn intermediate(device: &DeviceContext, output: Extent) -> RenderResult<GpuImage> {
    GpuImage::new(
        device.device.clone(),
        device.allocator(),
        "sharpen-input",
        vk::Extent2D {
            width: output.width.max(1),
            height: output.height.max(1),
        },
        FORMAT,
        // The back end writes here: TRANSFER_DST in a vendor build (NGX may clear its output,
        // ADR-0021 amendment 5).
        vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED | super::VENDOR_OUTPUT_USAGE,
        vk::ImageAspectFlags::COLOR,
    )
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}

/// Every stage a vendor SDK may write its output from (compute in practice; NGX and the
/// FidelityFX and XeSS runtimes do not document it), and what it may write with.
const BACK_END_STAGES: vk::PipelineStageFlags2 = vk::PipelineStageFlags2::ALL_COMMANDS;
const BACK_END_WRITES: vk::AccessFlags2 = vk::AccessFlags2::MEMORY_WRITE;

/// The intermediate's transition on first use or after a reset: `UNDEFINED` → `GENERAL`. A
/// layout transition is a write, and a pending frame's `sharpen` may still be reading the
/// image (and its back end writing it), so the source is every earlier command and its writes,
/// never `NONE` (ADR-0021 amendment 2, item 28).
pub(crate) fn fresh_barrier(
    image: vk::Image,
    range: vk::ImageSubresourceRange,
) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .src_stage_mask(BACK_END_STAGES | vk::PipelineStageFlags2::COMPUTE_SHADER)
        .src_access_mask(BACK_END_WRITES)
        .dst_stage_mask(BACK_END_STAGES)
        .dst_access_mask(BACK_END_WRITES | vk::AccessFlags2::MEMORY_READ)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::GENERAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(range)
}

/// Before the back end writes the intermediate again: the previous frame's `sharpen` read
/// (compute) happens first (WAR).
pub(crate) fn reuse_barrier(
    image: vk::Image,
    range: vk::ImageSubresourceRange,
) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(BACK_END_STAGES)
        .dst_access_mask(BACK_END_WRITES | vk::AccessFlags2::MEMORY_READ)
        .old_layout(vk::ImageLayout::GENERAL)
        .new_layout(vk::ImageLayout::GENERAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(range)
}

/// After the back end: its writes, from any stage, are visible to `sharpen`'s compute read
/// (RAW).
pub(crate) fn written_barrier(
    image: vk::Image,
    range: vk::ImageSubresourceRange,
) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .src_stage_mask(BACK_END_STAGES)
        .src_access_mask(BACK_END_WRITES)
        .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
        .old_layout(vk::ImageLayout::GENERAL)
        .new_layout(vk::ImageLayout::GENERAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(range)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_push_block_is_16_bytes() {
        assert_eq!(core::mem::size_of::<SharpenPush>(), 16);
    }

    /// A reset re-initialises an image a pending frame's `sharpen` may still read and its
    /// back end write: the transition waits for both (amendment 2, item 28; synchronization
    /// validation does not catch a missing source stage on lavapipe, RTX or RADV).
    #[test]
    fn fresh_barrier_waits_for_earlier_compute_and_back_end_writes() {
        let b = fresh_barrier(vk::Image::null(), color_range());
        assert!(
            b.src_stage_mask
                .contains(vk::PipelineStageFlags2::COMPUTE_SHADER)
        );
        assert!(
            b.src_stage_mask
                .contains(vk::PipelineStageFlags2::ALL_COMMANDS)
        );
        assert!(b.src_access_mask.contains(vk::AccessFlags2::MEMORY_WRITE));
        assert_eq!(b.old_layout, vk::ImageLayout::UNDEFINED);
        assert_eq!(b.new_layout, vk::ImageLayout::GENERAL);
    }

    /// The back end's next write waits for the previous frame's `sharpen` read (WAR), and
    /// `sharpen` waits for the back end's write from whatever stage it used (RAW).
    #[test]
    fn steady_barriers_order_the_back_end_and_sharpen() {
        let reuse = reuse_barrier(vk::Image::null(), color_range());
        assert!(
            reuse
                .src_stage_mask
                .contains(vk::PipelineStageFlags2::COMPUTE_SHADER)
        );
        assert!(
            reuse
                .dst_stage_mask
                .contains(vk::PipelineStageFlags2::ALL_COMMANDS)
        );
        assert_eq!(reuse.old_layout, vk::ImageLayout::GENERAL);
        let written = written_barrier(vk::Image::null(), color_range());
        assert!(
            written
                .src_stage_mask
                .contains(vk::PipelineStageFlags2::ALL_COMMANDS)
        );
        assert!(
            written
                .src_access_mask
                .contains(vk::AccessFlags2::MEMORY_WRITE)
        );
        assert!(
            written
                .dst_access_mask
                .contains(vk::AccessFlags2::SHADER_SAMPLED_READ)
        );
    }
}
