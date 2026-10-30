//! Procedural, self-generated sample textures (no external art; see `THIRD_PARTY.md`).
//!
//! `sentinel` is a tileable armour-plate set: panels separated by bevelled seams with rivets
//! at the panel corners. The four maps share one height field so they line up:
//!
//! - `base` (colour, sRGB): pale painted panels with soft wear, dark bronze seams/rivets;
//! - `normal` (tangent-space XY): derived from the height field (seam bevels, rivet domes);
//! - `mr` (data): metallic 1 / roughness 0.35 on the bronze trim, 0 / ~0.7 on the paint;
//! - `mask` (one channel): 1 on painted panels, where the unit's team colour tints them.
//!
//! `terrain` is a set of tileable ground layers ([`TERRAIN_LAYERS`]: `grass`, `dirt`, `rock`,
//! `moss`, `pebbles`), each one height field turned into two maps:
//!
//! - albedo (colour, sRGB) with **ambient occlusion / cavity in alpha** (linear, 1 = open,
//!   down to [`TERRAIN_AO_MIN`] in crevices: height below a local box mean, plus depth below
//!   the layer's top);
//! - tangent-space normal (XY), derived from the height like `sentinel`'s.
//!
//! The layers are deliberately neutral (mean linear luminance about 0.37–0.46, low to
//! moderate saturation) because maps recolour each with a tint; they are stylised and keep
//! their detail at four texels or more so they do not alias from a top-down camera. `rock`
//! has warped strata with ledges and joints (it is sampled triplanar on slopes and cliffs).
//!
//! Every pattern is periodic over the 256-texel tile (all noise wraps with the tile), and the
//! generator uses only integer hashing and correctly rounded `f32` operations (no libm), so the
//! output is identical on every platform.

/// Texture size of the generated set (tiles seamlessly).
pub(crate) const SIZE: u32 = 256;
/// Panels per tile along each axis.
const PANELS: u32 = 4;

/// Integer hash → `0.0..1.0` (deterministic, platform-independent).
fn hash(x: u32, y: u32, seed: u32) -> f32 {
    let mut h =
        x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65_535.0
}

/// Tileable value noise with period `cells` over the texture.
fn noise(x: f32, y: f32, cells: u32, seed: u32) -> f32 {
    let scale = cells as f32 / SIZE as f32;
    let (fx, fy) = (x * scale, y * scale);
    let (ix, iy) = (fx.floor() as u32, fy.floor() as u32);
    let (tx, ty) = (fx - fx.floor(), fy - fy.floor());
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (smooth(tx), smooth(ty));
    let corner = |dx: u32, dy: u32| hash((ix + dx) % cells, (iy + dy) % cells, seed);
    let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * sx;
    let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * sx;
    top + (bottom - top) * sy
}

/// Distance in texels from `(x, y)` to the nearest panel seam, and to the nearest rivet.
fn features(x: f32, y: f32) -> (f32, f32) {
    let panel = (SIZE / PANELS) as f32;
    let (px, py) = (x.rem_euclid(panel), y.rem_euclid(panel));
    let seam = px.min(panel - px).min(py).min(panel - py);
    let inset = 9.0;
    let rivet = [
        (inset, inset),
        (panel - inset, inset),
        (inset, panel - inset),
        (panel - inset, panel - inset),
    ]
    .iter()
    .map(|&(rx, ry)| ((px - rx).powi(2) + (py - ry).powi(2)).sqrt())
    .fold(f32::MAX, f32::min);
    (seam, rivet)
}

const RIVET_RADIUS: f32 = 3.5;
const SEAM_HALF_WIDTH: f32 = 2.0;
const BEVEL: f32 = 5.0;

/// Height in `0..=1`: seams low, bevelled up to the panel face, rivets domed.
fn height(x: f32, y: f32) -> f32 {
    let (seam, rivet) = features(x, y);
    let face = ((seam - SEAM_HALF_WIDTH) / BEVEL).clamp(0.0, 1.0);
    let face = face * face * (3.0 - 2.0 * face);
    let dome = (1.0 - (rivet / RIVET_RADIUS).powi(2)).max(0.0).sqrt();
    let plate = 0.6 * face + 0.03 * noise(x, y, 32, 7);
    if dome > 0.0 {
        plate.max(0.6 + 0.35 * dome)
    } else {
        plate
    }
}

