//! Baked ambient occlusion for a map's static occluders (R2b, ADR-0016).
//!
//! A map visuals descriptor's `[lighting] baked_ao = "<name>"` resolves, on each client, to
//! two files under `<asset root>/maps/ao/` ([`baked_ao_paths`]):
//!
//! - `<name>.ktx2`: a square `R8_UNORM` occlusion map (1 = open) covering the map's
//!   `[0, extent]²` world square; texel `(u, v)` is world `x = (u + 0.5) · extent / texels`,
//!   `y = (v + 0.5) · extent / texels` (row 0 is the `y = 0` edge);
//! - `<name>.toml`: this sidecar:
//!
//! ```toml
//! extent = 256.0                       # world units the map covers (1..=65535)
//! texels = 512                         # width = height of the texture (1..=2048)
//! height = 3.0                         # optional falloff above the ground, metres (0.1..=64)
//! fingerprint = "0x1234abcd5678ef90"   # ao_fingerprint of the inputs it was baked from
//! ```
//!
//! The `moba-tools ao-bake` baker writes both. The client recomputes [`ao_fingerprint`] for
//! the walls and props it actually bound and ignores a bake whose fingerprint differs, so a
//! visuals name reused by a pack with other walls never gets wrong occlusion. Like the rest
//! of the map visuals, bakes are untrusted, client-local and cosmetic: they never reach the
//! simulation, the wire or the content hash.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::map_visuals::{MapProp, valid_mesh_name};
use crate::mesh::MeshData;

/// Directory under the asset root that holds baked AO (`maps/ao`).
pub const BAKED_AO_DIR: &str = "maps/ao";
/// Largest sidecar accepted, in bytes.
pub const MAX_BAKED_AO_BYTES: u64 = 2048;
/// Largest baked AO texture side, texels (2048² R8 is 4 MiB).
pub const MAX_BAKED_AO_TEXELS: u32 = 2048;
/// Default falloff height above the ground, metres.
pub const DEFAULT_BAKED_AO_HEIGHT: f32 = 3.0;
const MAX_EXTENT: f32 = 65_535.0;
const HEIGHT_RANGE: (f32, f32) = (0.1, 64.0);

/// `<root>/maps/ao/<name>.toml` and `<root>/maps/ao/<name>.ktx2`, or `None` for a name that
/// is not 1-32 of `[A-Za-z0-9_-]` (so it can never leave the directory).
#[must_use]
pub fn baked_ao_paths(root: &Path, name: &str) -> Option<(PathBuf, PathBuf)> {
    valid_mesh_name(name).then(|| {
        let dir = root.join(BAKED_AO_DIR);
        (
            dir.join(format!("{name}.toml")),
            dir.join(format!("{name}.ktx2")),
        )
    })
}

/// The shape of a prop's mesh as far as a bake is concerned ([`ao_fingerprint`]): the mesh's
/// model-space bounds and its vertex and index counts, or `None` when the prop draws the
/// built-in block (no mesh, or one that failed to load or is degenerate).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct PropMeshShape {
    /// Model-space minimum corner.
    pub min: [f32; 3],
    /// Model-space maximum corner.
    pub max: [f32; 3],
    /// Vertex count.
    pub vertices: u32,
    /// Index count.
    pub indices: u32,
}

impl PropMeshShape {
    /// The shape of `mesh`, or `None` if its extent is unusable (not finite, flat or empty);
    /// the client and the baker then both use the built-in block.
    #[must_use]
    pub fn of(mesh: &MeshData) -> Option<Self> {
        let (min, max) = mesh.bounds();
        let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        let usable = size.iter().all(|value| value.is_finite())
            && size[1] > 1e-4
            && size.iter().copied().fold(0.0f32, f32::max) > 1e-4;
        usable.then(|| Self {
            min,
            max,
            vertices: u32::try_from(mesh.positions().len()).unwrap_or(u32::MAX),
            indices: u32::try_from(mesh.indices().len()).unwrap_or(u32::MAX),
        })
    }
}

