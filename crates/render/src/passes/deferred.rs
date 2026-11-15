//! The deferred lighting on Tiered (ADR-0023 §2, N2b): `light_classify` sorts the rendered
//! sub-rectangle's 16 × 16 tiles by the shading models they hold (`light_classify.slang`),
//! then `deferred_light` lights each class with one indirect dispatch of its permutation
//! (`deferred_light.comp`, built from the forward path's `lighting.glsl`, so a deferred pixel
//! evaluates the same lights, shadows, ambient, AO and fog as a forward one at the same
//! point). The G-buffer pass wrote the emissive into `hdr`; the lighting replaces it with the
//! lit, fogged surface. The sky tiles are not dispatched: the `sky` pass draws them.
//!
//! The tile buffer ([`TileLayout`]) holds the three classes' `VkDispatchIndirectCommand`s,
//! their tile lists and a per-tile info triple for the debug views. `mask_clear` resets the
//! arguments each frame ([`TileLayout::RESET`]).

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};
use gpu_allocator::MemoryLocation;

use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuBuffer, GraphicsPipeline, PipelineLayout,
};
use crate::shaders::ShaderSource;

/// Descriptor sets: one per frame-in-flight slot.
const SET_RING: usize = crate::renderer::FRAMES_IN_FLIGHT;
/// The tile edge, in pixels (`numthreads` of both shaders).
pub(crate) const TILE: u32 = 16;

/// The tile classes `light_classify` sorts into (`CLASS_*` in `light_classify.slang`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum TileClass {
    /// Every pixel at the far plane: not lit (the `sky` pass draws it), so only the shaders
    /// and the tests name it.
    #[cfg_attr(not(test), allow(dead_code))]
    Sky = 0,
    /// Only `STANDARD` and `UNLIT` pixels: the fast permutation.
    Standard = 1,
    /// Any other model: the full permutation.
    Extended = 2,
}

/// The tile buffer's layout (`light_classify.slang`, `deferred_light.comp`), in 32-bit words:
/// the three classes' dispatch arguments (`x, y, z` each) and padding, then the three lists
/// of `capacity` packed tiles (`x | y << 16`), then `capacity` info triples (model mask, the
/// far and the near depth as float bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TileLayout {
    /// Tiles per row of the largest rendered sub-rectangle.
    pub(crate) tiles_x: u32,
    /// Tiles each list holds: every tile of the largest rendered sub-rectangle.
    pub(crate) capacity: u32,
}

impl TileLayout {
    /// Words before the lists (`TILE_HEADER_WORDS` in `light_classify.slang`).
    pub(crate) const HEADER_WORDS: u32 = 12;
    /// What `mask_clear` writes over the header each frame: every class's dispatch is
    /// `(0, 1, 1)`, so `light_classify`'s counts start at 0.
    pub(crate) const RESET: [u32; 12] = [0, 1, 1, 0, 1, 1, 0, 1, 1, 0, 0, 0];

    /// The layout for rendered sub-rectangles up to `extent` (the swapchain's).
    pub(crate) fn for_extent(extent: vk::Extent2D) -> Self {
        let tiles_x = extent.width.max(1).div_ceil(TILE);
        let tiles_y = extent.height.max(1).div_ceil(TILE);
        Self {
            tiles_x,
            capacity: tiles_x * tiles_y,
        }
    }

    /// The buffer's size in bytes.
    pub(crate) fn bytes(self) -> u64 {
        4 * (u64::from(Self::HEADER_WORDS) + 6 * u64::from(self.capacity))
    }

    /// The byte offset of `class`'s `VkDispatchIndirectCommand`.
    pub(crate) const fn args_offset(class: TileClass) -> u64 {
        12 * class as u64
    }

    /// The word offset of `class`'s tile list.
    pub(crate) const fn list_offset(self, class: TileClass) -> u32 {
        Self::HEADER_WORDS + class as u32 * self.capacity
    }

    /// The word offset of the per-tile info (`light_classify` writes it for a debug reader).
    #[cfg(test)]
    pub(crate) const fn info_offset(self) -> u32 {
        Self::HEADER_WORDS + 3 * self.capacity
    }
}

/// `ClassifyPush` in `light_classify.slang` (16 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ClassifyPush {
    render_size: [u32; 2],
    tiles_x: u32,
    capacity: u32,
}

