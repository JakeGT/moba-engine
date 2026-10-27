//! Validated asset manifests, fallback resolution and (optionally skinned) meshes.
//!
//! Asset paths are relative, ASCII and must stay inside the pack. A missing named asset
//! resolves through its declared fallback chain; the renderer can then use a procedural
//! placeholder instead of failing a match.
//!
//! [`MeshData`] is the validated hand-off type between loaders and the renderer. The `glb`
//! feature adds [`load_glb`], a bounded binary-glTF loader for untrusted files (ADR-0007);
//! [`MeshSkin`]/[`Skeleton`] carry skinning data and [`Skeleton::sample`] poses it.

#![forbid(unsafe_code)]

#[cfg(feature = "glb")]
mod glb;
mod mesh;
mod skin;

use std::collections::BTreeMap;

use serde::Deserialize;

#[cfg(feature = "glb")]
pub use glb::{GlbError, MAX_GLB_BYTES, MAX_JSON_BYTES, load_glb, load_glb_file};
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
    /// Empty, non-ASCII, absolute or escaping path.
    BadPath,
    /// Duplicate asset name in one list.
    Duplicate,
    /// Fallback chain is missing, cyclic or too deep.
    BadFallback,
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
            Self::Asset(error) => write!(f, "asset manifest: {error:?}"),
        }
    }
}

impl std::error::Error for AssetManifestError {}

fn validate_path(path: &str) -> Result<(), AssetError> {
    if path.is_empty()
        || !path.is_ascii()
        || path.starts_with('/')
        || path.starts_with('\\')
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
path = "maps/blue.png"
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
}