/// Whether the texel is bronze trim (seam or rivet) rather than painted panel.
fn is_trim(x: f32, y: f32) -> bool {
    let (seam, rivet) = features(x, y);
    seam < SEAM_HALF_WIDTH + 1.0 || rivet < RIVET_RADIUS
}

/// One of the sentinel maps as tightly packed RGBA8 (`SIZE` × `SIZE`), or `None` for an
/// unknown map name (`base`, `normal`, `mr`, `mask`).
pub(crate) fn sentinel(map: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let trim = is_trim(fx, fy);
            let pixel = match map {
                "base" => {
                    let wear = noise(fx, fy, 16, 1) * 0.6 + noise(fx, fy, 64, 2) * 0.4;
                    let (seam, _) = features(fx, fy);
                    // Paint is lighter where edges wear; trim is dark bronze.
                    let edge = (1.0 - ((seam - SEAM_HALF_WIDTH) / 10.0).clamp(0.0, 1.0)) * 0.08;
                    if trim {
                        let v = 0.85 + 0.15 * wear;
                        [(140.0 * v) as u8, (104.0 * v) as u8, (62.0 * v) as u8, 255]
                    } else {
                        let v = (0.86 + 0.10 * wear + edge).min(1.0);
                        let g = (232.0 * v) as u8;
                        [g, g, (226.0 * v) as u8, 255]
                    }
                }
                "normal" => {
                    let strength = 3.0;
                    let dx = (height(fx + 1.0, fy) - height(fx - 1.0, fy)) * 0.5 * strength;
                    let dy = (height(fx, fy + 1.0) - height(fx, fy - 1.0)) * 0.5 * strength;
                    let length = (dx * dx + dy * dy + 1.0).sqrt();
                    let encode = |value: f32| ((value / length * 0.5 + 0.5) * 255.0).round() as u8;
                    // +V runs down the image, so the Y gradient is negated for +Y up.
                    [encode(-dx), encode(dy), 255, 255]
                }
                "mr" => {
                    if trim {
                        [255, 89, 0, 255]
                    } else {
                        let roughness = 0.62 + 0.16 * noise(fx, fy, 32, 3);
                        [0, (roughness * 255.0) as u8, 0, 255]
                    }
                }
                "mask" => {
                    let v = if trim { 0 } else { 255 };
                    [v, v, v, 255]
                }
                _ => return None,
            };
            out.extend_from_slice(&pixel);
        }
    }
    Some(out)
}

/// The terrain layers `terrain` knows, in the order maps usually list them.
pub(crate) const TERRAIN_LAYERS: [&str; 5] = ["grass", "dirt", "rock", "moss", "pebbles"];

/// One terrain layer's two source images, each tightly packed RGBA8 (`SIZE` × `SIZE`).
pub(crate) struct TerrainLayer {
    /// sRGB albedo in RGB, linear ambient occlusion / cavity in alpha (1 = open).
    pub(crate) albedo: Vec<u8>,
    /// Tangent-space normal (X in R, Y in G, +Y up), derived from the layer's height.
    pub(crate) normal: Vec<u8>,
}

/// GLSL-style smoothstep.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    core::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
}

/// Periodic value noise with `cells` lattice cells across the tile; any coordinate wraps.
fn wrap_noise(x: f32, y: f32, cells: u32, seed: u32) -> f32 {
    wrap_noise_grid(x, y, (cells, cells), seed)
}

/// `wrap_noise` over a `columns` × `rows` lattice (stretched features, still periodic).
fn wrap_noise_grid(x: f32, y: f32, (columns, rows): (u32, u32), seed: u32) -> f32 {
    let (fx, fy) = (
        x * (columns as f32 / SIZE as f32),
        y * (rows as f32 / SIZE as f32),
    );
    let (floor_x, floor_y) = (fx.floor(), fy.floor());
    let (sx, sy) = (
        smoothstep(0.0, 1.0, fx - floor_x),
        smoothstep(0.0, 1.0, fy - floor_y),
    );
    let (ix, iy) = (floor_x as i32, floor_y as i32);
    let corner = |dx: i32, dy: i32| {
        hash(
            (ix + dx).rem_euclid(columns.cast_signed()) as u32,
            (iy + dy).rem_euclid(rows.cast_signed()) as u32,
            seed,
        )
    };
    let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * sx;
    let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * sx;
    top + (bottom - top) * sy
}

