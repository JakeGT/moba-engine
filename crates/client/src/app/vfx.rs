//! Client-side VFX runtime (ADR-0013): presentation events and animation sockets drive
//! content-defined effects whose emitters spawn GPU particles.
//!
//! Presentation only: effects read replicated state and presentation events and never feed
//! back into the simulation, the wire or replays. Randomness comes from a client-only RNG
//! stream with a constant seed (so a replay's effects look the same every run), never the
//! simulation's.
//!
//! Each frame [`VfxSystem::update`] releases the events whose tick the view has reached,
//! advances every live effect instance (its anchors follow units, sockets and projectiles)
//! and turns its emitters' bursts, rates, trails and beams into [`ParticleSpawn`]s for the
//! renderer. Budgets and readability rules apply here:
//!
//! - **distance and visibility culling**: an effect farther than the tier's
//!   [`VfxBudget::max_distance`] from the camera's focus on the ground (scaled up when the
//!   camera is zoomed out past the default 46 m), or off screen, spawns nothing that frame;
//! - **particle and overdraw budgets**: the system keeps a running estimate of live
//!   particles and of their projected pixel area (an expiry histogram, no per-particle
//!   state) and drops new spawns by importance as either nears the tier's budget: low at
//!   50 %, normal at 80 %, high at 95 %;
//! - **enemy telegraphs** (`telegraph = true` emitters of enemy effects) are never culled by
//!   distance, visibility or budget, and the GPU keeps them at least
//!   [`VfxBudget::telegraph_floor`] opaque. **Enemy projectiles** (every emitter of a
//!   projectile effect that is not an ally's) are exempt the same way: a skillshot is
//!   gameplay information. Exempt particles are put first in the frame's spawn list and may
//!   use the whole per-frame spawn cap; the rest stop at [`EXEMPT_RESERVE`] short of it;
//! - **projectile fallback**: a projectile whose effect did not resolve, or that spawned no
//!   particle in the last [`PLACEHOLDER_SECONDS`] (culled or over budget), is drawn as the
//!   pre-VFX cube ([`VfxSystem::placeholders`]), so it never becomes invisible;
//! - **allied effects** (not the viewer's own) are scaled by the allied-VFX opacity setting,
//!   and their alpha-blended particles are capped at [`ALLY_MAX_ALPHA`] so they never fully
//!   hide an enemy behind them;
//! - **effect lights** (an effect's optional `[light]` block, ADR-0016) become
//!   [`PointLight`]s for [`moba_render::Scene::with_lights`]: enemy and neutral lights first,
//!   then the viewer's own and allies' (allies scaled by the allied-VFX opacity, own effects
//!   never, as for particles), so the renderer's per-tier cap drops allied lights before
//!   enemy ones. A light belongs to an effect
//!   instance the client already plays, so it never shows anything the client was not sent.
//!
//! Every buffer is preallocated: steady-state frames do not allocate.

mod events;
mod library;

use moba_core::math::{Mat4, Quat, TAU, Vec3, Vec4};
use moba_core::rng::Rng;
use moba_render::{
    ParticleBlend, ParticleRole, ParticleShape, ParticleSpawn, PointLight, UnitInstance, VfxBudget,
    VfxFrame,
};

pub(crate) use events::{Trigger, VfxEvent, from_net, from_sim, world};
pub(crate) use library::{EffectId, VfxLibrary};

use moba_assets::{
    VfxAnchor, VfxBlend, VfxDirection, VfxEmitter, VfxImportance, VfxLight, VfxPrimitive, VfxShape,
};

/// Most live effect instances.
pub(crate) const MAX_INSTANCES: usize = 256;
/// Events waiting for the view to reach their tick.
const MAX_PENDING: usize = 128;
/// Casts remembered to bind a later projectile, hit or zone to its ability.
const MAX_CASTS: usize = 32;
/// Most particles one frame may emit (the renderer applies the tier's own cap).
pub(crate) const MAX_FRAME_SPAWNS: usize = 4_096;
/// Opacity cap of allied alpha-blended particles: they never fully hide what is behind.
pub(crate) const ALLY_MAX_ALPHA: f32 = 0.75;
/// Width of one bucket of the live-particle estimate, seconds.
const BUCKET_SECONDS: f32 = 0.1;
/// Buckets in the estimate: lifetimes up to 10 s, plus slack.
const BUCKETS_I64: i64 = 128;
const BUCKETS: usize = BUCKETS_I64 as usize;
/// Height of a projectile's flight above the ground, metres.
const PROJECTILE_HEIGHT: f32 = 1.2;
/// A trail emits a segment once its anchor moved this far, metres.
const TRAIL_STEP: f32 = 0.05;
/// A flying projectile that spawned no particle for this long draws its fallback cube.
pub(crate) const PLACEHOLDER_SECONDS: f32 = 0.1;
/// Spawns of the per-frame cap kept free for exempt (telegraph, enemy projectile) particles.
pub(crate) const EXEMPT_RESERVE: usize = 256;
/// Camera distance the tier's `max_distance` is tuned for (the default follow camera).
const DEFAULT_CAMERA_DISTANCE: f32 = 46.0;
/// Most projectile placeholders per frame.
const MAX_PLACEHOLDERS: usize = 64;
/// Anchor ids below this are found through a direct table; others by a scan.
const ANCHOR_TABLE: usize = 1 << 16;

/// Environment variable that turns VFX off (`off`/`0`).
pub(crate) const VFX_ENV: &str = "MOBA_VFX";
/// Environment variable naming an effect to preview (authoring mode).
pub(crate) const VFX_PREVIEW_ENV: &str = "MOBA_VFX_PREVIEW";
/// Environment variable multiplying every burst and rate (`1..=16`; budget stress tests).
pub(crate) const VFX_SCALE_ENV: &str = "MOBA_VFX_SCALE";

/// [`VFX_SCALE_ENV`], else 1.
pub(crate) fn vfx_scale_from_env() -> u32 {
    std::env::var(VFX_SCALE_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .map_or(1, |scale| scale.clamp(1, 16))
}

/// `false` when [`VFX_ENV`] is `off` or `0`.
pub(crate) fn vfx_enabled_from_env(setting: bool) -> bool {
    match std::env::var(VFX_ENV) {
        Ok(value) => !matches!(value.trim(), "off" | "0" | "false"),
        Err(_) => setting,
    }
}

/// A unit an effect can attach to this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct UnitAnchor {
    pub(crate) id: u32,
    /// Ground position.
    pub(crate) position: Vec3,
    /// Visual height, metres (effects attach at half of it).
    pub(crate) height: f32,
    /// Rig index and model-to-world transform of an animated hero (its sockets are
    /// relative to it).
    pub(crate) model: Option<(usize, Mat4)>,
}

