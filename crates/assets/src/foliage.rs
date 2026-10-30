//! Procedural foliage placement (ADR-0017 §2).
//!
//! [`generate_foliage`] is a pure, deterministic function of the `[foliage]` table, the
//! `[terrain]` table, the simulation's map ([`TerrainSite`]), the generated heightfield and the
//! `[[water]]` bodies; the client calls it at map bind and `content-check` can run the same
//! code. The only randomness is integer hash noise keyed by `foliage.seed ^ terrain.seed`.
//!
//! - **Scatter.** A jittered grid over the board at `density` clumps per 10 m², kept where a
//!   low-frequency noise mask allows (meadows and bare patches); a few clumps carry flowers.
//! - **Reeds** (`reeds = true`) line every water body's banks, `width / 2 + 0.2` to
//!   `width / 2 + 2` m from its centre line.
//! - **Patches** add clumps of their kind inside their disc.
//! - **Never** within the lane corridors (`lane_width / 2 + 1` m of a lane), 6 m of a
//!   structure, 10 m of a spawn, 2 m of a wall's face, or in water (`width / 2 + 0.5`; reeds
//!   `width / 2 + 0.2`), and never off the board.
//! - The result is decimated by a fixed stride to `max_instances` (at most
//!   [`MAX_FOLIAGE_INSTANCES`]); `density_scale` (a quality tier's `0..=1`) thins every source
//!   first.
//!
//! Foliage is decorative: the simulation has no brush areas, and nothing here reads or writes
//! simulation state.

use crate::map_visuals::{FoliageKind, MapFoliage, MapTerrain, MapWater};
use crate::terrain::{
    Heightfield, TerrainSite, WALL_THICKNESS, distance, fbm01, finite, finite_clamp, hash, lerp,
    polyline_distance, segment_distance, segments, smoothstep,
};

/// Most instances [`generate_foliage`] returns.
pub const MAX_FOLIAGE_INSTANCES: usize = 65_536;

/// Clear distance beyond a lane corridor (`lane_width / 2`), metres.
const LANE_CLEAR_PAD: f32 = 1.0;
/// Clear distance from a structure's centre, metres.
const STRUCTURE_CLEAR: f32 = 6.0;
/// Clear distance from a spawn, metres.
const SPAWN_CLEAR: f32 = 10.0;
/// Clear distance from a wall's face, metres.
const WALL_CLEAR: f32 = 2.0;
/// Clear distance beyond a water body's half width (reeds use their own band).
const WATER_CLEAR: f32 = 0.5;
/// Reed band beyond a water body's half width, metres.
const REED_BAND: (f32, f32) = (0.2, 2.0);
/// Reeds per square metre of bank band at full density.
const REED_DENSITY: f32 = 0.6;
/// Candidates generated before the stride decimation, per output instance at most.
const OVERSAMPLE: usize = 4;

/// One foliage clump.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct FoliageInstance {
    /// `[x, height, y]`: simulation `x`, the heightfield height there, simulation `y`.
    pub position: [f32; 3],
    /// Rotation about the vertical axis, radians, `0..2π`.
    pub yaw: f32,
    /// Clump height, metres.
    pub scale: f32,
    /// What it is.
    pub kind: FoliageKind,
    /// Brightness variation, `0.8..=1.2`.
    pub tint: f32,
}

