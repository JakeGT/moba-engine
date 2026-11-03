//! Post-processing configuration (ADR-0015): exposure, grading, bloom, anti-aliasing,
//! ambient occlusion, resolution scaling and the gameplay screen effects.
//!
//! [`PostConfig`] picks which passes the frame graph has; it is fixed when the renderer is
//! created ([`RenderConfig::post`](crate::RenderConfig::post)), like the quality tier.
//! [`PostLook`] is the map's art direction (exposure, bloom strength, grading strength),
//! changed at map load with [`Renderer::set_post_look`](crate::Renderer::set_post_look), and
//! [`ScreenEffects`] are per-frame gameplay values on the [`Scene`](crate::Scene).

use crate::quality::QualityTier;

/// Smallest accepted render scale (`MOBA_RENDER_SCALE`, dynamic resolution's floor).
pub const MIN_RENDER_SCALE: f32 = 0.5;
/// Largest accepted render scale (native resolution).
pub const MAX_RENDER_SCALE: f32 = 1.0;

/// Anti-aliasing of the scene (ADR-0015 §4, ADR-0021 §4b). The HUD is never anti-aliased by
/// these.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AntiAliasing {
    /// None.
    Off,
    /// FXAA on the tonemapped image at render resolution, before the upscale and the UI.
    #[default]
    Fxaa,
    /// 4× MSAA on the scene (and the AO prepass), resolved before the VFX and post passes.
    /// Compat only: the tiered path refuses it with one log line and runs
    /// [`Self::Temporal`] (ADR-0021 §7).
    Msaa4,
    /// Temporal anti-aliasing and upscaling (ADR-0021 §3, §4): a jittered render-resolution
    /// scene reconstructed at output resolution by the [`UpscalerKind`] back end. The tiered
    /// default; Compat has no temporal path and refuses it with one log line (FXAA).
    Temporal,
}

impl AntiAliasing {
    /// Every mode.
    pub const ALL: &'static [Self] = &[Self::Off, Self::Fxaa, Self::Msaa4, Self::Temporal];

    /// Lower-case name, as accepted by [`Self::from_name`] (`MOBA_AA`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Fxaa => "fxaa",
            Self::Msaa4 => "msaa4",
            Self::Temporal => "temporal",
        }
    }

    /// Parses `off`, `fxaa`, `msaa4` or `temporal`, ignoring ASCII case and surrounding
    /// whitespace.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .iter()
            .copied()
            .find(|mode| mode.name().eq_ignore_ascii_case(name))
    }
}

impl core::fmt::Display for AntiAliasing {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

/// The temporal upscaler back ends behind one interface (ADR-0021 §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UpscalerKind {
    /// The in-house TAA/TAAU (Slang): the baseline on every tiered device and the final
    /// fallback (ADR-0021 §3).
    Taau,
    /// AMD FSR 3.1 (the `fsr3` port, N1b).
    Fsr3,
    /// NVIDIA DLSS Super Resolution through NGX (the `dlss` feature, N1c).
    Dlss,
    /// AMD FSR 4 (the `fsr4` feature, Windows, N1c).
    Fsr4,
    /// Intel XeSS (the `xess` feature, Windows, N1c).
    Xess,
}

impl UpscalerKind {
    /// Every back end.
    pub const ALL: &'static [Self] = &[Self::Taau, Self::Fsr3, Self::Dlss, Self::Fsr4, Self::Xess];

    /// Lower-case name (`MOBA_UPSCALER`, the frame log's `upscaler=`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Taau => "taau",
            Self::Fsr3 => "fsr3",
            Self::Dlss => "dlss",
            Self::Fsr4 => "fsr4",
            Self::Xess => "xess",
        }
    }

    /// Parses a back-end name, ignoring ASCII case and surrounding whitespace. `auto` is not a
    /// back end: [`PostConfig::upscaler`] is `None` for it.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.name().eq_ignore_ascii_case(name))
    }
}

impl core::fmt::Display for UpscalerKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

/// The upscaler mode, by output ÷ render ratio per axis (ADR-0021 §4b): one naming scheme
/// for every back end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UpscaleMode {
    /// 1.0: temporal anti-aliasing at native resolution.
    Native,
    /// 1.5.
    Quality,
    /// 1.7.
    Balanced,
    /// 2.0.
    Performance,
    /// 3.0.
    UltraPerformance,
}

