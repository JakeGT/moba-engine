//! Physical/logical device selection, queues and the GPU allocator.

use core::ffi::{CStr, c_char};
use core::fmt::Write as _;
use std::sync::{Arc, Mutex};

use ash::{Instance, vk};
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};

use crate::error::{RenderError, RenderResult};
use crate::instance::InstanceContext;

const SWAPCHAIN: &CStr = c"VK_KHR_swapchain";
const DYNAMIC_RENDERING: &CStr = c"VK_KHR_dynamic_rendering";
const SYNCHRONIZATION2: &CStr = c"VK_KHR_synchronization2";
const PORTABILITY_SUBSET: &CStr = c"VK_KHR_portability_subset";

pub(crate) struct DeviceContext {
    pub(crate) device: ash::Device,
    pub(crate) physical: vk::PhysicalDevice,
    pub(crate) graphics_family: u32,
    pub(crate) present_family: u32,
    pub(crate) graphics_queue: vk::Queue,
    pub(crate) present_queue: vk::Queue,
    pub(crate) allocator: Option<Arc<Mutex<Allocator>>>,
    pub(crate) name: String,
    pub(crate) surface: ash::khr::surface::Instance,
    core_13: bool,
    dynamic_rendering: Option<ash::khr::dynamic_rendering::Device>,
    synchronization2: Option<ash::khr::synchronization2::Device>,
}

struct Candidate {
    physical: vk::PhysicalDevice,
    properties: vk::PhysicalDeviceProperties,
    graphics_family: u32,
    present_family: u32,
    score: u32,
    core_13: bool,
    extensions: Vec<&'static CStr>,
    dynamic_rendering_extension: bool,
    synchronization2_extension: bool,
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
            if properties.api_version < vk::API_VERSION_1_2 {
                let _ = writeln!(reasons, "device too old ({:#x}); ", properties.api_version);
                continue;
            }
            let name = device_name(&properties);
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

            let core_13 = context.instance_version() >= vk::API_VERSION_1_3
                && properties.api_version >= vk::API_VERSION_1_3;
            let dynamic_rendering_extension = extension_supported(&available, DYNAMIC_RENDERING);
            let synchronization2_extension = extension_supported(&available, SYNCHRONIZATION2);
            if !core_13 && (!dynamic_rendering_extension || !synchronization2_extension) {
                let _ = writeln!(reasons, "{name}: no dynamic rendering/synchronization2; ");
                continue;
            }

            let features_12 = query_vulkan12(instance, physical);
            let (supports_dynamic_rendering, supports_synchronization2) = if core_13 {
                let features_13 = query_vulkan13(instance, physical);
                (
                    features_13.dynamic_rendering == vk::TRUE
                        && features_13.synchronization2 == vk::TRUE,
                    true,
                )
            } else {
                query_extension_features(instance, physical)
            };
            if !supports_dynamic_rendering || !supports_synchronization2 {
                let _ = writeln!(reasons, "{name}: required features unsupported; ");
                continue;
            }
            if !supports_descriptor_indexing(&features_12) {
                let _ = writeln!(reasons, "{name}: descriptor indexing unsupported; ");
                continue;
            }

            let mut extensions = vec![SWAPCHAIN];
            if extension_supported(&available, PORTABILITY_SUBSET) {
                extensions.push(PORTABILITY_SUBSET);
            }
            if !core_13 {
                extensions.push(DYNAMIC_RENDERING);
                extensions.push(SYNCHRONIZATION2);
            }

            let mut score = match properties.device_type {
                vk::PhysicalDeviceType::DISCRETE_GPU => 1000,
                vk::PhysicalDeviceType::INTEGRATED_GPU => 500,
                vk::PhysicalDeviceType::VIRTUAL_GPU => 200,
                vk::PhysicalDeviceType::CPU => 100,
                _ => 10,
            };
            if core_13 {
                score += 25;
            }
            if graphics_family == present_family {
                score += 10;
            }

            let candidate = Candidate {
                physical,
                properties,
                graphics_family,
                present_family,
                score,
                core_13,
                extensions,
                dynamic_rendering_extension,
                synchronization2_extension,
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

        let mut features_12 = vk::PhysicalDeviceVulkan12Features::default()
            .descriptor_indexing(true)
            .shader_sampled_image_array_non_uniform_indexing(true)
            .shader_storage_buffer_array_non_uniform_indexing(true)
            .descriptor_binding_partially_bound(true)
            .runtime_descriptor_array(true);
        let mut features_13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true);
        let mut dynamic_rendering =
            vk::PhysicalDeviceDynamicRenderingFeatures::default().dynamic_rendering(true);
        let mut synchronization2 =
            vk::PhysicalDeviceSynchronization2Features::default().synchronization2(true);

