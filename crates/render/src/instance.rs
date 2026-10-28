//! Vulkan instance, validation layer and debug messenger.

use ash::{Entry, Instance, vk};
use core::ffi::{CStr, c_char};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{RenderError, RenderResult};

/// How aggressively to enable the Khronos validation layer.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ValidationMode {
    /// Never enable validation.
    Off,
    /// Enable validation when the layer is installed.
    #[default]
    IfAvailable,
    /// Fail device creation when validation is unavailable.
    Required,
}

/// Result of a headless Vulkan instance/device probe.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct VulkanProbe {
    /// Loader/instance API version.
    pub api_version: u32,
    /// Number of physical devices enumerated.
    pub devices: u32,
    /// Whether `VK_KHR_portability_enumeration` was advertised and enabled.
    pub portability: bool,
}

/// Creates an instance and enumerates physical devices without a surface.
///
/// Used by CI to exercise the macOS/MoltenVK portability path on a runner.
///
/// # Errors
/// Returns the same instance/extension errors as the renderer initialisation path.
pub fn probe_vulkan() -> RenderResult<VulkanProbe> {
    let context = InstanceContext::new(&[], ValidationMode::Off)?;
    // SAFETY: the instance is live and the entry point belongs to it.
    let devices = unsafe { context.instance.enumerate_physical_devices() }?;
    Ok(VulkanProbe {
        api_version: context.instance_version,
        devices: u32::try_from(devices.len()).unwrap_or(u32::MAX),
        portability: context.portability,
    })
}

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";
const PORTABILITY_ENUMERATION: &CStr = c"VK_KHR_portability_enumeration";
static VALIDATION_ERRORS: AtomicU64 = AtomicU64::new(0);

/// Owns the Vulkan entry point and instance.
pub(crate) struct InstanceContext {
    // `instance` must be destroyed before `entry` unloads the loader.
    pub(crate) instance: Instance,
    pub(crate) entry: Entry,
    debug_messenger: Option<vk::DebugUtilsMessengerEXT>,
    validation_enabled: bool,
    portability: bool,
    instance_version: u32,
}

impl InstanceContext {
    #[allow(clippy::too_many_lines)] // Instance setup is one explicit ordered sequence.
    pub(crate) fn new(
        required_extensions: &[&CStr],
        validation: ValidationMode,
    ) -> RenderResult<Self> {
        // SAFETY: loading the platform Vulkan loader is the entry point of the process's
        // Vulkan usage. The loader stays loaded for the lifetime of `Entry`.
        let entry = unsafe { Entry::load() }.map_err(RenderError::Loader)?;

        // SAFETY: the loader is initialised and the call has no pointer arguments.
        let layer_properties = unsafe { entry.enumerate_instance_layer_properties() }?;
        let has_validation = layer_properties
            .iter()
            .any(|layer| cstr_from_fixed(&layer.layer_name) == VALIDATION_LAYER);
        let validation_enabled = match validation {
            ValidationMode::Off => false,
            ValidationMode::IfAvailable => has_validation,
            ValidationMode::Required => {
                if !has_validation {
                    return Err(RenderError::MissingLayer(String::from(
                        "VK_LAYER_KHRONOS_validation",
                    )));
                }
                true
            }
        };

        // SAFETY: the loader is initialised; passing null asks for global extensions.
        let available_extensions = unsafe { entry.enumerate_instance_extension_properties(None) }?;
        let mut extensions: Vec<*const c_char> = required_extensions
            .iter()
            .map(|name| name.as_ptr())
            .collect();
        let mut create_flags = vk::InstanceCreateFlags::empty();
        let portability = extension_supported(&available_extensions, PORTABILITY_ENUMERATION);
        if portability {
            create_flags |= vk::InstanceCreateFlags::ENUMERATE_PORTABILITY_KHR;
            if !required_extensions.contains(&PORTABILITY_ENUMERATION) {
                extensions.push(PORTABILITY_ENUMERATION.as_ptr());
            }
        }
        for name in required_extensions {
            if !extension_supported(&available_extensions, name) {
                return Err(RenderError::MissingInstanceExtension(
                    name.to_string_lossy().into_owned(),
                ));
            }
        }
        let debug_utils = c"VK_EXT_debug_utils";
        if validation_enabled {
            if !extension_supported(&available_extensions, debug_utils) {
                return Err(RenderError::MissingInstanceExtension(String::from(
                    "VK_EXT_debug_utils",
                )));
            }
            extensions.push(debug_utils.as_ptr());
        }

        let mut layers: Vec<*const c_char> = Vec::new();
        if validation_enabled {
            layers.push(VALIDATION_LAYER.as_ptr());
        }

        // SAFETY: `try_enumerate_instance_version` is a Vulkan 1.1 entry point; loaders
        // older than 1.1 return `None`, which we treat as Vulkan 1.0.
        let loader_version =
            unsafe { entry.try_enumerate_instance_version() }?.unwrap_or(vk::API_VERSION_1_0);
        if loader_version < vk::API_VERSION_1_3 {
            return Err(RenderError::UnsupportedApiVersion(loader_version));
        }
        // Vulkan 1.3 is the baseline (ADR-0002); request exactly that API version.
        let instance_version = vk::API_VERSION_1_3;

        let application_name = c"MOBA";
        let engine_name = c"moba-engine";
        let application_info = vk::ApplicationInfo::default()
            .application_name(application_name)
            .application_version(vk::make_api_version(0, 0, 1, 0))
            .engine_name(engine_name)
            .engine_version(vk::make_api_version(0, 0, 1, 0))
            .api_version(instance_version);

        let mut create_info = vk::InstanceCreateInfo::default()
            .application_info(&application_info)
            .enabled_extension_names(&extensions)
            .enabled_layer_names(&layers)
            .flags(create_flags);
        let mut synchronization_validation = vk::ValidationFeaturesEXT::default()
            .enabled_validation_features(&[
                vk::ValidationFeatureEnableEXT::SYNCHRONIZATION_VALIDATION,
            ]);
        if validation_enabled && validation == ValidationMode::Required {
            create_info = create_info.push_next(&mut synchronization_validation);
        }

        // SAFETY: `create_info` points at live arrays for the duration of the call, and
        // every extension/layer was checked against the loader's list.
        let instance = unsafe { entry.create_instance(&create_info, None) }?;

        let debug_messenger = if validation_enabled {
            let callback_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(
                    vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                        | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
                )
                .message_type(
                    vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                        | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                        | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                )
                .pfn_user_callback(Some(debug_callback));
            let debug_utils = ash::ext::debug_utils::Instance::new(&entry, &instance);
            // SAFETY: the instance is live and owns the debug-utils entry points that the
            // callback info references.
            Some(unsafe { debug_utils.create_debug_utils_messenger(&callback_info, None) }?)
        } else {
            None
        };

        Ok(Self {
            instance,
            entry,
            debug_messenger,
            validation_enabled,
            portability,
            instance_version,
        })
    }

