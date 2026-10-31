//! Procedural meshes and one-time GPU upload.

use ash::vk;
use bytemuck::{Pod, Zeroable};
use gpu_allocator::MemoryLocation;

use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::resources::{CommandPool, GpuBuffer};

/// Vertex layout of the static scene pipelines (unit and terrain), stride 48. `uv` is
/// `(0, 0)` for meshes without texture coordinates and `tangent` (glTF convention: xyz along
/// +u, w the bitangent sign) is from the file or generated from the UVs at registration;
/// only textured permutations read them.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct Vertex {
    pub(crate) position: [f32; 3],
    pub(crate) normal: [f32; 3],
    pub(crate) uv: [f32; 2],
    pub(crate) tangent: [f32; 4],
}

/// Vertex layout of the skinned pipeline (`skinned.vert`), stride 56: position and normal
/// as in [`Vertex`], then four `R8G8B8A8_UINT` joints, four `R8G8B8A8_UNORM` weights, the
/// UV and the tangent.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct SkinnedVertex {
    pub(crate) position: [f32; 3],
    pub(crate) normal: [f32; 3],
    pub(crate) joints: [u8; 4],
    pub(crate) weights: [u8; 4],
    pub(crate) uv: [f32; 2],
    pub(crate) tangent: [f32; 4],
}

/// The tangent used when a mesh has none (no UVs): +X, right-handed. Never read by the
/// vertex-colour permutations.
const DEFAULT_TANGENT: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

/// Per-vertex tangents for `data`: the file's (renormalised), else generated from the UVs
/// (`MikkTSpace`, `MeshData::generate_tangents`), else [`DEFAULT_TANGENT`].
fn tangents(data: &moba_assets::MeshData) -> Vec<[f32; 4]> {
    let count = data.vertex_count();
    let normalised = |tangent: &[f32; 4]| {
        let [x, y, z, w] = *tangent;
        let length = (x * x + y * y + z * z).sqrt();
        let sign = if w < 0.0 { -1.0 } else { 1.0 };
        if length.is_finite() && length > 1e-6 {
            [x / length, y / length, z / length, sign]
        } else {
            // Zero (a degenerate file tangent): the shader keeps the geometric normal.
            [0.0, 0.0, 0.0, sign]
        }
    };
    match data.tangents() {
        Some(tangents) if tangents.len() == count => tangents.iter().map(normalised).collect(),
        _ => data
            .generate_tangents()
            .filter(|tangents| tangents.len() == count)
            .map_or_else(
                || vec![DEFAULT_TANGENT; count],
                |tangents| tangents.iter().map(normalised).collect(),
            ),
    }
}

/// CPU-side interleaved skinned mesh, ready for [`upload_bytes`].
pub(crate) struct CpuSkinnedMesh {
    pub(crate) vertices: Vec<SkinnedVertex>,
    /// LOD0's indices, then every baked level's (see [`CpuMesh::lods`]).
    pub(crate) indices: Vec<u32>,
    pub(crate) joint_count: u32,
    pub(crate) has_uvs: bool,
    pub(crate) lods: MeshLodTable,
    /// Culling sphere over the clips' sampled poses ([`moba_assets::MeshData::cull_sphere`]).
    pub(crate) bounds: [f32; 4],
}

impl CpuSkinnedMesh {
    /// Interleaves a validated skinned asset into [`SkinnedVertex`].
    ///
    /// Returns `None` unless the mesh has a skin with 1..=`MAX_JOINTS` joints, one
    /// joint/weight entry per vertex, every joint index below the joint count, and passes
    /// [`CpuMesh::from_asset`]'s index checks. `moba_assets` guarantees all of this; the
    /// renderer re-checks at load time because the shader's bone reads depend on it (the
    /// shader also clamps joints to the draw's range).
    pub(crate) fn from_asset(data: &moba_assets::MeshData) -> Option<Self> {
        let skin = data.skin()?;
        let joint_count = skin.skeleton().joint_count();
        let vertex_count = data.positions().len();
        if joint_count == 0
            || joint_count > moba_assets::MAX_JOINTS
            || skin.joints().len() != vertex_count
            || skin.weights().len() != vertex_count
            || skin
                .joints()
                .iter()
                .flatten()
                .any(|&joint| usize::from(joint) >= joint_count)
        {
            return None;
        }
        let base = CpuMesh::from_asset(data)?;
        let vertices = base
            .vertices
            .iter()
            .zip(skin.joints().iter().zip(skin.weights()))
            .map(|(vertex, (&joints, &weights))| SkinnedVertex {
                position: vertex.position,
                normal: vertex.normal,
                joints,
                weights,
                uv: vertex.uv,
                tangent: vertex.tangent,
            })
            .collect();
        Some(Self {
            vertices,
            indices: base.indices,
            joint_count: u32::try_from(joint_count).ok()?,
            has_uvs: base.has_uvs,
            lods: base.lods,
            bounds: data.cull_sphere(),
        })
    }
}