impl UpscaleMode {
    /// Every mode, from native down.
    pub const ALL: &'static [Self] = &[
        Self::Native,
        Self::Quality,
        Self::Balanced,
        Self::Performance,
        Self::UltraPerformance,
    ];

    /// Lower-case name (`MOBA_UPSCALE_MODE`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Quality => "quality",
            Self::Balanced => "balanced",
            Self::Performance => "performance",
            Self::UltraPerformance => "ultra-performance",
        }
    }

    /// Parses a mode name, ignoring ASCII case and surrounding whitespace.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .iter()
            .copied()
            .find(|mode| mode.name().eq_ignore_ascii_case(name))
    }

    /// Output ÷ render per axis.
    #[must_use]
    pub const fn ratio(self) -> f32 {
        match self {
            Self::Native => 1.0,
            Self::Quality => 1.5,
            Self::Balanced => 1.7,
            Self::Performance => 2.0,
            Self::UltraPerformance => 3.0,
        }
    }

    /// The render scale, `1 / ratio`.
    #[must_use]
    pub fn scale(self) -> f32 {
        1.0 / self.ratio()
    }

    /// The tier's default mode at an output `height` pixels tall (ADR-0021 §4b, ADR-0020
    /// §5b): Quality on Low, Medium and High; Ultra Quality at 1440p and Performance at 4K;
    /// Overdrive Performance.
    #[must_use]
    pub const fn for_tier(tier: QualityTier, height: u32) -> Self {
        match tier {
            QualityTier::Ultra if height >= 2160 => Self::Performance,
            QualityTier::Overdrive => Self::Performance,
            QualityTier::Low | QualityTier::Medium | QualityTier::High | QualityTier::Ultra => {
                Self::Quality
            }
        }
    }
}

impl core::fmt::Display for UpscaleMode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

/// Smallest render scale the temporal path accepts (Ultra Performance, 3×).
pub const MIN_TEMPORAL_RENDER_SCALE: f32 = 1.0 / 3.0;
/// Default [`PostConfig::sharpness`]: TAAU's and FSR 3.1's starting point (ADR-0021 §3).
pub const DEFAULT_SHARPNESS: f32 = 0.25;

/// `MOBA_DEBUG_VIEW` (ADR-0021 §3, §5; ADR-0023 §1f): what the temporal pass, the G-buffer
/// debug pass or the overlay draws instead of the frame, for inspection. Tiered only (the
/// G-buffer views with temporal AA, which writes the G-buffer); ignored elsewhere.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TemporalDebugView {
    /// The frame.
    #[default]
    Off,
    /// Motion vectors (TAAU).
    Motion,
    /// The reactive mask (TAAU).
    Reactive,
    /// The transparency-and-composition mask (TAAU).
    TcMask,
    /// Where TAAU found the history disoccluded.
    Disocclusion,
    /// TAAU's upsampling confidence.
    Confidence,
    /// The overlay's ID target.
    OverlayIds,
    /// The G-buffer's base colour (ADR-0023 §1f).
    Albedo,
    /// The G-buffer's world normal, as colour.
    Normal,
    /// The G-buffer's perceptual roughness.
    Roughness,
    /// The G-buffer's metallic.
    Metallic,
    /// The G-buffer's shading model, one colour per ID.
    Model,
    /// The G-buffer's team-rim slot, one colour per slot.
    RimSlot,
    /// The G-buffer's flags: red `UNIT`, green `NO_FOG_OF_WAR`, blue reserved.
    Flags,
}

