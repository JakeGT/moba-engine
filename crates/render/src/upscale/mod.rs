//! The temporal upscaler interface (ADR-0021 §4): one trait over the in-house TAAU, FSR 3.1,
//! DLSS, FSR 4 and XeSS, chosen at renderer creation. Vendor code stays behind
//! `crate::vendor` (ADR-0020 §6a); call sites never `cfg`.
//!
//! **Inputs** ([`UpscaleInputs`]) are all at render resolution, in the render sub-rectangle
//! at the top-left of their allocation: the jittered linear HDR colour, depth (`D32_SFLOAT`,
//! not inverted, finite far plane), `motion` (`R16G16_SFLOAT`, render pixels from the current
//! sample to its previous position, x right and y down, without jitter: every back end takes
//! it with a scale of (1, 1)), `reactive` and `tc_mask` (`R8_UNORM`), a 1×1 `R32_SFLOAT`
//! linear exposure multiplier, and the frame's scalars. The graph moves each input to
//! `SHADER_READ_ONLY_OPTIMAL` and the output ([`UpscaleOutput`], `RGBA16F` at output
//! resolution) to `GENERAL` before the `temporal` pass; a back end records only its own work
//! (its dispatches and the barriers of the resources it owns, such as its history).
//!
//! **Contexts** are created with the renderer at the maximum render size of the
//! dynamic-resolution range. A swapchain resize only re-sizes their images
//! ([`TemporalUpscaler::resize`]); pipelines are built at creation and on a settings change,
//! never mid-match (`locked_pipelines=0`).

pub(crate) mod exposure;
pub(crate) mod frame;
pub(crate) mod taau;

use ash::vk;

pub(crate) use crate::post::{UpscaleMode, UpscalerKind};
pub(crate) use crate::temporal::Extent;

/// One image the upscaler reads or writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UpscaleImage {
    pub(crate) image: vk::Image,
    pub(crate) view: vk::ImageView,
    pub(crate) format: vk::Format,
    /// The image's allocation (the valid region is [`UpscaleInputs::render_size`] for the
    /// inputs, the whole image for the output).
    pub(crate) extent: Extent,
}

/// This frame's inputs (ADR-0021 §4a), all at render resolution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct UpscaleInputs {
    /// The jittered linear HDR colour.
    pub(crate) color: UpscaleImage,
    /// Depth, `D32_SFLOAT`, 0 near, 1 far.
    pub(crate) depth: UpscaleImage,
    /// Motion vectors, `R16G16_SFLOAT`, render pixels to the previous position.
    pub(crate) motion: UpscaleImage,
    /// FSR's reactive mask (`R8_UNORM`): particles and distortion.
    pub(crate) reactive: UpscaleImage,
    /// FSR's transparency-and-composition mask (`R8_UNORM`): water and animated-UV
    /// materials.
    pub(crate) transparency: UpscaleImage,
    /// 1×1 `R32_SFLOAT` linear exposure multiplier (`exposure_linear`, ADR-0021 §4a).
    pub(crate) exposure: UpscaleImage,
    /// The jitter in render pixels, x right and y down.
    pub(crate) jitter: [f32; 2],
    /// The render size this frame (the valid sub-rectangle of every input).
    pub(crate) render_size: Extent,
    /// Frame time in milliseconds.
    pub(crate) dt_ms: f32,
    /// The camera's near and far planes and vertical field of view (radians).
    pub(crate) near: f32,
    pub(crate) far: f32,
    pub(crate) fov_y: f32,
    /// Start from the current frame: no history.
    pub(crate) reset: bool,
    /// The shared sharpening amount, `0..=1`.
    pub(crate) sharpness: f32,
    /// The frame-in-flight slot being recorded (`< FRAMES_IN_FLIGHT`). The caller waited on
    /// this slot's previous submission, so per-slot state (descriptor sets, constants) is
    /// free to rewrite, also after recorded frames were dropped unsubmitted.
    pub(crate) frame_slot: usize,
}

/// The upscaler's output: `hdr_out`, `RGBA16F` storage at output resolution, in `GENERAL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UpscaleOutput {
    pub(crate) image: UpscaleImage,
}