/// This frame's units, rebuilt with the scene (preallocated). Ids below [`ANCHOR_TABLE`]
/// (every net id, and local entity indices) are looked up through a direct table.
#[derive(Debug)]
pub(crate) struct Anchors {
    units: Vec<UnitAnchor>,
    /// Index into `units` per id, or `u16::MAX`.
    index: Vec<u16>,
}

impl Anchors {
    pub(crate) fn new() -> Self {
        Self {
            units: Vec::with_capacity(moba_sim::MAX_UNITS + 16),
            index: vec![u16::MAX; ANCHOR_TABLE],
        }
    }

    pub(crate) fn clear(&mut self) {
        for unit in &self.units {
            if let Some(slot) = self.index.get_mut(unit.id as usize) {
                *slot = u16::MAX;
            }
        }
        self.units.clear();
    }

    /// Adds a unit; past the preallocated capacity it is ignored (no allocation).
    pub(crate) fn push(&mut self, anchor: UnitAnchor) {
        if self.units.len() < self.units.capacity() {
            if let (Some(slot), Ok(position)) = (
                self.index.get_mut(anchor.id as usize),
                u16::try_from(self.units.len()),
            ) {
                *slot = position;
            }
            self.units.push(anchor);
        }
    }

    pub(crate) fn get(&self, id: u32) -> Option<&UnitAnchor> {
        match self.index.get(id as usize) {
            Some(&u16::MAX) => None,
            Some(&slot) => self
                .units
                .get(usize::from(slot))
                .filter(|unit| unit.id == id),
            None => self.units.iter().find(|unit| unit.id == id),
        }
    }
}

/// How the viewer sees this frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VfxView {
    pub(crate) eye: Vec3,
    /// Where the view axis meets the ground: distances are culled from here.
    pub(crate) focus: Vec3,
    pub(crate) view_proj: Mat4,
    /// Viewport height in pixels (for the overdraw estimate).
    pub(crate) viewport: [f32; 2],
    /// The viewer's team; `None` for a spectator (no ally/enemy rules).
    pub(crate) team: Option<u8>,
    /// The viewer's own unit (its effects are never dimmed).
    pub(crate) own: Option<u32>,
}

impl VfxView {
    /// The view of `camera` with a `viewport` (pixels), the viewer's team and own unit.
    pub(crate) fn new(
        camera: &moba_render::Camera,
        viewport: [f32; 2],
        team: Option<u8>,
        own: Option<u32>,
    ) -> Self {
        Self {
            eye: camera.position,
            focus: ground_focus(camera.view_proj, camera.position),
            view_proj: camera.view_proj,
            viewport,
            team,
            own,
        }
    }

    /// The distance effects are culled at for `budget`: its `max_distance`, scaled up with
    /// the camera's distance from its focus past the default follow distance.
    fn cull_distance(&self, budget: &VfxBudget) -> f32 {
        let zoom = (self.eye.distance(self.focus) / DEFAULT_CAMERA_DISTANCE).max(1.0);
        budget.max_distance * zoom
    }
}

/// Where the centre of the view meets the ground plane (`y = 0`); the eye's ground point
/// when the view does not look down.
pub(crate) fn ground_focus(view_proj: Mat4, eye: Vec3) -> Vec3 {
    let inverse = view_proj.inverse();
    let far = inverse * Vec4::new(0.0, 0.0, 1.0, 1.0);
    let far = far.truncate() / far.w;
    let direction = far - eye;
    if direction.y < -1e-4 && eye.y > 0.0 {
        eye + direction * (eye.y / -direction.y)
    } else {
        Vec3::new(eye.x, 0.0, eye.z)
    }
}

/// CPU-side counters for logs and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct VfxCounters {
    pub(crate) instances: u32,
    pub(crate) spawned: u32,
    pub(crate) culled_distance: u32,
    pub(crate) culled_budget: u32,
    pub(crate) dropped_events: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Relation {
    Neutral,
    Own,
    Ally,
    Enemy,
}

#[derive(Clone, Copy, Debug)]
struct Flight {
    /// Ground-plane position at the flight height (the arc is added on top).
    position: Vec3,
    direction: Vec3,
    speed: f32,
    remaining: f32,
    total: f32,
    arc: f32,
    /// Collision radius (sizes the fallback cube).
    radius: f32,
}

#[derive(Clone, Copy, Debug)]
struct Instance {
    alive: bool,
    /// `None` for a projectile whose effect did not resolve: it only flies its fallback cube.
    effect: Option<EffectId>,
    /// The ability binding the instance plays for (projectiles keep it for their hit).
    ability: Option<u16>,
    /// Time of the last particle this instance spawned.
    last_spawn: f32,
    start: f32,
    emit: f32,
    end: f32,
    team: u8,
    relation: Relation,
    importance: VfxImportance,
    telegraph: bool,
    source: Option<u32>,
    /// The ability binding whose cast socket the source attaches to.
    socket: Option<u16>,
    target_unit: Option<u32>,
    target: Vec3,
    source_position: Vec3,
    flight: Option<Flight>,
    /// Anchor position in the previous frame (trails).
    previous: Option<Vec3>,
    radius: f32,
    tint: Vec3,
    rate_accumulator: [f32; moba_assets::MAX_VFX_EMITTERS],
    bursts: u8,
}

impl Instance {
    const DEAD: Self = Self {
        alive: false,
        effect: None,
        ability: None,
        last_spawn: f32::NEG_INFINITY,
        start: 0.0,
        emit: 0.0,
        end: 0.0,
        team: 0,
        relation: Relation::Neutral,
        importance: VfxImportance::Normal,
        telegraph: false,
        source: None,
        socket: None,
        target_unit: None,
        target: Vec3::ZERO,
        source_position: Vec3::ZERO,
        flight: None,
        previous: None,
        radius: 1.0,
        tint: Vec3::ONE,
        rate_accumulator: [0.0; moba_assets::MAX_VFX_EMITTERS],
        bursts: 0,
    };
}

/// Running estimate of live particles and their pixel area, bucketed by expiry time.
#[derive(Debug)]
struct LoadEstimate {
    counts: [u32; BUCKETS],
    pixels: [f32; BUCKETS],
    live: u32,
    live_pixels: f32,
    /// Absolute index of the oldest bucket still counted.
    cursor: i64,
}

impl LoadEstimate {
    fn new() -> Self {
        Self {
            counts: [0; BUCKETS],
            pixels: [0.0; BUCKETS],
            live: 0,
            live_pixels: 0.0,
            cursor: 0,
        }
    }

    fn bucket(time: f32) -> i64 {
        (time / BUCKET_SECONDS).floor() as i64
    }

