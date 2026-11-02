//! Intel XeSS super resolution (feature `xess`, ADR-0020 §6).
//!
//! The library is opened at run time from beside the executable with `libloading`, so a
//! build with the feature still starts, and falls back, when it is absent. XeSS 3.0.2 ships
//! Windows binaries only (`libxess.dll`); on other platforms the probe looks for the
//! equivalent shared object and reports it missing. The N0 probe opens the library, resolves
//! `xessGetVersion` and calls it; N1 creates the Vulkan context.
//!
//! Licence: Intel Simplified Software License: redistribute the binaries unmodified, with the
//! notice; no reverse engineering, including at run time (`THIRD_PARTY.md`).

use super::{Backend, ProbeContext, Unavailable, load_beside_executable};

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
    include!(concat!(env!("OUT_DIR"), "/xess.rs"));
}

const SDK_VERSION: &str = env!("MOBA_SDK_XESS_VERSION");

/// The library file the probe opens.
const LIBRARY: &str = if cfg!(windows) {
    "libxess.dll"
} else {
    "libxess.so"
};

/// The XeSS back end: the opened library and the version it reported.
struct Xess {
    _api: sys::XessApi,
    version: String,
}

impl Backend for Xess {
    fn name(&self) -> &'static str {
        "xess"
    }

    fn detail(&self) -> String {
        format!("XeSS SDK {SDK_VERSION}, library {}", self.version)
    }
}

/// Opens the XeSS library and asks it for its version.
pub(crate) fn probe(_context: &ProbeContext<'_>) -> Result<Box<dyn Backend>, Unavailable> {
    let library = load_beside_executable(LIBRARY).map_err(|reason| {
        if cfg!(windows) {
            reason
        } else {
            format!("{reason} (XeSS {SDK_VERSION} ships Windows binaries only)")
        }
    })?;
    // SAFETY: the library is XeSS's; `from_library` resolves every declared entry point by
    // name and fails if one is missing, so each stored pointer has the declared signature.
    let api = unsafe { sys::XessApi::from_library(library) }
        .map_err(|error| format!("{LIBRARY} lacks xessGetVersion: {error}"))?;
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
    Ok(Box::new(Xess {
        _api: api,
        version: format!("{}.{}.{}", version.major, version.minor, version.patch),
    }))
}
