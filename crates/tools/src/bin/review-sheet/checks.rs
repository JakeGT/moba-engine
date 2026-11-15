//! The numeric checks on the review frames: CON-1/CON-4 and VAL-2 to VAL-4 (palette-and-value),
//! PBR-EMIT-1 to PBR-EMIT-3 rendered (material-vocabulary §8), SIL-1 to SIL-8
//! (silhouettes-and-scale). Thresholds are the bible's, read literally; where a rule leaves
//! a reading open the choice is named in the limit text and in ADR-0022 amendment 2.

// Pixel indices and coordinates stay far below 2^31 (frames are at most 8192² by `read_png`).
#![allow(clippy::cast_possible_wrap)]

use std::collections::HashSet;
use std::path::Path;

use crate::imaging::{self, Image};
use crate::meta::{Meta, ModelMeta};

/// One rule's outcome (`None` = information or a human check).
#[derive(Clone, Debug)]
pub(crate) struct Check {
    pub(crate) rule: String,
    pub(crate) pass: Option<bool>,
    pub(crate) value: String,
    pub(crate) limit: String,
}

/// SIL-7's ranged quorum: the weapon is visible from this many of the 8 yaws. Every other
/// silhouette rule is read literally, at every yaw ("tested ... at eight yaw angles").
pub(crate) const RANGED_YAWS: usize = 6;
/// Samples per 1080p pixel side in the `1080p4x` silhouettes (SIL-8).
pub(crate) const SIL8_SAMPLES: i64 = 4;
/// Silhouette-and-scale size classes (height in metres).
pub(crate) const SIZE_CLASSES: [(&str, f64, f64); 9] = [
    ("minion-ranged", 1.2, 1.4),
    ("minion-melee", 1.3, 1.5),
    ("minion-siege", 2.2, 2.6),
    ("hero-s", 1.6, 1.8),
    ("hero-m", 1.8, 2.0),
    ("hero-l", 2.1, 2.6),
    ("large-monster", 4.0, 7.0),
    ("tower", 12.0, 16.0),
    ("core", 18.0, 24.0),
];

/// A boolean pixel mask.
#[derive(Clone, Debug)]
pub(crate) struct Mask {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) bits: Vec<bool>,
}

impl Mask {
    pub(crate) fn from_image(image: &Image, test: impl Fn([f32; 3]) -> bool) -> Self {
        Self {
            width: image.width,
            height: image.height,
            bits: (0..image.len())
                .map(|index| test(image.rgb(index)))
                .collect(),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_rows(rows: &[&str]) -> Self {
        Self {
            width: rows.first().map_or(0, |row| row.len() as u32),
            height: rows.len() as u32,
            bits: rows
                .iter()
                .flat_map(|row| row.bytes().map(|byte| byte == b'#'))
                .collect(),
        }
    }

    fn at(&self, x: i64, y: i64) -> bool {
        x >= 0
            && y >= 0
            && x < i64::from(self.width)
            && y < i64::from(self.height)
            && self.bits[y as usize * self.width as usize + x as usize]
    }

    pub(crate) fn count(&self) -> usize {
        self.bits.iter().filter(|bit| **bit).count()
    }

    /// `[x0, y0, x1, y1]`, exclusive ends.
    pub(crate) fn bbox(&self) -> Option<[i64; 4]> {
        let mut out: Option<[i64; 4]> = None;
        for (index, _) in self.bits.iter().enumerate().filter(|(_, bit)| **bit) {
            let (x, y) = (
                (index % self.width as usize) as i64,
                (index / self.width as usize) as i64,
            );
            out = Some(match out {
                None => [x, y, x + 1, y + 1],
                Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)],
            });
        }
        out
    }

    /// Connected components of `value` pixels (8- or 4-connected), as pixel index lists.
    pub(crate) fn components(&self, value: bool, eight: bool) -> Vec<Vec<usize>> {
        let (width, height) = (i64::from(self.width), i64::from(self.height));
        let mut seen = vec![false; self.bits.len()];
        let mut out = Vec::new();
        let steps: &[(i64, i64)] = if eight {
            &[
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ]
        } else {
            &[(1, 0), (-1, 0), (0, 1), (0, -1)]
        };
        for start in 0..self.bits.len() {
            if seen[start] || self.bits[start] != value {
                continue;
            }
            seen[start] = true;
            let mut stack = vec![start];
            let mut component = Vec::new();
            while let Some(index) = stack.pop() {
                component.push(index);
                let (x, y) = (
                    (index % self.width as usize) as i64,
                    (index / self.width as usize) as i64,
                );
                for (dx, dy) in steps {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= width || ny >= height {
                        continue;
                    }
                    let next = (ny * width + nx) as usize;
                    if !seen[next] && self.bits[next] == value {
                        seen[next] = true;
                        stack.push(next);
                    }
                }
            }
            out.push(component);
        }
        out
    }