/// One temporal upscaler back end (ADR-0021 §4a). Exactly this shape: FSR 3.1 (N1b) and the
/// vendor back ends (N1c) implement it too, so a change is an amendment to the ADR.
pub(crate) trait TemporalUpscaler {
    /// Which back end this is.
    fn kind(&self) -> UpscalerKind; // Taau, Fsr3, Dlss, Fsr4, Xess
    /// The smallest and largest render size this back end accepts for `output` in `mode`.
    fn render_size_range(&self, output: Extent, mode: UpscaleMode) -> (Extent, Extent);
    /// Jitter phases for this frame's sizes.
    fn jitter_phases(&self, render: Extent, output: Extent) -> u32;
    /// Whether the back end sharpens itself (otherwise the renderer's `sharpen` pass runs).
    fn has_sharpening(&self) -> bool;
    /// Records the back end's work for this frame into `cmd`.
    ///
    /// # Safety
    ///
    /// The same contract as FSR 3.1's `Fsr3::record` (ADR-0021 amendment 2, item 17):
    /// - `cmd` is a command buffer of the back end's device in the recording state.
    /// - **Every command buffer recorded here is submitted exactly once, in `record` order, on
    ///   one queue.** A back end advances its own state when it records (history parity,
    ///   first-use layout transitions) and orders each frame only against earlier commands
    ///   on the same queue (a history reset's re-initialisation waits for every earlier
    ///   compute write and read of its images, amendment 2 item 28). A caller that
    ///   drops a recorded command buffer unsubmitted (a failure between recording and
    ///   submission) calls [`Self::reset_history`] before the next `record`, which makes that
    ///   record a first frame again, or treats the failure as fatal and drops the back end.
    /// - **At most `FRAMES_IN_FLIGHT` (2) records are pending, and `inputs.frame_slot`'s last
    ///   submission has finished.** Per-frame state (descriptor sets, constants) is keyed by
    ///   the slot, not by a ring of records, so frames dropped unsubmitted cannot make a
    ///   rewrite land on a set a pending frame reads.
    /// - Every image and view in `inputs` and `out` stays alive until this command buffer has
    ///   finished executing, in the layouts the module documentation states.
    unsafe fn record(
        &mut self,
        cmd: vk::CommandBuffer,
        inputs: &UpscaleInputs,
        out: &UpscaleOutput,
    );
    /// Forgets every history: the next frame starts from its current sample, as a first
    /// frame (its own images are re-initialised, whatever was or was not submitted before).
    fn reset_history(&mut self);
    /// Re-sizes the back end's own images for a new `output` and dynamic-resolution ceiling
    /// `max_render` (a swapchain resize) without building pipelines (`locked_pipelines=0`,
    /// ADR-0021 amendment 2, item 18). Forgets every history.
    ///
    /// # Safety
    ///
    /// The device is idle: no command buffer recorded by [`Self::record`] is pending. The
    /// back end drops its old images and rewrites its descriptor sets here, which a pending
    /// frame may still use (ADR-0021 amendment 4).
    unsafe fn resize(
        &mut self,
        device: &crate::device::DeviceContext,
        output: Extent,
        max_render: Extent,
    ) -> crate::error::RenderResult<()>;
    /// Device memory the back end owns (its histories and internal targets).
    fn vram_bytes(&self) -> u64;
    /// Debug builds' shader hot reload (`MOBA_SHADER_DIR`): rebuilds the back end's pipelines
    /// from `shaders`, replacing them only on success. The device is idle. A back end without
    /// shaders of ours (the vendor SDKs) keeps the default, which does nothing.
    fn reload_shaders(
        &mut self,
        _device: &crate::device::DeviceContext,
        _shaders: &crate::shaders::ShaderSource,
    ) -> crate::error::RenderResult<()> {
        Ok(())
    }
}

/// The render-size range for `mode` at `output` with a dynamic-resolution floor of `floor`
/// (a render scale): the mode's size and the floor's, both at least 1×1 and with the output's
/// aspect ratio (the controller scales both axes together, ADR-0021 §4c). TAAU's rule; vendor
/// back ends clamp it to what their SDK reports.
///
/// **Rounding is deliberate** (ADR-0021 amendment 2, item 19): sizes round to nearest, the
/// same rule as the dynamic-resolution controller (`dynres::render_extent`), so TAAU's
/// ceiling is exactly a size the controller can produce. Quality at 2560 is 1707 here; FSR
/// 3.1's SDK truncates (1706). A back end with an SDK rule reports its own range, and the
/// controller clamps to it.
pub(crate) fn scaled_range(output: Extent, mode: UpscaleMode, floor: f32) -> (Extent, Extent) {
    let size = |scale: f32| crate::dynres::render_extent(output, output, scale);
    let ceiling = mode.scale();
    (size(floor.min(ceiling)), size(ceiling))
}

