//! Gate 3 (ADR-0023's verification gates): each shape's LTC evaluation against a Monte Carlo reference of the
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
    /// The evaluation error: the analytic integral of the LTC lobe against Monte Carlo of the
    /// same lobe (the evaluation and, for the capsule, its quad, without the table's fit),
    /// `|LTC − MC| / max(MC, 0.02)`: relative above 0.02, absolute (over 0.02) below, which
    /// is most cases.
    evaluation: f64,
    /// The diffuse error: the form factor against Monte Carlo, `|LTC − MC| / max(MC, 0.001)`.
    diffuse: f64,
    /// The specular error as a fraction of the lobe's energy: `|LTC − MC|` at F0 = 0.04 and
    /// 1, over that lobe's whole reflected energy (`F0 × magnitude + (1 − F0) × Fresnel`):
    /// the table's fit and the evaluation together. Not a relative error.
    specular: f64,
    /// The plain relative errors `|LTC − MC| / MC` of the evaluation, the diffuse and the
    /// specular (both F0), over the cases whose reference is at least 0.01.
    relative: [f64; 3],
    cases: [String; 3],
}

/// The recorded tolerances (ADR-0023 amendment 4), per shape `(evaluation, diffuse,
/// specular)` as `Worst` defines them: the worst measured over `cases()` (sphere 0.058 /
/// 0.0002 / 0.099, capsule 0.145 / 0.042 / 0.056, rect 0.116 / 0.002 / 0.105) with ~15%
/// headroom. The reference is fixed-seed, so the margin only absorbs `f32` differences
/// between targets; it is a regression bound, not the evidence for `RGBA16F` (that is
/// `the_half_table_integrates_as_the_full_one`). What the errors are: the evaluation's is the
/// clipless method's horizon (the sphere table stands in for clipping the polygon or ellipse
/// in LTC space; worst on glossy lobes at grazing view) plus, for the capsule, its quad's
/// square ends; the specular's adds the table's fit of GGX, which is loosest at grazing view
/// (a lobe's peak there sits ~5° from GGX's).
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
        let plain = |estimate: f64, reference: f64| {
            if reference >= 0.01 {
                (estimate - reference).abs() / reference
            } else {
                0.0
            }
        };
        let relative = [
            plain(evaluation, reference[3]),
            plain(estimate[0], reference[0]),
            plain(estimate[1], reference[1]).max(plain(estimate[2], reference[2])),
        ];
        for (k, error) in relative.into_iter().enumerate() {
            entry.relative[k] = entry.relative[k].max(error);
        }
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
            "{name}: evaluation error {:.4} ({}), diffuse error {:.4} ({}), specular error \
             (fraction of the lobe's energy) {:.4} ({}); plain relative over references >= \
             0.01: evaluation {:.3}, diffuse {:.3}, specular {:.3}",
            entry.evaluation,
            entry.cases[0],
            entry.diffuse,
            entry.cases[1],
            entry.specular,
            entry.cases[2],
            entry.relative[0],
            entry.relative[1],
            entry.relative[2]
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

