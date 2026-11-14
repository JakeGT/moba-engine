//! DLSS super resolution through NGX (feature `dlss`, ADR-0020 §6).
//!
//! NGX's static library is linked (`build.rs`); the DLSS feature binary
//! (`libnvidia-ngx-dlss.so.*` / `nvngx_dlss.dll`) is loaded by NGX itself from beside the
//! executable, where the packaging step copies it. N1 creates and evaluates the feature.
//!
//! - **Before creation** (ADR-0020 §1e), NGX reports the instance and device extensions DLSS
//!   needs (`VK_NVX_binary_import`, `VK_NVX_image_view_handle` and others):
//!   [`instance_extensions`] before `vkCreateInstance`, [`device_extensions`] before
//!   `vkCreateDevice`. The device query loads the driver's NGX core, so it is asked only on
//!   NVIDIA's own driver.
//! - **The probe** repeats both queries against what was enabled, then initialises NGX on the
//!   renderer's device (through our loader's `vkGetInstanceProcAddr`) and asks whether DLSS
//!   super resolution is available. Without the feature binary NGX says no and the upscaler
//!   falls back.
//!
//! NGX keeps logs and downloaded data in a per-user cache directory ([`data_dir`]), never in a
//! shared temporary directory.
//!
//! Licence: NVIDIA RTX SDKs License (DLSS `LICENSE.txt`); NVIDIA GPUs only, notify NVIDIA
//! before a commercial release, NVIDIA marks in the credits (`THIRD_PARTY.md`).

use std::ffi::{CStr, c_char, c_int};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Arc, OnceLock};

use ash::vk::{self, Handle as _};

use super::{
    Backend, ProbeContext, Unavailable, UpscalerRequest, UpscalerResult, device_label,
    nvidia_driver, require_enabled,
};

mod dlss;
use crate::caps::RenderPath;

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

/// The DLSS back end: an initialised NGX session on the renderer's device, shared with the
/// DLSS upscaler made from it. Shut down before the device is destroyed (the renderer drops
/// its temporal state, then its vendor selection, then the device).
struct Dlss {
    session: Arc<Session>,
}

impl Backend for Dlss {
    fn name(&self) -> &'static str {
        "dlss"
    }

    fn detail(&self) -> String {
        format!("DLSS SDK {SDK_VERSION}")
    }

    fn upscaler(&self, request: &UpscalerRequest<'_>) -> Option<UpscalerResult> {
        Some(
            dlss::DlssUpscaler::new(Arc::clone(&self.session), request)
                .map(|upscaler| Box::new(upscaler) as Box<dyn crate::upscale::TemporalUpscaler>),
        )
    }
}

