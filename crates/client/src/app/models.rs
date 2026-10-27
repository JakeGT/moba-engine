//! Client-local hero models with procedural fallback (WP-3.2, ADR-0007).
//!
//! Network clients never see pack files, only the wire catalogue, so visuals are *names*
//! resolved against a client-local asset root. Differing local assets are cosmetic only:
//! nothing here feeds the simulation, the wire or the content hash.
//!
//! - Asset root: `MOBA_ASSET_ROOT` (default `content`). A model named `sentinel` resolves to
//!   `<root>/models/sentinel.glb`.
//! - Hero → model: until packs name a model per hero (#285, `HeroSpec.model`),
//!   `MOBA_HERO_MODELS` maps hero names to model names: `Ranger=sentinel,*=knight`, or a bare
//!   `sentinel` for every hero. Unset means no models, i.e. today's procedural rig.
//!   [`ModelLibrary::bind_heroes`] takes the per-hero model names, so switching to the
//!   schema field only changes where those names come from.
//! - Any failure (bad name, missing/unreadable/malformed file, mesh-slot exhaustion) is
//!   logged once per model and that hero keeps the procedural rig; a match never fails
//!   because of an asset.
//!
//! - Skinned models (WP-4.1): a model whose file has a skin is uploaded twice, as a static
//!   mesh (the fallback) and as a skinned mesh, and its [`Skeleton`] is kept for per-frame
//!   posing. `MOBA_NO_SKINNING=1`, a renderer without the skinned pipeline or a failed
//!   skinned upload leaves the hero on the static mesh.
//!
//! Loading and GPU upload happen only in `bind_heroes` (startup and content reload). The
//! per-frame lookup is a bounds-checked index into a preallocated table and never
//! allocates.

use std::path::{Path, PathBuf};

use moba_assets::Skeleton;
use moba_core::math::{Mat4, Vec3};
use moba_render::{MeshData, MeshId};

/// Environment variable naming the client-local asset root.
pub(crate) const ASSET_ROOT_ENV: &str = "MOBA_ASSET_ROOT";
/// Environment variable mapping hero names to model names.
pub(crate) const HERO_MODELS_ENV: &str = "MOBA_HERO_MODELS";
/// Environment variable that disables GPU skinning (`1`): skinned models draw static.
pub(crate) const NO_SKINNING_ENV: &str = "MOBA_NO_SKINNING";
/// Default asset root, relative to the working directory.
const DEFAULT_ASSET_ROOT: &str = "content";
/// Height every hero model is normalised to, matching the procedural rig (metres).
pub(crate) const HERO_MODEL_HEIGHT: f32 = 3.6;
/// Longest accepted model name.
const MAX_MODEL_NAME: usize = 32;
/// Hero table size; the wire catalogue carries at most `u8::MAX + 1` heroes.
const MAX_HEROES: usize = 256;

/// A loaded hero model: the mesh plus the transform that stands it on the ground at
/// [`HERO_MODEL_HEIGHT`], centred on the unit's position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeroModel {
    /// Static mesh: always present, and the fallback for a skinned model.
    pub(crate) mesh: MeshId,
    pub(crate) normalize: Mat4,
    /// GPU-skinned variant, when the file has a skin and skinning is available.
    pub(crate) skin: Option<SkinnedModel>,
}

/// The skinned upload of a model and its skeleton in [`ModelLibrary::rig`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SkinnedModel {
    pub(crate) mesh: MeshId,
    pub(crate) rig: usize,
}

/// Which upload `bind_heroes` asks the renderer for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Upload {
    /// `Renderer::register_mesh`.
    Static,
    /// `Renderer::register_skinned_mesh`.
    Skinned,
}

/// `true` unless [`NO_SKINNING_ENV`] is set to `1`. Read once at startup.
pub(crate) fn skinning_enabled_from_env() -> bool {
    std::env::var(NO_SKINNING_ENV).map_or(true, |value| value.trim() != "1")
}

