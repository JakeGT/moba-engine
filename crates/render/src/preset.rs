//! Tier presets (ADR-0020 §5a, §5e): the roadmap's §3 feature matrix as data, and how each
//! tier maps onto today's switches until the phases that build its features land.
//!
//! The tiered path runs today's frame (§2c) with N1a's temporal foundation. [`config_tier`]
//! picks the Compat tier whose configuration it runs: tiered Low uses today's High and tiered
//! Medium and up today's Ultra, with temporal AA and TAAU in place of FXAA and MSAA 4×
//! (MSAA is Compat-only, ADR-0021 §7).
//! [`TierPreset::features`] lists the §3 matrix per tier; a feature not built yet is
//! [`FeatureState::Planned`] with its phase, so a settings menu can show it greyed out.

use crate::caps::RenderPath;
use crate::quality::QualityTier;

/// A next-gen roadmap phase (roadmap §8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Phase {
    /// Temporal and upscaling.
    N1,
    /// Frame architecture and materials.
    N2,
    /// Baked lighting.
    N3,
    /// Ray-tracing infrastructure.
    N4,
    /// Hybrid RT (Low–High).
    N5,
    /// Atmosphere and weather.
    N6,
    /// GPU-driven geometry.
    N7,
    /// Ultra GI and many lights.
    N8,
    /// Overdrive.
    N9,
    /// Characters and showcase.
    N10,
    /// Content scale.
    N11,
}

impl Phase {
    /// `"N1"` … `"N11"`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::N1 => "N1",
            Self::N2 => "N2",
            Self::N3 => "N3",
            Self::N4 => "N4",
            Self::N5 => "N5",
            Self::N6 => "N6",
            Self::N7 => "N7",
            Self::N8 => "N8",
            Self::N9 => "N9",
            Self::N10 => "N10",
            Self::N11 => "N11",
        }
    }
}

/// Whether a preset's feature exists yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FeatureState {
    /// Built: today's frame has it.
    Built,
    /// Built by that phase; until then the preset runs without it.
    Planned(Phase),
    /// The tier does not offer it ("—" in the matrix).
    NotOffered,
}

/// One cell of the §3 feature matrix: what a tier asks of one area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct PresetFeature {
    /// The matrix row, e.g. `"Sun shadows"`.
    pub area: &'static str,
    /// The tier's level of it, e.g. `"RT sun shadows (SIGMA)"`.
    pub level: &'static str,
    /// Built, planned or not offered.
    pub state: FeatureState,
}

/// One tier's preset on the tiered path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TierPreset {
    /// The tier.
    pub tier: QualityTier,
    /// The Compat tier whose switches the preset runs in N0 ([`config_tier`]).
    pub base: QualityTier,
}

impl TierPreset {
    /// The preset of `tier` on the tiered path.
    #[must_use]
    pub const fn of(tier: QualityTier) -> Self {
        Self {
            tier,
            base: config_tier(RenderPath::Tiered, tier),
        }
    }

    /// The tier's column of the §3 feature matrix, one entry per row.
    #[must_use]
    pub fn features(self) -> [PresetFeature; MATRIX.len()] {
        let column = match self.tier {
            QualityTier::Low => 0,
            QualityTier::Medium => 1,
            QualityTier::High => 2,
            QualityTier::Ultra => 3,
            QualityTier::Overdrive => 4,
        };
        MATRIX.map(|(area, cells)| {
            let (level, state) = cells[column];
            PresetFeature { area, level, state }
        })
    }
}

/// The Compat tier whose configuration (lighting, post, world, environment, VFX, LODs)
/// `tier` runs on `path` in N0 (ADR-0020 §5e). Compat clamps Overdrive to Ultra; tiered Low is
/// today's High, tiered Medium and up today's Ultra.
#[must_use]
pub const fn config_tier(path: RenderPath, tier: QualityTier) -> QualityTier {
    match path {
        RenderPath::Compat => tier.compat(),
        RenderPath::Tiered => match tier {
            QualityTier::Low => QualityTier::High,
            _ => QualityTier::Ultra,
        },
    }
}

/// The device memory budget of `tier` on `path`: today's table on Compat (ADR-0018 §4), the
/// re-derived tiered table on Tiered (ADR-0020 §5e).
#[must_use]
pub fn total_memory_budget(path: RenderPath, tier: QualityTier) -> u64 {
    match path {
        RenderPath::Compat => tier.compat().budget().total_bytes,
        RenderPath::Tiered => moba_assets::gpu_budget(tier.name()).map_or_else(
            || tier.compat().budget().total_bytes,
            |budget| budget.vram_bytes,
        ),
    }
}

