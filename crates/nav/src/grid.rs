//! Deterministic fixed-cell spatial grid.
//!
//! Items are appended in insertion order and counting-sorted by cell on the next query.
//! Query order is row-major cell order followed by insertion order, matching the historical
//! `Vec<Vec<T>>` layout without per-cell allocations.

use core::cell::RefCell;

use moba_core::{Fix, FixVec2};

/// A uniform spatial grid over a fixed world extent.
#[derive(Clone, Debug)]
pub struct SpatialGrid<T> {
    origin: FixVec2,
    inv_cell_size: Fix,
    cols: u32,
    rows: u32,
    items: Vec<(FixVec2, T)>,
    cell_index: Vec<u32>,
    sort: RefCell<SortBuffers>,
}

#[derive(Clone, Debug, Default)]
struct SortBuffers {
    dirty: bool,
    sorted: Vec<u32>,
    starts: Vec<u32>,
    counts: Vec<u32>,
    offsets: Vec<u32>,
}

impl<T> SpatialGrid<T> {
    /// Creates a grid covering `[origin, origin + extent)`.
    ///
    /// # Panics
    /// Panics if `cell_size <= 0` or `extent` has a non-positive axis.
    #[must_use]
    pub fn new(origin: FixVec2, extent: FixVec2, cell_size: Fix) -> Self {
        assert!(cell_size > Fix::ZERO, "cell size must be positive");
        assert!(
            extent.x > Fix::ZERO && extent.y > Fix::ZERO,
            "extent must be positive"
        );
        let cols = cell_count(extent.x, cell_size);
        let rows = cell_count(extent.y, cell_size);
        let cell_total = usize::try_from(cols * rows).expect("grid cells fit usize");
        Self {
            origin,
            inv_cell_size: Fix::ONE / cell_size,
            cols,
            rows,
            items: Vec::with_capacity(cell_total.min(256)),
            cell_index: Vec::with_capacity(cell_total.min(256)),
            sort: RefCell::new(SortBuffers {
                dirty: false,
                sorted: Vec::with_capacity(cell_total.min(256)),
                starts: vec![0; cell_total + 1],
                counts: vec![0; cell_total],
                offsets: vec![0; cell_total + 1],
            }),
        }
    }

    /// Removes all items while keeping the allocations.
    pub fn clear(&mut self) {
        self.items.clear();
        self.cell_index.clear();
        let sort = self.sort.get_mut();
        sort.dirty = false;
        sort.sorted.clear();
        sort.starts.fill(0);
        sort.counts.fill(0);
        sort.offsets.fill(0);
    }

