//! The in-house TAA/TAAU back end (ADR-0021 §3): `temporal_taau.slang`, one compute pass at
//! output resolution with RCAS fused, on every tiered device and in every golden.
//!
//! **Its own resources** (ADR-0021 amendment 2): the colour history (`RGBA16F` at output
//! resolution) and the previous linear depth (`R32F` at the maximum render size), each a
//! ping-pong pair indexed by the parity of the frames it has recorded. They live in `GENERAL`
//! from their first use and one global memory barrier per frame orders last frame's writes
//! before this frame's reads (and this frame's writes after last frame's reads). The back end
//! keeps a clone of the logical device for its descriptor writes, as every SDK back end
//! does.
//!
//! **Descriptors:** one set per frame-in-flight slot ([`SET_RING`] of them), chosen by
//! `UpscaleInputs::frame_slot` and rewritten before use. The renderer records a slot only
//! after waiting on that slot's fence, so a set is never rewritten while a submitted frame
//! reads it, however many recorded frames were dropped unsubmitted (the renderer asserts
//! `SET_RING >= FRAMES_IN_FLIGHT`).
//!
//! **Resize** keeps the pipeline, layouts, pool and sampler and re-creates only the four
//! images, so a mid-match window resize builds no pipeline.

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};

use super::{Extent, TemporalUpscaler, UpscaleInputs, UpscaleMode, UpscaleOutput, UpscalerKind};
use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuImage, GraphicsPipeline, PipelineLayout, Sampler,
};
use crate::shaders::ShaderSource;

/// Descriptor sets: one per frame-in-flight slot (asserted against `FRAMES_IN_FLIGHT` in the
/// renderer).
pub(crate) const SET_RING: usize = 2;
/// The colour history's format.
pub(crate) const HISTORY_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
/// The previous linear depth's format.
pub(crate) const PREV_DEPTH_FORMAT: vk::Format = vk::Format::R32_SFLOAT;
/// The pass's workgroup edge (output pixels).
const TILE: u32 = 16;

/// `push.flags` bits of `temporal_taau.slang`.
const FLAG_RESET: u32 = 1;
const FLAG_SHARPEN: u32 = 2;

/// `MOBA_DEBUG_VIEW` values TAAU draws itself (ADR-0021 §3), the shader's `flags >> 8`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaauDebugView {
    None = 0,
    Motion = 1,
    Reactive = 2,
    TcMask = 3,
    Disocclusion = 4,
    Confidence = 5,
}

impl TaauDebugView {
    /// TAAU's part of `view` (the overlay draws its own).
    #[must_use]
    pub(crate) const fn of(view: crate::post::TemporalDebugView) -> Self {
        use crate::post::TemporalDebugView as View;
        match view {
            View::Motion => Self::Motion,
            View::Reactive => Self::Reactive,
            View::TcMask => Self::TcMask,
            View::Disocclusion => Self::Disocclusion,
            View::Confidence => Self::Confidence,
            View::Off
            | View::OverlayIds
            | View::Albedo
            | View::Normal
            | View::Roughness
            | View::Metallic
            | View::Model
            | View::RimSlot
            | View::Flags => Self::None,
        }
    }
}

/// `TaauPush` in `temporal_taau.slang` (56 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct TaauPush {
    render_size: [u32; 2],
    output_size: [u32; 2],
    jitter: [f32; 2],
    prev_render_size: [u32; 2],
    history_scale: [f32; 2],
    p22: f32,
    p23: f32,
    sharpness: f32,
    flags: u32,
}

/// `(p22, p23)` with linear depth `p23 / (depth + p22)` for the upscaler inputs' reversed-Z
/// depth (1 near, 0 far, ADR-0023 §1c) with a finite far plane; `(0, 1)` (linear = 1/d, still
/// monotonic) without one.
#[must_use]
pub(crate) fn depth_linearisation(near: f32, far: f32) -> (f32, f32) {
    if near.is_finite() && far.is_finite() && near > 0.0 && far > near {
        let range = far - near;
        (near / range, near * far / range)
    } else {
        (0.0, 1.0)
    }
}

