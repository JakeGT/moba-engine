//! Client-side animation driver (ADR-0012, R4a).
//!
//! Animation is presentation only: it reads replicated state and presentation events and
//! never feeds back into the simulation, the wire or replays. For each skinned hero whose
//! model has an animation set (`<asset root>/animations/<model>.toml`), the driver keeps one
//! [`Animator`] and feeds it:
//!
//! - **locomotion** from the interpolated speed (the blend space picks idle ↔ run);
//! - **casts** from `EVENT_CAST` (`value` is the ability slot, whose catalogue ability name
//!   can pick a dedicated state), stamped with the tick of the snapshot that carried them;
//! - **hit reactions** (the additive `hit` state) when the hero's health drops, at most every
//!   0.3 s;
//! - **death** from `EVENT_UNIT_DIED` or, if that event was dropped, the dead flag; and
//!   **respawn** from the dead flag clearing (the simulation's `HeroRespawned` event is not
//!   replicated);
//! - **basic attacks** from `EVENT_ATTACK` (protocol v41): `source` hit `target` on the
//!   snapshot tick `tick - aux`, and can hit again `value` ticks later (its attack interval).
//!   The swing is stamped so its release frame lands on that tick, and the hero turns to its
//!   target. The interpolated view shows the tick `SNAPSHOT_DELAY_TICKS` after the event
//!   arrives, which is shorter than a wind-up, so the next swing is scheduled one attack
//!   interval ahead (the simulation's attack timer) to get its whole wind-up; it is
//!   withdrawn if no attack event confirms it. Melee and ranged attacks are the same: the
//!   simulation resolves both instantly, without a projectile.
//!
//! Time is the render tick in seconds, so a pinned capture tick (`MOBA_CAPTURE_TICK`) gives
//! a pose that does not depend on frame pacing; while pinned, spring bones restart from the
//! animated pose every frame and the LOD is bypassed.
//!
//! **LOD.** `MOBA_ANIM_LOD` sets the update-rate reduction: `off` evaluates every hero every
//! frame; `<near>,<far>` (metres from the camera eye, default `80,140`) evaluates heroes
//! within `near` every frame, up to `far` every 2nd frame, and farther or off-screen ones
//! every 4th frame. Skipped frames reuse the animator's last matrices.
//!
//! `MOBA_NO_ANIMATION_SETS=1` ignores every animation set (the WP-4.1 pose). The driver
//! preallocates everything; steady-state frames do not allocate.

use moba_assets::{AnimationEvent, AnimationGraph, AnimationScratch, Animator, Skeleton};
use moba_core::math::{Mat4, Vec3, Vec4};
use moba_net::protocol::{
    EVENT_ATTACK, EVENT_CAST, EVENT_UNIT_DIED, NetEvent, SnapshotState, WireAbility, WireHero,
};
use moba_render::JointMatrix;

/// Environment variable for the animation LOD (`off`, or `<near>,<far>` metres).
pub(crate) const ANIM_LOD_ENV: &str = "MOBA_ANIM_LOD";
/// Environment variable that ignores animation sets (`1`).
pub(crate) const NO_ANIMATION_SETS_ENV: &str = "MOBA_NO_ANIMATION_SETS";
/// Most heroes with an animator at once (10 players plus margin for rematches).
pub(crate) const MAX_ANIMATED: usize = 32;
/// Presentation events buffered until their snapshot's tick is known.
const MAX_QUEUED_EVENTS: usize = 64;
/// Locomotion phase offset per net id, seconds (as in the WP-4.1 pose).
const PHASE_SECONDS: f32 = 0.137;
/// A hero that moved less than this between snapshots is standing still, metres.
const STATIONARY: f32 = 0.05;
/// Fewest ticks between two hit reactions of one hero (damage over time ticks often).
const HIT_REACTION_TICKS: u32 = 9;
/// A snapshot step longer than this (metres) is a teleport (respawn), not a facing.
const TELEPORT: f32 = 3.0;
/// Seconds before the first snapshot that a hero first seen dead is stamped as having died,
/// so it is not drawn playing a death that happened before this client saw it.
const LONG_DEAD_SECONDS: f32 = 3_600.0;
/// Ticks past a scheduled hit after which a swing that no attack event confirmed is
/// withdrawn.
const PREDICTION_SLACK_TICKS: u32 = 1;
/// A confirmed hit this many ticks or fewer from the scheduled one is the same swing (the
/// animator ignores a repeat within 0.1 s).
const SAME_SWING_TICKS: u32 = 2;
/// Most entities whose previous health is remembered.
const MAX_TRACKED: usize = moba_sim::MAX_UNITS;

/// `true` unless [`NO_ANIMATION_SETS_ENV`] is `1`. Read once at startup.
pub(crate) fn animation_sets_enabled_from_env() -> bool {
    std::env::var(NO_ANIMATION_SETS_ENV).map_or(true, |value| value.trim() != "1")
}

/// Animation update-rate reduction by distance and visibility.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AnimLod {
    /// `None` when disabled.
    bands: Option<(f32, f32)>,
}

impl Default for AnimLod {
    fn default() -> Self {
        Self {
            bands: Some((80.0, 140.0)),
        }
    }
}

