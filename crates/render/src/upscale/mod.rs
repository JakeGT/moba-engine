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
//! dynamic-resolution range and re-created with it on a resize or a settings change, never
//! mid-match (`locked_pipelines=0`).

pub(crate) mod exposure;
pub(crate) mod frame;
pub(crate) mod taau;

use ash::vk;

pub use crate::post::{UpscaleMode, UpscalerKind};
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
    fn record(&mut self, cmd: vk::CommandBuffer, inputs: &UpscaleInputs, out: &UpscaleOutput);
    /// Forgets every history: the next frame starts from its current sample.
    fn reset_history(&mut self);
    /// Device memory the back end owns (its histories and internal targets).
    fn vram_bytes(&self) -> u64;
}

/// The render-size range for `mode` at `output` with a dynamic-resolution floor of `floor`
/// (a render scale): the mode's size and the floor's, both at least 1×1 and with the output's
/// aspect ratio (the controller scales both axes together, ADR-0021 §4c). TAAU's rule; vendor
/// back ends clamp it to what their SDK reports.
#[must_use]
pub(crate) fn scaled_range(output: Extent, mode: UpscaleMode, floor: f32) -> (Extent, Extent) {
    let size = |scale: f32| crate::dynres::render_extent(output, output, scale);
    let ceiling = mode.scale();
    (size(floor.min(ceiling)), size(ceiling))
}

/// What `MOBA_UPSCALER` asked for and what runs: `auto` takes the first back end compiled in
/// and supported (DLSS, FSR 4, XeSS, then TAAU; ADR-0021 §4b and Q1), and a named back end that
/// is not available falls back to TAAU with one log line. No vendor back end is wired into the
/// frame yet (N1b, N1c), so today every choice resolves to TAAU.
#[must_use]
pub(crate) fn resolve(requested: Option<UpscalerKind>) -> UpscalerKind {
    match requested {
        None | Some(UpscalerKind::Taau) => UpscalerKind::Taau,
        Some(other) => {
            let phase = match other {
                UpscalerKind::Fsr3 => "the FSR 3.1 port (N1b)",
                _ => "the vendor back ends (N1c)",
            };
            eprintln!(
                "[render] upscaler {} is not available in this build until {phase} lands; \
                 using taau",
                other.name()
            );
            UpscalerKind::Taau
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn every_choice_resolves_to_taau_until_the_ports_land() {
        assert_eq!(resolve(None), UpscalerKind::Taau);
        for &kind in UpscalerKind::ALL {
            assert_eq!(resolve(Some(kind)), UpscalerKind::Taau);
        }
    }
}
