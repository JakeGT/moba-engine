//! Per-tier asset and memory budgets (R8, ADR-0018 §4).
//!
//! One table shared by the renderer (VRAM accounting, texture degradation, the finest mesh
//! level a tier draws) and `content-check` (the per-tier budget report), so a pack that the
//! checker passes is the pack the client loads without degrading. The tiers are indexed
//! Low, Medium, High, Ultra (`moba_render::QualityTier` maps onto [`TIER_BUDGETS`] by name).

use crate::mesh_lod::MeshLod;

/// One tier's budgets.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct TierBudget {
    /// Tier name (`low`, `medium`, `high`, `ultra`).
    pub name: &'static str,
    /// Every device allocation the renderer makes: textures, render targets and buffers.
    pub total_bytes: u64,
    /// Textures uploaded from assets (materials, terrain layers, VAT, environment, LUTs, AO).
    pub texture_bytes: u64,
    /// Triangles of one hero or unit model as drawn at full detail on this tier.
    pub model_triangles: usize,
    /// Joints of one skinned model.
    pub model_joints: usize,
    /// Texture bytes one model's material may reference (every mip level).
    pub model_texture_bytes: u64,
    /// Mesh LOD selection tolerance: the largest projected error, in pixels, a level may have.
    pub lod_tolerance_px: f32,
}

const MIB: u64 = 1024 * 1024;

/// Low, Medium, High, Ultra (ADR-0018 §4).
pub const TIER_BUDGETS: [TierBudget; 4] = [
    TierBudget {
        name: "low",
        total_bytes: 1024 * MIB,
        texture_bytes: 384 * MIB,
        model_triangles: 8_000,
        model_joints: 32,
        model_texture_bytes: 4 * MIB,
        lod_tolerance_px: 2.0,
    },
    TierBudget {
        name: "medium",
        total_bytes: 2048 * MIB,
        texture_bytes: 1024 * MIB,
        model_triangles: 16_000,
        model_joints: 48,
        model_texture_bytes: 16 * MIB,
        lod_tolerance_px: 1.0,
    },
    TierBudget {
        name: "high",
        total_bytes: 4096 * MIB,
        texture_bytes: 2560 * MIB,
        model_triangles: 32_000,
        model_joints: 64,
        model_texture_bytes: 32 * MIB,
        lod_tolerance_px: 0.75,
    },
    TierBudget {
        name: "ultra",
        total_bytes: 6144 * MIB,
        texture_bytes: 4096 * MIB,
        model_triangles: 64_000,
        model_joints: 64,
        model_texture_bytes: 64 * MIB,
        lod_tolerance_px: 0.5,
    },
];

const GIB: u64 = 1024 * MIB;

/// One tier's GPU budget on the tiered path (ADR-0020 §5b, roadmap §3), measured as the p95
/// GPU time of the teamfight replay with frame generation off.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct GpuBudget {
    /// Tier name (`low` … `overdrive`).
    pub name: &'static str,
    /// The reference GPU class the budget is for.
    pub reference: &'static str,
    /// Output height the tier targets (1080 or 1440).
    pub output_height: u32,
    /// Target frame rate at p95, frame generation off.
    pub target_fps: u32,
    /// GPU time per frame, milliseconds, p95.
    pub budget_ms: f32,
    /// Dynamic resolution's `(floor, ceiling)` at the tier's output (§5c): the ceiling is the
    /// preset's upscaler mode, the floor two modes down (or one, below Performance).
    pub scale_range: (f32, f32),
    /// The range at 4K output, where it differs (Ultra uses Performance at 4K).
    pub scale_range_4k: Option<(f32, f32)>,
    /// Device memory budget (§5e): at most 75 % of the smallest card in the class.
    pub vram_bytes: u64,
}

impl GpuBudget {
    /// Dynamic resolution's `(floor, ceiling)` for an output `height` pixels tall.
    #[must_use]
    pub fn scale_range_at(&self, height: u32) -> (f32, f32) {
        match self.scale_range_4k {
            Some(range) if height >= 2160 => range,
            _ => self.scale_range,
        }
    }
}