    /// Retires every bucket whose particles have expired by `now`.
    fn advance(&mut self, now: f32) {
        let current = Self::bucket(now);
        let steps = (current - self.cursor).clamp(0, BUCKETS_I64);
        for step in 0..steps {
            let index = (self.cursor + step).rem_euclid(BUCKETS_I64) as usize;
            self.live = self.live.saturating_sub(self.counts[index]);
            self.live_pixels = (self.live_pixels - self.pixels[index]).max(0.0);
            self.counts[index] = 0;
            self.pixels[index] = 0.0;
        }
        self.cursor = current;
    }

    fn add(&mut self, expiry: f32, pixels: f32) {
        let bucket = Self::bucket(expiry)
            .clamp(self.cursor + 1, self.cursor + BUCKETS_I64 - 1)
            .rem_euclid(BUCKETS_I64) as usize;
        self.counts[bucket] += 1;
        self.pixels[bucket] += pixels;
        self.live += 1;
        self.live_pixels += pixels;
    }
}

/// The per-frame particle emitter state.
#[derive(Debug)]
pub(crate) struct VfxSystem {
    library: VfxLibrary,
    instances: Vec<Instance>,
    pending: Vec<(u32, VfxEvent)>,
    /// Events received before their snapshot's tick is known.
    unstamped: Vec<VfxEvent>,
    casts: [(u32, u16, f32); MAX_CASTS],
    cast_cursor: usize,
    spawns: Vec<ParticleSpawn>,
    /// This frame's non-exempt spawns, appended after the exempt ones in `spawns`.
    deferred: Vec<ParticleSpawn>,
    /// This frame's projectile fallback cubes: position, size and team.
    placeholders: Vec<(Vec3, f32, u8)>,
    /// This frame's effect lights: enemy and neutral first, then (appended at the end of
    /// `update`) `ally_lights`.
    lights: Vec<PointLight>,
    /// This frame's own and allied effect lights.
    ally_lights: Vec<PointLight>,
    rng: Rng,
    time: f32,
    dt: f32,
    budget: VfxBudget,
    enabled: bool,
    ally_opacity: f32,
    /// Emission multiplier ([`VFX_SCALE_ENV`]).
    scale: u32,
    load: LoadEstimate,
    counters: VfxCounters,
}

/// Uniform in `[low, high]`.
fn uniform(rng: &mut Rng, [low, high]: [f32; 2]) -> f32 {
    low + (high - low) * rng.next_f32()
}

fn random_unit(rng: &mut Rng) -> Vec3 {
    let z = rng.next_f32() * 2.0 - 1.0;
    let angle = rng.next_f32() * TAU;
    let r = (1.0 - z * z).max(0.0).sqrt();
    Vec3::new(r * angle.cos(), z, r * angle.sin())
}

/// A direction within `spread` degrees of `axis`.
fn cone(rng: &mut Rng, axis: Vec3, spread: f32) -> Vec3 {
    let axis = axis.normalize_or(Vec3::Y);
    let cos_max = spread.to_radians().cos();
    let z = cos_max + (1.0 - cos_max) * rng.next_f32();
    let angle = rng.next_f32() * TAU;
    let r = (1.0 - z * z).max(0.0).sqrt();
    let helper = if axis.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let u = axis.cross(helper).normalize();
    let v = axis.cross(u);
    (u * (r * angle.cos()) + v * (r * angle.sin()) + axis * z).normalize_or(axis)
}

/// The point light `light` casts at `age` seconds into `instance` (`None` while it is dark).
/// Flicker is a fixed function of time and the instance's slot: presentation only, so it
/// never touches the effect RNG stream.
fn effect_light(
    light: &VfxLight,
    instance: &Instance,
    age: f32,
    time: f32,
    slot: usize,
) -> Option<PointLight> {
    let envelope = light.envelope(age, instance.emit);
    if envelope <= 0.0 {
        return None;
    }
    let phase = slot as f32 * 1.618;
    let wobble = 0.5 + 0.25 * ((time * 23.0 + phase).sin() + (time * 13.7 + phase * 2.3).sin());
    let flicker = 1.0 - light.flicker * wobble.clamp(0.0, 1.0);
    let color = Vec3::from_array(light.color).lerp(team_color(instance.team), light.team_tint)
        * (light.intensity * envelope * flicker);
    let anchor = match light.anchor {
        VfxAnchor::Source => instance.source_position,
        _ => instance.target,
    };
    let radius = if light.scale_with_radius {
        light.radius * instance.radius.max(0.25)
    } else {
        light.radius
    };
    Some(PointLight::new(
        anchor + Vec3::from_array(light.offset),
        radius.min(moba_assets::MAX_VFX_LIGHT_RADIUS),
        color,
    ))
}

fn team_color(team: u8) -> Vec3 {
    if team == 0 {
        Vec3::new(0.25, 0.55, 1.0)
    } else {
        Vec3::new(1.0, 0.3, 0.22)
    }
}

/// Screen-space radius in pixels of a sphere at `center`, or `None` when it is off screen.
fn projected_radius(view: &VfxView, up: Vec3, center: Vec3, radius: f32) -> Option<f32> {
    let clip = view.view_proj * center.extend(1.0);
    if clip.w <= 1e-3 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    let edge = view.view_proj * (center + up * radius).extend(1.0);
    let r_ndc = (edge.truncate() / edge.w.max(1e-3) - ndc)
        .truncate()
        .length();
    let margin = 1.0 + r_ndc;
    (ndc.x.abs() <= margin && ndc.y.abs() <= margin && (0.0..=1.0).contains(&ndc.z))
        .then_some(r_ndc * 0.5 * view.viewport[1])
}

/// Everything one emitter needs to spawn this frame.
struct EmitContext {
    source: Vec3,
    target: Vec3,
    forward: Vec3,
    radius: f32,
    tint: Vec3,
    team: u8,
    relation: Relation,
    mesh_slot: Option<u8>,
}

impl VfxSystem {
    /// A system playing `library`'s effects. It stays disabled when `enabled` is false or the
    /// library has nothing to play (no bindings file, or a preview effect that did not load),
    /// so the pre-VFX projectile cubes keep drawing.
    pub(crate) fn new(library: VfxLibrary, budget: VfxBudget, enabled: bool) -> Self {
        let enabled = enabled && library.playable();
        Self {
            library,
            instances: vec![Instance::DEAD; MAX_INSTANCES],
            pending: Vec::with_capacity(MAX_PENDING),
            unstamped: Vec::with_capacity(moba_net::protocol::MAX_SNAPSHOT_EVENTS * 4),
            casts: [(u32::MAX, 0, f32::NEG_INFINITY); MAX_CASTS],
            cast_cursor: 0,
            spawns: Vec::with_capacity(MAX_FRAME_SPAWNS),
            deferred: Vec::with_capacity(MAX_FRAME_SPAWNS),
            placeholders: Vec::with_capacity(MAX_PLACEHOLDERS),
            lights: Vec::with_capacity(moba_render::MAX_LIGHTS),
            ally_lights: Vec::with_capacity(moba_render::MAX_LIGHTS),
            rng: Rng::from_seed(0x7f4a_7c15_9e37_79b9),
            time: 0.0,
            dt: 0.0,
            budget,
            enabled,
            ally_opacity: 1.0,
            scale: 1,
            load: LoadEstimate::new(),
            counters: VfxCounters::default(),
        }
    }

