//! Effect bindings, descriptors, mesh slots and the atlas, loaded from the asset root
//! (ADR-0013).
//!
//! `<root>/vfx/bindings.toml` names an effect per ability event and per unit event; every
//! referenced `<root>/vfx/<name>.toml` is loaded once. A missing or invalid file is logged
//! once per load and resolves to no effect: it never fails a match. Mesh emitters share up
//! to [`VFX_MESH_SLOTS`] mesh slots (`cube` or `<root>/models/<name>.glb`, scaled to fit a
//! unit cube); further meshes draw as billboards. `<root>/vfx/atlas.ktx2`, if present,
//! replaces the built-in procedural atlas.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use moba_assets::{
    MAX_VFX_EMITTERS, MeshData, VfxBindings, VfxEffect, VfxError, load_ktx2_file, vfx_path,
};
use moba_core::math::Vec3;
use moba_render::{MeshId, Renderer, VFX_MESH_SLOTS};

/// Index of a loaded effect.
pub(crate) type EffectId = u16;

/// One loaded effect and its emitters' mesh slots.
#[derive(Debug)]
pub(crate) struct LoadedEffect {
    pub(crate) name: String,
    pub(crate) effect: VfxEffect,
    /// Mesh slot of each mesh emitter (`None`: draw a billboard instead).
    pub(crate) mesh_slots: [Option<u8>; MAX_VFX_EMITTERS],
}

/// The effects of one catalogue ability.
#[derive(Clone, Debug, Default)]
pub(crate) struct AbilityEffects {
    pub(crate) name: String,
    pub(crate) cast: Option<EffectId>,
    pub(crate) cast_socket: Option<String>,
    pub(crate) projectile: Option<EffectId>,
    pub(crate) hit: Option<EffectId>,
    pub(crate) zone: Option<EffectId>,
    pub(crate) tint: Vec3,
}

/// Unit-event effects and fallbacks for unbound abilities.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct UnitEffects {
    pub(crate) cast: Option<EffectId>,
    pub(crate) projectile: Option<EffectId>,
    pub(crate) hit: Option<EffectId>,
    pub(crate) zone: Option<EffectId>,
    pub(crate) hero_death: Option<EffectId>,
    pub(crate) minion_death: Option<EffectId>,
    pub(crate) structure_destroyed: Option<EffectId>,
    pub(crate) attack: Option<EffectId>,
    pub(crate) recall: Option<EffectId>,
}

/// Every loaded effect, resolved by name.
#[derive(Debug, Default)]
pub(crate) struct VfxLibrary {
    root: PathBuf,
    effects: Vec<LoadedEffect>,
    abilities: Vec<AbilityEffects>,
    defaults: UnitEffects,
    /// Mesh name per slot.
    meshes: Vec<String>,
    /// Registered meshes by name (cleared when the renderer is recreated).
    registered: Vec<(String, MeshId)>,
    /// File count and newest modification time of `<root>/vfx` at the last load.
    stamp: Option<(usize, Option<SystemTime>)>,
    /// The single effect a preview loads instead of the bindings.
    preview: Option<String>,
    /// `bindings.toml` loaded (an empty or partly broken one still counts).
    bindings_loaded: bool,
}

fn log_failure(what: &str, path: &Path, error: &VfxError) {
    eprintln!("[vfx] {what} {}: {error}; no effect", path.display());
}

impl VfxLibrary {
    /// Loads `<root>/vfx/bindings.toml` and every effect it names.
    pub(crate) fn load(root: PathBuf) -> Self {
        let mut library = Self {
            root,
            ..Self::default()
        };
        library.reload();
        library
    }

    /// Loads only `effect` (the `MOBA_VFX_PREVIEW` authoring mode).
    pub(crate) fn preview(root: PathBuf, effect: &str) -> Self {
        let mut library = Self {
            root,
            preview: Some(String::from(effect)),
            ..Self::default()
        };
        library.reload();
        library
    }

