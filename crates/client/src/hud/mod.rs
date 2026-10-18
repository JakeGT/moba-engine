//! HUD/layout drawing shared by network and local-preview modes.

mod geometry;
mod healthbars;
mod minimap;
mod model;
mod status;
pub(crate) mod theme;

pub(crate) use geometry::*;
pub(crate) use healthbars::*;
pub(crate) use minimap::*;
pub(crate) use model::*;
pub(crate) use status::*;

use std::time::Instant;

use moba_core::math::{Vec2, Vec3, Vec4};
use moba_core::{Fix, FixVec2};
use moba_game::{
    EMPTY_ITEM, LEVEL_CAP, MELEE_MINION, NEXUS, RANGED_MINION, TOWER, Team, xp_to_next,
};
use moba_net::protocol::{MatchPhase, dequantize_position};
use moba_platform::{InputState, Key};
use moba_render::{Camera, UiDrawList, VisionCircles};
use moba_sim::Simulation;

use crate::app::NetworkView;