/// Quality (0.67), Performance (0.50) and Ultra Performance (0.33) render scales.
const QUALITY: f32 = 0.67;
const PERFORMANCE: f32 = 0.5;
const ULTRA_PERFORMANCE: f32 = 0.33;

/// The tiered path's budgets, Low first (ADR-0020 §5b, provisional until the reference GPUs
/// are measured, #9/#127). `perf-gate` and `moba-render` both read this table. Ultra's 9 ms
/// is longer than its 120 fps frame (8.33 ms), as in the roadmap; it awaits the maintainer.
pub const GPU_BUDGETS: [GpuBudget; 5] = [
    GpuBudget {
        name: "low",
        reference: "RTX 2060, RX 6600",
        output_height: 1080,
        target_fps: 60,
        budget_ms: 12.0,
        scale_range: (PERFORMANCE, QUALITY),
        scale_range_4k: None,
        vram_bytes: 4 * GIB,
    },
    GpuBudget {
        name: "medium",
        reference: "RTX 3060, RX 6700 XT",
        output_height: 1080,
        target_fps: 144,
        budget_ms: 6.0,
        scale_range: (PERFORMANCE, QUALITY),
        scale_range_4k: None,
        vram_bytes: 6 * GIB,
    },
    GpuBudget {
        name: "high",
        reference: "RTX 4060 Ti / 4070, RX 7700 XT",
        output_height: 1440,
        target_fps: 120,
        budget_ms: 7.5,
        scale_range: (PERFORMANCE, QUALITY),
        scale_range_4k: None,
        vram_bytes: 6 * GIB,
    },
    GpuBudget {
        name: "ultra",
        reference: "RTX 4070 Ti Super / 5070 Ti, RX 9070 XT",
        output_height: 1440,
        target_fps: 120,
        budget_ms: 9.0,
        scale_range: (PERFORMANCE, QUALITY),
        scale_range_4k: Some((ULTRA_PERFORMANCE, PERFORMANCE)),
        vram_bytes: 12 * GIB,
    },
    GpuBudget {
        name: "overdrive",
        reference: "RTX 4080 / 5080 and up",
        output_height: 1440,
        target_fps: 60,
        budget_ms: 15.0,
        scale_range: (ULTRA_PERFORMANCE, PERFORMANCE),
        scale_range_4k: None,
        vram_bytes: 12 * GIB,
    },
];

/// The Compat path's GPU budgets, today's (ms, p95; ADR-0020 §2d), Low first. They keep
/// failing `perf-gate`.
pub const COMPAT_GPU_BUDGETS: [(&str, f32); 4] = [
    ("low", 14.0),
    ("medium", 6.0),
    ("high", 6.0),
    ("ultra", 7.5),
];

/// The tiered budget of the tier named `name` (ASCII case-insensitive).
#[must_use]
pub fn gpu_budget(name: &str) -> Option<&'static GpuBudget> {
    GPU_BUDGETS
        .iter()
        .find(|budget| budget.name.eq_ignore_ascii_case(name.trim()))
}

/// The Compat GPU budget, ms, of the tier named `name` (ASCII case-insensitive).
#[must_use]
pub fn compat_gpu_budget_ms(name: &str) -> Option<f32> {
    COMPAT_GPU_BUDGETS
        .iter()
        .find(|(tier, _)| tier.eq_ignore_ascii_case(name.trim()))
        .map(|&(_, budget)| budget)
}

/// The budget of the tier named `name` (ASCII case-insensitive).
#[must_use]
pub fn tier_budget(name: &str) -> Option<&'static TierBudget> {
    TIER_BUDGETS
        .iter()
        .find(|budget| budget.name.eq_ignore_ascii_case(name.trim()))
}

/// The finest level a tier with `budget` triangles draws: `Some(0)` when LOD0 fits, the first
/// baked level that fits (`Some(1)` is LOD1), or `None` when nothing fits (then the renderer
/// draws the coarsest level it has).
#[must_use]
pub fn finest_level_within(
    lod0_triangles: usize,
    lods: &[MeshLod],
    budget: usize,
) -> Option<usize> {
    if lod0_triangles <= budget {
        return Some(0);
    }
    lods.iter()
        .position(|level| level.triangle_count() <= budget)
        .map(|index| index + 1)
}

