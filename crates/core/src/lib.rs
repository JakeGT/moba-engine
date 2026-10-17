//! Foundation types shared by every crate in the engine.
//!
//! The guiding rule of this crate is **deterministic by construction**:
//!
//! - authoritative math is fixed-point ([`fix`]), never floating point;
//! - randomness is seeded and stream-split ([`rng`]), never OS-random in simulation;
//! - time is a tick counter ([`time`]), never a wall clock inside a tick;
//! - registries are dense and iteration order is explicit ([`arena`], [`bitset`]);
//! - checksums use a frozen hash ([`hash`]).
//!
//! Presentation code (renderer, UI, camera) uses [`math`], which re-exports `glam`. The
//! conversion helpers in [`fix`] and [`math`] are the only sanctioned boundary between the
//! two numeric worlds.

#![forbid(unsafe_code)]

pub mod arena;
pub mod bitset;
pub mod fix;
pub mod hash;
pub mod math;
pub mod rng;
pub mod time;

pub use arena::{Arena, ArenaKey};
pub use bitset::{BitSet, BitSet64};
pub use fix::{Fix, FixVec2};
pub use hash::{StableHasher, fnv1a64};
pub use rng::Rng;
pub use time::{FixedStep, NANOS_PER_TICK, TICKS_PER_SECOND, Tick};