/// Periodic fractal noise in `0..=1`: `octaves` of `wrap_noise` from `cells` up, halving.
fn fbm(x: f32, y: f32, cells: u32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut total, mut amplitude) = (0.0, 0.0, 1.0);
    for octave in 0..octaves {
        sum += wrap_noise(x, y, cells << octave, seed + octave) * amplitude;
        total += amplitude;
        amplitude *= 0.5;
    }
    sum / total
}

/// Periodic domain warp: `(x, y)` displaced by up to `amount` texels.
fn warp(x: f32, y: f32, cells: u32, seed: u32, amount: f32) -> (f32, f32) {
    (
        x + (fbm(x, y, cells, 2, seed) - 0.5) * 2.0 * amount,
        y + (fbm(x, y, cells, 2, seed + 17) - 0.5) * 2.0 * amount,
    )
}

/// Nearest and second-nearest jittered feature point of a periodic `cells` × `cells` grid.
struct Voronoi {
    /// Distance to the nearest point, in cell units.
    f1: f32,
    /// Distance to the second-nearest point, in cell units.
    f2: f32,
    /// Offset from the nearest point, in cell units.
    offset: (f32, f32),
    /// Wrapped id of the nearest point's cell (stable across the tile seam).
    id: u32,
}

fn voronoi(x: f32, y: f32, cells: u32, seed: u32) -> Voronoi {
    voronoi_grid(x, y, (cells, cells), seed)
}

/// `voronoi` over a `columns` × `rows` grid (anisotropic cells, still periodic).
fn voronoi_grid(x: f32, y: f32, (columns, rows): (u32, u32), seed: u32) -> Voronoi {
    let (fx, fy) = (
        x * (columns as f32 / SIZE as f32),
        y * (rows as f32 / SIZE as f32),
    );
    let (ix, iy) = (fx.floor() as i32, fy.floor() as i32);
    let mut best = Voronoi {
        f1: f32::MAX,
        f2: f32::MAX,
        offset: (0.0, 0.0),
        id: 0,
    };
    for oy in -1..=1 {
        for ox in -1..=1 {
            let (cx, cy) = (ix + ox, iy + oy);
            let (wx, wy) = (
                cx.rem_euclid(columns.cast_signed()) as u32,
                cy.rem_euclid(rows.cast_signed()) as u32,
            );
            let px = cx as f32 + 0.1 + 0.8 * hash(wx, wy, seed);
            let py = cy as f32 + 0.1 + 0.8 * hash(wx, wy, seed + 1);
            let (dx, dy) = (fx - px, fy - py);
            let distance = (dx * dx + dy * dy).sqrt();
            if distance < best.f1 {
                best.f2 = best.f1;
                best.f1 = distance;
                best.offset = (dx, dy);
                best.id = wy * columns + wx;
            } else if distance < best.f2 {
                best.f2 = distance;
            }
        }
    }
    best
}

/// One terrain texel: height in `0..=1` and sRGB albedo in `0..=1`.
type Sample = (f32, [f32; 3]);

/// How a layer turns its height into normals and occlusion.
struct TerrainSpec {
    sample: fn(f32, f32) -> Sample,
    /// Height-to-slope gain for the normal map.
    strength: f32,
    /// Radius (texels) of the local mean the cavity term compares against.
    radius: usize,
    /// Occlusion per unit of height below the local mean.
    cavity: f32,
    /// Occlusion per unit of height below the top of the layer.
    depth: f32,
}

