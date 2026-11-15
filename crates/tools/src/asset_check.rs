//! `content-check --source`: the art bible's material rules and the roadmap's texel-density
//! targets on one asset's **source** maps, before KTX2 encoding (A1, ADR-0022 amendment 2).
//!
//! The rules and every threshold come from `docs/art/material-vocabulary.md` §1.1, §2 and §6,
//! `docs/art/palette-and-value.md` (PAL-1, PAL-2, PAL-4, PAL-5, VAL-1) and roadmap §9.1
//! (texel density). The asset's source manifest (`<asset>.source.toml`, kept with the art
//! sources and never shipped) names the maps and the source-side class-ID map the bible decided
//! on (material-vocabulary §2, decision 5). See `docs/content-authoring.md`, "Source checks".

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::imaging::{
    Image, luma, luminance, oklch_display, parse_hex, percentile, read_png, round, srgb_to_linear,
    std_dev,
};

/// Largest source manifest read, in bytes.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// Most texel visits the UV rasteriser makes (tiling UVs are wrapped, not unbounded).
const MAX_RASTER_VISITS: u64 = 1 << 28;
/// The team hues (palette-and-value §1): standard and colour-blind self, ally and enemy.
pub(crate) const TEAM_HUES: [f32; 6] = [28.0, 77.0, 105.0, 142.0, 236.0, 259.0];
/// Half-width of a reserved team-colour zone, in degrees (palette-and-value §1).
pub(crate) const TEAM_ZONE_DEG: f32 = 25.0;
/// Golden Crossing's lane grit centre (palette-and-value §6), the albedo `L_ref` is defined on.
pub(crate) const LANE_GRIT_HEX: &str = "#906B58";
/// Golden Crossing's `[lighting] sun_intensity` (lighting-mood.md, option A).
pub(crate) const GOLDEN_SUN_INTENSITY: f32 = 1.5;
/// Golden Crossing's sun elevation in degrees (lighting-mood.md, option A).
pub(crate) const GOLDEN_SUN_ELEVATION_DEG: f32 = 28.0;

/// What an asset is, which picks its thresholds.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AssetClass {
    /// A hero (PBR-RGH-2's 0.08 floor, PBR-EMIT-2's 5 %, PBR-WEAR-1).
    Hero,
    /// A lane minion (as a hero).
    Minion,
    /// A neutral monster: a unit for the emissive and dust limits; the bible has no size or
    /// texture class for it.
    Neutral,
    /// A tower, core or wall (PBR-EMIT-2's 10 %; texel density 256–512 px/m).
    Structure,
    /// An environment prop (PBR-EMIT-2's 3 %; texel density 128–256 px/m).
    Prop,
    /// A terrain layer (PAL-1's ground chroma, VAL-1; texel density 256–512 px/m).
    Terrain,
}

impl AssetClass {
    fn is_unit(self) -> bool {
        matches!(self, Self::Hero | Self::Minion | Self::Neutral)
    }

    /// Roadmap §9.1's texel density in px/m, where it gives one.
    fn density_target(self) -> Option<(f32, f32)> {
        match self {
            Self::Structure | Self::Terrain => Some((256.0, 512.0)),
            Self::Prop => Some((128.0, 256.0)),
            Self::Hero | Self::Minion | Self::Neutral => None,
        }
    }

    /// Roadmap §9.1's largest authored texture side, where it gives one.
    fn max_texture_side(self) -> Option<u32> {
        match self {
            Self::Hero => Some(2048),
            Self::Minion => Some(1024),
            _ => None,
        }
    }

    /// PBR-EMIT-2's emissive area cap (material-vocabulary §1.1).
    fn emissive_area_cap(self) -> f32 {
        match self {
            Self::Hero | Self::Minion | Self::Neutral => 0.05,
            Self::Structure => 0.10,
            Self::Prop | Self::Terrain => 0.03,
        }
    }
}

/// One material class of material-vocabulary §2: albedo luma, roughness and metalness.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ClassRange {
    /// The class key used in manifests (`METAL`, `STONE-1`, …).
    pub(crate) key: &'static str,
    /// Its §2 rule id.
    pub(crate) rule: &'static str,
    /// Albedo luma, sRGB 0..=255 (p1, p99).
    pub(crate) albedo: (f32, f32),
    /// Perceptual roughness of the base layer (p1, p99).
    pub(crate) roughness: (f32, f32),
    /// Metalness: `Some(true)` metal, `Some(false)` dielectric, `None` either (paint, worn
    /// through).
    pub(crate) metal: Option<bool>,
}

/// Material-vocabulary §2, one row per class. Clear-coat ranges are not here: the source maps
/// hold the base layer only (the clear coat arrives with the §6.6 material model).
pub(crate) const CLASSES: [ClassRange; 15] = [
    class(
        "METAL",
        "PBR-METAL-1",
        (180.0, 255.0),
        (0.15, 0.70),
        Some(true),
    ),
    class(
        "OXIDE",
        "PBR-OXIDE-1",
        (30.0, 140.0),
        (0.55, 1.0),
        Some(false),
    ),
    class("PAINT", "PBR-PAINT-1", (30.0, 240.0), (0.35, 0.85), None),
    class(
        "STONE-1",
        "PBR-STONE-1",
        (35.0, 210.0),
        (0.55, 0.95),
        Some(false),
    ),
    class(
        "STONE-2",
        "PBR-STONE-2",
        (30.0, 235.0),
        (0.05, 0.35),
        Some(false),
    ),
    class(
        "CERAM",
        "PBR-CERAM-1",
        (40.0, 230.0),
        (0.70, 0.95),
        Some(false),
    ),
    class(
        "SOIL",
        "PBR-SOIL-1",
        (50.0, 200.0),
        (0.85, 1.0),
        Some(false),
    ),
    class(
        "CHAR",
        "PBR-CHAR-1",
        (30.0, 170.0),
        (0.85, 1.0),
        Some(false),
    ),
    class(
        "GLASS",
        "PBR-GLASS-1",
        (30.0, 240.0),
        (0.045, 0.30),
        Some(false),
    ),
    class(
        "SKIN",
        "PBR-SKIN-1",
        (60.0, 220.0),
        (0.35, 0.60),
        Some(false),
    ),
    class(
        "HAIR",
        "PBR-HAIR-1",
        (30.0, 200.0),
        (0.30, 0.60),
        Some(false),
    ),
    class(
        "CLOTH",
        "PBR-CLOTH-1",
        (30.0, 230.0),
        (0.70, 1.0),
        Some(false),
    ),
    class(
        "LEATHER",
        "PBR-LEATHER-1",
        (30.0, 180.0),
        (0.40, 0.85),
        Some(false),
    ),
    class(
        "WOOD",
        "PBR-WOOD-1",
        (35.0, 200.0),
        (0.50, 0.95),
        Some(false),
    ),
    class(
        "BONE",
        "PBR-BONE-1",
        (150.0, 235.0),
        (0.40, 0.95),
        Some(false),
    ),
];

const fn class(
    key: &'static str,
    rule: &'static str,
    albedo: (f32, f32),
    roughness: (f32, f32),
    metal: Option<bool>,
) -> ClassRange {
    ClassRange {
        key,
        rule,
        albedo,
        roughness,
        metal,
    }
}

/// The §2 row for `key`.
pub(crate) fn class_range(key: &str) -> Option<&'static ClassRange> {
    CLASSES.iter().find(|range| range.key == key)
}