/// The TAAU back end.
pub(crate) struct Taau {
    device: ash::Device,
    pipeline: GraphicsPipeline,
    pipeline_layout: PipelineLayout,
    _set_layout: DescriptorSetLayout,
    _pool: DescriptorPool,
    sets: [vk::DescriptorSet; SET_RING],
    sampler: Sampler,
    /// Colour history pair, output resolution.
    history: [GpuImage; 2],
    /// Previous linear depth pair, the maximum render size.
    prev_depth: [GpuImage; 2],
    /// Which pair member this frame reads (it writes the other).
    parity: usize,
    /// The images are still `UNDEFINED`.
    fresh: bool,
    /// Forget the history at the next record.
    reset: bool,
    /// The previous frame's render size (its depth's valid region).
    prev_render: Extent,
    debug: TaauDebugView,
    vram: u64,
}

impl Taau {
    /// Creates the back end for `output` (the swapchain) with histories for render sizes up
    /// to `max_render` (the dynamic-resolution ceiling).
    #[allow(clippy::too_many_lines)] // Layout, pool, pipeline, sampler and images, in order.
    pub(crate) fn new(
        device: &DeviceContext,
        shaders: &ShaderSource,
        output: Extent,
        max_render: Extent,
        debug: TaauDebugView,
    ) -> RenderResult<Self> {
        let compute = vk::ShaderStageFlags::COMPUTE;
        let binding = |binding, kind| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(binding)
                .descriptor_type(kind)
                .descriptor_count(1)
                .stage_flags(compute)
        };
        let sampled = vk::DescriptorType::SAMPLED_IMAGE;
        let storage = vk::DescriptorType::STORAGE_IMAGE;
        let bindings = [
            binding(0, sampled),
            binding(1, sampled),
            binding(2, sampled),
            binding(3, sampled),
            binding(4, sampled),
            binding(5, sampled),
            binding(6, sampled),
            binding(7, storage),
            binding(8, storage),
            binding(9, sampled),
            binding(10, storage),
            binding(11, vk::DescriptorType::SAMPLER),
        ];
        let set_layout = DescriptorSetLayout::new(device.device.clone(), &bindings)?;
        let ring = u32::try_from(SET_RING).expect("small");
        let pool = DescriptorPool::new(
            device.device.clone(),
            &[
                vk::DescriptorPoolSize::default()
                    .ty(sampled)
                    .descriptor_count(8 * ring),
                vk::DescriptorPoolSize::default()
                    .ty(storage)
                    .descriptor_count(3 * ring),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLER)
                    .descriptor_count(ring),
            ],
            ring,
        )?;
        let allocated = pool.allocate(&device.device, &[set_layout.handle(); SET_RING])?;
        let sets = core::array::from_fn(|index| allocated[index]);
        let push = [vk::PushConstantRange::default()
            .stage_flags(compute)
            .offset(0)
            .size(u32::try_from(core::mem::size_of::<TaauPush>()).expect("small"))];
        let pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[set_layout.handle()],
            &push,
        )?;
        let pipeline = build_pipeline(device, shaders, pipeline_layout.handle())?;
        let sampler = Sampler::new(
            device.device.clone(),
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::LINEAR)
                .min_filter(vk::Filter::LINEAR)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
        )?;
        let (history, prev_depth, vram) = images(device, output, max_render)?;
        Ok(Self {
            device: device.device.clone(),
            pipeline,
            pipeline_layout,
            _set_layout: set_layout,
            _pool: pool,
            sets,
            sampler,
            history,
            prev_depth,
            parity: 0,
            fresh: true,
            reset: true,
            prev_render: max_render,
            debug,
            vram,
        })
    }

    /// Writes `set` for this frame's inputs and history parity.
    fn write_set(&self, set: vk::DescriptorSet, inputs: &UpscaleInputs, out: &UpscaleOutput) {
        let read = self.parity;
        let write = 1 - self.parity;
        let sampled = |view| {
            [vk::DescriptorImageInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
        };
        let general = |view| {
            [vk::DescriptorImageInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::GENERAL)]
        };
        let infos = [
            sampled(inputs.color.view),
            sampled(inputs.depth.view),
            sampled(inputs.motion.view),
            sampled(inputs.reactive.view),
            sampled(inputs.transparency.view),
            sampled(inputs.exposure.view),
            general(self.history[read].view()),
            general(self.history[write].view()),
            general(out.image.view),
            general(self.prev_depth[read].view()),
            general(self.prev_depth[write].view()),
        ];
        let sampler = [vk::DescriptorImageInfo::default().sampler(self.sampler.handle())];
        let kind = |binding: usize| match binding {
            7 | 8 | 10 => vk::DescriptorType::STORAGE_IMAGE,
            _ => vk::DescriptorType::SAMPLED_IMAGE,
        };
        let mut writes = [vk::WriteDescriptorSet::default(); 12];
        for (binding, info) in infos.iter().enumerate() {
            writes[binding] = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(u32::try_from(binding).expect("small"))
                .descriptor_type(kind(binding))
                .image_info(info);
        }
        writes[11] = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(11)
            .descriptor_type(vk::DescriptorType::SAMPLER)
            .image_info(&sampler);
        // SAFETY: the set belongs to this pool and layout; no submitted frame reads it (the
        // caller waited on its slot's fence); every view is live for this frame.
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
    }

    /// The barriers of the back end's own images before this frame's dispatch.
    fn barriers(&mut self, cmd: vk::CommandBuffer) {
        let images = [
            self.history[0].image(),
            self.history[1].image(),
            self.prev_depth[0].image(),
            self.prev_depth[1].image(),
        ];
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        if self.fresh {
            // First use, or a history reset: UNDEFINED → GENERAL (the reset below ignores the
            // contents). After a mid-stream reset the previous frame's dispatch may still be
            // reading and writing these images, so the transition waits for every earlier
            // compute work on the queue and its storage writes (WAR and WAW); at creation
            // there is none and the dependency is free.
            let barriers = images.map(|image| fresh_barrier(image, range));
            let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
            // SAFETY: the images are live and owned here; recording into `cmd`.
            unsafe { self.device.cmd_pipeline_barrier2(cmd, &dependency) };
            self.fresh = false;
            self.reset = true;
        } else {
            // Last frame's history writes before this frame's reads, and last frame's reads
            // before this frame's writes (same queue, earlier submission).
            let barrier = [vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
                .dst_access_mask(
                    vk::AccessFlags2::SHADER_SAMPLED_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE,
                )];
            let dependency = vk::DependencyInfo::default().memory_barriers(&barrier);
            // SAFETY: recording into `cmd`; a global memory barrier.
            unsafe { self.device.cmd_pipeline_barrier2(cmd, &dependency) };
        }
    }

    /// The push block for this frame.
    fn push(&self, inputs: &UpscaleInputs, out: &UpscaleOutput, reset: bool) -> TaauPush {
        let (p22, p23) = depth_linearisation(inputs.near, inputs.far);
        let sharpness = if inputs.sharpness.is_finite() {
            inputs.sharpness.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut flags = (self.debug as u32) << 8;
        if reset {
            flags |= FLAG_RESET;
        }
        if sharpness > 0.0 {
            flags |= FLAG_SHARPEN;
        }
        TaauPush {
            render_size: [inputs.render_size.width, inputs.render_size.height],
            output_size: [out.image.extent.width, out.image.extent.height],
            jitter: inputs.jitter,
            prev_render_size: [self.prev_render.width, self.prev_render.height],
            history_scale: [1.0, 1.0],
            p22,
            p23,
            sharpness,
            flags,
        }
    }
}