/// `auto`'s back end in the open build, after the vendor back ends (ADR-0021 §4b and Q1).
/// FSR 3.1 takes the slot only once its port passes the readability gates and costs within
/// 0.2 ms of TAAU on the RTX 5070 Ti; the Q1 measurement is in perf/0033 (amendment 4).
pub(crate) const AUTO_OPEN_BUILD: UpscalerKind = UpscalerKind::Taau;

/// What `MOBA_UPSCALER` asked for and what to try: `auto` takes the first back end compiled in
/// and supported (DLSS, FSR 4, XeSS, then [`AUTO_OPEN_BUILD`]; ADR-0021 §4b and Q1), and a
/// named back end that is not compiled in or not wired yet falls back to TAAU with one log
/// line. FSR 3.1 is wired (`fsr3`, N1b); the vendor back ends arrive with N1c. Whether the
/// chosen back end can run on this device is [`create`]'s question.
#[must_use]
pub(crate) fn resolve(requested: Option<UpscalerKind>) -> UpscalerKind {
    let fsr3 = cfg!(feature = "fsr3");
    match requested {
        None if AUTO_OPEN_BUILD == UpscalerKind::Fsr3 && !fsr3 => UpscalerKind::Taau,
        None => AUTO_OPEN_BUILD,
        Some(UpscalerKind::Taau) => UpscalerKind::Taau,
        Some(UpscalerKind::Fsr3) if fsr3 => UpscalerKind::Fsr3,
        Some(other) => {
            let reason = match other {
                UpscalerKind::Fsr3 => "the `fsr3` feature is not compiled in",
                _ => "the vendor back ends arrive with N1c",
            };
            eprintln!(
                "[render] upscaler {} is not available in this build ({reason}); using taau",
                other.name()
            );
            UpscalerKind::Taau
        }
    }
}

/// Creates the back end `requested` resolves to ([`resolve`]) for `output` with render sizes up
/// to `max_render`. One that cannot run on this device (FSR 3.1 without its format features or
/// quad subgroups, ADR-0021 amendment 1) falls back to TAAU with one log line (§4b): the device
/// is never rejected. Only TAAU's resources are allocated when TAAU runs, and only the other
/// back end's when it does (amendment 1: the two never hold memory at once).
///
/// # Errors
/// TAAU itself cannot be created (a Vulkan or allocation error).
pub(crate) fn create(
    device: &crate::device::DeviceContext,
    shaders: &crate::shaders::ShaderSource,
    requested: Option<UpscalerKind>,
    output: Extent,
    max_render: Extent,
    debug: taau::TaauDebugView,
) -> crate::error::RenderResult<Box<dyn TemporalUpscaler>> {
    let kind = resolve(requested);
    if kind == UpscalerKind::Fsr3 {
        match new_fsr3(device, output, max_render) {
            Ok(fsr3) => {
                if debug != taau::TaauDebugView::None {
                    eprintln!(
                        "[render] MOBA_DEBUG_VIEW {debug:?} is drawn by taau only; fsr3 shows \
                         the frame"
                    );
                }
                return Ok(fsr3);
            }
            Err(error) => {
                eprintln!("[render] upscaler fsr3 cannot run here ({error}); using taau");
            }
        }
    }
    Ok(Box::new(taau::Taau::new(
        device, shaders, output, max_render, debug,
    )?))
}

/// FSR 3.1's context for `output` and render sizes up to `max_render` (at least 1×1, at most
/// the output).
#[cfg(feature = "fsr3")]
fn new_fsr3(
    device: &crate::device::DeviceContext,
    output: Extent,
    max_render: Extent,
) -> crate::error::RenderResult<Box<dyn TemporalUpscaler>> {
    let output = Extent {
        width: output.width.max(1),
        height: output.height.max(1),
    };
    let max_render = Extent {
        width: max_render.width.clamp(1, output.width),
        height: max_render.height.clamp(1, output.height),
    };
    Ok(Box::new(crate::fsr3::Fsr3::new(
        device, max_render, output,
    )?))
}

