//! `rt_smoke` (ADR-0020 gate 3): a BLAS/TLAS and `GL_EXT_ray_query`, checked exactly.
//!
//! One bottom-level structure holds three right triangles; the top level holds three
//! instances of it at different heights (one mirrored). One ray per pixel of a 64×64 grid
//! over `[-1, 1]²` starts at `z = 10` and points straight down; the compute shader stores the
//! closest hit as `[1 + instance index, custom index, primitive index, round(4 t)]`, or zeros
//! for a miss.
//!
//! **No ray is near an edge.** Pixel centres sit on odd multiples of 1/64; every leg lies on an
//! even multiple of 1/64 and every hypotenuse on `x ± y = c` with `c` an odd multiple of 1/64,
//! and the instance translations are multiples of 1/32, which keeps both properties. So every
//! ray is at least `1/(64·√2)` from every edge line ([`expected`] measures it, and a unit test
//! keeps it above [`MIN_MARGIN`]), far beyond any driver's rounding, and the expectation does
//! not depend on watertightness rules. The instances' heights differ, so there are no ties.

use ash::vk;
use ash::vk::TaggedStructure as _;
use gpu_allocator::MemoryLocation;

use super::bindless::{copy_to_buffer, host_read_barrier};
use super::{Headless, SmokeReport};
use crate::bindless::{BindlessTable, color_barrier, submit_one_shot};
use crate::error::{RenderError, RenderResult};
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuBuffer, GpuImage, GraphicsPipeline, PipelineLayout,
    ShaderModule,
};

/// The grid's side, in rays.
pub(crate) const SIZE: u32 = 64;
/// Rays start at this height.
const ORIGIN_Z: f32 = 10.0;
/// Rays end this far along.
const MAX_T: f32 = 100.0;
/// The smallest distance allowed between a ray and an edge line, in world units.
pub(crate) const MIN_MARGIN: f64 = 0.01;
const FORMAT: vk::Format = vk::Format::R32G32B32A32_UINT;

/// The BLAS's triangles in its own space (`z = 0`), as `[x, y]` corners.
pub(crate) const TRIANGLES: [[[f32; 2]; 3]; 3] = [
    // Legs on x = -56/64 and y = -52/64, hypotenuse x + y = -57/64.
    [
        [-0.875, -0.8125],
        [-0.078_125, -0.8125],
        [-0.875, -0.015_625],
    ],
    // Legs on x = 56/64 and y = 4/64, hypotenuse y - x = -5/64.
    [[0.875, 0.0625], [0.875, 0.796_875], [0.140_625, 0.0625]],
    // Legs on x = -44/64 and y = 12/64, hypotenuse x + y = 3/64.
    [
        [-0.6875, 0.1875],
        [-0.140_625, 0.1875],
        [-0.6875, 0.734_375],
    ],
];

/// One TLAS instance: a row-major 3×4 transform and its custom index.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Instance {
    pub(crate) transform: [[f32; 4]; 3],
    pub(crate) custom_index: u32,
}

/// The TLAS's instances, nearest the rays first only by accident of order: heights 0, -1.5
/// and -0.5.
pub(crate) const INSTANCES: [Instance; 3] = [
    Instance {
        transform: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ],
        custom_index: 7,
    },
    Instance {
        transform: [
            [1.0, 0.0, 0.0, 0.25],
            [0.0, 1.0, 0.0, -0.375],
            [0.0, 0.0, 1.0, -1.5],
        ],
        custom_index: 3,
    },
    // Mirrored in x: the triangles' winding flips, which a ray without culling ignores.
    Instance {
        transform: [
            [-1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0625],
            [0.0, 0.0, 1.0, -0.5],
        ],
        custom_index: 5,
    },
];