    /// Whether some 2×2 block lies wholly inside `component` (the part is ≥ 2 px wide).
    fn two_wide(&self, component: &[usize]) -> bool {
        let set: HashSet<usize> = component.iter().copied().collect();
        let width = self.width as usize;
        component.iter().any(|&index| {
            index % width + 1 < width
                && set.contains(&(index + 1))
                && set.contains(&(index + width))
                && set.contains(&(index + width + 1))
        })
    }

    /// Pixels in no all-set `block`×`block` square: thinner than `block` pixels (the
    /// `block`×`block` morphological opening removes them).
    pub(crate) fn thin(&self, block: i64) -> Self {
        let (width, height) = (i64::from(self.width), i64::from(self.height));
        // `full[y][x]`: the square with its top-left corner at (x, y) is all set.
        let full: Vec<bool> = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| (0..block).all(|dy| (0..block).all(|dx| self.at(x + dx, y + dy))))
            .collect();
        let is_full = |x: i64, y: i64| {
            x >= 0 && y >= 0 && x < width && y < height && full[(y * width + x) as usize]
        };
        let mut bits = vec![false; self.bits.len()];
        for y in 0..height {
            for x in 0..width {
                if self.at(x, y) {
                    let covered = (0..block).any(|dy| (0..block).any(|dx| is_full(x - dx, y - dy)));
                    bits[(y * width + x) as usize] = !covered;
                }
            }
        }
        Self {
            width: self.width,
            height: self.height,
            bits,
        }
    }

    /// SIL-6: enclosed holes plus floating parts beyond the main body, each ≥ 2 px wide.
    pub(crate) fn void_tell_count(&self) -> usize {
        let width = self.width as usize;
        let height = self.height as usize;
        let holes = self
            .components(false, false)
            .into_iter()
            .filter(|component| {
                component.iter().all(|&index| {
                    let (x, y) = (index % width, index / width);
                    x > 0 && y > 0 && x + 1 < width && y + 1 < height
                })
            })
            .filter(|component| self.two_wide(component))
            .count();
        let mut parts = self.components(true, true);
        parts.sort_by_key(|component| std::cmp::Reverse(component.len()));
        let floating = parts
            .iter()
            .skip(1)
            .filter(|component| self.two_wide(component))
            .count();
        holes + floating
    }

    /// Set pixels relative to the bbox's bottom-centre (the feet).
    fn anchored(&self) -> HashSet<(i64, i64)> {
        let Some([x0, _, x1, y1]) = self.bbox() else {
            return HashSet::new();
        };
        let cx = i64::midpoint(x0, x1);
        self.bits
            .iter()
            .enumerate()
            .filter(|(_, bit)| **bit)
            .map(|(index, _)| {
                let (x, y) = (
                    (index % self.width as usize) as i64,
                    (index / self.width as usize) as i64,
                );
                (x - cx, y - y1)
            })
            .collect()
    }
}

/// SIL-3: the best `IoU` of two silhouettes aligned at the feet, over shifts of ±`search` px.
pub(crate) fn best_iou(a: &Mask, b: &Mask, search: i64) -> f64 {
    let (a, b) = (a.anchored(), b.anchored());
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut best = 0.0f64;
    for dy in -search..=search {
        for dx in -search..=search {
            let inter = a
                .iter()
                .filter(|(x, y)| b.contains(&(x + dx, y + dy)))
                .count();
            let union = a.len() + b.len() - inter;
            best = best.max(inter as f64 / union as f64);
        }
    }
    best
}

/// SIL-8 on a silhouette rendered at `block`× the checked resolution: (hair-thin parts, thin
/// pixels). A thin pixel is narrower than one checked pixel (`block` samples); a part is a
/// connected run of them at least two checked pixels long (shorter runs are tapering tips).
pub(crate) fn hair_parts(mask: &Mask, block: i64) -> (usize, usize) {
    let thin = mask.thin(block);
    let width = thin.width as usize;
    let parts = thin
        .components(true, true)
        .iter()
        .filter(|component| {
            let xs = component.iter().map(|index| index % width);
            let ys = component.iter().map(|index| index / width);
            let span_x = xs.clone().max().unwrap_or(0) - xs.min().unwrap_or(0) + 1;
            let span_y = ys.clone().max().unwrap_or(0) - ys.min().unwrap_or(0) + 1;
            span_x.max(span_y) as i64 >= 2 * block
        })
        .count();
    (parts, thin.count())
}

fn push(checks: &mut Vec<Check>, rule: &str, pass: Option<bool>, value: String, limit: &str) {
    checks.push(Check {
        rule: rule.to_owned(),
        pass,
        value,
        limit: limit.to_owned(),
    });
}

