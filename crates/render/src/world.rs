//! World presentation (R3a, ADR-0014): the terrain, fog of war v2 and atmosphere settings.
//!
//! - [`WorldConfig`] holds the renderer-lifetime, tiered features
//!   ([`WorldConfig::for_tier`]) and their off switches.
//! - [`TerrainData`] is a map's heightfield and splat-textured layers, uploaded once per map
//!   ([`Renderer::set_terrain`](crate::Renderer::set_terrain)).
//! - [`FogOfWar`] is one frame's vision: the client's **own-team** sources and the map's
//!   static line-of-sight walls. The renderer rasterises it into a low-resolution visibility
//!   texture on the GPU, blurs it and fades it over time. It is presentation only: the server
//!   already withholds what the team cannot see, and the texture is built only from what the
//!   caller passes, so it can never reveal a hidden unit.
//! - [`SceneAtmosphere`] is the per-map art direction for height fog, the sky and the fog of
//!   war's look.
//!
//! Everything here is client-local and cosmetic; nothing reaches the simulation or the wire.

use moba_assets::TextureData;
use moba_core::math::Vec3;

use crate::quality::QualityTier;

/// Most terrain layers a map may use (two RGBA splat layers).
pub const MAX_TERRAIN_LAYERS: usize = 8;
/// Line-of-sight walls uploaded per frame; more are dropped and counted.
pub const MAX_VISION_WALLS: usize = 256;
/// Default [`RenderConfig::max_vision_sources`](crate::RenderConfig::max_vision_sources).
pub const DEFAULT_MAX_VISION_SOURCES: usize = 1_024;
/// Upper bound of [`RenderConfig::max_vision_sources`](crate::RenderConfig::max_vision_sources)
/// (also capped by the device's `maxStorageBufferRange / 16`).
pub const MAX_VISION_SOURCES: usize = 65_536;
/// Smallest accepted fog-of-war texture edge.
pub const MIN_FOG_OF_WAR_RESOLUTION: u32 = 64;
/// Largest accepted fog-of-war texture edge.
pub const MAX_FOG_OF_WAR_RESOLUTION: u32 = 1_024;
/// A [`FogOfWar::dt`] that settles the texture in one frame (no reveal or fade from earlier
/// frames): for repeatable captures and goldens.
pub const FOG_OF_WAR_SETTLE_DT: f32 = 10.0;

/// How the fog of war is drawn (ADR-0014).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FogOfWarMode {
    /// No fog of war.
    Off,
    /// The pre-R3a loop over up to 16 vision circles and 32 walls in the camera uniform
    /// ([`Scene::vision`](crate::Scene::vision)), kept for comparison.
    Legacy,
    /// The visibility texture built from [`Scene::fog_of_war`](crate::Scene::fog_of_war).
    #[default]
    Texture,
}

impl FogOfWarMode {
    /// Every mode.
    pub const ALL: &'static [Self] = &[Self::Off, Self::Legacy, Self::Texture];

    /// Lower-case name, as accepted by [`Self::from_name`].
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Legacy => "legacy",
            Self::Texture => "texture",
        }
    }

    /// Parses a mode name, ignoring ASCII case and surrounding whitespace.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .iter()
            .copied()
            .find(|mode| mode.name().eq_ignore_ascii_case(name))
    }
}

/// How the terrain is shaded (ADR-0014).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TerrainShading {
    /// The R2a plane with its procedural grid, whatever the map provides.
    Flat,
    /// The map's heightfield with splat-textured layers when it has a terrain, else flat.
    #[default]
    Splat,
}

impl TerrainShading {
    /// Every mode.
    pub const ALL: &'static [Self] = &[Self::Flat, Self::Splat];

    /// Lower-case name, as accepted by [`Self::from_name`].
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Splat => "splat",
        }
    }

    /// Parses a mode name, ignoring ASCII case and surrounding whitespace.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .iter()
            .copied()
            .find(|mode| mode.name().eq_ignore_ascii_case(name))
    }
}

