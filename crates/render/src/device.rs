//! Physical/logical device selection, queues and the GPU allocator.

use core::ffi::{CStr, c_char};
use core::fmt::Write as _;
use std::sync::{Arc, Mutex};

use ash::{Instance, vk};
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};

use crate::error::{RenderError, RenderResult};
use crate::instance::InstanceContext;

const SWAPCHAIN: &CStr = c"VK_KHR_swapchain";
const PORTABILITY_SUBSET: &CStr = c"VK_KHR_portability_subset";

pub(crate) struct DeviceContext {
    pub(crate) device: ash::Device,
    /// The instance's function table, for physical-device queries after creation.
    pub(crate) instance: ash::Instance,
    pub(crate) physical: vk::PhysicalDevice,
    pub(crate) graphics_family: u32,
    pub(crate) present_family: u32,
    pub(crate) graphics_queue: vk::Queue,
    pub(crate) present_queue: vk::Queue,
    pub(crate) allocator: Option<Arc<Mutex<Allocator>>>,
    pub(crate) name: String,
    pub(crate) surface: ash::khr::surface::Instance,
    /// `VkPhysicalDeviceLimits::maxStorageBufferRange`: the largest storage-buffer
    /// descriptor range (bytes).
    pub(crate) max_storage_buffer_range: u32,
    /// `VkPhysicalDeviceLimits::timestampPeriod`: nanoseconds per timestamp tick.
    pub(crate) timestamp_period: f32,
    /// `timestampValidBits` of the graphics queue family; `0` means no timestamps.
    pub(crate) timestamp_valid_bits: u32,
}

struct Candidate {
    physical: vk::PhysicalDevice,
    properties: vk::PhysicalDeviceProperties,
    graphics_family: u32,
    present_family: u32,
    score: u32,
    extensions: Vec<&'static CStr>,
}

impl DeviceContext {
    #[allow(clippy::too_many_lines)] // Device selection is one auditable decision table.
    pub(crate) fn new(context: &InstanceContext, surface: vk::SurfaceKHR) -> RenderResult<Self> {
        let instance = &context.instance;
        let surface_loader = ash::khr::surface::Instance::new(&context.entry, instance);
        // SAFETY: the instance is live; enumeration returns owned handles.
        let devices = unsafe { instance.enumerate_physical_devices() }?;
        if devices.is_empty() {
            return Err(RenderError::NoSuitableDevice(String::from(
                "the loader found no Vulkan devices",
            )));
        }

        let mut best: Option<Candidate> = None;
        let mut reasons = String::new();
        for physical in devices {
            // SAFETY: `physical` came from this instance.
            let properties = unsafe { instance.get_physical_device_properties(physical) };
            let name = device_name(&properties);
            if properties.api_version < vk::API_VERSION_1_3 {
                let version = properties.api_version;
                let _ = writeln!(
                    reasons,
                    "{name}: Vulkan {}.{} < 1.3; ",
                    vk::api_version_major(version),
                    vk::api_version_minor(version)
                );
                continue;
            }
            // SAFETY: `physical` came from this instance.
            let families =
                unsafe { instance.get_physical_device_queue_family_properties(physical) };
            let Some(graphics_family) = families
                .iter()
                .position(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            else {
                let _ = writeln!(reasons, "{name}: no graphics queue; ");
                continue;
            };
            let graphics_family = u32::try_from(graphics_family).expect("queue family fits u32");
            let present_family = families.iter().enumerate().find_map(|(index, _)| {
                // SAFETY: the surface and physical device belong to this instance.
                match unsafe {
                    surface_loader.get_physical_device_surface_support(
                        physical,
                        u32::try_from(index).expect("queue family fits u32"),
                        surface,
                    )
                } {
                    Ok(true) => Some(u32::try_from(index).expect("queue family fits u32")),
                    _ => None,
                }
            });
            let Some(present_family) = present_family else {
                let _ = writeln!(reasons, "{name}: no present queue; ");
                continue;
            };

            // SAFETY: `physical` came from this instance.
            let available = unsafe { instance.enumerate_device_extension_properties(physical) }?;
            if !extension_supported(&available, SWAPCHAIN) {
                let _ = writeln!(reasons, "{name}: no swapchain extension; ");
                continue;
            }

            // Both features are mandatory in Vulkan 1.3; they are still checked because
            // portability drivers (MoltenVK) are not fully conformant.
            let features_13 = query_vulkan13(instance, physical);
            if features_13.dynamic_rendering != vk::TRUE || features_13.synchronization2 != vk::TRUE
            {
                let _ = writeln!(reasons, "{name}: no dynamic rendering/synchronization2; ");
                continue;
            }
            let mut extensions = vec![SWAPCHAIN];
            if extension_supported(&available, PORTABILITY_SUBSET) {
                extensions.push(PORTABILITY_SUBSET);
            }

            let mut score = match properties.device_type {
                vk::PhysicalDeviceType::DISCRETE_GPU => 1000,
                vk::PhysicalDeviceType::INTEGRATED_GPU => 500,
                vk::PhysicalDeviceType::VIRTUAL_GPU => 200,
                vk::PhysicalDeviceType::CPU => 100,
                _ => 10,
            };
            if graphics_family == present_family {
                score += 10;
            }

            let candidate = Candidate {
                physical,
                properties,
                graphics_family,
                present_family,
                score,
                extensions,
            };
            if best
                .as_ref()
                .is_none_or(|current| candidate.score > current.score)
            {
                best = Some(candidate);
            }
        }

        let Some(candidate) = best else {
            return Err(RenderError::NoSuitableDevice(reasons));
        };
        Self::create(context, surface_loader, &candidate)
    }

    fn create(
        context: &InstanceContext,
        surface: ash::khr::surface::Instance,
        candidate: &Candidate,
    ) -> RenderResult<Self> {
        let instance = &context.instance;
        let physical = candidate.physical;

        let mut features_13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true);
        let mut features_2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut features_13);

