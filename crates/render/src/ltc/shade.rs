//! The Rust twin of one shaped light's term in `lighting.glsl`'s `dynamic_lights` (Tiered,
//! a `STANDARD` surface, grey light and albedo, the multiscatter compensation and the
//! specular tint at 1, one light so the irradiance cap never applies): the radiance scales,
//! `moba_rect_axes`, `moba_representative_point`, its specular normalisation, the LTC span
//! cut-off and blend, and the point path the cut-off hands over to. The gate-3 tests hold it
//! against the engine's point light and against a Monte Carlo reference.

use moba_core::math::Vec3;

use super::twin::{self, Frame, Minv};

const PI: f32 = core::f32::consts::PI;

/// `MOBA_LIGHT_NEAR_FIELD` (ADR-0016): the inverse-square falloff's 1 m floor.
pub(crate) const NEAR_FIELD: f32 = 1.0;

/// `MOBA_LTC_SPAN_LOW` and `MOBA_LTC_SPAN_HIGH` (`lighting.glsl`): a shaped light whose
/// largest half-extent spans less than `SPAN_LOW` of its centre's distance is lit at its
/// representative point; from `SPAN_HIGH` with LTC; between, a smoothstep blend of the two
/// (ADR-0023 amendment 4 item 15).
pub(crate) const SPAN_LOW: f32 = 0.08;
pub(crate) const SPAN_HIGH: f32 = 0.12;

/// `MOBA_AREA_WIDEN` (`lighting.glsl`): the representative point's lobe widening per unit of
/// angular half-extent, `α' = clamp(α + k e / d, 0, 1)` per axis of the shape (Karis 2013's
/// normalisation; `k` measured against Monte Carlo, ADR-0023 amendment 4 item 8).
pub(crate) const WIDEN: f32 = 0.5;

/// The shape tags (`LightShape::tag`).
pub(crate) const SPHERE: f32 = 1.0;
pub(crate) const CAPSULE: f32 = 2.0;
pub(crate) const RECT: f32 = 3.0;

/// The surface: grey albedo, metallic, the dielectric F0, wrap and perceptual roughness
/// (`alpha` is its square, as `shade()` passes it).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Surface {
    pub(crate) albedo: f32,
    pub(crate) metallic: f32,
    pub(crate) f0: f32,
    pub(crate) wrap: f32,
    pub(crate) roughness: f32,
}

/// One light as the shader reads it: from the shaded point to its centre, its (binned)
/// radius, grey colour, shape tag and shape `vec4` (`LightShape::words`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct LightIn {
    pub(crate) to_light: Vec3,
    pub(crate) radius: f32,
    pub(crate) color: f32,
    pub(crate) tag: f32,
    pub(crate) shape: [f32; 4],
}

fn xyz(shape: [f32; 4]) -> Vec3 {
    Vec3::new(shape[0], shape[1], shape[2])
}

/// `moba_rect_axes`: the normal and the half-extent vectors (`ex × ey` along the normal).
pub(crate) fn rect_axes(shape: [f32; 4]) -> (Vec3, Vec3, Vec3) {
    let half_width = xyz(shape).length();
    let normal = xyz(shape) / half_width.max(1e-6);
    let across = Vec3::Y.cross(normal);
    let width = if across.length_squared() > 1e-8 {
        across.normalize()
    } else {
        Vec3::X
    };
    (normal, width * half_width, normal.cross(width) * shape[3])
}

/// `moba_shape_extent`: the largest half-extent (`LightShape::extent`).
pub(crate) fn extent(tag: f32, shape: [f32; 4]) -> f32 {
    if tag > 2.5 {
        xyz(shape).length().hypot(shape[3])
    } else if tag > 1.5 {
        xyz(shape).length() + shape[3]
    } else {
        shape[3]
    }
}

