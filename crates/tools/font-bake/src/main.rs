#![allow(clippy::print_stdout)]
//! Bake the HUD font into a multi-channel signed-distance-field atlas (R7a).
//!
//! Usage:
//!
//! ```text
//! cargo run --release --locked --manifest-path crates/tools/font-bake/Cargo.toml -- \
//!     [font.ttf] [atlas.rgba] [metrics.rs]
//! ```
//!
//! The defaults read `crates/tools/fonts/OpenSans-CondensedSemiBold.ttf` (SIL OFL 1.1, see
//! `crates/tools/fonts/OFL.txt` and `THIRD_PARTY.md`) and write the checked-in
//! `crates/render/assets/ui-font.rgba` plus `crates/render/src/font_metrics.rs`, so normal
//! builds never need this tool or its dependencies.
//!
//! The atlas is an MTSDF: RGB hold the multi-channel field (sharp corners at any scale) and
//! alpha holds the true single-channel distance (soft shadows and wide outlines). Glyphs are
//! printable ASCII, generated with `fdsm` (MIT) from outlines read by `skrifa`
//! (MIT OR Apache-2.0). The output is deterministic: a fixed edge-colouring seed, a fixed
//! glyph order and a fixed shelf packing.

use std::fmt::Write as _;
use std::path::PathBuf;

use fdsm::bezier::scanline::FillRule;
use fdsm::correct_error::{ErrorCorrectionConfig, correct_error_mtsdf};
use fdsm::generate::generate_mtsdf;
use fdsm::render::correct_sign_mtsdf;
use fdsm::shape::Shape;
use fdsm::transform::Transform;
use image::{ImageBuffer, Rgba};
use nalgebra::{Affine2, Matrix3};
use skrifa::prelude::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};

/// Atlas pixels per em.
const EM_PX: f64 = 40.0;
/// Total distance range in atlas pixels (`±RANGE/2` around the edge).
const RANGE_PX: f64 = 8.0;
/// Empty margin around each glyph so outlines and shadows reach `RANGE_PX / 2`.
const MARGIN_PX: f64 = RANGE_PX * 0.5 + 1.0;
/// Atlas width in pixels; the height is rounded up to a multiple of 4.
const ATLAS_WIDTH: u32 = 512;
/// Gutter between packed cells.
const GUTTER: u32 = 1;
/// Corner threshold for the edge colouring (sin of ~3 degrees, msdfgen's default).
const CORNER_SIN_ALPHA: f64 = 0.03;
/// Fixed edge-colouring seed for a reproducible bake.
const COLORING_SEED: u64 = 0x4D4F_4241;
const FIRST: u32 = 0x20;
const LAST: u32 = 0x7E;

struct Glyph {
    code: u32,
    advance: f64,
    /// Plane bounds in em: left, bottom, right, top (y up from the baseline).
    plane: [f64; 4],
    /// Cell size in atlas pixels.
    size: [u32; 2],
    /// Cell position once packed.
    at: [u32; 2],
    pixels: Vec<[u8; 4]>,
}

