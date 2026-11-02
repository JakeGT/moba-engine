//! Headless run of the Slang toolchain smoke module (ADR-0020 §4c, N0).
//!
//! [`slang_smoke`] creates a compute-only device, builds a pipeline from
//! [`moba_shaders::TOOLCHAIN_SMOKE_SLANG`], dispatches it once, and checks the output on the
//! CPU. The point is that a real driver (lavapipe and `MoltenVK` in CI, with validation
//! required) consumes `slangc`'s SPIR-V before the first tiered pass depends on it. It also
//! checks the frame-constants data path and the matrix convention: `mul(view_proj, v)` in
//! Slang must equal glam's `view_proj * v`.
//!
//! It is a CI and `vulkan-probe --slang-smoke` tool, not part of the renderer: the frame
//! loop never creates this module.

use std::ffi::{CStr, c_char};
use std::sync::{Arc, Mutex};

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use moba_core::math::{Mat4, Vec4};

use crate::error::{RenderError, RenderResult};
use crate::instance::{InstanceContext, ValidationMode};
use crate::resources::{
    CommandPool, DescriptorPool, DescriptorSetLayout, Fence, GpuBuffer, GraphicsPipeline,
    PipelineLayout, ShaderModule,
};
use crate::scene::{CameraUniform, VISION_CIRCLES};

const PORTABILITY_SUBSET: &CStr = c"VK_KHR_portability_subset";
/// What the shader writes past the circles it copies: their count is below the maximum.
const SENTINEL: f32 = -7.0;
/// How many vision circles the smoke uploads (fewer than [`VISION_CIRCLES`], so the
/// shader's bound is exercised).
const CIRCLES: usize = 5;

/// The result of [`slang_smoke`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SlangSmoke {
    /// The device the module ran on.
    pub device: String,
    /// Whether the Khronos validation layer was enabled.
    pub validation: bool,
    /// Validation errors reported while the smoke ran.
    pub validation_errors: u64,
    /// `None` when the output matched exactly; otherwise what differed.
    pub mismatch: Option<String>,
}

impl SlangSmoke {
    /// True when the output matched and validation reported no error.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.mismatch.is_none() && self.validation_errors == 0
    }
}

/// Destroys the device last (declared before every child object, so dropped after them).
struct OwnedDevice(ash::Device);

impl Drop for OwnedDevice {
    fn drop(&mut self) {
        // SAFETY: the device is live; waiting has no other precondition.
        let _ = unsafe { self.0.device_wait_idle() };
        // SAFETY: every child object was destroyed by its RAII owner before this runs (they
        // are declared later), the device is idle, and it is destroyed exactly once.
        unsafe { self.0.destroy_device(None) };
    }
}

/// Runs `toolchain_smoke.slang` once on the first Vulkan 1.3 device with a compute queue
/// and checks its output on the CPU: the copied vision circles, untouched entries past them,
/// and `mul(view_proj, camera_pos)` against glam's `view_proj * camera_pos` (the matrix
/// convention, ADR-0020 §4c). A CI and `vulkan-probe --slang-smoke` tool: the renderer never
/// creates this module.
///
/// # Errors
/// Fails when Vulkan or a 1.3 compute device is unavailable, when `validation` is
/// [`ValidationMode::Required`] and the layer is missing, or when a Vulkan call fails.
pub fn slang_smoke(validation: ValidationMode) -> RenderResult<SlangSmoke> {
    let errors_before = InstanceContext::validation_error_count();
    let context = InstanceContext::new(&[], validation)?;
    // Every device object is destroyed when `run` returns, so validation errors raised by
    // their destruction are counted too.
    let (device, mismatch) = run(&context)?;
    Ok(SlangSmoke {
        device,
        validation: context.validation_enabled(),
        validation_errors: InstanceContext::validation_error_count() - errors_before,
        mismatch,
    })
}

