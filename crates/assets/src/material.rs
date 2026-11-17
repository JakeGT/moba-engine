//! Client-local material descriptors (ADR-0010; the material model of ADR-0023 §6, frozen in
//! N2c).
//!
//! A model named `sentinel` may have a material at `<asset root>/materials/sentinel.toml`;
//! without one it keeps the vertex-colour look. A material is a shader permutation, a
//! parameter block, a texture set and (ADR-0023 §6) a shading model:
//!
//! ```toml
//! shader = "textured"                  # "vertex-color" (the default look) or "textured"
//! base_color = "sentinel-base"         # <root>/textures/sentinel-base.ktx2, sRGB colour
//! normal = "sentinel-normal"           # optional, tangent-space XY, linear 2-channel
//! metallic_roughness = "sentinel-mr"   # optional, R metallic / G roughness, linear; a
//!                                      # 3- or 4-channel map's B is the emissive mask
//! mask = "sentinel-mask"               # optional, team-colour mask, linear 1-channel
//! base_color_factor = [1.0, 1.0, 1.0, 1.0]  # optional, linear RGBA in 0..=1 (default 1)
//! metallic = 0.0                       # optional, 0..=1 (default 0)
//! roughness = 1.0                      # optional, 0..=1 (default 1)
//! normal_scale = 1.0                   # optional, 0..=2 (default 1)
//! team_tint = 1.0                      # optional, 0..=1: how much the mask tints (default 1)
//! emissive = [0.0, 0.0, 0.0]           # optional, linear RGB in 0..=16 (default 0)
//! # Stylisation (ADR-0011), all optional:
//! wrap = 0.3                           # 0..=1 wrap lighting (default: the map's)
//! rim = 1.0                            # 0..=4 multiplier of the map's rim (default 1)
//! ramp = 1.0                           # 0..=1 multiplier of the map's ramp strength (default 1)
//! specular = 1.0                       # 0..=2 specular scale (default 1)
//! specular_tint = [1.0, 1.0, 1.0]      # linear RGB in 0..=1, painted-specular tint (default 1)
//! # The material model (ADR-0023 §6f), all optional, textured materials only:
//! model = "standard"                   # see `ShadingModel`; default "standard"
//! ior = 1.5                            # 1..=2.5, dielectric F0 = ((ior - 1)/(ior + 1))^2 * specular
//! dust = 0.0                           # 0..=1, world-space dust on up-facing surfaces (Tiered)
//! emissive_texture = "x-emissive"      # sRGB colour, times `emissive` (Tiered only)
//! model_texture = "x-model"            # linear RGBA; R, G, B scale the model's data per texel
//! [clearcoat]                          # one table, named after `model`, with its parameters
//! weight = 1.0
//! roughness = 0.1
//! [compat]                             # overrides of the Compat fallback (ADR-0023 §10)
//! roughness = 0.3
//! ```
//!
//! Like map visuals (#287), descriptors are untrusted, client-local and cosmetic: they never
//! reach the simulation, the wire or the content hash. [`MaterialDesc::load_file`] reads at
//! most [`MAX_MATERIAL_BYTES`], rejects unknown keys, range-checks every number and accepts
//! only texture names that cannot leave `<root>/textures/`. Each texture slot also fixes the
//! formats it accepts ([`TextureSlot::accepts`]), so sRGB and linear data cannot be swapped.
//!
//! **One description, two evaluators** (ADR-0023 §10): Tiered evaluates the whole description
//! (the G-buffer's shading models, [`MaterialDesc::dielectric_f0`], dust, the emissive and
//! model textures); Compat evaluates [`MaterialDesc::compat_fallback`], the collapse of each
//! model to today's forward `shade()`, overridable per material by `[compat]`.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::map_visuals::valid_mesh_name;
use crate::texture::TextureFormat;

/// Largest material descriptor accepted, in bytes.
pub const MAX_MATERIAL_BYTES: u64 = 4 * 1024;

/// The dielectric F0 range the G-buffer stores (`0.16 x²` in 8 bits, ADR-0023 §6c): a
/// material whose `ior` and `specular` give more is refused.
pub const MAX_DIELECTRIC_F0: f32 = 0.16;

/// The default `ior` (glTF's 1.5).
const DEFAULT_IOR: f32 = 1.5;
/// IOR 1.5's F0 against air, `((1.5 − 1)/(1.5 + 1))²`: the base layer of every model but
/// `standard` and `transmissive` (ADR-0023 amendment 3).
pub const DEFAULT_DIELECTRIC_F0: f32 = 0.04;

/// The thin-film thickness range every material's film lies in, in nanometres (ADR-0023 §6c;
/// `KHR_materials_iridescence`'s defaults). The G-buffer stores a thickness in this range.
pub const THIN_FILM_THICKNESS_NM: (f32, f32) = (100.0, 400.0);

/// The thin-film IOR range (ADR-0023 §6c).
pub const THIN_FILM_IOR: (f32, f32) = (1.0, 2.0);

/// The shader permutation a material draws with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MaterialShader {
    /// The unit colour, lit: the look of a model without a material. Takes no textures.
    #[default]
    VertexColor,
    /// The `MATERIAL_TEXTURED` permutation: base colour texture × factor, tinted by the unit
    /// colour through the mask, with the normal map applied.
    Textured,
}

impl MaterialShader {
    /// Name used in descriptors (`"vertex-color"`, `"textured"`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::VertexColor => "vertex-color",
            Self::Textured => "textured",
        }
    }

    /// The shader called `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::VertexColor, Self::Textured]
            .into_iter()
            .find(|shader| shader.name() == name)
    }
}

/// The shading model of a material (ADR-0023 §6c): a bounded set, one optional top layer at
/// most, each named after its glTF `KHR_materials_*` / `OpenPBR` source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ShadingModel {
    /// GGX + Lambert + multiscatter; `ior` and `specular` give the dielectric F0.
    #[default]
    Standard,
    /// The base plus one GGX coat lobe (IOR 1.5, the base normal) (`KHR_materials_clearcoat`).
    Clearcoat,
    /// The base plus Charlie sheen (`KHR_materials_sheen`).
    Sheen,
    /// Anisotropic GGX (`KHR_materials_anisotropy`).
    Anisotropic,
    /// GGX with a thin-film Fresnel (`KHR_materials_iridescence`, Belcour and Barla 2017).
    ThinFilm,
    /// Two-sided Lambert with light through the leaf (`KHR_materials_diffuse_transmission`,
    /// a release candidate).
    Foliage,
    /// No lighting: the emissive alone (`KHR_materials_unlit`).
    Unlit,
    /// Transmissive glass or crystal (`KHR_materials_transmission`, `_volume`): the forward
    /// `glass` pass on Medium and up (N2f); until then, and on Low, the opaque fallback.
    Transmissive,
}

impl ShadingModel {
    /// Every model, in G-buffer ID order where they have one.
    pub const ALL: &'static [Self] = &[
        Self::Unlit,
        Self::Standard,
        Self::Clearcoat,
        Self::Sheen,
        Self::Anisotropic,
        Self::ThinFilm,
        Self::Foliage,
        Self::Transmissive,
    ];

    /// Name used in descriptors (`model = "..."`), and the name of the model's parameter
    /// table.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Clearcoat => "clearcoat",
            Self::Sheen => "sheen",
            Self::Anisotropic => "anisotropic",
            Self::ThinFilm => "thin_film",
            Self::Foliage => "foliage",
            Self::Unlit => "unlit",
            Self::Transmissive => "transmissive",
        }
    }

    /// The model called `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|model| model.name() == name)
    }

    /// The model's ID in the G-buffer (`gb2.a` bits 4–7, ADR-0023 §6c). A transmissive
    /// material drawn into the G-buffer is `TRANSMISSIVE_FALLBACK` (7).
    #[must_use]
    pub const fn gbuffer_id(self) -> u8 {
        match self {
            Self::Unlit => 0,
            Self::Standard => 1,
            Self::Clearcoat => 2,
            Self::Sheen => 3,
            Self::Anisotropic => 4,
            Self::ThinFilm => 5,
            Self::Foliage => 6,
            Self::Transmissive => 7,
        }
    }
}

/// A model's parameters, in glTF units (ADR-0023 §6f: the table named after the model).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum ModelParams {
    /// No parameters.
    Standard,
    /// `[clearcoat]`: `weight` 0..=1 (default 1), `roughness` 0..=1 (default 0).
    Clearcoat {
        /// Coat weight (`clearcoatFactor`).
        weight: f32,
        /// Coat perceptual roughness (`clearcoatRoughnessFactor`).
        roughness: f32,
    },
    /// `[sheen]`: `weight` 0..=1 (default 1), `roughness` 0..=1 (default 0.5), `tint` 0..=1
    /// (default 0: white sheen; 1: the base colour).
    Sheen {
        /// Sheen weight (the brightest channel of `sheenColorFactor`).
        weight: f32,
        /// Sheen perceptual roughness (`sheenRoughnessFactor`).
        roughness: f32,
        /// How much the sheen takes the base colour's hue.
        tint: f32,
    },
    /// `[anisotropic]`: `strength` 0..=1 (default 0.5), `rotation` −π..=π radians (default 0),
    /// measured in a basis built from the normal (ADR-0023 §6c, no stored tangent).
    Anisotropic {
        /// `anisotropyStrength`.
        strength: f32,
        /// `anisotropyRotation`, radians.
        rotation: f32,
    },
    /// `[thin_film]`: `thickness_min` and `thickness_max` in nanometres within
    /// [`THIN_FILM_THICKNESS_NM`] (defaults 100 and 400; the model texture's R picks between
    /// them, the maximum without one) and the film `ior` within [`THIN_FILM_IOR`] (default
    /// 1.3).
    ThinFilm {
        /// `iridescenceThicknessMinimum`, nm.
        thickness_min: f32,
        /// `iridescenceThicknessMaximum`, nm.
        thickness_max: f32,
        /// `iridescenceIor`.
        ior: f32,
    },
    /// `[foliage]`: `translucency` 0..=1 (default 0.35).
    Foliage {
        /// Light through the leaf (`diffuseTransmissionFactor`).
        translucency: f32,
    },
    /// No parameters.
    Unlit,
    /// No parameters in the model table; see [`Transmission`].
    Transmissive,
}