    pub(crate) fn library(&self) -> &VfxLibrary {
        &self.library
    }

    pub(crate) fn library_mut(&mut self) -> &mut VfxLibrary {
        &mut self.library
    }

    /// Whether effects spawn at all (setting, environment and renderer support).
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.clear();
        }
    }

    pub(crate) fn set_budget(&mut self, budget: VfxBudget) {
        self.budget = budget;
    }

    /// This frame's projectile fallback cubes (see the module docs): world position, cube
    /// height and team.
    pub(crate) fn placeholders(&self) -> &[(Vec3, f32, u8)] {
        &self.placeholders
    }

    pub(crate) fn set_ally_opacity(&mut self, opacity: f32) {
        self.ally_opacity = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
    }

    /// Multiplies every burst and rate by `scale` (`1..=16`; trails and beams excepted).
    pub(crate) fn set_scale(&mut self, scale: u32) {
        self.scale = scale.clamp(1, 16);
    }

    #[cfg_attr(not(test), allow(dead_code))] // Tests and debugging.
    pub(crate) fn counters(&self) -> VfxCounters {
        self.counters
    }

    /// Drops every effect and pending event (content reload, rematch).
    pub(crate) fn clear(&mut self) {
        self.instances.fill(Instance::DEAD);
        self.pending.clear();
        self.unstamped.clear();
        self.casts = [(u32::MAX, 0, f32::NEG_INFINITY); MAX_CASTS];
    }

    /// Queues a network event until its snapshot's tick is known ([`Self::stamp`]).
    pub(crate) fn queue_unstamped(&mut self, event: VfxEvent) {
        if !self.enabled {
            return;
        }
        if self.unstamped.len() < self.unstamped.capacity() {
            self.unstamped.push(event);
        } else {
            self.counters.dropped_events += 1;
        }
    }

    /// Stamps the queued events with the tick of the snapshot that carried them.
    pub(crate) fn stamp(&mut self, tick: u32) {
        for index in 0..self.unstamped.len() {
            let event = self.unstamped[index];
            self.queue(tick, event);
        }
        self.unstamped.clear();
    }

    /// Queues an event for the frame whose render tick reaches `tick`.
    pub(crate) fn queue(&mut self, tick: u32, event: VfxEvent) {
        if !self.enabled {
            return;
        }
        if self.pending.len() < self.pending.capacity() {
            self.pending.push((tick, event));
        } else {
            self.counters.dropped_events += 1;
        }
    }

    /// This frame's effect lights, most important first, for
    /// [`moba_render::Scene::with_lights`].
    pub(crate) fn lights(&self) -> &[PointLight] {
        &self.lights
    }

    /// The particles to spawn this frame, for [`moba_render::Scene::with_vfx`].
    pub(crate) fn frame(&self) -> VfxFrame<'_> {
        VfxFrame::new(&self.spawns, self.dt).with_ally_opacity(self.ally_opacity)
    }

    /// Releases the events up to `render_tick`, advances every effect by `dt` seconds and
    /// builds this frame's spawns. `socket` returns a unit's socket position by name.
    pub(crate) fn update(
        &mut self,
        dt: f32,
        render_tick: f32,
        view: &VfxView,
        anchors: &Anchors,
        socket: &dyn Fn(u32, &str) -> Option<Vec3>,
    ) {
        self.spawns.clear();
        self.deferred.clear();
        self.placeholders.clear();
        self.lights.clear();
        self.ally_lights.clear();
        self.counters.spawned = 0;
        let dt = if dt.is_finite() {
            dt.clamp(0.0, 0.1)
        } else {
            0.0
        };
        self.dt = dt;
        if !self.enabled {
            return;
        }
        self.time += dt;
        self.load.advance(self.time);
        // Release in arrival order (a cast before the projectile it fires), keeping the rest
        // in order too: a stable in-place compaction, no allocation.
        let mut kept = 0;
        for index in 0..self.pending.len() {
            let (tick, event) = self.pending[index];
            if tick as f32 <= render_tick + 0.01 {
                self.trigger(&event, view, anchors);
            } else {
                self.pending[kept] = (tick, event);
                kept += 1;
            }
        }
        self.pending.truncate(kept);
        let up = {
            let (_, up) = camera_up(view.view_proj);
            up
        };
        let mut live = 0;
        for slot in 0..self.instances.len() {
            if !self.instances[slot].alive {
                continue;
            }
            if self.time > self.instances[slot].end {
                self.instances[slot].alive = false;
                continue;
            }
            live += 1;
            self.advance_instance(slot, dt, view, anchors, socket, up);
        }
        self.counters.instances = live;
        // Exempt spawns first: past the renderer's per-frame cap, spawns drop in list order.
        let room = self.spawns.capacity() - self.spawns.len();
        let take = self.deferred.len().min(room);
        self.spawns.extend_from_slice(&self.deferred[..take]);
        let room = self.lights.capacity() - self.lights.len();
        let take = self.ally_lights.len().min(room);
        self.lights.extend_from_slice(&self.ally_lights[..take]);
    }

    /// Starts the effects an event triggers.
    pub(crate) fn trigger(&mut self, event: &VfxEvent, view: &VfxView, anchors: &Anchors) {
        let defaults = *self.library.defaults();
        let ability = match event.trigger {
            Trigger::Cast => {
                if let (Some(source), Some(binding)) = (event.source, event.ability) {
                    self.casts[self.cast_cursor] = (source, binding, self.time);
                    self.cast_cursor = (self.cast_cursor + 1) % MAX_CASTS;
                }
                event.ability
            }
            Trigger::ProjectileHit => {
                // The landing projectile's own ability, else the owner's latest cast.
                let landed = self.land_projectile(event);
                landed.or_else(|| event.source.and_then(|source| self.last_cast(source)))
            }
            Trigger::ProjectileSpawn | Trigger::Zone => {
                event.source.and_then(|source| self.last_cast(source))
            }
            _ => None,
        };
        let binding = ability.and_then(|index| self.library.ability_effects(index));
        let tint = binding.map_or(Vec3::ONE, |binding| binding.tint);
        let socket = binding
            .filter(|binding| binding.cast_socket.is_some())
            .and(ability);
        let effect = match event.trigger {
            Trigger::Cast => binding.and_then(|binding| binding.cast).or(defaults.cast),
            Trigger::ProjectileSpawn => binding
                .and_then(|binding| binding.projectile)
                .or(defaults.projectile),
            Trigger::ProjectileHit => binding.and_then(|binding| binding.hit).or(defaults.hit),
            Trigger::Zone => binding.and_then(|binding| binding.zone).or(defaults.zone),
            Trigger::HeroDeath => defaults.hero_death,
            Trigger::MinionDeath => defaults.minion_death,
            Trigger::StructureDestroyed => defaults.structure_destroyed,
            Trigger::Attack => defaults.attack,
        };
        let effect = event.effect.or(effect);
        // A projectile without an effect still flies its fallback cube.
        if effect.is_none() && event.trigger != Trigger::ProjectileSpawn {
            return;
        }
        self.spawn_instance(effect, ability, event, tint, socket, view, anchors);
    }

    /// The ability binding of `source`'s most recent cast.
    fn last_cast(&self, source: u32) -> Option<u16> {
        self.casts
            .iter()
            .filter(|(caster, _, _)| *caster == source)
            .max_by(|a, b| a.2.total_cmp(&b.2))
            .map(|&(_, binding, _)| binding)
    }

    /// Ends the flight of the source's projectile nearest the impact and returns its ability.
    fn land_projectile(&mut self, event: &VfxEvent) -> Option<u16> {
        let mut best: Option<(usize, f32)> = None;
        for (slot, instance) in self.instances.iter().enumerate() {
            let Some(flight) = instance.flight.filter(|_| instance.alive) else {
                continue;
            };
            if instance.source != event.source {
                continue;
            }
            let distance = flight
                .position
                .distance(event.position + Vec3::Y * PROJECTILE_HEIGHT);
            if best.is_none_or(|(_, nearest)| distance < nearest) {
                best = Some((slot, distance));
            }
        }
        let (slot, _) = best?;
        let instance = &mut self.instances[slot];
        if let Some(flight) = instance.flight.as_mut() {
            flight.remaining = 0.0;
        }
        // Stop emitting; live particles fade out (a bare fallback cube ends at once).
        instance.emit = instance.emit.min(self.time - instance.start);
        if instance.effect.is_none() {
            instance.alive = false;
        }
        instance.ability
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)] // One instance's inputs.
    fn spawn_instance(
        &mut self,
        effect: Option<EffectId>,
        ability: Option<u16>,
        event: &VfxEvent,
        tint: Vec3,
        socket: Option<u16>,
        view: &VfxView,
        anchors: &Anchors,
    ) {
        let loaded = effect.and_then(|effect| self.library.effect(effect));
        if loaded.is_none() && event.trigger != Trigger::ProjectileSpawn {
            return;
        }
        let (duration, importance, telegraph) =
            loaded.map_or((0.0, VfxImportance::High, false), |loaded| {
                (
                    loaded.effect.duration,
                    loaded.effect.importance,
                    loaded.effect.has_telegraph(),
                )
            });
        let Some(slot) = self.instances.iter().position(|instance| !instance.alive) else {
            self.counters.dropped_events += 1;
            return;
        };
        let relation = match view.team {
            None => Relation::Neutral,
            Some(_) if event.source.is_some() && event.source == view.own => Relation::Own,
            Some(team) if team == event.team => Relation::Ally,
            Some(_) => Relation::Enemy,
        };
        let flight = (event.trigger == Trigger::ProjectileSpawn).then(|| Flight {
            position: event.position + Vec3::Y * PROJECTILE_HEIGHT,
            direction: event.direction,
            speed: event.speed.max(0.0),
            remaining: event.position.distance(event.end),
            total: event.position.distance(event.end),
            arc: event.arc.clamp(0.0, 20.0),
            radius: event.radius.clamp(0.0, 20.0),
        });
        let flight_seconds = flight.map_or(0.0, |flight| {
            if flight.speed > 0.0 {
                (flight.remaining / flight.speed).min(moba_assets::MAX_EFFECT_SECONDS)
            } else {
                0.0
            }
        });
        let emit = match (flight.is_some(), loaded) {
            (true, _) => flight_seconds.max(duration),
            (false, Some(loaded)) => loaded.effect.emit_seconds(event.seconds),
            (false, None) => 0.0,
        };
        let lifespan = loaded.map_or(emit, |loaded| loaded.effect.lifespan(emit));
        // Units a death or attack names are attached to their mid-height.
        let unit_point = |id: Option<u32>| {
            id.and_then(|id| anchors.get(id))
                .map(|unit| unit.position + Vec3::Y * unit.height * 0.5)
        };
        let target = match event.trigger {
            Trigger::Attack | Trigger::ProjectileHit => {
                unit_point(event.target).unwrap_or(event.position + Vec3::Y)
            }
            _ => event.position,
        };
        if event.trigger == Trigger::Attack && unit_point(event.target).is_none() {
            return; // The attacked unit is not visible to this client.
        }
        let source_position = unit_point(event.source).unwrap_or(target);
        self.instances[slot] = Instance {
            alive: true,
            effect: loaded.and(effect),
            ability,
            last_spawn: f32::NEG_INFINITY,
            start: self.time,
            emit,
            end: self.time + lifespan.max(emit),
            team: event.team,
            relation,
            importance,
            telegraph,
            source: event.source,
            socket,
            target_unit: match event.trigger {
                Trigger::Attack | Trigger::ProjectileHit => event.target,
                _ => None,
            },
            target,
            source_position,
            flight,
            previous: None,
            radius: if event.radius > 0.0 {
                event.radius
            } else {
                1.0
            },
            tint,
            rate_accumulator: [0.0; moba_assets::MAX_VFX_EMITTERS],
            bursts: 0,
        };
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)] // One instance's frame.
    fn advance_instance(
        &mut self,
        slot: usize,
        dt: f32,
        view: &VfxView,
        anchors: &Anchors,
        socket: &dyn Fn(u32, &str) -> Option<Vec3>,
        up: Vec3,
    ) {
        let mut instance = self.instances[slot];
        let age = self.time - instance.start;
        let previous_age = age - dt;
        // Anchors follow their units, sockets and projectiles.
        if let Some(source) = instance.source {
            let socket_name = instance
                .socket
                .and_then(|index| self.library.ability_effects(index))
                .and_then(|binding| binding.cast_socket.as_deref());
            let position = socket_name
                .and_then(|name| socket(source, name))
                .or_else(|| {
                    anchors
                        .get(source)
                        .map(|unit| unit.position + Vec3::Y * unit.height * 0.5)
                });
            if let Some(position) = position {
                instance.source_position = position;
            }
        }
        if let Some(target) = instance
            .target_unit
            .and_then(|id| anchors.get(id))
            .map(|unit| unit.position + Vec3::Y * unit.height * 0.5)
        {
            instance.target = target;
        }
        let mut forward = (instance.target - instance.source_position) * Vec3::new(1.0, 0.0, 1.0);
        if let Some(flight) = instance.flight.as_mut() {
            let step = (flight.speed * dt).min(flight.remaining.max(0.0));
            flight.position += flight.direction * step;
            flight.remaining -= step;
            let progress = if flight.total > 0.0 {
                (1.0 - flight.remaining / flight.total).clamp(0.0, 1.0)
            } else {
                1.0
            };
            instance.target =
                flight.position + Vec3::Y * flight.arc * (core::f32::consts::PI * progress).sin();
            forward = flight.direction;
        }
        let anchor = instance.target;
        // Enemy (and, for spectators, every) projectile is gameplay information.
        let exempt_projectile = instance.flight.is_some()
            && matches!(instance.relation, Relation::Enemy | Relation::Neutral);
        let loaded = instance
            .effect
            .and_then(|effect| self.library.effect(effect));
        let Some(loaded) = loaded else {
            // A bare projectile: its fallback cube only.
            if let Some(flight) = instance.flight {
                self.placeholder(anchor, flight.radius, instance.team);
            }
            self.instances[slot] = instance;
            return;
        };
        // Culling: distance from the camera focus and visibility, never for telegraphs or
        // exempt projectiles.
        let distance = view.focus.distance(anchor);
        let limit = view.cull_distance(&self.budget);
        let visible = projected_radius(view, up, anchor, 4.0 * instance.radius.max(1.0)).is_some();
        let culled = !instance.telegraph && !exempt_projectile && (distance > limit || !visible);
        if culled && distance > limit {
            self.counters.culled_distance += 1;
        }
        let context = EmitContext {
            source: instance.source_position,
            target: anchor,
            forward: forward.normalize_or(Vec3::X),
            radius: instance.radius,
            tint: instance.tint,
            team: instance.team,
            relation: instance.relation,
            mesh_slot: None,
        };
        let emit_end = instance.emit;
        if let Some(light) = loaded.effect.light
            && distance <= limit
            && let Some(point) = effect_light(&light, &instance, age, self.time, slot)
        {
            let (list, scale) = match instance.relation {
                Relation::Enemy | Relation::Neutral => (&mut self.lights, 1.0),
                // Own effects are never dimmed (as for particles, `VfxFrame::ally_opacity`
                // scales allies only); they still list after enemies and neutrals.
                Relation::Own => (&mut self.ally_lights, 1.0),
                Relation::Ally => (&mut self.ally_lights, self.ally_opacity),
            };
            if list.len() < list.capacity() && scale > 0.0 {
                list.push(PointLight::new(
                    point.position,
                    point.radius,
                    point.color * scale,
                ));
            }
        }
        let moved = instance
            .previous
            .is_none_or(|previous| previous.distance(anchor) >= TRAIL_STEP);
        for (index, emitter) in loaded.effect.emitters.iter().enumerate() {
            let window_end = emitter.stop.map_or(emit_end, |stop| stop.min(emit_end));
            let mut count = 0u32;
            let bit = 1u8 << index;
            if instance.bursts & bit == 0 && age >= emitter.delay {
                instance.bursts |= bit;
                count += emitter.burst;
            }
            let active = age >= emitter.delay && previous_age < window_end;
            match emitter.primitive {
                VfxPrimitive::Trail => {
                    if active && moved && instance.previous.is_some() {
                        count = count.max(1);
                    }
                }
                VfxPrimitive::Beam => {
                    if active {
                        count = count.max(1);
                    }
                }
                _ => {
                    if active && emitter.rate > 0.0 {
                        let from = previous_age.max(emitter.delay);
                        let to = age.min(window_end);
                        let accumulator = &mut instance.rate_accumulator[index];
                        *accumulator += emitter.rate * (to - from).max(0.0);
                        let whole = accumulator.floor();
                        *accumulator -= whole;
                        count += whole as u32;
                    }
                }
            }
            if !matches!(emitter.primitive, VfxPrimitive::Trail | VfxPrimitive::Beam) {
                count = count.saturating_mul(self.scale);
            }
            if count == 0 || culled {
                continue;
            }
            let telegraph = emitter.telegraph && instance.relation == Relation::Enemy;
            let exempt = telegraph || exempt_projectile;
            if !exempt && !self.admit(instance.importance, view) {
                self.counters.culled_budget += count;
                continue;
            }
            let context = EmitContext {
                mesh_slot: loaded.mesh_slots[index],
                ..context
            };
            // Exempt spawns may fill the renderer's per-frame cap; the rest leave a reserve.
            let cap = self.budget.max_spawns_per_frame as usize;
            let reserve = if exempt {
                0
            } else {
                EXEMPT_RESERVE.min(cap / 4)
            };
            for _ in 0..count {
                let total = self.spawns.len() + self.deferred.len();
                if total + reserve >= cap || total >= self.spawns.capacity() {
                    self.counters.culled_budget += 1;
                    break;
                }
                let spawn = build_particle(
                    &mut self.rng,
                    emitter,
                    &context,
                    instance.previous,
                    telegraph,
                );
                let pixels = estimate_pixels(view, up, &spawn);
                self.load.add(self.time + spawn.lifetime, pixels);
                if exempt {
                    self.spawns.push(spawn);
                } else {
                    self.deferred.push(spawn);
                }
                self.counters.spawned += 1;
                instance.last_spawn = self.time;
            }
        }
        if moved {
            instance.previous = Some(anchor);
        }
        // A projectile that drew nothing lately (culled, over budget) keeps its cube.
        if let Some(flight) = instance.flight
            && flight.remaining > 0.0
            && self.time - instance.last_spawn > PLACEHOLDER_SECONDS
        {
            self.placeholder(anchor, flight.radius, instance.team);
        }
        self.instances[slot] = instance;
    }

    fn placeholder(&mut self, position: Vec3, radius: f32, team: u8) {
        if self.placeholders.len() < self.placeholders.capacity() {
            self.placeholders.push((position, 1.2 + radius * 0.2, team));
        }
    }

    /// Whether a spawn of `importance` fits the particle and overdraw budgets.
    fn admit(&self, importance: VfxImportance, view: &VfxView) -> bool {
        let particles = self.load.live as f32 / self.budget.max_particles.max(1) as f32;
        let screen = (view.viewport[0] * view.viewport[1]).max(1.0);
        let overdraw = self.load.live_pixels / screen / self.budget.max_overdraw.max(0.01);
        let pressure = particles.max(overdraw);
        let limit = match importance {
            VfxImportance::Low => 0.5,
            VfxImportance::High => 0.95,
            _ => 0.8,
        };
        pressure < limit
    }
}

