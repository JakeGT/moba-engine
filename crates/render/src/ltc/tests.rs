//! Gate 3 (ADR-0023 §9b): each shape's LTC evaluation against a Monte Carlo reference of the
//! engine's own lobes (`lighting.glsl`'s GGX × height-correlated Smith × Schlick, and the
//! Lambertian form factor), over roughness, view angle, size and placement.

// The reference's (and the maths') single-letter names, kept for side-by-side reading.
#![allow(clippy::many_single_char_names)]

use super::twin::*;
use super::{LTC_LAYER_BYTES, LTC_SIZE, LTC_TABLE};
use moba_core::math::Vec3;

const PI: f32 = core::f32::consts::PI;

/// A small fixed-seed generator (PCG32), so the reference is the same on every run.
struct Pcg(u64);

impl Pcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        #[allow(clippy::cast_possible_truncation)] // The PCG output permutation.
        let xorshifted = (((self.0 >> 18) ^ self.0) >> 27) as u32;
        #[allow(clippy::cast_possible_truncation)]
        let rot = (self.0 >> 59) as u32;
        let bits = xorshifted.rotate_right(rot);
        #[allow(clippy::cast_precision_loss)] // 24 bits.
        let unit = (bits >> 8) as f32 / (1u32 << 24) as f32;
        unit
    }
}

/// The shapes under test, relative to the shaded point (world axes).
#[derive(Clone, Copy, Debug)]
enum Shape {
    Sphere { centre: Vec3, radius: f32 },
    Capsule { p1: Vec3, p2: Vec3, radius: f32 },
    Rect { centre: Vec3, ex: Vec3, ey: Vec3 },
}

impl Shape {
    fn bounds(self) -> (Vec3, f32) {
        match self {
            Self::Sphere { centre, radius } => (centre, radius),
            Self::Capsule { p1, p2, radius } => {
                ((p1 + p2) * 0.5, (p2 - p1).length() * 0.5 + radius)
            }
            Self::Rect { centre, ex, ey } => (centre, (ex + ey).length().max((ex - ey).length())),
        }
    }

    /// Whether the ray from the shaded point along unit `w` hits the shape's emitting side.
    fn hit(self, w: Vec3) -> bool {
        match self {
            Self::Sphere { centre, radius } => {
                let t = centre.dot(w);
                t > 0.0 && (centre - w * t).length_squared() <= radius * radius
            }
            Self::Capsule { p1, p2, radius } => ray_segment_distance2(w, p1, p2) <= radius * radius,
            Self::Rect { centre, ex, ey } => {
                let normal = ex.cross(ey);
                let denominator = w.dot(normal);
                if denominator >= 0.0 {
                    return false; // Parallel, or the back face.
                }
                let t = centre.dot(normal) / denominator;
                if t <= 0.0 {
                    return false;
                }
                let local = w * t - centre;
                local.dot(ex).abs() <= ex.length_squared()
                    && local.dot(ey).abs() <= ey.length_squared()
            }
        }
    }

    fn evaluate(self, frame: &Frame, minv: Minv) -> f32 {
        match self {
            Self::Sphere { centre, radius } => sphere(frame, minv, centre, radius),
            Self::Capsule { p1, p2, radius } => capsule(frame, minv, p1, p2, radius),
            Self::Rect { centre, ex, ey } => rect(frame, minv, centre, ex, ey),
        }
    }
}

