//! The deferred lighting on Tiered (ADR-0023 §2, N2b): `light_classify` sorts the rendered
//! sub-rectangle's 16 × 16 tiles by the shading models they hold (`light_classify.slang`),
//! then `deferred_light` lights each class with one indirect dispatch of its permutation
//! (`deferred_light.comp`, built from the forward path's `lighting.glsl`, so a deferred pixel
//! evaluates the same lights, shadows, ambient, AO and fog as a forward one at the same
//! point). The G-buffer pass wrote the units' emissive and the sky's radiance into `hdr`;
//! the lighting replaces every lit pixel with the lit, fogged surface (reading the emissive
//! only for unit pixels) and leaves the sky, whose tiles are not even listed.
//!
//! The tile buffer ([`TileLayout`]) holds the classes' `VkDispatchIndirectCommand`s, their
//! tile counts and the standard and extended tile lists. `mask_clear` resets the header each
//! frame ([`TileLayout::RESET`]). The arguments are rows of `tiles_x` groups, so no dimension
//! exceeds `maxComputeWorkGroupCount`'s guaranteed 65,535 however many tiles a class holds.
//!
//! The descriptor sets are written once per transient binding ([`DeferredPass::bind_targets`],
//! at creation and after a resize), not per frame. The debug views are a specialisation
//! constant of the lighting pipelines (`DEBUG_VIEW`), fixed at creation, so the shipped
//! pipelines carry no debug branches.

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};
use gpu_allocator::MemoryLocation;

use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuBuffer, GraphicsPipeline, PipelineLayout,
};
use crate::shaders::ShaderSource;

/// The tile edge, in pixels (`numthreads` of both shaders).
pub(crate) const TILE: u32 = 16;
/// Frame slots of the tile counts' readback (the frames in flight).
const COUNT_SLOTS: usize = crate::renderer::FRAMES_IN_FLIGHT;

/// The tile classes `light_classify` sorts into (`CLASS_*` in `light_classify.slang`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum TileClass {
    /// Every pixel at the far plane: not lit and not listed (the G-buffer pass drew the sky),
    /// so only the shaders and the tests name it.
    #[cfg_attr(not(test), allow(dead_code))]
    Sky = 0,
    /// Only `STANDARD` and `UNLIT` pixels: the fast permutation.
    Standard = 1,
    /// Any other model: the full permutation.
    Extended = 2,
}

/// The tile buffer's layout (`light_classify.slang`, `deferred_light.comp`), in 32-bit words:
/// the three classes' dispatch arguments (`x, y, z` each) and their tile counts (the sky's
/// stay at their reset values), then the standard and the extended lists of `capacity` packed
/// tiles (`x | y << 16`).
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
    /// The word of the sky class's tile count (`TILE_COUNT_WORD`); the others follow.
    pub(crate) const COUNT_WORD: u32 = 9;
    /// What `mask_clear` writes over the header each frame: every class's dispatch is
    /// `(0, 0, 1)` and its count 0; `light_classify` raises `x` and `y` with atomic maxima.
    pub(crate) const RESET: [u32; 12] = [0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 0];

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
        4 * (u64::from(Self::HEADER_WORDS) + 2 * u64::from(self.capacity))
    }

    /// The byte offset of `class`'s `VkDispatchIndirectCommand`.
    pub(crate) const fn args_offset(class: TileClass) -> u64 {
        12 * class as u64
    }

    /// The word offset of `class`'s tile list (standard or extended: the sky has none).
    pub(crate) const fn list_offset(self, class: TileClass) -> u32 {
        Self::HEADER_WORDS + (class as u32 - 1) * self.capacity
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

/// `DeferredPush` in `deferred_light.comp` (24 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct DeferredPush {
    render_size: [u32; 2],
    list_offset: u32,
    tile_class: u32,
    row_width: u32,
    capacity: u32,
}

/// The transient views the passes read and write, bound once per transient binding.
pub(crate) struct DeferredViews {
    /// `gb0`–`gb2`, in `SHADER_READ_ONLY_OPTIMAL`.
    pub(crate) gbuffer: [vk::ImageView; 3],
    /// The scene depth, in `SHADER_READ_ONLY_OPTIMAL`.
    pub(crate) depth: vk::ImageView,
    /// `hdr`, in `GENERAL` (the lighting reads the units' emissive and writes the lit
    /// surface).
    pub(crate) hdr: vk::ImageView,
}

