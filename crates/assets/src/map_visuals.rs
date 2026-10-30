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
//! baked_ao = "map-grass-ao"      # <root>/maps/ao/map-grass-ao.{toml,ktx2} (ADR-0016)
//! ao_strength = 1.0              # baked AO strength, 0..=1 (default 1)
//!
//! [atmosphere]                   # optional height fog and sky, every key optional
//! fog_color = [0.5, 0.6, 0.7]    # linear RGB 0..=4
//! fog_density = 0.012            # per metre at the ground, 0..=0.2
//! fog_height = 40.0              # height falloff scale, metres, 1..=400
//! fog_start = 60.0               # metres before fog begins, 0..=2000
//! fog_max = 0.7                  # maximum opacity, 0..=0.85
//! sky_zenith = [0.2, 0.3, 0.6]   # linear RGB 0..=8
//! sky_horizon = [0.6, 0.65, 0.7] # linear RGB 0..=8
//!
//! [fog_of_war]                   # optional fog-of-war style, every key optional
//! color = [0.02, 0.03, 0.05]     # linear RGB 0..=1
//! darken = 0.72                  # 0.2..=0.9
//! desaturate = 0.5               # 0..=1
//! mist_height = 3.0              # metres, 0..=32: ground mist in unseen areas
//! mist = 0.4                     # 0..=1
//!
//! [post]                         # optional post-processing (R6), every key optional
//! exposure = 0.0                 # exposure compensation, EV stops, -4..=4 (default 0)
//! auto_exposure = true           # opt in to eye adaptation (default off)
//! exposure_min = -1.0            # auto-exposure EV clamp, -8..=8 (default -2)
//! exposure_max = 1.0             # -8..=8, >= exposure_min (default 2)
//! adaptation = 1.5               # adaptation rate per second, 0.1..=10 (default 1.5)
//! bloom = 0.05                   # bloom strength, 0..=0.1 (default 0.04)
//! grading = "ember"              # <root>/grading/ember.cube colour-grading LUT
//! grading_strength = 0.9         # 0..=1 blend towards the graded colour
//! ```
//!
//! For a heightfield terrain (ADR-0014), `terrain` is a table instead of the tint, with the
//! tint inside it (a TOML key cannot be both):
//!
//! ```toml
//! wall = [0.58, 0.22, 0.10]
//!
//! [terrain]                      # replaces `terrain = [...]`; without it, the flat plane
//! tint = [0.30, 0.11, 0.06]      # required: the flat plane's tint (the fallback)
//! relief = 0.35                  # board relief amplitude, metres, 0..=0.5 (default 0.25)
//! edge = "cliffs"                # "none" | "hills" (default) | "cliffs" outside the board
//! edge_height = 22.0             # metres, 0..=48 (default 18)
//! edge_mesh = "rampart"          # optional <root>/models/<name>.glb around the board edge
//! edge_spacing = 24.0            # metres between edge props, 8..=128 (default 24)
//! edge_mesh_height = 10.0        # metres, 1..=48 (default 10)
//! lane_width = 12.0              # generated path width along lanes, 2..=40 (default 12)
//! seed = 3                       # u32 noise seed (default 1)
//! splat = "ember-splat"          # optional <root>/textures/<name>.ktx2, else generated
//! splat2 = "ember-splat2"        # optional: layers 5-8 (needs splat and > 4 layers)
//!
//! [[terrain.layers]]             # 1..=8; by position: base, path, rock, variation, extras
//! albedo = "terrain-grass"       # <root>/textures/<name>.ktx2 (sRGB colour, AO in alpha)
//! normal = "terrain-grass-normal"  # optional <root>/textures/<name>.ktx2 (BC5/RG8)
//! tint = [0.55, 0.75, 0.45]      # linear RGB multiplier 0..=2 (default 1)
//! tiling = 6.0                   # metres per texture repeat, 0.5..=64 (default 8)
//! ```
//!
//! The `[terrain]` generator and the visual-versus-simulation consistency rules live in
//! [`crate::generate_terrain`] and [`crate::check_map_consistency`].
//!
//! With a `[terrain]` table, a descriptor may add foliage and shallow water (ADR-0017):
//!
//! ```toml
//! [foliage]                      # optional; every key optional
//! density = 1.5                  # grass clumps per 10 m² of board, 0..=8
//! height = [0.3, 0.6]            # clump height range, metres, 0.1..=1.5
//! base_color = [0.05, 0.16, 0.04]  # linear RGB 0..=1 at the root
//! tip_color = [0.35, 0.55, 0.18]   # at the tip
//! flower_color = [0.9, 0.75, 0.3]
//! wind = [0.8, 0.3]              # direction times strength, length 0..=4
//! wind_speed = 1.2               # gust speed, 0..=8
//! reeds = true                   # reeds line the water's banks
//! seed = 1                       # u32 placement seed
//!
//! [[foliage.patches]]            # optional, at most 32
//! center = [96, 140]             # world units
//! radius = 8.0                   # metres, 1..=60
//! kind = "brush"                 # grass | brush | reeds | flowers
//! density = 4.0                  # clumps per 10 m², 0..=20
//!
//! [[water]]                      # optional, at most 8 bodies
//! points = [[40, 90], [128, 120], [210, 150]]  # 1..=32; one point is a round pool
//! width = 12.0                   # metres, 2..=60
//! level = 0.1                    # surface height, metres, -0.5..=0.3
//! depth = 0.45                   # bed depth below the surface, 0.05..=0.8
//!                                # (level - depth >= -0.5: the bed stays in the relief limit)
//! flow = 1.0                     # flow speed along the points, metres per second, 0..=8
//! color = [0.02, 0.12, 0.16]     # deep colour, linear RGB 0..=1
//! shallow_color = [0.10, 0.30, 0.30]
//! clarity = 0.35                 # metres of depth per e-fold of the deep colour, 0.05..=4
//! foam = 0.6                     # shoreline foam, 0..=1
//! ```
//!
//! [`crate::generate_foliage`], [`crate::generate_water`] and
//! [`crate::generate_terrain_with_water`] turn these into instances, the water mesh and the
//! carved bed.
//!
//! Descriptors are untrusted, client-local and cosmetic: they never reach the simulation, the
//! wire or the content hash. [`MapVisuals::load_file`] reads at most
//! [`MAX_MAP_VISUALS_BYTES`], rejects unknown keys, and range-checks every number, so a
//! hostile file can only fail to load (the client then falls back to the built-in theme).

use std::io::Read;
use std::path::Path;

use serde::Deserialize;

use crate::terrain::{MAX_EDGE_HEIGHT, MAX_PLAY_RELIEF, MAX_TERRAIN_LAYERS};

/// Largest descriptor file accepted, in bytes.
pub const MAX_MAP_VISUALS_BYTES: u64 = 32 * 1024;
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
const MAX_BLOOM_STRENGTH: f32 = 0.1;

/// Board relief amplitude when `[terrain]` sets none, metres.
pub const DEFAULT_TERRAIN_RELIEF: f32 = 0.25;
/// Edge height when `[terrain]` sets none, metres.
pub const DEFAULT_EDGE_HEIGHT: f32 = 18.0;
/// Distance between edge props when `[terrain]` sets none, metres.
pub const DEFAULT_EDGE_SPACING: f32 = 24.0;
/// Edge prop height when `[terrain]` sets none, metres.
pub const DEFAULT_EDGE_MESH_HEIGHT: f32 = 10.0;
/// Width of the generated path along lanes when `[terrain]` sets none, metres.
pub const DEFAULT_LANE_WIDTH: f32 = 12.0;
/// Terrain noise seed when `[terrain]` sets none.
pub const DEFAULT_TERRAIN_SEED: u32 = 1;
/// Metres per texture repeat when a terrain layer sets none.
pub const DEFAULT_LAYER_TILING: f32 = 8.0;
/// Highest accepted `[atmosphere] fog_max`: height fog never fully hides a unit.
pub const MAX_FOG_OPACITY: f32 = 0.85;
/// Accepted `[fog_of_war] darken` range: below it the fog boundary is hard to see, above it
/// unseen ground stops being readable.
pub const FOW_DARKEN_RANGE: (f32, f32) = (0.2, 0.9);

pub(crate) const EDGE_SPACING_RANGE: (f32, f32) = (8.0, 128.0);
pub(crate) const EDGE_MESH_HEIGHT_RANGE: (f32, f32) = (1.0, MAX_EDGE_HEIGHT);
pub(crate) const LANE_WIDTH_RANGE: (f32, f32) = (2.0, 40.0);
const LAYER_TILING_RANGE: (f32, f32) = (0.5, 64.0);
const WALL_HEIGHT_RANGE: (f32, f32) = (0.5, 32.0);
const PROP_HEIGHT_RANGE: (f32, f32) = (0.1, 32.0);
const YAW_RANGE: (f32, f32) = (-360.0, 360.0);
/// Prop positions are world units; anything a `u16` map extent can hold, with margin.
const POSITION_RANGE: (f32, f32) = (-65_536.0, 65_536.0);

