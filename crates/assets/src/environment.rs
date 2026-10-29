//! Per-map lighting environments for image-based lighting (R2a).
//!
//! A map visuals descriptor names an environment; on each client it resolves to two files
//! under `<asset root>/environments/` ([`environment_paths`]):
//!
//! - `<name>.ktx2`: the prefiltered specular cube map (`RGBA16F`, a full mip chain; level
//!   `m` of `M` is prefiltered for GGX roughness `m / (M - 1)`), loaded with
//!   [`load_ktx2_file`](crate::load_ktx2_file);
//! - `<name>.toml`: this descriptor, the diffuse ambient as real spherical harmonics (L2,
//!   nine RGB coefficients) plus an intensity and the cube map's mip count:
//!
//! ```toml
//! sh = [[0.31, 0.36, 0.42], ...]   # nine RGB rows, order below
//! intensity = 1.0                  # optional, 0..=16 (default 1)
//! specular_mips = 7                # optional, 1..=11 (default 7)
//! ```
//!
//! **SH convention.** World space, +Y up, `n = (x, y, z)` normalised; basis order and
//! constants `Y0 = 0.282095`, `Y1 = 0.488603 y`, `Y2 = 0.488603 z`, `Y3 = 0.488603 x`,
//! `Y4 = 1.092548 xy`, `Y5 = 1.092548 yz`, `Y6 = 0.315392 (3z² - 1)`, `Y7 = 1.092548 xz`,
//! `Y8 = 0.546274 (x² - y²)` ([`sh_basis`]). The stored coefficients are already convolved
//! with the clamped cosine *and divided by π*, so the diffuse ambient of an albedo `a` is
//! `a * max(0, Σ sh[i] Y_i(n))` ([`EnvironmentDesc::irradiance_over_pi`]). The
//! `moba-tools env-bake` baker writes both files from an analytic sky.
//!
//! Like map visuals, environments are untrusted, client-local and cosmetic: they never reach
//! the simulation, the wire or the content hash. [`EnvironmentDesc::load_file`] reads at most
//! [`MAX_ENVIRONMENT_BYTES`], rejects unknown keys and range-checks every number.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::map_visuals::valid_mesh_name;

/// Directory under the asset root that holds environments.
pub const ENVIRONMENTS_DIR: &str = "environments";
/// Largest descriptor file accepted, in bytes.
pub const MAX_ENVIRONMENT_BYTES: u64 = 4096;
/// Spherical-harmonics coefficients per colour channel (bands 0..=2).
pub const SH_COEFFICIENTS: usize = 9;
/// Most specular mip levels a descriptor may declare (a 1024² cube's full chain).
pub const MAX_SPECULAR_MIPS: u32 = 11;
/// Default `specular_mips` (a 64² cube's full chain).
const DEFAULT_SPECULAR_MIPS: u32 = 7;
/// Largest accepted |coefficient|.
const MAX_SH_MAGNITUDE: f32 = 64.0;
/// Largest accepted intensity.
const MAX_INTENSITY: f32 = 16.0;

/// The real SH-L2 basis at the normalised direction `n` (see the module docs for the order).
#[must_use]
pub fn sh_basis(n: [f32; 3]) -> [f32; SH_COEFFICIENTS] {
    let [x, y, z] = n;
    [
        0.282_095,
        0.488_603 * y,
        0.488_603 * z,
        0.488_603 * x,
        1.092_548 * x * y,
        1.092_548 * y * z,
        0.315_392 * (3.0 * z * z - 1.0),
        1.092_548 * x * z,
        0.546_274 * (x * x - y * y),
    ]
}

/// A validated environment descriptor.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct EnvironmentDesc {
    /// Cosine-convolved diffuse SH-L2 divided by π, linear RGB per coefficient.
    pub sh: [[f32; 3]; SH_COEFFICIENTS],
    /// Multiplier for both the diffuse SH and the specular cube map.
    pub intensity: f32,
    /// Mip levels of the specular cube map (`roughness = level / (mips - 1)`).
    pub specular_mips: u32,
}

impl EnvironmentDesc {
    /// A descriptor with these coefficients, intensity 1 and the default mip count.
    #[must_use]
    pub const fn new(sh: [[f32; 3]; SH_COEFFICIENTS]) -> Self {
        Self {
            sh,
            intensity: 1.0,
            specular_mips: DEFAULT_SPECULAR_MIPS,
        }
    }

    /// Returns this descriptor with [`Self::intensity`] set.
    #[must_use]
    pub const fn with_intensity(mut self, intensity: f32) -> Self {
        self.intensity = intensity;
        self
    }