/// Renderer-lifetime terrain, fog-of-war and atmosphere features (startup only; see
/// [`RenderConfig::world`](crate::RenderConfig::world)).
///
/// [`WorldConfig::for_tier`] gives each [`QualityTier`]'s defaults; the `with_*` methods are
/// the overrides and off switches (the client maps `MOBA_TERRAIN`, `MOBA_FOW`,
/// `MOBA_FOW_SIZE`, `MOBA_FOG` and `MOBA_SKY` onto them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)] // Independent feature switches, like LightingConfig.
pub struct WorldConfig {
    /// Terrain shading; [`TerrainShading::Flat`] ignores [`Renderer::set_terrain`](crate::Renderer::set_terrain).
    pub terrain: TerrainShading,
    /// Heightfield cells per side a client should generate (the renderer accepts any grid
    /// up to 1024 cells).
    pub terrain_grid: u32,
    /// Most splat layers sampled per pixel (1..=8); later layers fold into the first.
    pub terrain_layers: u32,
    /// Sample the layers' normal maps.
    pub terrain_normal_maps: bool,
    /// Project the layers triplanar on slopes (else top-down everywhere).
    pub terrain_triplanar: bool,
    /// Fog-of-war drawing; [`FogOfWarMode::Texture`] adds two render-graph passes.
    pub fog_of_war: FogOfWarMode,
    /// Fog-of-war texture edge in texels, clamped to
    /// [`MIN_FOG_OF_WAR_RESOLUTION`]`..=`[`MAX_FOG_OF_WAR_RESOLUTION`].
    pub fog_of_war_resolution: u32,
    /// Height fog in the lit scene shaders. Off keeps the R2a distance fade on the terrain.
    pub height_fog: bool,
    /// Draw the sky behind the scene (else the clear colour shows).
    pub sky: bool,
    /// Let the sky sample the map's environment cube (else a gradient).
    pub sky_environment: bool,
}

impl WorldConfig {
    /// The defaults of `tier` (ADR-0014):
    ///
    /// | Tier | Grid | Layers | Normal maps | Triplanar | Fog of war | Sky |
    /// |---|---|---|---|---|---|---|
    /// | `Low` | 128 | 4 | off | off | 128² | gradient |
    /// | `Medium` | 192 | 8 | on | off | 256² | environment |
    /// | `High` | 256 | 8 | on | on | 256² | environment |
    /// | `Ultra` | 384 | 8 | on | on | 512² | environment |
    #[must_use]
    pub const fn for_tier(tier: QualityTier) -> Self {
        let (grid, layers, normals, triplanar, fow, environment) = match tier {
            QualityTier::Low => (128, 4, false, false, 128, false),
            QualityTier::Medium => (192, 8, true, false, 256, true),
            QualityTier::High => (256, 8, true, true, 256, true),
            QualityTier::Ultra => (384, 8, true, true, 512, true),
        };
        Self {
            terrain: TerrainShading::Splat,
            terrain_grid: grid,
            terrain_layers: layers,
            terrain_normal_maps: normals,
            terrain_triplanar: triplanar,
            fog_of_war: FogOfWarMode::Texture,
            fog_of_war_resolution: fow,
            height_fog: true,
            sky: true,
            sky_environment: environment,
        }
    }

    /// Returns this config with [`WorldConfig::terrain`] set.
    #[must_use]
    pub const fn with_terrain(mut self, terrain: TerrainShading) -> Self {
        self.terrain = terrain;
        self
    }

    /// Returns this config with [`WorldConfig::terrain_grid`] set.
    #[must_use]
    pub const fn with_terrain_grid(mut self, cells: u32) -> Self {
        self.terrain_grid = cells;
        self
    }

    /// Returns this config with [`WorldConfig::terrain_layers`] set.
    #[must_use]
    pub const fn with_terrain_layers(mut self, layers: u32) -> Self {
        self.terrain_layers = layers;
        self
    }

    /// Returns this config with [`WorldConfig::terrain_normal_maps`] set.
    #[must_use]
    pub const fn with_terrain_normal_maps(mut self, normal_maps: bool) -> Self {
        self.terrain_normal_maps = normal_maps;
        self
    }

    /// Returns this config with [`WorldConfig::terrain_triplanar`] set.
    #[must_use]
    pub const fn with_terrain_triplanar(mut self, triplanar: bool) -> Self {
        self.terrain_triplanar = triplanar;
        self
    }

    /// Returns this config with [`WorldConfig::fog_of_war`] set.
    #[must_use]
    pub const fn with_fog_of_war(mut self, mode: FogOfWarMode) -> Self {
        self.fog_of_war = mode;
        self
    }

    /// Returns this config with [`WorldConfig::fog_of_war_resolution`] set.
    #[must_use]
    pub const fn with_fog_of_war_resolution(mut self, resolution: u32) -> Self {
        self.fog_of_war_resolution = resolution;
        self
    }

    /// Returns this config with [`WorldConfig::height_fog`] set.
    #[must_use]
    pub const fn with_height_fog(mut self, height_fog: bool) -> Self {
        self.height_fog = height_fog;
        self
    }

