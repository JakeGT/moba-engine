//! `gbuffer_debug` (ADR-0023 §1f): `MOBA_DEBUG_VIEW=albedo|normal|roughness|metallic|model|
//! rim-slot|flags` decodes one G-buffer channel (`gbuffer_debug.slang`, the Slang
//! `decode_gbuffer`) over the upscaler's output, after the `temporal` pass. The frame graph
//! has the pass only when such a view is selected, so a normal frame never pays for it.

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};

use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::resources::{DescriptorPool, DescriptorSetLayout, GraphicsPipeline, PipelineLayout};
use crate::shaders::ShaderSource;

/// Descriptor sets: one per frame-in-flight slot.
const SET_RING: usize = crate::renderer::FRAMES_IN_FLIGHT;
/// `numthreads` of `gbuffer_debug.slang`.
const GROUP: u32 = 8;

/// `DebugPush` in `gbuffer_debug.slang` (24 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct DebugPush {
    render_size: [u32; 2],
    output_size: [u32; 2],
    view: u32,
    reserved: u32,
}

/// The views one frame's pass reads and writes.
pub(crate) struct GBufferDebugTargets {
    /// `gb0`–`gb2`, in `SHADER_READ_ONLY_OPTIMAL`.
    pub(crate) gbuffer: [vk::ImageView; 3],
    /// The 1×1 linear exposure, in `SHADER_READ_ONLY_OPTIMAL`.
    pub(crate) exposure: vk::ImageView,
    /// `hdr_out`, in `GENERAL`.
    pub(crate) output: vk::ImageView,
    pub(crate) render_size: vk::Extent2D,
    pub(crate) output_size: vk::Extent2D,
}

/// The `gbuffer_debug` pass.
pub(crate) struct GBufferDebugPass {
    device: ash::Device,
    pipeline: GraphicsPipeline,
    layout: PipelineLayout,
    _set_layout: DescriptorSetLayout,
    _pool: DescriptorPool,
    sets: [vk::DescriptorSet; SET_RING],
    /// `push.view` (1–7, [`crate::post::TemporalDebugView::gbuffer_view`]).
    view: u32,
}

impl GBufferDebugPass {
    /// The pass for `view` (1–7).
    pub(crate) fn new(
        device: &DeviceContext,
        shaders: &ShaderSource,
        view: u32,
    ) -> RenderResult<Self> {
        let compute = vk::ShaderStageFlags::COMPUTE;
        let binding = |binding, ty| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(binding)
                .descriptor_type(ty)
                .descriptor_count(1)
                .stage_flags(compute)
        };
        let sampled = vk::DescriptorType::SAMPLED_IMAGE;
        let bindings = [
            binding(0, sampled),
            binding(1, sampled),
            binding(2, sampled),
            binding(3, sampled),
            binding(4, vk::DescriptorType::STORAGE_IMAGE),
        ];
        let set_layout = DescriptorSetLayout::new(device.device.clone(), &bindings)?;
        let ring = u32::try_from(SET_RING).expect("small");
        let pool = DescriptorPool::new(
            device.device.clone(),
            &[
                vk::DescriptorPoolSize::default()
                    .ty(sampled)
                    .descriptor_count(4 * ring),
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
            .size(u32::try_from(core::mem::size_of::<DebugPush>()).expect("small"))];
        let layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[set_layout.handle()],
            &push,
        )?;
        let module = shaders.module(device, "gbuffer_debug.slang")?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(compute)
            .module(module.handle())
            .name(c"main");
        let handle = device.create_compute_pipeline(
            &vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(layout.handle()),
        )?;
        Ok(Self {
            device: device.device.clone(),
            pipeline: GraphicsPipeline::from_raw(device.device.clone(), handle),
            layout,
            _set_layout: set_layout,
            _pool: pool,
            sets,
            view,
        })
    }

    /// Records the pass into `cmd` for frame-in-flight slot `frame_slot` (its fence was
    /// waited on, so its descriptor set is free).
    pub(crate) fn record(
        &self,
        cmd: vk::CommandBuffer,
        frame_slot: usize,
        targets: &GBufferDebugTargets,
    ) {
        let set = self.sets[frame_slot % SET_RING];
        let read = |view| {
            [vk::DescriptorImageInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
        };
        let inputs = [
            read(targets.gbuffer[0]),
            read(targets.gbuffer[1]),
            read(targets.gbuffer[2]),
            read(targets.exposure),
        ];
        let output = [vk::DescriptorImageInfo::default()
            .image_view(targets.output)
            .image_layout(vk::ImageLayout::GENERAL)];
        let mut writes: Vec<vk::WriteDescriptorSet<'_>> = inputs
            .iter()
            .enumerate()
            .map(|(binding, info)| {
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(u32::try_from(binding).expect("small"))
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(info)
            })
            .collect();
        writes.push(
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(4)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&output),
        );
        let push = DebugPush {
            render_size: [targets.render_size.width, targets.render_size.height],
            output_size: [targets.output_size.width, targets.output_size.height],
            view: self.view,
            reserved: 0,
        };
        // SAFETY: no submitted frame reads this slot's set (its fence was waited on), and every
        // view is live in the layout named.
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
        // SAFETY: the pipeline is live; recording into `cmd`.
        unsafe {
            self.device.cmd_bind_pipeline(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline.handle(),
            );
        }
        // SAFETY: the layout and set are live and the set was written above.
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
        // SAFETY: the range lies in the layout's compute push-constant range.
        unsafe {
            self.device.cmd_push_constants(
                cmd,
                self.layout.handle(),
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytes_of(&push),
            );
        }
        let groups = |size: u32| size.div_ceil(GROUP);
        // SAFETY: the pipeline, set and push constants are bound above.
        unsafe {
            self.device.cmd_dispatch(
                cmd,
                groups(targets.output_size.width),
                groups(targets.output_size.height),
                1,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DebugPush;

    /// `DebugPush` matches the Slang `DebugPush` the reflection reports (ADR-0020 §4c).
    #[test]
    fn push_matches_the_slang_layout() {
        let json = moba_shaders::reflection("gbuffer_debug.slang").expect("reflection");
        let reflection: serde_json::Value = serde_json::from_str(json).expect("json");
        let push = reflection["parameters"]
            .as_array()
            .expect("parameters")
            .iter()
            .find(|parameter| parameter["name"] == "push")
            .expect("push");
        let fields = push["type"]["elementType"]["fields"]
            .as_array()
            .expect("fields");
        let offsets: Vec<(String, u64)> = fields
            .iter()
            .map(|field| {
                (
                    field["name"].as_str().expect("name").to_owned(),
                    field["binding"]["offset"].as_u64().expect("offset"),
                )
            })
            .collect();
        let expected = [
            ("render_size", core::mem::offset_of!(DebugPush, render_size)),
            ("output_size", core::mem::offset_of!(DebugPush, output_size)),
            ("view", core::mem::offset_of!(DebugPush, view)),
            ("reserved", core::mem::offset_of!(DebugPush, reserved)),
        ]
        .map(|(name, offset)| (name.to_owned(), offset as u64));
        assert_eq!(offsets, expected);
        assert_eq!(core::mem::size_of::<DebugPush>(), 24);
    }
}