/// Levels per mesh: LOD0 plus up to [`moba_assets::MAX_MESH_LODS`] baked levels (R8).
pub(crate) const MAX_LODS: usize = 1 + moba_assets::MAX_MESH_LODS;

/// One level's index range in the mesh's index buffer and its object-space error.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LodRange {
    pub(crate) first_index: u32,
    pub(crate) index_count: u32,
    /// Deviation from LOD0 in model units (0 for LOD0).
    pub(crate) error: f32,
}

/// A mesh's levels (R8, ADR-0018): level 0 is the full mesh, levels `1..count` the baked
/// index-only levels appended after it in the same index buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MeshLodTable {
    pub(crate) levels: [LodRange; MAX_LODS],
    pub(crate) count: u8,
}

impl MeshLodTable {
    /// A single level of `index_count` indices.
    pub(crate) fn single(index_count: u32) -> Self {
        let mut levels = [LodRange::default(); MAX_LODS];
        levels[0].index_count = index_count;
        Self { levels, count: 1 }
    }

    /// Level `level`, clamped to the coarsest one.
    pub(crate) fn level(&self, level: usize) -> LodRange {
        self.levels[level.min(usize::from(self.count.max(1)) - 1)]
    }
}

/// CPU-side interleaved mesh, ready for [`upload`].
pub(crate) struct CpuMesh {
    pub(crate) vertices: Vec<Vertex>,
    /// LOD0's indices, then every baked level's, back to back ([`Self::lods`] has the ranges).
    pub(crate) indices: Vec<u32>,
    /// Whether the UVs are real texture coordinates (a textured material needs them).
    pub(crate) has_uvs: bool,
    pub(crate) lods: MeshLodTable,
}

/// Device-local vertex/index buffers.
pub(crate) struct GpuMesh {
    pub(crate) vertices: GpuBuffer,
    pub(crate) indices: GpuBuffer,
    /// LOD0's index count (the first `index_count` indices of the buffer).
    pub(crate) index_count: u32,
    /// Every level's range (R8); level 0 is `0..index_count`.
    pub(crate) lods: MeshLodTable,
    /// Model-space bounding sphere `[x, y, z, radius]` for culling (R8, ADR-0018); skinned
    /// meshes' radius is widened for poses, VAT meshes' covers every baked frame.
    pub(crate) bounds: [f32; 4],
    /// `0` for a static [`Vertex`] mesh; otherwise the skin's joint count and the vertices
    /// are [`SkinnedVertex`].
    pub(crate) joint_count: u32,
    /// Whether the mesh has texture coordinates.
    pub(crate) has_uvs: bool,
}

