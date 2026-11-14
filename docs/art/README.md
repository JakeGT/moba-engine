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
| 2026-11-14 | **Art bible v0 approved.** The maintainer's words: "The Art style in the bible is approved - follow the recommended designs. Blender is installed. If you need any other tools installed to create models, ask." Every "Decision needed" in this folder is settled on its recommended option ([list below](#decisions-taken-2026-11-14)). Blender is the modelling tool: models are made by hand from the boards and model sheets ([concept generation](#concept-generation), step 3) | Maintainer |

## Decisions taken 2026-11-14

All taken by the maintainer's approval above, on the bible's recommended option. The numbers
are the ones the files used while the items were open, so other documents (ADR-0023 §8c) can
still cite them. Where an item had no explicit recommendation, the bible's leading option was
taken; those are marked *leading option*.

1. **The slice hero:** the **Kiln Warden** (Hero M, Forge, the `blade` kit). The Still Oracle
   and the Ash Strider stay A4 candidates. See [heroes](heroes-and-minions-v0.md).
2. **Map 1's lighting mood:** **Golden Crossing**. No time-of-day or weather variant for the
   slice. See [lighting](lighting-mood.md).
3. **Faction readings:** **Forge rec** (B with A's emblems and C's machinery) and **Void rec**
   (B with A's ornament). See [world](world-and-factions.md#faction-readings).
4. **Which faction holds which base:** **Forge west, Void east** (presentation only; the sim's
   Blue and Red are unchanged). See [world](world-and-factions.md#ember-crossing-who-stands-where).
5. **Heroes are free agents:** any hero plays on either side. See [world](world-and-factions.md#ember-crossing-who-stands-where).
6. **The ember crystals:** **white-gold cores** (PAL-4), small non-emissive orange facet rims,
   kept off the lanes. See [palette](palette-and-value.md#6-ember-crossing-palette-map-1).
7. **How dark the Void may go:** **dark bodies plus mandatory pale elements** (bone or silver,
   ≥ 10 % of every Void unit's projected area; Void heroes also ≥ 15 % light material). See
   [palette](palette-and-value.md#5-faction-palettes).
8. **The chroma caps and measurement method:** C ≤ 0.06 on the ground (PAL-1, kept at 0.06)
   and ≤ 0.08 elsewhere (p99 of texels); albedo ranges measured per texel as luma
   percentiles (p1/p99). See
   [palette](palette-and-value.md) and [materials](material-vocabulary.md#8-decisions).
9. **`team_tint` on photoreal content:** **0**, with the mask kept for Compat. The Tiered
   renderer's default flips when the rims pass CON-2 ([ADR-0023](../adr/0023-frame-architecture-materials.md) §8c). See
   [materials](material-vocabulary.md#8-decisions).
10. **Ability-type coding:** **by shape and motion**, never colour. See [VFX](vfx-language.md).
11. **Rim vs outline on allies:** **rim on everyone, outline only on enemies and the hovered
    unit**. The renderer's default flips when CON-2 passes (ADR-0023 §8c). See
    [silhouettes](silhouettes-and-scale.md#team-rims-the-team-layer-on-units).
12. **Hero scale:** **role-scaled S/M/L**. See [silhouettes](silhouettes-and-scale.md#size-classes).
13. **The rest:**
    - names: the working names are kept as internal names (world option (b)); the public
      names come from the brand pass. Heroes get new display names; kit IDs are unchanged;
    - the neutral monsters: **ember scarabs** for the slice camp, and a dormant Builder warden
      machine as the large objective in A4;
    - the Builders appear only as ruins and relic machines, never as figures;
    - green/blue/red stay the team colours for v0;
    - every option in [material-vocabulary.md §8](material-vocabulary.md#8-decisions) on its
      recommendation: (a) for each item, and (c) (both) for how dark the Void may go;
    - the lane's value step: lift the lane to luma about 115 (*leading option*; the alternative
      was darkening the burnt ash beside it).

## Open

- **The designer's brief** ([ui-hud-typography.md](ui-hud-typography.md)): faction-skinned or
  neutral HUD, the display face, the icon style, and the UI-1 to UI-4 thresholds. Owned by a
  graphic designer who has not been engaged yet.
- **Marketing's brief** ([brand-and-key-art.md](brand-and-key-art.md)): the game's name and
  logo, the public faction names, and which key-art direction to develop. Owned by a marketing
  executive and the designer, not yet engaged.
- **Void luma floor vs two material targets.** The decided floor (luma ≥ 45 on Void heroes)
  sits above two targets in [material-vocabulary.md §3.2](material-vocabulary.md#32-the-void):
  obsidian metamaterial (42) and Void lacquer (39). Whether those targets rise or stay as
  structure-only materials needs a maintainer decision.

## How to propose a change

1. Open a PR that edits the relevant part. Say which decision it changes, and why.
2. If it changes a numeric rule, update [checks.md](checks.md) in the same PR.
3. The part's lead approves (the table above). The maintainer's approval of v0 on 2026-11-14
   is recorded in the decision log; every later change to this folder still needs a
   maintainer decision before it merges. Agents draft and open PRs: people decide.
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