impl AnimLod {
    /// Parses `off` or `<near>,<far>`; anything else keeps the default (and is logged).
    pub(crate) fn parse(spec: &str) -> Self {
        let spec = spec.trim();
        if spec.eq_ignore_ascii_case("off") || spec == "0" {
            return Self { bands: None };
        }
        let parsed = spec.split_once(',').and_then(|(near, far)| {
            let near: f32 = near.trim().parse().ok()?;
            let far: f32 = far.trim().parse().ok()?;
            (near.is_finite() && far.is_finite() && near >= 0.0 && far >= near)
                .then_some((near, far))
        });
        if let Some(bands) = parsed {
            Self { bands: Some(bands) }
        } else {
            eprintln!("[anim] {ANIM_LOD_ENV}={spec} is not `off` or `<near>,<far>`; using 80,140");
            Self::default()
        }
    }

    /// From [`ANIM_LOD_ENV`], else the default.
    pub(crate) fn from_env() -> Self {
        std::env::var(ANIM_LOD_ENV).map_or_else(|_| Self::default(), |spec| Self::parse(&spec))
    }

    /// Frames between evaluations for a hero at `distance` metres from the eye.
    pub(crate) fn interval(self, distance: f32, on_screen: bool) -> u64 {
        let Some((near, far)) = self.bands else {
            return 1;
        };
        if !on_screen || !distance.is_finite() || distance > far {
            4
        } else if distance > near {
            2
        } else {
            1
        }
    }
}

/// The camera as the LOD sees it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LodView {
    pub(crate) eye: Vec3,
    pub(crate) view_proj: Mat4,
}

impl LodView {
    fn classify(&self, position: Vec3) -> (f32, bool) {
        let clip = self.view_proj * Vec4::new(position.x, position.y + 1.8, position.z, 1.0);
        let on_screen = clip.w > 1e-4 && {
            let ndc = clip.truncate() / clip.w;
            ndc.x.abs() <= 1.2 && ndc.y.abs() <= 1.2
        };
        (self.eye.distance(position), on_screen)
    }
}

/// One hero's animation state.
#[derive(Debug)]
struct Slot {
    net_id: Option<u16>,
    /// The rig the animator was set up for (a model change resets it).
    rig: usize,
    animator: Animator,
    dead: bool,
    /// Tick of the last hit reaction.
    flinched: Option<u32>,
    /// Tick of the next swing scheduled one attack interval after the last attack event,
    /// and that interval (seconds), until an attack event confirms or replaces it.
    predicted: Option<(u32, f32)>,
    /// Facing for a stationary hero: the last snapshot movement direction, or towards the
    /// target of its last attack (map x/y plane, radians).
    facing: Option<f32>,
    /// Whether a snapshot has shown this hero to the slot yet.
    seen: bool,
    /// Frame the animator last evaluated.
    evaluated: Option<u64>,
}

impl Slot {
    fn empty() -> Self {
        Self {
            net_id: None,
            rig: usize::MAX,
            animator: Animator::new(0.0),
            dead: false,
            flinched: None,
            predicted: None,
            facing: None,
            seen: false,
            evaluated: None,
        }
    }
}

/// One basic attack from an `EVENT_ATTACK`, ready to apply to its hero's slot.
#[derive(Clone, Copy, Debug)]
struct Swing {
    /// Snapshot tick the hit landed on.
    hit: u32,
    /// Ticks until the attacker can hit again (0 if unknown).
    interval: u16,
    /// Direction to the target, when known.
    facing: Option<f32>,
    /// The interpolated view's time now, seconds.
    view: f32,
    tick_seconds: f32,
}

impl Swing {
    fn apply(self, slot: &mut Slot, rig: RigRef<'_>) {
        if slot.dead {
            return;
        }
        let seconds = f32::from(self.interval) * self.tick_seconds;
        let event = AnimationEvent::Attack { interval: seconds };
        if let Some((scheduled, scheduled_seconds)) = slot.predicted.take()
            && scheduled.abs_diff(self.hit) > SAME_SWING_TICKS
        {
            // The attack came at another time than its timer said (the target left range
            // for a while): replace the scheduled swing.
            let scheduled_event = AnimationEvent::Attack {
                interval: scheduled_seconds,
            };
            slot.animator.cancel(
                rig.graph,
                scheduled_event,
                scheduled as f32 * self.tick_seconds,
                self.view,
            );
        }
        // Release frame on the hit; a scheduled swing for this hit is not restarted.
        slot.animator
            .trigger(rig.graph, event, self.hit as f32 * self.tick_seconds);
        if let Some(facing) = self.facing {
            slot.facing = Some(facing);
        }
        if self.interval > 0 {
            // The attack timer: the earliest next hit is one interval later. Scheduling it
            // now gives that swing its whole wind-up.
            let next = self.hit.saturating_add(u32::from(self.interval));
            slot.animator
                .trigger(rig.graph, event, next as f32 * self.tick_seconds);
            slot.predicted = Some((next, seconds));
        }
    }
}

/// What the driver needs to know about one hero's model.
#[derive(Clone, Copy)]
pub(crate) struct RigRef<'a> {
    pub(crate) index: usize,
    pub(crate) skeleton: &'a Skeleton,
    pub(crate) graph: &'a AnimationGraph,
}

/// Animators for the network view's heroes, plus the snapshot history that hit reactions
/// and facing need. Allocated once.
pub(crate) struct AnimationDriver {
    slots: Box<[Slot]>,
    scratch: Box<AnimationScratch>,
    /// `(net id, health, x, y)` of the previous snapshot, sorted by net id.
    previous: Vec<(u16, u16, u16, u16)>,
    current: Vec<(u16, u16, u16, u16)>,
    queued: [NetEvent; MAX_QUEUED_EVENTS],
    queued_len: usize,
    lod: AnimLod,
    frame: u64,
    /// Seconds per tick (1 / the server tick rate).
    tick_seconds: f32,
}