/// FNV-1a (64-bit) over the inputs a bake depends on: the map extent, every wall segment
/// (`[ax, ay, bx, by]` world units, in order), the wall height and each prop's position,
/// height, yaw, mesh name and mesh shape (`meshes[i]` for `props[i]`: bounds and vertex and
/// index counts, so editing the model behind a name invalidates the bake; a missing entry is
/// the block). Colours are not included (they do not occlude).
#[must_use]
pub fn ao_fingerprint(
    extent: f32,
    walls: &[[f32; 4]],
    wall_height: f32,
    props: &[MapProp],
    meshes: &[Option<PropMeshShape>],
) -> u64 {
    let mut hash = Fnv(0xcbf2_9ce4_8422_2325);
    hash.float(extent);
    hash.bytes(&(walls.len() as u64).to_le_bytes());
    for wall in walls {
        for value in wall {
            hash.float(*value);
        }
    }
    hash.float(wall_height);
    hash.bytes(&(props.len() as u64).to_le_bytes());
    for (index, prop) in props.iter().enumerate() {
        hash.float(prop.position[0]);
        hash.float(prop.position[1]);
        hash.float(prop.height);
        hash.float(prop.yaw);
        let mesh = prop.mesh.as_deref().unwrap_or("");
        hash.bytes(&(mesh.len() as u64).to_le_bytes());
        hash.bytes(mesh.as_bytes());
        match meshes.get(index).copied().flatten() {
            Some(shape) => {
                hash.bytes(&[1]);
                for value in shape.min.into_iter().chain(shape.max) {
                    hash.float(value);
                }
                hash.bytes(&shape.vertices.to_le_bytes());
                hash.bytes(&shape.indices.to_le_bytes());
            }
            None => hash.bytes(&[0]),
        }
    }
    hash.0
}

/// 64-bit FNV-1a.
struct Fnv(u64);

impl Fnv {
    fn bytes(&mut self, data: &[u8]) {
        for byte in data {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    /// `-0.0` and `0.0` hash alike.
    fn float(&mut self, value: f32) {
        let value = if value == 0.0 { 0.0 } else { value };
        self.bytes(&value.to_bits().to_le_bytes());
    }
}

/// A validated baked AO sidecar.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct BakedAoDesc {
    /// World units the texture covers (`[0, extent]²`).
    pub extent: f32,
    /// Texture width and height, texels.
    pub texels: u32,
    /// Falloff height above the ground, metres.
    pub height: f32,
    /// [`ao_fingerprint`] of the inputs the bake was made from.
    pub fingerprint: u64,
}

impl BakedAoDesc {
    /// A sidecar for an `extent`-unit map baked at `texels`² with this fingerprint and the
    /// default falloff height.
    #[must_use]
    pub const fn new(extent: f32, texels: u32, fingerprint: u64) -> Self {
        Self {
            extent,
            texels,
            height: DEFAULT_BAKED_AO_HEIGHT,
            fingerprint,
        }
    }

    /// Returns this sidecar with [`Self::height`] set.
    #[must_use]
    pub const fn with_height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    /// Serialises the sidecar as TOML (the keys [`Self::parse`] reads), after `header` lines
    /// that are written as `#` comments.
    #[must_use]
    pub fn to_toml(&self, header: &[String]) -> String {
        use core::fmt::Write as _;
        let mut text = String::new();
        for line in header {
            let _ = writeln!(text, "# {line}");
        }
        let _ = writeln!(text, "extent = {:?}", self.extent);
        let _ = writeln!(text, "texels = {}", self.texels);
        let _ = writeln!(text, "height = {:?}", self.height);
        let _ = writeln!(text, "fingerprint = \"{:#018x}\"", self.fingerprint);
        text
    }

    /// Parses and validates a sidecar.
    ///
    /// # Errors
    /// [`BakedAoError::TooLarge`], a TOML error or the first out-of-range value.
    pub fn parse(text: &str) -> Result<Self, BakedAoError> {
        if text.len() as u64 > MAX_BAKED_AO_BYTES {
            return Err(BakedAoError::TooLarge);
        }
        let raw: RawBakedAo =
            toml::from_str(text).map_err(|error| BakedAoError::Toml(error.to_string()))?;
        if !raw.extent.is_finite() || !(1.0..=MAX_EXTENT).contains(&raw.extent) {
            return Err(BakedAoError::Invalid("extent must be 1..=65535"));
        }
        if !(1..=MAX_BAKED_AO_TEXELS).contains(&raw.texels) {
            return Err(BakedAoError::Invalid("texels must be 1..=2048"));
        }
        let height = raw.height.unwrap_or(DEFAULT_BAKED_AO_HEIGHT);
        if !height.is_finite() || !(HEIGHT_RANGE.0..=HEIGHT_RANGE.1).contains(&height) {
            return Err(BakedAoError::Invalid("height must be 0.1..=64 metres"));
        }
        let digits = raw
            .fingerprint
            .strip_prefix("0x")
            .filter(|digits| (1..=16).contains(&digits.len()))
            .ok_or(BakedAoError::Invalid(
                "fingerprint must be \"0x\" + 1-16 hex digits",
            ))?;
        let fingerprint = u64::from_str_radix(digits, 16)
            .map_err(|_| BakedAoError::Invalid("fingerprint must be \"0x\" + 1-16 hex digits"))?;
        Ok(Self {
            extent: raw.extent,
            texels: raw.texels,
            height,
            fingerprint,
        })
    }

