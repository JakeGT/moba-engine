//! Procedural terrain for a map visuals descriptor's `[terrain]` table (ADR-0014).
//!
//! Everything here is a pure, deterministic function of the descriptor ([`MapTerrain`]) and
//! the simulation's map facts ([`TerrainSite`]): the client builds its terrain mesh and splat
//! map with it at map bind, and `content-check` runs the same code to enforce the
//! visual-versus-simulation rules ([`check_map_consistency`]). Nothing here is hashed, sent
//! or fed back into the simulation. The only randomness is integer hash noise keyed by
//! [`MapTerrain::seed`].
//!
//! # Coordinates
//! Simulation coordinates: the origin is the map corner, `x` to the right and `y` along the
//! renderer's `+z` ("down" on the minimap); the walkable board is `[0, extent]²`. The
//! heightfield and splat cover the board plus a margin of [`terrain_margin`] on every side.
//!
//! # Heightfield rules ([`generate_heightfield`])
//! - **Inside the board** smooth fbm relief in `[-relief, relief]`, with `relief` clamped to
//!   [`MAX_PLAY_RELIEF`] (units stand at height 0; half a metre never reads as a ramp, ridge
//!   or pit). The relief tapers to 0 over the last 2 m before the board edge, so the edge is
//!   continuous.
//! - **Outside the board** the ground only rises. With `d` the Euclidean distance to the board
//!   rectangle: [`EdgeStyle::None`] is a flat apron with tiny non-negative noise;
//!   [`EdgeStyle::Hills`] is `edge_height × smoothstep(0, 0.35·margin, d) × (0.7 + 0.3·fbm)`,
//!   rolling; [`EdgeStyle::Cliffs`] rises steeply over `smoothstep(2, 14, d)` with terraced,
//!   strata-like noise, up to `edge_height`. Both warp `d` by a low-frequency noise factor
//!   (0.8..=1.2 for hills, 0.75..=1.25 for cliffs) so the rise does not follow the board
//!   outline exactly. `d` is measured from the end of a flat apron one cell of the coarsest
//!   render grid wide (`(extent + 2·margin) / 128`, [`TERRAIN_GRID_SIZES`]), so no tier's
//!   interpolated surface carries the rise onto the board. Every style is 0 within the apron,
//!   never negative and never above `edge_height` (5 cm for the flat apron style).
//! - **Occlusion** is a cheap horizon ambient occlusion from neighbours at three radii in
//!   eight directions, `0.35..=1`; flat ground is 1.
//!
//! # Splat roles ([`generate_splat`])
//! Layers have roles by index: 0 base ground; 1 path (lane corridors within `lane_width / 2`
//! of the lane polyline with a noisy ±2 m edge, discs of 7 m around structures and 12 m
//! around spawns); 2 rock (slopes, fully rock above a gradient of ~0.5, and ~0.7 within 3 m of
//! a wall segment); 3 variation patches on the base; 4..=7 sparse extra patches. Rock covers
//! path, path covers the ground layers. A role without a layer folds its weight into the base,
//! so only `layers.len()` layers ever get weight, and each texel's eight bytes sum to 255.
//!
//! # Water beds ([`generate_terrain_with_water`], ADR-0017 §3)
//! Before occlusion and the splat, each `[[water]]` body carves its bed into the board
//! samples (never outside the board, where the ground only rises). With `d` the distance to
//! the body's centre polyline and `half` half its width: inside (`d <= half`) the ground is
//! `level − depth × (1 − smoothstep(0.35, 1, d / half))`, meeting the surface exactly at the
//! bank; on the banks (`half < d < half + 3`) it rises to at least `level + 6 cm` and blends
//! back to the original relief by `half + 3`. Inside wins over any other body's bank, and
//! everything is clamped to ±[`MAX_PLAY_RELIEF`]. The splat's path layer (pebbles, sand)
//! covers the bed to `half + 1`. Without water the output is exactly [`generate_terrain`]'s.

use crate::map_visuals::{
    DEFAULT_EDGE_MESH_HEIGHT, DEFAULT_EDGE_SPACING, DEFAULT_LANE_WIDTH, EDGE_MESH_HEIGHT_RANGE,
    EDGE_SPACING_RANGE, EdgeStyle, FoliageKind, LANE_WIDTH_RANGE, MAX_WADE_DEPTH, MAX_WATER_LEVEL,
    MapTerrain, MapVisuals, MapWater,
};
use crate::texture::{TextureData, TextureFormat, full_mip_chain};

/// Largest relief inside the board, metres (`|height| <=` this).
pub const MAX_PLAY_RELIEF: f32 = 0.5;
/// Lowest `wall_height` that still reads as a line-of-sight blocker next to a 2 m hero.
pub const MIN_WALL_HEIGHT: f32 = 2.0;
/// Highest accepted `edge_height`, metres.
pub const MAX_EDGE_HEIGHT: f32 = 48.0;
/// Most layers one `[terrain]` table may have.
pub const MAX_TERRAIN_LAYERS: usize = 8;
/// Splat map size the client generates when the descriptor names none.
pub const DEFAULT_SPLAT_SIZE: u32 = 512;
/// Depth of the client's built-in wall block, metres.
pub const WALL_THICKNESS: f32 = 2.5;

/// Minimum height of a prop that [`check_map_consistency`] considers an apparent obstacle.
const OBSTACLE_HEIGHT: f32 = 2.0;
/// Footprint radius per metre of prop height: the built-in block is `height × 0.45` wide.
const PROP_RADIUS_PER_HEIGHT: f32 = 0.225;
/// Clear distance a tall prop keeps from a structure's centre beyond its own radius.
const STRUCTURE_CLEARANCE: f32 = 4.0;
/// Most edge props [`edge_placements`] returns.
const MAX_EDGE_PROPS: usize = 256;
/// Heightfield grid sizes (cells per side) the renderer's quality tiers draw (Low 128, Medium
/// 192, High 256, Ultra 384; `WorldConfig::for_tier` in `moba-render`). The edge rise starts
/// one cell of the coarsest past the board edge, and [`check_map_consistency`] samples the
/// interpolated surface of every one of them.
pub const TERRAIN_GRID_SIZES: [u32; 4] = [128, 192, 256, 384];

/// Simulation map facts the generator and the consistency check read, in simulation
/// coordinates (see the module docs).
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TerrainSite {
    /// Board side length, world units (metres).
    pub extent: f32,
    /// Lane polylines.
    pub lanes: Vec<Vec<[f32; 2]>>,
    /// Vision-blocking wall segments.
    pub walls: Vec<([f32; 2], [f32; 2])>,
    /// Structure positions.
    pub structures: Vec<[f32; 2]>,
    /// Spawn positions.
    pub spawns: Vec<[f32; 2]>,
}

impl TerrainSite {
    /// An empty board of side `extent`.
    #[must_use]
    pub fn new(extent: f32) -> Self {
        Self {
            extent,
            ..Self::default()
        }
    }

    /// Returns this site with one more lane polyline.
    #[must_use]
    pub fn with_lane(mut self, lane: Vec<[f32; 2]>) -> Self {
        self.lanes.push(lane);
        self
    }

    /// Returns this site with one more wall segment.
    #[must_use]
    pub fn with_wall(mut self, a: [f32; 2], b: [f32; 2]) -> Self {
        self.walls.push((a, b));
        self
    }

    /// Returns this site with one more structure.
    #[must_use]
    pub fn with_structure(mut self, position: [f32; 2]) -> Self {
        self.structures.push(position);
        self
    }

    /// Returns this site with one more spawn.
    #[must_use]
    pub fn with_spawn(mut self, position: [f32; 2]) -> Self {
        self.spawns.push(position);
        self
    }

    /// The extent, or 1 when it is not a positive finite number.
    fn board(&self) -> f32 {
        if self.extent.is_finite() && self.extent > 0.0 {
            self.extent.min(65_536.0)
        } else {
            1.0
        }
    }
}

/// Width of the terrain margin around a board of side `extent`: `0.6 × extent`, clamped to
/// 96..=240 m.
#[must_use]
pub fn terrain_margin(extent: f32) -> f32 {
    (extent * 0.6).clamp(96.0, 240.0)
}

/// A square height grid over `[-margin, extent + margin]²` (simulation coordinates).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Heightfield {
    /// Samples per side (cells + 1).
    pub resolution: u32,
    /// Simulation coordinates of sample `(0, 0)`.
    pub origin: [f32; 2],
    /// Distance between neighbouring samples, metres.
    pub spacing: f32,
    /// Heights in metres, row-major, the row index along `y`.
    pub heights: Vec<f32>,
    /// Baked relief ambient occlusion per sample, `0.35..=1`.
    pub occlusion: Vec<f32>,
}

impl Heightfield {
    /// Side length of the covered square, metres.
    #[must_use]
    pub fn size(&self) -> f32 {
        self.spacing * self.resolution.saturating_sub(1) as f32
    }

    /// Height of sample `(ix, iy)`, clamped to the grid.
    #[must_use]
    pub fn height(&self, ix: u32, iy: u32) -> f32 {
        let last = self.resolution.saturating_sub(1);
        let index = iy.min(last) as usize * self.resolution as usize + ix.min(last) as usize;
        self.heights.get(index).copied().unwrap_or(0.0)
    }

    /// Bilinear height at simulation `(x, y)`, clamped to the grid.
    #[must_use]
    pub fn sample(&self, x: f32, y: f32) -> f32 {
        let last = self.resolution.saturating_sub(1) as f32;
        if last < 1.0 || self.spacing <= 0.0 {
            return self.height(0, 0);
        }
        let gx = ((x - self.origin[0]) / self.spacing).clamp(0.0, last);
        let gy = ((y - self.origin[1]) / self.spacing).clamp(0.0, last);
        let ix = (gx.floor() as u32).min(self.resolution - 2);
        let iy = (gy.floor() as u32).min(self.resolution - 2);
        let fx = gx - ix as f32;
        let fy = gy - iy as f32;
        let top = lerp(self.height(ix, iy), self.height(ix + 1, iy), fx);
        let bottom = lerp(self.height(ix, iy + 1), self.height(ix + 1, iy + 1), fx);
        lerp(top, bottom, fy)
    }

    /// Unit surface normal at sample `(ix, iy)` from central differences, in render axes:
    /// `x`, height (up), simulation `y` as `z`.
    #[must_use]
    pub fn normal(&self, ix: u32, iy: u32) -> [f32; 3] {
        let (dx, dy) = self.gradient(ix, iy);
        let length = (dx * dx + 1.0 + dy * dy).sqrt();
        [-dx / length, 1.0 / length, -dy / length]
    }

    /// Height gradient `(dh/dx, dh/dy)` at a sample (one-sided at the border).
    fn gradient(&self, ix: u32, iy: u32) -> (f32, f32) {
        let last = self.resolution.saturating_sub(1);
        if last == 0 || self.spacing <= 0.0 {
            return (0.0, 0.0);
        }
        let (x0, x1) = (ix.saturating_sub(1), (ix + 1).min(last));
        let (y0, y1) = (iy.saturating_sub(1), (iy + 1).min(last));
        let dx = (self.height(x1, iy) - self.height(x0, iy)) / ((x1 - x0) as f32 * self.spacing);
        let dy = (self.height(ix, y1) - self.height(ix, y0)) / ((y1 - y0) as f32 * self.spacing);
        (dx, dy)
    }
}

