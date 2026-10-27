//! Snapshot state: entities, deltas, events and delta building.

use moba_core::FixVec2;
use moba_sim::ObservedUnit;

use super::bytes::{clamp_u16, clamp_u16_u32};
use super::{WireError, dequantize_position, quantize_position};

/// High-level match phase carried in snapshots.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MatchPhase {
    /// Waiting for players; the simulation is paused.
    Lobby = 0,
    /// Pre-match countdown; the simulation is paused.
    Countdown = 1,
    /// The simulation is running.
    Playing = 2,
    /// Nexus destroyed; showing results before a rematch.
    Results = 3,
}

impl MatchPhase {
    /// Decodes a phase byte.
    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Lobby,
            1 => Self::Countdown,
            2 => Self::Playing,
            3 => Self::Results,
            _ => return None,
        })
    }
}

/// Network entity identifier assigned per connection.
pub type NetId = u16;

/// Hero progression carried in snapshots.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct NetProgress {
    /// Current level.
    pub level: u8,
    /// Experience towards the next level.
    pub xp: u16,
    /// Gold (only filled for the receiving player's hero).
    pub gold: u16,
    /// Inventory item ids (`0xFF` empty).
    pub items: [u8; 6],
    /// Remaining item active cooldowns in ticks.
    pub item_cooldowns: [u16; 6],
}

/// Quantised entity state carried in snapshots.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NetEntity {
    /// Connection-local id.
    pub net_id: NetId,
    /// Unit kind (`moba_sim::UnitKind` discriminant).
    pub kind: u8,
    /// Team discriminant.
    pub team: u8,
    /// Hero slot or `0xFF`.
    pub hero_slot: u8,
    /// Hero definition index or `0xFF`.
    pub hero_pick: u8,
    /// Legacy u8 flags (dead + standard status tint bits).
    pub flags: u8,
    /// Status bitset (wide; includes burn/haste and future tags).
    pub status_bits: u16,
    /// Maximum health.
    pub max_health: u16,
    /// Maximum mana (zero for non-heroes).
    pub max_mana: u16,
    /// Quantised X.
    pub x: u16,
    /// Quantised Y.
    pub y: u16,
    /// Health.
    pub health: u16,
    /// Mana.
    pub mana: u16,
    /// Remaining Q/W/E cooldowns in ticks.
    pub cooldowns: [u16; 3],
    /// Level, XP, gold and inventory.
    pub progress: NetProgress,
}

/// Flag bit for dead heroes.
pub const ENTITY_FLAG_DEAD: u8 = 1;
/// A temporary shield is active.
pub const ENTITY_FLAG_SHIELD: u8 = 1 << 1;
/// A movement slow is active.
pub const ENTITY_FLAG_SLOW: u8 = 1 << 2;
/// The unit is rooted.
pub const ENTITY_FLAG_ROOT: u8 = 1 << 3;
/// The unit is silenced.
pub const ENTITY_FLAG_SILENCE: u8 = 1 << 4;
/// The unit is burning.
pub const ENTITY_FLAG_BURN: u8 = 1 << 5;
/// A movement haste is active.
pub const ENTITY_FLAG_HASTE: u8 = 1 << 6;

/// Wide status bit: dead hero.
pub const STATUS_DEAD: u16 = 1 << 0;
/// Wide status bit: shielded.
pub const STATUS_SHIELD: u16 = 1 << 1;
/// Wide status bit: slowed.
pub const STATUS_SLOW: u16 = 1 << 2;
/// Wide status bit: rooted.
pub const STATUS_ROOT: u16 = 1 << 3;
/// Wide status bit: silenced.
pub const STATUS_SILENCE: u16 = 1 << 4;
/// Wide status bit: burning.
pub const STATUS_BURN: u16 = 1 << 5;
/// Wide status bit: hasted.
pub const STATUS_HASTE: u16 = 1 << 6;

