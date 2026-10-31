# Palette and value

**Status:** v0 draft, 2026-10-31. **Leads:** maintainer and graphic designer. Agents drafted
the numbers. Every threshold marked *proposed* waits for the maintainer and designer, and
becomes a check ([checks.md](checks.md)).

These swatches are **display colours** for boards and the review app. The authored PBR values
(albedo as luma, roughness, metalness) and their per-class checks are in
[material-vocabulary.md](material-vocabulary.md); the chroma rules (PAL-1 to PAL-5) here are
the single source for both documents.

Colours are given as sRGB hex plus **OKLCH** (L lightness 0–1, C chroma, H hue in degrees)
and **Y** (relative luminance, linear). OKLCH is used because equal steps look equal, which
makes "how saturated" and "how bright" testable. The values come from
`tools/oklch.py` in the review app folder (a plain sRGB → OKLab conversion).

## 1. Team colours stay the HUD's

**Decided:** factions by shape and material, teams by rim. Team colour comes only from rims,
outlines, health bars, selection rings, minimap icons and the HUD. Today's palettes are kept
as they are (`crates/client/src/hud/theme.rs`):

| Role | sRGB | L | C | H | Y |
|---|---|---|---|---|---|
| Team self (standard) | `#59DB52` | 0.79 | 0.211 | 142 | 0.534 |
| Team ally (standard) | `#458FFF` | 0.66 | 0.182 | 259 | 0.281 |
| Team enemy (standard) | `#EB3830` | 0.62 | 0.216 | 28 | 0.207 |
| Team self (colour-blind) | `#F0E442` | 0.90 | 0.172 | 105 | 0.744 |
| Team ally (colour-blind) | `#56B4E9` | 0.73 | 0.117 | 236 | 0.405 |
| Team enemy (colour-blind) | `#E69F00` | 0.75 | 0.158 | 77 | 0.416 |

> **Decision needed (designer + maintainer): keep green/blue/red?** It is the genre
> convention and players expect it. **Recommendation: keep it for v0.** Revisit in the UI pass
> ([ui-hud-typography.md](ui-hud-typography.md)), not here.

### Reserved team-colour zones

Every team colour has **C ≥ 0.117**. Their hues are 28°, 77°, 105°, 142°, 236° and 259°.
Within ±25° of those, saturated colour belongs to the team layer. The free hue bands are
roughly **167–211° (teal–cyan)** and **284–345° (violet–magenta)**. Rose (345–3°) is left
out on purpose: it reads as the enemy red.

So the warm desert and the Forge's brass sit **inside** the zones. That's fine because they
are low-chroma: brass is C 0.08 against the colour-blind enemy's 0.16 at the same hue. The
rule is a **chroma budget**, not a hue ban.