impl HeroModel {
    /// Fits a mesh's bounds to the hero height with its feet at `y = 0`.
    ///
    /// Returns `None` for a mesh with no height (it could not be placed sensibly).
    pub(crate) fn fit(mesh: MeshId, data: &MeshData) -> Option<Self> {
        let (min, max) = data.bounds();
        let height = max[1] - min[1];
        if !height.is_finite() || height <= 1e-4 {
            return None;
        }
        let scale = HERO_MODEL_HEIGHT / height;
        let center = Vec3::new(min[0].midpoint(max[0]), min[1], min[2].midpoint(max[2]));
        Some(Self {
            mesh,
            normalize: Mat4::from_scale(Vec3::splat(scale)) * Mat4::from_translation(-center),
            skin: None,
        })
    }
}

/// `true` for 1–32 ASCII letters, digits, `_` or `-` (no separators, dots or drive colons,
/// so a name can never escape `<root>/models/`).
pub(crate) fn valid_model_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_MODEL_NAME
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Resolves a model name to `<root>/models/<name>.glb`.
pub(crate) fn model_path(root: &Path, name: &str) -> Option<PathBuf> {
    valid_model_name(name).then(|| root.join("models").join(format!("{name}.glb")))
}

/// Interim hero → model mapping parsed from [`HERO_MODELS_ENV`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModelBindings {
    rules: Vec<(String, String)>,
}

impl ModelBindings {
    /// Parses `hero=model` pairs separated by commas; `*` matches any hero and a bare
    /// `model` means `*=model`. Empty entries are ignored.
    pub(crate) fn parse(spec: &str) -> Self {
        let rules = spec
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| match entry.split_once('=') {
                Some((hero, model)) => (hero.trim().to_owned(), model.trim().to_owned()),
                None => (String::from("*"), entry.to_owned()),
            })
            .collect();
        Self { rules }
    }

    /// Model name for `hero`: an exact (ASCII case-insensitive) rule wins over `*`.
    pub(crate) fn model_for(&self, hero: &str) -> Option<&str> {
        self.rules
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(hero))
            .or_else(|| self.rules.iter().find(|(name, _)| name == "*"))
            .map(|(_, model)| model.as_str())
    }
}

/// Loaded models and the per-hero lookup table.
#[derive(Debug)]
pub(crate) struct ModelLibrary {
    root: PathBuf,
    bindings: ModelBindings,
    /// Every model name tried so far, with its result; failures are cached so each one is
    /// logged once and never retried.
    loaded: Vec<(String, Option<HeroModel>)>,
    /// Indexed by hero pick (catalogue index).
    heroes: Vec<Option<HeroModel>>,
    /// Skeletons of the skinned models, indexed by [`SkinnedModel::rig`].
    rigs: Vec<Skeleton>,
    /// Whether to upload skinned variants at all.
    skinning: bool,
}

impl ModelLibrary {
    /// Builds an empty library from [`ASSET_ROOT_ENV`] and [`HERO_MODELS_ENV`]. No I/O.
    pub(crate) fn from_env() -> Self {
        let root = std::env::var_os(ASSET_ROOT_ENV)
            .filter(|value| !value.is_empty())
            .map_or_else(|| PathBuf::from(DEFAULT_ASSET_ROOT), PathBuf::from);
        let bindings = std::env::var(HERO_MODELS_ENV)
            .map(|spec| ModelBindings::parse(&spec))
            .unwrap_or_default();
        let mut library = Self::new(root, bindings);
        library.skinning = skinning_enabled_from_env();
        library
    }

    pub(crate) fn new(root: PathBuf, bindings: ModelBindings) -> Self {
        Self {
            root,
            bindings,
            loaded: Vec::new(),
            heroes: Vec::with_capacity(MAX_HEROES),
            rigs: Vec::new(),
            skinning: true,
        }
    }