/// Draws this frame's projectile fallback cubes (the pre-VFX look) into `units`, within its
/// capacity (no allocation).
pub(crate) fn push_placeholders(vfx: &VfxSystem, units: &mut Vec<UnitInstance>) {
    for &(position, height, team) in vfx.placeholders() {
        if units.len() >= units.capacity() {
            break;
        }
        let color = if team == 0 {
            Vec4::new(0.35, 0.72, 1.0, 1.0)
        } else {
            Vec4::new(1.0, 0.45, 0.3, 1.0)
        };
        units.push(UnitInstance::at(position, height, color));
    }
}

/// Camera right and up vectors from a view-projection matrix (screen right and up).
pub(crate) fn camera_up(view_proj: Mat4) -> (Vec3, Vec3) {
    let inverse = view_proj.inverse();
    let unproject = |x: f32, y: f32| {
        let point = inverse * Vec4::new(x, y, 0.5, 1.0);
        point.truncate() / point.w
    };
    let centre = unproject(0.0, 0.0);
    let right = (unproject(0.5, 0.0) - centre).normalize_or(Vec3::X);
    let up = (unproject(0.0, 0.5) - centre).normalize_or(Vec3::Y);
    (right, up)
}

fn estimate_pixels(view: &VfxView, up: Vec3, spawn: &ParticleSpawn) -> f32 {
    let size = spawn.size.max(spawn.size_end);
    match spawn.shape {
        ParticleShape::Segment => {
            let middle = spawn.position + spawn.segment * 0.5;
            let along = projected_radius(view, up, middle, spawn.segment.length() * 0.5);
            let across = projected_radius(view, up, middle, size * 0.5);
            along.zip(across).map_or(0.0, |(a, b)| 4.0 * a * b)
        }
        _ => projected_radius(view, up, spawn.position, size * 0.5).map_or(0.0, |r| 4.0 * r * r),
    }
}

