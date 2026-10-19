//! Immediate-mode screen-space UI: solid quads and a built-in 5x7 bitmap font.

use bytemuck::{Pod, Zeroable};
use moba_core::math::Vec4;

use crate::font_data::FONT_5X7;

/// One UI vertex in physical pixels (top-left origin).
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub struct UiVertex {
    /// `[x, y]` in pixels.
    pub position: [f32; 2],
    /// Texture coordinates in the atlas.
    pub uv: [f32; 2],
    /// Atlas index (0 = UI atlas; reserved for future bindless textures).
    pub atlas: u32,
    /// Linear RGBA colour.
    pub color: [f32; 4],
}

/// Atlas cell size in pixels.
pub(crate) const ATLAS_CELL: u32 = 8;
/// Atlas width in pixels (16 cells).
pub const ATLAS_WIDTH: u32 = 16 * ATLAS_CELL;
/// Atlas height in pixels (8 rows: 6 glyph rows + 2 icon rows).
pub const ATLAS_HEIGHT: u32 = 8 * ATLAS_CELL;
/// White texel used by solid geometry.
pub(crate) const WHITE_UV: [f32; 2] = [0.5 / ATLAS_WIDTH as f32, 0.5 / ATLAS_HEIGHT as f32];

/// Builds the default RGBA atlas: one white cell followed by the 5x7 font glyphs.
#[must_use]
pub fn build_atlas() -> Vec<u8> {
    let mut pixels = vec![0u8; (ATLAS_WIDTH * ATLAS_HEIGHT * 4) as usize];
    let mut put = |cell: u32, x: u32, y: u32| {
        let px = (cell % 16) * ATLAS_CELL + x;
        let py = (cell / 16) * ATLAS_CELL + y;
        let offset = ((py * ATLAS_WIDTH + px) * 4) as usize;
        pixels[offset..offset + 4].copy_from_slice(&[255, 255, 255, 255]);
    };
    // Cell 0 is a solid white texel.
    put(0, 0, 0);
    for character in 0x20u32..=0x7e {
        let cell = character - 0x20 + 1;
        let index = (character - 0x20) as usize * 5;
        let glyph = FONT_5X7.get(index..index + 5).unwrap_or(&[0; 5]);
        for (column, bits) in glyph.iter().enumerate() {
            for row in 0..7 {
                if bits & (1 << row) != 0 {
                    put(cell, 1 + column as u32, 1 + row as u32);
                }
            }
        }
    }
    for (icon, pattern) in ICON_PATTERNS.iter().enumerate() {
        let cell = 96 + icon as u32;
        for (row, line) in pattern.iter().enumerate() {
            for (column, byte) in line.bytes().enumerate() {
                if byte == b'#' {
                    put(cell, column as u32, row as u32);
                }
            }
        }
    }
    pixels
}

/// UV rectangle for one glyph cell.
#[must_use]
pub(crate) fn glyph_uv(character: char) -> ([f32; 2], [f32; 2]) {
    let index = (character as u32).wrapping_sub(0x20).min(94);
    let cell = index + 1;
    let x = (cell % 16) as f32 * ATLAS_CELL as f32;
    let y = (cell / 16) as f32 * ATLAS_CELL as f32;
    let u0 = (x + 1.0) / ATLAS_WIDTH as f32;
    let v0 = (y + 1.0) / ATLAS_HEIGHT as f32;
    let u1 = (x + 6.0) / ATLAS_WIDTH as f32;
    let v1 = (y + 8.0) / ATLAS_HEIGHT as f32;
    ([u0, v0], [u1, v1])
}

/// Number of 8x8 icon cells in the atlas.
pub(crate) const ICON_COUNT: u32 = 16;

const ICON_PATTERNS: [[&str; 8]; 16] = [
    [
        "........", "......#.", ".....##.", "....##..", "..###...", ".##.....", "#.......",
        "........",
    ],
    [
        "..####..", ".######.", "##....##", "##....##", "##....##", ".######.", "..####..",
        "........",
    ],
    [
        "...##...", "...##...", "..####..", ".######.", ".#.##.#.", ".######.", "..####..",
        "........",
    ],
    [
        "..##....", ".####...", "..##....", "..##....", "..##....", ".####...", "######..",
        "........",
    ],
    [
        "...##...", "..####..", ".######.", "########", ".######.", "..####..", "...##...",
        "........",
    ],
    [
        "..####..", ".##..##.", "##....##", "##....##", "##....##", ".##..##.", "..####..",
        "........",
    ],
    [
        "...##...", "..####..", ".######.", "##.##.##", ".######.", "..####..", "...##...",
        "........",
    ],
    [
        "########", "#......#", "#.####.#", "#.#..#.#", "#.####.#", "#......#", "########",
        "........",
    ],
    [
        "...#....", "..###...", ".#####..", "#######.", ".#####..", "..###...", "...#....",
        "........",
    ],
    [
        "#..#..#.", ".#.#.#..", "..#.#...", "#######.", "..#.#...", ".#.#.#..", "#..#..#.",
        "........",
    ],
    [
        "..###...", ".#...#..", "#.....#.", "#.....#.", "#.....#.", ".#...#..", "..###...",
        "........",
    ],
    [
        "...##...", "..####..", ".######.", "##.##.##", "##.##.##", ".######.", "..####..",
        "...##...",
    ],
    [
        "#......#", ".#....#.", "..#..#..", "...#.#..", "...#.#..", "..#..#..", ".#....#.",
        "#......#",
    ],
    [
        "..####..", ".#....#.", "#..##..#", "#.####.#", "#.####.#", "#..##..#", ".#....#.",
        "..####..",
    ],
    [
        "...#....", "..###...", ".#####..", "#######.", "#######.", ".#####..", "..###...",
        "...#....",
    ],
    [
        "........", ".######.", "#......#", "#.####.#", "#......#", ".######.", "........",
        "........",
    ],
];