#[allow(clippy::too_many_lines)] // One linear bake: load, generate, pack, write.
fn main() {
    let mut args = std::env::args_os().skip(1);
    let font = args.next().map_or_else(
        || PathBuf::from("crates/tools/fonts/OpenSans-CondensedSemiBold.ttf"),
        PathBuf::from,
    );
    let atlas_path = args.next().map_or_else(
        || PathBuf::from("crates/render/assets/ui-font.rgba"),
        PathBuf::from,
    );
    let metrics_path = args.next().map_or_else(
        || PathBuf::from("crates/render/src/font_metrics.rs"),
        PathBuf::from,
    );
    let bytes = std::fs::read(&font).expect("read font");
    let face = FontRef::new(&bytes).expect("parse font");
    let location = LocationRef::default();
    let metrics = face.metrics(Size::unscaled(), location);
    let units_per_em = f64::from(metrics.units_per_em);
    let charmap = face.charmap();
    let advances = face.glyph_metrics(Size::unscaled(), location);
    let shrink = units_per_em / EM_PX;

    let mut glyphs = Vec::new();
    for code in FIRST..=LAST {
        let character = char::from_u32(code).expect("ascii");
        let id = charmap.map(character).expect("font covers printable ASCII");
        let advance = f64::from(advances.advance_width(id).unwrap_or(0.0)) / units_per_em;
        let (mut shape, _) =
            fdsm_skrifa::load_shape_from_face(&face, id, location).expect("glyph outline");
        let Some([x_min, y_min, x_max, y_max]) = bounds(&shape) else {
            glyphs.push(Glyph {
                code,
                advance,
                plane: [0.0; 4],
                size: [0, 0],
                at: [0, 0],
                pixels: Vec::new(),
            });
            continue;
        };
        // Font units (y up) -> cell pixels (y down) with the margin on every side.
        let origin_x = MARGIN_PX - x_min / shrink;
        let origin_y = MARGIN_PX + y_max / shrink;
        let transform = Affine2::from_matrix_unchecked(Matrix3::new(
            1.0 / shrink,
            0.0,
            origin_x,
            0.0,
            -1.0 / shrink,
            origin_y,
            0.0,
            0.0,
            1.0,
        ));
        shape.transform(&transform);
        let width = ((x_max - x_min) / shrink + 2.0 * MARGIN_PX).ceil() as u32;
        let height = ((y_max - y_min) / shrink + 2.0 * MARGIN_PX).ceil() as u32;
        let colored = Shape::edge_coloring_simple(shape, CORNER_SIN_ALPHA, COLORING_SEED);
        let prepared = colored.prepare();
        let mut field: ImageBuffer<Rgba<f32>, Vec<f32>> = ImageBuffer::new(width, height);
        generate_mtsdf(&prepared, RANGE_PX, &mut field);
        correct_error_mtsdf(
            &mut field,
            &colored,
            &prepared,
            RANGE_PX,
            &ErrorCorrectionConfig::default(),
        );
        correct_sign_mtsdf(&mut field, &prepared, FillRule::Nonzero);
        let pixels = field.pixels().map(|pixel| pixel.0.map(quantize)).collect();
        glyphs.push(Glyph {
            code,
            advance,
            plane: [
                -origin_x / EM_PX,
                -(f64::from(height) - origin_y) / EM_PX,
                (f64::from(width) - origin_x) / EM_PX,
                origin_y / EM_PX,
            ],
            size: [width, height],
            at: [0, 0],
            pixels,
        });
    }

    let height = pack(&mut glyphs);
    let mut atlas = vec![0u8; (ATLAS_WIDTH * height * 4) as usize];
    for glyph in &glyphs {
        for row in 0..glyph.size[1] {
            for column in 0..glyph.size[0] {
                let source = glyph.pixels[(row * glyph.size[0] + column) as usize];
                let offset =
                    (((glyph.at[1] + row) * ATLAS_WIDTH + glyph.at[0] + column) * 4) as usize;
                atlas[offset..offset + 4].copy_from_slice(&source);
            }
        }
    }
    let em = units_per_em;
    let source = metrics_source(
        &glyphs,
        height,
        [
            f64::from(metrics.ascent) / em,
            f64::from(metrics.descent) / em,
            f64::from(metrics.leading) / em,
            f64::from(metrics.cap_height.unwrap_or(metrics.ascent * 0.7)) / em,
        ],
    );
    for path in [&atlas_path, &metrics_path] {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create output directory");
        }
    }
    std::fs::write(&atlas_path, &atlas).expect("write atlas");
    std::fs::write(&metrics_path, source).expect("write metrics");
    println!(
        "wrote {} ({ATLAS_WIDTH}x{height} MTSDF, {} bytes) and {}",
        atlas_path.display(),
        atlas.len(),
        metrics_path.display()
    );
}

/// Control-point bounding box of a shape in font units (conservative for curves).
fn bounds(shape: &Shape<fdsm::shape::Contour>) -> Option<[f64; 4]> {
    let mut out: Option<[f64; 4]> = None;
    for contour in &shape.contours {
        for segment in &contour.segments {
            let points = match segment.order() {
                fdsm::bezier::Order::Linear => 2,
                fdsm::bezier::Order::Quadratic => 3,
                fdsm::bezier::Order::Cubic => 4,
            };
            for index in 0..points {
                let point = segment.control_point(index);
                let entry = out.get_or_insert([point.x, point.y, point.x, point.y]);
                entry[0] = entry[0].min(point.x);
                entry[1] = entry[1].min(point.y);
                entry[2] = entry[2].max(point.x);
                entry[3] = entry[3].max(point.y);
            }
        }
    }
    out
}

