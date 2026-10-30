//! Client-local hero models with procedural fallback (WP-3.2, ADR-0007).
//!
//! Network clients never see pack files, only the wire catalogue, so visuals are *names*
//! resolved against a client-local asset root. Differing local assets are cosmetic only:
//! nothing here feeds the simulation, the wire or the content hash.
//!
//! - Asset root: `MOBA_ASSET_ROOT` (default `content`). A model named `sentinel` resolves to
//!   `<root>/models/sentinel.glb`.
//! - Hero → model: the catalogue's per-hero model name (`HeroSpec::model` in a local match,
//!   `WireHero::model` from a server; #285). `None` draws the procedural rig.
//! - Dev override: `MOBA_HERO_MODELS` maps hero names to model names
//!   (`Ranger=sentinel,*=knight`, or a bare `sentinel` for every hero). A rule that matches a
//!   hero wins over the catalogue; heroes it does not match keep the catalogue's model. It is
//!   local and cosmetic, like the asset root.
//! - Any failure (bad name, missing/unreadable/malformed file, mesh-slot exhaustion) is
//!   logged once per model and that hero keeps the procedural rig; a match never fails
//!   because of an asset.
//!
//! - Skinned models (WP-4.1): a model whose file has a skin is uploaded twice, as a static
//!   mesh (the fallback) and as a skinned mesh, and its [`Skeleton`] is kept for per-frame
//!   posing. `MOBA_NO_SKINNING=1`, a renderer without the skinned pipeline or a failed
//!   skinned upload leaves the hero on the static mesh.
//! - Animation sets (ADR-0012): a skinned model `name` may have a client-local
//!   `<root>/animations/<name>.toml` descriptor, bound to its skeleton at load. A missing or
//!   invalid one is logged once and the hero keeps the WP-4.1 pose (rest pose blended to the
//!   run clip by gait); `MOBA_NO_ANIMATION_SETS=1` skips them.
//!
//! - Bone LOD (R4b, `app::bone_lod`): a skinned model also uploads two reduced skins
//!   (`moba_assets::SkinLod`) unless `MOBA_BONE_LOD=off`; a failed reduced upload just leaves
//!   that level on the full skin.
//! - Attachments (R4b): an animation set's sockets may name a static `model` drawn at the
//!   socket (`<root>/models/<model>.glb`, loaded once, logged once on failure).
//!   `MOBA_ATTACHMENTS=off` skips them.
//!
//! Loading and GPU upload happen only in `bind_heroes` (startup and content reload). The
//! per-frame lookup is a bounds-checked index into a preallocated table and never
//! allocates.

use std::path::{Path, PathBuf};

use moba_assets::{AnimationGraph, AnimationSet, MAX_SOCKETS, Skeleton, SkinLod, SocketId};
use moba_core::math::{Mat4, Vec3};
use moba_render::{MeshData, MeshId};

use crate::app::materials::{MaterialLibrary, MaterialSink, ModelMeshes};

/// Environment variable naming the client-local asset root.
pub(crate) const ASSET_ROOT_ENV: &str = "MOBA_ASSET_ROOT";
/// Environment variable mapping hero names to model names (dev override of the catalogue).
pub(crate) const HERO_MODELS_ENV: &str = "MOBA_HERO_MODELS";
/// Environment variable that disables GPU skinning (`1`): skinned models draw static.
pub(crate) const NO_SKINNING_ENV: &str = "MOBA_NO_SKINNING";
/// Environment variable that hides socket attachments (`off` or `0`).
pub(crate) const ATTACHMENTS_ENV: &str = "MOBA_ATTACHMENTS";
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
    /// The reduced skins of bone LOD 1 and 2 ([`ModelLibrary::skin_lod`]), when uploaded.
    pub(crate) lods: [Option<MeshId>; crate::app::bone_lod::LOD_LEVELS],
}

/// One visible socket attachment of a rig: the socket, the static mesh and its scale.
pub(crate) type Attachment = (SocketId, MeshId, f32);