    /// Returns this config with [`WorldConfig::sky`] set.
    #[must_use]
    pub const fn with_sky(mut self, sky: bool) -> Self {
        self.sky = sky;
        self
    }

    /// Returns this config with [`WorldConfig::sky_environment`] set.
    #[must_use]
    pub const fn with_sky_environment(mut self, environment: bool) -> Self {
        self.sky_environment = environment;
        self
    }

    /// The fog-of-war texture edge actually used.
    pub(crate) fn clamped_fog_resolution(self) -> u32 {
        self.fog_of_war_resolution
            .clamp(MIN_FOG_OF_WAR_RESOLUTION, MAX_FOG_OF_WAR_RESOLUTION)
    }

    /// The terrain layers sampled per pixel, `1..=8`.
    pub(crate) fn clamped_layers(self) -> u32 {
        self.terrain_layers.clamp(1, MAX_TERRAIN_LAYERS as u32)
    }
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self::for_tier(QualityTier::default())
    }
}

/// One vision source for the fog of war: a circle in render space (world `x`, `z`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct VisionSource {
    /// Render-space x.
    pub x: f32,
    /// Render-space z.
    pub z: f32,
    /// Vision radius in world units.
    pub radius: f32,
}

impl VisionSource {
    /// A source at `(x, z)` seeing `radius` world units.
    #[must_use]
    pub const fn new(x: f32, z: f32, radius: f32) -> Self {
        Self { x, z, radius }
    }
}

/// One static line-of-sight wall segment in render space (`[x, z]` endpoints).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct VisionWall {
    /// First endpoint.
    pub a: [f32; 2],
    /// Second endpoint.
    pub b: [f32; 2],
}

impl VisionWall {
    /// A wall from `a` to `b`.
    #[must_use]
    pub const fn new(a: [f32; 2], b: [f32; 2]) -> Self {
        Self { a, b }
    }
}

/// One frame's fog-of-war input (ADR-0014): the team's vision and the map's walls.
///
/// **Pass only what the client legitimately knows:** its own team's units (and allied
/// wards) as sources and the map's static walls. The texture is built from nothing else.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct FogOfWar<'a> {
    /// Vision sources; up to [`RenderConfig::max_vision_sources`](crate::RenderConfig::max_vision_sources)
    /// are used, the rest are dropped and counted ([`vision_sources_dropped`](crate::vision_sources_dropped)).
    pub sources: &'a [VisionSource],
    /// Line-of-sight walls; up to [`MAX_VISION_WALLS`].
    pub walls: &'a [VisionWall],
    /// Render-space `[x, z]` of the texture's first texel corner (the board's corner).
    pub origin: [f32; 2],
    /// Side of the square the texture covers, world units (the board's extent).
    pub size: f32,
    /// Seconds since the previous frame, for the reveal and fade (clamped to
    /// `0..=`[`FOG_OF_WAR_SETTLE_DT`], which settles the texture at once).
    pub dt: f32,
}

impl<'a> FogOfWar<'a> {
    /// Fog of war over the square at `origin` with side `size`, with no sources yet (all
    /// fogged) and a 60 Hz time step.
    #[must_use]
    pub const fn new(origin: [f32; 2], size: f32) -> Self {
        Self {
            sources: &[],
            walls: &[],
            origin,
            size,
            dt: 1.0 / 60.0,
        }
    }

    /// Sets the vision sources.
    #[must_use]
    pub const fn with_sources(mut self, sources: &'a [VisionSource]) -> Self {
        self.sources = sources;
        self
    }

    /// Sets the line-of-sight walls.
    #[must_use]
    pub const fn with_walls(mut self, walls: &'a [VisionWall]) -> Self {
        self.walls = walls;
        self
    }

    /// Sets the frame time step in seconds.
    #[must_use]
    pub const fn with_dt(mut self, dt: f32) -> Self {
        self.dt = dt;
        self
    }
}