        let mut features_2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut features_12);
        if candidate.core_13 {
            features_2 = features_2.push_next(&mut features_13);
        } else {
            debug_assert!(candidate.dynamic_rendering_extension);
            debug_assert!(candidate.synchronization2_extension);
            features_2 = features_2
                .push_next(&mut dynamic_rendering)
                .push_next(&mut synchronization2);
        }

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

        // SAFETY: the queues belong to this device and family 0 was created.
        let graphics_queue = unsafe { device.get_device_queue(candidate.graphics_family, 0) };
        // SAFETY: same as above for the present family.
        let present_queue = unsafe { device.get_device_queue(candidate.present_family, 0) };

        let dynamic_rendering = if candidate.core_13 {
            None
        } else {
            Some(ash::khr::dynamic_rendering::Device::new(
                &context.instance,
                &device,
            ))
        };
        let synchronization2 = if candidate.core_13 {
            None
        } else {
            Some(ash::khr::synchronization2::Device::new(
                &context.instance,
                &device,
            ))
        };

        let allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.clone(),
            device: device.clone(),
            physical_device: physical,
            debug_settings: gpu_allocator::AllocatorDebugSettings {
                log_leaks_on_shutdown: false,
                ..Default::default()
            },
            buffer_device_address: false,
            allocation_sizes: gpu_allocator::AllocationSizes::default(),
        })?;

        Ok(Self {
            device,
            physical,
            graphics_family: candidate.graphics_family,
            present_family: candidate.present_family,
            graphics_queue,
            present_queue,
            allocator: Some(Arc::new(Mutex::new(allocator))),
            name: device_name(&candidate.properties),
            surface,
            core_13: candidate.core_13,
            dynamic_rendering,
            synchronization2,
        })
    }

    /// Starts dynamic rendering. Uses core Vulkan 1.3 on modern devices and the KHR
    /// extension on 1.2 devices.
    pub(crate) fn cmd_begin_rendering(
        &self,
        command_buffer: vk::CommandBuffer,
        rendering_info: &vk::RenderingInfo<'_>,
    ) {
        if self.core_13 {
            // SAFETY: the command buffer is recording and `rendering_info` is live.
            unsafe {
                self.device
                    .cmd_begin_rendering(command_buffer, rendering_info);
            }
        } else {
            let loader = self
                .dynamic_rendering
                .as_ref()
                .expect("extension loader exists on 1.2 devices");
            // SAFETY: the extension was enabled at device creation and the command buffer
            // is recording.
            unsafe { loader.cmd_begin_rendering(command_buffer, rendering_info) };
        }
    }

    /// Ends dynamic rendering.
    pub(crate) fn cmd_end_rendering(&self, command_buffer: vk::CommandBuffer) {
        if self.core_13 {
            // SAFETY: a dynamic rendering pass is active on this command buffer.
            unsafe { self.device.cmd_end_rendering(command_buffer) };
        } else {
            let loader = self
                .dynamic_rendering
                .as_ref()
                .expect("extension loader exists on 1.2 devices");
            // SAFETY: a dynamic rendering pass is active on this command buffer.
            unsafe { loader.cmd_end_rendering(command_buffer) };
        }
    }

    /// Records a synchronization2 pipeline barrier.
    pub(crate) fn cmd_pipeline_barrier2(
        &self,
        command_buffer: vk::CommandBuffer,
        dependency_info: &vk::DependencyInfo<'_>,
    ) {
        if self.core_13 {
            // SAFETY: the command buffer is recording and the dependency info is live.
            unsafe {
                self.device
                    .cmd_pipeline_barrier2(command_buffer, dependency_info);
            }
        } else {
            let loader = self
                .synchronization2
                .as_ref()
                .expect("extension loader exists on 1.2 devices");
            // SAFETY: the extension was enabled at device creation and the command buffer
            // is recording.
            unsafe { loader.cmd_pipeline_barrier2(command_buffer, dependency_info) };
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

fn query_vulkan12(
    instance: &Instance,
    physical: vk::PhysicalDevice,
) -> vk::PhysicalDeviceVulkan12Features<'static> {
    let mut features = vk::PhysicalDeviceVulkan12Features::default();
    {
        let mut features_2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut features);
        // SAFETY: the physical device belongs to this instance and the feature chain is
        // valid for the duration of the call.
        unsafe { instance.get_physical_device_features2(physical, &mut features_2) };
    }
    features
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

fn query_extension_features(instance: &Instance, physical: vk::PhysicalDevice) -> (bool, bool) {
    let mut dynamic_rendering = vk::PhysicalDeviceDynamicRenderingFeatures::default();
    let mut synchronization2 = vk::PhysicalDeviceSynchronization2Features::default();
    {
        let mut features_2 = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut dynamic_rendering)
            .push_next(&mut synchronization2);
        // SAFETY: the physical device belongs to this instance and every struct in the
        // chain is a valid extension feature struct for the call.
        unsafe { instance.get_physical_device_features2(physical, &mut features_2) };
    }
    (
        dynamic_rendering.dynamic_rendering == vk::TRUE,
        synchronization2.synchronization2 == vk::TRUE,
    )
}

fn supports_descriptor_indexing(features: &vk::PhysicalDeviceVulkan12Features) -> bool {
    features.descriptor_indexing == vk::TRUE
        && features.runtime_descriptor_array == vk::TRUE
        && features.descriptor_binding_partially_bound == vk::TRUE
        && features.shader_sampled_image_array_non_uniform_indexing == vk::TRUE
        && features.shader_storage_buffer_array_non_uniform_indexing == vk::TRUE
}
