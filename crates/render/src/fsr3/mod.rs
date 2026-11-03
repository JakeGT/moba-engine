//! Our port of the AMD FidelityFX **FSR 3.1.6 upscaler** (feature `fsr3`; ADR-0020 §6a and
//! Q9, ADR-0021 §4b). Frame generation is not ported.
//!
//! The SDK's MIT HLSL passes are vendored unmodified under `third_party/fidelityfx/` and built
//! by `moba-shaders` with the pinned `slangc` ([`moba_shaders::fsr3`]). This module is the
//! host side of the SDK's `ffx_fsr3upscaler.cpp` and its Vulkan back end, rewritten in Rust
//! over our RAII wrappers: the internal resources, the per-frame constants, the clears, and
//! the dispatch sequence with its barriers. `docs/dev/fsr3.md` maps each piece to its
//! upstream source.
//!
//! [`Fsr3`] is a [`TemporalUpscaler`](crate::upscale::TemporalUpscaler) (ADR-0021 §4a,
//! amendment 2 item 19): `MOBA_UPSCALER=fsr3` selects it. Its inputs are the trait's
//! [`UpscaleInputs`](crate::upscale::UpscaleInputs): render-resolution images in the top-left
//! sub-rectangle, moved to `SHADER_READ_ONLY_OPTIMAL` by the caller, and an `RGBA16F` storage
//! output in `GENERAL`. Motion vectors are render pixels to the previous position (scale
//! (1, 1)), depth is `D32_SFLOAT` with 0 near and a finite far plane, and the exposure is a
//! 1×1 linear multiplier.
//!
//! **Render sizes (ADR-0021 §4c).** One context accepts any render size from 1×1 up to the
//! maximum it was created with, changing from frame to frame without a reset: every pass reads
//! the frame's size from the constants, clamps its samples to that sub-rectangle, and
//! reprojects with the previous frame's size. `fsr3_smoke` checks a sweep of sizes inside one
//! context (ADR-0021 amendment 1).

mod constants;
mod context;

use ash::vk;

pub(crate) use context::{Fsr3, SET_RING};

use crate::upscale::UpscaleMode;

/// The FSR 3.1 quality modes by ratio (output ÷ render per axis), ADR-0021 §4b.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fsr3Mode {
    /// 1.0: anti-aliasing only.
    NativeAa,
    /// 1.5.
    Quality,
    /// 1.7.
    Balanced,
    /// 2.0.
    Performance,
    /// 3.0.
    UltraPerformance,
}

impl From<UpscaleMode> for Fsr3Mode {
    /// The menu's modes by ratio (ADR-0021 §4b): `Native` is FSR's native AA, the rest by name
    /// (amendment 2, item 19).
    fn from(mode: UpscaleMode) -> Self {
        match mode {
            UpscaleMode::Native => Self::NativeAa,
            UpscaleMode::Quality => Self::Quality,
            UpscaleMode::Balanced => Self::Balanced,
            UpscaleMode::Performance => Self::Performance,
            UpscaleMode::UltraPerformance => Self::UltraPerformance,
        }
    }
}

impl Fsr3Mode {
    /// Every mode, largest render size first.
    #[cfg(test)]
    pub(crate) const ALL: &'static [Self] = &[
        Self::NativeAa,
        Self::Quality,
        Self::Balanced,
        Self::Performance,
        Self::UltraPerformance,
    ];

    /// Output ÷ render per axis (`ffxFsr3UpscalerGetUpscaleRatioFromQualityMode`).
    #[must_use]
    pub(crate) const fn ratio(self) -> f32 {
        match self {
            Self::NativeAa => 1.0,
            Self::Quality => 1.5,
            Self::Balanced => 1.7,
            Self::Performance => 2.0,
            Self::UltraPerformance => 3.0,
        }
    }

    /// The mode's render size for `output`, as the SDK computes it
    /// (`ffxFsr3UpscalerGetRenderResolutionFromQualityMode`: the quotient truncated), at
    /// least 1×1.
    pub(crate) fn render_size(self, output: vk::Extent2D) -> vk::Extent2D {
        let ratio = self.ratio();
        let axis = |size: u32| ((size as f32 / ratio) as u32).max(1);
        vk::Extent2D {
            width: axis(output.width),
            height: axis(output.height),
        }
    }
}

/// ADR-0021 §1's phase count: `ceil(8 r²)` with `r` = output ÷ render width rounded to three
/// decimals: 8 at native, 18 at exactly 1.5×, 24 at 1.7×, 32 at 2× and 72 at exactly 3×. The
/// SDK's mode sizes are truncated, so their ratios land just above the nominal ones: at 2560
/// output, Quality (1706) gives 1.501 and 19 phases, Ultra Performance (853) 3.001 and 73.
/// FSR's own `ffxFsr3UpscalerGetJitterPhaseCount` truncates instead (23 at 1.7×); the port
/// passes this count to the shaders as the jitter sequence length, so they see the sequence
/// the renderer actually runs (ADR-0021 amendment 1).
#[must_use]
pub(crate) fn jitter_phases(render: vk::Extent2D, output: vk::Extent2D) -> u32 {
    let ratio = f64::from(output.width) / f64::from(render.width.max(1));
    let ratio = (ratio * 1000.0).round() / 1000.0;
    (8.0 * ratio * ratio).ceil().max(1.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extent(width: u32, height: u32) -> vk::Extent2D {
        vk::Extent2D { width, height }
    }

    #[test]
    fn modes_match_the_sdk_ratios_and_sizes() {
        let ratios: Vec<f32> = Fsr3Mode::ALL.iter().map(|mode| mode.ratio()).collect();
        assert_eq!(ratios, [1.0, 1.5, 1.7, 2.0, 3.0]);
        let output = extent(2560, 1440);
        let sizes: Vec<(u32, u32)> = Fsr3Mode::ALL
            .iter()
            .map(|mode| {
                let size = mode.render_size(output);
                (size.width, size.height)
            })
            .collect();
        assert_eq!(
            sizes,
            [
                (2560, 1440),
                (1706, 960),
                (1505, 847),
                (1280, 720),
                (853, 480)
            ]
        );
        assert_eq!(
            Fsr3Mode::UltraPerformance.render_size(extent(2, 1)),
            extent(1, 1)
        );
    }

    #[test]
    fn the_menu_modes_map_by_ratio() {
        for &mode in UpscaleMode::ALL {
            assert_eq!(Fsr3Mode::from(mode).ratio(), mode.ratio(), "{mode:?}");
        }
        assert_eq!(Fsr3Mode::from(UpscaleMode::Native), Fsr3Mode::NativeAa);
    }

    #[test]
    fn phase_counts_follow_adr_0021() {
        let output = extent(2560, 1440);
        let phases: Vec<u32> = Fsr3Mode::ALL
            .iter()
            .map(|mode| jitter_phases(mode.render_size(output), output))
            .collect();
        // The SDK truncates the mode sizes, so the ratios land just above the nominal ones:
        // 2560 / 1706 = 1.5006 rounds to 1.501, ceil(18.02) = 19 (2560 / 1707 = 1.49971 rounds
        // to 1.5: the ADR's 18), and 2560 / 853 = 3.0012 rounds to 3.001, ceil(72.05) = 73.
        assert_eq!(phases, [8, 19, 24, 32, 73]);
        assert_eq!(jitter_phases(extent(640, 360), extent(1920, 1080)), 72);
        assert_eq!(jitter_phases(extent(1707, 960), output), 18);
        assert_eq!(jitter_phases(extent(1920, 1080), extent(1920, 1080)), 8);
    }
}
