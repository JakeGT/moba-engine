//! Scene pass: terrain, instanced units and GPU-skinned meshes into the HDR target.
//!
//! Every pipeline, each material permutation included, is built in [`ScenePass::new`] (or
//! [`ScenePass::rebuild_pipelines`] on a dev shader reload), never while drawing. A mesh
//! with a textured material ([`ScenePass::set_mesh_material`]) draws with the
//! `MATERIAL_TEXTURED` permutation and its material's descriptor set (set 2); every other
//! mesh keeps the vertex-colour look.

use core::sync::atomic::{AtomicU64, Ordering};

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};
use gpu_allocator::MemoryLocation;
use moba_assets::JointMatrix;

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::mesh::{self, GpuMesh};
use crate::pipeline::{self, PipelineSpec, VertexLayout};
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuBuffer, GraphicsPipeline, PipelineLayout,
};
use crate::scene::{BoneRange, CameraUniform, GpuInstance, MeshId, Scene};
use crate::shaders::ShaderSource;

/// Mesh slots, including the built-in cube in slot 0. Bucketing scratch is sized by this.
pub(crate) const MAX_MESHES: usize = 64;
/// Bytes per joint in the bone buffer: three `vec4` rows.
const BONE_STRIDE: u64 = core::mem::size_of::<JointMatrix>() as u64;
const CAMERA_UNIFORM_SIZE: u64 = core::mem::size_of::<CameraUniform>() as u64;
static BONES_TRUNCATED: AtomicU64 = AtomicU64::new(0);
static SKINNED_DROPPED: AtomicU64 = AtomicU64::new(0);

/// Joint matrices from [`Scene::bones`] dropped from the most recent frame because they
/// exceeded [`RenderConfig::max_bones`](crate::RenderConfig::max_bones).
#[must_use]
pub fn bones_truncated() -> u64 {
    BONES_TRUNCATED.load(Ordering::Relaxed)
}

/// Skinned-mesh instances not drawn in the most recent frame: no bone range, a range past
/// the uploaded bones, or more skinned draws than the per-frame budget.
#[must_use]
pub fn skinned_instances_dropped() -> u64 {
    SKINNED_DROPPED.load(Ordering::Relaxed)
}

/// Push-constant block of `skinned.vert`: the draw's bone range (16 bytes, vertex stage).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct SkinPush {
    first_bone: u32,
    joint_count: u32,
    reserved: [u32; 2],
}
const SKIN_PUSH_SIZE: u32 = core::mem::size_of::<SkinPush>() as u32;

/// One skinned instance: drawn alone so its push constant can select its bones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SkinnedDraw {
    slot: u32,
    instance: u32,
    first_bone: u32,
    joint_count: u32,
}

/// Per-frame-in-flight scene buffers and their descriptor sets.
struct SceneFrame {
    camera_buffer: GpuBuffer,
    instance_buffer: GpuBuffer,
    /// Joint matrices for this frame's skinned draws (set 1, binding 1).
    bone_buffer: GpuBuffer,
    camera_set: vk::DescriptorSet,
    instance_set: vk::DescriptorSet,
}

/// Attachments the scene pass renders into.
pub(crate) struct SceneTargets {
    pub(crate) color: vk::ImageView,
    pub(crate) depth: vk::ImageView,
    pub(crate) extent: vk::Extent2D,
    pub(crate) clear_color: [f32; 4],
}

/// Every scene pipeline: the vertex-colour set and the `MATERIAL_TEXTURED` permutations.
pub(crate) struct ScenePipelines {
    unit: GraphicsPipeline,
    unit_textured: GraphicsPipeline,
    terrain: GraphicsPipeline,
    /// `None` when skinning is disabled or the pipeline failed to build.
    skinned: Option<GraphicsPipeline>,
    /// `None` with `skinned`, or if the textured skinned permutation failed to build (then
    /// textured skinned meshes draw with `skinned`).
    skinned_textured: Option<GraphicsPipeline>,
}

