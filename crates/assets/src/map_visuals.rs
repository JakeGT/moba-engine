//! Per-map visuals descriptors (#287).
//!
//! A map's `visuals` name resolves, on each client, to a small TOML descriptor at
//! `<asset root>/maps/<name>.toml`:
//!
//! ```toml
//! terrain = [0.30, 0.11, 0.06]   # terrain tint, linear RGB in 0..=1
//! wall = [0.58, 0.22, 0.10]      # wall colour
//! wall_height = 6.0              # optional, metres (0.5..=32, default 4)
//! wall_mesh = "rampart"          # optional: <root>/models/rampart.glb along each wall
//!
//! [[props]]                      # optional decoration, at most 32
//! position = [96, 60]            # world units, like the pack's map coordinates
//! color = [1.0, 0.42, 0.12]
//! height = 5.0                   # optional, metres (0.1..=32, default 3)
//! yaw = 30.0                     # optional, degrees (-360..=360, default 0)
//! mesh = "crystal"               # optional: <root>/models/crystal.glb, else a block
//!
//! [lighting]                     # optional, every key optional (ADR-0011)
//! environment = "ember"          # <root>/environments/ember.{toml,ktx2} (IBL), else the
//!                                # hemispheric ambient below
//! sun_direction = [0.3, 0.6, 0.5]  # towards the sun, normalised on load (y >= 0.2)
//! sun_color = [1.0, 0.8, 0.6]    # linear RGB 0..=4, times sun_intensity
//! sun_intensity = 1.2            # 0..=8
//! ambient = [0.3, 0.2, 0.2]      # hemispheric sky ambient (no environment), 0..=4
//! ambient_ground = [0.2, 0.1, 0.1]
//! ibl_intensity = 1.0            # environment multiplier, 0..=8
//! shadow_strength = 0.8          # 0 = no shadows .. 0.9 (shadow always keeps some sun)
//! shadow_softness = 0.03         # sun angular radius in radians (PCSS penumbra), 0..=0.2
//! wrap = 0.25                    # wrap lighting, 0..=1
//! rim = 0.3                      # rim strength, 0..=2
//! rim_color = [1.0, 0.7, 0.5]    # 0..=4
//! ramp = "ember-ramp"            # <root>/textures/ember-ramp.ktx2: N.L ramp (sRGB colour)
//! ramp_strength = 0.5            # 0..=1
//! specular = 1.0                 # specular scale, 0..=2
//! specular_tint = [1.0, 0.9, 0.8]  # painted-specular tint, 0..=1
//! unit_roughness = 0.55          # roughness of vertex-colour units, 0..=1
//! terrain_roughness = 0.9        # 0..=1
//!
//! [post]                         # optional post-processing (R6), every key optional
//! exposure = 0.0                 # exposure compensation, EV stops, -4..=4 (default 0)
//! auto_exposure = true           # opt in to eye adaptation (default off)
//! exposure_min = -1.0            # auto-exposure EV clamp, -8..=8 (default -2)
//! exposure_max = 1.0             # -8..=8, >= exposure_min (default 2)
//! adaptation = 1.5               # adaptation rate per second, 0.1..=10 (default 1.5)
//! bloom = 0.05                   # bloom strength, 0..=0.25 (default 0.04)
//! grading = "ember"              # <root>/grading/ember.cube colour-grading LUT
//! grading_strength = 0.9         # 0..=1 blend towards the graded colour
//! ```
//!
//! Descriptors are untrusted, client-local and cosmetic: they never reach the simulation, the
//! wire or the content hash. [`MapVisuals::load_file`] reads at most
//! [`MAX_MAP_VISUALS_BYTES`], rejects unknown keys, and range-checks every number, so a
//! hostile file can only fail to load (the client then falls back to the built-in theme).

use std::io::Read;
use std::path::Path;

use serde::Deserialize;

