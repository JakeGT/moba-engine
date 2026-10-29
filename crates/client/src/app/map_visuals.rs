//! Per-map visuals resolved from the client-local asset root (WP-3.3, #287).
//!
//! A map's visuals *name* (`MapSpec::visuals_name()` locally, `WireMap::visuals` or the
//! theme's built-in name from a server) resolves to the descriptor
//! `<MOBA_ASSET_ROOT>/maps/<name>.toml` ([`moba_assets::MapVisuals`]): terrain tint, wall
//! colour and height, an optional wall mesh and optional props. Meshes resolve like hero
//! models, to `<root>/models/<mesh>.glb`.
//!
//! Fallbacks, each logged once per name and never failing a match:
//! - no descriptor (or an invalid one): the built-in theme table ([`theme_visuals`]) for the
//!   map's theme id, exactly as before #287;
//! - a missing, malformed or unuploadable mesh: that wall or prop draws the built-in block
//!   with the descriptor's colours.
//!
//! Everything here is cosmetic and client-local: nothing feeds the simulation, the wire or
//! the content hash. Files are read and meshes uploaded only in [`MapLibrary::bind`]
//! (startup and content updates); the per-frame [`MapLibrary::push_instances`] copies a
//! prebuilt slice and does not allocate once `units` has capacity.

use std::path::{Path, PathBuf};

use moba_assets::{DEFAULT_WALL_HEIGHT, MapVisuals, valid_mesh_name};
use moba_core::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use moba_game::{MapSpec, MapTheme};
use moba_net::protocol::WireMap;
use moba_render::{MeshData, MeshId, UnitInstance};

use crate::app::models::model_path;

/// Directory under the asset root that holds map visuals descriptors.
pub(crate) const MAPS_DIR: &str = "maps";
/// Wall thickness in metres (the built-in walls' depth).
const WALL_THICKNESS: f32 = 2.5;
/// Descriptor and mesh caches are cleared past this many names (each content update can add
/// at most one descriptor and four meshes, so this only bounds a pathological server).
const MAX_CACHED: usize = 64;
/// Most renderer mesh slots (of 64, never freed) map meshes may take in one session, so a
/// server cycling through many maps cannot starve hero models.
const MAX_MAP_MESH_SLOTS: usize = 16;
/// Most wall instances built (the content and wire limit).
const MAX_WALLS: usize = moba_game::MAX_CONTENT_WALLS;

/// The built-in theme table: `(terrain tint, wall colour)` per wire theme id. It is the
/// fallback whenever a map has no usable descriptor.
pub(crate) fn theme_visuals(theme: u8) -> ([f32; 4], Vec4) {
    match theme {
        1 => ([0.20, 0.20, 0.19, 1.0], Vec4::new(0.34, 0.33, 0.30, 1.0)),
        2 => ([0.48, 0.56, 0.68, 1.0], Vec4::new(0.62, 0.68, 0.76, 1.0)),
        3 => ([0.34, 0.25, 0.14, 1.0], Vec4::new(0.48, 0.36, 0.22, 1.0)),
        4 => ([0.15, 0.08, 0.26, 1.0], Vec4::new(0.32, 0.24, 0.44, 1.0)),
        _ => ([0.10, 0.16, 0.11, 1.0], Vec4::new(0.24, 0.21, 0.16, 1.0)),
    }
}

/// `<root>/maps/<name>.toml`, or `None` for a name that is not 1-32 of `[A-Za-z0-9_-]` (so a
/// name can never leave the maps directory).
pub(crate) fn descriptor_path(root: &Path, name: &str) -> Option<PathBuf> {
    valid_mesh_name(name).then(|| root.join(MAPS_DIR).join(format!("{name}.toml")))
}