/// Painterly grass: three overlapping layers of short, tapered blade strokes that bend with a
/// broad warp, denser clumps from low-frequency noise, soft green/yellow value patches.
fn grass(x: f32, y: f32) -> Sample {
    let broad = fbm(x, y, 3, 3, 10);
    let dry = smoothstep(0.45, 0.75, fbm(x, y, 4, 2, 11));
    let clumps = smoothstep(0.3, 0.7, fbm(x, y, 6, 2, 12));
    let (wx, wy) = warp(x, y, 4, 13, 22.0);
    let mut blade = 0.0f32;
    let mut tip = 0.0f32;
    for (grid, seed) in [((30, 10), 14), ((38, 12), 15), ((26, 9), 16)] {
        let cell = voronoi_grid(wx, wy, grid, seed);
        let width = 0.45 + 0.2 * hash(cell.id, 0, seed + 5);
        let body = 1.0 - smoothstep(0.0, width, cell.f1);
        // Blades lean: they rise towards one end of the stroke.
        let along = (0.5 - cell.offset.1).clamp(0.0, 1.0);
        let h = body * (0.55 + 0.45 * along) * (0.8 + 0.2 * hash(cell.id, 1, seed + 5));
        if h > blade {
            blade = h;
            tip = along * body;
        }
    }
    let height = (0.15 + 0.6 * blade * (0.7 + 0.3 * clumps) + 0.25 * broad).clamp(0.0, 1.0);
    let low = mix3([0.56, 0.64, 0.42], [0.62, 0.65, 0.44], dry);
    let high = mix3([0.73, 0.81, 0.56], [0.80, 0.81, 0.58], dry);
    let colour = mix3(low, high, smoothstep(0.15, 0.85, 0.6 * blade + 0.4 * tip));
    let shade = 0.91 + 0.12 * broad + 0.04 * clumps;
    (height, colour.map(|c| c * shade))
}

/// Dry soil: soft undulation, small rounded clods, patches of fine cracks.
fn dirt(x: f32, y: f32) -> Sample {
    let broad = fbm(x, y, 4, 3, 20);
    let (wx, wy) = warp(x, y, 8, 21, 6.0);
    let clods = voronoi(wx, wy, 24, 22);
    let clod_radius = 0.22 + 0.2 * hash(clods.id, 1, 23);
    let clod = if hash(clods.id, 0, 23) > 0.35 {
        let r = (clods.f1 / clod_radius).min(1.0);
        (1.0 - r * r).sqrt()
    } else {
        0.0
    };
    let (cx, cy) = warp(x, y, 5, 24, 16.0);
    let cracks = voronoi(cx, cy, 5, 25);
    let crack_mask = smoothstep(0.42, 0.62, fbm(x, y, 4, 2, 26));
    let crack = (1.0 - smoothstep(0.0, 0.06, cracks.f2 - cracks.f1)) * crack_mask;
    let grit = fbm(x, y, 32, 2, 27);
    let height = (0.35 + 0.3 * broad + 0.3 * clod + 0.08 * grit - 0.4 * crack).clamp(0.0, 1.0);
    let soil = mix3([0.67, 0.58, 0.48], [0.78, 0.70, 0.59], broad);
    let colour = mix3(soil, [0.82, 0.74, 0.63], 0.4 * clod);
    let colour = mix3(colour, [0.46, 0.39, 0.32], crack);
    let shade = 0.92 + 0.12 * grit;
    (height, colour.map(|c| c * shade))
}