/// Largest descriptor file accepted, in bytes.
pub const MAX_MAP_VISUALS_BYTES: u64 = 16 * 1024;
/// Most props one descriptor may place.
pub const MAX_MAP_PROPS: usize = 32;
/// Most distinct mesh names (wall and props together) one descriptor may reference.
pub const MAX_MAP_MESHES: usize = 4;
/// Longest accepted mesh name (`[A-Za-z0-9_-]`), matching hero model names.
pub const MAX_MESH_NAME: usize = 32;
/// Wall height when the descriptor gives none (the built-in themes' height), metres.
pub const DEFAULT_WALL_HEIGHT: f32 = 4.0;
/// Prop height when a prop gives none, metres.
pub const DEFAULT_PROP_HEIGHT: f32 = 3.0;
/// Roughness of vertex-colour units when `[lighting]` sets none (the renderer's default).
pub const DEFAULT_UNIT_ROUGHNESS: f32 = 0.55;
/// Terrain roughness when `[lighting]` sets none (the renderer's default).
pub const DEFAULT_TERRAIN_ROUGHNESS: f32 = 0.9;
/// Highest accepted `shadow_strength`: shadow never removes all direct light, so units in
/// shadow keep their team colour (ADR-0011 readability rule 2).
pub const MAX_SHADOW_STRENGTH: f32 = 0.9;
/// Lowest accepted sun elevation (`sun_direction.y` after normalising). Lower suns stretch
/// shadows past what one fitted map covers well.
pub const MIN_SUN_ELEVATION: f32 = 0.2;

/// Bloom strength when `[post]` sets none (the renderer's default).
pub const DEFAULT_BLOOM_STRENGTH: f32 = 0.04;
/// Eye-adaptation rate when `[post]` sets none, per second.
pub const DEFAULT_ADAPTATION: f32 = 1.5;
/// Lowest auto-exposure compensation when `[post]` sets no `exposure_min`, EV stops.
pub const DEFAULT_EXPOSURE_MIN: f32 = -2.0;
/// Highest auto-exposure compensation when `[post]` sets no `exposure_max`, EV stops.
pub const DEFAULT_EXPOSURE_MAX: f32 = 2.0;

const EXPOSURE_RANGE: (f32, f32) = (-4.0, 4.0);
const EXPOSURE_CLAMP_RANGE: (f32, f32) = (-8.0, 8.0);
const ADAPTATION_RANGE: (f32, f32) = (0.1, 10.0);
const MAX_BLOOM_STRENGTH: f32 = 0.25;

const WALL_HEIGHT_RANGE: (f32, f32) = (0.5, 32.0);
const PROP_HEIGHT_RANGE: (f32, f32) = (0.1, 32.0);
const YAW_RANGE: (f32, f32) = (-360.0, 360.0);
/// Prop positions are world units; anything a `u16` map extent can hold, with margin.
const POSITION_RANGE: (f32, f32) = (-65_536.0, 65_536.0);

/// A validated map visuals descriptor.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MapVisuals {
    /// Terrain tint, linear RGB in `0..=1`.
    pub terrain: [f32; 3],
    /// Wall colour, linear RGB in `0..=1`.
    pub wall: [f32; 3],
    /// Wall height in metres.
    pub wall_height: f32,
    /// Mesh drawn along each wall segment instead of the built-in block.
    pub wall_mesh: Option<String>,
    /// Decorative props, in file order.
    pub props: Vec<MapProp>,
    /// Lighting and art-style overrides (`[lighting]`); unset keys keep the engine defaults.
    pub lighting: MapLighting,
    /// Post-processing overrides (`[post]`); unset keys keep the engine defaults.
    pub post: MapPost,
}

/// The `[post]` table of a map visuals descriptor (R6 post-processing). Every field is
/// optional: `None` keeps the renderer's default for that parameter.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct MapPost {
    /// Exposure compensation in EV stops, `-4..=4` (default 0).
    pub exposure: Option<f32>,
    /// The map opts in to eye adaptation (auto exposure; default off).
    pub auto_exposure: Option<bool>,
    /// Lowest auto-exposure compensation in EV stops, `-8..=8` (default
    /// [`DEFAULT_EXPOSURE_MIN`]).
    pub exposure_min: Option<f32>,
    /// Highest auto-exposure compensation in EV stops, `-8..=8` and at least
    /// `exposure_min` (default [`DEFAULT_EXPOSURE_MAX`]).
    pub exposure_max: Option<f32>,
    /// Eye-adaptation rate per second, `0.1..=10` (default [`DEFAULT_ADAPTATION`]).
    pub adaptation: Option<f32>,
    /// Bloom strength, `0..=0.25` (default [`DEFAULT_BLOOM_STRENGTH`]).
    pub bloom: Option<f32>,
    /// Colour-grading LUT name: `<root>/grading/<name>.cube`
    /// ([`grading_path`](crate::grading_path)).
    pub grading: Option<String>,
    /// Blend from the ungraded to the graded colour, `0..=1`.
    pub grading_strength: Option<f32>,
}