/// Per-map atmosphere and fog-of-war art direction (ADR-0014), copied into the camera
/// uniform each frame. [`SceneAtmosphere::DEFAULT`] is a light haze, a blue gradient sky and
/// the pre-R3a fog-of-war darkening.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct SceneAtmosphere {
    /// Height-fog colour (linear RGB); also the sky's horizon blend.
    pub fog_color: Vec3,
    /// Height-fog density per metre at the ground; `0` turns height fog off for the scene.
    pub fog_density: f32,
    /// Height over which the density falls by `e`, metres.
    pub fog_height: f32,
    /// Distance from the eye before the fog starts, metres.
    pub fog_start: f32,
    /// Largest fog opacity (`0..=0.85`), so a distant unit never vanishes.
    pub fog_max: f32,
    /// Sky colour straight up (linear RGB).
    pub sky_zenith: Vec3,
    /// Sky colour at the horizon (linear RGB).
    pub sky_horizon: Vec3,
    /// Colour of unseen ground (linear RGB).
    pub fow_color: Vec3,
    /// How far unseen ground goes towards [`Self::fow_color`] (`0.2..=0.9`).
    pub fow_darken: f32,
    /// How much unseen ground is desaturated (`0..=1`).
    pub fow_desaturate: f32,
    /// Height scale of the ground mist in unseen areas, metres (`0` = no mist).
    pub fow_mist_height: f32,
    /// Ground-mist strength in unseen areas (`0..=1`).
    pub fow_mist: f32,
}

impl SceneAtmosphere {
    /// The defaults.
    pub const DEFAULT: Self = Self {
        fog_color: Vec3::new(0.38, 0.42, 0.50),
        fog_density: 0.0015,
        fog_height: 30.0,
        fog_start: 60.0,
        fog_max: 0.7,
        sky_zenith: Vec3::new(0.10, 0.17, 0.34),
        sky_horizon: Vec3::new(0.38, 0.42, 0.50),
        fow_color: Vec3::new(0.012, 0.020, 0.035),
        fow_darken: 0.72,
        fow_desaturate: 0.35,
        fow_mist_height: 0.0,
        fow_mist: 0.0,
    };

    /// Sets the height fog.
    #[must_use]
    pub const fn with_height_fog(
        mut self,
        color: Vec3,
        density: f32,
        height: f32,
        start: f32,
        max: f32,
    ) -> Self {
        self.fog_color = color;
        self.fog_density = density;
        self.fog_height = height;
        self.fog_start = start;
        self.fog_max = max;
        self
    }

    /// Sets the sky gradient.
    #[must_use]
    pub const fn with_sky(mut self, zenith: Vec3, horizon: Vec3) -> Self {
        self.sky_zenith = zenith;
        self.sky_horizon = horizon;
        self
    }

    /// Sets the fog of war's look.
    #[must_use]
    pub const fn with_fog_of_war_style(
        mut self,
        color: Vec3,
        darken: f32,
        desaturate: f32,
        mist_height: f32,
        mist: f32,
    ) -> Self {
        self.fow_color = color;
        self.fow_darken = darken;
        self.fow_desaturate = desaturate;
        self.fow_mist_height = mist_height;
        self.fow_mist = mist;
        self
    }
}

impl Default for SceneAtmosphere {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A map's terrain for [`Renderer::set_terrain`](crate::Renderer::set_terrain) (ADR-0014):
/// a square heightfield in render space and the splat-textured layers.
///
/// Samples are row-major with rows along +z: sample `(i, j)` is at
/// `origin + (i, j) × spacing` with height `heights[j × resolution + i]`.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct TerrainData<'a> {
    /// Samples per side (`2..=1025`).
    pub resolution: u32,
    /// Render-space `[x, z]` of sample `(0, 0)`.
    pub origin: [f32; 2],
    /// Distance between samples, world units.
    pub spacing: f32,
    /// `resolution²` heights, metres.
    pub heights: &'a [f32],
    /// `resolution²` baked relief ambient occlusion (`0..=1`), or empty for none.
    pub occlusion: &'a [f32],
    /// One or two linear RGBA splat maps (layers 1–4, 5–8) over the heightfield's square.
    pub splat: &'a [TextureData],
    /// Layer albedo (sRGB colour, ambient occlusion in alpha), 1..=8, one size and format.
    pub albedo: &'a [TextureData],
    /// Layer normal maps (BC5/RG8), empty or one per albedo layer.
    pub normal: &'a [TextureData],
    /// Per-layer linear tint (multiplies the albedo).
    pub tints: [[f32; 3]; MAX_TERRAIN_LAYERS],
    /// Per-layer tiling: metres per texture repeat.
    pub tiling: [f32; MAX_TERRAIN_LAYERS],
}

impl<'a> TerrainData<'a> {
    /// A terrain from its heightfield, splat maps and albedo layers, without normal maps,
    /// untinted and tiling every 8 m.
    #[must_use]
    pub const fn new(
        resolution: u32,
        origin: [f32; 2],
        spacing: f32,
        heights: &'a [f32],
        splat: &'a [TextureData],
        albedo: &'a [TextureData],
    ) -> Self {
        Self {
            resolution,
            origin,
            spacing,
            heights,
            occlusion: &[],
            splat,
            albedo,
            normal: &[],
            tints: [[1.0; 3]; MAX_TERRAIN_LAYERS],
            tiling: [8.0; MAX_TERRAIN_LAYERS],
        }
    }

