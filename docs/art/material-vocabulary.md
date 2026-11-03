# Art bible: material vocabulary

**Status:** v0 draft, 2026-10-31. Not approved.
**Owner and approver:** the maintainer. **Drafted by:** an agent, from physicallybased.info
(CC0) and the constraints in the [next-gen graphics roadmap](../plans/next-gen-graphics-roadmap.md)
§2, §4, §6.6, §9.1, §9.4 and §9.6, and [ADR-0011](../adr/0011-art-style-lighting.md).

This part of the art bible lists the materials the game may use and the numeric ranges they
must stay in. Once approved, each range becomes a `content-check` PBR rule (roadmap §9.6). Every
number here is **proposed** until the maintainer signs it off. Items marked
**Decision needed (maintainer)** are real choices with options and a recommendation; they are
collected in [§8](#8-decisions-needed).

## How to read this document

- **Albedo** is given as 8-bit sRGB (0–255), as an artist sees it in a texture. Linear values,
  where useful, are in brackets: `118 [0.181]`.
- **"Luma"** means the relative luminance of the colour (Rec. 709 weights on linear RGB),
  re-encoded to 8-bit sRGB. It is the single number the albedo ranges are checked against.
- **Chroma and hue** are OKLCH: chroma `C` (0 = grey; the team colours sit at 0.12–0.22) and
  hue `h` in degrees.
- **Roughness** is perceptual roughness, 0–1, as stored in the `metallic_roughness` texture's
  G channel (ADR-0011: `alpha = roughness²`, clamped to ≥ 0.045).
- **Metalness** is 0 or 1. Values in between appear only in masked transitions (worn paint
  edges, dust over metal), a few texels wide.
- **Specular (F0)** for dielectrics defaults to **0.04** (IOR 1.5). Exceptions are listed per
  class. Today's material format has no F0 or IOR key: its `specular` key is ADR-0011's
  art-direction scale, not F0. F0 and IOR arrive with the §6.6 material model
  (`KHR_materials_ior` / `KHR_materials_specular`).
- **Measured** values come from physicallybased.info. **Engine target** values are our own
  choices for materials the database does not cover, or deliberate departures from a measured
  value. Both are labelled.
- physicallybased.info stores colours as linear sRGB. Its roughness values are coarse
  placeholders (mostly 0, 0.5, 0.9 or 1.0; 0 for every pure metal, meaning a polished
  reference sample), so **every roughness range below is an engine target**.

### The camera sets what matters

At the default 46 m camera a hero is about 40 px tall at 1440p (27 px/m on the ground) and one
4K pixel covers about 2.5 cm at the focus (roadmap §2). Materials therefore read through three
things: **albedo value, roughness and metal response**. Micro-detail (pores, scratches, weave)
is below a pixel and belongs to the zoom (8 m), spectator and showcase cameras. A material
that only reads through its micro-detail is the wrong material for gameplay space.

## 1. Global rules

| # | Rule | Why |
|---|---|---|
| G1 | **Dielectric albedo luma 30–240.** | Nothing real and diffuse is darker than charcoal (`39`) or brighter than fresh snow (`237`) in normal use; values outside are almost always baked light, crushed shadows or blown highlights. Musou Black (`18`) and Spectralon (`254`) are lab materials, not game materials. |
| G2 | **Metal albedo luma 180–255.** | Metal albedo is F0, the reflectance at normal incidence. Clean iron is `190`, steel `210`, brass `228`. A dark "metal" is really an oxide, soot or paint layer, which is a dielectric (class `oxide` or `paint`). |
| G3 | **No baked lighting in albedo.** No AO, no cavity shadow, no directional light, no specular highlight. | The renderer bakes GI and AO itself (roadmap §6.1); double-darkened crevices read as dirt-black at 46 m and break under RT GI. AI-generated and scanned textures must pass the delight step (§9.4). |
| G4 | **Metalness is binary** except for masked transitions. | In-between metalness is not a physical surface and reads as grey plastic. |
| G5 | **Roughness is never uniform.** | Uniform roughness is the most visible "CG" tell at a top-down camera, where large flat areas catch the sun's highlight. |
| G6 | **Normal maps are unit length** and use the engine's convention (BC5 X/Y, +Y up, MikkTSpace). | Short normals darken shading and fight the specular anti-aliasing. |
| G7 | **Emissive is reserved** for gameplay meaning and small faction accents. | Every bright pixel competes with telegraphs, abilities and the HUD. |
| G8 | **No saturated hues near the team colours** in playable space. | Team identity comes only from rims, outlines and the HUD (roadmap §1, §4). Materials must not imitate them. |
| G9 | **Dust, wear and history on everything**, within the ranges above. | The tone is epic and weathered. Clean, new surfaces are the exception and are called out. |

### 1.1 Proposed numbers

| Quantity | Proposed value | Notes |
|---|---|---|
| Dielectric albedo | luma p1 ≥ 30, p99 ≤ 240 over dielectric texels | Percentiles, so a few texels of filtering overshoot do not fail an asset. |
| Metal albedo | luma p1 ≥ 180 over metal texels (metalness ≥ 0.5) | Checked on luma, not per channel: measured Brass has B = 174 and Gold B = 150, so a per-channel floor would reject real metals. |
| Transition texels | ≤ 3 % of texels with 0.1 < metalness < 0.9 | Worn edges and dust only. |
| Roughness variation | within each material region: standard deviation ≥ 0.03 **and** p95 − p5 ≥ 0.08 | Exempt: `glass` class and wet masks. |
| Roughness floor | ≥ 0.045 everywhere (already clamped by ADR-0011); ≥ 0.08 on heroes and minions | Sub-pixel highlights on moving units shimmer even with TAA. |
| Normal length | source normals 1 ± 0.02 before compression; BC5 X² + Y² ≤ 1.0 for every texel | Mips: renormalise and fold the lost length into roughness at bake time (Toksvig-style). Engine-side change, proposed. |
| Delight | correlation between blurred albedo luma and the baked AO map < 0.3 | Heuristic; catches cavity shadows painted into albedo. |
| Albedo chroma, ground | terrain and ground decals: OKLCH C ≤ 0.06 (p99) | Keeps the ground matte and lets units read as crisper shapes on it (ADR-0011 rule 3). |
| Albedo chroma, everything else | units, structures, props: C ≤ 0.08 (p99) | The least saturated team colour (CB sky, `#56B4E9`) is C 0.117, so this keeps at least a 1.45× chroma gap. Measured Terracotta (C 0.098), Gold (0.101) and Amber (0.148) all exceed it and need desaturated engine targets. |
| Emissive chroma | ≤ 0.06 within ±25° of any team hue; ≤ 0.12 elsewhere (PAL-4, PAL-5 in [palette-and-value.md](palette-and-value.md)) | Team hues: red 28°, CB orange 77°, CB yellow 105°, green 142°, CB sky 236°, blue 259°. The free bands are **teal 167–211°** and **violet 284–345°**. |
| Emissive luminance | faction accents ≤ +1 EV over `L_ref`; gameplay emissives ≤ +2 EV | `L_ref` is defined below. +2 to +4 EV is left to telegraphs and ability VFX (the VFX language part). |
| Emissive area, per asset | ≤ 5 % of surface area (heroes, minions), ≤ 10 % (structures), ≤ 3 % (environment props) | Area where the emissive term exceeds 0.25 × `L_ref`. |
| Emissive area, on screen | ≤ 2 % of playable-space pixels from static and idle emissives | Measured in the standard readability captures (roadmap §4.4) at 46 m, before VFX. |

**`L_ref`, the reference luminance.** Under ADR-0011's units a white Lambert surface facing the
sun shows `albedo × sun`. `L_ref` is the map's lane material in direct sun:
`L_ref = lane albedo (linear) × sun_intensity × sin(sun elevation)`. For Ember Crossing with the
lane target below (linear 0.11) and a low sun at 25° elevation, `L_ref ≈ 0.047 × sun_intensity`,
so a faction accent may emit at most about `0.093 × sun_intensity` (linear). A low sun dims the
ground, so the same emissive reads brighter on a low-sun map; checking against every shipped
map's `[lighting]` table covers that. As an illustration only: with a physical sun of about
100,000 lux at normal incidence (about 42,000 lux on flat ground at 25°) the sunlit lane is
about 1,500 cd/m², so +1 EV is about 3,000 cd/m².

**Decision needed (maintainer): how the albedo ranges are measured.**
Options: (a) per-texel luma with p1/p99 percentiles; (b) every channel of every texel;
(c) the mean only. Recommendation: **(a)**. Option (b) rejects measured brass and gold; (c)
lets dark smears and blown highlights through.

**Decision needed (maintainer): the metal floor.** Measured Titanium is luma `171` and the
database's "Cast Iron" entry is a metal at luma `45` (a dark surface finish, measured by
spectrophotometer). Options: (a) keep 180 and model dark metals as a dielectric oxide layer;
(b) lower the floor to 165; (c) keep 180 with a named exception list. Recommendation: **(a)**.
No material in this vocabulary needs titanium, and treating scale, soot and tarnish as their
own dielectric layer is how they behave physically.