impl CpuMesh {
    /// Unit cube centred at the origin, one metre tall before scaling.
    pub(crate) fn cube() -> Self {
        let mut vertices = Vec::with_capacity(24);
        let mut indices = Vec::with_capacity(36);
        let faces = [
            (
                [1.0, 0.0, 0.0],
                [
                    [0.5, -0.5, -0.5],
                    [0.5, -0.5, 0.5],
                    [0.5, 0.5, 0.5],
                    [0.5, 0.5, -0.5],
                ],
            ),
            (
                [-1.0, 0.0, 0.0],
                [
                    [-0.5, -0.5, 0.5],
                    [-0.5, -0.5, -0.5],
                    [-0.5, 0.5, -0.5],
                    [-0.5, 0.5, 0.5],
                ],
            ),
            (
                [0.0, 1.0, 0.0],
                [
                    [-0.5, 0.5, -0.5],
                    [0.5, 0.5, -0.5],
                    [0.5, 0.5, 0.5],
                    [-0.5, 0.5, 0.5],
                ],
            ),
            (
                [0.0, -1.0, 0.0],
                [
                    [-0.5, -0.5, 0.5],
                    [0.5, -0.5, 0.5],
                    [0.5, -0.5, -0.5],
                    [-0.5, -0.5, -0.5],
                ],
            ),
            (
                [0.0, 0.0, 1.0],
                [
                    [-0.5, -0.5, 0.5],
                    [-0.5, 0.5, 0.5],
                    [0.5, 0.5, 0.5],
                    [0.5, -0.5, 0.5],
                ],
            ),
            (
                [0.0, 0.0, -1.0],
                [
                    [0.5, -0.5, -0.5],
                    [0.5, 0.5, -0.5],
                    [-0.5, 0.5, -0.5],
                    [-0.5, -0.5, -0.5],
                ],
            ),
        ];
        let corner_uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        for (normal, corners) in faces {
            let base = u32::try_from(vertices.len()).expect("vertex count fits u32");
            for (corner, uv) in corners.into_iter().zip(corner_uvs) {
                vertices.push(Vertex {
                    position: corner,
                    normal,
                    uv,
                    tangent: DEFAULT_TANGENT,
                });
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        Self::with_generated_tangents(vertices, indices)
    }

    /// Fills the tangents of a built-in mesh from its UVs (the same generator as assets).
    fn with_generated_tangents(mut vertices: Vec<Vertex>, indices: Vec<u32>) -> Self {
        let asset = moba_assets::MeshData::new(
            vertices.iter().map(|vertex| vertex.position).collect(),
            vertices.iter().map(|vertex| vertex.normal).collect(),
            indices.clone(),
        )
        .and_then(|mesh| mesh.with_uvs(vertices.iter().map(|vertex| vertex.uv).collect()));
        if let Ok(asset) = asset {
            for (vertex, tangent) in vertices.iter_mut().zip(tangents(&asset)) {
                vertex.tangent = tangent;
            }
        }
        let lods = MeshLodTable::single(u32::try_from(indices.len()).expect("built-in mesh"));
        Self {
            vertices,
            indices,
            has_uvs: true,
            lods,
        }
    }

    /// Ground plane of the given size, centred at the origin.
    pub(crate) fn plane(size: f32) -> Self {
        let half = size * 0.5;
        // One UV unit per 8 m, for a later tiling terrain material.
        let corner = |x: f32, z: f32| Vertex {
            position: [x, 0.0, z],
            normal: [0.0, 1.0, 0.0],
            uv: [x / 8.0, z / 8.0],
            tangent: DEFAULT_TANGENT,
        };
        let vertices = vec![
            corner(-half, -half),
            corner(half, -half),
            corner(half, half),
            corner(-half, half),
        ];
        let indices = vec![0, 1, 2, 0, 2, 3];
        Self::with_generated_tangents(vertices, indices)
    }

    /// Interleaves a validated asset mesh into the shared `{position, normal}` layout.
    ///
    /// Returns `None` unless the mesh is a non-empty triangle list with one normal per
    /// position and every index below the vertex count. `moba_assets::MeshData` already
    /// guarantees this; the renderer re-checks (load time, O(n)) because the indexed draws'
    /// memory safety depends on it and the type lives in another crate.
    pub(crate) fn from_asset(data: &moba_assets::MeshData) -> Option<Self> {
        let vertex_count = data.positions().len();
        if vertex_count == 0
            || data.normals().len() != vertex_count
            || data.indices().is_empty()
            || !data.indices().len().is_multiple_of(3)
            || data
                .indices()
                .iter()
                .any(|&index| index as usize >= vertex_count)
        {
            return None;
        }
        let uvs = data.uvs().filter(|uvs| uvs.len() == vertex_count);
        let tangents = tangents(data);
        let vertices = data
            .positions()
            .iter()
            .zip(data.normals())
            .zip(&tangents)
            .enumerate()
            .map(|(index, ((&position, &normal), &tangent))| Vertex {
                position,
                normal,
                uv: uvs.map_or([0.0, 0.0], |uvs| uvs[index]),
                tangent,
            })
            .collect();
        let (indices, lods) = with_levels(data)?;
        Some(Self {
            vertices,
            indices,
            has_uvs: uvs.is_some(),
            lods,
        })
    }
}

/// `data`'s LOD0 indices followed by its baked levels (R8), and their ranges. A level that is
/// empty, not a triangle list or indexes past the vertices is dropped with every level after
/// it (`MeshData` already guarantees this; the renderer re-checks because the draws' memory
/// safety depends on it).
fn with_levels(data: &moba_assets::MeshData) -> Option<(Vec<u32>, MeshLodTable)> {
    let vertex_count = data.positions().len();
    let lod0 = data.indices();
    let extra: usize = data.lods().iter().map(|level| level.indices().len()).sum();
    let mut indices = Vec::with_capacity(lod0.len() + extra);
    indices.extend_from_slice(lod0);
    let mut table = MeshLodTable::single(u32::try_from(lod0.len()).ok()?);
    for level in data.lods().iter().take(MAX_LODS - 1) {
        let level_indices = level.indices();
        if level_indices.is_empty()
            || !level_indices.len().is_multiple_of(3)
            || level_indices
                .iter()
                .any(|&index| index as usize >= vertex_count)
            || !level.error().is_finite()
        {
            break;
        }
        let slot = usize::from(table.count);
        table.levels[slot] = LodRange {
            first_index: u32::try_from(indices.len()).ok()?,
            index_count: u32::try_from(level_indices.len()).ok()?,
            error: level.error().max(0.0),
        };
        table.count += 1;
        indices.extend_from_slice(level_indices);
    }
    Some((indices, table))
}

fn sphere_of<T>(vertices: &[T], position: impl Fn(&T) -> [f32; 3]) -> [f32; 4] {
    let positions: Vec<[f32; 3]> = vertices.iter().map(position).collect();
    moba_assets::bounding_sphere(&positions)
}

/// Uploads a static mesh to device-local memory through staging buffers.
pub(crate) fn upload(device: &DeviceContext, data: &CpuMesh) -> RenderResult<GpuMesh> {
    let bounds = sphere_of(&data.vertices, |vertex| vertex.position);
    upload_bytes(
        device,
        bytemuck::cast_slice(&data.vertices),
        &data.indices,
        MeshShape {
            joint_count: 0,
            has_uvs: data.has_uvs,
            lods: data.lods,
            bounds,
        },
    )
}

/// Uploads a skinned mesh to device-local memory through staging buffers.
pub(crate) fn upload_skinned(
    device: &DeviceContext,
    data: &CpuSkinnedMesh,
) -> RenderResult<GpuMesh> {
    upload_bytes(
        device,
        bytemuck::cast_slice(&data.vertices),
        &data.indices,
        MeshShape {
            joint_count: data.joint_count,
            has_uvs: data.has_uvs,
            lods: data.lods,
            bounds: data.bounds,
        },
    )
}

/// What [`upload_bytes`] records about a mesh besides its buffers.
#[derive(Clone, Copy)]
struct MeshShape {
    joint_count: u32,
    has_uvs: bool,
    lods: MeshLodTable,
    bounds: [f32; 4],
}

/// Uploads raw interleaved vertex bytes and `u32` indices (the R3b water mesh).
pub(crate) fn upload_raw(
    device: &DeviceContext,
    vertex_bytes: &[u8],
    indices: &[u32],
) -> RenderResult<GpuMesh> {
    let index_count = u32::try_from(indices.len()).map_err(|_| {
        crate::RenderError::NoSuitableDevice(String::from("water mesh has too many indices"))
    })?;
    upload_bytes(
        device,
        vertex_bytes,
        indices,
        MeshShape {
            joint_count: 0,
            has_uvs: false,
            lods: MeshLodTable::single(index_count),
            // Water is drawn whole, never culled by its sphere.
            bounds: [0.0, 0.0, 0.0, f32::MAX],
        },
    )
}

/// Uploads `bytes` into a new device-local storage buffer read by vertex shaders (the R3b
/// foliage instances), through a staging buffer, and waits for the copy. Load time only.
pub(crate) fn upload_storage(
    device: &DeviceContext,
    name: &str,
    bytes: &[u8],
) -> RenderResult<GpuBuffer> {
    let allocator = device.allocator();
    let mut staging = GpuBuffer::new(
        device.device.clone(),
        allocator.clone(),
        "storage-staging",
        bytes.len() as u64,
        vk::BufferUsageFlags::TRANSFER_SRC,
        MemoryLocation::CpuToGpu,
    )?;
    staging.write_bytes(0, bytes);
    crate::memory::count_upload();
    let buffer = GpuBuffer::new(
        device.device.clone(),
        allocator,
        name,
        bytes.len() as u64,
        vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
        MemoryLocation::GpuOnly,
    )?;
    let pool = CommandPool::new(device.device.clone(), device.graphics_family)?;
    let command = pool.allocate_one()?;
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: the command buffer is allocated and not recording.
    unsafe { device.device.begin_command_buffer(command, &begin) }?;
    let copy = vk::BufferCopy::default().size(bytes.len() as u64);
    // SAFETY: both buffers are valid, large enough, and created for transfer use.
    unsafe {
        device.device.cmd_copy_buffer(
            command,
            staging.handle(),
            buffer.handle(),
            core::slice::from_ref(&copy),
        );
    }
    let barrier = vk::BufferMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::VERTEX_SHADER)
        .dst_access_mask(vk::AccessFlags2::SHADER_STORAGE_READ)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .buffer(buffer.handle())
        .offset(0)
        .size(vk::WHOLE_SIZE);
    let dependency =
        vk::DependencyInfo::default().buffer_memory_barriers(core::slice::from_ref(&barrier));
    device.cmd_pipeline_barrier2(command, &dependency);
    // SAFETY: the command buffer is recording and is ended once.
    unsafe { device.device.end_command_buffer(command) }?;
    let commands = [command];
    let submit = vk::SubmitInfo::default().command_buffers(&commands);
    // SAFETY: as in `upload_bytes`: the queue is externally synchronised by `&DeviceContext`
    // borrowed from the renderer, and the command buffer is executable.
    unsafe {
        device
            .device
            .queue_submit(device.graphics_queue, &[submit], vk::Fence::null())
    }?;
    // SAFETY: as in `upload_bytes`.
    if let Err(error) = unsafe { device.device.queue_wait_idle(device.graphics_queue) } {
        // SAFETY: the device handle is live.
        if unsafe { device.device.device_wait_idle() }.is_err() {
            core::mem::forget(pool);
            core::mem::forget(staging);
            core::mem::forget(buffer);
        }
        return Err(error.into());
    }
    drop(staging);
    Ok(buffer)
}

/// Uploads interleaved vertex bytes and `u32` indices through staging buffers.
#[allow(clippy::too_many_lines)] // One-time upload sequence; splitting hides the barriers.
fn upload_bytes(
    device: &DeviceContext,
    vertex_bytes: &[u8],
    indices_data: &[u32],
    shape: MeshShape,
) -> RenderResult<GpuMesh> {
    let allocator = device.allocator();
    let index_bytes: &[u8] = bytemuck::cast_slice(indices_data);

    let mut vertex_staging = GpuBuffer::new(
        device.device.clone(),
        allocator.clone(),
        "mesh-vertices-staging",
        vertex_bytes.len() as u64,
        vk::BufferUsageFlags::TRANSFER_SRC,
        MemoryLocation::CpuToGpu,
    )?;
    vertex_staging.write_bytes(0, vertex_bytes);
    let mut index_staging = GpuBuffer::new(
        device.device.clone(),
        allocator.clone(),
        "mesh-indices-staging",
        index_bytes.len() as u64,
        vk::BufferUsageFlags::TRANSFER_SRC,
        MemoryLocation::CpuToGpu,
    )?;
    index_staging.write_bytes(0, index_bytes);

    let vertices = GpuBuffer::new(
        device.device.clone(),
        allocator.clone(),
        "mesh-vertices",
        vertex_bytes.len() as u64,
        vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
        MemoryLocation::GpuOnly,
    )?;
    let indices = GpuBuffer::new(
        device.device.clone(),
        allocator,
        "mesh-indices",
        index_bytes.len() as u64,
        vk::BufferUsageFlags::INDEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
        MemoryLocation::GpuOnly,
    )?;

    let pool = CommandPool::new(device.device.clone(), device.graphics_family)?;
    let command = pool.allocate_one()?;
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: the command buffer is allocated and not recording.
    unsafe { device.device.begin_command_buffer(command, &begin) }?;

    let vertex_copy = vk::BufferCopy::default()
        .src_offset(0)
        .dst_offset(0)
        .size(vertex_bytes.len() as u64);
    // SAFETY: both buffers are valid, large enough, and created for transfer use.
    unsafe {
        device.device.cmd_copy_buffer(
            command,
            vertex_staging.handle(),
            vertices.handle(),
            core::slice::from_ref(&vertex_copy),
        );
    }
    let index_copy = vk::BufferCopy::default()
        .src_offset(0)
        .dst_offset(0)
        .size(index_bytes.len() as u64);
    // SAFETY: same argument as the vertex copy.
    unsafe {
        device.device.cmd_copy_buffer(
            command,
            index_staging.handle(),
            indices.handle(),
            core::slice::from_ref(&index_copy),
        );
    }

    let barriers = [
        vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::VERTEX_ATTRIBUTE_INPUT)
            .dst_access_mask(vk::AccessFlags2::VERTEX_ATTRIBUTE_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(vertices.handle())
            .offset(0)
            .size(vk::WHOLE_SIZE),
        vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::INDEX_INPUT)
            .dst_access_mask(vk::AccessFlags2::INDEX_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(indices.handle())
            .offset(0)
            .size(vk::WHOLE_SIZE),
    ];
    let dependency = vk::DependencyInfo::default().buffer_memory_barriers(&barriers);
    device.cmd_pipeline_barrier2(command, &dependency);

    // SAFETY: the command buffer is recording and can be ended once.
    unsafe { device.device.end_command_buffer(command) }?;
    let commands = [command];
    let submit = vk::SubmitInfo::default().command_buffers(&commands);
    // SAFETY: the command buffer is executable and the graphics queue belongs to the device.
    // The queue is externally synchronised: every submit to it happens on the renderer's
    // thread, through `&DeviceContext` borrowed from a `&mut Renderer` (or during `new`).
    unsafe {
        device
            .device
            .queue_submit(device.graphics_queue, &[submit], vk::Fence::null())
    }?;
    // SAFETY: same external synchronisation as the submit. The wait also covers any frame
    // still in flight (load-time stall only; `register_mesh` is never called per frame).
    if let Err(error) = unsafe { device.device.queue_wait_idle(device.graphics_queue) } {
        // The copy may still be executing: dropping the pool or buffers now would free
        // resources the GPU is using. Wait for the whole device instead; if that fails too,
        // leak them rather than risk a use-after-free.
        // SAFETY: the device handle is live; waiting has no other preconditions.
        if unsafe { device.device.device_wait_idle() }.is_err() {
            core::mem::forget(pool);
            core::mem::forget(vertex_staging);
            core::mem::forget(index_staging);
            core::mem::forget(vertices);
            core::mem::forget(indices);
        }
        return Err(error.into());
    }

    drop(vertex_staging);
    drop(index_staging);

    crate::memory::count_upload();
    let MeshShape {
        joint_count,
        has_uvs,
        lods,
        bounds,
    } = shape;
    Ok(GpuMesh {
        vertices,
        indices,
        index_count: lods.levels[0].index_count,
        lods,
        bounds,
        joint_count,
        has_uvs,
    })
}

#[cfg(test)]
mod tests {
    use super::{CpuMesh, CpuSkinnedMesh, SkinnedVertex};