    pub(crate) fn validation_enabled(&self) -> bool {
        self.validation_enabled
    }

    pub(crate) fn validation_error_count() -> u64 {
        VALIDATION_ERRORS.load(Ordering::Relaxed)
    }
}

impl Drop for InstanceContext {
    fn drop(&mut self) {
        if let Some(messenger) = self.debug_messenger.take() {
            let debug_utils = ash::ext::debug_utils::Instance::new(&self.entry, &self.instance);
            // SAFETY: the messenger was created from this instance and is destroyed once.
            unsafe { debug_utils.destroy_debug_utils_messenger(messenger, None) };
        }
        // SAFETY: every child object of the instance has been destroyed by its owner, and
        // the instance is destroyed exactly once.
        unsafe { self.instance.destroy_instance(None) };
    }
}

fn extension_supported(available: &[vk::ExtensionProperties], name: &CStr) -> bool {
    available
        .iter()
        .any(|extension| cstr_from_fixed(&extension.extension_name) == name)
}

fn cstr_from_fixed(name: &[c_char; vk::MAX_EXTENSION_NAME_SIZE]) -> &CStr {
    // SAFETY: Vulkan guarantees extension/layer names are NUL-terminated inside the fixed
    // array, and the array is borrowed for the returned lifetime.
    unsafe { CStr::from_ptr(name.as_ptr()) }
}

unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    message_type: vk::DebugUtilsMessageTypeFlagsEXT,
    callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user_data: *mut core::ffi::c_void,
) -> vk::Bool32 {
    if callback_data.is_null() {
        return vk::FALSE;
    }
    // SAFETY: Vulkan only calls the callback with a valid pointer to callback data for the
    // duration of the call.
    let data = unsafe { &*callback_data };
    let message = if data.p_message.is_null() {
        "<no message>"
    } else {
        // SAFETY: `p_message` is documented as NUL-terminated when non-null.
        unsafe { CStr::from_ptr(data.p_message) }
            .to_str()
            .unwrap_or("<non-utf8 message>")
    };
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        VALIDATION_ERRORS.fetch_add(1, Ordering::Relaxed);
    }
    eprintln!("[vulkan {severity:?} {message_type:?}] {message}");
    vk::FALSE
}