/// The generator's output for one map.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct GeneratedTerrain {
    /// The height grid.
    pub heightfield: Heightfield,
    /// Generated splat weights, or `None` when the descriptor names a `splat` texture.
    pub splat: Option<[TextureData; 2]>,
}

/// Generates the heightfield, plus the splat when the descriptor names no `splat` texture.
/// `cells` is clamped to 16..=1024 and `splat_size` as in [`generate_splat`].
#[must_use]
pub fn generate_terrain(
    terrain: &MapTerrain,
    site: &TerrainSite,
    cells: u32,
    splat_size: u32,
) -> GeneratedTerrain {
    generate_terrain_with_water(terrain, site, &[], cells, splat_size)
}

/// [`generate_terrain`] with the `[[water]]` beds carved into the heightfield before its
/// occlusion, and the bed painted with the splat's path layer (see the module docs). With no
/// water it is exactly [`generate_terrain`].
#[must_use]
pub fn generate_terrain_with_water(
    terrain: &MapTerrain,
    site: &TerrainSite,
    water: &[MapWater],
    cells: u32,
    splat_size: u32,
) -> GeneratedTerrain {
    let heightfield = heightfield(terrain, site, water, cells);
    let splat = terrain
        .splat
        .is_none()
        .then(|| splat(terrain, site, water, &heightfield, splat_size));
    GeneratedTerrain { heightfield, splat }
}

/// Clamped generator parameters (values built in code are not range-checked).
struct Params {
    extent: f32,
    margin: f32,
    relief: f32,
    edge: EdgeStyle,
    edge_height: f32,
    seed: u32,
    /// Flat ground past the board edge before any style rises: one cell of the coarsest
    /// render grid, so no grid's interpolation carries the rise onto the board.
    apron: f32,
}

impl Params {
    fn new(terrain: &MapTerrain, site: &TerrainSite) -> Self {
        let extent = site.board();
        let margin = terrain_margin(extent);
        Self {
            extent,
            margin,
            apron: (extent + 2.0 * margin) / TERRAIN_GRID_SIZES[0] as f32,
            relief: finite_clamp(terrain.relief, 0.0, MAX_PLAY_RELIEF, 0.0),
            edge: terrain.edge,
            edge_height: finite_clamp(terrain.edge_height, 0.0, MAX_EDGE_HEIGHT, 0.0),
            seed: terrain.seed,
        }
    }

    fn height(&self, x: f32, y: f32) -> f32 {
        let extent = self.extent;
        let outside_x = (-x).max(x - extent).max(0.0);
        let outside_y = (-y).max(y - extent).max(0.0);
        let d = (outside_x * outside_x + outside_y * outside_y).sqrt();
        if d <= 0.0 {
            let inner = x.min(y).min(extent - x).min(extent - y).max(0.0);
            let taper = smoothstep(0.0, 2.0, inner);
            let bumps = 0.75 * fbm(x / 48.0, y / 48.0, self.seed, 3)
                + 0.25 * fbm(x / 13.0, y / 13.0, self.seed ^ 0x5151, 2);
            return (self.relief * bumps * taper).clamp(-self.relief, self.relief);
        }
        // Distance into the rise, past the flat apron (every style is 0 for `d <= 0`).
        let d = d - self.apron;
        let seed = self.seed;
        let height = match self.edge {
            EdgeStyle::None => {
                0.05 * fbm01(x / 12.0, y / 12.0, seed ^ 0x0e0e, 2) * smoothstep(0.0, 6.0, d)
            }
            EdgeStyle::Hills => {
                let reach = 0.35 * self.margin;
                let warp = 0.8 + 0.4 * fbm01(x / 70.0, y / 70.0, seed ^ 0x1111, 2);
                let roll = fbm01(
                    x / (0.2 * self.margin),
                    y / (0.2 * self.margin),
                    seed ^ 0x2222,
                    3,
                );
                self.edge_height * smoothstep(0.0, reach, d * warp) * (0.7 + 0.3 * roll)
            }
            EdgeStyle::Cliffs => {
                let warp = 0.75 + 0.5 * fbm01(x / 24.0, y / 24.0, seed ^ 0x3333, 2);
                let rise = smoothstep(2.0, 14.0, d * warp);
                let base = self.edge_height
                    * rise
                    * (0.85 + 0.15 * fbm01(x / 40.0, y / 40.0, seed ^ 0x4444, 3));
                // Terraces: ledges every quarter of the height, softened, with strata noise.
                let step = (self.edge_height * 0.25).max(1.0);
                let level = base / step;
                let ledge = level.floor();
                let terraced = step * (ledge + smoothstep(0.3, 0.7, level - ledge));
                let strata = 0.15 * step * fbm(x / 9.0, y / 9.0, seed ^ 0x5555, 2) * rise;
                0.65 * terraced + 0.35 * base + strata
            }
        };
        // The flat apron keeps its few centimetres of noise even with `edge_height = 0`.
        height.clamp(0.0, self.edge_height.max(0.05))
    }
}

/// Generates the heightfield over the board and its margin (see the module docs for the
/// rules). `cells` (per side) is clamped to 16..=1024.
#[must_use]
pub fn generate_heightfield(terrain: &MapTerrain, site: &TerrainSite, cells: u32) -> Heightfield {
    heightfield(terrain, site, &[], cells)
}

/// The heightfield with the water beds carved before the occlusion.
fn heightfield(
    terrain: &MapTerrain,
    site: &TerrainSite,
    water: &[MapWater],
    cells: u32,
) -> Heightfield {
    let params = Params::new(terrain, site);
    let cells = cells.clamp(16, 1024);
    let resolution = cells + 1;
    let spacing = (params.extent + 2.0 * params.margin) / cells as f32;
    let origin = [-params.margin, -params.margin];
    let count = resolution as usize * resolution as usize;
    let mut heights = Vec::with_capacity(count);
    for iy in 0..resolution {
        let y = origin[1] + iy as f32 * spacing;
        for ix in 0..resolution {
            heights.push(params.height(origin[0] + ix as f32 * spacing, y));
        }
    }
    let mut heightfield = Heightfield {
        resolution,
        origin,
        spacing,
        heights,
        occlusion: Vec::new(),
    };
    carve_water(&mut heightfield, water, params.extent);
    heightfield.occlusion = occlusion(&heightfield);
    heightfield
}

/// Width of the raised bank beyond a water body's half width, metres.
const WATER_BANK: f32 = 3.0;
/// Height the banks keep above the water surface, metres.
const WATER_BANK_LIFT: f32 = 0.06;

/// Inclusive sample index range of `field` covering `[low, high]` along one axis.
fn sample_range(field: &Heightfield, low: f32, high: f32, axis: usize) -> Option<(u32, u32)> {
    let last = field.resolution.saturating_sub(1);
    let spacing = field.spacing.max(1e-6);
    let first = ((low - field.origin[axis]) / spacing).ceil().max(0.0);
    let end = ((high - field.origin[axis]) / spacing).floor();
    if !(first.is_finite() && end.is_finite()) || end < first || end < 0.0 {
        return None;
    }
    Some((first as u32, (end as u32).min(last)))
}

/// Carves every water body's bed into the board samples (module docs, "Water beds").
fn carve_water(field: &mut Heightfield, water: &[MapWater], extent: f32) {
    if water.is_empty() || field.heights.is_empty() {
        return;
    }
    let width = field.resolution as usize;
    // Per sample: the lowest carved bed of the bodies containing it, and the highest bank.
    let mut bed = vec![f32::INFINITY; field.heights.len()];
    let mut bank = field.heights.clone();
    for body in water {
        let points: Vec<[f32; 2]> = body.finite_points().collect();
        let Some((low, high)) = bounds(&points) else {
            continue;
        };
        let half = body.half_width();
        let (level, depth) = (body.surface(), body.bed_depth());
        let reach = half + WATER_BANK;
        let x_range = sample_range(
            field,
            (low[0] - reach).max(0.0),
            (high[0] + reach).min(extent),
            0,
        );
        let y_range = sample_range(
            field,
            (low[1] - reach).max(0.0),
            (high[1] + reach).min(extent),
            1,
        );
        let (Some((x0, x1)), Some((y0, y1))) = (x_range, y_range) else {
            continue;
        };
        for iy in y0..=y1 {
            let y = field.origin[1] + iy as f32 * field.spacing;
            for ix in x0..=x1 {
                let x = field.origin[0] + ix as f32 * field.spacing;
                let index = iy as usize * width + ix as usize;
                let d = polyline_distance([x, y], &points);
                let original = field.heights[index];
                if d <= half {
                    let carved = level - depth * (1.0 - smoothstep(0.35, 1.0, d / half));
                    bed[index] = bed[index].min(carved);
                } else if d < reach {
                    let lift = level + WATER_BANK_LIFT * smoothstep(half, half + 1.0, d);
                    let target = lerp(
                        original.max(lift),
                        original,
                        smoothstep(half + 1.0, reach, d),
                    );
                    bank[index] = bank[index].max(target);
                }
            }
        }
    }
    for (index, height) in field.heights.iter_mut().enumerate() {
        let value = if bed[index].is_finite() {
            bed[index]
        } else {
            bank[index]
        };
        if value.to_bits() != height.to_bits() {
            *height = value.clamp(-MAX_PLAY_RELIEF, MAX_PLAY_RELIEF);
        }
    }
}

/// Bounding box of `points`, or `None` without any.
pub(crate) fn bounds(points: &[[f32; 2]]) -> Option<([f32; 2], [f32; 2])> {
    let first = *points.first()?;
    Some(points.iter().fold((first, first), |(low, high), p| {
        (
            [low[0].min(p[0]), low[1].min(p[1])],
            [high[0].max(p[0]), high[1].max(p[1])],
        )
    }))
}

/// Horizon ambient occlusion: per direction, the steepest rise among neighbours 2, 4 and 8
/// samples away, as the sine of its elevation angle; averaged over eight directions.
fn occlusion(field: &Heightfield) -> Vec<f32> {
    const DIRECTIONS: [(i32, i32); 8] = [
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
        (1, -1),
    ];
    const RADII: [i32; 3] = [2, 4, 8];
    const REACH: i32 = 8;
    let resolution = field.resolution.cast_signed();
    let width = field.resolution as usize;
    let heights = &field.heights;
    // 1 / horizontal distance per (direction, radius), and the interior index offsets.
    let mut inverse = [[0.0f32; 3]; 8];
    let mut offsets = [[0isize; 3]; 8];
    for (d, (dx, dy)) in DIRECTIONS.iter().enumerate() {
        let step = if *dx != 0 && *dy != 0 {
            core::f32::consts::SQRT_2
        } else {
            1.0
        } * field.spacing.max(1e-6);
        for (r, radius) in RADII.iter().enumerate() {
            inverse[d][r] = 1.0 / (*radius as f32 * step);
            offsets[d][r] = (dy * radius) as isize * width.cast_signed() + (dx * radius) as isize;
        }
    }
    let mut out = Vec::with_capacity(heights.len());
    for iy in 0..resolution {
        let interior_row = iy >= REACH && iy < resolution - REACH;
        for ix in 0..resolution {
            let index = iy as usize * width + ix as usize;
            let centre = heights[index];
            let interior = interior_row && ix >= REACH && ix < resolution - REACH;
            let mut total = 0.0;
            for d in 0..DIRECTIONS.len() {
                // The steepest rise's tangent, then its sine once per direction.
                let mut tangent: f32 = 0.0;
                for r in 0..RADII.len() {
                    let neighbour = if interior {
                        heights[index.wrapping_add_signed(offsets[d][r])]
                    } else {
                        let (dx, dy) = DIRECTIONS[d];
                        let nx = (ix + dx * RADII[r]).clamp(0, resolution - 1) as usize;
                        let ny = (iy + dy * RADII[r]).clamp(0, resolution - 1) as usize;
                        heights[ny * width + nx]
                    };
                    tangent = tangent.max((neighbour - centre) * inverse[d][r]);
                }
                if tangent > 0.0 {
                    total += tangent / (1.0 + tangent * tangent).sqrt();
                }
            }
            out.push((1.0 - 0.9 * total / 8.0).clamp(0.35, 1.0));
        }
    }
    out
}