impl NetEntity {
    /// Quantises an observed unit.
    #[must_use]
    pub fn from_observed(net_id: NetId, unit: &ObservedUnit) -> Self {
        let (x, y) = quantize_position(unit.position);
        Self {
            net_id,
            kind: unit.kind as u8,
            team: unit.team.index() as u8,
            hero_slot: unit.hero_slot.unwrap_or(0xFF),
            hero_pick: unit.hero_pick,
            flags: (if unit.dead { ENTITY_FLAG_DEAD } else { 0 })
                | (if unit.shielded { ENTITY_FLAG_SHIELD } else { 0 })
                | (if unit.slowed { ENTITY_FLAG_SLOW } else { 0 })
                | (if unit.rooted { ENTITY_FLAG_ROOT } else { 0 })
                | (if unit.silenced {
                    ENTITY_FLAG_SILENCE
                } else {
                    0
                })
                | (if unit.burning { ENTITY_FLAG_BURN } else { 0 })
                | (if unit.hasted { ENTITY_FLAG_HASTE } else { 0 }),
            status_bits: unit.status_bits
                | (if unit.dead { STATUS_DEAD } else { 0 })
                | (if unit.shielded { STATUS_SHIELD } else { 0 })
                | (if unit.slowed { STATUS_SLOW } else { 0 })
                | (if unit.rooted { STATUS_ROOT } else { 0 })
                | (if unit.silenced { STATUS_SILENCE } else { 0 })
                | (if unit.burning { STATUS_BURN } else { 0 })
                | (if unit.hasted { STATUS_HASTE } else { 0 }),
            max_health: clamp_u16(unit.max_health),
            max_mana: clamp_u16(unit.max_mana),
            x,
            y,
            health: clamp_u16(unit.health),
            mana: clamp_u16(unit.mana),
            cooldowns: [
                clamp_u16_u32(unit.cooldowns[0]),
                clamp_u16_u32(unit.cooldowns[1]),
                clamp_u16_u32(unit.cooldowns[2]),
            ],
            progress: NetProgress {
                level: unit.level,
                xp: clamp_u16_u32(unit.xp),
                gold: clamp_u16_u32(unit.gold),
                items: unit.items,
                item_cooldowns: [0; 6],
            },
        }
    }

    /// Decodes the quantised position.
    #[must_use]
    pub fn position(self) -> FixVec2 {
        dequantize_position(self.x, self.y)
    }

    /// Whether this entity is a dead hero.
    #[must_use]
    pub const fn is_dead(self) -> bool {
        self.flags & ENTITY_FLAG_DEAD != 0
    }

    /// Returns the changed dynamic fields relative to `previous`.
    #[must_use]
    pub fn diff(&self, previous: &Self) -> NetUpdate {
        NetUpdate {
            net_id: self.net_id,
            position: (self.x != previous.x || self.y != previous.y).then_some((self.x, self.y)),
            health: (self.health != previous.health).then_some(self.health),
            mana: (self.mana != previous.mana).then_some(self.mana),
            flags: (self.flags != previous.flags).then_some(self.flags),
            status_bits: (self.status_bits != previous.status_bits).then_some(self.status_bits),
            cooldowns: (self.cooldowns != previous.cooldowns).then_some(self.cooldowns),
            progress: (self.progress != previous.progress).then_some(self.progress),
        }
    }

    /// Applies a delta update.
    pub fn apply(&mut self, update: &NetUpdate) {
        if let Some((x, y)) = update.position {
            self.x = x;
            self.y = y;
        }
        if let Some(health) = update.health {
            self.health = health;
        }
        if let Some(mana) = update.mana {
            self.mana = mana;
        }
        if let Some(flags) = update.flags {
            self.flags = flags;
        }
        if let Some(status_bits) = update.status_bits {
            self.status_bits = status_bits;
        }
        if let Some(cooldowns) = update.cooldowns {
            self.cooldowns = cooldowns;
        }
        if let Some(progress) = update.progress {
            self.progress = progress;
        }
    }
}

/// Changed fields of one entity.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NetUpdate {
    /// Entity id.
    pub net_id: NetId,
    /// Position when changed.
    pub position: Option<(u16, u16)>,
    /// Health when changed.
    pub health: Option<u16>,
    /// Mana when changed.
    pub mana: Option<u16>,
    /// Flags when changed.
    pub flags: Option<u8>,
    /// Wide status bitset when changed.
    pub status_bits: Option<u16>,
    /// Cooldowns when changed.
    pub cooldowns: Option<[u16; 3]>,
    /// Progression/inventory when changed.
    pub progress: Option<NetProgress>,
}

