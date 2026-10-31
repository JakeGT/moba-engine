# Checks: how the bible is enforced

**Status:** v0 draft, 2026-10-31. Every rule here is **proposed** until the maintainer approves
it. Roadmap §9.6: each decision becomes a numeric rule, so the bible is enforced on every
asset rather than remembered.

## Where a rule runs

| Where | What it sees | Phase that builds it |
|---|---|---|
| **`content-check`** (authoring) | Source textures, meshes and manifests, before KTX2 | A0 (licence gate) and later asset-track work |
| **Review renders** (roadmap §9.4) | Contact sheets: 46 m at 1080p, 1440p and 4K; 8 m zoom; showcase turntable; lineup under each map grade | Asset pipeline |
| **Readability captures** (roadmap §4.4) | Headless standard scenes per tier, on lavapipe in CI and on the GPU runner | N1 and later (with the overlay layer) |
| **Renderer locks** | Ranked and tournament settings | N0/N1 tier presets |

## Index

| Rule | Summary | Runs in | Defined in |
|---|---|---|---|
| PAL-1 | Ground chroma C ≤ 0.06 | content-check + captures | [palette-and-value.md](palette-and-value.md) |
| PAL-2 | Other surfaces C ≤ 0.08 | content-check + captures | palette |
| PAL-3 | Accents ≤ 5 % area: C ≤ 0.11 in team zones, ≤ 0.13 outside | review renders | palette |
| PAL-4 | Emissive in team zones C ≤ 0.06 | content-check + captures | palette |
| PAL-5 | Emissive outside zones C ≤ 0.12 | content-check | palette |
| VAL-1 | Terrain albedo L 0.32–0.62 | content-check | palette |
| VAL-2 | Rendered ground L 0.30–0.58, spread ≤ 0.30 | captures | palette |
| VAL-3 | Shadow floor L ≥ 0.18 | captures | palette |
| VAL-4 | Non-gameplay highlight ceiling L ≤ 0.85 | captures | palette |
| VAL-5 | Off-board transition ≤ 0.15 L/m | captures | palette |
| CON-1 | Hero vs ground \|ΔL\| ≥ 0.10 without rims | review renders + captures | palette |
| CON-2 | With rims \|ΔL\| ≥ 0.15; rim C ≥ 0.10 at the team hue | captures | palette |
| CON-3 | Enemy telegraph ΔE_OK ≥ 0.20, edge ΔL ≥ 0.25 | captures | palette |
| CON-4 | Minion vs ground \|ΔL\| ≥ 0.08 | review renders + captures | palette |
| CON-5 | No tier worse than Low − 0.02 | captures | palette |
| CB-1 to CB-3 | Colour-blind separation of team colours, accents and telegraphs | captures | palette |
| EXP-1 to EXP-4 | Exposure lock in ranked, ±0.5 EV casual, VFX don't drive the meter | renderer locks | [lighting-mood.md](lighting-mood.md) |
| SHD-1 to SHD-3 | No unlit playable area; units in shadow pass CON-1; shadow azimuth | captures + map review | lighting |
| SIL-1 to SIL-9 | Size classes, minion vs hero, silhouette IoU < 0.80, top-third tell, faction tells, thin parts | review renders | [silhouettes-and-scale.md](silhouettes-and-scale.md) |
| RIM-1 to RIM-6 | Team rim width, survives fog, enemy > ally, no rim-like edges in team hues | captures | silhouettes |
| TEL-1 to TEL-3 | Telegraph contrast, overlay-only, bodies don't cover enemy edges | captures | [vfx-language.md](vfx-language.md) |
| VFX-1 to VFX-8 | Body chroma, VFX light ceiling, ally dimming, budgets, flashes, shake, distortion, motion vectors | content-check (VFX files) + captures | vfx |
| UI-1 to UI-4 | Text and icon contrast, font licences, minimum size | HUD tests + licence gate | [ui-hud-typography.md](ui-hud-typography.md) |
| PBR-ALB-1, PBR-ALB-2 | Dielectric luma 30–240, metal luma ≥ 180 (p1/p99) | content-check | [material-vocabulary.md](material-vocabulary.md) |
| PBR-MET-1 | ≤ 3 % metal-transition texels | content-check | materials |
| PBR-RGH-1, PBR-RGH-2 | Roughness variation and floor | content-check | materials |
| PBR-NRM-1, PBR-NRM-2 | Normal length; BC5 validity | content-check | materials |
| PBR-DELIGHT-1 | No baked lighting in albedo | content-check | materials |
| PBR-CLASS-1, PBR-VOCAB-1 | Every texel has a class; every material names a vocabulary entry | content-check | materials |
| PBR-METAL-1 … PBR-BONE-1 | Per-class albedo, roughness, metalness ranges | content-check | materials |
| PBR-GLASS-1, PBR-SKIN-1 | IOR and SSS parameters | content-check | materials |
| PBR-EMIT-1 to PBR-EMIT-3 | Emissive brightness, area and screen share | content-check + captures | materials |
| PBR-WEAR-1 | Dust ≤ 20 % on units | review renders | materials |

## Order of implementation (proposal)

1. **With A0:** the licence and provenance gate, and `PBR-ALB-*`, `PBR-NRM-*`, PAL-1, PAL-2
   and PAL-4 on source textures. These are cheap and catch most AI-pipeline mistakes.
2. **With the review renders:** SIL-*, CON-1, CON-4 and PAL-3, run on every new asset.
3. **With N1's overlay layer and the readability captures:** VAL-*, CON-2, CON-3, CON-5,
   CB-*, RIM-*, TEL-* and the heatmap debug view.
4. **With the tier presets:** EXP-* and the ranked locks.

A rule moves from "proposed" to "enforced" only when the maintainer approves its threshold,
and the first real Ember Crossing captures confirm it doesn't fail good art.