/// Which upload `bind_heroes` asks the renderer for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Upload {
    /// `Renderer::register_mesh`.
    Static,
    /// `Renderer::register_skinned_mesh`.
    Skinned,
}

/// `false` when [`ATTACHMENTS_ENV`] is `off` or `0`. Read once at startup.
pub(crate) fn attachments_enabled_from_env() -> bool {
    std::env::var(ATTACHMENTS_ENV).map_or(true, |value| {
        let value = value.trim();
        !(value.eq_ignore_ascii_case("off") || value == "0")
    })
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

/// The client-local asset root: [`ASSET_ROOT_ENV`], else `content`. Shared by hero models
/// and map visuals.
pub(crate) fn asset_root_from_env() -> PathBuf {
    std::env::var_os(ASSET_ROOT_ENV)
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from(DEFAULT_ASSET_ROOT), PathBuf::from)
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

/// Loads a model and, when `<model>.lod` sits beside it (`moba-tools mesh-bake lod`), its
/// mesh levels (R8, ADR-0018 §3). A level file that is stale or malformed is logged and
/// ignored: the model draws at LOD0.
pub(crate) fn load_model_file(path: &Path) -> Result<MeshData, moba_assets::GlbError> {
    let (data, lod_error) = moba_assets::load_glb_file_with_lods(path)?;
    if let Some(error) = lod_error {
        eprintln!(
            "[lod] {}: {error}; drawing LOD0 only (re-run scripts/bake-mesh-lods.sh)",
            moba_assets::lod_path(path).display()
        );
    }
    Ok(data)
}

/// Resolves a model name to its animation set, `<root>/animations/<name>.toml`.
pub(crate) fn animation_set_path(root: &Path, name: &str) -> Option<PathBuf> {
    valid_model_name(name).then(|| root.join("animations").join(format!("{name}.toml")))
}

/// Dev-override hero → model mapping parsed from [`HERO_MODELS_ENV`].
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

/// Mesh slots a bind leaves free for later registrations (map props, VAT minions, effect
/// meshes) before it gives any to bone-LOD skins.
pub(crate) const LOD_SLOT_RESERVE: usize = 8;

/// A skinned model whose bone-LOD skins are built after every hero's base meshes.
struct PendingLod {
    rig: usize,
    name: String,
    data: MeshData,
}

/// Loaded models and the per-hero lookup table.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)] // Independent load-time switches.
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
    /// Bound animation sets, parallel to `rigs` (`None`: the WP-4.1 pose).
    graphs: Vec<Option<AnimationGraph>>,
    /// Reduced skins of bone LOD 1 and 2, parallel to `rigs`.
    skin_lods: Vec<[Option<SkinLod>; crate::app::bone_lod::LOD_LEVELS]>,
    /// Visible socket attachments, parallel to `rigs`.
    attachments: Vec<[Option<Attachment>; MAX_SOCKETS]>,
    /// Attachment meshes by model name (failures cached as `None`, logged once).
    attachment_meshes: Vec<(String, Option<MeshId>)>,
    /// Whether to build reduced skins.
    bone_lod: bool,
    /// Mesh slots the renderer has free for this bind (bone-LOD skins are skipped when they
    /// would leave fewer than [`LOD_SLOT_RESERVE`]).
    mesh_budget: usize,
    /// Whether a skipped-LOD warning was logged (once per library).
    lod_budget_logged: bool,
    /// Whether to load socket attachments.
    show_attachments: bool,
    /// Whether to look for animation sets at all.
    animation_sets: bool,
    /// Whether to upload skinned variants at all.
    skinning: bool,
    /// Client-local materials of the loaded models (`materials/<model>.toml`, ADR-0010).
    materials: MaterialLibrary,
}

impl ModelLibrary {
    /// Builds an empty library from [`ASSET_ROOT_ENV`] and [`HERO_MODELS_ENV`]. No I/O.
    pub(crate) fn from_env() -> Self {
        let root = asset_root_from_env();
        let bindings = std::env::var(HERO_MODELS_ENV)
            .map(|spec| ModelBindings::parse(&spec))
            .unwrap_or_default();
        let mut library = Self::new(root, bindings);
        library.skinning = skinning_enabled_from_env();
        library.animation_sets = crate::app::animation::animation_sets_enabled_from_env();
        library.show_attachments = attachments_enabled_from_env();
        library
    }

