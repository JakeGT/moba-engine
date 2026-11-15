//! Tiered-path smoke tests (ADR-0020 gate 3, N0.5).
//!
//! Each test creates a headless device (no window or surface) admitted to
//! [`RenderPath::Tiered`], renders one small fixed frame, reads it back and compares every value
//! **exactly** with an expectation computed on the CPU:
//!
//! - [`bindless_smoke`]: a fullscreen triangle and a compute pass sample 211 bindless 2×2
//!   textures (and the fallbacks) through per-pixel, divergent handles and samplers
//!   (`nonuniformEXT`), read from a buffer device address, at a coordinate where every palette
//!   entry gives a different exact value; the compute pass also checks the comparison
//!   samplers on a depth texture and writes through a bindless storage image. It releases,
//!   retires and reuses handles (checking that a retired slot holds the magenta fallback
//!   again), and writes descriptors while the frame is pending (update after bind).
//! - [`rt_smoke`]: builds a BLAS and a TLAS and traces one ray per pixel with
//!   `GL_EXT_ray_query`, writing the closest hit's instance, custom index, primitive and `t` as
//!   integers.
//! - [`fsr3_smoke`] (N1b, ADR-0021 §4b and §4c): runs the FSR 3.1.6 port on synthetic inputs
//!   with a known ideal output. Being an upscaler it is compared within error bounds rather than
//!   exactly, and it must be bit-identical across two runs.
//! - [`gbuffer_smoke`] (N2a, ADR-0023 verification gate 1): 4096 fixed surfaces through the
//!   G-buffer encoding, in a GLSL fragment shader into the three targets' real formats and in
//!   a Slang compute shader, and the Slang decode of the written texels; every stored byte
//!   must equal the Rust twin's (`crate::gbuffer`), and every decoded value its decode.
//!
//! `moba-tools` runs them as the `bindless-smoke`, `rt-smoke`, `fsr3-smoke` and
//! `gbuffer-smoke` binaries. A CPU
//! device such as lavapipe needs `MOBA_RENDER_PATH=tiered`; any validation message fails a
//! run.

pub(crate) mod bindless;
#[cfg(feature = "fsr3")]
pub(crate) mod fsr3;
pub(crate) mod gbuffer;
pub(crate) mod rt;

use crate::caps::{PathChoice, RenderPath};
use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::instance::{InstanceContext, ValidationMode};

/// Mismatches quoted in a report; the rest are only counted.
const QUOTED_MISMATCHES: usize = 8;

/// The outcome of one tiered smoke test.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SmokeReport {
    /// `"bindless_smoke"`, `"rt_smoke"`, `"fsr3_smoke"` or `"gbuffer_smoke"`.
    pub name: &'static str,
    /// The device's name.
    pub device: String,
    /// The device's admission (`path=… max_tier=… missing=… admission=…`).
    pub admission: String,
    /// Whether the Khronos validation layer was enabled.
    pub validation_enabled: bool,
    /// Validation was turned off explicitly ([`ValidationMode::Off`]); only then may a run
    /// without the layer pass.
    pub validation_waived: bool,
    /// Values compared with the CPU expectation.
    pub checked: usize,
    /// Values that differed.
    pub mismatches: usize,
    /// The first few mismatches, described.
    pub first_mismatches: Vec<String>,
    /// Validation errors reported during the test.
    pub validation_errors: u64,
    /// Validation warnings reported during the test.
    pub validation_warnings: u64,
    /// What the test did, one fact per line (handles, descriptor writes, coverage).
    pub notes: Vec<String>,
}

impl SmokeReport {
    fn new(
        name: &'static str,
        device: &DeviceContext,
        validation_enabled: bool,
        validation: ValidationMode,
    ) -> Self {
        Self {
            name,
            device: device.name.clone(),
            admission: device.admission.log_fields(),
            validation_enabled,
            validation_waived: validation == ValidationMode::Off,
            checked: 0,
            mismatches: 0,
            first_mismatches: Vec::new(),
            validation_errors: 0,
            validation_warnings: 0,
            notes: Vec::new(),
        }
    }

