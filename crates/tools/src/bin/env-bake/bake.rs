//! Cube-map texel directions, SH-L2 projection and GGX specular prefiltering.

use moba_assets::{SH_COEFFICIENTS, TextureData, TextureFormat, f16_from_f32, sh_basis};

use crate::sky::{V3, normalize};

/// Faces in KTX2/Vulkan order.
pub(crate) const FACE_NAMES: [&str; 6] = ["+X", "-X", "+Y", "-Y", "+Z", "-Z"];

/// The (unnormalised) direction of face coordinates `s, t` in `[-1, 1]` (`t` grows down the
/// rows) on `face`, matching Vulkan's cube-map face selection.
pub(crate) fn face_direction(face: usize, s: f32, t: f32) -> V3 {
    match face {
        0 => [1.0, -t, -s],
        1 => [-1.0, -t, s],
        2 => [s, 1.0, t],
        3 => [s, -1.0, -t],
        4 => [s, -t, 1.0],
        _ => [-s, -t, -1.0],
    }
}

/// Face coordinate of texel `index` (plus a sub-texel `offset` in texels) on a `size` edge.
fn coordinate(index: u32, offset: f32, size: u32) -> f32 {
    2.0 * (index as f32 + 0.5 + offset) / size as f32 - 1.0
}

/// The normalised direction through the centre (plus offset) of texel `x, y` of `face`.
pub(crate) fn texel_direction(face: usize, x: u32, y: u32, size: u32, offset: [f32; 2]) -> V3 {
    normalize(face_direction(
        face,
        coordinate(x, offset[0], size),
        coordinate(y, offset[1], size),
    ))
}

/// Cosine-convolved SH-L2 of `radiance`, divided by π (the convention in
/// `moba_assets::EnvironmentDesc`), integrated over a `size`² cube with exact per-texel solid
/// angles.
pub(crate) fn project_sh(radiance: &dyn Fn(V3) -> V3, size: u32) -> [[f32; 3]; SH_COEFFICIENTS] {
    let mut sums = [[0.0f64; 3]; SH_COEFFICIENTS];
    let mut total_weight = 0.0f64;
    for face in 0..6 {
        for y in 0..size {
            for x in 0..size {
                let (s, t) = (coordinate(x, 0.0, size), coordinate(y, 0.0, size));
                let d = normalize(face_direction(face, s, t));
                let r2 = 1.0 + s * s + t * t;
                // Solid angle of a texel of area (2/size)² at distance sqrt(r2).
                let weight = f64::from(4.0 / (size * size) as f32 / (r2 * r2.sqrt()));
                total_weight += weight;
                let light = radiance(d);
                for (sum, basis) in sums.iter_mut().zip(sh_basis(d)) {
                    for channel in 0..3 {
                        sum[channel] += f64::from(light[channel] * basis) * weight;
                    }
                }
            }
        }
    }
    // Renormalise the texel solid angles to exactly 4π, then convolve with the clamped
    // cosine (A0 = π, A1 = 2π/3, A2 = π/4) and divide by π.
    let scale = 4.0 * core::f64::consts::PI / total_weight;
    let band = [
        1.0,
        2.0 / 3.0,
        2.0 / 3.0,
        2.0 / 3.0,
        0.25,
        0.25,
        0.25,
        0.25,
        0.25,
    ];
    let mut out = [[0.0f32; 3]; SH_COEFFICIENTS];
    for ((row, sum), factor) in out.iter_mut().zip(sums).zip(band) {
        for channel in 0..3 {
            row[channel] = (sum[channel] * scale * factor) as f32;
        }
    }
    out
}

/// Van der Corput radical inverse in base 2.
fn radical_inverse(index: u32) -> f32 {
    index.reverse_bits() as f32 * (1.0 / 4_294_967_296.0)
}

