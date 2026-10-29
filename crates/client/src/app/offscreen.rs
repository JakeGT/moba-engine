//! Offscreen HUD images (R7b): 3D hero portraits and the baked top-down minimap.
//!
//! Both are rendered by the renderer's offscreen passes ([`OffscreenRender`]) with the scene
//! pipelines, then sampled by the HUD (`hud::images`). Everything here is presentation: it
//! reads replicated state the client already has (hero picks, teams and slots from the
//! snapshot, the bound map visuals) and never feeds the simulation.
//!
//! - **Portraits:** one 128-pixel cell per `(team, hero slot)` in a single-row strip, lit
//!   simply over a team-coloured backdrop. The strip is one orthographic "studio" view: cell
//!   `k`'s hero stands at `x_k` on a line across the camera, so one camera and one draw per
//!   mesh render every cell (a single row means a hero's lower body leaves the image instead
//!   of entering a neighbour's cell). Tiered ([`PortraitMode`]): `Low` renders once per
//!   change (static), higher tiers re-render a few times a second with each skinned hero's
//!   idle animation from its R4a animation graph (static mesh and procedural rig fallbacks).
//!   `MOBA_PORTRAITS=off|static|animated|<hz>` overrides the tier.
//! - **Minimap:** the terrain, walls and props of [`MapLibrary`] from straight above,
//!   orthographic, rendered once per map bind (startup, content update, map rotation).
//!   `MOBA_MINIMAP_BAKE=off|once|<frames>` (`<frames>` re-bakes every that many frames, for
//!   GPU-time measurement).
//!
//! Every buffer is sized at creation, so steady-state frames allocate nothing.

use moba_assets::{AnimationScratch, Animator};
use moba_core::math::Vec3;
use moba_core::{Fix, FixVec2};
use moba_render::{
    Camera, ClearRect, OFFSCREEN_MAX_BONES, OFFSCREEN_MAX_INSTANCES, OffscreenRender, QualityTier,
    Scene, UiImage, UnitInstance,
};

use crate::app::NetworkView;
use crate::app::map_visuals::MapLibrary;
use crate::app::models::ModelLibrary;
use crate::app::scene_build::{HeroPose, animated_rig, hero_transform, push_hero, team_from_u8};
use crate::app::skinning::BonePool;
use crate::hud::images::{HudImages, PORTRAIT_CELLS, portrait_cell};

/// Overrides the portrait tier: `off`, `static`, `animated` or an update rate in Hz.
pub(crate) const PORTRAITS_ENV: &str = "MOBA_PORTRAITS";
/// Minimap bake: `off`, `once` (default) or a re-bake interval in frames.
pub(crate) const MINIMAP_BAKE_ENV: &str = "MOBA_MINIMAP_BAKE";

/// World size of one portrait cell (metres): chest and head of a 3.6 m hero.
const CELL_WORLD: f32 = 2.3;
/// Height the portrait camera centres on.
const PORTRAIT_FOCUS: f32 = 2.75;
/// Downward pitch of the portrait camera (radians).
const PORTRAIT_PITCH: f32 = 0.17;
/// Turn of each hero away from the camera (radians), a three-quarter view.
const PORTRAIT_TURN: f32 = 0.35;
/// Grid-line strength of the baked minimap's terrain (the world uses 1).
const MINIMAP_GRID: f32 = 0.2;
/// Instances one portrait cell can add (the procedural rig's six parts).
const UNITS_PER_CELL: usize = 6;

/// How hero portraits update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PortraitMode {
    /// No portraits: the HUD keeps its text-only layout.
    Off,
    /// Rendered when the heroes change; posed at rest (`Low`).
    Static,
    /// Re-rendered `hz` times a second with each hero's idle animation.
    Animated { hz: f32 },
}

