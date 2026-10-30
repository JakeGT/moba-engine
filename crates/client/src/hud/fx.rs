//! Retained HUD presentation state (R7a): health-bar damage ghosts, floating combat text and
//! screen transitions.
//!
//! Everything here is derived on the client from replicated snapshots. It never feeds back
//! into the simulation. Storage is fixed-capacity (an `ArrayVec` of tracked units and a ring
//! of combat texts), so the steady state allocates nothing.
//!
//! Timing uses match ticks: a change seen in the snapshot for tick `T` is stamped `T` and
//! animated against the render tick, which lags the newest snapshot by the interpolation
//! delay. Combat text therefore appears when the interpolated units reach the hit, and a
//! pinned capture tick (`MOBA_CAPTURE_TICK`) gives a repeatable frame. Screen fades are the
//! one wall-clock effect; capture mode settles them.
//!
//! The wire carries no per-hit damage events, attacker or crit flag. Combat numbers are the
//! health change of a unit between two processed snapshots (several hits inside one snapshot
//! interval add up), and "crit" emphasis marks a single change of at least
//! [`CRIT_FRACTION`] of the unit's maximum health. The shield segment shows that a shield is
//! up, not its size: snapshots carry only the shield status bit.

use arrayvec::ArrayVec;
use moba_net::protocol::{NetEntity, SnapshotState};

use super::theme::TeamPalette;

/// Ticks a damage ghost holds at the pre-hit health before draining.
pub(crate) const GHOST_HOLD_TICKS: f32 = 9.0;
/// Ghost drain speed, as a fraction of maximum health per tick (a full bar in 1.5 s).
pub(crate) const GHOST_DRAIN_PER_TICK: f32 = 1.0 / 45.0;
/// A single health drop of at least this fraction of maximum health is emphasised.
pub(crate) const CRIT_FRACTION: f32 = 0.12;
/// New combat texts accepted per processed snapshot (at most one snapshot per frame).
pub(crate) const MAX_SPAWNS_PER_SNAPSHOT: usize = 8;
/// Ticks between a snapshot and the render tick reaching it (the interpolation delay,
/// `SNAPSHOT_DELAY_TICKS`, plus jitter headroom).
pub(crate) const RENDER_LAG_TICKS: usize = 8;
/// Longest combat-text life in ticks ([`CombatText::life`]).
pub(crate) const MAX_TEXT_LIFE_TICKS: usize = 36;
/// Combat texts alive at once: a ring big enough that a full spawn budget every snapshot is
/// never overwritten before it has been shown and faded.
pub(crate) const TEXT_CAPACITY: usize =
    MAX_SPAWNS_PER_SNAPSHOT * (RENDER_LAG_TICKS + MAX_TEXT_LIFE_TICKS + 1);
/// Combat texts drawn per frame at most: the newest ones.
pub(crate) const MAX_DRAWN_TEXTS: usize = 48;
/// A unit unseen for more than this many ticks (a snapshot stall) is re-initialised silently
/// instead of reporting the whole gap as one number.
const STALL_TICKS: f32 = 15.0;
/// Units with health bars tracked for ghosts and combat text (heroes and structures).
pub(crate) const TRACK_CAPACITY: usize = 96;
/// Snapshots a unit may go unseen (fog, death removal) before its track is dropped.
const TRACK_TIMEOUT: u32 = 90;
/// Gold gains below this are passive income and get no text.
const MIN_GOLD_TEXT: u16 = 5;
/// Screen fade duration in seconds.
pub(crate) const FADE_SECONDS: f32 = 0.45;
/// Minimum time the loading screen shows after connecting or a map change.
pub(crate) const SPLASH_SECONDS: f32 = 1.2;

/// What a floating number reports.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum CombatKind {
    #[default]
    Damage,
    Heal,
    Gold,
}

/// One floating combat number.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct CombatText {
    pub(crate) kind: CombatKind,
    pub(crate) amount: u32,
    /// Emphasised hit.
    pub(crate) crit: bool,
    /// The damaged unit is on the viewer's team (drawn red).
    pub(crate) friendly: bool,
    /// World position at spawn (simulation units) and the unit kind (for its HUD height).
    pub(crate) world: [f32; 2],
    pub(crate) unit_kind: u8,
    /// Snapshot tick of the change.
    pub(crate) tick: f32,
    /// Horizontal stagger slot (-2..=2): consecutive numbers on one unit and numbers on
    /// stacked units spread out instead of piling up.
    pub(crate) lane: i8,
    pub(crate) active: bool,
}