/// A validated map visuals descriptor.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MapVisuals {
    /// Terrain tint, linear RGB in `0..=1`: the flat plane's colour (the top-level
    /// `terrain = [r, g, b]`, or `tint` in a `[terrain]` table).
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
    /// Heightfield terrain (the `[terrain]` table, ADR-0014); `None` (the top-level
    /// `terrain = [r, g, b]` form) keeps the flat R2a plane.
    pub ground: Option<MapTerrain>,
    /// Height fog and sky (`[atmosphere]`); unset keys keep the engine defaults.
    pub atmosphere: MapAtmosphere,
    /// Fog-of-war styling (`[fog_of_war]`); unset keys keep the engine defaults.
    pub fog_of_war: MapFogStyle,
    /// Post-processing overrides (`[post]`); unset keys keep the engine defaults.
    pub post: MapPost,
    /// Grass, brush, reeds and flowers (`[foliage]`, ADR-0017); needs `ground`.
    pub foliage: Option<MapFoliage>,
    /// Shallow rivers and pools (`[[water]]`, ADR-0017), in file order; needs `ground`.
    pub water: Vec<MapWater>,
}

/// Most `[[water]]` bodies one descriptor may have.
pub const MAX_MAP_WATER: usize = 8;
/// Most centre-line points one water body may have.
pub const MAX_WATER_POINTS: usize = 32;
/// Highest water surface above the ground units stand on (height 0), metres.
pub const MAX_WATER_LEVEL: f32 = 0.3;
/// Deepest water allowed above the carved bed, metres: a 2 m hero visibly wades, never swims.
pub const MAX_WADE_DEPTH: f32 = 0.8;
/// Most `[[foliage.patches]]` one descriptor may have.
pub const MAX_FOLIAGE_PATCHES: usize = 32;

pub(crate) const WATER_WIDTH_RANGE: (f32, f32) = (2.0, 60.0);
pub(crate) const WATER_DEPTH_RANGE: (f32, f32) = (0.05, MAX_WADE_DEPTH);
const WATER_FLOW_RANGE: (f32, f32) = (0.0, 8.0);
const WATER_CLARITY_RANGE: (f32, f32) = (0.05, 4.0);
const FOLIAGE_DENSITY_RANGE: (f32, f32) = (0.0, 8.0);
const FOLIAGE_HEIGHT_RANGE: (f32, f32) = (0.1, 1.5);
const PATCH_RADIUS_RANGE: (f32, f32) = (1.0, 60.0);
const PATCH_DENSITY_RANGE: (f32, f32) = (0.0, 20.0);
const MAX_WIND: f32 = 4.0;

/// One shallow water body (`[[water]]`, ADR-0017 §3): a river along a centre polyline, or a
/// round pool around a single point. Values built in code are not range-checked; the
/// generators clamp them to the limits the parser enforces.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MapWater {
    /// Centre polyline, world units (simulation coordinates); one point is a round pool.
    pub points: Vec<[f32; 2]>,
    /// Width across the river (the pool's diameter), metres, `2..=60`.
    pub width: f32,
    /// Surface height, metres, `-0.5..=`[`MAX_WATER_LEVEL`].
    pub level: f32,
    /// Bed depth below the surface at the centre line, metres, `0.05..=`[`MAX_WADE_DEPTH`].
    pub depth: f32,
    /// Flow speed along the polyline, metres per second, `0..=8` (pools do not flow).
    pub flow: f32,
    /// Deep colour, linear RGB `0..=1`.
    pub color: [f32; 3],
    /// Shallow colour, linear RGB `0..=1`.
    pub shallow_color: [f32; 3],
    /// Metres of depth per e-fold towards the deep colour, `0.05..=4`.
    pub clarity: f32,
    /// Shoreline foam strength, `0..=1`.
    pub foam: f32,
}

impl MapWater {
    /// Width when a body sets none, metres.
    pub const DEFAULT_WIDTH: f32 = 12.0;
    /// Surface level when a body sets none, metres.
    pub const DEFAULT_LEVEL: f32 = 0.1;
    /// Bed depth when a body sets none, metres.
    pub const DEFAULT_DEPTH: f32 = 0.45;
    /// Flow speed when a body sets none, metres per second.
    pub const DEFAULT_FLOW: f32 = 1.0;
    /// Deep colour when a body sets none.
    pub const DEFAULT_COLOR: [f32; 3] = [0.02, 0.12, 0.16];
    /// Shallow colour when a body sets none.
    pub const DEFAULT_SHALLOW_COLOR: [f32; 3] = [0.10, 0.30, 0.30];
    /// Clarity when a body sets none, metres.
    pub const DEFAULT_CLARITY: f32 = 0.35;
    /// Foam when a body sets none.
    pub const DEFAULT_FOAM: f32 = 0.6;

    /// A body along `points` with every other value at its default.
    #[must_use]
    pub fn new(points: Vec<[f32; 2]>) -> Self {
        Self {
            points,
            width: Self::DEFAULT_WIDTH,
            level: Self::DEFAULT_LEVEL,
            depth: Self::DEFAULT_DEPTH,
            flow: Self::DEFAULT_FLOW,
            color: Self::DEFAULT_COLOR,
            shallow_color: Self::DEFAULT_SHALLOW_COLOR,
            clarity: Self::DEFAULT_CLARITY,
            foam: Self::DEFAULT_FOAM,
        }
    }

    /// Returns this body with a width.
    #[must_use]
    pub fn with_width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Returns this body with a surface level and bed depth.
    #[must_use]
    pub fn with_level(mut self, level: f32, depth: f32) -> Self {
        self.level = level;
        self.depth = depth;
        self
    }

    /// Returns this body with a flow speed.
    #[must_use]
    pub fn with_flow(mut self, flow: f32) -> Self {
        self.flow = flow;
        self
    }

    /// Returns this body with its deep and shallow colours.
    #[must_use]
    pub fn with_colors(mut self, color: [f32; 3], shallow_color: [f32; 3]) -> Self {
        self.color = color;
        self.shallow_color = shallow_color;
        self
    }

    /// Returns this body with a clarity and foam strength.
    #[must_use]
    pub fn with_style(mut self, clarity: f32, foam: f32) -> Self {
        self.clarity = clarity;
        self.foam = foam;
        self
    }

    /// Half the width, clamped to the parser's range (the generators' value).
    pub(crate) fn half_width(&self) -> f32 {
        0.5 * crate::terrain::finite_clamp(
            self.width,
            WATER_WIDTH_RANGE.0,
            WATER_WIDTH_RANGE.1,
            Self::DEFAULT_WIDTH,
        )
    }

    /// Surface level, clamped to the parser's range (the generators' value).
    pub(crate) fn surface(&self) -> f32 {
        crate::terrain::finite_clamp(
            self.level,
            -MAX_PLAY_RELIEF,
            MAX_WATER_LEVEL,
            Self::DEFAULT_LEVEL,
        )
    }

    /// Bed depth, clamped to the parser's range (the generators' value).
    pub(crate) fn bed_depth(&self) -> f32 {
        crate::terrain::finite_clamp(
            self.depth,
            WATER_DEPTH_RANGE.0,
            WATER_DEPTH_RANGE.1,
            Self::DEFAULT_DEPTH,
        )
    }

    /// The centre points with finite coordinates, at most [`MAX_WATER_POINTS`].
    pub(crate) fn finite_points(&self) -> impl Iterator<Item = [f32; 2]> + '_ {
        self.points
            .iter()
            .copied()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .take(MAX_WATER_POINTS)
    }
}

/// What a foliage clump is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum FoliageKind {
    /// Short grass tufts (the generated scatter).
    #[default]
    Grass,
    /// Waist-high brush (decorative only: the simulation has no brush areas).
    Brush,
    /// Tall reeds (along water banks).
    Reeds,
    /// Grass with flower heads.
    Flowers,
}

impl FoliageKind {
    /// Every kind, in [`Self::index`] order.
    pub const ALL: [Self; 4] = [Self::Grass, Self::Brush, Self::Reeds, Self::Flowers];

    /// The GPU kind index: grass 0, brush 1, reeds 2, flowers 3.
    #[must_use]
    pub const fn index(self) -> u32 {
        match self {
            Self::Grass => 0,
            Self::Brush => 1,
            Self::Reeds => 2,
            Self::Flowers => 3,
        }
    }

    /// The descriptor spelling (`"grass"`, `"brush"`, `"reeds"`, `"flowers"`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Grass => "grass",
            Self::Brush => "brush",
            Self::Reeds => "reeds",
            Self::Flowers => "flowers",
        }
    }

    /// Parses the descriptor spelling.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// One `[[foliage.patches]]` entry: extra clumps of one kind in a disc.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct FoliagePatch {
    /// Disc centre, world units.
    pub center: [f32; 2],
    /// Disc radius, metres, `1..=60`.
    pub radius: f32,
    /// Kind of every clump in the patch.
    pub kind: FoliageKind,
    /// Clumps per 10 m², `0..=20`.
    pub density: f32,
}

