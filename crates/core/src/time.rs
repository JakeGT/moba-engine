//! Simulation time.
//!
//! The authoritative simulation advances in whole ticks. Nothing in a tick reads a wall
//! clock, so a replay of `(seed, inputs)` is bit-identical on any machine. The client's
//! main loop converts real elapsed time into whole ticks with [`FixedStep`]; the
//! accumulator is integer nanoseconds, avoiding the drift a float accumulator collects.

use core::fmt;

/// The authoritative simulation rate: 30 ticks per second.
pub const TICKS_PER_SECOND: u32 = 30;

/// Nanoseconds in one tick at [`TICKS_PER_SECOND`].
pub const NANOS_PER_TICK: u64 = 1_000_000_000 / TICKS_PER_SECOND as u64;

/// A simulation tick counter.
///
/// Comparison is wrap-safe for the 32-bit range: `Tick::is_newer_than` and elapsed
/// arithmetic use wrapping signed distance, so a long-running server (or a rollback that
/// rewinds the counter) behaves correctly.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Tick(u32);

impl Tick {
    /// Tick zero: the start of a match.
    pub const ZERO: Self = Self(0);

    /// Creates a tick from a raw counter.
    #[inline]
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the raw counter.
    #[inline]
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Advances by one tick.
    #[inline]
    pub fn advance(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }

    /// Wrapping addition.
    #[inline]
    #[must_use]
    pub const fn wrapping_add(self, ticks: u32) -> Self {
        Self(self.0.wrapping_add(ticks))
    }

    /// Wrapping subtraction.
    #[inline]
    #[must_use]
    pub const fn wrapping_sub(self, ticks: u32) -> Self {
        Self(self.0.wrapping_sub(ticks))
    }

    /// Signed distance from `reference`; negative means `self` is older.
    #[inline]
    #[must_use]
    #[allow(clippy::cast_possible_wrap)] // Reinterpreting the wrap distance as signed is the point.
    pub const fn signed_diff(self, reference: Self) -> i32 {
        self.0.wrapping_sub(reference.0) as i32
    }

    /// Returns `true` if this tick happened after `reference`, wrap-safe.
    #[inline]
    #[must_use]
    pub const fn is_newer_than(self, reference: Self) -> bool {
        self.signed_diff(reference) > 0
    }

    /// Returns `true` if this tick happened at or after `reference`, wrap-safe.
    #[inline]
    #[must_use]
    pub const fn is_at_or_after(self, reference: Self) -> bool {
        self.signed_diff(reference) >= 0
    }

    /// Elapsed ticks since `earlier`, saturating at zero if `earlier` is newer.
    #[inline]
    #[must_use]
    pub const fn elapsed_since(self, earlier: Self) -> u32 {
        let diff = self.signed_diff(earlier);
        if diff > 0 { diff as u32 } else { 0 }
    }

    /// Converts to seconds at the authoritative tick rate.
    #[inline]
    #[must_use]
    pub fn as_secs(self) -> f32 {
        self.0 as f32 / TICKS_PER_SECOND as f32
    }
}

impl fmt::Debug for Tick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Tick({})", self.0)
    }
}

impl fmt::Display for Tick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u32> for Tick {
    #[inline]
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<Tick> for u32 {
    #[inline]
    fn from(value: Tick) -> Self {
        value.0
    }
}

/// Converts real elapsed time into a whole number of fixed simulation ticks.
///
/// The accumulator is nanoseconds, so 30 Hz ticks stay aligned over hours. If the game
/// stalls (debugger, breakpoint, window drag), the catch-up is capped: running an
/// unbounded number of ticks to "catch up" turns one stall into a spiral of death.
#[derive(Clone, Debug)]
pub struct FixedStep {
    step_ns: u64,
    accumulator_ns: u128,
    max_catch_up: u32,
    tick: Tick,
}

impl FixedStep {
    /// Creates a clock running at `hz` ticks per second.
    ///
    /// # Panics
    /// Panics if `hz` is zero or larger than one billion.
    #[must_use]
    pub fn new(hz: u32) -> Self {
        assert!(
            hz > 0 && hz <= 1_000_000_000,
            "fixed step rate must be in 1..=1e9"
        );
        Self {
            step_ns: 1_000_000_000 / u64::from(hz),
            accumulator_ns: 0,
            max_catch_up: 5,
            tick: Tick::ZERO,
        }
    }

    /// Creates a clock at the authoritative 30 Hz rate.
    #[must_use]
    pub fn authoritative() -> Self {
        Self::new(TICKS_PER_SECOND)
    }

