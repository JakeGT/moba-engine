//! The thin G-buffer's encoding (ADR-0023 §1a, §6c) and its Rust twin.
//!
//! Three render-resolution targets of 4 bytes each, written by the tiered G-buffer prepass
//! next to N1's depth and motion:
//!
//! | Target | Format | Channels |
//! |---|---|---|
//! | `gb0` | `R8G8B8A8_UNORM` | rgb base colour, sRGB-encoded in the shader; a metallic |
//! | `gb1` | `A2B10G10R10_UNORM_PACK32` | rg octahedral world normal; b perceptual roughness; a flags (bit 0 `UNIT`, bit 1 reserved) |
//! | `gb2` | `R8G8B8A8_UNORM` | rgb model data (§6c); a model (bits 4–7), `NO_FOG_OF_WAR` (bit 3), rim slot (bits 0–2) |
//!
//! [`encode`] and [`decode`] are the twins of `encode_gbuffer` in
//! `shaders/include/gbuffer.glsl` (the G-buffer fragment stages) and `encode_gbuffer` /
//! `decode_gbuffer` in `shaders/slang/lib/gbuffer.slang` (the debug views, N2b's lighting).
//! All three quantise explicitly, `floor(x · (2^b − 1) + 0.5)`, so the stored bytes do not
//! depend on the device's float-to-UNORM rounding: a shader writes `k / (2^b − 1)`, which every
//! conformant device stores as `k`. `gbuffer-smoke` checks the shaders' bytes against
//! [`encode`] on a device (verification gate 1).
//!
//! **Model data** is kept in physical units in [`SurfaceParams::model_data`] and mapped to
//! `[0, 1]` per model (§6c): a dielectric F0 as `sqrt(F0 / 0.16)`, film thickness over
//! 100–400 nm, film IOR over 1–2, the anisotropy angle over `[0, π)`. The material AO sits in
//! the model's AO channel; a model without one decodes AO as 1.

/// `gb0`'s format.
pub(crate) const GB0_FORMAT: ash::vk::Format = ash::vk::Format::R8G8B8A8_UNORM;
/// `gb1`'s format.
pub(crate) const GB1_FORMAT: ash::vk::Format = ash::vk::Format::A2B10G10R10_UNORM_PACK32;
/// `gb2`'s format.
pub(crate) const GB2_FORMAT: ash::vk::Format = ash::vk::Format::R8G8B8A8_UNORM;

/// `SurfaceParams::flags`: a unit pixel (`gb1.a` bit 0). Replaces N1's unit mask for the heat
/// haze in N2b and drives the team rim (§8).
pub(crate) const FLAG_UNIT: u32 = 1;
/// `SurfaceParams::flags`: reserved for N3 (`gb1.a` bit 1).
pub(crate) const FLAG_RESERVED: u32 = 2;
/// `SurfaceParams::flags`: the fog of war does not darken this pixel (`gb2.a` bit 3), today's
/// rule for everything but terrain and foliage.
pub(crate) const FLAG_NO_FOG_OF_WAR: u32 = 4;

/// The perceptual roughness floor (ADR-0011, `MOBA_MIN_ROUGHNESS`).
pub(crate) const MIN_ROUGHNESS: f32 = 0.045;
/// The art bible's roughness floor on units.
pub(crate) const MIN_UNIT_ROUGHNESS: f32 = 0.08;
/// The dielectric F0 range `gb2` stores, `0.16 x²`.
pub(crate) const MAX_DIELECTRIC_F0: f32 = 0.16;
/// The thin-film thickness range in nanometres.
pub(crate) const FILM_THICKNESS_NM: (f32, f32) = (100.0, 400.0);
/// The thin-film IOR range.
pub(crate) const FILM_IOR: (f32, f32) = (1.0, 2.0);

/// A shading model ID (`gb2.a` bits 4–7, ADR-0023 §6c). 8–15 are reserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub(crate) enum ShadingModel {
    /// Emissive only; no lighting.
    Unlit = 0,
    /// GGX + Lambert: dielectric F0, material AO.
    Standard = 1,
    /// A coat lobe over the base: coat weight, coat roughness, material AO.
    Clearcoat = 2,
    /// Charlie sheen: weight, roughness, tint.
    Sheen = 3,
    /// Anisotropic GGX: strength, angle, material AO.
    Anisotropic = 4,
    /// Thin-film Fresnel: thickness, film IOR, material AO.
    ThinFilm = 5,
    /// Two-sided Lambert: translucency, material AO.
    Foliage = 6,
    /// A transmissive material drawn opaque (Low): as `Standard`.
    TransmissiveFallback = 7,
}

