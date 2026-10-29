//! Procedural, self-generated sample textures (no external art; see `THIRD_PARTY.md`).
//!
//! `sentinel` is a tileable armour-plate set: panels separated by bevelled seams with rivets
//! at the panel corners. The four maps share one height field so they line up:
//!
//! - `base` (colour, sRGB): pale painted panels with soft wear, dark bronze seams/rivets;
//! - `normal` (tangent-space XY): derived from the height field (seam bevels, rivet domes);
//! - `mr` (data): metallic 1 / roughness 0.35 on the bronze trim, 0 / ~0.7 on the paint;
//! - `mask` (one channel): 1 on painted panels, where the unit's team colour tints them.

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
}
