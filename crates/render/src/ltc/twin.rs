//! The Rust twin of `ltc.glsl` (see the module above): the table fetch and each shape's
//! evaluation, step for step.
//!
//! Adapted from the LTC reference code (`THIRD_PARTY.md`), whose notice is:
//!
//! ```text
//! Copyright (c) 2017, Eric Heitz, Jonathan Dupuy, Stephen Hill and David Neubelt.
//! All rights reserved.
//!
//! Redistribution and use in source and binary forms, with or without
//! modification, are permitted provided that the following conditions are met:
//!
//! * If you use (or adapt) the source code in your own work, please include a
//!   reference to the paper:
//!
//!   Real-Time Polygonal-Light Shading with Linearly Transformed Cosines.
//!   Eric Heitz, Jonathan Dupuy, Stephen Hill and David Neubelt.
//!   ACM Transactions on Graphics (Proceedings of ACM SIGGRAPH 2016) 35(4), 2016.
//!   Project page: https://eheitzresearch.wordpress.com/415-2/
//!
//! * Redistributions of source code must retain the above copyright notice, this
//!   list of conditions and the following disclaimer.
//!
//! * Redistributions in binary form must reproduce the above copyright notice,
//!   this list of conditions and the following disclaimer in the documentation
//!   and/or other materials provided with the distribution.
//!
//! THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
//! AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
//! IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
//! DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
//! FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
//! DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
//! SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
//! CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
//! OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
//! OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
//! ```

// The reference's (and the maths') single-letter names, kept for side-by-side reading.
#![allow(clippy::many_single_char_names)]

use moba_core::math::Vec3;

use super::{LTC_LAYER_BYTES, LTC_SIZE, LTC_TABLE};

std::thread_local! {
    /// A full-precision table standing in for the committed one on this thread
    /// (`the_half_table_integrates_as_the_full_one`): two layers of `LTC_SIZE`² texels.
    pub(crate) static FULL_TABLE: std::cell::RefCell<Option<Vec<[f32; 4]>>> =
        const { std::cell::RefCell::new(None) };
}

/// One texel of `layer` as `f32`.
pub(crate) fn texel(layer: usize, x: usize, y: usize) -> [f32; 4] {
    if let Some(texel) = FULL_TABLE.with_borrow(|table| {
        table
            .as_ref()
            .map(|table| table[layer * LTC_SIZE * LTC_SIZE + y * LTC_SIZE + x])
    }) {
        return texel;
    }
    let at = layer * LTC_LAYER_BYTES + (y * LTC_SIZE + x) * 8;
    core::array::from_fn(|channel| {
        let bytes = [LTC_TABLE[at + 2 * channel], LTC_TABLE[at + 2 * channel + 1]];
        moba_assets::f32_from_f16(u16::from_le_bytes(bytes))
    })
}

/// `ltc_fetch` in `ltc.glsl`: a bilinear fetch of `layer` at `coord` in 0..=1 on both axes,
/// texel `i` at `i / (size − 1)` (the upstream's `LUT_SCALE`/`LUT_BIAS`).
pub(crate) fn fetch(layer: usize, coord: [f32; 2]) -> [f32; 4] {
    let last = (LTC_SIZE - 1) as f32;
    let x = coord[0].clamp(0.0, 1.0) * last;
    let y = coord[1].clamp(0.0, 1.0) * last;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=63.
    let (x0, y0) = (
        (x.floor() as usize).min(LTC_SIZE - 2),
        (y.floor() as usize).min(LTC_SIZE - 2),
    );
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let [a, b, c, d] = [
        texel(layer, x0, y0),
        texel(layer, x0 + 1, y0),
        texel(layer, x0, y0 + 1),
        texel(layer, x0 + 1, y0 + 1),
    ];
    core::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bottom = c[i] + (d[i] - c[i]) * fx;
        top + (bottom - top) * fy
    })
}

/// The inverse LTC matrix `[a 0 c; 0 1 0; b 0 d]` (rows), from `ltc_1`'s `(a, b, c, d)`:
/// the shader's `mat3(vec3(a, 0, b), vec3(0, 1, 0), vec3(c, 0, d))` (columns).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Minv(pub(crate) [f32; 4]);

impl Minv {
    pub(crate) const IDENTITY: Self = Self([1.0, 0.0, 0.0, 1.0]);

    pub(crate) fn apply(self, p: Vec3) -> Vec3 {
        let [a, b, c, d] = self.0;
        Vec3::new(a * p.x + c * p.z, p.y, b * p.x + d * p.z)
    }