    /// Reads and parses a sidecar, reading at most [`MAX_BAKED_AO_BYTES`] + 1 bytes whatever
    /// the file claims or grows to.
    ///
    /// # Errors
    /// An I/O, size, TOML or range error.
    pub fn load_file(path: &Path) -> Result<Self, BakedAoError> {
        let file =
            std::fs::File::open(path).map_err(|error| BakedAoError::Io(error.to_string()))?;
        let metadata = file
            .metadata()
            .map_err(|error| BakedAoError::Io(error.to_string()))?;
        if !metadata.is_file() {
            return Err(BakedAoError::NotAFile);
        }
        if metadata.len() > MAX_BAKED_AO_BYTES {
            return Err(BakedAoError::TooLarge);
        }
        let mut text = String::new();
        file.take(MAX_BAKED_AO_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|error| BakedAoError::Io(error.to_string()))?;
        Self::parse(&text)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBakedAo {
    extent: f32,
    texels: u32,
    #[serde(default)]
    height: Option<f32>,
    fingerprint: String,
}

/// Why a baked AO sidecar could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BakedAoError {
    /// The file does not exist or cannot be read; the text is the I/O error.
    Io(String),
    /// The path is not a regular file.
    NotAFile,
    /// The file is larger than [`MAX_BAKED_AO_BYTES`].
    TooLarge,
    /// TOML syntax or schema error (unknown key, wrong type, missing field).
    Toml(String),
    /// A value is out of range; the text names it.
    Invalid(&'static str),
}

impl core::fmt::Display for BakedAoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "missing or unreadable ({error})"),
            Self::NotAFile => f.write_str("not a regular file"),
            Self::TooLarge => write!(f, "larger than {MAX_BAKED_AO_BYTES} bytes"),
            Self::Toml(error) => write!(f, "baked AO TOML: {}", error.trim_end()),
            Self::Invalid(what) => write!(f, "baked AO: {what}"),
        }
    }
}