impl PortraitMode {
    /// The tier's default: `Low` static, `Medium` 5 Hz, `High` and `Ultra` 10 Hz.
    pub(crate) fn for_tier(tier: QualityTier) -> Self {
        match tier {
            QualityTier::Low => Self::Static,
            QualityTier::Medium => Self::Animated { hz: 5.0 },
            _ => Self::Animated { hz: 10.0 },
        }
    }

    /// Parses [`PORTRAITS_ENV`]: `off`/`0`, `static`, `animated`/`on` (the tier's rate, at
    /// least 5 Hz) or a rate in Hz (clamped to 1-60; `1` is 1 Hz).
    pub(crate) fn parse(spec: &str, tier: QualityTier) -> Option<Self> {
        match spec.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "none" => Some(Self::Off),
            "static" => Some(Self::Static),
            "animated" | "on" => Some(match Self::for_tier(tier) {
                Self::Animated { hz } => Self::Animated { hz },
                _ => Self::Animated { hz: 5.0 },
            }),
            other => other
                .parse::<f32>()
                .ok()
                .filter(|hz| hz.is_finite() && *hz > 0.0)
                .map(|hz| Self::Animated {
                    hz: hz.clamp(1.0, 60.0),
                }),
        }
    }

    /// [`PORTRAITS_ENV`] if set and valid, else the tier's default. Read once at startup.
    pub(crate) fn from_env(tier: QualityTier) -> Self {
        match std::env::var(PORTRAITS_ENV) {
            Ok(spec) => Self::parse(&spec, tier).unwrap_or_else(|| {
                eprintln!(
                    "{PORTRAITS_ENV}={spec:?} is not off/static/animated/<hz>; using the tier"
                );
                Self::for_tier(tier)
            }),
            Err(_) => Self::for_tier(tier),
        }
    }
}

/// `(hero pick, team)` shown in a cell.
type CellHero = Option<(u8, u8)>;

/// Builds the portrait strip's [`OffscreenRender`] when it is due.
pub(crate) struct PortraitStudio {
    mode: PortraitMode,
    /// Cells across the strip (its width over its height, at most [`PORTRAIT_CELLS`]).
    cells: usize,
    /// Strip size in pixels.
    extent: [u32; 2],
    units: Vec<UnitInstance>,
    bones: BonePool,
    backdrops: [ClearRect; PORTRAIT_CELLS],
    backdrop_count: usize,
    /// One animator per cell (idle only), allocated once.
    animators: Box<[Animator]>,
    scratch: Box<AnimationScratch>,
    /// What each cell held at the last submitted render, and its animation time.
    shown: [CellHero; PORTRAIT_CELLS],
    shown_time: Option<f32>,
    /// What the render built this frame holds (committed by [`Self::finish`]).
    building: [CellHero; PORTRAIT_CELLS],
    building_time: f32,
    pending: bool,
    camera: Camera,
}

impl PortraitStudio {
    pub(crate) fn new(mode: PortraitMode) -> Self {
        Self {
            mode,
            cells: 0,
            extent: [0, 0],
            units: Vec::with_capacity(PORTRAIT_CELLS * UNITS_PER_CELL),
            bones: BonePool::new(OFFSCREEN_MAX_BONES),
            backdrops: [ClearRect::new([0; 4], [0.0; 4]); PORTRAIT_CELLS],
            backdrop_count: 0,
            animators: (0..PORTRAIT_CELLS)
                .map(|cell| Animator::new(cell as f32 * 0.37))
                .collect(),
            scratch: Box::default(),
            shown: [None; PORTRAIT_CELLS],
            shown_time: None,
            building: [None; PORTRAIT_CELLS],
            building_time: 0.0,
            pending: false,
            camera: Camera::orthographic(Vec3::Z, Vec3::ZERO, Vec3::Y, 1.0, 1.0, 0.1, 1.0),
        }
    }

    /// The portrait mode (`Off` keeps the HUD text-only).
    pub(crate) fn mode(&self) -> PortraitMode {
        self.mode
    }