/// The world position of ray `(x, y)` (mirrors the shader; exact in `f32`).
fn ray_xy(x: u32, y: u32) -> (f32, f32) {
    let to_world = |pixel: u32| (pixel as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
    (to_world(x), to_world(y))
}

/// The expected value of every ray, row by row, and the smallest distance between a ray and
/// an edge line of any triangle it could hit or miss.
pub(crate) fn expected() -> (Vec<[u32; 4]>, f64) {
    let mut values = Vec::with_capacity((SIZE * SIZE) as usize);
    let mut margin = f64::INFINITY;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (px, py) = ray_xy(x, y);
            let (px, py) = (f64::from(px), f64::from(py));
            let mut closest: Option<(f64, [u32; 4])> = None;
            for (instance_index, instance) in (0u32..).zip(&INSTANCES) {
                let m = instance.transform;
                // Exact constants, compared exactly.
                #[allow(clippy::float_cmp)]
                let horizontal = m[2][0] == 0.0 && m[2][1] == 0.0 && m[2][2] == 1.0;
                assert!(horizontal, "instances keep the triangles horizontal");
                let t = f64::from(ORIGIN_Z) - f64::from(m[2][3]);
                for (primitive, triangle) in (0u32..).zip(&TRIANGLES) {
                    let corners = triangle.map(|[cx, cy]| {
                        let (cx, cy) = (f64::from(cx), f64::from(cy));
                        (
                            f64::from(m[0][0]) * cx + f64::from(m[0][1]) * cy + f64::from(m[0][3]),
                            f64::from(m[1][0]) * cx + f64::from(m[1][1]) * cy + f64::from(m[1][3]),
                        )
                    });
                    let (inside, certainty) = classify(corners, (px, py));
                    margin = margin.min(certainty);
                    if !inside {
                        continue;
                    }
                    let value = [
                        1 + instance_index,
                        instance.custom_index,
                        primitive,
                        (t * 4.0).round() as u32,
                    ];
                    match closest {
                        Some((best, _)) if best < t => {}
                        Some((best, _)) => {
                            assert!(best > t, "two hits at the same t would be ambiguous");
                            closest = Some((t, value));
                        }
                        None => closest = Some((t, value)),
                    }
                }
            }
            values.push(closest.map_or([0; 4], |(_, value)| value));
        }
    }
    (values, margin)
}

/// Whether `point` is inside the triangle, and how certain that is: the distance to the
/// nearest edge line when inside, and the largest distance outside any edge line when outside.
fn classify(corners: [(f64, f64); 3], point: (f64, f64)) -> (bool, f64) {
    let [a, b, c] = corners;
    let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    let orientation = area.signum();
    let distance = |from: (f64, f64), to: (f64, f64)| {
        let (ex, ey) = (to.0 - from.0, to.1 - from.1);
        orientation * (ex * (point.1 - from.1) - ey * (point.0 - from.0)) / ex.hypot(ey)
    };
    let d = [distance(a, b), distance(b, c), distance(c, a)];
    let inside = d.iter().all(|&d| d > 0.0);
    let certainty = if inside {
        d.iter().copied().fold(f64::INFINITY, f64::min)
    } else {
        d.iter().map(|&d| -d).fold(f64::NEG_INFINITY, f64::max)
    };
    (inside, certainty)
}

/// Set 1 of the smoke's layout: the TLAS (ADR-0020 §3a puts acceleration structures in a small
/// set at index 1).
pub(crate) fn scene_set_bindings() -> [vk::DescriptorSetLayoutBinding<'static>; 1] {
    [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::COMPUTE)]
}

/// An acceleration structure and the buffer that stores it, destroyed in that order.
struct AccelerationStructure {
    loader: ash::khr::acceleration_structure::Device,
    handle: vk::AccelerationStructureKHR,
    _storage: GpuBuffer,
}

impl AccelerationStructure {
    fn new(
        gpu: &Headless,
        loader: &ash::khr::acceleration_structure::Device,
        ty: vk::AccelerationStructureTypeKHR,
        size: u64,
    ) -> RenderResult<Self> {
        let storage = GpuBuffer::new(
            gpu.device.device.clone(),
            gpu.device.allocator(),
            "smoke-acceleration-structure",
            size,
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR
                | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            MemoryLocation::GpuOnly,
        )?;
        let info = vk::AccelerationStructureCreateInfoKHR::default()
            .buffer(storage.handle())
            .size(size)
            .ty(ty);
        // SAFETY: `VK_KHR_acceleration_structure` and its feature are enabled on this tiered
        // device; the buffer is live, has the storage usage and holds `size` bytes.
        let handle = unsafe { loader.create_acceleration_structure(&info, None) }?;
        Ok(Self {
            loader: loader.clone(),
            handle,
            _storage: storage,
        })
    }

