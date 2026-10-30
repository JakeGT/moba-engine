//! Team readability outlines (R2b, ADR-0016): which outline slot each drawn unit gets and the
//! palette the slots resolve to.
//!
//! Slots (higher wins where outlines overlap on screen):
//!
//! | Slot | Unit | Colour | Width |
//! |---|---|---|---|
//! | 1 | allied hero | palette ally | 1.5 px |
//! | 2 | enemy hero | palette enemy | 2 px |
//! | 3 | the viewer's hero | palette own | 1.5 px |
//! | 4 | hovered ally (any kind) | ally, lightened | 2.5 px |
//! | 5 | hovered enemy (any kind) | enemy, lightened | 2.5 px |
//! | 6 | the current attack target | enemy, near white | 3 px |
//!
//! The hover is what a click would attack (`input::hovered_unit`: the nearest enemy within
//! 4 m of the cursor, the attack order's own pick), so today only slot 5 is set by the
//! network view; slot 4 stays in the palette for a future ally-targeted command.
//!
//! Colours come from the HUD's [`TeamPalette`], so the colour-blind palette applies to
//! outlines too. Outlines are only ever set on instances the client is drawing, built from
//! the units it was sent: a unit in fog or brush that the server did not send has no
//! instance, so it can never get an outline (the renderer's outline passes are depth-tested
//! as well, so an outline never shows through a wall).

use moba_core::math::Vec3;
use moba_render::{OutlinePalette, UnitInstance};

use crate::hud::theme::TeamPalette;

/// Allied hero.
pub(crate) const ALLY_HERO: u8 = 1;
/// Enemy hero.
pub(crate) const ENEMY_HERO: u8 = 2;
/// The viewer's own hero.
pub(crate) const OWN_HERO: u8 = 3;
/// A hovered allied (or own) unit.
pub(crate) const HOVER_ALLY: u8 = 4;
/// A hovered enemy unit.
pub(crate) const HOVER_ENEMY: u8 = 5;
/// The unit the viewer last ordered an attack on.
pub(crate) const TARGET: u8 = 6;

/// How the viewer relates to one unit, and whether the cursor or an attack order points at
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // Independent facts about one unit.
pub(crate) struct OutlineUnit {
    pub(crate) hero: bool,
    pub(crate) own: bool,
    pub(crate) enemy: bool,
    pub(crate) hovered: bool,
    pub(crate) target: bool,
}

/// The slot for `unit` (0 for none).
pub(crate) const fn slot(unit: OutlineUnit) -> u8 {
    if unit.target && unit.enemy {
        TARGET
    } else if unit.hovered {
        if unit.enemy { HOVER_ENEMY } else { HOVER_ALLY }
    } else if !unit.hero {
        0
    } else if unit.own {
        OWN_HERO
    } else if unit.enemy {
        ENEMY_HERO
    } else {
        ALLY_HERO
    }
}

/// Sets `slot` on every instance in `units` (the parts of one unit just pushed).
pub(crate) fn mark(units: &mut [UnitInstance], slot: u8) {
    if slot == 0 {
        return;
    }
    for unit in units {
        *unit = unit.with_outline(slot);
    }
}

/// Moves `color` a fraction `amount` of the way to white.
fn lighten(color: Vec3, amount: f32) -> Vec3 {
    color.lerp(Vec3::ONE, amount)
}

/// The outline palette for the HUD's team palette. Never allocates.
pub(crate) fn palette(team: TeamPalette) -> OutlinePalette {
    let own = team.own().truncate();
    let ally = team.ally().truncate();
    let enemy = team.enemy().truncate();
    OutlinePalette::NONE
        .with_slot(ALLY_HERO, ally, 1.5)
        .with_slot(ENEMY_HERO, enemy, 2.0)
        .with_slot(OWN_HERO, own, 1.5)
        .with_slot(HOVER_ALLY, lighten(ally, 0.35), 2.5)
        .with_slot(HOVER_ENEMY, lighten(enemy, 0.35), 2.5)
        .with_slot(TARGET, lighten(enemy, 0.7), 3.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HERO: OutlineUnit = OutlineUnit {
        hero: true,
        own: false,
        enemy: false,
        hovered: false,
        target: false,
    };

    #[test]
    fn slots_rank_target_over_hover_over_heroes() {
        assert_eq!(slot(HERO), ALLY_HERO);
        assert_eq!(slot(OutlineUnit { own: true, ..HERO }), OWN_HERO);
        assert_eq!(
            slot(OutlineUnit {
                enemy: true,
                ..HERO
            }),
            ENEMY_HERO
        );
        let minion = OutlineUnit {
            hero: false,
            ..HERO
        };
        assert_eq!(slot(minion), 0);
        assert_eq!(
            slot(OutlineUnit {
                hovered: true,
                ..minion
            }),
            HOVER_ALLY
        );
        let enemy_minion = OutlineUnit {
            enemy: true,
            ..minion
        };
        assert_eq!(
            slot(OutlineUnit {
                hovered: true,
                ..enemy_minion
            }),
            HOVER_ENEMY
        );
        assert_eq!(
            slot(OutlineUnit {
                hovered: true,
                target: true,
                ..enemy_minion
            }),
            TARGET
        );
        // An ally is never an attack target.
        assert_eq!(
            slot(OutlineUnit {
                target: true,
                ..HERO
            }),
            ALLY_HERO
        );
    }

    #[test]
    fn palettes_follow_the_team_palette_and_mark_sets_every_part() {
        for team in [TeamPalette::Standard, TeamPalette::ColorBlind] {
            let palette = palette(team);
            assert_eq!(palette.slots[0], moba_core::math::Vec4::ZERO);
            assert_eq!(
                palette.slots[usize::from(ENEMY_HERO)].truncate(),
                team.enemy().truncate()
            );
            assert_eq!(
                palette.slots[usize::from(ALLY_HERO)].truncate(),
                team.ally().truncate()
            );
            assert!(
                palette.slots[usize::from(TARGET)].w > palette.slots[usize::from(ENEMY_HERO)].w
            );
            assert_eq!(palette.slots[7], moba_core::math::Vec4::ZERO);
        }
        assert_ne!(
            palette(TeamPalette::Standard).slots[usize::from(ENEMY_HERO)],
            palette(TeamPalette::ColorBlind).slots[usize::from(ENEMY_HERO)]
        );
        let mut units = [UnitInstance::at(Vec3::ZERO, 1.0, moba_core::math::Vec4::ONE); 3];
        mark(&mut units[1..], ENEMY_HERO);
        assert_eq!(units.map(|unit| unit.outline), [0, ENEMY_HERO, ENEMY_HERO]);
    }
}
