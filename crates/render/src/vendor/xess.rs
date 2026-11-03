//! Intel XeSS super resolution (feature `xess`, ADR-0020 §6).
//!
//! The library is opened at run time from beside the executable with `libloading`, so a
//! build with the feature still starts, and falls back, when it is absent. XeSS 3.0.2 ships
//! Windows binaries only (`libxess.dll`); on other platforms the probe looks for the
//! equivalent shared object and reports it missing. The library is opened once, before
//! `vkCreateInstance`, and stays loaded for the rest of the process.
//!
//! - **Before creation** (ADR-0020 §1e, ADR-0021 amendment 5): `xessVKGetRequiredInstanceExtensions`
//!   and `xessVKGetRequiredDeviceExtensions` give the instance and device extensions XeSS
//!   needs, which the instance and the tiered device request ([`super::instance_extensions`],
//!   [`super::device_extensions`]); `xessVKGetRequiredDeviceFeatures` gives its device
//!   features, which [`super::device_features`] folds into the renderer's core feature
//!   structures rather than chaining XeSS's standalone ones beside them
//!   (`VUID-VkDeviceCreateInfo-pNext-02830`, [`super::features`]).
//! - **The probe** checks the API version XeSS asks for, that every extension and feature it
//!   asks for was enabled, and calls `xessGetVersion`.
//! - **The upscaler** ([`upscaler::XessUpscaler`]) is created per renderer.
//!
//! XeSS 3.0.2 ships Windows binaries only, so on this project's Linux machines the probe
//! reports the library missing; the back end is compile-only until a Windows run.
//!
//! Licence: Intel Simplified Software License: redistribute the binaries unmodified, with the
//! notice; no reverse engineering, including at run time (`THIRD_PARTY.md`).

use std::ffi::{CStr, c_char};
use std::ptr;

use ash::vk::{self, Handle as _};

use super::features::FeatureBits;
use super::{
    Backend, Loaded, ProbeContext, Unavailable, UpscalerRequest, UpscalerResult,
    load_beside_executable, require_enabled,
};
use crate::caps::RenderPath;

mod upscaler;

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_imports,
    unreachable_pub,
    missing_docs,
    missing_debug_implementations,
    unsafe_op_in_unsafe_fn,
    clippy::all,
    clippy::pedantic,
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block
)]
mod sys {
    include!(concat!(env!("OUT_DIR"), "/xess.rs"));
}

const SDK_VERSION: &str = env!("MOBA_SDK_XESS_VERSION");

/// The library file the probe opens.
const LIBRARY: &str = if cfg!(windows) {
    "libxess.dll"
} else {
    "libxess.so"
};

/// The API version the instance is created with (`InstanceContext`).
const INSTANCE_API: u32 = vk::API_VERSION_1_3;