    /// Re-reads every file (keeps registered meshes, which [`Self::bind_renderer`] reuses).
    pub(crate) fn reload(&mut self) {
        self.effects.clear();
        self.abilities.clear();
        self.defaults = UnitEffects::default();
        self.meshes.clear();
        self.bindings_loaded = false;
        self.stamp = Some(stamp(&self.root.join("vfx")));
        if let Some(name) = self.preview.clone() {
            if self.effect_id(&name).is_none() {
                eprintln!("[vfx] preview effect {name:?} did not load");
            }
            return;
        }
        let path = self.root.join("vfx").join("bindings.toml");
        let bindings = match VfxBindings::load_file(&path) {
            Ok(bindings) => bindings,
            Err(VfxError::Io(_)) => {
                eprintln!("[vfx] no {}; effects off", path.display());
                return;
            }
            Err(error) => {
                eprintln!("[vfx] bindings {}: {error}; effects off", path.display());
                return;
            }
        };
        let defaults = &bindings.defaults;
        let mut resolve = |name: &Option<String>| name.as_deref().and_then(|n| self.effect_id(n));
        let unit = UnitEffects {
            cast: resolve(&defaults.cast),
            projectile: resolve(&defaults.projectile),
            hit: resolve(&defaults.hit),
            zone: resolve(&defaults.zone),
            hero_death: resolve(&defaults.hero_death),
            minion_death: resolve(&defaults.minion_death),
            structure_destroyed: resolve(&defaults.structure_destroyed),
            attack: resolve(&defaults.attack),
            recall: resolve(&defaults.recall),
        };
        let mut abilities = Vec::with_capacity(bindings.abilities.len());
        for binding in &bindings.abilities {
            if abilities
                .iter()
                .any(|known: &AbilityEffects| known.name.eq_ignore_ascii_case(&binding.name))
            {
                continue;
            }
            let mut resolve =
                |name: &Option<String>| name.as_deref().and_then(|n| self.effect_id(n));
            abilities.push(AbilityEffects {
                name: binding.name.clone(),
                cast: resolve(&binding.cast),
                cast_socket: binding.cast_socket.clone(),
                projectile: resolve(&binding.projectile),
                hit: resolve(&binding.hit),
                zone: resolve(&binding.zone),
                tint: Vec3::from_array(binding.tint),
            });
        }
        self.defaults = unit;
        self.abilities = abilities;
        self.bindings_loaded = true;
        eprintln!(
            "[vfx] {} effects for {} abilities from {}",
            self.effects.len(),
            self.abilities.len(),
            path.display()
        );
    }

    /// Whether this is the `MOBA_VFX_PREVIEW` library (one effect, no bindings).
    pub(crate) fn is_preview(&self) -> bool {
        self.preview.is_some()
    }

    /// The id of effect `name`, loading it on first use (`None` if it does not load).
    pub(crate) fn effect_id(&mut self, name: &str) -> Option<EffectId> {
        if let Some(index) = self.effects.iter().position(|loaded| loaded.name == name) {
            return EffectId::try_from(index).ok();
        }
        let Some(path) = vfx_path(&self.root, name) else {
            eprintln!("[vfx] effect name {name:?} is invalid; no effect");
            return None;
        };
        let effect = match VfxEffect::load_file(&path) {
            Ok(effect) => effect,
            Err(error) => {
                log_failure("effect", &path, &error);
                return None;
            }
        };
        let mut mesh_slots = [None; MAX_VFX_EMITTERS];
        for (slot, emitter) in mesh_slots.iter_mut().zip(&effect.emitters) {
            let Some(mesh) = emitter.mesh.as_deref() else {
                continue;
            };
            *slot = match self.meshes.iter().position(|known| known == mesh) {
                Some(index) => u8::try_from(index).ok(),
                None if self.meshes.len() < VFX_MESH_SLOTS => {
                    self.meshes.push(String::from(mesh));
                    u8::try_from(self.meshes.len() - 1).ok()
                }
                None => {
                    eprintln!(
                        "[vfx] effect {name}: mesh {mesh} exceeds the {VFX_MESH_SLOTS} mesh \
                         slots; drawn as billboards"
                    );
                    None
                }
            };
        }
        let id = EffectId::try_from(self.effects.len()).ok()?;
        self.effects.push(LoadedEffect {
            name: String::from(name),
            effect,
            mesh_slots,
        });
        Some(id)
    }

    /// The loaded effect `id`.
    pub(crate) fn effect(&self, id: EffectId) -> Option<&LoadedEffect> {
        self.effects.get(usize::from(id))
    }

    /// The preview effect, if this is a preview library and it loaded.
    pub(crate) fn preview_effect(&self) -> Option<EffectId> {
        self.preview
            .as_ref()
            .and_then(|_| (!self.effects.is_empty()).then_some(0))
    }

