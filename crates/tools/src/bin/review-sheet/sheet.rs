//! Contact-sheet composition: frames, crops and 5×7 text on an 8-bit canvas.

// Pixel coordinates and text lengths stay far below 2^31.
#![allow(clippy::cast_possible_wrap)]

use std::path::Path;

use crate::checks::{self, Check, Mask};
use crate::font5x7::FONT_5X7;
use crate::imaging::{self, Canvas};
use crate::meta::Meta;

/// Every sheet is this wide.
pub(crate) const SHEET_WIDTH: u32 = 2560;
const BACKGROUND: [u8; 3] = [24, 24, 26];
const TEXT: [u8; 3] = [230, 230, 230];
const DIM: [u8; 3] = [150, 150, 150];
const PASS: [u8; 3] = [110, 210, 120];
const FAIL: [u8; 3] = [240, 90, 80];
const MARK: [u8; 3] = [255, 220, 90];
const MARGIN: u32 = 16;

/// Draws `text` with its top-left at `(x, y)`, each font pixel `scale`² screen pixels.
pub(crate) fn draw_text(
    canvas: &mut Canvas,
    x: i64,
    y: i64,
    text: &str,
    scale: i64,
    colour: [u8; 3],
) {
    for (index, character) in text.chars().enumerate() {
        let code = character as u32;
        let glyph = if (0x20..0x80).contains(&code) {
            code - 0x20
        } else {
            u32::from(b'?') - 0x20
        };
        for column in 0..5i64 {
            let bits = FONT_5X7[glyph as usize * 5 + column as usize];
            for row in 0..8i64 {
                if bits >> row & 1 == 1 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            canvas.put(
                                x + (index as i64 * 6 + column) * scale + dx,
                                y + row * scale + dy,
                                colour,
                            );
                        }
                    }
                }
            }
        }
    }
}

/// One block of a sheet, stacked top to bottom.
pub(crate) enum Block {
    /// Lines of text at a scale, each with its colour.
    Text(i64, Vec<(String, [u8; 3])>),
    /// Images left to right, each with a caption above it.
    Row(Vec<(Canvas, String)>),
}

fn block_height(block: &Block) -> u32 {
    match block {
        Block::Text(scale, lines) => lines.len() as u32 * (9 * *scale as u32 + 4) + MARGIN,
        Block::Row(items) => {
            items
                .iter()
                .map(|(canvas, _)| canvas.height)
                .max()
                .unwrap_or(0)
                + 22
                + MARGIN
        }
    }
}

/// Stacks `blocks` into one sheet `SHEET_WIDTH` wide.
pub(crate) fn compose(blocks: &[Block]) -> Canvas {
    let height = MARGIN + blocks.iter().map(block_height).sum::<u32>();
    let mut canvas = Canvas::new(SHEET_WIDTH, height, BACKGROUND);
    let mut y = i64::from(MARGIN);
    for block in blocks {
        match block {
            Block::Text(scale, lines) => {
                for (index, (line, colour)) in lines.iter().enumerate() {
                    let line_y = y + index as i64 * (9 * scale + 4);
                    draw_text(
                        &mut canvas,
                        i64::from(MARGIN),
                        line_y,
                        line,
                        *scale,
                        *colour,
                    );
                }
            }
            Block::Row(items) => {
                let mut x = i64::from(MARGIN);
                for (image, caption) in items {
                    draw_text(&mut canvas, x, y, caption, 2, DIM);
                    canvas.paste(image, x, y + 20);
                    x += i64::from(image.width) + i64::from(MARGIN);
                }
            }
        }
        y += i64::from(block_height(block));
    }
    canvas
}

fn header(meta: &Meta, title: &str) -> Block {
    let mut lines = vec![(format!("{} - {title}", meta.name), TEXT)];
    lines.extend(
        crate::meta::camera_and_sun_lines(meta)
            .into_iter()
            .map(|line| (line, DIM)),
    );
    Block::Text(2, lines)
}

/// The check lines, coloured by outcome.
pub(crate) fn check_lines(checks: &[Check]) -> Block {
    Block::Text(
        2,
        checks
            .iter()
            .map(|check| {
                let (status, colour) = match check.pass {
                    Some(true) => ("PASS", PASS),
                    Some(false) => ("FAIL", FAIL),
                    None => ("info", DIM),
                };
                (
                    format!(
                        "{status}  {:<34} {:<44} {}",
                        check.rule, check.value, check.limit
                    ),
                    colour,
                )
            })
            .collect(),
    )
}

