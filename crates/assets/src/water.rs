//! The water surface mesh (ADR-0017 §3).
//!
//! [`generate_water`] covers the heightfield cells each `[[water]]` body reaches with a flat
//! surface at the body's level, using **the terrain grid's own vertices and diagonal split**
//! (the renderer's `grid_mesh` rule: a cell is cut along the diagonal whose corners differ
//! less in height). The per-vertex bed depth (surface minus terrain height) therefore
//! interpolates across each triangle exactly like the terrain surface the GPU draws
//! underneath, so the shoreline (depth 0) lands where the water meets the drawn ground. The
//! depth is also clipped to `width / 2` minus the distance from the centre line, so relief
//! that dips below the surface beyond the carved bank stays dry instead of flooding in
//! cell-shaped steps. The shader draws nothing over dry ground (negative depth).
//!
//! Pure and deterministic; positions are simulation coordinates (`x`, height, `y`), like
//! [`crate::Heightfield`].

use crate::map_visuals::MapWater;
use crate::terrain::{Heightfield, bounds, segment_distance};

/// One water surface vertex.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct WaterVertex {
    /// `[x, level, y]`: simulation `x`, the body's surface height, simulation `y`.
    pub position: [f32; 3],
    /// Flow velocity (simulation `x`, `y`), metres per second: along the nearest centre-line
    /// segment, slowing towards the banks; zero for pools.
    pub flow: [f32; 2],
    /// Surface minus terrain height at this vertex, metres (negative over dry ground).
    pub depth: f32,
    /// Index of the body in the descriptor's `[[water]]` list.
    pub body: u32,
}

/// The water surface of every body, as one indexed triangle list.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct WaterMesh {
    /// Vertices; each body has its own (bodies at different levels never share one).
    pub vertices: Vec<WaterVertex>,
    /// Triangle indices into `vertices`, counter-clockwise seen from above like the terrain.
    pub indices: Vec<u32>,
}