/// Generates the splat weights over the heightfield's square: `[layers 1-4, layers 5-8]` as
/// linear RGBA8 ([`TextureFormat::Rgba8Unorm`]) with a full box-filtered mip chain. Texel
/// `(0, 0)` is the corner at the heightfield origin and rows run along `+y`. `size` is
/// clamped to 16..=2048 and rounded up to a power of two. See the module docs for the roles.
#[must_use]
pub fn generate_splat(
    terrain: &MapTerrain,
    site: &TerrainSite,
    heightfield: &Heightfield,
    size: u32,
) -> [TextureData; 2] {
    splat(terrain, site, &[], heightfield, size)
}

/// [`generate_splat`] with the path layer over the water beds.
fn splat(
    terrain: &MapTerrain,
    site: &TerrainSite,
    water: &[MapWater],
    heightfield: &Heightfield,
    size: u32,
) -> [TextureData; 2] {
    let size = size.clamp(16, 2048).next_power_of_two();
    let n = size as usize;
    let layers = terrain.layers.len().clamp(1, MAX_TERRAIN_LAYERS);
    let seed = terrain.seed;
    let texel = heightfield.size().max(1e-3) / size as f32;
    let origin = heightfield.origin;
    let half_width = 0.5
        * finite_clamp(
            terrain.lane_width,
            LANE_WIDTH_RANGE.0,
            LANE_WIDTH_RANGE.1,
            DEFAULT_LANE_WIDTH,
        );
    let grid = Grid { n, origin, texel };

    let Masks {
        lane_distance,
        discs,
        wall_rock,
    } = Masks::rasterise(site, &grid, layers, half_width);
    let water_path = if layers > 1 {
        water_path(water, &grid)
    } else {
        Vec::new()
    };

    // The patch noises are smooth at the texel scale: evaluate them at half resolution and
    // interpolate, which keeps a 512² splat well inside its time budget.
    let coarse = |scale: f32, seed: u32, octaves: u32| {
        NoiseField::new(&grid, |x, y| fbm(x / scale, y / scale, seed, octaves))
    };
    let path_edge = (layers > 1).then(|| coarse(9.0, seed ^ 0x7a7a, 2));
    let variation = (layers > 3).then(|| coarse(22.0, seed ^ 0x2b2b, 3));
    let extras: Vec<NoiseField> = (4..layers)
        .map(|extra| {
            let k = (extra - 4) as u32;
            coarse(14.0 + 5.0 * k as f32, seed.wrapping_add(31 + k * 7919), 2)
        })
        .collect();
    let slopes = (layers > 2).then(|| slope_grid(heightfield));

    let mut first = vec![0u8; n * n * 4];
    let mut second = vec![0u8; n * n * 4];
    for iy in 0..n {
        let y = origin[1] + (iy as f32 + 0.5) * texel;
        for ix in 0..n {
            let x = origin[0] + (ix as f32 + 0.5) * texel;
            let index = iy * n + ix;
            let mut weights = [0.0f32; 8];
            let mut rest = 1.0;
            if let Some(slopes) = &slopes {
                let rock = smoothstep(0.35, 0.65, slopes.sample(x, y)).max(wall_rock[index]);
                weights[2] = rock;
                rest *= 1.0 - rock;
            }
            if let Some(path_edge) = &path_edge {
                let mut path = discs[index];
                if let Some(bed) = water_path.get(index) {
                    path = path.max(*bed);
                }
                let d = lane_distance[index];
                if d < half_width + 3.0 {
                    let edge = half_width + (2.0 * path_edge.at(ix, iy)).clamp(-2.0, 2.0);
                    path = path.max(1.0 - smoothstep(edge - 1.0, edge + 1.0, d));
                }
                weights[1] = rest * path;
                rest *= 1.0 - path;
            }
            for (offset, field) in extras.iter().enumerate().rev() {
                let patch = smoothstep(0.55, 0.68, field.at(ix, iy));
                weights[4 + offset] = rest * patch;
                rest *= 1.0 - patch;
            }
            if let Some(variation) = &variation {
                let patch = smoothstep(0.25, 0.45, variation.at(ix, iy));
                weights[3] = rest * patch;
                rest *= 1.0 - patch;
            }
            weights[0] = rest;
            let bytes = quantise(weights);
            first[index * 4..index * 4 + 4].copy_from_slice(&bytes[..4]);
            second[index * 4..index * 4 + 4].copy_from_slice(&bytes[4..]);
        }
    }
    [splat_texture(size, first), splat_texture(size, second)]
}

/// Path weight over the water beds, per texel (empty without water): full on the bed,
/// fading out by `half + 1`.
fn water_path(water: &[MapWater], grid: &Grid) -> Vec<f32> {
    if water.is_empty() {
        return Vec::new();
    }
    let mut weights = vec![0.0f32; grid.n * grid.n];
    for body in water {
        let points: Vec<[f32; 2]> = body.finite_points().collect();
        let half = body.half_width();
        for (a, b) in segments(&points) {
            grid.for_each_near(a, b, half + 1.0, |index, point| {
                let d = segment_distance(point, a, b);
                let weight = 1.0 - smoothstep(half + 0.25, half + 1.0, d);
                weights[index] = weights[index].max(weight);
            });
        }
    }
    weights
}

/// A polyline's segments; one point is a zero-length segment.
pub(crate) fn segments(points: &[[f32; 2]]) -> Vec<([f32; 2], [f32; 2])> {
    match points {
        [] => Vec::new(),
        [single] => vec![(*single, *single)],
        many => many.windows(2).map(|pair| (pair[0], pair[1])).collect(),
    }
}

/// Per-texel distances and weights rasterised from the site's geometry.
struct Masks {
    /// Distance to the nearest lane polyline (empty with one layer).
    lane_distance: Vec<f32>,
    /// Path weight of the structure and spawn discs (empty with one layer).
    discs: Vec<f32>,
    /// Rock weight at the feet of walls (empty with fewer than three layers).
    wall_rock: Vec<f32>,
}

impl Masks {
    fn rasterise(site: &TerrainSite, grid: &Grid, layers: usize, half_width: f32) -> Self {
        let n = grid.n;
        // Path: distance to the nearest lane, and the structure / spawn discs.
        let mut lane_distance = vec![f32::INFINITY; if layers > 1 { n * n } else { 0 }];
        let mut discs = vec![0.0f32; lane_distance.len()];
        // Rock near walls.
        let mut wall_rock = vec![0.0f32; if layers > 2 { n * n } else { 0 }];
        if layers > 1 {
            let reach = half_width + 3.0;
            for lane in &site.lanes {
                let points: Vec<[f32; 2]> = lane.iter().copied().filter(|p| finite(*p)).collect();
                let segments: Vec<([f32; 2], [f32; 2])> = match points.as_slice() {
                    [] => Vec::new(),
                    [single] => vec![(*single, *single)],
                    many => many.windows(2).map(|pair| (pair[0], pair[1])).collect(),
                };
                for (a, b) in segments {
                    grid.for_each_near(a, b, reach, |index, point| {
                        let d = segment_distance(point, a, b);
                        if d < lane_distance[index] {
                            lane_distance[index] = d;
                        }
                    });
                }
            }
            let structure_discs = site.structures.iter().map(|p| (*p, 7.0));
            let spawn_discs = site.spawns.iter().map(|p| (*p, 12.0));
            for (centre, radius) in structure_discs.chain(spawn_discs) {
                if !finite(centre) {
                    continue;
                }
                grid.for_each_near(centre, centre, radius + 2.0, |index, point| {
                    let d = distance(point, centre);
                    let weight = 1.0 - smoothstep(radius - 1.5, radius + 1.5, d);
                    discs[index] = discs[index].max(weight);
                });
            }
        }
        if layers > 2 {
            for &(a, b) in &site.walls {
                if !finite(a) || !finite(b) {
                    continue;
                }
                grid.for_each_near(a, b, 3.5, |index, point| {
                    let weight = 0.7 * (1.0 - smoothstep(2.0, 3.5, segment_distance(point, a, b)));
                    wall_rock[index] = wall_rock[index].max(weight);
                });
            }
        }
        Self {
            lane_distance,
            discs,
            wall_rock,
        }
    }
}

/// Texel addressing for the splat rasteriser.
struct Grid {
    n: usize,
    origin: [f32; 2],
    texel: f32,
}

impl Grid {
    /// Calls `visit(index, centre)` for every texel whose centre lies in the box around the
    /// segment `a`-`b` grown by `reach`.
    fn for_each_near(
        &self,
        a: [f32; 2],
        b: [f32; 2],
        reach: f32,
        mut visit: impl FnMut(usize, [f32; 2]),
    ) {
        let range = |low: f32, high: f32, origin: f32| {
            let first = ((low - reach - origin) / self.texel - 0.5).floor().max(0.0) as usize;
            let last = ((high + reach - origin) / self.texel - 0.5).ceil();
            if last < 0.0 {
                return (1, 0);
            }
            (first, (last as usize).min(self.n - 1))
        };
        let (x0, x1) = range(a[0].min(b[0]), a[0].max(b[0]), self.origin[0]);
        let (y0, y1) = range(a[1].min(b[1]), a[1].max(b[1]), self.origin[1]);
        // An empty range (`first > last`) visits nothing.
        for iy in y0..=y1 {
            let y = self.origin[1] + (iy as f32 + 0.5) * self.texel;
            for ix in x0..=x1 {
                let x = self.origin[0] + (ix as f32 + 0.5) * self.texel;
                visit(iy * self.n + ix, [x, y]);
            }
        }
    }
}

/// A scalar field sampled at every other splat texel, read back bilinearly per texel.
struct NoiseField {
    side: usize,
    values: Vec<f32>,
}

impl NoiseField {
    /// Evaluates `noise` at the centres of texels `(2i, 2j)` for `i, j` in `0..=n/2`.
    fn new(grid: &Grid, noise: impl Fn(f32, f32) -> f32) -> Self {
        let side = grid.n / 2 + 1;
        let mut values = Vec::with_capacity(side * side);
        for j in 0..side {
            let y = grid.origin[1] + (2.0 * j as f32 + 0.5) * grid.texel;
            for i in 0..side {
                let x = grid.origin[0] + (2.0 * i as f32 + 0.5) * grid.texel;
                values.push(noise(x, y));
            }
        }
        Self { side, values }
    }

