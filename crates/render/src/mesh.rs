//! Procedural meshes and one-time GPU upload.

use ash::vk;
use bytemuck::{Pod, Zeroable};
use gpu_allocator::MemoryLocation;

use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::resources::{CommandPool, GpuBuffer};

/// Vertex layout shared by every scene pipeline.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct Vertex {
    pub(crate) position: [f32; 3],
    pub(crate) normal: [f32; 3],
}

/// CPU-side interleaved mesh, ready for [`upload`].
pub(crate) struct CpuMesh {
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) indices: Vec<u32>,
}

/// Device-local vertex/index buffers.
pub(crate) struct GpuMesh {
    pub(crate) vertices: GpuBuffer,
    pub(crate) indices: GpuBuffer,
    pub(crate) index_count: u32,
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
        for (normal, corners) in faces {
            let base = u32::try_from(vertices.len()).expect("vertex count fits u32");
            for corner in corners {
                vertices.push(Vertex {
                    position: corner,
                    normal,
                });
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        Self { vertices, indices }
    }

    /// Ground plane of the given size, centred at the origin.
    pub(crate) fn plane(size: f32) -> Self {
        let half = size * 0.5;
        let vertices = vec![
            Vertex {
                position: [-half, 0.0, -half],
                normal: [0.0, 1.0, 0.0],
            },
            Vertex {
                position: [half, 0.0, -half],
                normal: [0.0, 1.0, 0.0],
            },
            Vertex {
                position: [half, 0.0, half],
                normal: [0.0, 1.0, 0.0],
            },
            Vertex {
                position: [-half, 0.0, half],
                normal: [0.0, 1.0, 0.0],
            },
        ];
        let indices = vec![0, 1, 2, 0, 2, 3];
        Self { vertices, indices }
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
        let vertices = data
            .positions()
            .iter()
            .zip(data.normals())
            .map(|(&position, &normal)| Vertex { position, normal })
            .collect();
        Some(Self {
            vertices,
            indices: data.indices().to_vec(),
        })
    }
}

/// Uploads a mesh to device-local memory through staging buffers.
#[allow(clippy::too_many_lines)] // One-time upload sequence; splitting hides the barriers.
pub(crate) fn upload(device: &DeviceContext, data: &CpuMesh) -> RenderResult<GpuMesh> {
    let allocator = device.allocator();
    let vertex_bytes: &[u8] = bytemuck::cast_slice(&data.vertices);
    let index_bytes: &[u8] = bytemuck::cast_slice(&data.indices);

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

    let index_count = u32::try_from(data.indices.len()).expect("index count fits u32");
    Ok(GpuMesh {
        vertices,
        indices,
        index_count,
    })
}

#[cfg(test)]
mod tests {
    use super::CpuMesh;

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
        assert_eq!(cpu.indices, vec![0, 1, 2]);
    }
}