fn display_l(image: &Image, index: usize) -> f32 {
    imaging::oklab_l_display(image.rgb(index))
}

fn is_unit(model: &ModelMeta) -> bool {
    model.role == "hero" || model.role == "minion"
}

fn emissive_cap(model: &ModelMeta) -> f64 {
    match model.role.as_str() {
        "structure" => 0.10,
        "prop" => 0.03,
        _ => 0.05,
    }
}

/// Silhouette masks for one model at one resolution: (whole, tell, weapon) per yaw.
pub(crate) struct SilhouetteSet {
    pub(crate) whole: Vec<Mask>,
    pub(crate) tell: Vec<Mask>,
    pub(crate) weapon: Vec<Mask>,
    pub(crate) images: Vec<Image>,
}

pub(crate) fn load_silhouettes(dir: &Path, files: &[String]) -> Result<SilhouetteSet, String> {
    let mut set = SilhouetteSet {
        whole: Vec::new(),
        tell: Vec::new(),
        weapon: Vec::new(),
        images: Vec::new(),
    };
    for file in files {
        let image = imaging::read_png(&dir.join(file))?;
        set.whole
            .push(Mask::from_image(&image, |c| c.iter().any(|v| *v > 0.5)));
        set.tell.push(Mask::from_image(&image, |c| c[1] > 0.5));
        set.weapon.push(Mask::from_image(&image, |c| c[2] > 0.5));
        set.images.push(image);
    }
    Ok(set)
}

/// Model pixels in the 46 m frame, by id colour.
pub(crate) fn id_mask(ids: &Image, colour: [u8; 3]) -> Mask {
    Mask::from_image(ids, |c| {
        c.iter()
            .zip(colour)
            .all(|(value, want)| (*value > 0.5) == (want > 0))
    })
}

/// Every numeric check, in the bible's order.
pub(crate) fn run(meta: &Meta, dir: &Path) -> Result<Vec<Check>, String> {
    let mut checks = frame_checks(meta, dir)?;
    silhouette_checks(meta, dir, &mut checks)?;
    for (rule, what) in HUMAN_OR_NOT_BUILT {
        push(&mut checks, rule, None, "-".into(), what);
    }
    Ok(checks)
}

/// The rules the sheets leave to a person, or that are not built.
const HUMAN_OR_NOT_BUILT: [(&str, &str); 10] = [
    (
        "SIL-4",
        "role tell in the top third (human, on the silhouette sheet)",
    ),
    (
        "SIL-9",
        "idle, run and cast key poses each pass SIL-3 (human; one pose per render)",
    ),
    ("PAL-3", "accent area and chroma (not built)"),
    (
        "VOID-1",
        "bone or silver >= 10 % of projected area (not built)",
    ),
    ("EMB-1", "ember crystal hue and facet rim (not built)"),
    ("PBR-HEAT-1", "temper colours and patina (not built)"),
    (
        "PBR-WEAR-1r",
        "dust on the lower body (rendered half not built)",
    ),
    (
        "PBR-VOID-1r",
        ">= 15 % of each Void hero's projected area in pewter, silver or bone (not built)",
    ),
    (
        "SIL-2 (parts)",
        "no capes, halos or orbiting parts on minions (human, on the turntable sheet)",
    ),
    (
        "VFX-9",
        "ability type coded by shape and motion, not colour (no VFX in these renders)",
    ),
];

