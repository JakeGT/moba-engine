# UI, HUD and typography: brief for the graphic designer

**Status:** v0 brief, 2026-10-31. **Lead:** graphic designer. This is a short brief, not a
design. Agents wrote down the constraints the HUD has to meet and the open questions; the
designer decides the look.

## What exists today

- A working HUD: health and mana bars, ability bar, status icons, minimap, scoreboard,
  settings. Code: `crates/client/src/hud/`.
- **Font:** Open Sans Condensed SemiBold (SIL OFL 1.1), baked into an MSDF atlas by
  `crates/tools/font-bake`. Licence in `THIRD_PARTY.md`.
- **Team palettes:** `Standard` (green self, blue ally, red enemy) and `ColorBlind`
  (Okabe-Ito yellow, sky blue, orange). See [palette-and-value.md](palette-and-value.md) §1.
- The overlay layer (outlines, telegraphs, bars, rings) draws after upscaling at output
  resolution (roadmap §4.1), so it stays sharp at every tier.

## Constraints the design must meet

1. **Readability is carried here.** The world is photoreal and doesn't stylise for
   readability (roadmap §1), so the HUD, outlines and telegraphs do that job.
2. **Team colours are reserved** for the team layer (bars, rings, outlines, minimap,
   telegraphs). Decorative UI chrome uses neutrals and low-chroma faction accents, never the
   team hues at full chroma.
3. **Contrast:** text and icons meet **WCAG 2.2 AA (4.5:1 for text, 3:1 for icons and bars)**
   against their own backing plate, never against the live 3D scene.
4. **Colour-blind safe:** every state colour has a second cue (shape, icon, pattern or
   position); the ColorBlind palette stays a first-class option.
5. **Numbers first:** health, damage and timers use tabular (monospaced) figures. Big numbers
   abbreviate consistently (1.2k), with exact values on hover.
6. **Scaling:** the HUD scales 75–150 % and stays sharp (MSDF); minimum text size 12 px at
   1080p and 100 %.
7. **Licences:** fonts under **SIL OFL** (as today) or another open licence the maintainer
   approves; icons and UI art made by the team go in the repo as **CC0 or CC-BY** (or under a
   written assignment). The logo stays all-rights-reserved ([brand-and-key-art.md](brand-and-key-art.md)).
8. **Localisation:** fonts need Latin Extended at least; plan the glyph coverage before
   committing to a display face.

## Faction flavour in the UI

The two factions can flavour the UI frame without touching the team layer:
- the **player's own side's faction** can skin the frame (Forge: gold-bronze, machined seams,
  stepped corners; Void: thin silver lines, floating panels with gaps). Physical frames only:
  the art direction rules out holographic interfaces in the world, so the HUD never imitates
  one either;
- or the UI stays **faction-neutral** (a Builder-relic style) and only the loading screen and
  victory screen are faction-skinned.

## Open questions for the designer

> **Decision needed (designer): faction-skinned HUD or a neutral HUD?** Agents' lean: neutral
> HUD (one design to maintain, no confusion for players switching sides), with
> faction-skinned loading, victory and defeat screens.

> **Decision needed (designer): the display face.** Keep Open Sans Condensed for body and
> numbers, and choose an OFL display face for titles in the science-fantasy register
> (candidates to evaluate: Cinzel, Marcellus, Cormorant (serif, "ancient"), or Rajdhani,
> Saira, Chakra Petch ("relic tech")). Check each licence on the font's own repository.

> **Decision needed (designer): icon style.** Ability icons have to read at 48–64 px: painted
> illustrative vs flat glyph. The pipeline (roadmap §9.3) can produce painted drafts; a
> glyph style is easier to keep consistent.

> **Decision needed (designer + maintainer): keep green/blue/red?** See palette §1.

## Checks this becomes

| Rule | What | Proposed |
|---|---|---|
| **UI-1** | Text contrast against its plate | ≥ 4.5:1 (WCAG AA) |
| **UI-2** | Icon and bar contrast against its plate | ≥ 3:1 |
| **UI-3** | Font licences | OFL (or approved), listed in `THIRD_PARTY.md`; checked by the licence gate (A0) |
| **UI-4** | Minimum text size | 12 px at 1080p, 100 % scale |