/// How a combat text looks at one moment of its life.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct TextFrame {
    /// Pixels above the spawn point.
    pub(crate) rise: f32,
    pub(crate) alpha: f32,
    /// Multiplier on the base text scale (the crit "pop").
    pub(crate) scale: f32,
}

impl CombatText {
    /// Lifetime in ticks.
    pub(crate) fn life(&self) -> f32 {
        match (self.kind, self.crit) {
            (CombatKind::Damage, false) => 27.0,
            (CombatKind::Heal, _) => 30.0,
            (CombatKind::Damage, true) | (CombatKind::Gold, _) => 36.0,
        }
    }

    /// The frame at `render_tick`, or `None` before the hit is rendered or after it faded.
    pub(crate) fn frame(&self, render_tick: f32) -> Option<TextFrame> {
        let age = render_tick - self.tick;
        let life = self.life();
        if !self.active || age < 0.0 || age > life {
            return None;
        }
        let t = age / life;
        let ease = 1.0 - (1.0 - t) * (1.0 - t);
        let rise = if self.kind == CombatKind::Gold {
            26.0
        } else {
            38.0
        } * ease;
        let alpha = if t < 0.6 { 1.0 } else { 1.0 - (t - 0.6) / 0.4 };
        let scale = if self.crit {
            1.25 + 0.55 * (1.0 - (t / 0.18).min(1.0))
        } else {
            1.0
        };
        Some(TextFrame { rise, alpha, scale })
    }
}

/// Fixed-capacity ring of combat texts.
#[derive(Debug)]
pub(crate) struct CombatTextPool {
    slots: [CombatText; TEXT_CAPACITY],
    next: usize,
}

impl Default for CombatTextPool {
    fn default() -> Self {
        Self {
            slots: [CombatText::default(); TEXT_CAPACITY],
            next: 0,
        }
    }
}

impl CombatTextPool {
    /// Adds a text, overwriting the oldest slot when full.
    pub(crate) fn spawn(&mut self, text: CombatText) {
        self.slots[self.next] = CombatText {
            active: true,
            ..text
        };
        self.next = (self.next + 1) % TEXT_CAPACITY;
    }

    /// The newest (up to [`MAX_DRAWN_TEXTS`]) texts visible at `render_tick`, returned
    /// oldest first so the newest draw on top. Fixed-capacity, no allocation.
    pub(crate) fn visible(
        &self,
        render_tick: f32,
    ) -> impl Iterator<Item = (&CombatText, TextFrame)> {
        let mut newest = ArrayVec::<(usize, TextFrame), MAX_DRAWN_TEXTS>::new();
        for offset in 1..=TEXT_CAPACITY {
            let index = (self.next + TEXT_CAPACITY - offset) % TEXT_CAPACITY;
            if let Some(frame) = self.slots[index].frame(render_tick)
                && newest.try_push((index, frame)).is_err()
            {
                break;
            }
        }
        newest.reverse();
        newest
            .into_iter()
            .map(move |(index, frame)| (&self.slots[index], frame))
    }

    /// Every live text at `render_tick`, oldest first (tests).
    #[cfg(test)]
    pub(crate) fn all_visible(&self, render_tick: f32) -> impl Iterator<Item = &CombatText> {
        (0..TEXT_CAPACITY)
            .map(move |offset| &self.slots[(self.next + offset) % TEXT_CAPACITY])
            .filter(move |text| text.frame(render_tick).is_some())
    }

    pub(crate) fn clear(&mut self) {
        for slot in &mut self.slots {
            slot.active = false;
        }
    }
}

/// Health history of one unit with a health bar.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct UnitTrack {
    pub(crate) net_id: u16,
    pub(crate) health: i32,
    pub(crate) max_health: i32,
    pub(crate) dead: bool,
    /// Ghost value when its drain starts, and the tick it starts.
    pub(crate) ghost_top: f32,
    pub(crate) drain_start: f32,
    seen: u32,
    /// Snapshot tick of the last observation.
    tick: f32,
    /// Health before the latest change, and the tick of that change: the bar shows
    /// `before` until the render tick reaches `changed`, in step with the numbers.
    before: i32,
    changed: f32,
    /// Next combat-text lane for this unit.
    lane: u8,
}

