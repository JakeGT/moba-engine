//! Streamline, the Windows path for DLSS frame generation (feature `streamline`, implied by
//! `dlss-fg`; ADR-0020 §6 and Q8).
//!
//! The interposer (`sl.interposer.dll`) and its plug-ins are opened at run time from beside
//! the executable. Streamline 2.14.16 ships Windows binaries only. The N0 probe opens the
//! interposer and resolves `slInit` without calling it (initialising Streamline hooks Vulkan,
//! which belongs to N9).
//!
//! Licence: Streamline is MIT; `sl_nvperf.h` and `sl.nvperf.dll` are under the Nsight Perf
//! SDK License and are not shipped; the DLSS-G binaries it loads fall under the NVIDIA RTX
//! SDKs License (`THIRD_PARTY.md`).

use super::{Backend, ProbeContext, Unavailable, device_label, load_beside_executable, pci};

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unreachable_pub,
    missing_docs,
    missing_debug_implementations,
    clippy::all,
    clippy::pedantic
)]
mod sys {
    include!(concat!(env!("OUT_DIR"), "/streamline.rs"));
}

/// The interposer the probe opens.
const INTERPOSER: &str = if cfg!(windows) {
    "sl.interposer.dll"
} else {
    "libsl.interposer.so"
};

/// The Streamline back end: the opened interposer.
struct Streamline {
    _library: libloading::Library,
}

impl Backend for Streamline {
    fn name(&self) -> &'static str {
        "streamline"
    }

    fn detail(&self) -> String {
        format!(
            "Streamline SDK {}.{}.{}",
            sys::SL_VERSION_MAJOR,
            sys::SL_VERSION_MINOR,
            sys::SL_VERSION_PATCH
        )
    }
}

/// Needs an NVIDIA GPU (DLSS-G), then the interposer beside the executable.
pub(crate) fn probe(context: &ProbeContext<'_>) -> Result<Box<dyn Backend>, Unavailable> {
    if context.capabilities.vendor_id != pci::NVIDIA {
        return Err(format!(
            "DLSS frame generation needs an NVIDIA RTX GPU, and the device is {}",
            device_label(context.capabilities)
        ));
    }
    let library = load_beside_executable(INTERPOSER).map_err(|reason| {
        if cfg!(windows) {
            reason
        } else {
            format!(
                "{reason} (Streamline {}.{}.{} ships Windows binaries only)",
                sys::SL_VERSION_MAJOR,
                sys::SL_VERSION_MINOR,
                sys::SL_VERSION_PATCH
            )
        }
    })?;
    // SAFETY: only the symbol's presence is checked; the pointer is never called or kept.
    let found = unsafe { library.get::<unsafe extern "C" fn()>(b"slInit\0") }.is_ok();
    if !found {
        return Err(format!("{INTERPOSER} does not export slInit"));
    }
    Ok(Box::new(Streamline { _library: library }))
}
