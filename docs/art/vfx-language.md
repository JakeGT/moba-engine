# VFX language

**Status:** v0 draft, 2026-10-31. **Leads:** maintainer and graphic designer. Agents drafted
this, building on the R5 readability rules ([ADR-0013](../adr/0013-vfx-runtime.md)), which
carry over unchanged (roadmap §4.7).

## Two layers, two jobs

| Layer | Carries | Colour comes from |
|---|---|---|
| **Gameplay overlay** (after upscaling, roadmap §4.1): telegraphs, range indicators, selection rings, outlines | **Who** it threatens and **where** | **Team colour**, player-relative (self/ally/enemy) |
| **Effect body** (particles, meshes, distortion, VFX light): the spell itself | **What** it is and **whose faction** made it | **Faction element palette** (below), never team hues |

This split is the VFX version of "factions by shape, teams by rim". A Forge fireball cast by
an enemy is white-gold fire inside a **red** telegraph. Cast by an ally, it's the same fire
inside a **blue** one.

## Faction element languages

| | **Solar Forge** | **Void** | **Ashen** (neutral, placeholder) |
|---|---|---|---|
| Element | Light and heat | Gravity and absence | Wind, ash and bone |
| Core colour | White-gold `#FFF0C8` (C 0.05) | Violet-white `#D9CCFF` (C 0.07) | Pale ash `#D8D2C8` |
| Accent | Thin amber edges, ≤ 5 % of the effect's area | Violet edge `#9F86D9`, thin | Dust ochre, low chroma |
| Light | Emits real light (RT/clustered lights): warm, hard-edged | Emits little light; **dark cores with bright rims**; screen-space lensing (distortion pass) | No light; reads by value against the ground |
| Particles | Sparks falling with gravity, molten droplets, heat shimmer, black smoke | Debris and dust drifting **upwards**, slow orbits, starfield glints, implosion | Sand ribbons, ash flakes, bone shards |
| Motion | Fast start, heavy fall-off: hammer blows | Slow wind-up, sudden collapse: a held breath | Swirls and gusts |
| Impact | Flash, then sparks and a scorch decal | Inward pull, then a soundless pop and a crater ring | Dust burst |

> **Decision needed (maintainer + designer): ability-type coding.** The team colours take
> most of the hue wheel, so ability types can't be colour-coded the usual way. Options:
> (a) **code type by shape and motion (recommended):** damage = sharp and fast, crowd control
> = rings, chains and ground bindings, protection = domes and plates, mobility = streaks and
> trails, healing = rising motes. Colour stays faction-only. (b) A small secondary palette
> inside the free hue bands (teal for healing, violet for control). It conflicts with the
> Void's violet.

## Telegraphs (overlay layer)

R5 rules stay: enemy telegraphs are exempt from every cull and budget and keep at least
**35 % opacity** until the last 15 % of their life, and enemy projectiles never vanish.

| Kind | Style |
|---|---|
| Area, instant | Fill at **20–35 %** opacity with a crisp **2 px** edge at output resolution |
| Area, delayed | The fill grows from centre to edge over the delay; the edge is fully drawn from the start |
| Line skillshot | Two edge lines plus chevrons pointing in the travel direction; no fill |
| Persistent zone | Edge only, slow pulse (≤ 1 Hz), fill ≤ 15 % |
| Range indicator (own) | Thin circle, self colour, 40 % opacity |

| Rule | What | Proposed threshold |
|---|---|---|
| **TEL-1** | Telegraph contrast | CON-3 in [palette-and-value.md](palette-and-value.md), on every terrain layer and mood, every tier |
| **TEL-2** | No distortion, bloom or DLSS history on telegraphs | Overlay layer only (roadmap §4.1) |
| **TEL-3** | Effect bodies never cover an enemy telegraph's edge | Alpha particles over an enemy telegraph edge are capped at **50 %** |

## Budgets and rules for effect bodies

| Rule | What | Proposed threshold |
|---|---|---|
| **VFX-1** | Body colours obey PAL-4 and PAL-5 | Emissive C ≤ 0.06 inside team hue zones; ≤ 0.13 outside |
| **VFX-2** | VFX light (RT or clustered) doesn't blow the board | Ground lit by ability light stays L ≤ **0.85** (VAL-4) after **0.3 s** |
| **VFX-3** | Ally effects dimmed | R5 allied-VFX opacity setting; alpha ≤ **75 %** |
| **VFX-4** | Overdraw and particle budgets | R5 per-tier budgets (Low 1.5 … Ultra 8 screens) unchanged |
| **VFX-5** | Flashes | No full-screen flash longer than **0.15 s**; never more than **3 flashes a second** (WCAG 2.3.1) |
| **VFX-6** | Camera effects | No screen shake in ranked except own death and the core's destruction |
| **VFX-7** | Distortion (Void lensing, heat haze) | Never inside an enemy telegraph; strength capped so CON-1 holds through it |
| **VFX-8** | Motion vectors and reactive masks | Every effect writes them (roadmap §4.8) so DLSS/FSR don't ghost it |

## Status effects on units

Statuses show in the HUD (icons and colours in `crates/client/src/hud/theme.rs`). On the
unit they are **shape-coded** so they read without colour:

| Status | On-unit cue |
|---|---|
| Root | Ground ring with chains or binding glyphs at the feet |
| Slow | Heavy particles falling around the unit, plus a dragging trail |
| Silence | A broken halo or a rune slashed above the head |
| Shield | A thin plated shell (Forge) or orbiting slabs (Void), following the caster's faction |
| Burn | Embers rising from the body, with a char decal |
| Haste | Streaks behind, plus dust kicked up |
| Regeneration | Rising motes |

## Existing effects to rework

Today's `content/vfx/*.toml` effects (arc bolt, nova, cinder zone, ember curse, frost field,
judgement, …) were authored for the stylised look. In A3 the slice hero's effects are rebuilt
to this language first. The rest follow with A4. Two must change whichever mood is picked,
because they use saturated orange or red bodies that fall in the enemy hue zones:
`cinder-zone` and `ember-curse` (PAL-4).