/// One particle of `emitter`.
#[allow(clippy::too_many_lines)] // One table of emitter fields to particle fields.
fn build_particle(
    rng: &mut Rng,
    emitter: &VfxEmitter,
    context: &EmitContext,
    previous: Option<Vec3>,
    telegraph: bool,
) -> ParticleSpawn {
    let scale = if emitter.scale_with_radius {
        context.radius
    } else {
        1.0
    };
    let base = match emitter.anchor {
        VfxAnchor::Source => context.source,
        VfxAnchor::Line => context.source.lerp(context.target, rng.next_f32()),
        _ => context.target,
    };
    let radius = emitter.radius * scale;
    let local = match emitter.shape {
        VfxShape::Sphere => random_unit(rng) * radius * rng.next_f32().cbrt(),
        VfxShape::Disc | VfxShape::Circle => {
            let angle = rng.next_f32() * TAU;
            let r = if emitter.shape == VfxShape::Disc {
                radius * rng.next_f32().sqrt()
            } else {
                radius
            };
            Vec3::new(angle.cos() * r, 0.0, angle.sin() * r)
        }
        _ => Vec3::ZERO,
    };
    let offset = Vec3::from_array(emitter.offset);
    let position = base + offset + local;
    let direction = match emitter.direction {
        VfxDirection::Up => cone(rng, Vec3::Y, emitter.spread),
        VfxDirection::Random => random_unit(rng),
        VfxDirection::Forward => cone(rng, context.forward, emitter.spread),
        VfxDirection::None => Vec3::ZERO,
        _ => {
            if local.length_squared() > 1e-6 {
                local.normalize()
            } else {
                random_unit(rng)
            }
        }
    };
    let speed = uniform(rng, emitter.speed);
    let size = uniform(rng, emitter.size) * scale;
    let life = uniform(rng, emitter.lifetime);
    let team = team_color(context.team);
    let shade = |color: [f32; 4]| {
        let rgb = Vec3::new(color[0], color[1], color[2]) * context.tint;
        let intensity = rgb.max_element().max(1.0);
        let rgb = rgb.lerp(team * intensity, emitter.team_tint);
        rgb.extend(color[3])
    };
    let mut color = shade(emitter.color);
    let mut color_end = shade(emitter.color_end);
    let alpha = emitter.blend == VfxBlend::Alpha;
    let role = match context.relation {
        Relation::Ally => {
            if alpha {
                color.w = color.w.min(ALLY_MAX_ALPHA);
                color_end.w = color_end.w.min(ALLY_MAX_ALPHA);
            }
            ParticleRole::Ally
        }
        _ if telegraph => ParticleRole::EnemyTelegraph,
        _ => ParticleRole::Neutral,
    };
    let spin = uniform(rng, emitter.spin).to_radians();
    let mut spawn = ParticleSpawn::new(position, life, size, color)
        .with_velocity(direction * speed)
        .with_size_end(size * emitter.size_end)
        .with_color_end(color_end)
        .with_rotation(rng.next_f32() * TAU, spin)
        .with_forces(emitter.gravity, emitter.drag)
        .with_blend(if alpha {
            ParticleBlend::Alpha
        } else {
            ParticleBlend::Additive
        })
        .with_role(role);
    let sprite = emitter.sprite;
    spawn = match emitter.primitive {
        VfxPrimitive::Flipbook => spawn.with_sprite(sprite.cell, sprite.frames, emitter.fps),
        _ => spawn.with_sprite(sprite.cell, 1, 0.0),
    };
    let width = emitter.width * scale;
    match emitter.primitive {
        VfxPrimitive::Trail | VfxPrimitive::Beam => {
            let (from, to) = if emitter.primitive == VfxPrimitive::Trail {
                (previous.unwrap_or(context.target), context.target)
            } else {
                (context.source, context.target)
            };
            let mut segment = spawn
                .with_segment(to - from)
                .with_rotation(0.0, 0.0)
                .with_velocity(Vec3::ZERO)
                .with_forces(0.0, 0.0);
            segment.position = from + offset;
            segment.size = width;
            segment.size_end = width * emitter.size_end;
            segment
        }
        VfxPrimitive::Ring => {
            // Rings lie on the (flat) ground under their anchor, whatever its height.
            let mut ring = spawn.with_shape(ParticleShape::Ground);
            ring.position.y = 0.0;
            ring
        }
        VfxPrimitive::Mesh => match context.mesh_slot {
            Some(slot) => spawn.with_mesh(slot),
            None => spawn.with_sprite(5, 1, 0.0),
        },
        _ => spawn,
    }
}