    /// Sets the strip size (the renderer's [`UiImage::PORTRAITS`] extent) and forgets what
    /// it held (a new renderer starts black).
    pub(crate) fn configure(&mut self, extent: [u32; 2]) {
        self.extent = extent;
        let [width, height] = extent;
        self.cells = if self.mode == PortraitMode::Off || height == 0 {
            0
        } else {
            usize::try_from(width / height)
                .unwrap_or(0)
                .min(PORTRAIT_CELLS)
        };
        // An orthographic strip: each `CELL_WORLD` of width is one square cell.
        let half_width = width as f32 / height.max(1) as f32 * CELL_WORLD * 0.5;
        let target = Vec3::new(0.0, PORTRAIT_FOCUS, 0.0);
        let back = Vec3::new(0.0, PORTRAIT_PITCH.sin(), PORTRAIT_PITCH.cos());
        let mut camera = Camera::orthographic(
            target + back * 40.0,
            target,
            Vec3::Y,
            half_width,
            CELL_WORLD * 0.5,
            1.0,
            80.0,
        );
        // A studio key light from the front, above and to the side.
        camera.light_direction = Vec3::new(0.85, 0.65, 0.45).normalize();
        camera.light_intensity = 1.0;
        camera.ambient = Vec3::new(0.13, 0.14, 0.19);
        self.camera = camera;
        self.invalidate();
    }

    /// Forces a re-render (content reload: hero models may have changed).
    pub(crate) fn invalidate(&mut self) {
        self.shown = [None; PORTRAIT_CELLS];
        self.shown_time = None;
    }

    /// What the HUD may draw: the cells the strip holds after this frame.
    pub(crate) fn hud(&self, images: &mut HudImages) {
        let held = if self.pending {
            &self.building
        } else {
            &self.shown
        };
        images.portrait_cells = u8::try_from(self.cells).unwrap_or(0);
        images.portraits = held
            .iter()
            .enumerate()
            .filter(|(_, hero)| hero.is_some())
            .fold(0u16, |mask, (cell, _)| mask | (1 << cell));
    }

    /// Decides whether the strip re-renders this frame and, if so, builds it. Returns
    /// whether [`Self::render`] should be sent. Never allocates.
    pub(crate) fn prepare(&mut self, network: &NetworkView, models: &ModelLibrary) -> bool {
        self.pending = false;
        if self.cells == 0 {
            return false;
        }
        let mut heroes: [CellHero; PORTRAIT_CELLS] = [None; PORTRAIT_CELLS];
        if let Some(snapshot) = network.client.snapshot() {
            for entity in snapshot.entities.iter().filter(|entity| entity.kind == 0) {
                if let Some(cell) =
                    portrait_cell(entity.team, entity.hero_slot).filter(|&cell| cell < self.cells)
                {
                    heroes[cell] = Some((entity.hero_pick, entity.team));
                }
            }
        }
        let pinned = network.pinned_tick().is_some();
        let now = match self.mode {
            PortraitMode::Animated { .. } => {
                network.render_tick() / f32::from(network.client.tick_rate().max(1))
            }
            _ => 0.0,
        };
        self.prepare_heroes(&heroes, models, now, pinned)
    }

    /// [`Self::prepare`] once the cells' heroes and the animation time (`0` unless
    /// animated) are known.
    fn prepare_heroes(
        &mut self,
        heroes: &[CellHero; PORTRAIT_CELLS],
        models: &ModelLibrary,
        now: f32,
        pinned: bool,
    ) -> bool {
        self.pending = false;
        if self.cells == 0 || !self.decide(heroes, now, pinned) {
            return false;
        }
        self.build(heroes, models, now, pinned);
        true
    }