    /// Returns this descriptor with [`Self::specular_mips`] set.
    #[must_use]
    pub const fn with_specular_mips(mut self, specular_mips: u32) -> Self {
        self.specular_mips = specular_mips;
        self
    }

    /// `max(0, Σ sh[i] Y_i(n))` per channel (before [`Self::intensity`]): the diffuse
    /// ambient of a white surface facing `n` (normalised).
    #[must_use]
    pub fn irradiance_over_pi(&self, n: [f32; 3]) -> [f32; 3] {
        let basis = sh_basis(n);
        let mut out = [0.0f32; 3];
        for (coefficient, weight) in self.sh.iter().zip(basis) {
            for channel in 0..3 {
                out[channel] += coefficient[channel] * weight;
            }
        }
        out.map(|value| value.max(0.0))
    }

    /// Serialises the descriptor as TOML (the keys [`Self::parse`] reads), after `header`
    /// lines that are written as `#` comments.
    #[must_use]
    pub fn to_toml(&self, header: &[String]) -> String {
        use core::fmt::Write as _;
        let mut text = String::new();
        for line in header {
            let _ = writeln!(text, "# {line}");
        }
        text.push_str("sh = [\n");
        for row in &self.sh {
            let _ = writeln!(text, "    [{:?}, {:?}, {:?}],", row[0], row[1], row[2]);
        }
        text.push_str("]\n");
        let _ = writeln!(text, "intensity = {:?}", self.intensity);
        let _ = writeln!(text, "specular_mips = {}", self.specular_mips);
        text
    }

    /// Parses and validates a descriptor.
    ///
    /// # Errors
    /// [`EnvironmentError::TooLarge`], a TOML error or the first out-of-range value.
    pub fn parse(text: &str) -> Result<Self, EnvironmentError> {
        if text.len() as u64 > MAX_ENVIRONMENT_BYTES {
            return Err(EnvironmentError::TooLarge);
        }
        let raw: RawEnvironment =
            toml::from_str(text).map_err(|error| EnvironmentError::Toml(error.to_string()))?;
        if raw
            .sh
            .iter()
            .flatten()
            .any(|value| !value.is_finite() || value.abs() > MAX_SH_MAGNITUDE)
        {
            return Err(EnvironmentError::Invalid(
                "sh coefficients must be finite with |value| <= 64",
            ));
        }
        let intensity = raw.intensity.unwrap_or(1.0);
        if !intensity.is_finite() || !(0.0..=MAX_INTENSITY).contains(&intensity) {
            return Err(EnvironmentError::Invalid("intensity must be 0..=16"));
        }
        let specular_mips = raw.specular_mips.unwrap_or(DEFAULT_SPECULAR_MIPS);
        if !(1..=MAX_SPECULAR_MIPS).contains(&specular_mips) {
            return Err(EnvironmentError::Invalid("specular_mips must be 1..=11"));
        }
        Ok(Self {
            sh: raw.sh,
            intensity,
            specular_mips,
        })
    }

    /// Reads and parses a descriptor file, reading at most [`MAX_ENVIRONMENT_BYTES`] + 1
    /// bytes whatever the file claims or grows to.
    ///
    /// # Errors
    /// An I/O, size, TOML or range error.
    pub fn load_file(path: &Path) -> Result<Self, EnvironmentError> {
        let file =
            std::fs::File::open(path).map_err(|error| EnvironmentError::Io(error.to_string()))?;
        let metadata = file
            .metadata()
            .map_err(|error| EnvironmentError::Io(error.to_string()))?;
        if !metadata.is_file() {
            return Err(EnvironmentError::NotAFile);
        }
        if metadata.len() > MAX_ENVIRONMENT_BYTES {
            return Err(EnvironmentError::TooLarge);
        }
        let mut text = String::new();
        file.take(MAX_ENVIRONMENT_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|error| EnvironmentError::Io(error.to_string()))?;
        Self::parse(&text)
    }
}