    /// Whether there is anything to play: the bindings loaded, or the preview effect did.
    /// When not, the client turns effects off (and keeps the pre-VFX projectile cubes).
    pub(crate) fn playable(&self) -> bool {
        if self.preview.is_some() {
            self.preview_effect().is_some()
        } else {
            self.bindings_loaded
        }
    }

    /// The binding index of the ability called `name` (no allocation).
    pub(crate) fn ability(&self, name: &str) -> Option<u16> {
        self.abilities
            .iter()
            .position(|binding| binding.name.eq_ignore_ascii_case(name))
            .and_then(|index| u16::try_from(index).ok())
    }

    /// The effects of binding `index`.
    pub(crate) fn ability_effects(&self, index: u16) -> Option<&AbilityEffects> {
        self.abilities.get(usize::from(index))
    }

    /// Unit-event effects and fallbacks.
    pub(crate) fn defaults(&self) -> &UnitEffects {
        &self.defaults
    }

    /// Number of loaded effects.
    #[cfg_attr(not(test), allow(dead_code))] // Tests.
    pub(crate) fn effect_count(&self) -> usize {
        self.effects.len()
    }

    /// Whether `<root>/vfx` changed since the last load (dev live reload; one directory
    /// listing, so poll it every few frames, not every frame).
    pub(crate) fn changed_on_disk(&self) -> bool {
        self.stamp
            .is_some_and(|stamp| stamp != self::stamp(&self.root.join("vfx")))
    }

    /// The renderer was recreated: its mesh table is new.
    pub(crate) fn renderer_recreated(&mut self) {
        self.registered.clear();
    }

    /// Binds the mesh slots and the atlas on `renderer`. Load time only (it may upload and
    /// wait for the device). Failures are logged and fall back (billboards, built-in atlas).
    pub(crate) fn bind_renderer(&mut self, renderer: &mut Renderer) {
        if !renderer.vfx_available() {
            return;
        }
        for slot in 0..VFX_MESH_SLOTS {
            let mesh = self
                .meshes
                .get(slot)
                .cloned()
                .and_then(|name| self.mesh_id(renderer, &name));
            if let Err(error) = renderer.set_vfx_mesh(slot, mesh) {
                eprintln!("[vfx] mesh slot {slot}: {error}");
            }
        }
        let atlas = self.root.join("vfx").join("atlas.ktx2");
        let texture = if atlas.is_file() {
            match load_ktx2_file(&atlas) {
                Ok(texture) => match renderer.register_texture(&texture) {
                    Ok(id) => Some(id),
                    Err(error) => {
                        eprintln!("[vfx] atlas {}: {error}; built-in atlas", atlas.display());
                        None
                    }
                },
                Err(error) => {
                    eprintln!("[vfx] atlas {}: {error}; built-in atlas", atlas.display());
                    None
                }
            }
        } else {
            None
        };
        if let Err(error) = renderer.set_vfx_atlas(texture) {
            eprintln!("[vfx] atlas: {error}");
        }
    }

    fn mesh_id(&mut self, renderer: &mut Renderer, name: &str) -> Option<MeshId> {
        if name == "cube" {
            return Some(MeshId::CUBE);
        }
        if let Some(&(_, id)) = self.registered.iter().find(|(known, _)| known == name) {
            return Some(id);
        }
        let path = crate::app::models::model_path(&self.root, name)?;
        let id = moba_assets::load_glb_file(&path)
            .map_err(|error| error.to_string())
            .and_then(|data| unit_mesh(&data).ok_or_else(|| String::from("degenerate bounds")))
            .and_then(|data| {
                renderer
                    .register_mesh(&data)
                    .map_err(|error| error.to_string())
            });
        match id {
            Ok(id) => {
                self.registered.push((String::from(name), id));
                Some(id)
            }
            Err(error) => {
                eprintln!(
                    "[vfx] mesh {}: {error}; drawn as billboards",
                    path.display()
                );
                None
            }
        }
    }
}

/// `mesh` scaled and centred to fit a unit cube at the origin (a particle's `size` is then
/// its extent in metres).
pub(crate) fn unit_mesh(mesh: &MeshData) -> Option<MeshData> {
    let (min, max) = mesh.bounds();
    let extent = (0..3)
        .map(|axis| max[axis] - min[axis])
        .fold(0.0f32, f32::max);
    if !extent.is_finite() || extent <= 1e-5 {
        return None;
    }
    let centre = [0, 1, 2].map(|axis| min[axis].midpoint(max[axis]));
    let positions = mesh
        .positions()
        .iter()
        .map(|position| [0, 1, 2].map(|axis| (position[axis] - centre[axis]) / extent))
        .collect();
    MeshData::new(positions, mesh.normals().to_vec(), mesh.indices().to_vec()).ok()
}