    /// The field at texel `(ix, iy)`.
    fn at(&self, ix: usize, iy: usize) -> f32 {
        let (i, j) = (ix / 2, iy / 2);
        let (i1, j1) = ((i + 1).min(self.side - 1), (j + 1).min(self.side - 1));
        let row = |j: usize, i: usize| self.values[j * self.side + i];
        if ix.is_multiple_of(2) && iy.is_multiple_of(2) {
            return row(j, i);
        }
        let fx = if ix.is_multiple_of(2) { 0.0 } else { 0.5 };
        let fy = if iy.is_multiple_of(2) { 0.0 } else { 0.5 };
        let top = lerp(row(j, i), row(j, i1), fx);
        let bottom = lerp(row(j1, i), row(j1, i1), fx);
        lerp(top, bottom, fy)
    }
}

/// Gradient magnitude `|∇h|` per heightfield sample, as a heightfield-shaped grid.
fn slope_grid(field: &Heightfield) -> Heightfield {
    let mut slopes = Vec::with_capacity(field.heights.len());
    for iy in 0..field.resolution {
        for ix in 0..field.resolution {
            let (dx, dy) = field.gradient(ix, iy);
            slopes.push((dx * dx + dy * dy).sqrt());
        }
    }
    Heightfield {
        resolution: field.resolution,
        origin: field.origin,
        spacing: field.spacing,
        heights: slopes,
        occlusion: Vec::new(),
    }
}

/// Weights summing to 1 → bytes summing to exactly 255 (the rounding remainder goes to the
/// heaviest layer).
fn quantise(weights: [f32; 8]) -> [u8; 8] {
    let mut bytes = [0u8; 8];
    let mut sum = 0u32;
    let mut heaviest = 0;
    for (index, weight) in weights.iter().enumerate() {
        let value = (weight.clamp(0.0, 1.0) * 255.0).floor() as u8;
        bytes[index] = value;
        sum += u32::from(value);
        if *weight > weights[heaviest] {
            heaviest = index;
        }
    }
    let remainder = 255u32.saturating_sub(sum) as u8;
    bytes[heaviest] = bytes[heaviest].saturating_add(remainder);
    bytes
}

/// A square RGBA8 texture with a full 2×2 box-filtered mip chain.
fn splat_texture(size: u32, base: Vec<u8>) -> TextureData {
    let count = full_mip_chain(size, size);
    let mut levels = Vec::with_capacity(count as usize);
    levels.push(base);
    let mut side = size as usize;
    while side > 1 {
        let half = side / 2;
        let previous = levels.last().expect("level 0 exists");
        let mut next = vec![0u8; half * half * 4];
        for y in 0..half {
            for x in 0..half {
                for channel in 0..4 {
                    let at =
                        |sx: usize, sy: usize| u32::from(previous[(sy * side + sx) * 4 + channel]);
                    let total = at(2 * x, 2 * y)
                        + at(2 * x + 1, 2 * y)
                        + at(2 * x, 2 * y + 1)
                        + at(2 * x + 1, 2 * y + 1);
                    next[(y * half + x) * 4 + channel] = ((total + 2) / 4) as u8;
                }
            }
        }
        levels.push(next);
        side = half;
    }
    TextureData::new(TextureFormat::Rgba8Unorm, size, size, &levels).expect("valid splat shape")
}

/// Where the client draws the visual wall for one simulation wall segment.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct WallBox {
    /// Centre of the box on the ground, simulation coordinates.
    pub center: [f32; 2],
    /// Length along the segment, metres (at least 0.5).
    pub length: f32,
    /// Depth across the segment, metres ([`WALL_THICKNESS`]).
    pub thickness: f32,
    /// Height, metres.
    pub height: f32,
    /// `atan2(dy, dx)` of the segment in the simulation plane, radians. The client rotates
    /// the box with `Quat::from_rotation_y(-angle)`.
    pub angle: f32,
}

/// The visual wall box for the simulation segment `a`-`b` (the client's placement math).
#[must_use]
#[allow(clippy::manual_midpoint)] // `(a + b) * 0.5` is the client's exact arithmetic.
pub fn wall_box(a: [f32; 2], b: [f32; 2], height: f32) -> WallBox {
    let direction = [b[0] - a[0], b[1] - a[1]];
    let length = (direction[0] * direction[0] + direction[1] * direction[1])
        .sqrt()
        .max(0.5);
    WallBox {
        center: [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5],
        length,
        thickness: WALL_THICKNESS,
        height,
        angle: direction[1].atan2(direction[0]),
    }
}

impl WallBox {
    /// `true` when `point` lies inside the box's ground footprint grown by `tolerance`.
    #[must_use]
    pub fn contains(&self, point: [f32; 2], tolerance: f32) -> bool {
        let (sin, cos) = self.angle.sin_cos();
        let rx = point[0] - self.center[0];
        let ry = point[1] - self.center[1];
        let along = rx * cos + ry * sin;
        let across = -rx * sin + ry * cos;
        along.abs() <= 0.5 * self.length + tolerance
            && across.abs() <= 0.5 * self.thickness + tolerance
    }
}

/// Where one edge prop (`[terrain] edge_mesh`) stands.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct EdgePlacement {
    /// Ground position, simulation coordinates (outside the board).
    pub position: [f32; 2],
    /// Yaw in degrees, the props' convention: the mesh's forward (`+Z`) faces the board, so a
    /// mesh authored along `+X` runs along the edge.
    pub yaw_degrees: f32,
}

/// Assumed footprint radius of an edge prop of this height when its mesh's bounds are not
/// known, metres.
fn edge_radius(height: f32) -> f32 {
    height * 0.25 + 2.0
}

/// Horizontal footprint radius of a mesh with model-space bounds `min..max` scaled uniformly
/// to stand `height` metres tall, centred in x/z (how the client places props): the half
/// diagonal of its scaled x/z bounds. `None` for degenerate or non-finite bounds.
#[must_use]
pub fn edge_footprint_radius(min: [f32; 3], max: [f32; 3], height: f32) -> Option<f32> {
    let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    if !size.iter().all(|value| value.is_finite()) || size[1] <= 1e-4 || !height.is_finite() {
        return None;
    }
    let scale = height / size[1];
    let radius = 0.5 * scale * (size[0] * size[0] + size[2] * size[2]).sqrt();
    radius.is_finite().then_some(radius)
}

/// The footprint radius edge props are placed and checked with: the mesh's real one when
/// known ([`edge_footprint_radius`]), never less than the assumed `edge_mesh_height × 0.25 +
/// 2` m, and at most half the terrain margin (a wider mesh is not pushed off the terrain; the
/// consistency check then reports it reaching the board).
fn edge_footprint(terrain: &MapTerrain, extent: f32, footprint: Option<f32>) -> (f32, f32) {
    let assumed = edge_radius(edge_mesh_height(terrain));
    let real = footprint
        .filter(|radius| radius.is_finite() && *radius >= 0.0)
        .map_or(assumed, |radius| radius.max(assumed));
    let placed = real.min(0.5 * terrain_margin(extent)).max(assumed);
    (placed, real)
}

/// Clamped `edge_mesh_height`.
fn edge_mesh_height(terrain: &MapTerrain) -> f32 {
    finite_clamp(
        terrain.edge_mesh_height,
        EDGE_MESH_HEIGHT_RANGE.0,
        EDGE_MESH_HEIGHT_RANGE.1,
        DEFAULT_EDGE_MESH_HEIGHT,
    )
}

/// Edge prop placements around the board perimeter: empty without `edge_mesh`; otherwise one
/// every `edge_spacing` metres (at most 256) on a square offset outward so each prop's
/// footprint stays outside the board. `footprint` is the scaled mesh's horizontal radius
/// ([`edge_footprint_radius`]); `None` assumes `edge_mesh_height × 0.25 + 2` m. The offset
/// is capped at half the terrain margin.
#[must_use]
pub fn edge_placements(
    terrain: &MapTerrain,
    extent: f32,
    footprint: Option<f32>,
) -> Vec<EdgePlacement> {
    if terrain.edge_mesh.is_none() || !(extent.is_finite() && extent > 0.0) {
        return Vec::new();
    }
    let spacing = finite_clamp(
        terrain.edge_spacing,
        EDGE_SPACING_RANGE.0,
        EDGE_SPACING_RANGE.1,
        DEFAULT_EDGE_SPACING,
    );
    let offset = edge_footprint(terrain, extent, footprint).0 + 1.0;
    let side = extent + 2.0 * offset;
    let perimeter = 4.0 * side;
    let count = ((perimeter / spacing).round() as usize).clamp(4, MAX_EDGE_PROPS);
    let step = perimeter / count as f32;
    let (low, high) = (-offset, extent + offset);
    (0..count)
        .map(|index| {
            let along = (index as f32 + 0.5) * step;
            let edge = ((along / side) as u32).min(3);
            let t = along - edge as f32 * side;
            let position = match edge {
                0 => [low + t, low],
                1 => [high, low + t],
                2 => [high - t, high],
                _ => [low, high - t],
            };
            let nearest = [
                position[0].clamp(0.0, extent),
                position[1].clamp(0.0, extent),
            ];
            let toward = [nearest[0] - position[0], nearest[1] - position[1]];
            EdgePlacement {
                position,
                yaw_degrees: (-toward[0]).atan2(toward[1]).to_degrees(),
            }
        })
        .collect()
}

/// Result of [`check_map_consistency`].
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct MapConsistency {
    /// Rule violations: the visuals contradict the simulation.
    pub errors: Vec<String>,
    /// Art-direction warnings: the visuals may mislead but art direction may accept them.
    pub warnings: Vec<String>,
}