/// The XeSS library, opened on first use and never unloaded (see [`super::Loaded`]).
fn api() -> Result<&'static sys::XessApi, Unavailable> {
    static API: Loaded<sys::XessApi> = Loaded::new();
    API.get_or_init(|| {
        let library = load_beside_executable(LIBRARY).map_err(|reason| {
            if cfg!(windows) {
                reason
            } else {
                format!("{reason} (XeSS {SDK_VERSION} ships Windows binaries only)")
            }
        })?;
        // SAFETY: the library is XeSS's; `from_library` resolves every declared entry point by
        // name and fails if one is missing, so each stored pointer has the declared signature.
        unsafe { sys::XessApi::from_library(library) }
            .map_err(|error| format!("{LIBRARY} lacks an XeSS entry point: {error}"))
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// Copies an XeSS-owned array of `count` C strings.
fn names(
    result: sys::xess_result_t,
    count: u32,
    list: *const *const c_char,
    query: &str,
) -> Result<Vec<String>, Unavailable> {
    if result != sys::XESS_RESULT_SUCCESS {
        return Err(format!("{query} failed (xess_result_t {result})"));
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    if list.is_null() {
        return Err(format!("{query} reported {count} extensions but no list"));
    }
    // SAFETY: XeSS succeeded and says `list` points at `count` C strings it owns.
    let list = unsafe { std::slice::from_raw_parts(list, count as usize) };
    let mut out = Vec::with_capacity(list.len());
    for &name in list {
        if name.is_null() {
            return Err(format!("{query} returned a null extension name"));
        }
        // SAFETY: each entry is a NUL-terminated extension name owned by the library, which
        // stays loaded.
        out.push(
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned(),
        );
    }
    Ok(out)
}

/// The instance extensions XeSS needs, and the minimum API version, before `vkCreateInstance`.
fn instance_requirements() -> Result<(Vec<String>, u32), Unavailable> {
    let api = api()?;
    let mut count = 0u32;
    let mut list: *const *const c_char = ptr::null();
    let mut min_api = 0u32;
    // SAFETY: the out pointers are valid for the call; XeSS needs no instance for it.
    let result = unsafe {
        api.xessVKGetRequiredInstanceExtensions(&raw mut count, &raw mut list, &raw mut min_api)
    };
    let names = names(result, count, list, "xessVKGetRequiredInstanceExtensions")?;
    Ok((names, min_api))
}

/// The instance extensions XeSS asks for (ADR-0020 §1e).
pub(crate) fn instance_extensions() -> Result<Vec<String>, Unavailable> {
    instance_requirements().map(|(names, _)| names)
}

/// The device extensions XeSS needs on `physical`, before `vkCreateDevice`.
pub(crate) fn device_extensions(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
) -> Result<Vec<String>, Unavailable> {
    let api = api()?;
    let mut count = 0u32;
    let mut list: *const *const c_char = ptr::null();
    // SAFETY: the instance is live and `physical` was enumerated from it (XeSS's handle types
    // are pointers of the same size, checked below); the out pointers are valid for the call.
    let result = unsafe {
        api.xessVKGetRequiredDeviceExtensions(
            ptr::with_exposed_provenance_mut(instance.handle().as_raw() as usize),
            ptr::with_exposed_provenance_mut(physical.as_raw() as usize),
            &raw mut count,
            &raw mut list,
        )
    };
    names(result, count, list, "xessVKGetRequiredDeviceExtensions")
}

/// The device features XeSS needs on `physical`, before `vkCreateDevice`, folded into core
/// feature bits.
pub(crate) fn device_features(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
) -> Result<FeatureBits, Unavailable> {
    let api = api()?;
    let mut features: *mut std::ffi::c_void = ptr::null_mut();
    // SAFETY: as for `device_extensions`; XeSS returns a feature chain it owns.
    let result = unsafe {
        api.xessVKGetRequiredDeviceFeatures(
            ptr::with_exposed_provenance_mut(instance.handle().as_raw() as usize),
            ptr::with_exposed_provenance_mut(physical.as_raw() as usize),
            &raw mut features,
        )
    };
    if result != sys::XESS_RESULT_SUCCESS {
        return Err(format!(
            "xessVKGetRequiredDeviceFeatures failed (xess_result_t {result})"
        ));
    }
    // SAFETY: XeSS succeeded and `features` is null or a feature chain the library owns
    // (it stays loaded).
    unsafe { FeatureBits::fold(features) }.map_err(|reason| format!("XeSS asks for {reason}"))
}

/// The XeSS back end: the process-lifetime library.
struct Xess {
    api: &'static sys::XessApi,
    version: sys::xess_version_t,
}

impl Backend for Xess {
    fn name(&self) -> &'static str {
        "xess"
    }

    fn detail(&self) -> String {
        format!(
            "XeSS {}.{}.{} (SDK {SDK_VERSION})",
            self.version.major, self.version.minor, self.version.patch
        )
    }

    fn upscaler(&self, request: &UpscalerRequest<'_>) -> Option<UpscalerResult> {
        Some(
            upscaler::XessUpscaler::new(self.api, request)
                .map(|upscaler| Box::new(upscaler) as Box<dyn crate::upscale::TemporalUpscaler>),
        )
    }
}

/// Opens the XeSS library and checks that everything it asks for was enabled: the API
/// version, the instance and device extensions and the device features (ADR-0021 amendment
/// 5), then asks it for its version. Tiered only, like DLSS and FSR 4.
pub(crate) fn probe(context: &ProbeContext<'_>) -> Result<Box<dyn Backend>, Unavailable> {
    let api = api()?;
    if context.path != RenderPath::Tiered {
        return Err(String::from(
            "the device runs the compat path, and the vendor upscalers are tiered-only",
        ));
    }
    let (instance, min_api) = instance_requirements()?;
    if min_api > INSTANCE_API {
        return Err(format!(
            "XeSS needs Vulkan {}.{}, and the instance is created with 1.3",
            vk::api_version_major(min_api),
            vk::api_version_minor(min_api)
        ));
    }
    require_enabled(
        "XeSS",
        "instance",
        &instance,
        context.instance_extensions,
        context.path,
    )?;
    let device = device_extensions(context.instance, context.physical)?;
    require_enabled(
        "XeSS",
        "device",
        &device,
        context.device_extensions,
        context.path,
    )?;
    let missing =
        device_features(context.instance, context.physical)?.missing(context.enabled_features);
    if !missing.is_empty() {
        return Err(format!(
            "XeSS needs {} device feature{} the device does not support",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" }
        ));
    }
    let mut version = sys::xess_version_t {
        major: 0,
        minor: 0,
        patch: 0,
        reserved: 0,
    };
    // SAFETY: xessGetVersion writes the version into the struct it is given, which outlives
    // the call; it needs no context.
    let result = unsafe { api.xessGetVersion(&raw mut version) };
    if result != sys::XESS_RESULT_SUCCESS {
        return Err(format!("xessGetVersion failed (xess_result_t {result})"));
    }
    Ok(Box::new(Xess { api, version }))
}

/// XeSS's handle types are the same size as ash's, so the conversions above preserve them.
const _: () = {
    assert!(size_of::<sys::VkInstance>() == size_of::<vk::Instance>());
    assert!(size_of::<sys::VkPhysicalDevice>() == size_of::<vk::PhysicalDevice>());
};