/// Device setup, one dispatch and the readback: the device name and any mismatch.
#[allow(clippy::too_many_lines)] // One explicit, ordered setup-dispatch-readback sequence.
#[allow(clippy::float_cmp)] // Exact on purpose: every expected value is exact in f32.
fn run(context: &InstanceContext) -> RenderResult<(String, Option<String>)> {
    let instance = &context.instance;

    // SAFETY: the instance is live.
    let physical_devices = unsafe { instance.enumerate_physical_devices() }?;
    let (physical, family, properties) = physical_devices
        .iter()
        .find_map(|&physical| {
            // SAFETY: `physical` was enumerated from this instance.
            let properties = unsafe { instance.get_physical_device_properties(physical) };
            if properties.api_version < vk::API_VERSION_1_3 {
                return None;
            }
            // SAFETY: as above.
            let families =
                unsafe { instance.get_physical_device_queue_family_properties(physical) };
            let family = families
                .iter()
                .position(|family| family.queue_flags.contains(vk::QueueFlags::COMPUTE))?;
            Some((physical, u32::try_from(family).ok()?, properties))
        })
        .ok_or_else(|| {
            RenderError::NoSuitableDevice(String::from(
                "no Vulkan 1.3 device with a compute queue for the Slang smoke",
            ))
        })?;
    // SAFETY: `deviceName` is a NUL-terminated array owned by the properties struct.
    let device_name = unsafe { CStr::from_ptr(properties.device_name.as_ptr()) }
        .to_string_lossy()
        .into_owned();

    // Portability drivers (MoltenVK) require their subset extension to be enabled.
    // SAFETY: `physical` was enumerated from this instance.
    let available = unsafe { instance.enumerate_device_extension_properties(physical) }?;
    let portability = available.iter().any(|extension| {
        // SAFETY: extension names are NUL-terminated inside their fixed array.
        let name = unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) };
        name == PORTABILITY_SUBSET
    });
    let extensions: Vec<*const c_char> = if portability {
        vec![PORTABILITY_SUBSET.as_ptr()]
    } else {
        Vec::new()
    };
    let priorities = [1.0f32];
    let queue_infos = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(family)
        .queue_priorities(&priorities)];
    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queue_infos)
        .enabled_extension_names(&extensions);
    // SAFETY: the physical device came from this instance, the only extension requested is
    // advertised, and every pointer in `create_info` outlives the call.
    let device = OwnedDevice(unsafe { instance.create_device(physical, &create_info, None) }?);
    let raw = &device.0;
    // SAFETY: queue 0 of `family` was created with the device.
    let queue = unsafe { raw.get_device_queue(family, 0) };

    let mut debug_settings = gpu_allocator::AllocatorDebugSettings::default();
    debug_settings.log_leaks_on_shutdown = false;
    let allocator = Arc::new(Mutex::new(Allocator::new(&AllocatorCreateDesc {
        instance: instance.clone(),
        device: raw.clone(),
        physical_device: physical,
        debug_settings,
        buffer_device_address: false,
        allocation_sizes: gpu_allocator::AllocationSizes::default(),
    })?));

    // Inputs: a non-symmetric matrix (a transposed product would differ) with small integer
    // entries, so every expected value is exact in f32.
    let view_proj = Mat4::from_cols_array(&core::array::from_fn(|index| index as f32 + 1.0));
    let camera_pos = Vec4::new(1.0, -2.0, 3.0, 0.5);
    let circles: Vec<Vec4> = (0..CIRCLES)
        .map(|index| {
            let index = index as f32;
            Vec4::new(index, index + 0.5, -index, 1.0)
        })
        .collect();
    let uniform = CameraUniform::for_slang_smoke(view_proj, camera_pos, &circles);
    let mut camera = GpuBuffer::new(
        raw.clone(),
        Arc::clone(&allocator),
        "slang smoke camera",
        core::mem::size_of::<CameraUniform>() as u64,
        vk::BufferUsageFlags::UNIFORM_BUFFER,
        MemoryLocation::CpuToGpu,
    )?;
    camera.write_bytes(0, bytemuck::bytes_of(&uniform));
    let entries = VISION_CIRCLES + 1;
    let mut output = GpuBuffer::new(
        raw.clone(),
        Arc::clone(&allocator),
        "slang smoke output",
        (entries * core::mem::size_of::<[f32; 4]>()) as u64,
        vk::BufferUsageFlags::STORAGE_BUFFER,
        // Read back by the host: GpuToCpu prefers cached, coherent memory (as the
        // renderer's timestamp readback does), not write-combined BAR memory.
        MemoryLocation::GpuToCpu,
    )?;
    output
        .mapped_slice_mut::<[f32; 4]>(entries)
        .ok_or(RenderError::Shader(
            "slang smoke output buffer is not mapped",
        ))?
        .fill([SENTINEL; 4]);

    // Set 0: binding 0 the frame constants, binding 1 the output (moba-shaders'
    // SLANG_BINDINGS table pins these).
    let bindings = [
        vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
    ];
    let set_layout = DescriptorSetLayout::new(raw.clone(), &bindings)?;
    let pool = DescriptorPool::new(
        raw.clone(),
        &[
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: 1,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 1,
            },
        ],
        1,
    )?;
    let set = pool.allocate(raw, &[set_layout.handle()])?[0];
    let camera_info = [vk::DescriptorBufferInfo::default()
        .buffer(camera.handle())
        .range(vk::WHOLE_SIZE)];
    let output_info = [vk::DescriptorBufferInfo::default()
        .buffer(output.handle())
        .range(vk::WHOLE_SIZE)];
    let writes = [
        vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .buffer_info(&camera_info),
        vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&output_info),
    ];
    // SAFETY: the set, both buffers and the info arrays are live for the call.
    unsafe { raw.update_descriptor_sets(&writes, &[]) };

    let module = ShaderModule::new(raw.clone(), moba_shaders::TOOLCHAIN_SMOKE_SLANG)?;
    let layout = PipelineLayout::with_push_constants(raw.clone(), &[set_layout.handle()], &[])?;
    let stage = vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module.handle())
        .name(c"main");
    let pipeline_info = vk::ComputePipelineCreateInfo::default()
        .stage(stage)
        .layout(layout.handle());
    // SAFETY: the module and layout are live, and the create info outlives the call.
    let pipeline = unsafe {
        raw.create_compute_pipelines(
            vk::PipelineCache::null(),
            core::slice::from_ref(&pipeline_info),
            None,
        )
    }
    .map_err(|(_, error)| RenderError::Vulkan(error))?;
    // `GraphicsPipeline` only owns a `vk::Pipeline` and destroys it; a compute pipeline is
    // destroyed the same way.
    let pipeline = GraphicsPipeline::from_raw(raw.clone(), pipeline[0]);

    let commands = CommandPool::new(raw.clone(), family)?;
    let command_buffer = commands.allocate_one()?;
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // The shader's writes must be visible to the host read below.
    let to_host = [vk::MemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::SHADER_WRITE)
        .dst_access_mask(vk::AccessFlags::HOST_READ)];
    // Every block below: the command buffer comes from a live pool and is recorded once,
    // in order; every handle it references (pipeline, layout, set and its buffers) outlives
    // the submission below.
    // SAFETY: as above; `begin` has no external pointers.
    unsafe { raw.begin_command_buffer(command_buffer, &begin) }?;
    // SAFETY: as above.
    unsafe {
        raw.cmd_bind_pipeline(
            command_buffer,
            vk::PipelineBindPoint::COMPUTE,
            pipeline.handle(),
        );
    }
    // SAFETY: as above; the set matches the layout's only set layout.
    unsafe {
        raw.cmd_bind_descriptor_sets(
            command_buffer,
            vk::PipelineBindPoint::COMPUTE,
            layout.handle(),
            0,
            &[set],
            &[],
        );
    }
    // SAFETY: as above; one 16-thread workgroup covers every circle.
    unsafe { raw.cmd_dispatch(command_buffer, 1, 1, 1) };
    // SAFETY: as above.
    unsafe {
        raw.cmd_pipeline_barrier(
            command_buffer,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::HOST,
            vk::DependencyFlags::empty(),
            &to_host,
            &[],
            &[],
        );
    }
    // SAFETY: as above.
    unsafe { raw.end_command_buffer(command_buffer) }?;
    let fence = Fence::new(raw.clone(), false)?;
    let submit =
        [vk::SubmitInfo::default().command_buffers(core::slice::from_ref(&command_buffer))];
    // SAFETY: the queue belongs to this device, the command buffer is fully recorded, and
    // the fence is unsignalled.
    unsafe { raw.queue_submit(queue, &submit, fence.handle()) }?;
    // SAFETY: the fence belongs to this device; ten seconds is ample for one workgroup.
    let waited = unsafe { raw.wait_for_fences(&[fence.handle()], true, 10_000_000_000) };
    if waited.is_err() {
        // A timeout or device loss: nothing the submission uses may be destroyed while it
        // could still run, so drain the device before the early return drops them.
        // SAFETY: the device is live; waiting has no other precondition.
        let _ = unsafe { raw.device_wait_idle() };
    }
    waited?;

    let actual = output
        .mapped_slice_mut::<[f32; 4]>(entries)
        .ok_or(RenderError::Shader(
            "slang smoke output buffer is not mapped",
        ))?
        .to_vec();
    let mut expected = vec![[SENTINEL; 4]; entries];
    for (destination, circle) in expected.iter_mut().zip(&circles) {
        *destination = circle.to_array();
    }
    expected[VISION_CIRCLES] = (view_proj * camera_pos).to_array();
    let mismatch = (actual != expected).then(|| {
        let mut differences: Vec<String> = actual
            .iter()
            .zip(&expected)
            .enumerate()
            .filter(|(_, (actual, expected))| actual != expected)
            .map(|(index, (actual, expected))| {
                format!("[{index}] got {actual:?}, expected {expected:?}")
            })
            .collect();
        if actual[VISION_CIRCLES] == (view_proj.transpose() * camera_pos).to_array() {
            differences.push(String::from(
                "the matrix product is transposed: check -matrix-layout-column-major",
            ));
        }
        differences.join("; ")
    });

    Ok((device_name, mismatch))
}
