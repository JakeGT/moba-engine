//! Validated asset manifests, fallback resolution and (optionally skinned) meshes.
//!
//! Asset paths are relative, ASCII and must stay inside the pack. A missing named asset
//! resolves through its declared fallback chain; the renderer can then use a procedural
//! placeholder instead of failing a match.
//!
//! [`MeshData`] is the validated hand-off type between loaders and the renderer. The `glb`
//! feature adds [`load_glb`], a bounded binary-glTF loader for untrusted files (ADR-0007);
//! [`MeshSkin`]/[`Skeleton`] carry skinning data and [`Skeleton::sample`] poses it.
//! [`MapVisuals`] is the bounded per-map visuals descriptor (`maps/<name>.toml`, #287).

#![forbid(unsafe_code)]

#[cfg(feature = "glb")]
mod glb;
mod map_visuals;
mod mesh;
mod skin;

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

#[cfg(feature = "glb")]
pub use glb::{GlbError, MAX_GLB_BYTES, MAX_JSON_BYTES, load_glb, load_glb_file};
pub use map_visuals::{
    DEFAULT_PROP_HEIGHT, DEFAULT_WALL_HEIGHT, MAX_MAP_MESHES, MAX_MAP_PROPS, MAX_MAP_VISUALS_BYTES,
    MAX_MESH_NAME, MapProp, MapVisuals, MapVisualsError, valid_mesh_name,
};
pub use mesh::{MeshData, MeshError};
pub use skin::{
    AnimationClip, Interpolation, JointMatrix, JointTransform, MAX_CLIP_KEYS, MAX_CLIP_SECONDS,
    MAX_JOINTS, MAX_TRACK_KEYS, MeshSkin, Skeleton, SkinError, Track, TrackPath, quantize_weights,
};

/// One asset entry.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AssetEntry {
    /// Stable asset name.
    pub name: String,
    /// Relative asset path.
    pub path: String,
    /// Name of the fallback asset.
    #[serde(default)]
    pub fallback: Option<String>,
}

/// Asset manifest split by consumer.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AssetManifest {
    /// Unit meshes/rigs.
    #[serde(default)]
    pub units: Vec<AssetEntry>,
    /// Per-map visual assets.
    #[serde(default)]
    pub maps: Vec<AssetEntry>,
}

/// Manifest validation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetError {
    /// Empty or non-ASCII name.
    BadName,
    /// Empty, non-ASCII, absolute or escaping path, or one with `\`, `:` or control bytes.
    BadPath,
    /// Duplicate asset name in one list.
    Duplicate,
    /// Fallback chain is missing, cyclic or too deep.
    BadFallback,
}

impl core::fmt::Display for AssetError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::BadName => "asset names must be non-empty ASCII",
            Self::BadPath => {
                "asset paths must be relative ASCII without `.`/`..` components, `\\`, `:` or control bytes"
            }
            Self::Duplicate => "asset names must be unique within `units` and within `maps`",
            Self::BadFallback => {
                "asset fallback names a missing entry, or the chain is cyclic or deeper than 16"
            }
        })
    }
}

impl std::error::Error for AssetError {}

/// Which manifest list an entry belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetList {
    /// `[[units]]` (hero models).
    Units,
    /// `[[maps]]` (per-map visuals).
    Maps,
}

impl AssetList {
    /// The TOML table name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Units => "units",
            Self::Maps => "maps",
        }
    }
}

/// Why a manifest entry's file is unusable (see [`AssetManifest::check_files`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetFileProblem {
    /// A unit path does not end in `.glb`, the only mesh format the engine loads (ADR-0007).
    NotGlb,
    /// A map path does not end in `.toml` (a map visuals descriptor, #287).
    NotToml,
    /// The file does not exist or cannot be read; the text is the I/O error.
    Missing(String),
    /// The path names something other than a regular file.
    NotAFile,
    /// The path resolves (through a symlink) outside the pack directory.
    OutsideRoot,
    /// The file is not a mesh the bounded loader accepts; the text is the loader error.
    Malformed(String),
    /// The map visuals descriptor does not parse or validate; the text is the error.
    BadDescriptor(String),
    /// A mesh the map visuals descriptor names (`models/<mesh>.glb`) is unusable.
    Mesh {
        /// Mesh name from the descriptor.
        name: String,
        /// What is wrong with its file.
        problem: Box<AssetFileProblem>,
    },
}

