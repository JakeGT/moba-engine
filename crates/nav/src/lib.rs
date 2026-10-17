//! Spatial queries and lane navigation primitives.
//!
//! The simulation rebuilds a fixed-cell spatial grid every tick with no allocation after
//! warm-up, then asks it for nearby candidates. Lane paths live in `moba-game`; this crate
//! provides the reusable spatial index and distance helpers.

#![forbid(unsafe_code)]

pub mod grid;

pub use grid::SpatialGrid;