    /// Whether the strip re-renders: the first time and whenever a cell's hero changes; when
    /// animated, also when `now` enters a new `1 / hz` period, and in a pinned capture at
    /// exactly its tick (so the captured frame repeats).
    fn decide(&self, heroes: &[CellHero; PORTRAIT_CELLS], now: f32, pinned: bool) -> bool {
        let changed = *heroes != self.shown || self.shown_time.is_none();
        let tick = match (self.mode, self.shown_time) {
            (PortraitMode::Animated { hz }, Some(last)) => {
                quantum(now, hz) != quantum(last, hz) || (pinned && now.to_bits() != last.to_bits())
            }
            _ => false,
        };
        changed || tick
    }

    fn build(
        &mut self,
        heroes: &[CellHero; PORTRAIT_CELLS],
        models: &ModelLibrary,
        now: f32,
        pinned: bool,
    ) {
        self.units.clear();
        self.bones.clear();
        self.backdrop_count = 0;
        let height = self.extent[1];
        let half_width = self.extent[0] as f32 / height.max(1) as f32 * CELL_WORLD * 0.5;
        for (cell, hero) in heroes.iter().enumerate().take(self.cells) {
            let Some((pick, team)) = *hero else {
                continue;
            };
            let Some(team_side) = team_from_u8(team) else {
                continue;
            };
            let backdrop = if team == 0 {
                [0.014, 0.03, 0.075, 1.0]
            } else {
                [0.075, 0.02, 0.014, 1.0]
            };
            let cell_px = u32::try_from(cell).unwrap_or(0) * height;
            self.backdrops[self.backdrop_count] =
                ClearRect::new([cell_px, 0, height, height], backdrop);
            self.backdrop_count += 1;
            let x = -half_width + (cell as f32 + 0.5) * CELL_WORLD;
            let model = models.hero(usize::from(pick));
            // `push_hero` turns a model by `pi/2 - facing` and the rig by `facing`.
            let facing = if model.is_some() {
                core::f32::consts::FRAC_PI_2 - PORTRAIT_TURN
            } else {
                PORTRAIT_TURN
            };
            let pose = HeroPose {
                position: FixVec2::new(Fix::from_f32(x), Fix::ZERO),
                team: team_side,
                health_fraction: 1.0,
                center: 0.0,
                time: now,
                phase_offset: cell as f32,
                facing,
                gait: 0.0,
            };
            // Skinned heroes with an animation graph play its idle (speed 0) through a
            // per-cell animator; the rest take `push_hero`'s fallbacks (the rest pose,
            // the static mesh or the procedural rig).
            let animated = match (animated_rig(models, model), model) {
                (Some(rig), Some(model)) => {
                    let animator = &mut self.animators[cell];
                    if pinned {
                        animator.reset_springs();
                    }
                    Some(animator.evaluate(
                        rig.graph,
                        rig.skeleton,
                        now,
                        0.0,
                        hero_transform(&model, &pose),
                        &mut self.scratch,
                    ))
                }
                _ => None,
            };
            push_hero(
                &mut self.units,
                &mut self.bones,
                models,
                model,
                &pose,
                animated,
            );
        }
        self.building = *heroes;
        self.building_time = now;
        self.pending = true;
    }

    /// This frame's render (after [`Self::prepare`] returned `true`).
    pub(crate) fn render(&self) -> OffscreenRender<'_> {
        let scene = Scene::new(self.camera, &self.units).with_bones(self.bones.bones());
        OffscreenRender::new(UiImage::PORTRAITS, scene)
            .with_clear_color([0.012, 0.014, 0.02, 1.0])
            .with_backdrops(&self.backdrops[..self.backdrop_count])
    }

    /// Commits a prepared render once its frame was submitted; a skipped frame retries.
    pub(crate) fn finish(&mut self, submitted: bool) {
        if self.pending && submitted {
            self.shown = self.building;
            self.shown_time = Some(self.building_time);
        }
        self.pending = false;
    }
}

/// The update period `time` falls in at `hz` updates a second.
fn quantum(time: f32, hz: f32) -> i64 {
    (f64::from(time) * f64::from(hz)).floor() as i64
}