/// File count and newest modification time below `dir` (not recursive).
fn stamp(dir: &Path) -> (usize, Option<SystemTime>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, None);
    };
    let mut count = 0;
    let mut newest = None;
    for entry in entries.flatten() {
        count += 1;
        if let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) {
            newest = newest.max(Some(modified));
        }
    }
    (count, newest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content_root() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content"))
    }

    /// The shipped sample set loads completely and covers every built-in and pack ability.
    #[test]
    fn shipped_effects_load_and_cover_the_shipped_abilities() {
        let library = VfxLibrary::load(content_root());
        let defaults = library.defaults();
        for (what, id) in [
            ("hero death", defaults.hero_death),
            ("minion death", defaults.minion_death),
            ("structure", defaults.structure_destroyed),
            ("attack", defaults.attack),
            ("recall", defaults.recall),
            ("cast", defaults.cast),
            ("projectile", defaults.projectile),
            ("hit", defaults.hit),
            ("zone", defaults.zone),
        ] {
            assert!(id.is_some(), "default {what} effect");
        }
        let mut names: Vec<String> = moba_game::GameContent::default()
            .abilities
            .iter()
            .map(|ability| ability.name.clone())
            .collect();
        for pack in ["ability-pack.toml", "example-pack.toml"] {
            let content =
                moba_game::load_content_file(&content_root().join(pack)).expect("pack loads");
            names.extend(content.abilities.iter().map(|ability| ability.name.clone()));
        }
        for name in names {
            let index = library
                .ability(&name)
                .unwrap_or_else(|| panic!("no VFX binding for {name}"));
            let effects = library.ability_effects(index).expect("binding");
            assert!(
                effects.cast.is_some() || effects.zone.is_some(),
                "{name} has no cast or zone effect"
            );
        }
        // Every bound file loaded (an invalid one would be missing from the table).
        let bindings =
            VfxBindings::load_file(&content_root().join("vfx/bindings.toml")).expect("bindings");
        assert_eq!(bindings.effect_names().count(), library.effect_count());
    }

    #[test]
    fn missing_or_invalid_files_resolve_to_no_effect() {
        let root = std::env::temp_dir().join(format!(
            "moba-vfx-lib-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("vfx")).expect("dir");
        std::fs::write(
            root.join("vfx/bindings.toml"),
            "[defaults]\nhero_death = \"good\"\nattack = \"broken\"\nhit = \"missing\"\n\
             [[abilities]]\nname = \"Nova\"\ncast = \"good\"\n",
        )
        .expect("bindings");
        std::fs::write(
            root.join("vfx/good.toml"),
            "duration = 0.5\n[[emitters]]\nburst = 4\nlifetime = [0.5, 1]\nsize = [1, 1]\n\
             color = [1, 1, 1, 1]\nprimitive = \"mesh\"\nmesh = \"cube\"\n",
        )
        .expect("effect");
        std::fs::write(root.join("vfx/broken.toml"), "duration = \"x\"").expect("effect");
        let library = VfxLibrary::load(root.clone());
        assert!(library.defaults().hero_death.is_some());
        assert!(library.defaults().attack.is_none());
        assert!(library.defaults().hit.is_none());
        assert_eq!(library.ability("NOVA"), Some(0));
        let good = library
            .effect(library.defaults().hero_death.expect("good"))
            .expect("loaded");
        assert_eq!(good.mesh_slots[0], Some(0));
        assert!(!library.changed_on_disk());
        let preview = VfxLibrary::preview(root.clone(), "good");
        assert_eq!(preview.preview_effect(), Some(0));
        let _ = std::fs::remove_dir_all(root);
        assert!(
            VfxLibrary::load(PathBuf::from("/nonexistent"))
                .defaults()
                .cast
                .is_none()
        );
    }

    #[test]
    fn meshes_are_fitted_to_a_unit_cube() {
        let mesh = MeshData::new(
            vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 2.0, 0.0]],
            vec![[0.0, 0.0, 1.0]; 3],
            vec![0, 1, 2],
        )
        .expect("mesh");
        let unit = unit_mesh(&mesh).expect("fits");
        let (min, max) = unit.bounds();
        assert_eq!(min, [-0.5, -0.25, 0.0]);
        assert_eq!(max, [0.5, 0.25, 0.0]);
    }
}