    fn address(&self) -> vk::DeviceAddress {
        let info = vk::AccelerationStructureDeviceAddressInfoKHR::default()
            .acceleration_structure(self.handle);
        // SAFETY: the structure is live and was created on this device.
        unsafe { self.loader.get_acceleration_structure_device_address(&info) }
    }
}

impl Drop for AccelerationStructure {
    fn drop(&mut self) {
        // SAFETY: the structure belongs to this device, no pending work uses it (every smoke
        // submission is waited on) and it is destroyed once, before its storage buffer.
        unsafe {
            self.loader
                .destroy_acceleration_structure(self.handle, None);
        }
    }
}

/// A scratch buffer and its address rounded up to the device's scratch alignment.
fn scratch(gpu: &Headless, size: u64, alignment: u64) -> RenderResult<(GpuBuffer, u64)> {
    let buffer = GpuBuffer::new(
        gpu.device.device.clone(),
        gpu.device.allocator(),
        "smoke-as-scratch",
        size + alignment,
        vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
        MemoryLocation::GpuOnly,
    )?;
    let address = super::address(&buffer).next_multiple_of(alignment);
    Ok((buffer, address))
}

/// A host-visible build-input buffer holding `bytes`.
fn input_buffer(gpu: &Headless, name: &str, bytes: &[u8]) -> RenderResult<GpuBuffer> {
    let mut buffer = GpuBuffer::new(
        gpu.device.device.clone(),
        gpu.device.allocator(),
        name,
        bytes.len() as u64,
        vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
        MemoryLocation::CpuToGpu,
    )?;
    buffer.write_bytes(0, bytes);
    Ok(buffer)
}