/// CON, VAL and rendered EMIT on the 46 m 1440p frame, its ids and its emission frame.
#[allow(clippy::too_many_lines)] // One rule after another, in the bible's order.
pub(crate) fn frame_checks(meta: &Meta, dir: &Path) -> Result<Vec<Check>, String> {
    let mut checks = Vec::new();
    let frame = &meta.frames.gameplay_1440p;
    let beauty = imaging::read_png(&dir.join(&frame.file))?;
    let ids =
        imaging::read_png(&dir.join(frame.ids.as_deref().ok_or("46m_1440p has no ids frame")?))?;
    let emit =
        imaging::read_png(&dir.join(frame.emit.as_deref().ok_or("46m_1440p has no emit frame")?))?;
    if (beauty.width, beauty.height) != (2560, 1440)
        || (ids.width, ids.height) != (2560, 1440)
        || (emit.width, emit.height) != (2560, 1440)
    {
        return Err("the 46 m 1440p frames are not 2560x1440".into());
    }
    if !emit.sixteen_bit {
        return Err("the emission frame must be 16-bit linear".into());
    }
    let ground = Mask::from_image(&ids, |c| c.iter().all(|v| *v <= 0.5));
    let post = id_mask(&ids, meta.post_colour);
    let l_ref = meta.l_ref;
    let emissive_px = |index: usize| f64::from(imaging::luminance(emit.rgb(index))) > 0.25 * l_ref;

    // Per model: contrast and rendered emission.
    let mut all_emissive = 0usize;
    // Each model with its 3x box (VAL-4 looks here), and the ground inside those boxes, cast
    // shadows included (VAL-3 looks here). The ground outside every box is the bare ground
    // VAL-2 reads.
    let mut near = vec![false; beauty.len()];
    let mut ring_ground = vec![false; beauty.len()];
    for model in &meta.models {
        let colour = *meta
            .id_colours
            .get(&model.name)
            .ok_or_else(|| format!("no id colour for {}", model.name))?;
        let mask = id_mask(&ids, colour);
        let Some([x0, y0, x1, y1]) = mask.bbox() else {
            push(
                &mut checks,
                &format!("FRAME ({})", model.name),
                Some(false),
                "not in the 46 m frame".into(),
                "model visible at 46 m",
            );
            continue;
        };
        let pixels: Vec<usize> = (0..mask.bits.len())
            .filter(|&index| mask.bits[index])
            .collect();
        let model_l = imaging::mean(
            &pixels
                .iter()
                .map(|&index| display_l(&beauty, index))
                .collect::<Vec<_>>(),
        );
        let (w, h) = (x1 - x0, y1 - y0);
        let mut ring: Vec<f32> = Vec::new();
        for y in (y0 - h).max(0)..(y1 + h).min(1440) {
            for x in (x0 - w).max(0)..(x1 + w).min(2560) {
                let index = (y * 2560 + x) as usize;
                near[index] = true;
                if ground.bits[index] {
                    ring_ground[index] = true;
                    if !(x >= x0 && x < x1 && y >= y0 && y < y1) {
                        ring.push(display_l(&beauty, index));
                    }
                }
            }
        }
        let (rule, need) = match model.role.as_str() {
            "hero" => ("CON-1", Some(0.10)),
            "minion" => ("CON-4", Some(0.08)),
            _ => ("CON", None),
        };
        let limit = match need {
            Some(_) if rule == "CON-1" => "hero vs ring 3x its bbox |dL| >= 0.10, no rims",
            Some(_) => "minion vs ring 3x its bbox |dL| >= 0.08, no rims",
            None => "no CON rule for this role (information)",
        };
        if ring.is_empty() {
            // Nothing to contrast against is not a pass.
            push(
                &mut checks,
                &format!("{rule} ({})", model.name),
                need.map(|_| false),
                "no ground ring (the 3x box holds no ground pixels)".into(),
                limit,
            );
        } else {
            let ring_l = imaging::mean(&ring);
            let delta = (model_l - ring_l).abs();
            push(
                &mut checks,
                &format!("{rule} ({})", model.name),
                need.map(|limit| delta >= limit),
                format!("|dL| {delta:.3} (model {model_l:.3}, ring {ring_l:.3})"),
                limit,
            );
        }
        let lit_emissive: Vec<f64> = pixels
            .iter()
            .filter(|&&index| emissive_px(index))
            .map(|&index| f64::from(imaging::luminance(emit.rgb(index))))
            .collect();
        all_emissive += lit_emissive.len();
        let share = lit_emissive.len() as f64 / pixels.len().max(1) as f64;
        push(
            &mut checks,
            &format!("PBR-EMIT-2r ({})", model.name),
            Some(share <= emissive_cap(model)),
            format!("{:.2} % of its pixels", share * 100.0),
            &format!(
                "rendered emissive area (> 0.25 L_ref) <= {:.0} % ({})",
                emissive_cap(model) * 100.0,
                model.role
            ),
        );
        if lit_emissive.is_empty() {
            push(
                &mut checks,
                &format!("PBR-EMIT-1r ({})", model.name),
                None,
                "no emissive pixels".into(),
                "rendered emission <= cap",
            );
        } else {
            let mut ev: Vec<f32> = lit_emissive
                .iter()
                .map(|lum| (lum / l_ref).log2() as f32)
                .collect();
            let peak = imaging::percentile(&mut ev, 99.0).unwrap_or(0.0);
            let usage = model.emissive.usage.as_deref().unwrap_or("accent");
            let cap = if usage == "gameplay" { 2.0 } else { 1.0 };
            push(
                &mut checks,
                &format!("PBR-EMIT-1r ({})", model.name),
                Some(f64::from(peak) <= cap),
                format!("p99 {peak:+.2} EV vs L_ref"),
                &format!("rendered emission <= +{cap:.0} EV ({usage})"),
            );
        }
    }
    // VAL-2..4 on this review frame (the bible runs them on captures; see the amendment).
    let mut bare: Vec<f32> = (0..beauty.len())
        .filter(|&index| ground.bits[index] && !near[index])
        .map(|index| display_l(&beauty, index))
        .collect();
    if bare.is_empty() {
        push(
            &mut checks,
            "VAL-2",
            Some(false),
            "no bare ground outside the models' 3x boxes".into(),
            "mean OKLab L 0.30-0.58, p5-p95 <= 0.30",
        );
    } else {
        let mean = imaging::mean(&bare);
        bare.sort_unstable_by(f32::total_cmp);
        let p5 = imaging::percentile_sorted(&bare, 5.0);
        let p95 = imaging::percentile_sorted(&bare, 95.0);
        push(
            &mut checks,
            "VAL-2",
            Some((0.30..=0.58).contains(&mean) && p95 - p5 <= 0.30),
            format!("bare ground mean L {mean:.3}, p5-p95 {:.3}", p95 - p5),
            "mean OKLab L 0.30-0.58, p5-p95 <= 0.30 on the ground outside every model's 3x box \
             (exposure is set to 0.50, so the spread is the test)",
        );
    }
    let mut shaded: Vec<f32> = (0..beauty.len())
        .filter(|&index| ring_ground[index])
        .map(|index| display_l(&beauty, index))
        .collect();
    match imaging::percentile(&mut shaded, 1.0) {
        Some(p1) => push(
            &mut checks,
            "VAL-3",
            Some(p1 >= 0.18),
            format!("p1 L {p1:.3}"),
            "shadow floor L >= 0.18 (p1 over the ground in the models' 3x boxes, cast shadows \
             included)",
        ),
        None => push(
            &mut checks,
            "VAL-3",
            Some(false),
            "no ground in the models' 3x boxes".into(),
            "shadow floor L >= 0.18",
        ),
    }
    let mut plain: Vec<f32> = (0..beauty.len())
        .filter(|&index| near[index] && !post.bits[index] && !emissive_px(index))
        .map(|index| display_l(&beauty, index))
        .collect();
    let p99 = imaging::percentile(&mut plain, 99.0).unwrap_or(0.0);
    push(
        &mut checks,
        "VAL-4",
        Some(p99 <= 0.85),
        format!("p99 L {p99:.3}"),
        "non-emissive highlight ceiling L <= 0.85 (p99 over the models and their 3x boxes)",
    );
    let screen = all_emissive as f64 / beauty.len() as f64;
    push(
        &mut checks,
        "PBR-EMIT-3",
        Some(screen <= 0.02),
        format!("{:.3} % of the frame", screen * 100.0),
        "static emissive screen share <= 2 % at 46 m (this lineup, not a map capture)",
    );

    Ok(checks)
}