/// Loads the descriptor a visuals name resolves to under `root`.
///
/// # Errors
/// A readable message for an invalid name or a missing, oversized or invalid file.
pub(crate) fn resolve_map_visuals(root: &Path, name: &str) -> Result<MapVisuals, String> {
    let path = descriptor_path(root, name)
        .ok_or_else(|| String::from("invalid visuals name (use 1-32 of [A-Za-z0-9_-])"))?;
    MapVisuals::load_file(&path).map_err(|error| format!("{}: {error}", path.display()))
}

/// Which map to bind: everything [`MapLibrary::bind`] needs from a catalogue.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MapKey<'a> {
    /// Visuals name; `None` when the theme id is unknown and no name was given.
    pub(crate) visuals: Option<&'a str>,
    /// Wire theme id (the fallback).
    pub(crate) theme: u8,
    /// Half the map extent (world minus this centres the map on the origin).
    pub(crate) center: f32,
}

impl<'a> MapKey<'a> {
    /// The key of a server's map: its explicit visuals, else its theme's built-in name.
    pub(crate) fn from_wire(map: &'a WireMap) -> Self {
        Self {
            visuals: map
                .visuals_str()
                .or_else(|| MapTheme::from_id(map.theme).map(MapTheme::asset_name)),
            theme: map.theme,
            center: f32::from(map.extent) * 0.5,
        }
    }

    /// The key of a local map.
    pub(crate) fn from_spec(map: &'a MapSpec) -> Self {
        Self {
            visuals: Some(map.visuals_name()),
            theme: map.theme.id(),
            center: map.extent.to_f32() * 0.5,
        }
    }
}

/// Wall segments of a server's map, in world units (the wire carries whole units).
pub(crate) fn wire_walls(map: &WireMap) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
    map.walls
        .iter()
        .take(usize::from(map.wall_count))
        .map(|wall| {
            (
                Vec2::new(f32::from(wall.a[0]), f32::from(wall.a[1])),
                Vec2::new(f32::from(wall.b[0]), f32::from(wall.b[1])),
            )
        })
}

/// Wall segments of a local map. Validated content has whole-unit endpoints, so these are
/// the same values a server sends in its catalogue.
pub(crate) fn spec_walls(map: &MapSpec) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
    map.walls.iter().map(|wall| {
        (
            Vec2::new(wall.a.x.to_f32(), wall.a.y.to_f32()),
            Vec2::new(wall.b.x.to_f32(), wall.b.y.to_f32()),
        )
    })
}

/// A registered mesh and its model-space bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FittedMesh {
    mesh: MeshId,
    min: Vec3,
    max: Vec3,
}

impl FittedMesh {
    fn new(mesh: MeshId, data: &MeshData) -> Option<Self> {
        let (min, max) = data.bounds();
        let (min, max) = (Vec3::from_array(min), Vec3::from_array(max));
        let size = max - min;
        (size.is_finite() && size.y > 1e-4 && size.max_element() > 1e-4).then_some(Self {
            mesh,
            min,
            max,
        })
    }

    /// Maps the bounds onto the unit cube centred on the origin (the built-in block), so a
    /// wall mesh replaces the block under the same transform.
    fn unit_box(&self) -> Mat4 {
        let size = (self.max - self.min).max(Vec3::splat(1e-4));
        Mat4::from_scale(size.recip()) * Mat4::from_translation(-(self.min + self.max) * 0.5)
    }

    /// Scales uniformly to `height`, feet at `y = 0`, centred in x/z.
    fn standing(&self, height: f32) -> Mat4 {
        let size_y = (self.max.y - self.min.y).max(1e-4);
        let base = Vec3::new(
            self.min.x.midpoint(self.max.x),
            self.min.y,
            self.min.z.midpoint(self.max.z),
        );
        Mat4::from_scale(Vec3::splat(height / size_y)) * Mat4::from_translation(-base)
    }
}

