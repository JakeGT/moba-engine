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