impl MapConsistency {
    /// `true` when there are no errors (warnings allowed).
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Checks a map visuals descriptor against the simulation's map (ADR-0014 §2).
///
/// **Errors:** (1) with `[terrain]`, board relief above [`MAX_PLAY_RELIEF`] or edge ground
/// below 0 on the generated 128-cell heightfield; (2) a wall below [`MIN_WALL_HEIGHT`] or a
/// wall box that does not cover its simulation segment; (3) a wall endpoint outside the board
/// (0.5 m tolerance); (4) an edge prop whose footprint reaches the board (assumed from
/// `edge_mesh_height`; [`check_map_consistency_with_edge_footprint`] takes the mesh's real
/// footprint).
/// **Warnings:** (6) a prop at least 2 m tall whose footprint (radius `height × 0.225`)
/// overlaps a lane corridor (`lane_width / 2` from the lane) or comes within 4 m of a
/// structure. Texture files (rule 5) are checked by [`crate::AssetManifest::check_files`].
///
/// **Water and foliage (ADR-0017 §4)**, on every grid size with the beds carved
/// ([`generate_terrain_with_water`]). Errors: a body's level outside
/// `[-MAX_PLAY_RELIEF, MAX_WATER_LEVEL]`; a body whose polyline widened by half its width
/// leaves the board; a board sample under a body deeper than [`MAX_WADE_DEPTH`] below its
/// surface; `[foliage]` or `[[water]]` without `[terrain]`; and the relief rule above with the
/// beds carved. Warnings: water within 4 m of a structure or covering a spawn; water crossing
/// a wall; overlapping bodies; a `brush` patch over a lane corridor or within 6 m of a
/// structure (it would read as cover the simulation does not have).
#[must_use]
pub fn check_map_consistency(visuals: &MapVisuals, site: &TerrainSite) -> MapConsistency {
    check_map_consistency_with_edge_footprint(visuals, site, None)
}

/// [`check_map_consistency`] with the edge mesh's real horizontal footprint radius once
/// scaled to `edge_mesh_height` ([`edge_footprint_radius`] of its bounds), as the client
/// places it; `None` assumes `edge_mesh_height × 0.25 + 2` m.
#[must_use]
pub fn check_map_consistency_with_edge_footprint(
    visuals: &MapVisuals,
    site: &TerrainSite,
    edge_footprint_radius: Option<f32>,
) -> MapConsistency {
    let mut report = MapConsistency::default();
    let extent = site.extent;
    if !(extent.is_finite() && extent > 0.0) {
        report
            .errors
            .push(format!("map extent {extent} is not a positive size"));
        return report;
    }
    if let Some(terrain) = &visuals.ground {
        check_relief(terrain, site, &visuals.water, &mut report);
    }
    check_water_and_foliage(visuals, site, &mut report);
    let inside = |p: [f32; 2]| {
        (-0.5..=extent + 0.5).contains(&p[0]) && (-0.5..=extent + 0.5).contains(&p[1])
    };
    for (index, &(a, b)) in site.walls.iter().enumerate() {
        let wall = format!("wall {index} ({} to {})", point(a), point(b));
        if visuals.wall_height.is_nan() || visuals.wall_height < MIN_WALL_HEIGHT {
            report.errors.push(format!(
                "{wall}: wall_height {} m is below {MIN_WALL_HEIGHT} m, too low to read as a \
                 line-of-sight blocker next to a hero",
                visuals.wall_height
            ));
        }
        let visual = wall_box(a, b, visuals.wall_height);
        if !visual.contains(a, 0.05) || !visual.contains(b, 0.05) {
            report.errors.push(format!(
                "{wall}: the visual wall does not cover the simulation segment"
            ));
        }
        for end in [a, b] {
            if !inside(end) {
                report.errors.push(format!(
                    "{wall}: endpoint {} is outside the board [0, {extent}]; its visual would \
                     stand on the edge slope",
                    point(end)
                ));
            }
        }
    }
    if let Some(terrain) = &visuals.ground {
        let radius = edge_footprint(terrain, extent, edge_footprint_radius).1;
        for (index, placement) in edge_placements(terrain, extent, edge_footprint_radius)
            .iter()
            .enumerate()
        {
            let gap = board_distance(placement.position, extent);
            if gap < radius - 1e-3 {
                report.errors.push(format!(
                    "edge prop {index} at {} (footprint radius {radius} m) reaches into the \
                     board: edge props must stand outside the walkable space",
                    point(placement.position)
                ));
            }
        }
    }
    let half_width = 0.5
        * visuals
            .ground
            .as_ref()
            .map_or(DEFAULT_LANE_WIDTH, |terrain| terrain.lane_width);
    for (index, prop) in visuals.props.iter().enumerate() {
        if prop.height < OBSTACLE_HEIGHT {
            continue;
        }
        let radius = prop.height * PROP_RADIUS_PER_HEIGHT;
        let name = format!(
            "prop {index} at {} ({} m tall, footprint radius {radius:.2} m)",
            point(prop.position),
            prop.height
        );
        for (lane_index, lane) in site.lanes.iter().enumerate() {
            let d = polyline_distance(prop.position, lane);
            if d < half_width + radius {
                report.warnings.push(format!(
                    "{name} overlaps lane {lane_index}'s corridor ({d:.1} m from the lane, \
                     half-width {half_width} m): looks like an obstacle units walk through"
                ));
                break;
            }
        }
        for (structure_index, &structure) in site.structures.iter().enumerate() {
            let d = distance(prop.position, structure);
            if d < radius + STRUCTURE_CLEARANCE {
                report.warnings.push(format!(
                    "{name} is {d:.1} m from structure {structure_index} at {}: looks like an \
                     obstacle units walk through",
                    point(structure)
                ));
            }
        }
    }
    report
}

/// Rule 1 on the generated heightfield, at every render grid size: the samples, plus the
/// interpolated surface of the cells that straddle the board edge (their outside corners may
/// rise, and the drawn surface must still stay within the limit on the board).
/// With water, every grid has the beds carved, and no board sample under a body may lie
/// deeper than [`MAX_WADE_DEPTH`] below its surface.
fn check_relief(
    terrain: &MapTerrain,
    site: &TerrainSite,
    water: &[MapWater],
    report: &mut MapConsistency,
) {
    let mut relief_reported = false;
    let mut wade_reported = vec![false; water.len()];
    for cells in TERRAIN_GRID_SIZES {
        let field = heightfield(terrain, site, water, cells);
        if !relief_reported {
            let before = report.errors.len();
            check_relief_grid(&field, site.extent, cells, report);
            // One grid's report is enough; the finer grids only repeat it.
            relief_reported = report.errors.len() > before;
        }
        for (index, body) in water.iter().enumerate() {
            if !wade_reported[index] {
                wade_reported[index] = check_wade_depth(&field, body, index, site.extent, report);
            }
        }
        if relief_reported && wade_reported.iter().all(|&done| done) {
            return;
        }
    }
}

/// The deepest board sample under `body` on `field`; reports it when it is deeper than
/// [`MAX_WADE_DEPTH`] and returns whether it did.
fn check_wade_depth(
    field: &Heightfield,
    body: &MapWater,
    index: usize,
    extent: f32,
    report: &mut MapConsistency,
) -> bool {
    let points: Vec<[f32; 2]> = body.finite_points().collect();
    let Some((low, high)) = bounds(&points) else {
        return false;
    };
    let half = body.half_width();
    let surface = body.surface();
    let x_range = sample_range(
        field,
        (low[0] - half).max(0.0),
        (high[0] + half).min(extent),
        0,
    );
    let y_range = sample_range(
        field,
        (low[1] - half).max(0.0),
        (high[1] + half).min(extent),
        1,
    );
    let (Some((x0, x1)), Some((y0, y1))) = (x_range, y_range) else {
        return false;
    };
    let mut worst: Option<(f32, [f32; 2])> = None;
    for iy in y0..=y1 {
        let y = field.origin[1] + iy as f32 * field.spacing;
        for ix in x0..=x1 {
            let x = field.origin[0] + ix as f32 * field.spacing;
            if polyline_distance([x, y], &points) > half {
                continue;
            }
            let depth = surface - field.height(ix, iy);
            if worst.is_none_or(|(deepest, _)| depth > deepest || depth.is_nan()) {
                worst = Some((depth, [x, y]));
            }
        }
    }
    match worst {
        Some((depth, at)) if depth.is_nan() || depth > MAX_WADE_DEPTH + 1e-3 => {
            report.errors.push(format!(
                "water {index}: {depth:.3} m deep at {} ({}-cell grid), deeper than the \
                 {MAX_WADE_DEPTH} m a hero visibly wades through (every unit walks everywhere)",
                point(at),
                field.resolution - 1
            ));
            true
        }
        _ => false,
    }
}

/// ADR-0017 §4: the water and foliage rules that do not need a heightfield.
#[allow(clippy::too_many_lines)] // One rule after another, in ADR order.
fn check_water_and_foliage(visuals: &MapVisuals, site: &TerrainSite, report: &mut MapConsistency) {
    let extent = site.extent;
    if visuals.ground.is_none() {
        if !visuals.water.is_empty() {
            report.errors.push(String::from(
                "[[water]] needs the [terrain] table: its bed is carved into the heightfield",
            ));
        }
        if visuals.foliage.is_some() {
            report.errors.push(String::from(
                "[foliage] needs the [terrain] table: its instances stand on the heightfield",
            ));
        }
        return;
    }
    let lane_half = 0.5
        * visuals
            .ground
            .as_ref()
            .map_or(DEFAULT_LANE_WIDTH, |terrain| terrain.lane_width);
    let bodies: Vec<(Vec<[f32; 2]>, f32)> = visuals
        .water
        .iter()
        .map(|body| (body.finite_points().collect(), body.half_width()))
        .collect();
    for (index, body) in visuals.water.iter().enumerate() {
        let name = format!("water {index}");
        if !(body.level.is_finite() && (-MAX_PLAY_RELIEF..=MAX_WATER_LEVEL).contains(&body.level)) {
            report.errors.push(format!(
                "{name}: level {} m is outside [-{MAX_PLAY_RELIEF}, {MAX_WATER_LEVEL}]: the \
                 surface must stay within wading height of the ground units stand on",
                body.level
            ));
        }
        let (points, half) = &bodies[index];
        let half = *half;
        if points.is_empty() || points.len() != body.points.len() {
            report
                .errors
                .push(format!("{name}: needs 1..=32 finite points"));
            continue;
        }
        if let Some(outside) = points.iter().find(|p| {
            p[0] - half < -1e-3
                || p[1] - half < -1e-3
                || p[0] + half > extent + 1e-3
                || p[1] + half > extent + 1e-3
        }) {
            report.errors.push(format!(
                "{name}: at {} its width ({} m) leaves the board [0, {extent}], where the \
                 ground only rises",
                point(*outside),
                2.0 * half
            ));
        }
        for (structure_index, &structure) in site.structures.iter().enumerate() {
            let d = polyline_distance(structure, points) - half;
            if d < 4.0 {
                report.warnings.push(format!(
                    "{name} comes within {:.1} m of structure {structure_index} at {}: \
                     structures should stand on dry ground",
                    d.max(0.0),
                    point(structure)
                ));
            }
        }
        for (spawn_index, &spawn) in site.spawns.iter().enumerate() {
            if polyline_distance(spawn, points) < half {
                report.warnings.push(format!(
                    "{name} covers spawn {spawn_index} at {}",
                    point(spawn)
                ));
            }
        }
        for (wall_index, &(a, b)) in site.walls.iter().enumerate() {
            let d = segments(points)
                .into_iter()
                .map(|(p, q)| segment_segment_distance(p, q, a, b))
                .fold(f32::INFINITY, f32::min);
            if d < half + 0.5 * WALL_THICKNESS {
                report.warnings.push(format!(
                    "{name} crosses wall {wall_index} ({} to {}): a wall standing in water",
                    point(a),
                    point(b)
                ));
            }
        }
        for (other_index, (other, other_half)) in bodies.iter().enumerate().skip(index + 1) {
            let d = polyline_polyline_distance(points, other);
            if d < half + other_half {
                report.warnings.push(format!(
                    "{name} overlaps water {other_index}: the beds merge and the surfaces may \
                     not meet"
                ));
            }
        }
    }
    let Some(foliage) = &visuals.foliage else {
        return;
    };
    for (index, patch) in foliage.patches.iter().enumerate() {
        if patch.kind != FoliageKind::Brush {
            continue;
        }
        let name = format!(
            "brush patch {index} at {} (radius {} m)",
            point(patch.center),
            patch.radius
        );
        for (lane_index, lane) in site.lanes.iter().enumerate() {
            let d = polyline_distance(patch.center, lane);
            if d < lane_half + patch.radius {
                report.warnings.push(format!(
                    "{name} overlaps lane {lane_index}'s corridor: it would read as cover the \
                     simulation does not have"
                ));
                break;
            }
        }
        for (structure_index, &structure) in site.structures.iter().enumerate() {
            let d = distance(patch.center, structure) - patch.radius;
            if d < 6.0 {
                report.warnings.push(format!(
                    "{name} comes within {:.1} m of structure {structure_index} at {}: it \
                     would read as cover the simulation does not have",
                    d.max(0.0),
                    point(structure)
                ));
            }
        }
    }
}

/// Distance between segments `a`-`b` and `c`-`d` (0 when they cross).
fn segment_segment_distance(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> f32 {
    let cross = |o: [f32; 2], p: [f32; 2], q: [f32; 2]| {
        (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0])
    };
    let (d1, d2) = (cross(c, d, a), cross(c, d, b));
    let (d3, d4) = (cross(a, b, c), cross(a, b, d));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return 0.0;
    }
    segment_distance(a, c, d)
        .min(segment_distance(b, c, d))
        .min(segment_distance(c, a, b))
        .min(segment_distance(d, a, b))
}