    /// Enables or disables the reduced bone-LOD skins for later binds (`MOBA_BONE_LOD=off`).
    pub(crate) fn set_bone_lod(&mut self, enabled: bool) {
        self.bone_lod = enabled;
    }

    /// Sets the renderer's free mesh slots for the next bind (`Renderer::mesh_capacity` minus
    /// `mesh_count`). Base meshes always come first; bone-LOD skins only use what is left.
    pub(crate) fn set_mesh_budget(&mut self, free_slots: usize) {
        self.mesh_budget = free_slots;
    }

    /// Enables or disables socket attachments for later binds (tests and the env opt-out).
    #[cfg(test)]
    pub(crate) fn set_attachments(&mut self, enabled: bool) {
        self.show_attachments = enabled;
    }

    /// The reduced skin of bone LOD `level` (1 or 2) of a skinned model. Never allocates.
    pub(crate) fn skin_lod(&self, rig: usize, level: usize) -> Option<&SkinLod> {
        let index = level.checked_sub(1)?;
        self.skin_lods.get(rig)?.get(index)?.as_ref()
    }

    /// The visible socket attachments of a skinned model. Never allocates.
    pub(crate) fn attachments(&self, rig: usize) -> impl Iterator<Item = Attachment> + '_ {
        self.attachments
            .get(rig)
            .into_iter()
            .flat_map(|attachments| attachments.iter().flatten().copied())
    }

    pub(crate) fn new(root: PathBuf, bindings: ModelBindings) -> Self {
        Self {
            materials: MaterialLibrary::new(root.clone()),
            root,
            bindings,
            loaded: Vec::new(),
            heroes: Vec::with_capacity(MAX_HEROES),
            rigs: Vec::new(),
            graphs: Vec::new(),
            skin_lods: Vec::new(),
            attachments: Vec::new(),
            attachment_meshes: Vec::new(),
            bone_lod: true,
            mesh_budget: usize::MAX,
            lod_budget_logged: false,
            show_attachments: true,
            animation_sets: true,
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

    /// The bound animation set of a skinned model, if it has a usable one. Never allocates.
    pub(crate) fn graph(&self, index: usize) -> Option<&AnimationGraph> {
        self.graphs.get(index).and_then(Option::as_ref)
    }

    /// Enables or disables animation sets for later binds (tests and the env opt-out).
    #[cfg(test)]
    pub(crate) fn set_animation_sets(&mut self, enabled: bool) {
        self.animation_sets = enabled;
    }

    /// Forgets every uploaded mesh. Call when a new renderer is created (its mesh table holds
    /// only the cube, so cached `MeshId`s would be stale). Cached failures are kept: the same
    /// files would fail again and were already logged.
    pub(crate) fn renderer_recreated(&mut self) {
        self.materials.renderer_recreated();
        self.loaded.retain(|(_, model)| model.is_none());
        self.heroes.clear();
        self.rigs.clear();
        self.graphs.clear();
        self.skin_lods.clear();
        self.attachments.clear();
        self.attachment_meshes.clear();
    }

    /// Applies the materials of every loaded model not handled yet (one registration call;
    /// see [`MaterialLibrary::bind`]). Load time only.
    pub(crate) fn bind_materials(&mut self, sink: &mut impl MaterialSink) {
        let Self {
            loaded, materials, ..
        } = self;
        materials.bind(
            sink,
            loaded.iter().filter_map(|(name, model)| {
                model.map(|model| ModelMeshes {
                    name,
                    meshes: [
                        Some(model.mesh),
                        model.skin.map(|skin| skin.mesh),
                        model.skin.and_then(|skin| skin.lods[0]),
                        model.skin.and_then(|skin| skin.lods[1]),
                    ],
                })
            }),
        );
    }

    /// The dev-override name mapping (see [`hero_model`]).
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
        let used = core::cell::Cell::new(0usize);
        let mut counted = |data: &MeshData, upload| {
            let result = register(data, upload);
            used.set(used.get() + usize::from(result.is_ok()));
            result
        };
        // Every hero's base meshes first, so reduced skins never take a later hero's slot.
        let mut pending = Vec::new();
        for model in models.into_iter().take(MAX_HEROES) {
            let resolved = model.and_then(|name| self.load(name, &mut counted, &mut pending));
            self.heroes.push(resolved);
        }
        let mut skipped = 0usize;
        for lod in pending {
            let free = self.mesh_budget.saturating_sub(used.get());
            if free < crate::app::bone_lod::LOD_LEVELS + LOD_SLOT_RESERVE {
                skipped += 1;
                continue;
            }
            self.bind_skin_lods(&lod, &mut counted);
        }
        if skipped > 0 && !self.lod_budget_logged {
            self.lod_budget_logged = true;
            eprintln!(
                "[anim] bone LOD skipped for {skipped} model(s): too few renderer mesh slots \
                 left; they draw their full skins"
            );
        }
    }

    /// Builds and registers the bone-LOD skins of one skinned model (the second bind pass)
    /// and points every hero using that rig at them.
    fn bind_skin_lods<R>(&mut self, lod: &PendingLod, register: &mut R)
    where
        R: FnMut(&MeshData, Upload) -> Result<MeshId, String>,
    {
        let graph = self.graphs.get(lod.rig).and_then(Option::as_ref);
        let (meshes, skins) = self.build_skin_lods(&lod.name, &lod.data, graph, register);
        if let Some(slot) = self.skin_lods.get_mut(lod.rig) {
            *slot = skins;
        }
        let rig = lod.rig;
        let update = |model: &mut Option<HeroModel>| {
            if let Some(skin) = model.as_mut().and_then(|model| model.skin.as_mut())
                && skin.rig == rig
            {
                skin.lods = meshes;
            }
        };
        self.loaded.iter_mut().for_each(|(_, model)| update(model));
        self.heroes.iter_mut().for_each(update);
    }

    fn load<R>(
        &mut self,
        name: &str,
        register: &mut R,
        pending: &mut Vec<PendingLod>,
    ) -> Option<HeroModel>
    where
        R: FnMut(&MeshData, Upload) -> Result<MeshId, String>,
    {
        if let Some((_, cached)) = self.loaded.iter().find(|(loaded, _)| loaded == name) {
            return *cached;
        }
        let result = self.try_load(name, register, pending);
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

    fn try_load<R>(
        &mut self,
        name: &str,
        register: &mut R,
        pending: &mut Vec<PendingLod>,
    ) -> Result<HeroModel, String>
    where
        R: FnMut(&MeshData, Upload) -> Result<MeshId, String>,
    {
        let path = model_path(&self.root, name)
            .ok_or_else(|| String::from("invalid model name (use 1-32 of [A-Za-z0-9_-])"))?;
        let data =
            load_model_file(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mesh =
            register(&data, Upload::Static).map_err(|error| format!("upload failed: {error}"))?;
        let mut model =
            HeroModel::fit(mesh, &data).ok_or_else(|| String::from("model has no height"))?;
        if let Some(skin) = data.skin().filter(|_| self.skinning) {
            match register(&data, Upload::Skinned) {
                Ok(skinned) => {
                    let graph = self
                        .animation_sets
                        .then(|| self.load_animation_set(name, skin.skeleton()))
                        .flatten();
                    let attachments = graph.as_ref().map_or([None; MAX_SOCKETS], |graph| {
                        self.load_attachments(graph, register)
                    });
                    model.skin = Some(SkinnedModel {
                        mesh: skinned,
                        rig: self.rigs.len(),
                        lods: [None; crate::app::bone_lod::LOD_LEVELS],
                    });
                    if self.bone_lod {
                        pending.push(PendingLod {
                            rig: self.rigs.len(),
                            name: name.to_owned(),
                            data: data.clone(),
                        });
                    }
                    self.rigs.push(skin.skeleton().clone());
                    self.graphs.push(graph);
                    self.skin_lods.push([None, None]);
                    self.attachments.push(attachments);
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

    /// Builds and uploads the bone-LOD skins of a skinned model (spring-chain joints are
    /// dropped first). A level that fails to build or upload stays on the full skin.
    #[allow(clippy::type_complexity)] // Two parallel per-level arrays.
    fn build_skin_lods<R>(
        &self,
        name: &str,
        data: &MeshData,
        graph: Option<&AnimationGraph>,
        register: &mut R,
    ) -> (
        [Option<MeshId>; crate::app::bone_lod::LOD_LEVELS],
        [Option<SkinLod>; crate::app::bone_lod::LOD_LEVELS],
    )
    where
        R: FnMut(&MeshData, Upload) -> Result<MeshId, String>,
    {
        let mut meshes = [None; crate::app::bone_lod::LOD_LEVELS];
        let mut skins = [None, None];
        let Some(joints) = data.skin().map(|skin| skin.skeleton().joint_count()) else {
            return (meshes, skins);
        };
        if !self.bone_lod {
            return (meshes, skins);
        }
        let springs: Vec<usize> =
            graph.map_or_else(Vec::new, |graph| graph.spring_joints().collect());
        for level in 1..=crate::app::bone_lod::LOD_LEVELS {
            let (max_joints, max_influences) = crate::app::bone_lod::lod_budget(level, joints);
            let Some(lod) = SkinLod::build(data, max_joints, max_influences, &springs) else {
                eprintln!("[anim] model '{name}': bone LOD{level} could not be built");
                continue;
            };
            match register(lod.mesh(), Upload::Skinned) {
                Ok(mesh) => {
                    meshes[level - 1] = Some(mesh);
                    skins[level - 1] = Some(lod);
                }
                Err(error) => {
                    eprintln!("[anim] model '{name}': bone LOD{level} upload failed ({error})");
                }
            }
        }
        (meshes, skins)
    }

    /// Loads the static meshes a graph's sockets attach (each model once; failures logged
    /// once and the socket left bare).
    fn load_attachments<R>(
        &mut self,
        graph: &AnimationGraph,
        register: &mut R,
    ) -> [Option<Attachment>; MAX_SOCKETS]
    where
        R: FnMut(&MeshData, Upload) -> Result<MeshId, String>,
    {
        let mut out = [None; MAX_SOCKETS];
        if !self.show_attachments {
            return out;
        }
        for ((socket, model, scale), slot) in graph.attachments().zip(&mut out) {
            let cached = self
                .attachment_meshes
                .iter()
                .find(|(name, _)| name == model)
                .map(|(_, mesh)| *mesh);
            let mesh = cached.unwrap_or_else(|| {
                let result = model_path(&self.root, model)
                    .ok_or_else(|| String::from("invalid model name"))
                    .and_then(|path| {
                        load_model_file(&path)
                            .map_err(|error| format!("{}: {error}", path.display()))
                    })
                    .and_then(|data| register(&data, Upload::Static));
                let mesh = match result {
                    Ok(mesh) => {
                        eprintln!("[anim] attachment '{model}' loaded");
                        Some(mesh)
                    }
                    Err(error) => {
                        eprintln!("[anim] attachment '{model}': {error}; the socket stays bare");
                        None
                    }
                };
                self.attachment_meshes.push((model.to_owned(), mesh));
                mesh
            });
            *slot = mesh.map(|mesh| (socket, mesh, scale));
        }
        out
    }

    /// Loads and binds `<root>/animations/<name>.toml`; any failure is logged once (models
    /// are cached, so this runs once per model) and leaves the WP-4.1 pose.
    fn load_animation_set(&self, name: &str, skeleton: &Skeleton) -> Option<AnimationGraph> {
        let path = animation_set_path(&self.root, name)?;
        let result =
            AnimationSet::load_file(&path).and_then(|set| AnimationGraph::bind(&set, skeleton));
        match result {
            Ok(graph) => {
                eprintln!("[anim] model '{name}': animation set {}", path.display());
                Some(graph)
            }
            Err(error) => {
                eprintln!(
                    "[anim] model '{name}': {}: {error}; using the single-clip pose",
                    path.display()
                );
                None
            }
        }
    }

    /// The model for a hero pick, or `None` to draw the procedural rig. Never allocates.
    pub(crate) fn hero(&self, pick: usize) -> Option<HeroModel> {
        self.heroes.get(pick).copied().flatten()
    }
}

/// The model to draw for one hero: a [`HERO_MODELS_ENV`] rule matching the hero (exact name
/// or `*`) overrides the catalogue's `HeroSpec::model`; otherwise the catalogue name is used.
pub(crate) fn hero_model<'a>(
    bindings: &'a ModelBindings,
    hero: &str,
    catalogue: Option<&'a str>,
) -> Option<&'a str> {
    bindings.model_for(hero).or(catalogue)
}

/// Resolves each hero's model name and (re)binds the library at startup or content reload.
///
/// `heroes` yields `(hero name, catalogue model)` in catalogue order; the model comes from
/// `HeroSpec::model` (local match) or `WireHero::model` (network), see [`hero_model`].
pub(crate) fn bind_hero_models<'a>(
    models: &mut ModelLibrary,
    renderer: &mut moba_render::Renderer,
    heroes: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) {
    let wanted: Vec<Option<String>> = heroes
        .into_iter()
        .map(|(hero, model)| hero_model(models.bindings(), hero, model).map(str::to_owned))
        .collect();
    models.set_mesh_budget(
        renderer
            .mesh_capacity()
            .saturating_sub(renderer.mesh_count()),
    );
    models.bind_heroes(wanted.iter().map(Option::as_deref), |data, upload| {
        match upload {
            Upload::Static => renderer.register_mesh(data),
            Upload::Skinned => renderer.register_skinned_mesh(data),
        }
        .map_err(|error| error.to_string())
    });
    models.bind_materials(renderer);
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
    fn catalogue_models_apply_unless_the_dev_override_matches() {
        let none = ModelBindings::default();
        assert_eq!(
            hero_model(&none, "Guardian", Some("sentinel")),
            Some("sentinel")
        );
        assert_eq!(hero_model(&none, "Blade", None), None);
        let exact = ModelBindings::parse("Guardian=knight");
        assert_eq!(
            hero_model(&exact, "guardian", Some("sentinel")),
            Some("knight")
        );
        assert_eq!(
            hero_model(&exact, "Blade", Some("sentinel")),
            Some("sentinel")
        );
        assert_eq!(hero_model(&exact, "Blade", None), None);
        let wildcard = ModelBindings::parse("*=knight");
        assert_eq!(hero_model(&wildcard, "Blade", None), Some("knight"));
        assert_eq!(
            hero_model(&wildcard, "Blade", Some("sentinel")),
            Some("knight")
        );
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
    fn bone_lods_never_take_a_later_heroes_mesh_slot() {
        let root = temp_root("lod-budget");
        let names: Vec<String> = (0..20).map(|index| format!("hero{index}")).collect();
        for name in &names {
            std::fs::write(root.join(format!("models/{name}.glb")), SKINNED_SAMPLE)
                .expect("write sample");
        }
        // A renderer with 63 free slots (64 minus the cube), as `bind_hero_models` passes.
        let mut library = ModelLibrary::new(root.clone(), ModelBindings::default());
        library.set_mesh_budget(63);
        let mut used = 0usize;
        library.bind_heroes(names.iter().map(|name| Some(name.as_str())), |_, _| {
            if used == 63 {
                return Err(String::from("too many meshes"));
            }
            used += 1;
            Ok(MeshId::CUBE)
        });
        // Every hero keeps its static and skinned meshes (40 slots) ...
        for index in 0..20 {
            assert!(
                library.hero(index).and_then(|hero| hero.skin).is_some(),
                "hero {index} lost its model"
            );
        }
        // ... and LOD skins take two slots each while the reserve stays free.
        let with_lods = (0..20)
            .filter(|&index| {
                let skin = library
                    .hero(index)
                    .and_then(|hero| hero.skin)
                    .expect("skin");
                skin.lods.iter().all(Option::is_some)
            })
            .count();
        assert_eq!(with_lods, 7);
        assert_eq!(used, 40 + 2 * 7);
        assert!(63 - used >= LOD_SLOT_RESERVE);
        // The first heroes got the LODs; the rest draw their full skins.
        let first = library.hero(0).and_then(|hero| hero.skin).expect("skin");
        assert!(library.skin_lod(first.rig, 1).is_some());
        let last = library.hero(19).and_then(|hero| hero.skin).expect("skin");
        assert!(last.lods.iter().all(Option::is_none));
        assert!(library.skin_lod(last.rig, 1).is_none());
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
        // Every hero's base meshes first (the static fallback and the skinned mesh), then the
        // two bone-LOD skins (R4b).
        assert_eq!(
            uploads,
            [
                Upload::Static,
                Upload::Skinned,
                Upload::Static,
                Upload::Skinned,
                Upload::Skinned
            ]
        );
        let walker = library.hero(0).expect("walker");
        let skin = walker.skin.expect("skinned variant");
        assert_eq!(library.rig(skin.rig).map(Skeleton::joint_count), Some(10));
        assert!(skin.lods.iter().all(Option::is_some));
        // Without an animation set no joint is a spring, so LOD1 keeps 8 of 10 joints and
        // LOD2 4, with 2 and 1 influences.
        let lod1 = library.skin_lod(skin.rig, 1).expect("lod1");
        let lod2 = library.skin_lod(skin.rig, 2).expect("lod2");
        assert_eq!((lod1.joint_count(), lod1.max_influences()), (8, 2));
        assert_eq!((lod2.joint_count(), lod2.max_influences()), (4, 1));
        assert!(library.skin_lod(skin.rig, 0).is_none());
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

        // `MOBA_BONE_LOD=off`: only the full skin.
        let mut library = ModelLibrary::new(root.clone(), ModelBindings::default());
        library.set_bone_lod(false);
        let mut skinned_requests = 0;
        library.bind_heroes([Some("walker")], |_: &MeshData, upload| {
            skinned_requests += usize::from(upload == Upload::Skinned);
            Ok(MeshId::CUBE)
        });
        assert_eq!(skinned_requests, 1);
        assert!(
            library
                .hero(0)
                .expect("walker")
                .skin
                .expect("skin")
                .lods
                .iter()
                .all(Option::is_none)
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
    fn socket_attachments_load_once_and_can_be_switched_off() {
        let root = temp_root("attach");
        std::fs::create_dir_all(root.join("animations")).expect("animations dir");
        std::fs::write(root.join("models/walker.glb"), SKINNED_SAMPLE).expect("model");
        std::fs::write(
            root.join("models/crystal.glb"),
            include_bytes!("../../../../content/models/crystal.glb"),
        )
        .expect("crystal");
        std::fs::write(
            root.join("animations/walker.toml"),
            include_str!("../../../../content/animations/sentinel-skinned.toml"),
        )
        .expect("set");
        let mut library = ModelLibrary::new(root.clone(), ModelBindings::default());
        let mut statics = 0;
        library.bind_heroes([Some("walker")], |_: &MeshData, upload| {
            statics += usize::from(upload == Upload::Static);
            Ok(MeshId::CUBE)
        });
        // The hero's static fallback and the crystal, once for both sockets.
        assert_eq!(statics, 2);
        let rig = library
            .hero(0)
            .and_then(|model| model.skin)
            .expect("skin")
            .rig;
        let attachments: Vec<_> = library.attachments(rig).collect();
        assert_eq!(attachments.len(), 2);
        assert!(attachments.iter().all(|(_, mesh, _)| *mesh == MeshId::CUBE));
        assert_eq!(attachments[0].2, 0.1);
        let mut off = ModelLibrary::new(root.clone(), ModelBindings::default());
        off.set_attachments(false);
        off.bind_heroes([Some("walker")], |_: &MeshData, _: Upload| Ok(MeshId::CUBE));
        let rig = off.hero(0).and_then(|model| model.skin).expect("skin").rig;
        assert_eq!(off.attachments(rig).count(), 0);
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
