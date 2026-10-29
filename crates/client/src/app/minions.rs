//! Vertex-animation-texture minions (R4b, ADR-0012 amendment).
//!
//! Minions are drawn from a VAT asset when the client has one: `<asset root>/vat/<name>.toml`
//! plus `<name>.ktx2`, baked by `moba-tools vat-bake` from a skinned model whose static mesh
//! (`models/<model>.glb`) is drawn. Melee minions try `minion-melee`, then `minion`; ranged
//! minions `minion-ranged`, then `minion`. A kind without a usable asset (missing, malformed,
//! mismatched, or the renderer has no VAT pipeline) is drawn as today's cube; every failure
//! is logged once. `MOBA_VAT=off` draws every minion as a cube.
//!
//! [`MinionDriver`] keeps one [`VatPlayer`] per minion, keyed by net id, and feeds it only what
//! the client already has: the interpolated speed (idle ↔ run), `EVENT_ATTACK` with a minion
//! source (the release frame on the hit tick) and `EVENT_UNIT_DIED` with a minion target. A
//! minion is despawned in the snapshot that reports its death, so the death plays on a
//! **corpse**: a copy of its player at the event's position, drawn for the death clip plus a
//! short linger. Facing is the last snapshot step or towards the attack target.
//!
//! Like the hero animators this is presentation only and client-local: nothing here reaches
//! the simulation, the wire or replays. Everything is preallocated; a frame never allocates.

use std::path::{Path, PathBuf};

use moba_assets::{
    MeshData, TextureData, VatAnimation, VatDescriptor, VatPlayer, VatPose, VatQuality,
};
use moba_core::math::{Mat4, Quat, Vec3, Vec4};
use moba_net::protocol::{
    EVENT_ATTACK, EVENT_UNIT_DIED, NetEvent, SnapshotState, dequantize_position,
};
use moba_render::{MeshId, QualityTier, UnitInstance};
use moba_sim::UnitKind;

/// Environment variable that disables VAT minions (`off` or `0`).
pub(crate) const VAT_ENV: &str = "MOBA_VAT";
/// Player slots, indexed by net id modulo this. Net ids are allocated sequentially, so two
/// live minions only share a slot if their ids are 1,024 apart.
const SLOTS: usize = 1_024;
/// Dying minions drawn at once.
const MAX_CORPSES: usize = 64;
/// Presentation events buffered until their snapshot's tick is known.
const MAX_QUEUED_EVENTS: usize = 64;
/// Locomotion phase offset per net id, seconds.
const PHASE_SECONDS: f32 = 0.173;
/// A minion that moved less than this between snapshots is standing still, metres.
const STATIONARY: f32 = 0.05;
/// A snapshot step longer than this (metres) is not a facing.
const TELEPORT: f32 = 3.0;
/// Drawn heights of the minion kinds, metres (as the cube fallback).
const MELEE_HEIGHT: f32 = 2.2;
const RANGED_HEIGHT: f32 = 1.8;

/// `false` when [`VAT_ENV`] is `off` or `0`. Read once at startup.
pub(crate) fn vat_enabled_from_env() -> bool {
    std::env::var(VAT_ENV).map_or(true, |value| {
        let value = value.trim();
        !(value.eq_ignore_ascii_case("off") || value == "0")
    })
}

/// The VAT shader work per tier: `Low` the nearest frame (2 fetches per vertex), `Medium`
/// interpolated frames (4), `High`/`Ultra` interpolated and crossfaded clips (8).
pub(crate) fn vat_quality(tier: QualityTier) -> VatQuality {
    match tier {
        QualityTier::Low => VatQuality::NEAREST,
        QualityTier::Medium => VatQuality::INTERPOLATED,
        _ => VatQuality::FULL,
    }
}

/// A minion kind's VAT mesh, the transform that stands it on the ground at its height, and
/// its clips.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MinionVisual {
    pub(crate) mesh: MeshId,
    pub(crate) normalize: Mat4,
    pub(crate) animation: VatAnimation,
}

