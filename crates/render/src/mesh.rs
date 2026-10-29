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
/// (MikkTSpace-style, `MeshData::generate_tangents`), else [`DEFAULT_TANGENT`].
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
    pub(crate) indices: Vec<u32>,
    pub(crate) joint_count: u32,
    pub(crate) has_uvs: bool,
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
        })
    }
}

/// CPU-side interleaved mesh, ready for [`upload`].
pub(crate) struct CpuMesh {
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) indices: Vec<u32>,
    /// Whether the UVs are real texture coordinates (a textured material needs them).
    pub(crate) has_uvs: bool,
}

/// Device-local vertex/index buffers.
pub(crate) struct GpuMesh {
    pub(crate) vertices: GpuBuffer,
    pub(crate) indices: GpuBuffer,
    pub(crate) index_count: u32,
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
        Self {
            vertices,
            indices,
            has_uvs: true,
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
        Some(Self {
            vertices,
            indices: data.indices().to_vec(),
            has_uvs: uvs.is_some(),
        })
    }
}

/// Uploads a static mesh to device-local memory through staging buffers.
pub(crate) fn upload(device: &DeviceContext, data: &CpuMesh) -> RenderResult<GpuMesh> {
    upload_bytes(
        device,
        bytemuck::cast_slice(&data.vertices),
        &data.indices,
        0,
        data.has_uvs,
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
        data.joint_count,
        data.has_uvs,
    )
}

/// Uploads interleaved vertex bytes and `u32` indices through staging buffers.
#[allow(clippy::too_many_lines)] // One-time upload sequence; splitting hides the barriers.
fn upload_bytes(
    device: &DeviceContext,
    vertex_bytes: &[u8],
    indices_data: &[u32],
    joint_count: u32,
    has_uvs: bool,
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

    let index_count = u32::try_from(indices_data.len()).expect("index count fits u32");
    Ok(GpuMesh {
        vertices,
        indices,
        index_count,
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