/// The `[lighting]` table of a map visuals descriptor (ADR-0011). Every field is optional:
/// `None` keeps the renderer's default for that parameter.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct MapLighting {
    /// Environment name: `<root>/environments/<name>.{toml,ktx2}` (image-based lighting).
    pub environment: Option<String>,
    /// Unit vector towards the sun (`y >=` [`MIN_SUN_ELEVATION`]).
    pub sun_direction: Option<[f32; 3]>,
    /// Linear RGB sun colour, `0..=4`.
    pub sun_color: Option<[f32; 3]>,
    /// Sun intensity, `0..=8`.
    pub sun_intensity: Option<f32>,
    /// Hemispheric sky ambient (used without an environment), linear RGB `0..=4`.
    pub ambient: Option<[f32; 3]>,
    /// Hemispheric ground ambient, linear RGB `0..=4`.
    pub ambient_ground: Option<[f32; 3]>,
    /// Environment intensity, `0..=8`.
    pub ibl_intensity: Option<f32>,
    /// How much shadow removes direct light, `0..=1`.
    pub shadow_strength: Option<f32>,
    /// Sun angular radius in radians for soft (PCSS) shadows, `0..=0.2`.
    pub shadow_softness: Option<f32>,
    /// Wrap lighting, `0..=1`.
    pub wrap: Option<f32>,
    /// Rim strength, `0..=2`.
    pub rim: Option<f32>,
    /// Rim colour, linear RGB `0..=4`.
    pub rim_color: Option<[f32; 3]>,
    /// Ramp texture name: `<root>/textures/<name>.ktx2`.
    pub ramp: Option<String>,
    /// Ramp strength, `0..=1`.
    pub ramp_strength: Option<f32>,
    /// Specular scale, `0..=2`.
    pub specular: Option<f32>,
    /// Painted-specular tint, linear RGB `0..=1`.
    pub specular_tint: Option<[f32; 3]>,
    /// Roughness of vertex-colour units, `0..=1`.
    pub unit_roughness: Option<f32>,
    /// Terrain roughness, `0..=1`.
    pub terrain_roughness: Option<f32>,
}

/// One decorative prop.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MapProp {
    /// World position `[x, y]` (the pack's map coordinates).
    pub position: [f32; 2],
    /// Linear RGB in `0..=1`.
    pub color: [f32; 3],
    /// Height in metres; a mesh is scaled uniformly to it.
    pub height: f32,
    /// Rotation about the vertical axis, degrees.
    pub yaw: f32,
    /// Mesh name, or `None` for the built-in block.
    pub mesh: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMapVisuals {
    terrain: [f32; 3],
    wall: [f32; 3],
    #[serde(default)]
    wall_height: Option<f32>,
    #[serde(default)]
    wall_mesh: Option<String>,
    #[serde(default)]
    props: Vec<RawProp>,
    #[serde(default)]
    lighting: Option<RawLighting>,
    #[serde(default)]
    post: Option<RawPost>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPost {
    #[serde(default)]
    exposure: Option<f32>,
    #[serde(default)]
    auto_exposure: Option<bool>,
    #[serde(default)]
    exposure_min: Option<f32>,
    #[serde(default)]
    exposure_max: Option<f32>,
    #[serde(default)]
    adaptation: Option<f32>,
    #[serde(default)]
    bloom: Option<f32>,
    #[serde(default)]
    grading: Option<String>,
    #[serde(default)]
    grading_strength: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLighting {
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    sun_direction: Option<[f32; 3]>,
    #[serde(default)]
    sun_color: Option<[f32; 3]>,
    #[serde(default)]
    sun_intensity: Option<f32>,
    #[serde(default)]
    ambient: Option<[f32; 3]>,
    #[serde(default)]
    ambient_ground: Option<[f32; 3]>,
    #[serde(default)]
    ibl_intensity: Option<f32>,
    #[serde(default)]
    shadow_strength: Option<f32>,
    #[serde(default)]
    shadow_softness: Option<f32>,
    #[serde(default)]
    wrap: Option<f32>,
    #[serde(default)]
    rim: Option<f32>,
    #[serde(default)]
    rim_color: Option<[f32; 3]>,
    #[serde(default)]
    ramp: Option<String>,
    #[serde(default)]
    ramp_strength: Option<f32>,
    #[serde(default)]
    specular: Option<f32>,
    #[serde(default)]
    specular_tint: Option<[f32; 3]>,
    #[serde(default)]
    unit_roughness: Option<f32>,
    #[serde(default)]
    terrain_roughness: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProp {
    position: [f32; 2],
    color: [f32; 3],
    #[serde(default)]
    height: Option<f32>,
    #[serde(default)]
    yaw: Option<f32>,
    #[serde(default)]
    mesh: Option<String>,
}

/// Why a descriptor could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MapVisualsError {
    /// The file does not exist or cannot be read; the text is the I/O error.
    Io(String),
    /// The path is not a regular file.
    NotAFile,
    /// The file is larger than [`MAX_MAP_VISUALS_BYTES`].
    TooLarge,
    /// TOML syntax or schema error (unknown key, wrong type, missing field).
    Toml(String),
    /// A value is out of range; the text names it.
    Invalid(&'static str),
}

impl core::fmt::Display for MapVisualsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "missing or unreadable ({error})"),
            Self::NotAFile => f.write_str("not a regular file"),
            Self::TooLarge => write!(f, "larger than {MAX_MAP_VISUALS_BYTES} bytes"),
            Self::Toml(error) => write!(f, "map visuals TOML: {}", error.trim_end()),
            Self::Invalid(what) => write!(f, "map visuals: {what}"),
        }
    }
}