    #[test]
    fn skinned_vertices_are_56_bytes_with_joints_weights_uv_then_tangent() {
        assert_eq!(core::mem::size_of::<SkinnedVertex>(), 56);
        assert_eq!(core::mem::offset_of!(SkinnedVertex, joints), 24);
        assert_eq!(core::mem::offset_of!(SkinnedVertex, weights), 28);
        assert_eq!(core::mem::offset_of!(SkinnedVertex, uv), 32);
        assert_eq!(core::mem::offset_of!(SkinnedVertex, tangent), 40);
        assert_eq!(core::mem::size_of::<super::Vertex>(), 48);
        assert_eq!(core::mem::offset_of!(super::Vertex, tangent), 32);
    }

    #[test]
    fn built_in_meshes_get_unit_tangents_orthogonal_to_their_normals() {
        for mesh in [CpuMesh::cube(), CpuMesh::plane(10.0)] {
            for vertex in &mesh.vertices {
                let [x, y, z, w] = vertex.tangent;
                let [nx, ny, nz] = vertex.normal;
                assert!(((x * x + y * y + z * z).sqrt() - 1.0).abs() < 1e-4);
                assert!((x * nx + y * ny + z * nz).abs() < 1e-4);
                assert_eq!(w.abs(), 1.0);
            }
        }
        // The terrain plane: +u along +X, v along +Z, so the bitangent (up the image, -v)
        // is -Z = cross(+Y, +X) * w with w = +1.
        assert_eq!(
            CpuMesh::plane(10.0).vertices[0].tangent,
            [1.0, 0.0, 0.0, 1.0]
        );
    }