impl std::fmt::Debug for AnimationDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnimationDriver")
            .field(
                "animated",
                &self
                    .slots
                    .iter()
                    .filter(|slot| slot.net_id.is_some())
                    .count(),
            )
            .field("lod", &self.lod)
            .finish_non_exhaustive()
    }
}

impl AnimationDriver {
    pub(crate) fn new(lod: AnimLod, tick_rate: u16) -> Self {
        Self {
            slots: (0..MAX_ANIMATED).map(|_| Slot::empty()).collect(),
            scratch: Box::default(),
            previous: Vec::with_capacity(MAX_TRACKED),
            current: Vec::with_capacity(MAX_TRACKED),
            queued: [NetEvent::default(); MAX_QUEUED_EVENTS],
            queued_len: 0,
            lod,
            frame: 0,
            tick_seconds: 1.0 / f32::from(tick_rate.max(1)),
        }
    }

    /// Forgets every hero (a rematch or a content reload renumbers net ids).
    pub(crate) fn clear(&mut self) {
        for slot in &mut self.slots {
            *slot = Slot::empty();
        }
        self.previous.clear();
        self.current.clear();
        self.queued_len = 0;
    }

    /// Buffers an attack, cast or death event until [`Self::flush_events`] learns its tick.
    pub(crate) fn queue_event(&mut self, event: &NetEvent) {
        if !matches!(event.kind, EVENT_ATTACK | EVENT_CAST | EVENT_UNIT_DIED) {
            return;
        }
        if let Some(slot) = self.queued.get_mut(self.queued_len) {
            *slot = *event;
            self.queued_len += 1;
        }
    }