impl TemporalDebugView {
    /// Every view, in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::Off,
        Self::Motion,
        Self::Reactive,
        Self::TcMask,
        Self::Disocclusion,
        Self::Confidence,
        Self::OverlayIds,
        Self::Albedo,
        Self::Normal,
        Self::Roughness,
        Self::Metallic,
        Self::Model,
        Self::RimSlot,
        Self::Flags,
    ];

    /// The `MOBA_DEBUG_VIEW` name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Motion => "motion",
            Self::Reactive => "reactive",
            Self::TcMask => "tc",
            Self::Disocclusion => "disocclusion",
            Self::Confidence => "confidence",
            Self::OverlayIds => "overlay-ids",
            Self::Albedo => "albedo",
            Self::Normal => "normal",
            Self::Roughness => "roughness",
            Self::Metallic => "metallic",
            Self::Model => "model",
            Self::RimSlot => "rim-slot",
            Self::Flags => "flags",
        }
    }

    /// The G-buffer debug pass's view (`gbuffer_debug.slang`'s `push.view`, 1–7), or `None`
    /// for a view something else draws.
    #[must_use]
    pub(crate) const fn gbuffer_view(self) -> Option<u32> {
        match self {
            Self::Albedo => Some(1),
            Self::Normal => Some(2),
            Self::Roughness => Some(3),
            Self::Metallic => Some(4),
            Self::Model => Some(5),
            Self::RimSlot => Some(6),
            Self::Flags => Some(7),
            Self::Off
            | Self::Motion
            | Self::Reactive
            | Self::TcMask
            | Self::Disocclusion
            | Self::Confidence
            | Self::OverlayIds => None,
        }
    }

    /// Parses a `MOBA_DEBUG_VIEW` value (case-insensitive, trimmed).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        Self::ALL.iter().copied().find(|view| view.name() == name)
    }
}

/// How the render-resolution image is scaled to the swapchain when the render scale is
/// below 1 or dynamic resolution is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UpscaleFilter {
    /// One bilinear tap (Low).
    Bilinear,
    /// Catmull-Rom bicubic from nine bilinear taps: sharper at 0.67-0.85.
    #[default]
    Bicubic,
}

impl UpscaleFilter {
    /// Every filter.
    pub const ALL: &'static [Self] = &[Self::Bilinear, Self::Bicubic];

    /// Lower-case name (`MOBA_UPSCALE`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bilinear => "bilinear",
            Self::Bicubic => "bicubic",
        }
    }

    /// Parses `bilinear` or `bicubic`, ignoring ASCII case and surrounding whitespace.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .iter()
            .copied()
            .find(|filter| filter.name().eq_ignore_ascii_case(name))
    }
}

impl core::fmt::Display for UpscaleFilter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

/// Which post-processing passes the frame has (ADR-0015 §8). Fixed at renderer creation.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)] // Independent feature switches.
pub struct PostConfig {
    /// The physically based bloom chain (`MOBA_BLOOM`).
    pub bloom: bool,
    /// Scene anti-aliasing (`MOBA_AA`).
    pub anti_aliasing: AntiAliasing,
    /// FXAA's fast preset (fewer edge-search steps; Low).
    pub fxaa_fast: bool,
    /// GTAO at half resolution with a depth prepass (`MOBA_AO`).
    pub ambient_occlusion: bool,
    /// GTAO slices per pixel (2..=4).
    pub ao_slices: u32,
    /// GTAO world radius in metres.
    pub ao_radius: f32,
    /// Allow eye adaptation for maps that opt in ([`PostLook::auto_exposure`];
    /// `MOBA_EXPOSURE=auto|fixed`). Needs the bloom down-chain, which then runs for metering.
    pub auto_exposure: bool,
    /// Apply the map's colour-grading LUT (`MOBA_GRADING`).
    pub grading: bool,
    /// Render resolution relative to the swapchain, `0.5..=1` (`MOBA_RENDER_SCALE`). With
    /// dynamic resolution it is the ceiling.
    pub render_scale: f32,
    /// Adjust the render scale each frame to hold [`Self::dynamic_target_ms`] of GPU time
    /// (`MOBA_DYNRES`). Needs GPU timestamps.
    pub dynamic_resolution: bool,
    /// Dynamic resolution's floor, `0.5..=render_scale`.
    pub min_render_scale: f32,
    /// Dynamic resolution's GPU-time target per frame, milliseconds, `0.1..=100` (low enough
    /// for a forced target to bite on a fast GPU, ADR-0020 gate 4).
    pub dynamic_target_ms: f32,
    /// The upscale filter when the image is scaled (`MOBA_UPSCALE`).
    pub upscale: UpscaleFilter,
    /// Apply [`ScreenEffects`] (`MOBA_SCREEN_EFFECTS`).
    pub screen_effects: bool,
    /// The temporal back end (`MOBA_UPSCALER`); `None` is `auto` (ADR-0021 §4b: DLSS, FSR 4,
    /// XeSS where compiled in and supported, else TAAU). Only with
    /// [`AntiAliasing::Temporal`].
    pub upscaler: Option<UpscalerKind>,
    /// The temporal mode (`MOBA_UPSCALE_MODE`); `None` is the tier's default at the output
    /// height ([`UpscaleMode::for_tier`]). Its scale is the render-scale ceiling under
    /// [`AntiAliasing::Temporal`] (with [`Self::render_scale`] able to lower it).
    pub upscale_mode: Option<UpscaleMode>,
    /// Sharpening after the temporal pass, `0..=1` (`MOBA_SHARPNESS`), shared by every back
    /// end ([`DEFAULT_SHARPNESS`]).
    pub sharpness: f32,
}