/// `moba_representative_point` (Karis 2013): the shape's point closest to the ray `r`.
pub(crate) fn representative_point(tag: f32, shape: [f32; 4], to_light: Vec3, r: Vec3) -> Vec3 {
    let mut centre = to_light;
    let radius = shape[3];
    if tag > 2.5 {
        let (normal, ex, ey) = rect_axes(shape);
        let facing = r.dot(normal);
        let t = if facing < 0.0 {
            to_light.dot(normal) / facing
        } else {
            -1.0
        };
        let local = (if t > 0.0 { r * t } else { to_light }) - to_light;
        let (half_width, half_height) = (ex.length(), ey.length());
        let x = ex / half_width.max(1e-6);
        let y = ey / half_height.max(1e-6);
        return to_light
            + x * local.dot(x).clamp(-half_width, half_width)
            + y * local.dot(y).clamp(-half_height, half_height);
    }
    if tag > 1.5 {
        let p0 = to_light - xyz(shape);
        let segment = 2.0 * xyz(shape);
        let r_dot_segment = r.dot(segment);
        let denominator = segment.dot(segment) - r_dot_segment * r_dot_segment;
        let t = if denominator > 1e-8 {
            ((r.dot(p0) * r_dot_segment - p0.dot(segment)) / denominator).clamp(0.0, 1.0)
        } else {
            0.5
        };
        centre = p0 + segment * t;
    }
    let centre_to_ray = r * centre.dot(r) - centre;
    let reach = centre_to_ray.length();
    centre + centre_to_ray * (radius / reach.max(1e-6)).clamp(0.0, 1.0)
}

/// `moba_area_normalisation`: the representative point's specular scale, `α / α'` per axis
/// of the shape with `α' = clamp(α + WIDEN × half-extent / distance, 0, 1)`: a sphere's two
/// axes its radius, a capsule's its radius and its half-length plus radius, a rect's its
/// half-extents.
pub(crate) fn normalisation(tag: f32, shape: [f32; 4], distance: f32, alpha: f32) -> f32 {
    let axis = |half_extent: f32| {
        alpha / (alpha + WIDEN * half_extent / distance.max(1e-4)).clamp(alpha.max(1e-6), 1.0)
    };
    let radius = shape[3];
    if tag > 2.5 {
        axis(xyz(shape).length()) * axis(radius)
    } else if tag > 1.5 {
        axis(radius) * axis(xyz(shape).length() + radius)
    } else {
        let s = axis(radius);
        s * s
    }
}

/// `moba_area_integrals` (with the frame and the lobe fetched by the caller):
/// `(diffuse, lobe)` and the radiance per unit of colour giving the point light's power.
pub(crate) fn area_integrals(
    tag: f32,
    shape: [f32; 4],
    to_light: Vec3,
    frame: &Frame,
    minv: Minv,
) -> ([f32; 2], f32) {
    let radius = shape[3];
    if tag < 1.5 {
        let scale = 1.0 / (radius * radius).max(1e-6);
        return (
            [
                twin::sphere(frame, Minv::IDENTITY, to_light, radius),
                twin::sphere(frame, minv, to_light, radius),
            ],
            scale,
        );
    }
    if tag > 2.5 {
        let (normal, ex, ey) = rect_axes(shape);
        let scale = PI / (ex.length() * ey.length()).max(1e-6);
        if to_light.dot(normal) >= 0.0 {
            return ([0.0; 2], scale);
        }
        return (
            [
                twin::rect(frame, Minv::IDENTITY, to_light, ex, ey),
                twin::rect(frame, minv, to_light, ex, ey),
            ],
            scale,
        );
    }
    let half = xyz(shape);
    let half_length = half.length();
    let scale = 1.0 / (radius * radius + radius * half_length).max(1e-6);
    let (p1, p2) = (to_light - half, to_light + half);
    (
        [
            twin::capsule(frame, Minv::IDENTITY, p1, p2, radius),
            twin::capsule(frame, minv, p1, p2, radius),
        ],
        scale,
    )
}

fn schlick(f0: f32, cosine: f32) -> f32 {
    f0 + (1.0 - f0) * (1.0 - cosine.clamp(0.0, 1.0)).powi(5)
}

fn ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let alpha2 = alpha * alpha;
    let d = n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0;
    alpha2 / (PI * d * d)
}

fn smith(n_dot_v: f32, n_dot_l: f32, alpha: f32) -> f32 {
    let alpha2 = alpha * alpha;
    let ggx_v = n_dot_l * (n_dot_v * n_dot_v * (1.0 - alpha2) + alpha2).sqrt();
    let ggx_l = n_dot_v * (n_dot_l * n_dot_l * (1.0 - alpha2) + alpha2).sqrt();
    0.5 / (ggx_v + ggx_l).max(1e-5)
}

/// The light's window at its centre's distance (ADR-0016), or `None` past its radius.
fn window(light: &LightIn) -> Option<f32> {
    let d2 = light.to_light.length_squared();
    let radius2 = light.radius * light.radius;
    if d2 >= radius2 {
        return None;
    }
    let ratio = d2 / radius2;
    Some((1.0 - ratio * ratio).clamp(0.0, 1.0))
}