impl FoliagePatch {
    /// Patch density when a patch sets none, clumps per 10 m².
    pub const DEFAULT_DENSITY: f32 = 4.0;

    /// A patch of `kind` at the default density.
    #[must_use]
    pub fn new(center: [f32; 2], radius: f32, kind: FoliageKind) -> Self {
        Self {
            center,
            radius,
            kind,
            density: Self::DEFAULT_DENSITY,
        }
    }

    /// Returns this patch with a density.
    #[must_use]
    pub fn with_density(mut self, density: f32) -> Self {
        self.density = density;
        self
    }
}

/// The `[foliage]` table (ADR-0017 §2): the generated grass scatter, reeds along water and
/// authored patches. Values built in code are not range-checked; [`crate::generate_foliage`]
/// clamps them to the limits the parser enforces.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MapFoliage {
    /// Grass clumps per 10 m² of board, `0..=8`.
    pub density: f32,
    /// Clump height range, metres, `0.1..=1.5`, low first.
    pub height: [f32; 2],
    /// Colour at the root, linear RGB `0..=1`.
    pub base_color: [f32; 3],
    /// Colour at the tip, linear RGB `0..=1`.
    pub tip_color: [f32; 3],
    /// Flower head colour, linear RGB `0..=1`.
    pub flower_color: [f32; 3],
    /// Wind direction times strength (simulation `x`, `y`), length `0..=4`.
    pub wind: [f32; 2],
    /// Gust speed, `0..=8`.
    pub wind_speed: f32,
    /// Reeds line the water's banks.
    pub reeds: bool,
    /// Placement seed (combined with the terrain seed).
    pub seed: u32,
    /// Authored patches, in file order (at most [`MAX_FOLIAGE_PATCHES`]).
    pub patches: Vec<FoliagePatch>,
}

impl Default for MapFoliage {
    fn default() -> Self {
        Self {
            density: Self::DEFAULT_DENSITY,
            height: Self::DEFAULT_HEIGHT,
            base_color: Self::DEFAULT_BASE_COLOR,
            tip_color: Self::DEFAULT_TIP_COLOR,
            flower_color: Self::DEFAULT_FLOWER_COLOR,
            wind: Self::DEFAULT_WIND,
            wind_speed: Self::DEFAULT_WIND_SPEED,
            reeds: true,
            seed: 1,
            patches: Vec::new(),
        }
    }
}

impl MapFoliage {
    /// Scatter density when `[foliage]` sets none, clumps per 10 m².
    pub const DEFAULT_DENSITY: f32 = 1.5;
    /// Clump height range when `[foliage]` sets none, metres.
    pub const DEFAULT_HEIGHT: [f32; 2] = [0.3, 0.6];
    /// Root colour when `[foliage]` sets none.
    pub const DEFAULT_BASE_COLOR: [f32; 3] = [0.05, 0.16, 0.04];
    /// Tip colour when `[foliage]` sets none.
    pub const DEFAULT_TIP_COLOR: [f32; 3] = [0.35, 0.55, 0.18];
    /// Flower colour when `[foliage]` sets none.
    pub const DEFAULT_FLOWER_COLOR: [f32; 3] = [0.9, 0.75, 0.3];
    /// Wind when `[foliage]` sets none.
    pub const DEFAULT_WIND: [f32; 2] = [0.8, 0.3];
    /// Gust speed when `[foliage]` sets none.
    pub const DEFAULT_WIND_SPEED: f32 = 1.2;

    /// Returns this table with a scatter density.
    #[must_use]
    pub fn with_density(mut self, density: f32) -> Self {
        self.density = density;
        self
    }

    /// Returns this table with reeds along water on or off.
    #[must_use]
    pub fn with_reeds(mut self, reeds: bool) -> Self {
        self.reeds = reeds;
        self
    }

    /// Returns this table with a placement seed.
    #[must_use]
    pub fn with_seed(mut self, seed: u32) -> Self {
        self.seed = seed;
        self
    }

    /// Returns this table with one more patch.
    #[must_use]
    pub fn with_patch(mut self, patch: FoliagePatch) -> Self {
        self.patches.push(patch);
        self
    }
}

/// How the ground outside the board rises (`[terrain] edge`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum EdgeStyle {
    /// A flat apron (tiny non-negative noise only).
    None,
    /// Rolling hills rising over a third of the margin.
    #[default]
    Hills,
    /// A steep, terraced rise a few metres past the board edge.
    Cliffs,
}

impl EdgeStyle {
    /// The descriptor spelling (`"none"`, `"hills"`, `"cliffs"`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Hills => "hills",
            Self::Cliffs => "cliffs",
        }
    }

    /// Parses the descriptor spelling.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "none" => Some(Self::None),
            "hills" => Some(Self::Hills),
            "cliffs" => Some(Self::Cliffs),
            _ => None,
        }
    }
}

/// One terrain texture layer (`[[terrain.layers]]`). Its role comes from its position (see
/// [`crate::generate_splat`]).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct TerrainLayer {
    /// Albedo texture name: `<root>/textures/<name>.ktx2`, sRGB colour with AO in alpha.
    pub albedo: String,
    /// Optional normal map name: `<root>/textures/<name>.ktx2`, BC5 or RG8.
    pub normal: Option<String>,
    /// Linear RGB multiplier, `0..=2`.
    pub tint: [f32; 3],
    /// Metres per texture repeat, `0.5..=64`.
    pub tiling: f32,
}

impl TerrainLayer {
    /// A layer with `albedo`, no normal map, a white tint and the default tiling.
    #[must_use]
    pub fn new(albedo: impl Into<String>) -> Self {
        Self {
            albedo: albedo.into(),
            normal: None,
            tint: [1.0; 3],
            tiling: DEFAULT_LAYER_TILING,
        }
    }

    /// Returns this layer with a normal map.
    #[must_use]
    pub fn with_normal(mut self, normal: impl Into<String>) -> Self {
        self.normal = Some(normal.into());
        self
    }

    /// Returns this layer with a tint.
    #[must_use]
    pub fn with_tint(mut self, tint: [f32; 3]) -> Self {
        self.tint = tint;
        self
    }

    /// Returns this layer with a tiling distance in metres.
    #[must_use]
    pub fn with_tiling(mut self, tiling: f32) -> Self {
        self.tiling = tiling;
        self
    }
}

/// The `[terrain]` table of a map visuals descriptor (ADR-0014): layers, an optional splat
/// map and the generator's parameters. Values built in code are not range-checked; the
/// generator clamps them to the limits the parser enforces.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MapTerrain {
    /// 1..=[`MAX_TERRAIN_LAYERS`] layers, in role order.
    pub layers: Vec<TerrainLayer>,
    /// Splat map for layers 1-4 (`<root>/textures/<name>.ktx2`, linear RGBA); `None` means
    /// the generator's.
    pub splat: Option<String>,
    /// Splat map for layers 5-8; only with `splat` and more than four layers.
    pub splat2: Option<String>,
    /// Board relief amplitude, metres, `0..=`[`MAX_PLAY_RELIEF`].
    pub relief: f32,
    /// How the ground outside the board rises.
    pub edge: EdgeStyle,
    /// Height of the edge rise, metres, `0..=`[`MAX_EDGE_HEIGHT`].
    pub edge_height: f32,
    /// Optional mesh (`<root>/models/<name>.glb`) placed around the board edge.
    pub edge_mesh: Option<String>,
    /// Metres between edge props, `8..=128`.
    pub edge_spacing: f32,
    /// Edge prop height, metres, `1..=48`.
    pub edge_mesh_height: f32,
    /// Width of the generated path along lanes, metres, `2..=40`.
    pub lane_width: f32,
    /// Noise seed.
    pub seed: u32,
}

impl MapTerrain {
    /// Terrain with these layers and every other value at its default.
    #[must_use]
    pub fn new(layers: Vec<TerrainLayer>) -> Self {
        Self {
            layers,
            splat: None,
            splat2: None,
            relief: DEFAULT_TERRAIN_RELIEF,
            edge: EdgeStyle::default(),
            edge_height: DEFAULT_EDGE_HEIGHT,
            edge_mesh: None,
            edge_spacing: DEFAULT_EDGE_SPACING,
            edge_mesh_height: DEFAULT_EDGE_MESH_HEIGHT,
            lane_width: DEFAULT_LANE_WIDTH,
            seed: DEFAULT_TERRAIN_SEED,
        }
    }

    /// Returns this terrain with a splat map for layers 1-4.
    #[must_use]
    pub fn with_splat(mut self, splat: impl Into<String>) -> Self {
        self.splat = Some(splat.into());
        self
    }

    /// Returns this terrain with a splat map for layers 5-8.
    #[must_use]
    pub fn with_splat2(mut self, splat2: impl Into<String>) -> Self {
        self.splat2 = Some(splat2.into());
        self
    }