impl PostConfig {
    /// The defaults of `tier` (ADR-0015 §8).
    #[must_use]
    pub const fn for_tier(tier: QualityTier) -> Self {
        let (bloom, anti_aliasing, ambient_occlusion, ao_slices, dynamic_resolution, upscale) =
            match tier {
                QualityTier::Low => (
                    false,
                    AntiAliasing::Fxaa,
                    false,
                    2,
                    true,
                    UpscaleFilter::Bilinear,
                ),
                QualityTier::Medium => (
                    true,
                    AntiAliasing::Fxaa,
                    false,
                    2,
                    false,
                    UpscaleFilter::Bicubic,
                ),
                QualityTier::High => (
                    true,
                    AntiAliasing::Fxaa,
                    true,
                    2,
                    false,
                    UpscaleFilter::Bicubic,
                ),
                QualityTier::Ultra | QualityTier::Overdrive => (
                    true,
                    AntiAliasing::Msaa4,
                    true,
                    3,
                    false,
                    UpscaleFilter::Bicubic,
                ),
            };
        Self {
            bloom,
            anti_aliasing,
            fxaa_fast: matches!(tier, QualityTier::Low),
            ambient_occlusion,
            ao_slices,
            ao_radius: 1.5,
            auto_exposure: !matches!(tier, QualityTier::Low),
            grading: true,
            render_scale: 1.0,
            dynamic_resolution,
            min_render_scale: 0.6,
            // The §7 GPU budget of the tier the dynamic scale protects.
            dynamic_target_ms: 14.0,
            upscale,
            screen_effects: true,
            upscaler: None,
            upscale_mode: None,
            sharpness: DEFAULT_SHARPNESS,
        }
    }

    /// Everything off: the R2 frame (exposure, `AgX`, no grading) at native resolution.
    #[must_use]
    pub const fn off() -> Self {
        Self {
            bloom: false,
            anti_aliasing: AntiAliasing::Off,
            fxaa_fast: false,
            ambient_occlusion: false,
            ao_slices: 2,
            ao_radius: 1.5,
            auto_exposure: false,
            grading: false,
            render_scale: 1.0,
            dynamic_resolution: false,
            min_render_scale: 0.6,
            dynamic_target_ms: 14.0,
            upscale: UpscaleFilter::Bilinear,
            screen_effects: false,
            upscaler: None,
            upscale_mode: None,
            sharpness: DEFAULT_SHARPNESS,
        }
    }

    /// Returns this config with [`Self::bloom`] set.
    #[must_use]
    pub const fn with_bloom(mut self, bloom: bool) -> Self {
        self.bloom = bloom;
        self
    }

    /// Returns this config with [`Self::anti_aliasing`] set.
    #[must_use]
    pub const fn with_anti_aliasing(mut self, anti_aliasing: AntiAliasing) -> Self {
        self.anti_aliasing = anti_aliasing;
        self
    }

    /// Returns this config with [`Self::ambient_occlusion`] set.
    #[must_use]
    pub const fn with_ambient_occlusion(mut self, ambient_occlusion: bool) -> Self {
        self.ambient_occlusion = ambient_occlusion;
        self
    }

    /// Returns this config with [`Self::auto_exposure`] set.
    #[must_use]
    pub const fn with_auto_exposure(mut self, auto_exposure: bool) -> Self {
        self.auto_exposure = auto_exposure;
        self
    }

    /// Returns this config with [`Self::grading`] set.
    #[must_use]
    pub const fn with_grading(mut self, grading: bool) -> Self {
        self.grading = grading;
        self
    }

    /// Returns this config with [`Self::render_scale`] set (clamped by
    /// [`Self::sanitized`]).
    #[must_use]
    pub const fn with_render_scale(mut self, render_scale: f32) -> Self {
        self.render_scale = render_scale;
        self
    }

