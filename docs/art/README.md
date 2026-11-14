# Art bible

**Version:** v0, 2026-10-31. **Status:** v0 approved 2026-11-14 by the maintainer (see the
[decision log](#decision-log)). The two briefs (UI/HUD and brand) stay open until their owners
are engaged.
**Owner:** the maintainer and their team (possibly a graphic designer and a marketing
executive). Agents draft, research and turn decisions into checks; **people decide**
([roadmap §9.6](../plans/next-gen-graphics-roadmap.md#96-art-bible)).

**Versions:**
- **v0** gates the vertical slice (A3): enough to build one map section of Ember Crossing,
  one hero and the minions.
- **v1** is signed off by the maintainer after the slice's go/no-go, and gates the roster (A4).

## Parts

| Part | Lead | File | v0 state |
|---|---|---|---|
| World and factions | Maintainer | [world-and-factions.md](world-and-factions.md) | Approved 2026-11-14; working names kept, public names left to the brand pass |
| Palette and value | Maintainer + designer | [palette-and-value.md](palette-and-value.md) | Approved 2026-11-14 (OKLCH budgets, team zones) |
| Lighting mood (Ember Crossing) | Maintainer | [lighting-mood.md](lighting-mood.md) | Approved 2026-11-14: Golden Crossing |
| Material vocabulary | Agent draft, maintainer approves | [material-vocabulary.md](material-vocabulary.md) | Approved 2026-11-14 |
| Silhouettes and scale | Maintainer | [silhouettes-and-scale.md](silhouettes-and-scale.md) | Approved 2026-11-14: role-scaled S/M/L heroes |
| Heroes, minions and structures (v0) | Maintainer | [heroes-and-minions-v0.md](heroes-and-minions-v0.md) | Approved 2026-11-14: the Kiln Warden is the slice hero |
| VFX language | Maintainer + designer | [vfx-language.md](vfx-language.md) | Approved 2026-11-14 |
| UI, HUD and typography | Designer | [ui-hud-typography.md](ui-hud-typography.md) | Brief only; **open** (designer not yet engaged) |
| Brand and key art | Marketing + designer | [brand-and-key-art.md](brand-and-key-art.md) | Brief only; **open** (marketing and designer not yet engaged) |
| References | Agent | [references.md](references.md) | Named CC0 candidates; no bulk downloads; each asset still picked before it is fetched |
| Checks | Agent | [checks.md](checks.md) | Every rule mapped to where it runs; thresholds approved 2026-11-14 |
| Boards | Agent | [boards/](boards/) | Curated boards, with [provenance](boards/PROVENANCE.md) |

## Decision log

| Date | Decision | By |
|---|---|---|
| 2026-10-30 | Full photoreal; readability from HUD, outlines, rims, telegraphs and VFX rules (roadmap §1, §4) | Maintainer |
| 2026-10-30 | Setting: **science fantasy**. Tone: **epic, weathered** | Maintainer |
| 2026-10-30 | **Two opposed factions: the Solar Forge vs the Void** | Maintainer |
| 2026-10-30 | **Factions by shape and material; teams by rim** (player-relative team colours stay) | Maintainer |
| 2026-10-30 | Map 1 / vertical slice: **Ember Crossing** | Maintainer |
| 2026-10-30 | Slice hero picked from **three drafted concepts** | Maintainer |
| 2026-10-30 | Concept generation with **Apache-licensed local models only**; outputs CC0 with provenance | Maintainer |
| 2026-10-31 | **Technology so advanced it looks like magic**: weapons, armour, architecture, units and the Builders' relics are all technology ([world](world-and-factions.md#technology-that-looks-like-magic)) | Maintainer |
| 2026-10-31 | Also the **Gemini API ("Nano Banana"), paid tier, for concept art and model sheets only**: a reference people model from, never shipped and never fed to a generator (enforced by the A0 provenance gate) | Maintainer |
| 2026-10-31 | **Nano Banana 2.1 for drafts, Pro for model sheets** (after a side-by-side test) | Maintainer |
| 2026-10-31 | Whole bible redrawn and rewritten for technology that looks like magic: rounds G3 (factions), G4 (everything else) and G5 (hero model sheets); materials renamed to technology names with measured stand-ins | Agent draft; approved by the maintainer 2026-11-14 (below) |
| 2026-11-14 | **Art bible v0 approved.** The maintainer's words: "The Art style in the bible is approved - follow the recommended designs. Blender is installed. If you need any other tools installed to create models, ask." Every "Decision needed" in this folder is settled on its recommended option ([list below](#decisions-taken-2026-11-14)), except the five questions in the two briefs (designer: HUD skin, display face, icon style; marketing: name and logo, key-art direction), which stay open, and the lane's value step, which had no recommendation (see [Open](#open)). Blender is installed for modelling; models are made by hand from the boards and model sheets, as [concept generation](#concept-generation) step 3 already says | Maintainer |
| 2026-11-14 | Slice hero: **the Kiln Warden** (the `Blade` kit). Oracle and Strider stay A4 candidates | Maintainer (approval above) |
| 2026-11-14 | The Kiln Warden's size class is **Hero M** (the draft said "M/L" at 2.0 m) | (derived by the agent; for the maintainer to confirm) |
| 2026-11-14 | Map 1's mood: **Golden Crossing**; no time-of-day or weather variant for the slice | Maintainer (approval above) |
| 2026-11-14 | Faction readings: **Forge rec** and **Void rec** | Maintainer (approval above) |
| 2026-11-14 | Bases: **Forge west, Void east** (presentation only) | Maintainer (approval above) |
| 2026-11-14 | **Heroes are free agents** | Maintainer (approval above) |
| 2026-11-14 | Ember crystals: **white-gold cores** with small orange facet rims, kept off the lanes | Maintainer (approval above) |
| 2026-11-14 | The ember facet rim is **non-emissive** (PAL-3 accent ≤ 3 %); the core swatch is `#FFE6BE` | (derived by the agent; for the maintainer to confirm) |
| 2026-11-14 | Void: **dark bodies with mandatory pale elements** (bone or silver ≥ 10 % of every Void unit's projected area), plus the material rule: Void body materials at albedo luma ≥ 45 and ≥ 15 % of each Void hero's area in a light material (area read as projected area: derived) | Maintainer (approval above) |
| 2026-11-14 | Four Void materials lifted to luma 46–48 (obsidian polished and knapped 46, composite 47, lacquer 48) at the same hue and chroma, and the palette's Void swatches to luma 46–47 | (derived by the agent; for the maintainer to confirm) |
| 2026-11-14 | Chroma caps **0.06 ground (PAL-1 kept) / 0.08 elsewhere**, p99; albedo ranges per texel at p1/p99 | Maintainer (approval above) |
| 2026-11-14 | **`team_tint` 0 on photoreal content**, mask kept for Compat: on Tiered the renderer ignores the tint (ADR-0023 §8c `team_tint_policy = zero`) once the rims pass CON-2; Compat keeps it | Maintainer (approval above) |
| 2026-11-14 | **Ability types coded by shape and motion**, never colour | Maintainer (approval above) |
| 2026-11-14 | **Rim on everyone; outline only on enemies and the hovered unit** | Maintainer (approval above) |
| 2026-11-14 | **Role-scaled S/M/L heroes** | Maintainer (approval above) |
| 2026-11-14 | Neutral monsters: **ember scarabs** (slice camp); a Builder warden machine for A4 | Maintainer (approval above) |
| 2026-11-14 | Names: **working names kept as internal names**, with public in-world names picked later by the brand pass; new hero display names, kit IDs unchanged; the Builders as ruins and relic machines only | Maintainer (approval above) |
| 2026-11-14 | Green/blue/red kept as team colours for v0 | Maintainer (approval above) |
| 2026-11-14 | Material vocabulary: every item on its recommendation, as listed in [§8](material-vocabulary.md#8-decisions); for how dark the Void may go, both the rims and the material floor | Maintainer (approval above) |
| 2026-11-14 | Lane value step: **lift the lane to luma 112–119** (no recommendation in the draft: its first-named option, over darkening the ash) | (derived by the agent; for the maintainer to confirm) |
| 2026-11-14 | The other derived numbers listed under [Open](#open) | (derived by the agent; for the maintainer to confirm) |

## Decisions taken 2026-11-14

All taken by the maintainer's approval above, on the bible's recommended option. The item
numbers are unchanged, so other documents (ADR-0023 §8c) can still cite them. Applying some
options needed numbers the draft did not have (the lane, `L_ref`, the ember swatches, VFX-1,
LANE-1 and the others listed under [Open](#open)); the agent derived those, and they are
marked as derived until the maintainer confirms them. The one item with no recommendation,
the lane's value step, took the draft's first-named option, also marked derived.

1. **The slice hero:** the **Kiln Warden** (Forge, the `Blade` kit; Hero M, derived). The Still Oracle
   and the Ash Strider stay A4 candidates. See [heroes](heroes-and-minions-v0.md).
2. **Map 1's lighting mood:** **Golden Crossing**. No time-of-day or weather variant for the
   slice. See [lighting](lighting-mood.md).
3. **Faction readings:** **Forge rec** (B with A's emblems and C's machinery) and **Void rec**
   (B with A's ornament). See [world](world-and-factions.md#faction-readings).
4. **Which faction holds which base:** **Forge west, Void east** (presentation only; the sim's
   Blue and Red are unchanged). See [world](world-and-factions.md#ember-crossing-who-stands-where).
5. **Heroes are free agents:** any hero plays on either side. See [world](world-and-factions.md#ember-crossing-who-stands-where).
6. **The ember crystals:** **white-gold cores** (PAL-4), small orange facet rims (non-emissive:
   derived), kept off the lanes. See [palette](palette-and-value.md#6-ember-crossing-palette-map-1).
7. **How dark the Void may go:** **dark bodies plus mandatory pale elements** (bone or silver,
   ≥ 10 % of every Void unit's projected area), and Void body materials at albedo luma ≥ 45
   with ≥ 15 % of each Void hero's area in a light material (read as projected area: derived). See
   [palette](palette-and-value.md#5-faction-palettes).
8. **The chroma caps and measurement method:** C ≤ 0.06 on the ground (PAL-1, kept at 0.06)
   and ≤ 0.08 elsewhere (p99 of texels); albedo ranges measured per texel as luma
   percentiles (p1/p99). See
   [palette](palette-and-value.md) and [materials](material-vocabulary.md#8-decisions).
9. **`team_tint` on photoreal content:** **0**, with the mask kept for Compat. This is a Tiered
   renderer policy, not a content value: the renderer's `team_tint_policy` flips to `zero`
   when the rims pass CON-2 ([ADR-0023](../adr/0023-frame-architecture-materials.md) §8c), and
   Compat keeps the team tint. See
   [materials](material-vocabulary.md#8-decisions).
10. **Ability-type coding:** **by shape and motion**, never colour. See [VFX](vfx-language.md).
11. **Rim vs outline on allies:** **rim on everyone, outline only on enemies and the hovered
    unit**. The renderer's default flips when CON-2 passes (ADR-0023 §8c). See
    [silhouettes](silhouettes-and-scale.md#team-rims-the-team-layer-on-units).
12. **Hero scale:** **role-scaled S/M/L**. See [silhouettes](silhouettes-and-scale.md#size-classes).
13. **The rest:**
    - names: the working names are kept as internal names, and the public in-world names
      come from the brand pass. Heroes get new display names; kit IDs are unchanged;
    - the neutral monsters: **ember scarabs** for the slice camp, and a dormant Builder warden
      machine as the large objective in A4;
    - the Builders appear only as ruins and relic machines, never as figures;
    - green/blue/red stay the team colours for v0;
    - every item in [material-vocabulary.md §8](material-vocabulary.md#8-decisions) on its
      recommendation; for how dark the Void may go, both the rims and the material floor;
    - the lane's value step: lift the lane to luma 112–119 (derived: the draft's first-named
      option; the alternative was darkening the burnt ash beside it).

## Open

- **The designer's brief** ([ui-hud-typography.md](ui-hud-typography.md)): faction-skinned or
  neutral HUD, the display face, the icon style, and the UI-1 to UI-4 thresholds. Owned by a
  graphic designer who has not been engaged yet.
- **Marketing's brief** ([brand-and-key-art.md](brand-and-key-art.md)): the game's name and
  logo, the public faction names, and which key-art direction to develop. Owned by a marketing
  executive and the designer, not yet engaged.
- **The lane lift raises every emissive cap.** `L_ref` is the lane in direct sun, so lifting
  the lane raises every emissive cap: about +40 % against the draft lane's centre (`#7E5A47`)
  and about +70 % against the draft's printed `L_ref` (0.047, at 25°). The maintainer can
  confirm the lift or take the other option (darken the ash beside the lane) instead.

### For the maintainer to confirm (derived by the agent)

These numbers were derived by the agent while applying the approved options. They are in
force for v0 but are not the maintainer's words:
- **Kiln Warden size class:** Hero M (the draft said "M/L" at 2.0 m; 2.0 m is in M).
- **The lane:** lifted to luma 112–119 (`#8D6855`–`#956F5D`, display swatch `#906B58`),
  LANE-1; and the `L_ref` consequence above.
- **Ember crystals:** the orange facet rim is non-emissive (PAL-3 accent ≤ 3 %); the draft
  display swatch `#FFD9A8` (C 0.076, above PAL-4) is replaced by the authored core `#FFE6BE`;
  the Ember Crossing display swatches now match the material targets.
- **VFX-1:** the outside-zone cap is 0.12 (was 0.13), to match PAL-5.
- **Violet edge:** `#A08CD0` everywhere (the VFX `#9F86D9` broke VFX-1).
- **PBR-FORGE-1:** sun-alloy is checked at C ≤ 0.075, the measured brass chroma (0.072) the
  recommendation named, so the bible's own brass passes. Gilt and sun-glass keep their decided
  C ≤ 0.07: the sun-glass tint moved from `#D9B98A` (C 0.072) to `#D8B98C` (C 0.069), and the
  gilt and worn sun-alloy display swatches were retargeted to ≤ 0.07.
- **Void:** obsidian (polished and knapped) moved to `#2F2D33` (luma 46), Void lacquer to
  `#352D39` (luma 48), Void composite to `#312D37` (luma 47), and the palette's Void swatches
  to luma 46–47, all at the same hue and chroma, so the targets sit above the floor rather than
  on it. PBR-VOID-1's floor applies to a material's median luma, with p1 allowed down to about
  40 for weathering (§4 rule 2). The 15 % light-material share is read as projected area.
- **Desert sand:** in playable space lowered to `#8B7D5C` (L 0.59, below VAL-1's 0.62 ceiling,
  leaving headroom for lighter texels and abrasion); the lighter draft sand is for off-board
  drifts only.
- **SIL-2:** minions ≤ 0.8× a Hero M (1.6 m), with siege judged by footprint and shape (the
  draft's "smallest hero" gave 1.28 m, which its own minion sizes break).
- **Builder alloy:** `#B4B6B8` (luma 182) with an aluminium or steel stand-in, so it passes
  the 180 metal floor (titanium, luma 171, does not).
- **The Still Oracle's crown:** one description, three hovering Void-composite segments.
- **Modelling:** the heroes' next steps say "model it by hand in Blender" instead of
  "image → TRELLIS.2".
- **Corrections (not decisions):** Dust Veil's sun elevation is ~30° (computed from its
  `sun_dir`; the draft said ~35°), and the VFX history's orange-fire note cites PAL-4 (the
  draft said PAL-5).
- **Thresholds:** every non-decision threshold (VAL, CON, CB, SIL, RIM, TEL, VFX, EXP, SHD,
  PBR) marked approved; UI-1 to UI-4 stay proposed with the designer's brief.
- **Briefs:** their "Decision needed" markers renamed "Open".
- **New check IDs:** VOID-1, EMB-1, LANE-1, RIM-7, VFX-9, PBR-FORGE-1, PBR-HEAT-1, PBR-VOID-1,
  PBR-VOID-2 and TINT-1.

### Follow-ups outside this folder

- [ADR-0023](../adr/0023-frame-architecture-materials.md) still calls decisions 9 and 11
  open, and its §1a quotes the draft's Void body range (luma 39–58; now 46–58). Its switches
  flip after CON-2 as written, so only the wording is stale.
- [boards/palettes.jpg](boards/palettes.jpg) shows the draft palette values; see
  [boards/PROVENANCE.md](boards/PROVENANCE.md).

## How to propose a change

1. Open a PR that edits the relevant part. Say which decision it changes, and why.
2. If it changes a numeric rule, update [checks.md](checks.md) in the same PR.
3. The part's lead approves (table above) and the approval is recorded in the decision log.
   Agents draft and open PRs and never merge changes to this folder, except a PR that
   records a decision the maintainer stated, quoted verbatim in the log, and makes no other
   change. Values an agent
   derives while applying it stay marked "for the maintainer to confirm" until the maintainer
   confirms them.
4. Record approved decisions in the decision log, with the date.

## Licences for what lives here

- Text and rules: the repository's licence.
- **Reference and concept images** in this folder are **CC0 or team-owned** only: CC0 photos
  and HDRIs (Poly Haven, ambientCG and others in [references.md](references.md)), and
  AI-generated images (CC0 per roadmap §9.5, each with a provenance record).
- **Gemini concept images** (model sheets, turnarounds, callouts) are the one exception:
  they come from a proprietary service under Google's terms and carry a SynthID watermark.
  They may appear on boards here, labelled "Gemini, concept only" in
  [boards/PROVENANCE.md](boards/PROVENANCE.md), but never in `content/`. See
  [concept generation](#concept-generation).
- Work made by team members goes in only as **CC0 or CC-BY**, or under a written assignment.
- **Brand marks (logo, name) are the exception:** all-rights-reserved, and listed as such.
- Images in git are kept small: ≤ 400 KB each and ≤ 25 MB for the folder. Variants and raw
  generations stay in the review app's folder, outside the repository.

## Concept generation

Three lanes, all recorded per image in the review app's `manifest.json`:

| Lane | Tool | Licence of the output | Use |
|---|---|---|---|
| Local | Z-Image-Turbo (Apache-2.0) in ComfyUI, `tools/gen.py` | CC0 (roadmap §9.5) | Exploration in bulk: moods, readings, variants |
| Gemini drafts | Gemini API, paid tier, **Nano Banana 2.1** (2K, batch: about $0.025 per image), `tools/gen_gemini.py` | Concept only (`LicenseRef-Gemini-API-Output`) | Bible reference and exploration: readings, structures, units, set pieces, VFX, fixes the local model can't draw |
| Gemini model sheets | Gemini API, paid tier, **Nano Banana Pro** (2K, batch: about $0.067 per image) | Concept only | Anything a modeller works from directly: turnarounds, A-pose fronts, detail callouts from a chosen concept |

The path from concept to final art:

1. Pick one concept per subject in the review app.
2. Make its **model sheet** in the Gemini lane, sending the pick as a reference image, and
   write down the answer to every inconsistency (left or right hand, counts, attachments).
3. **Model it by hand** from the sheet. The model is original work; its `provenance.toml`
   `notes` name the sheet it was modelled from. The A0 gate rejects any shipped asset
   generated with Gemini or from a Gemini image, whatever licence the entry claims.
4. Make **final artwork from the model**: renders, portraits and key art.

Why two Gemini models: a side-by-side of four faction prompts (2026-10-31) showed 2.1 matching
Pro's detail at the same resolution, with better wide establishing shots, for about 38% of the
price. Pro followed long lists of constraints slightly better and copies reference images more
closely, which is what a model sheet needs.

The Gemini lane needs an API key in `~/.config/moba/gemini.env`, outside every repository.
Paid-tier prompts and outputs are not used to improve Google's products (Gemini API pricing
page, 2026-10-31); the free tier's are, so it is not used.

## Review app

A local, throwaway web app for reviewing every option side by side, with **Approve / Reject
/ Prefer** and comments. It lives **outside** the repository, in
`~/Projects/MOBA/art-review/`, and saves picks to `feedback.json` there, for an agent to fold
into v1.

```bash
python3 ~/Projects/MOBA/art-review/serve.py
```

It then runs at <http://127.0.0.1:8765>; the tailnet URL is in the session's morning report.
It works on a phone as well as a desktop. A *Start here* page summarises the direction and
progress; in every section the **current direction** comes first and the fantasy-era
**earlier rounds** are folded away underneath. Tap an image to open it, then swipe or use the
arrow keys to move through its group. Readability tools: **actual size** (30 px / 40 px
silhouettes), **value only** (greyscale), and **colour-blind** simulation (protanopia,
deuteranopia, tritanopia), under *View*.
