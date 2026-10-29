//! HUD/layout drawing shared by network and local-preview modes.

mod abilities;
pub(crate) mod fx;
mod geometry;
mod healthbars;
mod hit;
pub(crate) mod images;
mod minimap;
mod model;
mod scoreboard;
mod screens;
mod shop;
mod status;
pub(crate) mod theme;

pub(crate) use fx::HudFx;
pub(crate) use geometry::*;
pub(crate) use healthbars::*;
pub(crate) use hit::{HitList, HudAction, minimap_world, rect_at, register};
pub(crate) use minimap::*;
pub(crate) use model::*;
pub(crate) use status::*;

use std::fmt::{self, Write as _};

use arrayvec::ArrayString;

/// Stack-formatted HUD text; no heap allocation.
pub(crate) type HudString = ArrayString<256>;

/// Formats HUD text into a fixed stack buffer, truncating past 256 bytes.
pub(crate) fn format_hud(args: fmt::Arguments<'_>) -> HudString {
    let mut buffer = HudString::new();
    let _ = buffer.write_fmt(args);
    buffer
}

/// Static HUD text in the same stack type.
pub(crate) fn static_hud(value: &str) -> HudString {
    HudString::from(value).unwrap_or_default()
}

/// Formats into the fixed 256-byte HUD stack buffer.
#[macro_export]
macro_rules! hud_fmt {
    ($($arg:tt)*) => {{
        $crate::hud::format_hud(format_args!($($arg)*))
    }};
}

/// Wraps a static string in the fixed HUD stack buffer.
#[macro_export]
macro_rules! hud_static {
    ($value:literal) => {{ $crate::hud::static_hud($value) }};
}

use moba_core::math::{Vec2, Vec3, Vec4};
use moba_core::{Fix, FixVec2};
use moba_game::Team;
use moba_platform::{InputState, Key};
use moba_render::{Camera, UiDrawList, VisionCircles};
use moba_sim::Simulation;

use crate::app::NetworkView;