impl ModelParams {
    /// The model these parameters belong to.
    #[must_use]
    pub const fn model(&self) -> ShadingModel {
        match self {
            Self::Standard => ShadingModel::Standard,
            Self::Clearcoat { .. } => ShadingModel::Clearcoat,
            Self::Sheen { .. } => ShadingModel::Sheen,
            Self::Anisotropic { .. } => ShadingModel::Anisotropic,
            Self::ThinFilm { .. } => ShadingModel::ThinFilm,
            Self::Foliage { .. } => ShadingModel::Foliage,
            Self::Unlit => ShadingModel::Unlit,
            Self::Transmissive => ShadingModel::Transmissive,
        }
    }

    /// `model`'s parameters with every default.
    #[must_use]
    pub const fn defaults(model: ShadingModel) -> Self {
        match model {
            ShadingModel::Standard => Self::Standard,
            ShadingModel::Clearcoat => Self::Clearcoat {
                weight: 1.0,
                roughness: 0.0,
            },
            ShadingModel::Sheen => Self::Sheen {
                weight: 1.0,
                roughness: 0.5,
                tint: 0.0,
            },
            ShadingModel::Anisotropic => Self::Anisotropic {
                strength: 0.5,
                rotation: 0.0,
            },
            ShadingModel::ThinFilm => Self::ThinFilm {
                thickness_min: THIN_FILM_THICKNESS_NM.0,
                thickness_max: THIN_FILM_THICKNESS_NM.1,
                ior: 1.3,
            },
            ShadingModel::Foliage => Self::Foliage { translucency: 0.35 },
            ShadingModel::Unlit => Self::Unlit,
            ShadingModel::Transmissive => Self::Transmissive,
        }
    }
}

/// `[transmission]` (`KHR_materials_transmission` and `_volume`), for `model = "transmissive"`
/// only. The forward `glass` pass (ADR-0023 §5c, N2f) evaluates it; the G-buffer's opaque
/// fallback does not.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Transmission {
    /// `transmissionFactor`, 0..=1 (default 1).
    pub transmission: f32,
    /// `thicknessFactor` in metres, 0..=10 (default 0: thin-walled).
    pub thickness: f32,
    /// `attenuationColor`, linear RGB in 0..=1 (default white).
    pub attenuation_color: [f32; 3],
    /// `attenuationDistance` in metres, 0.001..=1000; `None` (the default) is infinite.
    pub attenuation_distance: Option<f32>,
}

impl Default for Transmission {
    fn default() -> Self {
        Self {
            transmission: 1.0,
            thickness: 0.0,
            attenuation_color: [1.0; 3],
            attenuation_distance: None,
        }
    }
}

/// `[compat]`: per-material overrides of the Compat fallback (ADR-0023 §10), each in its
/// top-level key's range. Unset values come from the collapse rule of the material's model.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct CompatOverride {
    /// Replaces the collapsed base colour factor.
    pub base_color_factor: Option<[f32; 4]>,
    /// Replaces the collapsed metallic factor.
    pub metallic: Option<f32>,
    /// Replaces the collapsed roughness factor.
    pub roughness: Option<f32>,
    /// Replaces the collapsed emissive colour.
    pub emissive: Option<[f32; 3]>,
}

/// What Compat (and the forward Tiered frame) draws a material with: today's forward
/// `shade()` parameters after the model's collapse rule and the `[compat]` overrides
/// ([`MaterialDesc::compat_fallback`]).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct CompatMaterial {
    /// Linear RGBA base colour factor.
    pub base_color_factor: [f32; 4],
    /// Metallic factor.
    pub metallic: f32,
    /// Roughness factor.
    pub roughness: f32,
    /// Linear RGB emissive, added after lighting.
    pub emissive: [f32; 3],
    /// Painted-specular scale (ADR-0011), 0 for `UNLIT`.
    pub specular: f32,
    /// Rim multiplier (ADR-0011), 0 for `UNLIT`.
    pub rim: f32,
}

/// A texture binding of a material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum TextureSlot {
    /// sRGB base colour (`bc7-srgb` or `rgba8-srgb`).
    BaseColor,
    /// Tangent-space normal XY, linear (`bc5` or `rg8`).
    Normal,
    /// Metallic (R) and roughness (G), linear: `bc5` or `rg8`, or `bc7` or `rgba8`, whose B
    /// is then the emissive mask (ADR-0023 §6d).
    MetallicRoughness,
    /// Team-colour mask, linear (`bc4` or `r8`).
    Mask,
    /// Full-colour emissive, sRGB (`bc7-srgb` or `rgba8-srgb`); Tiered only (ADR-0023 §6d).
    Emissive,
    /// Per-texel model data, linear RGBA (`bc7` or `rgba8`): R, G and B scale the model's
    /// three G-buffer data channels (ADR-0023 §6c); Tiered only.
    Model,
}

impl TextureSlot {
    /// Every slot.
    pub const ALL: &'static [Self] = &[
        Self::BaseColor,
        Self::Normal,
        Self::MetallicRoughness,
        Self::Mask,
        Self::Emissive,
        Self::Model,
    ];

    /// Number of slots.
    pub const COUNT: usize = 6;

    /// The slots of the material descriptor set (set 2, bindings 1..=4) that Compat and the
    /// forward Tiered frame sample, in binding order. [`Self::Emissive`] and [`Self::Model`]
    /// are read only through the Tiered material table (ADR-0023 §6d).
    pub const SET_SLOTS: &'static [Self] = &[
        Self::BaseColor,
        Self::Normal,
        Self::MetallicRoughness,
        Self::Mask,
    ];

    /// Number of [`Self::SET_SLOTS`].
    pub const SET_COUNT: usize = 4;

    /// Position in [`Self::ALL`] (and, for the first four, the binding minus one).
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::BaseColor => 0,
            Self::Normal => 1,
            Self::MetallicRoughness => 2,
            Self::Mask => 3,
            Self::Emissive => 4,
            Self::Model => 5,
        }
    }

    /// Descriptor key (`"base_color"`, `"normal"`, `"metallic_roughness"`, `"mask"`,
    /// `"emissive_texture"`, `"model_texture"`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BaseColor => "base_color",
            Self::Normal => "normal",
            Self::MetallicRoughness => "metallic_roughness",
            Self::Mask => "mask",
            Self::Emissive => "emissive_texture",
            Self::Model => "model_texture",
        }
    }

    /// Whether a texture in `format` may be bound here: colour must be sRGB, data linear,
    /// with the slot's channel count.
    #[must_use]
    pub const fn accepts(self, format: TextureFormat) -> bool {
        match self {
            Self::BaseColor | Self::Emissive => {
                matches!(format, TextureFormat::Bc7Srgb | TextureFormat::Rgba8Srgb)
            }
            Self::Normal => matches!(format, TextureFormat::Bc5Unorm | TextureFormat::Rg8Unorm),
            Self::MetallicRoughness => matches!(
                format,
                TextureFormat::Bc5Unorm
                    | TextureFormat::Rg8Unorm
                    | TextureFormat::Bc7Unorm
                    | TextureFormat::Rgba8Unorm
            ),
            Self::Mask => matches!(format, TextureFormat::Bc4Unorm | TextureFormat::R8Unorm),
            Self::Model => matches!(format, TextureFormat::Bc7Unorm | TextureFormat::Rgba8Unorm),
        }
    }
}

/// Whether a metallic-roughness texture in `format` carries the emissive mask in B
/// (ADR-0023 §6d): a 3- or 4-channel map does; a 2-channel one has none, and its emissive is
/// unmasked (a mask of 1), so every material made before the mask keeps its look.
#[must_use]
pub const fn has_emissive_mask(format: TextureFormat) -> bool {
    format.channels() >= 3
}