/// The `MOBA_VFX_PREVIEW` authoring loop: an empty arena with a stand-in caster and target
/// where one effect replays as soon as the previous play ends.
#[derive(Debug)]
pub(crate) struct Preview {
    effect: EffectId,
    clock: f32,
    next: f32,
    probe: OutlineProbe,
}

/// `MOBA_OUTLINE_PROBE` (render tests, ADR-0016): an extra enemy-hero-outlined unit on the
/// far side of the arena, behind a wall (`hidden`) or in the open (`open`), or standing
/// beside the effect's target point under its light (`hit`, readability captures). With the wall,
/// a capture must be byte-identical to one with `MOBA_OUTLINES=off`: no outline pixel of a
/// unit fully behind a wall reaches the screen on either outline path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutlineProbe {
    /// No probe (the default).
    Off,
    /// The probe unit, fully behind a wall.
    Hidden,
    /// The probe unit in the open (the control: its outline must show).
    Open,
    /// The probe unit standing just behind the effect's target point, so the effect's light
    /// falls on its visible faces (readability: a hit unit keeps its team colour).
    Hit,
}

impl OutlineProbe {
    /// The environment variable.
    pub(crate) const ENV: &'static str = "MOBA_OUTLINE_PROBE";

    /// Parses `hidden`/`open`; anything else is off.
    pub(crate) fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("hidden") => Self::Hidden,
            Some("open") => Self::Open,
            Some("hit") => Self::Hit,
            _ => Self::Off,
        }
    }

    /// From [`Self::ENV`].
    pub(crate) fn from_env() -> Self {
        Self::parse(std::env::var(Self::ENV).ok().as_deref())
    }
}