/// What one frame's passes bind besides their own sets.
pub(crate) struct DeferredTargets {
    /// The main view's camera set (set 0) and lighting set (set 3), as the scene binds them.
    pub(crate) camera: vk::DescriptorSet,
    pub(crate) lighting: vk::DescriptorSet,
    pub(crate) render_size: vk::Extent2D,
    /// Whether this frame's light list holds a shaped light (N2d): without one the lighting
    /// runs its `AREA_MODE` 0 pipelines, which light every point light the same way.
    pub(crate) area_lights: bool,
}

/// The pass's pipelines: the classification, the two lighting permutations at the
/// configured `AREA_MODE` and, when that mode is not 0, the same two at mode 0 for frames
/// without a shaped light.
pub(crate) struct DeferredPipelines {
    classify: GraphicsPipeline,
    standard: GraphicsPipeline,
    extended: GraphicsPipeline,
    plain: Option<[GraphicsPipeline; 2]>,
}

/// The `light_classify` and `deferred_light` passes and their tile buffer.
pub(crate) struct DeferredPass {
    device: ash::Device,
    pipelines: DeferredPipelines,
    classify_layout: PipelineLayout,
    light_layout: PipelineLayout,
    _classify_set_layout: DescriptorSetLayout,
    _light_set_layout: DescriptorSetLayout,
    _empty_set_layout: DescriptorSetLayout,
    _pool: DescriptorPool,
    /// Written by [`Self::bind_targets`] while the device is idle; read by every frame.
    classify_set: vk::DescriptorSet,
    light_set: vk::DescriptorSet,
    tiles: GpuBuffer,
    /// Host-readable copies of the classes' tile counts, one 16-byte slot per frame slot
    /// ([`Self::record_reset`] copies the previous frame's before resetting them), for the
    /// frame log's record of what a run lit (N2c).
    counts: GpuBuffer,
    layout: TileLayout,
    /// The lighting pipelines' `DEBUG_VIEW` specialisation constant (0 off;
    /// [`crate::post::TemporalDebugView::deferred_view`]).
    debug: u32,
    /// Their `AREA_MODE` constant ([`crate::AreaLights`], N2d): the value the forward stages
    /// read from the camera's `occlusion.z`. A frame without a shaped light runs the mode-0
    /// pipelines instead ([`DeferredTargets::area_lights`]): the LTC evaluation's registers
    /// (121 against 80 on the RTX 5070 Ti, perf/0038) cost the whole pass occupancy even
    /// where no shaped light reaches.
    area_mode: u32,
}