/// The engine's point-light term along unit `l` with `attenuation` (`dynamic_lights` past the
/// shape code): wrapped diffuse with Schlick's `kd` at the half vector, GGX specular times
/// `specular_scale`.
fn point_term(
    surface: &Surface,
    n: Vec3,
    v: Vec3,
    l: Vec3,
    attenuation: f32,
    specular_scale: f32,
) -> f32 {
    let alpha = surface.roughness * surface.roughness;
    let n_dot_v = n.dot(v).max(1e-4);
    let n_dot_l_raw = n.dot(l);
    let n_dot_l = n_dot_l_raw.max(0.0);
    let wrapped = ((n_dot_l_raw + surface.wrap) / (1.0 + surface.wrap)).clamp(0.0, 1.0);
    let half = v + l;
    let h = if half.length_squared() > 1e-12 {
        half.normalize()
    } else {
        n
    };
    let f0 = surface.f0 + (surface.albedo - surface.f0) * surface.metallic;
    let fresnel = schlick(f0, v.dot(h).max(0.0));
    let kd = (1.0 - fresnel) * (1.0 - surface.metallic);
    let specular = PI
        * ggx(n.dot(h).max(0.0), alpha)
        * smith(n_dot_v, n_dot_l, alpha)
        * fresnel
        * n_dot_l
        * specular_scale;
    (kd * surface.albedo * wrapped + specular) * attenuation
}

/// The engine's point light at the light's centre (`MOBA_AREA_LIGHTS=off`, Compat, a
/// `Point`): per unit of colour.
pub(crate) fn point_light(surface: &Surface, n: Vec3, v: Vec3, light: &LightIn) -> f32 {
    let Some(window) = window(light) else {
        return 0.0;
    };
    let d2 = light.to_light.length_squared();
    let attenuation = window * window / d2.max(NEAR_FIELD);
    point_term(surface, n, v, light.to_light / d2.sqrt(), attenuation, 1.0) * light.color
}

/// One light's term in area mode `mode` (0 points, 1 the representative point, 2 LTC above
/// the span cut-off), as `dynamic_lights` adds it.
pub(crate) fn light(mode: u32, surface: &Surface, n: Vec3, v: Vec3, light: &LightIn) -> f32 {
    let Some(window) = window(light) else {
        return 0.0;
    };
    let d2 = light.to_light.length_squared();
    let distance = d2.sqrt();
    let mut attenuation = window * window / d2.max(NEAR_FIELD);
    let mut l = light.to_light / distance;
    let mut specular_scale = 1.0;
    let mut sum = 0.0;
    let alpha = surface.roughness * surface.roughness;
    if light.tag > 0.5 && mode != 0 {
        let span = extent(light.tag, light.shape) / distance.max(1e-4);
        let blend = if mode == 2 {
            smoothstep(SPAN_LOW, SPAN_HIGH, span)
        } else {
            0.0
        };
        if blend > 0.0 {
            let frame = Frame::new(n, v);
            let (minv, [magnitude, fresnel_average]) =
                twin::lobe(n.dot(v).max(1e-4), surface.roughness);
            let (integral, radiance_scale) =
                area_integrals(light.tag, light.shape, light.to_light, &frame, minv);
            // The point light's near-field floor (ADR-0016) at the centre's distance.
            let radiance =
                light.color * radiance_scale * window * window * d2 / d2.max(NEAR_FIELD) * blend;
            let half = v + l;
            let h = if half.length_squared() > 1e-12 {
                half.normalize()
            } else {
                n
            };
            let f0 = surface.f0 + (surface.albedo - surface.f0) * surface.metallic;
            let kd = (1.0 - schlick(f0, v.dot(h).max(0.0))) * (1.0 - surface.metallic);
            let specular = integral[1] * (f0 * magnitude + (1.0 - f0) * fresnel_average);
            sum += (kd * surface.albedo * integral[0] + specular) * radiance;
            if blend >= 1.0 {
                return sum;
            }
            attenuation *= 1.0 - blend;
        }
        let r = (2.0 * n * n.dot(v) - v).normalize();
        let point = representative_point(light.tag, light.shape, light.to_light, r);
        l = point / point.length().max(1e-4);
        if light.tag > 2.5 {
            let (normal, _, _) = rect_axes(light.shape);
            attenuation *= 4.0 * (-(light.to_light / distance).dot(normal)).max(0.0);
        }
        specular_scale = normalisation(light.tag, light.shape, distance, alpha);
    }
    sum + point_term(surface, n, v, l, attenuation, specular_scale) * light.color
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