impl ScenePipelines {
    fn build(
        device: &DeviceContext,
        shaders: &ShaderSource,
        layout: vk::PipelineLayout,
        color_format: vk::Format,
        depth_format: vk::Format,
        skinning: bool,
    ) -> RenderResult<Self> {
        let module = |name| shaders.module(device, name);
        let unit_vertex = module("unit.vert")?;
        let unit_fragment = module("unit.frag")?;
        let textured_vertex = module("unit_textured.vert")?;
        let textured_fragment = module("unit_textured.frag")?;
        let terrain_vertex = module("terrain.vert")?;
        let terrain_fragment = module("terrain.frag")?;
        let build = |vertex_layout, vertex: vk::ShaderModule, fragment: vk::ShaderModule| {
            pipeline::create(
                device,
                &PipelineSpec {
                    layout,
                    vertex_layout,
                    vertex,
                    fragment,
                    color_format,
                    depth_format,
                    cull_mode: vk::CullModeFlags::NONE,
                },
            )
            .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))
        };
        let unit = build(
            VertexLayout::Static,
            unit_vertex.handle(),
            unit_fragment.handle(),
        )?;
        let unit_textured = build(
            VertexLayout::StaticTextured,
            textured_vertex.handle(),
            textured_fragment.handle(),
        )?;
        let terrain = build(
            VertexLayout::Static,
            terrain_vertex.handle(),
            terrain_fragment.handle(),
        )?;
        let (skinned, skinned_textured) = if skinning {
            let skinned = module("skinned.vert").and_then(|vertex| {
                build(
                    VertexLayout::Skinned,
                    vertex.handle(),
                    unit_fragment.handle(),
                )
            });
            match skinned {
                Ok(skinned) => {
                    let textured = module("skinned_textured.vert").and_then(|vertex| {
                        build(
                            VertexLayout::SkinnedTextured,
                            vertex.handle(),
                            textured_fragment.handle(),
                        )
                    });
                    let textured = textured
                        .inspect_err(|error| {
                            eprintln!(
                                "[render] textured skinned pipeline unavailable ({error}); \
                                 textured skinned meshes use the vertex-colour look"
                            );
                        })
                        .ok();
                    (Some(skinned), textured)
                }
                Err(error) => {
                    eprintln!(
                        "[render] skinned pipeline unavailable ({error}); using static meshes"
                    );
                    (None, None)
                }
            }
        } else {
            (None, None)
        };
        Ok(Self {
            unit,
            unit_textured,
            terrain,
            skinned,
            skinned_textured,
        })
    }
}

/// Pipelines, meshes and per-frame buffers of the scene pass.
pub(crate) struct ScenePass {
    terrain_mesh: GpuMesh,
    /// Unit meshes: slot 0 is the cube, the rest come from `register_mesh`. Never shrinks,
    /// so a `MeshId` stays valid and no frame in flight can reference a freed mesh.
    meshes: Vec<GpuMesh>,
    /// Per mesh slot: its material's descriptor set, or null for the vertex-colour look.
    mesh_material_sets: [vk::DescriptorSet; MAX_MESHES],
    /// Per-frame bucketing scratch (instances per mesh, first instance, write cursor).
    mesh_counts: [u32; MAX_MESHES],
    mesh_starts: [u32; MAX_MESHES],
    mesh_cursors: [u32; MAX_MESHES],
    /// Per-frame skinned draws; capacity fixed at creation so pushing never reallocates.
    skinned_draws: Vec<SkinnedDraw>,
    /// Units uploaded for the frame being recorded.
    unit_count: usize,
    frames: Vec<SceneFrame>,
    _descriptor_pool: DescriptorPool,
    _camera_layout: DescriptorSetLayout,
    _instance_layout: DescriptorSetLayout,
    pipeline_layout: PipelineLayout,
    pipelines: ScenePipelines,
    color_format: vk::Format,
    depth_format: vk::Format,
    skinning: bool,
    max_instances: usize,
    max_bones: usize,
}