#[allow(clippy::too_many_lines)] // SIL-1 to SIL-8 in order.
fn silhouette_checks(meta: &Meta, dir: &Path, checks: &mut Vec<Check>) -> Result<(), String> {
    let mut heroes_1080: Vec<(String, SilhouetteSet)> = Vec::new();
    for model in &meta.models {
        let name = &model.name;
        // `glb` is relative to render_meta.json (older metas hold absolute paths; join keeps them).
        let glb = dir.join(&model.glb);
        let mesh = moba_assets::load_glb_file_with(&glb, moba_assets::GlbTangents::FileOnly)
            .map_err(|error| format!("{}: {error}", glb.display()))?;
        let (low, high) = mesh.bounds();
        let height = f64::from(high[1] - low[1]);
        match SIZE_CLASSES
            .iter()
            .find(|(key, _, _)| *key == model.size_class)
        {
            Some((key, lo, hi)) => push(
                checks,
                &format!("SIL-1 ({name})"),
                Some(height >= lo - 1e-3 && height <= hi + 1e-3),
                format!("{height:.2} m"),
                &format!("{key} {lo:.1}-{hi:.1} m (glb bounds)"),
            ),
            None => push(
                checks,
                &format!("SIL-1 ({name})"),
                None,
                format!("{height:.2} m"),
                "no size class declared (information)",
            ),
        }
        if model.role == "minion" && model.size_class != "minion-siege" {
            push(
                checks,
                &format!("SIL-2 ({name})"),
                Some(height <= 1.6 + 1e-3),
                format!("{height:.2} m"),
                "minion <= 0.8x a Hero M (1.6 m); siege exempt",
            );
        }
        let find = |res: &str| {
            meta.frames
                .silhouettes
                .iter()
                .find(|set| set.model == *name && set.res == res)
                .ok_or_else(|| format!("no {res} silhouettes for {name}"))
        };
        let s1080 = find("1080p")?;
        let s1440 = find("1440p")?;
        let s4x = find("1080p4x")?;
        if s1080.files.len() != 8 || s1440.files.len() != 8 || s4x.files.len() != 8 {
            return Err(format!("{name}: silhouettes need 8 yaws"));
        }
        let set1080 = load_silhouettes(dir, &s1080.files)?;
        let set1440 = load_silhouettes(dir, &s1440.files)?;
        let set4x = load_silhouettes(dir, &s4x.files)?;
        let yaws = set1080.whole.len();

        // SIL-5 / SIL-6: faction tells on units.
        if is_unit(model) && model.faction == "forge" {
            if model.tell.is_empty() {
                push(
                    checks,
                    &format!("SIL-5 ({name})"),
                    None,
                    "no `tell` joints declared".into(),
                    "Forge radial element >= 8 % of the silhouette at 1440p (human until declared)",
                );
            } else {
                let shares: Vec<f64> = (0..yaws)
                    .map(|k| {
                        set1440.tell[k].count() as f64 / set1440.whole[k].count().max(1) as f64
                    })
                    .collect();
                let good = shares.iter().filter(|share| **share >= 0.08).count();
                let low = shares.iter().copied().fold(f64::INFINITY, f64::min);
                push(
                    checks,
                    &format!("SIL-5 ({name})"),
                    Some(good == yaws),
                    format!("{good}/8 yaws, min {:.1} %", low * 100.0),
                    "radial tell >= 8 % of the silhouette at 1440p, every yaw",
                );
            }
        }
        if is_unit(model) && model.faction == "void" {
            let counts: Vec<usize> = set1440.whole.iter().map(Mask::void_tell_count).collect();
            let good = counts.iter().filter(|count| **count >= 2).count();
            push(
                checks,
                &format!("SIL-6 ({name})"),
                Some(good == yaws),
                format!("{good}/8 yaws, counts {counts:?}"),
                ">= 2 enclosed gaps or floating parts, each >= 2 px, at 1440p, every yaw",
            );
        }
        // SIL-7: weapons.
        if !model.weapon.is_empty() {
            if model.weapon_kind == "ranged" {
                let seen = set1440
                    .weapon
                    .iter()
                    .filter(|mask| mask.count() > 0)
                    .count();
                push(
                    checks,
                    &format!("SIL-7 ({name})"),
                    Some(seen >= RANGED_YAWS),
                    format!("visible from {seen}/8 yaws"),
                    "ranged weapon visible from >= 6 of 8 yaws",
                );
            } else {
                let growth: Vec<f64> = (0..yaws)
                    .map(|k| {
                        let whole = &set1440.whole[k];
                        let body = Mask {
                            width: whole.width,
                            height: whole.height,
                            bits: whole
                                .bits
                                .iter()
                                .zip(&set1440.weapon[k].bits)
                                .map(|(w, x)| *w && !*x)
                                .collect(),
                        };
                        match (whole.bbox(), body.bbox()) {
                            (Some(a), Some(b)) => {
                                let gw = (a[2] - a[0]) as f64 / (b[2] - b[0]).max(1) as f64;
                                let gh = (a[3] - a[1]) as f64 / (b[3] - b[1]).max(1) as f64;
                                gw.max(gh) - 1.0
                            }
                            _ => 0.0,
                        }
                    })
                    .collect();
                let good = growth.iter().filter(|g| **g >= 0.15).count();
                push(
                    checks,
                    &format!("SIL-7 ({name})"),
                    Some(good == yaws),
                    format!(
                        "{good}/8 yaws, max +{:.0} %",
                        growth.iter().copied().fold(0.0, f64::max) * 100.0
                    ),
                    "melee weapon extends the bbox >= 15 % (width or height) at 1440p, every yaw",
                );
            }
        }
        // SIL-8: parts under 1 px at 1080p, measured at 4x4 samples per pixel.
        let hair: Vec<usize> = set4x
            .whole
            .iter()
            .map(|mask| hair_parts(mask, SIL8_SAMPLES).0)
            .collect();
        let clean = hair.iter().filter(|parts| **parts == 0).count();
        push(
            checks,
            &format!("SIL-8 ({name})"),
            Some(clean == yaws),
            format!("{clean}/8 yaws clean, parts per yaw {hair:?}"),
            "no part under 1 px wide at 1080p (>= 2 px long; 4x4 samples per pixel), every yaw",
        );
        if model.role == "hero" {
            heroes_1080.push((name.clone(), set1080));
        }
    }
    // SIL-3 between every pair of heroes.
    if heroes_1080.len() < 2 {
        push(
            checks,
            "SIL-3",
            None,
            format!("{} hero(es)", heroes_1080.len()),
            "IoU < 0.80 between any two heroes (needs >= 2 heroes in one review)",
        );
    }
    for (index, (a_name, a)) in heroes_1080.iter().enumerate() {
        for (b_name, b) in &heroes_1080[index + 1..] {
            let worst = (0..a.whole.len().min(b.whole.len()))
                .map(|k| best_iou(&a.whole[k], &b.whole[k], 3))
                .fold(0.0, f64::max);
            push(
                checks,
                &format!("SIL-3 ({a_name} / {b_name})"),
                Some(worst < 0.80),
                format!("max IoU {worst:.3}"),
                "IoU < 0.80 at 1080p, feet-aligned +-3 px, every yaw",
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thin_lines_are_found_and_staircases_are_not() {
        let mask = Mask::from_rows(&[
            "..........",
            ".####.#...",
            ".####.#...",
            ".####.#...",
            ".####.#..#",
            "..........",
        ]);
        let (parts, thin) = hair_parts(&mask, 2);
        assert_eq!(
            (parts, thin),
            (1, 5),
            "the 4 px antenna is a part; the lone pixel a tip"
        );
        let stairs = Mask::from_rows(&["#.....", "##....", "###...", "####..", "#####.", "######"]);
        assert_eq!(hair_parts(&stairs, 2).0, 0, "a 45 deg edge is not thin");
        // At 4 samples per checked pixel: a 3-sample-wide strut is under 1 px, a 4-wide is not.
        let narrow = Mask::from_rows(&[".###."; 12]);
        let wide = Mask::from_rows(&[".####."; 12]);
        assert_eq!(hair_parts(&narrow, 4).0, 1);
        assert_eq!(hair_parts(&wide, 4), (0, 0));
    }

    #[test]
    fn void_tell_counts_holes_and_floating_parts_two_px_wide() {
        let mask = Mask::from_rows(&[
            "............",
            ".######..##.",
            ".#..###..##.",
            ".#..###.....",
            ".######..#..",
            "............",
        ]);
        // One 2x2 hole, one 2x2 floating part; the single floating pixel does not count.
        assert_eq!(mask.void_tell_count(), 2);
    }

    #[test]
    fn iou_aligns_at_the_feet() {
        let a = Mask::from_rows(&["......", ".##...", ".##...", "......"]);
        let b = Mask::from_rows(&["......", "......", "...##.", "...##."]);
        assert!((best_iou(&a, &b, 0) - 1.0).abs() < 1e-9);
        let c = Mask::from_rows(&["......", "...#..", "...#..", "......"]);
        assert!((best_iou(&a, &c, 1) - 0.5).abs() < 1e-9);
    }

    fn write_rgb(
        path: &Path,
        width: u32,
        height: u32,
        sixteen: bool,
        pixel: impl Fn(u32, u32) -> [u16; 3],
    ) {
        let file = std::fs::File::create(path).expect("create");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(if sixteen {
            png::BitDepth::Sixteen
        } else {
            png::BitDepth::Eight
        });
        let mut writer = encoder.write_header().expect("header");
        let mut bytes = Vec::new();
        for y in 0..height {
            for x in 0..width {
                for value in pixel(x, y) {
                    if sixteen {
                        bytes.extend_from_slice(&value.to_be_bytes());
                    } else {
                        bytes.push(value as u8);
                    }
                }
            }
        }
        writer.write_image_data(&bytes).expect("data");
    }

    /// A `render_meta.json` for the frame tests: hero `h` (red ids), minion `m` (green ids).
    fn test_meta() -> Meta {
        let l_ref = crate::meta::l_ref();
        let frame = |file: &str| {
            serde_json::json!({"file": file, "width": 2560, "height": 1440, "distance_m": 46.0,
                               "models": [], "post_bbox": [0, 0, 1, 1], "ids": "i.png", "emit": "e.png"})
        };
        let model = |name: &str, role: &str| {
            serde_json::json!({"name": name, "glb": "unused.glb", "role": role, "faction": "forge",
                               "pose": {"clip": "", "frame": 0, "applied": false}})
        };
        serde_json::from_value(serde_json::json!({
            "schema": 1, "name": "t", "renderer": "Cycles", "device": "CPU", "blender": "5.2.3",
            "samples": 1,
            "camera": {"yaw": -0.7, "pitch": 0.95, "distance_m": 46.0, "zoom_distance_m": 8.0, "fov_y_deg": 60.0},
            "sun": {"dir": [-0.885, 0.532, 0.465], "colour": [1.0, 0.8, 0.62], "intensity": 1.5, "elevation_deg": 28.0},
            "sky": {"ibl": 0.35, "zenith": [0.0, 0.0, 0.0], "horizon": [0.0, 0.0, 0.0], "ground": [0.0, 0.0, 0.0]},
            "ground_hex": "#6B6560", "lane_hex": "#906B58", "l_ref": l_ref, "post_m": 1.8,
            "view_transform": "AgX", "exposure_ev": 0.0, "ground_l_target": 0.5, "ground_l_calibrated": 0.5,
            "id_colours": {"h": [1, 0, 0], "m": [0, 1, 0]}, "post_colour": [1, 1, 1],
            "models": [model("h", "hero"), model("m", "minion")],
            "frames": {"46m_1440p": frame("b.png"), "46m_2160p": frame("b.png"), "zoom": [],
                       "lineup_1440p": frame("b.png"), "turntable": [], "silhouettes": []},
        }))
        .expect("meta")
    }

    #[test]
    fn frame_checks_measure_contrast_and_emission() {
        let dir = std::env::temp_dir().join(format!("moba-review-sheet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let hero = |x: u32, y: u32| (1000..1040).contains(&x) && (600..680).contains(&y);
        let minion = |x: u32, y: u32| (1400..1430).contains(&x) && (620..680).contains(&y);
        // Ground sRGB 120; the hero 60 (dark: CON-1 passes); the minion 118 (CON-4 fails).
        write_rgb(&dir.join("b.png"), 2560, 1440, false, |x, y| {
            let v = if hero(x, y) {
                60
            } else if minion(x, y) {
                118
            } else {
                120
            };
            [v; 3]
        });
        write_rgb(&dir.join("i.png"), 2560, 1440, false, |x, y| {
            if hero(x, y) {
                [255, 0, 0]
            } else if minion(x, y) {
                [0, 255, 0]
            } else {
                [0; 3]
            }
        });
        // 32 of the hero's 3200 pixels glow at 1.5 L_ref (+0.58 EV): 1 % of its area.
        let l_ref = crate::meta::l_ref();
        let glow = (1.5 * l_ref * 65535.0).round() as u16;
        write_rgb(&dir.join("e.png"), 2560, 1440, true, |x, y| {
            if hero(x, y) && y == 600 && x < 1032 {
                [glow; 3]
            } else {
                [0; 3]
            }
        });
        let meta = test_meta();
        let checks = frame_checks(&meta, &dir).expect("frame checks");
        let _ = std::fs::remove_dir_all(&dir);
        let outcome = |rule: &str| {
            checks
                .iter()
                .find(|check| check.rule == rule)
                .map(|check| check.pass)
        };
        assert_eq!(outcome("CON-1 (h)"), Some(Some(true)));
        assert_eq!(outcome("CON-4 (m)"), Some(Some(false)));
        assert_eq!(outcome("PBR-EMIT-2r (h)"), Some(Some(true)));
        assert_eq!(outcome("PBR-EMIT-1r (h)"), Some(Some(true)));
        assert_eq!(outcome("PBR-EMIT-1r (m)"), Some(None));
        assert_eq!(outcome("VAL-2"), Some(Some(true)));
        assert_eq!(outcome("VAL-3"), Some(Some(true)));
        let emit = checks
            .iter()
            .find(|check| check.rule == "PBR-EMIT-1r (h)")
            .expect("EMIT-1r");
        assert!(emit.value.contains("+0.58"), "{}", emit.value);
        let area = checks
            .iter()
            .find(|check| check.rule == "PBR-EMIT-2r (h)")
            .expect("EMIT-2r");
        assert!(area.value.starts_with("1.00 %"), "{}", area.value);
    }

    #[test]
    fn a_model_with_no_ground_around_it_fails_contrast_and_value() {
        let dir =
            std::env::temp_dir().join(format!("moba-review-sheet-ring-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        // The hero fills the frame: no ring, no bare ground, no ground in its box.
        write_rgb(&dir.join("b.png"), 2560, 1440, false, |_, _| [60; 3]);
        write_rgb(&dir.join("i.png"), 2560, 1440, false, |_, _| [255, 0, 0]);
        write_rgb(&dir.join("e.png"), 2560, 1440, true, |_, _| [0; 3]);
        let checks = frame_checks(&test_meta(), &dir).expect("frame checks");
        let _ = std::fs::remove_dir_all(&dir);
        let find = |rule: &str| {
            checks
                .iter()
                .find(|check| check.rule == rule)
                .unwrap_or_else(|| panic!("no {rule}"))
        };
        assert_eq!(find("CON-1 (h)").pass, Some(false));
        assert!(find("CON-1 (h)").value.contains("no ground ring"));
        assert_eq!(find("VAL-2").pass, Some(false));
        assert_eq!(find("VAL-3").pass, Some(false));
    }

    #[test]
    fn size_classes_match_the_bible() {
        let find = |key: &str| SIZE_CLASSES.iter().find(|(k, _, _)| *k == key).unwrap();
        assert_eq!((find("hero-m").1, find("hero-m").2), (1.8, 2.0));
        assert_eq!((find("minion-siege").1, find("minion-siege").2), (2.2, 2.6));
        assert_eq!((find("core").1, find("core").2), (18.0, 24.0));
    }
}