/// One snapshot, either a full state (`baseline_seq == 0`) or a delta.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotDelta {
    /// Server tick that produced the snapshot.
    pub tick: u32,
    /// Last client input sequence the server processed.
    pub acked_input: u32,
    /// Snapshot sequence this delta is based on (0 = full).
    pub baseline_seq: u32,
    /// Blue team hero kills.
    pub blue_score: u32,
    /// Red team hero kills.
    pub red_score: u32,
    /// Match outcome: 0 none, 1 blue, 2 red.
    pub outcome: u8,
    /// Current match phase.
    pub phase: u8,
    /// Ticks remaining in countdown/results.
    pub countdown: u16,
    /// Bitmask of player slots that are ready.
    pub ready_mask: u16,
    /// Bitmask of connected player slots.
    pub player_mask: u16,
    /// Newly visible entities (always full state).
    pub spawns: Vec<NetEntity>,
    /// Changed entities.
    pub updates: Vec<NetUpdate>,
    /// Entities that left visibility or died.
    pub despawns: Vec<NetId>,
    /// Presentation events since the previous snapshot.
    pub events: Vec<NetEvent>,
}

/// Maximum presentation events per snapshot.
pub const MAX_SNAPSHOT_EVENTS: usize = 16;

/// One presentation event in a snapshot.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct NetEvent {
    /// Event kind (see `EVENT_*` constants).
    pub kind: u8,
    /// Primary team (caster/owner/victim depending on kind).
    pub team: u8,
    /// Source entity net id (`0xFFFF` none).
    pub source: u16,
    /// Target entity net id (`0xFFFF` none).
    pub target: u16,
    /// Source hero slot (`0xFF` none).
    pub source_slot: u8,
    /// Target hero slot (`0xFF` none).
    pub target_slot: u8,
    /// Quantised primary position.
    pub x: u16,
    /// Quantised primary position.
    pub y: u16,
    /// Quantised secondary/end position.
    pub x2: u16,
    /// Quantised secondary/end position.
    pub y2: u16,
    /// Kind-specific value (ability id, unit kind, speed).
    pub value: u16,
    /// Kind-specific auxiliary value (radius, wave).
    pub aux: u16,
    /// Normalised direction X (`i16::MIN..=i16::MAX`).
    pub dx: i16,
    /// Normalised direction Y (`i16::MIN..=i16::MAX`).
    pub dy: i16,
    /// Visual projectile arc height in 1/16 world units.
    pub arc: i16,
}

/// Ability cast event.
pub const EVENT_CAST: u8 = 1;
/// Projectile spawned event.
pub const EVENT_PROJECTILE_SPAWN: u8 = 2;
/// Projectile hit event.
pub const EVENT_PROJECTILE_HIT: u8 = 3;
/// Unit died event.
pub const EVENT_UNIT_DIED: u8 = 4;
/// Structure destroyed event.
pub const EVENT_STRUCTURE_DESTROYED: u8 = 5;
/// Match ended event.
pub const EVENT_MATCH_ENDED: u8 = 6;
/// Zone spawned event.
pub const EVENT_ZONE_SPAWNED: u8 = 7;

impl SnapshotDelta {
    /// Whether the delta carries no state at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.spawns.is_empty()
            && self.updates.is_empty()
            && self.despawns.is_empty()
            && self.events.is_empty()
    }
}

/// Client snapshot store. Entities are sorted by id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotState {
    /// Snapshot sequence this state corresponds to.
    pub seq: u32,
    /// Server tick.
    pub tick: u32,
    /// Blue team hero kills.
    pub blue_score: u32,
    /// Red team hero kills.
    pub red_score: u32,
    /// Match outcome: 0 none, 1 blue, 2 red.
    pub outcome: u8,
    /// Current match phase.
    pub phase: u8,
    /// Ticks remaining in countdown/results.
    pub countdown: u16,
    /// Bitmask of player slots that are ready.
    pub ready_mask: u16,
    /// Bitmask of connected player slots.
    pub player_mask: u16,
    /// Entities sorted by network id.
    pub entities: Vec<NetEntity>,
}

impl SnapshotState {
    /// Builds a state from a full snapshot.
    #[must_use]
    pub fn from_full(seq: u32, delta: &SnapshotDelta) -> Self {
        let mut entities = delta.spawns.clone();
        entities.sort_unstable_by_key(|entity| entity.net_id);
        Self {
            seq,
            tick: delta.tick,
            blue_score: delta.blue_score,
            red_score: delta.red_score,
            outcome: delta.outcome,
            phase: delta.phase,
            countdown: delta.countdown,
            ready_mask: delta.ready_mask,
            player_mask: delta.player_mask,
            entities,
        }
    }