    /// Sets the baked relief occlusion.
    #[must_use]
    pub const fn with_occlusion(mut self, occlusion: &'a [f32]) -> Self {
        self.occlusion = occlusion;
        self
    }

    /// Sets the layer normal maps.
    #[must_use]
    pub const fn with_normals(mut self, normal: &'a [TextureData]) -> Self {
        self.normal = normal;
        self
    }

    /// Sets layer `layer`'s tint and tiling (ignored past [`MAX_TERRAIN_LAYERS`]).
    #[must_use]
    pub const fn with_layer_style(mut self, layer: usize, tint: [f32; 3], tiling: f32) -> Self {
        if layer < MAX_TERRAIN_LAYERS {
            self.tints[layer] = tint;
            self.tiling[layer] = tiling;
        }
        self
    }

    /// Side of the square the terrain covers.
    #[must_use]
    pub fn size(&self) -> f32 {
        self.spacing * self.resolution.saturating_sub(1) as f32
    }
}

/// The camera-uniform values of the fog of war and the atmosphere for one view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WorldFrame {
    /// xy texture origin (render x, z), z 1 / size, w mode (0 off, 1 legacy, 2 texture).
    pub(crate) fog_rect: [f32; 4],
    /// rgb unseen colour, w darken.
    pub(crate) fog_color: [f32; 4],
    /// x desaturate, y mist height, z mist strength, w reserved.
    pub(crate) fog_params: [f32; 4],
    /// rgb height-fog colour, w density (0 = off).
    pub(crate) haze_color: [f32; 4],
    /// x 1 / height scale, y start distance, z max opacity, w 1 = the R2a distance fade.
    pub(crate) haze_params: [f32; 4],
    /// rgb zenith, w 1 = sample the environment cube.
    pub(crate) sky_zenith: [f32; 4],
    /// rgb horizon, w environment LOD.
    pub(crate) sky_horizon: [f32; 4],
}

/// The fog-of-war shader mode of a view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FogShaderMode {
    Off,
    Legacy,
    Texture,
}

impl FogShaderMode {
    const fn value(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Legacy => 1.0,
            Self::Texture => 2.0,
        }
    }
}

fn sanitize(value: f32, low: f32, high: f32) -> f32 {
    if value.is_finite() {
        value.clamp(low, high)
    } else {
        low
    }
}

fn rgb(color: Vec3, high: f32) -> [f32; 3] {
    [color.x, color.y, color.z].map(|channel| sanitize(channel, 0.0, high))
}

impl WorldFrame {
    /// A view with no fog of war, no height fog and no sky (offscreen views): the R2a
    /// distance fade stays off too, since offscreen cameras frame their subject exactly.
    pub(crate) const OFFSCREEN: Self = Self {
        fog_rect: [0.0; 4],
        fog_color: [0.0; 4],
        fog_params: [0.0; 4],
        haze_color: [0.0; 4],
        haze_params: [0.0; 4],
        sky_zenith: [0.0; 4],
        sky_horizon: [0.0; 4],
    };

    /// The main view's values: `atmosphere` sanitised, height fog when `config` has it,
    /// the fog-of-war `mode` with the texture square of `fog` (if any) and the sky's
    /// environment sampling when `environment_lod` is `Some`.
    pub(crate) fn main(
        atmosphere: &SceneAtmosphere,
        config: WorldConfig,
        mode: FogShaderMode,
        fog: Option<&FogOfWar<'_>>,
        environment_lod: Option<f32>,
    ) -> Self {
        let [fr, fg, fb] = rgb(atmosphere.fow_color, 1.0);
        let [hr, hg, hb] = rgb(atmosphere.fog_color, 4.0);
        let [zr, zg, zb] = rgb(atmosphere.sky_zenith, 8.0);
        let [hzr, hzg, hzb] = rgb(atmosphere.sky_horizon, 8.0);
        let density = if config.height_fog {
            sanitize(atmosphere.fog_density, 0.0, 0.2)
        } else {
            0.0
        };
        let (origin, inverse_size) =
            fog.filter(|_| mode == FogShaderMode::Texture)
                .map_or(([0.0, 0.0], 0.0), |fog| {
                    let size = sanitize(fog.size, 1.0, 65_536.0);
                    (
                        [
                            sanitize(fog.origin[0], -65_536.0, 65_536.0),
                            sanitize(fog.origin[1], -65_536.0, 65_536.0),
                        ],
                        1.0 / size,
                    )
                });
        let environment = environment_lod.filter(|_| config.sky_environment);
        Self {
            fog_rect: [origin[0], origin[1], inverse_size, mode.value()],
            fog_color: [fr, fg, fb, sanitize(atmosphere.fow_darken, 0.0, 0.95)],
            fog_params: [
                sanitize(atmosphere.fow_desaturate, 0.0, 1.0),
                sanitize(atmosphere.fow_mist_height, 0.0, 64.0),
                sanitize(atmosphere.fow_mist, 0.0, 1.0),
                0.0,
            ],
            haze_color: [hr, hg, hb, density],
            haze_params: [
                1.0 / sanitize(atmosphere.fog_height, 1.0, 1_000.0),
                sanitize(atmosphere.fog_start, 0.0, 10_000.0),
                sanitize(atmosphere.fog_max, 0.0, 0.85),
                if density > 0.0 { 0.0 } else { 1.0 },
            ],
            sky_zenith: [zr, zg, zb, f32::from(u8::from(environment.is_some()))],
            sky_horizon: [hzr, hzg, hzb, environment.unwrap_or(0.0)],
        }
    }
}