/// How the minimap image is baked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MinimapMode {
    /// No bake: the HUD draws the flat minimap.
    Off,
    /// Once per map bind.
    Once,
    /// Also every `n` frames (GPU-time measurement).
    Every(u32),
}

impl MinimapMode {
    /// Parses [`MINIMAP_BAKE_ENV`].
    pub(crate) fn parse(spec: &str) -> Option<Self> {
        match spec.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "none" => Some(Self::Off),
            "once" | "on" | "1" => Some(Self::Once),
            other => other
                .parse::<u32>()
                .ok()
                .filter(|&frames| frames > 1)
                .map(Self::Every),
        }
    }

    /// [`MINIMAP_BAKE_ENV`] if set and valid, else [`MinimapMode::Once`]. Read once.
    pub(crate) fn from_env() -> Self {
        match std::env::var(MINIMAP_BAKE_ENV) {
            Ok(spec) => Self::parse(&spec).unwrap_or_else(|| {
                eprintln!("{MINIMAP_BAKE_ENV}={spec:?} is not off/once/<frames>; baking once");
                Self::Once
            }),
            Err(_) => Self::Once,
        }
    }
}

/// Builds the minimap's [`OffscreenRender`] when the map changed.
pub(crate) struct MinimapBaker {
    mode: MinimapMode,
    /// Map generation the image holds.
    baked: Option<u32>,
    /// Generation being baked this frame.
    building: Option<u32>,
    frames: u32,
    /// Walls and props of the bake; capacity [`OFFSCREEN_MAX_INSTANCES`], never grown.
    units: Vec<UnitInstance>,
    /// A map with more instances than one offscreen render takes is logged once.
    truncation_logged: bool,
    camera: Camera,
    terrain: [f32; 4],
}

impl MinimapBaker {
    pub(crate) fn new(mode: MinimapMode) -> Self {
        Self {
            mode,
            baked: None,
            building: None,
            frames: 0,
            units: Vec::with_capacity(OFFSCREEN_MAX_INSTANCES),
            truncation_logged: false,
            camera: Camera::orthographic(Vec3::Y, Vec3::ZERO, Vec3::NEG_Z, 1.0, 1.0, 0.1, 1.0),
            terrain: [0.0; 4],
        }
    }

    pub(crate) fn mode(&self) -> MinimapMode {
        self.mode
    }

    /// Forgets the bake (a new renderer starts black).
    pub(crate) fn invalidate(&mut self) {
        self.baked = None;
    }

    /// Whether the HUD may draw the image for map `generation` after this frame.
    pub(crate) fn ready(&self, generation: u32) -> bool {
        self.baked == Some(generation) || self.building == Some(generation)
    }

    /// Decides whether the minimap re-bakes this frame and, if so, builds it from `map`
    /// (whose map spans `extent` world units, centred on the origin). Never allocates.
    pub(crate) fn prepare(&mut self, map: &MapLibrary, extent: f32) -> bool {
        self.prepare_from(
            map.instances(),
            map.generation(),
            map.terrain_tint(),
            extent,
        )
    }

