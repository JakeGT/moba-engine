//! RTXDI, ReSTIR many-light direct lighting (feature `rtxdi`, ADR-0020 §6).
//!
//! RTXDI-Library is a shader library (HLSL, compiled through `slangc` from N2 on) whose
//! parameter structures are shared with the host: `build.rs` generates those from the fetched
//! headers. There is no run-time binary; the probe checks the device runs the tiered path with
//! ray queries.
//!
//! Licence: NVIDIA RTX SDKs License (RTXDI-Library copy): object code only (compiled shaders),
//! attribution in the credits, interoperable with NVIDIA hardware; not NVIDIA-only.

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
    clippy::pedantic
)]
mod sys {
    include!(concat!(env!("OUT_DIR"), "/rtxdi.rs"));
}

const SDK_VERSION: &str = env!("MOBA_SDK_RTXDI_VERSION");

/// The RTXDI back end (shaders only; nothing to own in N0).
struct Rtxdi;

impl Backend for Rtxdi {
    fn name(&self) -> &'static str {
        "rtxdi"
    }

    fn detail(&self) -> String {
        format!(
            "RTXDI {SDK_VERSION}, ReSTIR DI parameters {} bytes, reservoir block {}",
            size_of::<sys::RTXDI_Parameters>(),
            sys::RTXDI_RESERVOIR_BLOCK_SIZE
        )
    }
}

/// Needs the tiered path with ray queries.
pub(crate) fn probe(context: &ProbeContext<'_>) -> Result<Box<dyn Backend>, Unavailable> {
    ray_traced_lighting(context)?;
    Ok(Box::new(Rtxdi))
}