    /// Compares one value.
    #[allow(clippy::needless_pass_by_value)] // Called with temporaries.
    fn check<T: PartialEq + core::fmt::Debug>(
        &mut self,
        what: impl FnOnce() -> String,
        actual: T,
        expected: T,
    ) {
        self.checked += 1;
        if actual != expected {
            self.mismatches += 1;
            if self.first_mismatches.len() < QUOTED_MISMATCHES {
                self.first_mismatches
                    .push(format!("{}: got {actual:?}, expected {expected:?}", what()));
            }
        }
    }

    /// Every value matched, validation ran and reported nothing. A run without the
    /// validation layer passes only when validation was turned off explicitly
    /// ([`ValidationMode::Off`], `MOBA_VULKAN_VALIDATION=off`), never because the layer was
    /// missing under [`ValidationMode::IfAvailable`].
    #[must_use]
    pub fn passed(&self) -> bool {
        self.checked > 0
            && (self.validation_enabled || self.validation_waived)
            && self.mismatches == 0
            && self.validation_errors == 0
            && self.validation_warnings == 0
    }
}

/// Runs `bindless_smoke` on a headless tiered device: a fullscreen triangle and a compute pass
/// sample bindless textures through divergent `nonuniformEXT` handles read from a buffer device
/// address and divergent palette samplers (the compute pass also compares a depth texture and
/// writes through a bindless storage image), after handles have been released, retired and
/// reused, and with descriptors written while the frame is pending; every texel of the 64×64
/// result is compared exactly with the CPU expectation, and a retired slot must read the
/// magenta fallback.
///
/// # Errors
/// No device admitted to the tiered path under `choice` (the message names each device and
/// what it lacks, or says to set `MOBA_RENDER_PATH=tiered` for a CPU device), a missing
/// validation layer with [`ValidationMode::Required`], or a Vulkan error. A wrong pixel is
/// not an error: it is counted in the report.
pub fn bindless_smoke(choice: PathChoice, validation: ValidationMode) -> RenderResult<SmokeReport> {
    run("bindless_smoke", choice, validation, bindless::run)
}

/// Runs `rt_smoke` on a headless tiered device: builds a BLAS and a TLAS and traces one ray per
/// pixel with `GL_EXT_ray_query`, comparing the closest hit's instance, custom index, primitive
/// and `t` for every pixel of the 64×64 result exactly with the CPU expectation.
///
/// # Errors
/// As [`bindless_smoke`].
pub fn rt_smoke(choice: PathChoice, validation: ValidationMode) -> RenderResult<SmokeReport> {
    run("rt_smoke", choice, validation, rt::run)
}

/// Runs `fsr3_smoke` on a headless tiered device: the FSR 3.1.6 upscaler on synthetic
/// inputs whose ideal output is known (a flat colour, a static and a moving analytic field,
/// sharpening, and a sweep of render sizes inside one context, ADR-0021 §4c), checking every
/// phase's error against its bound and that a second run on a fresh context is bit-identical.
///
/// # Errors
/// As [`bindless_smoke`], and also when the device cannot run FSR 3.1 or the build lacks the
/// `fsr3` feature.
pub fn fsr3_smoke(choice: PathChoice, validation: ValidationMode) -> RenderResult<SmokeReport> {
    #[cfg(feature = "fsr3")]
    {
        run("fsr3_smoke", choice, validation, fsr3::run)
    }
    #[cfg(not(feature = "fsr3"))]
    {
        let _ = (choice, validation);
        Err(RenderError::NoSuitableDevice(String::from(
            "fsr3_smoke needs moba-render's `fsr3` feature",
        )))
    }
}