impl core::fmt::Display for AssetFileProblem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotGlb => f.write_str("not a .glb file"),
            Self::NotToml => f.write_str("not a .toml map visuals descriptor"),
            Self::Missing(error) => write!(f, "missing or unreadable ({error})"),
            Self::NotAFile => f.write_str("not a regular file"),
            Self::OutsideRoot => f.write_str("resolves outside the pack directory"),
            Self::Malformed(error) => write!(f, "malformed mesh ({error})"),
            Self::BadDescriptor(error) => write!(f, "invalid descriptor ({error})"),
            Self::Mesh { name, problem } => {
                write!(f, "mesh '{name}' (models/{name}.glb): {problem}")
            }
        }
    }
}

/// One unusable manifest entry found by [`AssetManifest::check_files`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetFileIssue {
    /// List the entry is in.
    pub list: AssetList,
    /// Entry name.
    pub name: String,
    /// Entry path as written in the manifest.
    pub path: String,
    /// What is wrong with it.
    pub problem: AssetFileProblem,
}

impl core::fmt::Display for AssetFileIssue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "assets.{} '{}' ({}): {}",
            self.list.as_str(),
            self.name,
            self.path,
            self.problem
        )
    }
}

impl AssetManifest {
    /// Parses and validates a TOML manifest.
    ///
    /// # Errors
    /// Returns a TOML error or [`AssetError`].
    pub fn parse(text: &str) -> Result<Self, AssetManifestError> {
        let manifest: Self = toml::from_str(text).map_err(AssetManifestError::Toml)?;
        manifest.validate().map_err(AssetManifestError::Asset)?;
        Ok(manifest)
    }

    /// Validates names, paths, duplicates and fallback chains.
    ///
    /// # Errors
    /// Returns the first invalid entry or fallback chain.
    pub fn validate(&self) -> Result<(), AssetError> {
        for list in [&self.units, &self.maps] {
            let mut names = BTreeMap::new();
            for entry in list {
                if entry.name.is_empty() || !entry.name.is_ascii() {
                    return Err(AssetError::BadName);
                }
                if names.insert(entry.name.as_str(), entry).is_some() {
                    return Err(AssetError::Duplicate);
                }
                validate_path(&entry.path)?;
            }
            for entry in list {
                Self::walk_fallback(entry, &names)?;
            }
        }
        Ok(())
    }

    fn walk_fallback(
        entry: &AssetEntry,
        names: &BTreeMap<&str, &AssetEntry>,
    ) -> Result<(), AssetError> {
        let mut current = entry;
        for _ in 0..16 {
            let Some(fallback) = current.fallback.as_deref() else {
                return Ok(());
            };
            let Some(next) = names.get(fallback) else {
                return Err(AssetError::BadFallback);
            };
            if std::ptr::eq(*next, entry) {
                return Err(AssetError::BadFallback);
            }
            current = next;
        }
        Err(AssetError::BadFallback)
    }

    /// Total number of entries in both lists.
    #[must_use]
    pub fn len(&self) -> usize {
        self.units.len() + self.maps.len()
    }

    /// `true` when the manifest has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The `[[units]]` entry with this exact name.
    #[must_use]
    pub fn unit(&self, name: &str) -> Option<&AssetEntry> {
        self.units.iter().find(|entry| entry.name == name)
    }

    /// The `[[maps]]` entry with this exact name.
    #[must_use]
    pub fn map(&self, name: &str) -> Option<&AssetEntry> {
        self.maps.iter().find(|entry| entry.name == name)
    }

    /// Checks every entry's file under `root` (the pack directory), in manifest order.
    ///
    /// Every file must exist as a regular file and stay inside `root` once symlinks are
    /// resolved. Unit paths must end in `.glb`; with the `glb` feature they are also parsed
    /// with the bounded loader. Map paths must end in `.toml` and parse as a [`MapVisuals`]
    /// descriptor, and every mesh it names must pass the unit checks at
    /// `root/models/<mesh>.glb` (where a client resolves it). Paths are assumed to have
    /// passed [`AssetManifest::validate`]. This is an authoring check (`content-check`): the
    /// engine never fails a match on a missing asset.
    #[must_use]
    pub fn check_files(&self, root: &Path) -> Vec<AssetFileIssue> {
        let canonical_root = root.canonicalize().ok();
        let mut issues = Vec::new();
        for (list, entries) in [
            (AssetList::Units, &self.units),
            (AssetList::Maps, &self.maps),
        ] {
            for entry in entries {
                if let Err(problem) = check_file(list, root, canonical_root.as_deref(), &entry.path)
                {
                    issues.push(AssetFileIssue {
                        list,
                        name: entry.name.clone(),
                        path: entry.path.clone(),
                        problem,
                    });
                }
            }
        }
        issues
    }