impl MinionVisual {
    /// The model-to-world transform of a minion standing at `ground` facing `facing` (the
    /// movement angle in the map's x/z plane; models face +Z).
    pub(crate) fn transform(&self, ground: Vec3, facing: f32) -> Mat4 {
        let rotation = Quat::from_rotation_y(core::f32::consts::FRAC_PI_2 - facing);
        Mat4::from_rotation_translation(rotation, ground) * self.normalize
    }
}

/// The loaded VAT minion visuals (melee, ranged).
#[derive(Debug)]
pub(crate) struct MinionLibrary {
    root: PathBuf,
    enabled: bool,
    quality: VatQuality,
    melee: Option<MinionVisual>,
    ranged: Option<MinionVisual>,
    /// Whether `bind` ran for the current renderer.
    bound: bool,
}

impl MinionLibrary {
    pub(crate) fn new(root: PathBuf, enabled: bool, quality: VatQuality) -> Self {
        Self {
            root,
            enabled,
            quality,
            melee: None,
            ranged: None,
            bound: false,
        }
    }

    /// From the asset root, [`VAT_ENV`] and the tier. No I/O.
    pub(crate) fn from_env(tier: QualityTier) -> Self {
        Self::new(
            crate::app::models::asset_root_from_env(),
            vat_enabled_from_env(),
            vat_quality(tier),
        )
    }

    /// Forgets the uploaded meshes (a new renderer has a fresh mesh table).
    pub(crate) fn renderer_recreated(&mut self) {
        self.melee = None;
        self.ranged = None;
        self.bound = false;
    }

    /// The shader work per instance.
    pub(crate) fn quality(&self) -> VatQuality {
        self.quality
    }

    /// Loads and registers the minion VAT assets once per renderer. `register` uploads one
    /// (normally [`moba_render::Renderer::register_vat_mesh`]). Load time only.
    pub(crate) fn bind<R>(&mut self, mut register: R)
    where
        R: FnMut(&MeshData, &TextureData, &VatDescriptor) -> Result<MeshId, String>,
    {
        if self.bound {
            return;
        }
        self.bound = true;
        if !self.enabled {
            eprintln!("[vat] {VAT_ENV}=off: minions draw as cubes");
            return;
        }
        // Each asset is uploaded once even when both kinds use it.
        let mut loaded: Vec<(&str, Option<Loaded>)> = Vec::with_capacity(3);
        let mut resolve = |names: [&'static str; 2], height: f32| {
            for name in names {
                let entry =
                    if let Some((_, entry)) = loaded.iter().find(|(loaded, _)| *loaded == name) {
                        *entry
                    } else {
                        let entry = match load(&self.root, name, &mut register) {
                            Ok(entry) => {
                                eprintln!("[vat] minion asset '{name}' loaded");
                                Some(entry)
                            }
                            Err(LoadError::Missing) => None,
                            Err(LoadError::Failed(reason)) => {
                                eprintln!("[vat] minion asset '{name}': {reason}; trying the next");
                                None
                            }
                        };
                        loaded.push((name, entry));
                        entry
                    };
                if let Some((mesh, (min, max), animation)) = entry {
                    let span = max[1] - min[1];
                    if !span.is_finite() || span <= 1e-4 {
                        continue;
                    }
                    let scale = height / span;
                    let center =
                        Vec3::new(min[0].midpoint(max[0]), min[1], min[2].midpoint(max[2]));
                    return Some(MinionVisual {
                        mesh,
                        normalize: Mat4::from_scale(Vec3::splat(scale))
                            * Mat4::from_translation(-center),
                        animation,
                    });
                }
            }
            None
        };
        self.melee = resolve(["minion-melee", "minion"], MELEE_HEIGHT);
        self.ranged = resolve(["minion-ranged", "minion"], RANGED_HEIGHT);
        for (kind, visual) in [("melee", self.melee), ("ranged", self.ranged)] {
            if visual.is_none() {
                eprintln!("[vat] no usable VAT asset for {kind} minions; drawing them as cubes");
            }
        }
    }

    /// The VAT visual of a minion kind, if it has one. Never allocates.
    pub(crate) fn visual(&self, kind: UnitKind) -> Option<&MinionVisual> {
        match kind {
            UnitKind::MeleeMinion => self.melee.as_ref(),
            UnitKind::RangedMinion => self.ranged.as_ref(),
            _ => None,
        }
    }