/// Never reached: [`resolve`] picks FSR 3.1 only with the `fsr3` feature.
#[cfg(not(feature = "fsr3"))]
fn new_fsr3(
    _device: &crate::device::DeviceContext,
    _output: Extent,
    _max_render: Extent,
) -> crate::error::RenderResult<Box<dyn TemporalUpscaler>> {
    Err(crate::error::RenderError::NoSuitableDevice(String::from(
        "the `fsr3` feature is not compiled in",
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TAAU rounds to nearest (amendment 2, item 19); FSR 3.1's SDK truncates.
    #[test]
    fn taau_mode_sizes_round_to_nearest() {
        let output = Extent {
            width: 2560,
            height: 1440,
        };
        let (_, quality) = scaled_range(output, UpscaleMode::Quality, 0.5);
        assert_eq!((quality.width, quality.height), (1707, 960));
        let (floor, _) = scaled_range(output, UpscaleMode::Quality, 0.5);
        assert_eq!((floor.width, floor.height), (1280, 720));
    }

    #[test]
    fn modes_have_one_naming_scheme_by_ratio() {
        let names: Vec<_> = UpscaleMode::ALL.iter().map(|mode| mode.name()).collect();
        assert_eq!(
            names,
            [
                "native",
                "quality",
                "balanced",
                "performance",
                "ultra-performance"
            ]
        );
        for &mode in UpscaleMode::ALL {
            assert_eq!(
                UpscaleMode::from_name(&mode.name().to_uppercase()),
                Some(mode)
            );
        }
        let ratios: Vec<_> = UpscaleMode::ALL.iter().map(|mode| mode.ratio()).collect();
        assert_eq!(ratios, [1.0, 1.5, 1.7, 2.0, 3.0]);
        assert_eq!(UpscalerKind::from_name(" DLSS "), Some(UpscalerKind::Dlss));
        assert_eq!(UpscalerKind::from_name("auto"), None);
    }

    #[test]
    fn tier_defaults_follow_the_adr() {
        use crate::QualityTier::{High, Low, Medium, Overdrive, Ultra};
        for tier in [Low, Medium, High] {
            assert_eq!(UpscaleMode::for_tier(tier, 1440), UpscaleMode::Quality);
        }
        assert_eq!(UpscaleMode::for_tier(Ultra, 1440), UpscaleMode::Quality);
        assert_eq!(UpscaleMode::for_tier(Ultra, 2160), UpscaleMode::Performance);
        assert_eq!(
            UpscaleMode::for_tier(Overdrive, 1440),
            UpscaleMode::Performance
        );
    }

    #[test]
    fn ranges_keep_the_aspect_and_the_floor() {
        let output = Extent {
            width: 2560,
            height: 1440,
        };
        let (low, high) = scaled_range(output, UpscaleMode::Quality, 0.5);
        assert_eq!(
            (high.width, high.height, low.width, low.height),
            (1707, 960, 1280, 720)
        );
        let (low, high) = scaled_range(output, UpscaleMode::Performance, 0.67);
        assert_eq!(
            (low, high),
            (high, high),
            "the floor never exceeds the ceiling"
        );
    }

    #[test]
    fn choices_resolve_to_what_this_build_has() {
        let fsr3 = if cfg!(feature = "fsr3") {
            UpscalerKind::Fsr3
        } else {
            UpscalerKind::Taau
        };
        let auto = if AUTO_OPEN_BUILD == UpscalerKind::Fsr3 {
            fsr3
        } else {
            UpscalerKind::Taau
        };
        assert_eq!(resolve(None), auto);
        assert_eq!(resolve(Some(UpscalerKind::Taau)), UpscalerKind::Taau);
        assert_eq!(resolve(Some(UpscalerKind::Fsr3)), fsr3);
        // The vendor back ends are not wired until N1c.
        for kind in [UpscalerKind::Dlss, UpscalerKind::Fsr4, UpscalerKind::Xess] {
            assert_eq!(resolve(Some(kind)), UpscalerKind::Taau);
        }
    }
}
