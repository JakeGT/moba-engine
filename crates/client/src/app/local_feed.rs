//! The local preview's animation feed (R4b, ADR-0012 amendment).
//!
//! The local bot preview (no `MOBA_SERVER`) runs the simulation in this process. So that it
//! plays the same animations as a network match, each simulated tick is turned into what a
//! spectator client would receive: a [`SnapshotState`] of every unit, quantised exactly like
//! the wire ([`NetEntity::from_observed`]), and the tick's [`SimEvent`]s as the `NetEvent`s the
//! server would send (`EVENT_CAST`, `EVENT_ATTACK`, `EVENT_UNIT_DIED`, `EVENT_RECALL`). The app then drives the
//! hero `AnimationDriver` and the `MinionDriver` with them exactly as it does in a match.
//!
//! This is a local adapter only: the wire, the server and the simulation are untouched (it
//! reads the simulation after its tick). Net ids are entity indices plus one. Every buffer is
//! preallocated; capturing a tick does not allocate.

use moba_core::FixVec2;
use moba_net::protocol::{
    EVENT_ATTACK, EVENT_CAST, EVENT_RECALL, EVENT_UNIT_DIED, NetEntity, NetEvent, RECALL_CANCELLED,
    RECALL_COMPLETED, RECALL_STARTED, SnapshotState, dequantize_position, quantize_position,
};
use moba_sim::{ObservedUnit, RecallEnd, SimEvent, Simulation};

/// Events kept per tick (the simulation's queue holds 256).
const MAX_EVENTS: usize = 256;

/// The local preview's per-tick snapshot and events.
#[derive(Debug)]
pub(crate) struct LocalFeed {
    observed: Vec<ObservedUnit>,
    snapshot: SnapshotState,
    /// `(net id, x, y)` of the previous tick, sorted by net id.
    previous: Vec<(u16, u16, u16)>,
    events: Vec<NetEvent>,
}

impl Default for LocalFeed {
    fn default() -> Self {
        Self::new()
    }
}

/// The local preview's net id of the entity with index `index`.
pub(crate) fn net_id(index: u32) -> u16 {
    u16::try_from(index).unwrap_or(u16::MAX - 1).wrapping_add(1)
}

impl LocalFeed {
    pub(crate) fn new() -> Self {
        let mut snapshot = SnapshotState::default();
        snapshot.entities = Vec::with_capacity(moba_sim::MAX_UNITS);
        Self {
            observed: Vec::with_capacity(moba_sim::MAX_UNITS),
            snapshot,
            previous: Vec::with_capacity(moba_sim::MAX_UNITS),
            events: Vec::with_capacity(MAX_EVENTS),
        }
    }

    /// Captures the state after `simulation`'s latest tick and that tick's events. Call after
    /// `tick` and before `clear_events`. Never allocates once warm.
    pub(crate) fn capture(&mut self, simulation: &mut Simulation) {
        self.previous.clear();
        for entity in &self.snapshot.entities {
            if self.previous.len() < self.previous.capacity() {
                self.previous.push((entity.net_id, entity.x, entity.y));
            }
        }
        simulation.observed_all(&mut self.observed);
        self.snapshot.tick = simulation.current_tick().get();
        self.snapshot.entities.clear();
        for unit in &self.observed {
            if self.snapshot.entities.len() < self.snapshot.entities.capacity() {
                self.snapshot
                    .entities
                    .push(NetEntity::from_observed(net_id(unit.entity.index()), unit));
            }
        }
        self.snapshot
            .entities
            .sort_unstable_by_key(|entity| entity.net_id);
        self.events.clear();
        for event in simulation.events() {
            if self.events.len() >= self.events.capacity() {
                break;
            }
            if let Some(event) = Self::convert(event) {
                self.events.push(event);
            }
        }
    }

    /// The `NetEvent` the server sends for `event` (only the kinds animation reads).
    #[allow(clippy::too_many_lines)] // One SimEvent mapping.
    fn convert(event: &SimEvent) -> Option<NetEvent> {
        let none = u16::MAX;
        Some(match *event {
            SimEvent::AbilityCast {
                caster,
                team,
                ability,
                target,
            } => {
                let (x, y) = quantize_position(target);
                NetEvent {
                    kind: EVENT_CAST,
                    team: team.index() as u8,
                    source: net_id(caster.index()),
                    target: none,
                    x,
                    y,
                    value: u16::from(ability),
                    ..NetEvent::default()
                }
            }
            SimEvent::BasicAttack {
                attacker,
                team,
                target,
                interval_ticks,
            } => NetEvent {
                kind: EVENT_ATTACK,
                team: team.index() as u8,
                source: net_id(attacker.index()),
                target: net_id(target.index()),
                value: u16::try_from(interval_ticks).unwrap_or(u16::MAX),
                // The snapshot is this tick's: the hit is 0 ticks old.
                aux: 0,
                ..NetEvent::default()
            },
            SimEvent::UnitDied {
                entity,
                kind,
                team,
                position,
                killer,
                hero_slot,
                killer_slot,
            } => {
                let (x, y) = quantize_position(position);
                NetEvent {
                    kind: EVENT_UNIT_DIED,
                    team: team.index() as u8,
                    source: killer.map_or(none, |killer| net_id(killer.index())),
                    target: net_id(entity.index()),
                    source_slot: killer_slot.unwrap_or(0xFF),
                    target_slot: hero_slot.unwrap_or(0xFF),
                    x,
                    y,
                    value: kind as u16,
                    ..NetEvent::default()
                }
            }
            SimEvent::RecallStarted {
                hero,
                team,
                position,
                duration_ticks,
            } => {
                let (x, y) = quantize_position(position);
                NetEvent {
                    kind: EVENT_RECALL,
                    team: team.index() as u8,
                    source: net_id(hero.index()),
                    target: none,
                    x,
                    y,
                    x2: x,
                    y2: y,
                    value: RECALL_STARTED,
                    aux: u16::try_from(duration_ticks).unwrap_or(u16::MAX),
                    ..NetEvent::default()
                }
            }
            SimEvent::RecallEnded {
                hero,
                team,
                position,
                end,
            } => {
                let (value, arrival) = match end {
                    RecallEnd::Completed { destination } => (RECALL_COMPLETED, destination),
                    _ => (RECALL_CANCELLED, position),
                };
                let (x, y) = quantize_position(position);
                let (x2, y2) = quantize_position(arrival);
                NetEvent {
                    kind: EVENT_RECALL,
                    team: team.index() as u8,
                    source: net_id(hero.index()),
                    target: none,
                    x,
                    y,
                    x2,
                    y2,
                    value,
                    ..NetEvent::default()
                }
            }
            _ => return None,
        })
    }