    fn slot_index(&self, net_id: u16) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.net_id == Some(net_id))
    }

    /// The slot for `net_id`, claiming a free one (or resetting it for a new rig).
    fn claim(slots: &mut [Slot], net_id: u16, rig: usize) -> Option<&mut Slot> {
        let index = slots
            .iter()
            .position(|slot| slot.net_id == Some(net_id))
            .or_else(|| slots.iter().position(|slot| slot.net_id.is_none()))?;
        let slot = &mut slots[index];
        if slot.net_id != Some(net_id) || slot.rig != rig {
            *slot = Slot {
                net_id: Some(net_id),
                rig,
                animator: Animator::new(f32::from(net_id) * PHASE_SECONDS),
                ..Slot::empty()
            };
        }
        Some(slot)
    }

    /// Applies the queued events of the snapshot at `tick`, then withdraws scheduled swings
    /// that no attack event confirmed. `rig_of` resolves a hero's net id to its rig (`None`
    /// for anything else, so a minion's attack or death claims no animator), and
    /// `ability_name` a `(net id, slot)` cast to its catalogue ability name.
    pub(crate) fn flush_events<'a>(
        &mut self,
        tick: u32,
        mut rig_of: impl FnMut(u16) -> Option<RigRef<'a>>,
        mut ability_name: impl FnMut(u16, u8) -> Option<&'a str>,
    ) {
        let at = tick as f32 * self.tick_seconds;
        // Where the interpolated view is now: a withdrawn swing fades out from there.
        let view = (tick as f32 - super::SNAPSHOT_DELAY_TICKS) * self.tick_seconds;
        for index in 0..self.queued_len {
            let event = self.queued[index];
            if event.kind == EVENT_ATTACK {
                let Some(rig) = rig_of(event.source) else {
                    continue;
                };
                let hit = tick.saturating_sub(u32::from(event.aux));
                let facing = self.facing_towards(event.source, event.target);
                let tick_seconds = self.tick_seconds;
                if let Some(slot) = Self::claim(&mut self.slots, event.source, rig.index) {
                    let swing = Swing {
                        hit,
                        interval: event.value,
                        facing,
                        view,
                        tick_seconds,
                    };
                    swing.apply(slot, rig);
                }
                continue;
            }
            let (net_id, animation) = match event.kind {
                EVENT_CAST => {
                    let slot = u8::try_from(event.value).unwrap_or(u8::MAX);
                    (
                        event.source,
                        AnimationEvent::Cast {
                            slot,
                            ability: ability_name(event.source, slot),
                        },
                    )
                }
                EVENT_UNIT_DIED => (event.target, AnimationEvent::Death),
                _ => continue,
            };
            let Some(rig) = rig_of(net_id) else {
                continue;
            };
            if let Some(slot) = Self::claim(&mut self.slots, net_id, rig.index) {
                if matches!(animation, AnimationEvent::Death) {
                    slot.dead = true;
                    slot.predicted = None;
                }
                slot.animator.trigger(rig.graph, animation, at);
            }
        }
        self.queued_len = 0;
        // A scheduled swing whose attack event did not come (the fight ended, the target
        // died, the event was lost) is withdrawn: dropped if still queued, else faded out
        // from where the view is, which is usually still in the wind-up.
        for slot in &mut self.slots {
            let Some((hit, seconds)) = slot.predicted else {
                continue;
            };
            if tick < hit.saturating_add(PREDICTION_SLACK_TICKS) {
                continue;
            }
            let Some(rig) = slot.net_id.and_then(&mut rig_of) else {
                slot.predicted = None;
                continue;
            };
            let event = AnimationEvent::Attack { interval: seconds };
            slot.animator
                .cancel(rig.graph, event, hit as f32 * self.tick_seconds, view);
            slot.predicted = None;
        }
    }

    /// The direction from `source` to `target` in the last snapshot (map x/y plane,
    /// radians), when both are known and apart.
    fn facing_towards(&self, source: u16, target: u16) -> Option<f32> {
        let (_, _, sx, sy) = lookup(&self.previous, source)?;
        let (_, _, tx, ty) = lookup(&self.previous, target)?;
        let from = moba_net::protocol::dequantize_position(sx, sy);
        let to = moba_net::protocol::dequantize_position(tx, ty);
        let (dx, dy) = ((to.x - from.x).to_f32(), (to.y - from.y).to_f32());
        (dx * dx + dy * dy > 1e-6).then(|| dy.atan2(dx))
    }

    /// Updates death/respawn edges, hit reactions and movement facing from a new snapshot.
    ///
    /// `rig_of` maps a hero pick to its rig.
    pub(crate) fn on_snapshot<'a>(
        &mut self,
        snapshot: &SnapshotState,
        mut rig_of: impl FnMut(u8) -> Option<RigRef<'a>>,
    ) {
        let tick = snapshot.tick;
        let at = tick as f32 * self.tick_seconds;
        self.current.clear();
        for entity in &snapshot.entities {
            if self.current.len() < self.current.capacity() {
                self.current
                    .push((entity.net_id, entity.health, entity.x, entity.y));
            }
        }
        // Snapshots are sorted by net id already; sorting keeps `lookup` correct regardless.
        self.current.sort_unstable_by_key(|entry| entry.0);
        for hero in snapshot.entities.iter().filter(|entity| entity.kind == 0) {
            let Some(rig) = rig_of(hero.hero_pick) else {
                continue;
            };
            let previous = lookup(&self.previous, hero.net_id);
            let Some(slot) = Self::claim(&mut self.slots, hero.net_id, rig.index) else {
                continue;
            };
            let first_sight = !slot.seen;
            slot.seen = true;
            let dead = hero.is_dead();
            if let Some((_, _, x, y)) = previous {
                let (from, to) = (
                    moba_net::protocol::dequantize_position(x, y),
                    hero.position(),
                );
                let (dx, dy) = ((to.x - from.x).to_f32(), (to.y - from.y).to_f32());
                let step = (dx * dx + dy * dy).sqrt();
                if step > STATIONARY && step <= TELEPORT {
                    slot.facing = Some(dy.atan2(dx));
                }
            }
            if dead && !slot.dead {
                // A hero already dead when this client first sees it (a mid-match join, a
                // reconnect, a reload) is marked dead without replaying its death.
                let died = if first_sight {
                    at - LONG_DEAD_SECONDS
                } else {
                    at
                };
                slot.animator
                    .trigger(rig.graph, AnimationEvent::Death, died);
                slot.predicted = None;
            } else if !dead && slot.dead {
                slot.animator
                    .trigger(rig.graph, AnimationEvent::Respawn, at);
                slot.predicted = None;
            }
            slot.dead = dead;
            if dead {
                continue;
            }
            // Taking damage plays the additive hit reaction, at most every 0.3 s.
            let hurt = previous.is_some_and(|(_, health, _, _)| hero.health < health);
            if hurt
                && slot
                    .flinched
                    .is_none_or(|last| tick.saturating_sub(last) >= HIT_REACTION_TICKS)
            {
                slot.animator.trigger(rig.graph, AnimationEvent::Hit, at);
                slot.flinched = Some(tick);
            }
        }
        core::mem::swap(&mut self.previous, &mut self.current);
    }

    /// Starts a frame (LOD bookkeeping).
    pub(crate) fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// The facing to draw a stationary hero with: its last snapshot movement direction, or
    /// towards its last attack's target. `None` for a hero without an animator.
    pub(crate) fn rest_facing(&self, net_id: u16) -> Option<f32> {
        self.slot_index(net_id)
            .and_then(|index| self.slots[index].facing)
    }

    /// Whether a hero should be drawn at render time `now` (seconds): alive, or dead while
    /// its death clip and linger last. Heroes without an animator are drawn when alive.
    pub(crate) fn visible(
        &self,
        net_id: u16,
        graph: &AnimationGraph,
        now: f32,
        dead: bool,
    ) -> bool {
        match self.slot_index(net_id) {
            Some(index) => self.slots[index].animator.visible(graph, now),
            None => !dead,
        }
    }

    /// Skinning matrices for one hero this frame: evaluated, or the last ones when the LOD
    /// skips this frame. `pinned` (a capture) bypasses the LOD and restarts springs.
    #[allow(clippy::too_many_arguments)] // One hero's full animation inputs.
    pub(crate) fn animate(
        &mut self,
        net_id: u16,
        rig: RigRef<'_>,
        now: f32,
        speed: f32,
        model_to_world: Mat4,
        view: Option<&LodView>,
        pinned: bool,
    ) -> Option<&[JointMatrix]> {
        let frame = self.frame;
        let interval = match (pinned, view) {
            (false, Some(view)) => {
                let (distance, on_screen) = view.classify(model_to_world.w_axis.truncate());
                self.lod.interval(distance, on_screen)
            }
            _ => 1,
        };
        let scratch = &mut *self.scratch;
        let slot = Self::claim(&mut self.slots, net_id, rig.index)?;
        let due = slot.evaluated.is_none_or(|last| {
            frame.wrapping_sub(last) >= interval
                || (frame.wrapping_add(u64::from(net_id))).is_multiple_of(interval)
        });
        if !due && slot.animator.matrices().len() == rig.skeleton.joint_count() {
            return Some(slot.animator.matrices());
        }
        if pinned {
            slot.animator.reset_springs();
        }
        slot.evaluated = Some(frame);
        Some(
            slot.animator
                .evaluate(rig.graph, rig.skeleton, now, speed, model_to_world, scratch),
        )
    }

    /// The animator of a hero: its sockets (`Animator::socket_transform`, the attach points
    /// R5 VFX will use) and its state. The renderer path does not need it yet.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn animator(&self, net_id: u16) -> Option<&Animator> {
        self.slot_index(net_id)
            .map(|index| &self.slots[index].animator)
    }
}