    /// Resolves an asset through its fallback chain.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<&AssetEntry> {
        let list = if self.units.iter().any(|entry| entry.name == name) {
            &self.units
        } else if self.maps.iter().any(|entry| entry.name == name) {
            &self.maps
        } else {
            return None;
        };
        let mut current = list.iter().find(|entry| entry.name == name)?;
        for _ in 0..16 {
            let Some(fallback) = current.fallback.as_deref() else {
                return Some(current);
            };
            let next = list.iter().find(|entry| entry.name == fallback)?;
            if std::ptr::eq(next, current) {
                return None;
            }
            current = next;
        }
        None
    }
}

/// Combined parse/validation error.
#[derive(Debug)]
pub enum AssetManifestError {
    /// TOML syntax/schema error.
    Toml(toml::de::Error),
    /// Semantic validation error.
    Asset(AssetError),
}

impl core::fmt::Display for AssetManifestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Toml(error) => write!(f, "asset manifest TOML: {error}"),
            Self::Asset(error) => write!(f, "asset manifest: {error}"),
        }
    }
}

impl std::error::Error for AssetManifestError {}

fn check_file(
    list: AssetList,
    root: &Path,
    canonical_root: Option<&Path>,
    path: &str,
) -> Result<(), AssetFileProblem> {
    let (extension, wrong) = match list {
        AssetList::Units => ("glb", AssetFileProblem::NotGlb),
        AssetList::Maps => ("toml", AssetFileProblem::NotToml),
    };
    if !path
        .rsplit_once('.')
        .is_some_and(|(_, found)| found.eq_ignore_ascii_case(extension))
    {
        return Err(wrong);
    }
    let canonical = root
        .join(path)
        .canonicalize()
        .map_err(|error| AssetFileProblem::Missing(error.to_string()))?;
    if canonical_root.is_some_and(|root| !canonical.starts_with(root)) {
        return Err(AssetFileProblem::OutsideRoot);
    }
    let metadata = std::fs::metadata(&canonical)
        .map_err(|error| AssetFileProblem::Missing(error.to_string()))?;
    if !metadata.is_file() {
        return Err(AssetFileProblem::NotAFile);
    }
    if list == AssetList::Maps {
        let visuals = MapVisuals::load_file(&canonical)
            .map_err(|error| AssetFileProblem::BadDescriptor(error.to_string()))?;
        for name in visuals.mesh_names() {
            // `valid_mesh_name` already ruled out separators, dots and colons.
            let mesh_path = format!("models/{name}.glb");
            check_file(AssetList::Units, root, canonical_root, &mesh_path).map_err(|problem| {
                AssetFileProblem::Mesh {
                    name: name.to_owned(),
                    problem: Box::new(problem),
                }
            })?;
        }
        return Ok(());
    }
    #[cfg(feature = "glb")]
    load_glb_file(&canonical).map_err(|error| AssetFileProblem::Malformed(error.to_string()))?;
    Ok(())
}