impl std::error::Error for MapVisualsError {}

/// `true` for 1..=[`MAX_MESH_NAME`] ASCII letters, digits, `_` or `-`: a name can never hold
/// a separator, dot or drive colon, so `models/<name>.glb` cannot leave the asset root.
#[must_use]
pub fn valid_mesh_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_MESH_NAME
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn check_color(color: [f32; 3], what: &'static str) -> Result<[f32; 3], MapVisualsError> {
    if color
        .iter()
        .all(|channel| channel.is_finite() && (0.0..=1.0).contains(channel))
    {
        Ok(color)
    } else {
        Err(MapVisualsError::Invalid(what))
    }
}

fn check_range(
    value: f32,
    (min, max): (f32, f32),
    what: &'static str,
) -> Result<f32, MapVisualsError> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(MapVisualsError::Invalid(what))
    }
}

fn check_rgb(
    color: Option<[f32; 3]>,
    high: f32,
    what: &'static str,
) -> Result<Option<[f32; 3]>, MapVisualsError> {
    match color {
        Some(rgb)
            if !rgb
                .iter()
                .all(|c| c.is_finite() && (0.0..=high).contains(c)) =>
        {
            Err(MapVisualsError::Invalid(what))
        }
        other => Ok(other),
    }
}

fn check_scalar(
    value: Option<f32>,
    high: f32,
    what: &'static str,
) -> Result<Option<f32>, MapVisualsError> {
    value
        .map(|value| check_range(value, (0.0, high), what))
        .transpose()
}

fn check_lighting(raw: Option<RawLighting>) -> Result<MapLighting, MapVisualsError> {
    let Some(raw) = raw else {
        return Ok(MapLighting::default());
    };
    let name = |name: Option<String>, what| match name {
        Some(name) if !valid_mesh_name(&name) => Err(MapVisualsError::Invalid(what)),
        other => Ok(other),
    };
    let sun_direction = match raw.sun_direction {
        None => None,
        Some(direction) => {
            let length = direction.iter().map(|c| c * c).sum::<f32>().sqrt();
            if !length.is_finite() || length < 1e-3 {
                return Err(MapVisualsError::Invalid("sun_direction must be non-zero"));
            }
            let unit = direction.map(|c| c / length);
            if unit[1] < MIN_SUN_ELEVATION {
                return Err(MapVisualsError::Invalid(
                    "sun_direction must be at least 0.2 above the horizon (normalised y >= 0.2)",
                ));
            }
            Some(unit)
        }
    };
    let lighting = MapLighting {
        environment: name(
            raw.environment,
            "environment names must be 1-32 of [A-Za-z0-9_-]",
        )?,
        sun_direction,
        sun_color: check_rgb(raw.sun_color, 4.0, "sun_color must be RGB in 0..=4")?,
        sun_intensity: check_scalar(raw.sun_intensity, 8.0, "sun_intensity must be 0..=8")?,
        ambient: check_rgb(raw.ambient, 4.0, "ambient must be RGB in 0..=4")?,
        ambient_ground: check_rgb(
            raw.ambient_ground,
            4.0,
            "ambient_ground must be RGB in 0..=4",
        )?,
        ibl_intensity: check_scalar(raw.ibl_intensity, 8.0, "ibl_intensity must be 0..=8")?,
        shadow_strength: check_scalar(
            raw.shadow_strength,
            MAX_SHADOW_STRENGTH,
            "shadow_strength must be 0..=0.9 (units in shadow keep some sun, ADR-0011 rule 2)",
        )?,
        shadow_softness: check_scalar(raw.shadow_softness, 0.2, "shadow_softness must be 0..=0.2")?,
        wrap: check_scalar(raw.wrap, 1.0, "wrap must be 0..=1")?,
        rim: check_scalar(raw.rim, 2.0, "rim must be 0..=2")?,
        // Rule 3: silhouettes keep a rim.
        rim_color: check_rgb(raw.rim_color, 4.0, "rim_color must be RGB in 0..=4")?,
        ramp: name(raw.ramp, "ramp texture names must be 1-32 of [A-Za-z0-9_-]")?,
        ramp_strength: check_scalar(raw.ramp_strength, 1.0, "ramp_strength must be 0..=1")?,
        specular: check_scalar(raw.specular, 2.0, "specular must be 0..=2")?,
        specular_tint: check_rgb(raw.specular_tint, 1.0, "specular_tint must be RGB in 0..=1")?,
        unit_roughness: check_scalar(raw.unit_roughness, 1.0, "unit_roughness must be 0..=1")?,
        terrain_roughness: check_scalar(
            raw.terrain_roughness,
            1.0,
            "terrain_roughness must be 0..=1",
        )?,
    };
    check_readability(&lighting)?;
    Ok(lighting)
}

