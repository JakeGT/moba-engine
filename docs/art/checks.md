# Checks: how the bible is enforced

**Status:** v0 approved 2026-11-14. The maintainer approved the bible's thresholds with v0
([decision log](README.md#decision-log)), so every rule below is **approved** unless its
status says otherwise. Roadmap §9.6: each decision becomes a numeric rule, so the bible is
enforced on every asset rather than remembered. No check is built yet.

## Where a rule runs

| Where | What it sees | Phase that builds it |
|---|---|---|
| **`content-check`** (authoring) | Source textures, meshes and manifests, before KTX2 | A0 (licence gate) and later asset-track work |
| **Review renders** (roadmap §9.4) | Contact sheets: 46 m at 1080p, 1440p and 4K; 8 m zoom; showcase turntable; lineup under each map grade | Asset pipeline |
| **Readability captures** (roadmap §4.4) | Headless standard scenes per tier, on lavapipe in CI and on the GPU runner | N1 and later (with the overlay layer) |
| **Renderer locks** | Ranked and tournament settings | N0/N1 tier presets |

## Index

| Rule | Summary | Runs in | Defined in | Status |
|---|---|---|---|---|
| PAL-1 | Ground chroma C ≤ 0.06 (kept at 0.06, 2026-11-14) | content-check + captures | [palette-and-value.md](palette-and-value.md) | approved |
| PAL-2 | Other surfaces C ≤ 0.08 | content-check + captures | palette | approved |
| PAL-3 | Accents ≤ 5 % area: C ≤ 0.11 in team zones, ≤ 0.13 outside | review renders | palette | approved |
| PAL-4 | Emissive in team zones C ≤ 0.06 | content-check + captures | palette | approved |
| PAL-5 | Emissive outside zones C ≤ 0.12 | content-check | palette | approved |
| VAL-1 | Terrain albedo L 0.32–0.62 | content-check | palette | approved |
| VAL-2 | Rendered ground L 0.30–0.58, spread ≤ 0.30 | captures | palette | approved |
| VAL-3 | Shadow floor L ≥ 0.18 | captures | palette | approved |
| VAL-4 | Non-gameplay highlight ceiling L ≤ 0.85 | captures | palette | approved |
| VAL-5 | Off-board transition ≤ 0.15 L/m | captures | palette | approved |
| CON-1 | Hero vs ground \|ΔL\| ≥ 0.10 without rims | review renders + captures | palette | approved |
| CON-2 | With rims \|ΔL\| ≥ 0.15; rim C ≥ 0.10 at the team hue | captures | palette | approved |
| CON-3 | Enemy telegraph ΔE_OK ≥ 0.20, edge ΔL ≥ 0.25 | captures | palette | approved |
| CON-4 | Minion vs ground \|ΔL\| ≥ 0.08 | review renders + captures | palette | approved |
| CON-5 | No tier worse than Low − 0.02 | captures | palette | approved |
| CB-1 to CB-3 | Colour-blind separation of team colours, accents and telegraphs | captures | palette | approved |
| VOID-1 | Every Void unit carries a bone or silver element ≥ 10 % of its projected area | review renders | palette §5 | approved |
| EMB-1 | Ember crystals: white-gold core (PAL-4), ≤ +1 EV (≤ +2 EV only in a gameplay state); orange only as a non-emissive facet rim ≤ 3 % of area (PAL-3); none on the lanes | content-check + review renders | palette §6, [material-vocabulary.md](material-vocabulary.md) §3.3 | approved |
| LANE-1 | Lane grit albedo C ≤ 0.06 (p99), h 44–55°, luma 110–120 (a value step above the ash) | content-check | materials §3.3 | approved |
| EXP-1 to EXP-4 | Exposure lock in ranked, ±0.5 EV casual, VFX don't drive the meter | renderer locks | [lighting-mood.md](lighting-mood.md) | approved |
| SHD-1 to SHD-3 | No unlit playable area; units in shadow pass CON-1; shadow azimuth | captures + map review | lighting | approved |
| SIL-1 to SIL-9 | Size classes (role-scaled Hero S/M/L, 2026-11-14), minion vs hero, silhouette IoU < 0.80, top-third tell, faction tells, thin parts | review renders | [silhouettes-and-scale.md](silhouettes-and-scale.md) | approved |
| RIM-1 to RIM-6 | Team rim width, survives fog, enemy > ally, no rim-like edges in team hues | captures | silhouettes | approved |
| RIM-7 | Allies: rim, no outline; outline only on enemies and the hovered unit | captures | silhouettes | approved; renderer default flips after CON-2 (ADR-0023 §8c) |
| TEL-1 to TEL-3 | Telegraph contrast, overlay-only, bodies don't cover enemy edges | captures | [vfx-language.md](vfx-language.md) | approved |
| VFX-1 to VFX-8 | Body chroma (C ≤ 0.06 in team zones, ≤ 0.12 outside), VFX light ceiling, ally dimming, budgets, flashes, shake, distortion, motion vectors | content-check (VFX files) + captures | vfx | approved |
| VFX-9 | Ability type coded by shape and motion, never colour | review renders | vfx | approved |
| UI-1 to UI-4 | Text and icon contrast, font licences, minimum size | HUD tests + licence gate | [ui-hud-typography.md](ui-hud-typography.md) | **proposed** (designer brief, open) |
| PBR-ALB-1, PBR-ALB-2 | Dielectric luma 30–240, metal luma ≥ 180 (p1/p99 per texel) | content-check | [material-vocabulary.md](material-vocabulary.md) | approved |
| PBR-MET-1 | ≤ 3 % metal-transition texels | content-check | materials | approved |
| PBR-RGH-1, PBR-RGH-2 | Roughness variation and floor | content-check | materials | approved |
| PBR-NRM-1, PBR-NRM-2 | Normal length; BC5 validity | content-check | materials | approved |
| PBR-DELIGHT-1 | No baked lighting in albedo | content-check | materials | approved |
| PBR-CLASS-1, PBR-VOCAB-1 | Every texel has a class (source-side class-ID map); every material names a vocabulary entry | content-check | materials | approved |
| PBR-METAL-1 … PBR-BONE-1 | Per-class albedo, roughness, metalness ranges | content-check | materials | approved |
| PBR-GLASS-1, PBR-SKIN-1 | IOR and SSS parameters | content-check | materials | approved |
| PBR-EMIT-1 to PBR-EMIT-3 | Emissive brightness (accents ≤ +1 EV, gameplay ≤ +2 EV), area and screen share | content-check + captures | materials | approved |
| PBR-WEAR-1 | Dust ≤ 20 % on units, lower body only | review renders | materials | approved |
| PBR-FORGE-1 | Sun-alloy, in-play gilt and sun-glass tint C ≤ 0.07 | content-check | materials §3.1 | approved |
| PBR-HEAT-1 | In playable space: temper colours straw to purple only (no blue); bronze patina statuary brown-black (no verdigris) | review renders | materials §3.1, §4 | approved |
| PBR-VOID-1 | Void hero body materials luma ≥ 45; ≥ 15 % of each Void hero's surface in pewter, silver or bone | content-check | materials §3.2 | approved; floor vs two material targets open ([README](README.md#open)) |
| PBR-VOID-2 | Void emissive hue: violet 284–345° main, teal 167–211° minor | content-check | materials §3.2 | approved |
| TINT-1 | `team_tint = 0` on photoreal (Tiered) content; the mask kept for Compat | content-check | materials §1.1 | approved; renderer default flips after CON-2 (ADR-0023 §8c) |

## Order of implementation

1. **With A0:** the licence and provenance gate, and `PBR-ALB-*`, `PBR-NRM-*`, PAL-1, PAL-2,
   PAL-4, LANE-1, PBR-FORGE-1 and TINT-1 on source textures and materials. These are cheap and
   catch most AI-pipeline mistakes.
2. **With the review renders:** SIL-*, CON-1, CON-4, PAL-3, VOID-1, EMB-1, PBR-HEAT-1 and
   VFX-9, run on every new asset.
3. **With N1's overlay layer and the readability captures:** VAL-*, CON-2, CON-3, CON-5,
   CB-*, RIM-*, TEL-* and the heatmap debug view.
4. **With the tier presets:** EXP-* and the ranked locks.

A rule moves from **approved** to **enforced** when its check is built and the first real
Ember Crossing captures confirm it doesn't fail good art. Changing an approved threshold
(including after those captures) needs a maintainer decision, recorded in the
[decision log](README.md#decision-log).