/// The CPU reference of the fog-of-war raster (`fow_raster.frag`) before blur and fade:
/// texel `(i, j)` covers `origin + ((i, j) + 0.5) × size / resolution`; it is visible
/// (`255`) when inside a source's disc (inset by one texel, fading over the 1.5 texels inside
/// that) and no wall crosses the segment from the source to the texel centre. Row-major, rows
/// along +z.
#[cfg(test)]
pub(crate) fn rasterize_reference(fog: &FogOfWar<'_>, resolution: u32) -> Vec<u8> {
    let resolution = resolution.max(1) as usize;
    let texel = fog.size / resolution as f32;
    let mut out = vec![0u8; resolution * resolution];
    for source in fog.sources {
        if source.radius <= 0.0 {
            continue;
        }
        let edge = 1.5 * texel;
        let outer = source.radius - texel;
        for j in 0..resolution {
            for i in 0..resolution {
                let x = fog.origin[0] + (i as f32 + 0.5) * texel;
                let z = fog.origin[1] + (j as f32 + 0.5) * texel;
                let distance = ((x - source.x).powi(2) + (z - source.z).powi(2)).sqrt();
                let t = ((distance - (outer - edge)) / edge).clamp(0.0, 1.0);
                let coverage = 1.0 - t * t * (3.0 - 2.0 * t);
                if coverage <= 0.0 {
                    continue;
                }
                let blocked = fog
                    .walls
                    .iter()
                    .any(|wall| segments_cross([source.x, source.z], [x, z], wall.a, wall.b));
                if !blocked {
                    let value = (coverage * 255.0).round() as u8;
                    let cell = &mut out[j * resolution + i];
                    *cell = (*cell).max(value);
                }
            }
        }
    }
    out
}

