//! Immediate-mode screen-space UI: solid quads, gradients, cooldown sweeps, MSDF text and
//! pixel-art icons.
//!
//! Text is drawn from a baked multi-channel signed-distance-field font (R7a,
//! [`crate::font`]), anti-aliased in the shader from screen-space derivatives, so it stays
//! crisp at any UI scale. The original RGBA atlas keeps the pixel-art icons and the legacy
//! 5x7 bitmap font ([`UiDrawList::text_bitmap`]).

use bytemuck::{Pod, Zeroable};
use moba_core::math::Vec4;

use crate::font;
use crate::font_data::FONT_5X7;

/// One UI vertex in logical pixels (top-left origin; the renderer applies the UI scale).
///
/// A GPU vertex layout (48 bytes), exhaustive by design. `mode` selects how the fragment
/// shader shades the triangle (`MODE_*`); `outline` and `params` are mode-specific.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub struct UiVertex {
    /// `[x, y]` in pixels.
    pub position: [f32; 2],
    /// Texture coordinates: the RGBA atlas, the font atlas or the quad's unit square,
    /// depending on `mode`.
    pub uv: [f32; 2],
    /// Shading mode, one of the `UiVertex::MODE_*` constants.
    pub mode: u32,
    /// Linear RGBA colour (fill).
    pub color: [f32; 4],
    /// Secondary colour as packed RGBA8 (red in the low byte): text outline or sweep edge.
    pub outline: u32,
    /// Mode parameters: text outline width / shadow dilation and softness, or the sweep
    /// fraction and ring radius.
    pub params: [f32; 2],
}

impl UiVertex {
    /// Samples the RGBA atlas alpha (pixel-art icons and the bitmap font).
    pub const MODE_ATLAS: u32 = 0;
    /// Solid colour; no texture read.
    pub const MODE_SOLID: u32 = 1;
    /// MSDF text, with an outline when `params[0] > 0`.
    pub const MODE_TEXT: u32 = 2;
    /// Soft text shadow or glow from the true distance field (`params` = dilation, softness).
    pub const MODE_TEXT_SOFT: u32 = 3;
    /// Radial sweep over the quad (`params` = covered fraction, ring inner radius).
    pub const MODE_SWEEP: u32 = 4;
}

/// Packs a linear RGBA colour into the vertex `outline` field (red in the low byte).
#[must_use]
pub fn pack_rgba8(color: Vec4) -> u32 {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
    channel(color.x) | (channel(color.y) << 8) | (channel(color.z) << 16) | (channel(color.w) << 24)
}

/// How a line of MSDF text is drawn: fill colour plus optional outline and soft shadow.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// Fill colour.
    pub color: Vec4,
    /// Outline colour (alpha 0 disables the outline).
    pub outline: Vec4,
    /// Outline width in em (up to about 0.09 em; the baked range limits it).
    pub outline_width: f32,
    /// Shadow colour (alpha 0 disables the shadow).
    pub shadow: Vec4,
    /// Shadow offset in em (x right, y down).
    pub shadow_offset: [f32; 2],
    /// Shadow blur radius in em.
    pub shadow_softness: f32,
}

impl TextStyle {
    /// Plain filled text.
    #[must_use]
    pub const fn new(color: Vec4) -> Self {
        Self {
            color,
            outline: Vec4::ZERO,
            outline_width: 0.0,
            shadow: Vec4::ZERO,
            shadow_offset: [0.0; 2],
            shadow_softness: 0.0,
        }
    }

    /// Returns this style with an outline of `width` em.
    #[must_use]
    pub const fn with_outline(mut self, color: Vec4, width: f32) -> Self {
        self.outline = color;
        self.outline_width = width;
        self
    }

    /// Returns this style with a soft drop shadow (`offset` and `softness` in em).
    #[must_use]
    pub const fn with_shadow(mut self, color: Vec4, offset: [f32; 2], softness: f32) -> Self {
        self.shadow = color;
        self.shadow_offset = offset;
        self.shadow_softness = softness;
        self
    }
}

