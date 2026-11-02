//! DLSS super resolution through NGX (feature `dlss`, ADR-0020 §6).
//!
//! NGX's static library is linked (`build.rs`); the DLSS feature binary
//! (`libnvidia-ngx-dlss.so.*` / `nvngx_dlss.dll`) is loaded by NGX itself from beside the
//! executable, where the packaging step copies it. The N0 probe initialises NGX on the
//! renderer's device and asks whether DLSS super resolution is available; without the feature
//! binary NGX says no and the upscaler falls back. N1 creates and evaluates the feature.
//!
//! Licence: NVIDIA RTX SDKs License (DLSS `LICENSE.txt`); NVIDIA GPUs only, notify NVIDIA
//! before a commercial release, NVIDIA marks in the credits (`THIRD_PARTY.md`).

use std::ffi::{CStr, c_char, c_int};
use std::path::Path;
use std::ptr;

use ash::vk::{self, Handle as _};

use super::{Backend, ProbeContext, Unavailable, device_label, pci};

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unreachable_pub,
    missing_docs,
    missing_debug_implementations,
    clippy::all,
    clippy::pedantic,
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block
)]
mod sys {
    include!(concat!(env!("OUT_DIR"), "/ngx.rs"));
}

/// The engine's NGX project ID (a fixed GUID; NGX identifies custom engines by it).
const PROJECT_ID: &CStr = c"dc9bf563-a766-45c7-bcbd-4ce9459e6cc3";
/// The engine version NGX records with the project ID.
const ENGINE_VERSION: &CStr =
    match CStr::from_bytes_with_nul(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes()) {
        Ok(version) => version,
        Err(_) => panic!("CARGO_PKG_VERSION contains a NUL"),
    };
const SDK_VERSION: &str = env!("MOBA_SDK_DLSS_VERSION");

/// The DLSS back end: an initialised NGX session on the renderer's device. Shut down before
/// the device is destroyed (the renderer drops its vendor selection first).
struct Dlss {
    _session: Session,
}

impl Backend for Dlss {
    fn name(&self) -> &'static str {
        "dlss"
    }

    fn detail(&self) -> String {
        format!("DLSS SDK {SDK_VERSION}")
    }
}

/// An NGX session; dropping it shuts NGX down for the device.
struct Session {
    /// `VkDevice` as a raw handle (plain data, so the back end is `Send`).
    device: u64,
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: NGX was initialised on this device, which is still alive: the renderer drops
        // its vendor selection after waiting for the device to go idle and before destroying
        // it. Shutdown1 with the device releases the per-device state exactly once.
        unsafe { sys::NVSDK_NGX_VULKAN_Shutdown1(handle(self.device)) };
    }
}

/// A Vulkan handle as the pointer type the NGX declarations use.
fn handle<T>(raw: u64) -> *mut T {
    ptr::with_exposed_provenance_mut(raw as usize)
}

/// `NVSDK_NGX_SUCCEED`.
fn succeeded(result: sys::NVSDK_NGX_Result) -> bool {
    (result & 0xFFF0_0000) != sys::NVSDK_NGX_Result_Fail
}

/// A readable name for an NGX result code.
fn result_name(result: sys::NVSDK_NGX_Result) -> String {
    let name = match result {
        sys::NVSDK_NGX_Result_Success => "Success",
        sys::NVSDK_NGX_Result_Fail => "Fail",
        sys::NVSDK_NGX_Result_FAIL_FeatureNotSupported => "FeatureNotSupported",
        sys::NVSDK_NGX_Result_FAIL_PlatformError => "PlatformError",
        sys::NVSDK_NGX_Result_FAIL_FeatureNotFound => "FeatureNotFound",
        sys::NVSDK_NGX_Result_FAIL_InvalidParameter => "InvalidParameter",
        sys::NVSDK_NGX_Result_FAIL_OutOfDate => "OutOfDate",
        sys::NVSDK_NGX_Result_FAIL_NotInitialized => "NotInitialized",
        sys::NVSDK_NGX_Result_FAIL_UnsupportedParameter => "UnsupportedParameter",
        sys::NVSDK_NGX_Result_FAIL_Denied => "Denied",
        _ => "",
    };
    if name.is_empty() {
        format!("{result:#010x}")
    } else {
        format!("NVSDK_NGX_Result_{name}, {result:#010x}")
    }
}

/// The application data path NGX logs into, as the platform's `wchar_t` string.
#[cfg(windows)]
fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// The application data path NGX logs into, as the platform's `wchar_t` string (UTF-32).
#[cfg(not(windows))]
fn wide_path(path: &Path) -> Vec<u32> {
    path.to_string_lossy()
        .chars()
        .map(u32::from)
        .chain(std::iter::once(0))
        .collect()
}