/// Distance between two polylines (one point each is a point).
fn polyline_polyline_distance(a: &[[f32; 2]], b: &[[f32; 2]]) -> f32 {
    let others = segments(b);
    segments(a)
        .into_iter()
        .flat_map(|(p, q)| {
            others
                .iter()
                .map(move |&(r, s)| segment_segment_distance(p, q, r, s))
        })
        .fold(f32::INFINITY, f32::min)
}

/// The worst interpolated height on the board within the cells that straddle its edge:
/// `(height, position)`. Bilinear samples on a 5×5 lattice over each cell's on-board part.
#[must_use]
pub fn board_edge_relief(field: &Heightfield, extent: f32) -> Option<(f32, [f32; 2])> {
    let mut worst: Option<(f32, [f32; 2])> = None;
    let cells = field.resolution.saturating_sub(1);
    for iy in 0..cells {
        for ix in 0..cells {
            let x0 = field.origin[0] + ix as f32 * field.spacing;
            let y0 = field.origin[1] + iy as f32 * field.spacing;
            let (x1, y1) = (x0 + field.spacing, y0 + field.spacing);
            let (ox0, ox1) = (x0.max(0.0), x1.min(extent));
            let (oy0, oy1) = (y0.max(0.0), y1.min(extent));
            let inside = x0 >= 0.0 && y0 >= 0.0 && x1 <= extent && y1 <= extent;
            if ox0 > ox1 || oy0 > oy1 || inside {
                continue;
            }
            for j in 0..5 {
                for i in 0..5 {
                    let x = lerp(ox0, ox1, i as f32 / 4.0);
                    let y = lerp(oy0, oy1, j as f32 / 4.0);
                    let h = field.sample(x, y);
                    if worst.is_none_or(|(value, _)| h.abs() > value.abs() || h.is_nan()) {
                        worst = Some((h, [x, y]));
                    }
                }
            }
        }
    }
    worst
}

fn check_relief_grid(field: &Heightfield, extent: f32, cells: u32, report: &mut MapConsistency) {
    let mut board: Option<(usize, f32, [f32; 2])> = None;
    let mut edge: Option<(usize, f32, [f32; 2])> = None;
    let record = |slot: &mut Option<(usize, f32, [f32; 2])>, value: f32, at: [f32; 2]| {
        let (count, worst, place) = slot.get_or_insert((0, value, at));
        *count += 1;
        if value.abs() > worst.abs() {
            *worst = value;
            *place = at;
        }
    };
    for iy in 0..field.resolution {
        for ix in 0..field.resolution {
            let at = [
                field.origin[0] + ix as f32 * field.spacing,
                field.origin[1] + iy as f32 * field.spacing,
            ];
            let h = field.height(ix, iy);
            let on_board = (-1e-3..=extent + 1e-3).contains(&at[0])
                && (-1e-3..=extent + 1e-3).contains(&at[1]);
            if on_board && (h.is_nan() || h.abs() > MAX_PLAY_RELIEF + 1e-3) {
                record(&mut board, h, at);
            } else if !on_board && (h.is_nan() || h < -1e-3) {
                record(&mut edge, h, at);
            }
        }
    }
    if board.is_none()
        && let Some((h, at)) = board_edge_relief(field, extent)
        && (h.is_nan() || h.abs() > MAX_PLAY_RELIEF + 1e-3)
    {
        board = Some((1, h, at));
    }
    if let Some((count, worst, at)) = board {
        report.errors.push(format!(
            "terrain relief {worst:.3} m at {} ({count} samples, {cells}-cell grid) exceeds the \
             {MAX_PLAY_RELIEF} m board limit: units stand at height 0, so it would read as a \
             ramp or pit",
            point(at)
        ));
    }
    if let Some((count, worst, at)) = edge {
        report.errors.push(format!(
            "terrain outside the board dips to {worst:.3} m at {} ({count} samples): edge ground \
             may only rise",
            point(at)
        ));
    }
}

pub(crate) fn point(p: [f32; 2]) -> String {
    format!("[{}, {}]", p[0], p[1])
}

pub(crate) fn finite(p: [f32; 2]) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

pub(crate) fn finite_clamp(value: f32, low: f32, high: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(low, high)
    } else {
        fallback
    }
}

pub(crate) fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub(crate) fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(crate) fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

pub(crate) fn segment_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let length2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if length2 > 0.0 {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / length2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    distance(p, [a[0] + t * ab[0], a[1] + t * ab[1]])
}

pub(crate) fn polyline_distance(p: [f32; 2], line: &[[f32; 2]]) -> f32 {
    match line {
        [] => f32::INFINITY,
        [single] => distance(p, *single),
        many => many
            .windows(2)
            .map(|pair| segment_distance(p, pair[0], pair[1]))
            .fold(f32::INFINITY, f32::min),
    }
}

/// Euclidean distance from `p` to the board square `[0, extent]²` (0 inside).
pub(crate) fn board_distance(p: [f32; 2], extent: f32) -> f32 {
    let dx = (-p[0]).max(p[0] - extent).max(0.0);
    let dy = (-p[1]).max(p[1] - extent).max(0.0);
    (dx * dx + dy * dy).sqrt()
}

/// Integer lattice hash (no floats involved, so it is the same on every platform).
pub(crate) fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = x.cast_unsigned().wrapping_mul(0x8da6_b343)
        ^ y.cast_unsigned().wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f).wrapping_add(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^ (h >> 15)
}

/// Lattice value in `[-1, 1]`.
fn lattice(x: i32, y: i32, seed: u32) -> f32 {
    (hash(x, y, seed) >> 8) as f32 / ((1u32 << 24) - 1) as f32 * 2.0 - 1.0
}

/// Smooth value noise in `[-1, 1]`.
fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xf, yf) = (x.floor(), y.floor());
    let (ix, iy) = (xf as i32, yf as i32);
    let (fx, fy) = (x - xf, y - yf);
    let (u, v) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let top = lerp(lattice(ix, iy, seed), lattice(ix + 1, iy, seed), u);
    let bottom = lerp(lattice(ix, iy + 1, seed), lattice(ix + 1, iy + 1, seed), u);
    lerp(top, bottom, v)
}