/// The active map's look plus caches of descriptors and meshes.
#[derive(Debug)]
pub(crate) struct MapLibrary {
    root: PathBuf,
    /// Every visuals name tried, with its descriptor; failures are cached so each is logged
    /// once.
    descriptors: Vec<(String, Option<MapVisuals>)>,
    /// Every mesh name tried for this renderer.
    meshes: Vec<(String, Option<FittedMesh>)>,
    terrain: [f32; 4],
    /// Walls then props, in render space; rebuilt by `bind`.
    instances: Vec<UnitInstance>,
    /// Name of the descriptor in use, `None` for the theme table.
    active: Option<String>,
    /// Incremented by every `bind` (map changes are rare; wrapping is harmless).
    generation: u32,
}

impl MapLibrary {
    /// An empty library (theme-table grass, no walls). No I/O.
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            descriptors: Vec::new(),
            meshes: Vec::new(),
            terrain: theme_visuals(0).0,
            instances: Vec::with_capacity(MAX_WALLS + moba_assets::MAX_MAP_PROPS),
            active: None,
            generation: 0,
        }
    }

    /// Counts binds, so a caller can tell that the map changed.
    pub(crate) fn generation(&self) -> u32 {
        self.generation
    }

    /// Terrain tint for the scene. Never allocates.
    pub(crate) fn terrain_tint(&self) -> [f32; 4] {
        self.terrain
    }

    /// The descriptor name in use, or `None` when the theme table is.
    #[cfg(test)]
    pub(crate) fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// The map's walls and props, in render space (the minimap bake takes a bounded prefix).
    pub(crate) fn instances(&self) -> &[UnitInstance] {
        &self.instances
    }

    /// Appends the map's walls and props. Never allocates once `units` has capacity.
    pub(crate) fn push_instances(&self, units: &mut Vec<UnitInstance>) {
        units.extend_from_slice(&self.instances);
    }

    /// Forgets uploaded meshes (a new renderer starts with only the cube) and the instances
    /// that referenced them; call [`MapLibrary::bind`] again afterwards. Cached failures are
    /// kept, as in the model library.
    pub(crate) fn renderer_recreated(&mut self) {
        self.meshes.retain(|(_, mesh)| mesh.is_none());
        self.instances.clear();
    }

    /// Resolves `key`'s visuals and rebuilds the walls and props (startup and content
    /// update). `register` uploads a mesh (normally `Renderer::register_mesh`).
    pub(crate) fn bind<R>(
        &mut self,
        key: MapKey<'_>,
        walls: impl IntoIterator<Item = (Vec2, Vec2)>,
        mut register: R,
    ) where
        R: FnMut(&MeshData) -> Result<MeshId, String>,
    {
        let descriptor = key
            .visuals
            .and_then(|name| self.descriptor(name, key.theme));
        let (terrain, wall_color) = match &descriptor {
            Some(visuals) => (rgba(visuals.terrain), color(visuals.wall)),
            None => theme_visuals(key.theme),
        };
        let wall_height = descriptor
            .as_ref()
            .map_or(DEFAULT_WALL_HEIGHT, |visuals| visuals.wall_height);
        let wall_mesh = descriptor
            .as_ref()
            .and_then(|visuals| visuals.wall_mesh.as_deref())
            .and_then(|name| self.mesh(name, &mut register));
        self.terrain = terrain;
        self.active = descriptor.as_ref().and(key.visuals.map(str::to_owned));
        self.generation = self.generation.wrapping_add(1);
        self.instances.clear();
        let center = key.center;
        for (a, b) in walls.into_iter().take(MAX_WALLS) {
            let direction = b - a;
            let length = direction.length().max(0.5);
            let angle = direction.y.atan2(direction.x);
            let mid = (a + b) * 0.5;
            let transform = Mat4::from_scale_rotation_translation(
                Vec3::new(length, wall_height, WALL_THICKNESS),
                Quat::from_rotation_y(-angle),
                Vec3::new(mid.x - center, wall_height * 0.5, mid.y - center),
            );
            self.instances.push(match wall_mesh {
                Some(fitted) => UnitInstance::new(transform * fitted.unit_box(), wall_color)
                    .with_mesh(fitted.mesh),
                None => UnitInstance::new(transform, wall_color),
            });
        }
        let Some(visuals) = descriptor else {
            return;
        };
        for prop in &visuals.props {
            let ground = Vec3::new(prop.position[0] - center, 0.0, prop.position[1] - center);
            let rotation = Quat::from_rotation_y(-prop.yaw.to_radians());
            let prop_color = color(prop.color);
            let fitted = prop
                .mesh
                .as_deref()
                .and_then(|name| self.mesh(name, &mut register));
            self.instances.push(match fitted {
                Some(fitted) => UnitInstance::new(
                    Mat4::from_rotation_translation(rotation, ground)
                        * fitted.standing(prop.height),
                    prop_color,
                )
                .with_mesh(fitted.mesh),
                None => UnitInstance::new(
                    Mat4::from_scale_rotation_translation(
                        Vec3::new(prop.height * 0.45, prop.height, prop.height * 0.45),
                        rotation,
                        ground + Vec3::Y * (prop.height * 0.5),
                    ),
                    prop_color,
                ),
            });
        }
    }

    /// The cached descriptor for `name`, loading (and logging) it the first time.
    fn descriptor(&mut self, name: &str, theme: u8) -> Option<MapVisuals> {
        if let Some((_, cached)) = self.descriptors.iter().find(|(known, _)| known == name) {
            return cached.clone();
        }
        if self.descriptors.len() >= MAX_CACHED {
            self.descriptors.clear();
        }
        let fallback = MapTheme::from_id(theme).map_or("grass", theme_label);
        let loaded = match resolve_map_visuals(&self.root, name) {
            Ok(visuals) => {
                eprintln!(
                    "[assets] map visuals '{name}' loaded ({} props)",
                    visuals.props.len()
                );
                Some(visuals)
            }
            // A theme's own name with no file is the normal built-in case: stay quiet.
            Err(_)
                if MapTheme::from_id(theme).is_some_and(|theme| theme.asset_name() == name)
                    && descriptor_path(&self.root, name).is_some_and(|path| !path.exists()) =>
            {
                None
            }
            Err(reason) => {
                eprintln!(
                    "[assets] map visuals '{name}': {reason}; using the built-in {fallback} theme"
                );
                None
            }
        };
        self.descriptors.push((name.to_owned(), loaded.clone()));
        loaded
    }

    /// The cached mesh for `name`, loading and uploading it the first time.
    fn mesh<R>(&mut self, name: &str, register: &mut R) -> Option<FittedMesh>
    where
        R: FnMut(&MeshData) -> Result<MeshId, String>,
    {
        if let Some((_, cached)) = self.meshes.iter().find(|(known, _)| known == name) {
            return *cached;
        }
        if self.meshes.len() >= MAX_CACHED {
            self.meshes.retain(|(_, mesh)| mesh.is_some());
        }
        let uploaded = self
            .meshes
            .iter()
            .filter(|(_, mesh)| mesh.is_some())
            .count();
        let result = model_path(&self.root, name)
            .ok_or_else(|| String::from("invalid mesh name"))
            .and_then(|path| {
                // Mesh slots are never freed and are shared with hero models.
                if uploaded >= MAX_MAP_MESH_SLOTS {
                    Err(format!("map mesh budget ({MAX_MAP_MESH_SLOTS}) used"))
                } else {
                    Ok(path)
                }
            })
            .and_then(|path| {
                moba_assets::load_glb_file(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))
            })
            .and_then(|data| {
                // Check the extent first: an upload takes a slot that is never freed.
                FittedMesh::new(MeshId::CUBE, &data)
                    .ok_or_else(|| String::from("mesh has no extent"))?;
                let mesh = register(&data).map_err(|error| format!("upload failed: {error}"))?;
                FittedMesh::new(mesh, &data).ok_or_else(|| String::from("mesh has no extent"))
            });
        let fitted = match result {
            Ok(fitted) => Some(fitted),
            Err(reason) => {
                eprintln!("[assets] map mesh '{name}': {reason}; using the built-in block");
                None
            }
        };
        self.meshes.push((name.to_owned(), fitted));
        fitted
    }
}