    pub(crate) fn determinant(self) -> f32 {
        let [a, b, c, d] = self.0;
        a * d - b * c
    }
}

/// The fitted lobe at `(n_dot_v, roughness)`: the inverse matrix, its magnitude and average
/// Fresnel (`ltc_lobe` in `ltc.glsl`).
pub(crate) fn lobe(n_dot_v: f32, roughness: f32) -> (Minv, [f32; 2]) {
    let coord = [roughness, (1.0 - n_dot_v.clamp(0.0, 1.0)).sqrt()];
    let t1 = fetch(0, coord);
    let t2 = fetch(1, coord);
    (Minv(t1), [t2[0], t2[1]])
}

/// The horizon-clipped sphere's integral for a vector form factor `form` (its length is the
/// unclipped sphere's form factor, its direction the average): `len × table(z, len)`.
fn sphere_clip(form: Vec3) -> f32 {
    let len = form.length();
    if len <= 0.0 {
        return 0.0;
    }
    let z = form.z / len;
    len * fetch(1, [z * 0.5 + 0.5, len])[3]
}

/// `IntegrateEdgeVec`: the edge's vector form factor, with the reference's rational fit of
/// `θ / sin θ` (the `1 / 2π` included).
fn edge(v1: Vec3, v2: Vec3) -> Vec3 {
    let x = v1.dot(v2);
    let y = x.abs();
    let a = 0.854_398_5 + (0.496_515_5 + 0.014_520_6 * y) * y;
    let b = 3.417_594 + (4.161_672_4 + y) * y;
    let v = a / b;
    let theta_sintheta = if x > 0.0 {
        v
    } else {
        0.5 / (1.0 - x * x).max(1e-7).sqrt() - v
    };
    v1.cross(v2) * theta_sintheta
}

/// A unit vector not parallel to unit `v` (`ltc.glsl`'s fallback): +X, or +Y near X.
fn helper(v: Vec3) -> Vec3 {
    if v.x.abs() < 0.9 { Vec3::X } else { Vec3::Y }
}

/// The shading frame: `p` (relative to the shaded point, world axes) in `(t1, t2, n)`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    pub(crate) t1: Vec3,
    pub(crate) t2: Vec3,
    pub(crate) n: Vec3,
}

impl Frame {
    /// `t1` is V projected onto the tangent plane (any tangent when V is the normal).
    pub(crate) fn new(n: Vec3, v: Vec3) -> Self {
        let projected = v - n * v.dot(n);
        let t1 = if projected.length_squared() > 1e-8 {
            projected.normalize()
        } else {
            n.cross(helper(n)).normalize()
        };
        Self {
            t1,
            t2: n.cross(t1),
            n,
        }
    }

    pub(crate) fn local(&self, p: Vec3) -> Vec3 {
        Vec3::new(self.t1.dot(p), self.t2.dot(p), self.n.dot(p))
    }
}

/// A rect seen from the shaded point: the integral over it (one-sided: zero from behind).
/// `centre` is relative to the shaded point (world axes); `ex`, `ey` the half-extent vectors
/// with `ex × ey` the emitting normal.
pub(crate) fn rect(frame: &Frame, minv: Minv, centre: Vec3, ex: Vec3, ey: Vec3) -> f32 {
    if centre.dot(ex.cross(ey)) >= 0.0 {
        return 0.0; // The shaded point is behind (or in) the emitting plane.
    }
    let grow = grow(centre, ex.length().max(ey.length()));
    let (ex, ey) = (ex * grow, ey * grow);
    polygon(
        frame,
        minv,
        [
            centre - ex - ey,
            centre - ex + ey,
            centre + ex + ey,
            centre + ex - ey,
        ],
    ) / (grow * grow)
}

/// `ltc.glsl`'s `ltc_grow`: how much to enlarge a shape whose largest half-extent `extent`
/// spans less than `MIN_SPAN` of its distance. Below that the quad's corners are too close
/// together for `f32` (its edges' cross products cancel to the noise of the normalisation),
/// while its integral is already its area times the lobe's value: the grown shape's integral,
/// divided by the square of the growth, is the small shape's to `O(MIN_SPAN²)`.
pub(crate) fn grow(centre: Vec3, extent: f32) -> f32 {
    (MIN_SPAN * centre.length() / extent.max(1e-6)).max(1.0)
}

/// `ltc.glsl`'s `MOBA_LTC_MIN_SPAN`.
pub(crate) const MIN_SPAN: f32 = 0.01;