/// An NGX session; dropping it shuts NGX down for the device. Every DLSS feature and parameter
/// map holds the session, so NGX shuts down after the last of them is released.
pub(super) struct Session {
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

/// An `NVSDK_NGX_Result`'s bits. The C enum has values above `INT_MAX`, so it is `unsigned
/// int` for GCC and Clang but `int` for MSVC, and bindgen follows the target: comparisons go
/// through the bits so they read the same on both.
fn bits(result: sys::NVSDK_NGX_Result) -> u32 {
    u32::from_ne_bytes(result.to_ne_bytes())
}

/// `NVSDK_NGX_SUCCEED`.
fn succeeded(result: sys::NVSDK_NGX_Result) -> bool {
    (bits(result) & 0xFFF0_0000) != bits(sys::NVSDK_NGX_Result_Fail)
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

/// NGX's application data directory and its path as the platform's `wchar_t` string.
struct DataDir {
    path: PathBuf,
    #[cfg(windows)]
    wide: Vec<u16>,
    #[cfg(not(windows))]
    wide: Vec<u32>,
}

/// The per-user directory NGX logs into and keeps its data in, created on first use:
/// `$XDG_CACHE_HOME` (or `~/.cache`) on Linux and `%LOCALAPPDATA%` on Windows, each under
/// `moba-engine/ngx`, beside the client's pipeline cache. On Unix it must be a real directory
/// (not a symlink) owned by this user and writable by nobody else. Checked once per process.
fn data_dir() -> Result<&'static DataDir, Unavailable> {
    static DIR: OnceLock<Result<DataDir, Unavailable>> = OnceLock::new();
    DIR.get_or_init(|| {
        let path = cache_base()?.join("moba-engine").join("ngx");
        create_private_dir(&path)
            .map_err(|reason| format!("NGX's data directory {}: {reason}", path.display()))?;
        #[cfg(windows)]
        let wide = wide_path(&path);
        // NGX takes the path as a `wchar_t` string; a lossy conversion would point it at a
        // different directory, so a path that is not UTF-8 is refused instead.
        #[cfg(not(windows))]
        let wide = wide_path(&path).ok_or_else(|| {
            format!(
                "NGX's data directory {} is not valid UTF-8, which NGX's wide-character path needs",
                path.display()
            )
        })?;
        Ok(DataDir { path, wide })
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// The per-user cache directory (the same base the client's pipeline cache uses).
fn cache_base() -> Result<PathBuf, Unavailable> {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let base = if cfg!(windows) {
        set("LOCALAPPDATA").map(PathBuf::from)
    } else {
        // XDG: a relative $XDG_CACHE_HOME is invalid and ignored.
        set("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| set("HOME").map(|home| PathBuf::from(home).join(".cache")))
    };
    base.ok_or_else(|| {
        String::from(if cfg!(windows) {
            "no per-user cache directory for NGX's data (%LOCALAPPDATA% is not set)"
        } else {
            "no per-user cache directory for NGX's data ($XDG_CACHE_HOME and $HOME are not set)"
        })
    })
}

/// Creates `path` (mode 0700) unless it exists, then checks it: a directory, not a symlink,
/// owned by this process's user and not writable by group or others.
#[cfg(unix)]
fn create_private_dir(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("cannot create it ({error})"))?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("cannot create it ({error})")),
    }
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| format!("cannot inspect it ({error})"))?;
    if metadata.file_type().is_symlink() {
        return Err(String::from("it is a symbolic link"));
    }
    if !metadata.is_dir() {
        return Err(String::from("it is not a directory"));
    }
    // The effective user, which owns what this process creates (Linux uses the filesystem
    // uid, equal to the effective one since nothing here calls setfsuid). Not the owner of
    // `/proc/self`, which is root for a non-dumpable process (setuid, file capabilities).
    // SAFETY: geteuid has no preconditions, cannot fail and only reads process state.
    let user = unsafe { libc::geteuid() };
    if metadata.uid() != user {
        return Err(format!(
            "it is owned by uid {}, not the current user (uid {user})",
            metadata.uid()
        ));
    }
    if metadata.mode() & 0o022 != 0 {
        return Err(format!(
            "it is writable by other users (mode {:o})",
            metadata.mode() & 0o777
        ));
    }
    Ok(())
}

/// Creates `path` unless it exists. `%LOCALAPPDATA%` is private to the user by its ACL.
#[cfg(windows)]
fn create_private_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|error| format!("cannot create it ({error})"))?;
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| format!("cannot inspect it ({error})"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(String::from("it is not a plain directory"));
    }
    Ok(())
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

/// The application data path NGX logs into, as the platform's `wchar_t` string (UTF-32);
/// `None` when the path is not UTF-8.
#[cfg(not(windows))]
fn wide_path(path: &Path) -> Option<Vec<u32>> {
    Some(
        path.to_str()?
            .chars()
            .map(u32::from)
            .chain(std::iter::once(0))
            .collect(),
    )
}

/// What NGX's requirement queries are asked about: DLSS super resolution for this project.
/// Borrows `data` for the data path; the result must not outlive it.
fn discovery_info(data: &DataDir) -> sys::NVSDK_NGX_FeatureDiscoveryInfo {
    sys::NVSDK_NGX_FeatureDiscoveryInfo {
        SDKVersion: sys::NVSDK_NGX_Version_API,
        FeatureID: sys::NVSDK_NGX_Feature_SuperSampling,
        Identifier: sys::NVSDK_NGX_Application_Identifier {
            IdentifierType: sys::NVSDK_NGX_Application_Identifier_Type_Project_Id,
            v: sys::NVSDK_NGX_Application_Identifier_v {
                ProjectDesc: sys::NVSDK_NGX_ProjectIdDescription {
                    ProjectId: PROJECT_ID.as_ptr(),
                    EngineType: sys::NVSDK_NGX_ENGINE_TYPE_CUSTOM,
                    EngineVersion: ENGINE_VERSION.as_ptr(),
                },
            },
        },
        ApplicationDataPath: data.wide.as_ptr().cast(),
        FeatureInfo: ptr::null(),
    }
}