impl UnitTrack {
    fn new(entity: &NetEntity, tick: f32, seen: u32) -> Self {
        let health = i32::from(entity.health);
        Self {
            net_id: entity.net_id,
            health,
            max_health: i32::from(entity.max_health),
            dead: entity.is_dead(),
            ghost_top: health as f32,
            drain_start: tick,
            seen,
            tick,
            before: health,
            changed: tick,
            // Neighbouring net ids start on different lanes.
            lane: (entity.net_id % 5) as u8,
        }
    }

    /// The health the bar shows at `render_tick`: the latest snapshot's once the render tick
    /// reaches its change, the previous value before that.
    pub(crate) fn health_at(&self, render_tick: f32) -> i32 {
        if render_tick < self.changed {
            self.before
        } else {
            self.health
        }
    }

    /// The ghost (trailing) health at `tick`: never below the current health.
    pub(crate) fn ghost_at(&self, tick: f32) -> f32 {
        let drained = GHOST_DRAIN_PER_TICK
            * self.max_health.max(1) as f32
            * (tick - self.drain_start).max(0.0);
        (self.ghost_top - drained).max(self.health as f32)
    }
}

/// Which screen the client shows, for transitions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Screen {
    /// Connected, waiting for the first snapshot.
    Loading,
    /// Lobby or countdown.
    Lobby,
    Match,
    Results,
}

/// Screen-change fade state.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct ScreenFade {
    pub(crate) current: Screen,
    pub(crate) previous: Screen,
    /// Seconds since `current` began.
    pub(crate) since: f32,
}

impl Default for ScreenFade {
    fn default() -> Self {
        Self {
            current: Screen::Loading,
            previous: Screen::Loading,
            since: 0.0,
        }
    }
}

impl ScreenFade {
    /// Advances by `dt` seconds, restarting the fade when the screen changes.
    pub(crate) fn update(&mut self, screen: Screen, dt: f32, settle: bool) {
        if screen == self.current {
            self.since += dt.clamp(0.0, 0.25);
        } else {
            self.previous = self.current;
            self.current = screen;
            self.since = 0.0;
        }
        if settle {
            self.since = self.since.max(FADE_SECONDS);
        }
    }

    /// Eased progress of the current screen's fade-in, 0..1.
    pub(crate) fn progress(self) -> f32 {
        let t = (self.since / FADE_SECONDS).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// Opacity of the dip-to-dark overlay (0 once settled).
    pub(crate) fn dip(self) -> f32 {
        if self.previous == self.current || self.previous == Screen::Loading {
            return 0.0;
        }
        0.8 * (1.0 - self.progress())
    }

    /// Opacity of the loading screen: 1 while loading, fading out after it.
    pub(crate) fn loading(self) -> f32 {
        if self.current == Screen::Loading {
            1.0
        } else if self.previous == Screen::Loading {
            1.0 - self.progress()
        } else {
            0.0
        }
    }
}

/// Retained HUD presentation state for the network view.
#[derive(Debug)]
pub(crate) struct HudFx {
    pub(crate) palette: TeamPalette,
    tracks: ArrayVec<UnitTrack, TRACK_CAPACITY>,
    texts: CombatTextPool,
    pub(crate) screen: ScreenFade,
    gold: Option<u16>,
    generation: u32,
    /// Seconds since connecting or the last map change: the splash timer and the spinner.
    pub(crate) clock: f32,
    /// What the offscreen images hold this frame (R7b), set by the app before the HUD is
    /// built.
    pub(crate) images: super::images::HudImages,
}

impl HudFx {
    pub(crate) fn new(palette: TeamPalette) -> Self {
        Self {
            palette,
            tracks: ArrayVec::new(),
            texts: CombatTextPool::default(),
            screen: ScreenFade::default(),
            gold: None,
            generation: 0,
            clock: 0.0,
            images: super::images::HudImages {
                portraits: 0,
                portrait_cells: 0,
                minimap: false,
                fog: super::images::MinimapFog::Grid,
            },
        }
    }

    /// Forgets all history (rematch, content reload, map rotation).
    pub(crate) fn reset(&mut self) {
        self.tracks.clear();
        self.texts.clear();
        self.gold = None;
    }

    /// Shows the loading screen again (content reload or map rotation).
    pub(crate) fn restart_splash(&mut self) {
        self.clock = 0.0;
    }

    /// Advances wall-clock effects by `dt` seconds.
    pub(crate) fn update_screen(&mut self, screen: Screen, dt: f32, settle: bool) {
        self.screen.update(screen, dt, settle);
        self.clock += dt.clamp(0.0, 0.25);
    }