/// The clipless quad (`ltc_quad.fs`): the edges' vector form factor, then the sphere table
/// for the horizon. Either winding: the form factor's sign is taken toward the quad.
fn polygon(frame: &Frame, minv: Minv, corners: [Vec3; 4]) -> f32 {
    let l = corners.map(|corner| minv.apply(frame.local(corner)).normalize());
    let form = edge(l[0], l[1]) + edge(l[1], l[2]) + edge(l[2], l[3]) + edge(l[3], l[0]);
    let toward = l[0] + l[1] + l[2] + l[3];
    sphere_clip(if form.dot(toward) < 0.0 { -form } else { form })
}

/// `ltc_facing_quad`: the quad facing the shaded point standing in for a capsule,
/// `half_width` either side across the view at each end and `reach` beyond each end along the
/// axis (`ltc.glsl`).
fn facing_quad(p1: Vec3, p2: Vec3, half_width: f32, reach: f32) -> [Vec3; 4] {
    let axis = (p2 - p1).normalize_or_zero();
    let end = |p: Vec3, sign: f32| {
        let view = p / p.length().max(1e-6);
        let across = view.cross(axis);
        let across = if across.length_squared() > 1e-8 {
            across
        } else {
            view.cross(helper(view))
        }
        .normalize();
        (p + across.cross(view) * (sign * reach), across * half_width)
    };
    let (c1, s1) = end(p1, -1.0);
    let (c2, s2) = end(p2, 1.0);
    [c1 - s1, c1 + s1, c2 + s2, c2 - s2]
}

/// `SolveCubic` (`ltc_disk.fs`, after Peters, "How to solve a cubic equation, revisited"):
/// the three real roots of `c.w x³ + c.z x² + c.y x + c.x`, sorted so that `x` is the
/// largest... as the reference orders them (`x`, `y`, `z` with `y` the middle root).
#[allow(clippy::many_single_char_names)] // The reference's names.
fn solve_cubic(coefficient: [f32; 4]) -> [f32; 3] {
    let pi = core::f32::consts::PI;
    let w = coefficient[3];
    let (cx, cy, cz) = (
        coefficient[0] / w,
        coefficient[1] / w / 3.0,
        coefficient[2] / w / 3.0,
    );
    let (a, b, c, d) = (1.0f32, cz, cy, cx);
    let delta = [-cz * cz + cy, -cy * cz + cx, cz * cx - cy * cy];
    let discriminant = 4.0 * delta[0] * delta[2] - delta[1] * delta[1];
    // Algorithm A.
    let xlc = {
        let c_a = delta[0];
        let d_a = -2.0 * b * delta[0] + delta[1];
        let theta = discriminant.max(0.0).sqrt().atan2(-d_a) / 3.0;
        let x_1a = 2.0 * (-c_a).max(0.0).sqrt() * theta.cos();
        let x_3a = 2.0 * (-c_a).max(0.0).sqrt() * (theta + (2.0 / 3.0) * pi).cos();
        let xl = if x_1a + x_3a > 2.0 * b { x_1a } else { x_3a };
        [xl - b, a]
    };
    // Algorithm D.
    let xsc = {
        let c_d = delta[2];
        let d_d = -d * delta[1] + 2.0 * c * delta[2];
        let theta = (d * discriminant.max(0.0).sqrt()).atan2(-d_d) / 3.0;
        let x_1d = 2.0 * (-c_d).max(0.0).sqrt() * theta.cos();
        let x_3d = 2.0 * (-c_d).max(0.0).sqrt() * (theta + (2.0 / 3.0) * pi).cos();
        let xs = if x_1d + x_3d < 2.0 * c { x_1d } else { x_3d };
        [-d, xs + c]
    };
    let e = xlc[1] * xsc[1];
    let f = -xlc[0] * xsc[1] - xlc[1] * xsc[0];
    let g = xlc[0] * xsc[0];
    let xmc = [c * f - b * g, -b * f + c * e];
    let mut root = [xsc[0] / xsc[1], xmc[0] / xmc[1], xlc[0] / xlc[1]];
    if root[0] < root[1] && root[0] < root[2] {
        root = [root[1], root[0], root[2]];
    } else if root[2] < root[0] && root[2] < root[1] {
        root = [root[0], root[2], root[1]];
    }
    root
}

