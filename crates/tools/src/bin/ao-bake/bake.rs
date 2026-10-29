//! Height-field rasterisation and horizon ambient occlusion (no I/O).
#![allow(clippy::cast_possible_wrap)] // Texel offsets: grids are at most 2048 wide.
//!
//! Coordinates are map coordinates: `x` and `z` are the pack's world `x` and `y`
//! (`0..extent`), `y` is up. Texel `(u, v)` covers `x ∈ [u, u + 1) · texel`,
//! `z ∈ [v, v + 1) · texel`.

/// Horizon directions per texel.
pub(crate) const DIRECTIONS: usize = 16;
/// Horizon search radius, metres.
pub(crate) const RADIUS: f32 = 12.0;
/// Height of the receiver above the ground, metres (the shading point's feet).
const RECEIVER: f32 = 0.05;
/// Occlusion of texels deep inside an occluder that no dilation pass reached.
const INTERIOR: f32 = 0.6;
/// Dilation passes that spread the AO of open neighbours into occluder footprints, so the
/// base of a wall picks up the occlusion of the ground beside it.
const DILATE_PASSES: usize = 4;
/// Radius of the final box blur, texels (a 5×5 box at 0.5 m is a 2.5 m filter).
const BLUR: isize = 2;

/// A square top-down height field, row-major (`v * texels + u`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HeightField {
    pub(crate) texels: usize,
    /// Metres per texel.
    pub(crate) texel: f32,
    pub(crate) heights: Vec<f32>,
}

impl HeightField {
    /// A flat field covering `extent` metres at about `texel` metres per texel (at most
    /// `max_texels` per side).
    pub(crate) fn new(extent: f32, texel: f32, max_texels: usize) -> Self {
        let texels = ((extent / texel).ceil() as usize).clamp(1, max_texels);
        Self {
            texels,
            texel: extent / texels as f32,
            heights: vec![0.0; texels * texels],
        }
    }

    fn height(&self, u: isize, v: isize) -> f32 {
        let n = self.texels as isize;
        if u < 0 || v < 0 || u >= n || v >= n {
            return 0.0;
        }
        self.heights[(v * n + u) as usize]
    }

    /// Texel index range covering `min..=max` metres along one axis.
    fn span(&self, min: f32, max: f32) -> core::ops::Range<usize> {
        let first = ((min / self.texel).floor().max(0.0)) as usize;
        let last = ((max / self.texel).ceil().max(0.0) as usize).min(self.texels);
        first.min(last)..last
    }

    /// Raises the field to `height` inside an oriented box footprint: centre `(cx, cz)`,
    /// unit axis `(ax, az)`, half-length along the axis and half-width across it.
    pub(crate) fn add_box(
        &mut self,
        center: [f32; 2],
        axis: [f32; 2],
        half: [f32; 2],
        height: f32,
    ) {
        let [cx, cz] = center;
        let [ax, az] = axis;
        let reach = half[0].hypot(half[1]);
        for v in self.span(cz - reach, cz + reach) {
            for u in self.span(cx - reach, cx + reach) {
                let px = (u as f32 + 0.5) * self.texel - cx;
                let pz = (v as f32 + 0.5) * self.texel - cz;
                let along = px * ax + pz * az;
                let across = -px * az + pz * ax;
                if along.abs() <= half[0] && across.abs() <= half[1] {
                    let cell = &mut self.heights[v * self.texels + u];
                    *cell = cell.max(height);
                }
            }
        }
    }

    /// Raises the field to a triangle's top surface (vertices in map coordinates, `y` up).
    pub(crate) fn add_triangle(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3]) {
        let area = (b[0] - a[0]) * (c[2] - a[2]) - (c[0] - a[0]) * (b[2] - a[2]);
        if !area.is_finite() || area.abs() < 1e-8 {
            return;
        }
        let min_x = a[0].min(b[0]).min(c[0]);
        let max_x = a[0].max(b[0]).max(c[0]);
        let min_z = a[2].min(b[2]).min(c[2]);
        let max_z = a[2].max(b[2]).max(c[2]);
        for v in self.span(min_z, max_z) {
            for u in self.span(min_x, max_x) {
                let px = (u as f32 + 0.5) * self.texel;
                let pz = (v as f32 + 0.5) * self.texel;
                let w0 = ((b[0] - px) * (c[2] - pz) - (c[0] - px) * (b[2] - pz)) / area;
                let w1 = ((c[0] - px) * (a[2] - pz) - (a[0] - px) * (c[2] - pz)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4 {
                    let y = w0 * a[1] + w1 * b[1] + w2 * c[1];
                    let cell = &mut self.heights[v * self.texels + u];
                    *cell = cell.max(y);
                }
            }
        }
    }
}