    /// Returns this terrain with a board relief amplitude.
    #[must_use]
    pub fn with_relief(mut self, relief: f32) -> Self {
        self.relief = relief;
        self
    }

    /// Returns this terrain with an edge style and height.
    #[must_use]
    pub fn with_edge(mut self, edge: EdgeStyle, height: f32) -> Self {
        self.edge = edge;
        self.edge_height = height;
        self
    }

    /// Returns this terrain with an edge prop mesh, its spacing and height.
    #[must_use]
    pub fn with_edge_mesh(mut self, mesh: impl Into<String>, spacing: f32, height: f32) -> Self {
        self.edge_mesh = Some(mesh.into());
        self.edge_spacing = spacing;
        self.edge_mesh_height = height;
        self
    }

    /// Returns this terrain with a lane path width.
    #[must_use]
    pub fn with_lane_width(mut self, lane_width: f32) -> Self {
        self.lane_width = lane_width;
        self
    }

    /// Returns this terrain with a noise seed.
    #[must_use]
    pub fn with_seed(mut self, seed: u32) -> Self {
        self.seed = seed;
        self
    }
}

/// The `[atmosphere]` table: height fog and sky (ADR-0014 §4). Every field is optional:
/// `None` keeps the renderer's default.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct MapAtmosphere {
    /// Fog colour (also the sky's horizon blend), linear RGB `0..=4`.
    pub fog_color: Option<[f32; 3]>,
    /// Fog density per metre at the ground, `0..=0.2`.
    pub fog_density: Option<f32>,
    /// Height falloff scale, metres, `1..=400`.
    pub fog_height: Option<f32>,
    /// Distance before fog begins, metres, `0..=2000`.
    pub fog_start: Option<f32>,
    /// Maximum fog opacity, `0..=`[`MAX_FOG_OPACITY`].
    pub fog_max: Option<f32>,
    /// Sky gradient zenith colour, linear RGB `0..=8`.
    pub sky_zenith: Option<[f32; 3]>,
    /// Sky gradient horizon colour, linear RGB `0..=8`.
    pub sky_horizon: Option<[f32; 3]>,
}

impl MapAtmosphere {
    /// Returns this atmosphere with a fog colour.
    #[must_use]
    pub fn with_fog_color(mut self, color: [f32; 3]) -> Self {
        self.fog_color = Some(color);
        self
    }

    /// Returns this atmosphere with fog density (per metre), height falloff (metres) and
    /// start distance (metres).
    #[must_use]
    pub fn with_fog(mut self, density: f32, height: f32, start: f32) -> Self {
        self.fog_density = Some(density);
        self.fog_height = Some(height);
        self.fog_start = Some(start);
        self
    }

    /// Returns this atmosphere with a maximum fog opacity.
    #[must_use]
    pub fn with_fog_max(mut self, fog_max: f32) -> Self {
        self.fog_max = Some(fog_max);
        self
    }

    /// Returns this atmosphere with sky gradient colours.
    #[must_use]
    pub fn with_sky(mut self, zenith: [f32; 3], horizon: [f32; 3]) -> Self {
        self.sky_zenith = Some(zenith);
        self.sky_horizon = Some(horizon);
        self
    }
}

/// The `[fog_of_war]` table: how unseen areas look (ADR-0014 §3). Every field is optional:
/// `None` keeps the renderer's default.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct MapFogStyle {
    /// Fog-of-war colour, linear RGB `0..=1`.
    pub color: Option<[f32; 3]>,
    /// How much unseen areas darken, within [`FOW_DARKEN_RANGE`].
    pub darken: Option<f32>,
    /// How much unseen areas desaturate, `0..=1`.
    pub desaturate: Option<f32>,
    /// Ground mist height in unseen areas, metres, `0..=32`.
    pub mist_height: Option<f32>,
    /// Ground mist strength, `0..=1`.
    pub mist: Option<f32>,
}

impl MapFogStyle {
    /// Returns this style with a colour.
    #[must_use]
    pub fn with_color(mut self, color: [f32; 3]) -> Self {
        self.color = Some(color);
        self
    }

    /// Returns this style with darkening and desaturation.
    #[must_use]
    pub fn with_tone(mut self, darken: f32, desaturate: f32) -> Self {
        self.darken = Some(darken);
        self.desaturate = Some(desaturate);
        self
    }