    #[test]
    fn skinned_asset_meshes_interleave_joints_and_weights() {
        let triangle = || {
            moba_assets::MeshData::new(
                vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                vec![[0.0, 0.0, 1.0]; 3],
                vec![0, 1, 2],
            )
            .expect("valid mesh")
        };
        assert!(CpuSkinnedMesh::from_asset(&triangle()).is_none(), "no skin");
        let skeleton = moba_assets::Skeleton::new(
            vec![None, Some(0)],
            vec![moba_assets::JointTransform::IDENTITY; 2],
            vec![moba_core::math::Mat4::IDENTITY; 2],
            vec![moba_core::math::Mat4::IDENTITY; 2],
            None,
        )
        .expect("skeleton");
        let skin = moba_assets::MeshSkin::new(
            vec![[0, 0, 0, 0], [1, 0, 0, 0], [0, 1, 0, 0]],
            vec![[255, 0, 0, 0], [255, 0, 0, 0], [128, 127, 0, 0]],
            skeleton,
        )
        .expect("skin");
        let data = triangle().with_skin(skin).expect("skinned mesh");
        let cpu = CpuSkinnedMesh::from_asset(&data).expect("re-check passes");
        assert_eq!(cpu.joint_count, 2);
        assert_eq!(cpu.vertices[2].joints, [0, 1, 0, 0]);
        assert_eq!(cpu.vertices[2].weights, [128, 127, 0, 0]);
        assert_eq!(cpu.vertices[1].position, [1.0, 0.0, 0.0]);
        assert_eq!(cpu.indices, vec![0, 1, 2]);
    }

    #[test]
    fn asset_meshes_interleave_positions_and_normals() {
        let data = moba_assets::MeshData::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0.0, 0.0, 1.0]; 3],
            vec![0, 1, 2],
        )
        .expect("valid mesh");
        let cpu = CpuMesh::from_asset(&data).expect("re-check passes");
        assert_eq!(cpu.vertices.len(), 3);
        assert_eq!(cpu.vertices[1].position, [1.0, 0.0, 0.0]);
        assert_eq!(cpu.vertices[2].normal, [0.0, 0.0, 1.0]);
        assert_eq!(cpu.vertices[2].uv, [0.0, 0.0]);
        assert_eq!(cpu.vertices[2].tangent, super::DEFAULT_TANGENT);
        assert!(!cpu.has_uvs);
        assert_eq!(cpu.indices, vec![0, 1, 2]);
        let textured = data
            .with_uvs(vec![[0.0, 0.0], [1.0, 0.0], [0.5, 2.0]])
            .expect("uvs");
        let cpu = CpuMesh::from_asset(&textured).expect("re-check passes");
        assert!(cpu.has_uvs);
        assert_eq!(cpu.vertices[2].uv, [0.5, 2.0]);
    }
}
