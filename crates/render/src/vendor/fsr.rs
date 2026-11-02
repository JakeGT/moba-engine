//! AMD FidelityFX Super Resolution (ADR-0020 §6).
//!
//! - **FSR 3.1** (feature `fsr3`, on by default): the open build's upscaler. Its MIT shaders
//!   are ported into `third_party/fidelityfx/` with N1 (ADR-0020 Q9 and amendment 6); N0
//!   vendors nothing and registers no back end, so the upscaler stays on the built-in
//!   render-scale filter.
//! - **FSR 4** (feature `fsr4`, development-only, ADR-0020 Q6): the signed
//!   `amd_fidelityfx_loader_vk.dll` and `amd_fidelityfx_upscaler_vk.dll` from FidelityFX SDK
//!   2.4.0, opened at run time from beside the executable with `libloading` and kept loaded
//!   for the rest of the process. Windows only; AMD's own driver; RDNA 2 or newer, which the
//!   FidelityFX API checks when N1 creates the upscaling context. The N0 probe opens the
//!   loader and resolves the FidelityFX API entry points.
//!
//! Licence (FSR 4): the SDK's `license.md` makes binary-only redistribution without reverse
//! engineering the default and also lists the signed DLLs under MIT; until a legal reading
//! settles it, no distributed build enables `fsr4` (`THIRD_PARTY.md`).

#[cfg(feature = "fsr4")]
pub(crate) use fsr4::probe as probe_fsr4;

#[cfg(feature = "fsr4")]
mod fsr4 {
    use super::super::{
        Backend, Loaded, ProbeContext, Unavailable, amd_driver, beside_executable,
        load_beside_executable,
    };
    use crate::caps::RenderPath;

    #[allow(
        non_camel_case_types,
        non_snake_case,
        non_upper_case_globals,
        dead_code,
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
        include!(concat!(env!("OUT_DIR"), "/ffx.rs"));
    }

    const SDK_VERSION: &str = env!("MOBA_SDK_FIDELITYFX_VERSION");

    /// The FidelityFX API loader and the upscaler it loads.
    const LOADER: &str = if cfg!(windows) {
        "amd_fidelityfx_loader_vk.dll"
    } else {
        "libamd_fidelityfx_loader_vk.so"
    };
    const UPSCALER: &str = if cfg!(windows) {
        "amd_fidelityfx_upscaler_vk.dll"
    } else {
        "libamd_fidelityfx_upscaler_vk.so"
    };

    /// The FSR 4 back end: the process-lifetime FidelityFX API loader.
    struct Fsr4 {
        _api: &'static sys::FfxApi,
    }

    impl Backend for Fsr4 {
        fn name(&self) -> &'static str {
            "fsr4"
        }

        fn detail(&self) -> String {
            format!("FidelityFX SDK {SDK_VERSION}")
        }
    }

    /// Needs an AMD GPU on AMD's driver (checked first, so no AMD library is loaded
    /// elsewhere) and the tiered path, as DLSS and XeSS do (Compat changes only where shared
    /// code moves, ADR-0020 §2c), then the loader and the upscaler beside the executable. The
    /// FidelityFX API has no Vulkan extension-requirement query, so FSR 4 asks for nothing
    /// before creation.
    pub(crate) fn probe(context: &ProbeContext<'_>) -> Result<Box<dyn Backend>, Unavailable> {
        amd_driver(context.capabilities)?;
        if context.path != RenderPath::Tiered {
            return Err(String::from(
                "the device runs the compat path, and the vendor upscalers are tiered-only",
            ));
        }
        api().map(|api| Box::new(Fsr4 { _api: api }) as Box<dyn Backend>)
    }

    /// The loader, opened on first use and never unloaded (see [`super::super::Loaded`]).
    fn api() -> Result<&'static sys::FfxApi, Unavailable> {
        static API: Loaded<sys::FfxApi> = Loaded::new();
        API.get_or_init(|| {
            let windows_only = |reason: String| {
                if cfg!(windows) {
                    reason
                } else {
                    format!("{reason} (FidelityFX SDK {SDK_VERSION} ships Windows binaries only)")
                }
            };
            if !beside_executable(UPSCALER)?.is_file() {
                return Err(windows_only(format!(
                    "{UPSCALER} not found beside the executable"
                )));
            }
            let library = load_beside_executable(LOADER).map_err(windows_only)?;
            // SAFETY: the library is the FidelityFX API loader; `from_library` resolves every
            // declared entry point by name and fails if one is missing, so each stored pointer
            // has the declared signature.
            unsafe { sys::FfxApi::from_library(library) }
                .map_err(|error| format!("{LOADER} lacks a FidelityFX API entry point: {error}"))
        })
        .as_ref()
        .map_err(Clone::clone)
    }
}