/// A validated material descriptor.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MaterialDesc {
    /// Shader permutation.
    pub shader: MaterialShader,
    /// Texture names per [`TextureSlot`]; private so a new slot is not a breaking change (use
    /// [`MaterialDesc::texture`] and [`MaterialDesc::bound_textures`]).
    textures: [Option<String>; TextureSlot::COUNT],
    /// Linear RGBA multiplier of the base colour.
    pub base_color_factor: [f32; 4],
    /// Metallic factor, times the metallic-roughness texture's R.
    pub metallic: f32,
    /// Roughness factor, times the metallic-roughness texture's G.
    pub roughness: f32,
    /// Normal map strength.
    pub normal_scale: f32,
    /// How strongly the mask tints the base colour by the unit colour.
    pub team_tint: f32,
    /// Linear RGB emissive factor, added after lighting.
    pub emissive: [f32; 3],
    /// Wrap lighting `0..=1`; `None` uses the map's (ADR-0011).
    pub wrap: Option<f32>,
    /// Multiplier of the map's rim strength, `0..=4`.
    pub rim: f32,
    /// Multiplier of the map's ramp strength, `0..=1`.
    pub ramp: f32,
    /// Specular scale, `0..=2`: the painted-specular scale on Compat (ADR-0011) and, on
    /// Tiered, `KHR_materials_specular`'s factor on the dielectric F0 (ADR-0023 §6b).
    pub specular: f32,
    /// Painted-specular tint, linear RGB in `0..=1`.
    pub specular_tint: [f32; 3],
    /// The shading model and its parameters (ADR-0023 §6c, §6f).
    pub model: ModelParams,
    /// Index of refraction of the dielectric base, `1..=2.5` (default 1.5: F0 0.04).
    pub ior: f32,
    /// World-space dust amount on up-facing surfaces, `0..=1` (Tiered, ADR-0023 §6a).
    pub dust: f32,
    /// `[transmission]`, for [`ShadingModel::Transmissive`] only.
    pub transmission: Option<Transmission>,
    /// `[compat]` overrides of the Compat fallback.
    pub compat: CompatOverride,
}