/// Rules of the bible that `content-check --source` does not build yet, with the reason.
const NOT_BUILT: [(&str, &str); 10] = [
    (
        "PBR-VOCAB-1",
        "needs the §3 vocabulary as data (checks.md step 5)",
    ),
    (
        "PBR-FORGE-1",
        "needs the §3 vocabulary entry per class (checks.md step 5)",
    ),
    (
        "PBR-VOID-1",
        "needs the §3 vocabulary entry per class (checks.md step 5)",
    ),
    (
        "PBR-VOID-2",
        "needs the §3 vocabulary entry per class (checks.md step 5)",
    ),
    ("LANE-1", "needs the §3 vocabulary entry (checks.md step 5)"),
    ("PBR-HEAT-1", "a review-render judgement (checks.md)"),
    ("EMB-1", "needs the §3 vocabulary entry (checks.md step 5)"),
    (
        "PBR-GLASS-1",
        "IOR is not in the material format until the §6.6 material model",
    ),
    (
        "PBR-SKIN-1",
        "IOR and SSS radius are not in the material format until §6.6",
    ),
    ("PAL-3", "needs an accent mask; a review-render rule"),
];

/// `<asset>.source.toml`: an asset's source maps and declarations (never shipped).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceManifest {
    /// Asset (texture set) name: 1..=16 of `[A-Za-z0-9_-]`.
    pub(crate) asset: String,
    /// What the asset is.
    pub(crate) class: AssetClass,
    /// The exported `.glb` files this texture set is for (UV coverage, texel density and the
    /// export checks): one, or several sharing an atlas.
    #[serde(default)]
    pub(crate) models: Vec<ModelDecl>,
    /// Terrain layers: the tile size in metres (texel density without a model).
    pub(crate) tile_m: Option<f32>,
    /// The side the textures ship at, if not the source maps' (texel density, §9.1 sizes).
    pub(crate) shipped_size: Option<u32>,
    /// The source maps.
    pub(crate) maps: SourceMaps,
    /// 8-bit code value of one class id step in the class-ID map (`id = round(R / step)`).
    #[serde(default = "default_class_step")]
    pub(crate) class_step: u32,
    /// Class id → §2 class key (`"1" = "METAL"`).
    #[serde(default)]
    pub(crate) classes: BTreeMap<String, String>,
    /// Without a class-ID map: the one class every texel belongs to (terrain layers).
    pub(crate) material_class: Option<String>,
    /// The class id painted as the dust layer (PBR-WEAR-1 on units).
    pub(crate) dust_class: Option<u32>,
    /// The emissive colour and strength (with `maps.emissive`).
    pub(crate) emissive: Option<EmissiveDecl>,
    /// The map lighting `L_ref` is taken from (default: Golden Crossing).
    #[serde(default)]
    pub(crate) lighting: LightingDecl,
}

fn default_class_step() -> u32 {
    1
}

/// One exported model in a [`SourceManifest`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModelDecl {
    /// The `.glb`, relative to the manifest.
    pub(crate) path: String,
    /// Documented hover height of the lowest vertex, in metres (default 0: feet on y = 0).
    #[serde(default)]
    pub(crate) hover_m: f32,
}

/// The maps of a [`SourceManifest`], relative to the manifest's directory.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceMaps {
    /// Albedo, sRGB.
    pub(crate) base: String,
    /// Packed metallic (R) and roughness (G), the engine's packing.
    pub(crate) metallic_roughness: Option<String>,
    /// Metallic in R (instead of the packed map).
    pub(crate) metallic: Option<String>,
    /// Roughness in R (instead of the packed map).
    pub(crate) roughness: Option<String>,
    /// Tangent-space normal, +Y up: XYZ (or XY) in RGB.
    pub(crate) normal: Option<String>,
    /// Emissive mask in R.
    pub(crate) emissive: Option<String>,
    /// Baked ambient occlusion in R (PBR-DELIGHT-1).
    pub(crate) ao: Option<String>,
    /// Class-ID map in R (PBR-CLASS-1).
    pub(crate) class: Option<String>,
    /// Team mask in R (TINT-1).
    pub(crate) team_mask: Option<String>,
}

/// The emissive declaration: what `materials/<asset>.toml` would carry.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EmissiveDecl {
    /// Display colour, `#RRGGBB` (PAL-4, PAL-5).
    pub(crate) colour: String,
    /// Linear multiplier: the material's `emissive` is the colour (linear) times this.
    pub(crate) strength: f32,
    /// `accent` (≤ +1 EV) or `gameplay` (≤ +2 EV), PBR-EMIT-1.
    #[serde(rename = "use")]
    pub(crate) usage: EmissiveUse,
}

/// PBR-EMIT-1's two caps.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum EmissiveUse {
    /// A faction accent: ≤ +1 EV over `L_ref`.
    Accent,
    /// A gameplay emissive: ≤ +2 EV.
    Gameplay,
}

/// The lighting `L_ref` (material-vocabulary §1.1) is computed from.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LightingDecl {
    /// `[lighting] sun_intensity`.
    pub(crate) sun_intensity: f32,
    /// Sun elevation in degrees.
    pub(crate) sun_elevation_deg: f32,
    /// The lane albedo, `#RRGGBB`.
    pub(crate) lane: String,
}

impl Default for LightingDecl {
    fn default() -> Self {
        Self {
            sun_intensity: GOLDEN_SUN_INTENSITY,
            sun_elevation_deg: GOLDEN_SUN_ELEVATION_DEG,
            lane: String::from(LANE_GRIT_HEX),
        }
    }
}

impl LightingDecl {
    /// `L_ref = lane albedo (linear) × sun_intensity × sin(sun elevation)`.
    pub(crate) fn l_ref(&self) -> Result<f32, String> {
        let lane = parse_hex(&self.lane).ok_or_else(|| format!("bad lane colour {}", self.lane))?;
        Ok(luminance(lane.map(srgb_to_linear))
            * self.sun_intensity
            * self.sun_elevation_deg.to_radians().sin())
    }
}

/// One rule's outcome.
#[derive(Clone, Debug)]
pub(crate) struct RuleResult {
    /// Rule id (`PBR-ALB-1`, `TD-1`, …).
    pub(crate) rule: String,
    /// `Some(pass)`, or `None` for a report-only line or a rule that does not apply.
    pub(crate) pass: Option<bool>,
    /// The measured value.
    pub(crate) value: String,
    /// The threshold or a note.
    pub(crate) limit: String,
}

/// Everything `content-check --source` measured.
#[derive(Debug, Default)]
pub(crate) struct SourceReport {
    /// Asset name.
    pub(crate) asset: String,
    /// Rule outcomes in check order.
    pub(crate) rules: Vec<RuleResult>,
    /// Share of the base map covered by the model's UVs.
    pub(crate) uv_coverage: Option<f64>,
    /// Class key → share of covered surface.
    pub(crate) class_share: BTreeMap<String, f64>,
}

impl SourceReport {
    fn push(&mut self, rule: &str, pass: Option<bool>, value: String, limit: impl Into<String>) {
        self.rules.push(RuleResult {
            rule: rule.to_string(),
            pass,
            value,
            limit: limit.into(),
        });
    }

    /// Whether no rule failed.
    pub(crate) fn passed(&self) -> bool {
        self.rules.iter().all(|rule| rule.pass != Some(false))
    }