impl ShadingModel {
    /// Every model, by ID.
    pub(crate) const ALL: [Self; 8] = [
        Self::Unlit,
        Self::Standard,
        Self::Clearcoat,
        Self::Sheen,
        Self::Anisotropic,
        Self::ThinFilm,
        Self::Foliage,
        Self::TransmissiveFallback,
    ];

    /// The model with ID `id`, if it is not reserved.
    #[must_use]
    pub(crate) fn from_id(id: u8) -> Option<Self> {
        Self::ALL.get(usize::from(id)).copied()
    }

    /// The `gb2` channel holding the material AO, if the model stores it.
    #[must_use]
    pub(crate) const fn ao_channel(self) -> Option<usize> {
        match self {
            Self::Unlit | Self::Sheen => None,
            Self::Standard | Self::Foliage | Self::TransmissiveFallback => Some(1),
            Self::Clearcoat | Self::Anisotropic | Self::ThinFilm => Some(2),
        }
    }

    /// How many `gb2` channels the model's own data uses (before the AO channel).
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn data_channels(self) -> usize {
        match self {
            Self::Unlit => 0,
            Self::Standard | Self::Foliage | Self::TransmissiveFallback => 1,
            Self::Clearcoat | Self::Anisotropic | Self::ThinFilm => 2,
            Self::Sheen => 3,
        }
    }
}

/// One G-buffer pixel's surface, decoded (ADR-0023 §6d `SurfaceParams`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SurfaceParams {
    /// Linear base colour.
    pub(crate) base_color: [f32; 3],
    pub(crate) metallic: f32,
    /// Unit world-space normal.
    pub(crate) normal: [f32; 3],
    /// Perceptual roughness (`alpha = roughness²`).
    pub(crate) roughness: f32,
    pub(crate) model: ShadingModel,
    /// The model's parameters in physical units (§6c; see the module documentation).
    pub(crate) model_data: [f32; 3],
    /// Material AO (1 = open).
    pub(crate) ao: f32,
    /// `FLAG_*` bits.
    pub(crate) flags: u32,
    /// Team-rim slot, 0–7.
    pub(crate) rim_slot: u32,
}

/// One pixel's three G-buffer texels as stored: `gb0` and `gb2` as bytes (r, g, b, a), `gb1`
/// as the packed `A2B10G10R10` word (r bits 0–9, g 10–19, b 20–29, a 30–31).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct GBufferTexels {
    pub(crate) gb0: [u8; 4],
    pub(crate) gb1: u32,
    pub(crate) gb2: [u8; 4],
}

impl GBufferTexels {
    /// The four 10/10/10/2 fields of `gb1`.
    #[must_use]
    pub(crate) const fn gb1_fields(self) -> [u32; 4] {
        [
            self.gb1 & 0x3ff,
            (self.gb1 >> 10) & 0x3ff,
            (self.gb1 >> 20) & 0x3ff,
            self.gb1 >> 30,
        ]
    }
}

/// `x` clamped to `[0, 1]` and quantised to `steps` (`2^b − 1`): `floor(x · steps + 0.5)`.
/// NaN stores 0.
#[must_use]
pub(crate) fn quantise(x: f32, steps: u32) -> u32 {
    let clamped = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
    // In 0..=steps, so the cast is exact.
    (clamped * steps as f32 + 0.5).floor() as u32
}