/// `DeferredPush` in `deferred_light.comp` (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct DeferredPush {
    render_size: [u32; 2],
    list_offset: u32,
    debug: u32,
    inv_exposure: f32,
    tile_class: u32,
    reserved0: u32,
    reserved1: u32,
}

/// The views and sets one frame's passes read and write.
pub(crate) struct DeferredTargets {
    /// `gb0`–`gb2`, in `SHADER_READ_ONLY_OPTIMAL`.
    pub(crate) gbuffer: [vk::ImageView; 3],
    /// The scene depth, in `SHADER_READ_ONLY_OPTIMAL`.
    pub(crate) depth: vk::ImageView,
    /// `hdr`, in `GENERAL` (the lighting reads the emissive and writes the lit surface).
    pub(crate) hdr: vk::ImageView,
    /// The main view's camera set (set 0) and lighting set (set 3), as the scene binds them.
    pub(crate) camera: vk::DescriptorSet,
    pub(crate) lighting: vk::DescriptorSet,
    pub(crate) render_size: vk::Extent2D,
}

/// The `light_classify` and `deferred_light` passes and their tile buffer.
pub(crate) struct DeferredPass {
    device: ash::Device,
    classify: GraphicsPipeline,
    standard: GraphicsPipeline,
    extended: GraphicsPipeline,
    classify_layout: PipelineLayout,
    light_layout: PipelineLayout,
    _classify_set_layout: DescriptorSetLayout,
    _light_set_layout: DescriptorSetLayout,
    _empty_set_layout: DescriptorSetLayout,
    _pool: DescriptorPool,
    classify_sets: [vk::DescriptorSet; SET_RING],
    light_sets: [vk::DescriptorSet; SET_RING],
    tiles: GpuBuffer,
    layout: TileLayout,
    /// `push.debug` (0 off; [`crate::post::TemporalDebugView::deferred_view`]).
    debug: u32,
}

/// The bindings of `deferred_light.comp`'s set 1: the G-buffer and the depth (sampled),
/// `hdr` (storage image) and the tile buffer.
fn light_set_bindings() -> [vk::DescriptorSetLayoutBinding<'static>; 6] {
    let binding = |binding, ty| {
        vk::DescriptorSetLayoutBinding::default()
            .binding(binding)
            .descriptor_type(ty)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
    };
    let sampled = vk::DescriptorType::SAMPLED_IMAGE;
    [
        binding(0, sampled),
        binding(1, sampled),
        binding(2, sampled),
        binding(3, sampled),
        binding(4, vk::DescriptorType::STORAGE_IMAGE),
        binding(5, vk::DescriptorType::STORAGE_BUFFER),
    ]
}

/// The bindings of `light_classify.slang`'s set 0: the depth and `gb2` (sampled) and the
/// tile buffer.
fn classify_set_bindings() -> [vk::DescriptorSetLayoutBinding<'static>; 3] {
    let binding = |binding, ty| {
        vk::DescriptorSetLayoutBinding::default()
            .binding(binding)
            .descriptor_type(ty)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
    };
    [
        binding(0, vk::DescriptorType::SAMPLED_IMAGE),
        binding(1, vk::DescriptorType::SAMPLED_IMAGE),
        binding(2, vk::DescriptorType::STORAGE_BUFFER),
    ]
}