impl std::error::Error for BakedAoError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MapVisuals;

    fn props() -> Vec<MapProp> {
        MapVisuals::parse(
            "terrain = [0.1, 0.1, 0.1]\nwall = [0.2, 0.2, 0.2]\n\
             [[props]]\nposition = [10, 20]\ncolor = [1, 0, 0]\nheight = 5\nmesh = \"crystal\"\n",
        )
        .expect("visuals")
        .props
    }

    #[test]
    fn sidecar_round_trips_and_validates() {
        let desc = BakedAoDesc::new(256.0, 512, 0x0123_4567_89ab_cdef).with_height(2.5);
        let text = desc.to_toml(&[String::from("baked by a test")]);
        assert!(text.starts_with("# baked by a test\n"));
        assert!(text.contains("fingerprint = \"0x0123456789abcdef\""));
        assert_eq!(BakedAoDesc::parse(&text), Ok(desc));
        let minimal =
            BakedAoDesc::parse("extent = 64\ntexels = 128\nfingerprint = \"0xff\"").expect("ok");
        assert_eq!(minimal.height, DEFAULT_BAKED_AO_HEIGHT);
        assert_eq!(minimal.fingerprint, 0xff);
        for bad in [
            "extent = 0\ntexels = 128\nfingerprint = \"0x1\"",
            "extent = nan\ntexels = 128\nfingerprint = \"0x1\"",
            "extent = 64\ntexels = 0\nfingerprint = \"0x1\"",
            "extent = 64\ntexels = 4096\nfingerprint = \"0x1\"",
            "extent = 64\ntexels = 128\nheight = 0\nfingerprint = \"0x1\"",
            "extent = 64\ntexels = 128\nfingerprint = \"1234\"",
            "extent = 64\ntexels = 128\nfingerprint = \"0x\"",
            "extent = 64\ntexels = 128\nfingerprint = \"0x11112222333344445\"",
            "extent = 64\ntexels = 128\nfingerprint = \"0xzz\"",
            "extent = 64\ntexels = 128\nfingerprint = \"0x1\"\nextra = 1",
            "extent = 64\ntexels = 128",
        ] {
            assert!(BakedAoDesc::parse(bad).is_err(), "accepted {bad:?}");
        }
        let huge = format!(
            "# {}\nextent = 64\ntexels = 1\nfingerprint = \"0x1\"",
            "x".repeat(4096)
        );
        assert_eq!(BakedAoDesc::parse(&huge), Err(BakedAoError::TooLarge));
    }

    #[test]
    fn fingerprint_tracks_occluders_not_colours() {
        let walls = [[0.0, 0.0, 10.0, 0.0], [5.0, 5.0, 5.0, 20.0]];
        let props = props();
        let base = ao_fingerprint(64.0, &walls, 4.0, &props, &[]);
        assert_eq!(base, ao_fingerprint(64.0, &walls, 4.0, &props, &[]));
        assert_ne!(base, ao_fingerprint(65.0, &walls, 4.0, &props, &[]));
        assert_ne!(base, ao_fingerprint(64.0, &walls[..1], 4.0, &props, &[]));
        assert_ne!(base, ao_fingerprint(64.0, &walls, 6.0, &props, &[]));
        assert_ne!(base, ao_fingerprint(64.0, &walls, 4.0, &[], &[]));
        let mut moved = props.clone();
        moved[0].position[0] += 1.0;
        assert_ne!(base, ao_fingerprint(64.0, &walls, 4.0, &moved, &[]));
        let mut recoloured = props.clone();
        recoloured[0].color = [0.0, 1.0, 0.0];
        assert_eq!(base, ao_fingerprint(64.0, &walls, 4.0, &recoloured, &[]));
        let mut swapped = walls;
        swapped.swap(0, 1);
        assert_ne!(base, ao_fingerprint(64.0, &swapped, 4.0, &props, &[]));
        // The mesh behind a prop's name is part of it: other bounds or counts, other bake.
        let shape = PropMeshShape {
            min: [-0.5, 0.0, -0.5],
            max: [0.5, 2.0, 0.5],
            vertices: 24,
            indices: 36,
        };
        let meshed = ao_fingerprint(64.0, &walls, 4.0, &props, &[Some(shape)]);
        assert_ne!(base, meshed);
        let taller = PropMeshShape {
            max: [0.5, 3.0, 0.5],
            ..shape
        };
        assert_ne!(
            meshed,
            ao_fingerprint(64.0, &walls, 4.0, &props, &[Some(taller)])
        );
        let denser = PropMeshShape {
            vertices: 25,
            ..shape
        };
        assert_ne!(
            meshed,
            ao_fingerprint(64.0, &walls, 4.0, &props, &[Some(denser)])
        );
        assert_eq!(base, ao_fingerprint(64.0, &walls, 4.0, &props, &[None]));
    }

    #[test]
    fn paths_stay_in_the_ao_directory() {
        let root = Path::new("assets");
        let (toml, ktx2) = baked_ao_paths(root, "map-grass-ao").expect("valid");
        assert_eq!(toml, PathBuf::from("assets/maps/ao/map-grass-ao.toml"));
        assert_eq!(ktx2, PathBuf::from("assets/maps/ao/map-grass-ao.ktx2"));
        assert!(baked_ao_paths(root, "../x").is_none());
        assert!(baked_ao_paths(root, "").is_none());
    }

    #[test]
    fn load_file_bounds_size_and_kind() {
        let dir = std::env::temp_dir().join(format!("moba-baked-ao-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        assert_eq!(BakedAoDesc::load_file(&dir), Err(BakedAoError::NotAFile));
        let big = dir.join("big.toml");
        std::fs::write(&big, vec![b'#'; 4096]).expect("write");
        assert_eq!(BakedAoDesc::load_file(&big), Err(BakedAoError::TooLarge));
        assert!(matches!(
            BakedAoDesc::load_file(&dir.join("missing.toml")),
            Err(BakedAoError::Io(_))
        ));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
