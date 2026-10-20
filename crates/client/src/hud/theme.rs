//! Shared HUD palette, spacing and text scales.

use moba_core::math::Vec4;

/// Primary gold accent.
pub(crate) const GOLD: Vec4 = Vec4::new(0.95, 0.78, 0.3, 1.0);
/// Enemy/red accent.
pub(crate) const RED_ACCENT: Vec4 = Vec4::new(1.0, 0.42, 0.32, 1.0);
/// Muted blue-grey text.
pub(crate) const MUTED: Vec4 = Vec4::new(0.55, 0.68, 0.78, 1.0);
/// Panel chrome.
pub(crate) const PANEL_CHROME: Vec4 = Vec4::new(0.22, 0.42, 0.58, 0.9);
/// Bright text.
pub(crate) const TEXT: Vec4 = Vec4::new(0.88, 0.94, 1.0, 1.0);
/// Allied blue.
pub(crate) const BLUE_ACCENT: Vec4 = Vec4::new(0.35, 0.62, 1.0, 1.0);
/// Dark translucent panel background.
pub(crate) const PANEL_BG: Vec4 = Vec4::new(0.015, 0.025, 0.045, 0.82);

/// Health bar fill.
pub(crate) const HEALTH: Vec4 = Vec4::new(0.18, 0.82, 0.36, 1.0);
/// Mana bar fill.
pub(crate) const MANA: Vec4 = Vec4::new(0.22, 0.50, 1.0, 1.0);
/// Empty bar track.
pub(crate) const TRACK: Vec4 = Vec4::new(0.055, 0.075, 0.095, 1.0);
/// XP bar fill.
pub(crate) const XP: Vec4 = Vec4::new(0.85, 0.68, 0.25, 1.0);
/// Translucent tooltip/panel fill.
pub(crate) const TOOLTIP_BG: Vec4 = Vec4::new(0.01, 0.02, 0.035, 0.96);
/// Full-screen dim overlay.
pub(crate) const OVERLAY: Vec4 = Vec4::new(0.0, 0.0, 0.0, 0.35);
/// Victory accent.
pub(crate) const VICTORY: Vec4 = Vec4::new(0.35, 1.0, 0.5, 1.0);
/// Defeat accent.
pub(crate) const DEFEAT: Vec4 = Vec4::new(1.0, 0.35, 0.28, 1.0);
/// Respawn warning.
pub(crate) const RESPAWN: Vec4 = Vec4::new(1.0, 0.35, 0.25, 1.0);

/// Dimmed status text.
pub(crate) const STATUS_DIM: Vec4 = Vec4::new(0.55, 0.62, 0.72, 1.0);
/// Health-bar background.
pub(crate) const HEALTHBAR_BG: Vec4 = Vec4::new(0.02, 0.03, 0.04, 0.85);
/// Ally health fill.
pub(crate) const HEALTHBAR_ALLY: Vec4 = Vec4::new(0.2, 0.85, 0.38, 1.0);
/// Shield status color.
pub(crate) const SHIELD: Vec4 = Vec4::new(0.3, 0.75, 1.0, 1.0);
/// Slow status color.
pub(crate) const SLOW: Vec4 = Vec4::new(0.6, 0.4, 0.95, 1.0);
/// Haste status color.
pub(crate) const HASTE: Vec4 = Vec4::new(0.95, 0.8, 0.25, 1.0);
/// Root status color.
pub(crate) const ROOT: Vec4 = Vec4::new(0.7, 0.7, 0.75, 1.0);
/// Burn status color.
pub(crate) const BURN: Vec4 = Vec4::new(1.0, 0.45, 0.12, 1.0);
/// Regeneration status color.
pub(crate) const REGEN: Vec4 = Vec4::new(0.3, 0.95, 0.4, 1.0);
/// Minimap panel background.
pub(crate) const MINIMAP_BG: Vec4 = Vec4::new(0.01, 0.02, 0.035, 0.82);
/// Minimap ground.
pub(crate) const MINIMAP_GROUND: Vec4 = Vec4::new(0.04, 0.08, 0.06, 0.9);
/// Minimap lane.
pub(crate) const MINIMAP_LANE: Vec4 = Vec4::new(0.22, 0.38, 0.28, 0.85);
/// Minimap wall.
pub(crate) const MINIMAP_WALL: Vec4 = Vec4::new(0.55, 0.42, 0.28, 1.0);
/// Scoreboard background.
pub(crate) const SCOREBOARD_BG: Vec4 = Vec4::new(0.01, 0.02, 0.035, 0.9);
/// Scoreboard blue row.
pub(crate) const SCOREBOARD_BLUE: Vec4 = Vec4::new(0.45, 0.7, 1.0, 1.0);
/// Scoreboard red row.
pub(crate) const SCOREBOARD_RED: Vec4 = Vec4::new(1.0, 0.5, 0.4, 1.0);
/// Cooldown dim overlay.
pub(crate) const COOLDOWN: Vec4 = Vec4::new(0.0, 0.0, 0.0, 0.62);
/// Hero-name parchment.
pub(crate) const HERO_NAME: Vec4 = Vec4::new(0.92, 0.84, 0.62, 1.0);
/// Cast-range ring.
pub(crate) const CAST_RING: Vec4 = Vec4::new(0.35, 0.78, 1.0, 0.55);
/// Target ring.
pub(crate) const TARGET_RING: Vec4 = Vec4::new(1.0, 0.78, 0.30, 0.50);
/// Blue team panel accent.
pub(crate) const TEAM_BLUE: Vec4 = Vec4::new(0.4, 0.65, 1.0, 1.0);
/// Shop tooltip background.
pub(crate) const SHOP_TOOLTIP: Vec4 = Vec4::new(0.01, 0.02, 0.035, 0.94);
/// Disabled shop row.
pub(crate) const SHOP_DISABLED: Vec4 = Vec4::new(0.45, 0.5, 0.58, 1.0);
/// Shop warning text.
pub(crate) const SHOP_WARN: Vec4 = Vec4::new(0.55, 0.4, 0.3, 1.0);