    /// Returns this config with [`Self::dynamic_resolution`] set.
    #[must_use]
    pub const fn with_dynamic_resolution(mut self, dynamic_resolution: bool) -> Self {
        self.dynamic_resolution = dynamic_resolution;
        self
    }

    /// Returns this config with [`Self::dynamic_target_ms`] set.
    #[must_use]
    pub const fn with_dynamic_target_ms(mut self, target_ms: f32) -> Self {
        self.dynamic_target_ms = target_ms;
        self
    }

    /// Returns this config with [`Self::min_render_scale`] set.
    #[must_use]
    pub const fn with_min_render_scale(mut self, min_render_scale: f32) -> Self {
        self.min_render_scale = min_render_scale;
        self
    }

    /// Returns this config with [`Self::upscale`] set.
    #[must_use]
    pub const fn with_upscale(mut self, upscale: UpscaleFilter) -> Self {
        self.upscale = upscale;
        self
    }

    /// Returns this config with [`Self::upscaler`] set (`None`: `auto`).
    #[must_use]
    pub const fn with_upscaler(mut self, upscaler: Option<UpscalerKind>) -> Self {
        self.upscaler = upscaler;
        self
    }

    /// Returns this config with [`Self::upscale_mode`] set (`None`: the tier's).
    #[must_use]
    pub const fn with_upscale_mode(mut self, mode: Option<UpscaleMode>) -> Self {
        self.upscale_mode = mode;
        self
    }

    /// Returns this config with [`Self::sharpness`] set (clamped by [`Self::sanitized`]).
    #[must_use]
    pub const fn with_sharpness(mut self, sharpness: f32) -> Self {
        self.sharpness = sharpness;
        self
    }

    /// Returns this config with [`Self::screen_effects`] set.
    #[must_use]
    pub const fn with_screen_effects(mut self, screen_effects: bool) -> Self {
        self.screen_effects = screen_effects;
        self
    }

    /// The config with every number clamped to its range (non-finite values take the
    /// defaults): render scales into `0.5..=1` (`1/3..=1` under [`AntiAliasing::Temporal`],
    /// down to Ultra Performance) with the floor at most the ceiling, slices `2..=4`, a
    /// radius `0.25..=8` m, a target `0.1..=100` ms and the sharpness `0..=1`.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        let finite = |value: f32, default: f32| if value.is_finite() { value } else { default };
        let min_scale = if self.anti_aliasing == AntiAliasing::Temporal {
            MIN_TEMPORAL_RENDER_SCALE
        } else {
            MIN_RENDER_SCALE
        };
        self.render_scale = finite(self.render_scale, 1.0).clamp(min_scale, MAX_RENDER_SCALE);
        self.min_render_scale = finite(self.min_render_scale, 0.6)
            .clamp(min_scale, MAX_RENDER_SCALE)
            .min(self.render_scale);
        self.sharpness = finite(self.sharpness, DEFAULT_SHARPNESS).clamp(0.0, 1.0);
        self.ao_slices = self.ao_slices.clamp(2, 4);
        self.ao_radius = finite(self.ao_radius, 1.5).clamp(0.25, 8.0);
        self.dynamic_target_ms = finite(self.dynamic_target_ms, 14.0).clamp(0.1, 100.0);
        self
    }

    /// Whether the frame renders below native resolution (or may, with dynamic resolution),
    /// so it has an upscale pass.
    #[must_use]
    pub fn scaled(&self) -> bool {
        self.dynamic_resolution || self.render_scale < MAX_RENDER_SCALE
    }
}

impl Default for PostConfig {
    fn default() -> Self {
        Self::for_tier(QualityTier::default())
    }
}

/// The map's post-processing art direction (client-local `[post]` map visuals). Every value
/// is sanitised when it reaches the renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct PostLook {
    /// Exposure compensation in EV stops (`-4..=4`), on top of
    /// [`RenderConfig::exposure`](crate::RenderConfig::exposure).
    pub exposure_ev: f32,
    /// The map asks for eye adaptation (used when [`PostConfig::auto_exposure`] allows it).
    pub auto_exposure: bool,
    /// Lowest auto-exposure EV (`-8..=8`).
    pub exposure_min: f32,
    /// Highest auto-exposure EV (`-8..=8`, at least `exposure_min`).
    pub exposure_max: f32,
    /// Adaptation rate per second (`0.1..=10`).
    pub adaptation: f32,
    /// Bloom mix (`0..=0.1`, as a map's `[post] bloom`).
    pub bloom: f32,
    /// How much of the grading LUT applies (`0..=1`).
    pub grading_strength: f32,
}