/// Path rules: relative, ASCII, no `.`/`..` components, and no `\`, `:` (drive letters,
/// alternate data streams) or control bytes, so a path means the same on every OS and cannot
/// leave the pack directory.
fn validate_path(path: &str) -> Result<(), AssetError> {
    if path.is_empty()
        || !path.is_ascii()
        || path.starts_with('/')
        || path
            .bytes()
            .any(|byte| byte == b'\\' || byte == b':' || byte.is_ascii_control())
        || path.split('/').any(|part| part == ".." || part == ".")
    {
        return Err(AssetError::BadPath);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
[[units]]
name = "hero"
path = "units/hero.glb"
fallback = "hero_fallback"

[[units]]
name = "hero_fallback"
path = "units/fallback.glb"

[[maps]]
name = "blue"
path = "maps/blue.toml"
"#;

    #[test]
    fn parses_and_resolves_fallbacks() {
        let manifest = AssetManifest::parse(MANIFEST).expect("manifest");
        let resolved = manifest.resolve("hero").expect("hero");
        assert_eq!(resolved.name, "hero_fallback");
        assert!(resolved.fallback.is_none());
        assert!(manifest.resolve("missing").is_none());
    }

    #[test]
    fn rejects_unsafe_paths_and_cycles() {
        let unsafe_path = r#"
[[units]]
name = "x"
path = "../x.glb"
"#;
        assert!(matches!(
            AssetManifest::parse(unsafe_path),
            Err(AssetManifestError::Asset(AssetError::BadPath))
        ));
        let cycle = r#"
[[units]]
name = "a"
path = "a.glb"
fallback = "b"

[[units]]
name = "b"
path = "b.glb"
fallback = "a"
"#;
        assert!(matches!(
            AssetManifest::parse(cycle),
            Err(AssetManifestError::Asset(AssetError::BadFallback))
        ));
    }

    #[test]
    fn paths_reject_backslashes_drive_colons_and_control_bytes() {
        for bad in [
            "",
            "/abs.glb",
            "\\abs.glb",
            "models\\x.glb",
            "C:x.glb",
            "c:/x.glb",
            "x.glb:stream",
            "a/../x.glb",
            "./x.glb",
            "x\0.glb",
            "x\n.glb",
            "modèle.glb",
        ] {
            assert_eq!(validate_path(bad), Err(AssetError::BadPath), "{bad:?}");
        }
        for good in ["x.glb", "models/x.glb", "a/b/c-d_e.GLB"] {
            assert_eq!(validate_path(good), Ok(()), "{good:?}");
        }
    }

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "moba-assets-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("models/dir.glb")).expect("temp dir");
        root
    }

    #[test]
    fn check_files_reports_each_unusable_entry() {
        let root = temp_root("check");
        std::fs::write(root.join("models/ok.glb"), b"x").expect("write");
        std::fs::write(root.join("models/ok.png"), b"x").expect("write");
        let manifest = AssetManifest {
            units: vec![
                AssetEntry {
                    name: String::from("png"),
                    path: String::from("models/ok.png"),
                    fallback: None,
                },
                AssetEntry {
                    name: String::from("missing"),
                    path: String::from("models/missing.glb"),
                    fallback: None,
                },
                AssetEntry {
                    name: String::from("dir"),
                    path: String::from("models/dir.glb"),
                    fallback: None,
                },
            ],
            maps: ["ok", "glb", "bad", "nomesh"]
                .into_iter()
                .map(|name| AssetEntry {
                    name: String::from(name),
                    path: if name == "glb" {
                        String::from("models/ok.glb")
                    } else {
                        format!("maps/{name}.toml")
                    },
                    fallback: None,
                })
                .collect(),
        };
        std::fs::create_dir_all(root.join("maps")).expect("maps dir");
        let colors = "terrain = [0.1, 0.2, 0.3]\nwall = [0.4, 0.5, 0.6]\n";
        std::fs::write(root.join("maps/ok.toml"), colors).expect("write");
        std::fs::write(root.join("maps/bad.toml"), "terrain = [2, 0, 0]").expect("write");
        std::fs::write(
            root.join("maps/nomesh.toml"),
            format!("{colors}wall_mesh = \"gone\""),
        )
        .expect("write");
        assert_eq!(manifest.len(), 7);
        let issues = manifest.check_files(&root);
        let problems: Vec<(&str, &AssetFileProblem)> = issues
            .iter()
            .map(|issue| (issue.name.as_str(), &issue.problem))
            .collect();
        assert_eq!(problems.len(), 6, "{issues:?}");
        assert_eq!(problems[0], ("png", &AssetFileProblem::NotGlb));
        assert!(matches!(
            problems[1],
            ("missing", AssetFileProblem::Missing(_))
        ));
        assert_eq!(problems[2], ("dir", &AssetFileProblem::NotAFile));
        assert_eq!(problems[3], ("glb", &AssetFileProblem::NotToml));
        assert!(matches!(
            problems[4],
            ("bad", AssetFileProblem::BadDescriptor(_))
        ));
        assert!(matches!(
            problems[5],
            ("nomesh", AssetFileProblem::Mesh { name, problem })
                if name == "gone" && matches!(**problem, AssetFileProblem::Missing(_))
        ));
        assert!(
            issues[0]
                .to_string()
                .starts_with("assets.units 'png' (models/ok.png)")
        );
        assert!(issues[5].to_string().starts_with(
            "assets.maps 'nomesh' (maps/nomesh.toml): mesh 'gone' (models/gone.glb): missing"
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn check_files_rejects_symlinks_out_of_the_pack() {
        let root = temp_root("symlink");
        let outside = temp_root("symlink-target");
        std::fs::write(outside.join("secret.toml"), b"x").expect("write");
        std::fs::create_dir_all(root.join("maps")).expect("maps dir");
        std::os::unix::fs::symlink(outside.join("secret.toml"), root.join("maps/link.toml"))
            .expect("symlink");
        let manifest = AssetManifest {
            units: Vec::new(),
            maps: vec![AssetEntry {
                name: String::from("link"),
                path: String::from("maps/link.toml"),
                fallback: None,
            }],
        };
        let issues = manifest.check_files(&root);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].problem, AssetFileProblem::OutsideRoot);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }
}
