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