    /// [`Self::prepare`] from a map's walls and props, bind generation and terrain tint. At
    /// most [`OFFSCREEN_MAX_INSTANCES`] instances are kept (the rest are dropped and logged
    /// once), so the buffer never grows.
    fn prepare_from(
        &mut self,
        instances: &[UnitInstance],
        generation: u32,
        terrain: [f32; 4],
        extent: f32,
    ) -> bool {
        self.building = None;
        let periodic = match self.mode {
            MinimapMode::Off => return false,
            MinimapMode::Once => false,
            MinimapMode::Every(frames) => {
                self.frames = self.frames.wrapping_add(1);
                self.frames.is_multiple_of(frames)
            }
        };
        if self.baked == Some(generation) && !periodic {
            return false;
        }
        self.units.clear();
        let kept = instances.len().min(OFFSCREEN_MAX_INSTANCES);
        if kept < instances.len() && !self.truncation_logged {
            self.truncation_logged = true;
            eprintln!(
                "[render] minimap bake: {} map instances, drawing the first {kept}",
                instances.len()
            );
        }
        self.units.extend_from_slice(&instances[..kept]);
        let half = (extent * 0.5).max(1.0);
        // Straight down, map +y (render +z) towards the bottom of the image, like the HUD.
        let mut camera = Camera::orthographic(
            Vec3::new(0.0, 80.0, 0.0),
            Vec3::ZERO,
            Vec3::NEG_Z,
            half,
            half,
            1.0,
            200.0,
        );
        // `position` only feeds lighting and the terrain's distance fog: low enough that
        // the fog (from 240 m) never reaches the corners of a large map.
        camera.position = Vec3::new(0.0, 40.0, 0.0);
        camera.light_intensity = 1.05;
        camera.ambient = Vec3::new(0.42, 0.44, 0.5);
        self.camera = camera;
        // A faint grid: full-strength lines alias into noise at minimap scale.
        let [red, green, blue, _] = terrain;
        self.terrain = [red, green, blue, MINIMAP_GRID];
        self.building = Some(generation);
        true
    }