/// Places the foliage clumps (module docs). `density_scale` is clamped to `0..=1` and
/// `max_instances` to [`MAX_FOLIAGE_INSTANCES`].
#[must_use]
#[allow(clippy::too_many_lines)] // Three sources (scatter, reeds, patches), in order.
pub fn generate_foliage(
    foliage: &MapFoliage,
    terrain: &MapTerrain,
    site: &TerrainSite,
    field: &Heightfield,
    water: &[MapWater],
    density_scale: f32,
    max_instances: usize,
) -> Vec<FoliageInstance> {
    let budget = max_instances.min(MAX_FOLIAGE_INSTANCES);
    let scale = finite_clamp(density_scale, 0.0, 1.0, 0.0);
    if budget == 0 || scale <= 0.0 {
        return Vec::new();
    }
    let extent = site_extent(site);
    let rules = Rules::new(terrain, site, water, extent);
    let seed = hash(
        foliage.seed.cast_signed(),
        terrain.seed.cast_signed(),
        0x0f01_1a6e,
    );
    let height = {
        let low = finite_clamp(foliage.height[0], 0.1, 1.5, 0.3);
        let high = finite_clamp(foliage.height[1], 0.1, 1.5, 0.6);
        [low.min(high), low.max(high)]
    };

    // Expected candidates per source, to thin them all alike when they would exceed the
    // oversampled budget (keeps the work bounded on any board size).
    let base_density = finite_clamp(foliage.density, 0.0, 8.0, 0.0) * scale;
    let base_count = extent * extent * base_density / 10.0;
    let reed_bodies: Vec<(Vec<[f32; 2]>, f32)> = if foliage.reeds {
        rules.water.clone()
    } else {
        Vec::new()
    };
    let reed_count: f32 = reed_bodies
        .iter()
        .map(|(points, half)| reed_area(points, *half) * REED_DENSITY * scale)
        .sum();
    let patches: Vec<_> = foliage
        .patches
        .iter()
        .filter(|patch| finite(patch.center))
        .map(|patch| {
            let radius = finite_clamp(patch.radius, 1.0, 60.0, 1.0);
            let density = finite_clamp(patch.density, 0.0, 20.0, 0.0) * scale;
            (
                patch,
                radius,
                core::f32::consts::PI * radius * radius * density / 10.0,
            )
        })
        .collect();
    let patch_count: f32 = patches.iter().map(|(_, _, count)| count).sum();
    let total = base_count + reed_count + patch_count;
    let limit = (budget * OVERSAMPLE) as f32;
    let thin = if total > limit { limit / total } else { 1.0 };

    let mut out = Vec::new();
    // Scatter: a jittered grid with one candidate per `10 / density` m².
    if base_density * thin > 0.0 {
        let cell = (10.0 / (base_density * thin)).sqrt();
        let side = ((extent / cell).ceil() as i32).clamp(1, 4096);
        for j in 0..side {
            for i in 0..side {
                let r = |k: u32| unit(hash(i, j, seed ^ k.wrapping_mul(0x9e37_79b9)));
                let x = (i as f32 + r(1)) * cell;
                let y = (j as f32 + r(2)) * cell;
                let mask = smoothstep(0.3, 0.6, fbm01(x / 40.0, y / 40.0, seed ^ 0x3c3c, 3));
                if r(3) >= mask || rules.excluded([x, y], WATER_CLEAR) {
                    continue;
                }
                let flowers = fbm01(x / 25.0, y / 25.0, seed ^ 0x7171, 2) > 0.68 && r(4) < 0.35;
                let kind = if flowers {
                    FoliageKind::Flowers
                } else {
                    FoliageKind::Grass
                };
                out.push(instance(
                    field,
                    [x, y],
                    kind,
                    lerp(height[0], height[1], r(5)),
                    r(6),
                    r(7),
                ));
            }
        }
    }
    // Reeds along the banks.
    for (body_index, (points, half)) in reed_bodies.iter().enumerate() {
        let body_seed = seed ^ (0x5eed_0000 + body_index as u32);
        let count = (reed_area(points, *half) * REED_DENSITY * scale * thin) as i32;
        let pieces = segments(points);
        let lengths: Vec<f32> = pieces.iter().map(|&(a, b)| distance(a, b)).collect();
        let length: f32 = lengths.iter().sum();
        for k in 0..count.max(0) {
            let r = |n: u32| unit(hash(k, n.cast_signed(), body_seed));
            let offset = half + lerp(REED_BAND.0, REED_BAND.1, r(1));
            let p = if length <= 1e-3 {
                // A pool: around the centre.
                let angle = r(2) * core::f32::consts::TAU;
                let centre = pieces.first().map_or([0.0, 0.0], |piece| piece.0);
                [
                    centre[0] + offset * angle.cos(),
                    centre[1] + offset * angle.sin(),
                ]
            } else {
                let mut along = r(2) * length;
                let mut chosen = 0;
                for (index, &piece) in lengths.iter().enumerate() {
                    chosen = index;
                    if along <= piece {
                        break;
                    }
                    along -= piece;
                }
                let (a, b) = pieces[chosen];
                let t = (along / lengths[chosen].max(1e-6)).clamp(0.0, 1.0);
                let dir = [
                    (b[0] - a[0]) / lengths[chosen].max(1e-6),
                    (b[1] - a[1]) / lengths[chosen].max(1e-6),
                ];
                let side = if r(3) < 0.5 { -1.0 } else { 1.0 };
                [
                    a[0] + t * (b[0] - a[0]) - side * dir[1] * offset,
                    a[1] + t * (b[1] - a[1]) + side * dir[0] * offset,
                ]
            };
            if rules.excluded(p, REED_BAND.0) {
                continue;
            }
            out.push(instance(
                field,
                p,
                FoliageKind::Reeds,
                lerp(0.9, 1.5, r(4)),
                r(5),
                r(6),
            ));
        }
    }
    // Authored patches.
    for (patch_index, &(patch, radius, count)) in patches.iter().enumerate() {
        let patch_seed = seed ^ (0x9a7c_0000 + patch_index as u32);
        let reeds = patch.kind == FoliageKind::Reeds;
        for k in 0..(count * thin) as i32 {
            let r = |n: u32| unit(hash(k, n.cast_signed(), patch_seed));
            let angle = r(1) * core::f32::consts::TAU;
            let d = radius * r(2).sqrt();
            let p = [
                patch.center[0] + d * angle.cos(),
                patch.center[1] + d * angle.sin(),
            ];
            if rules.excluded(p, if reeds { REED_BAND.0 } else { WATER_CLEAR }) {
                continue;
            }
            let size = match patch.kind {
                FoliageKind::Brush => lerp(0.9, 1.4, r(3)),
                FoliageKind::Reeds => lerp(0.9, 1.5, r(3)),
                _ => lerp(height[0], height[1], r(3)),
            };
            out.push(instance(field, p, patch.kind, size, r(4), r(5)));
        }
    }
    decimate(out, budget)
}