/// How one model fares on one tier ([`model_report`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TierFit {
    /// LOD0 is within every budget.
    Fits,
    /// LOD0 is over the triangle budget but this baked level fits; the tier draws it as its
    /// finest level.
    Degrades {
        /// The finest level drawn (1 = LOD1).
        level: usize,
    },
    /// Over a budget with no level to fall back to (triangles), or over the joint or texture
    /// budget (which no mesh level fixes).
    Over,
}

/// A model's numbers against every tier.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ModelBudgetReport {
    /// Triangles of LOD0 then of each baked level.
    pub triangles: Vec<usize>,
    /// Skin joints (0 for a static model).
    pub joints: usize,
    /// Texture bytes its material references.
    pub texture_bytes: u64,
    /// Per tier, Low first.
    pub tiers: [TierFit; 4],
    /// Why the model's `.lod` file was not used (stale: baked from another version of the
    /// model, or malformed); `None` when it loaded or there is none. The model then draws
    /// LOD0 only, and the tiers above count no baked levels.
    pub lod_problem: Option<String>,
}

/// Checks one model (LOD0 triangles, baked levels, joints, material texture bytes) against
/// every tier.
#[must_use]
pub fn model_report(
    lod0_triangles: usize,
    lods: &[MeshLod],
    joints: usize,
    texture_bytes: u64,
) -> ModelBudgetReport {
    let mut triangles = Vec::with_capacity(1 + lods.len());
    triangles.push(lod0_triangles);
    triangles.extend(lods.iter().map(MeshLod::triangle_count));
    let tiers = core::array::from_fn(|tier| {
        let budget = &TIER_BUDGETS[tier];
        if joints > budget.model_joints || texture_bytes > budget.model_texture_bytes {
            return TierFit::Over;
        }
        match finest_level_within(lod0_triangles, lods, budget.model_triangles) {
            Some(0) => TierFit::Fits,
            Some(level) => TierFit::Degrades { level },
            None => TierFit::Over,
        }
    });
    ModelBudgetReport {
        triangles,
        joints,
        texture_bytes,
        tiers,
        lod_problem: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MeshData, MeshLods};

    fn strip(triangles: u32) -> MeshData {
        let positions: Vec<[f32; 3]> = (0..triangles + 2)
            .map(|i| [i as f32, (i % 2) as f32, 0.0])
            .collect();
        let normals = vec![[0.0, 0.0, 1.0]; positions.len()];
        let indices = (0..triangles).flat_map(|i| [i, i + 1, i + 2]).collect();
        MeshData::new(positions, normals, indices).expect("strip")
    }

    #[test]
    fn budgets_grow_with_the_tier_and_tolerances_tighten() {
        for pair in TIER_BUDGETS.windows(2) {
            assert!(pair[1].total_bytes > pair[0].total_bytes);
            assert!(pair[1].texture_bytes > pair[0].texture_bytes);
            assert!(pair[1].texture_bytes < pair[1].total_bytes);
            assert!(pair[1].model_triangles >= pair[0].model_triangles);
            assert!(pair[1].lod_tolerance_px < pair[0].lod_tolerance_px);
        }
        assert_eq!(
            tier_budget(" HIGH ").map(|budget| budget.name),
            Some("high")
        );
        assert!(tier_budget("epic").is_none());
    }

    #[test]
    fn gpu_budgets_are_ordered_and_follow_the_scale_rules() {
        let names: Vec<_> = GPU_BUDGETS.iter().map(|budget| budget.name).collect();
        assert_eq!(names, ["low", "medium", "high", "ultra", "overdrive"]);
        // Each tier above Medium gets more GPU time per frame, and more memory (or as much).
        for pair in GPU_BUDGETS[1..].windows(2) {
            assert!(pair[1].budget_ms > pair[0].budget_ms, "{}", pair[1].name);
        }
        for pair in GPU_BUDGETS.windows(2) {
            assert!(pair[1].vram_bytes >= pair[0].vram_bytes, "{}", pair[1].name);
        }
        for budget in &GPU_BUDGETS {
            // The budget fits the target frame rate, except Ultra: the roadmap gives it 9 ms
            // at 120 fps (8.33 ms a frame). That is a recorded question for the maintainer
            // (N0 report); the table follows the accepted ADR until it is answered.
            let frame_ms = 1000.0 / budget.target_fps as f32;
            assert_eq!(
                budget.budget_ms <= frame_ms,
                budget.name != "ultra",
                "{}",
                budget.name
            );
            let (floor, ceiling) = budget.scale_range;
            assert!(
                0.0 < floor && floor < ceiling && ceiling < 1.0,
                "{}",
                budget.name
            );
            assert_eq!(budget.scale_range_at(1080), budget.scale_range);
        }
        assert_eq!(
            gpu_budget("ultra").map(|b| b.scale_range_at(2160)),
            Some((0.33, 0.5))
        );
        assert_eq!(
            gpu_budget(" Overdrive ").map(|b| b.scale_range),
            Some((0.33, 0.5))
        );
        assert_eq!(compat_gpu_budget_ms("LOW"), Some(14.0));
        assert!(gpu_budget("epic").is_none() && compat_gpu_budget_ms("overdrive").is_none());
    }

    /// The table is the roadmap's §3 table (GPU budget and target), so neither drifts.
    #[test]
    fn gpu_budgets_match_the_roadmap() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/plans/next-gen-graphics-roadmap.md"
        );
        let Ok(roadmap) = std::fs::read_to_string(path) else {
            return; // Outside the repository (a packaged crate): nothing to compare.
        };
        let section = roadmap
            .split("## 3. The new tier scale")
            .nth(1)
            .and_then(|rest| rest.split("### Feature matrix").next())
            .expect("roadmap §3");
        let rows: Vec<Vec<&str>> = section
            .lines()
            .filter(|line| line.starts_with("| ") && !line.starts_with("| Tier"))
            .map(|line| line.trim_matches('|').split('|').map(str::trim).collect())
            .collect();
        assert_eq!(rows.len(), GPU_BUDGETS.len());
        for (row, budget) in rows.iter().zip(&GPU_BUDGETS) {
            assert!(
                row[0].to_ascii_lowercase().starts_with(budget.name),
                "{row:?}"
            );
            let number = |cell: &str| -> f32 {
                let digits: String = cell
                    .chars()
                    .skip_while(|c| !c.is_ascii_digit())
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect();
                digits.parse().expect("a number")
            };
            assert_eq!(number(row[5]), budget.budget_ms, "{}: budget", budget.name);
            assert_eq!(
                number(row[4]) as u32,
                budget.target_fps,
                "{}: target",
                budget.name
            );
        }
    }

    #[test]
    fn a_model_over_a_tier_degrades_to_the_first_level_that_fits() {
        let mesh = strip(10_000);
        let level = |triangles: u32| {
            (
                0.01,
                (0..triangles).flat_map(|i| [i, i + 1, i + 2]).collect(),
            )
        };
        let lods = MeshLods::new(&mesh, vec![level(5_000), level(2_500)]).expect("levels");
        let report = model_report(10_000, lods.levels(), 0, 0);
        assert_eq!(report.triangles, [10_000, 5_000, 2_500]);
        assert_eq!(
            report.tiers[0],
            TierFit::Degrades { level: 1 },
            "Low draws LOD1"
        );
        assert_eq!(report.tiers[1], TierFit::Fits);
        assert_eq!(finest_level_within(10_000, &[], 8_000), None);
        let report = model_report(10_000, &[], 64, 0);
        assert_eq!(
            report.tiers[0],
            TierFit::Over,
            "no level, and too many joints"
        );
        assert_eq!(report.tiers[3], TierFit::Fits);
        let textured = model_report(100, &[], 0, 8 * MIB);
        assert_eq!(
            textured.tiers[0],
            TierFit::Over,
            "textures are over Low's 4 MiB"
        );
        assert_eq!(textured.tiers[1], TierFit::Fits);
    }
}