/// `VkAccelerationStructureInstanceKHR` (64 bytes) for `instance` pointing at `blas`.
fn instance_bytes(instance: &Instance, blas: vk::DeviceAddress) -> [u8; 64] {
    let mut bytes = [0u8; 64];
    for (index, value) in instance.transform.iter().flatten().enumerate() {
        bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    let custom_and_mask = (instance.custom_index & 0x00ff_ffff) | (0xff << 24);
    bytes[48..52].copy_from_slice(&custom_and_mask.to_le_bytes());
    // Shader-binding-table offset 0, no flags.
    bytes[52..56].copy_from_slice(&0u32.to_le_bytes());
    bytes[56..64].copy_from_slice(&blas.to_le_bytes());
    bytes
}

#[allow(clippy::too_many_lines)] // Build inputs, two builds, one dispatch and the comparison.
pub(super) fn run(gpu: &Headless, report: &mut SmokeReport) -> RenderResult<()> {
    let device = &gpu.device;
    let (expected, margin) = expected();
    if margin < MIN_MARGIN {
        return Err(RenderError::Shader(
            "rt_smoke geometry puts a ray on an edge",
        ));
    }
    let mut table = BindlessTable::new(device)?;
    let loader =
        ash::khr::acceleration_structure::Device::load(&gpu.instance.instance, &device.device);
    let mut as_properties = vk::PhysicalDeviceAccelerationStructurePropertiesKHR::default();
    {
        let mut properties = vk::PhysicalDeviceProperties2::default().push(&mut as_properties);
        // SAFETY: the physical device belongs to this instance and advertises
        // `VK_KHR_acceleration_structure` (tiered admission); the chain is valid for the call.
        unsafe {
            gpu.instance
                .instance
                .get_physical_device_properties2(device.physical, &mut properties);
        }
    }
    let scratch_alignment =
        u64::from(as_properties.min_acceleration_structure_scratch_offset_alignment).max(1);

    // Bottom level: three triangles, not indexed.
    let vertices: Vec<u8> = TRIANGLES
        .iter()
        .flatten()
        .flat_map(|&[x, y]| [x, y, 0.0])
        .flat_map(f32::to_le_bytes)
        .collect();
    let vertex_buffer = input_buffer(gpu, "smoke-vertices", &vertices)?;
    let triangles = vk::AccelerationStructureGeometryTrianglesDataKHR::default()
        .vertex_format(vk::Format::R32G32B32_SFLOAT)
        .vertex_data(vk::DeviceOrHostAddressConstKHR {
            device_address: super::address(&vertex_buffer),
        })
        .vertex_stride(12)
        .max_vertex(TRIANGLES.len() as u32 * 3 - 1)
        .index_type(vk::IndexType::NONE_KHR);
    let blas_geometry = [vk::AccelerationStructureGeometryKHR::default()
        .geometry_type(vk::GeometryTypeKHR::TRIANGLES)
        .geometry(vk::AccelerationStructureGeometryDataKHR { triangles })
        .flags(vk::GeometryFlagsKHR::OPAQUE)];
    let blas_count = [TRIANGLES.len() as u32];
    let mut blas_info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
        .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL)
        .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
        .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
        .geometries(&blas_geometry);
    let mut blas_sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
    // SAFETY: the extension is enabled, the build info and counts describe one triangle
    // geometry, and the size struct is writable.
    unsafe {
        loader.get_acceleration_structure_build_sizes(
            vk::AccelerationStructureBuildTypeKHR::DEVICE,
            &blas_info,
            Some(&blas_count),
            &mut blas_sizes,
        );
    }
    let blas = AccelerationStructure::new(
        gpu,
        &loader,
        vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL,
        blas_sizes.acceleration_structure_size,
    )?;
    let (_blas_scratch, blas_scratch_address) =
        scratch(gpu, blas_sizes.build_scratch_size, scratch_alignment)?;
    blas_info = blas_info
        .dst_acceleration_structure(blas.handle)
        .scratch_data(vk::DeviceOrHostAddressKHR {
            device_address: blas_scratch_address,
        });

    // Top level: three instances of it.
    let instance_data: Vec<u8> = INSTANCES
        .iter()
        .flat_map(|instance| instance_bytes(instance, blas.address()))
        .collect();
    let instance_buffer = input_buffer(gpu, "smoke-instances", &instance_data)?;
    if !super::address(&instance_buffer).is_multiple_of(16) {
        return Err(RenderError::Shader(
            "TLAS instance buffer is not 16-byte aligned",
        ));
    }
    let instances = vk::AccelerationStructureGeometryInstancesDataKHR::default()
        .array_of_pointers(false)
        .data(vk::DeviceOrHostAddressConstKHR {
            device_address: super::address(&instance_buffer),
        });
    let tlas_geometry = [vk::AccelerationStructureGeometryKHR::default()
        .geometry_type(vk::GeometryTypeKHR::INSTANCES)
        .geometry(vk::AccelerationStructureGeometryDataKHR { instances })
        .flags(vk::GeometryFlagsKHR::OPAQUE)];
    let tlas_count = [INSTANCES.len() as u32];
    let mut tlas_info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
        .ty(vk::AccelerationStructureTypeKHR::TOP_LEVEL)
        .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
        .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
        .geometries(&tlas_geometry);
    let mut tlas_sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
    // SAFETY: as for the bottom level, with one instance geometry.
    unsafe {
        loader.get_acceleration_structure_build_sizes(
            vk::AccelerationStructureBuildTypeKHR::DEVICE,
            &tlas_info,
            Some(&tlas_count),
            &mut tlas_sizes,
        );
    }
    let tlas = AccelerationStructure::new(
        gpu,
        &loader,
        vk::AccelerationStructureTypeKHR::TOP_LEVEL,
        tlas_sizes.acceleration_structure_size,
    )?;
    let (_tlas_scratch, tlas_scratch_address) =
        scratch(gpu, tlas_sizes.build_scratch_size, scratch_alignment)?;
    tlas_info = tlas_info
        .dst_acceleration_structure(tlas.handle)
        .scratch_data(vk::DeviceOrHostAddressKHR {
            device_address: tlas_scratch_address,
        });

    // Set 1: the TLAS.
    let scene_bindings = scene_set_bindings();
    let scene_layout = DescriptorSetLayout::new(device.device.clone(), &scene_bindings)?;
    let scene_pool = DescriptorPool::new(
        device.device.clone(),
        &[vk::DescriptorPoolSize {
            ty: vk::DescriptorType::ACCELERATION_STRUCTURE_KHR,
            descriptor_count: 1,
        }],
        1,
    )?;
    let scene_set = scene_pool
        .allocate(&device.device, &[scene_layout.handle()])?
        .into_iter()
        .next()
        .ok_or(RenderError::Shader("no TLAS descriptor set"))?;
    let structures = [tlas.handle];
    let mut tlas_write = vk::WriteDescriptorSetAccelerationStructureKHR::default()
        .acceleration_structures(&structures);
    let write = vk::WriteDescriptorSet::default()
        .dst_set(scene_set)
        .dst_binding(0)
        .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
        .descriptor_count(1)
        .push(&mut tlas_write);
    // SAFETY: the set is live and unused; the write names one live TLAS for its one binding.
    unsafe {
        device
            .device
            .update_descriptor_sets(core::slice::from_ref(&write), &[]);
    }

    // Output, pipeline and readback.
    let extent = vk::Extent2D {
        width: SIZE,
        height: SIZE,
    };
    let output = GpuImage::new(
        device.device.clone(),
        device.allocator(),
        "smoke-rt-output",
        extent,
        FORMAT,
        vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC,
        vk::ImageAspectFlags::COLOR,
    )?;
    let output_handle = table.register_storage_image(output.view())?;
    let bindless_bindings = crate::bindless::set_layout_bindings();
    crate::descriptor_budget::check_tiered(
        "rt_smoke",
        &[&bindless_bindings, &scene_bindings],
        device.update_after_bind,
    )?;
    let push_range = vk::PushConstantRange {
        stage_flags: vk::ShaderStageFlags::COMPUTE,
        offset: 0,
        size: 16,
    };
    let layout = PipelineLayout::with_push_constants(
        device.device.clone(),
        &[table.layout(), scene_layout.handle()],
        core::slice::from_ref(&push_range),
    )?;
    let module = ShaderModule::new(device.device.clone(), moba_shaders::RT_SMOKE_COMP)?;
    let stage = vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module.handle())
        .name(c"main");
    let pipeline = GraphicsPipeline::from_raw(
        device.device.clone(),
        device.create_compute_pipeline(
            &vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(layout.handle()),
        )?,
    );
    let bytes = (SIZE * SIZE * 16) as usize;
    let readback = GpuBuffer::new(
        device.device.clone(),
        device.allocator(),
        "smoke-rt-readback",
        bytes as u64,
        vk::BufferUsageFlags::TRANSFER_DST,
        MemoryLocation::GpuToCpu,
    )?;
    let mut push = [0u8; 16];
    push[..4].copy_from_slice(&output_handle.index().to_le_bytes());
    push[4..8].copy_from_slice(&SIZE.to_le_bytes());
    push[8..12].copy_from_slice(&ORIGIN_Z.to_le_bytes());
    push[12..16].copy_from_slice(&MAX_T.to_le_bytes());
    let writes_before_frame = table.writes();

    submit_one_shot(device, |command| {
        let blas_range = [vk::AccelerationStructureBuildRangeInfoKHR::default()
            .primitive_count(TRIANGLES.len() as u32)];
        // SAFETY: the build info names a live destination and a scratch range of the
        // reported size at the required alignment; the vertex buffer holds `max_vertex + 1`
        // vertices and was written before submission.
        unsafe {
            loader.cmd_build_acceleration_structures(
                command,
                core::slice::from_ref(&blas_info),
                &[Some(&blas_range[..])],
            );
        }
        let built = vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR)
            .src_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_WRITE_KHR)
            .dst_stage_mask(vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR)
            .dst_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_READ_KHR);
        device.cmd_pipeline_barrier2(
            command,
            &vk::DependencyInfo::default().memory_barriers(core::slice::from_ref(&built)),
        );
        let tlas_range = [vk::AccelerationStructureBuildRangeInfoKHR::default()
            .primitive_count(INSTANCES.len() as u32)];
        // SAFETY: as above; the instances point at the BLAS, whose build the barrier orders
        // before this one.
        unsafe {
            loader.cmd_build_acceleration_structures(
                command,
                core::slice::from_ref(&tlas_info),
                &[Some(&tlas_range[..])],
            );
        }
        let to_trace = vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR)
            .src_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_WRITE_KHR)
            .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
            .dst_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_READ_KHR);
        let to_general = color_barrier(
            output.image(),
            (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
            (
                vk::PipelineStageFlags2::COMPUTE_SHADER,
                vk::AccessFlags2::SHADER_STORAGE_WRITE,
            ),
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::GENERAL,
        );
        device.cmd_pipeline_barrier2(
            command,
            &vk::DependencyInfo::default()
                .memory_barriers(core::slice::from_ref(&to_trace))
                .image_memory_barriers(core::slice::from_ref(&to_general)),
        );
        let sets = [table.set(), scene_set];
        let d = &device.device;
        // SAFETY: the command buffer is recording and the pipeline is live.
        unsafe { d.cmd_bind_pipeline(command, vk::PipelineBindPoint::COMPUTE, pipeline.handle()) };
        // SAFETY: both sets are live and match sets 0 and 1 of the layout.
        unsafe {
            d.cmd_bind_descriptor_sets(
                command,
                vk::PipelineBindPoint::COMPUTE,
                layout.handle(),
                0,
                &sets,
                &[],
            );
        }
        // SAFETY: the layout's push range is 16 bytes for the compute stage.
        unsafe { d.cmd_push_constants(command, layout.handle(), push_range.stage_flags, 0, &push) };
        // SAFETY: the dispatch covers exactly the 64×64 output in 8×8 groups; the TLAS build
        // and the output's layout transition are ordered before it.
        unsafe { d.cmd_dispatch(command, SIZE / 8, SIZE / 8, 1) };
        let to_copy = color_barrier(
            output.image(),
            (
                vk::PipelineStageFlags2::COMPUTE_SHADER,
                vk::AccessFlags2::SHADER_STORAGE_WRITE,
            ),
            (
                vk::PipelineStageFlags2::COPY,
                vk::AccessFlags2::TRANSFER_READ,
            ),
            vk::ImageLayout::GENERAL,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        );
        device.cmd_pipeline_barrier2(
            command,
            &vk::DependencyInfo::default().image_memory_barriers(core::slice::from_ref(&to_copy)),
        );
        copy_to_buffer(device, command, &output, &readback, extent);
        host_read_barrier(device, command);
    })?;
    report.check(
        || String::from("descriptor writes while recording and running the frame"),
        table.writes() - writes_before_frame,
        0,
    );

    let mut raw = vec![0u8; bytes];
    if !readback.read_bytes(0, &mut raw) {
        return Err(RenderError::Shader("smoke readback is not host visible"));
    }
    let mut coverage = [[0u32; 3]; 3];
    let mut misses = 0u32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let index = (y * SIZE + x) as usize;
            let mut actual = [0u32; 4];
            for (channel, value) in actual.iter_mut().enumerate() {
                let at = index * 16 + channel * 4;
                *value = u32::from_le_bytes(raw[at..at + 4].try_into().expect("four bytes"));
            }
            let want = expected[index];
            if want[0] == 0 {
                misses += 1;
            } else {
                coverage[want[0] as usize - 1][want[2] as usize] += 1;
            }
            report.check(|| format!("ray ({x}, {y})"), actual, want);
        }
    }
    report.notes.push(format!(
        "BLAS {} triangles, TLAS {} instances ({} + {} bytes); {misses} misses; closest hits \
         per instance and primitive {coverage:?}; smallest ray-to-edge distance {margin:.4}",
        TRIANGLES.len(),
        INSTANCES.len(),
        blas_sizes.acceleration_structure_size,
        tlas_sizes.acceleration_structure_size,
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{INSTANCES, MIN_MARGIN, SIZE, TRIANGLES, expected};

    /// Every ray is far from every edge, every instance and primitive is the closest hit
    /// somewhere, and some rays miss: the expectation is unambiguous and covers the scene.
    #[test]
    fn the_expectation_is_unambiguous_and_covers_every_instance_and_primitive() {
        let (values, margin) = expected();
        assert_eq!(values.len(), (SIZE * SIZE) as usize);
        assert!(margin >= MIN_MARGIN, "a ray is {margin} from an edge");
        for instance in 0..INSTANCES.len() as u32 {
            for primitive in 0..TRIANGLES.len() as u32 {
                assert!(
                    values
                        .iter()
                        .any(|value| value[0] == instance + 1 && value[2] == primitive),
                    "instance {instance} primitive {primitive} is never the closest hit"
                );
            }
        }
        assert!(values.iter().any(|value| value[0] == 0), "some rays miss");
        // t: 10, 11.5 and 10.5, as quarter units.
        let depths: std::collections::BTreeSet<_> = values
            .iter()
            .filter(|value| value[0] != 0)
            .map(|value| value[3])
            .collect();
        assert_eq!(depths.into_iter().collect::<Vec<_>>(), [40, 42, 46]);
    }
}