/// Layered rock: warped sedimentary strata that rise and break off at ledges, fine
/// lamination along the layers, vertical joints within each layer and a few long cracks.
fn rock(x: f32, y: f32) -> Sample {
    const BANDS: u32 = 5;
    const JOINTS: u32 = 5;
    let strata =
        y * BANDS as f32 / SIZE as f32 + 1.0 * fbm(x, y, 2, 2, 30) + 0.06 * fbm(x, y, 8, 2, 31);
    let band = strata.floor();
    let t = strata - band;
    let band_id = (band as i32).rem_euclid(BANDS.cast_signed()) as u32;
    // A coordinate that follows the layers (periodic: strata gains BANDS per tile).
    let along = strata * SIZE as f32 / BANDS as f32;
    let lamination = wrap_noise_grid(x, along, (4, 48), 37);
    // Each layer rises across the band and breaks off over the last few texels (no hard
    // discontinuity, which would alias), down to the next layer's foot.
    let rise = |t: f32| 0.25 * smoothstep(0.0, 0.12, t) + 0.75 * t;
    let ledge = rise(t) + (rise(0.0) - rise(0.92)) * smoothstep(0.92, 1.0, t);
    // Joints: wobbly vertical breaks, shifted per layer, some missing.
    let u = x * JOINTS as f32 / SIZE as f32
        + 0.5 * (fbm(x, along, 4, 2, 38) - 0.5)
        + hash(band_id, 2, 32) * JOINTS as f32;
    let joint_index = (u.floor() as i32).rem_euclid(JOINTS.cast_signed()) as u32;
    let fraction = u - u.floor();
    let joint_distance = fraction.min(1.0 - fraction);
    // Each joint runs part of the way through its layer.
    let start = 0.5 * hash(joint_index, band_id, 41);
    let end = start + 0.35 + 0.6 * hash(joint_index, band_id, 42);
    let joint = if hash(joint_index, band_id, 39) > 0.4 {
        (1.0 - smoothstep(0.0, 0.035, joint_distance))
            * smoothstep(start, start + 0.08, t)
            * (1.0 - smoothstep(end - 0.08, end, t))
            * (1.0 - smoothstep(0.88, 0.96, t))
    } else {
        0.0
    };
    let (cx, cy) = warp(x, y, 4, 33, 12.0);
    let long = voronoi(cx, cy, 3, 34);
    let crack_mask = smoothstep(0.5, 0.65, fbm(x, y, 3, 2, 35));
    let crack = (1.0 - smoothstep(0.0, 0.03, long.f2 - long.f1)) * crack_mask;
    let crack = crack.max(joint);
    let pits = fbm(x, y, 16, 3, 36);
    // Chiselled facets: a soft ridge network.
    let facets = voronoi(x, along, 6, 43);
    let facet = smoothstep(0.0, 0.4, facets.f2 - facets.f1);
    let height = (0.15 + 0.5 * ledge + 0.12 * pits + 0.08 * lamination + 0.1 * facet - 0.4 * crack)
        .clamp(0.0, 1.0);
    // Per-layer tone, blended into the next layer across the break so it stays continuous.
    let next_id = (band_id + 1) % BANDS;
    let blend = smoothstep(0.9, 1.0, t);
    let per_band =
        |k: u32| hash(band_id, k, 32) + (hash(next_id, k, 32) - hash(band_id, k, 32)) * blend;
    let tone = per_band(0);
    let grey = 0.655
        + 0.12 * tone
        + 0.06 * (lamination - 0.5)
        + 0.25 * (fbm(x, y, 4, 3, 40) - 0.5)
        + 0.05 * (ledge - 0.5)
        + 0.02 * facet;
    let warm = 0.03 * (per_band(1) - 0.5);
    let colour = [grey + warm, grey, grey - warm];
    // Ledge lips catch light; cracks and the foot of each step are darker.
    let lip = smoothstep(0.75, 0.88, t) * (1.0 - smoothstep(0.9, 0.97, t)) * 0.07;
    let foot = ((1.0 - smoothstep(0.0, 0.12, t)) + smoothstep(0.93, 1.0, t)) * 0.08;
    let shade = 0.93 + 0.1 * pits + lip - foot - 0.3 * crack;
    (height, colour.map(|c| c * shade))
}

/// Moss: soft overlapping grey-green cushions at two sizes with a fine spongy surface.
fn moss(x: f32, y: f32) -> Sample {
    let broad = fbm(x, y, 3, 3, 40);
    let (wx, wy) = warp(x, y, 5, 41, 12.0);
    let big = voronoi(wx, wy, 8, 42);
    let small = voronoi(wx, wy, 18, 43);
    let cushion_big = 1.0 - smoothstep(0.1, 0.9, big.f1);
    let cushion_small = 1.0 - smoothstep(0.05, 0.8, small.f1);
    let cushion = cushion_big.max(0.8 * cushion_small);
    let sponge = fbm(x, y, 32, 2, 44);
    let height = (0.15 + 0.6 * cushion + 0.15 * broad + 0.1 * sponge).clamp(0.0, 1.0);
    let colour = mix3(
        [0.52, 0.56, 0.46],
        [0.68, 0.75, 0.59],
        smoothstep(0.2, 0.9, height),
    );
    let colour = mix3(
        colour,
        [0.69, 0.70, 0.63],
        0.45 * smoothstep(0.45, 0.75, broad),
    );
    let shade = 0.9 + 0.12 * sponge + 0.06 * broad;
    (height, colour.map(|c| c * shade))
}