    /// This frame's bake (after [`Self::prepare`] returned `true`).
    pub(crate) fn render(&self) -> OffscreenRender<'_> {
        let scene = Scene::new(self.camera, &self.units).with_terrain_tint(self.terrain);
        OffscreenRender::new(UiImage::MINIMAP, scene)
            .with_terrain(true)
            .with_clear_color([0.012, 0.016, 0.02, 1.0])
    }

    /// Commits a bake once its frame was submitted; a skipped frame retries.
    pub(crate) fn finish(&mut self, submitted: bool) {
        if submitted && let Some(generation) = self.building {
            self.baked = Some(generation);
        }
        self.building = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MinimapBaker, MinimapMode, OFFSCREEN_MAX_INSTANCES, PORTRAIT_CELLS, PortraitMode,
        PortraitStudio,
    };
    use crate::app::models::{ModelBindings, ModelLibrary};
    use crate::hud::images::HudImages;
    use moba_core::math::{Mat4, Vec4};
    use moba_render::UnitInstance;
    use moba_render::{MeshId, QualityTier, UiImage};

    /// The skinned sample with its animation set (idle through the animator) in cell 0 and
    /// a hero without a model (the procedural rig) in cell 12; rebuilding the strip every
    /// update allocates nothing.
    #[test]
    fn portrait_strip_builds_each_cell_without_allocating() {
        let root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content"));
        let mut library = ModelLibrary::new(root, ModelBindings::default());
        library.bind_heroes([Some("sentinel-skinned"), None], |_, _| Ok(MeshId::CUBE));
        assert!(
            library.graph(0).is_some(),
            "the sample's animation set binds"
        );
        let mut studio = PortraitStudio::new(PortraitMode::Animated { hz: 10.0 });
        studio.configure([2048, 128]);
        let mut heroes = [None; PORTRAIT_CELLS];
        heroes[0] = Some((0, 0));
        heroes[12] = Some((1, 1));
        studio.build(&heroes, &library, 0.0, false);
        let render = studio.render();
        assert_eq!(render.image, UiImage::PORTRAITS);
        assert!(!render.terrain);
        assert_eq!(render.backdrops.len(), 2);
        assert_eq!(render.backdrops[1].rect, [12 * 128, 0, 128, 128]);
        let units = render.scene.units;
        assert_eq!(
            units.len(),
            1 + 6,
            "one skinned instance and the six-part rig"
        );
        assert!(units[0].bones.count > 0, "the skinned hero is posed");
        // Cell 0's hero stands left of cell 12's on the strip's line.
        assert!(units[0].transform.w_axis.x < units[1].transform.w_axis.x);
        let mut images = HudImages::default();
        studio.hud(&mut images);
        assert_eq!(images.portrait_cells, 16);
        assert_eq!(images.portraits, 1 | 1 << 12);
        studio.finish(true);

        let ((), allocations) = crate::app::allocation_probe::probe(|| {
            for update in 1..=20u8 {
                studio.build(&heroes, &library, f32::from(update) * 0.1, false);
                let _ = studio.render();
                studio.finish(true);
            }
        });
        assert_eq!(allocations, 0, "portrait updates allocated");
    }

    #[test]
    fn portrait_modes_follow_the_tier_and_the_override() {
        assert_eq!(
            PortraitMode::for_tier(QualityTier::Low),
            PortraitMode::Static
        );
        assert_eq!(
            PortraitMode::for_tier(QualityTier::High),
            PortraitMode::Animated { hz: 10.0 }
        );
        let parse = |spec| PortraitMode::parse(spec, QualityTier::Low);
        assert_eq!(parse("off"), Some(PortraitMode::Off));
        assert_eq!(parse(" Static "), Some(PortraitMode::Static));
        assert_eq!(parse("animated"), Some(PortraitMode::Animated { hz: 5.0 }));
        assert_eq!(parse("120"), Some(PortraitMode::Animated { hz: 60.0 }));
        // A number is a rate, `1` included (it is not an alias for `animated`).
        assert_eq!(parse("1"), Some(PortraitMode::Animated { hz: 1.0 }));
        assert_eq!(parse("on"), Some(PortraitMode::Animated { hz: 5.0 }));
        assert_eq!(parse("-1"), None);
        assert_eq!(parse("sometimes"), None);
    }

    fn rig_library() -> ModelLibrary {
        ModelLibrary::new(std::path::PathBuf::from("."), ModelBindings::default())
    }

    #[test]
    fn animated_portraits_update_per_period_retry_skips_and_repeat_pinned_ticks() {
        let library = rig_library();
        let mut studio = PortraitStudio::new(PortraitMode::Animated { hz: 10.0 });
        studio.configure([2048, 128]);
        let mut heroes = [None; PORTRAIT_CELLS];
        heroes[3] = Some((0, 0));
        // First render; a skipped frame (not submitted) renders again next frame.
        assert!(studio.prepare_heroes(&heroes, &library, 0.0, false));
        studio.finish(false);
        assert!(studio.prepare_heroes(&heroes, &library, 0.0, false));
        studio.finish(true);
        // Same 100 ms period: no update; the next period: an update.
        assert!(!studio.prepare_heroes(&heroes, &library, 0.05, false));
        studio.finish(true);
        assert!(studio.prepare_heroes(&heroes, &library, 0.1, false));
        studio.finish(true);
        // A pinned capture re-renders at its exact tick even inside the period, once.
        assert!(studio.prepare_heroes(&heroes, &library, 0.15, true));
        studio.finish(true);
        assert!(!studio.prepare_heroes(&heroes, &library, 0.15, true));
        studio.finish(true);
        // A hero change re-renders at once.
        heroes[11] = Some((1, 1));
        assert!(studio.prepare_heroes(&heroes, &library, 0.16, false));
        let mut images = HudImages::default();
        studio.hud(&mut images);
        assert_eq!(
            images.portraits,
            1 << 3 | 1 << 11,
            "the pending render's cells"
        );
        studio.finish(true);
        // Invalidation (a content reload) forces one.
        studio.invalidate();
        assert!(studio.prepare_heroes(&heroes, &library, 0.17, false));
    }

    #[test]
    fn static_portraits_render_only_on_change_and_off_never() {
        let library = rig_library();
        let mut studio = PortraitStudio::new(PortraitMode::Static);
        studio.configure([2048, 128]);
        let mut heroes = [None; PORTRAIT_CELLS];
        heroes[0] = Some((2, 0));
        assert!(studio.prepare_heroes(&heroes, &library, 0.0, false));
        studio.finish(true);
        for _ in 0..3 {
            assert!(!studio.prepare_heroes(&heroes, &library, 0.0, false));
            studio.finish(true);
        }
        heroes[0] = Some((3, 0));
        assert!(studio.prepare_heroes(&heroes, &library, 0.0, false));
        studio.finish(true);
        let mut images = HudImages::default();
        studio.hud(&mut images);
        assert_eq!(images.portraits, 1);

        let mut off = PortraitStudio::new(PortraitMode::Off);
        off.configure([2048, 128]);
        assert!(!off.prepare_heroes(&heroes, &library, 0.0, false));
        off.hud(&mut images);
        assert_eq!(
            images.portrait_cells, 0,
            "the HUD keeps its text-only layout"
        );
    }

    #[test]
    fn minimap_bakes_once_per_map_retries_skips_and_rebakes_periodically() {
        let mut once = MinimapBaker::new(MinimapMode::Once);
        assert!(!once.ready(0));
        assert!(once.prepare_from(&[], 0, [0.1; 4], 256.0));
        assert!(
            once.ready(0),
            "the image holds the map after this frame's bake"
        );
        once.finish(false);
        assert!(!once.ready(0), "a skipped frame baked nothing");
        assert!(once.prepare_from(&[], 0, [0.1; 4], 256.0), "retried");
        once.finish(true);
        assert!(!once.prepare_from(&[], 0, [0.1; 4], 256.0));
        once.finish(true);
        assert!(once.ready(0));
        // A new bind (content update, map rotation) bakes again.
        assert!(!once.ready(1));
        assert!(once.prepare_from(&[], 1, [0.1; 4], 256.0));
        once.finish(true);
        assert!(once.ready(1) && !once.ready(0));
        once.invalidate();
        assert!(
            once.prepare_from(&[], 1, [0.1; 4], 256.0),
            "a new renderer re-bakes"
        );

        let mut every = MinimapBaker::new(MinimapMode::Every(3));
        let bakes: Vec<bool> = (0..7)
            .map(|_| {
                let due = every.prepare_from(&[], 0, [0.1; 4], 256.0);
                every.finish(true);
                due
            })
            .collect();
        // The first frame bakes the map; then every third frame re-bakes.
        assert_eq!(bakes, [true, false, true, false, false, true, false]);

        let mut off = MinimapBaker::new(MinimapMode::Off);
        assert!(!off.prepare_from(&[], 0, [0.1; 4], 256.0));
        assert!(!off.ready(0));
    }

    #[test]
    fn minimap_bake_keeps_at_most_one_render_of_instances_without_growing() {
        let mut baker = MinimapBaker::new(MinimapMode::Once);
        let capacity = baker.units.capacity();
        let instance = UnitInstance::new(Mat4::IDENTITY, Vec4::ONE);
        let many = vec![instance; OFFSCREEN_MAX_INSTANCES + 76];
        assert!(baker.prepare_from(&many, 0, [0.1; 4], 256.0));
        assert_eq!(baker.render().scene.units.len(), OFFSCREEN_MAX_INSTANCES);
        assert_eq!(
            baker.units.capacity(),
            capacity,
            "the bake buffer never grows"
        );
        assert_eq!(baker.render().scene.terrain_tint[3], super::MINIMAP_GRID);
    }

    #[test]
    fn minimap_modes_parse() {
        assert_eq!(MinimapMode::parse("off"), Some(MinimapMode::Off));
        assert_eq!(MinimapMode::parse("once"), Some(MinimapMode::Once));
        assert_eq!(MinimapMode::parse("30"), Some(MinimapMode::Every(30)));
        assert_eq!(MinimapMode::parse("1"), Some(MinimapMode::Once));
        assert_eq!(MinimapMode::parse("x"), None);
    }
}