    /// Duration of one tick in nanoseconds.
    #[inline]
    #[must_use]
    pub const fn step_ns(&self) -> u64 {
        self.step_ns
    }

    /// Current tick count.
    #[inline]
    #[must_use]
    pub const fn tick(&self) -> Tick {
        self.tick
    }

    /// Sets the maximum number of ticks a single [`FixedStep::push`] may emit.
    pub fn set_max_catch_up(&mut self, max_ticks: u32) {
        assert!(max_ticks > 0, "must allow at least one tick per frame");
        self.max_catch_up = max_ticks;
    }

    /// Feeds elapsed nanoseconds and returns how many ticks should run.
    ///
    /// When more than `max_catch_up` ticks are due, the excess time is discarded and an
    /// error is not raised: the game slows down for one frame instead of freezing while
    /// it tries to simulate a backlog that no longer matters.
    pub fn push(&mut self, delta_ns: u64) -> u32 {
        self.accumulator_ns += u128::from(delta_ns);
        let step = u128::from(self.step_ns);
        let mut ticks = (self.accumulator_ns / step) as u32;
        if ticks > self.max_catch_up {
            ticks = self.max_catch_up;
            self.accumulator_ns = 0;
        } else {
            self.accumulator_ns %= step;
        }
        self.tick = self.tick.wrapping_add(ticks);
        ticks
    }

    /// Feed time in whole seconds. Convenience for tests and tools.
    pub fn push_secs(&mut self, seconds: f64) -> u32 {
        let nanos = (seconds.max(0.0) * 1e9) as u64;
        self.push(nanos)
    }

    /// Fraction of a tick that has elapsed since the last whole tick, in `[0, 1)`.
    #[must_use]
    pub fn alpha(&self) -> f32 {
        (self.accumulator_ns as f64 / self.step_ns as f64) as f32
    }

    /// Discards accumulated partial time and resets the clock to the given tick.
    pub fn reset(&mut self, tick: Tick) {
        self.accumulator_ns = 0;
        self.tick = tick;
    }
}

impl Default for FixedStep {
    fn default() -> Self {
        Self::authoritative()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_wraps_and_orders_correctly() {
        let before = Tick::new(u32::MAX - 1);
        let after = before.wrapping_add(3);
        assert_eq!(after, Tick::new(1));
        assert!(after.is_newer_than(before));
        assert!(!before.is_newer_than(after));
        assert_eq!(after.elapsed_since(before), 3);
        assert_eq!(before.elapsed_since(after), 0);
        assert!(after.is_at_or_after(after));
        assert_eq!(Tick::ZERO.wrapping_sub(1).get(), u32::MAX);
    }

    #[test]
    fn fixed_step_emits_the_expected_number_of_ticks() {
        let mut clock = FixedStep::authoritative();
        assert_eq!(clock.push(0), 0);
        assert_eq!(clock.push(NANOS_PER_TICK - 1), 0);
        assert_eq!(clock.push(1), 1);
        assert_eq!(clock.tick(), Tick::new(1));
        assert_eq!(clock.push(NANOS_PER_TICK), 1);
        assert_eq!(clock.tick(), Tick::new(2));
        for _ in 0..30 {
            clock.push(NANOS_PER_TICK);
        }
        assert_eq!(clock.tick(), Tick::new(32));
    }

    #[test]
    fn fixed_step_caps_catch_up() {
        let mut clock = FixedStep::authoritative();
        clock.set_max_catch_up(5);
        // Ten seconds of stall: only five ticks run, no backlog is carried.
        assert_eq!(clock.push(10 * 1_000_000_000), 5);
        assert_eq!(clock.tick(), Tick::new(5));
        assert_eq!(clock.push(0), 0);
    }

    #[test]
    fn alpha_tracks_the_partial_tick() {
        let mut clock = FixedStep::authoritative();
        clock.push(NANOS_PER_TICK / 2);
        assert!((clock.alpha() - 0.5).abs() < 1e-6);
        clock.push(NANOS_PER_TICK - NANOS_PER_TICK / 2);
        assert!(clock.alpha() < 1e-6);
        assert_eq!(clock.tick(), Tick::new(1));
    }

    #[test]
    fn reset_clears_the_accumulator() {
        let mut clock = FixedStep::authoritative();
        clock.push(NANOS_PER_TICK / 2);
        clock.reset(Tick::new(100));
        assert_eq!(clock.tick(), Tick::new(100));
        assert_eq!(clock.alpha(), 0.0);
    }
}