    /// The latest tick's snapshot.
    pub(crate) fn snapshot(&self) -> &SnapshotState {
        &self.snapshot
    }

    /// The latest tick's animation events.
    pub(crate) fn events(&self) -> &[NetEvent] {
        &self.events
    }

    /// An entity's position on the previous tick, for its speed and facing.
    pub(crate) fn previous_position(&self, net_id: u16) -> Option<FixVec2> {
        self.previous
            .binary_search_by_key(&net_id, |entry| entry.0)
            .ok()
            .map(|index| {
                let (_, x, y) = self.previous[index];
                dequantize_position(x, y)
            })
    }

    /// The hero pick of a hero's net id in the latest snapshot (`None` for anything else).
    pub(crate) fn hero_pick(&self, net_id: u16) -> Option<u8> {
        self.snapshot
            .entities
            .binary_search_by_key(&net_id, |entity| entity.net_id)
            .ok()
            .map(|index| &self.snapshot.entities[index])
            .filter(|entity| entity.kind == 0)
            .map(|entity| entity.hero_pick)
    }
}

#[cfg(test)]
mod tests {
    use super::LocalFeed;
    use moba_net::protocol::{EVENT_ATTACK, EVENT_UNIT_DIED};
    use moba_sim::{SimConfig, Simulation};

    #[test]
    fn bot_ticks_become_snapshots_and_events_without_allocating() {
        let mut simulation = Simulation::new(SimConfig::default());
        let mut feed = LocalFeed::new();
        let mut attacks = 0;
        let mut deaths = 0;
        // Warm-up: the first waves meet in lane after a while.
        for _ in 0..1_200 {
            simulation.tick_with_bots();
            feed.capture(&mut simulation);
            simulation.clear_events();
        }
        // Only the capture is counted (the simulation's own wave spawns may allocate a few
        // times per 300 ticks, within its separate allocation budget).
        let mut allocations = 0;
        for _ in 0..900 {
            simulation.tick_with_bots();
            let ((), count) = crate::app::allocation_probe::probe(|| feed.capture(&mut simulation));
            allocations += count;
            attacks += feed
                .events()
                .iter()
                .filter(|e| e.kind == EVENT_ATTACK)
                .count();
            deaths += feed
                .events()
                .iter()
                .filter(|e| e.kind == EVENT_UNIT_DIED)
                .count();
            simulation.clear_events();
        }
        assert_eq!(allocations, 0, "capturing a tick allocated");
        assert!(attacks > 0, "bots attack");
        assert!(deaths > 0, "units die");
        let snapshot = feed.snapshot();
        assert_eq!(snapshot.tick, simulation.current_tick().get());
        assert!(
            snapshot
                .entities
                .windows(2)
                .all(|pair| pair[0].net_id < pair[1].net_id),
            "sorted by net id"
        );
        let hero = snapshot
            .entities
            .iter()
            .find(|entity| entity.kind == 0)
            .expect("a hero");
        assert_eq!(feed.hero_pick(hero.net_id), Some(hero.hero_pick));
        assert!(feed.previous_position(hero.net_id).is_some());
    }

    #[test]
    fn recalls_become_recall_events_and_the_flag() {
        use moba_net::protocol::{EVENT_RECALL, RECALL_COMPLETED, RECALL_STARTED};
        use moba_sim::{PlayerCommand, TickInputs};
        let mut simulation = Simulation::new(SimConfig::default());
        let mut feed = LocalFeed::new();
        let mut inputs = TickInputs::new();
        inputs.set(0, PlayerCommand::Recall);
        simulation.tick(&inputs);
        feed.capture(&mut simulation);
        simulation.clear_events();
        let hero = simulation.hero_entity(0).expect("hero");
        let id = super::net_id(hero.index());
        let started: Vec<_> = feed
            .events()
            .iter()
            .filter(|event| event.kind == EVENT_RECALL)
            .map(|event| (event.source, event.value, event.aux))
            .collect();
        assert_eq!(started, vec![(id, RECALL_STARTED, 240)]);
        let flagged = |feed: &LocalFeed| {
            feed.snapshot()
                .entities
                .iter()
                .find(|entity| entity.net_id == id)
                .is_some_and(|entity| entity.is_recalling())
        };
        assert!(flagged(&feed));
        let idle = TickInputs::new();
        let mut completed = None;
        for _ in 0..240 {
            simulation.tick(&idle);
            feed.capture(&mut simulation);
            simulation.clear_events();
            completed = completed.or_else(|| {
                feed.events()
                    .iter()
                    .find(|event| event.kind == EVENT_RECALL)
                    .map(|event| event.value)
            });
        }
        assert_eq!(completed, Some(RECALL_COMPLETED));
        assert!(!flagged(&feed));
    }
}