/// Rounded stones of a periodic `cells` grid, one per cell: elliptical domes with random
/// size, orientation and elongation (the tallest wins where neighbours overlap). Returns the
/// height (0 between stones) and the stone's tone in `0..1`.
fn stones(x: f32, y: f32, cells: u32, seed: u32, radius: (f32, f32)) -> (f32, f32) {
    let scale = cells as f32 / SIZE as f32;
    let (fx, fy) = (x * scale, y * scale);
    let (ix, iy, period) = (fx.floor() as i32, fy.floor() as i32, cells.cast_signed());
    let mut best = (0.0f32, 0.0f32);
    for oy in -1..=1 {
        for ox in -1..=1 {
            let (cx, cy) = (ix + ox, iy + oy);
            let (wx, wy) = (cx.rem_euclid(period) as u32, cy.rem_euclid(period) as u32);
            let random = |k: u32| hash(wx, wy, seed + k);
            let (px, py) = (
                cx as f32 + 0.1 + 0.8 * random(0),
                cy as f32 + 0.1 + 0.8 * random(1),
            );
            let (mut ax, mut ay) = (random(2) - 0.5, random(3) - 0.5);
            let length = (ax * ax + ay * ay).sqrt();
            (ax, ay) = if length > 1e-3 {
                (ax / length, ay / length)
            } else {
                (1.0, 0.0)
            };
            let (dx, dy) = (fx - px, fy - py);
            let size = radius.0 + (radius.1 - radius.0) * random(4);
            let (along, across) = (
                (dx * ax + dy * ay) / size,
                (dy * ax - dx * ay) / (size * (0.65 + 0.35 * random(5))),
            );
            let d2 = along * along + across * across;
            if d2 < 1.0 {
                // Flattish pebble: a dome with a slightly squashed top.
                // (Eased in at the rim so the contact with the ground does not alias.)
                let dome = (1.0 - d2).sqrt() * smoothstep(0.0, 0.25, 1.0 - d2);
                let top = (dome * 1.3).min(1.0) * (0.7 + 0.3 * random(6));
                if top > best.0 {
                    best = (top, random(7));
                }
            }
        }
    }
    best
}

/// Pebbles: distinct rounded stones (each its own tone) on dark grit, smaller stones in the
/// gaps between the large ones.
fn pebbles(x: f32, y: f32) -> Sample {
    let (wx, wy) = warp(x, y, 8, 50, 3.0);
    let (large, large_tone) = stones(wx, wy, 9, 51, (0.4, 0.58));
    let (small, small_tone) = stones(wx, wy, 21, 52, (0.35, 0.5));
    let (stone, tone) = if large >= small * 0.6 {
        (large, large_tone)
    } else {
        (small * 0.6, small_tone)
    };
    let grit = fbm(x, y, 32, 2, 53);
    let speckle = fbm(x, y, 16, 2, 54);
    let height = (0.06 + 0.84 * stone + 0.1 * grit).clamp(0.0, 1.0);
    let warm = 0.04 * (hash((tone * 997.0) as u32, 0, 55) - 0.5);
    let light = 0.72 + 0.14 * tone + 0.05 * (speckle - 0.5);
    // Stones darken towards their rims, the way their occlusion does (which also keeps
    // colour and alpha correlated inside a 4x4 block, as single-mode BC7 needs).
    let rim = 0.84 + 0.16 * smoothstep(0.0, 0.8, stone);
    let pebble = [light + warm, light, light - warm - 0.02].map(|c| c * rim);
    let ground = [0.56, 0.54, 0.50].map(|c| c * (0.9 + 0.2 * grit));
    let colour = mix3(ground, pebble, smoothstep(0.0, 0.3, stone));
    (height, colour)
}

fn terrain_spec(layer: &str) -> Option<TerrainSpec> {
    let spec = |sample, strength, radius, cavity, depth| TerrainSpec {
        sample,
        strength,
        radius,
        cavity,
        depth,
    };
    Some(match layer {
        "grass" => spec(grass as fn(f32, f32) -> Sample, 6.0, 6, 1.6, 0.25),
        "dirt" => spec(dirt, 5.0, 5, 2.0, 0.2),
        "rock" => spec(rock, 7.0, 6, 2.0, 0.25),
        "moss" => spec(moss, 4.0, 8, 1.4, 0.25),
        "pebbles" => spec(pebbles, 7.0, 5, 2.2, 0.2),
        _ => return None,
    })
}

/// Box mean of a periodic `SIZE` × `SIZE` field over a `(2 radius + 1)²` window.
fn periodic_mean(field: &[f32], radius: usize) -> Vec<f32> {
    let size = SIZE as usize;
    let window = (2 * radius + 1) as f32;
    let blur = |field: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0.0; size * size];
        for row in 0..size {
            for column in 0..size {
                let mut sum = 0.0;
                for offset in 0..=2 * radius {
                    let k = (column + size + offset - radius) % size;
                    sum += if horizontal {
                        field[row * size + k]
                    } else {
                        field[k * size + row]
                    };
                }
                let index = if horizontal {
                    row * size + column
                } else {
                    column * size + row
                };
                out[index] = sum / window;
            }
        }
        out
    };
    blur(&blur(field, true), false)
}