impl WaterMesh {
    /// `true` when there is nothing to draw.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// Most vertices one map's water mesh may have: the renderer rejects a larger mesh
/// (`moba_render::MAX_WATER_VERTICES` is this), and the map consistency check
/// ([`crate::check_map_consistency`]) reports a map whose water exceeds it on any terrain
/// grid size, so a map never loses its water on one quality tier only.
pub const MAX_WATER_MESH_VERTICES: usize = 262_144;

/// Lowest flow speed factor on the banks (so the shoreline foam still drifts).
const BANK_FLOW: f32 = 0.15;

/// Builds the water mesh of `water` over `field` (the heightfield generated with these beds
/// carved, [`crate::generate_terrain_with_water`]). A cell is included when one of its
/// corners lies within half the body's width plus one cell of the centre line and at least
/// one corner is under the surface.
#[must_use]
pub fn generate_water(water: &[MapWater], field: &Heightfield) -> WaterMesh {
    let mut mesh = WaterMesh::default();
    let cells = field.resolution.saturating_sub(1);
    if cells == 0 || !(field.spacing.is_finite() && field.spacing > 0.0) {
        return mesh;
    }
    for (body_index, body) in water.iter().enumerate() {
        let points: Vec<[f32; 2]> = body.finite_points().collect();
        let Some((low, high)) = bounds(&points) else {
            continue;
        };
        let half = body.half_width();
        let level = body.surface();
        let flow = crate::terrain::finite_clamp(body.flow, 0.0, 8.0, 0.0);
        let reach = half + field.spacing;
        // Cell range covering the body's box grown by `reach`.
        let cell = |value: f32, axis: usize| ((value - field.origin[axis]) / field.spacing).floor();
        let x0 = cell(low[0] - reach, 0).max(0.0);
        let y0 = cell(low[1] - reach, 1).max(0.0);
        let x1 = cell(high[0] + reach, 0).min(cells as f32 - 1.0);
        let y1 = cell(high[1] + reach, 1).min(cells as f32 - 1.0);
        if !(x0 <= x1 && y0 <= y1) {
            continue;
        }
        let (x0, y0, x1, y1) = (x0 as u32, y0 as u32, x1 as u32, y1 as u32);
        // Sample-local vertex slots over the box's corners, `u32::MAX` for none yet.
        let side_x = (x1 - x0 + 2) as usize;
        let side_y = (y1 - y0 + 2) as usize;
        let mut slots = vec![u32::MAX; side_x * side_y];
        let mut near = vec![f32::NAN; side_x * side_y];
        let position = |ix: u32, iy: u32| {
            [
                field.origin[0] + ix as f32 * field.spacing,
                field.origin[1] + iy as f32 * field.spacing,
            ]
        };
        for iy in y0..=y1 + 1 {
            for ix in x0..=x1 + 1 {
                let slot = (iy - y0) as usize * side_x + (ix - x0) as usize;
                near[slot] = nearest(position(ix, iy), &points).0;
            }
        }
        // Water depth at a corner: the column above the terrain, but never past the body's
        // width. Beyond the carved bank the relief may dip below the surface again; clipping
        // by the distance keeps the shoreline at `width / 2` instead of flooding those cells.
        let depth_at = |(cx, cy): (u32, u32), d: f32| (level - field.height(cx, cy)).min(half - d);
        let body_id = u32::try_from(body_index).unwrap_or(u32::MAX);
        for iy in y0..=y1 {
            for ix in x0..=x1 {
                let corners = [(ix, iy), (ix + 1, iy), (ix + 1, iy + 1), (ix, iy + 1)];
                let slot = |(cx, cy): (u32, u32)| (cy - y0) as usize * side_x + (cx - x0) as usize;
                let close = corners.iter().any(|&c| near[slot(c)] <= reach);
                let wet = corners.iter().any(|&c| depth_at(c, near[slot(c)]) > 0.0);
                if !(close && wet) {
                    continue;
                }
                let mut index = [0u32; 4];
                for (k, &(cx, cy)) in corners.iter().enumerate() {
                    let s = slot((cx, cy));
                    if slots[s] == u32::MAX {
                        let at = position(cx, cy);
                        let (d, tangent) = nearest(at, &points);
                        let speed = flow * (1.0 - (d / half) * (d / half)).max(BANK_FLOW);
                        slots[s] = u32::try_from(mesh.vertices.len()).unwrap_or(u32::MAX);
                        mesh.vertices.push(WaterVertex {
                            position: [at[0], level, at[1]],
                            flow: [tangent[0] * speed, tangent[1] * speed],
                            depth: depth_at((cx, cy), d),
                            body: body_id,
                        });
                    }
                    index[k] = slots[s];
                }
                // The terrain grid's split: corners 00, 10, 11, 01.
                let [i00, i10, i11, i01] = index;
                let h = |(cx, cy): (u32, u32)| field.height(cx, cy);
                let (h00, h10, h11, h01) =
                    (h(corners[0]), h(corners[1]), h(corners[2]), h(corners[3]));
                if (h00 - h11).abs() <= (h10 - h01).abs() {
                    mesh.indices
                        .extend_from_slice(&[i00, i01, i11, i00, i11, i10]);
                } else {
                    mesh.indices
                        .extend_from_slice(&[i00, i01, i10, i10, i01, i11]);
                }
            }
        }
    }
    mesh
}

/// Distance to the nearest centre-line segment and that segment's unit direction (zero for a
/// pool or a zero-length segment).
fn nearest(p: [f32; 2], points: &[[f32; 2]]) -> (f32, [f32; 2]) {
    let mut best = (f32::INFINITY, [0.0, 0.0]);
    if let [single] = points {
        return (segment_distance(p, *single, *single), [0.0, 0.0]);
    }
    for pair in points.windows(2) {
        let d = segment_distance(p, pair[0], pair[1]);
        if d < best.0 {
            let along = [pair[1][0] - pair[0][0], pair[1][1] - pair[0][1]];
            let length = (along[0] * along[0] + along[1] * along[1]).sqrt();
            let tangent = if length > 1e-6 {
                [along[0] / length, along[1] / length]
            } else {
                [0.0, 0.0]
            };
            best = (d, tangent);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_visuals::{MapTerrain, TerrainLayer};
    use crate::terrain::{TerrainSite, generate_terrain_with_water};

    fn terrain() -> MapTerrain {
        MapTerrain::new(vec![TerrainLayer::new("grass"), TerrainLayer::new("path")])
    }

    fn river() -> MapWater {
        MapWater::new(vec![[40.0, 200.0], [128.0, 180.0], [200.0, 210.0]])
    }

    /// Bilinear-free check: the terrain grid's triangle containing `(x, y)`, interpolated
    /// with the given per-corner values (the same rule as the renderer's grid mesh).
    fn triangle_value(
        field: &Heightfield,
        values: impl Fn(u32, u32) -> f32,
        x: f32,
        y: f32,
    ) -> f32 {
        let gx = (x - field.origin[0]) / field.spacing;
        let gy = (y - field.origin[1]) / field.spacing;
        let (ix, iy) = (gx.floor() as u32, gy.floor() as u32);
        let (fx, fy) = (gx - ix as f32, gy - iy as f32);
        let (h00, h10, h11, h01) = (
            field.height(ix, iy),
            field.height(ix + 1, iy),
            field.height(ix + 1, iy + 1),
            field.height(ix, iy + 1),
        );
        let (v00, v10, v11, v01) = (
            values(ix, iy),
            values(ix + 1, iy),
            values(ix + 1, iy + 1),
            values(ix, iy + 1),
        );
        if (h00 - h11).abs() <= (h10 - h01).abs() {
            // Diagonal 00-11: triangles (00, 01, 11) above it and (00, 11, 10) below.
            if fy >= fx {
                v00 + fx * (v11 - v01) + fy * (v01 - v00)
            } else {
                v00 + fx * (v10 - v00) + fy * (v11 - v10)
            }
        } else if fx + fy <= 1.0 {
            // Diagonal 10-01: triangles (00, 01, 10) and (10, 01, 11).
            v00 + fx * (v10 - v00) + fy * (v01 - v00)
        } else {
            v11 + (1.0 - fx) * (v01 - v11) + (1.0 - fy) * (v10 - v11)
        }
    }

    #[test]
    fn the_mesh_shares_the_terrain_grid_and_its_depth_matches_the_drawn_bed() {
        let site = TerrainSite::new(256.0);
        let water = [river(), MapWater::new(vec![[60.0, 60.0]]).with_width(16.0)];
        let generated = generate_terrain_with_water(&terrain(), &site, &water, 192, 64);
        let field = &generated.heightfield;
        let mesh = generate_water(&water, field);
        assert!(!mesh.is_empty());
        assert_eq!(mesh.indices.len() % 3, 0);
        assert!(
            mesh.indices
                .iter()
                .all(|&i| (i as usize) < mesh.vertices.len())
        );
        for vertex in &mesh.vertices {
            let body = &water[vertex.body as usize];
            assert!((vertex.position[1] - body.level).abs() < 1e-6);
            // Every vertex is a heightfield sample: its depth is the surface minus that
            // sample's height.
            let gx = (vertex.position[0] - field.origin[0]) / field.spacing;
            let gy = (vertex.position[2] - field.origin[1]) / field.spacing;
            assert!((gx - gx.round()).abs() < 1e-3 && (gy - gy.round()).abs() < 1e-3);
            let h = field.height(gx.round() as u32, gy.round() as u32);
            let d = nearest(
                [vertex.position[0], vertex.position[2]],
                body.points.as_slice(),
            )
            .0;
            let expected = (body.level - h).min(body.half_width() - d);
            assert!((vertex.depth - expected).abs() < 1e-5);
            assert!(vertex.flow.iter().all(|f| f.is_finite()));
        }
        // The pool does not flow; the river does, along its centre line.
        assert!(
            mesh.vertices
                .iter()
                .filter(|v| v.body == 1)
                .all(|v| v.flow == [0.0, 0.0])
        );
        assert!(
            mesh.vertices
                .iter()
                .filter(|v| v.body == 0)
                .any(|v| v.flow[0] > 0.5)
        );
        // The depth interpolated on the shared triangles is the surface minus the drawn
        // terrain at any point: zero exactly where the drawn ground meets the surface.
        let level = water[0].level;
        for &(x, y) in &[
            (100.0, 186.0),
            (128.0, 183.5),
            (150.0, 191.2),
            (70.3, 196.0),
        ] {
            let ground = triangle_value(field, |ix, iy| field.height(ix, iy), x, y);
            let depth = triangle_value(field, |ix, iy| level - field.height(ix, iy), x, y);
            assert!((depth - (level - ground)).abs() < 1e-5, "{x}, {y}");
        }
        // The centre line is under the surface by about the bed depth.
        let centre = field.sample(128.0, 180.0);
        assert!(level - centre > 0.3, "{centre}");
    }

    /// Review fix: relief below the surface beyond the bank once flooded every mesh cell
    /// reaching it, in cell-shaped steps. The depth is clipped by the distance from the centre
    /// line, so the water ends at `width / 2` whatever the ground does.
    #[test]
    fn low_ground_beyond_the_bank_stays_dry() {
        let resolution = 65;
        let field = Heightfield {
            resolution,
            origin: [0.0, 0.0],
            spacing: 2.0,
            heights: vec![-0.4; (resolution * resolution) as usize],
            occlusion: vec![1.0; (resolution * resolution) as usize],
        };
        let body = MapWater::new(vec![[20.0, 64.0], [108.0, 64.0]]).with_width(10.0);
        let mesh = generate_water(core::slice::from_ref(&body), &field);
        assert!(!mesh.is_empty());
        for vertex in &mesh.vertices {
            let d = (vertex.position[2] - 64.0).abs();
            assert!(d <= 5.0 + 2.0 * field.spacing, "{vertex:?}");
            if d >= 5.0 {
                assert!(vertex.depth <= 0.0, "{vertex:?}");
            }
        }
    }

    #[test]
    fn no_water_no_mesh_and_hostile_bodies_stay_bounded() {
        let site = TerrainSite::new(256.0);
        let field = generate_terrain_with_water(&terrain(), &site, &[], 128, 16).heightfield;
        assert!(generate_water(&[], &field).is_empty());
        let hostile = [
            MapWater::new(Vec::new()),
            MapWater::new(vec![[f32::NAN, 3.0]]),
            MapWater::new(vec![[1e9, 1e9]]),
            MapWater::new(vec![[10.0, 10.0]])
                .with_width(f32::INFINITY)
                .with_level(f32::NAN, -4.0)
                .with_flow(f32::NAN),
        ];
        let mesh = generate_water(&hostile, &field);
        assert!(
            mesh.vertices
                .iter()
                .all(|v| v.position.iter().all(|c| c.is_finite())
                    && v.flow.iter().all(|c| c.is_finite())
                    && v.depth.is_finite())
        );
        // Deterministic.
        assert_eq!(
            generate_water(&[river()], &field),
            generate_water(&[river()], &field)
        );
    }
}