    /// Replaces the visuals (tests and the stress scene).
    #[cfg(test)]
    #[allow(clippy::large_types_passed_by_value)] // Test setup.
    pub(crate) fn set_visuals(
        &mut self,
        melee: Option<MinionVisual>,
        ranged: Option<MinionVisual>,
    ) {
        self.melee = melee;
        self.ranged = ranged;
        self.bound = true;
    }
}

enum LoadError {
    /// No descriptor file: not an error, the next name is tried.
    Missing,
    Failed(String),
}

type Loaded = (MeshId, ([f32; 3], [f32; 3]), VatAnimation);

/// Loads `<root>/vat/<name>.{toml,ktx2}` and its model and registers them.
fn load<R>(root: &Path, name: &str, register: &mut R) -> Result<Loaded, LoadError>
where
    R: FnMut(&MeshData, &TextureData, &VatDescriptor) -> Result<MeshId, String>,
{
    let (toml, ktx2) = moba_assets::vat_paths(root, name).ok_or(LoadError::Missing)?;
    if !toml.exists() {
        return Err(LoadError::Missing);
    }
    let failed = |what: String| LoadError::Failed(what);
    let descriptor = VatDescriptor::load_file(&toml)
        .map_err(|error| failed(format!("{}: {error}", toml.display())))?;
    let texture = moba_assets::load_ktx2_file(&ktx2)
        .map_err(|error| failed(format!("{}: {error}", ktx2.display())))?;
    descriptor
        .check_texture(&texture)
        .map_err(|error| failed(error.to_string()))?;
    let model = crate::app::models::model_path(root, &descriptor.model)
        .ok_or_else(|| failed(String::from("invalid model name")))?;
    let mesh = moba_assets::load_glb_file(&model)
        .map_err(|error| failed(format!("{}: {error}", model.display())))?;
    descriptor
        .check_mesh(&mesh)
        .map_err(|error| failed(error.to_string()))?;
    let id = register(&mesh, &texture, &descriptor).map_err(failed)?;
    Ok((id, mesh.bounds(), VatAnimation::bind(&descriptor)))
}

/// One minion's playback state.
#[derive(Clone, Copy, Debug)]
struct Slot {
    net_id: Option<u16>,
    player: Option<VatPlayer>,
    /// Facing for a standing minion: the last snapshot step or towards its attack target.
    facing: Option<f32>,
}

impl Slot {
    const EMPTY: Self = Self {
        net_id: None,
        player: None,
        facing: None,
    };
}

/// A dead minion playing its death clip where it died.
#[derive(Clone, Copy, Debug)]
struct Corpse {
    player: VatPlayer,
    kind: UnitKind,
    team: u8,
    x: u16,
    y: u16,
    facing: f32,
}

/// Per-minion VAT players, the previous snapshot (for facing) and the dying minions.
pub(crate) struct MinionDriver {
    slots: Box<[Slot]>,
    /// `(net id, kind, x, y)` of the latest snapshot, sorted by net id.
    previous: Vec<(u16, u8, u16, u16)>,
    current: Vec<(u16, u8, u16, u16)>,
    queued: [NetEvent; MAX_QUEUED_EVENTS],
    queued_len: usize,
    corpses: [Option<Corpse>; MAX_CORPSES],
    next_corpse: usize,
    tick_seconds: f32,
}

impl std::fmt::Debug for MinionDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MinionDriver")
            .field("corpses", &self.corpses.iter().flatten().count())
            .finish_non_exhaustive()
    }
}

fn kind_of(value: u8) -> Option<UnitKind> {
    match value {
        1 => Some(UnitKind::MeleeMinion),
        2 => Some(UnitKind::RangedMinion),
        _ => None,
    }
}

impl MinionDriver {
    pub(crate) fn new(tick_rate: u16) -> Self {
        Self {
            slots: vec![Slot::EMPTY; SLOTS].into_boxed_slice(),
            previous: Vec::with_capacity(moba_sim::MAX_UNITS),
            current: Vec::with_capacity(moba_sim::MAX_UNITS),
            queued: [NetEvent::default(); MAX_QUEUED_EVENTS],
            queued_len: 0,
            corpses: [None; MAX_CORPSES],
            next_corpse: 0,
            tick_seconds: 1.0 / f32::from(tick_rate.max(1)),
        }
    }