/// `NVSDK_NGX_Parameter_GetI`, `None` when NGX does not report `name`.
fn get_i(parameters: *mut sys::NVSDK_NGX_Parameter, name: &[u8]) -> Option<c_int> {
    let name = CStr::from_bytes_with_nul(name).ok()?;
    let mut value: c_int = 0;
    // SAFETY: `parameters` came from NVSDK_NGX_VULKAN_GetCapabilityParameters and is not yet
    // destroyed; `name` is NUL-terminated and `value` outlives the call.
    let result =
        unsafe { sys::NVSDK_NGX_Parameter_GetI(parameters, name.as_ptr(), &raw mut value) };
    succeeded(result).then_some(value)
}

/// Initialises NGX on the device and checks that DLSS super resolution is available.
pub(crate) fn probe(context: &ProbeContext<'_>) -> Result<Box<dyn Backend>, Unavailable> {
    if context.capabilities.vendor_id != pci::NVIDIA {
        return Err(format!(
            "needs an NVIDIA RTX GPU, and the device is {}",
            device_label(context.capabilities)
        ));
    }
    let data_dir = std::env::temp_dir().join("moba-ngx");
    let _ = std::fs::create_dir_all(&data_dir);
    let data_path = wide_path(&data_dir);
    let device = context.device.handle().as_raw();
    // SAFETY: the instance, physical device and device are the renderer's own, alive for the
    // call. The strings are NUL-terminated and outlive it (NGX copies what it keeps). Null
    // proc-address callbacks and feature info select NGX's defaults (the system loader and
    // feature binaries beside the executable).
    let result = unsafe {
        sys::NVSDK_NGX_VULKAN_Init_with_ProjectID(
            PROJECT_ID.as_ptr(),
            sys::NVSDK_NGX_ENGINE_TYPE_CUSTOM,
            ENGINE_VERSION.as_ptr(),
            data_path.as_ptr().cast(),
            handle(context.instance.handle().as_raw()),
            handle(context.physical.as_raw()),
            handle(device),
            None,
            None,
            ptr::null(),
            sys::NVSDK_NGX_Version_API,
        )
    };
    if !succeeded(result) {
        return Err(format!(
            "NGX initialisation failed on {} ({})",
            device_label(context.capabilities),
            result_name(result)
        ));
    }
    // From here on, dropping the session shuts NGX down again.
    let session = Session { device };

    let mut parameters: *mut sys::NVSDK_NGX_Parameter = ptr::null_mut();
    // SAFETY: NGX is initialised; the out pointer is valid for the call.
    let result = unsafe { sys::NVSDK_NGX_VULKAN_GetCapabilityParameters(&raw mut parameters) };
    if !succeeded(result) || parameters.is_null() {
        return Err(format!(
            "NGX capability query failed ({})",
            result_name(result)
        ));
    }
    let available = get_i(parameters, sys::NVSDK_NGX_Parameter_SuperSampling_Available);
    let needs_driver = get_i(
        parameters,
        sys::NVSDK_NGX_Parameter_SuperSampling_NeedsUpdatedDriver,
    );
    let min_major = get_i(
        parameters,
        sys::NVSDK_NGX_Parameter_SuperSampling_MinDriverVersionMajor,
    );
    let min_minor = get_i(
        parameters,
        sys::NVSDK_NGX_Parameter_SuperSampling_MinDriverVersionMinor,
    );
    let init_result = get_i(
        parameters,
        sys::NVSDK_NGX_Parameter_SuperSampling_FeatureInitResult,
    );
    // SAFETY: `parameters` came from GetCapabilityParameters and is destroyed exactly once,
    // with the matching function, before NGX shuts down.
    unsafe { sys::NVSDK_NGX_VULKAN_DestroyParameters(parameters) };

    if available == Some(1) {
        return Ok(Box::new(Dlss { _session: session }));
    }
    drop(session);
    if needs_driver == Some(1) {
        return Err(format!(
            "the driver is too old for DLSS {SDK_VERSION} (NGX asks for {}.{} or newer)",
            min_major.unwrap_or(0),
            min_minor.unwrap_or(0)
        ));
    }
    // FeatureInitResult holds an NVSDK_NGX_Result; reinterpret the int's bits.
    let init = init_result.map(c_int::cast_unsigned);
    Err(match init {
        Some(sys::NVSDK_NGX_Result_FAIL_FeatureNotFound) => format!(
            "NGX found no DLSS {SDK_VERSION} feature binary ({}) beside the executable or in the \
             driver (NVSDK_NGX_Result_FAIL_FeatureNotFound)",
            feature_binary()
        ),
        Some(code) => format!(
            "NGX reports DLSS super resolution unavailable ({})",
            result_name(code)
        ),
        None => String::from("NGX reports DLSS super resolution unavailable"),
    })
}

/// The feature binary NGX looks for.
fn feature_binary() -> String {
    if cfg!(windows) {
        String::from("nvngx_dlss.dll")
    } else {
        format!("libnvidia-ngx-dlss.so.{SDK_VERSION}")
    }
}

/// Keeps the Vulkan handle types in this module's signature checked against `ash`'s.
const _: () = {
    assert!(size_of::<vk::Instance>() == size_of::<*mut c_char>());
};