impl DeferredPass {
    /// The passes for rendered sub-rectangles up to `extent`, lighting with the scene's
    /// `camera_layout` (set 0) and `lighting_layout` (set 3); `debug` is
    /// [`crate::post::TemporalDebugView::deferred_view`].
    pub(crate) fn new(
        device: &DeviceContext,
        shaders: &ShaderSource,
        camera_layout: vk::DescriptorSetLayout,
        lighting_layout: vk::DescriptorSetLayout,
        extent: vk::Extent2D,
        debug: u32,
    ) -> RenderResult<Self> {
        let compute = vk::ShaderStageFlags::COMPUTE;
        let (camera_bindings, _) = super::scene::buffer_set_layout_bindings();
        let light_bindings = light_set_bindings();
        // The lighting layout's compute stage reads sets 0, 1 and 3.
        crate::descriptor_budget::check(
            "deferred_light",
            &[
                &camera_bindings,
                &light_bindings,
                &crate::lighting::set_layout_bindings(),
            ],
            crate::descriptor_budget::StageTotals {
                samplers: device.max_per_stage_samplers,
                sampled_images: device.max_per_stage_sampled_images,
                storage_buffers: device.max_per_stage_storage_buffers,
            },
        )?;
        let light_set_layout = DescriptorSetLayout::new(device.device.clone(), &light_bindings)?;
        let classify_set_layout =
            DescriptorSetLayout::new(device.device.clone(), &classify_set_bindings())?;
        let empty_set_layout = DescriptorSetLayout::new(device.device.clone(), &[])?;
        let ring = u32::try_from(SET_RING).expect("small");
        let pool = DescriptorPool::new(
            device.device.clone(),
            &[
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(6 * ring),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_IMAGE)
                    .descriptor_count(ring),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(2 * ring),
            ],
            2 * ring,
        )?;
        let allocated = pool.allocate(
            &device.device,
            &[
                [classify_set_layout.handle(); SET_RING],
                [light_set_layout.handle(); SET_RING],
            ]
            .concat(),
        )?;
        let classify_sets = core::array::from_fn(|index| allocated[index]);
        let light_sets = core::array::from_fn(|index| allocated[SET_RING + index]);
        let push = |size: usize| {
            [vk::PushConstantRange::default()
                .stage_flags(compute)
                .offset(0)
                .size(u32::try_from(size).expect("small"))]
        };
        let classify_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[classify_set_layout.handle()],
            &push(core::mem::size_of::<ClassifyPush>()),
        )?;
        let light_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[
                camera_layout,
                light_set_layout.handle(),
                empty_set_layout.handle(),
                lighting_layout,
            ],
            &push(core::mem::size_of::<DeferredPush>()),
        )?;
        let [classify, standard, extended] = Self::build(
            device,
            shaders,
            classify_layout.handle(),
            light_layout.handle(),
        )?;
        let layout = TileLayout::for_extent(extent);
        let tiles = Self::tile_buffer(device, layout)?;
        Ok(Self {
            device: device.device.clone(),
            classify,
            standard,
            extended,
            classify_layout,
            light_layout,
            _classify_set_layout: classify_set_layout,
            _light_set_layout: light_set_layout,
            _empty_set_layout: empty_set_layout,
            _pool: pool,
            classify_sets,
            light_sets,
            tiles,
            layout,
            debug,
        })
    }

    fn tile_buffer(device: &DeviceContext, layout: TileLayout) -> RenderResult<GpuBuffer> {
        GpuBuffer::new(
            device.device.clone(),
            device.allocator(),
            "deferred tiles",
            layout.bytes(),
            vk::BufferUsageFlags::STORAGE_BUFFER
                | vk::BufferUsageFlags::INDIRECT_BUFFER
                | vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuOnly,
        )
    }

    fn build(
        device: &DeviceContext,
        shaders: &ShaderSource,
        classify_layout: vk::PipelineLayout,
        light_layout: vk::PipelineLayout,
    ) -> RenderResult<[GraphicsPipeline; 3]> {
        let pipeline = |name, layout| -> RenderResult<GraphicsPipeline> {
            let module = shaders.module(device, name)?;
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
        };
        Ok([
            pipeline("light_classify.slang", classify_layout)?,
            pipeline("deferred_light.comp", light_layout)?,
            pipeline("deferred_light_extended.comp", light_layout)?,
        ])
    }

    /// Debug builds' shader hot reload: new pipelines from `shaders` (installed with
    /// [`Self::install_pipelines`]).
    pub(crate) fn build_pipelines(
        &self,
        device: &DeviceContext,
        shaders: &ShaderSource,
    ) -> RenderResult<[GraphicsPipeline; 3]> {
        Self::build(
            device,
            shaders,
            self.classify_layout.handle(),
            self.light_layout.handle(),
        )
    }

    /// Replaces the pipelines (the device is idle).
    pub(crate) fn install_pipelines(
        &mut self,
        [classify, standard, extended]: [GraphicsPipeline; 3],
    ) {
        self.classify = classify;
        self.standard = standard;
        self.extended = extended;
    }

    /// Re-sizes the tile buffer for rendered sub-rectangles up to `extent` (the device is
    /// idle: swapchain recreation).
    pub(crate) fn resize(
        &mut self,
        device: &DeviceContext,
        extent: vk::Extent2D,
    ) -> RenderResult<()> {
        let layout = TileLayout::for_extent(extent);
        if layout != self.layout {
            self.tiles = Self::tile_buffer(device, layout)?;
            self.layout = layout;
        }
        Ok(())
    }

    /// The tile buffer (the frame graph's imported `deferred-tiles`).
    pub(crate) fn tile_buffer_handle(&self) -> vk::Buffer {
        self.tiles.handle()
    }

    /// Records `mask_clear`'s part: the dispatch arguments back to `(0, 1, 1)`. The graph
    /// moved the buffer to a transfer write.
    pub(crate) fn record_reset(&self, cmd: vk::CommandBuffer) {
        // SAFETY: the buffer is live and at least the header long; the graph ordered the
        // previous frame's reads before this transfer write.
        unsafe {
            self.device.cmd_update_buffer(
                cmd,
                self.tiles.handle(),
                0,
                bytemuck::cast_slice(&TileLayout::RESET),
            );
        }
    }

    /// Records `light_classify` for frame-in-flight slot `frame_slot` (its fence was waited
    /// on, so its sets are free).
    pub(crate) fn record_classify(
        &self,
        cmd: vk::CommandBuffer,
        frame_slot: usize,
        targets: &DeferredTargets,
    ) {
        let set = self.classify_sets[frame_slot % SET_RING];
        let read = |view| {
            [vk::DescriptorImageInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
        };
        let depth = read(targets.depth);
        let gb2 = read(targets.gbuffer[2]);
        let tiles = [vk::DescriptorBufferInfo::default()
            .buffer(self.tiles.handle())
            .offset(0)
            .range(vk::WHOLE_SIZE)];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&depth),
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&gb2),
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(2)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&tiles),
        ];
        let size = targets.render_size;
        let push = ClassifyPush {
            render_size: [size.width, size.height],
            tiles_x: self.layout.tiles_x,
            capacity: self.layout.capacity,
        };
        // SAFETY: no submitted frame reads this slot's set (its fence was waited on), and every
        // view and the buffer are live.
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
        // SAFETY: the pipeline, layout and set are live and the set was written above; the
        // push range lies in the layout's compute range; the rendered sub-rectangle is within
        // the extent the buffer was sized for, so the tiles fit its lists.
        unsafe {
            self.device.cmd_bind_pipeline(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.classify.handle(),
            );
            self.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.classify_layout.handle(),
                0,
                &[set],
                &[],
            );
            self.device.cmd_push_constants(
                cmd,
                self.classify_layout.handle(),
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytes_of(&push),
            );
            self.device.cmd_dispatch(
                cmd,
                size.width.div_ceil(TILE),
                size.height.div_ceil(TILE),
                1,
            );
        }
    }

    /// Records `deferred_light`: the standard tiles, then the extended ones, each one indirect
    /// dispatch over its list.
    pub(crate) fn record_light(
        &self,
        cmd: vk::CommandBuffer,
        frame_slot: usize,
        targets: &DeferredTargets,
    ) {
        let set = self.light_sets[frame_slot % SET_RING];
        let read = |view| {
            [vk::DescriptorImageInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
        };
        let inputs = [
            read(targets.gbuffer[0]),
            read(targets.gbuffer[1]),
            read(targets.gbuffer[2]),
            read(targets.depth),
        ];
        let hdr = [vk::DescriptorImageInfo::default()
            .image_view(targets.hdr)
            .image_layout(vk::ImageLayout::GENERAL)];
        let tiles = [vk::DescriptorBufferInfo::default()
            .buffer(self.tiles.handle())
            .offset(0)
            .range(vk::WHOLE_SIZE)];
        let sampled = |binding: u32, info| {
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(binding)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(info)
        };
        let writes = [
            sampled(0, &inputs[0]),
            sampled(1, &inputs[1]),
            sampled(2, &inputs[2]),
            sampled(3, &inputs[3]),
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(4)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&hdr),
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(5)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&tiles),
        ];
        // SAFETY: no submitted frame reads this slot's set (its fence was waited on), and every
        // view and the buffer are live.
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
        // SAFETY: the layout and sets are live: set 0 and 3 are the scene's camera and
        // lighting sets for this slot (written before the frame was recorded), set 1 was
        // written above; set 2 is empty.
        unsafe {
            self.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.light_layout.handle(),
                0,
                &[targets.camera, set],
                &[],
            );
            self.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.light_layout.handle(),
                3,
                &[targets.lighting],
                &[],
            );
        }
        let size = targets.render_size;
        for (class, pipeline) in [
            (TileClass::Standard, &self.standard),
            (TileClass::Extended, &self.extended),
        ] {
            let push = DeferredPush {
                render_size: [size.width, size.height],
                list_offset: self.layout.list_offset(class),
                debug: self.debug,
                inv_exposure: 1.0,
                tile_class: class as u32,
                reserved0: 0,
                reserved1: 0,
            };
            // SAFETY: the pipeline is live and uses the bound layout; the push range lies in
            // its compute range; the arguments were written by `light_classify` (at most
            // `capacity` groups, each reading its own list entry) and the graph made them
            // visible to the indirect read.
            unsafe {
                self.device.cmd_bind_pipeline(
                    cmd,
                    vk::PipelineBindPoint::COMPUTE,
                    pipeline.handle(),
                );
                self.device.cmd_push_constants(
                    cmd,
                    self.light_layout.handle(),
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    bytes_of(&push),
                );
                self.device.cmd_dispatch_indirect(
                    cmd,
                    self.tiles.handle(),
                    TileLayout::args_offset(class),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ClassifyPush, DeferredPush, TileClass, TileLayout};

    /// The push-constant block's member offsets in a SPIR-V module: the struct the
    /// `PushConstant` variable points to (`OpVariable`, `OpTypePointer`), its
    /// `OpMemberDecorate … Offset`s in member order. The build strips names.
    fn push_constant_offsets(spirv: &[u8]) -> Vec<u32> {
        const OP_TYPE_POINTER: u32 = 32;
        const OP_VARIABLE: u32 = 59;
        const OP_MEMBER_DECORATE: u32 = 72;
        const PUSH_CONSTANT: u32 = 9;
        const OFFSET: u32 = 35;
        let words: Vec<u32> = spirv
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("word")))
            .collect();
        let mut pointers = Vec::new();
        let mut variable_type = None;
        let mut offsets = Vec::new();
        let mut index = 5;
        while index < words.len() {
            let count = (words[index] >> 16) as usize;
            let operands = &words[index + 1..index + count.max(1)];
            match words[index] & 0xFFFF {
                OP_TYPE_POINTER => pointers.push((operands[0], operands[1], operands[2])),
                OP_VARIABLE if operands[2] == PUSH_CONSTANT => variable_type = Some(operands[0]),
                OP_MEMBER_DECORATE if operands[2] == OFFSET => {
                    offsets.push((operands[0], operands[1], operands[3]));
                }
                _ => {}
            }
            index += count.max(1);
        }
        let pointer = variable_type.expect("a push-constant block");
        let block = pointers
            .iter()
            .find(|&&(id, class, _)| id == pointer && class == PUSH_CONSTANT)
            .map(|&(_, _, pointee)| pointee)
            .expect("its pointer type");
        let mut members: Vec<(u32, u32)> = offsets
            .into_iter()
            .filter(|&(id, _, _)| id == block)
            .map(|(_, member, offset)| (member, offset))
            .collect();
        members.sort_unstable();
        members.into_iter().map(|(_, offset)| offset).collect()
    }

    /// `DeferredPush` matches the GLSL block in both permutations.
    #[test]
    fn deferred_push_matches_the_shader() {
        let expected: Vec<u32> = [
            core::mem::offset_of!(DeferredPush, render_size),
            core::mem::offset_of!(DeferredPush, list_offset),
            core::mem::offset_of!(DeferredPush, debug),
            core::mem::offset_of!(DeferredPush, inv_exposure),
            core::mem::offset_of!(DeferredPush, tile_class),
            core::mem::offset_of!(DeferredPush, reserved0),
            core::mem::offset_of!(DeferredPush, reserved1),
        ]
        .map(|offset| u32::try_from(offset).expect("small"))
        .to_vec();
        for spirv in [
            moba_shaders::DEFERRED_LIGHT_COMP,
            moba_shaders::DEFERRED_LIGHT_EXTENDED_COMP,
        ] {
            assert_eq!(push_constant_offsets(spirv), expected);
        }
        assert_eq!(core::mem::size_of::<DeferredPush>(), 32);
    }

    /// `ClassifyPush` matches the Slang `ClassifyPush` the reflection reports (ADR-0020 §4c).
    #[test]
    fn classify_push_matches_the_slang_layout() {
        let json = moba_shaders::reflection("light_classify.slang").expect("reflection");
        let reflection: serde_json::Value = serde_json::from_str(json).expect("json");
        let push = reflection["parameters"]
            .as_array()
            .expect("parameters")
            .iter()
            .find(|parameter| parameter["name"] == "push")
            .expect("push");
        let offsets: Vec<(String, u64)> = push["type"]["elementType"]["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .map(|field| {
                (
                    field["name"].as_str().expect("name").to_owned(),
                    field["binding"]["offset"].as_u64().expect("offset"),
                )
            })
            .collect();
        let expected = [
            (
                "render_size",
                core::mem::offset_of!(ClassifyPush, render_size),
            ),
            ("tiles_x", core::mem::offset_of!(ClassifyPush, tiles_x)),
            ("capacity", core::mem::offset_of!(ClassifyPush, capacity)),
        ]
        .map(|(name, offset)| (name.to_owned(), offset as u64));
        assert_eq!(offsets, expected);
        assert_eq!(core::mem::size_of::<ClassifyPush>(), 16);
    }

    /// The lights `dynamic_lights` (include/lighting.glsl) visits at `p`: the grid column's
    /// list, each light kept when `p` is inside its radius. A twin of the shader's loop
    /// (`light_loop_twin_matches_the_shader` checks the lines it mirrors).
    fn visited(
        params: crate::lights::GridParams,
        lights: &[[f32; 4]],
        cells: &[u32],
        p: moba_core::math::Vec3,
    ) -> Vec<u32> {
        let [columns_x, columns_z, stride, total] = params.info;
        if total == 0 || stride < 2 {
            return Vec::new();
        }
        let dims = [columns_x.max(1), columns_z.max(1)];
        let cell = |value: f32, min: f32, inverse: f32, dim: u32| {
            #[allow(clippy::cast_possible_truncation)] // Clamped to the grid.
            let index = ((value - min) * inverse)
                .floor()
                .clamp(0.0, (dim - 1) as f32) as u32;
            index
        };
        let x = cell(p.x, params.grid[0], params.grid[2], dims[0]);
        let z = cell(p.z, params.grid[1], params.grid[3], dims[1]);
        let base = ((z * dims[0] + x) * stride) as usize;
        let count = cells[base].min(stride - 1);
        (0..count)
            .map(|i| cells[base + 1 + i as usize].min(total - 1))
            .filter(|&index| {
                let [lx, ly, lz, radius] = lights[index as usize * 2];
                let d = moba_core::math::Vec3::new(lx, ly, lz) - p;
                d.dot(d) < radius * radius
            })
            .collect()
    }

    /// The twin above follows the shader's column lookup and radius test line for line.
    #[test]
    fn light_loop_twin_matches_the_shader() {
        let lighting = include_str!("../../../shaders/shaders/include/lighting.glsl");
        for line in [
            "vec2 cell_f = (surface.world_pos.xz - camera.light_grid.xy) * camera.light_grid.zw;",
            "ivec2 cell = clamp(ivec2(floor(cell_f)), ivec2(0), ivec2(dims) - 1);",
            "uint base = (uint(cell.y) * dims.x + uint(cell.x)) * stride;",
            "uint count = min(light_grid.cells[base], stride - 1u);",
            "uint index = min(light_grid.cells[base + 1u + i], total - 1u);",
            "if (d2 >= radius2) {",
        ] {
            assert!(lighting.contains(line), "lighting.glsl changed: {line}");
        }
        // The deferred lighting calls the same `shade()`, which runs `dynamic_lights` at the
        // surface's `world_pos`: the reconstructed position.
        let deferred = include_str!("../../../shaders/shaders/deferred_light.comp");
        assert!(deferred.contains("#include \"lighting.glsl\""));
        assert!(deferred.contains(
            "Surface surface = default_surface(albedo, s.normal, world_pos, s.roughness);"
        ));
        assert!(deferred.contains("vec3 lit = shade(surface);"));
    }

    /// ADR-0023 gate 3 (light-set consistency): for the same surface point, the forward light
    /// loop (at the rasterised world position) and the deferred one (at the position
    /// reconstructed from the reversed-Z `D32_SFLOAT` depth through the inverse
    /// view-projection, as `deferred_light.comp` does) visit the same lights, under every
    /// tier's global and per-column cap, with columns overflowing at the low caps. Points
    /// within the reconstruction's error of a column edge or a light's radius are counted,
    /// not compared (there either loop may take the neighbour, and a light at its radius
    /// adds zero); they must stay rare.
    #[test]
    fn forward_and_deferred_visit_the_same_lights() {
        use crate::depth::DepthConvention;
        use crate::lights::{CELL_WORDS, GRID, LIGHT_VEC4S, MAX_LIGHTS, PointLight};
        use moba_core::math::{Vec3, Vec4};

        let (width, height) = (320u32, 180u32);
        let camera = crate::Camera::orbit(Vec3::new(3.0, 0.0, -2.0), 46.0, -0.7, 0.92, 16.0 / 9.0);
        let gpu = DepthConvention::Reversed.camera(&camera);
        let inverse = gpu.inverse();
        // Deterministic lights: a spread over the view and two tight clusters that overflow
        // the per-column caps (a teamfight's stacked effects).
        let mut seed = 0x2545_f491_u32;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            f32::from(u16::try_from(seed >> 16).expect("16 bits")) / 65_535.0
        };
        // The clusters first, so even Low's 16-light cap bins them.
        let mut lights = Vec::new();
        for centre in [Vec3::new(0.0, 1.0, 0.0), Vec3::new(9.0, 1.0, -6.0)] {
            for _ in 0..30 {
                let offset = Vec3::new(random() - 0.5, random(), random() - 0.5) * 3.0;
                lights.push(PointLight::new(
                    centre + offset,
                    4.0 + random() * 4.0,
                    Vec3::ONE,
                ));
            }
        }
        for _ in 0..180 {
            let position = Vec3::new(
                random() * 70.0 - 32.0,
                random() * 3.0,
                random() * 50.0 - 27.0,
            );
            lights.push(PointLight::new(position, 2.0 + random() * 10.0, Vec3::ONE));
        }
        // Surface points: the ground (y = 0) and a slope (y = 0.3 x + 1), through pixel
        // centres of a `width` x `height` render with the scene's flipped viewport.
        let unproject = |ndc: Vec4| {
            let world = inverse * ndc;
            world.truncate() / world.w
        };
        let mut points = Vec::new();
        for py in 0..height {
            for px in 0..width {
                let ndc_x = (px as f32 + 0.5) / width as f32 * 2.0 - 1.0;
                let ndc_y = 1.0 - (py as f32 + 0.5) / height as f32 * 2.0;
                let near = unproject(Vec4::new(ndc_x, ndc_y, 1.0, 1.0)).as_dvec3();
                let far = unproject(Vec4::new(ndc_x, ndc_y, 0.0, 1.0)).as_dvec3();
                let ray = far - near;
                // The nearer of the two surfaces the ray meets in front of the camera.
                let ground = -near.y / ray.y;
                let slope = (0.3 * near.x + 1.0 - near.y) / (ray.y - 0.3 * ray.x);
                let t = [ground, slope]
                    .into_iter()
                    .filter(|t| t.is_finite() && *t > 0.0 && *t < 1.0)
                    .fold(f64::INFINITY, f64::min);
                if t.is_finite() {
                    points.push(((near + ray * t).as_vec3(), ndc_x, ndc_y));
                }
            }
        }
        assert!(points.len() > (width * height / 2) as usize);
        let footprint = crate::lights::ground_footprint(camera.view_proj);
        // The reconstruction is off by at most about 24 µm here (D32 reversed-Z at 40-60 m).
        let tolerance = 1e-4_f32;
        let mut worst = 0.0_f32;
        for &tier in crate::QualityTier::ALL {
            let config = crate::LightingConfig::for_tier(tier);
            let mut light_words = vec![[0.0; 4]; MAX_LIGHTS * LIGHT_VEC4S];
            let mut cell_words = vec![0u32; GRID * GRID * CELL_WORDS];
            let mut counts = [0u8; GRID * GRID];
            let (params, stats) = crate::lights::bin(
                &lights,
                footprint,
                config.light_cap(),
                config.cell_cap(),
                &mut light_words,
                &mut cell_words,
                &mut counts,
            );
            assert!(stats.binned > 0, "{tier:?}");
            assert!(
                stats.cell_overflows > 0,
                "{tier:?}: the clusters overflow a column"
            );
            let (mut compared, mut skipped, mut nonempty) = (0usize, 0usize, 0usize);
            for &(forward, ndc_x, ndc_y) in &points {
                // The rasteriser's depth: this point's reversed-Z depth, stored as f32.
                let clip = gpu * forward.extend(1.0);
                let depth = clip.z / clip.w;
                let deferred = unproject(Vec4::new(ndc_x, ndc_y, depth, 1.0));
                let error = (deferred - forward).length();
                worst = worst.max(error);
                assert!(
                    error < tolerance,
                    "{tier:?}: reconstruction off by {error} m"
                );
                // Near a column edge or a radius the loops may legitimately differ.
                let edge = |value: f32, min: f32, inverse: f32| {
                    let cell = (value - min) * inverse;
                    (cell - cell.round()).abs() < tolerance * inverse
                };
                let near_edge = params.info[3] > 0
                    && (edge(forward.x, params.grid[0], params.grid[2])
                        || edge(forward.z, params.grid[1], params.grid[3]));
                let near_radius = lights.iter().any(|light| {
                    ((light.position - forward).length() - light.radius).abs() < tolerance
                });
                if near_edge || near_radius {
                    skipped += 1;
                    continue;
                }
                let forward_set = visited(params, &light_words, &cell_words, forward);
                let deferred_set = visited(params, &light_words, &cell_words, deferred);
                assert_eq!(forward_set, deferred_set, "{tier:?} at {forward:?}");
                compared += 1;
                nonempty += usize::from(!forward_set.is_empty());
            }
            assert!(
                nonempty >= 1000,
                "{tier:?}: the lights reach {nonempty} of {compared} points"
            );
            assert!(
                skipped * 100 < points.len(),
                "{tier:?}: {skipped} of {} points skipped",
                points.len()
            );
        }
        assert!(worst > 0.0, "the reconstruction is exercised");
    }

    /// The tile layout's constants are the shaders' (`TILE_HEADER_WORDS`, the class order,
    /// the 16-pixel tile) and the buffer holds every tile of the extent in each list.
    #[test]
    fn tile_layout_matches_the_shaders() {
        let classify = include_str!("../../../shaders/shaders/slang/light_classify.slang");
        assert!(classify.contains("static const uint TILE_HEADER_WORDS = 12;"));
        assert!(classify.contains("static const uint TILE = 16;"));
        assert!(classify.contains("static const uint CLASS_SKY = 0;"));
        assert!(classify.contains("static const uint CLASS_STANDARD = 1;"));
        assert!(classify.contains("static const uint CLASS_EXTENDED = 2;"));
        assert!(classify.contains("[numthreads(16, 16, 1)]"));
        let light = include_str!("../../../shaders/shaders/deferred_light.comp");
        assert!(light.contains("local_size_x = 16, local_size_y = 16"));
        assert_eq!(TileLayout::HEADER_WORDS as usize, TileLayout::RESET.len());
        assert_eq!(TileClass::Sky as u32, 0);
        let layout = TileLayout::for_extent(ash::vk::Extent2D {
            width: 1920,
            height: 1080,
        });
        assert_eq!((layout.tiles_x, layout.capacity), (120, 120 * 68));
        assert_eq!(layout.list_offset(TileClass::Standard), 12 + 8160);
        assert_eq!(layout.info_offset(), 12 + 3 * 8160);
        assert_eq!(layout.bytes(), 4 * (12 + 6 * 8160));
        assert_eq!(TileLayout::args_offset(TileClass::Extended), 24);
        // Each reset dispatch is (0, 1, 1).
        for class in 0..3 {
            assert_eq!(&TileLayout::RESET[3 * class..3 * class + 3], &[0, 1, 1]);
        }
    }
}