/// Environment variable for [`SwitchCapture`].
pub(crate) const CAPTURE_MAPS_ENV: &str = "MOBA_CAPTURE_MAPS";

/// `MOBA_CAPTURE_MAPS=<prefix>` (network mode): one capture per map, so a single client
/// session can show maps rendering back to back across content reloads and rotations.
#[derive(Debug)]
pub(crate) struct SwitchCapture {
    prefix: PathBuf,
    generation: u32,
    frames: u64,
    written: u32,
}

impl SwitchCapture {
    pub(crate) fn new(prefix: PathBuf) -> Self {
        Self {
            prefix,
            generation: 0,
            frames: 0,
            written: 0,
        }
    }

    /// Reads [`CAPTURE_MAPS_ENV`]; `None` when unset or empty.
    pub(crate) fn from_env() -> Option<Self> {
        std::env::var_os(CAPTURE_MAPS_ENV)
            .filter(|value| !value.is_empty())
            .map(|value| Self::new(PathBuf::from(value)))
    }

    /// Call once per rendered frame that shows a snapshot. Returns the path to capture this
    /// frame to: the `after`-th such frame since the map last changed (`generation`).
    /// Allocates only when it returns a path.
    pub(crate) fn due(&mut self, generation: u32, after: u64) -> Option<PathBuf> {
        if generation != self.generation {
            self.generation = generation;
            self.frames = 0;
        }
        self.frames = self.frames.saturating_add(1);
        if self.frames != after.max(1) {
            return None;
        }
        self.written += 1;
        let mut name = self.prefix.clone().into_os_string();
        name.push(format!("-{}.ppm", self.written));
        Some(PathBuf::from(name))
    }
}