/// Lowest ambient occlusion a terrain layer bakes (deepest crevice).
pub(crate) const TERRAIN_AO_MIN: f32 = 0.55;

/// A terrain layer's albedo (+ occlusion in alpha) and normal map, or `None` for an
/// unknown layer (see [`TERRAIN_LAYERS`]).
pub(crate) fn terrain(layer: &str) -> Option<TerrainLayer> {
    let spec = terrain_spec(layer)?;
    let size = SIZE as usize;
    let mut height = Vec::with_capacity(size * size);
    let mut colour = Vec::with_capacity(size * size);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (h, c) = (spec.sample)(x as f32 + 0.5, y as f32 + 0.5);
            height.push(h);
            colour.push(c);
        }
    }
    let mean = periodic_mean(&height, spec.radius);
    let to_byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut albedo = Vec::with_capacity(size * size * 4);
    let mut normal = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let index = y * size + x;
            let h = height[index];
            let occlusion = 1.0 - spec.cavity * (mean[index] - h).max(0.0) - spec.depth * (1.0 - h);
            let ao = occlusion.clamp(TERRAIN_AO_MIN, 1.0);
            let [r, g, b] = colour[index];
            albedo.extend_from_slice(&[to_byte(r), to_byte(g), to_byte(b), to_byte(ao)]);
            let at = |dx: usize, dy: usize| height[((y + dy) % size) * size + (x + dx) % size];
            let dx = (at(1, 0) - at(size - 1, 0)) * 0.5 * spec.strength;
            let dy = (at(0, 1) - at(0, size - 1)) * 0.5 * spec.strength;
            let length = (dx * dx + dy * dy + 1.0).sqrt();
            let encode = |value: f32| ((value / length * 0.5 + 0.5) * 255.0).round() as u8;
            // Same convention as `sentinel`: +V runs down the image, so Y is negated for +Y up.
            normal.extend_from_slice(&[encode(-dx), encode(dy), 255, 255]);
        }
    }
    Some(TerrainLayer { albedo, normal })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_are_deterministic_tileable_and_named() {
        for map in ["base", "normal", "mr", "mask"] {
            let a = sentinel(map).expect("known map");
            assert_eq!(a.len(), (SIZE * SIZE * 4) as usize);
            assert_eq!(a, sentinel(map).expect("known map"), "{map}");
        }
        assert!(sentinel("albedo").is_none());
        // Opposite edges match up to the period (seams at 0 and SIZE).
        assert_eq!(is_trim(0.5, 100.5), is_trim(SIZE as f32 - 0.5, 100.5));
        let mask = sentinel("mask").expect("mask");
        let painted = mask
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[0] == 255)
            .count();
        assert!(
            painted > (SIZE * SIZE / 2) as usize,
            "mostly painted: {painted}"
        );
    }

    #[test]
    fn terrain_layers_are_deterministic_and_tile_seamlessly() {
        for layer in TERRAIN_LAYERS {
            let a = terrain(layer).expect("known layer");
            assert_eq!(a.albedo.len(), (SIZE * SIZE * 4) as usize);
            assert_eq!(a.normal.len(), (SIZE * SIZE * 4) as usize);
            let b = terrain(layer).expect("known layer");
            assert!(a.albedo == b.albedo && a.normal == b.normal, "{layer}");
            // The pattern is periodic: one tile over, it is the same surface (up to float
            // rounding of the shifted coordinates, which may move a feature edge by a hair).
            let sample = terrain_spec(layer).expect("spec").sample;
            let period = SIZE as f32;
            let mut worst = 0.0f32;
            for y in (0..SIZE).step_by(3) {
                for x in (0..SIZE).step_by(3) {
                    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let (h, c) = sample(fx, fy);
                    for (h2, c2) in [sample(fx + period, fy), sample(fx, fy + period)] {
                        worst = worst.max((h - h2).abs());
                        for (p, q) in c.iter().zip(&c2) {
                            worst = worst.max((p - q).abs());
                        }
                    }
                }
            }
            assert!(worst < 0.02, "{layer}: off by {worst} one period over");
        }
        assert!(terrain("lava").is_none());
    }
}