    /// Whether skinned models are uploaded skinned (false with `MOBA_NO_SKINNING=1`).
    pub(crate) fn skinning(&self) -> bool {
        self.skinning
    }

    /// Enables or disables skinned uploads for later binds (tests and the env opt-out).
    #[cfg(test)]
    pub(crate) fn set_skinning(&mut self, enabled: bool) {
        self.skinning = enabled;
    }

    /// The skeleton of a skinned model. Never allocates.
    pub(crate) fn rig(&self, index: usize) -> Option<&Skeleton> {
        self.rigs.get(index)
    }

    /// Forgets every uploaded mesh. Call when a new renderer is created (its mesh table holds
    /// only the cube, so cached `MeshId`s would be stale). Cached failures are kept: the same
    /// files would fail again and were already logged.
    pub(crate) fn renderer_recreated(&mut self) {
        self.loaded.retain(|(_, model)| model.is_none());
        self.heroes.clear();
        self.rigs.clear();
    }

    /// The interim name mapping, used by callers until hero specs carry model names.
    pub(crate) fn bindings(&self) -> &ModelBindings {
        &self.bindings
    }

    /// Rebuilds the hero table from one optional model name per hero (in catalogue order),
    /// loading and registering each distinct model once.
    ///
    /// `register` uploads a mesh (normally [`moba_render::Renderer::register_mesh`], or
    /// `register_skinned_mesh` for [`Upload::Skinned`]). Every failure is logged once and
    /// leaves that hero on the procedural rig (or, for a failed skinned upload, on the
    /// static mesh).
    pub(crate) fn bind_heroes<'a, R>(
        &mut self,
        models: impl IntoIterator<Item = Option<&'a str>>,
        mut register: R,
    ) where
        R: FnMut(&MeshData, Upload) -> Result<MeshId, String>,
    {
        self.heroes.clear();
        for model in models.into_iter().take(MAX_HEROES) {
            let resolved = model.and_then(|name| self.load(name, &mut register));
            self.heroes.push(resolved);
        }
    }

    fn load<R>(&mut self, name: &str, register: &mut R) -> Option<HeroModel>
    where
        R: FnMut(&MeshData, Upload) -> Result<MeshId, String>,
    {
        if let Some((_, cached)) = self.loaded.iter().find(|(loaded, _)| loaded == name) {
            return *cached;
        }
        let result = self.try_load(name, register);
        let model = match result {
            Ok(model) => {
                let kind = if model.skin.is_some() {
                    "skinned"
                } else {
                    "static"
                };
                eprintln!("[assets] model '{name}' loaded ({kind})");
                Some(model)
            }
            Err(reason) => {
                eprintln!("[assets] model '{name}': {reason}; using the procedural rig");
                None
            }
        };
        self.loaded.push((name.to_owned(), model));
        model
    }

    fn try_load<R>(&mut self, name: &str, register: &mut R) -> Result<HeroModel, String>
    where
        R: FnMut(&MeshData, Upload) -> Result<MeshId, String>,
    {
        let path = model_path(&self.root, name)
            .ok_or_else(|| String::from("invalid model name (use 1-32 of [A-Za-z0-9_-])"))?;
        let data = moba_assets::load_glb_file(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let mesh =
            register(&data, Upload::Static).map_err(|error| format!("upload failed: {error}"))?;
        let mut model =
            HeroModel::fit(mesh, &data).ok_or_else(|| String::from("model has no height"))?;
        if let Some(skin) = data.skin().filter(|_| self.skinning) {
            match register(&data, Upload::Skinned) {
                Ok(skinned) => {
                    model.skin = Some(SkinnedModel {
                        mesh: skinned,
                        rig: self.rigs.len(),
                    });
                    self.rigs.push(skin.skeleton().clone());
                }
                Err(error) => {
                    eprintln!(
                        "[assets] model '{name}': skinned upload failed ({error}); drawing it static"
                    );
                }
            }
        }
        Ok(model)
    }

    /// The model for a hero pick, or `None` to draw the procedural rig. Never allocates.
    pub(crate) fn hero(&self, pick: usize) -> Option<HeroModel> {
        self.heroes.get(pick).copied().flatten()
    }
}

