//! SHaRC, the spatial hash radiance cache (feature `sharc`, ADR-0020 §6).
//!
//! SHaRC is HLSL/GLSL headers only (no host API to bind); `build.rs` checks the fetched
//! headers are present for the phase that compiles them. There is no run-time binary; the
//! probe checks the device runs the tiered path with ray queries.
//!
//! Licence: NVIDIA RTX SDKs License (SHARC copy): object code only (compiled shaders),
//! attribution in the credits, interoperable with NVIDIA hardware; not NVIDIA-only.

use super::{Backend, ProbeContext, Unavailable, ray_traced_lighting};

const SDK_VERSION: &str = env!("MOBA_SDK_SHARC_VERSION");

/// The SHaRC back end (shaders only; nothing to own in N0).
struct Sharc;

impl Backend for Sharc {
    fn name(&self) -> &'static str {
        "sharc"
    }

    fn detail(&self) -> String {
        format!("SHaRC {SDK_VERSION}")
    }
}

/// Needs the tiered path with ray queries.
pub(crate) fn probe(context: &ProbeContext<'_>) -> Result<Box<dyn Backend>, Unavailable> {
    ray_traced_lighting(context)?;
    Ok(Box::new(Sharc))
}