fn check_post(raw: Option<RawPost>) -> Result<MapPost, MapVisualsError> {
    let Some(raw) = raw else {
        return Ok(MapPost::default());
    };
    let range = |value: Option<f32>, range, what| {
        value
            .map(|value| check_range(value, range, what))
            .transpose()
    };
    let post = MapPost {
        exposure: range(raw.exposure, EXPOSURE_RANGE, "exposure must be -4..=4 EV")?,
        auto_exposure: raw.auto_exposure,
        exposure_min: range(
            raw.exposure_min,
            EXPOSURE_CLAMP_RANGE,
            "exposure_min must be -8..=8 EV",
        )?,
        exposure_max: range(
            raw.exposure_max,
            EXPOSURE_CLAMP_RANGE,
            "exposure_max must be -8..=8 EV",
        )?,
        adaptation: range(
            raw.adaptation,
            ADAPTATION_RANGE,
            "adaptation must be 0.1..=10 per second",
        )?,
        bloom: check_scalar(raw.bloom, MAX_BLOOM_STRENGTH, "bloom must be 0..=0.25")?,
        grading: match raw.grading {
            Some(name) if !valid_mesh_name(&name) => {
                return Err(MapVisualsError::Invalid(
                    "grading LUT names must be 1-32 of [A-Za-z0-9_-]",
                ));
            }
            other => other,
        },
        grading_strength: check_scalar(
            raw.grading_strength,
            1.0,
            "grading_strength must be 0..=1",
        )?,
    };
    // An unset bound takes its default, so `exposure_min = 3` alone is as wrong as with an
    // explicit `exposure_max = 2`.
    if post.exposure_min.unwrap_or(DEFAULT_EXPOSURE_MIN)
        > post.exposure_max.unwrap_or(DEFAULT_EXPOSURE_MAX)
    {
        return Err(MapVisualsError::Invalid(
            "exposure_min must not exceed exposure_max (defaults -2 and 2)",
        ));
    }
    Ok(post)
}

/// ADR-0011 readability rules 2 and 3 that a descriptor alone can break.
fn check_readability(lighting: &MapLighting) -> Result<(), MapVisualsError> {
    // Rule 3: units keep a rim and stay glossier than the matte ground.
    if lighting.rim.is_some_and(|rim| rim <= 0.0) {
        return Err(MapVisualsError::Invalid(
            "rim must be above 0: units keep a silhouette rim (ADR-0011 rule 3)",
        ));
    }
    let unit = lighting.unit_roughness.unwrap_or(DEFAULT_UNIT_ROUGHNESS);
    let terrain = lighting
        .terrain_roughness
        .unwrap_or(DEFAULT_TERRAIN_ROUGHNESS);
    if terrain <= unit {
        return Err(MapVisualsError::Invalid(
            "terrain_roughness must exceed unit_roughness: units read glossier than the ground \
             (ADR-0011 rule 3)",
        ));
    }
    // Rule 2: something lights the shadowed side. Without an environment (or with it at
    // zero intensity) the hemispheric ambient is all there is.
    let no_ibl = lighting.environment.is_none() || lighting.ibl_intensity == Some(0.0);
    let dark = |rgb: Option<[f32; 3]>| rgb.is_some_and(|rgb| rgb.iter().all(|&c| c < 0.01));
    if no_ibl && dark(lighting.ambient) && lighting.ambient_ground.is_none_or(|g| dark(Some(g))) {
        return Err(MapVisualsError::Invalid(
            "no ambient light: with ambient at zero and no environment, units in shadow go \
             black (ADR-0011 rule 2)",
        ));
    }
    Ok(())
}