fn quantize(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Shelf-packs glyph cells (tallest first, code as the tie-break); returns the atlas height.
fn pack(glyphs: &mut [Glyph]) -> u32 {
    let mut order: Vec<usize> = (0..glyphs.len()).collect();
    order.sort_by_key(|&index| (std::cmp::Reverse(glyphs[index].size[1]), glyphs[index].code));
    let (mut x, mut y, mut shelf) = (GUTTER, GUTTER, 0);
    for index in order {
        let [width, height] = glyphs[index].size;
        if width == 0 {
            continue;
        }
        if x + width + GUTTER > ATLAS_WIDTH {
            x = GUTTER;
            y += shelf + GUTTER;
            shelf = 0;
        }
        glyphs[index].at = [x, y];
        x += width + GUTTER;
        shelf = shelf.max(height);
    }
    (y + shelf + GUTTER).next_multiple_of(4)
}

fn metrics_source(
    glyphs: &[Glyph],
    height: u32,
    [ascent, descent, leading, cap]: [f64; 4],
) -> String {
    let mut out = String::new();
    out.push_str(
        "//! Generated by `crates/tools/font-bake` (`bake-ui-font`).\n\
         //! Do not edit: re-run the baker. Metrics are in em; atlas rectangles in pixels.\n\
         //! Font: Open Sans Condensed `SemiBold` (SIL OFL 1.1, see `THIRD_PARTY.md`).\n\n\
         use crate::font::FontGlyph;\n\n",
    );
    let _ = writeln!(out, "/// Atlas width in pixels.");
    let _ = writeln!(
        out,
        "pub(crate) const FONT_ATLAS_WIDTH: u32 = {ATLAS_WIDTH};"
    );
    let _ = writeln!(out, "/// Atlas height in pixels.");
    let _ = writeln!(out, "pub(crate) const FONT_ATLAS_HEIGHT: u32 = {height};");
    let _ = writeln!(out, "/// Atlas pixels per em.");
    let _ = writeln!(out, "pub(crate) const FONT_EM_PX: f32 = {EM_PX:.1};");
    let _ = writeln!(out, "/// Total distance range in atlas pixels.");
    let _ = writeln!(out, "pub(crate) const FONT_RANGE_PX: f32 = {RANGE_PX:.1};");
    let _ = writeln!(out, "/// Ascender above the baseline.");
    let _ = writeln!(out, "#[allow(dead_code)] // For multi-line layout.");
    let _ = writeln!(out, "pub(crate) const FONT_ASCENT: f32 = {ascent:.5};");
    let _ = writeln!(out, "/// Descender (negative: below the baseline).");
    let _ = writeln!(out, "#[allow(dead_code)] // For multi-line layout.");
    let _ = writeln!(out, "pub(crate) const FONT_DESCENT: f32 = {descent:.5};");
    let _ = writeln!(out, "/// Line gap.");
    let _ = writeln!(out, "#[allow(dead_code)] // For multi-line layout.");
    let _ = writeln!(out, "pub(crate) const FONT_LEADING: f32 = {leading:.5};");
    let _ = writeln!(out, "/// Capital height.");
    let _ = writeln!(out, "pub(crate) const FONT_CAP_HEIGHT: f32 = {cap:.5};");
    out.push_str("\n/// Printable ASCII from `0x20`: advance, plane bounds (l, b, r, t), atlas x, y, w, h.\n");
    out.push_str("#[rustfmt::skip]\n");
    let _ = writeln!(
        out,
        "pub(crate) const FONT_GLYPHS: [FontGlyph; {}] = [",
        glyphs.len()
    );
    for glyph in glyphs {
        let [l, b, r, t] = glyph.plane;
        let _ = writeln!(
            out,
            "    FontGlyph::new({:.5}, [{l:.5}, {b:.5}, {r:.5}, {t:.5}], [{}, {}, {}, {}]), // {:?}",
            glyph.advance,
            glyph.at[0],
            glyph.at[1],
            glyph.size[0],
            glyph.size[1],
            char::from_u32(glyph.code).unwrap_or(' ')
        );
    }
    out.push_str("];\n");
    out
}