impl ScenePass {
    /// Builds the scene pipelines for `color_format`/`depth_format` and `frames` sets of
    /// per-frame buffers. `max_bones` is clamped to the storage-buffer range and returned
    /// through the pass (see [`Self::max_bones`]). `material_layout` is set 2 of every scene
    /// pipeline.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)] // Wiring belongs together.
    pub(crate) fn new(
        device: &DeviceContext,
        shaders: &ShaderSource,
        frames: usize,
        color_format: vk::Format,
        depth_format: vk::Format,
        material_layout: vk::DescriptorSetLayout,
        max_instances: usize,
        max_bones: usize,
        skinning: bool,
    ) -> RenderResult<Self> {
        let camera_binding = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)];
        let camera_layout = DescriptorSetLayout::new(device.device.clone(), &camera_binding)?;
        // Binding 0: instances; binding 1: joint matrices (read only by `skinned.vert`).
        let instance_binding = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX),
        ];
        let instance_layout = DescriptorSetLayout::new(device.device.clone(), &instance_binding)?;
        let frame_count = u32::try_from(frames).expect("frame count fits u32");
        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(frame_count),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(2 * frame_count),
        ];
        let descriptor_pool =
            DescriptorPool::new(device.device.clone(), &pool_sizes, 2 * frame_count)?;

        // One layout for every scene pipeline keeps the bound descriptor sets compatible
        // across them: sets 0/1 (camera, instances + bones) are bound once per pass and set 2
        // (the material) per textured mesh. Only `skinned*.vert` reads the push constant.
        let layouts = [
            camera_layout.handle(),
            instance_layout.handle(),
            material_layout,
        ];
        let skin_push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(SKIN_PUSH_SIZE)];
        let pipeline_layout =
            PipelineLayout::with_push_constants(device.device.clone(), &layouts, &skin_push)?;
        let pipelines = ScenePipelines::build(
            device,
            shaders,
            pipeline_layout.handle(),
            color_format,
            depth_format,
            skinning,
        )?;

        let terrain_mesh = mesh::upload(device, &mesh::CpuMesh::plane(300.0))?;
        let mut meshes = Vec::with_capacity(MAX_MESHES);
        meshes.push(mesh::upload(device, &mesh::CpuMesh::cube())?);

        // Both SSBOs are bound with `WHOLE_SIZE`, so each must fit one storage-buffer range.
        let range_limit = u64::from(device.max_storage_buffer_range);
        let instance_buffer_size = u64::try_from(core::mem::size_of::<GpuInstance>())
            .ok()
            .zip(u64::try_from(max_instances).ok())
            .and_then(|(size, count)| size.checked_mul(count))
            .filter(|&bytes| bytes <= range_limit)
            .ok_or_else(|| {
                RenderError::NoSuitableDevice(format!(
                    "max_instances {max_instances} exceeds the device's storage-buffer range ({range_limit} bytes)"
                ))
            })?;
        // Clamping the bone budget to the range also keeps `(first_bone + joint) * 3` in
        // skinned.vert far below `u32::MAX`.
        let bones_that_fit = usize::try_from(range_limit / BONE_STRIDE).unwrap_or(usize::MAX);
        let max_bones = if max_bones > bones_that_fit {
            eprintln!(
                "[render] max_bones {max_bones} exceeds the storage-buffer range; using {bones_that_fit}"
            );
            bones_that_fit
        } else {
            max_bones
        };
        // At least one joint so the descriptor always names a non-empty buffer.
        let bone_buffer_size =
            BONE_STRIDE * u64::try_from(max_bones.max(1)).expect("bounded by the range limit");

        let set_layouts: Vec<vk::DescriptorSetLayout> = (0..frames)
            .flat_map(|_| [camera_layout.handle(), instance_layout.handle()])
            .collect();
        let sets = descriptor_pool.allocate(&device.device, &set_layouts)?;
        let mut scene_frames = Vec::with_capacity(frames);
        for frame in 0..frames {
            let buffer = |name, size, usage| {
                GpuBuffer::new(
                    device.device.clone(),
                    device.allocator(),
                    name,
                    size,
                    usage,
                    MemoryLocation::CpuToGpu,
                )
            };
            let scene_frame = SceneFrame {
                camera_buffer: buffer(
                    "camera-uniform",
                    CAMERA_UNIFORM_SIZE,
                    vk::BufferUsageFlags::UNIFORM_BUFFER,
                )?,
                instance_buffer: buffer(
                    "unit-instances",
                    instance_buffer_size,
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                )?,
                bone_buffer: buffer(
                    "skin-bones",
                    bone_buffer_size,
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                )?,
                camera_set: sets[frame * 2],
                instance_set: sets[frame * 2 + 1],
            };
            write_descriptors(&device.device, &scene_frame);
            scene_frames.push(scene_frame);
        }

        Ok(Self {
            terrain_mesh,
            meshes,
            mesh_material_sets: [vk::DescriptorSet::null(); MAX_MESHES],
            mesh_counts: [0; MAX_MESHES],
            mesh_starts: [0; MAX_MESHES],
            mesh_cursors: [0; MAX_MESHES],
            skinned_draws: Vec::with_capacity(max_instances.min(max_bones)),
            unit_count: 0,
            frames: scene_frames,
            _descriptor_pool: descriptor_pool,
            _camera_layout: camera_layout,
            _instance_layout: instance_layout,
            pipeline_layout,
            pipelines,
            color_format,
            depth_format,
            skinning,
            max_instances,
            max_bones,
        })
    }

    /// Builds a replacement for every pipeline from `shaders` (dev shader reload) without
    /// installing it, so the caller can swap all passes at once or none. A set that lost a
    /// pipeline the current one has (a skinned permutation that no longer builds) is an
    /// error, so a reload never silently drops skinning.
    pub(crate) fn build_pipelines(
        &self,
        device: &DeviceContext,
        shaders: &ShaderSource,
    ) -> RenderResult<ScenePipelines> {
        let pipelines = ScenePipelines::build(
            device,
            shaders,
            self.pipeline_layout.handle(),
            self.color_format,
            self.depth_format,
            self.skinning,
        )?;
        if pipelines.skinned.is_some() != self.pipelines.skinned.is_some()
            || pipelines.skinned_textured.is_some() != self.pipelines.skinned_textured.is_some()
        {
            return Err(RenderError::Shader("a skinned pipeline failed to rebuild"));
        }
        Ok(pipelines)
    }

    /// Installs pipelines from [`Self::build_pipelines`]. The device must be idle.
    pub(crate) fn install_pipelines(&mut self, pipelines: ScenePipelines) {
        self.pipelines = pipelines;
    }

    /// Draws `mesh` with the material whose descriptor set is `set` (null for the
    /// vertex-colour look). Load time only.
    ///
    /// # Errors
    /// [`RenderError::InvalidMaterial`] for an unknown mesh, or a textured material on a mesh
    /// without texture coordinates.
    pub(crate) fn set_mesh_material(
        &mut self,
        mesh: MeshId,
        set: vk::DescriptorSet,
    ) -> RenderResult<()> {
        let gpu = self
            .meshes
            .get(mesh.index())
            .ok_or(RenderError::InvalidMaterial("unknown mesh id"))?;
        if set != vk::DescriptorSet::null() && !gpu.has_uvs {
            return Err(RenderError::InvalidMaterial(
                "a textured material needs a mesh with texture coordinates",
            ));
        }
        self.mesh_material_sets[mesh.index()] = set;
        Ok(())
    }

    /// The bone budget after clamping to the device's storage-buffer range.
    pub(crate) fn max_bones(&self) -> usize {
        self.max_bones
    }

    /// Whether the skinned pipeline exists.
    pub(crate) fn skinning_available(&self) -> bool {
        self.pipelines.skinned.is_some()
    }

    /// Number of registered meshes, including the built-in cube.
    pub(crate) fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    /// This frame's bone buffer, for the render graph's host-write → vertex-read barrier.
    pub(crate) fn bone_buffer(&self, frame: usize) -> vk::Buffer {
        self.frames[frame].bone_buffer.handle()
    }

    /// Uploads a static mesh into the next slot (load time only).
    pub(crate) fn register_mesh(
        &mut self,
        device: &DeviceContext,
        data: &crate::MeshData,
    ) -> RenderResult<MeshId> {
        let id = self.next_mesh_id()?;
        let cpu = mesh::CpuMesh::from_asset(data).ok_or(RenderError::InvalidMesh)?;
        self.meshes.push(mesh::upload(device, &cpu)?);
        Ok(id)
    }

    /// Uploads a skinned mesh into the next slot (load time only).
    pub(crate) fn register_skinned_mesh(
        &mut self,
        device: &DeviceContext,
        data: &crate::MeshData,
    ) -> RenderResult<MeshId> {
        if self.pipelines.skinned.is_none() {
            return Err(RenderError::SkinningUnavailable);
        }
        let id = self.next_mesh_id()?;
        let cpu = mesh::CpuSkinnedMesh::from_asset(data).ok_or(RenderError::InvalidMesh)?;
        self.meshes.push(mesh::upload_skinned(device, &cpu)?);
        Ok(id)
    }

    fn next_mesh_id(&self) -> RenderResult<MeshId> {
        if self.meshes.len() >= MAX_MESHES {
            return Err(RenderError::TooManyMeshes);
        }
        let id = u16::try_from(self.meshes.len()).map_err(|_| RenderError::TooManyMeshes)?;
        Ok(MeshId::from_index(id))
    }

    /// Writes this frame's camera, instances and bones and buckets the draws. The frame's
    /// fence must have been waited on.
    pub(crate) fn prepare(&mut self, frame: usize, scene: &Scene<'_>) -> RenderResult<()> {
        let unit_count = scene.units.len().min(self.max_instances);
        let bone_count = scene.bones.len().min(self.max_bones);
        BONES_TRUNCATED.store(
            u64::try_from(scene.bones.len() - bone_count).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.skinned_draws.clear();
        self.unit_count = unit_count;
        let mut skinned_dropped = 0u64;
        let target = &mut self.frames[frame];
        let uniform = CameraUniform::new(scene.camera, &scene.vision, scene.terrain_tint);
        target.camera_buffer.write_bytes(0, bytes_of(&uniform));
        if bone_count > 0 {
            let mapped = target
                .bone_buffer
                .mapped_slice_mut::<JointMatrix>(bone_count)
                .ok_or_else(|| {
                    RenderError::NoSuitableDevice(String::from("bone buffer is not host visible"))
                })?;
            mapped.copy_from_slice(&scene.bones[..bone_count]);
        }
        // `bone_count <= max_bones`, which the bone buffer was sized from.
        let uploaded_bones = u32::try_from(bone_count).unwrap_or(u32::MAX);
        if unit_count > 0 {
            let units = &scene.units[..unit_count];
            bucket_by_mesh(
                units,
                self.meshes.len(),
                &mut self.mesh_counts,
                &mut self.mesh_starts,
                &mut self.mesh_cursors,
            );
            let mapped = target
                .instance_buffer
                .mapped_slice_mut::<GpuInstance>(unit_count)
                .ok_or_else(|| {
                    RenderError::NoSuitableDevice(String::from(
                        "instance buffer is not host visible",
                    ))
                })?;
            // Each mesh's instances land contiguously at its `mesh_starts` slot, so one draw
            // per mesh reads them through `firstInstance` (gl_InstanceIndex).
            for unit in units {
                let slot = mesh_slot(unit.mesh, self.meshes.len());
                let cursor = &mut self.mesh_cursors[slot];
                let instance = *cursor;
                mapped[instance as usize] = GpuInstance::from(*unit);
                *cursor += 1;
                let joints = self.meshes[slot].joint_count;
                if joints == 0 {
                    continue;
                }
                // A skinned mesh is only drawn with a bone range inside this frame's upload;
                // the capacity check keeps `push` from reallocating.
                match skinned_draw(unit.bones, joints, uploaded_bones, slot, instance) {
                    Some(draw) if self.skinned_draws.len() < self.skinned_draws.capacity() => {
                        self.skinned_draws.push(draw);
                    }
                    _ => skinned_dropped += 1,
                }
            }
        }
        SKINNED_DROPPED.store(skinned_dropped, Ordering::Relaxed);
        Ok(())
    }

    /// Records the pass: clear, terrain, static meshes (one instanced draw per mesh:
    /// vertex-colour meshes, then textured meshes with their material sets), then one draw per
    /// skinned instance (vertex-colour, then textured). The graph has already transitioned
    /// the targets.
    #[allow(clippy::too_many_lines)] // One explicit, ordered draw stream.
    pub(crate) fn record(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        targets: &SceneTargets,
    ) {
        let extent = targets.extent;
        let color_attachment = vk::RenderingAttachmentInfo::default()
            .image_view(targets.color)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: targets.clear_color,
                },
            });
        // Nothing samples depth yet; a later reader switches this to STORE.
        let depth_attachment = vk::RenderingAttachmentInfo::default()
            .image_view(targets.depth)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .clear_value(vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            });
        let render_info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            })
            .layer_count(1)
            .color_attachments(core::slice::from_ref(&color_attachment))
            .depth_attachment(&depth_attachment);
        device.cmd_begin_rendering(command_buffer, &render_info);
        super::set_viewport_scissor(
            device,
            command_buffer,
            super::flipped_viewport(extent),
            extent,
        );

        let scene_frame = &self.frames[frame];
        let descriptor_sets = [scene_frame.camera_set, scene_frame.instance_set];
        // SAFETY: both sets were allocated from this frame's layout and the pipeline layout
        // declares them at indices 0 and 1.
        unsafe {
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout.handle(),
                0,
                &descriptor_sets,
                &[],
            );
        }

        // Terrain.
        bind_pipeline(device, command_buffer, &self.pipelines.terrain);
        bind_mesh(device, command_buffer, &self.terrain_mesh);
        // SAFETY: the mesh is uploaded and drawable; its index buffer holds `index_count`
        // in-range `u32` indices.
        unsafe {
            device.device.cmd_draw_indexed(
                command_buffer,
                self.terrain_mesh.index_count,
                1,
                0,
                0,
                0,
            );
        }

        if self.unit_count > 0 {
            for textured in [false, true] {
                self.record_static(device, command_buffer, textured);
            }
        }

        if !self.skinned_draws.is_empty()
            && let Some(skinned) = &self.pipelines.skinned
        {
            // Textured skinned meshes fall back to the vertex-colour pipeline if their
            // permutation is missing.
            let textured = self.pipelines.skinned_textured.as_ref();
            self.record_skinned(device, command_buffer, skinned, false);
            self.record_skinned(device, command_buffer, textured.unwrap_or(skinned), true);
        }

        device.cmd_end_rendering(command_buffer);
    }

    /// One instanced draw per static mesh whose material is (`textured`) or is not textured.
    fn record_static(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        textured: bool,
    ) {
        let pipeline = if textured {
            &self.pipelines.unit_textured
        } else {
            &self.pipelines.unit
        };
        let mut bound = false;
        {
            for (slot, mesh) in self.meshes.iter().enumerate() {
                let count = self.mesh_counts[slot];
                let set = self.mesh_material_sets[slot];
                // Skinned meshes are drawn per instance below, with their own pipeline.
                if count == 0
                    || mesh.joint_count > 0
                    || (set != vk::DescriptorSet::null()) != textured
                {
                    continue;
                }
                if !bound {
                    bind_pipeline(device, command_buffer, pipeline);
                    bound = true;
                }
                if textured {
                    bind_material(device, command_buffer, self.pipeline_layout.handle(), set);
                }
                bind_mesh(device, command_buffer, mesh);
                // SAFETY: indices: `index_count == indices.len()`, so firstIndex 0 +
                // indexCount fits the index buffer, and every index is below the vertex
                // count (cube by construction; registered meshes re-checked in
                // `CpuMesh::from_asset`), so no vertex fetch leaves the vertex buffer.
                // Instances: `mesh_starts[slot] + count <= unit_count <= max_instances`, the
                // instance buffer's element capacity, by construction in `bucket_by_mesh`.
                unsafe {
                    device.device.cmd_draw_indexed(
                        command_buffer,
                        mesh.index_count,
                        count,
                        0,
                        0,
                        self.mesh_starts[slot],
                    );
                }
            }
        }
    }

    /// One draw per skinned instance whose mesh's material is (`textured`) or is not
    /// textured, with `pipeline`.
    fn record_skinned(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        pipeline: &GraphicsPipeline,
        textured: bool,
    ) {
        {
            let mut bound_pipeline = false;
            let mut bound_slot = u32::MAX;
            for draw in &self.skinned_draws {
                let mesh = &self.meshes[draw.slot as usize];
                let set = self.mesh_material_sets[draw.slot as usize];
                if (set != vk::DescriptorSet::null()) != textured {
                    continue;
                }
                if !bound_pipeline {
                    // The pipeline was created with the shared layout the descriptor sets
                    // are bound with, so sets 0/1 stay valid for it.
                    bind_pipeline(device, command_buffer, pipeline);
                    bound_pipeline = true;
                }
                if draw.slot != bound_slot {
                    bound_slot = draw.slot;
                    if textured {
                        bind_material(device, command_buffer, self.pipeline_layout.handle(), set);
                    }
                    // Skinned meshes (joint_count > 0) hold `SkinnedVertex` data, the layout
                    // of the bound skinned pipeline.
                    bind_mesh(device, command_buffer, mesh);
                }
                let push = SkinPush {
                    first_bone: draw.first_bone,
                    joint_count: draw.joint_count,
                    reserved: [0; 2],
                };
                // SAFETY: the shared pipeline layout declares exactly this 16-byte
                // vertex-stage range at offset 0.
                unsafe {
                    device.device.cmd_push_constants(
                        command_buffer,
                        self.pipeline_layout.handle(),
                        vk::ShaderStageFlags::VERTEX,
                        0,
                        bytes_of(&push),
                    );
                }
                // SAFETY: indices are in range for the vertex buffer (re-checked in
                // `CpuSkinnedMesh::from_asset`). `draw.instance < unit_count <= max_instances`
                // (it is a bucketed instance slot). Bones: `skinned_draw` guarantees
                // `joint_count >= 1` and `first_bone + joint_count <= bone_count <=
                // max_bones`, the bone buffer's capacity, and the shader clamps each joint
                // index below `joint_count`, so every bone read stays inside this frame's
                // upload.
                unsafe {
                    device.device.cmd_draw_indexed(
                        command_buffer,
                        mesh.index_count,
                        1,
                        0,
                        0,
                        draw.instance,
                    );
                }
            }
        }
    }
}

