//! Client-side prediction and conservative reconciliation for the local hero.
//!
//! The client applies its own movement order immediately, before the server sees it, so
//! movement feels responsive on a 150 ms link. Reconciliation is deliberately conservative:
//! - large errors (dash, respawn, server clamp) snap immediately;
//! - while moving, small/medium lag offsets are tolerated (they are latency, not desync);
//! - while idle, the predicted position converges smoothly to the authoritative one.
//!
//! The server remains authoritative; this module only decides where to *draw* the local
//! hero until the next snapshot arrives.

use moba_core::{Fix, FixVec2};

use crate::protocol::POSITION_SCALE;

/// Fixed-point duration of one simulation tick.
const TICK: Fix = moba_sim::TICK_DURATION;
/// Distance above which prediction snaps to the server instead of easing.
const SNAP_DISTANCE: Fix = Fix::from_i32(8);
/// Idle convergence step per snapshot.
const IDLE_CORRECTION: Fix = Fix::from_bits(2_147_483_648); // 0.5 units

/// Predicted local-hero state.
#[derive(Clone, Debug)]
pub struct PredictionState {
    position: FixVec2,
    target: Option<FixVec2>,
    speed_per_tick: Fix,
    initialized: bool,
    dead: bool,
}

impl PredictionState {
    /// Creates a predictor for a hero with `move_speed_per_sec`.
    #[must_use]
    pub fn new(move_speed_per_sec: Fix) -> Self {
        Self {
            position: FixVec2::ZERO,
            target: None,
            speed_per_tick: move_speed_per_sec * TICK,
            initialized: false,
            dead: false,
        }
    }

    /// Current predicted position.
    #[inline]
    #[must_use]
    pub const fn position(&self) -> FixVec2 {
        self.position
    }

    /// Current local movement target.
    #[inline]
    #[must_use]
    pub const fn target(&self) -> Option<FixVec2> {
        self.target
    }

    /// Whether the hero is currently dead.
    #[inline]
    #[must_use]
    pub const fn is_dead(&self) -> bool {
        self.dead
    }

    /// Sets a new local movement order (clamped to the map).
    pub fn set_target(&mut self, target: Option<FixVec2>) {
        self.target = target.map(clamp_position);
    }

    /// Marks the hero dead or alive; death drops any movement order.
    pub fn set_dead(&mut self, dead: bool) {
        self.dead = dead;
        if dead {
            self.target = None;
        }
    }

    /// Reconciles with the authoritative position from a snapshot.
    pub fn reconcile(&mut self, authoritative: FixVec2) {
        let authoritative = clamp_position(authoritative);
        if !self.initialized || self.dead {
            self.position = authoritative;
            self.initialized = true;
            return;
        }
        let error = self.position.distance(authoritative);
        if error > SNAP_DISTANCE {
            self.position = authoritative;
            return;
        }
        if self.target.is_none() && error > Fix::EPSILON {
            self.position = self
                .position
                .move_towards(authoritative, IDLE_CORRECTION.min(error));
        }
    }

    /// Advances one tick of local prediction.
    pub fn step(&mut self) {
        if self.dead {
            return;
        }
        if let Some(target) = self.target {
            self.position = self.position.move_towards(target, self.speed_per_tick);
            if self.position == target {
                self.target = None;
            }
        }
    }

    /// Converts an authoritative quantised position into a simulation position.
    #[must_use]
    pub fn from_quantized(x: u16, y: u16) -> FixVec2 {
        crate::protocol::dequantize_position(x, y)
    }

    /// Quantises the current target for a `MoveTo` command.
    #[must_use]
    pub fn target_quantized(&self) -> Option<(u16, u16)> {
        self.target.map(crate::protocol::quantize_position)
    }

    /// Quantisation scale, re-exported for callers building commands.
    pub const SCALE: i32 = POSITION_SCALE;
}

fn clamp_position(position: FixVec2) -> FixVec2 {
    let min = Fix::from_i32(2);
    let max = Fix::from_i32(254);
    FixVec2::new(position.x.clamp(min, max), position.y.clamp(min, max))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(x: i32, y: i32) -> FixVec2 {
        FixVec2::from_ints(x, y)
    }

    #[test]
    fn moves_immediately_toward_the_target() {
        let mut prediction = PredictionState::new(Fix::from_i32(6));
        prediction.reconcile(position(10, 10));
        prediction.set_target(Some(position(20, 10)));
        prediction.step();
        assert!(prediction.position().x > Fix::from_i32(10));
        for _ in 0..60 {
            prediction.step();
        }
        assert_eq!(prediction.position(), position(20, 10));
        assert!(prediction.target().is_none());
    }

    #[test]
    fn small_lag_is_tolerated_while_moving() {
        let mut prediction = PredictionState::new(Fix::from_i32(7));
        prediction.reconcile(position(10, 10));
        prediction.set_target(Some(position(100, 10)));
        for _ in 0..30 {
            prediction.step();
        }
        let ahead = prediction.position();
        // Server is 4 units behind because it has not processed the order yet.
        prediction.reconcile(position(10, 10));
        assert!(prediction.position().distance(position(10, 10)) > Fix::from_i32(3));
        prediction.step();
        assert!(prediction.position().x >= ahead.x);
    }

    #[test]
    fn large_errors_snap_and_idle_errors_converge() {
        let mut prediction = PredictionState::new(Fix::from_i32(7));
        prediction.reconcile(position(10, 10));
        prediction.reconcile(position(40, 10));
        assert_eq!(prediction.position(), position(40, 10), "dash/respawn snap");

        prediction.set_target(None);
        prediction.reconcile(position(42, 10));
        assert!(prediction.position().x > Fix::from_i32(40));
        assert!(prediction.position().x < Fix::from_i32(42));
        for _ in 0..20 {
            prediction.reconcile(position(42, 10));
        }
        assert_eq!(prediction.position(), position(42, 10));
    }

    #[test]
    fn death_stops_movement_and_snaps_on_respawn() {
        let mut prediction = PredictionState::new(Fix::from_i32(7));
        prediction.reconcile(position(100, 100));
        prediction.set_target(Some(position(120, 100)));
        prediction.set_dead(true);
        prediction.step();
        assert_eq!(prediction.position(), position(100, 100));
        prediction.set_dead(false);
        prediction.reconcile(position(40, 40));
        assert_eq!(prediction.position(), position(40, 40));
    }
}
