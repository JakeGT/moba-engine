# Art bible

**Version:** v0 draft, 2026-10-31. **Status:** for review: nothing here is final until the
owner of each part signs it off.
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
| World and factions | Maintainer | [world-and-factions.md](world-and-factions.md) | Drafted from the decisions below; names and lore are placeholders |
| Palette and value | Maintainer + designer | [palette-and-value.md](palette-and-value.md) | Numbers drafted (OKLCH budgets, team zones) |
| Lighting mood (Ember Crossing) | Maintainer | [lighting-mood.md](lighting-mood.md) | Four moods as bake inputs; one recommended |
| Material vocabulary | Agent draft, maintainer approves | [material-vocabulary.md](material-vocabulary.md) | Drafted from physicallybased.info |
| Silhouettes and scale | Maintainer | [silhouettes-and-scale.md](silhouettes-and-scale.md) | Size classes and rules drafted |
| Heroes, minions and structures (v0) | Maintainer | [heroes-and-minions-v0.md](heroes-and-minions-v0.md) | Three slice-hero concepts to pick from |
| VFX language | Maintainer + designer | [vfx-language.md](vfx-language.md) | Drafted on top of the R5 rules |
| UI, HUD and typography | Designer | [ui-hud-typography.md](ui-hud-typography.md) | Brief only |
| Brand and key art | Marketing + designer | [brand-and-key-art.md](brand-and-key-art.md) | Brief only |
| References | Agent | [references.md](references.md) | Named CC0 assets; no bulk downloads |
| Checks | Agent | [checks.md](checks.md) | Every rule mapped to where it runs |
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
| 2026-10-31 | Also the **Gemini API ("Nano Banana"), paid tier, for concept art and model sheets only**: a reference people model from, never shipped and never fed to a generator (enforced by the A0 provenance gate) | Maintainer |

## Decisions needed

Every open choice is marked **"Decision needed (…)"** in its file, with options and a
recommendation. In priority order for the slice:

1. **The slice hero:** Kiln Warden (recommended), Still Oracle or Ash Strider. See [heroes](heroes-and-minions-v0.md).
2. **Map 1's lighting mood:** Golden Crossing (recommended). See [lighting](lighting-mood.md).
3. **Faction readings:** Forge B (Sun-Kiln) with A's emblems and C's machinery, Void B (Gravity Monastery) with A's ornament (recommended after the first concept round). See [world](world-and-factions.md#interpretations-for-review).
4. **Which faction holds which base:** Forge west, Void east (recommended). See [world](world-and-factions.md#ember-crossing-who-stands-where).
5. **Are heroes faction-bound?** Free agents (recommended). See [world](world-and-factions.md#ember-crossing-who-stands-where).
6. **The ember crystals' colour:** white-gold cores (recommended). See [palette](palette-and-value.md#6-ember-crossing-palette-map-1).
7. **How dark the Void may go:** dark bodies plus mandatory pale elements (recommended). See [palette](palette-and-value.md#5-faction-palettes-draft).
8. **The chroma caps and measurement method.** See [palette](palette-and-value.md) and [materials](material-vocabulary.md#8-decisions-needed).
9. **`team_tint` on photoreal content:** 0, with the mask kept for Compat (recommended). See [materials](material-vocabulary.md#8-decisions-needed).
10. **Ability-type coding:** by shape and motion, not colour (recommended). See [VFX](vfx-language.md).
11. **Rim vs outline on allies.** See [silhouettes](silhouettes-and-scale.md#team-rims-the-team-layer-on-units).
12. **Hero scale:** role-scaled S/M/L (recommended). See [silhouettes](silhouettes-and-scale.md#size-classes).
13. Names (world, factions, heroes), neutral monsters, the Builders' presence, and the
    designer's and marketing's open questions in their briefs.

## How to propose a change

1. Open a PR that edits the relevant part. Say which decision it changes, and why.
2. If it changes a numeric rule, update [checks.md](checks.md) in the same PR.
3. The part's lead approves (the table above). Agents may open PRs but never merge changes
   to this folder: people decide.
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

Two lanes, both recorded per image in the review app's `manifest.json`:

| Lane | Tool | Licence of the output | Use |
|---|---|---|---|
| Local | Z-Image-Turbo (Apache-2.0) in ComfyUI, `tools/gen.py` | CC0 (roadmap §9.5) | Exploration in bulk: moods, readings, variants |
| Gemini | Gemini API, paid tier, `tools/gen_gemini.py` | Concept only (`LicenseRef-Gemini-API-Output`) | Model sheets from a chosen concept: turnarounds, A-pose fronts, detail callouts, fixes the local model can't draw |

The path from concept to final art:

1. Pick one concept per subject in the review app.
2. Make its **model sheet** in the Gemini lane, sending the pick as a reference image, and
   write down the answer to every inconsistency (left or right hand, counts, attachments).
3. **Model it by hand** from the sheet. The model is original work; its `provenance.toml`
   `notes` name the sheet it was modelled from. The A0 gate rejects any shipped asset
   generated with Gemini or from a Gemini image, whatever licence the entry claims.
4. Make **final artwork from the model**: renders, portraits and key art.

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
Readability tools: **actual size** (30 px / 40 px heroes), **value only** (greyscale), and
**colour-blind** simulation (protanopia, deuteranopia, tritanopia).
