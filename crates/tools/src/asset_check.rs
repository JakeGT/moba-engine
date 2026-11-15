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
use std::path::Path;
use std::rc::Rc;

use serde::Deserialize;

use crate::imaging::{
    Image, luma, luminance, oklch_display, parse_hex, percentile, percentile_sorted, read_png,
    round, srgb_to_linear, std_dev,
};

/// Largest source manifest read, in bytes.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
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

/// A material-vocabulary §3 entry whose numbers disagree with its §2 class row. Where a class
/// names one with `vocab`, the §3 numbers apply, reading §3 as the more specific rule. That is
/// the A1 review's reading, not yet confirmed by the maintainer: ADR-0022 amendment 2 lists each
/// conflict as an open decision. This is a minimal slice of PBR-VOCAB-1: only the entries that
/// conflict are data; every other entry is checked against its §2 row.
#[derive(Clone, Copy, Debug)]
#[allow(clippy::struct_excessive_bools)] // Independent per-entry exemptions from §3.
pub(crate) struct VocabEntry {
    /// The §3 material name, as the manifest gives it (matched ignoring case).
    pub(crate) name: &'static str,
    /// The §2 class the entry belongs to; the manifest must declare the same class.
    pub(crate) class: &'static str,
    /// Where in material-vocabulary.md the numbers come from.
    pub(crate) source: &'static str,
    /// The entry's albedo luma range, if it replaces the §2 one.
    pub(crate) albedo: Option<(f32, f32)>,
    /// The entry's roughness range, if it replaces the §2 one.
    pub(crate) roughness: Option<(f32, f32)>,
    /// A wet mask: exempt from PBR-RGH-1 (§1.1, §6).
    pub(crate) wet: bool,
    /// A dark metal: left out of PBR-ALB-2's metal floor.
    pub(crate) dark_metal: bool,
    /// Its §3 roughness goes below 0.08: PBR-RGH-2's floor on heroes and minions is 0.045 here.
    pub(crate) glossy: bool,
    /// A dielectric with bare-metal chips (metalness ≥ 0.9): chip texels are left out of the
    /// class's dielectric metalness check and reported; partial metalness still counts.
    pub(crate) metal_chips: bool,
}

const fn vocab(
    name: &'static str,
    class: &'static str,
    source: &'static str,
    roughness: (f32, f32),
) -> VocabEntry {
    VocabEntry {
        name,
        class,
        source,
        albedo: None,
        roughness: Some(roughness),
        wet: false,
        dark_metal: false,
        glossy: false,
        metal_chips: false,
    }
}

/// The §3 entries that conflict with §2 (ADR-0022 amendment 2 lists each one as a maintainer
/// decision).
pub(crate) const VOCAB: [VocabEntry; 15] = [
    vocab(
        "Void composite, matte",
        "CERAM",
        "§3.2 Void composite, matte: roughness 0.55-0.80",
        (0.55, 0.80),
    ),
    vocab(
        "Void composite, glazed",
        "CERAM",
        "§3.2 Void composite, glazed: base roughness 0.5-0.7",
        (0.50, 0.70),
    ),
    vocab(
        "Heat-shield ceramic, glazed",
        "CERAM",
        "§3.1 Heat-shield ceramic, glazed: base roughness 0.6-0.8",
        (0.60, 0.80),
    ),
    vocab(
        "Void silk",
        "CLOTH",
        "§3.2 Void silk: roughness 0.55-0.80",
        (0.55, 0.80),
    ),
    vocab(
        "Fused scorch (crust)",
        "CHAR",
        "§3.1 Fused scorch: crust roughness 0.7-0.9",
        (0.70, 0.90),
    ),
    vocab(
        "Fused scorch (fracture faces)",
        "GLASS",
        "§3.1 Fused scorch (CHAR / GLASS): fracture faces roughness 0.15-0.35",
        (0.15, 0.35),
    ),
    VocabEntry {
        roughness: None,
        metal_chips: true,
        ..vocab(
            "Forge dark alloy, scaled",
            "OXIDE",
            "§3.1 Forge dark alloy, scaled: metal 0, 1 where the scale is chipped",
            (0.0, 0.0),
        )
    },
    vocab(
        "Levitating basalt (cut faces)",
        "STONE-1",
        "§3.2 Levitating basalt: cut faces roughness 0.4-0.55",
        (0.40, 0.55),
    ),
    vocab(
        "Forge basalt masonry (polished treads)",
        "STONE-1",
        "§3.1 Forge basalt masonry: polished treads roughness 0.35-0.5",
        (0.35, 0.50),
    ),
    vocab(
        "Polished black stone",
        "STONE-2",
        "§3.2 Polished black stone: roughness 0.15-0.40",
        (0.15, 0.40),
    ),
    VocabEntry {
        wet: true,
        ..vocab(
            "Basalt, wet",
            "STONE-1",
            "§3.3 Basalt, wet: roughness 0.10-0.30 where wet; wet masks are exempt from \
             PBR-RGH-1 (§1.1)",
            (0.10, 0.30),
        )
    },
    VocabEntry {
        albedo: Some((30.0, 140.0)),
        dark_metal: true,
        ..vocab(
            "Emitter lattice",
            "METAL",
            "§3.1 Emitter lattice: metal 1 with the albedo of Forge dark alloy, scaled \
             (#3A3836, luma 56; its OXIDE range 30-140), roughness 0.35-0.60",
            (0.35, 0.60),
        )
    },
    VocabEntry {
        glossy: true,
        ..vocab(
            "Void-glass",
            "GLASS",
            "§3.2 Void-glass: roughness 0.045-0.15",
            (0.045, 0.15),
        )
    },
    VocabEntry {
        glossy: true,
        ..vocab(
            "Sun-glass",
            "GLASS",
            "§3.1 Sun-glass: roughness 0.05-0.20",
            (0.05, 0.20),
        )
    },
    VocabEntry {
        glossy: true,
        ..vocab(
            "Obsidian metamaterial, polished",
            "STONE-2",
            "§3.2 Obsidian metamaterial, polished: roughness 0.05-0.20",
            (0.05, 0.20),
        )
    },
];

/// The conflicting §3 entry called `name` (ignoring case).
pub(crate) fn vocab_entry(name: &str) -> Option<&'static VocabEntry> {
    VOCAB
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(name.trim()))
}

/// Rules of the bible that `content-check --source` does not build yet, with the reason.
const NOT_BUILT: [(&str, &str); 10] = [
    (
        "PBR-VOCAB-1",
        "only the §2/§3 conflicts are data (`vocab`, ADR-0022 amendment 2); naming a §3 entry \
         for every material is not checked (checks.md step 5)",
    ),
    (
        "PBR-FORGE-1",
        "needs the §3 vocabulary entry per class (checks.md step 5)",
    ),
    (
        "PBR-VOID-1",
        "median luma half needs the §3 entry per class (checks.md step 5); the projected-area \
         half is a review-render rule",
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
    /// Class id → §2 class key (`"1" = "METAL"`), or a table naming the §3 entry too
    /// (`"2" = { class = "CERAM", vocab = "Void composite, matte" }`).
    #[serde(default)]
    pub(crate) classes: BTreeMap<String, ClassDecl>,
    /// Without a class-ID map: the one class every texel belongs to (terrain layers), as in
    /// `classes`.
    pub(crate) material_class: Option<ClassDecl>,
    /// The normal map stores X and Y in R and G only (B unused); a grey + alpha map is always
    /// X in grey and Y in alpha.
    #[serde(default)]
    pub(crate) normal_xy: bool,
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

/// A class declaration: a §2 key, or a key with the §3 entry it is.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum ClassDecl {
    /// `"METAL"`.
    Key(String),
    /// `{ class = "CERAM", vocab = "Void composite, matte" }`.
    Entry(ClassEntry),
}

/// The table form of a [`ClassDecl`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClassEntry {
    /// The §2 class key.
    pub(crate) class: String,
    /// The material-vocabulary §3 entry (PBR-VOCAB-1).
    pub(crate) vocab: Option<String>,
}

