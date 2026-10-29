//! MSDF HUD font: glyph metrics and single-line layout (R7a).
//!
//! The atlas (`assets/ui-font.rgba`) and the metrics table (`font_metrics.rs`) are baked
//! offline by `crates/tools/font-bake` from Open Sans Condensed `SemiBold` (SIL OFL 1.1).
//! Text is sized by the legacy HUD `scale`: capital letters are `7 * scale` pixels tall, as
//! with the old 5x7 bitmap font, so existing layouts keep their proportions.

use crate::font_metrics::{
    FONT_ATLAS_HEIGHT, FONT_ATLAS_WIDTH, FONT_CAP_HEIGHT, FONT_EM_PX, FONT_GLYPHS, FONT_RANGE_PX,
};

/// One baked glyph.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct FontGlyph {
    /// Horizontal advance in em.
    pub(crate) advance: f32,
    /// Quad bounds relative to the pen on the baseline, in em: left, bottom, right, top
    /// (y up).
    pub(crate) plane: [f32; 4],
    /// Atlas cell in pixels: x, y, width, height (zero size for blank glyphs).
    pub(crate) atlas: [u32; 4],
}

impl FontGlyph {
    /// A glyph entry; used by the generated table.
    pub(crate) const fn new(advance: f32, plane: [f32; 4], atlas: [u32; 4]) -> Self {
        Self {
            advance,
            plane,
            atlas,
        }
    }

    /// Whether the glyph draws anything (the space does not).
    pub(crate) const fn is_blank(&self) -> bool {
        self.atlas[2] == 0 || self.atlas[3] == 0
    }

    /// Atlas UV rectangle `(min, max)`.
    pub(crate) fn uv(&self) -> ([f32; 2], [f32; 2]) {
        let [x, y, width, height] = self.atlas;
        let w = FONT_ATLAS_WIDTH as f32;
        let h = FONT_ATLAS_HEIGHT as f32;
        (
            [x as f32 / w, y as f32 / h],
            [(x + width) as f32 / w, (y + height) as f32 / h],
        )
    }
}

/// Extra spacing between glyphs, in em (HUD text is mostly capitals).
pub(crate) const TRACKING: f32 = 0.02;

/// Font size (pixels per em) for a legacy HUD text `scale`.
#[must_use]
pub(crate) fn em_px(scale: f32) -> f32 {
    7.0 * scale / FONT_CAP_HEIGHT
}

/// The glyph drawn for `character`: printable ASCII, common typographic punctuation folded to
/// its ASCII look-alike, and `?` for everything else.
#[must_use]
pub(crate) fn glyph(character: char) -> &'static FontGlyph {
    let character = match character {
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{2018}' | '\u{2019}' | '\u{2032}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{2033}' => '"',
        '\u{00D7}' => 'x',
        '\u{00A0}' | '\u{2009}' | '\u{202F}' => ' ',
        other => other,
    };
    let code = u32::from(character);
    let index = if (0x20..=0x7e).contains(&code) {
        code - 0x20
    } else {
        u32::from('?') - 0x20
    };
    &FONT_GLYPHS[index as usize]
}

/// Width of one line of `text` at `scale`, in pixels (advances plus tracking, without the
/// trailing tracking).
#[must_use]
pub(crate) fn text_width(text: &str, scale: f32) -> f32 {
    let mut ems = 0.0;
    let mut count = 0u32;
    for character in text.chars() {
        ems += glyph(character).advance + TRACKING;
        count += 1;
    }
    if count == 0 {
        0.0
    } else {
        (ems - TRACKING) * em_px(scale)
    }
}

/// Converts a width in em to the shader's normalised distance units (0.5 = half the baked
/// range), clamped to what the atlas can represent.
#[must_use]
pub(crate) fn em_to_distance(width_em: f32) -> f32 {
    (width_em * FONT_EM_PX / FONT_RANGE_PX).clamp(0.0, 0.45)
}

/// Font atlas dimensions.
pub(crate) const ATLAS_SIZE: [u32; 2] = [FONT_ATLAS_WIDTH, FONT_ATLAS_HEIGHT];

/// Baked distance range in atlas pixels (the shader's `font_range_px`).
pub(crate) const RANGE_PX: f32 = FONT_RANGE_PX;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baked_atlas_matches_metrics() {
        let atlas = include_bytes!("../assets/ui-font.rgba");
        assert_eq!(
            atlas.len(),
            (FONT_ATLAS_WIDTH * FONT_ATLAS_HEIGHT * 4) as usize
        );
        for (index, glyph) in FONT_GLYPHS.iter().enumerate() {
            let [x, y, width, height] = glyph.atlas;
            assert!(
                x + width <= FONT_ATLAS_WIDTH,
                "glyph {index} past the atlas"
            );
            assert!(
                y + height <= FONT_ATLAS_HEIGHT,
                "glyph {index} past the atlas"
            );
            assert!(glyph.advance >= 0.0);
            if !glyph.is_blank() {
                assert!(glyph.plane[0] < glyph.plane[2] && glyph.plane[1] < glyph.plane[3]);
                // The cell centre of a visible glyph is inside the outline for most glyphs;
                // at least the cell holds both inside (> 0.5) and outside (< 0.5) texels.
                let mut inside = false;
                let mut outside = false;
                for row in y..y + height {
                    for column in x..x + width {
                        let alpha = atlas[((row * FONT_ATLAS_WIDTH + column) * 4 + 3) as usize];
                        inside |= alpha > 140;
                        outside |= alpha < 100;
                    }
                }
                assert!(inside && outside, "glyph {index} has no edge");
            }
        }
        assert!(glyph(' ').is_blank());
        assert!(!glyph('A').is_blank());
    }

    #[test]
    fn capitals_are_seven_scale_pixels_tall() {
        let a = glyph('H');
        // The plane includes the distance margin; the outline itself is the cap height.
        let margin = (FONT_RANGE_PX * 0.5 + 1.0) / FONT_EM_PX;
        let height = (a.plane[3] - a.plane[1] - 2.0 * margin) * em_px(2.0);
        assert!((height - 14.0).abs() < 0.8, "cap height {height}");
    }

    #[test]
    fn width_sums_advances_and_maps_unknown_characters() {
        assert_eq!(text_width("", 2.0), 0.0);
        let one = text_width("A", 2.0);
        let two = text_width("AA", 2.0);
        assert!((two - (2.0 * one + TRACKING * em_px(2.0))).abs() < 1e-3);
        assert_eq!(glyph('é'), glyph('?'));
        assert_eq!(glyph('\u{2014}'), glyph('-'), "the HUD's em dashes");
        assert_eq!(glyph('\u{2019}'), glyph('\''));
        assert!(text_width("W", 1.0) > text_width("i", 1.0));
    }
}
