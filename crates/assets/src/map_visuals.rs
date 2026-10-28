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

const WALL_HEIGHT_RANGE: (f32, f32) = (0.5, 32.0);
const PROP_HEIGHT_RANGE: (f32, f32) = (0.1, 32.0);
const YAW_RANGE: (f32, f32) = (-360.0, 360.0);
/// Prop positions are world units; anything a `u16` map extent can hold, with margin.
const POSITION_RANGE: (f32, f32) = (-65_536.0, 65_536.0);

/// A validated map visuals descriptor.
#[derive(Clone, Debug, PartialEq)]
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
}

/// One decorative prop.
#[derive(Clone, Debug, PartialEq)]
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