/// The probe unit: on the far side of the arena from the preview camera (`Camera::orbit` at
/// yaw −0.7 puts the eye at +x, −z), and the wall 2.5 m in front of it, 12 m wide and 8 m
/// tall, square to the camera's heading: the view ray to the unit's top crosses the wall
/// below 5 m.
const PROBE_UNIT: Vec3 = Vec3::new(-6.0, 0.0, 6.0);
const PROBE_UNIT_HEIGHT: f32 = 3.6;
const PROBE_WALL_SIZE: Vec3 = Vec3::new(12.0, 8.0, 1.0);

/// The preview's stand-in caster, the effect's target point and a stand-in target unit
/// beside it (so the unit does not hide what plays at the point).
const PREVIEW_SOURCE: Vec3 = Vec3::new(-6.0, 0.0, 0.0);
const PREVIEW_TARGET: Vec3 = Vec3::new(6.0, 0.0, 0.0);
const PREVIEW_TARGET_UNIT: Vec3 = Vec3::new(6.0, 0.0, -6.0);

impl Preview {
    pub(crate) fn new(effect: EffectId) -> Self {
        Self {
            effect,
            clock: 0.0,
            next: 0.0,
            probe: OutlineProbe::Off,
        }
    }

    /// With the [`OutlineProbe`] staged.
    pub(crate) const fn with_probe(mut self, probe: OutlineProbe) -> Self {
        self.probe = probe;
        self
    }

    /// The probe's instances (none when off): the outlined unit and, when hidden, its wall.
    fn push_probe(&self, units: &mut Vec<UnitInstance>) {
        if self.probe == OutlineProbe::Off {
            return;
        }
        let position = if self.probe == OutlineProbe::Hit {
            // Just behind the effect's point as the camera sees it, so its lights (about
            // 1 m up) sit a metre in front of the faces the camera sees: the near field.
            PREVIEW_TARGET + Vec3::new(-1.2, 0.0, 1.0)
        } else {
            PROBE_UNIT
        };
        units.push(
            UnitInstance::at(position, PROBE_UNIT_HEIGHT, Vec4::new(1.0, 0.34, 0.28, 1.0))
                .with_outline(super::outlines::ENEMY_HERO),
        );
        if self.probe == OutlineProbe::Hidden {
            // Square to the eye's heading (towards +x, -z from the unit).
            let toward = Vec3::new(16.8, 0.0, -14.1).normalize();
            let center = PROBE_UNIT + toward * 2.5 + Vec3::Y * (PROBE_WALL_SIZE.y * 0.5);
            let yaw = toward.x.atan2(toward.z);
            units.push(UnitInstance::new(
                Mat4::from_scale_rotation_translation(
                    PROBE_WALL_SIZE,
                    Quat::from_rotation_y(yaw),
                    center,
                ),
                Vec4::new(0.45, 0.42, 0.4, 1.0),
            ));
        }
    }

    /// Plays the effect again at once (after a reload).
    pub(crate) fn restart(&mut self) {
        self.next = self.clock;
    }

    /// Draws the stand-ins, registers them as anchors and re-triggers the effect when the
    /// previous play has ended. A projectile effect (one with a trail) flies from the caster
    /// to the target, a zone effect (`event_duration`) lasts 3 s with a 4 m radius, and any
    /// other effect plays as a cast at the target.
    pub(crate) fn frame(
        &mut self,
        vfx: &mut VfxSystem,
        anchors: &mut Anchors,
        units: &mut Vec<UnitInstance>,
        frame_ms: f32,
    ) {
        self.clock += (frame_ms / 1_000.0).clamp(0.0, 0.1);
        self.push_probe(units);
        for (id, position, color) in [
            (1, PREVIEW_SOURCE, Vec4::new(0.28, 0.55, 1.0, 1.0)),
            (2, PREVIEW_TARGET_UNIT, Vec4::new(1.0, 0.34, 0.28, 1.0)),
        ] {
            units.push(UnitInstance::at(position, 3.6, color));
            anchors.push(UnitAnchor {
                id,
                position,
                height: 3.6,
                model: None,
            });
        }
        if self.clock < self.next {
            return;
        }
        let Some(loaded) = vfx.library().effect(self.effect) else {
            return;
        };
        let effect = &loaded.effect;
        let projectile = effect
            .emitters
            .iter()
            .any(|emitter| emitter.primitive == VfxPrimitive::Trail);
        let mut event = VfxEvent {
            trigger: Trigger::Cast,
            team: 1,
            source: Some(1),
            target: Some(2),
            position: PREVIEW_TARGET,
            end: PREVIEW_TARGET,
            direction: Vec3::X,
            speed: 14.0,
            radius: 1.0,
            seconds: 0.0,
            arc: 0.0,
            ability: None,
            effect: Some(self.effect),
        };
        let seconds = if projectile {
            event.trigger = Trigger::ProjectileSpawn;
            event.position = PREVIEW_SOURCE;
            PREVIEW_SOURCE.distance(PREVIEW_TARGET) / event.speed
        } else if effect.event_duration {
            event.trigger = Trigger::Zone;
            event.radius = 4.0;
            event.seconds = 3.0;
            3.0
        } else {
            0.0
        };
        let lifespan = effect.lifespan(seconds).max(seconds);
        vfx.queue(0, event);
        self.next = self.clock + lifespan + 0.6;
    }
}

#[cfg(test)]
mod tests;
