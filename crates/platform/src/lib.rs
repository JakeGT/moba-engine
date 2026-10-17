//! Windowing, input, clocks and the fixed-step application loop.
//!
//! This is the only crate that links `winit`. Simulation crates must never depend on it;
//! the client and renderer consume the abstract [`Window`], [`InputState`] and [`Frame`]
//! types defined here.
//!
//! The loop contract:
//! - [`run`] creates the window and drives the app;
//! - every [`Frame`] says exactly how many fixed simulation ticks are due (`frame.ticks()`)
//!   and how far into the next tick the renderer is (`frame.alpha()`);
//! - wall-clock time appears only in this crate. The simulation is tick-driven.

#![forbid(unsafe_code)]

mod app;
mod input;
mod window;

pub use app::{App, Frame, FrameOutcome, run};
pub use input::{InputState, Key, MouseButton};
pub use window::{PlatformError, Window, WindowConfig};