    /// Forgets every minion (a rematch or content reload renumbers net ids).
    pub(crate) fn clear(&mut self) {
        self.slots.fill(Slot::EMPTY);
        self.previous.clear();
        self.current.clear();
        self.queued_len = 0;
        self.corpses = [None; MAX_CORPSES];
    }

    /// Buffers an attack or death event until [`Self::flush_events`] learns its tick.
    pub(crate) fn queue_event(&mut self, event: &NetEvent) {
        if !matches!(event.kind, EVENT_ATTACK | EVENT_UNIT_DIED) {
            return;
        }
        if let Some(slot) = self.queued.get_mut(self.queued_len) {
            *slot = *event;
            self.queued_len += 1;
        }
    }

    fn lookup(&self, net_id: u16) -> Option<(u16, u8, u16, u16)> {
        self.previous
            .binary_search_by_key(&net_id, |entry| entry.0)
            .ok()
            .map(|index| self.previous[index])
    }

    /// The slot of `net_id`, reset (with a fresh player for `visual`) if it held another
    /// minion.
    fn claim(&mut self, net_id: u16, visual: &MinionVisual) -> &mut Slot {
        let slot = &mut self.slots[usize::from(net_id) % SLOTS];
        if slot.net_id != Some(net_id) || slot.player.is_none() {
            // A facing recorded before the first pose belongs to this minion; another
            // minion's does not.
            let facing = if slot.net_id == Some(net_id) {
                slot.facing
            } else {
                None
            };
            *slot = Slot {
                net_id: Some(net_id),
                player: Some(VatPlayer::new(
                    &visual.animation,
                    f32::from(net_id) * PHASE_SECONDS,
                )),
                facing,
            };
        }
        slot
    }

    /// Applies the queued events of the snapshot at `tick`. `view_tick` is the tick the view
    /// shows now (the snapshot tick minus the interpolation delay; the tick itself for the
    /// local preview): a corpse's death starts there, so it plays as the live minion
    /// disappears.
    pub(crate) fn flush_events(&mut self, tick: u32, view_tick: f32, library: &MinionLibrary) {
        let view = view_tick * self.tick_seconds;
        for index in 0..self.queued_len {
            let event = self.queued[index];
            match event.kind {
                EVENT_ATTACK => {
                    let Some((_, kind, sx, sy)) = self.lookup(event.source) else {
                        continue;
                    };
                    let Some(visual) = kind_of(kind).and_then(|kind| library.visual(kind)).copied()
                    else {
                        continue;
                    };
                    let facing = self.lookup(event.target).and_then(|(_, _, tx, ty)| {
                        let (from, to) = (dequantize_position(sx, sy), dequantize_position(tx, ty));
                        let (dx, dy) = ((to.x - from.x).to_f32(), (to.y - from.y).to_f32());
                        (dx * dx + dy * dy > 1e-6).then(|| dy.atan2(dx))
                    });
                    let hit = tick.saturating_sub(u32::from(event.aux)) as f32 * self.tick_seconds;
                    let interval = f32::from(event.value) * self.tick_seconds;
                    let slot = self.claim(event.source, &visual);
                    if let Some(player) = slot.player.as_mut() {
                        player.attack(&visual.animation, hit, interval);
                    }
                    if facing.is_some() {
                        slot.facing = facing;
                    }
                }
                EVENT_UNIT_DIED => {
                    let Some(kind) = u8::try_from(event.value).ok().and_then(kind_of) else {
                        continue;
                    };
                    let Some(visual) = library.visual(kind).copied() else {
                        continue;
                    };
                    let slot = &mut self.slots[usize::from(event.target) % SLOTS];
                    let (player, facing) = if slot.net_id == Some(event.target) {
                        (slot.player, slot.facing)
                    } else {
                        (None, None)
                    };
                    *slot = Slot::EMPTY;
                    let mut player = player.unwrap_or_else(|| {
                        VatPlayer::new(&visual.animation, f32::from(event.target) * PHASE_SECONDS)
                    });
                    player.die(&visual.animation, view);
                    self.corpses[self.next_corpse] = Some(Corpse {
                        player,
                        kind,
                        team: event.team,
                        x: event.x,
                        y: event.y,
                        facing: facing.unwrap_or(0.0),
                    });
                    self.next_corpse = (self.next_corpse + 1) % MAX_CORPSES;
                }
                _ => {}
            }
        }
        self.queued_len = 0;
    }

