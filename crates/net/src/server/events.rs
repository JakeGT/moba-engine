//! Simulation events retained between ticks and filtered per viewer.

use crate::protocol::{
    EVENT_ATTACK, EVENT_CAST, EVENT_MATCH_ENDED, EVENT_PROJECTILE_HIT, EVENT_PROJECTILE_SPAWN,
    EVENT_STRUCTURE_DESTROYED, EVENT_UNIT_DIED, EVENT_ZONE_SPAWNED, MATCH_ENDED_DRAW, NetEvent,
    quantize_position,
};

use super::clients::ClientConn;

/// Event retained between ticks and filtered per viewer when snapshots are sent.
#[derive(Copy, Clone, Debug)]
pub(super) struct RawEvent {
    pub(super) kind: u8,
    pub(super) team: moba_game::Team,
    pub(super) source: Option<moba_ecs::Entity>,
    pub(super) target: Option<moba_ecs::Entity>,
    pub(super) source_slot: u8,
    pub(super) target_slot: u8,
    pub(super) position: moba_core::FixVec2,
    pub(super) end: moba_core::FixVec2,
    pub(super) direction: moba_core::FixVec2,
    pub(super) value: u16,
    pub(super) aux: u16,
    pub(super) arc: moba_core::Fix,
    /// First snapshot tick whose state shows the event's effect (the simulation tick count
    /// after the tick that produced it). [`EVENT_ATTACK`] sends its age relative to this.
    pub(super) tick: u32,
}

impl RawEvent {
    #[allow(clippy::too_many_lines)] // One exhaustive SimEvent conversion.
    pub(super) fn from_sim(
        event: &moba_sim::SimEvent,
        simulation: &moba_sim::Simulation,
    ) -> Option<Self> {
        let slot = |entity: moba_ecs::Entity| {
            simulation
                .world()
                .get::<moba_sim::HeroSlot>(entity)
                .map_or(0xFF, |slot| slot.0)
        };
        let tick = simulation.current_tick().get();
        let raw = match *event {
            moba_sim::SimEvent::AbilityCast {
                caster,
                team,
                ability,
                target,
            } => Self {
                kind: EVENT_CAST,
                team,
                source: Some(caster),
                target: None,
                source_slot: slot(caster),
                target_slot: 0xFF,
                position: target,
                end: target,
                direction: moba_core::FixVec2::ZERO,
                value: u16::from(ability),
                aux: 0,
                arc: moba_core::Fix::ZERO,
                tick,
            },
            moba_sim::SimEvent::ProjectileSpawned {
                owner,
                team,
                position,
                direction,
                speed_per_sec,
                radius,
                range,
                arc,
            } => Self {
                kind: EVENT_PROJECTILE_SPAWN,
                team,
                source: Some(owner),
                target: None,
                source_slot: slot(owner),
                target_slot: 0xFF,
                position,
                end: position + direction * range,
                direction,
                value: u16::try_from(speed_per_sec.to_i32().max(0)).unwrap_or(u16::MAX),
                aux: u16::try_from(radius.to_i32().max(0)).unwrap_or(u16::MAX),
                arc,
                tick,
            },
            moba_sim::SimEvent::ProjectileHit {
                owner,
                team,
                target,
                position,
            } => Self {
                kind: EVENT_PROJECTILE_HIT,
                team,
                source: Some(owner),
                target: Some(target),
                source_slot: slot(owner),
                target_slot: slot(target),
                position,
                end: position,
                direction: moba_core::FixVec2::ZERO,
                value: 0,
                aux: 0,
                arc: moba_core::Fix::ZERO,
                tick,
            },
            moba_sim::SimEvent::UnitDied {
                entity,
                kind,
                team,
                position,
                killer,
                hero_slot,
                killer_slot,
            } => Self {
                kind: EVENT_UNIT_DIED,
                team,
                source: killer,
                target: Some(entity),
                source_slot: killer_slot.unwrap_or(0xFF),
                target_slot: hero_slot.unwrap_or(0xFF),
                position,
                end: position,
                direction: moba_core::FixVec2::ZERO,
                value: kind as u16,
                aux: 0,
                arc: moba_core::Fix::ZERO,
                tick,
            },
            moba_sim::SimEvent::StructureDestroyed {
                team,
                kind,
                position,
            } => Self {
                kind: EVENT_STRUCTURE_DESTROYED,
                team,
                source: None,
                target: None,
                source_slot: 0xFF,
                target_slot: 0xFF,
                position,
                end: position,
                direction: moba_core::FixVec2::ZERO,
                value: kind as u16,
                aux: 0,
                arc: moba_core::Fix::ZERO,
                tick,
            },
            moba_sim::SimEvent::MatchEnded { winner } => Self {
                kind: EVENT_MATCH_ENDED,
                // A draw has no winner; `value` carries `MATCH_ENDED_DRAW` and `team` is unused.
                team: winner.unwrap_or(moba_game::Team::Blue),
                source: None,
                target: None,
                source_slot: 0xFF,
                target_slot: 0xFF,
                position: moba_core::FixVec2::ZERO,
                end: moba_core::FixVec2::ZERO,
                direction: moba_core::FixVec2::ZERO,
                value: winner.map_or(MATCH_ENDED_DRAW, |team| team.index() as u16),
                aux: 0,
                arc: moba_core::Fix::ZERO,
                tick,
            },
            moba_sim::SimEvent::ZoneSpawned {
                owner,
                team,
                position,
                radius,
                duration_ticks,
            } => Self {
                kind: EVENT_ZONE_SPAWNED,
                team,
                source: Some(owner),
                target: None,
                source_slot: slot(owner),
                target_slot: 0xFF,
                position,
                end: position,
                direction: moba_core::FixVec2::ZERO,
                value: u16::try_from(radius.to_i32().max(0)).unwrap_or(u16::MAX),
                aux: u16::try_from(duration_ticks).unwrap_or(u16::MAX),
                arc: moba_core::Fix::ZERO,
                tick,
            },
            moba_sim::SimEvent::BasicAttack {
                attacker,
                team,
                target,
                interval_ticks,
            } => Self {
                kind: EVENT_ATTACK,
                team,
                source: Some(attacker),
                target: Some(target),
                source_slot: slot(attacker),
                target_slot: slot(target),
                position: moba_core::FixVec2::ZERO,
                end: moba_core::FixVec2::ZERO,
                direction: moba_core::FixVec2::ZERO,
                value: u16::try_from(interval_ticks).unwrap_or(u16::MAX),
                aux: 0,
                arc: moba_core::Fix::ZERO,
                tick,
            },
            // `WaveSpawned` and `HeroRespawned` are not replicated. `SimEvent` is
            // `#[non_exhaustive]`: a new simulation event is not sent to clients until it
            // gets a wire mapping here.
            _ => return None,
        };
        Some(raw)
    }