        let priorities = [1.0f32];
        let mut queue_infos = vec![
            vk::DeviceQueueCreateInfo::default()
                .queue_family_index(candidate.graphics_family)
                .queue_priorities(&priorities),
        ];
        if candidate.present_family != candidate.graphics_family {
            queue_infos.push(
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(candidate.present_family)
                    .queue_priorities(&priorities),
            );
        }

        let extension_names: Vec<*const c_char> = candidate
            .extensions
            .iter()
            .map(|name| name.as_ptr())
            .collect();
        let create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&extension_names)
            .push_next(&mut features_2);

        // SAFETY: the physical device was enumerated from this instance, all requested
        // extensions/features were checked above, and the pointers in `create_info` are
        // valid for the duration of the call.
        let device = unsafe { instance.create_device(physical, &create_info, None) }?;

        // SAFETY: `physical` came from this instance.
        let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
        let timestamp_valid_bits = families
            .get(candidate.graphics_family as usize)
            .map_or(0, |family| family.timestamp_valid_bits);

        // SAFETY: the queues belong to this device and family 0 was created.
        let graphics_queue = unsafe { device.get_device_queue(candidate.graphics_family, 0) };
        // SAFETY: same as above for the present family.
        let present_queue = unsafe { device.get_device_queue(candidate.present_family, 0) };

        // `AllocatorDebugSettings` is `#[non_exhaustive]` since gpu-allocator 0.28.
        let mut debug_settings = gpu_allocator::AllocatorDebugSettings::default();
        debug_settings.log_leaks_on_shutdown = false;
        let allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.clone(),
            device: device.clone(),
            physical_device: physical,
            debug_settings,
            buffer_device_address: false,
            allocation_sizes: gpu_allocator::AllocationSizes::default(),
        })?;

        Ok(Self {
            device,
            instance: instance.clone(),
            physical,
            graphics_family: candidate.graphics_family,
            present_family: candidate.present_family,
            graphics_queue,
            present_queue,
            allocator: Some(Arc::new(Mutex::new(allocator))),
            name: device_name(&candidate.properties),
            surface,
            max_storage_buffer_range: candidate.properties.limits.max_storage_buffer_range,
            timestamp_period: candidate.properties.limits.timestamp_period,
            timestamp_valid_bits,
        })
    }

    /// Starts dynamic rendering (core Vulkan 1.3).
    pub(crate) fn cmd_begin_rendering(
        &self,
        command_buffer: vk::CommandBuffer,
        rendering_info: &vk::RenderingInfo<'_>,
    ) {
        // SAFETY: the command buffer is recording, `rendering_info` is live, and the
        // `dynamicRendering` feature was enabled at device creation.
        unsafe {
            self.device
                .cmd_begin_rendering(command_buffer, rendering_info);
        }
    }

    /// Ends dynamic rendering.
    pub(crate) fn cmd_end_rendering(&self, command_buffer: vk::CommandBuffer) {
        // SAFETY: a dynamic rendering pass is active on this command buffer.
        unsafe { self.device.cmd_end_rendering(command_buffer) };
    }

    /// Records a synchronization2 pipeline barrier (core Vulkan 1.3).
    pub(crate) fn cmd_pipeline_barrier2(
        &self,
        command_buffer: vk::CommandBuffer,
        dependency_info: &vk::DependencyInfo<'_>,
    ) {
        // SAFETY: the command buffer is recording, the dependency info is live, and the
        // `synchronization2` feature was enabled at device creation.
        unsafe {
            self.device
                .cmd_pipeline_barrier2(command_buffer, dependency_info);
        }
    }

    /// Returns the allocator handle used by GPU resources.
    pub(crate) fn allocator(&self) -> Arc<Mutex<Allocator>> {
        Arc::clone(
            self.allocator
                .as_ref()
                .expect("allocator is alive while the device is"),
        )
    }
}

impl Drop for DeviceContext {
    fn drop(&mut self) {
        // SAFETY: this waits for all queues of a live device to go idle.
        let _ = unsafe { self.device.device_wait_idle() };
        // Free GPU memory before destroying the device. Every `GpuBuffer`/`GpuImage` is
        // dropped before the device context by `Renderer`'s field order, so the allocator
        // should have no other owners left.
        if let Some(allocator) = self.allocator.take() {
            debug_assert_eq!(Arc::strong_count(&allocator), 1);
            drop(allocator);
        }
        // SAFETY: every child object has been destroyed and the device is destroyed once.
        unsafe { self.device.destroy_device(None) };
    }
}

fn device_name(properties: &vk::PhysicalDeviceProperties) -> String {
    // SAFETY: `deviceName` is a NUL-terminated array owned by the properties struct.
    unsafe { CStr::from_ptr(properties.device_name.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn extension_supported(available: &[vk::ExtensionProperties], name: &CStr) -> bool {
    available.iter().any(|extension| {
        // SAFETY: Vulkan guarantees NUL termination inside the fixed-size name array.
        let extension_name = unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) };
        extension_name == name
    })
}

fn query_vulkan13(
    instance: &Instance,
    physical: vk::PhysicalDevice,
) -> vk::PhysicalDeviceVulkan13Features<'static> {
    let mut features = vk::PhysicalDeviceVulkan13Features::default();
    {
        let mut features_2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut features);
        // SAFETY: the physical device belongs to this instance and the feature chain is
        // valid for the duration of the call.
        unsafe { instance.get_physical_device_features2(physical, &mut features_2) };
    }
    features
}