    /// Number of cells along each axis.
    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.cols, self.rows)
    }

    /// Cell coordinates for a world position, clamped to the grid.
    #[must_use]
    pub fn cell_of(&self, position: FixVec2) -> (u32, u32) {
        let x = ((position.x - self.origin.x) * self.inv_cell_size).to_i64();
        let y = ((position.y - self.origin.y) * self.inv_cell_size).to_i64();
        (
            x.clamp(0, i64::from(self.cols) - 1) as u32,
            y.clamp(0, i64::from(self.rows) - 1) as u32,
        )
    }

    /// Inserts an item at a world position.
    pub fn insert(&mut self, position: FixVec2, item: T) {
        let (x, y) = self.cell_of(position);
        self.items.push((position, item));
        self.cell_index.push(y * self.cols + x);
        self.sort.get_mut().dirty = true;
    }

    /// Calls `visit` for every item whose centre is within `radius` of `center`.
    ///
    /// Visit order is deterministic: row-major cells, insertion order inside a cell.
    pub fn for_each_in_circle(&self, center: FixVec2, radius: Fix, mut visit: impl FnMut(T))
    where
        T: Copy,
    {
        self.ensure_sorted();
        let sort = self.sort.borrow();
        let radius_sq = radius * radius;
        let min = self.cell_of(center - FixVec2::splat(radius));
        let max = self.cell_of(center + FixVec2::splat(radius));
        for y in min.1..=max.1 {
            for x in min.0..=max.0 {
                let cell = usize::try_from(y * self.cols + x).expect("cell index fits usize");
                let start = sort.starts[cell] as usize;
                let end = sort.starts[cell + 1] as usize;
                for &index in &sort.sorted[start..end] {
                    let (position, item) = self.items[index as usize];
                    if position.distance_sq(center) <= radius_sq {
                        visit(item);
                    }
                }
            }
        }
    }

    /// Returns true when `visit` accepts any item within `radius` of `center`.
    pub fn any_in_circle(
        &self,
        center: FixVec2,
        radius: Fix,
        mut visit: impl FnMut(T) -> bool,
    ) -> bool
    where
        T: Copy,
    {
        self.ensure_sorted();
        let sort = self.sort.borrow();
        let radius_sq = radius * radius;
        let min = self.cell_of(center - FixVec2::splat(radius));
        let max = self.cell_of(center + FixVec2::splat(radius));
        for y in min.1..=max.1 {
            for x in min.0..=max.0 {
                let cell = usize::try_from(y * self.cols + x).expect("cell index fits usize");
                let start = sort.starts[cell] as usize;
                let end = sort.starts[cell + 1] as usize;
                for &index in &sort.sorted[start..end] {
                    let (position, item) = self.items[index as usize];
                    if position.distance_sq(center) <= radius_sq && visit(item) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn ensure_sorted(&self) {
        if !self.sort.borrow().dirty {
            return;
        }
        let mut sort = self.sort.borrow_mut();
        sort.sorted.clear();
        sort.counts.fill(0);
        for &cell in &self.cell_index {
            sort.counts[cell as usize] += 1;
        }
        let mut running = 0u32;
        for index in 0..sort.counts.len() {
            let count = sort.counts[index];
            sort.starts[index] = running;
            running += count;
        }
        let cell_total = sort.starts.len() - 1;
        sort.starts[cell_total] = running;
        sort.sorted.resize(usize::try_from(running).unwrap_or(0), 0);
        // Stable counting sort by cell index preserves insertion order within a cell.
        {
            let SortBuffers {
                offsets, starts, ..
            } = &mut *sort;
            offsets.copy_from_slice(starts);
        }
        for (item, &cell) in self.cell_index.iter().enumerate() {
            let slot = sort.offsets[cell as usize] as usize;
            sort.sorted[slot] = u32::try_from(item).expect("grid item index fits u32");
            sort.offsets[cell as usize] += 1;
        }
        sort.dirty = false;
    }
}

fn cell_count(extent: Fix, cell_size: Fix) -> u32 {
    let count = ((extent + cell_size - Fix::EPSILON) / cell_size).to_i64();
    u32::try_from(count.max(1)).expect("grid dimension fits u32")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> SpatialGrid<u32> {
        SpatialGrid::new(
            FixVec2::ZERO,
            FixVec2::splat(Fix::from_i32(64)),
            Fix::from_i32(8),
        )
    }

    #[test]
    fn cells_cover_positions_and_clamp_outside() {
        let grid = grid();
        assert_eq!(grid.dimensions(), (8, 8));
        assert_eq!(grid.cell_of(FixVec2::ZERO), (0, 0));
        assert_eq!(grid.cell_of(FixVec2::from_ints(8, 8)), (1, 1));
        assert_eq!(grid.cell_of(FixVec2::from_ints(63, 63)), (7, 7));
        assert_eq!(grid.cell_of(FixVec2::from_ints(-5, 100)), (0, 7));
    }

    #[test]
    fn queries_return_items_from_overlapping_cells() {
        let mut grid = grid();
        grid.insert(FixVec2::from_ints(10, 10), 1);
        grid.insert(FixVec2::from_ints(12, 12), 2);
        grid.insert(FixVec2::from_ints(40, 40), 3);

        let mut found = Vec::new();
        grid.for_each_in_circle(FixVec2::from_ints(11, 11), Fix::from_i32(4), |item| {
            found.push(item);
        });
        found.sort_unstable();
        assert_eq!(found, vec![1, 2]);

        grid.clear();
        let mut found = Vec::new();
        grid.for_each_in_circle(FixVec2::from_ints(11, 11), Fix::from_i32(4), |item| {
            found.push(item);
        });
        assert_eq!(found.len(), 0);
    }

    #[test]
    fn cell_count_rounds_up() {
        assert_eq!(cell_count(Fix::from_i32(64), Fix::from_i32(8)), 8);
        assert_eq!(cell_count(Fix::from_i32(65), Fix::from_i32(8)), 9);
        assert_eq!(cell_count(Fix::from_i32(1), Fix::from_i32(8)), 1);
    }
}
