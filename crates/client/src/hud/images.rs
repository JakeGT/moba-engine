//! HUD use of the renderer's offscreen images (R7b): hero-portrait cells and the baked
//! minimap.
//!
//! The images are rendered by `app::offscreen` and sampled here with
//! [`UiDrawList::image`]. The HUD only reads [`HudImages`], a small `Copy` summary of what
//! the images currently hold, so it never draws a cell or a map that was not rendered.

use moba_core::math::Vec4;
use moba_render::{UiDrawList, UiImage};

/// Portrait cells per team: a team's hero slots `0..8`.
pub(crate) const PORTRAIT_SLOTS_PER_TEAM: usize = 8;
/// Cells in the portrait strip (two teams).
pub(crate) const PORTRAIT_CELLS: usize = 2 * PORTRAIT_SLOTS_PER_TEAM;

/// What the offscreen images hold this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct HudImages {
    /// Bit `c` is set when portrait cell `c` holds its hero.
    pub(crate) portraits: u16,
    /// Cells across the portrait strip (`0` when portraits are off).
    pub(crate) portrait_cells: u8,
    /// The minimap image holds the current map.
    pub(crate) minimap: bool,
}

/// The portrait cell of a team's hero slot, or `None` past [`PORTRAIT_SLOTS_PER_TEAM`].
pub(crate) fn portrait_cell(team: u8, hero_slot: u8) -> Option<usize> {
    let slot = usize::from(hero_slot);
    (team < 2 && slot < PORTRAIT_SLOTS_PER_TEAM)
        .then(|| usize::from(team) * PORTRAIT_SLOTS_PER_TEAM + slot)
}

impl HudImages {
    /// The strip region of a hero's portrait, when its cell was rendered.
    pub(crate) fn portrait_uv(self, team: u8, hero_slot: u8) -> Option<[f32; 4]> {
        let cell = portrait_cell(team, hero_slot)?;
        let cells = usize::from(self.portrait_cells);
        if cell >= cells || self.portraits & (1 << cell) == 0 {
            return None;
        }
        let width = 1.0 / cells as f32;
        Some([cell as f32 * width, 0.0, (cell + 1) as f32 * width, 1.0])
    }
}

/// A team's portrait backdrop as a HUD colour, for the placeholder shown before a cell's
/// first render.
pub(crate) fn portrait_placeholder(team: u8) -> Vec4 {
    if team == 0 {
        Vec4::new(0.08, 0.14, 0.26, 1.0)
    } else {
        Vec4::new(0.26, 0.09, 0.07, 1.0)
    }
}

/// Draws a hero's portrait in a `size` square at `(x, y)` with a `frame` border; a dead hero
/// is dimmed and desaturated by the tint. Returns `false` (drawing nothing) when the cell
/// was not rendered, so the caller can fall back.
#[allow(clippy::too_many_arguments)] // Position, hero and look.
pub(crate) fn draw_portrait(
    ui: &mut UiDrawList,
    images: HudImages,
    team: u8,
    hero_slot: u8,
    [x, y, size]: [f32; 3],
    dead: bool,
    frame: Vec4,
) -> bool {
    let Some(uv) = images.portrait_uv(team, hero_slot) else {
        return false;
    };
    let tint = if dead {
        Vec4::new(0.35, 0.35, 0.38, 1.0)
    } else {
        Vec4::ONE
    };
    ui.image(x, y, size, size, UiImage::PORTRAITS, uv, tint);
    ui.outline(x, y, size, size, 1.0, frame);
    true
}

#[cfg(test)]
mod tests {
    use super::{HudImages, PORTRAIT_CELLS, draw_portrait, portrait_cell};
    use moba_core::math::Vec4;
    use moba_render::{UiDrawList, UiVertex};

    #[test]
    fn portrait_cells_split_the_strip_by_team_and_slot() {
        assert_eq!(portrait_cell(0, 0), Some(0));
        assert_eq!(portrait_cell(1, 4), Some(12));
        assert_eq!(portrait_cell(0, 8), None);
        assert_eq!(portrait_cell(2, 0), None);
        let images = HudImages {
            portraits: 1 << 12,
            portrait_cells: u8::try_from(PORTRAIT_CELLS).expect("fits"),
            minimap: false,
        };
        assert_eq!(
            images.portrait_uv(1, 4),
            Some([12.0 / 16.0, 0.0, 13.0 / 16.0, 1.0])
        );
        assert_eq!(images.portrait_uv(0, 4), None, "cell 4 was not rendered");
        let mut ui = UiDrawList::new();
        assert!(!draw_portrait(
            &mut ui,
            images,
            0,
            4,
            [0.0, 0.0, 32.0],
            false,
            Vec4::ONE
        ));
        assert!(ui.is_empty());
        assert!(draw_portrait(
            &mut ui,
            images,
            1,
            4,
            [0.0, 0.0, 32.0],
            true,
            Vec4::ONE
        ));
        assert_eq!(ui.vertices()[0].mode, UiVertex::MODE_IMAGE);
        // A narrower strip (a smaller configured atlas) has fewer cells.
        let narrow = HudImages {
            portrait_cells: 8,
            ..images
        };
        assert_eq!(narrow.portrait_uv(1, 4), None);
    }
}