impl Default for MaterialDesc {
    fn default() -> Self {
        Self {
            shader: MaterialShader::VertexColor,
            textures: [None, None, None, None, None, None],
            base_color_factor: [1.0; 4],
            metallic: 0.0,
            roughness: 1.0,
            normal_scale: 1.0,
            team_tint: 1.0,
            emissive: [0.0; 3],
            wrap: None,
            rim: 1.0,
            ramp: 1.0,
            specular: 1.0,
            specular_tint: [1.0; 3],
            model: ModelParams::Standard,
            ior: DEFAULT_IOR,
            dust: 0.0,
            transmission: None,
            compat: CompatOverride::default(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMaterial {
    shader: String,
    #[serde(default)]
    base_color: Option<String>,
    #[serde(default)]
    normal: Option<String>,
    #[serde(default)]
    metallic_roughness: Option<String>,
    #[serde(default)]
    mask: Option<String>,
    #[serde(default)]
    emissive_texture: Option<String>,
    #[serde(default)]
    model_texture: Option<String>,
    #[serde(default)]
    base_color_factor: Option<[f32; 4]>,
    #[serde(default)]
    metallic: Option<f32>,
    #[serde(default)]
    roughness: Option<f32>,
    #[serde(default)]
    normal_scale: Option<f32>,
    #[serde(default)]
    team_tint: Option<f32>,
    #[serde(default)]
    emissive: Option<[f32; 3]>,
    #[serde(default)]
    wrap: Option<f32>,
    #[serde(default)]
    rim: Option<f32>,
    #[serde(default)]
    ramp: Option<f32>,
    #[serde(default)]
    specular: Option<f32>,
    #[serde(default)]
    specular_tint: Option<[f32; 3]>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    ior: Option<f32>,
    #[serde(default)]
    dust: Option<f32>,
    #[serde(default)]
    clearcoat: Option<RawClearcoat>,
    #[serde(default)]
    sheen: Option<RawSheen>,
    #[serde(default)]
    anisotropic: Option<RawAnisotropic>,
    #[serde(default)]
    thin_film: Option<RawThinFilm>,
    #[serde(default)]
    foliage: Option<RawFoliage>,
    #[serde(default)]
    standard: Option<RawEmpty>,
    #[serde(default)]
    unlit: Option<RawEmpty>,
    #[serde(default)]
    transmission: Option<RawTransmission>,
    #[serde(default)]
    compat: Option<RawCompat>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEmpty {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClearcoat {
    #[serde(default)]
    weight: Option<f32>,
    #[serde(default)]
    roughness: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSheen {
    #[serde(default)]
    weight: Option<f32>,
    #[serde(default)]
    roughness: Option<f32>,
    #[serde(default)]
    tint: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAnisotropic {
    #[serde(default)]
    strength: Option<f32>,
    #[serde(default)]
    rotation: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawThinFilm {
    #[serde(default)]
    thickness_min: Option<f32>,
    #[serde(default)]
    thickness_max: Option<f32>,
    #[serde(default)]
    ior: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFoliage {
    #[serde(default)]
    translucency: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTransmission {
    #[serde(default)]
    transmission: Option<f32>,
    #[serde(default)]
    thickness: Option<f32>,
    #[serde(default)]
    attenuation_color: Option<[f32; 3]>,
    #[serde(default)]
    attenuation_distance: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCompat {
    #[serde(default)]
    base_color_factor: Option<[f32; 4]>,
    #[serde(default)]
    metallic: Option<f32>,
    #[serde(default)]
    roughness: Option<f32>,
    #[serde(default)]
    emissive: Option<[f32; 3]>,
}

/// Why a material descriptor could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MaterialError {
    /// The file does not exist or cannot be read; the text is the I/O error.
    Io(String),
    /// The path is not a regular file.
    NotAFile,
    /// The file is larger than [`MAX_MATERIAL_BYTES`].
    TooLarge,
    /// TOML syntax or schema error (unknown key, wrong type, missing field).
    Toml(String),
    /// A value is out of range or inconsistent; the text names it.
    Invalid(&'static str),
}

impl core::fmt::Display for MaterialError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "missing or unreadable ({error})"),
            Self::NotAFile => f.write_str("not a regular file"),
            Self::TooLarge => write!(f, "larger than {MAX_MATERIAL_BYTES} bytes"),
            Self::Toml(error) => write!(f, "material TOML: {}", error.trim_end()),
            Self::Invalid(what) => write!(f, "material: {what}"),
        }
    }
}

impl std::error::Error for MaterialError {}

fn in_range(value: f32, low: f32, high: f32) -> bool {
    value.is_finite() && (low..=high).contains(&value)
}

/// `value` (or `default`) if it lies in `low..=high`.
fn ranged(
    value: Option<f32>,
    default: f32,
    low: f32,
    high: f32,
    what: &'static str,
) -> Result<f32, MaterialError> {
    match value {
        None => Ok(default),
        Some(value) if in_range(value, low, high) => Ok(value),
        Some(_) => Err(MaterialError::Invalid(what)),
    }
}

fn unit_colour<const N: usize>(
    value: Option<[f32; N]>,
    default: [f32; N],
    high: f32,
    what: &'static str,
) -> Result<[f32; N], MaterialError> {
    let value = value.unwrap_or(default);
    if value.iter().all(|&channel| in_range(channel, 0.0, high)) {
        Ok(value)
    } else {
        Err(MaterialError::Invalid(what))
    }
}

/// The dielectric F0 of `ior` against air: `((ior − 1)/(ior + 1))²` (`KHR_materials_ior`).
#[must_use]
pub fn ior_to_f0(ior: f32) -> f32 {
    let r = (ior - 1.0) / (ior + 1.0);
    r * r
}

/// The model's parameters from the descriptor's tables: exactly the declared model's table
/// may be present.
#[allow(clippy::too_many_lines)] // One arm per model, each with its ranges.
fn model_params(raw: &RawMaterial, model: ShadingModel) -> Result<ModelParams, MaterialError> {
    let tables = [
        (ShadingModel::Standard, raw.standard.is_some()),
        (ShadingModel::Clearcoat, raw.clearcoat.is_some()),
        (ShadingModel::Sheen, raw.sheen.is_some()),
        (ShadingModel::Anisotropic, raw.anisotropic.is_some()),
        (ShadingModel::ThinFilm, raw.thin_film.is_some()),
        (ShadingModel::Foliage, raw.foliage.is_some()),
        (ShadingModel::Unlit, raw.unlit.is_some()),
    ];
    if tables
        .iter()
        .any(|&(table, present)| present && table != model)
    {
        return Err(MaterialError::Invalid(
            "a model parameter table must be named after `model` (one table at most)",
        ));
    }
    let defaults = ModelParams::defaults(model);
    Ok(match (model, defaults) {
        (ShadingModel::Clearcoat, ModelParams::Clearcoat { weight, roughness }) => {
            let table = raw.clearcoat.as_ref();
            ModelParams::Clearcoat {
                weight: ranged(
                    table.and_then(|t| t.weight),
                    weight,
                    0.0,
                    1.0,
                    "clearcoat.weight must be in 0..=1",
                )?,
                roughness: ranged(
                    table.and_then(|t| t.roughness),
                    roughness,
                    0.0,
                    1.0,
                    "clearcoat.roughness must be in 0..=1",
                )?,
            }
        }
        (
            ShadingModel::Sheen,
            ModelParams::Sheen {
                weight,
                roughness,
                tint,
            },
        ) => {
            let table = raw.sheen.as_ref();
            ModelParams::Sheen {
                weight: ranged(
                    table.and_then(|t| t.weight),
                    weight,
                    0.0,
                    1.0,
                    "sheen.weight must be in 0..=1",
                )?,
                roughness: ranged(
                    table.and_then(|t| t.roughness),
                    roughness,
                    0.0,
                    1.0,
                    "sheen.roughness must be in 0..=1",
                )?,
                tint: ranged(
                    table.and_then(|t| t.tint),
                    tint,
                    0.0,
                    1.0,
                    "sheen.tint must be in 0..=1",
                )?,
            }
        }
        (ShadingModel::Anisotropic, ModelParams::Anisotropic { strength, rotation }) => {
            let table = raw.anisotropic.as_ref();
            ModelParams::Anisotropic {
                strength: ranged(
                    table.and_then(|t| t.strength),
                    strength,
                    0.0,
                    1.0,
                    "anisotropic.strength must be in 0..=1",
                )?,
                rotation: ranged(
                    table.and_then(|t| t.rotation),
                    rotation,
                    -core::f32::consts::PI,
                    core::f32::consts::PI,
                    "anisotropic.rotation must be in -pi..=pi radians",
                )?,
            }
        }
        (
            ShadingModel::ThinFilm,
            ModelParams::ThinFilm {
                thickness_min,
                thickness_max,
                ior,
            },
        ) => {
            let table = raw.thin_film.as_ref();
            let (low, high) = THIN_FILM_THICKNESS_NM;
            let thickness_min = ranged(
                table.and_then(|t| t.thickness_min),
                thickness_min,
                low,
                high,
                "thin_film.thickness_min must be in 100..=400 nm",
            )?;
            let thickness_max = ranged(
                table.and_then(|t| t.thickness_max),
                thickness_max,
                low,
                high,
                "thin_film.thickness_max must be in 100..=400 nm",
            )?;
            if thickness_max < thickness_min {
                return Err(MaterialError::Invalid(
                    "thin_film.thickness_max must not be below thickness_min",
                ));
            }
            ModelParams::ThinFilm {
                thickness_min,
                thickness_max,
                ior: ranged(
                    table.and_then(|t| t.ior),
                    ior,
                    THIN_FILM_IOR.0,
                    THIN_FILM_IOR.1,
                    "thin_film.ior must be in 1..=2",
                )?,
            }
        }
        (ShadingModel::Foliage, ModelParams::Foliage { translucency }) => ModelParams::Foliage {
            translucency: ranged(
                raw.foliage.as_ref().and_then(|t| t.translucency),
                translucency,
                0.0,
                1.0,
                "foliage.translucency must be in 0..=1",
            )?,
        },
        (_, defaults) => defaults,
    })
}

impl MaterialDesc {
    /// Parses and validates a descriptor.
    ///
    /// # Errors
    /// [`MaterialError::Toml`] for syntax/schema errors, [`MaterialError::Invalid`] for an
    /// unknown shader or model, a bad texture name, an out-of-range number, a textured
    /// material without a base colour, a vertex-colour material with textures or material
    /// model keys, a model table that does not match `model`, `[transmission]` on another
    /// model, or an `ior` and `specular` whose dielectric F0 exceeds [`MAX_DIELECTRIC_F0`].
    #[allow(clippy::too_many_lines)] // One check per key.
    pub fn parse(text: &str) -> Result<Self, MaterialError> {
        if text.len() as u64 > MAX_MATERIAL_BYTES {
            return Err(MaterialError::TooLarge);
        }
        let raw: RawMaterial =
            toml::from_str(text).map_err(|error| MaterialError::Toml(error.to_string()))?;
        let shader = MaterialShader::from_name(&raw.shader).ok_or(MaterialError::Invalid(
            "shader must be \"vertex-color\" or \"textured\"",
        ))?;
        let textures = [
            raw.base_color.clone(),
            raw.normal.clone(),
            raw.metallic_roughness.clone(),
            raw.mask.clone(),
            raw.emissive_texture.clone(),
            raw.model_texture.clone(),
        ];
        if textures.iter().flatten().any(|name| !valid_mesh_name(name)) {
            return Err(MaterialError::Invalid(
                "texture names are 1-32 of [A-Za-z0-9_-]",
            ));
        }
        let model_keys = raw.model.is_some()
            || raw.ior.is_some()
            || raw.dust.is_some()
            || raw.standard.is_some()
            || raw.clearcoat.is_some()
            || raw.sheen.is_some()
            || raw.anisotropic.is_some()
            || raw.thin_film.is_some()
            || raw.foliage.is_some()
            || raw.unlit.is_some()
            || raw.transmission.is_some()
            || raw.compat.is_some();
        match shader {
            MaterialShader::Textured if textures[TextureSlot::BaseColor.index()].is_none() => {
                return Err(MaterialError::Invalid(
                    "a textured material needs base_color",
                ));
            }
            MaterialShader::VertexColor if textures.iter().any(Option::is_some) => {
                return Err(MaterialError::Invalid(
                    "a vertex-color material takes no textures",
                ));
            }
            MaterialShader::VertexColor if model_keys => {
                return Err(MaterialError::Invalid(
                    "the material model keys need shader = \"textured\"",
                ));
            }
            _ => {}
        }
        let defaults = Self::default();
        let base_color_factor = unit_colour(
            raw.base_color_factor,
            defaults.base_color_factor,
            1.0,
            "base_color_factor must be in 0..=1",
        )?;
        let metallic = ranged(
            raw.metallic,
            defaults.metallic,
            0.0,
            1.0,
            "metallic must be in 0..=1",
        )?;
        let roughness = ranged(
            raw.roughness,
            defaults.roughness,
            0.0,
            1.0,
            "roughness must be in 0..=1",
        )?;
        let team_tint = ranged(
            raw.team_tint,
            defaults.team_tint,
            0.0,
            1.0,
            "team_tint must be in 0..=1",
        )?;
        let normal_scale = ranged(
            raw.normal_scale,
            defaults.normal_scale,
            0.0,
            2.0,
            "normal_scale must be in 0..=2",
        )?;
        let emissive = unit_colour(
            raw.emissive,
            defaults.emissive,
            16.0,
            "emissive must be in 0..=16",
        )?;
        let wrap = match raw.wrap {
            None => None,
            Some(value) if in_range(value, 0.0, 1.0) => Some(value),
            Some(_) => return Err(MaterialError::Invalid("wrap must be in 0..=1")),
        };
        let rim = ranged(raw.rim, defaults.rim, 0.0, 4.0, "rim must be in 0..=4")?;
        let ramp = ranged(raw.ramp, defaults.ramp, 0.0, 1.0, "ramp must be in 0..=1")?;
        let specular = ranged(
            raw.specular,
            defaults.specular,
            0.0,
            2.0,
            "specular must be in 0..=2",
        )?;
        let specular_tint = unit_colour(
            raw.specular_tint,
            defaults.specular_tint,
            1.0,
            "specular_tint must be in 0..=1",
        )?;
        let model = match raw.model.as_deref() {
            None => ShadingModel::Standard,
            Some(name) => ShadingModel::from_name(name).ok_or(MaterialError::Invalid(
                "model must be \"standard\", \"clearcoat\", \"sheen\", \"anisotropic\", \
                 \"thin_film\", \"foliage\", \"unlit\" or \"transmissive\"",
            ))?,
        };
        let params = model_params(&raw, model)?;
        let ior = ranged(raw.ior, defaults.ior, 1.0, 2.5, "ior must be in 1..=2.5")?;
        if ior_to_f0(ior) * specular > MAX_DIELECTRIC_F0 + 1e-6 {
            return Err(MaterialError::Invalid(
                "ior and specular give a dielectric F0 above 0.16 (the G-buffer's range)",
            ));
        }
        let dust = ranged(raw.dust, defaults.dust, 0.0, 1.0, "dust must be in 0..=1")?;
        let transmission = match (&raw.transmission, model) {
            (None, ShadingModel::Transmissive) => Some(Transmission::default()),
            (None, _) => None,
            (Some(_), model) if model != ShadingModel::Transmissive => {
                return Err(MaterialError::Invalid(
                    "[transmission] needs model = \"transmissive\"",
                ));
            }
            (Some(table), _) => {
                let fallback = Transmission::default();
                Some(Transmission {
                    transmission: ranged(
                        table.transmission,
                        fallback.transmission,
                        0.0,
                        1.0,
                        "transmission.transmission must be in 0..=1",
                    )?,
                    thickness: ranged(
                        table.thickness,
                        fallback.thickness,
                        0.0,
                        10.0,
                        "transmission.thickness must be in 0..=10 m",
                    )?,
                    attenuation_color: unit_colour(
                        table.attenuation_color,
                        fallback.attenuation_color,
                        1.0,
                        "transmission.attenuation_color must be in 0..=1",
                    )?,
                    attenuation_distance: match table.attenuation_distance {
                        None => None,
                        Some(value) if in_range(value, 0.001, 1000.0) => Some(value),
                        Some(_) => {
                            return Err(MaterialError::Invalid(
                                "transmission.attenuation_distance must be in 0.001..=1000 m",
                            ));
                        }
                    },
                })
            }
        };
        let compat = match &raw.compat {
            None => CompatOverride::default(),
            Some(table) => CompatOverride {
                base_color_factor: table
                    .base_color_factor
                    .map(|value| {
                        unit_colour(
                            Some(value),
                            value,
                            1.0,
                            "compat.base_color_factor must be in 0..=1",
                        )
                    })
                    .transpose()?,
                metallic: table
                    .metallic
                    .map(|value| {
                        ranged(
                            Some(value),
                            value,
                            0.0,
                            1.0,
                            "compat.metallic must be in 0..=1",
                        )
                    })
                    .transpose()?,
                roughness: table
                    .roughness
                    .map(|value| {
                        ranged(
                            Some(value),
                            value,
                            0.0,
                            1.0,
                            "compat.roughness must be in 0..=1",
                        )
                    })
                    .transpose()?,
                emissive: table
                    .emissive
                    .map(|value| {
                        unit_colour(
                            Some(value),
                            value,
                            16.0,
                            "compat.emissive must be in 0..=16",
                        )
                    })
                    .transpose()?,
            },
        };
        Ok(Self {
            shader,
            textures,
            base_color_factor,
            metallic,
            roughness,
            normal_scale,
            team_tint,
            emissive,
            wrap,
            rim,
            ramp,
            specular,
            specular_tint,
            model: params,
            ior,
            dust,
            transmission,
            compat,
        })
    }

    /// Reads and validates a descriptor file of at most [`MAX_MATERIAL_BYTES`].
    ///
    /// # Errors
    /// [`MaterialError::Io`], [`MaterialError::NotAFile`], [`MaterialError::TooLarge`] or a
    /// [`Self::parse`] error.
    pub fn load_file(path: &Path) -> Result<Self, MaterialError> {
        let metadata =
            std::fs::metadata(path).map_err(|error| MaterialError::Io(error.to_string()))?;
        if !metadata.is_file() {
            return Err(MaterialError::NotAFile);
        }
        if metadata.len() > MAX_MATERIAL_BYTES {
            return Err(MaterialError::TooLarge);
        }
        let file =
            std::fs::File::open(path).map_err(|error| MaterialError::Io(error.to_string()))?;
        let mut text = String::new();
        file.take(MAX_MATERIAL_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|error| MaterialError::Io(error.to_string()))?;
        Self::parse(&text)
    }

    /// The texture name bound to `slot`, if any.
    #[must_use]
    pub fn texture(&self, slot: TextureSlot) -> Option<&str> {
        self.textures[slot.index()].as_deref()
    }

    /// `(slot, name)` for every bound texture, in slot order.
    pub fn bound_textures(&self) -> impl Iterator<Item = (TextureSlot, &str)> {
        TextureSlot::ALL
            .iter()
            .filter_map(|&slot| self.texture(slot).map(|name| (slot, name)))
    }

    /// The shading model.
    #[must_use]
    pub const fn shading_model(&self) -> ShadingModel {
        self.model.model()
    }

    /// The dielectric F0 of `ior` and `specular` (ADR-0023 §6b):
    /// `((ior − 1)/(ior + 1))² × specular`, at most [`MAX_DIELECTRIC_F0`]. 0.04 by default.
    #[must_use]
    pub fn dielectric_f0(&self) -> f32 {
        (ior_to_f0(self.ior) * self.specular).min(MAX_DIELECTRIC_F0)
    }

    /// Whether the model's base layer takes its dielectric F0 from `ior` and `specular`:
    /// `standard` and `transmissive`, whose G-buffer channel stores it. Every other model's
    /// base is IOR 1.5's 0.04 on every path (ADR-0023 amendment 3), and `content-check`
    /// refuses an `ior` or `specular` on it.
    #[must_use]
    pub const fn model_takes_ior(&self) -> bool {
        matches!(
            self.model,
            ModelParams::Standard | ModelParams::Transmissive
        )
    }

    /// The dielectric F0 the Tiered paths shade the base layer with: [`Self::dielectric_f0`]
    /// where [`Self::model_takes_ior`], 0.04 (IOR 1.5) otherwise. The deferred lighting, the
    /// forward Tiered frame and the thin film's Compat tint all use this.
    #[must_use]
    pub fn model_dielectric_f0(&self) -> f32 {
        if self.model_takes_ior() {
            self.dielectric_f0()
        } else {
            DEFAULT_DIELECTRIC_F0
        }
    }

    /// The Compat fallback (ADR-0023 §10): the model's collapse rule to today's forward
    /// `shade()`, then the `[compat]` overrides. Compat draws this; so does the forward
    /// Tiered frame (`MOBA_DEFERRED=off`, or a device that cannot run the deferred passes),
    /// whose shading is Compat's plus the Tiered BRDF core.
    ///
    /// | Model | Collapse |
    /// |---|---|
    /// | `standard`, `foliage`, `sheen` | the base as it is |
    /// | `clearcoat` | roughness `mix(base, coat, 0.5 × weight)` |
    /// | `anisotropic` | isotropic at the mean of the two GGX alphas |
    /// | `thin_film` | the base colour tinted by the film's normal-incidence reflectance |
    /// | `transmissive` | the opaque base and the emissive (mask) |
    /// | `unlit` | the emissive alone: no base colour, specular or rim |
    #[must_use]
    pub fn compat_fallback(&self) -> CompatMaterial {
        let mut out = CompatMaterial {
            base_color_factor: self.base_color_factor,
            metallic: self.metallic,
            roughness: self.roughness,
            emissive: self.emissive,
            specular: self.specular,
            rim: self.rim,
        };
        match self.model {
            ModelParams::Clearcoat { weight, roughness } => {
                out.roughness += (roughness - out.roughness) * 0.5 * weight;
            }
            ModelParams::Anisotropic { strength, .. } => {
                // KHR_materials_anisotropy: alpha_t = mix(alpha, 1, strength²), alpha_b = alpha.
                let alpha = out.roughness * out.roughness;
                let alpha_t = alpha + (1.0 - alpha) * strength * strength;
                out.roughness = f32::midpoint(alpha, alpha_t).sqrt();
            }
            ModelParams::ThinFilm {
                thickness_max, ior, ..
            } => {
                // The film over the base at normal incidence, as a tint of the base colour
                // (the film's thickness without a model texture is its maximum). Only a
                // metal's base colour is its F0: a dielectric's albedo is diffuse, which the
                // film does not colour, so the tint is mixed in by `metallic` (otherwise
                // 0.04's ratio would scale the albedo several-fold and clip it to white).
                let f0 = self.base_f0_rgb();
                let film = thin_film_reflectance(1.0, ior, thickness_max, f0);
                for channel in 0..3 {
                    let tint = if f0[channel] > 1e-4 {
                        film[channel] / f0[channel]
                    } else {
                        1.0
                    };
                    let tint = 1.0 + (tint - 1.0) * self.metallic;
                    out.base_color_factor[channel] =
                        (out.base_color_factor[channel] * tint).clamp(0.0, 1.0);
                }
            }
            ModelParams::Unlit => {
                out.base_color_factor = [0.0, 0.0, 0.0, out.base_color_factor[3]];
                out.metallic = 0.0;
                out.specular = 0.0;
                out.rim = 0.0;
            }
            ModelParams::Standard
            | ModelParams::Sheen { .. }
            | ModelParams::Foliage { .. }
            | ModelParams::Transmissive => {}
        }
        if let Some(value) = self.compat.base_color_factor {
            out.base_color_factor = value;
        }
        if let Some(value) = self.compat.metallic {
            out.metallic = value;
        }
        if let Some(value) = self.compat.roughness {
            out.roughness = value;
        }
        if let Some(value) = self.compat.emissive {
            out.emissive = value;
        }
        out
    }

    /// `content-check`'s evaluator rules (ADR-0023 §10, gate 8): the material against both
    /// evaluators, given whether its `metallic_roughness` texture carries the emissive mask
    /// (a third channel, [`has_emissive_mask`]). Returns the Compat fallback when
    ///
    /// - every collapsed value is finite and in its key's range (base colour, metallic and
    ///   roughness 0..=1, emissive 0..=16), `[compat]` overrides included;
    /// - an `unlit` material emits something on both evaluators (a non-zero `emissive`, which
    ///   Tiered multiplies by any emissive texture, and a non-zero Compat emissive), since
    ///   both draw nothing else;
    /// - `ior` and `specular` are at their defaults (1.5 and 1) on every model but `standard`
    ///   and `transmissive` ([`Self::model_takes_ior`]): the others' base is F0 0.04;
    /// - an `emissive_texture` with a non-zero `emissive` comes with the mask or a
    ///   `[compat] emissive`: Compat ignores the texture and would glow over the whole
    ///   surface (an `unlit` material is all emissive, so it may);
    /// - a `model_texture` is on a model that reads it (every model but `unlit`).
    ///
    /// Bible ranges per material class are not checked here: they belong to the art bible
    /// (PR #347), which is not approved.
    ///
    /// # Errors
    /// [`MaterialError::Invalid`] naming the first rule that fails.
    pub fn check_evaluators(&self, emissive_mask: bool) -> Result<CompatMaterial, MaterialError> {
        let compat = self.compat_fallback();
        let unit = |value: f32| in_range(value, 0.0, 1.0);
        if !compat.base_color_factor.iter().all(|&value| unit(value))
            || !unit(compat.metallic)
            || !unit(compat.roughness)
            || !compat
                .emissive
                .iter()
                .all(|&value| in_range(value, 0.0, 16.0))
        {
            return Err(MaterialError::Invalid(
                "the Compat fallback leaves its range (base colour, metallic and roughness \
                 0..=1, emissive 0..=16)",
            ));
        }
        let lit = |emissive: [f32; 3]| emissive.iter().any(|&value| value > 0.0);
        if self.shading_model() == ShadingModel::Unlit {
            if !lit(self.emissive) {
                return Err(MaterialError::Invalid(
                    "an unlit material draws its emissive alone: its emissive is zero, so it is \
                     black on Tiered",
                ));
            }
            if !lit(compat.emissive) {
                return Err(MaterialError::Invalid(
                    "an unlit material draws its emissive alone: Compat's emissive is zero, so it \
                     is black there",
                ));
            }
            if self.texture(TextureSlot::Model).is_some() {
                return Err(MaterialError::Invalid(
                    "model_texture is never read by an unlit material",
                ));
            }
        }
        if !self.model_takes_ior()
            && ((self.ior - DEFAULT_IOR).abs() > f32::EPSILON
                || (self.specular - 1.0).abs() > f32::EPSILON)
        {
            return Err(MaterialError::Invalid(
                "ior and specular shape only a standard or transmissive material's F0: every \
                 other model's base layer is IOR 1.5 (F0 0.04) on Tiered",
            ));
        }
        if self.shading_model() != ShadingModel::Unlit
            && self.texture(TextureSlot::Emissive).is_some()
            && lit(self.emissive)
            && self.compat.emissive.is_none()
            && !emissive_mask
        {
            return Err(MaterialError::Invalid(
                "emissive_texture needs an emissive mask (a 3- or 4-channel metallic_roughness) \
                 or a [compat] emissive: Compat ignores the texture and would glow over the \
                 whole surface",
            ));
        }
        Ok(compat)
    }

    /// The base layer's F0 per channel from the factors (no texture): the base colour for a
    /// metal, the model's dielectric F0 otherwise, mixed by `metallic`.
    fn base_f0_rgb(&self) -> [f32; 3] {
        let dielectric = self.model_dielectric_f0();
        core::array::from_fn(|channel| {
            dielectric + (self.base_color_factor[channel] - dielectric) * self.metallic
        })
    }
}

/// CIE 1931 XYZ → linear Rec. 709 (sRGB primaries, D65).
const XYZ_TO_REC709: [[f32; 3]; 3] = [
    [3.240_454_2, -1.537_138_5, -0.498_531_4],
    [-0.969_266, 1.876_010_8, 0.041_556],
    [0.055_643_4, -0.204_025_9, 1.057_225_2],
];

fn schlick(f0: f32, cosine: f32) -> f32 {
    let f = (1.0 - cosine.clamp(0.0, 1.0)).powi(5);
    f0 + (1.0 - f0) * f
}

/// The reflectance of a thin film of index `film_ior` and `thickness_nm` between a medium of
/// index `outside_ior` and a base of normal-incidence reflectance `base_f0`, seen at
/// `cos_theta`, in linear Rec. 709 (Belcour and Barla 2017, "A Practical Extension to
/// Microfacet Theory for the Modeling of Varying Iridescence": the Airy sum to second order
/// with the CIE sensitivities as Gaussians in Fourier space; the form
/// `KHR_materials_iridescence` specifies). The CPU twin of `model_thin_film_fresnel` in
/// `shaders/include/shading_models.glsl`; the Compat fallback uses it at normal incidence.
///
/// Adapted from the Khronos glTF Sample Viewer's `iridescence.glsl` (Copyright The Khronos
/// Group Inc., Apache License 2.0; see `THIRD_PARTY.md`), as are `sensitivity` and
/// `ior_to_f0_between`; changed: ported to Rust, per channel.
#[must_use]
#[allow(clippy::many_single_char_names)] // The paper's names.
pub fn thin_film_reflectance(
    cos_theta: f32,
    film_ior: f32,
    thickness_nm: f32,
    base_f0: [f32; 3],
) -> [f32; 3] {
    const OUTSIDE_IOR: f32 = 1.0;
    let pi = core::f32::consts::PI;
    // A vanishing film fades to the outside medium, so the base shows through.
    let smooth = {
        let t = (thickness_nm / 0.03).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let film = OUTSIDE_IOR + (film_ior - OUTSIDE_IOR) * smooth;
    let sin2 = (OUTSIDE_IOR / film).powi(2) * (1.0 - cos_theta * cos_theta);
    let cos2_sq = 1.0 - sin2;
    if cos2_sq < 0.0 {
        return [1.0; 3]; // Total internal reflection.
    }
    let cos2 = cos2_sq.sqrt();
    // The first interface (outside → film).
    let r0 = ior_to_f0_between(film, OUTSIDE_IOR);
    let r12 = schlick(r0, cos_theta);
    let t121 = 1.0 - r12;
    let phi12 = if film < OUTSIDE_IOR { pi } else { 0.0 };
    let phi21 = pi - phi12;
    // The second interface (film → base), per channel.
    let opd = 2.0 * film * thickness_nm * cos2;
    let mut out = [0.0_f32; 3];
    let mut cm = [0.0_f32; 3];
    let mut r123_root = [0.0_f32; 3];
    let mut phi = [0.0_f32; 3];
    for channel in 0..3 {
        let f0 = base_f0[channel].clamp(0.0, 0.9999);
        let base_ior = (1.0 + f0.sqrt()) / (1.0 - f0.sqrt());
        let r1 = ior_to_f0_between(base_ior, film);
        let r23 = schlick(r1, cos2);
        let phi23 = if base_ior < film { pi } else { 0.0 };
        let r123 = (r12 * r23).clamp(1e-5, 0.9999);
        let rs = t121 * t121 * r23 / (1.0 - r123);
        // The m = 0 term, then the pairs of Diracs for m = 1, 2 below.
        out[channel] = r12 + rs;
        cm[channel] = rs - t121;
        r123_root[channel] = r123.sqrt();
        phi[channel] = phi21 + phi23;
    }
    for order in 1..=2_u8 {
        let m = f32::from(order);
        let rgb = sensitivity(m * opd, phi.map(|shift| shift * m));
        for channel in 0..3 {
            cm[channel] *= r123_root[channel];
            out[channel] += cm[channel] * 2.0 * rgb[channel];
        }
    }
    out.map(|value| value.max(0.0))
}

fn ior_to_f0_between(transmitted: f32, incident: f32) -> f32 {
    let r = (transmitted - incident) / (transmitted + incident);
    r * r
}

/// The CIE XYZ sensitivities' Fourier transform at optical path difference `opd` (nm) with a
/// per-channel phase shift, as linear Rec. 709 (Belcour and Barla 2017's Gaussian fit).
fn sensitivity(opd: f32, shift: [f32; 3]) -> [f32; 3] {
    let phase = 2.0 * core::f32::consts::PI * opd * 1.0e-9;
    let val = [5.4856e-13_f32, 4.4201e-13, 5.2481e-13];
    let pos = [1.6810e6_f32, 1.7953e6, 2.2084e6];
    let var = [4.3278e9_f32, 9.3046e9, 6.6121e9];
    let tau = 2.0 * core::f32::consts::PI;
    let mut xyz: [f32; 3] = core::array::from_fn(|axis| {
        val[axis]
            * (tau * var[axis]).sqrt()
            * (pos[axis] * phase + shift[axis]).cos()
            * (-phase * phase * var[axis]).exp()
    });
    xyz[0] += 9.7470e-14
        * (tau * 4.5282e9_f32).sqrt()
        * (2.2399e6 * phase + shift[0]).cos()
        * (-4.5282e9 * phase * phase).exp();
    let xyz = xyz.map(|value| value / 1.0685e-7);
    core::array::from_fn(|row| {
        XYZ_TO_REC709[row][0] * xyz[0]
            + XYZ_TO_REC709[row][1] * xyz[1]
            + XYZ_TO_REC709[row][2] * xyz[2]
    })
}

/// `<root>/materials/<model>.toml`, or `None` for an invalid model name.
#[must_use]
pub fn material_path(root: &Path, model: &str) -> Option<PathBuf> {
    valid_mesh_name(model).then(|| root.join("materials").join(format!("{model}.toml")))
}

/// `<root>/textures/<name>.ktx2`, or `None` for an invalid texture name.
#[must_use]
pub fn texture_path(root: &Path, name: &str) -> Option<PathBuf> {
    valid_mesh_name(name).then(|| root.join("textures").join(format!("{name}.ktx2")))
}
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
shader = "textured"
base_color = "sentinel-base"
normal = "sentinel-normal"
mask = "sentinel-mask"
base_color_factor = [1.0, 0.9, 0.8, 1.0]
roughness = 0.6
"#;

    #[test]
    fn parses_a_textured_material_with_defaults() {
        let material = MaterialDesc::parse(SAMPLE).expect("valid");
        assert_eq!(material.shader, MaterialShader::Textured);
        assert_eq!(
            material.texture(TextureSlot::BaseColor),
            Some("sentinel-base")
        );
        assert_eq!(material.texture(TextureSlot::MetallicRoughness), None);
        assert_eq!(material.base_color_factor, [1.0, 0.9, 0.8, 1.0]);
        assert!((material.roughness - 0.6).abs() < f32::EPSILON);
        assert!((material.metallic).abs() < f32::EPSILON);
        assert!((material.team_tint - 1.0).abs() < f32::EPSILON);
        assert_eq!(material.wrap, None);
        assert_eq!(material.specular_tint, [1.0; 3]);
        let styled = MaterialDesc::parse(&format!(
            "{SAMPLE}wrap = 0.4\nrim = 2.0\nramp = 0.5\nspecular = 0.5\nspecular_tint = [1.0, 0.8, 0.6]\n"
        ))
        .expect("stylised");
        assert_eq!(styled.wrap, Some(0.4));
        assert_eq!((styled.rim, styled.ramp, styled.specular), (2.0, 0.5, 0.5));
        assert_eq!(styled.specular_tint, [1.0, 0.8, 0.6]);
        let slots: Vec<_> = material.bound_textures().map(|(slot, _)| slot).collect();
        assert_eq!(
            slots,
            [
                TextureSlot::BaseColor,
                TextureSlot::Normal,
                TextureSlot::Mask
            ]
        );
        assert_eq!(
            MaterialDesc::parse("shader = \"vertex-color\"").map(|m| m.shader),
            Ok(MaterialShader::VertexColor)
        );
    }

    #[test]
    fn rejects_bad_descriptors() {
        let invalid =
            |text: &str| matches!(MaterialDesc::parse(text), Err(MaterialError::Invalid(_)));
        assert!(invalid("shader = \"pbr\""));
        assert!(invalid("shader = \"textured\""), "needs a base colour");
        assert!(invalid("shader = \"vertex-color\"\nbase_color = \"x\""));
        assert!(invalid("shader = \"textured\"\nbase_color = \"../x\""));
        assert!(invalid("shader = \"textured\"\nbase_color = \"a.ktx2\""));
        assert!(invalid(
            "shader = \"textured\"\nbase_color = \"x\"\nmetallic = 1.5"
        ));
        assert!(invalid(
            "shader = \"textured\"\nbase_color = \"x\"\nroughness = nan"
        ));
        assert!(invalid(
            "shader = \"textured\"\nbase_color = \"x\"\nnormal_scale = 3.0"
        ));
        assert!(invalid(
            "shader = \"textured\"\nbase_color = \"x\"\nbase_color_factor = [1, 1, 1, 2]"
        ));
        assert!(invalid(
            "shader = \"textured\"\nbase_color = \"x\"\nemissive = [0, 0, 17]"
        ));
        for bad in [
            "wrap = 1.5",
            "rim = 5",
            "ramp = -0.1",
            "specular = inf",
            "specular_tint = [1, 2, 1]",
        ] {
            assert!(
                invalid(&format!("shader = \"textured\"\nbase_color = \"x\"\n{bad}")),
                "{bad}"
            );
        }
        assert!(matches!(
            MaterialDesc::parse("shader = \"textured\"\nbase_color = \"x\"\nalbedo = \"y\""),
            Err(MaterialError::Toml(_))
        ));
        assert!(matches!(
            MaterialDesc::parse("base_color = \"x\""),
            Err(MaterialError::Toml(_))
        ));
        let long = format!("shader = \"vertex-color\"\n#{}", "x".repeat(5000));
        assert_eq!(MaterialDesc::parse(&long), Err(MaterialError::TooLarge));
    }

    #[test]
    fn slots_fix_the_colour_encoding() {
        use TextureFormat as F;
        assert!(TextureSlot::BaseColor.accepts(F::Bc7Srgb));
        assert!(TextureSlot::BaseColor.accepts(F::Rgba8Srgb));
        assert!(
            !TextureSlot::BaseColor.accepts(F::Bc7Unorm),
            "colour must be sRGB"
        );
        assert!(TextureSlot::Normal.accepts(F::Bc5Unorm));
        assert!(
            !TextureSlot::Normal.accepts(F::Rgba8Srgb),
            "data must be linear"
        );
        assert!(TextureSlot::Mask.accepts(F::Bc4Unorm));
        assert!(!TextureSlot::Mask.accepts(F::Bc5Unorm));
        for (index, slot) in TextureSlot::ALL.iter().enumerate() {
            assert_eq!(slot.index(), index);
        }
    }

    #[test]
    fn paths_stay_under_the_asset_root() {
        let root = Path::new("assets");
        assert_eq!(
            material_path(root, "sentinel"),
            Some(PathBuf::from("assets/materials/sentinel.toml"))
        );
        assert_eq!(
            texture_path(root, "sentinel-base"),
            Some(PathBuf::from("assets/textures/sentinel-base.ktx2"))
        );
        for bad in ["", "../x", "a/b", "x.ktx2", "c:x"] {
            assert_eq!(material_path(root, bad), None, "{bad}");
            assert_eq!(texture_path(root, bad), None, "{bad}");
        }
    }

    #[test]
    fn parses_the_material_model() {
        let base = "shader = \"textured\"\nbase_color = \"x\"\n";
        let standard = MaterialDesc::parse(base).expect("standard");
        assert_eq!(standard.shading_model(), ShadingModel::Standard);
        assert!(
            (standard.dielectric_f0() - 0.04).abs() < 1e-6,
            "IOR 1.5 is F0 0.04"
        );
        assert_eq!(standard.transmission, None);
        let coat = MaterialDesc::parse(&format!(
            "{base}model = \"clearcoat\"\nior = 1.52\nspecular = 0.5\ndust = 0.25\n\
             emissive_texture = \"e\"\nmodel_texture = \"m\"\n\
             [clearcoat]\nweight = 0.5\nroughness = 0.1\n[compat]\nroughness = 0.3\n"
        ))
        .expect("clearcoat");
        assert_eq!(
            coat.model,
            ModelParams::Clearcoat {
                weight: 0.5,
                roughness: 0.1
            }
        );
        assert!((coat.dielectric_f0() - ior_to_f0(1.52) * 0.5).abs() < 1e-6);
        assert_eq!(coat.dust, 0.25);
        assert_eq!(coat.texture(TextureSlot::Emissive), Some("e"));
        assert_eq!(coat.texture(TextureSlot::Model), Some("m"));
        assert_eq!(coat.compat.roughness, Some(0.3));
        assert_eq!(coat.compat_fallback().roughness, 0.3, "[compat] wins");
        // Each model's table, with its defaults.
        for model in ShadingModel::ALL {
            let parsed = MaterialDesc::parse(&format!("{base}model = \"{}\"\n", model.name()))
                .unwrap_or_else(|_| panic!("{}", model.name()));
            assert_eq!(parsed.shading_model(), *model);
            assert_eq!(parsed.model, ModelParams::defaults(*model));
            assert_eq!(ShadingModel::from_name(model.name()), Some(*model));
        }
        let ids: Vec<u8> = ShadingModel::ALL.iter().map(|m| m.gbuffer_id()).collect();
        assert_eq!(ids, [0, 1, 2, 3, 4, 5, 6, 7], "ADR-0023 §6c's IDs");
        let glass = MaterialDesc::parse(&format!(
            "{base}model = \"transmissive\"\n[transmission]\nthickness = 0.2\n\
             attenuation_color = [0.9, 0.5, 0.2]\nattenuation_distance = 0.5\n"
        ))
        .expect("transmissive");
        let transmission = glass.transmission.expect("table");
        assert_eq!(transmission.transmission, 1.0);
        assert_eq!(transmission.attenuation_distance, Some(0.5));
        let film = MaterialDesc::parse(&format!(
            "{base}model = \"thin_film\"\n[thin_film]\nthickness_min = 150\n\
             thickness_max = 300\nior = 1.6\n"
        ))
        .expect("thin film");
        assert_eq!(
            film.model,
            ModelParams::ThinFilm {
                thickness_min: 150.0,
                thickness_max: 300.0,
                ior: 1.6
            }
        );
    }

    #[test]
    fn rejects_bad_model_keys() {
        let invalid =
            |text: &str| matches!(MaterialDesc::parse(text), Err(MaterialError::Invalid(_)));
        let base = "shader = \"textured\"\nbase_color = \"x\"\n";
        for bad in [
            "model = \"pbr\"",
            "model = \"sheen\"\n[clearcoat]\nweight = 1",
            "[clearcoat]\nweight = 1",
            "model = \"clearcoat\"\n[clearcoat]\nweight = 1.5",
            "model = \"sheen\"\n[sheen]\ntint = -1",
            "model = \"anisotropic\"\n[anisotropic]\nrotation = 4",
            "model = \"thin_film\"\n[thin_film]\nthickness_min = 50",
            "model = \"thin_film\"\n[thin_film]\nthickness_min = 300\nthickness_max = 200",
            "model = \"thin_film\"\n[thin_film]\nior = 2.5",
            "model = \"foliage\"\n[foliage]\ntranslucency = nan",
            "[transmission]\nthickness = 1",
            "model = \"transmissive\"\n[transmission]\nattenuation_distance = 0",
            "ior = 0.9",
            "ior = 2.6",
            "ior = 2.2\nspecular = 1.5",
            "dust = 2",
            "[compat]\nroughness = 1.5",
            "[compat]\nemissive = [0, 0, 20]",
        ] {
            assert!(invalid(&format!("{base}{bad}\n")), "{bad}");
        }
        assert!(matches!(
            MaterialDesc::parse(&format!("{base}[compat]\nwrap = 0.5\n")),
            Err(MaterialError::Toml(_))
        ));
        assert!(matches!(
            MaterialDesc::parse(&format!(
                "{base}model = \"clearcoat\"\n[clearcoat]\nfoo = 1\n"
            )),
            Err(MaterialError::Toml(_))
        ));
        assert!(invalid("shader = \"vertex-color\"\nmodel = \"standard\""));
        assert!(invalid(
            "shader = \"vertex-color\"\n[compat]\nroughness = 0.3"
        ));
        assert!(invalid(&format!("{base}emissive_texture = \"../e\"\n")));
    }

    #[test]
    fn every_model_collapses_for_compat() {
        let base = "shader = \"textured\"\nbase_color = \"x\"\nroughness = 0.4\n\
                    base_color_factor = [0.8, 0.6, 0.4, 1.0]\nemissive = [1, 2, 3]\n";
        let collapse = |extra: &str| {
            MaterialDesc::parse(&format!("{base}{extra}\n"))
                .expect(extra)
                .compat_fallback()
        };
        let standard = collapse("");
        assert_eq!(standard.base_color_factor, [0.8, 0.6, 0.4, 1.0]);
        assert_eq!(
            (standard.roughness, standard.specular, standard.rim),
            (0.4, 1.0, 1.0)
        );
        assert_eq!(standard.emissive, [1.0, 2.0, 3.0]);
        // Sheen, foliage and transmissive keep the base.
        for model in ["sheen", "foliage", "transmissive"] {
            assert_eq!(
                collapse(&format!("model = \"{model}\"")),
                standard,
                "{model}"
            );
        }
        let coat = collapse("model = \"clearcoat\"\n[clearcoat]\nweight = 1\nroughness = 0.1");
        assert!((coat.roughness - 0.25).abs() < 1e-6, "mix(0.4, 0.1, 0.5)");
        let aniso = collapse("model = \"anisotropic\"\n[anisotropic]\nstrength = 1");
        assert!((aniso.roughness - f32::midpoint(0.16, 1.0).sqrt()).abs() < 1e-6);
        assert!(aniso.roughness > 0.4, "anisotropy widens the mean lobe");
        let unlit = collapse("model = \"unlit\"");
        assert_eq!(unlit.base_color_factor, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!((unlit.specular, unlit.rim, unlit.metallic), (0.0, 0.0, 0.0));
        assert_eq!(
            unlit.emissive,
            [1.0, 2.0, 3.0],
            "UNLIT is its emissive alone"
        );
        let film = collapse("metallic = 1\nmodel = \"thin_film\"");
        assert_ne!(film.base_color_factor, standard.base_color_factor);
        assert!(
            film.base_color_factor[..3]
                .iter()
                .all(|channel| (0.0..=1.0).contains(channel))
        );
        let overridden = collapse(
            "model = \"unlit\"\n[compat]\nbase_color_factor = [0.1, 0.2, 0.3, 1]\nmetallic = 0.5\n\
             emissive = [0, 0, 0]",
        );
        assert_eq!(overridden.base_color_factor, [0.1, 0.2, 0.3, 1.0]);
        assert_eq!((overridden.metallic, overridden.emissive), (0.5, [0.0; 3]));
    }

    #[test]
    fn evaluator_rules_hold_for_both_paths() {
        let base = "shader = \"textured\"\nbase_color = \"x\"\n";
        let check = |extra: &str, mask: bool| {
            MaterialDesc::parse(&format!("{base}{extra}\n"))
                .expect(extra)
                .check_evaluators(mask)
        };
        // Every model with its defaults passes (unlit needs an emissive).
        for model in ShadingModel::ALL {
            let extra = if *model == ShadingModel::Unlit {
                format!("model = \"{}\"\nemissive = [1, 1, 1]", model.name())
            } else {
                format!("model = \"{}\"", model.name())
            };
            assert!(check(&extra, false).is_ok(), "{extra}");
        }
        // An unlit material that is black on Compat, or carries an unread model texture.
        assert!(check("model = \"unlit\"", false).is_err());
        assert!(
            check(
                "model = \"unlit\"\nemissive = [1, 0, 0]\n[compat]\nemissive = [0, 0, 0]",
                false
            )
            .is_err()
        );
        assert!(
            check(
                "model = \"unlit\"\nemissive = [1, 0, 0]\nmodel_texture = \"m\"",
                false
            )
            .is_err()
        );
        // An emissive texture needs the mask or a Compat emissive.
        let glow = "emissive_texture = \"e\"\nemissive = [2, 2, 2]";
        assert!(check(glow, false).is_err());
        assert!(check(glow, true).is_ok());
        assert!(
            check(
                &format!("{glow}\n[compat]\nemissive = [0.5, 0.5, 0.5]"),
                false
            )
            .is_ok()
        );
        assert!(
            check("emissive_texture = \"e\"", false).is_ok(),
            "no emissive factor"
        );
    }

    /// An `unlit` material must emit on both evaluators: a Compat emissive alone (from
    /// `[compat]`) leaves it black on Tiered, which draws its own `emissive`.
    #[test]
    fn unlit_emits_on_both_evaluators() {
        let base = "shader = \"textured\"\nbase_color = \"x\"\nmodel = \"unlit\"\n";
        let check = |extra: &str| {
            MaterialDesc::parse(&format!("{base}{extra}\n"))
                .expect(extra)
                .check_evaluators(false)
        };
        let error = check("[compat]\nemissive = [1, 1, 1]").expect_err("black on Tiered");
        assert!(error.to_string().contains("Tiered"), "{error}");
        let error = check("emissive = [1, 1, 1]\n[compat]\nemissive = [0, 0, 0]")
            .expect_err("black on Compat");
        assert!(error.to_string().contains("Compat"), "{error}");
        assert!(check("emissive = [1, 1, 1]").is_ok());
    }

    /// `ior` and `specular` shape only `standard`'s and `transmissive`'s F0: every other model's
    /// base is 0.04 on every path, so `content-check` refuses them there, and the Tiered F0
    /// (`model_dielectric_f0`) and the thin film's Compat tint use 0.04.
    #[test]
    fn ior_and_specular_apply_to_standard_and_transmissive_only() {
        let base = "shader = \"textured\"\nbase_color = \"x\"\n";
        let parse = |extra: &str| MaterialDesc::parse(&format!("{base}{extra}\n")).expect(extra);
        for model in ShadingModel::ALL {
            let name = model.name();
            let emissive = if *model == ShadingModel::Unlit {
                "\nemissive = [1, 1, 1]"
            } else {
                ""
            };
            let plain = parse(&format!("model = \"{name}\"{emissive}"));
            for key in ["ior = 1.8", "specular = 0.5"] {
                let desc = parse(&format!("model = \"{name}\"{emissive}\n{key}"));
                let takes = matches!(model, ShadingModel::Standard | ShadingModel::Transmissive);
                assert_eq!(desc.model_takes_ior(), takes, "{name}");
                assert_eq!(desc.check_evaluators(false).is_ok(), takes, "{name} {key}");
                if takes {
                    assert!((desc.model_dielectric_f0() - desc.dielectric_f0()).abs() < 1e-7);
                } else {
                    assert!((desc.model_dielectric_f0() - DEFAULT_DIELECTRIC_F0).abs() < 1e-7);
                }
            }
            assert!(
                (plain.dielectric_f0() - DEFAULT_DIELECTRIC_F0).abs() < 1e-6,
                "IOR 1.5's F0 is 0.04"
            );
        }
    }

    /// The thin film's Compat tint colours a metal's base colour (its F0) only: on a
    /// dielectric it is mixed out by `metallic`, so 0.04's ratio cannot clip the albedo white.
    #[test]
    fn thin_film_tints_metals_not_dielectrics() {
        let base = "shader = \"textured\"\nbase_color = \"x\"\n\
                    base_color_factor = [0.4, 0.4, 0.4, 1]\nmodel = \"thin_film\"\n";
        let dielectric = MaterialDesc::parse(&format!("{base}metallic = 0\n"))
            .expect("dielectric")
            .compat_fallback();
        assert_eq!(dielectric.base_color_factor, [0.4, 0.4, 0.4, 1.0]);
        let metal = MaterialDesc::parse(&format!("{base}metallic = 1\n"))
            .expect("metal")
            .compat_fallback();
        assert_ne!(metal.base_color_factor, [0.4, 0.4, 0.4, 1.0]);
        assert!(
            metal
                .base_color_factor
                .iter()
                .all(|&value| (0.0..=1.0).contains(&value))
        );
    }

    #[test]
    fn thin_film_tends_to_its_base() {
        // A vanishing film shows the base; a film over a dielectric base at normal incidence
        // stays a reflectance in 0..=1.
        let base = [0.9, 0.6, 0.3];
        let bare = thin_film_reflectance(1.0, 1.5, 0.0, base);
        for channel in 0..3 {
            assert!((bare[channel] - base[channel]).abs() < 0.02, "{bare:?}");
        }
        for thickness in [100.0, 200.0, 300.0, 400.0] {
            let film = thin_film_reflectance(1.0, 1.3, thickness, [0.04; 3]);
            assert!(
                film.iter().all(|value| (0.0..=1.0).contains(value)),
                "{thickness}: {film:?}"
            );
        }
        // Interference colours: the channels differ for a mid-range film.
        let film = thin_film_reflectance(1.0, 1.8, 300.0, [0.04; 3]);
        assert!((film[0] - film[2]).abs() > 0.01, "{film:?}");
    }

    #[test]
    fn the_emissive_mask_needs_a_third_channel() {
        assert!(!has_emissive_mask(TextureFormat::Bc5Unorm));
        assert!(!has_emissive_mask(TextureFormat::Rg8Unorm));
        assert!(has_emissive_mask(TextureFormat::Bc7Unorm));
        assert!(has_emissive_mask(TextureFormat::Rgba8Unorm));
        assert!(TextureSlot::MetallicRoughness.accepts(TextureFormat::Bc7Unorm));
        assert!(!TextureSlot::MetallicRoughness.accepts(TextureFormat::Bc7Srgb));
        assert!(TextureSlot::Emissive.accepts(TextureFormat::Bc7Srgb));
        assert!(!TextureSlot::Emissive.accepts(TextureFormat::Bc4Unorm));
        assert!(TextureSlot::Model.accepts(TextureFormat::Rgba8Unorm));
        assert!(!TextureSlot::Model.accepts(TextureFormat::Bc5Unorm));
        assert_eq!(TextureSlot::SET_SLOTS.len(), TextureSlot::SET_COUNT);
        assert_eq!(
            &TextureSlot::ALL[..TextureSlot::SET_COUNT],
            TextureSlot::SET_SLOTS
        );
    }
}