use FeatureState::{Built, NotOffered, Planned};
use Phase::{N1, N2, N3, N5, N6, N7, N8, N9, N10};

type Cell = (&'static str, FeatureState);

/// Roadmap §3's feature matrix: per row, the Low, Medium, High, Ultra and Overdrive cells.
/// "Built" marks what today's frame already has at that level.
const MATRIX: [(&str, [Cell; 5]); 16] = [
    (
        "Frame",
        [
            ("thin G-buffer, deferred", Planned(N2)),
            ("thin G-buffer, deferred", Planned(N2)),
            ("thin G-buffer, deferred", Planned(N2)),
            ("thin G-buffer, deferred", Planned(N2)),
            (
                "thin G-buffer, deferred, plus path-traced lighting",
                Planned(N9),
            ),
        ],
    ),
    (
        "Anti-aliasing and upscaling",
        [
            ("TAA or DLSS/FSR/XeSS", Planned(N1)),
            ("TAA or DLSS/FSR/XeSS", Planned(N1)),
            ("TAA or DLSS/FSR/XeSS", Planned(N1)),
            ("TAA or DLSS/FSR/XeSS, DLAA option", Planned(N1)),
            ("DLSS RR (NRD and an upscaler elsewhere)", Planned(N9)),
        ],
    ),
    (
        "Static GI",
        [
            ("baked (lightmaps and SH probes)", Planned(N3)),
            ("baked", Planned(N3)),
            ("baked", Planned(N3)),
            ("baked plus RT GI", Planned(N8)),
            ("path-traced", Planned(N9)),
        ],
    ),
    (
        "Dynamic GI",
        [
            ("probe-lit dynamics", Planned(N3)),
            ("probe-lit dynamics plus half-res SSGI", Planned(N5)),
            ("plus an RT probe volume near the camera", Planned(N5)),
            ("hash radiance cache with per-pixel gather", Planned(N8)),
            ("path-traced plus SHaRC (NRC optional)", Planned(N9)),
        ],
    ),
    (
        "Sun shadows",
        [
            ("tight shadow map with PCSS", Built),
            ("shadow map plus RT contact shadows", Planned(N5)),
            ("RT sun shadows (SIGMA)", Planned(N5)),
            ("RT sun shadows (SIGMA)", Planned(N5)),
            ("path-traced", Planned(N9)),
        ],
    ),
    (
        "Local and ability lights",
        [
            ("clustered, 2–4 shadowed", Planned(N2)),
            ("plus LTC area lights", Planned(N2)),
            ("plus RT shadow rays, top-N per tile", Planned(N5)),
            ("ReSTIR DI", Planned(N8)),
            ("ReSTIR DI", Planned(N9)),
        ],
    ),
    (
        "Reflections",
        [
            ("baked probes", Planned(N3)),
            ("SSR plus probes", Planned(N5)),
            ("half-res RT below 0.45 roughness", Planned(N5)),
            ("full-res RT, multi-bounce through the cache", Planned(N8)),
            ("path-traced", Planned(N9)),
        ],
    ),
    (
        "Ambient occlusion",
        [
            ("baked plus GTAO", Built),
            ("baked plus GTAO", Built),
            ("RT AO", Planned(N5)),
            ("from GI", Planned(N8)),
            ("from path tracing", Planned(N9)),
        ],
    ),
    (
        "Materials",
        [
            ("multi-layer PBR", Planned(N2)),
            ("multi-layer PBR", Planned(N2)),
            ("multi-layer PBR", Planned(N2)),
            ("multi-layer PBR", Planned(N2)),
            ("multi-layer PBR", Planned(N2)),
        ],
    ),
    (
        "Skin",
        [
            ("pre-integrated", Planned(N10)),
            ("pre-integrated", Planned(N10)),
            ("separable SSS", Planned(N10)),
            ("separable SSS", Planned(N10)),
            ("path-traced SSS", Planned(N10)),
        ],
    ),
    (
        "Hair",
        [
            ("cards", Planned(N10)),
            ("cards", Planned(N10)),
            ("cards", Planned(N10)),
            ("cards", Planned(N10)),
            ("cards; strands in showcase", Planned(N10)),
        ],
    ),
    (
        "Atmosphere",
        [
            ("analytic height fog, Hillaire sky", Planned(N6)),
            ("plus froxel volumetrics", Planned(N6)),
            ("plus froxel volumetrics", Planned(N6)),
            ("plus volumetric shadows from local lights", Planned(N6)),
            ("plus volumetric shadows from local lights", Planned(N6)),
        ],
    ),
    (
        "Weather and wetness",
        [
            ("puddle masks", Planned(N6)),
            ("plus ripples and streaks", Planned(N6)),
            ("plus ripples and streaks", Planned(N6)),
            ("plus ripples and streaks", Planned(N6)),
            ("plus ripples and streaks", Planned(N6)),
        ],
    ),
    (
        "Geometry",
        [
            ("GPU-driven, discrete LODs", Planned(N7)),
            ("plus the mesh-shader path", Planned(N7)),
            ("plus the mesh-shader path", Planned(N7)),
            ("plus cluster LOD for static", Planned(N7)),
            ("plus cluster LOD, Mega Geometry optional", Planned(N7)),
        ],
    ),
    (
        "Frame generation",
        [
            ("—", NotOffered),
            ("—", NotOffered),
            ("—", NotOffered),
            ("opt-in", Planned(N9)),
            ("opt-in", Planned(N9)),
        ],
    ),
    (
        "DLSS 5",
        [
            ("—", NotOffered),
            ("—", NotOffered),
            ("—", NotOffered),
            ("—", NotOffered),
            ("opt-in, not in ranked (RTX 50)", Planned(N9)),
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_map_onto_todays_switches() {
        use QualityTier::{High, Low, Medium, Ultra};
        let tiered: Vec<_> = QualityTier::ALL
            .iter()
            .map(|&tier| config_tier(RenderPath::Tiered, tier))
            .collect();
        assert_eq!(tiered, [High, Ultra, Ultra, Ultra, Ultra]);
        let compat: Vec<_> = QualityTier::ALL
            .iter()
            .map(|&tier| config_tier(RenderPath::Compat, tier))
            .collect();
        assert_eq!(
            compat,
            [Low, Medium, High, Ultra, Ultra],
            "Compat is today's, Overdrive clamps"
        );
        assert_eq!(TierPreset::of(Low).base, High);
        assert_eq!(TierPreset::of(Medium).base, Ultra);
    }

    #[test]
    fn memory_budgets_follow_the_path() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let tiered: Vec<_> = QualityTier::ALL
            .iter()
            .map(|&tier| total_memory_budget(RenderPath::Tiered, tier) / GIB)
            .collect();
        assert_eq!(tiered, [4, 6, 6, 12, 12]);
        for &tier in QualityTier::COMPAT {
            assert_eq!(
                total_memory_budget(RenderPath::Compat, tier),
                tier.budget().total_bytes
            );
        }
        assert_eq!(
            total_memory_budget(RenderPath::Compat, QualityTier::Overdrive),
            QualityTier::Ultra.budget().total_bytes
        );
    }

    #[test]
    fn the_matrix_has_every_row_and_tier() {
        let rows: Vec<_> = MATRIX.iter().map(|(area, _)| *area).collect();
        let mut unique = rows.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), rows.len(), "no duplicate rows");
        for &tier in QualityTier::ALL {
            let features = TierPreset::of(tier).features();
            assert_eq!(features.len(), 16);
            for feature in features {
                assert!(!feature.level.is_empty(), "{tier}: {}", feature.area);
                assert_eq!(
                    feature.level == "—",
                    feature.state == FeatureState::NotOffered,
                    "{tier}: {}",
                    feature.area
                );
            }
        }
        // Only Overdrive offers DLSS 5; frame generation starts at Ultra.
        let offered = |tier: QualityTier, area: &str| {
            TierPreset::of(tier)
                .features()
                .iter()
                .find(|feature| feature.area == area)
                .is_some_and(|feature| feature.state != FeatureState::NotOffered)
        };
        assert!(offered(QualityTier::Overdrive, "DLSS 5"));
        assert!(!offered(QualityTier::Ultra, "DLSS 5"));
        assert!(offered(QualityTier::Ultra, "Frame generation"));
        assert!(!offered(QualityTier::High, "Frame generation"));
        assert_eq!(Phase::N10.name(), "N10");
    }
}