fn theme_label(theme: MapTheme) -> &'static str {
    theme.asset_name().trim_start_matches("map-")
}

fn rgba(rgb: [f32; 3]) -> [f32; 4] {
    [rgb[0], rgb[1], rgb[2], 1.0]
}

fn color(rgb: [f32; 3]) -> Vec4 {
    Vec4::new(rgb[0], rgb[1], rgb[2], 1.0)
}

/// Binds a server's map (startup and `ContentUpdated`).
pub(crate) fn bind_network_map(
    library: &mut MapLibrary,
    renderer: &mut moba_render::Renderer,
    map: &WireMap,
) {
    library.bind(MapKey::from_wire(map), wire_walls(map), |data| {
        renderer
            .register_mesh(data)
            .map_err(|error| error.to_string())
    });
}

/// Binds a local preview's map.
pub(crate) fn bind_local_map(
    library: &mut MapLibrary,
    renderer: &mut moba_render::Renderer,
    map: &MapSpec,
) {
    library.bind(MapKey::from_spec(map), spec_walls(map), |data| {
        renderer
            .register_mesh(data)
            .map_err(|error| error.to_string())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRYSTAL: &[u8] = include_bytes!("../../../../content/models/crystal.glb");

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "moba-map-visuals-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("models")).expect("models dir");
        std::fs::create_dir_all(root.join(MAPS_DIR)).expect("maps dir");
        root
    }

    fn key(visuals: Option<&str>, theme: u8) -> MapKey<'_> {
        MapKey {
            visuals,
            theme,
            center: 128.0,
        }
    }

    const WALL: [(Vec2, Vec2); 1] = [(Vec2::new(110.0, 100.0), Vec2::new(110.0, 156.0))];

    #[test]
    fn names_cannot_escape_the_maps_directory() {
        let root = Path::new("assets");
        assert_eq!(
            descriptor_path(root, "ember"),
            Some(PathBuf::from("assets/maps/ember.toml"))
        );
        for bad in ["", "../x", "a/b", "a\\b", "c:x", "x.toml", &"x".repeat(33)] {
            assert_eq!(descriptor_path(root, bad), None, "{bad}");
        }
    }

    /// Without a descriptor the look is exactly the pre-#287 theme table and wall box, so
    /// the default and networked goldens are unchanged.
    #[test]
    fn missing_descriptors_use_the_theme_table() {
        let root = temp_root("fallback");
        let mut library = MapLibrary::new(root.clone());
        let mut uploads = 0;
        for (visuals, theme) in [(Some("map-grass"), 0), (Some("lava"), 3), (None, 9)] {
            library.bind(key(visuals, theme), WALL, |_: &MeshData| {
                uploads += 1;
                Ok(MeshId::CUBE)
            });
            let (terrain, wall) = theme_visuals(theme);
            assert_eq!(library.terrain_tint(), terrain);
            assert_eq!(library.active(), None);
            let mut units = Vec::new();
            library.push_instances(&mut units);
            assert_eq!(units.len(), 1);
            let expected = Mat4::from_scale_rotation_translation(
                Vec3::new(56.0, 4.0, 2.5),
                Quat::from_rotation_y(-core::f32::consts::FRAC_PI_2),
                Vec3::new(110.0 - 128.0, 2.0, 128.0 - 128.0),
            );
            assert_eq!(units[0], UnitInstance::new(expected, wall));
        }
        assert_eq!(uploads, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn descriptors_style_walls_and_props_and_meshes_fall_back_per_part() {
        let root = temp_root("descriptor");
        std::fs::write(root.join("models/crystal.glb"), CRYSTAL).expect("write mesh");
        std::fs::write(
            root.join("maps/ember.toml"),
            "terrain = [0.3, 0.1, 0.05]\nwall = [0.6, 0.2, 0.1]\nwall_height = 6\n\
             wall_mesh = \"missing\"\n\
             [[props]]\nposition = [96, 60]\ncolor = [1, 0.4, 0.1]\nheight = 5\nmesh = \"crystal\"\n\
             [[props]]\nposition = [160, 196]\ncolor = [1, 0.4, 0.1]\nmesh = \"crystal\"\n\
             [[props]]\nposition = [128, 128]\ncolor = [0.2, 0.2, 0.2]\n",
        )
        .expect("write descriptor");
        let mut library = MapLibrary::new(root.clone());
        let mut uploads = 0;
        let mut register = |_: &MeshData| {
            uploads += 1;
            Ok(MeshId::CUBE)
        };
        library.bind(key(Some("ember"), 3), WALL, &mut register);
        assert_eq!(library.active(), Some("ember"));
        assert_eq!(library.terrain_tint(), [0.3, 0.1, 0.05, 1.0]);
        let mut units = Vec::new();
        library.push_instances(&mut units);
        assert_eq!(units.len(), 4, "one wall and three props");
        // The wall mesh is missing: the block, but with the descriptor's colour and height.
        assert_eq!(units[0].color, Vec4::new(0.6, 0.2, 0.1, 1.0));
        let top = units[0]
            .transform
            .transform_point3(Vec3::new(0.0, 0.5, 0.0));
        assert!((top.y - 6.0).abs() < 1e-5, "{top:?}");
        // The crystal stands on the ground at its height.
        let (min, max) = moba_assets::load_glb(CRYSTAL).expect("crystal").bounds();
        let crown = units[1]
            .transform
            .transform_point3(Vec3::new(0.0, max[1], 0.0));
        let foot = units[1]
            .transform
            .transform_point3(Vec3::new(0.0, min[1], 0.0));
        assert!((crown.y - 5.0).abs() < 1e-4 && foot.y.abs() < 1e-5);
        assert!((foot.x - (96.0 - 128.0)).abs() < 1.0);
        // Rebinding (rotation back to this map) reuses both caches.
        library.bind(key(Some("ember"), 3), WALL, &mut register);
        assert_eq!(
            uploads, 1,
            "one crystal upload; the missing mesh is not retried"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn two_maps_switch_back_to_back_and_invalid_descriptors_fall_back() {
        let root = temp_root("switch");
        std::fs::write(
            root.join("maps/ember.toml"),
            "terrain = [0.3, 0.1, 0.05]\nwall = [0.6, 0.2, 0.1]\n",
        )
        .expect("write");
        std::fs::write(
            root.join("maps/frost.toml"),
            "terrain = [0.5, 0.6, 0.7]\nwall = [0.8, 0.9, 1.0]\nwall_height = 8\n",
        )
        .expect("write");
        std::fs::write(root.join("maps/broken.toml"), "terrain = [9, 9, 9]").expect("write");
        let mut library = MapLibrary::new(root.clone());
        let register = |_: &MeshData| Ok(MeshId::CUBE);
        library.bind(key(Some("ember"), 3), WALL, register);
        assert_eq!(library.terrain_tint(), [0.3, 0.1, 0.05, 1.0]);
        library.bind(key(Some("frost"), 2), [], register);
        assert_eq!(library.active(), Some("frost"));
        assert_eq!(library.terrain_tint(), [0.5, 0.6, 0.7, 1.0]);
        let mut units = Vec::new();
        library.push_instances(&mut units);
        assert!(units.is_empty(), "the second map has no walls");
        library.bind(key(Some("broken"), 1), WALL, register);
        assert_eq!(library.active(), None);
        assert_eq!(library.terrain_tint(), theme_visuals(1).0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn local_and_wire_maps_build_the_same_walls() {
        let content =
            moba_game::load_content_toml(include_str!("../../../../content/map-pack.toml"))
                .expect("map pack");
        let wire = moba_net::protocol::WireCatalog::from(&content).map;
        let local: Vec<_> = spec_walls(&content.map).collect();
        let remote: Vec<_> = wire_walls(&wire).collect();
        assert_eq!(local.len(), 1);
        assert_eq!(local, remote);
        let local_key = MapKey::from_spec(&content.map);
        let wire_key = MapKey::from_wire(&wire);
        assert_eq!(local_key.visuals, wire_key.visuals);
        assert_eq!(local_key.theme, wire_key.theme);
        assert_eq!(local_key.center, wire_key.center);
    }

    #[test]
    fn map_meshes_have_a_slot_budget() {
        let root = temp_root("budget");
        let mut library = MapLibrary::new(root.clone());
        let mut uploads = 0;
        let mut register = |_: &MeshData| {
            uploads += 1;
            Ok(MeshId::CUBE)
        };
        for index in 0..=MAX_MAP_MESH_SLOTS {
            let name = format!("m{index}");
            std::fs::write(root.join(format!("models/{name}.glb")), CRYSTAL).expect("write");
            let fitted = library.mesh(&name, &mut register);
            assert_eq!(fitted.is_some(), index < MAX_MAP_MESH_SLOTS, "{name}");
        }
        assert_eq!(uploads, MAX_MAP_MESH_SLOTS);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn switch_capture_fires_once_per_map() {
        let mut capture = SwitchCapture::new(PathBuf::from("out/map"));
        let mut paths = Vec::new();
        for generation in [1, 1, 1, 1, 2, 2, 2, 2, 2] {
            paths.extend(capture.due(generation, 3));
        }
        assert_eq!(
            paths,
            [
                PathBuf::from("out/map-1.ppm"),
                PathBuf::from("out/map-2.ppm")
            ]
        );
    }

    #[test]
    fn pushing_instances_does_not_allocate() {
        let root = temp_root("alloc");
        let mut library = MapLibrary::new(root.clone());
        library.bind(key(Some("map-grass"), 0), WALL, |_: &MeshData| {
            Ok(MeshId::CUBE)
        });
        let mut units = Vec::with_capacity(64);
        let ((), allocations) = crate::app::allocation_probe::probe(|| {
            for _ in 0..100 {
                units.clear();
                library.push_instances(&mut units);
                let _ = library.terrain_tint();
            }
        });
        assert_eq!(allocations, 0);
        let _ = std::fs::remove_dir_all(root);
    }
}