impl ClassDecl {
    fn parts(&self) -> (&str, Option<&str>) {
        match self {
            Self::Key(key) => (key, None),
            Self::Entry(entry) => (&entry.class, entry.vocab.as_deref()),
        }
    }
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
    /// Tangent-space normal, +Y up: XYZ in RGB, XY in RG (`normal_xy`), or X in grey and Y in
    /// alpha.
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
#[derive(Debug)]
struct ModelGeometry {
    name: String,
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

/// Texel visits the UV rasteriser makes before it stops: 64 per texel of the map, at least
/// 2^28 (tiling UVs wrap, so the work grows with how far they tile, not with the map).
pub(crate) fn raster_budget(width: u32, height: u32) -> u64 {
    (64 * u64::from(width) * u64::from(height)).max(1 << 28)
}

/// Largest UV extent, in texels, of one triangle or of any coordinate: far beyond any real
/// tiling (2^28 texels is 65 536 repeats of a 4096² map), and small enough that every texel
/// index below fits an `i64` exactly.
const MAX_UV_TEXELS: f64 = (1u64 << 28) as f64;

/// What [`rasterise_uvs`] measured.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Raster {
    /// Every triangle was rasterised into the coverage.
    Done,
    /// The visit budget ran out: the coverage is incomplete and must not be used.
    OverBudget,
}

/// Rasterises UV triangles (glTF convention: `(0, 0)` is the image's top-left) at texel centres,
/// wrapping tiling UVs, and adds each triangle's world area (m²), spread over its texels, to
/// `area` (`width × height`, row-major). `visits` carries the texel visits across models.
/// Returns an error for UVs no tiling explains (a coordinate or a triangle beyond
/// [`MAX_UV_TEXELS`]).
#[allow(clippy::many_single_char_names)] // Geometry: p, t, w, h, x, y.
pub(crate) fn rasterise_uvs(
    positions: &[[f32; 3]],
    uvs: &[[f32; 2]],
    indices: &[u32],
    (width, height): (u32, u32),
    area: &mut [f32],
    visits: &mut u64,
) -> Result<Raster, String> {
    let (w, h) = (f64::from(width), f64::from(height));
    let budget = raster_budget(width, height);
    let mut texels: Vec<usize> = Vec::new();
    for (number, triangle) in indices.as_chunks::<3>().0.iter().enumerate() {
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
        // Bound the coordinates before any conversion to integers: a huge finite UV would
        // saturate the cast and wrap the span to nothing (or to an endless loop).
        let reach = [min_x, max_x, min_y, max_y]
            .iter()
            .fold(0f64, |most, v| most.max(v.abs()));
        if reach > MAX_UV_TEXELS || max_x - min_x > MAX_UV_TEXELS || max_y - min_y > MAX_UV_TEXELS {
            return Err(format!(
                "triangle {number} has UVs {reach:.3e} texels from the origin or across: \
                 beyond any tiling (limit {MAX_UV_TEXELS:.0} texels)"
            ));
        }
        let (x0, x1) = ((min_x - 0.5).ceil() as i64, (max_x - 0.5).floor() as i64);
        let (y0, y1) = ((min_y - 0.5).ceil() as i64, (max_y - 0.5).floor() as i64);
        let columns = x1
            .checked_sub(x0)
            .and_then(|d| d.checked_add(1))
            .unwrap_or(0)
            .max(0);
        let rows = y1
            .checked_sub(y0)
            .and_then(|d| d.checked_add(1))
            .unwrap_or(0)
            .max(0);
        let span = (columns as u64)
            .checked_mul(rows as u64)
            .ok_or_else(|| format!("triangle {number}: UV span overflows"))?;
        *visits = visits.saturating_add(span);
        if *visits > budget {
            return Ok(Raster::OverBudget);
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
            // A sliver smaller than a texel: credit the texel under its centroid (bounded
            // above, so the cast is exact).
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
    Ok(Raster::Done)
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

/// One channel of a loaded map (the packed metallic-roughness image is shared by two).
struct Channel {
    image: Rc<Image>,
    channel: usize,
}

impl Channel {
    fn at(&self, index: usize) -> f32 {
        self.image.at(index, self.channel)
    }
}

/// The maps most rules read, loaded and checked for matching sizes. The single-use maps
/// (normal, AO, emissive, team mask) are read by their own rule and dropped after it, so a
/// 4096² asset never holds them all at once.
struct Maps {
    base: Image,
    metallic: Option<Channel>,
    roughness: Option<Channel>,
    class: Option<Image>,
}

/// Reads the map `name` (relative to `dir`) and checks it has the base map's size.
fn read_same(dir: &Path, name: &str, what: &str, size: (u32, u32)) -> Result<Image, String> {
    let image = read_png(&dir.join(name))?;
    if (image.width, image.height) == size {
        Ok(image)
    } else {
        Err(format!(
            "maps.{what} is {}x{}, maps.base {}x{}: bake the source maps at one size",
            image.width, image.height, size.0, size.1
        ))
    }
}

fn load_maps(manifest: &SourceManifest, dir: &Path) -> Result<Maps, String> {
    let base = read_png(&dir.join(&manifest.maps.base))?;
    let size = (base.width, base.height);
    let optional = |name: &Option<String>, what: &str| -> Result<Option<Image>, String> {
        name.as_deref()
            .map(|name| read_same(dir, name, what, size))
            .transpose()
    };
    let single = |image: Image| Channel {
        image: Rc::new(image),
        channel: 0,
    };
    let (metallic, roughness) = match (
        &manifest.maps.metallic_roughness,
        &manifest.maps.metallic,
        &manifest.maps.roughness,
    ) {
        (Some(packed), None, None) => {
            let image = read_same(dir, packed, "metallic_roughness", size)?;
            if image.channels < 3 {
                return Err(String::from(
                    "maps.metallic_roughness needs R (metallic) and G (roughness): it is grey",
                ));
            }
            let image = Rc::new(image);
            (
                Some(Channel {
                    image: Rc::clone(&image),
                    channel: 0,
                }),
                Some(Channel { image, channel: 1 }),
            )
        }
        (None, metallic, roughness) => (
            optional(metallic, "metallic")?.map(single),
            optional(roughness, "roughness")?.map(single),
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
        class: optional(&manifest.maps.class, "class")?,
        base,
    })
}

/// A declared class: its id, its §2 row with any §3 override applied, and the §3 entry.
struct Declared {
    id: u32,
    range: ClassRange,
    /// The `vocab` name the manifest gave.
    vocab: Option<String>,
    /// The conflicting §3 entry it names, if it names one.
    entry: Option<&'static VocabEntry>,
}

/// `class_of` value of a texel with no declared class.
const NO_CLASS: u8 = u8::MAX;

fn declare(id: u32, decl: &ClassDecl, what: &str) -> Result<Declared, String> {
    let (key, vocab) = decl.parts();
    let mut range = *class_range(key)
        .ok_or_else(|| format!("{what}: {key:?} is not a material-vocabulary §2 class"))?;
    let entry = vocab.and_then(vocab_entry);
    if let Some(entry) = entry {
        if entry.class != range.key {
            return Err(format!(
                "{what}: the §3 entry {:?} is a {} material, not {key}",
                entry.name, entry.class
            ));
        }
        if let Some(albedo) = entry.albedo {
            range.albedo = albedo;
        }
        if let Some(roughness) = entry.roughness {
            range.roughness = roughness;
        }
    }
    Ok(Declared {
        id,
        range,
        vocab: vocab.map(String::from),
        entry,
    })
}

/// The declared classes (at most 255) and the class of every texel, as an index into them.
fn classify(
    manifest: &SourceManifest,
    maps: &Maps,
    report: &mut SourceReport,
) -> Result<(Vec<Declared>, Vec<u8>), String> {
    let count = maps.base.len();
    let mut declared = Vec::new();
    for (id, decl) in &manifest.classes {
        let number: u32 = id
            .parse()
            .ok()
            .filter(|&number| number < u32::from(NO_CLASS))
            .ok_or_else(|| {
                format!("classes: {id:?} is not a class id 0..=254 (255 is reserved for no class)")
            })?;
        declared.push(declare(number, decl, &format!("classes.{id}"))?);
    }
    let class_of = match (&maps.class, &manifest.material_class) {
        (Some(image), _) => {
            let mut lookup = [NO_CLASS; 256];
            for (index, class) in declared.iter().enumerate() {
                lookup[class.id as usize] = index as u8;
            }
            let step = manifest.class_step as f32 / 255.0;
            (0..count)
                .map(|i| lookup[((image.at(i, 0) / step).round() as usize).min(255)])
                .collect()
        }
        (None, Some(decl)) => {
            if declared.len() >= usize::from(NO_CLASS) {
                return Err(String::from(
                    "material_class: 255 classes are already declared (at most 255 in all)",
                ));
            }
            declared.push(declare(0, decl, "material_class")?);
            vec![(declared.len() - 1) as u8; count]
        }
        (None, None) => vec![NO_CLASS; count],
    };
    for class in &declared {
        let Some(name) = &class.vocab else { continue };
        let rule = format!("PBR-VOCAB (id {})", class.id);
        match class.entry {
            Some(entry) => {
                let base = class_range(entry.class).expect("vocabulary classes are §2 classes");
                let replaces = match (entry.albedo, entry.roughness) {
                    (Some(_), Some(_)) => format!(
                        "replaces §2 {} albedo {}-{}, roughness {}-{}",
                        base.rule, base.albedo.0, base.albedo.1, base.roughness.0, base.roughness.1
                    ),
                    (Some(_), None) => format!(
                        "replaces §2 {} albedo {}-{}",
                        base.rule, base.albedo.0, base.albedo.1
                    ),
                    (None, Some(_)) => format!(
                        "replaces §2 {} roughness {}-{}",
                        base.rule, base.roughness.0, base.roughness.1
                    ),
                    (None, None) => format!("§2 {} ranges kept", base.rule),
                };
                let chips = if entry.metal_chips {
                    "; chips at metalness ≥ 0.9 are left out of the dielectric check"
                } else {
                    ""
                };
                report.push(
                    &rule,
                    None,
                    format!("{}: §3 applies", entry.name),
                    format!(
                        "{}; {replaces}{chips} (§3 read as the more specific rule, not yet \
                         confirmed by the maintainer; ADR-0022 amendment 2)",
                        entry.source,
                    ),
                );
            }
            None => report.push(
                &rule,
                None,
                format!("{name:?}: no §2/§3 conflict recorded"),
                format!("§2 {} applies", class.range.rule),
            ),
        }
    }
    Ok((declared, class_of))
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
    let size = (maps.base.width, maps.base.height);
    let (width, height) = size;
    let count = maps.base.len();

    // Coverage: the models' UVs, or every texel of a tiling layer.
    let mut area = vec![0f32; count];
    if models.is_empty() {
        area.fill(1.0);
    } else {
        let mut visits = 0u64;
        let mut outcome = Raster::Done;
        for geometry in &models {
            outcome = rasterise_uvs(
                &geometry.positions,
                &geometry.uvs,
                &geometry.indices,
                size,
                &mut area,
                &mut visits,
            )?;
            if outcome == Raster::OverBudget {
                break;
            }
        }
        if outcome == Raster::OverBudget {
            area.fill(1.0);
            report.push(
                "UV-COVERAGE",
                None,
                String::from("coverage not measured"),
                format!(
                    "the UVs tile past the raster budget ({} texel visits): every texel counts \
                     as covered, padding included, and class shares are texel shares",
                    raster_budget(width, height)
                ),
            );
        } else {
            let covered = area.iter().filter(|&&a| a > 0.0).count();
            report.uv_coverage = Some(round(covered as f32 / count as f32, 4));
        }
    }
    let area: &[f32] = &area;
    let covered = move || (0..count).filter(move |&i| area[i] > 0.0);
    let covered_count = covered().count();
    if covered_count == 0 {
        return Err(String::from("the model's UVs cover no texel of maps.base"));
    }
    let share_of = |texels: usize| texels as f32 / covered_count as f32;
    let total_area: f64 = covered().map(|i| f64::from(area[i])).sum();

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
        let tiled = manifest
            .tile_m
            .filter(|&tile| tile.is_finite() && tile > 0.0)
            .map(|tile| {
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
            (None, Some((lo, hi))) => report.push(
                &rule,
                Some(false),
                String::from("nothing to measure"),
                format!("{lo:.0}-{hi:.0} px/m needs [[models]] with UV'd triangles, or tile_m > 0"),
            ),
            (None, None) => report.push(
                &rule,
                None,
                String::from("not measured"),
                "no px/m target for this class (needs [[models]] or tile_m to report)",
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
    let metal_at = |i: usize| maps.metallic.as_ref().map_or(0.0, |map| map.at(i));
    let rough_at = |i: usize| maps.roughness.as_ref().map_or(1.0, |map| map.at(i));

    // PBR-CLASS-1 and the class of every texel.
    let (declared, class_of) = classify(&manifest, &maps, &mut report)?;
    let entry_of = |i: usize| -> Option<&'static VocabEntry> {
        declared
            .get(usize::from(class_of[i]))
            .and_then(|class| class.entry)
    };
    let classed = covered().filter(|&i| class_of[i] != NO_CLASS).count();
    report.push(
        "PBR-CLASS-1",
        Some(classed == covered_count),
        format!(
            "{:.4} of covered texels ({} without)",
            share_of(classed),
            covered_count - classed
        ),
        if maps.class.is_some() || manifest.material_class.is_some() {
            "100 % have a declared class"
        } else {
            "100 % have a declared class: no class-ID map (maps.class) or material_class given"
        },
    );

    // PBR-ALB-1, PBR-ALB-2, PBR-MET-1.
    {
        let mut dielectric: Vec<f32> = covered()
            .filter(|&i| metal_at(i) < 0.5)
            .map(|i| lumas[i])
            .collect();
        dielectric.sort_unstable_by(f32::total_cmp);
        if !dielectric.is_empty() {
            let (p1, p99) = (
                percentile_sorted(&dielectric, 1.0),
                percentile_sorted(&dielectric, 99.0),
            );
            report.push(
                "PBR-ALB-1",
                Some(p1 >= 30.0 && p99 <= 240.0),
                format!("p1 {p1:.1}, p99 {p99:.1} ({} texels)", dielectric.len()),
                "dielectric luma p1 ≥ 30, p99 ≤ 240",
            );
        }
    }
    {
        let mut dark = 0usize;
        let mut metal: Vec<f32> = covered()
            .filter(|&i| metal_at(i) >= 0.5)
            .filter(|&i| {
                let is_dark = entry_of(i).is_some_and(|entry| entry.dark_metal);
                dark += usize::from(is_dark);
                !is_dark
            })
            .map(|i| lumas[i])
            .collect();
        let note = if dark > 0 {
            format!(
                "metal luma p1 ≥ 180 ({dark} texels of a §3 dark metal left out: Emitter lattice)"
            )
        } else {
            String::from("metal luma p1 ≥ 180")
        };
        if let Some(p1) = percentile(&mut metal, 1.0) {
            report.push(
                "PBR-ALB-2",
                Some(p1 >= 180.0),
                format!("p1 {p1:.1} ({} texels)", metal.len()),
                note,
            );
        } else if dark > 0 {
            report.push(
                "PBR-ALB-2",
                None,
                String::from("no bright-metal texels"),
                note,
            );
        }
    }
    if maps.metallic.is_some() {
        let transition = covered()
            .filter(|&i| metal_at(i) > 0.1 && metal_at(i) < 0.9)
            .count();
        let transition = share_of(transition);
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

    // PBR-RGH-2: the roughness floor (0.045 on §3 entries whose own range goes below 0.08).
    if maps.roughness.is_some() {
        let unit = matches!(manifest.class, AssetClass::Hero | AssetClass::Minion);
        let floor_at = |i: usize| {
            if unit && !entry_of(i).is_some_and(|entry| entry.glossy) {
                0.08
            } else {
                0.045
            }
        };
        let (mut lowest, mut under) = (f32::INFINITY, 0usize);
        for i in covered() {
            let rough = rough_at(i);
            lowest = lowest.min(rough);
            under += usize::from(rough < floor_at(i) - 0.5 / 255.0);
        }
        let glossy: Vec<&str> = declared
            .iter()
            .filter_map(|class| class.entry.filter(|entry| entry.glossy))
            .map(|entry| entry.name)
            .collect();
        let limit = match (unit, glossy.is_empty()) {
            (true, false) => format!(
                "≥ 0.08 everywhere, ≥ 0.045 on §3 entries below 0.08 ({})",
                glossy.join(", ")
            ),
            (true, true) => String::from("≥ 0.08 everywhere"),
            (false, _) => String::from("≥ 0.045 everywhere"),
        };
        report.push(
            "PBR-RGH-2",
            Some(under == 0),
            format!("min {lowest:.3}, {under} texels under"),
            limit,
        );
    } else {
        report.push(
            "PBR-RGH-2",
            Some(false),
            String::from("no roughness map"),
            "maps.metallic_roughness or maps.roughness is required",
        );
    }

    // Interior texels: all 8 neighbours covered and of the same class (class borders are
    // filtered mixes of two materials). A bitset: 2 MiB at 4096².
    let mut interior_bits = vec![0u64; count.div_ceil(64)];
    let (cols, rows) = (width as usize, height as usize);
    for i in covered() {
        let class = class_of[i];
        if class == NO_CLASS {
            continue;
        }
        let (column, row) = (i % cols, i / cols);
        if column == 0 || row == 0 || column + 1 >= cols || row + 1 >= rows {
            continue;
        }
        let inside = (row - 1..=row + 1).all(|ny| {
            (column - 1..=column + 1).all(|nx| {
                let n = ny * cols + nx;
                area[n] > 0.0 && class_of[n] == class
            })
        });
        if inside {
            interior_bits[i / 64] |= 1 << (i % 64);
        }
    }
    let interior = |i: usize| interior_bits[i / 64] >> (i % 64) & 1 == 1;

    // Per-class ranges and PBR-RGH-1 on interior texels.
    let mut class_area = vec![0f64; declared.len()];
    for i in covered() {
        if let Some(sum) = class_area.get_mut(usize::from(class_of[i])) {
            *sum += f64::from(area[i]);
        }
    }
    for (index, class) in declared.iter().enumerate() {
        let (id, range) = (class.id, &class.range);
        *report
            .class_share
            .entry(format!("{id}:{}", range.key))
            .or_default() += (class_area[index] / total_area * 10_000.0).round() / 10_000.0;
        let texels: Vec<u32> = covered()
            .filter(|&i| usize::from(class_of[i]) == index && interior(i))
            .map(|i| i as u32)
            .collect();
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
        let range_note = class
            .entry
            .map_or(String::new(), |entry| format!(" (§3 {})", entry.name));
        {
            let mut albedo: Vec<f32> = texels.iter().map(|&i| lumas[i as usize]).collect();
            albedo.sort_unstable_by(f32::total_cmp);
            let (a1, a99) = (
                percentile_sorted(&albedo, 1.0),
                percentile_sorted(&albedo, 99.0),
            );
            report.push(
                &format!("{name} albedo"),
                Some(a1 >= range.albedo.0 && a99 <= range.albedo.1),
                format!("p1 {a1:.1}, p99 {a99:.1} ({} texels)", texels.len()),
                format!("luma {}-{}{range_note}", range.albedo.0, range.albedo.1),
            );
        }
        if maps.roughness.is_some() {
            let mut rough: Vec<f32> = texels.iter().map(|&i| rough_at(i as usize)).collect();
            let sigma = std_dev(&rough);
            rough.sort_unstable_by(f32::total_cmp);
            let at = |q: f32| percentile_sorted(&rough, q);
            let (r1, r5, r95, r99) = (at(1.0), at(5.0), at(95.0), at(99.0));
            let (lo, hi) = range.roughness;
            report.push(
                &format!("{name} roughness"),
                Some(r1 >= lo && r99 <= hi),
                format!("p1 {r1:.3}, p99 {r99:.3}"),
                format!("{lo}-{hi}{range_note}"),
            );
            let rule = format!("PBR-RGH-1 (id {id} {})", range.key);
            if class.entry.is_some_and(|entry| entry.wet) {
                report.push(
                    &rule,
                    None,
                    format!("σ {sigma:.4}, p95-p5 {:.4}", r95 - r5),
                    "exempt: a wet mask (material-vocabulary §1.1)",
                );
            } else if range.key != "GLASS" {
                report.push(
                    &rule,
                    Some(sigma >= 0.03 && r95 - r5 >= 0.08),
                    format!("σ {sigma:.4}, p95-p5 {:.4}", r95 - r5),
                    "σ ≥ 0.03 and p95 − p5 ≥ 0.08",
                );
            }
        }
        if let Some(want_metal) = range.metal
            && maps.metallic.is_some()
        {
            let mut metalness: Vec<f32> = texels.iter().map(|&i| metal_at(i as usize)).collect();
            metalness.sort_unstable_by(f32::total_cmp);
            // A §3 entry with metal chips: texels at metalness ≥ 0.9 sort last and are left out
            // of the dielectric reading (partial metalness still counts against it).
            let chips = if class.entry.is_some_and(|entry| entry.metal_chips) && !want_metal {
                metalness.iter().filter(|&&m| m >= 0.9).count()
            } else {
                0
            };
            let rest = &metalness[..metalness.len() - chips];
            let m1 = percentile_sorted(&metalness, 1.0);
            let m99 = if rest.is_empty() {
                0.0
            } else {
                percentile_sorted(rest, 99.0)
            };
            let (ok, limit) = if want_metal {
                (
                    m1 >= 0.9,
                    String::from("metal: p1 ≥ 0.9 (outside the 0.1-0.9 transition band)"),
                )
            } else if chips > 0 {
                (
                    m99 <= 0.1,
                    format!(
                        "dielectric: p99 ≤ 0.1, chipped scale left out ({:.4} of the class \
                         at metalness ≥ 0.9, §3 metal chips)",
                        chips as f32 / metalness.len() as f32
                    ),
                )
            } else {
                (m99 <= 0.1, String::from("dielectric: p99 ≤ 0.1"))
            };
            report.push(
                &format!("{name} metalness"),
                Some(ok),
                format!("p1 {m1:.3}, p99 {m99:.3}"),
                limit,
            );
        }
    }

    // PBR-NRM-1 and PBR-NRM-2 (on the source PNG: NRM-2 is a proxy for the BC5 bake).
    match &manifest.maps.normal {
        None => report.push(
            "PBR-NRM-1",
            Some(false),
            String::from("no normal map"),
            "maps.normal is required (PBR-NRM-1, PBR-NRM-2)",
        ),
        Some(name) => {
            let normal = read_same(dir, name, "normal", size)?;
            check_normals(
                &normal,
                manifest.normal_xy,
                covered,
                covered_count,
                &mut report,
            );
        }
    }

    // PBR-DELIGHT-1: 8×8-blurred albedo luma against the baked AO, within each class's
    // interior texels (a dark material in the cavities is not painted-in shadow).
    if let Some(name) = &manifest.maps.ao {
        let ao = read_same(dir, name, "ao", size)?;
        let block = 8usize;
        let mut samples: Vec<(Vec<f64>, Vec<f64>)> = vec![(Vec::new(), Vec::new()); declared.len()];
        for by in 0..rows / block {
            for bx in 0..cols / block {
                let first = class_of[by * block * cols + bx * block];
                let Some((xs, ys)) = samples.get_mut(usize::from(first)) else {
                    continue;
                };
                let (mut ok, mut sum_l, mut sum_a) = (true, 0f64, 0f64);
                for y in by * block..(by + 1) * block {
                    for x in bx * block..(bx + 1) * block {
                        let i = y * cols + x;
                        ok &= interior(i) && class_of[i] == first;
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
        if declared.is_empty() {
            report.push(
                "PBR-DELIGHT-1",
                None,
                String::from("no declared class"),
                "measured within each class's interior texels",
            );
        }
        for (class, (xs, ys)) in declared.iter().zip(&samples) {
            let rule = format!("PBR-DELIGHT-1 (id {} {})", class.id, class.range.key);
            match correlation(xs, ys) {
                Some(r) => report.push(
                    &rule,
                    Some(r < 0.3),
                    format!("r {r:.3} over {} blocks", xs.len()),
                    "correlation of 8×8-blurred albedo luma with AO < 0.3, within the class",
                ),
                None => report.push(
                    &rule,
                    None,
                    format!("{} interior 8×8 blocks", xs.len()),
                    "too few, or no variation, to correlate",
                ),
            }
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
    let ground = manifest.class == AssetClass::Terrain;
    {
        let mut chroma: Vec<f32> = covered()
            .map(|i| oklch_display(maps.base.rgb(i))[1])
            .collect();
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
    }
    if ground {
        let mut lightness: Vec<f32> = covered()
            .map(|i| oklch_display(maps.base.rgb(i))[0])
            .collect();
        lightness.sort_unstable_by(f32::total_cmp);
        let (p1, p99) = (
            percentile_sorted(&lightness, 1.0),
            percentile_sorted(&lightness, 99.0),
        );
        report.push(
            "VAL-1",
            Some(p1 >= 0.32 && p99 <= 0.62),
            format!("L p1 {p1:.3}, p99 {p99:.3}"),
            "terrain albedo L 0.32-0.62 (read at p1 / p99)",
        );
    }

    // Emissive: PAL-4 / PAL-5, PBR-EMIT-1, PBR-EMIT-2.
    let l_ref = manifest.lighting.l_ref()?;
    match (&manifest.maps.emissive, &manifest.emissive) {
        (Some(name), Some(decl)) => {
            let mask = read_same(dir, name, "emissive", size)?;
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
                format!("≤ +{cap_ev:.0} EV ({usage}), one L_ref for the map's lighting"),
            );
            let gain = y / l_ref;
            let lit: f64 = covered()
                .filter(|&i| mask.at(i, 0) * gain > 0.25)
                .map(|i| f64::from(area[i]))
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
        (None, decl) => report.push(
            "PBR-EMIT-2",
            None,
            String::from("no emissive map"),
            if decl.is_some() {
                "nothing emits (the [emissive] table has no maps.emissive to apply to)"
            } else {
                "nothing emits"
            },
        ),
    }

    // PBR-WEAR-1 (units) and TINT-1.
    if manifest.class.is_unit() {
        match manifest.dust_class {
            Some(dust) => {
                let index = declared.iter().position(|class| class.id == dust);
                let share: f64 = covered()
                    .filter(|&i| index.is_some_and(|index| usize::from(class_of[i]) == index))
                    .map(|i| f64::from(area[i]))
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
        let (pass, value) = match &manifest.maps.team_mask {
            None => (false, String::from("no team mask")),
            Some(name) => match read_same(dir, name, "team_mask", size) {
                Ok(mask) => {
                    let marked = covered().filter(|&i| mask.at(i, 0) >= 0.5).count();
                    (
                        true,
                        format!("{:.4} of covered texels marked", share_of(marked)),
                    )
                }
                Err(error) => (false, error),
            },
        };
        report.push(
            "TINT-1",
            Some(pass),
            value,
            "team mask loads at the maps' size (Compat keeps the tint)",
        );
    }
    for (rule, why) in NOT_BUILT {
        report.push(rule, None, String::from("not built"), why);
    }
    Ok(report)
}

/// PBR-NRM-1 (stored length) and PBR-NRM-2 (X² + Y² ≤ 1) on the source normal map. A grey +
/// alpha map is X in grey and Y in alpha; `xy` says an RGB map holds X and Y in R and G only.
fn check_normals<I: Iterator<Item = usize>>(
    normal: &Image,
    xy: bool,
    covered: impl Fn() -> I,
    covered_count: usize,
    report: &mut SourceReport,
) {
    if normal.channels == 1 {
        report.push(
            "PBR-NRM-1",
            Some(false),
            String::from("grey normal map"),
            "a normal map needs X and Y (RG, RGB, or grey + alpha)",
        );
        return;
    }
    // One 8-bit code is 2/255 in [-1, 1]: rounding moves a component by up to 1/255.
    let quantum = if normal.sixteen_bit {
        1.0 / 65535.0
    } else {
        1.0 / 255.0
    };
    let decode = |i: usize, c: usize| normal.at(i, c) * 2.0 - 1.0;
    let (y_channel, xy_only) = if normal.channels == 2 {
        (3, true)
    } else {
        (1, xy)
    };
    if xy_only {
        report.push(
            "PBR-NRM-1",
            None,
            format!(
                "XY only (Y in {})",
                if y_channel == 3 { "alpha" } else { "G" }
            ),
            "an XY map has no stored length",
        );
    } else {
        let mut worst = 0f32;
        let within = covered()
            .filter(|&i| {
                let n = [decode(i, 0), decode(i, 1), decode(i, 2)];
                let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                worst = worst.max((length - 1.0).abs());
                (length - 1.0).abs() <= 0.02
            })
            .count();
        report.push(
            "PBR-NRM-1",
            Some(within == covered_count),
            format!(
                "{:.4} within, worst |len − 1| {worst:.3}",
                within as f32 / covered_count as f32
            ),
            "every texel 1 ± 0.02",
        );
    }
    let over = covered()
        .filter(|&i| {
            let x = (decode(i, 0).abs() - quantum).max(0.0);
            let y = (decode(i, y_channel).abs() - quantum).max(0.0);
            x * x + y * y > 1.0
        })
        .count();
    report.push(
        "PBR-NRM-2",
        Some(over == 0),
        format!("{over} texels over"),
        "X² + Y² ≤ 1 per texel, on the source PNG as a proxy for the BC5 bake (within one code)",
    );
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
    use crate::imaging::{Image, MAX_PNG_SIDE, oklch_display};
    use std::path::PathBuf;

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

    /// Rasterises one model into a fresh `width × height` coverage.
    fn raster(
        positions: &[[f32; 3]],
        uvs: &[[f32; 2]],
        indices: &[u32],
        size: (u32, u32),
    ) -> Result<(Raster, Vec<f32>), String> {
        let mut area = vec![0f32; size.0 as usize * size.1 as usize];
        let mut visits = 0;
        rasterise_uvs(positions, uvs, indices, size, &mut area, &mut visits)
            .map(|outcome| (outcome, area))
    }

    const QUAD: [[f32; 3]; 4] = [
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [2.0, 2.0, 0.0],
        [0.0, 2.0, 0.0],
    ];

    #[test]
    fn uv_raster_covers_a_square_and_spreads_its_area() {
        // A 2 m × 2 m quad mapped to the left half of an 8×8 texture.
        let uvs = [[0.0, 0.0], [0.5, 0.0], [0.5, 1.0], [0.0, 1.0]];
        let indices = [0, 1, 2, 0, 2, 3];
        let (outcome, area) = raster(&QUAD, &uvs, &indices, (8, 8)).expect("raster");
        assert_eq!(outcome, Raster::Done);
        let covered = area.iter().filter(|&&a| a > 0.0).count();
        assert_eq!(covered, 32);
        let total: f32 = area.iter().sum();
        assert!((total - 4.0).abs() < 1e-4);
        // Left half only.
        assert!(area[7] == 0.0 && area[0] > 0.0);
        // 4 texels across 2 m and 8 down 2 m: sqrt(32 / 4) = 2.83 px/m.
        let (p5, p50, p95) = texel_density(&QUAD, &uvs, &indices, 8, 8).expect("density");
        assert!(
            (p50 - 8f32.sqrt()).abs() < 1e-4 && (p5 - p50).abs() < 1e-6 && (p95 - p50).abs() < 1e-6
        );
    }

    #[test]
    fn uv_raster_wraps_tiling_uvs() {
        // A trim strip whose UVs run from 1.0 to 1.5: it lands on the left half, wrapped.
        let uvs = [[1.0, 0.0], [1.5, 0.0], [1.5, 1.0], [1.0, 1.0]];
        let (_, area) = raster(&QUAD, &uvs, &[0, 1, 2, 0, 2, 3], (4, 4)).expect("raster");
        let covered: Vec<usize> = (0..16).filter(|&i| area[i] > 0.0).collect();
        assert_eq!(covered, [0, 1, 4, 5, 8, 9, 12, 13]);
    }

    #[test]
    fn huge_uvs_are_refused_not_looped_over() {
        // u = ±1e30 is finite: the old cast saturated and the span wrapped (a hang).
        let across = [[-1e30, 0.0], [1e30, 0.0], [0.0, 1.0], [0.0, 1.0]];
        let error = raster(&QUAD, &across, &[0, 1, 2], (16, 16)).expect_err("huge span");
        assert!(error.contains("beyond any tiling"), "{error}");
        // Far from the origin but a tiny triangle: still no tiling explains it.
        let far = [[1e30, 0.0], [1e30, 0.5], [1e30, 1.0], [0.0, 0.0]];
        assert!(raster(&QUAD, &far, &[0, 1, 2], (16, 16)).is_err());
    }

    #[test]
    fn uvs_tiling_past_the_budget_are_not_measured() {
        // 1e5 repeats each way on a 16² map: 2.6e12 visits, far past 2^28.
        let uvs = [[0.0, 0.0], [1e5, 0.0], [1e5, 1e5], [0.0, 1e5]];
        let (outcome, _) = raster(&QUAD, &uvs, &[0, 1, 2, 0, 2, 3], (16, 16)).expect("raster");
        assert_eq!(outcome, Raster::OverBudget);
        // The budget follows the map: 64 visits per texel, never under 2^28.
        assert_eq!(raster_budget(16, 16), 1 << 28);
        assert_eq!(raster_budget(4096, 4096), 64 * 4096 * 4096);
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

    /// Writes an 8-bit PNG of `colour` from interleaved samples.
    fn write_png_as(path: &Path, width: u32, height: u32, colour: png::ColorType, bytes: &[u8]) {
        let file = std::fs::File::create(path).expect("create");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
        encoder.set_color(colour);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("header");
        writer.write_image_data(bytes).expect("data");
    }

    /// Writes an RGBA8 PNG.
    fn write_png(path: &Path, width: u32, height: u32, rgba: &[[u8; 4]]) {
        let bytes: Vec<u8> = rgba.iter().flatten().copied().collect();
        write_png_as(path, width, height, png::ColorType::Rgba, &bytes);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("moba-asset-check-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// Noisy roughness around `centre` (± 0.07 over five steps), metal 0, in the engine's
    /// packing (R metallic, G roughness).
    fn mr_map(n: usize, centre: f32) -> Vec<[u8; 4]> {
        (0..n)
            .map(|i| {
                let rough = centre - 0.07 + 0.035 * (i % 5) as f32;
                [0, (rough * 255.0).round() as u8, 0, 255]
            })
            .collect()
    }

    /// A `side`² set: base of `albedo` with a little jitter, `mr_map` around `rough`, a flat
    /// normal. Returns the directory's manifest path after writing `manifest`.
    fn set(dir: &Path, side: u32, albedo: [u8; 3], rough: f32, manifest: &str) -> PathBuf {
        let n = (side * side) as usize;
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
        write_png(&dir.join("base.png"), side, side, &base);
        write_png(&dir.join("mr.png"), side, side, &mr_map(n, rough));
        write_png(
            &dir.join("normal.png"),
            side,
            side,
            &vec![[128, 128, 255, 255]; n],
        );
        let path = dir.join("x.source.toml");
        std::fs::write(&path, manifest).expect("manifest");
        path
    }

    const MAPS: &str =
        "[maps]\nbase = \"base.png\"\nmetallic_roughness = \"mr.png\"\nnormal = \"normal.png\"\n";

    /// A 16×16 terrain-style layer: noisy roughness, a flat normal, one class.
    fn layer(dir: &Path, albedo: [u8; 3]) -> PathBuf {
        set(
            dir,
            16,
            albedo,
            0.93,
            &format!(
                "asset = \"ash\"\nclass = \"terrain\"\ntile_m = 4.0\nshipped_size = 1024\n\
                 material_class = \"CHAR\"\n{MAPS}"
            ),
        )
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
        assert_eq!(rule(&good, "PBR-NRM-2").pass, Some(true));
        // Crushed to luma ~20: PBR-ALB-1, the CHAR range and VAL-1 fail.
        let bad = check_source(&layer(&dir, [20, 20, 20])).expect("check");
        assert!(!bad.passed());
        assert_eq!(rule(&bad, "PBR-ALB-1").pass, Some(false));
        assert_eq!(rule(&bad, "PBR-CHAR-1 (id 0) albedo").pass, Some(false));
        assert_eq!(rule(&bad, "VAL-1").pass, Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_maps_and_density_inputs_fail_rather_than_skip() {
        let dir = scratch("missing");
        let ash = [0x6B, 0x65, 0x60];
        // No normal map, no tile_m: PBR-NRM-1 and TD-1 fail (terrain has a px/m target).
        let path = set(
            &dir,
            16,
            ash,
            0.93,
            "asset = \"ash\"\nclass = \"terrain\"\nmaterial_class = \"CHAR\"\n\
             [maps]\nbase = \"base.png\"\nmetallic_roughness = \"mr.png\"\n",
        );
        let report = check_source(&path).expect("check");
        assert_eq!(rule(&report, "PBR-NRM-1").pass, Some(false));
        assert_eq!(rule(&report, "TD-1 (ash)").pass, Some(false));
        // tile_m = 0 is no tile size either.
        std::fs::write(
            &path,
            format!(
                "asset = \"ash\"\nclass = \"terrain\"\ntile_m = 0.0\nmaterial_class = \"CHAR\"\n{MAPS}"
            ),
        )
        .expect("manifest");
        let report = check_source(&path).expect("check");
        assert_eq!(rule(&report, "TD-1 (ash)").pass, Some(false));
        // No metallic-roughness map: PBR-MET-1 and PBR-RGH-2 fail.
        std::fs::write(
            &path,
            "asset = \"ash\"\nclass = \"terrain\"\ntile_m = 4.0\nmaterial_class = \"CHAR\"\n\
             [maps]\nbase = \"base.png\"\nnormal = \"normal.png\"\n",
        )
        .expect("manifest");
        let report = check_source(&path).expect("check");
        assert_eq!(rule(&report, "PBR-MET-1").pass, Some(false));
        assert_eq!(rule(&report, "PBR-RGH-2").pass, Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transition_metalness_and_roughness_floor_fail() {
        let dir = scratch("metal");
        // A hero in leather: half its texels at metalness 0.5 and roughness 0.05.
        let path = set(
            &dir,
            16,
            [0x7A, 0x5A, 0x40],
            0.6,
            &format!("asset = \"hero\"\nclass = \"hero\"\nmaterial_class = \"LEATHER\"\n{MAPS}"),
        );
        let mr: Vec<[u8; 4]> = (0..256)
            .map(|i| {
                if i % 2 == 0 {
                    [128, 13, 0, 255]
                } else {
                    [0, 150, 0, 255]
                }
            })
            .collect();
        write_png(&dir.join("mr.png"), 16, 16, &mr);
        let report = check_source(&path).expect("check");
        assert_eq!(rule(&report, "PBR-MET-1").pass, Some(false));
        assert_eq!(rule(&report, "PBR-RGH-2").pass, Some(false));
        assert_eq!(rule(&report, "TINT-1").pass, Some(false)); // no team mask
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn emissive_rules_fail_on_hue_brightness_and_area() {
        let dir = scratch("emit");
        let manifest = |colour: &str, strength: f32| {
            format!(
                "asset = \"prop\"\nclass = \"prop\"\nmaterial_class = \"STONE-1\"\n\
                 [emissive]\ncolour = \"{colour}\"\nstrength = {strength}\nuse = \"accent\"\n\
                 {MAPS}emissive = \"emit.png\"\n"
            )
        };
        let path = set(
            &dir,
            16,
            [0x4A, 0x47, 0x44],
            0.75,
            &manifest("#A08CD0", 0.1),
        );
        // A quarter of the texels lit.
        let emit: Vec<[u8; 4]> = (0..256)
            .map(|i| {
                if i < 64 {
                    [255, 255, 255, 255]
                } else {
                    [0, 0, 0, 255]
                }
            })
            .collect();
        write_png(&dir.join("emit.png"), 16, 16, &emit);
        let report = check_source(&path).expect("check");
        assert_eq!(rule(&report, "PAL-5").pass, Some(true));
        assert_eq!(rule(&report, "PBR-EMIT-1").pass, Some(true));
        assert_eq!(rule(&report, "PBR-EMIT-2").pass, Some(false)); // 0.25 > 0.03
        // Saturated orange inside the CB-orange zone, far too bright.
        std::fs::write(&path, manifest("#FF8000", 40.0)).expect("manifest");
        let report = check_source(&path).expect("check");
        assert_eq!(rule(&report, "PAL-4").pass, Some(false));
        assert_eq!(rule(&report, "PBR-EMIT-1").pass, Some(false));
        // Saturated violet outside every zone: PAL-5 at C > 0.12.
        std::fs::write(&path, manifest("#8000FF", 0.1)).expect("manifest");
        let report = check_source(&path).expect("check");
        assert_eq!(rule(&report, "PAL-5").pass, Some(false));
        // An emissive map without the table is an error, not a silent skip.
        std::fs::write(
            &path,
            format!(
                "asset = \"prop\"\nclass = \"prop\"\nmaterial_class = \"STONE-1\"\n\
                 {MAPS}emissive = \"emit.png\"\n"
            ),
        )
        .expect("manifest");
        assert!(check_source(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn normal_maps_fail_on_length_and_read_xy_from_the_right_channel() {
        let dir = scratch("normal");
        let path = layer(&dir, [0x6B, 0x65, 0x60]);
        // Unnormalised (0.5 long) and over-long XY: both rules fail.
        write_png(
            &dir.join("normal.png"),
            16,
            16,
            &vec![[128, 128, 191, 255]; 256],
        );
        let report = check_source(&path).expect("check");
        assert_eq!(rule(&report, "PBR-NRM-1").pass, Some(false));
        write_png(
            &dir.join("normal.png"),
            16,
            16,
            &vec![[255, 255, 128, 255]; 256],
        );
        let report = check_source(&path).expect("check");
        assert_eq!(rule(&report, "PBR-NRM-2").pass, Some(false));
        // Grey + alpha: X in grey, Y in alpha. X = 0 and Y = +1 is valid; reading Y from the
        // grey would see X = 0, Y = 0 and pass either way, so make the alpha the bad one.
        let ga = |x: u8, y: u8| -> Vec<u8> { (0..256).flat_map(|_| [x, y]).collect() };
        write_png_as(
            &dir.join("normal.png"),
            16,
            16,
            png::ColorType::GrayscaleAlpha,
            &ga(128, 255),
        );
        let report = check_source(&path).expect("check");
        assert_eq!(rule(&report, "PBR-NRM-1").pass, None, "{}", report.text());
        assert_eq!(rule(&report, "PBR-NRM-2").pass, Some(true));
        write_png_as(
            &dir.join("normal.png"),
            16,
            16,
            png::ColorType::GrayscaleAlpha,
            &ga(255, 255),
        );
        let report = check_source(&path).expect("check");
        assert_eq!(rule(&report, "PBR-NRM-2").pass, Some(false));
        // RG in RGB with normal_xy: no length check, X² + Y² still checked.
        write_png(
            &dir.join("normal.png"),
            16,
            16,
            &vec![[128, 255, 0, 255]; 256],
        );
        let text = std::fs::read_to_string(&path).expect("read");
        std::fs::write(&path, format!("normal_xy = true\n{text}")).expect("write");
        let report = check_source(&path).expect("check");
        assert_eq!(rule(&report, "PBR-NRM-1").pass, None);
        assert_eq!(rule(&report, "PBR-NRM-2").pass, Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delight_fails_when_albedo_follows_the_ao_within_a_class() {
        let dir = scratch("delight");
        let path = set(
            &dir,
            32,
            [0x6B, 0x65, 0x60],
            0.93,
            &format!(
                "asset = \"ash\"\nclass = \"terrain\"\ntile_m = 4.0\nshipped_size = 1024\n\
                 material_class = \"CHAR\"\n{MAPS}ao = \"ao.png\"\n"
            ),
        );
        // AO by 8×8 block; albedo painted with the same cavities.
        let block = |i: usize| ((i % 32) / 8 + (i / 32) / 8 * 4) as u8;
        let ao: Vec<[u8; 4]> = (0..1024)
            .map(|i| {
                let v = 120 + 9 * block(i);
                [v, v, v, 255]
            })
            .collect();
        write_png(&dir.join("ao.png"), 32, 32, &ao);
        let base: Vec<[u8; 4]> = (0..1024)
            .map(|i| {
                let v = 70 + 4 * block(i);
                [v, v, v, 255]
            })
            .collect();
        write_png(&dir.join("base.png"), 32, 32, &base);
        let report = check_source(&path).expect("check");
        let delight = rule(&report, "PBR-DELIGHT-1 (id 0 CHAR)");
        assert_eq!(delight.pass, Some(false), "{}", report.text());
        assert!(delight.value.contains("4 blocks"), "{}", delight.value);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_vocab_entry_replaces_a_conflicting_section_2_range() {
        let dir = scratch("vocab");
        // Void composite, matte: CERAM at roughness 0.57-0.71, luma ~47.
        let manifest = |class: &str| {
            format!("asset = \"void\"\nclass = \"prop\"\nmaterial_class = {class}\n{MAPS}")
        };
        let path = set(&dir, 16, [0x31, 0x2D, 0x37], 0.64, &manifest("\"CERAM\""));
        let plain = check_source(&path).expect("check");
        assert_eq!(
            rule(&plain, "PBR-CERAM-1 (id 0) roughness").pass,
            Some(false)
        );
        std::fs::write(
            &path,
            manifest("{ class = \"CERAM\", vocab = \"void composite, matte\" }"),
        )
        .expect("manifest");
        let named = check_source(&path).expect("check");
        assert_eq!(
            rule(&named, "PBR-CERAM-1 (id 0) roughness").pass,
            Some(true)
        );
        assert!(
            rule(&named, "PBR-VOCAB (id 0)")
                .limit
                .contains("§3 read as the more specific")
        );
        // The entry must belong to the declared class.
        std::fs::write(
            &path,
            manifest("{ class = \"CLOTH\", vocab = \"Void composite, matte\" }"),
        )
        .expect("manifest");
        assert!(check_source(&path).is_err());
        // Every entry's class is a §2 class.
        for entry in &VOCAB {
            assert!(class_range(entry.class).is_some(), "{}", entry.name);
            assert!(vocab_entry(&entry.name.to_uppercase()).is_some());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_metal_chip_entry_leaves_chips_out_of_the_dielectric_check() {
        let dir = scratch("chips");
        // OXIDE at roughness 0.64, luma ~56, with 1 texel in 8 chipped to bare metal.
        let manifest = |class: &str| {
            format!("asset = \"alloy\"\nclass = \"prop\"\nmaterial_class = {class}\n{MAPS}")
        };
        let path = set(&dir, 16, [0x3A, 0x38, 0x36], 0.64, &manifest("\"OXIDE\""));
        let mut mr = mr_map(256, 0.64);
        for texel in mr.iter_mut().step_by(8) {
            texel[0] = 255;
        }
        write_png(&dir.join("mr.png"), 16, 16, &mr);
        let plain = check_source(&path).expect("check");
        assert_eq!(
            rule(&plain, "PBR-OXIDE-1 (id 0) metalness").pass,
            Some(false)
        );
        std::fs::write(
            &path,
            manifest("{ class = \"OXIDE\", vocab = \"Forge dark alloy, scaled\" }"),
        )
        .expect("manifest");
        let named = check_source(&path).expect("check");
        let metal = rule(&named, "PBR-OXIDE-1 (id 0) metalness");
        assert_eq!(metal.pass, Some(true), "{}", named.text());
        assert!(
            metal.limit.contains("chipped scale left out"),
            "{}",
            metal.limit
        );
        // Partial metalness is not a chip: it still fails.
        for texel in mr.iter_mut().step_by(8) {
            texel[0] = 128;
        }
        write_png(&dir.join("mr.png"), 16, 16, &mr);
        let half = check_source(&path).expect("check");
        assert_eq!(
            rule(&half, "PBR-OXIDE-1 (id 0) metalness").pass,
            Some(false)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn team_masks_are_loaded_and_size_checked() {
        let dir = scratch("tint");
        let manifest = format!(
            "asset = \"tower\"\nclass = \"structure\"\nmaterial_class = \"STONE-1\"\n\
             {MAPS}team_mask = \"team.png\"\n"
        );
        let path = set(&dir, 16, [0x4A, 0x47, 0x44], 0.75, &manifest);
        write_png(&dir.join("team.png"), 8, 8, &vec![[255, 255, 255, 255]; 64]);
        let report = check_source(&path).expect("check");
        let tint = rule(&report, "TINT-1");
        assert_eq!(tint.pass, Some(false));
        assert!(tint.value.contains("8x8"), "{}", tint.value);
        write_png(
            &dir.join("team.png"),
            16,
            16,
            &vec![[255, 255, 255, 255]; 256],
        );
        let report = check_source(&path).expect("check");
        assert_eq!(rule(&report, "TINT-1").pass, Some(true));
        std::fs::remove_file(dir.join("team.png")).expect("remove");
        let report = check_source(&path).expect("check");
        assert_eq!(rule(&report, "TINT-1").pass, Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    const SENTINEL: &[u8] = include_bytes!("../../../content/models/sentinel.glb");

    #[test]
    fn models_go_through_the_engines_loader_and_the_glb_rules() {
        let dir = scratch("glb");
        std::fs::write(dir.join("sentinel.glb"), SENTINEL).expect("model");
        let mut report = SourceReport::default();
        let decl = ModelDecl {
            path: String::from("sentinel.glb"),
            hover_m: 0.0,
        };
        let loaded = check_model(&decl, &dir.join("sentinel.glb"), &mut report);
        assert_eq!(rule(&report, "GLB-NAME (sentinel)").pass, Some(true));
        assert_eq!(rule(&report, "GLB-LOAD (sentinel)").pass, Some(true));
        let feet = rule(&report, "GLB-FEET (sentinel)").pass;
        let tangent = rule(&report, "GLB-TANGENT (sentinel)").pass;
        assert!(tangent.is_some() && feet.is_some());
        // A documented hover far from the mesh fails GLB-FEET.
        let mut hovering = SourceReport::default();
        let high = ModelDecl {
            path: String::from("sentinel.glb"),
            hover_m: 50.0,
        };
        let _ = check_model(&high, &dir.join("sentinel.glb"), &mut hovering);
        assert_eq!(rule(&hovering, "GLB-FEET (sentinel)").pass, Some(false));
        // A bad file stem fails GLB-NAME; a truncated file is refused by the loader.
        std::fs::write(dir.join("bad name.glb"), SENTINEL).expect("model");
        let mut named = SourceReport::default();
        let _ = check_model(&decl, &dir.join("bad name.glb"), &mut named);
        assert_eq!(rule(&named, "GLB-NAME (bad name)").pass, Some(false));
        std::fs::write(dir.join("broken.glb"), &SENTINEL[..64]).expect("model");
        let error = check_model(&decl, &dir.join("broken.glb"), &mut SourceReport::default())
            .expect_err("truncated");
        assert!(error.contains("loader refuses"), "{error}");
        // The geometry comes back for the UV checks when the mesh has UVs.
        if let Ok(geometry) = loaded {
            assert!(
                !geometry.positions.is_empty() && geometry.uvs.len() == geometry.positions.len()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_png_refuses_directories_garbage_and_oversized_images() {
        let dir = scratch("readpng");
        assert!(
            read_png(&dir)
                .expect_err("dir")
                .contains("not a regular file")
        );
        assert!(read_png(&dir.join("absent.png")).is_err());
        std::fs::write(dir.join("junk.png"), b"not a png at all").expect("write");
        assert!(read_png(&dir.join("junk.png")).is_err());
        // A real PNG cut short.
        write_png(&dir.join("whole.png"), 16, 16, &vec![[1, 2, 3, 255]; 256]);
        let bytes = std::fs::read(dir.join("whole.png")).expect("read");
        std::fs::write(dir.join("cut.png"), &bytes[..bytes.len() / 2]).expect("write");
        assert!(read_png(&dir.join("cut.png")).is_err());
        // One pixel past the side limit.
        let wide = vec![0u8; MAX_PNG_SIDE as usize + 1];
        write_png_as(
            &dir.join("wide.png"),
            MAX_PNG_SIDE + 1,
            1,
            png::ColorType::Grayscale,
            &wide,
        );
        let error = read_png(&dir.join("wide.png")).expect_err("too wide");
        assert!(error.contains("outside"), "{error}");
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
        std::fs::write(
            &path,
            "asset = \"x\"\nclass = \"prop\"\n[classes]\n256 = \"METAL\"\n[maps]\nbase = \"b.png\"\n",
        )
        .expect("write");
        assert!(check_source(&path).expect_err("id 256").contains("0..=254"));
        // 255 is the no-class sentinel: refused, so a declared class never reads as unclassed.
        std::fs::write(
            &path,
            "asset = \"x\"\nclass = \"prop\"\n[classes]\n255 = \"METAL\"\n[maps]\nbase = \"b.png\"\n",
        )
        .expect("write");
        assert!(
            check_source(&path)
                .expect_err("id 255")
                .contains("reserved")
        );
        // 255 ids (0..=254) plus material_class would be 256 classes: refused.
        let mut many =
            String::from("asset = \"x\"\nclass = \"prop\"\nmaterial_class = \"CHAR\"\n[classes]\n");
        for id in 0..255 {
            writeln!(many, "{id} = \"CHAR\"").expect("write");
        }
        many.push_str("[maps]\nbase = \"b.png\"\n");
        std::fs::write(&path, many).expect("write");
        assert!(
            check_source(&path)
                .expect_err("256 classes")
                .contains("at most 255")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn image_reads_sixteen_bit_grey_and_grey_alpha() {
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
        assert!((image.get(1, 0, 3) - 1.0).abs() < 1e-6); // no alpha reads 1
        let ga = dir.join("ga.png");
        write_png_as(&ga, 1, 1, png::ColorType::GrayscaleAlpha, &[51, 204]);
        let image = read_png(&ga).expect("read");
        assert_eq!(image.channels, 2);
        assert!((image.at(0, 1) - 0.2).abs() < 1e-6 && (image.at(0, 3) - 0.8).abs() < 1e-6);
        let rgb = dir.join("rgb.png");
        write_png_as(&rgb, 1, 1, png::ColorType::Rgb, &[51, 102, 204]);
        let image = read_png(&rgb).expect("read");
        assert_eq!(image.rgb(0), [0.2, 0.4, 0.8]);
        assert!((image.at(0, 3) - 1.0).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