/// GGX-distributed half vectors in tangent space (`z` along the normal) for `alpha`.
fn ggx_half_vectors(alpha: f32, samples: u32) -> Vec<V3> {
    let a2 = alpha * alpha;
    (0..samples)
        .map(|index| {
            let u = (index as f32 + 0.5) / samples as f32;
            let v = radical_inverse(index);
            let phi = 2.0 * core::f32::consts::PI * u;
            let cos_theta = ((1.0 - v) / (1.0 + (a2 - 1.0) * v)).max(0.0).sqrt();
            let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
            [sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta]
        })
        .collect()
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// GGX-prefiltered radiance around `n` (with `N = V = R`, the split-sum assumption).
fn prefiltered(radiance: &dyn Fn(V3) -> V3, n: V3, halves: &[V3]) -> V3 {
    let up = if n[2].abs() < 0.999 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let tangent = normalize(cross(up, n));
    let bitangent = cross(n, tangent);
    let mut sum = [0.0f32; 3];
    let mut weight = 0.0f32;
    for h in halves {
        let half = [
            tangent[0] * h[0] + bitangent[0] * h[1] + n[0] * h[2],
            tangent[1] * h[0] + bitangent[1] * h[1] + n[1] * h[2],
            tangent[2] * h[0] + bitangent[2] * h[1] + n[2] * h[2],
        ];
        let v_dot_h = dot(n, half);
        let l = [
            2.0 * v_dot_h * half[0] - n[0],
            2.0 * v_dot_h * half[1] - n[1],
            2.0 * v_dot_h * half[2] - n[2],
        ];
        let n_dot_l = dot(n, l);
        if n_dot_l > 0.0 {
            let light = radiance(normalize(l));
            for channel in 0..3 {
                sum[channel] += light[channel] * n_dot_l;
            }
            weight += n_dot_l;
        }
    }
    if weight > 0.0 {
        sum.map(|value| value / weight)
    } else {
        radiance(n)
    }
}

/// Every level of the prefiltered cube, faces concatenated per level (texel rows top
/// first). Level `m` of the `size`'s full chain is prefiltered for roughness `m / (M - 1)`;
/// level 0 is the sky itself, 2×2 supersampled. `samples` GGX samples per texel (twice that
/// above roughness 0.5).
pub(crate) fn prefilter_levels(
    radiance: &dyn Fn(V3) -> V3,
    size: u32,
    samples: u32,
) -> Vec<Vec<V3>> {
    let levels = moba_assets::full_mip_chain(size, size);
    (0..levels)
        .map(|level| {
            let edge = (size >> level).max(1);
            let roughness = if levels > 1 {
                level as f32 / (levels - 1) as f32
            } else {
                0.0
            };
            let count = if roughness > 0.5 {
                samples * 2
            } else {
                samples
            };
            let halves = ggx_half_vectors(roughness * roughness, count.max(1));
            let mut texels = Vec::with_capacity((edge * edge * 6) as usize);
            for face in 0..6 {
                for y in 0..edge {
                    for x in 0..edge {
                        let value = if level == 0 {
                            let mut sum = [0.0f32; 3];
                            for offset in
                                [[-0.25, -0.25], [0.25, -0.25], [-0.25, 0.25], [0.25, 0.25]]
                            {
                                let light = radiance(texel_direction(face, x, y, edge, offset));
                                for channel in 0..3 {
                                    sum[channel] += light[channel] * 0.25;
                                }
                            }
                            sum
                        } else {
                            prefiltered(
                                radiance,
                                texel_direction(face, x, y, edge, [0.0, 0.0]),
                                &halves,
                            )
                        };
                        texels.push(value);
                    }
                }
            }
            texels
        })
        .collect()
}

/// Packs prefiltered levels as an `RGBA16F` cube map (alpha 1).
pub(crate) fn to_texture(levels: &[Vec<V3>], size: u32) -> Result<TextureData, String> {
    let one = f16_from_f32(1.0).to_le_bytes();
    let bytes: Vec<Vec<u8>> = levels
        .iter()
        .map(|texels| {
            let mut out = Vec::with_capacity(texels.len() * 8);
            for texel in texels {
                for channel in texel {
                    out.extend_from_slice(&f16_from_f32(*channel).to_le_bytes());
                }
                out.extend_from_slice(&one);
            }
            out
        })
        .collect();
    TextureData::new_cube(TextureFormat::Rgba16Float, size, &bytes)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vulkan's cube-map face selection (spec "Cube Map Face Selection"): the major axis
    /// picks the face, then `(sc / |ma|, tc / |ma|)` are the face coordinates in `[-1, 1]`.
    fn select(d: V3) -> (usize, f32, f32) {
        let [x, y, z] = d;
        let (face, sc, tc, ma) = if x.abs() >= y.abs() && x.abs() >= z.abs() {
            if x >= 0.0 {
                (0, -z, -y, x)
            } else {
                (1, z, -y, x)
            }
        } else if y.abs() >= z.abs() {
            if y >= 0.0 {
                (2, x, z, y)
            } else {
                (3, x, -z, y)
            }
        } else if z >= 0.0 {
            (4, x, -y, z)
        } else {
            (5, -x, -y, z)
        };
        (face, sc / ma.abs(), tc / ma.abs())
    }

    #[test]
    fn face_directions_invert_hardware_face_selection() {
        // A deterministic spread of face coordinates on every face.
        let mut state = 0x1234_5678u32;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f32 / u32::MAX as f32) * 1.9 - 0.95
        };
        for (face, name) in FACE_NAMES.iter().enumerate() {
            for _ in 0..200 {
                let (s, t) = (next(), next());
                let (selected, ss, tt) = select(normalize(face_direction(face, s, t)));
                assert_eq!(selected, face, "{name} at {s},{t}");
                assert!((ss - s).abs() < 1e-5 && (tt - t).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn a_uniform_sky_projects_to_band_zero_only() {
        let sh = project_sh(&|_| [0.5, 1.0, 2.0], 16);
        let desc = moba_assets::EnvironmentDesc::new(sh);
        for n in [[0.0, 1.0, 0.0], [0.6, -0.8, 0.0], [0.0, 0.0, -1.0]] {
            let value = desc.irradiance_over_pi(n);
            for (got, want) in value.iter().zip([0.5, 1.0, 2.0]) {
                assert!((got - want).abs() < 1e-3, "{value:?}");
            }
        }
        for row in &sh[1..] {
            assert!(row.iter().all(|value| value.abs() < 1e-4), "{row:?}");
        }
        // A sky that is bright only above the horizon lights up-facing surfaces more.
        let sh = project_sh(&|d| if d[1] > 0.0 { [1.0; 3] } else { [0.0; 3] }, 16);
        let desc = moba_assets::EnvironmentDesc::new(sh);
        let up = desc.irradiance_over_pi([0.0, 1.0, 0.0])[0];
        let down = desc.irradiance_over_pi([0.0, -1.0, 0.0])[0];
        assert!(up > 0.9 && down < 0.1, "{up} {down}");
    }

    fn variance(texels: &[V3]) -> f32 {
        let luma: Vec<f32> = texels
            .iter()
            .map(|t| 0.2126 * t[0] + 0.7152 * t[1] + 0.0722 * t[2])
            .collect();
        let mean = luma.iter().sum::<f32>() / luma.len() as f32;
        luma.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / luma.len() as f32
    }

    #[test]
    fn prefiltered_levels_blur_monotonically_and_pack_as_a_cube() {
        let sky = crate::sky::Sky::preset("meadow").expect("preset");
        let sun = sky.sun();
        let radiance = |d| sky.radiance(d, sun);
        let levels = prefilter_levels(&radiance, 16, 64);
        assert_eq!(levels.len(), 5);
        let variances: Vec<f32> = levels.iter().map(|level| variance(level)).collect();
        assert!(
            variances.windows(2).all(|pair| pair[1] < pair[0]),
            "{variances:?}"
        );
        let texture = to_texture(&levels, 16).expect("cube");
        assert!(texture.is_cube());
        assert_eq!(texture.format(), TextureFormat::Rgba16Float);
        let bytes = moba_assets::write_ktx2(&texture, "test");
        assert_eq!(moba_assets::load_ktx2(&bytes), Ok(texture));
    }
}