/// UV rectangle for one icon cell.
#[must_use]
pub(crate) fn icon_uv(index: u32) -> ([f32; 2], [f32; 2]) {
    let index = index.min(ICON_COUNT - 1);
    let cell = 96 + index;
    let x = (cell % 16) as f32 * ATLAS_CELL as f32;
    let y = (cell / 16) as f32 * ATLAS_CELL as f32;
    let inset = 0.5 / ATLAS_CELL as f32;
    (
        [
            (x + inset) / ATLAS_WIDTH as f32,
            (y + inset) / ATLAS_HEIGHT as f32,
        ],
        [
            (x + ATLAS_CELL as f32 - inset) / ATLAS_WIDTH as f32,
            (y + ATLAS_CELL as f32 - inset) / ATLAS_HEIGHT as f32,
        ],
    )
}

/// CPU-built list of UI triangles for one frame.
#[derive(Clone, Debug, Default)]
pub struct UiDrawList {
    vertices: Vec<UiVertex>,
}

impl UiDrawList {
    /// Creates an empty list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Removes all geometry.
    pub fn clear(&mut self) {
        self.vertices.clear();
    }

    /// Number of vertices.
    #[must_use]
    pub fn len(&self) -> usize {
        self.vertices.len()
    }

    /// Whether the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    /// Raw vertices for the renderer.
    #[must_use]
    pub fn vertices(&self) -> &[UiVertex] {
        &self.vertices
    }

