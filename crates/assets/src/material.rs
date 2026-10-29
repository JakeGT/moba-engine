//! Client-local material descriptors (ADR-0010).
//!
//! A model named `sentinel` may have a material at `<asset root>/materials/sentinel.toml`;
//! without one it keeps the vertex-colour look. A material is a shader permutation, a
//! parameter block and a texture set:
//!
//! ```toml
//! shader = "textured"                  # "vertex-color" (the default look) or "textured"
//! base_color = "sentinel-base"         # <root>/textures/sentinel-base.ktx2, sRGB colour
//! normal = "sentinel-normal"           # optional, tangent-space XY, linear 2-channel
//! metallic_roughness = "sentinel-mr"   # optional, R metallic / G roughness, linear 2-channel
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
//! ```
//!
//! Like map visuals (#287), descriptors are untrusted, client-local and cosmetic: they never
//! reach the simulation, the wire or the content hash. [`MaterialDesc::load_file`] reads at
//! most [`MAX_MATERIAL_BYTES`], rejects unknown keys, range-checks every number and accepts
//! only texture names that cannot leave `<root>/textures/`. Each texture slot also fixes the
//! formats it accepts ([`TextureSlot::accepts`]), so sRGB and linear data cannot be swapped.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::map_visuals::valid_mesh_name;
use crate::texture::TextureFormat;

/// Largest material descriptor accepted, in bytes.
pub const MAX_MATERIAL_BYTES: u64 = 4 * 1024;

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

/// A texture binding of a material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum TextureSlot {
    /// sRGB base colour (`bc7-srgb` or `rgba8-srgb`).
    BaseColor,
    /// Tangent-space normal XY, linear (`bc5` or `rg8`).
    Normal,
    /// Metallic (R) and roughness (G), linear (`bc5` or `rg8`).
    MetallicRoughness,
    /// Team-colour mask, linear (`bc4` or `r8`).
    Mask,
}

impl TextureSlot {
    /// Every slot, in binding order (shader bindings 1..=4 of the material set).
    pub const ALL: &'static [Self] = &[
        Self::BaseColor,
        Self::Normal,
        Self::MetallicRoughness,
        Self::Mask,
    ];

    /// Number of slots.
    pub const COUNT: usize = 4;

    /// Position in [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::BaseColor => 0,
            Self::Normal => 1,
            Self::MetallicRoughness => 2,
            Self::Mask => 3,
        }
    }

    /// Descriptor key (`"base_color"`, `"normal"`, `"metallic_roughness"`, `"mask"`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BaseColor => "base_color",
            Self::Normal => "normal",
            Self::MetallicRoughness => "metallic_roughness",
            Self::Mask => "mask",
        }
    }

    /// Whether a texture in `format` may be bound here: colour must be sRGB, data linear,
    /// with the slot's channel count.
    #[must_use]
    pub const fn accepts(self, format: TextureFormat) -> bool {
        match self {
            Self::BaseColor => matches!(format, TextureFormat::Bc7Srgb | TextureFormat::Rgba8Srgb),
            Self::Normal | Self::MetallicRoughness => {
                matches!(format, TextureFormat::Bc5Unorm | TextureFormat::Rg8Unorm)
            }
            Self::Mask => matches!(format, TextureFormat::Bc4Unorm | TextureFormat::R8Unorm),
        }
    }
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
    /// Specular scale, `0..=2`.
    pub specular: f32,
    /// Painted-specular tint, linear RGB in `0..=1`.
    pub specular_tint: [f32; 3],
}

impl Default for MaterialDesc {
    fn default() -> Self {
        Self {
            shader: MaterialShader::VertexColor,
            textures: [None, None, None, None],
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

impl MaterialDesc {
    /// Parses and validates a descriptor.
    ///
    /// # Errors
    /// [`MaterialError::Toml`] for syntax/schema errors, [`MaterialError::Invalid`] for an
    /// unknown shader, a bad texture name, an out-of-range number, a textured material
    /// without a base colour or a vertex-colour material with textures.
    pub fn parse(text: &str) -> Result<Self, MaterialError> {
        if text.len() as u64 > MAX_MATERIAL_BYTES {
            return Err(MaterialError::TooLarge);
        }
        let raw: RawMaterial =
            toml::from_str(text).map_err(|error| MaterialError::Toml(error.to_string()))?;
        let shader = MaterialShader::from_name(&raw.shader).ok_or(MaterialError::Invalid(
            "shader must be \"vertex-color\" or \"textured\"",
        ))?;
        let textures = [raw.base_color, raw.normal, raw.metallic_roughness, raw.mask];
        if textures.iter().flatten().any(|name| !valid_mesh_name(name)) {
            return Err(MaterialError::Invalid(
                "texture names are 1-32 of [A-Za-z0-9_-]",
            ));
        }
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
            _ => {}
        }
        let defaults = Self::default();
        let base_color_factor = raw.base_color_factor.unwrap_or(defaults.base_color_factor);
        if !base_color_factor
            .iter()
            .all(|&value| in_range(value, 0.0, 1.0))
        {
            return Err(MaterialError::Invalid("base_color_factor must be in 0..=1"));
        }
        let unit = |value: Option<f32>, default: f32, what| match value {
            None => Ok(default),
            Some(value) if in_range(value, 0.0, 1.0) => Ok(value),
            Some(_) => Err(MaterialError::Invalid(what)),
        };
        let metallic = unit(raw.metallic, defaults.metallic, "metallic must be in 0..=1")?;
        let roughness = unit(
            raw.roughness,
            defaults.roughness,
            "roughness must be in 0..=1",
        )?;
        let team_tint = unit(
            raw.team_tint,
            defaults.team_tint,
            "team_tint must be in 0..=1",
        )?;
        let normal_scale = match raw.normal_scale {
            None => defaults.normal_scale,
            Some(value) if in_range(value, 0.0, 2.0) => value,
            Some(_) => return Err(MaterialError::Invalid("normal_scale must be in 0..=2")),
        };
        let emissive = raw.emissive.unwrap_or(defaults.emissive);
        if !emissive.iter().all(|&value| in_range(value, 0.0, 16.0)) {
            return Err(MaterialError::Invalid("emissive must be in 0..=16"));
        }
        let ranged = |value: Option<f32>, default: f32, high: f32, what| match value {
            None => Ok(default),
            Some(value) if in_range(value, 0.0, high) => Ok(value),
            Some(_) => Err(MaterialError::Invalid(what)),
        };
        let wrap = match raw.wrap {
            None => None,
            Some(value) if in_range(value, 0.0, 1.0) => Some(value),
            Some(_) => return Err(MaterialError::Invalid("wrap must be in 0..=1")),
        };
        let rim = ranged(raw.rim, defaults.rim, 4.0, "rim must be in 0..=4")?;
        let ramp = ranged(raw.ramp, defaults.ramp, 1.0, "ramp must be in 0..=1")?;
        let specular = ranged(
            raw.specular,
            defaults.specular,
            2.0,
            "specular must be in 0..=2",
        )?;
        let specular_tint = raw.specular_tint.unwrap_or(defaults.specular_tint);
        if !specular_tint.iter().all(|&value| in_range(value, 0.0, 1.0)) {
            return Err(MaterialError::Invalid("specular_tint must be in 0..=1"));
        }
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
}