/// Keeps `budget` evenly strided instances of `all` (all of them when it fits).
fn decimate(all: Vec<FoliageInstance>, budget: usize) -> Vec<FoliageInstance> {
    if all.len() <= budget {
        return all;
    }
    let len = all.len();
    (0..budget).map(|k| all[k * len / budget]).collect()
}

fn instance(
    field: &Heightfield,
    p: [f32; 2],
    kind: FoliageKind,
    scale: f32,
    yaw: f32,
    tint: f32,
) -> FoliageInstance {
    FoliageInstance {
        position: [p[0], field.sample(p[0], p[1]), p[1]],
        yaw: yaw * core::f32::consts::TAU,
        scale,
        kind,
        tint: 0.8 + 0.4 * tint,
    }
}

/// A hash as `[0, 1)`.
fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// The board side, or 1 when it is not a positive finite number (as the terrain generator).
fn site_extent(site: &TerrainSite) -> f32 {
    if site.extent.is_finite() && site.extent > 0.0 {
        site.extent.min(65_536.0)
    } else {
        1.0
    }
}

/// Area of a body's two reed bands, square metres.
fn reed_area(points: &[[f32; 2]], half: f32) -> f32 {
    let band = REED_BAND.1 - REED_BAND.0;
    match points {
        [] => 0.0,
        [_] => {
            core::f32::consts::PI * ((half + REED_BAND.1).powi(2) - (half + REED_BAND.0).powi(2))
        }
        many => {
            let length: f32 = many.windows(2).map(|pair| distance(pair[0], pair[1])).sum();
            2.0 * band * length
        }
    }
}

/// The exclusion zones, with clamped radii.
struct Rules<'a> {
    site: &'a TerrainSite,
    extent: f32,
    lane_clear: f32,
    /// Finite centre points and half width per water body.
    water: Vec<(Vec<[f32; 2]>, f32)>,
}

