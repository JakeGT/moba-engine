# Palette and value

**Status:** v0 approved 2026-11-14. **Leads:** maintainer and graphic designer. Agents drafted
the numbers; the maintainer approved every threshold here on 2026-11-14. Each becomes a check
([checks.md](checks.md)); a threshold changes only by a later maintainer decision (for
example after the first Ember Crossing captures).

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

**Decided (2026-11-14): green/blue/red stay for v0.** It is the genre convention and players
expect it. The designer may propose a change in the UI pass
([ui-hud-typography.md](ui-hud-typography.md)); that would be a new maintainer decision.

### Reserved team-colour zones

Every team colour has **C ≥ 0.117**. Their hues are 28°, 77°, 105°, 142°, 236° and 259°.
Within ±25° of those, saturated colour belongs to the team layer. The free hue bands are
roughly **167–211° (teal–cyan)** and **284–345° (violet–magenta)**. Rose (345–3°) is left
out on purpose: it reads as the enemy red.

So the warm desert and the Forge's brass sit **inside** the zones. That's fine because they
are low-chroma: brass is C 0.08 against the colour-blind enemy's 0.16 at the same hue. The
rule is a **chroma budget**, not a hue ban.

| Rule | Threshold |
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

| Rule | What | Threshold |
|---|---|---|
| **VAL-1** | Terrain-layer albedo (authored) in playable space | OKLCH L **0.32–0.62** (about sRGB 51–134 on a grey) |
| **VAL-2** | Rendered ground luminance in playable space, after grading, on every tier | Mean OKLab L **0.30–0.58**; 5th–95th percentile range ≤ **0.30** |
| **VAL-3** | Shadow floor in playable space (rendered) | L ≥ **0.18**: a unit in shadow still reads |
| **VAL-4** | Highlight ceiling for non-gameplay surfaces in playable space | L ≤ **0.85**. Only VFX, rims and UI may go above. |
| **VAL-5** | Off-board areas (cliffs above 6 m, the sky, the out-of-bounds basin) | Free, but the transition across the lane edge is ≤ **0.15** L per metre |

The debug **readability heatmap** (roadmap §4.3) paints pixels that break VAL-2 to VAL-4 and
PAL-1 to PAL-5 in playable space.

## 3. Contrast: units and telegraphs against the ground

Measured by the headless readability captures (roadmap §4.4) on standard scenes: a hero, a
minion wave and a tower on each terrain layer, under each lighting mood, on every tier.

| Rule | What | Threshold |
|---|---|---|
| **CON-1** | Hero silhouette vs the surrounding ring (3× its bounding box), **without rims** | \|ΔL\| ≥ **0.10** (mean OKLab L) |
| **CON-2** | The same **with team rims** | \|ΔL\| ≥ **0.15**, and the rim pixels reach C ≥ **0.10** within ±15° of the team hue |
| **CON-3** | Enemy telegraph fill vs the ground | ΔE_OK ≥ **0.20**; edge ΔL ≥ **0.25** |
| **CON-4** | Minion vs ground, without rims | \|ΔL\| ≥ **0.08** |
| **CON-5** | A higher tier never lowers CON-1 to CON-4 below the Low result | ≥ Low − **0.02** |

## 4. Colour-blind safety

| Rule | What | Threshold |
|---|---|---|
| **CB-1** | Team pairs (self/ally/enemy) under simulated protanopia, deuteranopia and tritanopia (Machado 2009), in both palettes | ΔE_OK ≥ **0.15** between every pair |
| **CB-2** | No faction or environment accent may become confusable with a team colour under simulation | ΔE_OK ≥ **0.10** from every simulated team colour, for accents with C > 0.08 |
| **CB-3** | Telegraphs keep CON-3 under simulation | as CON-3 |

The review app has protanopia/deuteranopia/tritanopia toggles to eyeball the same thing.

## 5. Faction palettes

### Solar Forge: warm metals, cream stone, white-gold light

| Swatch | sRGB | L | C | H | Use |
|---|---|---|---|---|---|
| Sun-alloy | `#A98C62` | 0.66 | 0.067 | 76 | Armour plates, tower crowns (display swatch: the authored metal base colour is in [material-vocabulary.md](material-vocabulary.md)) |
| Worn sun-alloy | `#8A6E43` | 0.56 | 0.069 | 77 | Older plates, recesses (PBR-FORGE-1) |
| Gilt | `#EBD7A6` | 0.88 | 0.068 | 88 | Accent only, worn through to the alloy; the material vocabulary's gilt target (PBR-FORGE-1) |
| Heat-shield ochre | `#A06C55` | 0.58 | 0.075 | 45 | Unglazed heat-shield tiles |
| Forge sandstone | `#C4AE8C` | 0.76 | 0.053 | 79 | Masonry |
| Scorch | `#4A3F38` | 0.38 | 0.019 | 54 | Heat-scorched stone and ground at the Forge base |
| Heat stain | `#2E2A26` | 0.29 | 0.009 | 67 | Streaks above radiator fins, recesses |
| Sun-glass core (emissive) | `#FFF0C8` | 0.96 | 0.054 | 90 | Light sources (PAL-4) |