    /// Adds one combat text directly (tests).
    #[cfg(test)]
    pub(crate) fn spawn_text(&mut self, text: CombatText) {
        self.texts.spawn(text);
    }

    /// The tracked unit `net_id`, if any.
    pub(crate) fn track(&self, net_id: u16) -> Option<&UnitTrack> {
        self.tracks.iter().find(|track| track.net_id == net_id)
    }

    /// Combat texts visible at `render_tick`.
    pub(crate) fn texts(&self, render_tick: f32) -> impl Iterator<Item = (&CombatText, TextFrame)> {
        self.texts.visible(render_tick)
    }

    /// Records a newly processed snapshot: updates ghosts and spawns combat text for health
    /// and own-gold changes. `own_team` is the viewer's team index.
    ///
    /// At most [`MAX_SPAWNS_PER_SNAPSHOT`] numbers are spawned, chosen by importance: the own
    /// hero first, then the viewer's team, then the largest changes (net id breaks ties), so
    /// no unit is starved by its id in a big fight.
    pub(crate) fn observe(
        &mut self,
        snapshot: &SnapshotState,
        own_net_id: Option<u16>,
        own_team: u8,
    ) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let tick = snapshot.tick as f32;
        // One candidate per tracked unit plus the gold number; sorted in place, no heap.
        let mut candidates = ArrayVec::<(u64, CombatText), { TRACK_CAPACITY + 1 }>::new();
        for entity in &snapshot.entities {
            let own = Some(entity.net_id) == own_net_id;
            if own {
                let gold = entity.progress.gold;
                if let Some(previous) = self.gold
                    && gold > previous
                    && gold - previous >= MIN_GOLD_TEXT
                {
                    let text = CombatText {
                        kind: CombatKind::Gold,
                        amount: u32::from(gold - previous),
                        world: position(entity),
                        unit_kind: entity.kind,
                        tick,
                        ..CombatText::default()
                    };
                    let _ = candidates.try_push((importance(true, true, text.amount, 0), text));
                }
                self.gold = Some(gold);
            }
            if !has_health_bar(entity.kind) {
                continue;
            }
            let Some(index) = self
                .tracks
                .iter()
                .position(|track| track.net_id == entity.net_id)
            else {
                // Full table: the unit simply has no ghost or numbers.
                let _ = self
                    .tracks
                    .try_push(UnitTrack::new(entity, tick, generation));
                continue;
            };
            let track = &mut self.tracks[index];
            let health = i32::from(entity.health);
            let max_health = i32::from(entity.max_health);
            let dead = entity.is_dead();
            // Missed a processed snapshot (fog) or a stall: the change is not one hit.
            let gap = generation.wrapping_sub(track.seen) > 1 || tick - track.tick > STALL_TICKS;
            if gap || (track.dead && !dead) || max_health <= 0 {
                // Respawn or re-sighting: no number and no ghost.
                *track = UnitTrack::new(entity, tick, generation);
                continue;
            }
            track.seen = generation;
            track.tick = tick;
            let previous = track.health;
            let friendly = entity.team == own_team;
            if health != previous {
                track.before = previous;
                track.changed = tick;
            }
            if health < previous {
                let ghost = track.ghost_at(tick).max(previous as f32);
                track.ghost_top = ghost;
                track.drain_start = tick + GHOST_HOLD_TICKS;
                let amount = (previous - health).unsigned_abs();
                let text = CombatText {
                    kind: CombatKind::Damage,
                    amount,
                    crit: entity.kind == 0 && amount as f32 >= CRIT_FRACTION * max_health as f32,
                    friendly,
                    world: position(entity),
                    unit_kind: entity.kind,
                    tick,
                    lane: stagger(&mut track.lane),
                    active: true,
                };
                let key = importance(own, friendly, amount, entity.net_id);
                let _ = candidates.try_push((key, text));
            } else {
                // Growth from a level or item is not healing.
                let heal = health - previous - (max_health - track.max_health).max(0);
                let threshold = (max_health / 50).max(12);
                if heal >= threshold {
                    let text = CombatText {
                        kind: CombatKind::Heal,
                        amount: heal.unsigned_abs(),
                        friendly,
                        world: position(entity),
                        unit_kind: entity.kind,
                        tick,
                        lane: stagger(&mut track.lane),
                        ..CombatText::default()
                    };
                    let key = importance(own, friendly, text.amount, entity.net_id);
                    let _ = candidates.try_push((key, text));
                }
            }
            track.health = health;
            track.max_health = max_health;
            track.dead = dead;
        }
        candidates.sort_unstable_by_key(|(key, _)| std::cmp::Reverse(*key));
        for (_, text) in candidates.iter().take(MAX_SPAWNS_PER_SNAPSHOT) {
            self.texts.spawn(*text);
        }
        self.tracks
            .retain(|track| generation.wrapping_sub(track.seen) <= TRACK_TIMEOUT);
    }
}