impl<'a> Rules<'a> {
    fn new(terrain: &MapTerrain, site: &'a TerrainSite, water: &[MapWater], extent: f32) -> Self {
        let lane_half = 0.5
            * finite_clamp(
                terrain.lane_width,
                crate::map_visuals::LANE_WIDTH_RANGE.0,
                crate::map_visuals::LANE_WIDTH_RANGE.1,
                crate::map_visuals::DEFAULT_LANE_WIDTH,
            );
        Self {
            site,
            extent,
            lane_clear: lane_half + LANE_CLEAR_PAD,
            water: water
                .iter()
                .map(|body| (body.finite_points().collect(), body.half_width()))
                .filter(|(points, _): &(Vec<[f32; 2]>, f32)| !points.is_empty())
                .collect(),
        }
    }

    /// Whether `p` is off the board or inside an exclusion zone (`water_clear` beyond each
    /// body's half width).
    fn excluded(&self, p: [f32; 2], water_clear: f32) -> bool {
        let margin = 0.5;
        if !(margin..=self.extent - margin).contains(&p[0])
            || !(margin..=self.extent - margin).contains(&p[1])
        {
            return true;
        }
        self.site
            .lanes
            .iter()
            .any(|lane| polyline_distance(p, lane) < self.lane_clear)
            || self
                .site
                .structures
                .iter()
                .any(|&s| distance(p, s) < STRUCTURE_CLEAR)
            || self
                .site
                .spawns
                .iter()
                .any(|&s| distance(p, s) < SPAWN_CLEAR)
            || self
                .site
                .walls
                .iter()
                .any(|&(a, b)| segment_distance(p, a, b) < 0.5 * WALL_THICKNESS + WALL_CLEAR)
            || self
                .water
                .iter()
                .any(|(points, half)| polyline_distance(p, points) < half + water_clear)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_visuals::{FoliagePatch, TerrainLayer};
    use crate::terrain::generate_terrain_with_water;

    fn terrain() -> MapTerrain {
        MapTerrain::new(vec![TerrainLayer::new("grass"), TerrainLayer::new("path")])
    }

    fn site() -> TerrainSite {
        TerrainSite::new(256.0)
            .with_lane(vec![[52.0, 128.0], [204.0, 128.0]])
            .with_wall([150.0, 40.0], [150.0, 90.0])
            .with_structure([84.0, 128.0])
            .with_structure([60.0, 200.0])
            .with_spawn([24.0, 232.0])
            .with_spawn([232.0, 24.0])
    }

    fn river() -> MapWater {
        MapWater::new(vec![[30.0, 60.0], [128.0, 80.0], [226.0, 60.0]])
    }

    fn generate(
        foliage: &MapFoliage,
        water: &[MapWater],
        scale: f32,
        cap: usize,
    ) -> Vec<FoliageInstance> {
        let terrain = terrain();
        let site = site();
        let field = generate_terrain_with_water(&terrain, &site, water, 128, 16).heightfield;
        generate_foliage(foliage, &terrain, &site, &field, water, scale, cap)
    }

    #[test]
    fn placement_is_deterministic_and_keeps_every_clearance() {
        let water = [river()];
        let foliage = MapFoliage::default()
            .with_density(3.0)
            .with_patch(FoliagePatch::new([200.0, 200.0], 10.0, FoliageKind::Brush))
            .with_patch(FoliagePatch::new([100.0, 180.0], 8.0, FoliageKind::Flowers));
        let first = generate(&foliage, &water, 1.0, MAX_FOLIAGE_INSTANCES);
        assert_eq!(
            first,
            generate(&foliage, &water, 1.0, MAX_FOLIAGE_INSTANCES)
        );
        assert!(first.len() > 1_000, "{}", first.len());
        let site = site();
        let lane_clear = 0.5 * crate::map_visuals::DEFAULT_LANE_WIDTH + LANE_CLEAR_PAD;
        for clump in &first {
            let p = [clump.position[0], clump.position[2]];
            assert!((0.0..=256.0).contains(&p[0]) && (0.0..=256.0).contains(&p[1]));
            for lane in &site.lanes {
                assert!(polyline_distance(p, lane) >= lane_clear, "{p:?} on a lane");
            }
            for &s in &site.structures {
                assert!(distance(p, s) >= STRUCTURE_CLEAR, "{p:?} at a structure");
            }
            for &s in &site.spawns {
                assert!(distance(p, s) >= SPAWN_CLEAR, "{p:?} at a spawn");
            }
            for &(a, b) in &site.walls {
                assert!(segment_distance(p, a, b) >= 0.5 * WALL_THICKNESS + WALL_CLEAR);
            }
            let d = polyline_distance(p, &water[0].points);
            let half = 0.5 * water[0].width;
            match clump.kind {
                FoliageKind::Reeds => {
                    assert!(
                        d >= half + REED_BAND.0 - 1e-3 && d <= half + REED_BAND.1 + 1e-3,
                        "{d}"
                    );
                }
                _ => assert!(d >= half + WATER_CLEAR, "{p:?} in water"),
            }
            assert!((0.8..=1.2).contains(&clump.tint));
            assert!((0.0..core::f32::consts::TAU + 1e-4).contains(&clump.yaw));
            assert!(clump.scale > 0.0 && clump.scale <= 1.5);
            assert!(clump.position[1].abs() <= crate::terrain::MAX_PLAY_RELIEF + 1e-4);
        }
        for kind in FoliageKind::ALL {
            assert!(first.iter().any(|c| c.kind == kind), "{kind:?} missing");
        }
        let brush = first.iter().filter(|c| c.kind == FoliageKind::Brush);
        assert!(brush.clone().all(|c| (0.9..=1.4).contains(&c.scale)));
        // Without reeds there are none.
        let dry = generate(
            &foliage.clone().with_reeds(false),
            &water,
            1.0,
            MAX_FOLIAGE_INSTANCES,
        );
        assert!(dry.iter().all(|c| c.kind != FoliageKind::Reeds));
    }

    #[test]
    fn tiers_thin_and_cap_the_instances() {
        let foliage = MapFoliage::default().with_density(8.0);
        let full = generate(&foliage, &[], 1.0, MAX_FOLIAGE_INSTANCES);
        let low = generate(&foliage, &[], 0.3, MAX_FOLIAGE_INSTANCES);
        assert!(
            low.len() < full.len() / 2,
            "{} vs {}",
            low.len(),
            full.len()
        );
        let capped = generate(&foliage, &[], 1.0, 8_000);
        assert_eq!(capped.len(), 8_000.min(full.len()));
        assert_eq!(generate(&foliage, &[], 0.0, 1000).len(), 0);
        assert_eq!(generate(&foliage, &[], 1.0, 0).len(), 0);
        assert_eq!(
            generate(&foliage.clone().with_density(0.0), &[], 1.0, 1000).len(),
            0
        );
        // A different seed moves the clumps.
        let other = generate(
            &foliage.clone().with_seed(7),
            &[],
            1.0,
            MAX_FOLIAGE_INSTANCES,
        );
        assert_ne!(other, full);
    }

    #[test]
    fn hostile_values_stay_bounded() {
        let mut foliage = MapFoliage::default()
            .with_density(8.0)
            .with_patch(FoliagePatch::new([f32::NAN, 1.0], 10.0, FoliageKind::Grass))
            .with_patch(
                FoliagePatch::new([128.0, 200.0], f32::INFINITY, FoliageKind::Brush)
                    .with_density(1e30),
            );
        foliage.height = [f32::NAN, -3.0];
        let huge = TerrainSite::new(65_536.0);
        let terrain = terrain();
        let field = generate_terrain_with_water(&terrain, &TerrainSite::new(256.0), &[], 16, 16)
            .heightfield;
        let out = generate_foliage(&foliage, &terrain, &huge, &field, &[], 1.0, 4_096);
        assert!(out.len() <= 4_096);
        assert!(out.iter().all(|c| c.position.iter().all(|v| v.is_finite())
            && c.scale.is_finite()
            && c.scale > 0.0));
    }
}
