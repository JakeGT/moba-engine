//! Streamline, the Windows path for DLSS frame generation (feature `streamline`, implied by
//! `dlss-fg`; ADR-0020 §6 and Q8).
//!
//! Streamline 2.14.16 ships Windows binaries only. Its interposer (`sl.interposer.dll`) must be
//! loaded, and `slInit` called, **before `vkCreateInstance`**: it interposes on the Vulkan
//! loader and hooks instance and device creation. N0 creates the instance without it, so the
//! N0 probe only reports whether the interposer is installed beside the executable; it never
//! loads it (loading it after the instance exists is exactly what Streamline forbids) and
//! never selects frame generation. The interposer and `slInit` come with frame generation in
//! N9 (ADR-0020 amendment 6).
//!
//! Licence: Streamline is MIT; `sl_nvperf.h` and `sl.nvperf.dll` are under the Nsight Perf
//! SDK License and are not shipped; the DLSS-G binaries it loads fall under the NVIDIA RTX
//! SDKs License (`THIRD_PARTY.md`).

use super::{Backend, ProbeContext, Unavailable, beside_executable, nvidia_driver};

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

/// The interposer the probe looks for.
const INTERPOSER: &str = if cfg!(windows) {
    "sl.interposer.dll"
} else {
    "libsl.interposer.so"
};

/// Needs an NVIDIA GPU on NVIDIA's driver (DLSS-G), then reports whether the interposer is
/// installed. Always unavailable in N0: Streamline is initialised before the instance from N9.
pub(crate) fn probe(context: &ProbeContext<'_>) -> Result<Box<dyn Backend>, Unavailable> {
    nvidia_driver(context.capabilities, "DLSS frame generation")?;
    let version = format!(
        "{}.{}.{}",
        sys::SL_VERSION_MAJOR,
        sys::SL_VERSION_MINOR,
        sys::SL_VERSION_PATCH
    );
    let path = beside_executable(INTERPOSER)?;
    if !path.is_file() {
        return Err(if cfg!(windows) {
            format!("{} not found", path.display())
        } else {
            format!(
                "{} not found (Streamline {version} ships Windows binaries only)",
                path.display()
            )
        });
    }
    Err(format!(
        "{} found, but Streamline {version} must be initialised before the Vulkan instance, \
         which comes with frame generation in N9",
        path.display()
    ))
}
