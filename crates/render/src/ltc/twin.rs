//! The Rust twin of `ltc.glsl` (see the module above): the table fetch and each shape's
//! evaluation, step for step.

// The reference's (and the maths') single-letter names, kept for side-by-side reading.
#![allow(clippy::many_single_char_names)]

use moba_core::math::Vec3;

use super::{LTC_LAYER_BYTES, LTC_SIZE, LTC_TABLE};

/// One texel of `layer` as `f32`.
pub(crate) fn texel(layer: usize, x: usize, y: usize) -> [f32; 4] {
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

/// `ltc_facing_quad`: the quad facing the shaded point standing in for a sphere or capsule,
/// `half_width` either side across the view at each end and `reach` beyond each end along the
/// axis (`ltc.glsl`).
fn facing_quad(p1: Vec3, p2: Vec3, half_width: f32, reach: f32) -> [Vec3; 4] {
    let axis = (p2 - p1).normalize_or_zero();
    let end = |p: Vec3, sign: f32| {
        let view = p.normalize();
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

/// A sphere of radius `radius` at `centre` (relative to the shaded point): the square, facing
/// the point, of the area of the disk spanning the sphere's cone.
pub(crate) fn sphere(frame: &Frame, minv: Minv, centre: Vec3, radius: f32) -> f32 {
    let grow = grow(centre, radius);
    let radius = radius * grow;
    let distance2 = centre.length_squared();
    let radius2 = radius * radius;
    let disk = radius * (distance2 / (distance2 - radius2).max(1e-6)).sqrt();
    let half_side = 0.5 * core::f32::consts::PI.sqrt() * disk;
    polygon(
        frame,
        minv,
        facing_quad(centre, centre, half_side, half_side),
    ) / (grow * grow)
}

/// A capsule: the segment from `p1` to `p2` (relative to the shaded point) of radius
/// `radius`, as the facing quad of its projected area.
pub(crate) fn capsule(frame: &Frame, minv: Minv, p1: Vec3, p2: Vec3, radius: f32) -> f32 {
    let centre = (p1 + p2) * 0.5;
    let half = (p2 - p1) * 0.5;
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