/// Fractal value noise in `[-1, 1]`.
pub(crate) fn fbm(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    let (mut sum, mut amplitude, mut frequency, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for octave in 0..octaves {
        sum += amplitude
            * value_noise(
                x * frequency + 17.3 * octave as f32,
                y * frequency - 9.1 * octave as f32,
                seed.wrapping_add(octave.wrapping_mul(0x68e3_1da4)),
            );
        norm += amplitude;
        amplitude *= 0.5;
        frequency *= 2.03;
    }
    sum / norm.max(1e-6)
}

/// Fractal value noise in `[0, 1]`.
pub(crate) fn fbm01(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    0.5 + 0.5 * fbm(x, y, seed, octaves)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_visuals::{FoliagePatch, MapFoliage, TerrainLayer};

    fn layers(count: usize) -> Vec<TerrainLayer> {
        (0..count)
            .map(|index| TerrainLayer::new(format!("layer{index}")))
            .collect()
    }

    fn site() -> TerrainSite {
        TerrainSite::new(256.0)
            .with_lane(vec![
                [32.0, 128.0],
                [96.0, 104.0],
                [160.0, 152.0],
                [224.0, 128.0],
            ])
            .with_wall([112.0, 60.0], [112.0, 100.0])
            .with_wall([60.0, 168.0], [100.0, 188.0])
            .with_structure([20.0, 128.0])
            .with_structure([72.0, 112.0])
            .with_spawn([32.0, 128.0])
            .with_spawn([224.0, 128.0])
    }

    /// A descriptor with `extra` appended; a `[terrain]` table in `extra` carries the tint.
    fn visuals(extra: &str) -> MapVisuals {
        let tint = if extra.contains("[terrain]") {
            ""
        } else {
            "terrain = [0.1, 0.1, 0.1]\n"
        };
        MapVisuals::parse(&format!(
            "{tint}wall = [0.2, 0.2, 0.2]\nwall_height = 6.0\n{extra}"
        ))
        .expect("descriptor")
    }

    #[test]
    fn heightfield_obeys_the_relief_rules_for_every_edge_style() {
        let site = site();
        for edge in [EdgeStyle::None, EdgeStyle::Hills, EdgeStyle::Cliffs] {
            for seed in [1, 7, 0xdead_beef] {
                // Builders are not range-checked: the generator clamps the relief.
                let terrain = MapTerrain::new(layers(3))
                    .with_relief(3.0)
                    .with_edge(edge, 30.0)
                    .with_seed(seed);
                let field = generate_heightfield(&terrain, &site, 96);
                assert_eq!(field.resolution, 97);
                assert_eq!(field.heights.len(), 97 * 97);
                assert_eq!(field.occlusion.len(), 97 * 97);
                assert!((field.size() - (256.0 + 2.0 * terrain_margin(256.0))).abs() < 1e-2);
                let mut peak: f32 = 0.0;
                for iy in 0..field.resolution {
                    for ix in 0..field.resolution {
                        let x = field.origin[0] + ix as f32 * field.spacing;
                        let y = field.origin[1] + iy as f32 * field.spacing;
                        let h = field.height(ix, iy);
                        let ao = field.occlusion[(iy * field.resolution + ix) as usize];
                        assert!((0.35..=1.0).contains(&ao), "{ao}");
                        if (0.0..=256.0).contains(&x) && (0.0..=256.0).contains(&y) {
                            assert!(h.abs() <= MAX_PLAY_RELIEF, "{edge:?} {h} at {x},{y}");
                            let inner = x.min(y).min(256.0 - x).min(256.0 - y);
                            assert!(inner < 50.0 || ao > 0.95, "flat board is unoccluded: {ao}");
                        } else {
                            assert!((0.0..=30.0).contains(&h), "{edge:?} {h}");
                            peak = peak.max(h);
                        }
                    }
                }
                match edge {
                    EdgeStyle::None => assert!(peak < 0.1, "{peak}"),
                    _ => assert!(peak > 20.0, "{edge:?} rises: {peak}"),
                }
                // Continuous at the board edge.
                for t in [10.0, 128.0, 250.0] {
                    for (x, y) in [(t, 0.0), (0.0, t), (256.0, t), (t, 256.0)] {
                        assert!(field.sample(x, y).abs() < 0.2, "{edge:?} edge at {x},{y}");
                    }
                }
            }
        }
    }

    /// The edge-strip check sees what the grid samples miss: a 10 m corner one cell outside
    /// the board interpolates to well over the limit on the board, though every sample on
    /// the board is flat.
    #[test]
    fn board_edge_relief_samples_the_interpolated_surface() {
        // 3x3 samples, 8 m apart, from (-4, -4): the board [0, 8]² straddles the cells.
        let mut heights = vec![0.0; 9];
        heights[0] = 10.0; // (-4, -4), outside the board
        let field = Heightfield {
            resolution: 3,
            origin: [-4.0, -4.0],
            spacing: 8.0,
            heights,
            occlusion: vec![1.0; 9],
        };
        let (h, at) = board_edge_relief(&field, 8.0).expect("edge cells");
        assert!(h > MAX_PLAY_RELIEF, "{h} at {at:?}");
        assert_eq!(at, [0.0, 0.0], "worst at the board corner nearest the peak");
        let mut report = MapConsistency::default();
        check_relief_grid(&field, 8.0, 2, &mut report);
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert!(
            report.errors[0].contains("2-cell grid"),
            "{}",
            report.errors[0]
        );
    }

    #[test]
    fn generation_is_deterministic_and_seeded() {
        let site = site();
        let terrain = MapTerrain::new(layers(5)).with_seed(3);
        let a = generate_terrain(&terrain, &site, 64, 64);
        let b = generate_terrain(&terrain, &site, 64, 64);
        assert_eq!(a, b);
        let other = generate_terrain(&terrain.clone().with_seed(4), &site, 64, 64);
        assert_ne!(a.heightfield.heights, other.heightfield.heights);
        let named = generate_terrain(&terrain.with_splat("baked"), &site, 64, 64);
        assert!(named.splat.is_none());
        assert_eq!(named.heightfield, a.heightfield);
    }

    #[test]
    fn heightfield_sampling_and_normals() {
        let terrain = MapTerrain::new(layers(1)).with_edge(EdgeStyle::Cliffs, 40.0);
        let field = generate_heightfield(&terrain, &TerrainSite::new(256.0), 8);
        assert_eq!(field.resolution, 17, "cells clamp to 16..=1024");
        // Bilinear sampling hits the samples exactly and clamps outside the grid.
        assert!((field.sample(field.origin[0], field.origin[1]) - field.height(0, 0)).abs() < 1e-5);
        let x = field.origin[0] + 3.0 * field.spacing;
        let y = field.origin[1] + 5.0 * field.spacing;
        assert!((field.sample(x, y) - field.height(3, 5)).abs() < 1e-4);
        assert!((field.sample(-1e6, -1e6) - field.height(0, 0)).abs() < 1e-5);
        for (ix, iy) in [(0, 0), (8, 8), (16, 3), (2, 16)] {
            let n = field.normal(ix, iy);
            let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((length - 1.0).abs() < 1e-4 && n[1] > 0.0, "{n:?}");
        }
        // The board centre is nearly flat; the cliff face leans towards the board.
        assert!(field.normal(8, 8)[1] > 0.99);
        let flat = MapTerrain::new(layers(1))
            .with_relief(0.0)
            .with_edge(EdgeStyle::None, 0.0);
        let field = generate_heightfield(&flat, &TerrainSite::new(256.0), 16);
        assert_eq!(field.normal(8, 8), [0.0, 1.0, 0.0]);
    }

    #[test]
    fn splat_roles_weights_and_mips() {
        let site = site();
        let terrain = MapTerrain::new(layers(5)).with_edge(EdgeStyle::Cliffs, 30.0);
        let field = generate_heightfield(&terrain, &site, 128);
        let [first, second] = generate_splat(&terrain, &site, &field, 100);
        assert_eq!(
            (first.width(), first.height()),
            (128, 128),
            "rounded up to 2^n"
        );
        assert_eq!(first.format(), TextureFormat::Rgba8Unorm);
        assert_eq!(first.level_count(), 8);
        assert_eq!(second.level_count(), 8);
        let (_, _, a) = first.level(0).expect("level 0");
        let (_, _, b) = second.level(0).expect("level 0");
        let texel = field.size() / 128.0;
        let weights = |x: f32, y: f32| {
            let ix = ((x - field.origin[0]) / texel) as usize;
            let iy = ((y - field.origin[1]) / texel) as usize;
            let i = (iy * 128 + ix) * 4;
            let mut w = [0u8; 8];
            w[..4].copy_from_slice(&a[i..i + 4]);
            w[4..].copy_from_slice(&b[i..i + 4]);
            w
        };
        for i in 0..128 * 128 {
            let sum: u32 = (0..4)
                .map(|c| u32::from(a[i * 4 + c]) + u32::from(b[i * 4 + c]))
                .sum();
            assert_eq!(sum, 255);
            // Only five layers exist: layers 6-8 never get weight.
            assert_eq!(&b[i * 4 + 1..i * 4 + 4], &[0, 0, 0]);
        }
        // On the lane: path. Near a wall: rock. On the cliff: rock.
        assert!(
            weights(128.0, 128.0)[1] > 200,
            "{:?}",
            weights(128.0, 128.0)
        );
        assert!(weights(112.0, 80.0)[2] > 120, "{:?}", weights(112.0, 80.0));
        let cliff = weights(-8.0, 200.0);
        assert!(cliff[2] > 200, "{cliff:?}");
        // Far from everything on the board: ground layers only.
        let ground = weights(200.0, 30.0);
        assert_eq!(u32::from(ground[1]) + u32::from(ground[2]), 0, "{ground:?}");
        // Mips are box filtered and keep the sum.
        let (w, h, top) = first.level(7).expect("1x1");
        assert_eq!((w, h), (1, 1));
        let (_, _, top2) = second.level(7).expect("1x1");
        let sum: u32 = top.iter().chain(top2).map(|&v| u32::from(v)).sum();
        assert!((250..=260).contains(&sum), "{sum}");
        // One layer: everything is base.
        let single = MapTerrain::new(layers(1));
        let [only, rest] = generate_splat(&single, &site, &field, 16);
        let (_, _, only) = only.level(0).expect("level");
        let (_, _, rest) = rest.level(0).expect("level");
        assert!(only.chunks(4).all(|t| t == [255, 0, 0, 0]));
        assert!(rest.iter().all(|&v| v == 0));
    }

    #[test]
    fn wall_box_matches_the_client_math() {
        let wall = wall_box([60.0, 168.0], [100.0, 188.0], 6.0);
        assert_eq!(wall.center, [80.0, 178.0]);
        assert!((wall.length - (40.0f32 * 40.0 + 20.0 * 20.0).sqrt()).abs() < 1e-5);
        assert_eq!(wall.thickness, WALL_THICKNESS);
        assert_eq!(wall.height, 6.0);
        assert!((wall.angle - 20.0f32.atan2(40.0)).abs() < 1e-6);
        assert!(wall.contains([60.0, 168.0], 0.05) && wall.contains([100.0, 188.0], 0.05));
        assert!(wall.contains([80.0, 178.0], 0.0));
        assert!(
            !wall.contains([80.0, 182.0], 0.05),
            "beyond the 2.5 m depth"
        );
        assert!(!wall.contains([104.0, 190.0], 0.05), "past the end");
        let dot = wall_box([5.0, 5.0], [5.0, 5.0], 4.0);
        assert_eq!(dot.length, 0.5);
        assert!(dot.contains([5.0, 5.0], 0.0));
    }

    #[test]
    fn edge_placements_ring_the_board_facing_inward() {
        let terrain = MapTerrain::new(layers(1));
        assert!(
            edge_placements(&terrain, 256.0, None).is_empty(),
            "no edge_mesh"
        );
        let terrain = terrain.with_edge_mesh("rampart", 24.0, 10.0);
        let placements = edge_placements(&terrain, 256.0, None);
        let radius = edge_radius(10.0);
        let expected = (4.0f32 * (256.0 + 2.0 * (radius + 1.0)) / 24.0).round() as usize;
        assert_eq!(placements.len(), expected);
        for placement in &placements {
            assert!(board_distance(placement.position, 256.0) >= radius);
            let yaw = placement.yaw_degrees.to_radians();
            // The client's forward (+Z) after `from_rotation_y(-yaw)` is (-sin, cos).
            let forward = [-yaw.sin(), yaw.cos()];
            let to_centre = [128.0 - placement.position[0], 128.0 - placement.position[1]];
            assert!(forward[0] * to_centre[0] + forward[1] * to_centre[1] > 0.0);
        }
        // The bottom edge (y < 0) faces +y: yaw 0.
        assert!(placements[1].position[1] < 0.0 && placements[1].yaw_degrees.abs() < 1e-3);
        let dense = MapTerrain::new(layers(1)).with_edge_mesh("x", 8.0, 1.0);
        assert_eq!(edge_placements(&dense, 4096.0, None).len(), MAX_EDGE_PROPS);
        assert_eq!(edge_placements(&dense, f32::NAN, None), []);
    }

    /// Review fix: a wide mesh is placed by its real footprint, not the assumed one, and a
    /// footprint too wide to fit the margin is reported reaching into the board.
    #[test]
    fn edge_props_use_the_mesh_footprint() {
        // A rock 1 m tall and 4 m wide: scaled to 48 m tall it is 192 m across.
        let radius =
            edge_footprint_radius([-2.0, 0.0, -0.5], [2.0, 1.0, 0.5], 48.0).expect("bounds");
        assert!(
            (radius - 0.5 * 48.0 * 17.0f32.sqrt()).abs() < 1e-3,
            "{radius}"
        );
        assert_eq!(edge_footprint_radius([0.0; 3], [1.0, 0.0, 1.0], 10.0), None);
        let site = site();
        // A 3 m wide, 1 m tall rock scaled to 10 m: a 21.2 m radius, far above the assumed 4.5.
        let wide = edge_footprint_radius([-1.5, 0.0, -1.5], [1.5, 1.0, 1.5], 10.0);
        let terrain = MapTerrain::new(layers(1)).with_edge_mesh("rock", 24.0, 10.0);
        for placement in edge_placements(&terrain, 256.0, wide) {
            assert!(board_distance(placement.position, 256.0) >= wide.expect("radius"));
        }
        let edged = |height: f32| {
            visuals(&format!(
                "[terrain]\ntint = [0.1, 0.1, 0.1]\nedge_mesh = \"rock\"\n\
                 edge_mesh_height = {height}\n[[terrain.layers]]\nalbedo = \"a\"\n"
            ))
        };
        let descriptor = edged(10.0);
        let clean = check_map_consistency_with_edge_footprint(&descriptor, &site, wide);
        assert!(clean.is_ok(), "{:?}", clean.errors);
        // 4 m wide and 48 m tall: wider than half the margin allows, so it reaches the board.
        let too_wide = edge_footprint_radius([-2.0, 0.0, -2.0], [2.0, 1.0, 2.0], 48.0);
        let descriptor = edged(48.0);
        let report = check_map_consistency_with_edge_footprint(&descriptor, &site, too_wide);
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("reaches into the board")),
            "{:?}",
            report.errors
        );
        assert!(check_map_consistency(&descriptor, &site).is_ok());
    }

    #[test]
    fn consistency_errors_and_warnings() {
        let site = site();
        let clean = visuals(
            "[[props]]\nposition = [200, 40]\ncolor = [1, 1, 1]\nheight = 10\n\
             [[props]]\nposition = [128, 128]\ncolor = [1, 1, 1]\nheight = 1.5\n\
             [terrain]\ntint = [0.1, 0.1, 0.1]\nedge = \"cliffs\"\nedge_mesh = \"rampart\"\n\
             [[terrain.layers]]\nalbedo = \"a\"\n",
        );
        let report = check_map_consistency(&clean, &site);
        assert!(report.is_ok() && report.warnings.is_empty(), "{report:?}");

        // Rule 2: walls too low; rule 3: a wall outside the board.
        let low = visuals("").with_ground(None);
        let mut low = low;
        low.wall_height = 1.0;
        let outside = site.clone().with_wall([250.0, 10.0], [270.0, 10.0]);
        let report = check_map_consistency(&low, &outside);
        assert_eq!(report.errors.len(), 4, "{report:?}");
        assert!(report.errors[0].starts_with("wall 0 ([112, 60] to [112, 100]): wall_height 1 m"));
        assert!(report.errors[3].contains("wall 2 ([250, 10] to [270, 10]): endpoint [270, 10]"));
        assert!(!report.is_ok());

        // Rule 6: tall props on a lane or next to a structure.
        let obstacles = visuals(
            "[[props]]\nposition = [128, 128]\ncolor = [1, 1, 1]\nheight = 16\n\
             [[props]]\nposition = [72, 115]\ncolor = [1, 1, 1]\nheight = 3\n",
        );
        let report = check_map_consistency(&obstacles, &site);
        assert!(report.is_ok());
        assert_eq!(report.warnings.len(), 3, "{:?}", report.warnings);
        assert!(report.warnings[0].starts_with("prop 0 at [128, 128] (16 m tall"));
        assert!(report.warnings[0].contains("lane 0"));
        assert!(report.warnings[2].contains("structure 1 at [72, 112]"));
        // A wider path catches props further away.
        let wide = visuals(
            "[[props]]\nposition = [128, 150]\ncolor = [1, 1, 1]\nheight = 4\n\
             [terrain]\ntint = [0.1, 0.1, 0.1]\nlane_width = 40\n[[terrain.layers]]\nalbedo = \"a\"\n",
        );
        assert_eq!(check_map_consistency(&wide, &site).warnings.len(), 1);

        // Rule 1 holds for the parser's ranges; a zero extent is an error.
        assert!(!check_map_consistency(&clean, &TerrainSite::new(0.0)).is_ok());
    }

    fn river() -> MapWater {
        MapWater::new(vec![[30.0, 220.0], [128.0, 214.0], [226.0, 222.0]])
    }

    #[test]
    fn water_beds_are_carved_inside_the_relief_rules() {
        let site = site();
        let terrain = MapTerrain::new(layers(3)).with_relief(0.5);
        // No water: exactly the ADR-0014 terrain.
        assert_eq!(
            generate_terrain(&terrain, &site, 192, 64),
            generate_terrain_with_water(&terrain, &site, &[], 192, 64)
        );
        let water = [
            river(),
            MapWater::new(vec![[200.0, 40.0]]).with_level(0.3, 0.8),
        ];
        let dry = generate_heightfield(&terrain, &site, 256);
        let wet = generate_terrain_with_water(&terrain, &site, &water, 256, 256);
        let field = &wet.heightfield;
        assert_eq!(field.heights.len(), dry.heights.len());
        let mut carved = 0;
        for iy in 0..field.resolution {
            for ix in 0..field.resolution {
                let p = [
                    field.origin[0] + ix as f32 * field.spacing,
                    field.origin[1] + iy as f32 * field.spacing,
                ];
                let (h, h0) = (field.height(ix, iy), dry.height(ix, iy));
                let on_board = (0.0..=256.0).contains(&p[0]) && (0.0..=256.0).contains(&p[1]);
                if !on_board {
                    assert_eq!(h, h0, "off-board ground is never carved at {p:?}");
                    continue;
                }
                assert!(h.abs() <= MAX_PLAY_RELIEF, "{h} at {p:?}");
                let mut near = false;
                for body in &water {
                    let d = polyline_distance(p, &body.points);
                    let half = body.half_width();
                    if d <= half {
                        assert!(h <= body.level + 1e-6 && h >= body.level - body.depth - 1e-6);
                        assert!(body.level - h <= MAX_WADE_DEPTH + 1e-6);
                        carved += 1;
                        near = true;
                    } else if d < half + 1.0 {
                        assert!(h >= body.level - 1e-6, "the bank stays dry at {p:?}");
                        near = true;
                    } else if d < half + WATER_BANK {
                        near = true;
                    }
                }
                if !near {
                    assert_eq!(h, h0, "{p:?}");
                }
            }
        }
        assert!(carved > 100, "{carved}");
        // The centre line is the full depth below the surface.
        let centre = field.sample(128.0, 214.0);
        assert!((centre - (0.1 - 0.45)).abs() < 1e-4, "{centre}");
        assert!((field.sample(200.0, 40.0) - (0.3 - 0.8)).abs() < 1e-4);
        // The bed takes the path layer.
        let [first, _] = wet.splat.expect("generated splat");
        let [dry_splat, _] = generate_splat(&terrain, &site, &dry, 256);
        let (_, _, a) = first.level(0).expect("level 0");
        let (_, _, b) = dry_splat.level(0).expect("level 0");
        let texel = field.size() / 256.0;
        let at = |x: f32, y: f32| {
            let ix = ((x - field.origin[0]) / texel) as usize;
            let iy = ((y - field.origin[1]) / texel) as usize;
            (iy * 256 + ix) * 4 + 1
        };
        assert!(a[at(128.0, 214.0)] > 200, "{}", a[at(128.0, 214.0)]);
        assert!(b[at(128.0, 214.0)] < 50, "{}", b[at(128.0, 214.0)]);
        // Hostile code-built bodies stay bounded.
        let hostile = [
            MapWater::new(vec![[f32::NAN, 1.0], [1e9, -1e9]]),
            MapWater::new(Vec::new()),
            MapWater::new(vec![[128.0, 128.0]]).with_level(f32::INFINITY, f32::NAN),
        ];
        let field = generate_terrain_with_water(&terrain, &site, &hostile, 64, 16).heightfield;
        assert!(field.heights.iter().all(|h| h.is_finite()));
    }

    #[test]
    fn water_and_foliage_consistency_rules() {
        let site = site();
        let terrain = "[terrain]\ntint = [0.1, 0.1, 0.1]\n[[terrain.layers]]\nalbedo = \"a\"\n";
        let clean = visuals(&format!(
            "{terrain}[[water]]\npoints = [[140, 220], [200, 224]]\n\
             [foliage]\n[[foliage.patches]]\ncenter = [200, 60]\nradius = 6\nkind = \"brush\"\n"
        ));
        let report = check_map_consistency(&clean, &site);
        assert!(report.is_ok() && report.warnings.is_empty(), "{report:?}");

        // Errors: leaving the board; deeper than wading; a level out of range; no terrain.
        let water = MapWater::new(vec![[3.0, 40.0], [60.0, 40.0]]);
        let report = check_map_consistency(&clean.clone().with_water(vec![water]), &site);
        assert_eq!(report.errors.len(), 1, "{report:?}");
        assert!(report.errors[0].starts_with("water 0: at [3, 40] its width"));
        let mut deep = MapWater::new(vec![[200.0, 220.0]]);
        deep.level = 0.6;
        let report = check_map_consistency(&clean.clone().with_water(vec![deep]), &site);
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.starts_with("water 0: level 0.6 m")),
            "{report:?}"
        );
        let mut lake = MapWater::new(vec![[200.0, 220.0]]);
        lake.depth = 2.0;
        lake.level = 0.3;
        // The generator clamps the bed at the relief limit, so it stays wadeable.
        let report = check_map_consistency(&clean.clone().with_water(vec![lake]), &site);
        assert!(report.is_ok(), "{report:?}");
        let flat = clean.clone().with_ground(None);
        let report = check_map_consistency(&flat, &site);
        assert_eq!(report.errors.len(), 2, "{report:?}");

        // Warnings: near a structure, over a spawn, across a wall, overlapping, brush cover.
        let busy = clean
            .clone()
            .with_water(vec![
                MapWater::new(vec![[72.0, 105.0]]).with_width(10.0),
                MapWater::new(vec![[224.0, 140.0]]).with_width(30.0),
                MapWater::new(vec![[40.0, 178.0], [120.0, 178.0]]),
                MapWater::new(vec![[100.0, 230.0], [140.0, 230.0]]),
                MapWater::new(vec![[130.0, 236.0], [180.0, 236.0]]),
            ])
            .with_foliage(Some(
                MapFoliage::default()
                    .with_patch(FoliagePatch::new([128.0, 140.0], 4.0, FoliageKind::Brush))
                    .with_patch(FoliagePatch::new([20.0, 136.0], 4.0, FoliageKind::Brush))
                    .with_patch(FoliagePatch::new([128.0, 140.0], 4.0, FoliageKind::Flowers)),
            ));
        let report = check_map_consistency(&busy, &site);
        assert!(report.is_ok(), "{report:?}");
        let has = |needle: &str| report.warnings.iter().any(|w| w.contains(needle));
        assert!(has("water 0 comes within"), "{:?}", report.warnings);
        assert!(has("water 1 covers spawn 1"), "{:?}", report.warnings);
        assert!(has("water 2 crosses wall 1"), "{:?}", report.warnings);
        assert!(has("water 3 overlaps water 4"), "{:?}", report.warnings);
        assert!(has(
            "brush patch 0 at [128, 140] (radius 4 m) overlaps lane 0"
        ));
        assert!(has("brush patch 1 at [20, 136] (radius 4 m) comes within"));
        assert!(!has("patch 2"), "flowers are not cover");
    }

    #[test]
    fn generation_is_fast_enough_for_map_bind() {
        let site = site();
        let terrain = MapTerrain::new(layers(8)).with_edge(EdgeStyle::Cliffs, 30.0);
        let start = std::time::Instant::now();
        let generated = generate_terrain(&terrain, &site, 384, DEFAULT_SPLAT_SIZE);
        let elapsed = start.elapsed();
        assert_eq!(generated.heightfield.resolution, 385);
        let splat = generated.splat.expect("generated splat");
        assert_eq!(splat[0].width(), DEFAULT_SPLAT_SIZE);
        // Budget (release): < 60 ms. Debug builds are far slower, so only report it here.
        let start = std::time::Instant::now();
        let field = generate_heightfield(&terrain, &site, 384);
        let heights = start.elapsed();
        eprintln!(
            "terrain generation (384 cells, 512² splat): {elapsed:?} (heightfield {heights:?})"
        );
        assert_eq!(field, generated.heightfield);
    }
}