fn check_mesh(name: Option<String>) -> Result<Option<String>, MapVisualsError> {
    match name {
        Some(name) if !valid_mesh_name(&name) => Err(MapVisualsError::Invalid(
            "mesh names must be 1-32 of [A-Za-z0-9_-]",
        )),
        other => Ok(other),
    }
}

impl MapVisuals {
    /// Parses and validates a descriptor.
    ///
    /// # Errors
    /// Returns [`MapVisualsError::TooLarge`], a TOML error or the first out-of-range value.
    pub fn parse(text: &str) -> Result<Self, MapVisualsError> {
        if text.len() as u64 > MAX_MAP_VISUALS_BYTES {
            return Err(MapVisualsError::TooLarge);
        }
        let raw: RawMapVisuals =
            toml::from_str(text).map_err(|error| MapVisualsError::Toml(error.to_string()))?;
        if raw.props.len() > MAX_MAP_PROPS {
            return Err(MapVisualsError::Invalid("at most 32 props"));
        }
        let mut props = Vec::with_capacity(raw.props.len());
        for prop in raw.props {
            props.push(MapProp {
                position: [
                    check_range(
                        prop.position[0],
                        POSITION_RANGE,
                        "prop position out of range",
                    )?,
                    check_range(
                        prop.position[1],
                        POSITION_RANGE,
                        "prop position out of range",
                    )?,
                ],
                color: check_color(prop.color, "prop color must be RGB in 0..=1")?,
                height: check_range(
                    prop.height.unwrap_or(DEFAULT_PROP_HEIGHT),
                    PROP_HEIGHT_RANGE,
                    "prop height must be 0.1..=32",
                )?,
                yaw: check_range(
                    prop.yaw.unwrap_or(0.0),
                    YAW_RANGE,
                    "prop yaw must be -360..=360 degrees",
                )?,
                mesh: check_mesh(prop.mesh)?,
            });
        }
        let visuals = Self {
            terrain: check_color(raw.terrain, "terrain must be RGB in 0..=1")?,
            wall: check_color(raw.wall, "wall must be RGB in 0..=1")?,
            wall_height: check_range(
                raw.wall_height.unwrap_or(DEFAULT_WALL_HEIGHT),
                WALL_HEIGHT_RANGE,
                "wall_height must be 0.5..=32",
            )?,
            wall_mesh: check_mesh(raw.wall_mesh)?,
            props,
            lighting: check_lighting(raw.lighting)?,
            post: check_post(raw.post)?,
        };
        if visuals.mesh_names().count() > MAX_MAP_MESHES {
            return Err(MapVisualsError::Invalid(
                "at most 4 distinct meshes per descriptor",
            ));
        }
        Ok(visuals)
    }

    /// Reads and parses a descriptor file, reading at most [`MAX_MAP_VISUALS_BYTES`] + 1
    /// bytes whatever the file claims or grows to.
    ///
    /// # Errors
    /// Returns an I/O, size, TOML or range error.
    pub fn load_file(path: &Path) -> Result<Self, MapVisualsError> {
        let file =
            std::fs::File::open(path).map_err(|error| MapVisualsError::Io(error.to_string()))?;
        let metadata = file
            .metadata()
            .map_err(|error| MapVisualsError::Io(error.to_string()))?;
        if !metadata.is_file() {
            return Err(MapVisualsError::NotAFile);
        }
        if metadata.len() > MAX_MAP_VISUALS_BYTES {
            return Err(MapVisualsError::TooLarge);
        }
        let mut text = String::new();
        file.take(MAX_MAP_VISUALS_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|error| MapVisualsError::Io(error.to_string()))?;
        Self::parse(&text)
    }