/// Resolves each hero's model name and (re)binds the library at startup or content reload.
///
/// Swap point for #285: once hero specs carry `model`, pass those names instead of the
/// `MOBA_HERO_MODELS` mapping.
pub(crate) fn bind_hero_models<'a>(
    models: &mut ModelLibrary,
    renderer: &mut moba_render::Renderer,
    hero_names: impl IntoIterator<Item = &'a str>,
) {
    let wanted: Vec<Option<String>> = hero_names
        .into_iter()
        .map(|hero| models.bindings().model_for(hero).map(str::to_owned))
        .collect();
    models.bind_heroes(wanted.iter().map(Option::as_deref), |data, upload| {
        match upload {
            Upload::Static => renderer.register_mesh(data),
            Upload::Skinned => renderer.register_skinned_mesh(data),
        }
        .map_err(|error| error.to_string())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = include_bytes!("../../../../content/models/sentinel.glb");
    const SKINNED_SAMPLE: &[u8] = include_bytes!("../../../../content/models/sentinel-skinned.glb");

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "moba-models-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("models")).expect("temp model dir");
        root
    }

    #[test]
    fn names_cannot_escape_the_models_directory() {
        let root = Path::new("assets");
        assert_eq!(
            model_path(root, "sentinel"),
            Some(PathBuf::from("assets/models/sentinel.glb"))
        );
        for bad in ["", "../x", "a/b", "a\\b", "c:x", "x.glb", &"x".repeat(33)] {
            assert_eq!(model_path(root, bad), None, "{bad}");
        }
    }

    #[test]
    fn bindings_prefer_exact_hero_names_over_the_wildcard() {
        let bindings = ModelBindings::parse(" Ranger = archer , *=sentinel,,");
        assert_eq!(bindings.model_for("ranger"), Some("archer"));
        assert_eq!(bindings.model_for("Vanguard"), Some("sentinel"));
        assert_eq!(
            ModelBindings::parse("sentinel").model_for("x"),
            Some("sentinel")
        );
        assert_eq!(ModelBindings::parse("").model_for("x"), None);
        assert_eq!(ModelBindings::parse("Ranger=archer").model_for("x"), None);
    }

    #[test]
    fn failures_fall_back_once_and_successes_are_shared() {
        let root = temp_root("fallback");
        std::fs::write(root.join("models/good.glb"), SAMPLE).expect("write sample");
        std::fs::write(root.join("models/bad.glb"), &SAMPLE[..100]).expect("write truncated");
        let mut library = ModelLibrary::new(root.clone(), ModelBindings::default());
        let mut uploads = 0u16;
        let mut register = |_: &MeshData, _: Upload| {
            uploads += 1;
            Ok(MeshId::CUBE)
        };
        let names = [
            Some("good"),
            Some("bad"),
            Some("missing"),
            Some("../escape"),
            None,
            Some("good"),
        ];
        library.bind_heroes(names, &mut register);
        // Rebinding (content reload) reuses the cache: no reload, no re-log, no re-upload.
        library.bind_heroes(names, &mut register);
        assert_eq!(uploads, 1);
        let good = library.hero(0).expect("good model");
        assert_eq!(library.hero(5), Some(good));
        for pick in 1..5 {
            assert_eq!(library.hero(pick), None, "pick {pick} falls back");
        }
        assert_eq!(library.hero(99), None);
        // The fitted sample stands on the ground at hero height.
        let top = good.normalize.transform_point3(Vec3::new(0.0, 3.78, 0.0));
        let feet = good.normalize.transform_point3(Vec3::ZERO);
        assert!((top.y - HERO_MODEL_HEIGHT).abs() < 0.05, "{top:?}");
        assert!(feet.y.abs() < 1e-5, "{feet:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_new_renderer_reuploads_models() {
        let root = temp_root("recreate");
        std::fs::write(root.join("models/good.glb"), SAMPLE).expect("write sample");
        let mut library = ModelLibrary::new(root.clone(), ModelBindings::default());
        let mut uploads = 0u16;
        let mut register = |_: &MeshData, _: Upload| {
            uploads += 1;
            Ok(MeshId::CUBE)
        };
        library.bind_heroes([Some("good"), Some("missing")], &mut register);
        library.renderer_recreated();
        assert_eq!(library.hero(0), None, "stale ids are dropped");
        library.bind_heroes([Some("good"), Some("missing")], &mut register);
        assert_eq!(uploads, 2, "the model is uploaded to the new renderer");
        assert!(library.hero(0).is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn upload_failures_fall_back_to_the_rig() {
        let root = temp_root("upload");
        std::fs::write(root.join("models/good.glb"), SAMPLE).expect("write sample");
        let mut library = ModelLibrary::new(root.clone(), ModelBindings::default());
        library.bind_heroes([Some("good")], |_: &MeshData, _: Upload| {
            Err(String::from("mesh limit reached"))
        });
        assert_eq!(library.hero(0), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn skinned_models_upload_twice_and_fall_back_to_static() {
        let root = temp_root("skinned");
        std::fs::write(root.join("models/walker.glb"), SKINNED_SAMPLE).expect("write sample");
        std::fs::write(root.join("models/statue.glb"), SAMPLE).expect("write sample");
        let mut library = ModelLibrary::new(root.clone(), ModelBindings::default());
        let mut uploads = Vec::new();
        library.bind_heroes([Some("walker"), Some("statue")], |_: &MeshData, upload| {
            uploads.push(upload);
            Ok(MeshId::CUBE)
        });
        assert_eq!(uploads, [Upload::Static, Upload::Skinned, Upload::Static]);
        let walker = library.hero(0).expect("walker");
        let skin = walker.skin.expect("skinned variant");
        assert_eq!(library.rig(skin.rig).map(Skeleton::joint_count), Some(7));
        assert!(library.hero(1).expect("statue").skin.is_none());

        // The skinned pipeline is missing: the hero keeps its static mesh.
        library.renderer_recreated();
        library.bind_heroes([Some("walker")], |_: &MeshData, upload| match upload {
            Upload::Static => Ok(MeshId::CUBE),
            Upload::Skinned => Err(String::from("GPU skinning is unavailable")),
        });
        let walker = library.hero(0).expect("static fallback");
        assert!(walker.skin.is_none());
        assert!(
            library.rig(0).is_none(),
            "rigs are cleared with the renderer"
        );

        // `MOBA_NO_SKINNING=1`: no skinned upload is attempted.
        let mut library = ModelLibrary::new(root.clone(), ModelBindings::default());
        library.set_skinning(false);
        let mut skinned_requests = 0;
        library.bind_heroes([Some("walker")], |_: &MeshData, upload| {
            skinned_requests += usize::from(upload == Upload::Skinned);
            Ok(MeshId::CUBE)
        });
        assert_eq!(skinned_requests, 0);
        assert!(library.hero(0).expect("static").skin.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn hero_lookup_does_not_allocate() {
        let root = temp_root("alloc");
        std::fs::write(root.join("models/good.glb"), SAMPLE).expect("write sample");
        let mut library = ModelLibrary::new(root.clone(), ModelBindings::default());
        library.bind_heroes([Some("good"), None], |_: &MeshData, _: Upload| {
            Ok(MeshId::CUBE)
        });
        let (found, allocations) = crate::app::allocation_probe::probe(|| {
            (0..1_000)
                .filter(|pick| library.hero(pick % 3).is_some())
                .count()
        });
        assert_eq!(found, 334);
        assert_eq!(allocations, 0);
        let _ = std::fs::remove_dir_all(root);
    }
}