/// Whether the segment `from → to` crosses the wall `a → b` (the shader's test: strictly
/// inside the ray, inclusive on the wall).
#[cfg(test)]
fn segments_cross(from: [f32; 2], to: [f32; 2], a: [f32; 2], b: [f32; 2]) -> bool {
    let ray = [to[0] - from[0], to[1] - from[1]];
    let segment = [b[0] - a[0], b[1] - a[1]];
    let denominator = ray[0] * segment[1] - ray[1] * segment[0];
    if denominator.abs() < 1e-5 {
        return false;
    }
    let offset = [a[0] - from[0], a[1] - from[1]];
    let along_ray = (offset[0] * segment[1] - offset[1] * segment[0]) / denominator;
    let along_wall = (offset[0] * ray[1] - offset[1] * ray[0]) / denominator;
    along_ray > 0.0 && along_ray < 1.0 && (0.0..=1.0).contains(&along_wall)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The simulation's line-of-sight rule (`segments_intersect` in `moba-sim`'s
    /// `sim/vision.rs`), in `f64`: the sight line `a → b` is blocked by the wall `c → d` when
    /// each segment's endpoints lie strictly on different sides of the other's line (an
    /// endpoint on the line counts as the non-positive side).
    fn sim_blocks(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
        let cross = |u: [f64; 2], v: [f64; 2]| u[0] * v[1] - u[1] * v[0];
        let sub = |u: [f64; 2], v: [f64; 2]| [u[0] - v[0], u[1] - v[1]];
        let (ab, cd) = (sub(b, a), sub(d, c));
        let d1 = cross(ab, sub(c, a));
        let d2 = cross(ab, sub(d, a));
        let d3 = cross(cd, sub(a, c));
        let d4 = cross(cd, sub(b, c));
        ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0))
    }

    /// Review fix: the fog-of-war wall test (`segments_cross`, the CPU mirror of
    /// `fow_raster.frag`) blocks every sight line the simulation blocks, so the texture never
    /// shows ground behind a wall the server hides. Random lines in a 256 m board, on a
    /// 1/8 m grid too so endpoints land exactly on walls and lines.
    #[test]
    fn the_fog_wall_test_blocks_whatever_the_simulation_blocks() {
        let sim_source = include_str!("../../sim/src/sim/vision.rs");
        assert!(
            sim_source.contains(
                "((d1 > Fix::ZERO) != (d2 > Fix::ZERO)) && ((d3 > Fix::ZERO) != (d4 > Fix::ZERO))"
            ),
            "the simulation's rule changed: update sim_blocks"
        );
        let mut rng = moba_core::rng::Rng::from_seed(0x0f0f_2024);
        let mut coordinate = |grid: bool| {
            let unit = f64::from(rng.next_u32()) / f64::from(u32::MAX);
            let value = unit * 256.0;
            if grid {
                (value * 8.0).round() / 8.0
            } else {
                value
            }
        };
        let mut blocked = 0;
        for sample in 0..200_000 {
            let grid = sample % 2 == 0;
            let mut point = || [coordinate(grid), coordinate(grid)];
            let (from, to, wall_a, wall_b) = (point(), point(), point(), point());
            if !sim_blocks(from, to, wall_a, wall_b) {
                continue;
            }
            blocked += 1;
            let single = |p: [f64; 2]| [p[0] as f32, p[1] as f32];
            assert!(
                segments_cross(single(from), single(to), single(wall_a), single(wall_b)),
                "sim blocks {from:?} -> {to:?} by {wall_a:?} -> {wall_b:?}, the fog does not"
            );
        }
        assert!(blocked > 10_000, "{blocked} blocked samples");
    }

    #[test]
    fn tiers_scale_terrain_and_fog_of_war() {
        let low = WorldConfig::for_tier(QualityTier::Low);
        let ultra = WorldConfig::for_tier(QualityTier::Ultra);
        assert!(!low.terrain_normal_maps && !low.terrain_triplanar);
        assert_eq!(low.clamped_layers(), 4);
        assert!(ultra.terrain_triplanar && ultra.terrain_grid > low.terrain_grid);
        assert!(ultra.clamped_fog_resolution() > low.clamped_fog_resolution());
        for (index, &tier) in QualityTier::ALL.iter().enumerate() {
            let config = WorldConfig::for_tier(tier);
            assert_eq!(config.fog_of_war, FogOfWarMode::Texture);
            assert!(config.height_fog && config.sky);
            // content-check verifies the interpolated surface of exactly these grids, and the
            // generator's edge apron is one cell of the first (coarsest).
            assert_eq!(config.terrain_grid, moba_assets::TERRAIN_GRID_SIZES[index]);
        }
        let off = low
            .with_fog_of_war(FogOfWarMode::Off)
            .with_terrain(TerrainShading::Flat)
            .with_fog_of_war_resolution(4)
            .with_height_fog(false);
        assert_eq!(off.clamped_fog_resolution(), MIN_FOG_OF_WAR_RESOLUTION);
        assert_eq!(
            FogOfWarMode::from_name(" Legacy "),
            Some(FogOfWarMode::Legacy)
        );
        assert_eq!(
            TerrainShading::from_name("FLAT"),
            Some(TerrainShading::Flat)
        );
        assert_eq!(FogOfWarMode::from_name("circles"), None);
    }

    #[test]
    fn world_frame_sanitises_untrusted_art_direction() {
        let hostile = SceneAtmosphere::DEFAULT
            .with_height_fog(Vec3::new(f32::NAN, 9.0, -1.0), 5.0, 0.0, -3.0, 2.0)
            .with_fog_of_war_style(Vec3::splat(4.0), 2.0, -1.0, f32::INFINITY, 3.0);
        let fog = FogOfWar::new([-128.0, -128.0], 0.0);
        let frame = WorldFrame::main(
            &hostile,
            WorldConfig::default(),
            FogShaderMode::Texture,
            Some(&fog),
            Some(2.0),
        );
        assert_eq!(frame.haze_color, [0.0, 4.0, 0.0, 0.2]);
        assert_eq!(
            frame.haze_params[2], 0.85,
            "fog never hides a unit completely"
        );
        assert_eq!(
            frame.haze_params[3], 0.0,
            "height fog replaces the distance fade"
        );
        assert_eq!(frame.fog_color, [1.0, 1.0, 1.0, 0.95]);
        assert_eq!(frame.fog_params[..3], [0.0, 0.0, 1.0]);
        assert_eq!(frame.fog_rect, [-128.0, -128.0, 1.0, 2.0]);
        assert_eq!(frame.sky_zenith[3], 1.0);
        // Height fog off: the R2a fade comes back; legacy mode has no texture square.
        let plain = WorldFrame::main(
            &SceneAtmosphere::DEFAULT,
            WorldConfig::default().with_height_fog(false),
            FogShaderMode::Legacy,
            Some(&fog),
            None,
        );
        assert_eq!(plain.haze_color[3], 0.0);
        assert_eq!(plain.haze_params[3], 1.0);
        assert_eq!(plain.fog_rect, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(plain.sky_zenith[3], 0.0);
        assert_eq!(WorldFrame::OFFSCREEN.fog_rect[3], 0.0);
    }

    /// A 5v5 with towers, nexuses, wards and a minion wave: 158 sources (far past the legacy
    /// 16-circle cap) all land in the texture, and walls still cast line-of-sight shadows.
    #[test]
    fn fog_texture_handles_ten_heroes_wards_and_structures() {
        let mut sources = Vec::new();
        // Ten heroes (35 m vision) spread over a 256 m board centred on the origin.
        for hero in 0..10 {
            let t = hero as f32 / 10.0;
            sources.push(VisionSource::new(
                -100.0 + 200.0 * t,
                -60.0 + 40.0 * (t * 6.0).sin(),
                35.0,
            ));
        }
        // Six wards (small, stationary), 22 structures and 120 minions.
        for ward in 0..6 {
            sources.push(VisionSource::new(-90.0 + 36.0 * ward as f32, 80.0, 12.0));
        }
        for structure in 0..22 {
            sources.push(VisionSource::new(
                -110.0 + 10.0 * structure as f32,
                110.0,
                25.0,
            ));
        }
        for minion in 0..120 {
            sources.push(VisionSource::new(-120.0 + 2.0 * minion as f32, 0.0, 16.0));
        }
        assert_eq!(sources.len(), 158);
        let walls = [VisionWall::new([-20.0, 30.0], [20.0, 30.0])];
        let fog = FogOfWar::new([-128.0, -128.0], 256.0)
            .with_sources(&sources)
            .with_walls(&walls);
        let resolution = 256;
        let texture = rasterize_reference(&fog, resolution);
        let at = |x: f32, z: f32| {
            let i = ((x + 128.0) / 256.0 * resolution as f32) as usize;
            let j = ((z + 128.0) / 256.0 * resolution as f32) as usize;
            texture[j * resolution as usize + i]
        };
        // Every source sees its own position: none was capped.
        for source in &sources {
            assert_eq!(at(source.x, source.z), 255, "{source:?}");
        }
        // A ward's vision ends at its radius.
        assert_eq!(at(-90.0 - 14.0, 80.0), 0);
        // A lone source in front of the wall: behind it stays fogged, while the same
        // distance past the wall's end is visible.
        let lone_source = [VisionSource::new(0.0, 20.0, 35.0)];
        let blocked = FogOfWar::new([-128.0, -128.0], 256.0)
            .with_sources(&lone_source)
            .with_walls(&walls);
        let lone = rasterize_reference(&blocked, resolution);
        let lone_at = |x: f32, z: f32| {
            let i = ((x + 128.0) / 256.0 * resolution as f32) as usize;
            let j = ((z + 128.0) / 256.0 * resolution as f32) as usize;
            lone[j * resolution as usize + i]
        };
        assert_eq!(lone_at(0.0, 40.0), 0, "behind the wall");
        assert_eq!(lone_at(30.0, 25.0), 255, "past the wall's end");
        assert_eq!(lone_at(0.0, 10.0), 255, "in front of the wall");
    }

    #[test]
    fn the_texture_is_built_only_from_the_sources_given() {
        // No sources: everything is fogged, whatever else exists in the world.
        let walls = [VisionWall::new([0.0, 0.0], [10.0, 0.0])];
        let fog = FogOfWar::new([-64.0, -64.0], 128.0).with_walls(&walls);
        assert!(
            rasterize_reference(&fog, 64)
                .iter()
                .all(|&texel| texel == 0)
        );
    }
}