impl PostLook {
    /// The engine defaults: no compensation, fixed exposure, bloom 0.04, full grading.
    pub const DEFAULT: Self = Self {
        exposure_ev: 0.0,
        auto_exposure: false,
        exposure_min: moba_assets::DEFAULT_EXPOSURE_MIN,
        exposure_max: moba_assets::DEFAULT_EXPOSURE_MAX,
        adaptation: moba_assets::DEFAULT_ADAPTATION,
        bloom: moba_assets::DEFAULT_BLOOM_STRENGTH,
        grading_strength: 1.0,
    };

    /// Returns this look with [`Self::exposure_ev`] set.
    #[must_use]
    pub const fn with_exposure_ev(mut self, exposure_ev: f32) -> Self {
        self.exposure_ev = exposure_ev;
        self
    }

    /// Returns this look with eye adaptation on or off and its EV clamp.
    #[must_use]
    pub const fn with_auto_exposure(mut self, on: bool, min_ev: f32, max_ev: f32) -> Self {
        self.auto_exposure = on;
        self.exposure_min = min_ev;
        self.exposure_max = max_ev;
        self
    }

    /// Returns this look with [`Self::adaptation`] set.
    #[must_use]
    pub const fn with_adaptation(mut self, adaptation: f32) -> Self {
        self.adaptation = adaptation;
        self
    }

    /// Returns this look with [`Self::bloom`] set.
    #[must_use]
    pub const fn with_bloom(mut self, bloom: f32) -> Self {
        self.bloom = bloom;
        self
    }

    /// Returns this look with [`Self::grading_strength`] set.
    #[must_use]
    pub const fn with_grading_strength(mut self, strength: f32) -> Self {
        self.grading_strength = strength;
        self
    }

    /// Every value clamped to its range; non-finite values take the defaults.
    #[must_use]
    pub fn sanitized(self) -> Self {
        let clamp = |value: f32, low: f32, high: f32, default: f32| {
            if value.is_finite() {
                value.clamp(low, high)
            } else {
                default
            }
        };
        let defaults = Self::DEFAULT;
        let exposure_min = clamp(self.exposure_min, -8.0, 8.0, defaults.exposure_min);
        Self {
            exposure_ev: clamp(self.exposure_ev, -4.0, 4.0, 0.0),
            auto_exposure: self.auto_exposure,
            exposure_min,
            exposure_max: clamp(self.exposure_max, -8.0, 8.0, defaults.exposure_max)
                .max(exposure_min),
            adaptation: clamp(self.adaptation, 0.1, 10.0, defaults.adaptation),
            bloom: clamp(self.bloom, 0.0, 0.1, defaults.bloom),
            grading_strength: clamp(self.grading_strength, 0.0, 1.0, 1.0),
        }
    }
}

impl Default for PostLook {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Per-frame gameplay screen effects (ADR-0015 §7), applied in the composite after grading
/// and before the HUD. All zero (the default) is a no-op. The client sets them from its own
/// presentation state and each player's accessibility settings.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct ScreenEffects {
    /// Fade towards greyscale, `0..=1` (death desaturation).
    pub desaturation: f32,
    /// Strength of a coloured vignette at the screen edges, `0..=1` (damage flash and the
    /// low-health pulse).
    pub vignette: f32,
    /// The vignette's display-space colour (`0..=1`).
    pub vignette_color: [f32; 3],
}

impl ScreenEffects {
    /// No effect.
    pub const NONE: Self = Self {
        desaturation: 0.0,
        vignette: 0.0,
        vignette_color: [0.04, 0.04, 0.05],
    };

    /// Returns these effects with [`Self::desaturation`] set.
    #[must_use]
    pub const fn with_desaturation(mut self, desaturation: f32) -> Self {
        self.desaturation = desaturation;
        self
    }

    /// Returns these effects with the vignette strength and colour set.
    #[must_use]
    pub const fn with_vignette(mut self, strength: f32, color: [f32; 3]) -> Self {
        self.vignette = strength;
        self.vignette_color = color;
        self
    }