### Void: dark polish, pale inlay, violet-white light

| Swatch | sRGB | L | C | H | Use |
|---|---|---|---|---|---|
| Void composite | `#2F2C34` | 0.30 | 0.015 | 303 | Bodies, monoliths (luma 45) |
| Obsidian metamaterial | `#2D2C35` | 0.30 | 0.016 | 291 | Polished faces (reads by **specular**, not albedo); luma 45 |
| Gravity-ring alloy | `#8A8A90` | 0.64 | 0.009 | 286 | Hovering rings, fittings (pewter-like) |
| Tarnished silver | `#B8B8BC` | 0.78 | 0.006 | 286 | Bright metal edges |
| Bone inlay | `#D6CFC2` | 0.86 | 0.019 | 83 | Star-chart lines, masks: keeps Void units off "black blob" |
| Plum lacquer | `#4A3646` | 0.36 | 0.038 | 333 | Cloth, lacquered panels |
| Void-glass core (emissive) | `#D9CCFF` | 0.87 | 0.071 | 297 | Light sources |
| Void-glass edge (emissive) | `#A08CD0` | 0.68 | 0.100 | 297 | Thin rims and cracks only (PAL-5) |

**Decided (2026-11-14): dark bodies plus mandatory pale elements.** Void bodies stay at about
L 0.30–0.36 (albedo luma 45 and up), darker than every Ember Crossing ground layer, which keeps
them distinct from the Forge. (The draft's L 0.25–0.30 swatches were lifted to the decided
luma floor of 45, at the same hue and chroma: derived by the agent.) Against
the risk of a dark blob on dark basalt, **every Void unit carries a bone or silver element of
≥ 10 % of its projected area** (VOID-1 in [checks.md](checks.md)). Void heroes also meet the
material rule in [material-vocabulary.md §3.2](material-vocabulary.md#32-the-void) (luma
floor 45 on body materials, ≥ 15 % of each hero's projected area in a light material). *Considered:* lifting the Void to mid-grey ceramics (safer,
less identity), or leaving it to CON-1 alone.

## 6. Ember Crossing palette (map 1)

| Swatch | sRGB | L | C | H | Use |
|---|---|---|---|---|---|
| Basalt | `#4A4744` | 0.40 | 0.006 | 68 | Cliffs, terraces, rampart bases |
| Ash ground | `#6B6560` | 0.51 | 0.011 | 62 | Base terrain layer (scorched ash) |
| Lane grit | `#906B58` | 0.56 | 0.055 | 48 | Trodden lanes: the centre of LANE-1's luma 112–119, a value step above the ash ground (derived) |
| Desert sand | `#938564` | 0.62 | 0.050 | 88 | Drifts, off-lane, high ground in playable space (VAL-1; derived). The lighter draft sand (`#B39A7C`, L 0.70) only off-board (VAL-5) |
| Ember crystal, today | `#FF6B1A` | 0.70 | **0.198** | 43 | Breaks PAL-4: sits between the enemy red and the colour-blind enemy orange |
| Ember crystal core (emissive) | `#FFE6BE` | 0.94 | 0.059 | 80 | White-gold core, within PAL-4; the same emissive as the material vocabulary's ember crystal |

**Decided (2026-11-14): white-gold ember cores.** The orange crystals were the map's
signature, but at C 0.20 they look like enemy colour, which is the exact mistake §4 exists to
stop. The core is white-gold (PAL-4, ≤ +1 EV); orange survives only as a thin facet rim
(**non-emissive**: derived by the agent, so it can't break PAL-4), ≤ 3 % of the crystal's area and within PAL-3; and the crystals
are kept off the lanes (EMB-1 in [checks.md](checks.md)). The earlier display swatch for the
whole crystal, `#FFD9A8` (C 0.076), sat above PAL-4 and is replaced by the core above.
*Considered:* orange kept on props more than 12 m from any lane, and crystals in the free
teal band (very readable, but it loses "ember").

## 7. Grading

Each map's LUT (`content/grading/*.cube`) is authored **inside** these budgets. VAL-2 and
PAL-1 to PAL-5 are checked after grading, so a LUT can't push the board out of them. Lighting
moods and their exposure lock are in [lighting-mood.md](lighting-mood.md).

## Swatch boards

The review app renders every palette above as swatch boards with value-only and colour-blind
views. [boards/](boards/) holds the curated palette board.
