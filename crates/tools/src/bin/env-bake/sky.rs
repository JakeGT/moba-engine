//! The analytic sky the environments are baked from.
//!
//! A gradient model in linear light: zenith colour overhead, horizon colour at the horizon,
//! a ground colour below it (with a soft blend across the horizon), plus a broad glow lobe
//! around the sun direction. There is deliberately **no sun disk**: the key light's direct
//! contribution comes from the shading's analytic sun (and its shadow map), so a disk in the
//! environment would light every surface twice.

/// A linear RGB colour or a direction.
pub(crate) type V3 = [f32; 3];

/// Every parameter of the analytic sky.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sky {
    /// Linear RGB straight up.
    pub(crate) zenith: V3,
    /// Linear RGB at the horizon (also the start of the ground blend).
    pub(crate) horizon: V3,
    /// Linear RGB of the ground bounce below the horizon.
    pub(crate) ground: V3,
    /// Direction *towards* the sun (normalised when baking).
    pub(crate) sun_dir: V3,
    /// Linear RGB peak of the glow lobe.
    pub(crate) sun_color: V3,
    /// Exponent of the glow lobe, `max(0, dot(d, sun))^glow`: larger is tighter.
    pub(crate) sun_glow: f32,
}

/// The shipped presets, by name.
pub(crate) const PRESETS: &[&str] = &["meadow", "ember", "frost"];

impl Sky {
    /// A named preset: `meadow` (the default grass map), `ember` or `frost`.
    pub(crate) fn preset(name: &str) -> Option<Self> {
        Some(match name {
            // Cool blue sky, warm hazy horizon, green-brown ground bounce; the sun is the
            // renderer's default key-light direction.
            "meadow" => Self {
                zenith: [0.18, 0.32, 0.62],
                horizon: [0.72, 0.68, 0.58],
                ground: [0.16, 0.17, 0.10],
                sun_dir: [0.45, 1.0, 0.35],
                sun_color: [1.6, 1.3, 0.9],
                sun_glow: 8.0,
            },
            // Smoky orange-red sky, dark red ground, low warm sun.
            "ember" => Self {
                zenith: [0.20, 0.07, 0.05],
                horizon: [0.95, 0.38, 0.12],
                ground: [0.14, 0.04, 0.02],
                sun_dir: [-0.6, 0.35, 0.45],
                sun_color: [2.4, 0.9, 0.3],
                sun_glow: 6.0,
            },
            // Pale blue-white sky, bright snow bounce, cold sun.
            "frost" => Self {
                zenith: [0.42, 0.58, 0.85],
                horizon: [0.85, 0.90, 0.98],
                ground: [0.55, 0.60, 0.68],
                sun_dir: [0.3, 0.8, -0.5],
                sun_color: [1.2, 1.3, 1.5],
                sun_glow: 10.0,
            },
            _ => return None,
        })
    }

    /// Radiance arriving from the normalised direction `d`.
    pub(crate) fn radiance(&self, d: V3, sun: V3) -> V3 {
        let y = d[1];
        let base = if y >= 0.0 {
            // Most of the gradient happens near the horizon.
            let t = y.sqrt();
            lerp(self.horizon, self.zenith, t)
        } else {
            lerp(self.horizon, self.ground, smoothstep(0.0, 0.15, -y))
        };
        let cosine = (d[0] * sun[0] + d[1] * sun[1] + d[2] * sun[2]).max(0.0);
        // The glow fades out below the horizon so the ground does not glow.
        let glow = cosine.powf(self.sun_glow) * smoothstep(-0.12, 0.04, y);
        [
            base[0] + self.sun_color[0] * glow,
            base[1] + self.sun_color[1] * glow,
            base[2] + self.sun_color[2] * glow,
        ]
    }

    /// The normalised sun direction.
    pub(crate) fn sun(&self) -> V3 {
        normalize(self.sun_dir)
    }

    /// One line per parameter, for the descriptor's header comments.
    pub(crate) fn describe(&self) -> Vec<String> {
        let v = |name: &str, value: V3| format!("{name} = {value:?}");
        vec![
            v("zenith", self.zenith),
            v("horizon", self.horizon),
            v("ground", self.ground),
            v("sun_dir", self.sun_dir),
            v("sun_color", self.sun_color),
            format!("sun_glow = {:?}", self.sun_glow),
        ]
    }
}

fn lerp(a: V3, b: V3, t: f32) -> V3 {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `v / |v|`, or +Y for a zero vector.
pub(crate) fn normalize(v: V3) -> V3 {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length > 1e-12 && length.is_finite() {
        [v[0] / length, v[1] / length, v[2] / length]
    } else {
        [0.0, 1.0, 0.0]
    }
}