    /// Records the snapshot's positions and updates each minion's movement facing.
    pub(crate) fn on_snapshot(&mut self, snapshot: &SnapshotState) {
        self.current.clear();
        for entity in &snapshot.entities {
            if self.current.len() < self.current.capacity() {
                self.current
                    .push((entity.net_id, entity.kind, entity.x, entity.y));
            }
        }
        self.current.sort_unstable_by_key(|entry| entry.0);
        for index in 0..self.current.len() {
            let (net_id, kind, x, y) = self.current[index];
            if kind_of(kind).is_none() {
                continue;
            }
            let Some((_, _, px, py)) = self.lookup(net_id) else {
                continue;
            };
            let (from, to) = (dequantize_position(px, py), dequantize_position(x, y));
            let (dx, dy) = ((to.x - from.x).to_f32(), (to.y - from.y).to_f32());
            let step = (dx * dx + dy * dy).sqrt();
            let slot = &mut self.slots[usize::from(net_id) % SLOTS];
            if step > STATIONARY && step <= TELEPORT && slot.net_id == Some(net_id) {
                slot.facing = Some(dy.atan2(dx));
            }
        }
        core::mem::swap(&mut self.previous, &mut self.current);
    }

    /// The facing to draw a standing minion with.
    pub(crate) fn rest_facing(&self, net_id: u16) -> Option<f32> {
        let slot = &self.slots[usize::from(net_id) % SLOTS];
        if slot.net_id == Some(net_id) {
            slot.facing
        } else {
            None
        }
    }

    /// The VAT pose of a live minion at render time `now` (seconds) moving at `speed` m/s.
    /// Never allocates.
    pub(crate) fn pose(
        &mut self,
        net_id: u16,
        visual: &MinionVisual,
        now: f32,
        speed: f32,
        quality: VatQuality,
    ) -> VatPose {
        let slot = self.claim(net_id, visual);
        slot.player
            .as_mut()
            .map_or_else(VatPose::default, |player| {
                player.pose(&visual.animation, now, speed, quality)
            })
    }

    /// Draws every dying minion still within its death clip and linger at `now`; finished
    /// corpses are dropped. `color` gives a dead minion's colour from its kind and team.
    /// Never allocates while `units` has capacity.
    pub(crate) fn push_corpses(
        &mut self,
        units: &mut Vec<UnitInstance>,
        library: &MinionLibrary,
        now: f32,
        center: f32,
        color: impl Fn(UnitKind, u8) -> Vec4,
    ) {
        let quality = library.quality();
        for entry in &mut self.corpses {
            let Some(corpse) = entry.as_mut() else {
                continue;
            };
            let Some(visual) = library.visual(corpse.kind) else {
                *entry = None;
                continue;
            };
            if !corpse.player.visible(&visual.animation, now) {
                *entry = None;
                continue;
            }
            if units.len() >= units.capacity() {
                continue;
            }
            let position = dequantize_position(corpse.x, corpse.y);
            let ground = Vec3::new(
                position.x.to_f32() - center,
                0.0,
                position.y.to_f32() - center,
            );
            let pose = corpse.player.pose(&visual.animation, now, 0.0, quality);
            units.push(
                UnitInstance::new(
                    visual.transform(ground, corpse.facing),
                    color(corpse.kind, corpse.team),
                )
                .with_mesh(visual.mesh)
                .with_vat(pose),
            );
        }
    }