fn load(dir: &Path, file: &str) -> Result<Canvas, String> {
    Ok(Canvas::from_image(&imaging::read_png(&dir.join(file))?))
}

/// A square-ish region 3× a bbox (at least `min` px), clipped to the frame later.
fn around(bbox: [i64; 4], min: i64) -> [i64; 4] {
    let (w, h) = ((bbox[2] - bbox[0]).max(1), (bbox[3] - bbox[1]).max(1));
    let side = (w.max(h) * 3).max(min);
    let (cx, cy) = (
        i64::midpoint(bbox[0], bbox[2]),
        i64::midpoint(bbox[1], bbox[3]),
    );
    [cx - side / 2, cy - side / 2, side, side]
}

/// Writes the five sheets into `out` and returns their file names.
#[allow(clippy::too_many_lines)] // Five sheets, one after another.
pub(crate) fn write_sheets(
    meta: &Meta,
    dir: &Path,
    out: &Path,
    checks: &[Check],
) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(out).map_err(|error| format!("{}: {error}", out.display()))?;
    let mut written = Vec::new();
    let mut save = |name: &str, canvas: &Canvas| -> Result<(), String> {
        canvas.write_png(&out.join(name))?;
        written.push(name.to_owned());
        Ok(())
    };

    // 46 m: the 1440p frame 1:1, then each model's 1440p and 4K crops enlarged, then the checks.
    let frames = &meta.frames;
    let f1440 = load(dir, &frames.gameplay_1440p.file)?;
    let f2160 = load(dir, &frames.gameplay_2160p.file)?;
    let mut crops = Vec::new();
    for model in &frames.gameplay_1440p.models {
        let region = around(model.bbox, 96);
        crops.push((
            f1440.crop(region).enlarge(4),
            format!("{} at 1440p (x4)", model.name),
        ));
        let region_4k = [
            region[0] * 3 / 2,
            region[1] * 3 / 2,
            region[2] * 3 / 2,
            region[3] * 3 / 2,
        ];
        let crop_4k = f2160.crop(region_4k);
        let enlarged = crop_4k.resample(region[2] as u32 * 4, region[3] as u32 * 4);
        crops.push((enlarged, format!("{} at 4K (same area)", model.name)));
    }
    let mut blocks = vec![
        header(
            meta,
            "46 m gameplay camera, 2560x1440 (1:1) and 3840x2160 crops",
        ),
        Block::Row(vec![(f1440.clone(), "46 m, 1440p, 1:1".into())]),
    ];
    for pair in crops.chunks(4) {
        blocks.push(Block::Row(pair.to_vec()));
    }
    blocks.push(check_lines(checks));
    save("sheet_46m.png", &compose(&blocks))?;

    // 8 m zoom: each frame at half size, two to a row.
    let mut blocks = vec![header(meta, "8 m zoom camera")];
    let zooms: Vec<(Canvas, String)> = frames
        .zoom
        .iter()
        .map(|frame| {
            Ok((
                load(dir, &frame.file)?.resample(1256, 706),
                format!(
                    "{} at 8 m (2560x1440, shown half size)",
                    frame.model.as_deref().unwrap_or("?")
                ),
            ))
        })
        .collect::<Result<_, String>>()?;
    for pair in zooms.chunks(2) {
        blocks.push(Block::Row(pair.to_vec()));
    }
    save("sheet_8m.png", &compose(&blocks))?;

    // Turntable: eight yaws per model.
    let mut blocks = vec![header(
        meta,
        "turntable, default camera angles, model yawed in 45 deg steps",
    )];
    for turn in &frames.turntable {
        let row = turn
            .files
            .iter()
            .zip(&turn.yaws_deg)
            .map(|(file, yaw)| {
                Ok((
                    load(dir, file)?.resample(300, 300),
                    format!("{} {yaw:.0} deg", turn.model),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        blocks.push(Block::Text(
            2,
            vec![(format!("{} at {:.1} m", turn.model, turn.distance_m), TEXT)],
        ));
        blocks.push(Block::Row(row));
    }
    save("sheet_turntable.png", &compose(&blocks))?;

    // Lineup with labels: name, glb height, size class.
    let lineup = &frames.lineup_1440p;
    let mut canvas = load(dir, &lineup.file)?;
    for (index, model_box) in lineup.models.iter().enumerate() {
        let [x0, y0, x1, y1] = model_box.bbox;
        canvas.outline([x0 - 2, y0 - 2, x1 + 2, y1 + 2], 1, MARK);
        let model = meta
            .models
            .iter()
            .find(|model| model.name == model_box.name);
        let label = model.map_or_else(String::new, |model| {
            let height =
                moba_assets::load_glb_file_with(&model.glb, moba_assets::GlbTangents::FileOnly)
                    .map(|mesh| {
                        let (low, high) = mesh.bounds();
                        format!("{:.2} m", high[1] - low[1])
                    })
                    .unwrap_or_default();
            format!("{} {height} {}", model.name, model.size_class)
        });
        // Neighbours' labels alternate between two heights so long names never overlap.
        let lift = if index % 2 == 0 { 24 } else { 46 };
        draw_text(&mut canvas, x0, y0 - lift, &label, 2, MARK);
    }
    let [px0, py0, px1, _] = lineup.post_bbox;
    draw_text(
        &mut canvas,
        px0.min(px1),
        py0 - 24,
        &format!("{:.1} m post", meta.post_m),
        2,
        MARK,
    );
    let blocks = vec![
        header(
            meta,
            &format!("lineup at {:.1} m, 1440p", lineup.distance_m),
        ),
        Block::Row(vec![(
            canvas,
            "lineup, Golden Crossing (the only Ember Crossing grade)".into(),
        )]),
    ];
    save("sheet_lineup.png", &compose(&blocks))?;

    // Silhouettes: flat masks (body red, tell green, weapon blue) per yaw, enlarged pixel-true.
    let mut blocks = vec![header(
        meta,
        "flat silhouettes from the gameplay camera, 8 yaws (red body, green tell, blue weapon)",
    )];
    for set in &frames.silhouettes {
        let loaded = checks::load_silhouettes(dir, &set.files)?;
        let largest = loaded
            .images
            .iter()
            .map(|image| image.width.max(image.height))
            .max()
            .unwrap_or(1);
        let factor = (280 / largest.max(1)).clamp(1, 6);
        let row = loaded
            .images
            .iter()
            .zip(&loaded.whole)
            .zip(&set.yaws_deg)
            .map(|((image, whole), yaw)| {
                let (parts, _) = checks::hair_parts(whole);
                let caption = if parts > 0 {
                    format!("{yaw:.0} deg, {parts} thin")
                } else {
                    format!("{yaw:.0} deg")
                };
                (outlined(image, whole).enlarge(factor), caption)
            })
            .collect();
        blocks.push(Block::Text(
            2,
            vec![(format!("{} at {} (x{factor})", set.model, set.res), TEXT)],
        ));
        blocks.push(Block::Row(row));
    }
    save("sheet_silhouettes.png", &compose(&blocks))?;
    Ok(written)
}

/// The silhouette with SIL-8's thin pixels marked in yellow.
fn outlined(image: &imaging::Image, whole: &Mask) -> Canvas {
    let mut canvas = Canvas::from_image(image);
    let thin = whole.thin();
    for (index, bit) in thin.bits.iter().enumerate() {
        if *bit {
            let (x, y) = (
                (index % thin.width as usize) as i64,
                (index / thin.width as usize) as i64,
            );
            canvas.put(x, y, MARK);
        }
    }
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_draws_glyph_pixels_at_scale() {
        let mut canvas = Canvas::new(20, 20, [0, 0, 0]);
        draw_text(&mut canvas, 0, 0, "|", 2, [255, 255, 255]);
        // '|' is column 2 = 0x77: rows 0, 1, 2, 4, 5, 6 lit, row 3 dark.
        assert_eq!(canvas.pixel(4, 0), [255, 255, 255]);
        assert_eq!(canvas.pixel(5, 1), [255, 255, 255]);
        assert_eq!(canvas.pixel(4, 6), [0, 0, 0]);
        assert_eq!(canvas.pixel(0, 0), [0, 0, 0]);
    }

    #[test]
    fn compose_stacks_blocks_at_sheet_width() {
        let sheet = compose(&[
            Block::Text(2, vec![("a".into(), TEXT)]),
            Block::Row(vec![(Canvas::new(10, 30, [9, 9, 9]), "x".into())]),
        ]);
        assert_eq!(sheet.width, SHEET_WIDTH);
        assert_eq!(sheet.height, MARGIN + (22 + MARGIN) + (30 + 22 + MARGIN));
    }
}