impl TemporalUpscaler for Taau {
    fn kind(&self) -> UpscalerKind {
        UpscalerKind::Taau
    }

    fn render_size_range(&self, output: Extent, mode: UpscaleMode) -> (Extent, Extent) {
        super::scaled_range(output, mode, crate::post::MIN_TEMPORAL_RENDER_SCALE)
    }

    fn jitter_phases(&self, render: Extent, output: Extent) -> u32 {
        crate::temporal::jitter_phases(render, output)
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
        self.barriers(cmd);
        let reset = self.reset || inputs.reset;
        let set = self.sets[inputs.frame_slot % SET_RING];
        self.write_set(set, inputs, out);
        let push = self.push(inputs, out, reset);
        let groups = [
            out.image.extent.width.div_ceil(TILE),
            out.image.extent.height.div_ceil(TILE),
        ];
        // SAFETY: the pipeline is live; recording into `cmd`.
        unsafe {
            self.device.cmd_bind_pipeline(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline.handle(),
            );
        }
        // SAFETY: the set matches the layout; the graph moved the inputs to
        // SHADER_READ_ONLY_OPTIMAL and the output to GENERAL, and `barriers` the history.
        unsafe {
            self.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_layout.handle(),
                0,
                &[set],
                &[],
            );
        }
        // SAFETY: the layout's push range is the compute stage's 56 bytes at offset 0.
        unsafe {
            self.device.cmd_push_constants(
                cmd,
                self.pipeline_layout.handle(),
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytes_of(&push),
            );
        }
        // SAFETY: the dispatch covers the output with the state bound above.
        unsafe { self.device.cmd_dispatch(cmd, groups[0], groups[1], 1) };
        self.parity = 1 - self.parity;
        self.reset = false;
        self.prev_render = inputs.render_size;
    }

    fn reset_history(&mut self) {
        // A first frame again: the images go UNDEFINED → GENERAL (always valid, contents
        // discarded), so a record that was never submitted leaves nothing assumed.
        self.fresh = true;
        self.reset = true;
    }

    unsafe fn resize(
        &mut self,
        device: &DeviceContext,
        output: Extent,
        max_render: Extent,
    ) -> RenderResult<()> {
        // Built before replacing anything, so a failed allocation keeps the old pair.
        let (history, prev_depth, vram) = images(device, output, max_render)?;
        self.history = history;
        self.prev_depth = prev_depth;
        self.vram = vram;
        self.prev_render = max_render;
        self.parity = 0;
        self.reset_history();
        Ok(())
    }

    fn vram_bytes(&self) -> u64 {
        self.vram
    }

    fn reload_shaders(
        &mut self,
        device: &DeviceContext,
        shaders: &ShaderSource,
    ) -> RenderResult<()> {
        self.pipeline = build_pipeline(device, shaders, self.pipeline_layout.handle())?;
        Ok(())
    }
}