/// Spawn priority: own hero, then the viewer's team, then the amount, then the net id (a
/// total order, so the choice is repeatable).
fn importance(own: bool, friendly: bool, amount: u32, net_id: u16) -> u64 {
    (u64::from(own) << 49)
        | (u64::from(friendly) << 48)
        | (u64::from(amount) << 16)
        | u64::from(net_id)
}

/// Heroes (kind 0) and structures (kinds 3 and up) get health bars; minions do not.
pub(crate) const fn has_health_bar(kind: u8) -> bool {
    kind == 0 || kind >= 3
}

fn position(entity: &NetEntity) -> [f32; 2] {
    let position = entity.position();
    [position.x.to_f32(), position.y.to_f32()]
}

/// Cycles a unit's numbers through five lanes, far apart first.
fn stagger(counter: &mut u8) -> i8 {
    const LANES: [i8; 5] = [0, 2, -1, 1, -2];
    let lane = LANES[usize::from(*counter % 5)];
    *counter = counter.wrapping_add(1);
    lane
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_net::protocol::{ENTITY_FLAG_DEAD, NetEntity, NetProgress, SnapshotState};

    fn hero(net_id: u16, team: u8, health: u16, max_health: u16) -> NetEntity {
        NetEntity {
            net_id,
            kind: 0,
            team,
            hero_slot: 0,
            hero_pick: 0,
            flags: 0,
            status_bits: 0,
            max_health,
            max_mana: 100,
            x: 1000,
            y: 1000,
            health,
            mana: 100,
            cooldowns: [0; 3],
            progress: NetProgress::default(),
        }
    }

    fn snapshot(tick: u32, entities: Vec<NetEntity>) -> SnapshotState {
        let mut state = SnapshotState::default();
        state.seq = tick;
        state.tick = tick;
        state.entities = entities;
        state
    }

    #[test]
    fn ghost_holds_then_drains_to_the_new_health() {
        let mut fx = HudFx::new(TeamPalette::Standard);
        fx.observe(&snapshot(100, vec![hero(7, 1, 900, 900)]), None, 0);
        fx.observe(&snapshot(101, vec![hero(7, 1, 600, 900)]), None, 0);
        let track = *fx.track(7).expect("tracked");
        assert_eq!(track.health, 600);
        // Holds at the pre-hit health for the hold time...
        assert_eq!(track.ghost_at(101.0), 900.0);
        assert_eq!(track.ghost_at(101.0 + GHOST_HOLD_TICKS), 900.0);
        // ...then drains at 1/45 of max per tick (20 HP/tick here)...
        let after = track.ghost_at(101.0 + GHOST_HOLD_TICKS + 5.0);
        assert!((after - 800.0).abs() < 1e-3, "{after}");
        // ...and never below the current health.
        assert_eq!(track.ghost_at(101.0 + GHOST_HOLD_TICKS + 100.0), 600.0);
    }

    #[test]
    fn a_second_hit_extends_the_ghost_from_where_it_is() {
        let mut fx = HudFx::new(TeamPalette::Standard);
        fx.observe(&snapshot(0, vec![hero(1, 1, 1000, 1000)]), None, 0);
        fx.observe(&snapshot(1, vec![hero(1, 1, 800, 1000)]), None, 0);
        // Mid-drain at tick 20: ghost = 1000 - (20 - 10) * 1000/45.
        let mid = fx.track(1).expect("tracked").ghost_at(20.0);
        for tick in 2..20 {
            fx.observe(&snapshot(tick, vec![hero(1, 1, 800, 1000)]), None, 0);
        }
        fx.observe(&snapshot(20, vec![hero(1, 1, 700, 1000)]), None, 0);
        let track = fx.track(1).expect("tracked");
        assert!((track.ghost_at(20.0) - mid).abs() < 1e-3);
        assert_eq!(track.drain_start, 20.0 + GHOST_HOLD_TICKS);
    }

    #[test]
    fn health_deltas_spawn_damage_heal_and_crit_text() {
        let mut fx = HudFx::new(TeamPalette::Standard);
        fx.observe(
            &snapshot(10, vec![hero(1, 0, 1000, 1000), hero(2, 1, 1000, 1000)]),
            None,
            0,
        );
        assert_eq!(fx.texts(10.0).count(), 0, "first sight is not a change");
        fx.observe(
            &snapshot(11, vec![hero(1, 0, 950, 1000), hero(2, 1, 700, 1000)]),
            None,
            0,
        );
        let texts: Vec<_> = fx.texts(11.0).map(|(text, _)| *text).collect();
        assert_eq!(texts.len(), 2);
        assert_eq!(
            (texts[0].amount, texts[0].crit, texts[0].friendly),
            (50, false, true)
        );
        assert_eq!(
            (texts[1].amount, texts[1].crit, texts[1].friendly),
            (300, true, false)
        );
        assert_ne!(texts[0].lane, texts[1].lane, "neighbouring ids start apart");
        // Not yet rendered (the render tick lags the snapshot) and gone after its life.
        assert_eq!(fx.texts(10.5).count(), 0);
        assert_eq!(fx.texts(11.0 + 40.0).count(), 0);
        // Small regeneration is not shown; a real heal is.
        fx.observe(
            &snapshot(12, vec![hero(1, 0, 953, 1000), hero(2, 1, 760, 1000)]),
            None,
            0,
        );
        let heals: Vec<_> = fx
            .texts(12.0)
            .filter(|(text, _)| text.kind == CombatKind::Heal)
            .map(|(text, _)| text.amount)
            .collect();
        assert_eq!(heals, [60]);
        // Consecutive hits on one unit change lane.
        fx.observe(
            &snapshot(13, vec![hero(1, 0, 900, 1000), hero(2, 1, 760, 1000)]),
            None,
            0,
        );
        let lanes: Vec<_> = fx
            .texts(13.0)
            .filter(|(text, _)| text.friendly && text.kind == CombatKind::Damage)
            .map(|(text, _)| text.lane)
            .collect();
        assert_eq!(lanes.len(), 2);
        assert_ne!(lanes[0], lanes[1]);
    }

    #[test]
    fn respawn_and_level_growth_are_not_heals() {
        let mut fx = HudFx::new(TeamPalette::Standard);
        let mut dead = hero(1, 0, 0, 1000);
        dead.flags = ENTITY_FLAG_DEAD;
        dead.status_bits = moba_net::protocol::STATUS_DEAD;
        fx.observe(&snapshot(1, vec![dead]), None, 0);
        fx.observe(&snapshot(2, vec![hero(1, 0, 1000, 1000)]), None, 0);
        fx.observe(&snapshot(3, vec![hero(1, 0, 1080, 1080)]), None, 0);
        assert_eq!(fx.texts(3.0).count(), 0);
        assert_eq!(fx.track(1).expect("tracked").ghost_at(3.0), 1080.0);
    }

    #[test]
    fn gold_gains_spawn_text_but_passive_income_does_not() {
        let mut fx = HudFx::new(TeamPalette::Standard);
        let mut own = hero(5, 0, 500, 500);
        own.progress.gold = 600;
        fx.observe(&snapshot(1, vec![own]), Some(5), 0);
        own.progress.gold = 601;
        fx.observe(&snapshot(2, vec![own]), Some(5), 0);
        own.progress.gold = 626;
        fx.observe(&snapshot(3, vec![own]), Some(5), 0);
        let gold: Vec<_> = fx
            .texts(3.0)
            .map(|(text, _)| (text.kind, text.amount))
            .collect();
        assert_eq!(gold, [(CombatKind::Gold, 25)]);
    }

    #[test]
    fn spawns_are_capped_per_snapshot_and_the_ring_wraps() {
        let mut fx = HudFx::new(TeamPalette::Standard);
        let full: Vec<_> = (0..20).map(|id| hero(id, 1, 1000, 1000)).collect();
        let hit: Vec<_> = (0..20).map(|id| hero(id, 1, 900, 1000)).collect();
        fx.observe(&snapshot(0, full), None, 0);
        fx.observe(&snapshot(1, hit), None, 0);
        assert_eq!(fx.texts(1.0).count(), MAX_SPAWNS_PER_SNAPSHOT);
        let mut pool = CombatTextPool::default();
        for index in 0..(TEXT_CAPACITY + 5) {
            pool.spawn(CombatText {
                amount: index as u32,
                tick: 0.0,
                ..CombatText::default()
            });
        }
        let all: Vec<_> = pool.all_visible(0.0).map(|text| text.amount).collect();
        assert_eq!(all.len(), TEXT_CAPACITY);
        assert_eq!(all[0], 5, "the oldest five were overwritten");
        // The draw list is the newest MAX_DRAWN_TEXTS, oldest of those first.
        let drawn: Vec<_> = pool.visible(0.0).map(|(text, _)| text.amount).collect();
        assert_eq!(drawn.len(), MAX_DRAWN_TEXTS);
        assert_eq!(*drawn.last().expect("drawn"), (TEXT_CAPACITY + 4) as u32);
        assert_eq!(drawn[0], (TEXT_CAPACITY + 5 - MAX_DRAWN_TEXTS) as u32);
    }

    #[test]
    fn a_full_spawn_budget_every_snapshot_is_never_overwritten_early() {
        // Every text must survive the render lag plus its whole life.
        let longest = [
            CombatText::default(),
            CombatText {
                crit: true,
                ..CombatText::default()
            },
            CombatText {
                kind: CombatKind::Heal,
                ..CombatText::default()
            },
            CombatText {
                kind: CombatKind::Gold,
                ..CombatText::default()
            },
        ]
        .iter()
        .map(CombatText::life)
        .fold(0.0, f32::max);
        assert_eq!(longest, MAX_TEXT_LIFE_TICKS as f32);
        let mut pool = CombatTextPool::default();
        for tick in 0..200u32 {
            for slot in 0..MAX_SPAWNS_PER_SNAPSHOT {
                pool.spawn(CombatText {
                    amount: tick * 100 + slot as u32,
                    tick: tick as f32,
                    crit: true,
                    ..CombatText::default()
                });
            }
        }
        // Rendering lags the newest snapshot (199) by RENDER_LAG_TICKS: every text spawned
        // in the life window before the render tick is still in the ring.
        let render = (199 - RENDER_LAG_TICKS) as f32;
        let alive = pool.all_visible(render).count();
        assert_eq!(alive, MAX_SPAWNS_PER_SNAPSHOT * (MAX_TEXT_LIFE_TICKS + 1));
    }

    #[test]
    fn spawn_priority_prefers_the_own_hero_team_and_big_hits() {
        let mut fx = HudFx::new(TeamPalette::Standard);
        // 20 enemies and, at the highest net ids, the own hero (90) and an ally (91).
        let mut full: Vec<_> = (0..20).map(|id| hero(id, 1, 1000, 1000)).collect();
        full.push(hero(90, 0, 1000, 1000));
        full.push(hero(91, 0, 1000, 1000));
        let mut hit: Vec<_> = (0..20)
            .map(|id| hero(id, 1, if id == 19 { 500 } else { 990 }, 1000))
            .collect();
        hit.push(hero(90, 0, 999, 1000));
        hit.push(hero(91, 0, 998, 1000));
        fx.observe(&snapshot(0, full), Some(90), 0);
        fx.observe(&snapshot(1, hit), Some(90), 0);
        let texts: Vec<_> = fx
            .texts(1.0)
            .map(|(text, _)| (text.amount, text.friendly))
            .collect();
        assert_eq!(texts.len(), MAX_SPAWNS_PER_SNAPSHOT);
        assert!(
            texts.contains(&(1, true)),
            "the own hero's 1-damage hit is kept"
        );
        assert!(texts.contains(&(2, true)), "an ally's hit beats enemy hits");
        assert!(
            texts.contains(&(500, false)),
            "the biggest enemy hit is kept"
        );
    }

    #[test]
    fn fog_gaps_and_stalls_reinitialise_silently() {
        let mut fx = HudFx::new(TeamPalette::Standard);
        fx.observe(
            &snapshot(1, vec![hero(1, 1, 400, 1000), hero(2, 1, 900, 1000)]),
            None,
            0,
        );
        // Unit 1 is fogged for one processed snapshot, then seen healed.
        fx.observe(&snapshot(2, vec![hero(2, 1, 900, 1000)]), None, 0);
        fx.observe(
            &snapshot(3, vec![hero(1, 1, 1000, 1000), hero(2, 1, 900, 1000)]),
            None,
            0,
        );
        assert_eq!(fx.texts(3.0).count(), 0, "no +600 heal after fog");
        // A stall longer than STALL_TICKS hides the drop as well (no ghost either).
        fx.observe(
            &snapshot(40, vec![hero(1, 1, 300, 1000), hero(2, 1, 900, 1000)]),
            None,
            0,
        );
        assert_eq!(fx.texts(40.0).count(), 0);
        assert_eq!(fx.track(1).expect("tracked").ghost_at(40.0), 300.0);
    }

    #[test]
    fn bars_change_when_the_render_tick_reaches_the_hit() {
        let mut fx = HudFx::new(TeamPalette::Standard);
        fx.observe(&snapshot(10, vec![hero(1, 1, 1000, 1000)]), None, 0);
        fx.observe(&snapshot(11, vec![hero(1, 1, 700, 1000)]), None, 0);
        let track = fx.track(1).expect("tracked");
        assert_eq!(
            track.health_at(7.0),
            1000,
            "the render tick lags the snapshot"
        );
        assert_eq!(track.health_at(11.0), 700);
    }

    #[test]
    fn combat_text_rises_fades_and_pops_on_crits() {
        let text = CombatText {
            tick: 10.0,
            active: true,
            ..CombatText::default()
        };
        let start = text.frame(10.0).expect("visible");
        let late = text.frame(10.0 + 25.0).expect("visible");
        assert_eq!(start.rise, 0.0);
        assert_eq!(start.alpha, 1.0);
        assert!(late.rise > 30.0 && late.alpha < 0.3);
        assert_eq!(text.frame(9.0), None);
        let crit = CombatText { crit: true, ..text };
        assert!(crit.frame(10.0).expect("visible").scale > 1.7);
        assert!((crit.frame(20.0).expect("visible").scale - 1.25).abs() < 1e-6);
    }

    #[test]
    fn tracks_time_out_when_unseen() {
        let mut fx = HudFx::new(TeamPalette::Standard);
        fx.observe(&snapshot(0, vec![hero(1, 0, 10, 10)]), None, 0);
        for tick in 1..=TRACK_TIMEOUT + 1 {
            fx.observe(&snapshot(tick, vec![]), None, 0);
        }
        assert!(fx.track(1).is_none());
    }

    #[test]
    fn screen_fades_dip_and_settle() {
        let mut fade = ScreenFade::default();
        assert_eq!(fade.loading(), 1.0);
        fade.update(Screen::Lobby, 0.016, false);
        assert_eq!(
            fade.loading(),
            1.0,
            "the loading screen fades out from full"
        );
        assert_eq!(
            fade.dip(),
            0.0,
            "leaving the loading screen is a cross-fade"
        );
        // Frame times are clamped to 0.25 s, so a hitch cannot skip the fade.
        fade.update(Screen::Lobby, 1.0, false);
        assert!(fade.loading() > 0.0);
        fade.update(Screen::Lobby, 0.25, false);
        assert_eq!(fade.loading(), 0.0);
        fade.update(Screen::Match, 0.016, false);
        assert!(fade.dip() > 0.7);
        fade.update(Screen::Match, FADE_SECONDS * 0.5, false);
        assert!(fade.dip() > 0.0 && fade.dip() < 0.7);
        fade.update(Screen::Results, 0.0, true);
        assert_eq!(fade.dip(), 0.0, "capture mode settles");
        assert_eq!(fade.progress(), 1.0);
    }

    #[test]
    fn steady_state_observe_and_draw_queries_do_not_allocate() {
        let mut fx = HudFx::new(TeamPalette::Standard);
        let a = snapshot(
            1,
            (0..40)
                .map(|id| hero(id, (id % 2) as u8, 1000, 1000))
                .collect(),
        );
        let b = snapshot(
            2,
            (0..40)
                .map(|id| hero(id, (id % 2) as u8, 900, 1000))
                .collect(),
        );
        fx.observe(&a, Some(0), 0);
        let ((), allocations) = crate::app::allocation_probe::probe(|| {
            for _ in 0..50 {
                fx.observe(&b, Some(0), 0);
                fx.observe(&a, Some(0), 0);
                fx.update_screen(Screen::Match, 0.016, false);
                let visible = fx.texts(2.0).count();
                assert!(visible <= TEXT_CAPACITY);
                let ghost = fx.track(3).map(|track| track.ghost_at(5.0));
                assert!(ghost.is_some());
            }
        });
        assert_eq!(allocations, 0, "steady-state HUD fx allocation");
    }
}