/// A sphere of radius `radius` at `centre` (relative to the shaded point): with the identity
/// (the diffuse) its own vector form factor, `R² / d²` toward its centre; otherwise the disk
/// spanning its cone of directions, through `minv`, then the ellipse's form factor
/// (`ltc_disk.fs`'s `LTC_Evaluate`). In the shading frame, as `ltc.glsl`.
pub(crate) fn sphere(frame: &Frame, minv: Minv, centre: Vec3, radius: f32) -> f32 {
    let grow = grow(centre, radius);
    let radius = radius * grow;
    let centre = frame.local(centre);
    let distance2 = centre.length_squared();
    let radius2 = radius * radius;
    let integral = if distance2 <= radius2 * 1.0001 {
        // Inside the light: the whole (transformed) upper hemisphere.
        1.0
    } else if minv == Minv::IDENTITY {
        sphere_clip(centre * (radius2 / (distance2 * distance2.sqrt())))
    } else {
        disk(minv, centre, radius)
    };
    integral / (grow * grow)
}

/// `ltc_sphere`'s disk: the one at the centre's distance spanning the sphere's cone,
/// `tan(asin(R / d)) × d` across, facing the point (`centre` in the shading frame).
fn disk(minv: Minv, centre: Vec3, radius: f32) -> f32 {
    let distance2 = centre.length_squared();
    let disk = radius * (distance2 / (distance2 - radius * radius)).sqrt();
    let axis = centre / distance2.sqrt();
    let e1 = axis.cross(helper(axis)).normalize();
    let e2 = axis.cross(e1);
    let c = minv.apply(centre);
    let mut v1 = minv.apply(e1 * disk);
    let mut v2 = minv.apply(e2 * disk);
    let d11 = v1.dot(v1);
    let d22 = v2.dot(v2);
    let d12 = v1.dot(v2);
    // The ellipse's axes: the eigenvectors of the symmetric `[d11 d12; d12 d22]`, by one
    // rotation, stable where the disk is close to a circle (`atan2(0, 0)` guarded).
    let (off, diff) = (2.0 * d12, d11 - d22);
    let theta = if off.abs() + diff.abs() > 1e-7 * (d11 + d22) {
        0.5 * off.atan2(diff)
    } else {
        0.0
    };
    let (sin, cos) = theta.sin_cos();
    let w1 = v1 * cos + v2 * sin;
    let w2 = v2 * cos - v1 * sin;
    let a = 1.0 / w1.dot(w1);
    let b = 1.0 / w2.dot(w2);
    v1 = w1 * a.sqrt();
    v2 = w2 * b.sqrt();
    let mut v3 = v1.cross(v2);
    if c.dot(v3) < 0.0 {
        v3 = -v3;
    }
    let l = v3.dot(c);
    let x0 = v1.dot(c) / l;
    let y0 = v2.dot(c) / l;
    let (a, b) = (a * l * l, b * l * l);
    let c0 = a * b;
    let c1 = a * b * (1.0 + x0 * x0 + y0 * y0) - a - b;
    let c2 = 1.0 - a * (1.0 + x0 * x0) - b * (1.0 + y0 * y0);
    let [e1r, e2r, e3r] = solve_cubic([c0, c1, c2, 1.0]);
    let average = Vec3::new(a * x0 / (a - e2r), b * y0 / (b - e2r), 1.0);
    let average = (v1 * average.x + v2 * average.y + v3 * average.z).normalize();
    let l1 = (-e2r / e3r).max(0.0).sqrt();
    let l2 = (-e2r / e1r).max(0.0).sqrt();
    let form_factor = l1 * l2 / ((1.0 + l1 * l1) * (1.0 + l2 * l2)).sqrt();
    // A degenerate ellipse (the roots coincide) adds nothing rather than a NaN.
    if form_factor > 0.0 {
        sphere_clip(average * form_factor)
    } else {
        0.0
    }
}

/// A capsule: the segment from `p1` to `p2` (relative to the shaded point) of radius
/// `radius`, as the facing quad of its projected area.
pub(crate) fn capsule(frame: &Frame, minv: Minv, p1: Vec3, p2: Vec3, radius: f32) -> f32 {
    let centre = (p1 + p2) * 0.5;
    let half = (p2 - p1) * 0.5;
    // Inside the capsule: the whole (transformed) upper hemisphere, as `sphere`.
    let segment = p2 - p1;
    let along = ((-p1).dot(segment) / segment.length_squared().max(1e-12)).clamp(0.0, 1.0);
    if (p1 + segment * along).length_squared() <= radius * radius {
        return 1.0;
    }
    let grow = grow(centre, radius + half.length());
    let (half, radius) = (half * grow, radius * grow);
    polygon(
        frame,
        minv,
        facing_quad(
            centre - half,
            centre + half,
            radius,
            0.25 * core::f32::consts::PI * radius,
        ),
    ) / (grow * grow)
}