/// The squared distance from the ray `t w` (t ≥ 0) to the segment `p1 p2`: Ericson's
/// closest points between two segments (Real-Time Collision Detection §5.1.9), the ray cut
/// at 1 km.
fn ray_segment_distance2(w: Vec3, p1: Vec3, p2: Vec3) -> f32 {
    let d1 = w * 1000.0;
    let d2 = p2 - p1;
    let r = -p1;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    let c = d1.dot(r);
    let b = d1.dot(d2);
    let denominator = a * e - b * b;
    let mut s = if denominator > 0.0 {
        ((b * f - c * e) / denominator).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut t = (b * s + f) / e;
    if t < 0.0 {
        t = 0.0;
        s = (-c / a).clamp(0.0, 1.0);
    } else if t > 1.0 {
        t = 1.0;
        s = ((b - c) / a).clamp(0.0, 1.0);
    }
    (d1 * s - (p1 + d2 * t)).length_squared()
}

/// The engine's specular lobe times N·L at `l` (local frame, `n` = +z), with F0 = `f0`:
/// D × Vis × F × N·L (`lighting.glsl` without its π, which is the point light's I = πC).
fn engine_specular(v: Vec3, l: Vec3, alpha: f32, f0: f32) -> f32 {
    let n_dot_l = l.z;
    if n_dot_l <= 0.0 {
        return 0.0;
    }
    let n_dot_v = v.z.max(1e-4);
    let h = (v + l).normalize();
    let alpha2 = alpha * alpha;
    let d = {
        let x = h.z * h.z * (alpha2 - 1.0) + 1.0;
        alpha2 / (PI * x * x)
    };
    let vis = {
        let ggx_v = n_dot_l * (n_dot_v * n_dot_v * (1.0 - alpha2) + alpha2).sqrt();
        let ggx_l = n_dot_v * (n_dot_l * n_dot_l * (1.0 - alpha2) + alpha2).sqrt();
        0.5 / (ggx_v + ggx_l).max(1e-5)
    };
    let fresnel = f0 + (1.0 - f0) * (1.0 - v.dot(h).max(0.0)).powi(5);
    d * vis * fresnel * n_dot_l
}

/// The Monte Carlo reference: `(form factor, specular at F0 = 0.04, specular at F0 = 1)`,
/// by stratified jittered directions over the cone bounding the shape, each traced
/// against the shape.
fn reference(
    shape: Shape,
    frame: &Frame,
    v: Vec3,
    alpha: f32,
    minv: Minv,
    strata: u32,
) -> [f64; 4] {
    let (centre, bound) = shape.bounds();
    let distance = centre.length();
    assert!(
        distance > bound,
        "the reference needs the point outside the bounding sphere"
    );
    let cos_max = (1.0 - (bound / distance).powi(2)).max(0.0).sqrt();
    let axis = centre / distance;
    let (b1, b2) = axis.any_orthonormal_pair();
    let solid_angle = 2.0 * f64::from(PI) * f64::from(1.0 - cos_max);
    let mut rng = Pcg(0x5eed_1234_abcd_0001);
    let mut sums = [0.0f64; 4];
    for i in 0..strata {
        for j in 0..strata {
            #[allow(clippy::cast_precision_loss)]
            let u1 = (i as f32 + rng.next()) / strata as f32;
            #[allow(clippy::cast_precision_loss)]
            let u2 = (j as f32 + rng.next()) / strata as f32;
            let cos_theta = 1.0 - u1 * (1.0 - cos_max);
            let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
            let phi = 2.0 * PI * u2;
            let w = axis * cos_theta + (b1 * phi.cos() + b2 * phi.sin()) * sin_theta;
            if !shape.hit(w) {
                continue;
            }
            let l = frame.local(w);
            if l.z <= 0.0 {
                continue;
            }
            sums[0] += f64::from(l.z / PI);
            sums[1] += f64::from(engine_specular(v, l, alpha, 0.04));
            sums[2] += f64::from(engine_specular(v, l, alpha, 1.0));
            sums[3] += f64::from(distribution(minv, l));
        }
    }
    let count = f64::from(strata * strata);
    sums.map(|sum| sum * solid_angle / count)
}

/// The LTC estimate of the same three integrals.
fn estimate(shape: Shape, frame: &Frame, n_dot_v: f32, roughness: f32) -> [f64; 3] {
    let diffuse = shape.evaluate(frame, Minv::IDENTITY);
    let (minv, [magnitude, fresnel]) = lobe(n_dot_v, roughness);
    let lobe_integral = shape.evaluate(frame, minv);
    let specular = |f0: f32| lobe_integral * (f0 * magnitude + (1.0 - f0) * fresnel);
    [diffuse, specular(0.04), specular(1.0)].map(f64::from)
}

/// The cases: a shaded point at the origin facing +y (the engine's up), viewed from `view`
/// degrees off the normal, with each shape `distance` away along a direction `elevation`
/// degrees above the horizon.
fn cases() -> Vec<(String, Shape, f32, f32)> {
    let mut cases = Vec::new();
    for roughness in [0.15f32, 0.3, 0.5, 0.75, 1.0] {
        for view in [0.0f32, 40.0, 75.0] {
            for (elevation, azimuth) in
                [(80.0f32, 0.0f32), (45.0, 180.0), (30.0, 0.0), (20.0, 90.0)]
            {
                let (el, az) = (elevation.to_radians(), azimuth.to_radians());
                let direction = Vec3::new(el.cos() * az.cos(), el.sin(), el.cos() * az.sin());
                for (size, distance) in [(0.25f32, 3.0f32), (0.75, 2.5)] {
                    let centre = direction * distance;
                    let tag =
                        format!("r{roughness} v{view} e{elevation}/{azimuth} s{size} d{distance}");
                    cases.push((
                        format!("sphere {tag}"),
                        Shape::Sphere {
                            centre,
                            radius: size,
                        },
                        roughness,
                        view,
                    ));
                    // A capsule lying across the direction to it, tilted.
                    let across = direction.cross(Vec3::Y).normalize_or(Vec3::X);
                    let tilt = (across + Vec3::Y * 0.4).normalize() * (size * 2.0);
                    cases.push((
                        format!("capsule {tag}"),
                        Shape::Capsule {
                            p1: centre - tilt,
                            p2: centre + tilt,
                            radius: size * 0.4,
                        },
                        roughness,
                        view,
                    ));
                    // A rect facing the point, turned 30 degrees about its vertical.
                    let facing = -direction;
                    let ex = facing.cross(Vec3::Y).normalize_or(Vec3::X);
                    let ey = ex.cross(facing);
                    let turn = 30f32.to_radians();
                    let normal = facing * turn.cos() + ex * turn.sin();
                    let ex = ey.cross(normal);
                    cases.push((
                        format!("rect {tag}"),
                        Shape::Rect {
                            centre,
                            ex: ex * size * 1.5,
                            ey: ey * size,
                        },
                        roughness,
                        view,
                    ));
                }
            }
        }
    }
    cases
}

fn frame_for(view_degrees: f32) -> (Frame, Vec3, f32) {
    let n = Vec3::Y;
    let angle = view_degrees.to_radians();
    let v = Vec3::new(angle.sin(), angle.cos(), 0.0);
    let frame = Frame::new(n, v);
    (frame, frame.local(v), v.dot(n))
}

/// The LTC distribution at the unit direction `w` (local frame), per steradian:
/// `D_o(Minv w / |Minv w|) |det Minv| / |Minv w|³` with `D_o` the clamped cosine.
fn distribution(minv: Minv, w: Vec3) -> f32 {
    let o = minv.apply(w);
    let length = o.length();
    let [a, b, c, d] = minv.0;
    (o.z / length).max(0.0) / PI * (a * d - b * c).abs() / (length * length * length)
}

/// The worst errors of one shape over every case.
#[derive(Debug, Default)]
struct Worst {
    /// The analytic integral of the LTC lobe against Monte Carlo of the same lobe: the
    /// evaluation (and, for the capsule, its quad) alone, relative, floored at 0.02.
    evaluation: f64,
    /// The form factor (the diffuse) against Monte Carlo, relative, floored at 0.001.
    diffuse: f64,
    /// The specular at F0 = 0.04 and 1 against Monte Carlo of the engine's lobe, as a
    /// fraction of that lobe's whole reflected energy (`F0 × magnitude + (1 − F0) × Fresnel`):
    /// the table's fit and the evaluation together.
    specular: f64,
    cases: [String; 3],
}

/// The recorded tolerances (ADR-0023 amendment 4), per shape `(evaluation, diffuse,
/// specular)`: the worst measured over `cases()` (sphere 0.058 / 0.0002 / 0.099, capsule
/// 0.145 / 0.042 / 0.056, rect 0.116 / 0.002 / 0.105) with ~15% headroom. The reference is
/// fixed-seed, so the margin only absorbs `f32` differences between targets. What the
/// errors are: the evaluation's is the clipless method's horizon (the sphere table stands
/// in for clipping the polygon or ellipse in LTC space; worst on glossy lobes at grazing
/// view) plus, for the capsule, its quad's square ends; the specular's adds the table's fit
/// of GGX, which is loosest at grazing view (a lobe's peak there sits ~5° from GGX's).
const TOLERANCE: [(&str, [f64; 3]); 3] = [
    ("sphere", [0.07, 0.002, 0.12]),
    ("capsule", [0.17, 0.05, 0.07]),
    ("rect", [0.13, 0.003, 0.12]),
];

#[test]
fn ltc_matches_monte_carlo() {
    let report = std::env::var_os("MOBA_LTC_REPORT").is_some();
    let mut worst: Vec<(&str, Worst)> = TOLERANCE
        .iter()
        .map(|(name, _)| (*name, Worst::default()))
        .collect();
    for (name, shape, roughness, view) in cases() {
        let (frame, v_local, n_dot_v) = frame_for(view);
        let alpha = roughness * roughness;
        let (minv, [magnitude, fresnel]) = lobe(n_dot_v, roughness);
        // Glossy lobes are narrow: more strata so that the reference resolves them.
        let strata = if roughness <= 0.3 { 400 } else { 160 };
        let reference = reference(shape, &frame, v_local, alpha, minv, strata);
        let estimate = estimate(shape, &frame, n_dot_v, roughness);
        let evaluation = f64::from(shape.evaluate(&frame, minv));
        let energy = |f0: f32| f64::from(f0 * magnitude + (1.0 - f0) * fresnel);
        let errors = [
            (evaluation - reference[3]).abs() / reference[3].max(0.02),
            (estimate[0] - reference[0]).abs() / reference[0].max(0.001),
            ((estimate[1] - reference[1]).abs() / energy(0.04))
                .max((estimate[2] - reference[2]).abs() / energy(1.0)),
        ];
        if report {
            eprintln!(
                "{name}: diffuse {:.5} vs {:.5}, spec(0.04) {:.5} vs {:.5}, spec(1) {:.5} vs {:.5}, lobe {:.5} vs {:.5}",
                estimate[0],
                reference[0],
                estimate[1],
                reference[1],
                estimate[2],
                reference[2],
                evaluation,
                reference[3]
            );
        }
        let (_, entry) = worst
            .iter_mut()
            .find(|(shape_name, _)| name.starts_with(shape_name))
            .expect("a known shape");
        for (k, error) in errors.into_iter().enumerate() {
            let slot = match k {
                0 => &mut entry.evaluation,
                1 => &mut entry.diffuse,
                _ => &mut entry.specular,
            };
            if error > *slot {
                *slot = error;
                entry.cases[k] = name.clone();
            }
        }
    }
    for ((name, entry), (_, tolerance)) in worst.iter().zip(TOLERANCE) {
        eprintln!(
            "{name}: evaluation {:.4} ({}), diffuse {:.4} ({}), specular {:.4} ({})",
            entry.evaluation,
            entry.cases[0],
            entry.diffuse,
            entry.cases[1],
            entry.specular,
            entry.cases[2]
        );
        assert!(
            entry.evaluation <= tolerance[0],
            "{name}: evaluation {:.4} > {}",
            entry.evaluation,
            tolerance[0]
        );
        assert!(
            entry.diffuse <= tolerance[1],
            "{name}: diffuse {:.4} > {}",
            entry.diffuse,
            tolerance[1]
        );
        assert!(
            entry.specular <= tolerance[2],
            "{name}: specular {:.4} > {}",
            entry.specular,
            tolerance[2]
        );
    }
}

/// The radius → 0 limit (ADR-0023 §9b gate 3): a small sphere of the same power (radiance
/// `C / R²`) lights like the point light at its centre: the diffuse exactly the point's
/// `C cos θ / d²`, the specular the LTC lobe's point value `π × magnitude × D(w) / d²`.
#[test]
fn a_shrinking_sphere_tends_to_the_point_light() {
    for (roughness, view) in [(0.3f32, 0.0f32), (0.5, 40.0), (0.8, 75.0)] {
        let (frame, _, n_dot_v) = frame_for(view);
        let (minv, [magnitude, _]) = lobe(n_dot_v, roughness);
        for direction in [
            Vec3::new(0.3, 1.0, 0.2),
            Vec3::new(-0.8, 0.5, 0.1),
            Vec3::new(0.1, 0.4, -0.9),
        ] {
            let centre = direction.normalize() * 2.0;
            let w = frame.local(centre).normalize();
            let point_diffuse = w.z / 4.0;
            let point_specular = PI * magnitude * distribution(minv, w) / 4.0;
            let mut previous = [f32::INFINITY; 2];
            for radius in [0.2f32, 0.05, 0.01, 1e-3, 1e-5] {
                let r2 = radius * radius;
                let diffuse = sphere(&frame, Minv::IDENTITY, centre, radius) / r2;
                let specular = sphere(&frame, minv, centre, radius) * magnitude / r2;
                let errors = [
                    (diffuse - point_diffuse).abs() / point_diffuse,
                    (specular - point_specular).abs() / point_specular,
                ];
                // Converging: each step closer than the last, and close at the end.
                for k in 0..2 {
                    assert!(
                        errors[k] <= previous[k] + 2e-3,
                        "r{roughness} v{view} R{radius}: {errors:?}"
                    );
                }
                previous = errors;
            }
            assert!(
                previous[0] < 2e-3 && previous[1] < 0.02,
                "r{roughness} v{view}: {previous:?}"
            );
        }
    }
}

/// The capsule's limits: with no length it is the sphere, and a long thin one is lit along
/// its whole length (the quad's ends are where the capsule's are).
#[test]
fn a_capsule_without_length_is_its_sphere() {
    let (frame, _, n_dot_v) = frame_for(40.0);
    let (minv, _) = lobe(n_dot_v, 0.5);
    let centre = Vec3::new(0.4, 2.0, -0.3);
    for radius in [0.1f32, 0.3] {
        for m in [Minv::IDENTITY, minv] {
            let capsule = capsule(&frame, m, centre, centre, radius);
            let sphere = sphere(&frame, m, centre, radius);
            assert!(
                (capsule - sphere).abs() <= 0.05 * sphere,
                "R{radius}: {capsule} vs {sphere}"
            );
        }
    }
}

/// The half-precision table: every texel finite, `ltc_1`'s inverse matrices invertible
/// (positive determinant: no mirroring), magnitudes and Fresnel in 0..=1, the sphere table
/// in 0..=1.
#[test]
fn the_table_is_well_formed() {
    assert_eq!(LTC_TABLE.len(), 2 * LTC_LAYER_BYTES);
    for y in 0..LTC_SIZE {
        for x in 0..LTC_SIZE {
            let t1 = texel(0, x, y);
            let t2 = texel(1, x, y);
            assert!(t1.iter().chain(&t2).all(|v| v.is_finite()), "{x},{y}");
            assert!(Minv(t1).determinant() > 0.0, "{x},{y}: {t1:?}");
            assert!(
                (0.0..=1.0).contains(&t2[0]) && (0.0..=1.0).contains(&t2[1]),
                "{x},{y}: {t2:?}"
            );
            assert!((0.0..=1.0001).contains(&t2[3]), "{x},{y}: {t2:?}");
        }
    }
}

/// The shaders' constants match the Rust ones: the atlas size `ltc.glsl` divides by, the
/// split-sum LUT's corner of it, and where the shapes start in the light list.
#[test]
fn the_shader_constants_match() {
    let lighting = include_str!("../../../shaders/shaders/include/lighting.glsl");
    let ltc = include_str!("../../../shaders/shaders/include/ltc.glsl");
    let [width, height] = super::LUT_ATLAS_EXTENT;
    assert!(lighting.contains(&format!(
        "#define MOBA_LUT_ATLAS vec2({width}.0, {height}.0)"
    )));
    assert_eq!(
        width as usize,
        crate::lighting::BRDF_LUT_SIZE as usize + 2 * LTC_SIZE
    );
    assert!(ltc.contains(&format!("#define MOBA_LTC_SIZE {LTC_SIZE}.0")));
    assert!(ltc.contains(&format!(
        "#define MOBA_LTC_MIN_SPAN {}",
        super::twin::MIN_SPAN
    )));
    assert!(lighting.contains(&format!(
        "#define MOBA_LIGHT_SHAPE_BASE {}u",
        crate::lights::SHAPE_BASE
    )));
}

/// The atlas holds the split-sum LUT at its origin and the two layers side by side, texel
/// for texel.
#[test]
fn the_atlas_holds_both_tables() {
    let lut = crate::lighting::brdf_lut_tiered();
    let size = crate::lighting::BRDF_LUT_SIZE as usize;
    let atlas = super::lut_atlas(lut, size);
    let width = super::LUT_ATLAS_EXTENT[0] as usize;
    assert_eq!(atlas.len(), width * LTC_SIZE * 8);
    let at = |x: usize, y: usize| &atlas[(y * width + x) * 8..(y * width + x) * 8 + 8];
    for (x, y) in [(0, 0), (31, 31), (5, 17)] {
        assert_eq!(
            &at(x, y)[..4],
            &lut[(y * size + x) * 4..(y * size + x) * 4 + 4]
        );
        assert_eq!(&at(x, y)[4..], &[0; 4]);
    }
    assert_eq!(at(3, 40), &[0; 8]);
    for (layer, x, y) in [(0, 0, 0), (0, 63, 63), (1, 0, 0), (1, 63, 63), (1, 17, 42)] {
        let from = layer * LTC_LAYER_BYTES + (y * LTC_SIZE + x) * 8;
        assert_eq!(
            at(size + layer * LTC_SIZE + x, y),
            &LTC_TABLE[from..from + 8]
        );
    }
}
