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

/// Damage "ghost": the trailing segment of health just lost.
pub(crate) const HEALTH_GHOST: Vec4 = Vec4::new(0.98, 0.86, 0.62, 0.95);
/// Shield segment after the health fill.
pub(crate) const HEALTH_SHIELD: Vec4 = Vec4::new(0.88, 0.94, 1.0, 1.0);
/// Health-bar frame.
pub(crate) const HEALTHBAR_FRAME: Vec4 = Vec4::new(0.0, 0.0, 0.0, 0.88);
/// Health-bar tick marks.
pub(crate) const HEALTHBAR_TICK: Vec4 = Vec4::new(0.0, 0.0, 0.0, 0.55);
/// Hero mana strip under the health bar.
pub(crate) const HEALTHBAR_MANA: Vec4 = Vec4::new(0.32, 0.58, 1.0, 1.0);
/// Ability slot tint while the hero cannot afford the cast.
pub(crate) const NO_MANA: Vec4 = Vec4::new(0.06, 0.16, 0.55, 0.55);
/// Cooldown sweep leading edge.
pub(crate) const COOLDOWN_EDGE: Vec4 = Vec4::new(1.0, 0.92, 0.7, 0.9);
/// Combat text: damage.
pub(crate) const DAMAGE_TEXT: Vec4 = Vec4::new(1.0, 0.40, 0.30, 1.0);
/// Combat text: large hits (crit emphasis).
pub(crate) const CRIT_TEXT: Vec4 = Vec4::new(1.0, 0.80, 0.20, 1.0);
/// Combat text: healing.
pub(crate) const HEAL_TEXT: Vec4 = Vec4::new(0.45, 1.0, 0.52, 1.0);
/// Combat text: gold earned.
pub(crate) const GOLD_TEXT: Vec4 = Vec4::new(1.0, 0.86, 0.30, 1.0);
/// Screen-transition dip colour.
pub(crate) const TRANSITION: Vec4 = Vec4::new(0.005, 0.01, 0.02, 1.0);
/// Loading-screen gradient, top.
pub(crate) const LOADING_TOP: Vec4 = Vec4::new(0.02, 0.04, 0.09, 1.0);
/// Loading-screen gradient, bottom.
pub(crate) const LOADING_BOTTOM: Vec4 = Vec4::new(0.09, 0.03, 0.10, 1.0);

/// Team colours for health bars and the minimap (the `team_palette` setting).
///
/// `Standard` is the genre's green self / blue ally / red enemy. `ColorBlind` uses Okabe-Ito
/// colours (yellow self, sky-blue ally, orange enemy), which stay distinct under protanopia,
/// deuteranopia and tritanopia. Values are linear (the UI is written to an sRGB swapchain);
/// the comments give the displayed sRGB colour.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum TeamPalette {
    #[default]
    Standard,
    ColorBlind,
}

impl TeamPalette {
    /// Parses a settings name: `standard` or `colorblind` (case, `-`, `_` and spaces ignored).
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        let mut key = arrayvec::ArrayString::<32>::new();
        for character in name.chars().filter(|c| !matches!(c, '-' | '_' | ' ')) {
            key.try_push(character.to_ascii_lowercase()).ok()?;
        }
        match key.as_str() {
            "standard" | "default" => Some(Self::Standard),
            "colorblind" | "colourblind" => Some(Self::ColorBlind),
            _ => None,
        }
    }

    /// The settings name.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::ColorBlind => "colorblind",
        }
    }

    /// The local player's own hero.
    pub(crate) const fn own(self) -> Vec4 {
        match self {
            Self::Standard => Vec4::new(0.100, 0.711, 0.084, 1.0), // sRGB #59DB52
            Self::ColorBlind => Vec4::new(0.871, 0.776, 0.054, 1.0), // #F0E442
        }
    }

    /// Allied units.
    pub(crate) const fn ally(self) -> Vec4 {
        match self {
            Self::Standard => Vec4::new(0.059, 0.274, 1.0, 1.0), // sRGB #458FFF
            Self::ColorBlind => Vec4::new(0.093, 0.456, 0.815, 1.0), // #56B4E9
        }
    }

    /// Enemy units.
    pub(crate) const fn enemy(self) -> Vec4 {
        match self {
            Self::Standard => Vec4::new(0.828, 0.040, 0.030, 1.0), // sRGB #EB3830
            Self::ColorBlind => Vec4::new(0.791, 0.347, 0.0, 1.0), // #E69F00
        }
    }

    /// Colour of a unit on `team`, seen by a player on `own_team` (`own`: their hero).
    pub(crate) const fn relation(self, team: u8, own_team: u8, own: bool) -> Vec4 {
        if own {
            self.own()
        } else if team == own_team {
            self.ally()
        } else {
            self.enemy()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TeamPalette;

    #[test]
    fn palettes_parse_and_differ() {
        assert_eq!(
            TeamPalette::from_name("standard"),
            Some(TeamPalette::Standard)
        );
        assert_eq!(
            TeamPalette::from_name("Color-Blind"),
            Some(TeamPalette::ColorBlind)
        );
        assert_eq!(
            TeamPalette::from_name("colour_blind"),
            Some(TeamPalette::ColorBlind)
        );
        assert_eq!(TeamPalette::from_name("neon"), None);
        for palette in [TeamPalette::Standard, TeamPalette::ColorBlind] {
            assert_eq!(TeamPalette::from_name(palette.name()), Some(palette));
            assert_ne!(palette.ally(), palette.enemy());
            assert_eq!(palette.relation(1, 1, true), palette.own());
            assert_eq!(palette.relation(1, 1, false), palette.ally());
            assert_eq!(palette.relation(0, 1, false), palette.enemy());
        }
        // The colour-blind enemy has no strong red/green opposition with the ally colour:
        // they differ mainly along the blue-yellow axis.
        let ally = TeamPalette::ColorBlind.ally();
        let enemy = TeamPalette::ColorBlind.enemy();
        assert!((ally.z - enemy.z).abs() > 0.8 && (ally.x - enemy.x).abs() > 0.6);
    }
}