**Decision needed (maintainer): the chroma caps.** Options: (a) C ≤ 0.06 ground / 0.08
elsewhere, as above; (b) a single 0.10 cap; (c) 0.06 everywhere. Recommendation: **(a)**.
(b) admits terracotta-orange and gold close to the enemy red and CB orange; (c) makes brass,
skin and rust read grey.

**Decision needed (maintainer): `team_tint` and the team mask on photoreal content.** Today's
format and `docs/content-authoring.md` tell artists to keep large team-coloured mask areas,
and ADR-0011 rule 1 puts team colour in albedo. The 2026-10-30 decision moves team identity to
rims, outlines and the HUD. Options: (a) `team_tint = 0` for all photoreal content, the mask
kept only for Compat; (b) a small tinted accent (≤ 10 % of the hero) on top of rims;
(c) keep large masks. Recommendation: **(a)**, with ADR-0011 rule 1 amended when the §4 rim
and outline work lands. Tinted albedo would also break G8 for the faction materials.

## 2. Material classes

Each class becomes one `content-check` rule. Ranges are luma in sRGB unless stated. "Refs" are
physicallybased.info entries (v2 API); the values quoted are in [§7](#7-sources).

| Class | Rule ID | Albedo luma | Roughness (engine target) | Metal | F0 / IOR | Refs (physicallybased.info) |
|---|---|---|---|---|---|---|
| Bare metal | `PBR-METAL-1` | 180–255 | 0.15–0.70 | 1 | F0 = albedo | Iron, Brass, Copper, Silver, Gold, Aluminum, Stainless Steel, Lead, Nickel, Tungsten |
| Oxidised or tarnished metal (the oxide layer) | `PBR-OXIDE-1` | 30–140 | 0.55–1.0 | 0 | 0.04 | Rust (`118,68,35`, r 1.0); Cast Iron (dark finish, see §1.1) |
| Painted or lacquered metal | `PBR-PAINT-1` | 30–240 | base 0.35–0.85; clear coat 0.05–0.25 | 0 (1 where worn through) | 0.04 | Car Paint (`89`, clear coat), Blackboard (`56`, r 0.9) |
| Stone, natural | `PBR-STONE-1` | 35–210 | 0.55–0.95 | 0 | 0.04 | Concrete (`189`), Asphalt (Fresh) (`57`, IOR 1.6), Marble (`231`) |
| Stone, polished | `PBR-STONE-2` | 30–235 | 0.05–0.35 | 0 | 0.04 | Marble (r 0, SSS radius listed) |
| Masonry and ceramic | `PBR-CERAM-1` | 40–230 | unglazed 0.70–0.95; glaze via clear coat 0.05–0.25 | 0 | 0.04 | Brick (`140,87,70`, r 1.0), Terracotta (`196,127,93`, r 1.0), Clay, Porcelain (`224`) |
| Soil and sand | `PBR-SOIL-1` | 50–200 | 0.85–1.0 | 0 | 0.04 | Sand (`177,167,132`, r 1.0), Rust (pigment) |
| Ash and char | `PBR-CHAR-1` | 30–170 | 0.85–1.0 | 0 | 0.04 | Charcoal (`39`, r 1.0) |
| Glass and crystal | `PBR-GLASS-1` | tint 30–240 (transmission colour) | 0.045–0.30 | 0 | IOR 1.31–1.77 (F0 0.018–0.077) | Glass (Soda-lime) IOR 1.52, Quartz 1.54, Sapphire 1.768, Amber (transmissive, 1.5), Ice 1.31 |
| Skin | `PBR-SKIN-1` | 60–220 | 0.35–0.60 | 0 | IOR 1.4 (F0 0.028) | Skin I–VI (`85,63,39` to `237,209,196`, r 0.5), with SSS radii |
| Hair | `PBR-HAIR-1` | 30–200 | 0.30–0.60 along the strand | 0 | IOR 1.55 (engine target) | none in the database |
| Cloth | `PBR-CLOTH-1` | 30–230 | 0.70–1.0, plus sheen | 0 | 0.04 | none in the database |
| Leather | `PBR-LEATHER-1` | 30–180 | 0.40–0.85 | 0 | 0.04 | none in the database |
| Wood | `PBR-WOOD-1` | 35–200 | 0.50–0.95 | 0 | 0.04 | none (Cardboard `160,126,93` is the nearest fibre reference) |
| Bone, horn, shell | `PBR-BONE-1` | 150–235 | 0.40–0.95 | 0 | 0.04 (Pearl: IOR 1.68) | Bone (`230,230,213`, r 0.9), Egg Shell (Brown), Pearl |

Notes on the classes:
- **Oxide layers are dielectric.** Rust, forge scale, silver tarnish, patina and soot sit on
  top of metal and are authored with metalness 0, in their own class. That is how a "dark
  metal" stays within G2.
- **Stone and basalt.** The database has no basalt, sandstone or obsidian entry (checked
  against all 122 entries of the v2 API, updated 2026-10-26). Their targets in §3 are engine
  targets, anchored on Charcoal and Asphalt (dark end) and Concrete (mid).
- **Snow** (`237`, r 0.5, IOR 1.31) is the database's brightest natural diffuse material and
  sits just inside G1. It is not used on Ember Crossing but is the upper anchor of G1.
- **Gold's** linear red is 1.059 in the database (outside sRGB); it clamps to `255`.
- **v1 and v2 differ** on some roughness placeholders (Sand, Charcoal, Brick: 0.9 in v1, 1.0
  in v2). This document uses v2.
- **Every material declares its class.** `PBR-CLASS-1` (see §6) needs to know which class each
  texel belongs to, because one hero texture set holds skin, cloth and metal.

**Decision needed (maintainer): how a material declares its classes.** Options: (a) a
source-side class-ID map (an 8-bit index texture kept in the asset's source manifest, never
shipped) that `content-check` reads with the textures; (b) a `class` key per material TOML,
with mixed-class assets checked only against the dielectric/metal rules; (c) split every
asset into single-class materials. Recommendation: **(a)**. It costs one extra bake output and
checks every texel against the right class; (c) multiplies draw calls.

## 3. Named material vocabulary

These are the allowed "look" materials. A new look material needs a row here first.

**Technology names, measured stand-ins (2026-10-31).** The world's materials are advanced
technology ([world](world-and-factions.md#technology-that-looks-like-magic)): sun-alloy, heat-shield
ceramic, void composite and so on. They have no measured values, so each row keeps a **real
material as its physical stand-in** (brass for sun-alloy, terracotta and glazed ceramic for
heat-shield ceramic, pewter for gravity-ring alloy). The PBR numbers come from the stand-in;
the name, the forms and the wear come from the fiction. The rows below were renamed, not
re-measured. Hex values
are targets for the clean base; the ranges allow for weathering (§4). "Tier" refers to the
roadmap's feature matrix: the multi-layer model (§6.6) is the same on every tier; what changes
is skin SSS, and reflection quality on smooth surfaces (RT reflections below roughness 0.45
from High up; SSR and probes on Medium; baked probes on Low). **Compat** (today's frozen
renderer) has no clear coat, sheen or transmission, so every material must also read with its
base layer alone.

### 3.1 The Solar Forge

A sun-worshipping empire of solar and fusion technology. Warm alloys, heat-shield ceramics and
pale stone; light and heat as material, never as saturated colour. Hue family 60–95°, chroma kept low so the enemy red and
the CB orange and yellow stay unique.

| Material | Look | Class | Albedo target | Roughness | Metal | Layers | Tier note |
|---|---|---|---|---|---|---|---|
| Sun-alloy, polished (stand-in: brass) | Ceremonial plates, collector discs; bright where hands and sand rub | `METAL` | measured Brass `#F5E4AE` (luma 228, C 0.072) | 0.15–0.35 | 1 | none | RT reflections on High+; probes below |
| Sun-alloy, worn | The default Forge metal: brushed, dulled, dust in recesses | `METAL` + `OXIDE` | Brass on raised areas; brown oxide luma 60–90 in recesses | 0.35–0.60 metal; 0.7–0.9 oxide | 1 / 0 | none | all tiers alike |
| Cast sun-alloy, patinated (stand-in: bronze) | Statues, drum cores, armour; dark statuary patina, bright on wear | `METAL` + `OXIDE` | engine target `#DDB98F`–`#E2C29C` (luma 191–199, between Copper and Brass); patina `#4A3E36` | 0.30–0.55 metal; 0.65–0.9 patina | 1 / 0 | none | all tiers alike |
| Forge dark alloy, heat-tinted (stand-in: iron) | Blades, bands, lattice grilles; straw-to-bronze-to-purple temper colours near hot zones | `METAL` | measured Iron `#C0BEBB` (luma 190) | 0.30–0.60 | 1 | iridescence (thin film) for temper colours | thin film on all tiers; Compat: tint baked into albedo within G2 |
| Forge dark alloy, scaled | Blackened alloy with heat scale; the Forge's dark metal | `OXIDE` | `#3A3836` (luma 56), C ≤ 0.01 | 0.60–0.85 | 0 (1 where scale is chipped) | none | all tiers alike |
| Heat-shield ceramic, unglazed (stand-in: terracotta) | Buff-ochre heat-shield tiles on stepped walls and armour | `CERAM` | engine target `#B49272` (luma 152, C 0.061); desaturated from measured Terracotta | 0.75–0.95 | 0 | none | all tiers alike |
| Heat-shield ceramic, glazed | Cream and honey glaze on terraces, domes and breastplates, crazed and chipped | `CERAM` | base `#D8C9A6` (luma 202, C 0.050) | base 0.6–0.8; clear coat 0.08–0.20 | 0 | clear coat | Compat: base only, roughness 0.3 |
| Sun-glass (fusion lens) | Thick honey-tinted lenses on tower crowns and hammers; bright white-gold core when charged | `GLASS` | tint `#D9B98A` (C 0.072); emissive core `#FFE6BE` (C 0.059) | 0.05–0.20 | 0 | transmission; emissive core | transmission from Medium; Low and Compat: opaque base + emissive |
| Fused scorch (stand-in: slag) | Glassy black-brown crust where fusion energy has struck stone | `CHAR` / `GLASS` | `#3A3634` (luma 55), C ≤ 0.02 | crust 0.7–0.9; fracture faces 0.15–0.35 | 0 | optional molten seam (below) | all tiers alike |
| Molten seam | Thin cracks of white-hot metal in fused scorch, cores and impact points | emissive only | emissive `#FFE6BE`, never orange | n/a | n/a | emissive | counts towards the emissive budget |
| Forge sandstone | Dressed ashlar of temples and walls; pale buff, sand-scoured | `STONE-1` | engine target `#B8A07C` (luma 163, C 0.057); Sand entry as parent mineral | 0.75–0.95 | 0 | none | all tiers alike |
| Forge basalt masonry | Dark dressed basalt plinths and thresholds; pairs with sandstone | `STONE-1` | engine target `#4A4744` (luma 71) | 0.65–0.85 (polished treads 0.35–0.5) | 0 | none | all tiers alike |
| Gilt leaf | Thin gold on brass, stone or wood, rubbed through to the base | `METAL` | engine target `#EBD7A6` (luma 216, C 0.068); desaturated from measured Gold | 0.20–0.45 | 1 | none | all tiers alike |
| Linen | Sun-bleached undyed wraps and banners | `CLOTH` | engine target `#CBBFA6` (luma 192, C 0.037) | 0.80–0.95 | 0 | sheen | Compat: base only |
| Forge leather | Oiled tan straps, aprons and grips | `LEATHER` | engine target `#7A5A40` (luma 96, C 0.057) | 0.45–0.80 | 0 | none | all tiers alike |

**Technology materials with no row above:**

| Material | Look | Class | Albedo target | Roughness | Metal | Layers | Tier note |
|---|---|---|---|---|---|---|---|
| Emitter lattice | A fine machined grille of dark alloy with a contained white-gold glow behind it: the visible source of Forge light | `METAL` + emissive | Forge dark alloy, scaled; emissive `#FFE6BE` behind the grille | 0.35–0.60 | 1 | emissive (behind) | counts towards the emissive budget; lit only in gameplay states |

**Decision needed (maintainer): how saturated the Forge's sun-alloy may be.** Brass (its stand-in, h 93°) sits
between the CB orange (77°) and CB yellow (105°). Options: (a) measured brass chroma
(≈ 0.07) as the ceiling, with a rich look coming from roughness and reflection; (b) a warmer,
more golden brass up to C 0.10; (c) a pale, almost silver "white brass" at C ≤ 0.04.
Recommendation: **(a)**. It is the real metal, it stays below the 0.08 cap, and on High+ the
RT reflections of the warm sky give it richness without extra chroma.

**Decision needed (maintainer): gilt leaf.** Measured Gold is C 0.101, above the cap.
Options: (a) a desaturated, worn gilt at C ≤ 0.07 on small areas; (b) real gold, allowed
only outside playable space (showcase, menus); (c) no gilt. Recommendation: **(a)** in play,
**(b)** for showcase variants.

**Decision needed (maintainer): sun-glass tint.** Measured Amber is a transmissive
`#EB9235` at C 0.148, close to the CB orange. Options: (a) pale honey tint C ≤ 0.07 with a
low-chroma white-gold emissive core; (b) real amber colour, small areas only; (c) clear glass
with only the core coloured. Recommendation: **(a)**.

**Decision needed (maintainer): the Forge's emissive colour (molten seams, sun-glass cores).**
The fiction wants orange heat, but saturated orange-red is the enemy colour and the CB
orange. Options: (a) **white-gold** (blackbody-hot, C ≤ 0.06), with heat conveyed by the
surrounding slag and scale rather than the glow's hue; (b) saturated orange, but capped at
+1 EV and 1 % screen area; (c) a shift to pale gold-yellow. Recommendation: **(a)**. It
passes G8 in both colour-blind and normal team palettes, and white-hot reads as hotter than
orange anyway.

**Decision needed (maintainer): heat tint and the blue band.** Real temper colours run straw,
bronze, purple, then blue, which falls near the ally blue (259°) and CB sky (236°). Options:
(a) straw to purple only in playable space; (b) all temper colours, blue kept at C ≤ 0.04;
(c) no temper colours. Recommendation: **(a)**; blue tempering only on showcase assets.

### 3.2 The Void

A gravity-bending order. Cold dark composites and glassy stone, tarnished pale alloys,
suspended masses held by gravity rings. Value is low and surfaces are smoother than the Forge's; colour lives in
the free bands (violet 284–345°, teal 167–211°) and only in small accents.

| Material | Look | Class | Albedo target | Roughness | Metal | Layers | Tier note |
|---|---|---|---|---|---|---|---|
| Void composite, matte (stand-in: black ceramic) | The Void's main body material: black with a violet cast, fine and dense | `CERAM` | engine target `#302C36` (luma 46, C 0.019, h 304) | 0.55–0.80 | 0 | none | all tiers alike |
| Void composite, glazed | Gloss black panels and masks; mirror-dark highlights | `CERAM` | base `#35303D` (luma 50) | base 0.5–0.7; clear coat 0.05–0.15 | 0 | clear coat | RT reflections High+; Compat: base 0.25 |
| Obsidian metamaterial, polished (stand-in: obsidian) | Blades, inlaid panels, monoliths; reflects the sky darkly | `STONE-2` | engine target `#2B292F` (luma 42); IOR as Glass (1.52, F0 0.043) | 0.05–0.20 | 0 | none (opaque) | relies on reflections: RT High+, probes below |
| Obsidian, knapped | Raw conchoidal fractures; sharp, glassy facets with rough cortex | `STONE-1` / `STONE-2` | `#2B292F` facets; cortex luma 60–80 | facets 0.08–0.25; cortex 0.7–0.9 | 0 | none | all tiers alike |
| Polished black stone | Dressed architectural stone, less glassy than obsidian | `STONE-2` | engine target `#34323A` (luma 51) | 0.15–0.40 | 0 | none | all tiers alike |
| Levitating basalt | Raw basalt masses held in the air; cut undersides with void-glass seams | `STONE-1` | as Ember basalt `#4A4744`; cut faces 0.4–0.55 roughness | 0.55–0.85 | 0 | void-glass seam (below) | all tiers alike |
| Gravity-ring alloy (stand-in: pewter) | Hovering rings, fittings, filigree; dull and soft-edged | `METAL` | engine target `#C4C6C8` (luma 198), near measured Lead | 0.35–0.60 | 1 | none | all tiers alike |
| Tarnished silver | Bright silver on raised edges, brown-grey tarnish in recesses | `METAL` + `OXIDE` | measured Silver `#FEFDFC` (luma 253) on wear; tarnish `#5A5048` (luma 82) | 0.15–0.35 metal; 0.6–0.85 tarnish | 1 / 0 | none | RT reflections High+ |
| Void-glass | Cold, nearly clear glass with a teal or violet emissive edge | `GLASS` | tint near neutral (C ≤ 0.04); emissive edge teal `#7CCFC4` (h 185, C 0.083) or violet `#A58AD6` (h 300, C 0.113) | 0.045–0.15 | 0 | transmission; emissive edge | transmission from Medium; Low and Compat: dark opaque base + emissive edge |
| Void lacquer | Deep violet-black lacquer on wood and metal armour | `PAINT` | `#2C2430` (luma 39, C 0.024) | base 0.4–0.6; clear coat 0.05–0.15 | 0 | clear coat | Compat: base only, roughness 0.25 |
| Void silk | Robes and banners in plum and violet-black | `CLOTH` | `#4A3346` (luma 58, C 0.045, h 333) | 0.55–0.80 | 0 | sheen, anisotropy | Compat: base only |
| Void felt | Matte padding and hoods, charcoal-violet | `CLOTH` | `#3A3440` (luma 54, C 0.022) | 0.85–1.0 | 0 | sheen (low) | all tiers alike |
| Bone-white inlay | Polished bone or nacre lines set into black ceramic and stone | `BONE` | `#DCD6C6` (luma 214), near measured Bone | 0.40–0.65 | 0 | none (Pearl-like iridescence: showcase only) | all tiers alike |

**Decision needed (maintainer): the Void's emissive hue.** Options: (a) violet 284–345° as
the main accent with teal 167–211° as a secondary; (b) teal main, violet secondary; (c) one
hue only. Recommendation: **(a)**. Violet is the furthest band from every team hue in both
palettes and from the Forge's warm family; teal sits 43° from the self green and 51° from the
CB sky, so it should stay the minor accent.

**Decision needed (maintainer): how dark the Void may go.** Its body materials sit at luma
39–58, near the G1 floor. In the shade of an Ember Crossing cliff, a Void hero could merge
with basalt (`71`). Options: (a) keep them dark and rely on rims and outlines; (b) raise the
Void floor to luma 45 and give every Void hero at least 15 % of its area in a light material
(pewter, silver, bone inlay); (c) both. Recommendation: **(c)**. Shape and material should
separate the factions without leaning entirely on team rims.

### 3.3 Environment: Ember Crossing

A desert basin of terraced basalt cliffs, scorched ash and brown ground, a rust-red trodden
lane and ember crystals, under a low warm sun. The ground stays matte (C ≤ 0.06, roughness
≥ 0.8) so units read against it.

| Material | Look | Class | Albedo target | Roughness | Metal | Layers | Tier note |
|---|---|---|---|---|---|---|---|
| Basalt, dry | Terraced columnar cliffs; dark warm grey, dusty ledges | `STONE-1` | engine target `#4A4744` (luma 71); lit faces up to `#5C5853` (89) | 0.70–0.90 | 0 | dust layer on up-facing ledges | all tiers alike |
| Basalt, wet | Seeps and the rare rain variant; darker and glossy | `STONE-1` + wet mask | dry albedo × 0.7 | 0.10–0.30 where wet | 0 | puddle and streak masks (feature matrix: weather) | ripples and streaks from Medium |
| Scorched ash ground | The basin floor: grey-brown ash, burnt patches, footprints | `CHAR` | engine target `#6B6560` (luma 102); burnt patches `#4E4945` (74) | 0.90–1.0 | 0 | none | all tiers alike |
| Packed desert soil | Brown ground between ash fields; cracked crust | `SOIL` | engine target `#8C7A66` (luma 125, C 0.037) | 0.85–1.0 | 0 | none | all tiers alike |
| Rust-red lane grit | The trodden lane: iron-oxide grit, compacted, rutted | `SOIL` | engine target `#7A5444`–`#84604C` (luma 93–104, C 0.055–0.057, h 44–50); hue from measured Rust | 0.85–0.98 | 0 | none | all tiers alike |
| Desert sand | Wind-blown drifts against cliffs and ruins | `SOIL` | engine target `#BDAE8C` (luma 175, C 0.050); measured Sand `#B1A784` is the anchor | 0.90–1.0 | 0 | none | all tiers alike |
| Ember crystal | Clusters of dark smoky crystal with a white-gold glow deep inside | `GLASS` | body `#4A3430` (luma 57, C 0.033); emissive core `#FFE6BE` | 0.08–0.25 | 0 | transmission; emissive core | transmission from Medium; Low and Compat: opaque body + core |
| Builder alloy | The fallen sky-engine's ribs and the Builder machines: seamless pale grey, fine precise seams, dead emitter rings | `METAL` | engine target `#AEB0B2` (luma 175, C ≤ 0.01); stand-in: titanium | 0.30–0.55; dust 0.85+ | 1 | dust layer on up-facing surfaces | all tiers alike |
| Hard light | The Ashen bowstring and arrows: a thin line of pale light, no body | emissive only | emissive `#F2F0EA` (C ≤ 0.02) | n/a | n/a | emissive | counts towards the emissive budget |
| Sun-bleached bone | Huge old skeletons half-buried in sand | `BONE` | engine target `#D8D2C2` (luma 210), below measured Bone (230) for age and dust | 0.70–0.95 | 0 | none | all tiers alike |
| Dead wood | Silvered, cracked trunks and old timber | `WOOD` | engine target `#9A9086` (luma 146, C 0.019) | 0.75–0.95 | 0 | none | all tiers alike |
| Charred wood | Burnt beams and stumps; alligator-cracked | `CHAR` | engine target `#2F2B29` (luma 44) | 0.85–1.0 | 0 | none (no glow: G7) | all tiers alike |
| Dry scrub | Sparse olive-grey desert plants | `WOOD` / foliage | engine target `#7C7A62` (luma 121, C 0.035); measured Grass (C 0.068, h 120) is too green | 0.60–0.85 | 0 | thin-sheet translucency (foliage) | all tiers alike |

**Decision needed (maintainer): how red the lane may be.** The enemy red is h 28°, C 0.216.
Measured Rust is h 52°, C 0.083. Options: (a) a brown-red at C ≤ 0.06, h 44–55° (the
targets above): clearly "rust" against grey ash, a 3.5× chroma gap to the enemy colour;
(b) the measured Rust chroma, C ≈ 0.08; (c) a near-neutral brown lane, with the lane
told apart by value and texture only. Recommendation: **(a)**. The value contrast between the
lane (luma ~100) and the ash (~100) is small, so add a value step: either lift the lane to
luma ~115 or darken the burnt ash patches that border it.

**Decision needed (maintainer): ember crystals.** They are the map's signature feature and the
one place the fiction asks for a large warm glow. Options: (a) dark smoky body, white-gold core
at ≤ +1 EV, a few clusters per screen (the targets above); (b) saturated orange-red glow,
which collides with the enemy red and CB orange; (c) give them a gameplay meaning and let
the glow appear only in that state (≤ +2 EV). Recommendation: **(a)**, with **(c)** if
design gives them a mechanic. Saturated orange is ruled out by G8.

### 3.4 Concept-image cross-check

The review app's *Materials* section has two AI swatches per named material (round 1). They
are **look references only**: PBR values come from the tables above and from CC0 scans, never
from generated images. Three of them confirm risks this document already names:
- **Lane grit** came out saturated orange-red. It is the same lane problem the
  [lighting measurements](lighting-mood.md#first-measurements-compat-renderer-2026-10-31)
  found.
- **Void ceramic** came out as plain matte black. The fine engraved lines must be in the
  normal map, because the albedo alone reads as a flat void.
- **Void glass** came out as clear cracked glass with no inner light. The violet-white
  emissive must be explicit in the material, within the PAL-5 limits.

**Round G4** (advanced tech, Nano Banana 2.1, two swatches per material in the review app):

- **The technology materials read as intended:** the emitter lattice (a fine grille over a
  white-gold glow), the fusion lens, heat-shield tiles and the gravity ring all show where
  the "magic" comes from. They are the reference for normal-map detail, not for albedo.
- **Void composite** finally carries its engraved star-map lines, so it no longer reads as a
  flat black void.
- **Sun-alloy** came out dull gold-bronze: within the PAL-3 cap, as intended.
- The first environment swatches (ash, basalt, lane grit, ember crystal) were cluttered with
  Ashen cloth and bones by a prompt error; round G5 redrew them as natural ground only.

## 4. Weathering and wear vocabulary

Weathering is what makes the world "epic, weathered". At 46 m it reads only through broad
changes of value and roughness. Scale guide: at 1440p one ground pixel is about 3.7 cm, and on
a 40 px hero one pixel is about 4.5 cm.

| Layer | Physical change | Albedo | Roughness | Metalness | Where it goes |
|---|---|---|---|---|---|
| Edge wear | Paint, oxide or glaze rubbed off convex edges | base material shows through: lighter metal, lighter stone | metal edges smoother (−0.1 to −0.2); stone edges rounder (normal) | 1 where paint or oxide is worn off metal | Convex edges, handles, treads, shield rims |
| Dust | Fine sand and ash settle on surfaces | towards the map's ground: ash `#6B6560`, sand `#BDAE8C` | up to 0.85–0.95 | 0 where coverage > 50 % (dust is a dielectric) | Up-facing surfaces (world-space normal·up), ledges, crevices on top |
| Soot | Carbon from fires and forges | towards luma 40–60, C ≤ 0.01 | 0.85–1.0 | 0 | Above heat sources, vertical streaks on walls, chimney mouths |
| Heat tint | Thin oxide film on hot steel | temper colours straw–bronze–purple (see the heat-tint decision) | unchanged | stays 1 (thin film) | Near forge zones, blade spines, grilles |
| Patina | Corrosion of copper alloys and silver | statuary brown-black `#4A3E36`; silver tarnish `#5A5048` | 0.65–0.90 | 0 (it is an oxide) | Recesses and sheltered sides; worn off on high points |
| Sand abrasion | Wind-driven grit scours one side | windward faces lighter (+5–10 % luma) and flatter | stone +0.05; metal rises to 0.5–0.7 | unchanged | Windward faces, low parts of walls and statues, lower legs |

**Rules of thumb at 46 m:**
1. **Three pixels or nothing.** A wear feature must span at least 3 px at 1440p to read:
   about 11 cm on structures. Anything smaller goes only into roughness and normal, for the
   zoom and showcase cameras.
2. **Weathering moves value by at most ±25 %** of the clean material's luma, and never out of
   its class range. Dust may not turn brass into a dielectric across a whole surface: worn
   high points must still show metal.
3. **Gravity decides.** Dust on top, streaks below sources, wear on edges a hand or foot would
   touch. Random noise wear reads as texture noise, not history.
4. **Units carry less weathering than the world.** Dust on heroes and minions ≤ 20 % of
   their surface, kept low (feet, lower legs, hems); otherwise they take on the ground's
   colour and value and merge with it.
5. **One story per asset.** A Forge structure is soot, heat and sand; a Void structure is
   tarnish, fine dust and polish where it is touched. Mixing every layer everywhere makes
   everything read the same.
6. **Weathering never adds saturation.** Patina and rust stay inside the chroma caps; no
   verdigris green in playable space (see below).

**Decision needed (maintainer): bronze patina colour.** Real outdoor bronze turns verdigris
green-blue (h 160–190°), close to the self green and inside the Void's teal accent band.
Options: (a) statuary brown-black patina in playable space, verdigris only out of play or in
showcase; (b) verdigris at C ≤ 0.03 (grey-green, `#7E968E`); (c) full verdigris.
Recommendation: **(a)**. It keeps teal as a Void signal and keeps green away from the self
colour.

**Decision needed (maintainer): dust on units.** Options: (a) ≤ 20 % coverage, lower body
only (rule 4); (b) no dust on units; (c) no limit. Recommendation: **(a)**.

## 5. How this maps to today's material format

Today's `materials/<model>.toml` ([`content/materials/sentinel.toml`](../../content/materials/sentinel.toml),
`docs/content-authoring.md`) has `base_color`, `normal`, `metallic_roughness` and `mask`
textures, factors, `team_tint` and a constant `emissive` (linear RGB, 0..=16). For this
vocabulary:
- **Clear coat, sheen, anisotropy, transmission, iridescence and IOR** are not in the format
  yet. They arrive with the §6.6 material model, following `KHR_materials_*`.
- **Emissive** is one constant per material today. Molten seams, sun-glass cores and
  void-glass edges need an emissive texture (or mask) slot; until then, those assets use
  separate emissive submeshes.
- **The class map** (if the class decision picks option (a)) is a source-pipeline file, not a
  runtime slot.
- **`team_tint`**: see the decision in §1.1.

## 6. Checks this becomes

Each check runs in `content-check` on the asset's source textures (after delight, before KTX2
encoding) unless it says "capture". All are **proposed**.

| Rule ID | What `content-check` verifies | Threshold | Status |
|---|---|---|---|
| `PBR-ALB-1` | Dielectric texel luma | p1 ≥ 30, p99 ≤ 240 | proposed |
| `PBR-ALB-2` | Metal texel luma (metalness ≥ 0.5) | p1 ≥ 180 | proposed |
| `PBR-MET-1` | Transition texels (0.1 < metalness < 0.9) | ≤ 3 % of texels | proposed |
| `PBR-RGH-1` | Roughness variation per class region (not `GLASS`, not wet masks) | σ ≥ 0.03 and p95 − p5 ≥ 0.08 | proposed |
| `PBR-RGH-2` | Roughness floor | ≥ 0.045 all; ≥ 0.08 on heroes and minions | proposed |
| `PBR-NRM-1` | Source normal length | 1 ± 0.02 per texel | proposed |
| `PBR-NRM-2` | BC5 normal X² + Y² | ≤ 1.0 per texel | proposed |
| `PBR-DELIGHT-1` | Correlation of blurred albedo luma with baked AO | < 0.3 | proposed |
| `PBR-CLASS-1` | Every texel has a declared class | 100 % | proposed |
| `PBR-METAL-1` … `PBR-BONE-1` | Per-class albedo luma, roughness and metalness ranges (table in §2) | the §2 ranges, p1/p99 | proposed |
| `PBR-GLASS-1` | Glass and crystal IOR | 1.31–1.77 | proposed |
| `PBR-SKIN-1` | Skin IOR and SSS radius present | IOR 1.4; radius from Skin I–VI | proposed |
| `PAL-CHROMA-1` (= PAL-1) | Albedo chroma, terrain and ground decals | OKLCH C ≤ 0.06 (p99) | proposed |
| `PAL-CHROMA-2` (= PAL-2) | Albedo chroma, units, structures, props | C ≤ 0.08 (p99) | proposed |
| `PAL-HUE-1` (= PAL-4) | Emissive chroma near team hues (28°, 77°, 105°, 142°, 236°, 259° ± 25°) | C ≤ 0.06 | proposed |
| `PAL-HUE-2` (= PAL-5) | Emissive chroma elsewhere | C ≤ 0.12 | proposed |
| `PBR-EMIT-1` | Emissive luminance against each shipped map's `L_ref` | accents ≤ +1 EV; gameplay ≤ +2 EV | proposed |
| `PBR-EMIT-2` | Emissive area per asset (emissive > 0.25 × `L_ref`) | ≤ 5 % units; ≤ 10 % structures; ≤ 3 % props | proposed |
| `PBR-EMIT-3` | Static emissive screen share (capture, 46 m, before VFX) | ≤ 2 % of playable-space pixels | proposed |
| `PBR-WEAR-1` | Dust coverage on units (capture or dust mask) | ≤ 20 % of surface | proposed |
| `PBR-VOCAB-1` | Every material names a vocabulary entry from §3 | listed entry | proposed |

## 7. Sources

All values are from **physicallybased.info** (Anton Palmqvist and contributors, CC0 1.0),
fetched on 2026-10-31 from the v2 API (schema 2.2, data updated 2026-10-26, 122 entries).
Colours are the database's `srgb-linear` values, converted to 8-bit sRGB here.

| Entry | Linear colour | sRGB | Roughness | Metal | IOR / other |
|---|---|---|---|---|---|
| Aluminum | 0.916, 0.923, 0.924 | 245, 246, 246 | 0 | 1 | |
| Brass | 0.910, 0.778, 0.423 | 245, 228, 174 | 0 | 1 | |
| Copper | 0.932, 0.623, 0.522 | 247, 207, 191 | 0 | 1 | |
| Gold | 1.059, 0.773, 0.307 | 255, 228, 150 | 0 | 1 | |
| Iron | 0.530, 0.513, 0.494 | 192, 190, 187 | 0 | 1 | |
| Lead | 0.626, 0.640, 0.693 | 207, 209, 217 | 0 | 1 | |
| Nickel | 0.697, 0.641, 0.563 | 217, 209, 198 | 0 | 1 | |
| Silver | 0.991, 0.985, 0.974 | 254, 253, 252 | 0 | 1 | |
| Stainless Steel | 0.669, 0.639, 0.598 | 214, 209, 203 | 0 | 1 | |
| Titanium | 0.441, 0.400, 0.361 | 177, 170, 162 | 0 | 1 | luma 171, below G2 |
| Tungsten | 0.537, 0.536, 0.519 | 194, 193, 191 | 0 | 1 | |
| Cast Iron | 0.027, 0.026, 0.026 | 46, 45, 45 | 0.3 | 1 | spectrophotometer measurement |
| Rust | 0.181, 0.057, 0.017 | 118, 68, 35 | 1.0 | 0 | 1.5; spectrophotometer measurement |
| Car Paint | 0.1, 0.1, 0.1 | 89, 89, 89 | 0 | 0 | 1.5; clear-coated |
| Blackboard | 0.039 | 56 | 0.9 | 0 | 1.5 |
| Concrete | 0.51 | 189 | 0.5 | 0 | 1.5 |
| Asphalt (Fresh) | 0.0429, 0.0414, 0.0401 | 58, 57, 56 | 0.5 | 0 | 1.6 |
| Marble | 0.830, 0.791, 0.753 | 235, 230, 225 | 0 | 0 | 1.5; SSS radius 0.851, 0.557, 0.395 |
| Brick | 0.262, 0.095, 0.061 | 140, 87, 70 | 1.0 | 0 | 1.5 |
| Terracotta | 0.555, 0.212, 0.110 | 196, 127, 93 | 1.0 | 0 | 1.5 |
| Clay | 0.256, 0.118, 0.091 | 138, 96, 85 | 0.5 | 0 | 1.5 |
| Porcelain | 0.745, 0.745, 0.723 | 224, 224, 221 | 0 | 0 | 1.5 |
| Sand | 0.440, 0.386, 0.231 | 177, 167, 132 | 1.0 | 0 | 1.5 |
| Charcoal | 0.02 | 39 | 1.0 | 0 | 1.5 |
| Musou Black | 0.006 | 18 | 1.0 | 0 | below G1 (lab material) |
| Snow | 0.85 | 237 | 0.5 | 0 | 1.3098; SSS radius 0.2, 0.2, 1 |
| Glass (Soda-lime) | 0.984, 0.995, 0.995 | 253, 254, 254 | 0 | 0 | 1.52; transmission 1 |
| Quartz | 1, 1, 1 | 255 | 0 | 0 | 1.54; transmission 1 |
| Sapphire | 0.086, 0.323, 0.855 | 83, 154, 238 | 0 | 0 | 1.7682; transmission 1 |
| Amber | 0.830, 0.288, 0.036 | 235, 146, 53 | 0 | 0 | 1.5; transmission 1 |
| Ice | 0.973, 0.995, 1.0 | 252, 254, 255 | 0 | 0 | 1.3098; transmission 1 |
| Skin I | 0.847, 0.638, 0.552 | 237, 209, 196 | 0.5 | 0 | 1.4; SSS radius 0.482, 0.169, 0.109 |
| Skin III | 0.623, 0.433, 0.343 | 207, 176, 158 | 0.5 | 0 | 1.4; SSS radius 0.482, 0.169, 0.109 |
| Skin IV | 0.436, 0.227, 0.131 | 176, 131, 101 | 0.5 | 0 | 1.4; SSS radius 0.367, 0.137, 0.068 |
| Skin VI | 0.09, 0.05, 0.02 | 85, 63, 39 | 0.5 | 0 | 1.4; SSS radius 0.367, 0.137, 0.068 |
| Bone | 0.793, 0.793, 0.664 | 230, 230, 213 | 0.9 | 0 | 1.5 |
| Egg Shell (Brown) | 0.493, 0.248, 0.123 | 186, 136, 98 | 0.9 | 0 | 1.5 |
| Pearl | 0.80, 0.75, 0.70 | 231, 225, 218 | 0.35 | 0 | 1.68; SSS radius 0.3, 0.5, 0.3 |
| Cardboard | 0.351, 0.208, 0.110 | 160, 126, 93 | 0.5 | 0 | 1.5 |
| Grass | 0.105, 0.133, 0.041 | 91, 102, 57 | 0.5 | 0 | 1.5 |
| Spectralon | 0.99 | 254 | 0.99 | 0 | 1.35; above G1 (lab material) |

Skin II and V are also in the database (Skin II `231,185,159`; Skin V `145,107,79`). Values
are the same in v1 and v2 except some roughness placeholders (Brick, Charcoal, Sand and Musou
Black 0.9 → 1.0; Ice 0.5 → 0) and Ice's colour; this document uses v2. The database has **no** entry for bronze, pewter, tin, basalt,
sandstone, obsidian, ash, slag, leather, wood, linen, silk or felt; every value given for those
here is an engine target.

**URLs:**
- Site: <https://physicallybased.info/>
- API (v2, used here): <https://api.physicallybased.info/v2/materials>
- API (v1): <https://api.physicallybased.info/materials>
- Data repository (CC0 1.0): <https://github.com/AntonPalmqvist/physically-based-api>
- Spectrophotometer measurement folders cited above:
  [rust](https://github.com/AntonPalmqvist/physically-based-api/tree/main/data/materials/rust/),
  [cast-iron](https://github.com/AntonPalmqvist/physically-based-api/tree/main/data/materials/cast-iron/),
  [terracotta](https://github.com/AntonPalmqvist/physically-based-api/tree/main/data/materials/terracotta/),
  [clay](https://github.com/AntonPalmqvist/physically-based-api/tree/main/data/materials/clay/),
  [brick](https://github.com/AntonPalmqvist/physically-based-api/tree/main/data/materials/brick/),
  [porcelain](https://github.com/AntonPalmqvist/physically-based-api/tree/main/data/materials/porcelain/),
  [charcoal](https://github.com/AntonPalmqvist/physically-based-api/tree/main/data/materials/charcoal/),
  [blackboard](https://github.com/AntonPalmqvist/physically-based-api/tree/main/data/materials/blackboard/).
- The database's own upstream sources (refractiveindex.info for metals and glasses, published
  skin optics papers, Lagarde's PBR charts) are listed per entry in the API response.

## 8. Decisions needed

| # | Decision | Section | Recommendation |
|---|---|---|---|
| 1 | How albedo ranges are measured | §1.1 | Per-texel luma, p1/p99 |
| 2 | Metal floor vs Titanium and dark Cast Iron | §1.1 | Keep 180; dark metals are oxide layers |
| 3 | Chroma caps | §1.1 | 0.06 ground, 0.08 elsewhere |
| 4 | `team_tint` and the team mask on photoreal content | §1.1 | `team_tint = 0`; mask for Compat only |
| 5 | How a material declares its classes | §2 | Source-side class-ID map |
| 6 | Forge brass saturation | §3.1 | Measured brass chroma (≈ 0.07) as ceiling |
| 7 | Gilt leaf | §3.1 | Worn gilt C ≤ 0.07 in play; real gold in showcase |
| 8 | Sun-glass tint | §3.1 | Pale honey C ≤ 0.07, white-gold core |
| 9 | Forge emissive colour | §3.1 | White-gold, C ≤ 0.06 |
| 10 | Heat tint and the blue band | §3.1 | Straw to purple only in play |
| 11 | Void emissive hue | §3.2 | Violet main, teal secondary |
| 12 | How dark the Void may go | §3.2 | Luma floor 45 and ≥ 15 % light material per hero |
| 13 | Lane redness | §3.3 | C ≤ 0.06, h 44–55°, plus a value step to the ash |
| 14 | Ember crystals | §3.3 | Dark body, white-gold core ≤ +1 EV |
| 15 | Bronze patina colour | §4 | Statuary brown-black in play |
| 16 | Dust on units | §4 | ≤ 20 %, lower body |
| 17 | Emissive caps (+1/+2 EV, area shares) | §1.1 | As proposed, then tuned on the first Ember Crossing captures |