    /// Returns this style with a ground mist height (metres) and strength.
    #[must_use]
    pub fn with_mist(mut self, height: f32, mist: f32) -> Self {
        self.mist_height = Some(height);
        self.mist = Some(mist);
        self
    }
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
    /// Bloom strength, `0..=0.1` (default [`DEFAULT_BLOOM_STRENGTH`]): more washes team
    /// colours and silhouettes out (ADR-0015 §3).
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
    /// Baked AO name: `<root>/maps/ao/<name>.{toml,ktx2}` ([`crate::baked_ao_paths`],
    /// ADR-0016).
    pub baked_ao: Option<String>,
    /// Baked AO strength, `0..=1`.
    pub ao_strength: Option<f32>,
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
    /// `[r, g, b]` (the tint alone) or a `[terrain]` table ([`RawTerrain`]).
    terrain: toml::Value,
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
    atmosphere: Option<RawAtmosphere>,
    #[serde(default)]
    fog_of_war: Option<RawFogStyle>,
    #[serde(default)]
    post: Option<RawPost>,
    foliage: Option<RawFoliage>,
    #[serde(default)]
    water: Vec<RawWater>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFoliage {
    #[serde(default)]
    density: Option<f32>,
    #[serde(default)]
    height: Option<[f32; 2]>,
    #[serde(default)]
    base_color: Option<[f32; 3]>,
    #[serde(default)]
    tip_color: Option<[f32; 3]>,
    #[serde(default)]
    flower_color: Option<[f32; 3]>,
    #[serde(default)]
    wind: Option<[f32; 2]>,
    #[serde(default)]
    wind_speed: Option<f32>,
    #[serde(default)]
    reeds: Option<bool>,
    #[serde(default)]
    seed: Option<u32>,
    #[serde(default)]
    patches: Vec<RawPatch>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPatch {
    center: [f32; 2],
    radius: f32,
    kind: String,
    #[serde(default)]
    density: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWater {
    points: Vec<[f32; 2]>,
    #[serde(default)]
    width: Option<f32>,
    #[serde(default)]
    level: Option<f32>,
    #[serde(default)]
    depth: Option<f32>,
    #[serde(default)]
    flow: Option<f32>,
    #[serde(default)]
    color: Option<[f32; 3]>,
    #[serde(default)]
    shallow_color: Option<[f32; 3]>,
    #[serde(default)]
    clarity: Option<f32>,
    #[serde(default)]
    foam: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTerrain {
    tint: [f32; 3],
    layers: Vec<RawLayer>,
    #[serde(default)]
    splat: Option<String>,
    #[serde(default)]
    splat2: Option<String>,
    #[serde(default)]
    relief: Option<f32>,
    #[serde(default)]
    edge: Option<String>,
    #[serde(default)]
    edge_height: Option<f32>,
    #[serde(default)]
    edge_mesh: Option<String>,
    #[serde(default)]
    edge_spacing: Option<f32>,
    #[serde(default)]
    edge_mesh_height: Option<f32>,
    #[serde(default)]
    lane_width: Option<f32>,
    #[serde(default)]
    seed: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLayer {
    albedo: String,
    #[serde(default)]
    normal: Option<String>,
    #[serde(default)]
    tint: Option<[f32; 3]>,
    #[serde(default)]
    tiling: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAtmosphere {
    #[serde(default)]
    fog_color: Option<[f32; 3]>,
    #[serde(default)]
    fog_density: Option<f32>,
    #[serde(default)]
    fog_height: Option<f32>,
    #[serde(default)]
    fog_start: Option<f32>,
    #[serde(default)]
    fog_max: Option<f32>,
    #[serde(default)]
    sky_zenith: Option<[f32; 3]>,
    #[serde(default)]
    sky_horizon: Option<[f32; 3]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFogStyle {
    #[serde(default)]
    color: Option<[f32; 3]>,
    #[serde(default)]
    darken: Option<f32>,
    #[serde(default)]
    desaturate: Option<f32>,
    #[serde(default)]
    mist_height: Option<f32>,
    #[serde(default)]
    mist: Option<f32>,
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
    #[serde(default)]
    baked_ao: Option<String>,
    #[serde(default)]
    ao_strength: Option<f32>,
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
        baked_ao: name(raw.baked_ao, "baked_ao names must be 1-32 of [A-Za-z0-9_-]")?,
        ao_strength: check_scalar(raw.ao_strength, 1.0, "ao_strength must be 0..=1")?,
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
        bloom: check_scalar(raw.bloom, MAX_BLOOM_STRENGTH, "bloom must be 0..=0.1")?,
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

fn check_name(name: Option<String>, what: &'static str) -> Result<Option<String>, MapVisualsError> {
    match name {
        Some(name) if !valid_mesh_name(&name) => Err(MapVisualsError::Invalid(what)),
        other => Ok(other),
    }
}

/// The `terrain` key: the tint, and the `[terrain]` table when it is one.
fn split_terrain(value: toml::Value) -> Result<([f32; 3], Option<MapTerrain>), MapVisualsError> {
    let toml_error = |error: toml::de::Error| MapVisualsError::Toml(format!("terrain: {error}"));
    match value {
        toml::Value::Table(_) => {
            let raw: RawTerrain = value.try_into().map_err(toml_error)?;
            let tint = check_color(raw.tint, "terrain tint must be RGB in 0..=1")?;
            Ok((tint, Some(check_terrain(raw)?)))
        }
        value => {
            let tint: [f32; 3] = value.try_into().map_err(toml_error)?;
            Ok((check_color(tint, "terrain must be RGB in 0..=1")?, None))
        }
    }
}

fn check_terrain(raw: RawTerrain) -> Result<MapTerrain, MapVisualsError> {
    if raw.layers.is_empty() || raw.layers.len() > MAX_TERRAIN_LAYERS {
        return Err(MapVisualsError::Invalid("terrain needs 1..=8 layers"));
    }
    let mut layers = Vec::with_capacity(raw.layers.len());
    for layer in raw.layers {
        if !valid_mesh_name(&layer.albedo) {
            return Err(MapVisualsError::Invalid(
                "terrain layer albedo names must be 1-32 of [A-Za-z0-9_-]",
            ));
        }
        let tint = layer.tint.unwrap_or([1.0; 3]);
        if !tint
            .iter()
            .all(|c| c.is_finite() && (0.0..=2.0).contains(c))
        {
            return Err(MapVisualsError::Invalid(
                "terrain layer tint must be RGB in 0..=2",
            ));
        }
        layers.push(TerrainLayer {
            albedo: layer.albedo,
            normal: check_name(
                layer.normal,
                "terrain layer normal names must be 1-32 of [A-Za-z0-9_-]",
            )?,
            tint,
            tiling: check_range(
                layer.tiling.unwrap_or(DEFAULT_LAYER_TILING),
                LAYER_TILING_RANGE,
                "terrain layer tiling must be 0.5..=64 metres",
            )?,
        });
    }
    let splat = check_name(raw.splat, "splat names must be 1-32 of [A-Za-z0-9_-]")?;
    let splat2 = check_name(raw.splat2, "splat2 names must be 1-32 of [A-Za-z0-9_-]")?;
    if splat2.is_some() && (splat.is_none() || layers.len() <= 4) {
        return Err(MapVisualsError::Invalid(
            "splat2 (layers 5-8) needs splat and more than 4 layers",
        ));
    }
    let edge = match raw.edge.as_deref() {
        None => EdgeStyle::default(),
        Some(name) => EdgeStyle::from_name(name).ok_or(MapVisualsError::Invalid(
            "terrain edge must be \"none\", \"hills\" or \"cliffs\"",
        ))?,
    };
    Ok(MapTerrain {
        layers,
        splat,
        splat2,
        relief: check_range(
            raw.relief.unwrap_or(DEFAULT_TERRAIN_RELIEF),
            (0.0, MAX_PLAY_RELIEF),
            "terrain relief must be 0..=0.5 metres (units stand on flat ground, ADR-0014)",
        )?,
        edge,
        edge_height: check_range(
            raw.edge_height.unwrap_or(DEFAULT_EDGE_HEIGHT),
            (0.0, MAX_EDGE_HEIGHT),
            "terrain edge_height must be 0..=48 metres",
        )?,
        edge_mesh: check_mesh(raw.edge_mesh)?,
        edge_spacing: check_range(
            raw.edge_spacing.unwrap_or(DEFAULT_EDGE_SPACING),
            EDGE_SPACING_RANGE,
            "terrain edge_spacing must be 8..=128 metres",
        )?,
        edge_mesh_height: check_range(
            raw.edge_mesh_height.unwrap_or(DEFAULT_EDGE_MESH_HEIGHT),
            EDGE_MESH_HEIGHT_RANGE,
            "terrain edge_mesh_height must be 1..=48 metres",
        )?,
        lane_width: check_range(
            raw.lane_width.unwrap_or(DEFAULT_LANE_WIDTH),
            LANE_WIDTH_RANGE,
            "terrain lane_width must be 2..=40 metres",
        )?,
        seed: raw.seed.unwrap_or(DEFAULT_TERRAIN_SEED),
    })
}

fn check_in(
    value: Option<f32>,
    range: (f32, f32),
    what: &'static str,
) -> Result<Option<f32>, MapVisualsError> {
    value
        .map(|value| check_range(value, range, what))
        .transpose()
}

fn check_atmosphere(raw: Option<RawAtmosphere>) -> Result<MapAtmosphere, MapVisualsError> {
    let Some(raw) = raw else {
        return Ok(MapAtmosphere::default());
    };
    Ok(MapAtmosphere {
        fog_color: check_rgb(raw.fog_color, 4.0, "fog_color must be RGB in 0..=4")?,
        fog_density: check_scalar(
            raw.fog_density,
            0.2,
            "fog_density must be 0..=0.2 per metre",
        )?,
        fog_height: check_in(
            raw.fog_height,
            (1.0, 400.0),
            "fog_height must be 1..=400 metres",
        )?,
        fog_start: check_scalar(raw.fog_start, 2000.0, "fog_start must be 0..=2000 metres")?,
        fog_max: check_scalar(
            raw.fog_max,
            MAX_FOG_OPACITY,
            "fog_max must be 0..=0.85 (fog never fully hides a unit, ADR-0011 rule 1)",
        )?,
        sky_zenith: check_rgb(raw.sky_zenith, 8.0, "sky_zenith must be RGB in 0..=8")?,
        sky_horizon: check_rgb(raw.sky_horizon, 8.0, "sky_horizon must be RGB in 0..=8")?,
    })
}

fn check_fog_style(raw: Option<RawFogStyle>) -> Result<MapFogStyle, MapVisualsError> {
    let Some(raw) = raw else {
        return Ok(MapFogStyle::default());
    };
    Ok(MapFogStyle {
        color: check_rgb(raw.color, 1.0, "fog_of_war color must be RGB in 0..=1")?,
        darken: check_in(
            raw.darken,
            FOW_DARKEN_RANGE,
            "fog_of_war darken must be 0.2..=0.9 (the boundary stays legible and unseen \
             ground readable)",
        )?,
        desaturate: check_scalar(raw.desaturate, 1.0, "fog_of_war desaturate must be 0..=1")?,
        mist_height: check_scalar(
            raw.mist_height,
            32.0,
            "fog_of_war mist_height must be 0..=32 metres",
        )?,
        mist: check_scalar(raw.mist, 1.0, "fog_of_war mist must be 0..=1")?,
    })
}

fn check_position(p: [f32; 2], what: &'static str) -> Result<[f32; 2], MapVisualsError> {
    Ok([
        check_range(p[0], POSITION_RANGE, what)?,
        check_range(p[1], POSITION_RANGE, what)?,
    ])
}

fn check_foliage(raw: RawFoliage) -> Result<MapFoliage, MapVisualsError> {
    if raw.patches.len() > MAX_FOLIAGE_PATCHES {
        return Err(MapVisualsError::Invalid("at most 32 foliage patches"));
    }
    let mut patches = Vec::with_capacity(raw.patches.len());
    for patch in raw.patches {
        patches.push(FoliagePatch {
            center: check_position(patch.center, "foliage patch center out of range")?,
            radius: check_range(
                patch.radius,
                PATCH_RADIUS_RANGE,
                "foliage patch radius must be 1..=60 metres",
            )?,
            kind: FoliageKind::from_name(&patch.kind).ok_or(MapVisualsError::Invalid(
                "foliage patch kind must be \"grass\", \"brush\", \"reeds\" or \"flowers\"",
            ))?,
            density: check_range(
                patch.density.unwrap_or(FoliagePatch::DEFAULT_DENSITY),
                PATCH_DENSITY_RANGE,
                "foliage patch density must be 0..=20 clumps per 10 m²",
            )?,
        });
    }
    let height = raw.height.unwrap_or(MapFoliage::DEFAULT_HEIGHT);
    let height = [
        check_range(
            height[0],
            FOLIAGE_HEIGHT_RANGE,
            "foliage height must be 0.1..=1.5",
        )?,
        check_range(
            height[1],
            FOLIAGE_HEIGHT_RANGE,
            "foliage height must be 0.1..=1.5",
        )?,
    ];
    if height[0] > height[1] {
        return Err(MapVisualsError::Invalid(
            "foliage height must be [low, high] with low <= high",
        ));
    }
    let wind = raw.wind.unwrap_or(MapFoliage::DEFAULT_WIND);
    let strength = (wind[0] * wind[0] + wind[1] * wind[1]).sqrt();
    if !strength.is_finite() || strength > MAX_WIND {
        return Err(MapVisualsError::Invalid(
            "foliage wind must be a vector of length 0..=4",
        ));
    }
    let color =
        |value: Option<[f32; 3]>, fallback, what| check_color(value.unwrap_or(fallback), what);
    Ok(MapFoliage {
        density: check_range(
            raw.density.unwrap_or(MapFoliage::DEFAULT_DENSITY),
            FOLIAGE_DENSITY_RANGE,
            "foliage density must be 0..=8 clumps per 10 m²",
        )?,
        height,
        base_color: color(
            raw.base_color,
            MapFoliage::DEFAULT_BASE_COLOR,
            "foliage base_color must be RGB in 0..=1",
        )?,
        tip_color: color(
            raw.tip_color,
            MapFoliage::DEFAULT_TIP_COLOR,
            "foliage tip_color must be RGB in 0..=1",
        )?,
        flower_color: color(
            raw.flower_color,
            MapFoliage::DEFAULT_FLOWER_COLOR,
            "foliage flower_color must be RGB in 0..=1",
        )?,
        wind,
        wind_speed: check_range(
            raw.wind_speed.unwrap_or(MapFoliage::DEFAULT_WIND_SPEED),
            (0.0, 8.0),
            "foliage wind_speed must be 0..=8",
        )?,
        reeds: raw.reeds.unwrap_or(true),
        seed: raw.seed.unwrap_or(1),
        patches,
    })
}

fn check_water(raw: Vec<RawWater>) -> Result<Vec<MapWater>, MapVisualsError> {
    if raw.len() > MAX_MAP_WATER {
        return Err(MapVisualsError::Invalid("at most 8 water bodies"));
    }
    let mut bodies = Vec::with_capacity(raw.len());
    for body in raw {
        if body.points.is_empty() || body.points.len() > MAX_WATER_POINTS {
            return Err(MapVisualsError::Invalid("a water body needs 1..=32 points"));
        }
        let mut points = Vec::with_capacity(body.points.len());
        for point in body.points {
            points.push(check_position(point, "water point out of range")?);
        }
        let level = check_range(
            body.level.unwrap_or(MapWater::DEFAULT_LEVEL),
            (-MAX_PLAY_RELIEF, MAX_WATER_LEVEL),
            "water level must be -0.5..=0.3 metres (units stand at height 0 and wade)",
        )?;
        let depth = check_range(
            body.depth.unwrap_or(MapWater::DEFAULT_DEPTH),
            WATER_DEPTH_RANGE,
            "water depth must be 0.05..=0.8 metres (a hero wades, never swims)",
        )?;
        if level - depth < -MAX_PLAY_RELIEF - 1e-4 {
            return Err(MapVisualsError::Invalid(
                "water level - depth must be at least -0.5: the bed stays within the board's \
                 relief limit (ADR-0014)",
            ));
        }
        let color =
            |value: Option<[f32; 3]>, fallback, what| check_color(value.unwrap_or(fallback), what);
        bodies.push(MapWater {
            points,
            width: check_range(
                body.width.unwrap_or(MapWater::DEFAULT_WIDTH),
                WATER_WIDTH_RANGE,
                "water width must be 2..=60 metres",
            )?,
            level,
            depth,
            flow: check_range(
                body.flow.unwrap_or(MapWater::DEFAULT_FLOW),
                WATER_FLOW_RANGE,
                "water flow must be 0..=8 metres per second",
            )?,
            color: color(
                body.color,
                MapWater::DEFAULT_COLOR,
                "water color must be RGB in 0..=1",
            )?,
            shallow_color: color(
                body.shallow_color,
                MapWater::DEFAULT_SHALLOW_COLOR,
                "water shallow_color must be RGB in 0..=1",
            )?,
            clarity: check_range(
                body.clarity.unwrap_or(MapWater::DEFAULT_CLARITY),
                WATER_CLARITY_RANGE,
                "water clarity must be 0.05..=4 metres",
            )?,
            foam: check_range(
                body.foam.unwrap_or(MapWater::DEFAULT_FOAM),
                (0.0, 1.0),
                "water foam must be 0..=1",
            )?,
        });
    }
    Ok(bodies)
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
        let (terrain, ground) = split_terrain(raw.terrain)?;
        if ground.is_none() && (raw.foliage.is_some() || !raw.water.is_empty()) {
            return Err(MapVisualsError::Invalid(
                "[foliage] and [[water]] need the [terrain] table (they stand on the heightfield)",
            ));
        }
        let foliage = raw.foliage.map(check_foliage).transpose()?;
        let water = check_water(raw.water)?;
        let visuals = Self {
            terrain,
            wall: check_color(raw.wall, "wall must be RGB in 0..=1")?,
            wall_height: check_range(
                raw.wall_height.unwrap_or(DEFAULT_WALL_HEIGHT),
                WALL_HEIGHT_RANGE,
                "wall_height must be 0.5..=32",
            )?,
            wall_mesh: check_mesh(raw.wall_mesh)?,
            props,
            lighting: check_lighting(raw.lighting)?,
            ground,
            atmosphere: check_atmosphere(raw.atmosphere)?,
            fog_of_war: check_fog_style(raw.fog_of_war)?,
            post: check_post(raw.post)?,
            foliage,
            water,
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

    /// Distinct mesh names the descriptor references (wall first, then the terrain's edge
    /// mesh, then props in order).
    pub fn mesh_names(&self) -> impl Iterator<Item = &str> {
        let all = self
            .wall_mesh
            .as_deref()
            .into_iter()
            .chain(
                self.ground
                    .as_ref()
                    .and_then(|ground| ground.edge_mesh.as_deref()),
            )
            .chain(self.props.iter().filter_map(|prop| prop.mesh.as_deref()));
        distinct(all)
    }

    /// Distinct texture names the `[terrain]` table references: each layer's albedo and
    /// normal map in layer order, then the splat maps. (The `[lighting]` ramp is not
    /// included.)
    pub fn texture_names(&self) -> impl Iterator<Item = &str> {
        let terrain = self.ground.as_ref();
        let layers = terrain.into_iter().flat_map(|terrain| {
            terrain.layers.iter().flat_map(|layer| {
                core::iter::once(layer.albedo.as_str()).chain(layer.normal.as_deref())
            })
        });
        let splats = terrain.into_iter().flat_map(|terrain| {
            terrain
                .splat
                .as_deref()
                .into_iter()
                .chain(terrain.splat2.as_deref())
        });
        distinct(layers.chain(splats))
    }

    /// Returns this descriptor with a `[terrain]` table (or none: the flat plane).
    #[must_use]
    pub fn with_ground(mut self, ground: Option<MapTerrain>) -> Self {
        self.ground = ground;
        self
    }

    /// Returns this descriptor with an `[atmosphere]` table.
    #[must_use]
    pub fn with_atmosphere(mut self, atmosphere: MapAtmosphere) -> Self {
        self.atmosphere = atmosphere;
        self
    }

    /// Returns this descriptor with a `[fog_of_war]` table.
    #[must_use]
    pub fn with_fog_of_war(mut self, fog_of_war: MapFogStyle) -> Self {
        self.fog_of_war = fog_of_war;
        self
    }

    /// Returns this descriptor with a `[foliage]` table (or none).
    #[must_use]
    pub fn with_foliage(mut self, foliage: Option<MapFoliage>) -> Self {
        self.foliage = foliage;
        self
    }

    /// Returns this descriptor with these `[[water]]` bodies.
    #[must_use]
    pub fn with_water(mut self, water: Vec<MapWater>) -> Self {
        self.water = water;
        self
    }
}

fn distinct<'a>(names: impl Iterator<Item = &'a str>) -> impl Iterator<Item = &'a str> {
    let mut seen: Vec<&str> = Vec::new();
    names.filter(move |name| {
        if seen.contains(name) {
            false
        } else {
            seen.push(name);
            true
        }
    })
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
             shadow_strength = 0.7\nwrap = 0.3\nramp = \"ember-ramp\"\nramp_strength = 0.5\n\
             baked_ao = \"ember-ao\"\nao_strength = 0.8\n"
        ))
        .expect("lighting");
        let lighting = &visuals.lighting;
        assert_eq!(lighting.environment.as_deref(), Some("ember"));
        assert_eq!(lighting.sun_direction, Some([0.0, 1.0, 0.0]));
        assert_eq!(lighting.sun_color, Some([1.0, 0.5, 0.25]));
        assert_eq!(lighting.shadow_strength, Some(0.7));
        assert_eq!(lighting.ramp.as_deref(), Some("ember-ramp"));
        assert_eq!(lighting.sun_intensity, None);
        assert_eq!(lighting.baked_ao.as_deref(), Some("ember-ao"));
        assert_eq!(lighting.ao_strength, Some(0.8));
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
            "baked_ao = \"../ao\"",
            "ao_strength = 1.5",
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
            "bloom = 0.1",
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
            "bloom = 0.11",
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
            format!("{base}#{}", "x".repeat(33 * 1024)),
        ] {
            assert!(MapVisuals::parse(&bad).is_err(), "{bad:?}");
        }
    }

    const TERRAIN: &str = r#"
wall = [0.2, 0.2, 0.2]
wall_mesh = "rampart"

[terrain]
tint = [0.3, 0.1, 0.05]
relief = 0.35
edge = "cliffs"
edge_height = 22.0
edge_mesh = "cliff"
edge_spacing = 30.0
edge_mesh_height = 12.0
lane_width = 10.0
seed = 3
splat = "ember-splat"
splat2 = "ember-splat2"

[[terrain.layers]]
albedo = "terrain-grass"
normal = "terrain-grass-normal"
tint = [0.55, 0.75, 0.45]
tiling = 6.0

[[terrain.layers]]
albedo = "terrain-dirt"
normal = "terrain-dirt-normal"

[[terrain.layers]]
albedo = "terrain-rock"
normal = "terrain-grass-normal"

[[terrain.layers]]
albedo = "terrain-moss"

[[terrain.layers]]
albedo = "terrain-grass"

[atmosphere]
fog_color = [0.5, 0.6, 0.7]
fog_density = 0.012
fog_height = 40.0
fog_start = 60.0
fog_max = 0.7
sky_zenith = [0.2, 0.3, 0.6]
sky_horizon = [0.6, 0.65, 0.7]

[fog_of_war]
color = [0.02, 0.03, 0.05]
darken = 0.72
desaturate = 0.5
mist_height = 3.0
mist = 0.4
"#;

    #[test]
    fn terrain_atmosphere_and_fog_of_war_tables_parse() {
        let visuals = MapVisuals::parse(TERRAIN).expect("descriptor");
        assert_eq!(visuals.terrain, [0.3, 0.1, 0.05], "the table's tint");
        let terrain = visuals.ground.as_ref().expect("terrain table");
        assert_eq!(terrain.layers.len(), 5);
        assert_eq!(terrain.layers[0].tint, [0.55, 0.75, 0.45]);
        assert_eq!(terrain.layers[0].tiling, 6.0);
        assert_eq!(terrain.layers[1].tint, [1.0; 3]);
        assert_eq!(terrain.layers[1].tiling, DEFAULT_LAYER_TILING);
        assert_eq!(terrain.layers[3].normal, None);
        assert_eq!(terrain.edge, EdgeStyle::Cliffs);
        assert_eq!(
            (terrain.relief, terrain.edge_height, terrain.seed),
            (0.35, 22.0, 3)
        );
        assert_eq!(
            (
                terrain.edge_spacing,
                terrain.edge_mesh_height,
                terrain.lane_width
            ),
            (30.0, 12.0, 10.0)
        );
        assert_eq!(terrain.splat2.as_deref(), Some("ember-splat2"));
        assert_eq!(
            visuals.mesh_names().collect::<Vec<_>>(),
            ["rampart", "cliff"],
            "the edge mesh counts as a map mesh"
        );
        assert_eq!(
            visuals.texture_names().collect::<Vec<_>>(),
            [
                "terrain-grass",
                "terrain-grass-normal",
                "terrain-dirt",
                "terrain-dirt-normal",
                "terrain-rock",
                "terrain-moss",
                "ember-splat",
                "ember-splat2"
            ]
        );
        assert_eq!(
            visuals.atmosphere,
            MapAtmosphere::default()
                .with_fog_color([0.5, 0.6, 0.7])
                .with_fog(0.012, 40.0, 60.0)
                .with_fog_max(0.7)
                .with_sky([0.2, 0.3, 0.6], [0.6, 0.65, 0.7])
        );
        assert_eq!(
            visuals.fog_of_war,
            MapFogStyle::default()
                .with_color([0.02, 0.03, 0.05])
                .with_tone(0.72, 0.5)
                .with_mist(3.0, 0.4)
        );
        // Defaults: a minimal table, and no tables at all.
        let minimal = MapVisuals::parse(
            "wall = [0, 0, 0]\n[terrain]\ntint = [0, 0, 0]\n[[terrain.layers]]\nalbedo = \"a\"\n",
        )
        .expect("minimal");
        let expected = MapTerrain::new(vec![TerrainLayer::new("a")]);
        assert_eq!(minimal.ground.as_ref(), Some(&expected));
        assert_eq!(expected.edge, EdgeStyle::Hills);
        assert_eq!(
            (expected.relief, expected.edge_height, expected.lane_width),
            (0.25, 18.0, 12.0)
        );
        let flat = MapVisuals::parse("terrain = [0, 0, 0]\nwall = [1, 1, 1]").expect("flat");
        assert_eq!(flat.ground, None);
        assert_eq!(flat.texture_names().count(), 0);
        assert_eq!(flat.atmosphere, MapAtmosphere::default());
        assert_eq!(flat.fog_of_war, MapFogStyle::default());
        for style in [EdgeStyle::None, EdgeStyle::Hills, EdgeStyle::Cliffs] {
            assert_eq!(EdgeStyle::from_name(style.name()), Some(style));
        }
        assert_eq!(EdgeStyle::from_name("mountains"), None);
    }

    #[test]
    fn terrain_atmosphere_and_fog_of_war_reject_out_of_range_values() {
        let terrain = |body: &str| {
            format!(
                "wall = [0, 0, 0]\n[terrain]\ntint = [0, 0, 0]\n{body}\n\
                 [[terrain.layers]]\nalbedo = \"a\"\n"
            )
        };
        let layer = |body: &str| {
            format!(
                "wall = [0, 0, 0]\n[terrain]\ntint = [0, 0, 0]\n\
                 [[terrain.layers]]\nalbedo = \"a\"\n{body}\n"
            )
        };
        let base = "terrain = [0.1, 0.1, 0.1]\nwall = [0.2, 0.2, 0.2]\n";
        let five = "[[terrain.layers]]\nalbedo = \"a\"\n".repeat(4);
        for bad in [
            // The tint and the table cannot both be `terrain`.
            format!("{base}[terrain]\ntint = [0, 0, 0]\n[[terrain.layers]]\nalbedo = \"a\"\n"),
            String::from("terrain = 3\nwall = [0, 0, 0]"),
            String::from("wall = [0, 0, 0]\n[terrain]\n[[terrain.layers]]\nalbedo = \"a\"\n"),
            String::from("wall = [0, 0, 0]\n[terrain]\ntint = [0, 0, 0]\n"),
            terrain("tint = [2, 0, 0]").replace("tint = [0, 0, 0]\n", ""),
            format!(
                "wall = [0, 0, 0]\n[terrain]\ntint = [0, 0, 0]\n{}",
                "[[terrain.layers]]\nalbedo = \"a\"\n".repeat(9)
            ),
            terrain("relief = 0.6"),
            terrain("relief = -0.1"),
            terrain("relief = nan"),
            terrain("edge = \"mountains\""),
            terrain("edge_height = 49"),
            terrain("edge_height = -1"),
            terrain("edge_mesh = \"../x\""),
            terrain("edge_spacing = 4"),
            terrain("edge_spacing = 200"),
            terrain("edge_mesh_height = 0.5"),
            terrain("edge_mesh_height = 60"),
            terrain("lane_width = 1"),
            terrain("lane_width = 41"),
            terrain("seed = -1"),
            terrain("splat = \"a/b\""),
            terrain("splat2 = \"b\""),
            terrain("splat = \"a\"\nsplat2 = \"b\""),
            terrain("unknown = 1"),
            layer("tint = [3, 0, 0]"),
            layer("tint = [0, 0]"),
            layer("tiling = 0.1"),
            layer("tiling = 100"),
            layer("normal = \"c:x\""),
            layer("roughness = 1"),
            String::from(
                "wall = [0, 0, 0]\n[terrain]\ntint = [0, 0, 0]\n[[terrain.layers]]\nalbedo = \"\"\n",
            ),
            format!("{base}[atmosphere]\nfog_color = [5, 0, 0]"),
            format!("{base}[atmosphere]\nfog_density = 0.3"),
            format!("{base}[atmosphere]\nfog_height = 0.5"),
            format!("{base}[atmosphere]\nfog_height = 500"),
            format!("{base}[atmosphere]\nfog_start = -1"),
            format!("{base}[atmosphere]\nfog_start = 3000"),
            format!("{base}[atmosphere]\nfog_max = 0.9"),
            format!("{base}[atmosphere]\nsky_zenith = [9, 0, 0]"),
            format!("{base}[atmosphere]\nsky_horizon = [nan, 0, 0]"),
            format!("{base}[atmosphere]\nsun = 1"),
            format!("{base}[fog_of_war]\ncolor = [1.5, 0, 0]"),
            format!("{base}[fog_of_war]\ndarken = 0.1"),
            format!("{base}[fog_of_war]\ndarken = 0.95"),
            format!("{base}[fog_of_war]\ndesaturate = 1.5"),
            format!("{base}[fog_of_war]\nmist_height = 40"),
            format!("{base}[fog_of_war]\nmist = -0.5"),
            format!("{base}[fog_of_war]\nopacity = 1"),
            // The edge mesh counts toward the four-mesh limit.
            format!(
                "wall = [0, 0, 0]\nwall_mesh = \"m0\"\n{}[terrain]\ntint = [0, 0, 0]\n\
                 edge_mesh = \"m4\"\n[[terrain.layers]]\nalbedo = \"a\"\n",
                ["m1", "m2", "m3"]
                    .map(|mesh| format!(
                        "[[props]]\nposition = [1, 1]\ncolor = [1, 1, 1]\nmesh = \"{mesh}\"\n"
                    ))
                    .concat()
            ),
        ] {
            assert!(MapVisuals::parse(&bad).is_err(), "{bad:?}");
        }
        // splat2 is fine with splat and more than four layers.
        assert!(
            MapVisuals::parse(&format!(
                "{}{five}",
                terrain("splat = \"a\"\nsplat2 = \"b\"")
            ))
            .is_ok()
        );
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
        std::fs::write(root.join("big.toml"), vec![b'#'; 40 * 1024]).expect("write");
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

    const HEIGHTFIELD: &str = "wall = [0.5, 0.2, 0.1]\n[terrain]\ntint = [0.3, 0.1, 0.1]\n\
                           [[terrain.layers]]\nalbedo = \"grass\"\n";

    #[test]
    fn foliage_and_water_parse_with_defaults() {
        let visuals = MapVisuals::parse(&format!(
            "{HEIGHTFIELD}[foliage]\ndensity = 2.5\nheight = [0.2, 0.9]\nwind = [1.0, -2.0]\n\
             reeds = false\nseed = 9\n\
             [[foliage.patches]]\ncenter = [96, 140]\nradius = 8\nkind = \"brush\"\n\
             [[foliage.patches]]\ncenter = [20, 20]\nradius = 3\nkind = \"flowers\"\ndensity = 12\n\
             [[water]]\npoints = [[40, 90], [128, 120], [210, 150]]\nwidth = 14\nlevel = 0.2\n\
             depth = 0.6\nflow = 2\ncolor = [0.0, 0.1, 0.2]\nclarity = 0.5\nfoam = 0.2\n\
             [[water]]\npoints = [[200, 40]]\n"
        ))
        .expect("foliage and water");
        let foliage = visuals.foliage.as_ref().expect("foliage");
        assert!((foliage.density - 2.5).abs() < 1e-6);
        assert_eq!(foliage.height, [0.2, 0.9]);
        assert_eq!(foliage.wind, [1.0, -2.0]);
        assert!(!foliage.reeds);
        assert_eq!(foliage.seed, 9);
        assert_eq!(foliage.tip_color, MapFoliage::DEFAULT_TIP_COLOR);
        assert_eq!(foliage.patches.len(), 2);
        assert_eq!(foliage.patches[0].kind, FoliageKind::Brush);
        assert!((foliage.patches[0].density - FoliagePatch::DEFAULT_DENSITY).abs() < 1e-6);
        assert_eq!(foliage.patches[1].kind.index(), 3);
        assert_eq!(visuals.water.len(), 2);
        let river = &visuals.water[0];
        assert_eq!(river.points.len(), 3);
        assert!((river.width - 14.0).abs() < 1e-6 && (river.level - 0.2).abs() < 1e-6);
        assert!((river.depth - 0.6).abs() < 1e-6 && (river.flow - 2.0).abs() < 1e-6);
        assert_eq!(river.shallow_color, MapWater::DEFAULT_SHALLOW_COLOR);
        assert_eq!(visuals.water[1], MapWater::new(vec![[200.0, 40.0]]));
        // Defaults when the table is empty.
        let plain = MapVisuals::parse(&format!("{HEIGHTFIELD}[foliage]\n")).expect("empty foliage");
        assert_eq!(plain.foliage, Some(MapFoliage::default()));
        assert_eq!(plain.water.len(), 0);
        let none = MapVisuals::parse(HEIGHTFIELD).expect("no foliage");
        assert_eq!((none.foliage, none.water.len()), (None, 0));
        for kind in FoliageKind::ALL {
            assert_eq!(FoliageKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(FoliageKind::from_name("tree"), None);
    }

    #[test]
    fn rejects_hostile_or_out_of_range_foliage_and_water() {
        let water = |body: &str| format!("{HEIGHTFIELD}[[water]]\npoints = [[10, 10]]\n{body}");
        let foliage = |body: &str| format!("{HEIGHTFIELD}[foliage]\n{body}");
        let patch = |body: &str| {
            format!(
                "{HEIGHTFIELD}[[foliage.patches]]\ncenter = [1, 1]\nradius = 2\nkind = \"grass\"\n{body}"
            )
        };
        assert!(MapVisuals::parse(&water("")).is_ok());
        assert!(MapVisuals::parse(&patch("")).is_ok());
        for bad in [
            // Both need the heightfield.
            String::from("terrain = [0.1, 0.1, 0.1]\nwall = [0.1, 0.1, 0.1]\n[foliage]\n"),
            String::from(
                "terrain = [0.1, 0.1, 0.1]\nwall = [0.1, 0.1, 0.1]\n[[water]]\npoints = [[1, 1]]\n",
            ),
            water("extra = 1"),
            water("width = 1"),
            water("width = 61"),
            water("level = 0.31"),
            water("level = -0.6"),
            water("depth = 0.01"),
            water("depth = 0.9"),
            // The bed would sink below the relief limit.
            water("level = -0.2\ndepth = 0.4"),
            water("flow = -1"),
            water("flow = 9"),
            water("color = [2, 0, 0]"),
            water("clarity = 0"),
            water("foam = 1.5"),
            water("level = nan"),
            format!("{HEIGHTFIELD}[[water]]\npoints = []\n"),
            format!("{HEIGHTFIELD}[[water]]\nwidth = 4\n"),
            format!("{HEIGHTFIELD}[[water]]\npoints = [[1e9, 1]]\n"),
            format!(
                "{HEIGHTFIELD}[[water]]\npoints = [{}]\n",
                vec!["[1, 1]"; MAX_WATER_POINTS + 1].join(", ")
            ),
            format!(
                "{HEIGHTFIELD}{}",
                "[[water]]\npoints = [[1, 1]]\n".repeat(MAX_MAP_WATER + 1)
            ),
            foliage("extra = 1"),
            foliage("density = 9"),
            foliage("density = -1"),
            foliage("height = [0.05, 0.5]"),
            foliage("height = [0.9, 0.3]"),
            foliage("height = [0.3, 2]"),
            foliage("wind = [3, 3]"),
            foliage("wind = [inf, 0]"),
            foliage("wind_speed = 9"),
            foliage("tip_color = [1, 1, 2]"),
            foliage("seed = -1"),
            patch("extra = 1"),
            patch("density = 21"),
            format!(
                "{HEIGHTFIELD}[[foliage.patches]]\ncenter = [1, 1]\nradius = 0.5\nkind = \"grass\"\n"
            ),
            format!(
                "{HEIGHTFIELD}[[foliage.patches]]\ncenter = [1, 1]\nradius = 61\nkind = \"grass\"\n"
            ),
            format!(
                "{HEIGHTFIELD}[[foliage.patches]]\ncenter = [1, 1]\nradius = 2\nkind = \"tree\"\n"
            ),
            format!("{HEIGHTFIELD}[[foliage.patches]]\ncenter = [1, 1]\nradius = 2\n"),
            format!(
                "{HEIGHTFIELD}{}",
                "[[foliage.patches]]\ncenter = [1, 1]\nradius = 2\nkind = \"grass\"\n"
                    .repeat(MAX_FOLIAGE_PATCHES + 1)
            ),
        ] {
            assert!(MapVisuals::parse(&bad).is_err(), "{bad:?}");
        }
        // The bed may reach the relief limit exactly.
        assert!(MapVisuals::parse(&water("level = 0.3\ndepth = 0.8")).is_ok());
        // Eight full bodies and 32 patches fit in a descriptor.
        let points = vec!["[100.25, 100.25]"; MAX_WATER_POINTS].join(", ");
        let full = format!(
            "{HEIGHTFIELD}{}{}",
            format!(
                "[[water]]\npoints = [{points}]\nwidth = 12.5\nlevel = 0.1\ndepth = 0.45\nflow = 1.0\n\
                 color = [0.02, 0.12, 0.16]\nshallow_color = [0.10, 0.30, 0.30]\nclarity = 0.35\n\
                 foam = 0.6\n"
            )
            .repeat(MAX_MAP_WATER),
            "[[foliage.patches]]\ncenter = [100.5, 100.5]\nradius = 10.0\nkind = \"flowers\"\n\
             density = 4.0\n"
                .repeat(MAX_FOLIAGE_PATCHES)
        );
        assert!(
            (full.len() as u64) < MAX_MAP_VISUALS_BYTES / 2,
            "{}",
            full.len()
        );
        let full = MapVisuals::parse(&full).expect("full descriptor");
        assert_eq!(full.water.len(), MAX_MAP_WATER);
    }
}