    pub(super) fn visible_to(
        &self,
        __viewer: moba_game::Team,
        spectator: bool,
        units: &[moba_sim::ObservedUnit],
    ) -> bool {
        if spectator || self.kind == EVENT_MATCH_ENDED {
            return true;
        }
        let visible = |entity: moba_ecs::Entity| units.iter().any(|unit| unit.entity == entity);
        // Never leak a hidden endpoint of an event. Dropping the event is safer than
        // encoding a hidden target id/position for the client.
        self.source.is_none_or(visible) && self.target.is_none_or(visible)
    }

    /// Send order when a snapshot cannot carry every event (lower first): deaths,
    /// structures and the match end; the other events; basic attacks by heroes; other basic
    /// attacks.
    pub(super) const fn priority(&self) -> u8 {
        match self.kind {
            EVENT_UNIT_DIED | EVENT_STRUCTURE_DESTROYED | EVENT_MATCH_ENDED => 0,
            EVENT_ATTACK if self.source_slot != 0xFF => 2,
            EVENT_ATTACK => 3,
            _ => 1,
        }
    }

    /// Whether this is a basic attack (kept in its own lower-priority queue).
    pub(super) const fn is_attack(&self) -> bool {
        self.kind == EVENT_ATTACK
    }

    /// The wire form for `client`, in the snapshot for simulation tick `snapshot_tick`.
    pub(super) fn to_net(self, client: &mut ClientConn, snapshot_tick: u32) -> NetEvent {
        if self.kind == EVENT_ATTACK {
            // Compact on the wire: no position, direction or arc. `aux` is the age of the
            // hit in ticks, so the client knows the tick it landed on.
            return NetEvent {
                kind: self.kind,
                team: self.team.index() as u8,
                source: self
                    .source
                    .map_or(0xFFFF, |entity| client.net_id_for(entity)),
                target: self
                    .target
                    .map_or(0xFFFF, |entity| client.net_id_for(entity)),
                source_slot: self.source_slot,
                target_slot: self.target_slot,
                value: self.value,
                aux: u16::try_from(snapshot_tick.saturating_sub(self.tick)).unwrap_or(u16::MAX),
                ..NetEvent::default()
            };
        }
        let (x, y) = quantize_position(self.position);
        let (x2, y2) = quantize_position(self.end);
        NetEvent {
            kind: self.kind,
            team: self.team.index() as u8,
            source: self
                .source
                .map_or(0xFFFF, |entity| client.net_id_for(entity)),
            target: self
                .target
                .map_or(0xFFFF, |entity| client.net_id_for(entity)),
            source_slot: self.source_slot,
            target_slot: self.target_slot,
            x,
            y,
            x2,
            y2,
            value: self.value,
            aux: self.aux,
            dx: (self.direction.x.to_f32() * 32_767.0).clamp(-32_767.0, 32_767.0) as i16,
            dy: (self.direction.y.to_f32() * 32_767.0).clamp(-32_767.0, 32_767.0) as i16,
            arc: (self.arc.to_f32() * 16.0).clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16,
        }
    }
}