/// Horizon AO of a ground-level receiver at every texel, `0..=1` (1 = open): per direction
/// the steepest occluder within [`RADIUS`] gives a horizon angle `h`, and the visible part of
/// a cosine-weighted hemisphere is `1 − mean(sin² h)`. Texels inside an occluder take their
/// open neighbours' value (wall and prop bases), then a 5×5 box blur removes stair-steps and jitter noise.
pub(crate) fn horizon_ao(field: &HeightField) -> Vec<f32> {
    let n = field.texels;
    let steps = ((RADIUS / field.texel).ceil() as usize).clamp(1, 96);
    let mut ao = vec![1.0f32; n * n];
    let mut inside = vec![false; n * n];
    for v in 0..n {
        for u in 0..n {
            let index = v * n + u;
            if field.heights[index] > RECEIVER {
                inside[index] = true;
                continue;
            }
            let mut occluded = 0.0f32;
            // A per-texel rotation of the direction set (a fixed hash, so the bake is
            // deterministic) turns the streaks a few thin occluders cast into fine noise
            // that the blur removes.
            let jitter = hash(u, v);
            for direction in 0..DIRECTIONS {
                let angle =
                    (direction as f32 + jitter) * core::f32::consts::TAU / DIRECTIONS as f32;
                let (dz, dx) = angle.sin_cos();
                let mut steepest = 0.0f32;
                for step in 1..=steps {
                    let distance = step as f32 * field.texel;
                    let su = (u as f32 + 0.5 + dx * step as f32).floor() as isize;
                    let sv = (v as f32 + 0.5 + dz * step as f32).floor() as isize;
                    let rise = field.height(su, sv) - RECEIVER;
                    if rise > 0.0 {
                        steepest = steepest.max(rise / distance);
                    }
                }
                // sin² of the horizon angle from its tangent.
                occluded += steepest * steepest / (1.0 + steepest * steepest);
            }
            ao[index] = 1.0 - occluded / DIRECTIONS as f32;
        }
    }
    dilate(&mut ao, &mut inside, n);
    blur(&ao, n)
}

/// A fixed per-texel value in `0..1`.
fn hash(u: usize, v: usize) -> f32 {
    let mut x = (u as u32).wrapping_mul(0x9e37_79b1) ^ (v as u32).wrapping_mul(0x85eb_ca77);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2c1b_3c6d);
    x ^= x >> 12;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// Spreads open texels' AO into occluder footprints, [`DILATE_PASSES`] texels deep.
fn dilate(ao: &mut [f32], inside: &mut [bool], n: usize) {
    for _ in 0..DILATE_PASSES {
        let snapshot = inside.to_vec();
        let values = ao.to_vec();
        for v in 0..n {
            for u in 0..n {
                let index = v * n + u;
                if !snapshot[index] {
                    continue;
                }
                let (mut sum, mut count) = (0.0f32, 0u32);
                for (du, dv) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                    let (nu, nv) = (u as isize + du, v as isize + dv);
                    if nu < 0 || nv < 0 || nu >= n as isize || nv >= n as isize {
                        continue;
                    }
                    let neighbour = nv as usize * n + nu as usize;
                    if !snapshot[neighbour] {
                        sum += values[neighbour];
                        count += 1;
                    }
                }
                if count > 0 {
                    ao[index] = sum / count as f32;
                    inside[index] = false;
                }
            }
        }
    }
    for (value, inside) in ao.iter_mut().zip(inside.iter()) {
        if *inside {
            *value = INTERIOR;
        }
    }
}

fn blur(ao: &[f32], n: usize) -> Vec<f32> {
    let mut out = vec![1.0f32; n * n];
    for v in 0..n {
        for u in 0..n {
            let (mut sum, mut count) = (0.0f32, 0.0f32);
            for dv in -BLUR..=BLUR {
                for du in -BLUR..=BLUR {
                    let (nu, nv) = (u as isize + du, v as isize + dv);
                    if nu >= 0 && nv >= 0 && nu < n as isize && nv < n as isize {
                        sum += ao[nv as usize * n + nu as usize];
                        count += 1.0;
                    }
                }
            }
            out[v * n + u] = sum / count;
        }
    }
    out
}

/// `R8_UNORM` bytes of an AO field (rounded, clamped).
pub(crate) fn to_r8(ao: &[f32]) -> Vec<u8> {
    ao.iter()
        .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_ground_is_fully_lit() {
        let field = HeightField::new(16.0, 0.5, 2048);
        assert_eq!(field.texels, 32);
        assert!(
            horizon_ao(&field)
                .iter()
                .all(|value| (value - 1.0).abs() < 1e-6)
        );
    }

    #[test]
    fn a_wall_darkens_the_ground_beside_it_and_fades_with_distance() {
        let mut field = HeightField::new(40.0, 0.5, 2048);
        // A 20 m wall along x at z = 20, 2.5 m thick and 6 m high.
        field.add_box([20.0, 20.0], [1.0, 0.0], [10.0, 1.25], 6.0);
        assert!(field.heights[40 * field.texels + 40] > 5.9);
        let ao = horizon_ao(&field);
        let at = |x: f32, z: f32| {
            ao[(z / field.texel) as usize * field.texels + (x / field.texel) as usize]
        };
        let near = at(20.0, 22.0);
        let far = at(20.0, 31.0);
        let open = at(20.0, 39.5);
        assert!(near < 0.75, "near {near}");
        assert!(near < far && far < open, "{near} {far} {open}");
        assert!(open > 0.97, "open {open}");
        // The base of the wall picked up its neighbours' occlusion, not the interior value.
        let base = at(20.0, 20.0);
        assert!(base < 0.9 && base > 0.3, "base {base}");
    }

    #[test]
    fn a_triangle_raises_only_its_footprint() {
        let mut field = HeightField::new(8.0, 1.0, 2048);
        field.add_triangle([1.0, 2.0, 1.0], [5.0, 2.0, 1.0], [1.0, 2.0, 5.0]);
        assert_eq!(field.heights[2 * 8 + 2], 2.0);
        assert_eq!(field.heights[6 * 8 + 6], 0.0);
        // Degenerate (vertical) triangles are skipped.
        field.add_triangle([7.0, 0.0, 7.0], [7.0, 9.0, 7.0], [7.0, 9.0, 7.0]);
        assert_eq!(field.heights[7 * 8 + 7], 0.0);
        assert_eq!(to_r8(&[0.0, 0.5, 1.0, 2.0]), [0, 128, 255, 255]);
    }
}