/// `<root>/environments/<name>.toml` and `<root>/environments/<name>.ktx2`, or `None` for a
/// name that is not 1-32 of `[A-Za-z0-9_-]` (so it can never leave the directory).
#[must_use]
pub fn environment_paths(root: &Path, name: &str) -> Option<(PathBuf, PathBuf)> {
    valid_mesh_name(name).then(|| {
        let dir = root.join(ENVIRONMENTS_DIR);
        (
            dir.join(format!("{name}.toml")),
            dir.join(format!("{name}.ktx2")),
        )
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnvironment {
    sh: [[f32; 3]; SH_COEFFICIENTS],
    #[serde(default)]
    intensity: Option<f32>,
    #[serde(default)]
    specular_mips: Option<u32>,
}

/// Why an environment descriptor could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EnvironmentError {
    /// The file does not exist or cannot be read; the text is the I/O error.
    Io(String),
    /// The path is not a regular file.
    NotAFile,
    /// The file is larger than [`MAX_ENVIRONMENT_BYTES`].
    TooLarge,
    /// TOML syntax or schema error (unknown key, wrong type, missing field).
    Toml(String),
    /// A value is out of range; the text names it.
    Invalid(&'static str),
}

impl core::fmt::Display for EnvironmentError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "missing or unreadable ({error})"),
            Self::NotAFile => f.write_str("not a regular file"),
            Self::TooLarge => write!(f, "larger than {MAX_ENVIRONMENT_BYTES} bytes"),
            Self::Toml(error) => write!(f, "environment TOML: {}", error.trim_end()),
            Self::Invalid(what) => write!(f, "environment: {what}"),
        }
    }
}

impl std::error::Error for EnvironmentError {}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIFORM: &str = "sh = [[3.5449, 3.5449, 3.5449], [0, 0, 0], [0, 0, 0], [0, 0, 0], \
                           [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0]]\n";

    #[test]
    fn parses_defaults_and_evaluates_the_basis() {
        let desc = EnvironmentDesc::parse(UNIFORM).expect("descriptor");
        assert_eq!(desc.intensity, 1.0);
        assert_eq!(desc.specular_mips, DEFAULT_SPECULAR_MIPS);
        for n in [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]] {
            let value = desc.irradiance_over_pi(n);
            assert!((value[0] - 1.0).abs() < 1e-3, "{value:?}");
        }
        let full = format!("{UNIFORM}intensity = 2.5\nspecular_mips = 9\n# comment\n");
        let desc = EnvironmentDesc::parse(&full).expect("descriptor");
        assert_eq!((desc.intensity, desc.specular_mips), (2.5, 9));
        let round = EnvironmentDesc::parse(&desc.to_toml(&[String::from("header")]));
        assert_eq!(round, Ok(desc));
        let basis = sh_basis([0.0, 0.0, 1.0]);
        assert!((basis[6] - 0.630_784).abs() < 1e-6 && basis[2] > 0.48);
    }

    #[test]
    fn rejects_hostile_or_out_of_range_input() {
        let rows = |value: &str| format!("sh = [{}]\n", [value; 9].join(", "));
        for bad in [
            String::new(),
            String::from("sh = [[0, 0, 0]]"),
            rows("[0, 0]"),
            rows("[nan, 0, 0]"),
            rows("[inf, 0, 0]"),
            rows("[65, 0, 0]"),
            format!("{UNIFORM}intensity = -1"),
            format!("{UNIFORM}intensity = 17"),
            format!("{UNIFORM}intensity = nan"),
            format!("{UNIFORM}specular_mips = 0"),
            format!("{UNIFORM}specular_mips = 12"),
            format!("{UNIFORM}extra = 1"),
            format!("{UNIFORM}[sky]\nzenith = 1"),
            format!("{UNIFORM}#{}", "x".repeat(5000)),
        ] {
            assert!(EnvironmentDesc::parse(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn paths_stay_inside_the_environments_directory() {
        let root = Path::new("assets");
        let (toml, ktx2) = environment_paths(root, "meadow").expect("valid name");
        assert_eq!(toml, root.join("environments").join("meadow.toml"));
        assert_eq!(ktx2, root.join("environments").join("meadow.ktx2"));
        for bad in ["", "../x", "a/b", "a.b", "c:x"] {
            assert_eq!(environment_paths(root, bad), None, "{bad}");
        }
    }

    #[test]
    fn load_file_bounds_size_and_kind() {
        let root = std::env::temp_dir().join(format!(
            "moba-environment-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("dir.toml")).expect("temp dir");
        std::fs::write(root.join("ok.toml"), UNIFORM).expect("write");
        std::fs::write(root.join("big.toml"), vec![b'#'; 5000]).expect("write");
        assert!(EnvironmentDesc::load_file(&root.join("ok.toml")).is_ok());
        assert_eq!(
            EnvironmentDesc::load_file(&root.join("big.toml")),
            Err(EnvironmentError::TooLarge)
        );
        assert!(matches!(
            EnvironmentDesc::load_file(&root.join("missing.toml")),
            Err(EnvironmentError::Io(_))
        ));
        assert!(EnvironmentDesc::load_file(&root.join("dir.toml")).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