    /// The report as plain text, one rule per line.
    pub(crate) fn text(&self) -> String {
        let mut out = String::new();
        for rule in &self.rules {
            let status = match rule.pass {
                Some(true) => "PASS",
                Some(false) => "FAIL",
                None => "info",
            };
            let _ = writeln!(
                out,
                "{status}  {:<22} {:<28} {}",
                rule.rule, rule.value, rule.limit
            );
        }
        if let Some(coverage) = self.uv_coverage {
            let _ = writeln!(out, "uv coverage {coverage}");
        }
        if !self.class_share.is_empty() {
            let shares: Vec<String> = self
                .class_share
                .iter()
                .map(|(class, share)| format!("{class} {share}"))
                .collect();
            let _ = writeln!(out, "class share (surface) {}", shares.join(", "));
        }
        out
    }

    /// The report as JSON.
    pub(crate) fn json(&self) -> serde_json::Value {
        let rules: Vec<serde_json::Value> = self
            .rules
            .iter()
            .map(|rule| {
                serde_json::json!({
                    "rule": rule.rule,
                    "pass": rule.pass,
                    "value": rule.value,
                    "limit": rule.limit,
                })
            })
            .collect();
        serde_json::json!({
            "asset": self.asset,
            "pass": self.passed(),
            "uv_coverage": self.uv_coverage,
            "class_share": self.class_share,
            "rules": rules,
        })
    }
}