    /// Every value clamped to `0..=1` (non-finite: 0).
    #[must_use]
    pub fn sanitized(self) -> Self {
        let unit = |value: f32| {
            if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        Self {
            desaturation: unit(self.desaturation),
            vignette: unit(self.vignette),
            vignette_color: self.vignette_color.map(unit),
        }
    }
}

impl Default for ScreenEffects {
    fn default() -> Self {
        Self::NONE
    }
}

/// The smallest blend factor an adaptation step uses on an `R16_SFLOAT` exposure image: with
/// a smaller factor, `factor × (target − adapted)` would round away below half-float
/// precision and adaptation would stall short of the target (about 0.6 EV at 240 fps and a
/// slow adaptation). With it the residual is at most half an ulp / step: ≤ 0.03 EV below
/// 4 EV, ≤ 0.06 EV up to 8.
pub(crate) const MIN_HALF_ADAPTATION_STEP: f32 = 1.0 / 32.0;

/// Accumulated time after which a step is taken whatever its factor (a guard for tiny rates).
const MAX_ADAPTATION_WAIT: f32 = 1.0;

/// One frame's exposure adaptation: blend `factor` of the metered EV into the adapted EV,
/// or `reset` (clear, then take the metered value).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AdaptationStep {
    pub(crate) factor: f32,
    pub(crate) reset: bool,
}