impl Default for TextStyle {
    fn default() -> Self {
        Self::new(Vec4::ONE)
    }
}

/// Shader parameters (dilation, softness) of a soft shadow, in normalised distance units.
///
/// The baked field ends at half its range (`0.5`), so the soft edge must reach zero alpha
/// inside it, or the glyph quads would show as boxes.
fn shadow_params(outline_width: f32, softness: f32) -> (f32, f32) {
    let dilation = font::em_to_distance(outline_width + softness * 0.5).min(0.3);
    let softness = font::em_to_distance(softness).clamp(0.02, 0.46 - dilation);
    (dilation, softness)
}

/// The HUD's default readable-over-anything shadow colour.
const SHADOW: Vec4 = Vec4::new(0.0, 0.0, 0.0, 0.75);

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
///
/// The list keeps its capacity across [`UiDrawList::clear`], so a HUD rebuilt every frame
/// stops allocating once warm.
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

    /// Removes all geometry (keeps the capacity).
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

    /// Two triangles for an axis-aligned quad with per-corner colours
    /// (`[top-left, top-right, bottom-right, bottom-left]`).
    #[allow(clippy::too_many_arguments)] // One quad is corners, UVs, mode and parameters.
    fn quad(
        &mut self,
        [x0, y0, x1, y1]: [f32; 4],
        [u0, v0]: [f32; 2],
        [u1, v1]: [f32; 2],
        mode: u32,
        colors: [Vec4; 4],
        outline: u32,
        params: [f32; 2],
    ) {
        let vertex = |x: f32, y: f32, u: f32, v: f32, color: Vec4| UiVertex {
            position: [x, y],
            uv: [u, v],
            mode,
            color: color.to_array(),
            outline,
            params,
        };
        let top_left = vertex(x0, y0, u0, v0, colors[0]);
        let top_right = vertex(x1, y0, u1, v0, colors[1]);
        let bottom_right = vertex(x1, y1, u1, v1, colors[2]);
        let bottom_left = vertex(x0, y1, u0, v1, colors[3]);
        self.vertices.extend_from_slice(&[
            top_left,
            top_right,
            bottom_right,
            top_left,
            bottom_right,
            bottom_left,
        ]);
    }

    /// Adds an axis-aligned rectangle.
    pub fn rect(&mut self, x: f32, y: f32, width: f32, height: f32, color: Vec4) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        self.quad(
            [x, y, x + width, y + height],
            WHITE_UV,
            WHITE_UV,
            UiVertex::MODE_SOLID,
            [color; 4],
            0,
            [0.0; 2],
        );
    }

    /// Adds a rectangle with a vertical gradient from `top` to `bottom`.
    pub fn rect_gradient(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        top: Vec4,
        bottom: Vec4,
    ) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        self.quad(
            [x, y, x + width, y + height],
            WHITE_UV,
            WHITE_UV,
            UiVertex::MODE_SOLID,
            [top, top, bottom, bottom],
            0,
            [0.0; 2],
        );
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
        let vertex = |x: f32, y: f32| UiVertex {
            position: [x, y],
            uv: WHITE_UV,
            mode: UiVertex::MODE_SOLID,
            color: color.to_array(),
            outline: 0,
            params: [0.0; 2],
        };
        let a = vertex(x1 + nx, y1 + ny);
        let b = vertex(x2 + nx, y2 + ny);
        let c = vertex(x2 - nx, y2 - ny);
        let d = vertex(x1 - nx, y1 - ny);
        self.vertices.extend_from_slice(&[a, b, c, a, c, d]);
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

    /// Adds a clockwise radial sweep over a square: the part still `fraction` (0..1) away
    /// from 12 o'clock is covered in `color`, with a leading edge in `edge` (alpha 0 for
    /// none). Used for cooldowns.
    pub fn sweep(&mut self, x: f32, y: f32, size: f32, fraction: f32, color: Vec4, edge: Vec4) {
        if size <= 0.0 || fraction <= 0.0 {
            return;
        }
        self.quad(
            [x, y, x + size, y + size],
            [0.0, 0.0],
            [1.0, 1.0],
            UiVertex::MODE_SWEEP,
            [color; 4],
            pack_rgba8(edge),
            [fraction.min(1.0), 0.0],
        );
    }

    /// Adds an anti-aliased ring arc centred on `(cx, cy)`: `fraction` of the circle,
    /// ending at 12 o'clock, `thickness` pixels wide.
    pub fn ring(
        &mut self,
        cx: f32,
        cy: f32,
        radius: f32,
        thickness: f32,
        fraction: f32,
        color: Vec4,
    ) {
        if radius <= 0.0 || thickness <= 0.0 || fraction <= 0.0 {
            return;
        }
        let inner = ((radius - thickness) / radius).clamp(0.001, 0.999);
        self.quad(
            [cx - radius, cy - radius, cx + radius, cy + radius],
            [0.0, 0.0],
            [1.0, 1.0],
            UiVertex::MODE_SWEEP,
            [color; 4],
            0,
            [fraction.min(1.0), inner],
        );
    }

    /// Width in pixels of `text` at `scale` (MSDF font).
    #[must_use]
    pub fn text_width(text: &str, scale: f32) -> f32 {
        font::text_width(text, scale)
    }

    /// Width in pixels of `text` at `scale` in the legacy 5x7 bitmap font.
    #[must_use]
    pub fn bitmap_text_width(text: &str, scale: f32) -> f32 {
        let count = text.chars().count();
        if count == 0 {
            0.0
        } else {
            count as f32 * 6.0 * scale - scale
        }
    }

    /// Adds one line of MSDF text with the capital-letter top-left at `(x, y)`.
    ///
    /// `scale` keeps the bitmap-font convention: capitals are `7 * scale` pixels tall.
    pub fn text(&mut self, x: f32, y: f32, scale: f32, color: Vec4, text: &str) {
        self.glyph_run(x, y, scale, text, UiVertex::MODE_TEXT, color, 0, [0.0; 2]);
    }

    /// Adds one line of text in `style`: shadow first, then the outlined fill.
    pub fn text_styled(&mut self, x: f32, y: f32, scale: f32, style: &TextStyle, text: &str) {
        let em = font::em_px(scale);
        if style.shadow.w > 0.0 {
            // A wider dilation keeps the shadow visible under an outline.
            let (dilation, softness) = shadow_params(style.outline_width, style.shadow_softness);
            self.glyph_run(
                x + style.shadow_offset[0] * em,
                y + style.shadow_offset[1] * em,
                scale,
                text,
                UiVertex::MODE_TEXT_SOFT,
                style.shadow,
                0,
                [dilation, softness],
            );
        }
        if style.color.w <= 0.0 && style.outline.w <= 0.0 {
            // A shadow-only style (a glow).
            return;
        }
        let outline = if style.outline.w > 0.0 {
            font::em_to_distance(style.outline_width)
        } else {
            0.0
        };
        self.glyph_run(
            x,
            y,
            scale,
            text,
            UiVertex::MODE_TEXT,
            style.color,
            pack_rgba8(style.outline),
            [outline, 0.0],
        );
    }

    /// Adds text with a soft drop shadow, readable over busy scenes.
    pub fn text_shadowed(&mut self, x: f32, y: f32, scale: f32, color: Vec4, text: &str) {
        let style = TextStyle::new(color).with_shadow(SHADOW, [0.05, 0.07], 0.06);
        self.text_styled(x, y, scale, &style, text);
    }

    /// Adds text with a dark outline (combat numbers, labels over the 3D scene).
    pub fn text_outlined(&mut self, x: f32, y: f32, scale: f32, color: Vec4, text: &str) {
        let style =
            TextStyle::new(color).with_outline(Vec4::new(0.0, 0.0, 0.0, color.w * 0.9), 0.06);
        self.text_styled(x, y, scale, &style, text);
    }

    #[allow(clippy::too_many_arguments)] // One run is position, text and its shading.
    fn glyph_run(
        &mut self,
        x: f32,
        y: f32,
        scale: f32,
        text: &str,
        mode: u32,
        color: Vec4,
        outline: u32,
        params: [f32; 2],
    ) {
        let em = font::em_px(scale);
        if em <= 0.0 {
            return;
        }
        let baseline = y + 7.0 * scale;
        let mut pen = x;
        for character in text.chars() {
            let glyph = font::glyph(character);
            if !glyph.is_blank() {
                let [left, bottom, right, top] = glyph.plane;
                let (uv0, uv1) = glyph.uv();
                self.quad(
                    [
                        pen + left * em,
                        baseline - top * em,
                        pen + right * em,
                        baseline - bottom * em,
                    ],
                    uv0,
                    uv1,
                    mode,
                    [color; 4],
                    outline,
                    params,
                );
            }
            pen += (glyph.advance + font::TRACKING) * em;
        }
    }

    /// Adds one line in the legacy 5x7 bitmap font (pixel-art labels), top-left at `(x, y)`.
    pub fn text_bitmap(&mut self, x: f32, y: f32, scale: f32, color: Vec4, text: &str) {
        let mut cursor = x;
        for character in text.chars() {
            let (uv0, uv1) = glyph_uv(character);
            self.quad(
                [cursor, y, cursor + 5.0 * scale, y + 7.0 * scale],
                uv0,
                uv1,
                UiVertex::MODE_ATLAS,
                [color; 4],
                0,
                [0.0; 2],
            );
            cursor += 6.0 * scale;
        }
    }

    /// Adds one atlas icon scaled to a square.
    pub fn icon(&mut self, x: f32, y: f32, size: f32, index: u32, color: Vec4) {
        if size <= 0.0 {
            return;
        }
        let (uv0, uv1) = icon_uv(index);
        self.quad(
            [x, y, x + size, y + size],
            uv0,
            uv1,
            UiVertex::MODE_ATLAS,
            [color; 4],
            0,
            [0.0; 2],
        );
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
    fn vertex_layout_matches_the_pipeline() {
        assert_eq!(core::mem::size_of::<UiVertex>(), 48);
        assert_eq!(core::mem::offset_of!(UiVertex, mode), 16);
        assert_eq!(core::mem::offset_of!(UiVertex, color), 20);
        assert_eq!(core::mem::offset_of!(UiVertex, outline), 36);
        assert_eq!(core::mem::offset_of!(UiVertex, params), 40);
    }

    #[test]
    fn text_emits_one_quad_per_visible_glyph() {
        let mut ui = UiDrawList::new();
        ui.text(0.0, 0.0, 2.0, Vec4::ONE, "AB C");
        assert_eq!(ui.len(), 18, "the space has no quad");
        assert!(ui.vertices().iter().all(|v| v.mode == UiVertex::MODE_TEXT));
        let width = UiDrawList::text_width("AB C", 2.0);
        assert!(width > 20.0 && width < 60.0, "{width}");
        // Capitals sit between y and y + 7 * scale, with the distance margin around them.
        let top = ui
            .vertices()
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MAX, f32::min);
        assert!(top < 0.0 && top > -4.0, "{top}");
    }

    #[test]
    fn styled_text_draws_shadow_then_fill() {
        let mut ui = UiDrawList::new();
        let style = TextStyle::new(Vec4::ONE)
            .with_outline(Vec4::new(0.0, 0.0, 0.0, 1.0), 0.05)
            .with_shadow(Vec4::new(0.0, 0.0, 0.0, 0.5), [0.05, 0.05], 0.1);
        ui.text_styled(10.0, 10.0, 2.0, &style, "HI");
        assert_eq!(ui.len(), 24);
        assert_eq!(ui.vertices()[0].mode, UiVertex::MODE_TEXT_SOFT);
        assert_eq!(ui.vertices()[12].mode, UiVertex::MODE_TEXT);
        assert!(ui.vertices()[12].params[0] > 0.0);
        assert_eq!(
            ui.vertices()[12].outline,
            pack_rgba8(Vec4::new(0.0, 0.0, 0.0, 1.0))
        );
    }

    /// `ui.frag`'s outline coverage (`MODE_TEXT`), mirrored for testing: `median` and
    /// `alpha` are the sampled field, `width` the outline in normalised distance units and
    /// `screen_px_range` the on-screen distance range (clamped to at least 1 in the shader).
    fn outline_coverage(median: f32, alpha: f32, width: f32, screen_px_range: f32) -> f32 {
        let smoothstep = |edge0: f32, edge1: f32, x: f32| {
            let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let distance = (median - 0.5).max(alpha - 0.5) + width;
        (distance * screen_px_range + 0.5).clamp(0.0, 1.0) * smoothstep(0.0, 0.06, alpha)
    }

    #[test]
    fn outlines_fade_out_inside_the_baked_range_at_any_size() {
        // The widest outline the API allows, and the HUD's widths.
        let widths = [0.05, 0.06, 0.07, 1.0].map(font::em_to_distance);
        // From the shader's minimum (tiny text, e.g. UI scale 0.5) to large text.
        for screen_px_range in [1.0, 1.25, 2.0, 4.0, 16.0] {
            for width in widths {
                // The empty margin of a glyph cell: no box.
                assert_eq!(outline_coverage(0.0, 0.0, width, screen_px_range), 0.0);
                // Inside the glyph the outline is solid.
                assert_eq!(outline_coverage(0.95, 0.95, width, screen_px_range), 1.0);
            }
        }
        // Without the edge fade, tiny text would cover the margin: the bug this guards.
        let width = font::em_to_distance(0.07);
        assert!(((-0.5 + width) * 1.0 + 0.5_f32).clamp(0.0, 1.0) > 0.0);
    }

    #[test]
    fn soft_shadows_fade_out_inside_the_baked_range() {
        for (outline, softness) in [(0.0, 0.0), (0.05, 0.05), (0.0, 0.09), (0.09, 0.5)] {
            let (dilation, soft) = shadow_params(outline, softness);
            // At the far edge of a glyph cell the field reads 0 (distance -0.5).
            assert!(
                -0.5 + dilation < -soft,
                "{outline} {softness}: box edges would show"
            );
            assert!(soft >= 0.02);
        }
    }

    #[test]
    fn bitmap_text_keeps_the_legacy_metrics() {
        let mut ui = UiDrawList::new();
        ui.text_bitmap(0.0, 0.0, 2.0, Vec4::ONE, "ABC");
        assert_eq!(ui.len(), 18);
        assert_eq!(UiDrawList::bitmap_text_width("ABC", 2.0), 34.0);
    }

    #[test]
    fn sweeps_and_rings_carry_their_fraction() {
        let mut ui = UiDrawList::new();
        ui.sweep(0.0, 0.0, 40.0, 0.25, Vec4::ONE, Vec4::ZERO);
        ui.sweep(0.0, 0.0, 40.0, 0.0, Vec4::ONE, Vec4::ZERO);
        ui.ring(50.0, 50.0, 20.0, 4.0, 2.0, Vec4::ONE);
        assert_eq!(ui.len(), 12);
        assert_eq!(ui.vertices()[0].params, [0.25, 0.0]);
        assert_eq!(ui.vertices()[6].params[0], 1.0);
        assert!((ui.vertices()[6].params[1] - 0.8).abs() < 1e-6);
        assert_eq!(pack_rgba8(Vec4::new(1.0, 0.0, 0.0, 1.0)), 0xFF00_00FF);
    }

    #[test]
    fn atlas_contains_white_texel_and_glyph_alpha() {
        let atlas = build_atlas();
        assert_eq!(atlas.len(), (ATLAS_WIDTH * ATLAS_HEIGHT * 4) as usize);
        assert_eq!(&atlas[0..4], &[255, 255, 255, 255]);
        assert!(atlas.as_chunks::<4>().0.iter().any(|pixel| pixel[3] == 255));
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
        ui.rect_gradient(0.0, 0.0, 10.0, 10.0, Vec4::ONE, Vec4::ZERO);
        assert_eq!(ui.len(), 12);
        assert_eq!(ui.vertices()[6].color, [1.0; 4]);
        assert_eq!(ui.vertices()[8].color, [0.0; 4]);
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