/// Copies the names out of an NGX-owned `VkExtensionProperties` array.
fn extension_names(
    result: sys::NVSDK_NGX_Result,
    count: u32,
    properties: *const sys::VkExtensionProperties,
    what: &str,
) -> Result<Vec<String>, Unavailable> {
    if !succeeded(result) {
        return Err(format!(
            "NGX could not report the {what} extensions DLSS needs ({})",
            result_name(result)
        ));
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    if properties.is_null() {
        return Err(format!(
            "NGX reported {count} {what} extensions for DLSS but no list"
        ));
    }
    // SAFETY: NGX succeeded and says `properties` points at `count` initialised
    // VkExtensionProperties it owns, valid until the next NGX call; they are copied now.
    let properties = unsafe { std::slice::from_raw_parts(properties, count as usize) };
    Ok(properties
        .iter()
        .map(|property| {
            let name = &property.extensionName;
            // The array is NUL-terminated within its 256 bytes (Vulkan's
            // VK_MAX_EXTENSION_NAME_SIZE); stop at the first NUL, or the end.
            let len = name
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(name.len());
            name[..len]
                .iter()
                .map(|&byte| char::from(byte.to_ne_bytes()[0]))
                .collect()
        })
        .collect())
}

/// The instance extensions DLSS needs, asked before `vkCreateInstance`. NGX answers without
/// being initialised and without loading the driver's NGX core (it reports its built-in list
/// until the core is loaded).
pub(crate) fn instance_extensions() -> Result<Vec<String>, Unavailable> {
    let data = data_dir()?;
    let info = discovery_info(data);
    let mut count = 0u32;
    let mut properties: *mut sys::VkExtensionProperties = ptr::null_mut();
    // SAFETY: `info` and the data path it points at outlive the call; the out pointers are
    // valid for it. NGX does not need to be initialised for this query.
    let result = unsafe {
        sys::NVSDK_NGX_VULKAN_GetFeatureInstanceExtensionRequirements(
            &raw const info,
            &raw mut count,
            &raw mut properties,
        )
    };
    extension_names(result, count, properties, "instance")
}

/// The device extensions DLSS needs on `physical`, asked before `vkCreateDevice`. This loads
/// the driver's NGX core, so callers ask only on NVIDIA's own driver.
pub(crate) fn device_extensions(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
) -> Result<Vec<String>, Unavailable> {
    let data = data_dir()?;
    let info = discovery_info(data);
    let mut count = 0u32;
    let mut properties: *mut sys::VkExtensionProperties = ptr::null_mut();
    // SAFETY: the instance is live and `physical` was enumerated from it; `info` and the data
    // path outlive the call; the out pointers are valid for it.
    let result = unsafe {
        sys::NVSDK_NGX_VULKAN_GetFeatureDeviceExtensionRequirements(
            handle(instance.handle().as_raw()),
            handle(physical.as_raw()),
            &raw const info,
            &raw mut count,
            &raw mut properties,
        )
    };
    extension_names(result, count, properties, "device")
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

/// Checks the driver, the path and NGX's extension requirements against what was enabled,
/// then initialises NGX on the device and checks that DLSS super resolution is available.
#[allow(clippy::too_many_lines)] // One ordered sequence of gates, each with its own message.
pub(crate) fn probe(context: &ProbeContext<'_>) -> Result<Box<dyn Backend>, Unavailable> {
    nvidia_driver(context.capabilities, "DLSS")?;
    if context.path != RenderPath::Tiered {
        return Err(String::from(
            "the device runs the compat path, which enables no vendor extensions",
        ));
    }
    let data = data_dir()?;
    // The requirements are asked again rather than carried from before creation: two driver
    // queries, once at start-up, and the check stays independent of how creation merged the
    // back ends' requests.
    require_enabled(
        "DLSS",
        "instance",
        &instance_extensions()?,
        context.instance_extensions,
        context.path,
    )?;
    require_enabled(
        "DLSS",
        "device",
        &device_extensions(context.instance, context.physical)?,
        context.device_extensions,
        context.path,
    )?;

    // NGX resolves Vulkan through the renderer's own loader, not whichever one it finds.
    let gipa: vk::PFN_vkGetInstanceProcAddr = context.entry.static_fn().get_instance_proc_addr;
    let gdpa: vk::PFN_vkGetDeviceProcAddr = context.instance.fp_v1_0().get_device_proc_addr;
    // SAFETY: the two types differ only in how the handle parameter is spelled: ash's
    // `vk::Instance` is a `#[repr(transparent)]` 64-bit handle and NGX's `VkInstance` a
    // pointer, the same size and ABI class on every target `dlss` builds for (checked at the
    // end of this module), and "system" is "C" on those targets (64-bit Linux and Windows).
    // A function pointer is never null, so `Some` is the matching `Option` layout.
    let gipa = unsafe {
        std::mem::transmute::<vk::PFN_vkGetInstanceProcAddr, sys::PFN_vkGetInstanceProcAddr>(gipa)
    };
    // SAFETY: as above, for `vk::Device` and NGX's `VkDevice`.
    let gdpa = unsafe {
        std::mem::transmute::<vk::PFN_vkGetDeviceProcAddr, sys::PFN_vkGetDeviceProcAddr>(gdpa)
    };
    let device = context.device.handle().as_raw();
    // SAFETY: the instance, physical device and device are the renderer's own, alive for the
    // call, and the proc-address functions belong to the loader that created them. The
    // strings are NUL-terminated and outlive it (NGX copies what it keeps). Null feature info
    // selects NGX's default search for feature binaries (beside the executable).
    let result = unsafe {
        sys::NVSDK_NGX_VULKAN_Init_with_ProjectID(
            PROJECT_ID.as_ptr(),
            sys::NVSDK_NGX_ENGINE_TYPE_CUSTOM,
            ENGINE_VERSION.as_ptr(),
            data.wide.as_ptr().cast(),
            handle(context.instance.handle().as_raw()),
            handle(context.physical.as_raw()),
            handle(device),
            gipa,
            gdpa,
            ptr::null(),
            sys::NVSDK_NGX_Version_API,
        )
    };
    if !succeeded(result) {
        return Err(format!(
            "NGX initialisation failed on {} ({}; logs in {})",
            device_label(context.capabilities),
            result_name(result),
            data.path.display()
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
        return Ok(Box::new(Dlss {
            session: Arc::new(session),
        }));
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
    let init = init_result.map(|code| sys::NVSDK_NGX_Result::from_ne_bytes(code.to_ne_bytes()));
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

/// NGX's handle types are the same size as ash's, so the handle conversions above and the
/// proc-address transmutes are layout-preserving.
const _: () = {
    assert!(size_of::<sys::VkInstance>() == size_of::<vk::Instance>());
    assert!(size_of::<sys::VkPhysicalDevice>() == size_of::<vk::PhysicalDevice>());
    assert!(size_of::<sys::VkDevice>() == size_of::<vk::Device>());
    assert!(
        size_of::<sys::PFN_vkGetInstanceProcAddr>() == size_of::<vk::PFN_vkGetInstanceProcAddr>()
    );
    assert!(size_of::<sys::PFN_vkGetDeviceProcAddr>() == size_of::<vk::PFN_vkGetDeviceProcAddr>());
    assert!(size_of::<c_char>() == 1);
};

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    use super::{create_private_dir, wide_path};

    #[test]
    fn the_data_directory_must_be_a_private_directory_of_this_user() {
        let root = std::env::temp_dir().join(format!("moba-ngx-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("test root");

        // Created fresh: a 0700 directory.
        let fresh = root.join("cache").join("ngx");
        create_private_dir(&fresh).expect("a new directory is accepted");
        let mode = std::fs::metadata(&fresh)
            .expect("created")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        // An existing private directory is accepted again.
        create_private_dir(&fresh).expect("an existing private directory is accepted");

        // A symlink planted where the directory goes is refused, even to a good directory.
        let planted = root.join("planted");
        symlink(&fresh, &planted).expect("symlink");
        assert_eq!(
            create_private_dir(&planted),
            Err(String::from("it is a symbolic link"))
        );

        // A directory others can write to is refused.
        let shared = root.join("shared");
        std::fs::create_dir(&shared).expect("shared");
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).expect("chmod");
        let reason = create_private_dir(&shared).expect_err("world-writable");
        assert!(
            reason.starts_with("it is writable by other users"),
            "{reason}"
        );

        // A file is refused.
        let file = root.join("file");
        std::fs::write(&file, b"").expect("file");
        assert_eq!(
            create_private_dir(&file),
            Err(String::from("it is not a directory"))
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_data_path_is_utf32_and_a_non_utf8_path_is_refused() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;
        assert_eq!(
            wide_path(std::path::Path::new("/tmp/é")),
            Some(vec![
                u32::from('/'),
                u32::from('t'),
                u32::from('m'),
                u32::from('p'),
                u32::from('/'),
                u32::from('é'),
                0
            ])
        );
        let invalid = std::path::Path::new(OsStr::from_bytes(b"/tmp/\xff"));
        assert_eq!(wide_path(invalid), None);
    }
}
