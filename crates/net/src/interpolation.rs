//! Snapshot interpolation for remote entities.

use std::collections::VecDeque;
use std::time::Instant;

use moba_core::{Fix, FixVec2};

use crate::protocol::{NetEntity, NetId, SnapshotState};

/// Interpolates entity positions between the last few snapshots.
#[derive(Debug)]
pub struct Interpolator {
    states: VecDeque<SnapshotState>,
    delay_ticks: f32,
    last_arrival: Instant,
    tick_rate: f32,
}

impl Default for Interpolator {
    fn default() -> Self {
        Self::new(3.0, 30.0)
    }
}

impl Interpolator {
    /// Creates an interpolator with a render delay in ticks and a server tick rate.
    #[must_use]
    pub fn new(delay_ticks: f32, tick_rate: f32) -> Self {
        Self {
            states: VecDeque::with_capacity(32),
            delay_ticks,
            last_arrival: Instant::now(),
            tick_rate,
        }
    }

    /// Stores a newly received snapshot (deduplicated and sorted by server tick).
    pub fn push(&mut self, mut state: SnapshotState) {
        self.last_arrival = Instant::now();
        if self
            .states
            .back()
            .is_some_and(|last| last.tick >= state.tick)
        {
            return;
        }
        state.entities.sort_unstable_by_key(|entity| entity.net_id);
        self.states.push_back(state);
        while self.states.len() > 32 {
            self.states.pop_front();
        }
    }

    /// Drops all buffered snapshots (used when a rematch restarts the tick).
    pub fn clear(&mut self) {
        self.states.clear();
        self.last_arrival = Instant::now();
    }

    /// Latest received snapshot.
    #[must_use]
    pub fn latest(&self) -> Option<&SnapshotState> {
        self.states.back()
    }

    /// Interpolated render tick for the current wall-clock instant.
    #[must_use]
    pub fn render_tick(&self, now: Instant) -> f32 {
        let Some(latest) = self.states.back() else {
            return 0.0;
        };
        let elapsed = now.duration_since(self.last_arrival).as_secs_f32();
        latest.tick as f32 + elapsed * self.tick_rate - self.delay_ticks
    }

    /// Returns the interpolated position of an entity at `render_tick`.
    ///
    /// Returns `None` if the entity is not present in any buffered snapshot.
    #[must_use]
    pub fn position(&self, net_id: NetId, render_tick: f32) -> Option<FixVec2> {
        let mut before: Option<(f32, &NetEntity)> = None;
        let mut after: Option<(f32, &NetEntity)> = None;
        for state in &self.states {
            let tick = state.tick as f32;
            if tick <= render_tick {
                if let Some(entity) = state.get(net_id) {
                    before = Some((tick, entity));
                }
            } else if after.is_none() {
                if let Some(entity) = state.get(net_id) {
                    after = Some((tick, entity));
                }
            }
        }
        match (before, after) {
            (Some((before_tick, before)), Some((after_tick, after))) => {
                let span = after_tick - before_tick;
                let alpha = if span <= 0.0 {
                    0.0
                } else {
                    ((render_tick - before_tick) / span).clamp(0.0, 1.0)
                };
                Some(lerp(before.position(), after.position(), alpha))
            }
            (Some((_, entity)), None) | (None, Some((_, entity))) => Some(entity.position()),
            (None, None) => None,
        }
    }
}

fn lerp(a: FixVec2, b: FixVec2, alpha: f32) -> FixVec2 {
    let alpha = alpha.clamp(0.0, 1.0);
    FixVec2::new(
        a.x + (b.x - a.x) * Fix::from_f32(alpha),
        a.y + (b.y - a.y) * Fix::from_f32(alpha),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{NetEntity, SnapshotState};

    fn entity(id: NetId, x: u16, y: u16) -> NetEntity {
        NetEntity {
            net_id: id,
            kind: 0,
            team: 0,
            hero_slot: 0,
            hero_pick: 0,
            flags: 0,
            max_health: 600,
            max_mana: 300,
            x,
            y,
            health: 600,
            mana: 300,
            cooldowns: [0; 3],
            progress: crate::protocol::NetProgress::default(),
        }
    }

    fn state(seq: u32, tick: u32, entities: Vec<NetEntity>) -> SnapshotState {
        SnapshotState {
            seq,
            tick,
            blue_score: 0,
            red_score: 0,
            outcome: 0,
            phase: 0,
            countdown: 0,
            ready_mask: 0,
            player_mask: 0,
            entities,
        }
    }

    #[test]
    fn interpolates_between_snapshots() {
        let mut interpolator = Interpolator::new(0.0, 30.0);
        interpolator.push(state(1, 10, vec![entity(1, 0, 0)]));
        interpolator.push(state(2, 12, vec![entity(1, 32, 0)]));
        let midpoint = interpolator.position(1, 11.0).unwrap();
        assert_eq!(midpoint, FixVec2::from_ints(1, 0));
        assert_eq!(
            interpolator.position(1, 9.0).unwrap(),
            FixVec2::from_ints(0, 0)
        );
        assert!(interpolator.position(2, 11.0).is_none());
    }

    #[test]
    fn clear_discards_rematch_history() {
        let mut interpolator = Interpolator::new(0.0, 30.0);
        interpolator.push(state(10, 500, vec![entity(1, 160, 160)]));
        interpolator.clear();
        interpolator.push(state(11, 1, vec![entity(1, 160, 160)]));
        assert_eq!(interpolator.latest().unwrap().tick, 1);
        assert_eq!(
            interpolator.position(1, 1.0).unwrap(),
            FixVec2::from_ints(10, 10)
        );
    }

    #[test]
    fn ignores_duplicate_or_older_snapshots() {
        let mut interpolator = Interpolator::new(0.0, 30.0);
        interpolator.push(state(1, 10, vec![entity(1, 0, 0)]));
        interpolator.push(state(2, 10, vec![entity(1, 32, 0)]));
        interpolator.push(state(3, 9, vec![entity(1, 64, 0)]));
        assert_eq!(interpolator.latest().unwrap().tick, 10);
        assert_eq!(interpolator.latest().unwrap().seq, 1);
    }
}