    /// Applies a delta that is based on this state.
    ///
    /// # Errors
    /// Returns [`WireError::MissingBaseline`] if an update/despawn references an entity
    /// this state does not know.
    pub fn apply(&self, delta: &SnapshotDelta, seq: u32) -> Result<Self, WireError> {
        if delta.baseline_seq == 0 {
            return Ok(Self::from_full(seq, delta));
        }
        if delta.baseline_seq != self.seq {
            return Err(WireError::MissingBaseline);
        }
        let mut entities = self.entities.clone();
        for &net_id in &delta.despawns {
            let index = entities
                .binary_search_by_key(&net_id, |entity| entity.net_id)
                .map_err(|_| WireError::MissingBaseline)?;
            entities.remove(index);
        }
        for update in &delta.updates {
            let index = entities
                .binary_search_by_key(&update.net_id, |entity| entity.net_id)
                .map_err(|_| WireError::MissingBaseline)?;
            entities[index].apply(update);
        }
        for spawn in &delta.spawns {
            let index = entities
                .binary_search_by_key(&spawn.net_id, |entity| entity.net_id)
                .unwrap_or_else(|index| index);
            entities.insert(index, *spawn);
        }
        Ok(Self {
            seq,
            tick: delta.tick,
            blue_score: delta.blue_score,
            red_score: delta.red_score,
            outcome: delta.outcome,
            phase: delta.phase,
            countdown: delta.countdown,
            ready_mask: delta.ready_mask,
            player_mask: delta.player_mask,
            entities,
        })
    }

    /// Finds an entity by network id.
    #[must_use]
    pub fn get(&self, net_id: NetId) -> Option<&NetEntity> {
        self.entities
            .binary_search_by_key(&net_id, |entity| entity.net_id)
            .ok()
            .map(|index| &self.entities[index])
    }
}

/// Builds a delta from the current visible state and an optional baseline.
#[must_use]
pub fn build_delta(
    current: &[NetEntity],
    baseline: Option<&[NetEntity]>,
    tick: u32,
    acked_input: u32,
    baseline_seq: u32,
) -> SnapshotDelta {
    let mut delta = SnapshotDelta::default();
    build_delta_into(
        current,
        baseline,
        tick,
        acked_input,
        baseline_seq,
        &mut delta,
    );
    delta
}

/// Builds a snapshot into a reused delta, retaining vector capacity.
pub fn build_delta_into(
    current: &[NetEntity],
    baseline: Option<&[NetEntity]>,
    tick: u32,
    acked_input: u32,
    baseline_seq: u32,
    delta: &mut SnapshotDelta,
) {
    delta.clear();
    delta.tick = tick;
    delta.acked_input = acked_input;
    let Some(baseline) = baseline else {
        delta.baseline_seq = 0;
        delta.spawns.extend_from_slice(current);
        return;
    };
    delta.baseline_seq = baseline_seq;
    let (mut old_index, mut new_index) = (0usize, 0usize);
    while old_index < baseline.len() || new_index < current.len() {
        match (baseline.get(old_index), current.get(new_index)) {
            (Some(old), Some(new)) if old.net_id == new.net_id => {
                let update = new.diff(old);
                if update.position.is_some()
                    || update.health.is_some()
                    || update.mana.is_some()
                    || update.flags.is_some()
                    || update.status_bits.is_some()
                    || update.cooldowns.is_some()
                    || update.progress.is_some()
                {
                    delta.updates.push(update);
                }
                old_index += 1;
                new_index += 1;
            }
            (Some(old), Some(new)) if old.net_id < new.net_id => {
                delta.despawns.push(old.net_id);
                old_index += 1;
            }
            (_, Some(new)) => {
                delta.spawns.push(*new);
                new_index += 1;
            }
            (Some(old), None) => {
                delta.despawns.push(old.net_id);
                old_index += 1;
            }
            (None, None) => break,
        }
    }
}

impl SnapshotDelta {
    fn clear(&mut self) {
        self.tick = 0;
        self.acked_input = 0;
        self.baseline_seq = 0;
        self.blue_score = 0;
        self.red_score = 0;
        self.outcome = 0;
        self.phase = 0;
        self.countdown = 0;
        self.ready_mask = 0;
        self.player_mask = 0;
        self.spawns.clear();
        self.updates.clear();
        self.despawns.clear();
        self.events.clear();
    }
}
