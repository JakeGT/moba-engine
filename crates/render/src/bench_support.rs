//! Device-free entry points for the renderer's benchmarks (`benches/`). Not public API:
//! hidden from the docs and free to change.

use crate::lights::{self, CELL_WORDS, CellCounts, GRID, LIGHT_VEC4S, LightStats, PointLight};
use moba_core::math::Mat4;

/// The CPU half of [`Renderer`](crate::Renderer)'s light upload (footprint and binning)
/// over heap buffers shaped like the per-frame light list and grid. Heap memory is cached,
/// so this measures the binning itself, not the write-combined mapped memory it writes in a
/// real frame (which the binning only writes, never reads).
#[derive(Debug)]
pub struct LightBinning {
    list: Vec<[f32; 4]>,
    grid: Vec<u32>,
    counts: CellCounts,
}

impl LightBinning {
    /// Buffers for [`MAX_LIGHTS`](crate::MAX_LIGHTS) lights and the full grid.
    #[must_use]
    pub fn new() -> Self {
        Self {
            list: vec![[0.0; 4]; crate::MAX_LIGHTS * LIGHT_VEC4S],
            grid: vec![0; GRID * GRID * CELL_WORDS],
            counts: [0; GRID * GRID],
        }
    }

    /// Fits the grid to `view_proj` and bins `lights` with `max_lights` and `per_cell` caps.
    pub fn bin(
        &mut self,
        lights: &[PointLight],
        view_proj: Mat4,
        max_lights: usize,
        per_cell: usize,
    ) -> LightStats {
        let footprint = lights::ground_footprint(view_proj);
        lights::bin(
            lights,
            footprint,
            max_lights,
            per_cell,
            &mut self.list,
            &mut self.grid,
            &mut self.counts,
        )
        .1
    }
}

impl Default for LightBinning {
    fn default() -> Self {
        Self::new()
    }
}

/// The CPU half of the temporal path's per-unit motion work (ADR-0021 §2c): each frame, for
/// every keyed unit, the previous drawn state is looked up (`MotionHistory::previous`), its
/// previous-instance entry built and its previous joint palette copied out, and then what it
/// drew is recorded (`MotionHistory::record`). The destinations are heap buffers shaped like
/// the mapped previous-instance and previous-bone buffers.
pub struct MotionUpload {
    history: crate::motion::MotionHistory,
    instances: Vec<crate::passes::scene::PrevInstance>,
    bones: Vec<moba_assets::JointMatrix>,
}

impl core::fmt::Debug for MotionUpload {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MotionUpload")
            .field("instances", &self.instances.len())
            .field("bones", &self.bones.len())
            .finish_non_exhaustive()
    }
}

/// One keyed unit as the scene hands it over: key, transform and joint palette.
#[derive(Clone, Debug)]
pub struct MotionUnit {
    /// The motion key (non-zero).
    pub key: u32,
    /// Its transform this frame.
    pub transform: Mat4,
    /// Its joint matrices this frame (empty when unskinned).
    pub joints: Vec<moba_assets::JointMatrix>,
}

impl MotionUpload {
    /// Room for `units` instances and `max_joints` joint matrices per frame.
    #[must_use]
    pub fn new(units: usize, max_joints: usize) -> Self {
        Self {
            history: crate::motion::MotionHistory::new(max_joints),
            instances: vec![crate::passes::scene::PrevInstance::new(None); units],
            bones: vec![[[0.0; 4]; 3]; max_joints],
        }
    }

    /// One frame: look up and upload every unit's previous state, then record this frame's.
    /// Returns how many units had a previous state.
    pub fn frame(&mut self, units: &[MotionUnit]) -> usize {
        self.history.begin_frame(false);
        let mut cursor = 0;
        let mut found = 0;
        for (instance, unit) in self.instances.iter_mut().zip(units) {
            let drawn = self.history.previous(unit.key, false, &unit.transform);
            *instance = crate::passes::scene::PrevInstance::new(drawn.as_ref().map(|(s, _)| s));
            if let Some((_, palette)) = drawn {
                found += 1;
                let count = palette.len().min(self.bones.len() - cursor);
                self.bones[cursor..cursor + count].copy_from_slice(&palette[..count]);
                cursor += count;
            }
        }
        let pose = moba_assets::VatPose::single(moba_assets::VatSample::new(0, 0, 0.0));
        for unit in units {
            self.history
                .record(unit.key, unit.transform, pose, &unit.joints);
        }
        found
    }
}