/// The LTC evaluation's own radius → 0 limit: a small sphere of the same power (radiance
/// `C / R²`) evaluated by LTC tends to its centre's value under the same lobe: the diffuse
/// exactly the point's `C cos θ / d²`, the specular the LTC lobe's point value
/// `π × magnitude × D(w) / d²` (not the engine's GGX point light, which differs by the
/// table's fit: gate 3's limit, as worded, is
/// `a_shrinking_sphere_tends_to_the_engine_point_light`).
#[test]
fn the_ltc_sphere_tends_to_its_lobe_point_value() {
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
    // The shading twin's constants (`shade.rs`).
    for (name, value) in [
        ("MOBA_LTC_SPAN_LOW", super::shade::SPAN_LOW),
        ("MOBA_LTC_SPAN_HIGH", super::shade::SPAN_HIGH),
        ("MOBA_AREA_WIDEN", super::shade::WIDEN),
    ] {
        assert!(
            lighting.contains(&format!("#define {name} {value}")),
            "{name} {value}"
        );
    }
    assert!(lighting.contains(&format!(
        "const float MOBA_LIGHT_NEAR_FIELD = {:.1};",
        super::shade::NEAR_FIELD
    )));
    // The LTC radiance keeps ADR-0016's near-field floor.
    assert!(lighting.contains("/ max(d2, MOBA_LIGHT_NEAR_FIELD));"));
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

/// The sphere's lobe where the transformed disk is (nearly) a circle (ADR-0023 amendment 4
/// item 3): the ellipse's axes come from one stable rotation and the cubic's square roots
/// are clamped, so `f32` holds there. Found in review: the two-branch eigenvectors and an
/// unclamped discriminant were up to 3.7× off within ±1.7° of that direction (N·V 0.8,
/// roughness 1) and 0 on the axis at N·V 1, roughness 0.5 (`f64` 0.064).
#[test]
fn the_sphere_is_stable_where_its_disk_is_circular() {
    let mut failures = Vec::new();
    for (n_dot_v, roughness) in [(0.8f32, 0.85f32), (0.8, 1.0), (1.0, 0.5)] {
        let view = n_dot_v.acos().to_degrees();
        let (frame, v_local, n_dot_v) = frame_for(view);
        let alpha = roughness * roughness;
        let (minv, _) = lobe(n_dot_v, roughness);
        // The direction (in the plane of N and V) where the transformed disk is circular.
        let anisotropy = |theta: f32| {
            let c = Vec3::new(theta.sin(), 0.0, theta.cos());
            let e1 = c
                .cross(if c.x.abs() < 0.9 { Vec3::X } else { Vec3::Y })
                .normalize();
            let e2 = c.cross(e1);
            let (v1, v2) = (minv.apply(e1), minv.apply(e2));
            let (d11, d22, d12) = (v1.dot(v1), v2.dot(v2), v1.dot(v2));
            (d11 - d22).powi(2) + 4.0 * d12 * d12
        };
        let mut circular = 0.0f32;
        for step in -1500..=1500 {
            #[allow(clippy::cast_precision_loss)]
            let theta = step as f32 * 0.001;
            if theta.cos() > 0.02 && anisotropy(theta) < anisotropy(circular) {
                circular = theta;
            }
        }
        let mut worst = (0.0f64, 0.0f32, 0.0f32);
        for i in -3i8..=3 {
            for j in -3i8..=3 {
                let (d_theta, dy) = (f32::from(i) * 0.01, f32::from(j) * 0.01);
                let theta = circular + d_theta;
                let local = Vec3::new(theta.sin(), dy, theta.cos()).normalize() * 2.0;
                let centre = frame.t1 * local.x + frame.t2 * local.y + frame.n * local.z;
                let shape = Shape::Sphere {
                    centre,
                    radius: 0.25,
                };
                let reference = reference(shape, &frame, v_local, alpha, minv, 160)[3];
                let evaluation = f64::from(sphere(&frame, minv, centre, 0.25));
                let error = (evaluation - reference).abs() / reference.max(0.02);
                if i == 0 && j == 0 {
                    eprintln!("  at the circular direction: {evaluation:.5} vs {reference:.5}");
                }
                if error > worst.0 {
                    worst = (error, d_theta, dy);
                }
            }
        }
        eprintln!(
            "N·V {n_dot_v} r{roughness}: circular at {circular:+.3} rad, worst {:.4} at ({:+.2}, {:+.2})",
            worst.0, worst.1, worst.2
        );
        if worst.0 > TOLERANCE[0].1[0] {
            failures.push(format!("N·V {n_dot_v} r{roughness}: {worst:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

use super::shade::{self, LightIn, Surface};

/// A dielectric (`STANDARD`'s IOR 1.5 F0) without wrap, at `roughness`.
fn dielectric(roughness: f32, wrap: f32) -> Surface {
    Surface {
        albedo: 0.8,
        metallic: 0.0,
        f0: 0.04,
        wrap,
        roughness,
    }
}

/// A sphere light of `radius` at `centre` (relative to the shaded point), reaching 8 m.
fn sphere_light(centre: Vec3, radius: f32) -> LightIn {
    LightIn {
        to_light: centre,
        radius: 8.0,
        color: 1.0,
        tag: shade::SPHERE,
        shape: [0.0, 0.0, 0.0, radius],
    }
}

/// Gate 3's limit, as worded (ADR-0023 §9b): a sphere light's result tends to the point
/// light's as its radius tends to 0, the engine's point light itself (GGX × Smith × Schlick
/// at the half vector, the wrapped diffuse, the 1 m near-field floor), not the LTC lobe's
/// point value. Below `SPAN_LOW` a shape is lit at its representative point (amendment 4
/// item 15), which converges to the centre, with its normalisation to 1.
#[test]
fn a_shrinking_sphere_tends_to_the_engine_point_light() {
    for (roughness, view, wrap, metallic) in [
        (0.15f32, 0.0f32, 0.0f32, 0.0f32),
        (0.3, 40.0, 0.3, 0.0),
        (0.5, 40.0, 0.0, 1.0),
        (0.8, 75.0, 0.3, 0.0),
    ] {
        let (frame, _, _) = frame_for(view);
        let n = frame.n;
        let v = frame.t1 * (view.to_radians().sin()) + n * view.to_radians().cos();
        let surface = Surface {
            metallic,
            ..dielectric(roughness, wrap)
        };
        for direction in [
            Vec3::new(0.3, 1.0, 0.2),
            Vec3::new(-0.8, 0.5, 0.1),
            Vec3::new(0.6, 0.9, -0.1),
        ] {
            for distance in [0.5f32, 2.0] {
                let centre = direction.normalize() * distance;
                let point = shade::point_light(&surface, n, v, &sphere_light(centre, 0.0));
                let mut last = f32::INFINITY;
                for radius in [0.05f32, 0.02, 0.01, 1e-3, 1e-5] {
                    let radius = radius * distance;
                    let lit = shade::light(2, &surface, n, v, &sphere_light(centre, radius));
                    let error = (lit - point).abs() / point.max(1e-3);
                    assert!(
                        error <= last + 1e-3,
                        "r{roughness} v{view} d{distance} R{radius}: {lit} vs {point}"
                    );
                    last = error;
                }
                assert!(
                    last < 2e-3,
                    "r{roughness} v{view} d{distance}: {last} from the point light"
                );
            }
        }
    }
}

/// ADR-0016's near-field floor holds under LTC too: within 1 m a sphere's radiance is scaled
/// by d² / max(d², 1), so its diffuse is the point light's `cos θ / max(d², 1)`, not
/// `cos θ / d²` (4× brighter at 0.5 m before review).
#[test]
fn ltc_keeps_the_near_field_floor() {
    let (frame, _, _) = frame_for(0.0);
    let n = frame.n;
    let v = n;
    // Rough, so the term is the diffuse: the sphere's is its exact form factor.
    let surface = dielectric(1.0, 0.0);
    for distance in [0.3f32, 0.5, 0.8, 2.0] {
        for direction in [Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.5, 1.0, 0.3)] {
            let centre = direction.normalize() * distance;
            // Well above the span cut-off: LTC alone.
            let light = sphere_light(centre, 0.2 * distance);
            assert!(shade::extent(light.tag, light.shape) / distance >= shade::SPAN_HIGH);
            let lit = shade::light(2, &surface, n, v, &light);
            let point = shade::point_light(&surface, n, v, &light);
            assert!(
                (lit / point - 1.0).abs() < 0.08,
                "d{distance}: LTC {lit} against the point light {point}"
            );
        }
    }
}

/// Each shape's radiance scale gives it the point light's total power (ADR-0023 amendment 4
/// item 5): a Lambertian emitter of radiance L and area A radiates π L A, and the point
/// light (intensity π C) 4π² C.
#[test]
fn every_shape_has_the_point_light_power() {
    let (frame, _, _) = frame_for(0.0);
    let far = Vec3::new(0.0, 50.0, 0.0);
    let (radius, half_length, half_width, half_height) = (0.3f32, 1.2f32, 0.9f32, 0.5f32);
    let shapes = [
        (
            shade::SPHERE,
            [0.0, 0.0, 0.0, radius],
            4.0 * PI * radius * radius,
        ),
        (
            shade::CAPSULE,
            [half_length, 0.0, 0.0, radius],
            4.0 * PI * radius * radius + 4.0 * PI * radius * half_length,
        ),
        // Facing down at the point below it.
        (
            shade::RECT,
            [0.0, -half_width, 0.0, half_height],
            4.0 * half_width * half_height,
        ),
    ];
    for (tag, shape, area) in shapes {
        let (_, scale) = shade::area_integrals(tag, shape, far, &frame, Minv::IDENTITY);
        let power = PI * scale * area;
        assert!(
            (power / (4.0 * PI * PI) - 1.0).abs() < 1e-5,
            "tag {tag}: {power}"
        );
    }
}

/// `moba_rect_axes`: orthogonal half-extents of the shape's lengths, `ex × ey` along the
/// emitting normal; the width horizontal (+X under a vertical normal).
#[test]
fn rect_axes_follow_the_normal() {
    for normal in [
        Vec3::new(0.0, -0.6, -1.0),
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
    ] {
        let unit = normal.normalize();
        let shape = [unit.x * 0.9, unit.y * 0.9, unit.z * 0.9, 0.5];
        let (n, ex, ey) = shade::rect_axes(shape);
        assert!((n - unit).length() < 1e-5);
        assert!((ex.length() - 0.9).abs() < 1e-5 && (ey.length() - 0.5).abs() < 1e-5);
        assert!(ex.dot(ey).abs() < 1e-5 && ex.dot(n).abs() < 1e-5);
        assert!(ex.cross(ey).normalize().dot(n) > 0.9999);
        assert!(ex.y.abs() < 1e-5, "the width is horizontal: {ex:?}");
    }
    let (_, ex, _) = shade::rect_axes([0.0, -0.9, 0.0, 0.5]);
    assert!((ex.normalize() - Vec3::X).length() < 1e-5);
}

/// `moba_representative_point`: always on the shape, and on the reflection ray where the ray
/// hits it.
#[test]
fn the_representative_point_is_on_the_shape() {
    let r = Vec3::new(0.2, 1.0, 0.1).normalize();
    let centre = r * 3.0 + Vec3::new(0.15, 0.0, -0.1);
    // Sphere: on its surface, toward the ray, and on the ray where it crosses the sphere.
    let p = shade::representative_point(shade::SPHERE, [0.0, 0.0, 0.0, 0.5], centre, r);
    assert!((p - centre).length() <= 0.5 + 1e-5);
    assert!((p - r * p.dot(r)).length() < 1e-4, "on the ray: {p:?}");
    // Capsule: within its radius of the segment.
    let axis = Vec3::new(1.0, 0.0, 0.3);
    let p = shade::representative_point(shade::CAPSULE, [axis.x, axis.y, axis.z, 0.1], centre, r);
    let t = ((p - (centre - axis)).dot(2.0 * axis) / (4.0 * axis.length_squared())).clamp(0.0, 1.0);
    let on_axis = centre - axis + 2.0 * axis * t;
    assert!((p - on_axis).length() <= 0.1 + 1e-4);
    // Rect: in its plane and within its half-extents.
    let normal = -r;
    let shape = [normal.x * 0.9, normal.y * 0.9, normal.z * 0.9, 0.5];
    let (_, ex, ey) = shade::rect_axes(shape);
    let p = shade::representative_point(shade::RECT, shape, centre, r);
    let local = p - centre;
    assert!(local.dot(normal).abs() < 1e-4);
    assert!(local.dot(ex.normalize()).abs() <= 0.9 + 1e-4);
    assert!(local.dot(ey.normalize()).abs() <= 0.5 + 1e-4);
    assert!(
        (p - r * p.dot(r)).length() < 1e-3,
        "the ray hits the rect: {p:?}"
    );
}

/// A long thin capsule is lit along its whole length: the facing quad's ends are where the
/// capsule's are (beside its tip the form factor is still about half the middle's, past the
/// tip it falls away), and the light's radius covers twice its extent (`Light::sanitized`),
/// so the window at the tip is at least (1 − 1/16)² of the centre's curve.
#[test]
fn a_long_thin_capsule_is_lit_along_its_length() {
    let (frame, _, _) = frame_for(0.0);
    let half = Vec3::new(2.0, 0.0, 0.0);
    let form = |x: f32| {
        let centre = Vec3::new(-x, 0.5, 0.0);
        capsule(&frame, Minv::IDENTITY, centre - half, centre + half, 0.05)
    };
    let (middle, inside_tip, past_tip) = (form(0.0), form(1.9), form(2.6));
    assert!(inside_tip > 0.35 * middle, "{inside_tip} vs {middle}");
    assert!(past_tip < 0.5 * inside_tip, "{past_tip} vs {inside_tip}");
    let light = crate::lights::Light::new(Vec3::ZERO, 0.5, Vec3::ONE).with_shape(
        crate::lights::LightShape::Capsule {
            axis: half,
            radius: 0.05,
        },
    );
    let extent = light.shape.extent();
    let radius = light.sanitized_for_tests().expect("valid").radius;
    assert!(radius >= 2.0 * extent - 1e-5, "{radius} vs {extent}");
}

/// The span cut-off's error (ADR-0023 amendment 4 item 15), recorded: at `SPAN_LOW` (the
/// point light at the centre, or a rect's representative point) and at `SPAN_HIGH` (LTC
/// alone), each shape's whole term (a
/// dielectric, no wrap) against a Monte Carlo reference of the engine's BRDF over the true
/// shape, and the engine's point light at the centre against the same reference. Normalised
/// by the point light's irradiance at normal incidence; relative, floored at 0.02.
#[test]
fn the_span_cut_off_is_recorded() {
    let mut worst: Vec<(String, f64, String)> = Vec::new();
    for (name, shape, roughness, view) in cases() {
        let (frame, v_local, n_dot_v) = frame_for(view);
        let n = frame.n;
        let v = frame.t1 * v_local.x + frame.t2 * v_local.y + n * v_local.z;
        let alpha = roughness * roughness;
        let (minv, _) = lobe(n_dot_v, roughness);
        let surface = dielectric(roughness, 0.0);
        for (span_name, span) in [("low", shade::SPAN_LOW), ("high", shade::SPAN_HIGH)] {
            // The case's shape scaled so its largest half-extent spans `span`.
            let (tag, words, scaled) = scaled_shape(shape, span);
            let (centre, _) = scaled.bounds();
            let light = LightIn {
                to_light: centre,
                radius: 64.0,
                color: 1.0,
                tag,
                shape: words,
            };
            let d2 = centre.length_squared();
            let ratio = d2 / (64.0 * 64.0);
            let unit = (1.0 - ratio * ratio).powi(2) / d2.max(shade::NEAR_FIELD);
            let (_, scale) = shade::area_integrals(tag, words, centre, &frame, Minv::IDENTITY);
            let reference = reference(scaled, &frame, v_local, alpha, minv, 200);
            let h = (v + centre.normalize()).normalize();
            let kd = 1.0 - (0.04 + 0.96 * (1.0 - v.dot(h).max(0.0)).powi(5));
            // The engine's convention: the shape's radiance windowed and floored at its
            // centre's distance, so per `unit` it is `scale × d²` (d ≥ 1 m here).
            let truth = f64::from(scale * d2)
                * (f64::from(kd * surface.albedo) * reference[0] + reference[1]);
            let lit = f64::from(shade::light(2, &surface, n, v, &light) / unit);
            let point = f64::from(shade::point_light(&surface, n, v, &light) / unit);
            let error = |x: f64| (x - truth).abs() / truth.max(0.02);
            let shape_name = name.split(' ').next().unwrap_or_default();
            for (what, value) in [("area", error(lit)), ("point", error(point))] {
                let key = format!("{shape_name} {span_name} {what}");
                match worst.iter_mut().find(|(k, _, _)| *k == key) {
                    Some((_, w, at)) => {
                        if value > *w {
                            *w = value;
                            at.clone_from(&name);
                        }
                    }
                    None => worst.push((key, value, name.clone())),
                }
            }
        }
    }
    for (key, value, at) in &worst {
        eprintln!("{key}: {value:.3} ({at})");
    }
    for (key, value, _) in &worst {
        let limit = CUT_OFF_TOLERANCE
            .iter()
            .find(|(k, _)| key == k)
            .map_or(f64::INFINITY, |(_, limit)| *limit);
        assert!(*value <= limit, "{key}: {value:.3} > {limit}");
    }
}

/// `the_span_cut_off_is_recorded`'s recorded worst errors with ~15% headroom (amendment 4
/// item 15). At `SPAN_LOW` the sphere and capsule are the engine's point light at the centre,
/// the rect its representative point: sphere 0.349, capsule 0.176, rect 0.307; LTC alone at
/// `SPAN_HIGH`: sphere 0.267, capsule 0.273, rect 0.280 (the point light at the centre there:
/// 0.344, 0.228 and 0.786). The representative point at `SPAN_LOW` (Low's path) read sphere
/// 0.449, capsule 0.303 and rect 0.307 with its normalisation, 8.69, 0.62 and 4.35 without.
/// All are the worst over `cases()`: glossy lobes (roughness 0.15) seen at 40° with the
/// highlight on the shape, or grazing views at 75°.
const CUT_OFF_TOLERANCE: [(&str, f64); 6] = [
    ("sphere low area", 0.40),
    ("sphere high area", 0.31),
    ("capsule low area", 0.21),
    ("capsule high area", 0.32),
    ("rect low area", 0.36),
    ("rect high area", 0.33),
];

/// `shape` scaled about the shaded point's view so that its largest half-extent spans
/// `span` of its centre's distance: `(tag, shape words, the scaled shape)`.
fn scaled_shape(shape: Shape, span: f32) -> (f32, [f32; 4], Shape) {
    match shape {
        Shape::Sphere { centre, .. } => {
            let radius = span * centre.length();
            (
                shade::SPHERE,
                [0.0, 0.0, 0.0, radius],
                Shape::Sphere { centre, radius },
            )
        }
        Shape::Capsule { p1, p2, radius } => {
            let centre = (p1 + p2) * 0.5;
            let half = (p2 - p1) * 0.5;
            let k = span * centre.length() / (half.length() + radius);
            let (half, radius) = (half * k, radius * k);
            (
                shade::CAPSULE,
                [half.x, half.y, half.z, radius],
                Shape::Capsule {
                    p1: centre - half,
                    p2: centre + half,
                    radius,
                },
            )
        }
        Shape::Rect { centre, ex, ey } => {
            let k = span * centre.length() / ex.length().hypot(ey.length());
            let (ex, ey) = (ex * k, ey * k);
            // The shader's rect: the normal times the half-width, and the width horizontal.
            let normal = ex.cross(ey).normalize();
            let words = [
                normal.x * ex.length(),
                normal.y * ex.length(),
                normal.z * ex.length(),
                ey.length(),
            ];
            let (_, sx, sy) = shade::rect_axes(words);
            (
                shade::RECT,
                words,
                Shape::Rect {
                    centre,
                    ex: sx,
                    ey: sy,
                },
            )
        }
    }
}

/// The `RGBA16F` decision's evidence (gate 3): every shape's diffuse and lobe integrals over
/// `cases()` with the committed half-precision table against the same with `ltc-bake`'s
/// full-precision output (`ltc-bake <out.bin> <full.bin>`; not committed, so run by hand
/// with `MOBA_LTC_FULL_TABLE=<full.bin>`). Recorded in perf/0038: the largest difference of
/// an integral, absolute and relative (over integrals of at least 0.01).
#[test]
#[ignore = "needs ltc-bake's full-precision table in MOBA_LTC_FULL_TABLE"]
fn the_half_table_integrates_as_the_full_one() {
    let path = std::env::var_os("MOBA_LTC_FULL_TABLE").expect("MOBA_LTC_FULL_TABLE");
    let bytes = std::fs::read(path).expect("the full-precision table");
    assert_eq!(bytes.len(), 2 * LTC_SIZE * LTC_SIZE * 16);
    let full: Vec<[f32; 4]> = bytes
        .chunks_exact(16)
        .map(|texel| {
            core::array::from_fn(|i| {
                f32::from_le_bytes(texel[4 * i..4 * i + 4].try_into().expect("4 bytes"))
            })
        })
        .collect();
    let integrals = |shape: Shape, view: f32, roughness: f32| {
        let (frame, _, n_dot_v) = frame_for(view);
        let (minv, [magnitude, fresnel]) = lobe(n_dot_v, roughness);
        let lobe_integral = shape.evaluate(&frame, minv);
        [
            shape.evaluate(&frame, Minv::IDENTITY),
            lobe_integral * magnitude,
            lobe_integral * fresnel,
        ]
    };
    let (mut absolute, mut relative) = ([0.0f32; 3], [0.0f32; 3]);
    for (_, shape, roughness, view) in cases() {
        let half = integrals(shape, view, roughness);
        FULL_TABLE.set(Some(full.clone()));
        let reference = integrals(shape, view, roughness);
        FULL_TABLE.set(None);
        for k in 0..3 {
            let error = (half[k] - reference[k]).abs();
            absolute[k] = absolute[k].max(error);
            if reference[k] >= 0.01 {
                relative[k] = relative[k].max(error / reference[k]);
            }
        }
    }
    eprintln!(
        "half against full precision over cases(): largest absolute difference (diffuse, \
         lobe × magnitude, lobe × Fresnel) {absolute:?}, relative {relative:?}"
    );
    assert!(relative.iter().all(|&error| error < 0.01), "{relative:?}");
}
