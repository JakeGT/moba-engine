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