/// Binds a scene pipeline.
fn bind_pipeline(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    pipeline: &GraphicsPipeline,
) {
    // SAFETY: the pipeline is live and was created with the scene pass's shared layout, so
    // the descriptor sets bound with that layout stay valid for it.
    unsafe {
        device.device.cmd_bind_pipeline(
            command_buffer,
            vk::PipelineBindPoint::GRAPHICS,
            pipeline.handle(),
        );
    }
}

/// Binds a material's descriptor set at set 2.
fn bind_material(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    layout: vk::PipelineLayout,
    set: vk::DescriptorSet,
) {
    // SAFETY: `set` was allocated at load time with the material layout, which the shared
    // pipeline layout declares at index 2; its buffer and views live as long as the
    // renderer, and it is never updated after creation.
    unsafe {
        device.device.cmd_bind_descriptor_sets(
            command_buffer,
            vk::PipelineBindPoint::GRAPHICS,
            layout,
            2,
            &[set],
            &[],
        );
    }
}

/// Binds a mesh's vertex and `u32` index buffers.
fn bind_mesh(device: &DeviceContext, command_buffer: vk::CommandBuffer, mesh: &GpuMesh) {
    // SAFETY: the mesh buffers live as long as the renderer and match the bound pipeline's
    // vertex layout (callers pair static meshes with static pipelines, skinned with skinned).
    unsafe {
        device
            .device
            .cmd_bind_vertex_buffers(command_buffer, 0, &[mesh.vertices.handle()], &[0]);
    }
    // SAFETY: the buffer is live, was created with INDEX_BUFFER usage and holds `u32` indices.
    unsafe {
        device.device.cmd_bind_index_buffer(
            command_buffer,
            mesh.indices.handle(),
            0,
            vk::IndexType::UINT32,
        );
    }
}