    /// Distinct mesh names the descriptor references (wall first, then props in order).
    pub fn mesh_names(&self) -> impl Iterator<Item = &str> {
        let all = self
            .wall_mesh
            .as_deref()
            .into_iter()
            .chain(self.props.iter().filter_map(|prop| prop.mesh.as_deref()));
        let mut seen: Vec<&str> = Vec::new();
        all.filter(move |name| {
            if seen.contains(name) {
                false
            } else {
                seen.push(name);
                true
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMBER: &str = r#"
terrain = [0.30, 0.11, 0.06]
wall = [0.58, 0.22, 0.10]
wall_height = 6.0
wall_mesh = "rampart"

[[props]]
position = [96, 60]
color = [1.0, 0.42, 0.12]
height = 5.0
yaw = 30.0
mesh = "crystal"

[[props]]
position = [160.5, 196]
color = [0.9, 0.3, 0.1]
mesh = "crystal"

[[props]]
position = [128, 128]
color = [0.2, 0.2, 0.2]
"#;

    #[test]
    fn parses_defaults_and_distinct_meshes() {
        let visuals = MapVisuals::parse(EMBER).expect("descriptor");
        assert_eq!(visuals.terrain, [0.30, 0.11, 0.06]);
        assert_eq!(visuals.wall_height, 6.0);
        assert_eq!(visuals.props.len(), 3);
        assert_eq!(visuals.props[1].height, DEFAULT_PROP_HEIGHT);
        assert_eq!(visuals.props[1].position, [160.5, 196.0]);
        assert_eq!(visuals.props[2].mesh, None);
        assert_eq!(
            visuals.mesh_names().collect::<Vec<_>>(),
            ["rampart", "crystal"]
        );
        let minimal = MapVisuals::parse("terrain = [0, 0, 0]\nwall = [1, 1, 1]").expect("min");
        assert_eq!(minimal.wall_height, DEFAULT_WALL_HEIGHT);
        assert!(minimal.props.is_empty() && minimal.wall_mesh.is_none());
        assert_eq!(minimal.lighting, MapLighting::default());
        assert_eq!(minimal.post, MapPost::default());
    }

    #[test]
    fn lighting_table_is_optional_validated_and_normalised() {
        let base = "terrain = [0.1, 0.1, 0.1]\nwall = [0.2, 0.2, 0.2]\n[lighting]\n";
        let visuals = MapVisuals::parse(&format!(
            "{base}environment = \"ember\"\nsun_direction = [0, 2, 0]\nsun_color = [1, 0.5, 0.25]\n\
             shadow_strength = 0.7\nwrap = 0.3\nramp = \"ember-ramp\"\nramp_strength = 0.5\n"
        ))
        .expect("lighting");
        let lighting = &visuals.lighting;
        assert_eq!(lighting.environment.as_deref(), Some("ember"));
        assert_eq!(lighting.sun_direction, Some([0.0, 1.0, 0.0]));
        assert_eq!(lighting.sun_color, Some([1.0, 0.5, 0.25]));
        assert_eq!(lighting.shadow_strength, Some(0.7));
        assert_eq!(lighting.ramp.as_deref(), Some("ember-ramp"));
        assert_eq!(lighting.sun_intensity, None);
        // Dark ambient is fine with an environment to light the shadowed side.
        assert!(
            MapVisuals::parse(&format!("{base}environment = \"e\"\nambient = [0, 0, 0]\n")).is_ok()
        );
        for bad in [
            "environment = \"../sky\"",
            "sun_direction = [0, 0, 0]",
            "sun_direction = [1, 0, 0]",
            "sun_direction = [1, 0.1, 0]",
            "shadow_strength = 0.95",
            "rim = 0",
            "unit_roughness = 0.95",
            "terrain_roughness = 0.5",
            "unit_roughness = 0.6\nterrain_roughness = 0.6",
            "ambient = [0, 0, 0]",
            "sun_direction = [nan, 1, 0]",
            "sun_color = [5, 0, 0]",
            "sun_intensity = 9",
            "shadow_strength = 1.5",
            "shadow_softness = 0.5",
            "wrap = -0.1",
            "rim_color = [1, 1]",
            "ramp = \"a/b\"",
            "specular_tint = [1, 1, 2]",
            "unit_roughness = inf",
            "exposure = 1",
        ] {
            assert!(MapVisuals::parse(&format!("{base}{bad}")).is_err(), "{bad}");
        }
    }

    #[test]
    fn post_table_is_optional_and_validated() {
        let base = "terrain = [0.1, 0.1, 0.1]\nwall = [0.2, 0.2, 0.2]\n[post]\n";
        let visuals = MapVisuals::parse(&format!(
            "{base}exposure = -0.5\nauto_exposure = true\nexposure_min = -1\nexposure_max = 1.5\n\
             adaptation = 2\nbloom = 0.05\ngrading = \"ember\"\ngrading_strength = 0.9\n"
        ))
        .expect("post");
        let post = &visuals.post;
        assert_eq!(post.exposure, Some(-0.5));
        assert_eq!(post.auto_exposure, Some(true));
        assert_eq!(
            (post.exposure_min, post.exposure_max),
            (Some(-1.0), Some(1.5))
        );
        assert_eq!(post.adaptation, Some(2.0));
        assert_eq!(post.bloom, Some(0.05));
        assert_eq!(post.grading.as_deref(), Some("ember"));
        assert_eq!(post.grading_strength, Some(0.9));
        assert_eq!(visuals.lighting, MapLighting::default());
        // An empty table is all defaults; boundary values are accepted.
        let empty = MapVisuals::parse(base).expect("empty post");
        assert_eq!(empty.post, MapPost::default());
        for ok in [
            "exposure = 4",
            "exposure = -4",
            "exposure_min = -8\nexposure_max = 8",
            "exposure_min = 1\nexposure_max = 1",
            "exposure_max = -2",
            "adaptation = 0.1",
            "adaptation = 10",
            "bloom = 0",
            "bloom = 0.25",
            "grading_strength = 0",
            "auto_exposure = false",
        ] {
            assert!(MapVisuals::parse(&format!("{base}{ok}")).is_ok(), "{ok}");
        }
        for bad in [
            "exposure = 4.5",
            "exposure = -5",
            "exposure = nan",
            "auto_exposure = 1",
            "exposure_min = -9",
            "exposure_max = 8.5",
            "exposure_min = 1\nexposure_max = 0.5",
            "exposure_min = 3",
            "exposure_max = -3",
            "adaptation = 0",
            "adaptation = 11",
            "bloom = -0.01",
            "bloom = 0.3",
            "bloom = inf",
            "grading = \"../ember\"",
            "grading = \"ember.cube\"",
            "grading = \"\"",
            "grading_strength = 1.1",
            "grading_strength = -0.1",
            "vignette = 0.5",
            "sun_intensity = 1",
        ] {
            assert!(MapVisuals::parse(&format!("{base}{bad}")).is_err(), "{bad}");
        }
    }

    #[test]
    fn rejects_hostile_or_out_of_range_input() {
        let base = "terrain = [0.1, 0.1, 0.1]\nwall = [0.2, 0.2, 0.2]\n";
        let prop =
            |body: &str| format!("{base}[[props]]\nposition = [1, 1]\ncolor = [1, 1, 1]\n{body}");
        for bad in [
            String::new(),
            String::from("terrain = [0.1, 0.1]\nwall = [0.2, 0.2, 0.2]"),
            format!("{base}extra = 1"),
            String::from("terrain = [1.5, 0, 0]\nwall = [0, 0, 0]"),
            String::from("terrain = [nan, 0, 0]\nwall = [0, 0, 0]"),
            String::from("terrain = [0, 0, 0]\nwall = [-0.1, 0, 0]"),
            format!("{base}wall_height = 0.1"),
            format!("{base}wall_height = inf"),
            format!("{base}wall_mesh = \"../escape\""),
            format!("{base}wall_mesh = \"\""),
            format!("{base}wall_mesh = \"a.glb\""),
            format!("{base}wall_mesh = \"{}\"", "x".repeat(33)),
            prop("height = 40"),
            prop("yaw = 1e9"),
            prop("mesh = \"c:x\""),
            prop("tint = 1"),
            format!("{base}[[props]]\nposition = [1e30, 0]\ncolor = [1, 1, 1]"),
            format!(
                "{base}{}",
                "[[props]]\nposition = [1, 1]\ncolor = [1, 1, 1]\n".repeat(33)
            ),
            format!(
                "{base}{}",
                ["m0", "m1", "m2", "m3", "m4"]
                    .map(|mesh| {
                        String::from("[[props]]\nposition = [1, 1]\ncolor = [1, 1, 1]\nmesh = \"")
                            + mesh
                            + "\"\n"
                    })
                    .concat()
            ),
            format!("{base}#{}", "x".repeat(17 * 1024)),
        ] {
            assert!(MapVisuals::parse(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn load_file_bounds_size_and_kind() {
        let root = std::env::temp_dir().join(format!(
            "moba-map-visuals-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("dir.toml")).expect("temp dir");
        std::fs::write(root.join("ok.toml"), EMBER).expect("write");
        std::fs::write(root.join("big.toml"), vec![b'#'; 20 * 1024]).expect("write");
        std::fs::write(root.join("binary.toml"), [0xff, 0xfe, 0x00]).expect("write");
        assert!(MapVisuals::load_file(&root.join("ok.toml")).is_ok());
        assert_eq!(
            MapVisuals::load_file(&root.join("big.toml")),
            Err(MapVisualsError::TooLarge)
        );
        assert!(matches!(
            MapVisuals::load_file(&root.join("missing.toml")),
            Err(MapVisualsError::Io(_))
        ));
        assert!(MapVisuals::load_file(&root.join("binary.toml")).is_err());
        assert!(MapVisuals::load_file(&root.join("dir.toml")).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
