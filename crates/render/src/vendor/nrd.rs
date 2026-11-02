//! NVIDIA Real-time Denoisers (feature `nrd`, ADR-0020 §6).
//!
//! NRD is built from the fetched source by `build.rs` (CMake, SPIR-V embeds only) and linked
//! statically, so there is no run-time binary to miss. The N0 probe asks the linked library
//! for its descriptor and checks it against the headers it was built from; the device must run
//! the tiered path with ray queries, since NRD denoises ray-traced signals (N2 onwards).
//!
//! Licence: NVIDIA RTX SDKs License (NRD copy): object code only, attribution in the credits,
//! interoperable with NVIDIA hardware; not NVIDIA-only (THIRD_PARTY.md).

use super::{Backend, ProbeContext, Unavailable, ray_traced_lighting};

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
    include!(concat!(env!("OUT_DIR"), "/nrd.rs"));
}

/// The NRD back end: the linked library's version.
struct Nrd {
    version: (u8, u8, u8),
}

impl Backend for Nrd {
    fn name(&self) -> &'static str {
        "nrd"
    }

    fn detail(&self) -> String {
        let (major, minor, build) = self.version;
        format!("NRD {major}.{minor}.{build}")
    }
}

/// Reads the linked library's descriptor, then checks the device.
pub(crate) fn probe(context: &ProbeContext<'_>) -> Result<Box<dyn Backend>, Unavailable> {
    // SAFETY: GetLibraryDesc takes no arguments and returns a pointer to a static descriptor
    // owned by the library (or null), valid for the life of the process.
    let desc = unsafe { sys::GetLibraryDesc() };
    // SAFETY: a non-null result points at NRD's static `LibraryDesc`.
    let desc = unsafe { desc.as_ref() }.ok_or("the NRD library returned no descriptor")?;
    let version = (desc.versionMajor, desc.versionMinor, desc.versionBuild);
    let expected = (
        sys::NRD_VERSION_MAJOR as u8,
        sys::NRD_VERSION_MINOR as u8,
        sys::NRD_VERSION_BUILD as u8,
    );
    if version != expected {
        return Err(format!(
            "the linked NRD is {version:?} but its headers are {expected:?}"
        ));
    }
    ray_traced_lighting(context)?;
    Ok(Box::new(Nrd { version }))
}