    /// Dying minions currently drawn (tests and logs).
    #[cfg(test)]
    pub(crate) fn corpse_count(&self) -> usize {
        self.corpses.iter().flatten().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_net::protocol::NetEntity;

    const MODEL: &[u8] = include_bytes!("../../../../content/models/sentinel-skinned.glb");
    const TOML: &str = include_str!("../../../../content/vat/minion.toml");
    const KTX2: &[u8] = include_bytes!("../../../../content/vat/minion.ktx2");

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "moba-vat-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("models")).expect("models");
        std::fs::create_dir_all(root.join("vat")).expect("vat");
        std::fs::write(root.join("models/sentinel-skinned.glb"), MODEL).expect("model");
        root
    }

    fn library() -> MinionLibrary {
        let descriptor = VatDescriptor::parse(TOML).expect("descriptor");
        let visual = MinionVisual {
            mesh: MeshId::CUBE,
            normalize: Mat4::IDENTITY,
            animation: VatAnimation::bind(&descriptor),
        };
        let mut library = MinionLibrary::new(PathBuf::new(), true, VatQuality::FULL);
        library.set_visuals(Some(visual), Some(visual));
        library
    }

    #[test]
    fn assets_load_once_and_fall_back_when_missing_or_broken() {
        let root = temp_root("load");
        std::fs::write(root.join("vat/minion.toml"), TOML).expect("toml");
        std::fs::write(root.join("vat/minion.ktx2"), KTX2).expect("ktx2");
        // A broken melee-specific asset falls through to the shared one.
        std::fs::write(root.join("vat/minion-melee.toml"), "model = 3").expect("broken");
        let mut uploads = 0;
        let mut library = MinionLibrary::new(root.clone(), true, VatQuality::FULL);
        library.bind(|_: &MeshData, _: &TextureData, _: &VatDescriptor| {
            uploads += 1;
            Ok(MeshId::CUBE)
        });
        assert_eq!(uploads, 1, "both kinds share one upload");
        let melee = library.visual(UnitKind::MeleeMinion).expect("melee");
        let ranged = library.visual(UnitKind::RangedMinion).expect("ranged");
        // Fitted to the kinds' heights.
        let top = |visual: &MinionVisual| {
            visual
                .normalize
                .transform_point3(Vec3::new(0.0, 3.78, 0.0))
                .y
        };
        assert!((top(melee) - MELEE_HEIGHT).abs() < 0.05);
        assert!((top(ranged) - RANGED_HEIGHT).abs() < 0.05);
        assert!(library.visual(UnitKind::Tower).is_none());
        // A mismatched texture (another descriptor's size) or the off switch: cubes.
        std::fs::write(
            root.join("vat/minion.toml"),
            TOML.replace("height = 42", "height = 43"),
        )
        .expect("toml");
        let mut broken = MinionLibrary::new(root.clone(), true, VatQuality::FULL);
        broken.bind(|_: &MeshData, _: &TextureData, _: &VatDescriptor| Ok(MeshId::CUBE));
        assert!(broken.visual(UnitKind::MeleeMinion).is_none());
        let mut off = MinionLibrary::new(root.clone(), false, VatQuality::FULL);
        off.bind(
            |_: &MeshData, _: &TextureData, _: &VatDescriptor| -> Result<MeshId, String> {
                panic!("no upload with MOBA_VAT=off")
            },
        );
        assert!(off.visual(UnitKind::MeleeMinion).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    fn minion(net_id: u16, x: f32) -> NetEntity {
        let quantize = |value: f32| (value * 16.0) as u16;
        NetEntity {
            net_id,
            kind: 1,
            team: 0,
            hero_slot: 0xFF,
            hero_pick: 0xFF,
            flags: 0,
            status_bits: 0,
            max_health: 400,
            max_mana: 0,
            x: quantize(x),
            y: quantize(100.0),
            health: 400,
            mana: 0,
            cooldowns: [0; 3],
            progress: moba_net::protocol::NetProgress::default(),
        }
    }

    fn snapshot(tick: u32, entities: Vec<NetEntity>) -> SnapshotState {
        let mut state = SnapshotState::default();
        state.tick = tick;
        state.entities = entities;
        state
    }

    #[test]
    fn attacks_and_deaths_drive_players_and_corpses() {
        let library = library();
        let visual = *library.visual(UnitKind::MeleeMinion).expect("visual");
        let mut driver = MinionDriver::new(30);
        driver.on_snapshot(&snapshot(10, vec![minion(7, 100.0), minion(8, 104.0)]));
        driver.on_snapshot(&snapshot(11, vec![minion(7, 101.0), minion(8, 104.0)]));
        assert!(driver.rest_facing(7).is_none(), "no player yet");
        let idle = driver.pose(7, &visual, 0.5, 0.0, VatQuality::FULL);
        assert_eq!(idle.blend, 0.0);
        // Minion 7 hits minion 8 on tick 20 (event carried by the tick-20 snapshot).
        driver.queue_event(&NetEvent {
            kind: EVENT_ATTACK,
            source: 7,
            target: 8,
            value: 30,
            ..NetEvent::default()
        });
        driver.flush_events(20, 16.0, &library);
        assert_eq!(
            driver.rest_facing(7),
            Some(0.0),
            "faces +X towards its target"
        );
        let swinging = driver.pose(7, &visual, 20.0 / 30.0, 0.0, VatQuality::FULL);
        assert!(swinging.blend > 0.9, "attack clip on top: {swinging:?}");
        // Minion 8 dies: its corpse plays the death clip, then is dropped.
        driver.queue_event(&NetEvent {
            kind: EVENT_UNIT_DIED,
            target: 8,
            value: 1,
            x: 104 * 16,
            y: 100 * 16,
            ..NetEvent::default()
        });
        driver.flush_events(30, 26.0, &library);
        assert_eq!(driver.corpse_count(), 1);
        let mut units = Vec::with_capacity(8);
        let at = 26.0 / 30.0;
        driver.push_corpses(&mut units, &library, at + 0.5, 128.0, |_, _| Vec4::ONE);
        assert_eq!(units.len(), 1);
        assert!(units[0].vat.blend > 0.9, "death clip");
        units.clear();
        driver.push_corpses(&mut units, &library, at + 10.0, 128.0, |_, _| Vec4::ONE);
        assert_eq!(units.len(), 0);
        assert_eq!(driver.corpse_count(), 0);
        // Hero attacks and unknown sources are ignored.
        driver.queue_event(&NetEvent {
            kind: EVENT_ATTACK,
            source: 99,
            ..NetEvent::default()
        });
        driver.flush_events(31, 27.0, &library);
    }

    #[test]
    fn steady_state_minion_frames_do_not_allocate() {
        let library = library();
        let visual = *library.visual(UnitKind::MeleeMinion).expect("visual");
        let mut driver = MinionDriver::new(30);
        let mut units: Vec<UnitInstance> = Vec::with_capacity(256);
        let entities: Vec<NetEntity> = (0..150).map(|id| minion(id + 10, f32::from(id))).collect();
        let mut state = snapshot(0, entities);
        // Warm-up.
        driver.on_snapshot(&state);
        let ((), allocations) = crate::app::allocation_probe::probe(|| {
            for tick in 1..120u32 {
                state.tick = tick;
                for entity in &mut state.entities {
                    entity.x = entity.x.wrapping_add(4);
                }
                driver.queue_event(&NetEvent {
                    kind: EVENT_ATTACK,
                    source: 10 + (tick % 150) as u16,
                    target: 11,
                    value: 30,
                    ..NetEvent::default()
                });
                if tick % 10 == 0 {
                    driver.queue_event(&NetEvent {
                        kind: EVENT_UNIT_DIED,
                        target: 10 + (tick % 150) as u16,
                        value: 2,
                        ..NetEvent::default()
                    });
                }
                driver.flush_events(tick, tick as f32 - 4.0, &library);
                driver.on_snapshot(&state);
                units.clear();
                let now = tick as f32 / 30.0;
                for entity in &state.entities {
                    let pose = driver.pose(entity.net_id, &visual, now, 4.0, VatQuality::FULL);
                    units.push(UnitInstance::new(Mat4::IDENTITY, Vec4::ONE).with_vat(pose));
                }
                driver.push_corpses(&mut units, &library, now, 128.0, |_, _| Vec4::ONE);
            }
        });
        assert_eq!(allocations, 0, "minion frames allocated");
    }
}