/// The sRGB transfer function (linear → encoded), on `[0, 1]`.
#[must_use]
pub(crate) fn srgb_encode(linear: f32) -> f32 {
    let x = if linear.is_nan() {
        0.0
    } else {
        linear.clamp(0.0, 1.0)
    };
    if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

/// The inverse of [`srgb_encode`].
#[must_use]
pub(crate) fn srgb_decode(encoded: f32) -> f32 {
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// The octahedral map of a unit normal (Cigolle et al. 2014) to `[0, 1]²`, folded about the
/// world's up axis (y): the upper hemisphere is the inner diamond, where the terrain's normals
/// are.
#[must_use]
pub(crate) fn octahedral_encode(normal: [f32; 3]) -> [f32; 2] {
    let sum = normal[0].abs() + normal[1].abs() + normal[2].abs();
    if sum.is_nan() || sum <= 0.0 || sum.is_infinite() {
        // Degenerate: straight up.
        return [0.5, 0.5];
    }
    let mut p = [normal[0] / sum, normal[2] / sum];
    if normal[1] < 0.0 {
        p = [
            (1.0 - p[1].abs()) * sign(p[0]),
            (1.0 - p[0].abs()) * sign(p[1]),
        ];
    }
    [p[0] * 0.5 + 0.5, p[1] * 0.5 + 0.5]
}

/// The inverse of [`octahedral_encode`], normalised.
#[must_use]
pub(crate) fn octahedral_decode(encoded: [f32; 2]) -> [f32; 3] {
    let u = encoded[0] * 2.0 - 1.0;
    let v = encoded[1] * 2.0 - 1.0;
    let mut n = [u, 1.0 - u.abs() - v.abs(), v];
    let t = (-n[1]).max(0.0);
    n[0] += if n[0] >= 0.0 { -t } else { t };
    n[2] += if n[2] >= 0.0 { -t } else { t };
    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    [n[0] / length, n[1] / length, n[2] / length]
}

/// `±1` (`+1` for zero), as `signNotZero` in Cigolle et al.
fn sign(x: f32) -> f32 {
    if x >= 0.0 { 1.0 } else { -1.0 }
}

/// The roughness the G-buffer stores: clamped to [`MIN_ROUGHNESS`], or
/// [`MIN_UNIT_ROUGHNESS`] on unit pixels, and to 1.
#[must_use]
pub(crate) fn stored_roughness(roughness: f32, flags: u32) -> f32 {
    let floor = if flags & FLAG_UNIT != 0 {
        MIN_UNIT_ROUGHNESS
    } else {
        MIN_ROUGHNESS
    };
    let roughness = if roughness.is_nan() { 1.0 } else { roughness };
    roughness.clamp(floor, 1.0)
}

/// The model's data mapped to `[0, 1]` per channel (before quantisation), AO included.
#[must_use]
pub(crate) fn model_channels(model: ShadingModel, data: [f32; 3], ao: f32) -> [f32; 3] {
    let f0 = |f0: f32| (f0.max(0.0) / MAX_DIELECTRIC_F0).sqrt();
    let mut channels = match model {
        ShadingModel::Unlit => [0.0; 3],
        ShadingModel::Standard | ShadingModel::TransmissiveFallback => [f0(data[0]), 0.0, 0.0],
        ShadingModel::Clearcoat => [data[0], data[1], 0.0],
        ShadingModel::Sheen => data,
        ShadingModel::Anisotropic => {
            // `[0, π]` as is (π stores as 255 and decodes to itself), anything else wrapped:
            // the direction is π-periodic.
            let angle = if (0.0..=core::f32::consts::PI).contains(&data[1]) {
                data[1]
            } else {
                data[1].rem_euclid(core::f32::consts::PI)
            };
            [data[0], angle / core::f32::consts::PI, 0.0]
        }
        ShadingModel::ThinFilm => [
            (data[0] - FILM_THICKNESS_NM.0) / (FILM_THICKNESS_NM.1 - FILM_THICKNESS_NM.0),
            (data[1] - FILM_IOR.0) / (FILM_IOR.1 - FILM_IOR.0),
            0.0,
        ],
        ShadingModel::Foliage => [data[0], 0.0, 0.0],
    };
    if let Some(channel) = model.ao_channel() {
        channels[channel] = ao;
    }
    channels
}

/// The inverse of [`model_channels`]: `(model_data, ao)`.
#[must_use]
pub(crate) fn model_data(model: ShadingModel, channels: [f32; 3]) -> ([f32; 3], f32) {
    let ao = model.ao_channel().map_or(1.0, |channel| channels[channel]);
    let data = match model {
        ShadingModel::Unlit => [0.0; 3],
        ShadingModel::Standard | ShadingModel::TransmissiveFallback => {
            [MAX_DIELECTRIC_F0 * channels[0] * channels[0], 0.0, 0.0]
        }
        ShadingModel::Clearcoat => [channels[0], channels[1], 0.0],
        ShadingModel::Sheen => channels,
        ShadingModel::Anisotropic => [channels[0], channels[1] * core::f32::consts::PI, 0.0],
        ShadingModel::ThinFilm => [
            FILM_THICKNESS_NM.0 + channels[0] * (FILM_THICKNESS_NM.1 - FILM_THICKNESS_NM.0),
            FILM_IOR.0 + channels[1] * (FILM_IOR.1 - FILM_IOR.0),
            0.0,
        ],
        ShadingModel::Foliage => [channels[0], 0.0, 0.0],
    };
    (data, ao)
}

/// `encode_gbuffer`: one surface to its texels.
#[must_use]
pub(crate) fn encode(surface: &SurfaceParams) -> GBufferTexels {
    let byte = |x: f32| quantise(x, 255) as u8;
    let gb0 = [
        byte(srgb_encode(surface.base_color[0])),
        byte(srgb_encode(surface.base_color[1])),
        byte(srgb_encode(surface.base_color[2])),
        byte(surface.metallic),
    ];
    let [u, v] = octahedral_encode(surface.normal);
    let gb1 = quantise(u, 1023)
        | quantise(v, 1023) << 10
        | quantise(stored_roughness(surface.roughness, surface.flags), 1023) << 20
        | (surface.flags & (FLAG_UNIT | FLAG_RESERVED)) << 30;
    let channels = model_channels(surface.model, surface.model_data, surface.ao);
    let alpha = (surface.model as u32) << 4
        | u32::from(surface.flags & FLAG_NO_FOG_OF_WAR != 0) << 3
        | (surface.rim_slot & 7);
    let gb2 = [
        byte(channels[0]),
        byte(channels[1]),
        byte(channels[2]),
        alpha as u8,
    ];
    GBufferTexels { gb0, gb1, gb2 }
}

/// `decode_gbuffer`: texels back to a surface. `None` for a reserved model ID (8–15).
#[must_use]
pub(crate) fn decode(texels: GBufferTexels) -> Option<SurfaceParams> {
    let unit = |k: u8| f32::from(k) / 255.0;
    let [nu, nv, rough, flags] = texels.gb1_fields();
    let model = ShadingModel::from_id(texels.gb2[3] >> 4)?;
    let (model_data, ao) = model_data(
        model,
        [
            unit(texels.gb2[0]),
            unit(texels.gb2[1]),
            unit(texels.gb2[2]),
        ],
    );
    let no_fog = u32::from(texels.gb2[3] & 8 != 0) * FLAG_NO_FOG_OF_WAR;
    Some(SurfaceParams {
        base_color: [
            srgb_decode(unit(texels.gb0[0])),
            srgb_decode(unit(texels.gb0[1])),
            srgb_decode(unit(texels.gb0[2])),
        ],
        metallic: unit(texels.gb0[3]),
        normal: octahedral_decode([nu as f32 / 1023.0, nv as f32 / 1023.0]),
        roughness: rough as f32 / 1023.0,
        model,
        model_data,
        ao,
        flags: flags | no_fog,
        rim_slot: u32::from(texels.gb2[3] & 7),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(model: ShadingModel) -> SurfaceParams {
        SurfaceParams {
            base_color: [0.2, 0.5, 0.8],
            metallic: 0.25,
            normal: [0.0, 1.0, 0.0],
            roughness: 0.5,
            model,
            model_data: match model {
                ShadingModel::Unlit => [0.0; 3],
                ShadingModel::Standard | ShadingModel::TransmissiveFallback => [0.04, 0.0, 0.0],
                ShadingModel::Clearcoat => [0.8, 0.15, 0.0],
                ShadingModel::Sheen => [0.6, 0.4, 0.3],
                ShadingModel::Anisotropic => [0.7, 1.2, 0.0],
                ShadingModel::ThinFilm => [250.0, 1.4, 0.0],
                ShadingModel::Foliage => [0.45, 0.0, 0.0],
            },
            ao: if model.ao_channel().is_some() {
                0.7
            } else {
                1.0
            },
            flags: FLAG_UNIT | FLAG_NO_FOG_OF_WAR,
            rim_slot: 5,
        }
    }

    fn angle_degrees(a: [f32; 3], b: [f32; 3]) -> f64 {
        let dot = f64::from(a[0]) * f64::from(b[0])
            + f64::from(a[1]) * f64::from(b[1])
            + f64::from(a[2]) * f64::from(b[2]);
        dot.clamp(-1.0, 1.0).acos().to_degrees()
    }

    /// Every model round-trips: the IDs, flags and slot exactly, every value within its
    /// quantisation step, and re-encoding the decoded surface gives the same texels.
    #[test]
    fn every_model_round_trips() {
        for model in ShadingModel::ALL {
            let original = surface(model);
            let texels = encode(&original);
            let decoded = decode(texels).expect("a defined model");
            assert_eq!(decoded.model, model);
            assert_eq!(decoded.flags, original.flags, "{model:?}");
            assert_eq!(decoded.rim_slot, original.rim_slot);
            for (a, b) in decoded.base_color.iter().zip(original.base_color) {
                assert!((a - b).abs() < 0.01, "{model:?} base colour {a} vs {b}");
            }
            assert!((decoded.metallic - original.metallic).abs() <= 0.5 / 255.0 + 1e-6);
            assert!((decoded.roughness - original.roughness).abs() <= 0.5 / 1023.0 + 1e-6);
            assert!(angle_degrees(decoded.normal, original.normal) < 0.1);
            assert!(
                (decoded.ao - original.ao).abs() <= 0.5 / 255.0 + 1e-6,
                "{model:?}"
            );
            let tolerance = match model {
                ShadingModel::ThinFilm => [300.0 * 0.5 / 255.0, 0.5 / 255.0, 0.0],
                ShadingModel::Anisotropic => {
                    [0.5 / 255.0, core::f32::consts::PI * 0.5 / 255.0, 0.0]
                }
                // sqrt(F0/0.16) in 8 bits: at F0 0.04 the step is about 0.0005.
                ShadingModel::Standard | ShadingModel::TransmissiveFallback => [0.0006, 0.0, 0.0],
                _ => [0.5 / 255.0 + 1e-6; 3],
            };
            for (channel, tolerance) in tolerance.iter().enumerate() {
                assert!(
                    (decoded.model_data[channel] - original.model_data[channel]).abs()
                        <= tolerance + 1e-5,
                    "{model:?} channel {channel}: {:?} vs {:?}",
                    decoded.model_data,
                    original.model_data
                );
            }
            assert_eq!(encode(&decoded), texels, "{model:?} re-encodes to itself");
        }
    }

    /// Every texel value decodes and re-encodes to itself (the stored bytes are a fixed point
    /// of decode then encode), for every model, flag and rim slot.
    #[test]
    fn stored_texels_are_fixed_points() {
        for model in ShadingModel::ALL {
            for alpha_low in 0u8..16 {
                for value in (0u8..=255).step_by(17) {
                    let texels = GBufferTexels {
                        gb0: [value, 255 - value, value / 2, value],
                        gb1: (u32::from(value) * 4)
                            | ((1023 - u32::from(value) * 4) << 10)
                            | (600 << 20),
                        gb2: [value, 255 - value, value, (model as u8) << 4 | alpha_low],
                    };
                    let decoded = decode(texels).expect("defined");
                    let again = encode(&decoded);
                    assert_eq!(again.gb0, texels.gb0);
                    assert_eq!(again.gb2[3], texels.gb2[3]);
                    // Channels the model does not store come back 0.
                    let used = model.data_channels() + usize::from(model.ao_channel().is_some());
                    for channel in 0..3 {
                        let expected = if channel < used {
                            texels.gb2[channel]
                        } else {
                            0
                        };
                        assert_eq!(again.gb2[channel], expected, "{model:?} {channel}");
                    }
                }
            }
        }
        assert!(
            decode(GBufferTexels {
                gb2: [0, 0, 0, 8 << 4],
                ..GBufferTexels::default()
            })
            .is_none()
        );
    }

    /// The octahedral normal's largest angular error over a dense sphere, encoded to 10 + 10
    /// bits: recorded here (ADR-0023 verification gate 1), not assumed. Measured 0.2373° at
    /// most and 0.0836° on average over 1,000,000 directions (a Fibonacci sphere); the bounds
    /// leave about 5 % for other targets' `f32` rounding.
    #[test]
    fn octahedral_error_over_a_dense_sphere() {
        const POINTS: u32 = 1_000_000;
        let golden = core::f64::consts::PI * (3.0 - 5.0f64.sqrt());
        let mut worst = 0.0f64;
        let mut sum = 0.0f64;
        for i in 0..POINTS {
            let y = 1.0 - 2.0 * (f64::from(i) + 0.5) / f64::from(POINTS);
            let radius = (1.0 - y * y).sqrt();
            let theta = golden * f64::from(i);
            let n = [
                (radius * theta.cos()) as f32,
                y as f32,
                (radius * theta.sin()) as f32,
            ];
            let [u, v] = octahedral_encode(n);
            let stored = [
                quantise(u, 1023) as f32 / 1023.0,
                quantise(v, 1023) as f32 / 1023.0,
            ];
            let error = angle_degrees(octahedral_decode(stored), n);
            worst = worst.max(error);
            sum += error;
        }
        let mean = sum / f64::from(POINTS);
        eprintln!("octahedral 10+10: max {worst:.4}°, mean {mean:.4}° over {POINTS} normals");
        assert!(worst < OCTAHEDRAL_MAX_DEGREES, "max error {worst}°");
        assert!(mean < 0.088, "mean error {mean}°");
        // The exact axes survive unchanged.
        for axis in [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ] {
            let [u, v] = octahedral_encode(axis);
            let stored = [
                quantise(u, 1023) as f32 / 1023.0,
                quantise(v, 1023) as f32 / 1023.0,
            ];
            assert!(
                angle_degrees(octahedral_decode(stored), axis) < 0.12,
                "{axis:?}"
            );
        }
    }

    /// The recorded octahedral bound (see the test).
    const OCTAHEDRAL_MAX_DEGREES: f64 = 0.25;

    /// Quantisation: the base colour's sRGB bytes keep the dark end (the bible's Void bodies
    /// at luma 39–58), the dielectric F0 range has its precision where the bible's materials
    /// are, and roughness and AO are within half a step.
    #[test]
    fn quantisation_steps() {
        // sRGB 8-bit: one step at linear 0.02 (byte 39) is about 4.6 % relative (a linear
        // 8-bit store would be 20 %).
        let dark = 0.02f32;
        let encoded = quantise(srgb_encode(dark), 255);
        let next = srgb_decode((encoded + 1) as f32 / 255.0);
        let this = srgb_decode(encoded as f32 / 255.0);
        assert!((next - this) / this < 0.05, "{this} → {next}");
        // F0: 0.018–0.077 (IOR 1.31–1.77) in steps under 0.0012.
        for f0 in [0.018f32, 0.04, 0.043, 0.077] {
            let x = quantise((f0 / MAX_DIELECTRIC_F0).sqrt(), 255);
            let step =
                MAX_DIELECTRIC_F0 * (((x + 1) as f32 / 255.0).powi(2) - (x as f32 / 255.0).powi(2));
            assert!(step < 0.0012, "F0 {f0}: step {step}");
            let decoded = MAX_DIELECTRIC_F0 * (x as f32 / 255.0).powi(2);
            assert!((decoded - f0).abs() <= step, "F0 {f0} → {decoded}");
        }
        // F0 0.04 (IOR 1.5, the default) stores x = 0.5 exactly → 128/255.
        assert_eq!(quantise((0.04f32 / MAX_DIELECTRIC_F0).sqrt(), 255), 128);
        // Roughness in 10 bits and AO in 8, within half a step, and the floors.
        for r in [0.0f32, 0.045, 0.3, 0.6, 1.0] {
            let stored = quantise(stored_roughness(r, 0), 1023) as f32 / 1023.0;
            assert!((stored - r.max(MIN_ROUGHNESS)).abs() <= 0.5 / 1023.0 + 1e-6);
        }
        let unit = quantise(stored_roughness(0.01, FLAG_UNIT), 1023) as f32 / 1023.0;
        assert!((unit - MIN_UNIT_ROUGHNESS).abs() <= 0.5 / 1023.0);
        for ao in [0.0f32, 0.33, 0.7, 1.0] {
            let stored = quantise(ao, 255) as f32 / 255.0;
            assert!((stored - ao).abs() <= 0.5 / 255.0 + 1e-6);
        }
        // Out-of-range and NaN inputs clamp.
        assert_eq!(quantise(-1.0, 255), 0);
        assert_eq!(quantise(2.0, 255), 255);
        assert_eq!(quantise(f32::NAN, 255), 0);
        assert_eq!(quantise(0.5, 3), 2);
    }

    /// `gb2.a` packs the model, `NO_FOG_OF_WAR` and the rim slot as §1a lays them out, and
    /// `gb1.a` the unit and reserved flags.
    #[test]
    fn alpha_bit_layout() {
        let mut s = surface(ShadingModel::Foliage);
        s.flags = FLAG_NO_FOG_OF_WAR;
        s.rim_slot = 3;
        let t = encode(&s);
        assert_eq!(t.gb2[3], 6 << 4 | 1 << 3 | 3);
        assert_eq!(t.gb1 >> 30, 0);
        s.flags = FLAG_UNIT | FLAG_RESERVED;
        s.rim_slot = 9; // Only bits 0–2 are kept.
        let t = encode(&s);
        assert_eq!(t.gb2[3], 6 << 4 | 1);
        assert_eq!(t.gb1 >> 30, 3);
    }
}