/// Reads and parses a source manifest.
pub(crate) fn read_manifest(path: &Path) -> Result<SourceManifest, String> {
    let shown = path.display();
    let metadata = std::fs::metadata(path).map_err(|error| format!("{shown}: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES {
        return Err(format!(
            "{shown}: not a regular file of at most {MAX_MANIFEST_BYTES} bytes"
        ));
    }
    let text = std::fs::read_to_string(path).map_err(|error| format!("{shown}: {error}"))?;
    let manifest: SourceManifest =
        toml::from_str(&text).map_err(|error| format!("{shown}: {error}"))?;
    if !valid_name(&manifest.asset, 16) {
        return Err(format!(
            "{shown}: asset name {:?} is not 1..=16 of [A-Za-z0-9_-]",
            manifest.asset
        ));
    }
    if manifest.class_step == 0 || manifest.class_step > 255 {
        return Err(format!("{shown}: class_step must be 1..=255"));
    }
    Ok(manifest)
}

fn valid_name(name: &str, max: usize) -> bool {
    !name.is_empty()
        && name.len() <= max
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A model's triangles in metres and UVs, as the engine's loader reads them.
struct ModelGeometry {
    name: String,
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

/// Texels the model's UVs cover, and the model surface (m²) each one stands for.
#[derive(Debug)]
pub(crate) struct Coverage {
    /// Per texel: covered surface in m² (0 = not covered).
    pub(crate) area: Vec<f32>,
}

impl Coverage {
    fn covered(&self, index: usize) -> bool {
        self.area[index] > 0.0
    }
}

/// Rasterises UV triangles (glTF convention: `(0, 0)` is the image's top-left) at texel centres,
/// wrapping tiling UVs, and spreads each triangle's world area over its texels.
#[allow(clippy::many_single_char_names)] // Geometry: p, t, w, h, x, y.
pub(crate) fn rasterise_uvs(
    positions: &[[f32; 3]],
    uvs: &[[f32; 2]],
    indices: &[u32],
    width: u32,
    height: u32,
) -> Result<Coverage, String> {
    let (w, h) = (f64::from(width), f64::from(height));
    let mut area = vec![0f32; width as usize * height as usize];
    let mut visits = 0u64;
    let mut texels: Vec<usize> = Vec::new();
    for triangle in indices.as_chunks::<3>().0 {
        let corner = |k: usize| triangle[k] as usize;
        let p = [corner(0), corner(1), corner(2)].map(|i| positions[i].map(f64::from));
        let t = [corner(0), corner(1), corner(2)].map(|i| {
            let [u, v] = uvs[i];
            [f64::from(u) * w, f64::from(v) * h]
        });
        let world = triangle_area_3d(&p);
        if world <= 0.0 || !world.is_finite() {
            continue;
        }
        let min_x = t.iter().map(|q| q[0]).fold(f64::INFINITY, f64::min);
        let max_x = t.iter().map(|q| q[0]).fold(f64::NEG_INFINITY, f64::max);
        let min_y = t.iter().map(|q| q[1]).fold(f64::INFINITY, f64::min);
        let max_y = t.iter().map(|q| q[1]).fold(f64::NEG_INFINITY, f64::max);
        if !(min_x.is_finite() && max_x.is_finite() && min_y.is_finite() && max_y.is_finite()) {
            continue;
        }
        let (x0, x1) = ((min_x - 0.5).ceil() as i64, (max_x - 0.5).floor() as i64);
        let (y0, y1) = ((min_y - 0.5).ceil() as i64, (max_y - 0.5).floor() as i64);
        let span = (x1 - x0 + 1).max(0) as u64 * (y1 - y0 + 1).max(0) as u64;
        visits += span;
        if visits > MAX_RASTER_VISITS {
            return Err(String::from(
                "the model's UVs tile too far to rasterise (over 2^28 texel visits)",
            ));
        }
        let signed = edge(t[0], t[1], t[2]);
        texels.clear();
        if signed.abs() > 1e-12 {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let c = [x as f64 + 0.5, y as f64 + 0.5];
                    let e0 = edge(t[1], t[2], c) * signed.signum();
                    let e1 = edge(t[2], t[0], c) * signed.signum();
                    let e2 = edge(t[0], t[1], c) * signed.signum();
                    if e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0 {
                        let tx = x.rem_euclid(i64::from(width)) as usize;
                        let ty = y.rem_euclid(i64::from(height)) as usize;
                        texels.push(ty * width as usize + tx);
                    }
                }
            }
        }
        if texels.is_empty() {
            // A sliver smaller than a texel: credit the texel under its centroid.
            let cx = ((t[0][0] + t[1][0] + t[2][0]) / 3.0).floor() as i64;
            let cy = ((t[0][1] + t[1][1] + t[2][1]) / 3.0).floor() as i64;
            let tx = cx.rem_euclid(i64::from(width)) as usize;
            let ty = cy.rem_euclid(i64::from(height)) as usize;
            texels.push(ty * width as usize + tx);
        }
        let share = (world / texels.len() as f64) as f32;
        for &texel in &texels {
            area[texel] += share;
        }
    }
    Ok(Coverage { area })
}

fn edge(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn triangle_area_3d(p: &[[f64; 3]; 3]) -> f64 {
    let u = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
    let v = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
    let cross = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    0.5 * (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt()
}

/// Texel density percentiles in px/m: `(p5, p50, p95)`.
pub(crate) type Density = (f32, f32, f32);

/// Area-weighted texel density in px/m over the model's triangles: `(p5, p50, p95)`.
pub(crate) fn texel_density(
    positions: &[[f32; 3]],
    uvs: &[[f32; 2]],
    indices: &[u32],
    width: u32,
    height: u32,
) -> Option<Density> {
    let mut samples: Vec<(f32, f64)> = Vec::new();
    for triangle in indices.as_chunks::<3>().0 {
        let p = [0, 1, 2].map(|k| positions[triangle[k] as usize].map(f64::from));
        let world = triangle_area_3d(&p);
        let t = [0, 1, 2].map(|k| {
            let [u, v] = uvs[triangle[k] as usize];
            [
                f64::from(u) * f64::from(width),
                f64::from(v) * f64::from(height),
            ]
        });
        let texture = 0.5 * edge(t[0], t[1], t[2]).abs();
        if world > 1e-12 && texture.is_finite() {
            samples.push(((texture / world).sqrt() as f32, world));
        }
    }
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = samples.iter().map(|s| s.1).sum();
    let at = |q: f64| {
        let mut run = 0.0;
        for &(density, weight) in &samples {
            run += weight;
            if run >= q * total {
                return density;
            }
        }
        samples[samples.len() - 1].0
    };
    Some((at(0.05), at(0.50), at(0.95)))
}

/// The maps of one asset, loaded and checked for matching sizes.
struct Maps {
    base: Image,
    metallic: Option<(Image, usize)>,
    roughness: Option<(Image, usize)>,
    normal: Option<Image>,
    emissive: Option<Image>,
    ao: Option<Image>,
    class: Option<Image>,
    team_mask: bool,
}

fn load_maps(manifest: &SourceManifest, dir: &Path) -> Result<Maps, String> {
    let path = |name: &str| -> PathBuf { dir.join(name) };
    let base = read_png(&path(&manifest.maps.base))?;
    let same = |image: Image, what: &str| -> Result<Image, String> {
        if image.width == base.width && image.height == base.height {
            Ok(image)
        } else {
            Err(format!(
                "maps.{what} is {}x{}, maps.base {}x{}: bake the source maps at one size",
                image.width, image.height, base.width, base.height
            ))
        }
    };
    let optional = |name: &Option<String>, what: &str| -> Result<Option<Image>, String> {
        name.as_ref()
            .map(|name| read_png(&path(name)).and_then(|image| same(image, what)))
            .transpose()
    };
    let (metallic, roughness) = match (
        &manifest.maps.metallic_roughness,
        &manifest.maps.metallic,
        &manifest.maps.roughness,
    ) {
        (Some(packed), None, None) => {
            let image = same(read_png(&path(packed))?, "metallic_roughness")?;
            (Some((image.clone(), 0)), Some((image, 1)))
        }
        (None, metallic, roughness) => (
            optional(metallic, "metallic")?.map(|image| (image, 0)),
            optional(roughness, "roughness")?.map(|image| (image, 0)),
        ),
        _ => {
            return Err(String::from(
                "give maps.metallic_roughness or maps.metallic and maps.roughness, not both",
            ));
        }
    };
    Ok(Maps {
        metallic,
        roughness,
        normal: optional(&manifest.maps.normal, "normal")?,
        emissive: optional(&manifest.maps.emissive, "emissive")?,
        ao: optional(&manifest.maps.ao, "ao")?,
        class: optional(&manifest.maps.class, "class")?,
        team_mask: manifest.maps.team_mask.is_some(),
        base,
    })
}

/// Runs every built rule on the asset described by the manifest at `manifest_path`.
#[allow(clippy::too_many_lines)] // One rule after another, in the bible's order.
pub(crate) fn check_source(manifest_path: &Path) -> Result<SourceReport, String> {
    let manifest = read_manifest(manifest_path)?;
    let dir = manifest_path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut report = SourceReport {
        asset: manifest.asset.clone(),
        ..SourceReport::default()
    };
    let mut models = Vec::with_capacity(manifest.models.len());
    for decl in &manifest.models {
        models.push(check_model(decl, &dir.join(&decl.path), &mut report)?);
    }
    let maps = load_maps(&manifest, dir)?;
    let (width, height) = (maps.base.width, maps.base.height);
    let count = maps.base.len();

    // Coverage: the models' UVs, or every texel of a tiling layer.
    let coverage = if models.is_empty() {
        Coverage {
            area: vec![1.0; count],
        }
    } else {
        let mut area = vec![0f32; count];
        for geometry in &models {
            let one = rasterise_uvs(
                &geometry.positions,
                &geometry.uvs,
                &geometry.indices,
                width,
                height,
            )?;
            for (sum, add) in area.iter_mut().zip(one.area) {
                *sum += add;
            }
        }
        let coverage = Coverage { area };
        let covered = (0..count).filter(|&i| coverage.covered(i)).count();
        report.uv_coverage = Some(round(covered as f32 / count as f32, 4));
        coverage
    };
    let covered: Vec<usize> = (0..count).filter(|&i| coverage.covered(i)).collect();
    if covered.is_empty() {
        return Err(String::from("the model's UVs cover no texel of maps.base"));
    }
    let total_area: f64 = covered.iter().map(|&i| f64::from(coverage.area[i])).sum();

    // Texel density (roadmap §9.1) and the shipped texture size.
    let shipped = manifest.shipped_size.unwrap_or(width);
    let mut densities: Vec<(String, Option<Density>)> = models
        .iter()
        .map(|geometry| {
            (
                geometry.name.clone(),
                texel_density(
                    &geometry.positions,
                    &geometry.uvs,
                    &geometry.indices,
                    shipped,
                    shipped,
                ),
            )
        })
        .collect();
    if models.is_empty() {
        let tiled = manifest.tile_m.filter(|&tile| tile > 0.0).map(|tile| {
            let d = shipped as f32 / tile;
            (d, d, d)
        });
        densities.push((manifest.asset.clone(), tiled));
    }
    for (name, density) in densities {
        let rule = format!("TD-1 ({name})");
        match (density, manifest.class.density_target()) {
            (Some((p5, p50, p95)), Some((lo, hi))) => report.push(
                &rule,
                Some(p50 >= lo && p50 <= hi),
                format!("median {p50:.0} px/m (p5 {p5:.0}, p95 {p95:.0}) at {shipped}²"),
                format!("{lo:.0}-{hi:.0} px/m (roadmap §9.1, area-weighted median)"),
            ),
            (Some((p5, p50, p95)), None) => report.push(
                &rule,
                None,
                format!("median {p50:.0} px/m (p5 {p5:.0}, p95 {p95:.0}) at {shipped}²"),
                "no px/m target for this class in roadmap §9.1 or the bible (reported only)",
            ),
            (None, _) => report.push(
                &rule,
                None,
                String::from("not measured"),
                "needs [[models]] (UVs) or tile_m",
            ),
        }
    }
    if let Some(max) = manifest.class.max_texture_side() {
        report.push(
            "TD-2",
            Some(shipped <= max),
            format!("{shipped}²"),
            format!("authored ≤ {max}² (roadmap §9.1)"),
        );
    }

    // Per-texel quantities.
    let lumas: Vec<f32> = (0..count).map(|i| luma(maps.base.rgb(i))).collect();
    let metal_at = |i: usize| {
        maps.metallic
            .as_ref()
            .map_or(0.0, |(image, channel)| image.at(i, *channel))
    };
    let rough_at = |i: usize| {
        maps.roughness
            .as_ref()
            .map_or(1.0, |(image, channel)| image.at(i, *channel))
    };

    // PBR-CLASS-1 and the class of every texel.
    let step = manifest.class_step as f32 / 255.0;
    let mut declared: BTreeMap<u32, &'static ClassRange> = BTreeMap::new();
    for (id, key) in &manifest.classes {
        let id: u32 = id
            .parse()
            .map_err(|_| format!("classes: {id:?} is not a class id"))?;
        let range = class_range(key).ok_or_else(|| {
            format!("classes.{id}: {key:?} is not a material-vocabulary §2 class")
        })?;
        declared.insert(id, range);
    }
    let single = manifest
        .material_class
        .as_deref()
        .map(|key| {
            class_range(key).ok_or_else(|| {
                format!("material_class {key:?} is not a material-vocabulary §2 class")
            })
        })
        .transpose()?;
    let class_of: Vec<Option<u32>> = match (&maps.class, single) {
        (Some(image), _) => (0..count)
            .map(|i| {
                let id = (image.at(i, 0) / step).round() as u32;
                declared.contains_key(&id).then_some(id)
            })
            .collect(),
        (None, Some(range)) => {
            declared.insert(0, range);
            vec![Some(0); count]
        }
        (None, None) => vec![None; count],
    };
    let classed = covered.iter().filter(|&&i| class_of[i].is_some()).count();
    report.push(
        "PBR-CLASS-1",
        Some(classed == covered.len()),
        format!(
            "{:.4} of covered texels",
            classed as f32 / covered.len() as f32
        ),
        if maps.class.is_some() || manifest.material_class.is_some() {
            "100 % have a declared class"
        } else {
            "100 % have a declared class: no class-ID map (maps.class) or material_class given"
        },
    );

    // PBR-ALB-1, PBR-ALB-2, PBR-MET-1.
    let mut dielectric: Vec<f32> = covered
        .iter()
        .filter(|&&i| metal_at(i) < 0.5)
        .map(|&i| lumas[i])
        .collect();
    let mut metal: Vec<f32> = covered
        .iter()
        .filter(|&&i| metal_at(i) >= 0.5)
        .map(|&i| lumas[i])
        .collect();
    if let (Some(p1), Some(p99)) = (
        percentile(&mut dielectric, 1.0),
        percentile(&mut dielectric, 99.0),
    ) {
        report.push(
            "PBR-ALB-1",
            Some(p1 >= 30.0 && p99 <= 240.0),
            format!("p1 {p1:.1}, p99 {p99:.1} ({} texels)", dielectric.len()),
            "dielectric luma p1 ≥ 30, p99 ≤ 240",
        );
    }
    if let Some(p1) = percentile(&mut metal, 1.0) {
        report.push(
            "PBR-ALB-2",
            Some(p1 >= 180.0),
            format!("p1 {p1:.1} ({} texels)", metal.len()),
            "metal luma p1 ≥ 180",
        );
    }
    if maps.metallic.is_some() {
        let transition = covered
            .iter()
            .filter(|&&i| metal_at(i) > 0.1 && metal_at(i) < 0.9)
            .count() as f32
            / covered.len() as f32;
        report.push(
            "PBR-MET-1",
            Some(transition <= 0.03),
            format!("{transition:.4}"),
            "≤ 0.03 of texels with 0.1 < metalness < 0.9",
        );
    } else {
        report.push(
            "PBR-MET-1",
            Some(false),
            String::from("no metalness map"),
            "maps.metallic_roughness or maps.metallic is required",
        );
    }

    // PBR-RGH-2: the roughness floor.
    if maps.roughness.is_some() {
        let floor = if matches!(manifest.class, AssetClass::Hero | AssetClass::Minion) {
            0.08
        } else {
            0.045
        };
        let lowest = covered
            .iter()
            .map(|&i| rough_at(i))
            .fold(f32::INFINITY, f32::min);
        report.push(
            "PBR-RGH-2",
            Some(lowest >= floor - 0.5 / 255.0),
            format!("min {lowest:.3}"),
            format!("≥ {floor} everywhere"),
        );
    } else {
        report.push(
            "PBR-RGH-2",
            Some(false),
            String::from("no roughness map"),
            "maps.metallic_roughness or maps.roughness is required",
        );
    }

    // Per-class ranges and PBR-RGH-1 on interior texels (all 8 neighbours covered and of the
    // same class: class borders are filtered mixes of two materials).
    let interior = |i: usize| -> bool {
        let Some(id) = class_of[i] else { return false };
        let (cols, rows) = (width as usize, height as usize);
        let (column, row) = (i % cols, i / cols);
        for dy in -1isize..=1 {
            for dx in -1isize..=1 {
                let (Some(nx), Some(ny)) =
                    (column.checked_add_signed(dx), row.checked_add_signed(dy))
                else {
                    return false;
                };
                if nx >= cols || ny >= rows {
                    return false;
                }
                let n = ny * cols + nx;
                if !coverage.covered(n) || class_of[n] != Some(id) {
                    return false;
                }
            }
        }
        true
    };
    let interior_texels: Vec<usize> = covered.iter().copied().filter(|&i| interior(i)).collect();
    for (&id, range) in &declared {
        let texels: Vec<usize> = interior_texels
            .iter()
            .copied()
            .filter(|&i| class_of[i] == Some(id))
            .collect();
        let share: f64 = covered
            .iter()
            .filter(|&&i| class_of[i] == Some(id))
            .map(|&i| f64::from(coverage.area[i]))
            .sum::<f64>()
            / total_area;
        *report
            .class_share
            .entry(format!("{id}:{}", range.key))
            .or_default() += (share * 10_000.0).round() / 10_000.0;
        let name = format!("{} (id {id})", range.rule);
        if texels.is_empty() {
            report.push(
                &name,
                None,
                String::from("no interior texels"),
                "class declared but not painted",
            );
            continue;
        }
        let mut albedo: Vec<f32> = texels.iter().map(|&i| lumas[i]).collect();
        let (a1, a99) = (
            percentile(&mut albedo, 1.0).unwrap_or(0.0),
            percentile(&mut albedo, 99.0).unwrap_or(0.0),
        );
        report.push(
            &format!("{name} albedo"),
            Some(a1 >= range.albedo.0 && a99 <= range.albedo.1),
            format!("p1 {a1:.1}, p99 {a99:.1} ({} texels)", texels.len()),
            format!("luma {}-{}", range.albedo.0, range.albedo.1),
        );
        if maps.roughness.is_some() {
            let mut rough: Vec<f32> = texels.iter().map(|&i| rough_at(i)).collect();
            let sigma = std_dev(&rough);
            let (r1, r5, r95, r99) = (
                percentile(&mut rough, 1.0).unwrap_or(0.0),
                percentile(&mut rough, 5.0).unwrap_or(0.0),
                percentile(&mut rough, 95.0).unwrap_or(0.0),
                percentile(&mut rough, 99.0).unwrap_or(0.0),
            );
            let (lo, hi) = range.roughness;
            report.push(
                &format!("{name} roughness"),
                Some(r1 >= lo && r99 <= hi),
                format!("p1 {r1:.3}, p99 {r99:.3}"),
                format!("{lo}-{hi}"),
            );
            if range.key != "GLASS" {
                report.push(
                    &format!("PBR-RGH-1 (id {id} {})", range.key),
                    Some(sigma >= 0.03 && r95 - r5 >= 0.08),
                    format!("σ {sigma:.4}, p95-p5 {:.4}", r95 - r5),
                    "σ ≥ 0.03 and p95 − p5 ≥ 0.08",
                );
            }
        }
        if let Some(want_metal) = range.metal
            && maps.metallic.is_some()
        {
            let mut metalness: Vec<f32> = texels.iter().map(|&i| metal_at(i)).collect();
            let (m1, m99) = (
                percentile(&mut metalness, 1.0).unwrap_or(0.0),
                percentile(&mut metalness, 99.0).unwrap_or(0.0),
            );
            let (ok, limit) = if want_metal {
                (
                    m1 >= 0.9,
                    "metal: p1 ≥ 0.9 (outside the 0.1-0.9 transition band)",
                )
            } else {
                (m99 <= 0.1, "dielectric: p99 ≤ 0.1")
            };
            report.push(
                &format!("{name} metalness"),
                Some(ok),
                format!("p1 {m1:.3}, p99 {m99:.3}"),
                limit,
            );
        }
    }

    // PBR-NRM-1 and PBR-NRM-2.
    if let Some(normal) = &maps.normal {
        // One 8-bit code is 2/255 in [-1, 1]: rounding moves a component by up to 1/255.
        let quantum = if normal.sixteen_bit {
            1.0 / 65535.0
        } else {
            1.0 / 255.0
        };
        let decode = |i: usize, c: usize| normal.at(i, c) * 2.0 - 1.0;
        if normal.channels >= 3 {
            let mut worst = 0f32;
            let within = covered
                .iter()
                .filter(|&&i| {
                    let n = [decode(i, 0), decode(i, 1), decode(i, 2)];
                    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                    worst = worst.max((length - 1.0).abs());
                    (length - 1.0).abs() <= 0.02
                })
                .count();
            report.push(
                "PBR-NRM-1",
                Some(within == covered.len()),
                format!(
                    "{:.4} within, worst |len − 1| {worst:.3}",
                    within as f32 / covered.len() as f32
                ),
                "every texel 1 ± 0.02",
            );
        } else {
            report.push(
                "PBR-NRM-1",
                None,
                String::from("XY only"),
                "an XY map has no stored length",
            );
        }
        let over = covered
            .iter()
            .filter(|&&i| {
                let x = (decode(i, 0).abs() - quantum).max(0.0);
                let y = (decode(i, 1).abs() - quantum).max(0.0);
                x * x + y * y > 1.0
            })
            .count();
        report.push(
            "PBR-NRM-2",
            Some(over == 0),
            format!("{over} texels over"),
            "X² + Y² ≤ 1 per texel (within one code of rounding)",
        );
    }

    // PBR-DELIGHT-1: blurred albedo luma against the baked AO.
    if let Some(ao) = &maps.ao {
        let block = 8usize;
        let (bw, bh) = (width as usize / block, height as usize / block);
        let (mut xs, mut ys) = (Vec::new(), Vec::new());
        for by in 0..bh {
            for bx in 0..bw {
                let mut ok = true;
                let (mut sum_l, mut sum_a) = (0f64, 0f64);
                for y in by * block..(by + 1) * block {
                    for x in bx * block..(bx + 1) * block {
                        let i = y * width as usize + x;
                        ok &= coverage.covered(i);
                        sum_l += f64::from(lumas[i]);
                        sum_a += f64::from(ao.at(i, 0));
                    }
                }
                if ok {
                    xs.push(sum_l);
                    ys.push(sum_a);
                }
            }
        }
        match correlation(&xs, &ys) {
            Some(r) => report.push(
                "PBR-DELIGHT-1",
                Some(r < 0.3),
                format!("r {r:.3} over {} blocks", xs.len()),
                "correlation of 8×8-blurred albedo luma with AO < 0.3",
            ),
            None => report.push(
                "PBR-DELIGHT-1",
                None,
                String::from("too few fully covered 8×8 blocks"),
                "< 0.3",
            ),
        }
    } else {
        report.push(
            "PBR-DELIGHT-1",
            None,
            String::from("no AO map"),
            "give maps.ao to check",
        );
    }

    // PAL-1 / PAL-2 and VAL-1.
    let mut chroma: Vec<f32> = covered
        .iter()
        .map(|&i| oklch_display(maps.base.rgb(i))[1])
        .collect();
    let ground = manifest.class == AssetClass::Terrain;
    let (rule, cap) = if ground {
        ("PAL-1", 0.06)
    } else {
        ("PAL-2", 0.08)
    };
    if let Some(p99) = percentile(&mut chroma, 99.0) {
        report.push(
            rule,
            Some(p99 <= cap),
            format!("C p99 {p99:.4}"),
            format!("≤ {cap}"),
        );
    }
    if ground {
        let mut lightness: Vec<f32> = covered
            .iter()
            .map(|&i| oklch_display(maps.base.rgb(i))[0])
            .collect();
        let (p1, p99) = (
            percentile(&mut lightness, 1.0).unwrap_or(0.0),
            percentile(&mut lightness, 99.0).unwrap_or(0.0),
        );
        report.push(
            "VAL-1",
            Some(p1 >= 0.32 && p99 <= 0.62),
            format!("L p1 {p1:.3}, p99 {p99:.3}"),
            "terrain albedo L 0.32-0.62",
        );
    }

    // Emissive: PAL-4 / PAL-5, PBR-EMIT-1, PBR-EMIT-2.
    let l_ref = manifest.lighting.l_ref()?;
    match (&maps.emissive, &manifest.emissive) {
        (Some(mask), Some(decl)) => {
            let colour = parse_hex(&decl.colour)
                .ok_or_else(|| format!("emissive.colour {:?} is not #RRGGBB", decl.colour))?;
            let [_, c, h] = oklch_display(colour);
            let near = TEAM_HUES
                .iter()
                .any(|&team| hue_distance(h, team) <= TEAM_ZONE_DEG);
            let (rule, cap) = if near {
                ("PAL-4", 0.06)
            } else {
                ("PAL-5", 0.12)
            };
            report.push(
                rule,
                Some(c <= cap + 0.0005),
                format!("C {c:.3}, h {h:.1}"),
                format!(
                    "C ≤ {cap} ({} a team zone)",
                    if near { "inside" } else { "outside" }
                ),
            );
            let y = luminance(colour.map(srgb_to_linear)) * decl.strength;
            let ev = (y / l_ref).log2();
            let (cap_ev, usage) = match decl.usage {
                EmissiveUse::Accent => (1.0, "accent"),
                EmissiveUse::Gameplay => (2.0, "gameplay"),
            };
            report.push(
                "PBR-EMIT-1",
                Some(ev <= cap_ev + 0.005),
                format!("{ev:+.2} EV over L_ref {l_ref:.4}"),
                format!("≤ +{cap_ev:.0} EV ({usage})"),
            );
            let gain = y / l_ref;
            let lit: f64 = covered
                .iter()
                .filter(|&&i| mask.at(i, 0) * gain > 0.25)
                .map(|&i| f64::from(coverage.area[i]))
                .sum::<f64>()
                / total_area;
            let cap = manifest.class.emissive_area_cap();
            report.push(
                "PBR-EMIT-2",
                Some(lit as f32 <= cap),
                format!("{lit:.4} of the surface"),
                format!("≤ {cap} (emissive > 0.25 × L_ref)"),
            );
        }
        (Some(_), None) => {
            return Err(String::from(
                "maps.emissive needs an [emissive] table (colour, strength, use)",
            ));
        }
        (None, _) => report.push(
            "PBR-EMIT-2",
            None,
            String::from("no emissive map"),
            "nothing emits",
        ),
    }

    // PBR-WEAR-1 (units) and TINT-1.
    if manifest.class.is_unit() {
        match manifest.dust_class {
            Some(dust) => {
                let share: f64 = covered
                    .iter()
                    .filter(|&&i| class_of[i] == Some(dust))
                    .map(|&i| f64::from(coverage.area[i]))
                    .sum::<f64>()
                    / total_area;
                report.push(
                    "PBR-WEAR-1",
                    Some(share <= 0.20),
                    format!("{share:.4} of the surface"),
                    "dust ≤ 0.20 of a unit's surface (lower body: review renders)",
                );
            }
            None => report.push(
                "PBR-WEAR-1",
                None,
                String::from("no dust_class"),
                "declare the dust layer's class id to check",
            ),
        }
    }
    if matches!(
        manifest.class,
        AssetClass::Hero | AssetClass::Minion | AssetClass::Structure
    ) {
        report.push(
            "TINT-1",
            Some(maps.team_mask),
            String::from(if maps.team_mask {
                "team mask given"
            } else {
                "no team mask"
            }),
            "team mask present (Compat keeps the tint)",
        );
    }
    for (rule, why) in NOT_BUILT {
        report.push(rule, None, String::from("not built"), why);
    }
    Ok(report)
}

/// Pearson correlation of two equal-length samples (`None` under 2 samples or no variance).
pub(crate) fn correlation(xs: &[f64], ys: &[f64]) -> Option<f32> {
    if xs.len() < 2 || xs.len() != ys.len() {
        return None;
    }
    let n = xs.len() as f64;
    let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (x, y) in xs.iter().zip(ys) {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
        syy += (y - my) * (y - my);
    }
    (sxx > 0.0 && syy > 0.0).then(|| (sxy / (sxx * syy).sqrt()) as f32)
}

/// Shortest distance between two hues, in degrees.
pub(crate) fn hue_distance(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// The export checks on the model, through the engine's own loader (ADR-0007, ADR-0022).
fn check_model(
    decl: &ModelDecl,
    path: &Path,
    report: &mut SourceReport,
) -> Result<ModelGeometry, String> {
    let shown = path.display();
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    report.push(
        &format!("GLB-NAME ({stem})"),
        Some(valid_name(stem, 16)),
        format!("{stem:?}"),
        "model name (file stem) 1..=16 of [A-Za-z0-9_-]",
    );
    let mesh = moba_assets::load_glb_file_with(path, moba_assets::GlbTangents::FileOnly)
        .map_err(|error| format!("{shown}: the engine's loader refuses it: {error}"))?;
    report.push(
        &format!("GLB-LOAD ({stem})"),
        Some(true),
        format!(
            "{} vertices, {} triangles",
            mesh.vertex_count(),
            mesh.triangle_count()
        ),
        "loads with the engine's bounded loader (ADR-0007)",
    );
    let tangents = moba_assets::check_baked_tangents_file(path);
    report.push(
        &format!("GLB-TANGENT ({stem})"),
        Some(tangents.is_ok()),
        tangents
            .as_ref()
            .err()
            .map_or_else(|| String::from("TANGENT baked"), Clone::clone),
        "UVs need baked TANGENT (ADR-0022 amendment 1)",
    );
    if let (Some(file), Some(generated)) = (mesh.tangents(), mesh.generate_tangents()) {
        let (agree, handed) = tangent_agreement(file, &generated);
        report.push(
            &format!("GLB-MIKK ({stem})"),
            None,
            format!("{agree:.4} within 5°, {handed:.4} same handedness"),
            "file tangents against the engine's MikkTSpace on the same mesh (convention check)",
        );
    }
    let (low, high) = mesh.bounds();
    report.push(
        &format!("GLB-FEET ({stem})"),
        Some((low[1] - decl.hover_m).abs() <= 0.01),
        format!("min y {:.3}, height {:.3} m", low[1], high[1]),
        format!(
            "lowest vertex at y = {:.2} ± 0.01 m (feet on the ground, or hover_m)",
            decl.hover_m
        ),
    );
    let Some(uvs) = mesh.uvs() else {
        return Err(format!("{shown}: no TEXCOORD_0"));
    };
    Ok(ModelGeometry {
        name: stem.to_string(),
        positions: mesh.positions().to_vec(),
        uvs: uvs.to_vec(),
        indices: mesh.indices().to_vec(),
    })
}

/// Shares of vertices whose file tangent is within 5° of `generated`, and whose handedness
/// (`w`) matches.
pub(crate) fn tangent_agreement(file: &[[f32; 4]], generated: &[[f32; 4]]) -> (f32, f32) {
    let n = file.len().min(generated.len()).max(1) as f32;
    let cos5 = 5f32.to_radians().cos();
    let mut close = 0usize;
    let mut handed = 0usize;
    for (a, b) in file.iter().zip(generated) {
        let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let la = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        let lb = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt();
        if la > 0.0 && lb > 0.0 && dot / (la * lb) >= cos5 {
            close += 1;
        }
        if (a[3] >= 0.0) == (b[3] >= 0.0) {
            handed += 1;
        }
    }
    (close as f32 / n, handed as f32 / n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imaging::{Image, oklch_display};

    #[test]
    fn luma_matches_the_bible_examples() {
        // material-vocabulary §7: Iron #C0BEBB is luma 190, Charcoal 39 is 39 (grey stays).
        assert!((luma([192.0, 190.0, 187.0].map(|v| v / 255.0)) - 190.3).abs() < 1.0);
        assert!((luma([39.0 / 255.0; 3]) - 39.0).abs() < 0.01);
        // Brass #F5E4AE is luma 228 and C 0.072 (§3.1).
        let brass = [245.0, 228.0, 174.0].map(|v| v / 255.0);
        assert!((luma(brass) - 228.0).abs() < 1.0);
        assert!((oklch_display(brass)[1] - 0.072).abs() < 0.002);
    }

    #[test]
    fn golden_crossing_l_ref_matches_the_bible() {
        // §1.1: L_ref ≈ 0.080 × sun_intensity; with Golden Crossing's 1.5, 0.12.
        let l_ref = LightingDecl::default().l_ref().expect("default lighting");
        assert!(
            (l_ref / GOLDEN_SUN_INTENSITY - 0.080).abs() < 0.002,
            "{l_ref}"
        );
    }

    #[test]
    fn class_table_matches_material_vocabulary_section_2() {
        let metal = class_range("METAL").expect("metal");
        assert_eq!(metal.albedo, (180.0, 255.0));
        assert_eq!(metal.roughness, (0.15, 0.70));
        assert_eq!(class_range("CERAM").expect("ceram").roughness, (0.70, 0.95));
        assert_eq!(class_range("CLOTH").expect("cloth").roughness, (0.70, 1.0));
        assert_eq!(class_range("BONE").expect("bone").albedo, (150.0, 235.0));
        assert!(class_range("STONE").is_none());
        let mut keys: Vec<&str> = CLASSES.iter().map(|c| c.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), CLASSES.len());
    }

    #[test]
    fn emissive_colours_sit_where_the_palette_says() {
        // White-gold #FFE6BE: C 0.059 at h ~80, inside the CB-orange zone (PAL-4).
        let gold = parse_hex("#FFE6BE").expect("hex");
        let [_, c, h] = oklch_display(gold);
        assert!((c - 0.059).abs() < 0.002, "{c}");
        assert!(
            TEAM_HUES
                .iter()
                .any(|&t| hue_distance(h, t) <= TEAM_ZONE_DEG)
        );
        // Void violet #A08CD0: C 0.100 at h 297, outside every zone (PAL-5).
        let violet = parse_hex("#A08CD0").expect("hex");
        let [_, c, h] = oklch_display(violet);
        assert!((c - 0.100).abs() < 0.002, "{c}");
        assert!(
            TEAM_HUES
                .iter()
                .all(|&t| hue_distance(h, t) > TEAM_ZONE_DEG)
        );
        assert!((hue_distance(350.0, 10.0) - 20.0).abs() < 1e-4);
    }

    #[test]
    fn uv_raster_covers_a_square_and_spreads_its_area() {
        // A 2 m × 2 m quad mapped to the left half of an 8×8 texture.
        let positions = [
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 2.0, 0.0],
            [0.0, 2.0, 0.0],
        ];
        let uvs = [[0.0, 0.0], [0.5, 0.0], [0.5, 1.0], [0.0, 1.0]];
        let indices = [0, 1, 2, 0, 2, 3];
        let coverage = rasterise_uvs(&positions, &uvs, &indices, 8, 8).expect("raster");
        let covered = coverage.area.iter().filter(|&&a| a > 0.0).count();
        assert_eq!(covered, 32);
        let total: f32 = coverage.area.iter().sum();
        assert!((total - 4.0).abs() < 1e-4);
        // Left half only.
        assert!(coverage.area[7] == 0.0 && coverage.area[0] > 0.0);
        // 4 texels across 2 m and 8 down 2 m: sqrt(32 / 4) = 2.83 px/m.
        let (p5, p50, p95) = texel_density(&positions, &uvs, &indices, 8, 8).expect("density");
        assert!(
            (p50 - 8f32.sqrt()).abs() < 1e-4 && (p5 - p50).abs() < 1e-6 && (p95 - p50).abs() < 1e-6
        );
    }

    #[test]
    fn uv_raster_wraps_tiling_uvs() {
        // A trim strip whose UVs run from 1.0 to 1.5: it lands on the left half, wrapped.
        let positions = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let uvs = [[1.0, 0.0], [1.5, 0.0], [1.5, 1.0], [1.0, 1.0]];
        let coverage = rasterise_uvs(&positions, &uvs, &[0, 1, 2, 0, 2, 3], 4, 4).expect("raster");
        let covered: Vec<usize> = (0..16).filter(|&i| coverage.area[i] > 0.0).collect();
        assert_eq!(covered, [0, 1, 4, 5, 8, 9, 12, 13]);
    }

    #[test]
    fn correlation_and_tangents_helpers() {
        let r = correlation(&[1.0, 2.0, 3.0], &[2.0, 4.0, 6.0]).expect("r");
        assert!((r - 1.0).abs() < 1e-6);
        assert!(correlation(&[1.0, 1.0], &[2.0, 3.0]).is_none());
        let a = [[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, -1.0]];
        let b = [[1.0, 0.01, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0]];
        assert_eq!(tangent_agreement(&a, &b), (0.5, 0.5));
    }

    /// Writes an RGBA8 PNG.
    fn write_png(path: &Path, width: u32, height: u32, rgba: &[[u8; 4]]) {
        let file = std::fs::File::create(path).expect("create");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("header");
        let bytes: Vec<u8> = rgba.iter().flatten().copied().collect();
        writer.write_image_data(&bytes).expect("data");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("moba-asset-check-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// A 16×16 terrain-style layer: noisy roughness, a flat normal, one class.
    fn layer(dir: &Path, albedo: [u8; 3]) -> PathBuf {
        let n = 16 * 16;
        let base: Vec<[u8; 4]> = (0..n)
            .map(|i| {
                let jitter = (i % 5) as u8;
                [
                    albedo[0] + jitter,
                    albedo[1] + jitter,
                    albedo[2] + jitter,
                    255,
                ]
            })
            .collect();
        let mr: Vec<[u8; 4]> = (0..n)
            .map(|i| [0, 220 + (i * 7 % 35) as u8, 0, 255])
            .collect();
        let normal = vec![[128, 128, 255, 255]; n];
        write_png(&dir.join("base.png"), 16, 16, &base);
        write_png(&dir.join("mr.png"), 16, 16, &mr);
        write_png(&dir.join("normal.png"), 16, 16, &normal);
        let manifest = dir.join("ash.source.toml");
        std::fs::write(
            &manifest,
            "asset = \"ash\"\nclass = \"terrain\"\ntile_m = 4.0\nshipped_size = 1024\n\
             material_class = \"CHAR\"\n\
             [maps]\nbase = \"base.png\"\nmetallic_roughness = \"mr.png\"\nnormal = \"normal.png\"\n",
        )
        .expect("manifest");
        manifest
    }

    fn rule<'a>(report: &'a SourceReport, id: &str) -> &'a RuleResult {
        report
            .rules
            .iter()
            .find(|rule| rule.rule == id)
            .unwrap_or_else(|| panic!("no rule {id}: {}", report.text()))
    }

    #[test]
    fn an_ash_layer_passes_and_a_crushed_one_fails() {
        let dir = scratch("ash");
        // Ash ground #6B6560 (luma ~102), roughness 0.86-1.0: inside PBR-CHAR-1 and PAL-1.
        let good = check_source(&layer(&dir, [0x6B, 0x65, 0x60])).expect("check");
        assert!(good.passed(), "{}", good.text());
        assert_eq!(rule(&good, "TD-1 (ash)").pass, Some(true)); // 1024 px / 4 m = 256 px/m
        assert_eq!(rule(&good, "VAL-1").pass, Some(true));
        assert_eq!(rule(&good, "PBR-NRM-1").pass, Some(true));
        // Crushed to luma ~20: PBR-ALB-1, the CHAR range and VAL-1 fail.
        let bad = check_source(&layer(&dir, [20, 20, 20])).expect("check");
        assert!(!bad.passed());
        assert_eq!(rule(&bad, "PBR-ALB-1").pass, Some(false));
        assert_eq!(rule(&bad, "PBR-CHAR-1 (id 0) albedo").pass, Some(false));
        assert_eq!(rule(&bad, "VAL-1").pass, Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifests_reject_unknown_keys_and_classes() {
        let dir = scratch("manifest");
        let path = dir.join("x.source.toml");
        std::fs::write(
            &path,
            "asset = \"x\"\nclass = \"prop\"\nlicense = 1\n[maps]\nbase = \"b.png\"\n",
        )
        .expect("write");
        assert!(read_manifest(&path).is_err());
        std::fs::write(
            &path,
            "asset = \"bad name\"\nclass = \"prop\"\n[maps]\nbase = \"b.png\"\n",
        )
        .expect("write");
        assert!(read_manifest(&path).is_err());
        let base = vec![[100u8, 100, 100, 255]; 4];
        write_png(&dir.join("b.png"), 2, 2, &base);
        std::fs::write(
            &path,
            "asset = \"x\"\nclass = \"prop\"\nmaterial_class = \"STONE\"\n[maps]\nbase = \"b.png\"\n",
        )
        .expect("write");
        let error = check_source(&path).expect_err("STONE is not a §2 class");
        assert!(error.contains("not a material-vocabulary"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn image_reads_sixteen_bit_and_grey() {
        let dir = scratch("png16");
        let path = dir.join("g16.png");
        let file = std::fs::File::create(&path).expect("create");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 2, 1);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Sixteen);
        let mut writer = encoder.write_header().expect("header");
        writer
            .write_image_data(&[0xFF, 0xFF, 0x80, 0x00])
            .expect("data");
        drop(writer);
        let image: Image = read_png(&path).expect("read");
        assert_eq!((image.width, image.height, image.channels), (2, 1, 1));
        assert!((image.get(0, 0, 0) - 1.0).abs() < 1e-6);
        assert!((image.get(1, 0, 2) - 0.5).abs() < 1e-3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