/// The adaptation step for a frame `dt` seconds after the previous one, at `rate` (1/s):
/// `1 − exp(−t × rate)` over the time `pending` has accumulated. A step smaller than
/// `min_step` is deferred (factor 0, the time kept in `pending`), so a low-precision image
/// still converges; `min_step` 0 steps every frame.
pub(crate) fn adaptation_step(
    pending: &mut f32,
    dt: f32,
    rate: f32,
    min_step: f32,
    reset: bool,
) -> AdaptationStep {
    if reset {
        *pending = 0.0;
        return AdaptationStep {
            factor: 1.0,
            reset: true,
        };
    }
    let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
    let rate = if rate.is_finite() { rate.max(0.0) } else { 0.0 };
    *pending += dt;
    let factor = 1.0 - (-*pending * rate).exp();
    if factor >= min_step || *pending >= MAX_ADAPTATION_WAIT {
        *pending = 0.0;
        AdaptationStep {
            factor: factor.clamp(0.0, 1.0),
            reset: false,
        }
    } else {
        AdaptationStep {
            factor: 0.0,
            reset: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulates `seconds` of adaptation at `fps` from 0 EV towards `target`, rounding the
    /// stored value to half precision after every blend when `half`.
    fn adapt(target: f32, fps: f32, rate: f32, seconds: f32, half: bool) -> f32 {
        let min_step = if half { MIN_HALF_ADAPTATION_STEP } else { 0.0 };
        let mut pending = 0.0;
        let mut adapted = 0.0_f32;
        let frames = (seconds * fps) as usize;
        for _ in 0..frames {
            let step = adaptation_step(&mut pending, 1.0 / fps, rate, min_step, false);
            adapted += step.factor * (target - adapted);
            if half {
                adapted = moba_assets::f32_from_f16(moba_assets::f16_from_f32(adapted));
            }
        }
        adapted
    }

    #[test]
    fn adaptation_converges_at_high_frame_rates_even_in_half_precision() {
        // 240 fps, the slowest adaptation (0.1 / s) for 60 s: e^-6 of the way left.
        for target in [-3.0_f32, 1.5, 3.9, 6.0] {
            let full = adapt(target, 240.0, 0.1, 60.0, false);
            assert!((full - target).abs() < 0.02, "full {target}: {full}");
            let half = adapt(target, 240.0, 0.1, 60.0, true);
            assert!((half - target).abs() < 0.07, "half {target}: {half}");
        }
    }

    #[test]
    fn adaptation_steps_reset_defer_and_sanitise() {
        let mut pending = 0.3;
        let reset = adaptation_step(&mut pending, 0.01, 1.0, 0.0, true);
        assert_eq!(
            reset,
            AdaptationStep {
                factor: 1.0,
                reset: true
            }
        );
        assert_eq!(pending, 0.0);
        // Deferred below the minimum step, then taken with the accumulated time.
        let small = adaptation_step(&mut pending, 0.01, 1.0, 0.05, false);
        assert_eq!(small.factor, 0.0);
        let mut taken = small;
        for _ in 0..10 {
            taken = adaptation_step(&mut pending, 0.01, 1.0, 0.05, false);
            if taken.factor > 0.0 {
                break;
            }
        }
        assert!(taken.factor >= 0.05 && taken.factor < 0.07, "{taken:?}");
        assert_eq!(pending, 0.0);
        // Non-finite inputs never produce a non-finite factor.
        for (dt, rate) in [(f32::NAN, 1.0), (0.01, f32::NAN), (f32::INFINITY, 1.0)] {
            let step = adaptation_step(&mut pending, dt, rate, 0.0, false);
            assert!(step.factor.is_finite() && (0.0..=1.0).contains(&step.factor));
        }
    }

    #[test]
    fn tiers_scale_post_features_up_and_low_keeps_the_cheap_path() {
        let low = PostConfig::for_tier(QualityTier::Low);
        assert!(!low.bloom && !low.ambient_occlusion && !low.auto_exposure);
        assert_eq!(low.anti_aliasing, AntiAliasing::Fxaa);
        assert!(low.fxaa_fast && low.dynamic_resolution && low.scaled());
        assert_eq!(low.upscale, UpscaleFilter::Bilinear);
        let medium = PostConfig::for_tier(QualityTier::Medium);
        assert!(medium.bloom && !medium.ambient_occlusion && !medium.scaled());
        let high = PostConfig::for_tier(QualityTier::High);
        assert!(high.ambient_occlusion && high.auto_exposure);
        assert_eq!(high.anti_aliasing, AntiAliasing::Fxaa);
        let ultra = PostConfig::for_tier(QualityTier::Ultra);
        assert_eq!(ultra.anti_aliasing, AntiAliasing::Msaa4);
        assert!(ultra.ao_slices > high.ao_slices);
        for &tier in QualityTier::ALL {
            let config = PostConfig::for_tier(tier);
            assert!(config.grading && config.screen_effects);
            assert_eq!(config, config.sanitized(), "{tier}");
        }
        assert_eq!(PostConfig::default(), high);
        let off = PostConfig::off();
        assert!(!off.scaled() && !off.bloom && off.anti_aliasing == AntiAliasing::Off);
    }

    #[test]
    fn names_round_trip_and_hostile_numbers_are_clamped() {
        for &mode in AntiAliasing::ALL {
            assert_eq!(AntiAliasing::from_name(mode.name()), Some(mode));
        }
        assert_eq!(
            AntiAliasing::from_name(" MSAA4 "),
            Some(AntiAliasing::Msaa4)
        );
        assert_eq!(AntiAliasing::from_name("taa"), None);
        for &filter in UpscaleFilter::ALL {
            assert_eq!(UpscaleFilter::from_name(filter.name()), Some(filter));
        }
        let config = PostConfig::default()
            .with_render_scale(f32::NAN)
            .with_min_render_scale(0.1)
            .with_dynamic_target_ms(-3.0)
            .sanitized();
        assert_eq!(config.render_scale, 1.0);
        assert_eq!(config.min_render_scale, MIN_RENDER_SCALE);
        assert_eq!(config.dynamic_target_ms, 0.1);
        let scaled = PostConfig::default()
            .with_render_scale(0.7)
            .with_min_render_scale(0.9)
            .sanitized();
        assert_eq!(scaled.min_render_scale, 0.7);
        assert!(scaled.scaled());
        let look = PostLook::DEFAULT
            .with_exposure_ev(40.0)
            .with_auto_exposure(true, 3.0, -3.0)
            .with_bloom(f32::INFINITY)
            .sanitized();
        assert_eq!(look.exposure_ev, 4.0);
        assert_eq!((look.exposure_min, look.exposure_max), (3.0, 3.0));
        assert_eq!(look.bloom, PostLook::DEFAULT.bloom);
        let effects = ScreenEffects::NONE
            .with_desaturation(2.0)
            .with_vignette(f32::NAN, [2.0, -1.0, 0.5])
            .sanitized();
        assert_eq!(effects.desaturation, 1.0);
        assert_eq!(effects.vignette, 0.0);
        assert_eq!(effects.vignette_color, [1.0, 0.0, 0.5]);
    }
}