/// The stored `(net id, health, x, y)` for `net_id`.
fn lookup(table: &[(u16, u16, u16, u16)], net_id: u16) -> Option<(u16, u16, u16, u16)> {
    table
        .binary_search_by_key(&net_id, |entry| entry.0)
        .ok()
        .map(|index| table[index])
}

/// The ability name for a hero's cast slot from the wire catalogue.
pub(crate) fn ability_name<'a>(
    heroes: &'a [WireHero],
    abilities: &'a [WireAbility],
    hero_pick: u8,
    slot: u8,
) -> Option<&'a str> {
    let hero = heroes.get(usize::from(hero_pick))?;
    let ability = *hero.abilities.get(usize::from(slot))?;
    abilities
        .get(usize::from(ability))
        .map(WireAbility::name_str)
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use moba_core::math::{Mat4, Vec3};
    use moba_net::protocol::{
        ENTITY_FLAG_DEAD, EVENT_ATTACK, EVENT_CAST, EVENT_UNIT_DIED, NetEntity, NetEvent,
        NetProgress, SnapshotState,
    };
    use moba_render::{MeshData, MeshId};

    use super::{AnimLod, AnimationDriver, LodView, RigRef};
    use crate::app::models::{ModelBindings, ModelLibrary, Upload};
    use crate::app::scene_build::animated_rig;

    const SKINNED: &[u8] = include_bytes!("../../../../content/models/sentinel-skinned.glb");
    const SET: &str = include_str!("../../../../content/animations/sentinel-skinned.toml");
    const HERO: u16 = 3;
    const MINION: u16 = 40;

    fn library(tag: &str, set: Option<&str>) -> (ModelLibrary, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "moba-anim-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("models")).expect("models dir");
        std::fs::create_dir_all(root.join("animations")).expect("animations dir");
        std::fs::write(root.join("models/walker.glb"), SKINNED).expect("model");
        if let Some(set) = set {
            std::fs::write(root.join("animations/walker.toml"), set).expect("set");
        }
        let mut library = ModelLibrary::new(root.clone(), ModelBindings::default());
        library.bind_heroes([Some("walker")], |_: &MeshData, _: Upload| Ok(MeshId::CUBE));
        (library, root)
    }

    fn entity(net_id: u16, kind: u8, team: u8, x: f32, health: u16) -> NetEntity {
        let quantize = |value: f32| (value * 16.0) as u16;
        NetEntity {
            net_id,
            kind,
            team,
            hero_slot: if kind == 0 { 0 } else { 0xFF },
            hero_pick: if kind == 0 { 0 } else { 0xFF },
            flags: 0,
            status_bits: 0,
            max_health: 600,
            max_mana: 0,
            x: quantize(x),
            y: quantize(100.0),
            health,
            mana: 0,
            cooldowns: [0; 3],
            progress: NetProgress::default(),
        }
    }

    fn snapshot(tick: u32, entities: Vec<NetEntity>) -> SnapshotState {
        let mut state = SnapshotState::default();
        state.tick = tick;
        state.entities = entities;
        state
    }

    fn event(kind: u8, source: u16, target: u16, value: u16) -> NetEvent {
        NetEvent {
            kind,
            source,
            target,
            value,
            ..NetEvent::default()
        }
    }

    /// `source` hit `target` `age` ticks before the snapshot that carries the event, and can
    /// hit again `interval` ticks after the hit.
    fn attack(source: u16, target: u16, interval: u16, age: u16) -> NetEvent {
        NetEvent {
            aux: age,
            ..event(EVENT_ATTACK, source, target, interval)
        }
    }

    /// The headless R4 check: a scripted sequence of snapshot states and presentation events
    /// drives the state machine through idle, run, attack (hit frame on the `EVENT_ATTACK`
    /// tick), cast, death and respawn.
    #[test]
    #[allow(clippy::too_many_lines)] // One scripted sequence, step by step.
    fn scripted_snapshots_and_events_drive_the_state_machine() {
        let (models, root) = library("script", Some(SET));
        let rig_of = |_: u8| animated_rig(&models, models.hero(0));
        // As in the app: only a hero's net id resolves to a rig.
        let hero_rig = |net_id: u16| (net_id == HERO).then(|| rig_of(0)).flatten();
        let rig = rig_of(0).expect("the shipped set binds to the shipped model");
        let graph = rig.graph;
        let mut driver = AnimationDriver::new(AnimLod::parse("off"), 30);
        let seconds = |tick: u32| tick as f32 / 30.0;
        let state_name = |driver: &AnimationDriver, tick: u32, speed: f32| {
            let report =
                driver
                    .animator(HERO)
                    .expect("animator")
                    .report(graph, seconds(tick), speed);
            (
                report
                    .action
                    .map(|state| graph.state_name(state).to_owned()),
                report,
            )
        };
        let standing = |minion_health| {
            vec![
                entity(HERO, 0, 0, 100.0, 600),
                entity(MINION, 1, 1, 103.0, minion_health),
            ]
        };

        // Ticks 10-11: standing next to a minion that loses health (to someone else): idle.
        // Health drops no longer start swings; only attack events do.
        for (tick, health) in [(10, 300), (11, 270)] {
            driver.on_snapshot(&snapshot(tick, standing(health)), rig_of);
            driver.flush_events(tick, hero_rig, |_, _| None);
        }
        let (action, report) = state_name(&driver, 11, 0.0);
        assert_eq!(action, None);
        assert_eq!(report.locomotion.2, 0.0, "idle");
        // Moving at 6 m/s: the blend space is fully on the run clip.
        let (_, report) = state_name(&driver, 11, 6.0);
        let run = rig.skeleton.clip_index("run").expect("run");
        assert_eq!(report.locomotion, (run, run, 0.0));

        // Tick 14: the snapshot carries the hero's attack that hit on tick 12 (`aux` 2), and
        // a minion's attack, which claims no animator.
        driver.on_snapshot(&snapshot(14, standing(240)), rig_of);
        driver.queue_event(&attack(HERO, MINION, 24, 2));
        driver.queue_event(&attack(MINION, HERO, 30, 0));
        driver.flush_events(14, hero_rig, |_, _| None);
        assert!(driver.animator(MINION).is_none());
        let (action, report) = state_name(&driver, 12, 0.0);
        assert_eq!(action.as_deref(), Some("attack"));
        // The release frame (0.35 s, the thrust) is on the hit tick.
        assert!((report.action_time - 0.35).abs() < 1e-4, "{report:?}");
        // The wind-up began before the hit.
        let (action, _) = state_name(&driver, 6, 0.0);
        assert_eq!(action.as_deref(), Some("attack"));
        // The hero turned to the minion at +x.
        let facing = driver.rest_facing(HERO).expect("facing");
        assert!(facing.abs() < 1e-3, "{facing}");
        // The attack timer schedules the next swing one interval (24 ticks) later, with its
        // whole wind-up.
        let (action, report) = state_name(&driver, 36, 0.0);
        assert_eq!(action.as_deref(), Some("attack"));
        assert!((report.action_time - 0.35).abs() < 1e-4, "{report:?}");

        // Tick 20: the hero takes damage: the additive hit reaction plays over the idle.
        driver.on_snapshot(
            &snapshot(
                20,
                vec![
                    entity(HERO, 0, 0, 100.0, 550),
                    entity(MINION, 1, 1, 103.0, 240),
                ],
            ),
            rig_of,
        );
        let (_, report) = state_name(&driver, 21, 0.0);
        assert!(report.additive_weight > 0.99, "{report:?}");
        assert_eq!(state_name(&driver, 35, 0.0).1.additive_weight, 0.0);

        // Tick 37: no attack event confirmed the hit scheduled for tick 36 (the hero
        // stopped attacking), so that swing is withdrawn: it fades out from the view's
        // time (tick 33, where the frame loop has already started its wind-up) and is gone
        // before its thrust would have finished.
        driver.begin_frame();
        assert!(
            driver
                .animate(HERO, rig, seconds(33), 0.0, Mat4::IDENTITY, None, false)
                .is_some()
        );
        driver.flush_events(37, hero_rig, |_, _| None);
        let fading = state_name(&driver, 34, 0.0).1;
        assert!(
            fading.action.is_some() && fading.action_weight < 0.9,
            "{fading:?}"
        );
        assert_eq!(state_name(&driver, 39, 0.0).0, None);

        // Tick 40: a W cast event in that snapshot (slot 1 → cast2), released on its tick.
        driver.queue_event(&event(EVENT_CAST, HERO, 0xFFFF, 1));
        driver.flush_events(40, hero_rig, |_, _| None);
        let (action, report) = state_name(&driver, 40, 0.0);
        assert_eq!(action.as_deref(), Some("cast2"));
        assert!((report.action_time - 0.2).abs() < 1e-4);
        // Casting while running: still the cast (upper body), with the legs on the run.
        let (action, report) = state_name(&driver, 41, 6.0);
        assert_eq!(action.as_deref(), Some("cast2"));
        assert_eq!(report.locomotion, (run, run, 0.0));

        // Tick 60: the death event; the dead flag in later snapshots agrees. An attack event
        // that arrives for a dead hero changes nothing.
        driver.queue_event(&event(EVENT_UNIT_DIED, 0xFFFF, HERO, 0));
        driver.queue_event(&attack(HERO, MINION, 24, 0));
        driver.flush_events(60, hero_rig, |_, _| None);
        let mut dead = entity(HERO, 0, 0, 100.0, 0);
        dead.flags = ENTITY_FLAG_DEAD;
        driver.on_snapshot(&snapshot(61, vec![dead]), rig_of);
        let (action, report) = state_name(&driver, 70, 0.0);
        assert_eq!(action.as_deref(), Some("death"));
        assert!(report.dead);
        // Drawn while the death clip (1.2 s) and linger (1.5 s) play, then hidden.
        assert!(driver.visible(HERO, graph, seconds(60) + 2.6, true));
        assert!(!driver.visible(HERO, graph, seconds(60) + 2.8, true));

        // Tick 300: the dead flag clears (respawn is not a replicated event).
        driver.on_snapshot(&snapshot(300, vec![entity(HERO, 0, 0, 20.0, 600)]), rig_of);
        let (action, report) = state_name(&driver, 300, 0.0);
        assert_eq!(action, None);
        assert!(!report.dead);
        assert!(driver.visible(HERO, graph, seconds(300), false));

        // A rematch forgets every hero.
        driver.clear();
        assert!(driver.animator(HERO).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sustained_attacks_follow_the_attack_timer() {
        let (models, root) = library("cadence", Some(SET));
        let rig_of = |_: u8| animated_rig(&models, models.hero(0));
        let hero_rig = |net_id: u16| (net_id == HERO).then(|| rig_of(0)).flatten();
        let graph = rig_of(0).expect("rig").graph;
        let mut driver = AnimationDriver::new(AnimLod::default(), 30);
        let report = |driver: &AnimationDriver, tick: u32| {
            driver
                .animator(HERO)
                .expect("animator")
                .report(graph, tick as f32 / 30.0, 0.0)
        };
        let is_attack = |report: &moba_assets::AnimatorReport| {
            report.action.map(|state| graph.state_name(state)) == Some("attack")
        };
        // Hits at 24 and 48 (each event arrives in the hit's own snapshot), 24 ticks apart.
        for tick in 0..=48u32 {
            driver.on_snapshot(
                &snapshot(
                    tick,
                    vec![
                        entity(HERO, 0, 0, 100.0, 600),
                        entity(MINION, 1, 1, 102.0, 300),
                    ],
                ),
                rig_of,
            );
            if tick == 24 || tick == 48 {
                driver.queue_event(&attack(HERO, MINION, 24, 0));
            }
            driver.flush_events(tick, hero_rig, |_, _| None);
        }
        // The confirmed hit at 48 was the swing scheduled at 24, not restarted: its release
        // is still on tick 48. The next one, for tick 72, is scheduled with its wind-up.
        for hit in [48, 72] {
            let report = report(&driver, hit);
            assert!(is_attack(&report), "{report:?}");
            assert!((report.action_time - 0.35).abs() < 1e-3, "{report:?}");
        }
        // A haste brings the hit forward to 66 with a 20-tick interval: the swing scheduled
        // for 72 is replaced (a hit within two ticks of it would have been the same swing).
        driver.queue_event(&attack(HERO, MINION, 20, 0));
        driver.flush_events(66, hero_rig, |_, _| None);
        let at_hit = report(&driver, 66);
        assert!(is_attack(&at_hit), "{at_hit:?}");
        assert!((at_hit.action_time - 0.35).abs() < 1e-3, "{at_hit:?}");
        // The fight ends: no hit on 86, so at tick 87 the scheduled swing is withdrawn and
        // fades out from where the view is (tick 83, already in its wind-up), before its
        // thrust.
        driver.begin_frame();
        let rig = rig_of(0).expect("rig");
        assert!(
            driver
                .animate(HERO, rig, 83.0 / 30.0, 0.0, Mat4::IDENTITY, None, false)
                .is_some()
        );
        driver.flush_events(87, hero_rig, |_, _| None);
        let fading = report(&driver, 84);
        assert!(
            fading.action.is_some() && fading.action_weight < 0.9,
            "{fading:?}"
        );
        assert_eq!(report(&driver, 89).action, None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_or_invalid_sets_keep_the_single_clip_pose() {
        for (tag, set) in [
            ("none", None),
            ("broken", Some("states = 3")),
            ("unbound", Some("upper_body = \"tail\"")),
        ] {
            let (models, root) = library(tag, set);
            let model = models.hero(0).expect("model");
            assert!(model.skin.is_some(), "{tag}: still skinned");
            assert!(
                animated_rig(&models, Some(model)).is_none(),
                "{tag}: no graph"
            );
            let _ = std::fs::remove_dir_all(root);
        }
        // `MOBA_NO_ANIMATION_SETS=1` skips even a valid set.
        let root = std::env::temp_dir().join(format!("moba-anim-off-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("models")).expect("dir");
        std::fs::create_dir_all(root.join("animations")).expect("dir");
        std::fs::write(root.join("models/walker.glb"), SKINNED).expect("model");
        std::fs::write(root.join("animations/walker.toml"), SET).expect("set");
        let mut library = ModelLibrary::new(root.clone(), ModelBindings::default());
        library.set_animation_sets(false);
        library.bind_heroes([Some("walker")], |_: &MeshData, _: Upload| Ok(MeshId::CUBE));
        assert!(animated_rig(&library, library.hero(0)).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn lod_parses_and_skips_far_frames() {
        assert_eq!(AnimLod::parse("off").interval(1e6, false), 1);
        assert_eq!(AnimLod::parse(" 10 , 20 ").interval(15.0, true), 2);
        let lod = AnimLod::parse("bogus");
        assert_eq!(lod, AnimLod::default());
        assert_eq!(lod.interval(10.0, true), 1);
        assert_eq!(lod.interval(100.0, true), 2);
        assert_eq!(lod.interval(500.0, true), 4);
        assert_eq!(lod.interval(10.0, false), 4, "off-screen");
        assert_eq!(lod.interval(f32::NAN, true), 4);
        assert_eq!(AnimLod::parse("20,10"), AnimLod::default(), "far < near");

        let (models, root) = library("lod", Some(SET));
        let rig = animated_rig(&models, models.hero(0)).expect("rig");
        let view = LodView {
            eye: Vec3::new(0.0, 0.0, -1000.0),
            view_proj: Mat4::IDENTITY,
        };
        let mut driver = AnimationDriver::new(AnimLod::default(), 30);
        let mut evaluated = 0;
        let mut last: Option<Vec<[[f32; 4]; 3]>> = None;
        for frame in 0..16u8 {
            driver.begin_frame();
            let now = f32::from(frame) / 60.0;
            let matrices = driver
                .animate(HERO, rig, now, 6.0, Mat4::IDENTITY, Some(&view), false)
                .expect("slot")
                .to_vec();
            if last.as_ref() != Some(&matrices) {
                evaluated += 1;
            }
            last = Some(matrices);
        }
        assert_eq!(evaluated, 4, "a far hero animates every 4th frame");
        // A pinned capture always evaluates.
        let mut pinned = 0;
        for frame in 0..8u8 {
            driver.begin_frame();
            let now = 1.0 + f32::from(frame) / 60.0;
            let matrices = driver
                .animate(HERO, rig, now, 6.0, Mat4::IDENTITY, Some(&view), true)
                .expect("slot")
                .to_vec();
            if last.as_ref() != Some(&matrices) {
                pinned += 1;
            }
            last = Some(matrices);
        }
        assert_eq!(pinned, 8);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn steady_state_driver_frames_do_not_allocate() {
        let (models, root) = library("alloc", Some(SET));
        let rig = animated_rig(&models, models.hero(0)).expect("rig");
        let rig_of = move |_: u8| -> Option<RigRef<'_>> { Some(rig) };
        let mut driver = AnimationDriver::new(AnimLod::default(), 30);
        let view = LodView {
            eye: Vec3::ZERO,
            view_proj: Mat4::IDENTITY,
        };
        let states: Vec<SnapshotState> = (0..64u32)
            .map(|tick| {
                let health = 600 - u16::try_from(tick / 24).expect("small") * 40;
                snapshot(
                    tick,
                    (0..10u16)
                        .map(|hero| {
                            entity(hero, 0, u8::from(hero >= 5), 100.0 + f32::from(hero), 600)
                        })
                        .chain([entity(MINION, 1, 1, 103.0, health)])
                        .collect(),
                )
            })
            .collect();
        let frame = |driver: &mut AnimationDriver, state: &SnapshotState| {
            driver.on_snapshot(state, rig_of);
            driver.queue_event(&event(EVENT_CAST, 2, 0xFFFF, 0));
            if state.tick.is_multiple_of(12) {
                driver.queue_event(&attack(1, MINION, 12, 0));
            }
            driver.flush_events(state.tick, |_| Some(rig), |_, _| Some("Fireball"));
            driver.begin_frame();
            let mut sum = 0.0;
            for hero in 0..10u16 {
                let now = state.tick as f32 / 30.0;
                if let Some(matrices) = driver.animate(
                    hero,
                    rig,
                    now,
                    f32::from(hero % 3),
                    Mat4::IDENTITY,
                    Some(&view),
                    false,
                ) {
                    sum += matrices[0][0][0];
                }
            }
            sum
        };
        let (warm, rest) = states.split_at(8);
        for state in warm {
            frame(&mut driver, state);
        }
        let (sum, allocations) = crate::app::allocation_probe::probe(|| {
            rest.iter()
                .map(|state| frame(&mut driver, state))
                .sum::<f32>()
        });
        assert!(sum.is_finite());
        assert_eq!(allocations, 0, "animation driver frames allocated");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn long_dead_heroes_do_not_replay_their_death_and_facing_is_kept() {
        let (models, root) = library("facing", Some(SET));
        let rig_of = |_: u8| animated_rig(&models, models.hero(0));
        let graph = rig_of(0).expect("rig").graph;
        // First seen already dead (a mid-match join): dead, but not drawn dying.
        let mut driver = AnimationDriver::new(AnimLod::default(), 30);
        let mut dead = entity(HERO, 0, 0, 100.0, 0);
        dead.flags = ENTITY_FLAG_DEAD;
        driver.on_snapshot(&snapshot(500, vec![dead]), rig_of);
        assert!(driver.animator(HERO).expect("animator").is_dead());
        assert!(!driver.visible(HERO, graph, 500.0 / 30.0, true));
        // Seen alive first, then dying: the death plays.
        let mut driver = AnimationDriver::new(AnimLod::default(), 30);
        driver.on_snapshot(&snapshot(500, vec![entity(HERO, 0, 0, 100.0, 600)]), rig_of);
        driver.on_snapshot(&snapshot(501, vec![dead]), rig_of);
        assert!(driver.visible(HERO, graph, 502.0 / 30.0, true));

        // Facing: the last snapshot movement direction, then towards an attack target.
        let mut driver = AnimationDriver::new(AnimLod::default(), 30);
        assert_eq!(driver.rest_facing(HERO), None);
        let mut moved = entity(HERO, 0, 0, 100.0, 600);
        driver.on_snapshot(&snapshot(10, vec![moved]), rig_of);
        moved.y += 4; // 0.25 m along +y
        driver.on_snapshot(&snapshot(11, vec![moved]), rig_of);
        let facing = driver.rest_facing(HERO).expect("facing");
        assert!(
            (facing - core::f32::consts::FRAC_PI_2).abs() < 1e-3,
            "{facing}"
        );
        // A respawn-sized jump is a teleport, not a direction.
        moved.x += 16 * 50;
        driver.on_snapshot(&snapshot(12, vec![moved]), rig_of);
        assert_eq!(driver.rest_facing(HERO), Some(facing));
        // An attack on a minion at -x turns the hero towards it.
        let minion = |health| {
            let mut minion = entity(MINION, 1, 1, 0.0, health);
            minion.x = moved.x - 32;
            minion.y = moved.y;
            minion
        };
        driver.on_snapshot(&snapshot(13, vec![moved, minion(300)]), rig_of);
        driver.queue_event(&attack(HERO, MINION, 24, 0));
        driver.flush_events(14, |_| rig_of(0), |_, _| None);
        let facing = driver.rest_facing(HERO).expect("facing");
        assert!(
            (facing.abs() - core::f32::consts::PI).abs() < 1e-3,
            "{facing}"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