fn build_pipeline(
    device: &DeviceContext,
    shaders: &ShaderSource,
    layout: vk::PipelineLayout,
) -> RenderResult<GraphicsPipeline> {
    let module = shaders.module(device, "temporal_taau.slang")?;
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

/// The colour history pair at `output` and the previous-depth pair at `max_render`, with
/// their device memory in bytes.
fn images(
    device: &DeviceContext,
    output: Extent,
    max_render: Extent,
) -> RenderResult<([GpuImage; 2], [GpuImage; 2], u64)> {
    let usage = vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED;
    let image = |name, extent: Extent, format| {
        GpuImage::new(
            device.device.clone(),
            device.allocator(),
            name,
            vk::Extent2D {
                width: extent.width.max(1),
                height: extent.height.max(1),
            },
            format,
            usage,
            vk::ImageAspectFlags::COLOR,
        )
    };
    let history = [
        image("taau-history-0", output, HISTORY_FORMAT)?,
        image("taau-history-1", output, HISTORY_FORMAT)?,
    ];
    let prev_depth = [
        image("taau-prev-depth-0", max_render, PREV_DEPTH_FORMAT)?,
        image("taau-prev-depth-1", max_render, PREV_DEPTH_FORMAT)?,
    ];
    // The allocations' sizes, as FSR 3.1 reports its own (alignment and padding included).
    let vram = history
        .iter()
        .chain(&prev_depth)
        .map(GpuImage::allocated_bytes)
        .sum();
    Ok((history, prev_depth, vram))
}

/// The transition of one history or previous-depth image on first use or after a history
/// reset: `UNDEFINED` → `GENERAL`. A layout transition is a write, and after a mid-stream
/// reset the previous frame's dispatch may still be reading and writing the image, so the
/// source is every earlier compute work on the queue and its storage writes (WAR and WAW),
/// never `NONE` (ADR-0021 amendment 2, item 28; `fresh_barrier_waits_for_earlier_compute`).
pub(crate) fn fresh_barrier(
    image: vk::Image,
    range: vk::ImageSubresourceRange,
) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .dst_access_mask(
            vk::AccessFlags2::SHADER_SAMPLED_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE,
        )
        .old_layout(vk::ImageLayout::UNDEFINED)
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
    fn fresh_barrier_waits_for_earlier_compute() {
        // A mid-stream reset re-initialises images the previous frame's dispatch may still
        // use: the transition must wait for it (amendment 2, item 28). Synchronization
        // validation on lavapipe does not catch a `NONE` source, so this pins it.
        let b = fresh_barrier(vk::Image::null(), vk::ImageSubresourceRange::default());
        assert!(
            b.src_stage_mask
                .contains(vk::PipelineStageFlags2::COMPUTE_SHADER)
        );
        assert!(
            b.src_access_mask
                .contains(vk::AccessFlags2::SHADER_STORAGE_WRITE)
        );
        assert_eq!(b.old_layout, vk::ImageLayout::UNDEFINED);
        assert_eq!(b.new_layout, vk::ImageLayout::GENERAL);
    }

    #[test]
    fn the_push_block_is_56_bytes() {
        assert_eq!(core::mem::size_of::<TaauPush>(), 56);
    }

    #[test]
    fn depth_linearisation_inverts_the_engine_projection() {
        let (near, far) = (0.1_f32, 1_000.0_f32);
        let (p22, p23) = depth_linearisation(near, far);
        // Reversed-Z depth of a point at view distance z (1 near, 0 far).
        for z in [0.5_f32, 10.0, 300.0] {
            let depth = 1.0 - (far / (far - near) - far * near / ((far - near) * z));
            let linear = p23 / (depth + p22);
            assert!((linear - z).abs() < z * 1e-3, "{linear} vs {z}");
        }
        assert_eq!(depth_linearisation(0.0, 0.0), (0.0, 1.0));
    }

    #[test]
    fn debug_views_parse() {
        use crate::post::TemporalDebugView as View;
        assert_eq!(View::from_name(" Motion "), Some(View::Motion));
        assert_eq!(TaauDebugView::of(View::TcMask), TaauDebugView::TcMask);
        assert_eq!(
            TaauDebugView::of(View::from_name("overlay-ids").expect("named")),
            TaauDebugView::None
        );
        for &view in View::ALL {
            assert_eq!(View::from_name(view.name()), Some(view));
            // ADR-0023 §1f: the G-buffer views are drawn by their own pass, never by TAAU.
            if view.gbuffer_view().is_some() {
                assert_eq!(TaauDebugView::of(view), TaauDebugView::None, "{view:?}");
            }
        }
        let gbuffer: Vec<u32> = View::ALL
            .iter()
            .filter_map(|view| view.gbuffer_view())
            .collect();
        assert_eq!(gbuffer, [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(View::from_name("rim-slot"), Some(View::RimSlot));
    }
}