| Rule | Proposed threshold |
|---|---|
| **PAL-1** Ground in playable space (terrain layers, ground decals) | C ≤ **0.06** (p99 of texels) |
| **PAL-2** Every other surface: units, structures, props | C ≤ **0.08** (p99). The least saturated team colour (colour-blind sky, C 0.117) keeps a ≥ 1.45× gap |
| **PAL-3** Small accents (≤ 5 % of a unit's projected area) | C ≤ **0.13** outside the team zones; ≤ **0.11** inside them |
| **PAL-4** Emissive colour inside a team zone | C ≤ **0.06**: warm light must be white-gold, not orange-red |
| **PAL-5** Emissive colour outside the zones (e.g. Void violet) | C ≤ **0.12** |

These apply to authored albedo and emissive (`content-check`) and to the rendered frame
(the readability tests below), because lighting and grading can push chroma up.

## 2. Value: playable space is a mid-value stage

Units must read against the ground in a flat-lit, single-value silhouette (roadmap §4). So
the ground is kept in a narrow mid band, and units are allowed to go lighter or darker than it.

| Rule | What | Proposed threshold |
|---|---|---|
| **VAL-1** | Terrain-layer albedo (authored) in playable space | OKLCH L **0.32–0.62** (about sRGB 55–150 on a grey) |
| **VAL-2** | Rendered ground luminance in playable space, after grading, on every tier | Mean OKLab L **0.30–0.58**; 5th–95th percentile range ≤ **0.30** |
| **VAL-3** | Shadow floor in playable space (rendered) | L ≥ **0.18**: a unit in shadow still reads |
| **VAL-4** | Highlight ceiling for non-gameplay surfaces in playable space | L ≤ **0.85**. Only VFX, rims and UI may go above. |
| **VAL-5** | Off-board areas (cliffs above 6 m, the sky, the out-of-bounds basin) | Free, but the transition across the lane edge is ≤ **0.15** L per metre |

The debug **readability heatmap** (roadmap §4.3) paints pixels that break VAL-2 to VAL-4 and
PAL-1 to PAL-5 in playable space.

## 3. Contrast: units and telegraphs against the ground

Measured by the headless readability captures (roadmap §4.4) on standard scenes: a hero, a
minion wave and a tower on each terrain layer, under each lighting mood, on every tier.

| Rule | What | Proposed threshold |
|---|---|---|
| **CON-1** | Hero silhouette vs the surrounding ring (3× its bounding box), **without rims** | \|ΔL\| ≥ **0.10** (mean OKLab L) |
| **CON-2** | The same **with team rims** | \|ΔL\| ≥ **0.15**, and the rim pixels reach C ≥ **0.10** within ±15° of the team hue |
| **CON-3** | Enemy telegraph fill vs the ground | ΔE_OK ≥ **0.20**; edge ΔL ≥ **0.25** |
| **CON-4** | Minion vs ground, without rims | \|ΔL\| ≥ **0.08** |
| **CON-5** | A higher tier never lowers CON-1 to CON-4 below the Low result | ≥ Low − **0.02** |

## 4. Colour-blind safety

| Rule | What | Proposed threshold |
|---|---|---|
| **CB-1** | Team pairs (self/ally/enemy) under simulated protanopia, deuteranopia and tritanopia (Machado 2009), in both palettes | ΔE_OK ≥ **0.15** between every pair |
| **CB-2** | No faction or environment accent may become confusable with a team colour under simulation | ΔE_OK ≥ **0.10** from every simulated team colour, for accents with C > 0.08 |
| **CB-3** | Telegraphs keep CON-3 under simulation | as CON-3 |

The review app has protanopia/deuteranopia/tritanopia toggles to eyeball the same thing.

## 5. Faction palettes (draft)

### Solar Forge: warm metals, cream stone, white-gold light

| Swatch | sRGB | L | C | H | Use |
|---|---|---|---|---|---|
| Sun-brass | `#A98C62` | 0.66 | 0.067 | 76 | Armour plates, tower crowns (display swatch: the authored metal base colour is in [material-vocabulary.md](material-vocabulary.md)) |
| Worn brass | `#8C6E43` | 0.56 | 0.071 | 75 | Older plates, recesses |
| Gilt | `#C4A86E` | 0.74 | 0.083 | 85 | Accent only (PAL-3), worn through to the brass |
| Kiln terracotta | `#A06C55` | 0.58 | 0.075 | 45 | Tiles, unglazed ceramic |
| Forge sandstone | `#C4AE8C` | 0.76 | 0.053 | 79 | Masonry |
| Slag | `#4A3F38` | 0.38 | 0.019 | 54 | Ground at the Forge base, cast-off |
| Soot | `#2E2A26` | 0.29 | 0.009 | 67 | Streaks, recesses |
| Sun-glass core (emissive) | `#FFF0C8` | 0.96 | 0.054 | 90 | Light sources (PAL-4) |

### Void: dark polish, pale inlay, violet-white light

| Swatch | sRGB | L | C | H | Use |
|---|---|---|---|---|---|
| Void ceramic | `#2E2B33` | 0.30 | 0.015 | 303 | Bodies, monoliths |
| Obsidian | `#22212A` | 0.25 | 0.017 | 290 | Polished faces (reads by **specular**, not albedo) |
| Pewter | `#8A8A90` | 0.64 | 0.009 | 286 | Rings, fittings |
| Tarnished silver | `#B8B8BC` | 0.78 | 0.006 | 286 | Bright metal edges |
| Bone inlay | `#D6CFC2` | 0.86 | 0.019 | 83 | Star-chart lines, masks: keeps Void units off "black blob" |
| Plum lacquer | `#4A3646` | 0.36 | 0.038 | 333 | Cloth, lacquered panels |
| Void-glass core (emissive) | `#D9CCFF` | 0.87 | 0.071 | 297 | Light sources |
| Void-glass edge (emissive) | `#A08CD0` | 0.68 | 0.100 | 297 | Thin rims and cracks only (PAL-5) |

> **Decision needed (maintainer + designer): how dark may the Void get?** Its bodies are
> L 0.25–0.30, below the ground band (VAL-1), which keeps it distinct from the Forge. The
> risk is a dark blob on dark basalt. Options: (a) **dark bodies plus a mandatory bone or
> silver element on every Void unit, ≥ 10 % of its projected area (recommended)**; (b) lift
> the Void to mid-grey ceramics (safer, less identity); (c) leave it to CON-1 alone.

## 6. Ember Crossing palette (map 1)

| Swatch | sRGB | L | C | H | Use |
|---|---|---|---|---|---|
| Basalt | `#3E3B38` | 0.35 | 0.007 | 68 | Cliffs, terraces, rampart bases |
| Ash ground | `#5B4C44` | 0.43 | 0.024 | 50 | Base terrain layer |
| Lane grit | `#7E5A47` | 0.50 | 0.056 | 49 | Trodden lanes |
| Desert sand | `#B39A7C` | 0.70 | 0.051 | 72 | Drifts, off-lane, high ground |
| Ember crystal, today | `#FF6B1A` | 0.70 | **0.198** | 43 | Breaks PAL-4: sits between the enemy red and the colour-blind enemy orange |
| Ember crystal, proposed | `#FFD9A8` | 0.91 | 0.076 | 74 | White-gold core; colour comes from a thin orange **rim** in the crystal facets (≤ 3 % area) |

> **Decision needed (maintainer): the ember crystals.** The orange crystals are the map's
> signature, but at C 0.20 they look like enemy colour, which is the exact mistake §4 exists
> to stop. Options: (a) **white-gold cores with small orange facet rims, kept off the lanes
> (recommended)**; (b) keep the orange but only on props more than 12 m from any lane;
> (c) shift the crystals to the free teal band (C ≤ 0.13): very readable, but it loses "ember".

## 7. Grading

Each map's LUT (`content/grading/*.cube`) is authored **inside** these budgets. VAL-2 and
PAL-1 to PAL-5 are checked after grading, so a LUT can't push the board out of them. Lighting
moods and their exposure lock are in [lighting-mood.md](lighting-mood.md).

## Swatch boards

The review app renders every palette above as swatch boards with value-only and colour-blind
views. [boards/](boards/) holds the curated palette board.