    /// Adds an axis-aligned rectangle.
    pub fn rect(&mut self, x: f32, y: f32, width: f32, height: f32, color: Vec4) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let color = color.to_array();
        let uv = WHITE_UV;
        let vertices = [
            UiVertex {
                position: [x, y],
                uv,
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x + width, y],
                uv,
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x + width, y + height],
                uv,
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x, y],
                uv,
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x + width, y + height],
                uv,
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x, y + height],
                uv,
                atlas: 0,
                color,
            },
        ];
        self.vertices.extend_from_slice(&vertices);
    }

    /// Adds a line segment as a rotated quad.
    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, thickness: f32, color: Vec4) {
        let dx = x2 - x1;
        let dy = y2 - y1;
        let length = (dx * dx + dy * dy).sqrt();
        if length <= f32::EPSILON || thickness <= 0.0 {
            return;
        }
        let nx = -dy / length * thickness * 0.5;
        let ny = dx / length * thickness * 0.5;
        let color = color.to_array();
        let uv = WHITE_UV;
        let vertices = [
            UiVertex {
                position: [x1 + nx, y1 + ny],
                uv,
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x2 + nx, y2 + ny],
                uv,
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x2 - nx, y2 - ny],
                uv,
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x1 + nx, y1 + ny],
                uv,
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x2 - nx, y2 - ny],
                uv,
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x1 - nx, y1 - ny],
                uv,
                atlas: 0,
                color,
            },
        ];
        self.vertices.extend_from_slice(&vertices);
    }

    /// Adds a rectangular outline.
    pub fn outline(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        thickness: f32,
        color: Vec4,
    ) {
        self.rect(x, y, width, thickness, color);
        self.rect(x, y + height - thickness, width, thickness, color);
        self.rect(x, y, thickness, height, color);
        self.rect(x + width - thickness, y, thickness, height, color);
    }

    /// Adds a horizontal bar with background, fill and border.
    #[allow(clippy::too_many_arguments)] // Geometry plus three explicit colours.
    pub fn bar(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        fraction: f32,
        fill: Vec4,
        background: Vec4,
        border: Vec4,
    ) {
        self.rect(x, y, width, height, background);
        self.rect(x, y, width * fraction.clamp(0.0, 1.0), height, fill);
        self.outline(x, y, width, height, 1.0, border);
    }

    /// Width in pixels of `text` at `scale`.
    #[must_use]
    pub fn text_width(text: &str, scale: f32) -> f32 {
        let count = text.chars().count();
        if count == 0 {
            0.0
        } else {
            count as f32 * 6.0 * scale - scale
        }
    }

    /// Adds one line of text with its top-left at `(x, y)`.
    pub fn text(&mut self, x: f32, y: f32, scale: f32, color: Vec4, text: &str) {
        let mut cursor = x;
        let color = color.to_array();
        for character in text.chars() {
            let ([u0, v0], [u1, v1]) = glyph_uv(character);
            let width = 5.0 * scale;
            let height = 7.0 * scale;
            let vertices = [
                UiVertex {
                    position: [cursor, y],
                    uv: [u0, v0],
                    atlas: 0,
                    color,
                },
                UiVertex {
                    position: [cursor + width, y],
                    uv: [u1, v0],
                    atlas: 0,
                    color,
                },
                UiVertex {
                    position: [cursor + width, y + height],
                    uv: [u1, v1],
                    atlas: 0,
                    color,
                },
                UiVertex {
                    position: [cursor, y],
                    uv: [u0, v0],
                    atlas: 0,
                    color,
                },
                UiVertex {
                    position: [cursor + width, y + height],
                    uv: [u1, v1],
                    atlas: 0,
                    color,
                },
                UiVertex {
                    position: [cursor, y + height],
                    uv: [u0, v1],
                    atlas: 0,
                    color,
                },
            ];
            self.vertices.extend_from_slice(&vertices);
            cursor += 6.0 * scale;
        }
    }

    /// Adds one atlas icon scaled to a square.
    pub fn icon(&mut self, x: f32, y: f32, size: f32, index: u32, color: Vec4) {
        if size <= 0.0 {
            return;
        }
        let ([u0, v0], [u1, v1]) = icon_uv(index);
        let color = color.to_array();
        let vertices = [
            UiVertex {
                position: [x, y],
                uv: [u0, v0],
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x + size, y],
                uv: [u1, v0],
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x + size, y + size],
                uv: [u1, v1],
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x, y],
                uv: [u0, v0],
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x + size, y + size],
                uv: [u1, v1],
                atlas: 0,
                color,
            },
            UiVertex {
                position: [x, y + size],
                uv: [u0, v1],
                atlas: 0,
                color,
            },
        ];
        self.vertices.extend_from_slice(&vertices);
    }

    /// Adds text with a one-pixel-scale drop shadow.
    pub fn text_shadowed(&mut self, x: f32, y: f32, scale: f32, color: Vec4, text: &str) {
        let shadow = Vec4::new(0.0, 0.0, 0.0, 0.8);
        self.text(x + scale, y + scale, scale, shadow, text);
        self.text(x, y, scale, color, text);
    }

    /// Adds a filled panel with a border.
    pub fn panel(&mut self, x: f32, y: f32, width: f32, height: f32, fill: Vec4, border: Vec4) {
        self.rect(x, y, width, height, fill);
        self.outline(x, y, width, height, 1.0, border);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn baked_atlas_matches_builder() {
        assert_eq!(
            include_bytes!("../assets/ui-atlas.rgba").as_slice(),
            super::build_atlas()
        );
    }

    use super::*;

    #[test]
    fn text_emits_one_quad_per_glyph() {
        let mut ui = UiDrawList::new();
        ui.text(0.0, 0.0, 2.0, Vec4::ONE, "ABC");
        assert_eq!(ui.len(), 18);
        assert_eq!(UiDrawList::text_width("ABC", 2.0), 34.0);
    }

    #[test]
    fn atlas_contains_white_texel_and_glyph_alpha() {
        let atlas = build_atlas();
        assert_eq!(atlas.len(), (ATLAS_WIDTH * ATLAS_HEIGHT * 4) as usize);
        assert_eq!(&atlas[0..4], &[255, 255, 255, 255]);
        assert!(atlas.chunks_exact(4).any(|pixel| pixel[3] == 255));
        let (u0, v0) = glyph_uv('A');
        assert!(u0[0] > 0.0 && u0[1] > 0.0);
        assert!(u0[0] < v0[0] && u0[1] < v0[1]);
    }

    #[test]
    fn icons_sample_a_distinct_atlas_cell() {
        let mut ui = UiDrawList::new();
        ui.icon(10.0, 20.0, 16.0, 3, Vec4::ONE);
        assert_eq!(ui.len(), 6);
        let (a, b) = icon_uv(3);
        let (c, _d) = icon_uv(4);
        assert_ne!(a, c);
        assert!(b[0] > a[0] && b[1] > a[1]);
    }

    #[test]
    fn empty_rect_is_ignored() {
        let mut ui = UiDrawList::new();
        ui.rect(0.0, 0.0, 0.0, 10.0, Vec4::ONE);
        assert!(ui.is_empty());
        ui.rect(0.0, 0.0, 10.0, 10.0, Vec4::ONE);
        assert_eq!(ui.len(), 6);
    }
}

#[cfg(test)]
mod line_tests {
    use super::*;

    #[test]
    fn lines_emit_quads_and_ignore_degenerate_input() {
        let mut ui = UiDrawList::new();
        ui.line(0.0, 0.0, 10.0, 0.0, 2.0, Vec4::ONE);
        assert_eq!(ui.len(), 6);
        ui.line(1.0, 1.0, 1.0, 1.0, 2.0, Vec4::ONE);
        assert_eq!(ui.len(), 6);
    }
}