/// Why the deferred lighting cannot run on a device admitted to Tiered: the frame then
/// lights forward (ADR-0023 amendment 2 items 13 and 17).
pub(crate) fn unsupported(device: &DeviceContext) -> Option<String> {
    let (camera_bindings, _) = super::scene::buffer_set_layout_bindings();
    // The lighting layout's compute stage reads sets 0, 1 and 3.
    if let Err(error) = crate::descriptor_budget::check(
        "deferred_light",
        &[
            &camera_bindings,
            &light_set_bindings(),
            &crate::lighting::set_layout_bindings(),
        ],
        crate::descriptor_budget::StageTotals {
            samplers: device.max_per_stage_samplers,
            sampled_images: device.max_per_stage_sampled_images,
            storage_buffers: device.max_per_stage_storage_buffers,
        },
    ) {
        return Some(error.to_string());
    }
    // The G-buffer pass writes six colour outputs with the outline IDs (motion, gb0-gb2, the
    // unit mask and `hdr`); admission guarantees five.
    if device.max_fragment_output_attachments < 6 || device.max_fragment_combined_outputs < 6 {
        return Some(format!(
            "the G-buffer pass writes 6 fragment outputs; maxFragmentOutputAttachments is {} \
             and maxFragmentCombinedOutputResources {}",
            device.max_fragment_output_attachments, device.max_fragment_combined_outputs
        ));
    }
    // 16 x 16 workgroups (the core minimum is 128 invocations).
    if device.max_compute_work_group_invocations < TILE * TILE {
        return Some(format!(
            "the tiles are {} invocations; maxComputeWorkGroupInvocations is {}",
            TILE * TILE,
            device.max_compute_work_group_invocations
        ));
    }
    // `light_classify` ORs the tile's models across each subgroup.
    if !device.subgroup_arithmetic_compute {
        return Some(String::from(
            "light_classify needs subgroup arithmetic in compute shaders",
        ));
    }
    None
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
        area_mode: u32,
    ) -> RenderResult<Self> {
        let compute = vk::ShaderStageFlags::COMPUTE;
        // The renderer checked [`unsupported`] before choosing the deferred frame.
        let light_set_layout =
            DescriptorSetLayout::new(device.device.clone(), &light_set_bindings())?;
        let classify_set_layout =
            DescriptorSetLayout::new(device.device.clone(), &classify_set_bindings())?;
        let empty_set_layout = DescriptorSetLayout::new(device.device.clone(), &[])?;
        let pool = DescriptorPool::new(
            device.device.clone(),
            &[
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(6),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_IMAGE)
                    .descriptor_count(1),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(2),
            ],
            2,
        )?;
        let allocated = pool.allocate(
            &device.device,
            &[classify_set_layout.handle(), light_set_layout.handle()],
        )?;
        let (classify_set, light_set) = (allocated[0], allocated[1]);
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
        let pipelines = Self::build(
            device,
            shaders,
            classify_layout.handle(),
            light_layout.handle(),
            [debug, area_mode],
        )?;
        let layout = TileLayout::for_extent(extent);
        let tiles = Self::tile_buffer(device, layout)?;
        let counts = GpuBuffer::new(
            device.device.clone(),
            device.allocator(),
            "deferred tile counts",
            COUNT_SLOTS as u64 * 16,
            vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuToCpu,
        )?;
        Ok(Self {
            device: device.device.clone(),
            pipelines,
            classify_layout,
            light_layout,
            _classify_set_layout: classify_set_layout,
            _light_set_layout: light_set_layout,
            _empty_set_layout: empty_set_layout,
            _pool: pool,
            classify_set,
            light_set,
            tiles,
            counts,
            layout,
            debug,
            area_mode,
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
                | vk::BufferUsageFlags::TRANSFER_SRC
                | vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuOnly,
        )
    }

    fn build(
        device: &DeviceContext,
        shaders: &ShaderSource,
        classify_layout: vk::PipelineLayout,
        light_layout: vk::PipelineLayout,
        [debug, area_mode]: [u32; 2],
    ) -> RenderResult<DeferredPipelines> {
        // `deferred_light.comp`'s `DEBUG_VIEW` (constant_id 0) and `AREA_MODE` (constant_id 1,
        // N2d); `light_classify` has neither, and unused entries are ignored.
        let size = core::mem::size_of::<u32>();
        let entries = [
            vk::SpecializationMapEntry::default()
                .constant_id(0)
                .offset(0)
                .size(size),
            vk::SpecializationMapEntry::default()
                .constant_id(1)
                .offset(size as u32)
                .size(size),
        ];
        let pipeline = |name, layout, area_mode: u32| -> RenderResult<GraphicsPipeline> {
            let mut data = [0u8; 8];
            data[..4].copy_from_slice(&debug.to_ne_bytes());
            data[4..].copy_from_slice(&area_mode.to_ne_bytes());
            let specialization = vk::SpecializationInfo::default()
                .map_entries(&entries)
                .data(&data);
            let module = shaders.module(device, name)?;
            let stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(module.handle())
                .name(c"main")
                .specialization_info(&specialization);
            let handle = device.create_compute_pipeline(
                &vk::ComputePipelineCreateInfo::default()
                    .stage(stage)
                    .layout(layout),
            )?;
            Ok(GraphicsPipeline::from_raw(device.device.clone(), handle))
        };
        let pipelines = DeferredPipelines {
            classify: pipeline("light_classify.slang", classify_layout, area_mode)?,
            standard: pipeline("deferred_light.comp", light_layout, area_mode)?,
            extended: pipeline("deferred_light_extended.comp", light_layout, area_mode)?,
            plain: if area_mode == 0 {
                None
            } else {
                Some([
                    pipeline("deferred_light.comp", light_layout, 0)?,
                    pipeline("deferred_light_extended.comp", light_layout, 0)?,
                ])
            },
        };
        // `RenderConfig::pipeline_stats`: the permutations' registers (amendment 3 item 32).
        let plain = pipelines.plain.as_ref();
        for (name, pipeline) in [
            ("light_classify", Some(&pipelines.classify)),
            ("deferred_light", Some(&pipelines.standard)),
            ("deferred_light_extended", Some(&pipelines.extended)),
            ("deferred_light_plain", plain.map(|[standard, _]| standard)),
            (
                "deferred_light_extended_plain",
                plain.map(|[_, extended]| extended),
            ),
        ] {
            let Some(pipeline) = pipeline else { continue };
            for line in device.pipeline_statistics(pipeline.handle()) {
                eprintln!("[render] pipeline-stats {name} {line}");
            }
        }
        Ok(pipelines)
    }

    /// Debug builds' shader hot reload: new pipelines from `shaders` (installed with
    /// [`Self::install_pipelines`]).
    pub(crate) fn build_pipelines(
        &self,
        device: &DeviceContext,
        shaders: &ShaderSource,
    ) -> RenderResult<DeferredPipelines> {
        Self::build(
            device,
            shaders,
            self.classify_layout.handle(),
            self.light_layout.handle(),
            [self.debug, self.area_mode],
        )
    }

    /// Replaces the pipelines (the device is idle).
    pub(crate) fn install_pipelines(&mut self, pipelines: DeferredPipelines) {
        self.pipelines = pipelines;
    }

    /// Re-sizes the tile buffer for rendered sub-rectangles up to `extent` (the device is
    /// idle: swapchain recreation; [`Self::bind_targets`] follows).
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

    /// Records `mask_clear`'s part: with `counts`, the previous frame's tile counts copied
    /// into frame slot `slot`'s host-readable record ([`Self::tile_counts`]); then every
    /// class's dispatch arguments back to `(0, 0, 1)` and its count to 0
    /// ([`TileLayout::RESET`]). The graph moved the buffer to a transfer write; the copy's own
    /// barriers are recorded here: the classification's writes to the copy's read, that read
    /// before the reset's write, and the copy's write to the host. The copy costs about
    /// 0.005 ms (the barriers drain the queue), so it runs only for the frame log's counts.
    pub(crate) fn record_reset(
        &self,
        device: &DeviceContext,
        cmd: vk::CommandBuffer,
        slot: usize,
        counts: bool,
    ) {
        if counts {
            self.record_count_copy(device, cmd, slot);
        }
        // SAFETY: the buffer is live and at least the header long; the graph ordered the
        // previous frame's reads before this transfer write, and the copy's read (when
        // recorded) is ordered before it.
        unsafe {
            self.device.cmd_update_buffer(
                cmd,
                self.tiles.handle(),
                0,
                bytemuck::cast_slice(&TileLayout::RESET),
            );
        }
    }

    /// The tile counts' copy and its barriers ([`Self::record_reset`]).
    fn record_count_copy(&self, device: &DeviceContext, cmd: vk::CommandBuffer, slot: usize) {
        use vk::{AccessFlags2 as A, PipelineStageFlags2 as S};
        let tiles = |src_stage, src_access, dst_stage, dst_access| {
            vk::BufferMemoryBarrier2::default()
                .src_stage_mask(src_stage)
                .src_access_mask(src_access)
                .dst_stage_mask(dst_stage)
                .dst_access_mask(dst_access)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.tiles.handle())
                .offset(0)
                .size(u64::from(TileLayout::HEADER_WORDS) * 4)
        };
        let to_copy = [tiles(
            S::COMPUTE_SHADER,
            A::SHADER_STORAGE_WRITE,
            S::COPY,
            A::TRANSFER_READ,
        )];
        device.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().buffer_memory_barriers(&to_copy),
        );
        let slot = (slot % COUNT_SLOTS) as u64;
        let region = vk::BufferCopy::default()
            .src_offset(u64::from(TileLayout::COUNT_WORD) * 4)
            .dst_offset(slot * 16)
            .size(12);
        // SAFETY: both buffers are live, the source has TRANSFER_SRC and the destination
        // TRANSFER_DST, and both ranges lie inside them (the header; one 16-byte slot).
        unsafe {
            self.device.cmd_copy_buffer(
                cmd,
                self.tiles.handle(),
                self.counts.handle(),
                core::slice::from_ref(&region),
            );
        }
        // One array: a second `buffer_memory_barriers` call would replace the first.
        // `vkCmdUpdateBuffer` runs in the transfer (clear) stage, so the copy's read is
        // ordered before it with `ALL_TRANSFER`.
        let barriers = [
            tiles(
                S::COPY,
                A::TRANSFER_READ,
                S::ALL_TRANSFER,
                A::TRANSFER_WRITE,
            ),
            vk::BufferMemoryBarrier2::default()
                .src_stage_mask(S::COPY)
                .src_access_mask(A::TRANSFER_WRITE)
                .dst_stage_mask(S::HOST)
                .dst_access_mask(A::HOST_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.counts.handle())
                .offset(slot * 16)
                .size(16),
        ];
        device.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().buffer_memory_barriers(&barriers),
        );
    }

    /// The `[standard, extended]` tile counts `record_reset` copied in frame slot `slot`
    /// (the frame before that slot's last one), once the slot's fence has signalled; `None`
    /// before any copy or if the buffer cannot be read.
    pub(crate) fn tile_counts(&self, slot: usize) -> Option<[u32; 2]> {
        let mut bytes = [0_u8; 12];
        let offset = (slot % COUNT_SLOTS) * 16;
        if !self.counts.read_bytes(offset, &mut bytes) {
            return None;
        }
        let word = |index: usize| {
            u32::from_le_bytes([
                bytes[4 * index],
                bytes[4 * index + 1],
                bytes[4 * index + 2],
                bytes[4 * index + 3],
            ])
        };
        Some([word(1), word(2)])
    }

    /// Points both passes' sets at the current transients and the tile buffer. Called at
    /// creation and after every resize, while the device is idle (no recorded frame reads
    /// the sets), so the frames never write descriptors.
    pub(crate) fn bind_targets(&self, views: &DeferredViews) {
        let read = |view| {
            [vk::DescriptorImageInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
        };
        let [gb0, gb1, gb2] = views.gbuffer;
        let inputs = [read(gb0), read(gb1), read(gb2), read(views.depth)];
        let hdr = [vk::DescriptorImageInfo::default()
            .image_view(views.hdr)
            .image_layout(vk::ImageLayout::GENERAL)];
        let tiles = [vk::DescriptorBufferInfo::default()
            .buffer(self.tiles.handle())
            .offset(0)
            .range(vk::WHOLE_SIZE)];
        let write = |set, binding: u32, ty| {
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(binding)
                .descriptor_type(ty)
        };
        let sampled = vk::DescriptorType::SAMPLED_IMAGE;
        let storage = vk::DescriptorType::STORAGE_BUFFER;
        let writes = [
            // light_classify: the depth, gb2 and the tiles.
            write(self.classify_set, 0, sampled).image_info(&inputs[3]),
            write(self.classify_set, 1, sampled).image_info(&inputs[2]),
            write(self.classify_set, 2, storage).buffer_info(&tiles),
            // deferred_light: the G-buffer, the depth, `hdr` and the tiles.
            write(self.light_set, 0, sampled).image_info(&inputs[0]),
            write(self.light_set, 1, sampled).image_info(&inputs[1]),
            write(self.light_set, 2, sampled).image_info(&inputs[2]),
            write(self.light_set, 3, sampled).image_info(&inputs[3]),
            write(self.light_set, 4, vk::DescriptorType::STORAGE_IMAGE).image_info(&hdr),
            write(self.light_set, 5, storage).buffer_info(&tiles),
        ];
        // SAFETY: the device is idle (creation, or swapchain recreation after its wait), so
        // no submitted frame reads either set; every view and the buffer are live.
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
    }

    /// Records `light_classify` over this frame's rendered sub-rectangle.
    pub(crate) fn record_classify(&self, cmd: vk::CommandBuffer, targets: &DeferredTargets) {
        let size = targets.render_size;
        let push = ClassifyPush {
            render_size: [size.width, size.height],
            tiles_x: self.layout.tiles_x,
            capacity: self.layout.capacity,
        };
        // SAFETY: the pipeline is live; `cmd` is recording outside a render pass.
        unsafe {
            self.device.cmd_bind_pipeline(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipelines.classify.handle(),
            );
        }
        // SAFETY: the layout and set are live, and the set was written by `bind_targets`.
        unsafe {
            self.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.classify_layout.handle(),
                0,
                &[self.classify_set],
                &[],
            );
        }
        // SAFETY: the push range lies in the layout's compute range.
        unsafe {
            self.device.cmd_push_constants(
                cmd,
                self.classify_layout.handle(),
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytes_of(&push),
            );
        }
        // SAFETY: the pipeline, set and push constants are bound above; the rendered
        // sub-rectangle is within the extent the buffer was sized for, so the tiles fit its
        // lists, and the group counts are tiles per row and column of an image the device
        // created.
        unsafe {
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
    ///
    /// The two dispatches read and write `hdr` with no barrier between them. That is race
    /// free because their pixels are disjoint: `light_classify` puts each tile in exactly one
    /// list (by the models it holds), each group shades only the 16 x 16 pixels of its own
    /// tile, and the lists hold no tile twice (one append per tile). So no pixel is loaded or
    /// stored by both dispatches, and the graph's barrier after `deferred_light` orders both
    /// against what follows.
    pub(crate) fn record_light(&self, cmd: vk::CommandBuffer, targets: &DeferredTargets) {
        // SAFETY: the layout and sets are live: set 0 is the scene's camera set for this
        // frame's slot (written before the frame was recorded) and set 1 was written by
        // `bind_targets`.
        unsafe {
            self.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.light_layout.handle(),
                0,
                &[targets.camera, self.light_set],
                &[],
            );
        }
        // SAFETY: set 3 is the scene's lighting set for this slot, live and written before the
        // frame was recorded; set 2 is empty and never read.
        unsafe {
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
        let [standard, extended] = match &self.pipelines.plain {
            Some([standard, extended]) if !targets.area_lights => [standard, extended],
            _ => [&self.pipelines.standard, &self.pipelines.extended],
        };
        for (class, pipeline) in [
            (TileClass::Standard, standard),
            (TileClass::Extended, extended),
        ] {
            let push = DeferredPush {
                render_size: [size.width, size.height],
                list_offset: self.layout.list_offset(class),
                tile_class: class as u32,
                row_width: self.layout.tiles_x,
                capacity: self.layout.capacity,
            };
            // SAFETY: the pipeline is live and uses the bound layout.
            unsafe {
                self.device.cmd_bind_pipeline(
                    cmd,
                    vk::PipelineBindPoint::COMPUTE,
                    pipeline.handle(),
                );
            }
            // SAFETY: the push range lies in the layout's compute range.
            unsafe {
                self.device.cmd_push_constants(
                    cmd,
                    self.light_layout.handle(),
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    bytes_of(&push),
                );
            }
            // SAFETY: the pipeline, sets and push constants are bound above. The arguments
            // were written by `light_classify` as rows of `tiles_x` groups (`x` ≤ `tiles_x`,
            // `y` ≤ the layout's rows, both far below `maxComputeWorkGroupCount`'s minimum of
            // 65,535 for any image the device can create) and the graph made them visible to
            // the indirect read; each group reads only its own list entry, below both the
            // class's count and `capacity`.
            unsafe {
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
        let (chunks, rest) = spirv.as_chunks::<4>();
        assert!(rest.is_empty(), "SPIR-V is whole words");
        let words: Vec<u32> = chunks
            .iter()
            .map(|&word| u32::from_le_bytes(word))
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
            core::mem::offset_of!(DeferredPush, tile_class),
            core::mem::offset_of!(DeferredPush, row_width),
            core::mem::offset_of!(DeferredPush, capacity),
        ]
        .map(|offset| u32::try_from(offset).expect("small"))
        .to_vec();
        for spirv in [
            moba_shaders::DEFERRED_LIGHT_COMP,
            moba_shaders::DEFERRED_LIGHT_EXTENDED_COMP,
        ] {
            assert_eq!(push_constant_offsets(spirv), expected);
        }
        assert_eq!(core::mem::size_of::<DeferredPush>(), 24);
    }

    /// The debug views are `deferred_light.comp`'s specialisation constant 0 (the pipelines
    /// are built with `constant_id` 0 = the view), not a push-constant branch; N2d's area
    /// mode is constant 1, which the forward stages read from the camera's `occlusion.z`
    /// (`lighting.glsl`'s `MOBA_AREA_MODE`), both from `LightingConfig::area_lights`.
    #[test]
    fn debug_views_and_the_area_mode_are_specialisation_constants() {
        // OpDecorate <id> SpecId n in both permutations.
        const OP_DECORATE: u32 = 71;
        const SPEC_ID: u32 = 1;
        let light = include_str!("../../../shaders/shaders/deferred_light.comp");
        assert!(light.contains("layout(constant_id = 0) const uint DEBUG_VIEW = 0u;"));
        assert!(light.contains("layout(constant_id = 1) const uint AREA_MODE = 0u;"));
        assert!(light.contains("#define MOBA_AREA_MODE AREA_MODE"));
        assert!(!light.contains("push.debug"));
        let lighting = include_str!("../../../shaders/shaders/include/lighting.glsl");
        assert!(lighting.contains("#define MOBA_AREA_MODE uint(camera.occlusion.z)"));
        for spirv in [
            moba_shaders::DEFERRED_LIGHT_COMP,
            moba_shaders::DEFERRED_LIGHT_EXTENDED_COMP,
        ] {
            let (chunks, _) = spirv.as_chunks::<4>();
            let words: Vec<u32> = chunks.iter().map(|&w| u32::from_le_bytes(w)).collect();
            let mut index = 5;
            let mut found = [false; 2];
            while index < words.len() {
                let count = (words[index] >> 16) as usize;
                if words[index] & 0xFFFF == OP_DECORATE
                    && count >= 4
                    && words[index + 2] == SPEC_ID
                    && words[index + 3] < 2
                {
                    found[words[index + 3] as usize] = true;
                }
                index += count.max(1);
            }
            assert_eq!(found, [true; 2], "SpecId 0 and 1 decorated");
        }
        for tier in crate::QualityTier::ALL {
            let config = crate::LightingConfig::for_tier(*tier);
            let uniform = crate::lighting::FrameLighting::new(
                &crate::SceneLighting::DEFAULT,
                config,
                1024,
                None,
                &crate::lighting::EnvironmentState::NONE,
            )
            .with_area_lights(config)
            .occlusion[2];
            assert_eq!(uniform, config.area_lights.shader_mode() as f32, "{tier:?}");
        }
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
    #[allow(clippy::too_many_lines)] // The scene, the reconstruction and the comparison.
    fn forward_and_deferred_visit_the_same_lights() {
        use crate::depth::DepthConvention;
        use crate::lights::{CELL_WORDS, GRID, LIGHT_LIST_VEC4S, Light, LightShape};
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
                lights.push(Light::new(centre + offset, 4.0 + random() * 4.0, Vec3::ONE));
            }
        }
        for _ in 0..180 {
            let position = Vec3::new(
                random() * 70.0 - 32.0,
                random() * 3.0,
                random() * 50.0 - 27.0,
            );
            lights.push(Light::new(position, 2.0 + random() * 10.0, Vec3::ONE));
        }
        // Every shape (ADR-0023 gate 3): the shapes are binned and early-outed by the same
        // sphere as points, so the sets must still agree. Small shapes keep the radii.
        for (index, light) in lights.iter_mut().enumerate() {
            light.shape = match index % 4 {
                0 => LightShape::Point,
                1 => LightShape::Sphere { radius: 0.3 },
                2 => LightShape::Capsule {
                    axis: Vec3::new(0.8, 0.1, -0.4),
                    radius: 0.2,
                },
                _ => LightShape::Rect {
                    normal: Vec3::new(0.0, -1.0, 0.2),
                    half_width: 0.6,
                    half_height: 0.4,
                },
            };
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
            let mut light_words = vec![[0.0; 4]; LIGHT_LIST_VEC4S];
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
            assert!(stats.binned > 0 && stats.area > 0, "{tier:?}");
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

    /// `light_classify`'s two-dimensional arguments: the atomic maxima of
    /// `min(slot + 1, tiles_x)` and `slot / tiles_x + 1` over the appended slots give
    /// `min(n, tiles_x)` by `ceil(n / tiles_x)` groups, which cover all `n` tiles with less
    /// than a row to spare, and neither dimension exceeds 65,535 for the largest image a
    /// device may create (32,768 × 32,768), where a one-dimensional count of 4,194,304 would.
    #[test]
    fn indirect_arguments_are_rows_of_tiles() {
        for (width, height) in [(1920, 1080), (6016, 3384), (7680, 4320), (32_768, 32_768)] {
            let layout = TileLayout::for_extent(ash::vk::Extent2D { width, height });
            let row = layout.tiles_x;
            for n in [1, 2, row - 1, row, row + 1, 3 * row + 7, layout.capacity] {
                let (mut x, mut y) = (0, 0);
                for slot in 0..n {
                    x = x.max((slot + 1).min(row));
                    y = y.max(slot / row + 1);
                }
                assert_eq!(
                    (x, y),
                    (n.min(row), n.div_ceil(row)),
                    "{width}x{height} n={n}"
                );
                assert!(x * y >= n && x * y - n < row, "{width}x{height} n={n}");
                assert!(x <= 65_535 && y <= 65_535, "{width}x{height} n={n}");
            }
        }
    }

    /// The tile layout's constants are the shaders' (`TILE_HEADER_WORDS`, the class order,
    /// the 16-pixel tile) and the buffer holds every tile of the extent in each of the two
    /// lists (the sky's tiles are not listed).
    #[test]
    fn tile_layout_matches_the_shaders() {
        let classify = include_str!("../../../shaders/shaders/slang/light_classify.slang");
        assert!(classify.contains("static const uint TILE_HEADER_WORDS = 12;"));
        assert!(classify.contains("static const uint TILE_COUNT_WORD = 9;"));
        assert!(classify.contains("static const uint TILE = 16;"));
        assert!(classify.contains("static const uint CLASS_SKY = 0;"));
        assert!(classify.contains("static const uint CLASS_STANDARD = 1;"));
        assert!(classify.contains("static const uint CLASS_EXTENDED = 2;"));
        assert!(classify.contains("[numthreads(16, 16, 1)]"));
        let light = include_str!("../../../shaders/shaders/deferred_light.comp");
        assert!(light.contains("local_size_x = 16, local_size_y = 16"));
        assert!(light.contains("const uint TILE_COUNT_WORD = 9u;"));
        // The sky is not appended, and the list offset is the class's minus one.
        assert!(classify.contains("if (tile_class == CLASS_SKY)"));
        assert!(
            classify.contains("tiles[TILE_HEADER_WORDS + (tile_class - 1) * push.capacity + slot]")
        );
        assert_eq!(TileLayout::COUNT_WORD, 9);
        assert_eq!(TileLayout::HEADER_WORDS as usize, TileLayout::RESET.len());
        assert_eq!(TileClass::Sky as u32, 0);
        let layout = TileLayout::for_extent(ash::vk::Extent2D {
            width: 1920,
            height: 1080,
        });
        assert_eq!((layout.tiles_x, layout.capacity), (120, 120 * 68));
        assert_eq!(layout.list_offset(TileClass::Standard), 12);
        assert_eq!(layout.list_offset(TileClass::Extended), 12 + 8160);
        assert_eq!(layout.bytes(), 4 * (12 + 2 * 8160));
        assert_eq!(TileLayout::args_offset(TileClass::Extended), 24);
        // Each reset dispatch is (0, 0, 1), and every count 0.
        for class in 0..3 {
            assert_eq!(&TileLayout::RESET[3 * class..3 * class + 3], &[0, 0, 1]);
            assert_eq!(
                TileLayout::RESET[TileLayout::COUNT_WORD as usize + class],
                0
            );
        }
    }
}