/// Runs `gbuffer_smoke` on a headless tiered device (ADR-0023 verification gate 1): 4096
/// fixed surfaces through `encode_gbuffer` in a GLSL fragment shader (into the three G-buffer
/// targets' real formats) and in Slang, and through Slang's `decode_gbuffer` from the written
/// texels; every stored byte must equal the Rust twin's and every decoded value its decode.
///
/// # Errors
/// As [`bindless_smoke`].
pub fn gbuffer_smoke(choice: PathChoice, validation: ValidationMode) -> RenderResult<SmokeReport> {
    run("gbuffer_smoke", choice, validation, gbuffer::run)
}

fn run(
    name: &'static str,
    choice: PathChoice,
    validation: ValidationMode,
    test: fn(&Headless, &mut SmokeReport) -> RenderResult<()>,
) -> RenderResult<SmokeReport> {
    let errors = InstanceContext::validation_error_count();
    let warnings = InstanceContext::validation_warning_count();
    let mut report = {
        let gpu = Headless::new(choice, validation)?;
        let mut report = SmokeReport::new(
            name,
            &gpu.device,
            gpu.instance.validation_enabled(),
            validation,
        );
        test(&gpu, &mut report)?;
        report
        // The device and instance are destroyed here, so their messages are counted too.
    };
    report.validation_errors = InstanceContext::validation_error_count() - errors;
    report.validation_warnings = InstanceContext::validation_warning_count() - warnings;
    Ok(report)
}

/// `buffer`'s device address, for the smokes only.
fn address(buffer: &crate::resources::GpuBuffer) -> ash::vk::DeviceAddress {
    // SAFETY: the smokes create buffers only on a `Headless` device, and `Headless::new`
    // rejects any device that is not tiered: `bufferDeviceAddress` is enabled and the
    // allocator allocates with `DEVICE_ADDRESS`.
    unsafe { buffer.device_address() }
}

/// A device without a surface, admitted to the tiered path.
pub(crate) struct Headless {
    // Destroyed before the instance.
    pub(crate) device: DeviceContext,
    pub(crate) instance: InstanceContext,
}

impl Headless {
    fn new(choice: PathChoice, validation: ValidationMode) -> RenderResult<Self> {
        // The device enables `VK_KHR_swapchain`, which needs `VK_KHR_surface` on the instance.
        let instance = InstanceContext::new(&[c"VK_KHR_surface"], &[], validation)?;
        // The smokes take their choice from `MOBA_RENDER_PATH`, so `tiered` admits a CPU device
        // as that variable does (ADR-0020 amendment 2).
        let allow_cpu = choice == PathChoice::Tiered;
        let device = DeviceContext::new(&instance, None, false, choice, allow_cpu, false)?;
        if device.admission.path != RenderPath::Tiered {
            return Err(RenderError::NoSuitableDevice(format!(
                "{} runs {}; the tiered smoke tests need the tiered path \
                 (a CPU device such as lavapipe needs MOBA_RENDER_PATH=tiered)",
                device.name,
                device.admission.log_fields()
            )));
        }
        Ok(Self { device, instance })
    }
}

#[cfg(test)]
mod tests {
    use super::SmokeReport;

    fn report(validation_enabled: bool, validation_waived: bool) -> SmokeReport {
        SmokeReport {
            name: "test",
            device: String::new(),
            admission: String::new(),
            validation_enabled,
            validation_waived,
            checked: 1,
            mismatches: 0,
            first_mismatches: Vec::new(),
            validation_errors: 0,
            validation_warnings: 0,
            notes: Vec::new(),
        }
    }

    /// A clean run passes only when validation ran, or was turned off explicitly; a missing
    /// layer under `IfAvailable` is not a pass.
    #[test]
    fn a_run_without_validation_passes_only_when_validation_was_waived() {
        assert!(report(true, false).passed());
        assert!(report(false, true).passed());
        assert!(!report(false, false).passed());
        let mut warned = report(true, false);
        warned.validation_warnings = 1;
        assert!(!warned.passed());
    }
}