fn write_descriptors(device: &ash::Device, frame: &SceneFrame) {
    let camera_info = [vk::DescriptorBufferInfo::default()
        .buffer(frame.camera_buffer.handle())
        .offset(0)
        .range(CAMERA_UNIFORM_SIZE)];
    let instance_info = [vk::DescriptorBufferInfo::default()
        .buffer(frame.instance_buffer.handle())
        .offset(0)
        .range(vk::WHOLE_SIZE)];
    let bone_info = [vk::DescriptorBufferInfo::default()
        .buffer(frame.bone_buffer.handle())
        .offset(0)
        .range(vk::WHOLE_SIZE)];
    let writes = [
        vk::WriteDescriptorSet::default()
            .dst_set(frame.camera_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .buffer_info(&camera_info),
        vk::WriteDescriptorSet::default()
            .dst_set(frame.instance_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .buffer_info(&instance_info),
        vk::WriteDescriptorSet::default()
            .dst_set(frame.instance_set)
            .dst_binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .buffer_info(&bone_info),
    ];
    // SAFETY: both descriptor sets were allocated from a compatible pool and the buffer info
    // entries reference live buffers.
    unsafe { device.update_descriptor_sets(&writes, &[]) };
}

/// Maps an instance's mesh to a live slot; unknown ids draw as the cube.
fn mesh_slot(mesh: MeshId, mesh_count: usize) -> usize {
    let index = mesh.index();
    if index < mesh_count { index } else { 0 }
}

/// The draw for one skinned instance, or `None` if its bone range is empty or reaches past
/// the `uploaded` joint matrices. The pushed joint count is at most the mesh's.
fn skinned_draw(
    bones: BoneRange,
    mesh_joints: u32,
    uploaded: u32,
    slot: usize,
    instance: u32,
) -> Option<SkinnedDraw> {
    if bones.count == 0 || mesh_joints == 0 {
        return None;
    }
    if bones.first.checked_add(bones.count)? > uploaded {
        return None;
    }
    Some(SkinnedDraw {
        slot: u32::try_from(slot).ok()?,
        instance,
        first_bone: bones.first,
        joint_count: bones.count.min(mesh_joints),
    })
}

/// Counting-sort bucketing of `units` by mesh, without allocating.
///
/// On return `counts[m]` is the number of instances of mesh `m`, `starts[m]` the first
/// instance slot of that mesh's contiguous run, and `cursors[m] == starts[m]` (the write
/// cursor the caller advances while scattering). Runs are in mesh-slot order and keep scene
/// order within a mesh, so a scene that only uses the cube is laid out exactly as before.
fn bucket_by_mesh(
    units: &[crate::UnitInstance],
    mesh_count: usize,
    counts: &mut [u32; MAX_MESHES],
    starts: &mut [u32; MAX_MESHES],
    cursors: &mut [u32; MAX_MESHES],
) {
    counts.fill(0);
    for unit in units {
        counts[mesh_slot(unit.mesh, mesh_count)] += 1;
    }
    let mut next = 0u32;
    for slot in 0..MAX_MESHES {
        starts[slot] = next;
        next += counts[slot];
    }
    *cursors = *starts;
}

#[cfg(test)]
mod tests {
    use super::{MAX_MESHES, SkinPush, SkinnedDraw, bucket_by_mesh, skinned_draw};
    use crate::{BoneRange, MeshId, UnitInstance};
    use moba_core::math::{Mat4, Vec4};

    #[test]
    fn skin_push_is_16_bytes() {
        assert_eq!(core::mem::size_of::<SkinPush>(), 16);
    }

    #[test]
    fn skinned_draws_stay_inside_the_uploaded_bones() {
        let draw = skinned_draw(BoneRange::new(64, 64), 7, 128, 3, 9).expect("in range");
        assert_eq!(
            draw,
            SkinnedDraw {
                slot: 3,
                instance: 9,
                first_bone: 64,
                // Never more than the mesh's joints.
                joint_count: 7,
            }
        );
        assert_eq!(skinned_draw(BoneRange::NONE, 7, 128, 3, 9), None);
        assert_eq!(skinned_draw(BoneRange::new(122, 7), 7, 128, 3, 9), None);
        assert_eq!(
            skinned_draw(BoneRange::new(u32::MAX, 2), 7, 128, 3, 9),
            None
        );
        assert_eq!(skinned_draw(BoneRange::new(0, 7), 0, 128, 3, 9), None);
        let partial = skinned_draw(BoneRange::new(0, 3), 7, 3, 1, 0).expect("short range");
        assert_eq!(partial.joint_count, 3);
    }

    fn unit(mesh: u16) -> UnitInstance {
        UnitInstance::new(Mat4::IDENTITY, Vec4::ONE).with_mesh(MeshId::from_index(mesh))
    }

    #[test]
    fn buckets_are_contiguous_and_unknown_meshes_draw_as_cubes() {
        let units = [unit(2), unit(0), unit(1), unit(2), unit(9), unit(0)];
        let mut counts = [0; MAX_MESHES];
        let mut starts = [0; MAX_MESHES];
        let mut cursors = [0; MAX_MESHES];
        bucket_by_mesh(&units, 3, &mut counts, &mut starts, &mut cursors);
        // Mesh 9 is not registered (3 meshes), so it joins the cube bucket.
        assert_eq!(&counts[..4], &[3, 1, 2, 0]);
        assert_eq!(&starts[..4], &[0, 3, 4, 6]);
        assert_eq!(cursors, starts);
        assert_eq!(starts[MAX_MESHES - 1], 6);
    }

    #[test]
    fn cube_only_scenes_keep_one_run_in_scene_order() {
        let units = [unit(0); 5];
        let mut counts = [7; MAX_MESHES];
        let mut starts = [7; MAX_MESHES];
        let mut cursors = [7; MAX_MESHES];
        bucket_by_mesh(&units, 1, &mut counts, &mut starts, &mut cursors);
        assert_eq!(counts[0], 5);
        assert_eq!(starts[0], 0);
        assert!(counts[1..].iter().all(|&count| count == 0));
    }
}
