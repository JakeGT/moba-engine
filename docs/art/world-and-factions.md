# World and factions

**Status:** v0 draft, 2026-10-31. **Lead:** maintainer. Agents drafted this from the
maintainer's decisions of 2026-10-30. Names and lore are working placeholders unless a line
says *Decided*.

## Decided

| Topic | Decision (maintainer, 2026-10-30) |
|---|---|
| Fidelity | Full photoreal (roadmap §1). Fidelity is not a setting: nothing here is cyberpunk. |
| Setting | **Science fantasy**: ancient super-technology and magic coexist, and most people can't tell them apart. |
| Tone | **Epic, weathered.** Heroic scale, with wear, dust, repair and history on every surface. Not grimdark: the playable space stays mid-value so units read at 40 px. |
| Sides | **Two opposed factions**, each with its own architecture and palette. |
| The pair | **The Solar Forge vs the Void.** |
| Team identity | **Factions by shape and material; teams by rim.** Ally/enemy is told only by rims, outlines, health bars and the HUD, and stays player-relative and colour-blind safe ([palette-and-value.md](palette-and-value.md)). |
| Map 1 | **Ember Crossing.** |

## Premise (placeholder lore)

> Long ago an older civilisation, the **Builders** (placeholder), hung engines in the sky to
> hold the world in a stable orbit around a failing sun. When the Builders vanished, their
> machines kept working, badly. One of them fell. Where it struck, the ground glassed and
> cracked into a basin of basalt terraces, and fragments of its core — sunlight made solid —
> still burn in the rock. People call the scar **Ember Crossing**.
>
> Two powers grew out of the Builders' ruins and both claim the Crossing.
>
> **The Solar Forge** reads the Builders' engines as scripture: the sun must be fed and the
> world must work. It is an empire of foundries, guild-priests and brass automata that mines
> the ember shards to keep its furnaces — and, it believes, the sun — alight.
>
> **The Void** learned the other half of the Builders' craft: the dark engines that bent
> gravity and kept the sky in its place. It is an old, quiet order of astronomers and
> gravity-binders who think the dying sun is a cage, and that the shards the Forge burns
> belong to the night.

Why this premise works for the game:
- It explains **why everything is old and repaired**: weathered tone without grimdark.
- **Science and magic share one source** (Builder technology), so VFX can be physical light,
  heat and gravity rather than arbitrary sparkle. That suits photoreal lighting.
- **Ember Crossing becomes a real place**: the crash basin explains the terraced basalt, the
  scorched ground, the ember crystals and the low warm light.
- The factions are **light vs gravity**, not good vs evil. Neither is the villain.

> **Decision needed (maintainer): the world's name and the faction names.** "Solar Forge"
> and "Void" are working names. Options: (a) keep them as is; (b) keep them as internal
> names and give each an in-world name later (e.g. *the Kiln Dominion* / *the Still Choir*);
> (c) rename now. **Recommendation: (b).** The working names are clear for the team, and
> the brand pass (marketing) picks the public ones.

> **Decision needed (maintainer): the Builders.** Should the Builders ever appear (ruins,
> relic machines, a neutral boss) or stay off-screen? **Recommendation:** ruins and relic
> machines only, no Builder figures, so the mystery stays and no third character style is
> needed in v0.

## How the factions differ (identity in one line each)

| | **Solar Forge** | **Void** |
|---|---|---|
| Idea | Light, heat, industry, faith in work | Gravity, silence, the night sky, patience |
| Shape | **Mass and stacking**: wide bases, stepped profiles, radial fans | **Suspension and gaps**: floating segments, arcs, rings, hollow cores |
| Materials | Brass, bronze, forged iron, kiln ceramic, sandstone, amber sun-glass, slag | Black ceramic, obsidian, pewter and tarnished silver, levitating basalt, cool void-glass, bone-white inlay |
| Value | Mid to light, warm highlights | Mid to dark bodies with **strong cool speculars** and bone-white accents, never a black blob |
| Light motif | Light comes **out of** things: glowing seams, sun-glass cores, radiant halos | Light **bends round** things: lensing, dark cores with bright rims, starfield glints |
| Motion | Heavy, grounded, piston rhythms, sparks falling | Weightless, slow orbits, debris drifting **upwards** |
| Sound (for audio) | Bellows, hammering, hissing steam | Low hum, glass chimes, pressure drops |

## Shape language

At the gameplay camera a hero is about 40 px tall (1440p). Faction identity has to survive in
a **flat, single-value silhouette** at that size, and it must never compete with the team rim.
These rules are written so the review renders can check them ([silhouettes-and-scale.md](silhouettes-and-scale.md)).

### Solar Forge: mass and stacking

- **Base-heavy.** The widest part of a Forge silhouette sits in its lower half (stance,
  greaves, a plinth, a tower foot).
- **Stepped profiles**: ziggurat steps, stacked drums, banded columns.
- **Radial fans**: sunburst crests, halo discs and fin crowns. A Forge hero carries **one**
  radial element (crest, back-halo or shield boss). It's the faction's 40 px tell.
- **Straight lines meet at hard angles**; curves only as segments of a circle (sun discs).
- **Upward triangles** and chimneys on architecture.
- Surfaces are **plated and riveted**; seams glow on special units only.

### Void: suspension and gaps

- **Top-heavy or floating.** Mass sits high or floats free: hovering pauldrons, a crown of
  stones, a tower whose upper blocks hang above its base.
- **Constant gaps.** Pieces are separated by clear, regular air gaps that read as negative
  space in the silhouette. It's the faction's 40 px tell, alongside orbiting fragments.
- **Arcs, crescents and rings**, plus long slender verticals.
- **Smooth monolithic slabs**, polished, with fine engraved star-map lines (zoom camera only).
- Debris and cloth drift **upwards** around Void units (a VFX and animation rule, not geometry).

### Shared rules

- **No faction may use team colour as identity.** See the hue/chroma rules in
  [palette-and-value.md](palette-and-value.md).
- Hero silhouettes must still differ **within** a faction (role first: see the hero
  concepts in [heroes-and-minions-v0.md](heroes-and-minions-v0.md)).
- Faction tells are **secondary**: role and threat must read before faction.

## Architecture

### Solar Forge

- **Massing:** stepped sandstone and basalt platforms; foundry halls with **sawtooth roofs**
  and **sun-funnel chimneys**; bronze-banded drums; colonnades of squat columns.
- **Details:** brass sun discs; kiln-glazed tile friezes (mid-saturation ochre, cream,
  terracotta); chain hoists; slag heaps; heat-stained stone around vents.
- **Light:** warm interior glow through grilles; sun-glass lenses on tower crowns that focus
  light into beams (the tower attack).
- **Wear:** soot streaks up from vents, sand drifts against the windward side, edge wear on
  every step, repaired plates of a different alloy.

### Void

- **Massing:** dark monoliths and **ring gates**; terraces whose top courses float a metre
  above the courses below; observatory domes open to the sky.
- **Details:** star-chart inlays in pale stone; pewter astrolabe rings; obsidian mirrors;
  suspended basalt fragments locked in slow orbits.
- **Light:** almost none emitted. Thin cool rims along floating edges, and occasional
  void-glass cores (violet-white, low chroma).
- **Wear:** pitted, sand-scoured polish; cracks lit faintly from inside; dust collecting
  **under** floating stones in a circle (gravity has moved it).

### Neutral (the Crossing itself)

- Builder wreckage: huge curved ribs of a fallen engine, half buried, fused with glassed rock.
- Ember shards growing out of fractures in the basalt.
- Nomad camps and shrines of the Crossing's own people (the **Ashen**, placeholder): cloth,
  bone, scavenged brass and obsidian alike. This is a third, small visual vocabulary for
  neutral heroes, camps and monsters.

## Ember Crossing: who stands where

Today's map puts the blue spawn in the west (`[32, 128]`) and the red spawn in the east
(`[224, 128]`), and the low sun comes from the west (`sun_dir = [-0.6, 0.35, 0.45]`).

> **Decision needed (maintainer): which faction holds which base.**
> - (a) **Forge west, Void east (recommended).** The Forge sits with the low sun behind it;
>   the Void holds the side where the long shadows fall, which suits it. Good key-art framing:
>   sunlit brass against dark floating stone across the crossing.
> - (b) Void west, Forge east: the Forge faces into the sunset, with a strong rim on its
>   structures but harsher contrast on the playable east side.
> - (c) Mirror the art per player, so each player sees their own base as one fixed faction.
>   That is costly and breaks spectator consistency. Not recommended.
>
> Either way the sim's Blue and Red stay as they are; this is presentation only.

> **Decision needed (maintainer): are heroes bound to a faction?**
> - (a) **Heroes are free agents (recommended):** any hero can play on either side. Heroes
>   carry their own faction look (Forge, Void or Ashen) and the bases carry the side's. That
>   is standard for the genre, keeps picks open, and the rim and HUD carry team identity anyway.
> - (b) Faction-locked rosters: stronger identity, but the pick pool halves and every hero
>   needs two skins to mirror.

## What v0 must enable

Roadmap §9.6: v0 is enough to build **one map section, one hero and the minions** for the
vertical slice (A3). For world and factions that means:
- one Forge base section (tower, base wall segment and the core) and its Void opposite, at
  least as blockouts;
- the slice hero (picked from the three concepts in
  [heroes-and-minions-v0.md](heroes-and-minions-v0.md));
- both minion families;
- the neutral Crossing set dressing: basalt terraces, ember shards and one Builder rib.

## Interpretations for review

Each faction has three readings of the same identity. All of them keep the shape rules above;
they differ in culture, era and material emphasis. The review app shows each one as an
establishing shot, a tower, a common soldier and a material close-up.

| | Reading | What it emphasises | Risk |
|---|---|---|---|
| Forge A | Foundry Theocracy | Temple-foundries: stepped sandstone with brass banding, sun-disc emblems, sawtooth roofs and sun-funnel chimneys, kiln-glazed tile friezes, light through iron grilles | Busy ornament; needs restraint at 46 m |
| **Forge B** | **Sun-Kiln Empire** (recommended after round 1) | An ancient ziggurat civilisation in terracotta and glazed ceramic, bronze sun discs, squat colonnades | Terracotta sits near the enemy hue zone (PAL-3) |
| Forge C | Iron Dawn | Heavy industry built into basalt: forged iron, slag heaps, chain hoists, sun-glass lenses | Darkest and least "solar"; can blur into the Void's value range |
| Void A | Observatory Order | Astronomers: black ceramic monoliths, pewter astrolabe rings, open domes, bone-white star-chart inlay | Static; motion has to come from VFX |
| **Void B** | **Gravity Monastery** (recommended) | Basalt terraces whose upper courses float, ring gates, dust drifting upwards | Floating masses cost RT/BLAS refits if they move (static is fine) |
| Void C | Obsidian Court | Polished obsidian and black lacquer, silver fittings, plum banners, bone-white masks | Highly reflective black: VAL-1 and the "never a black blob" rule |

**What the first concept round showed** (96 images, review app *Factions*;
[boards/factions.jpg](boards/factions.jpg)):

- **Forge A came out as colourful, fairy-tale castles**: multicoloured tile, spires and gilt.
  It's charming but not "epic, weathered", and too busy for 46 m.
- **Forge B is the strongest Forge reading**: monumental stepped temples, colonnades,
  weathered terracotta and sandstone, with bronze sun discs (their patina reads teal, which sits
  in the free 167–211° band).
- **Forge C drifts into generic steampunk** (brass pipes and boilers). Its machinery still
  works in small doses on towers and minions.
- **Void A's monoliths** read clearly as "the night sky" (star inlay on black stone), but the
  soldier prompt never produced a person.
- **Void B is the strongest Void reading**: vast floating terraces, ring gates and violet light
  in the gaps. It is the most original of all six.
- **Void C turned the bone-white masks into skulls everywhere.** That is a death/necromancer
  cliché that clashes with the patient-astronomer premise: **avoid it**.

**Recommendation (revised after the first round):** **Forge B** (Sun-Kiln) for architecture,
with Forge A's sun-disc emblems and glazed friezes **used sparingly** and Forge C's machinery
on towers and minions. **Void B** (Gravity Monastery), with Void A's star-chart inlay and
astrolabe rings as the ornament. Round 2 in the review app explored "A + C" and "B + A" mixes
before this revision ([boards/faction-mixes.jpg](boards/faction-mixes.jpg)). It showed two things:
- **Literal star motifs turn the Void into sci-fi.** The B + A mix produced pods with painted
  stars and porthole rings, closer to a space station than a monastery. Keep Void A's inlay
  as fine engraving, visible only at the zoom camera.
- **The 46 m base mock-ups are far too busy for a play surface** in both factions: gilt,
  domes and machinery everywhere. Ornament belongs on structures' upper thirds and the
  out-of-bounds skyline, not on the walkable base floor (VAL-2/VAL-4).

> **Decision needed (maintainer): faction readings.** Pick one reading per faction, or a mix
> (recommended: Forge B with A's emblems and C's machinery; Void B with A's ornament).

## Concept boards

Generated concept boards for each faction (several interpretations each) are in the review
app (see [README.md](README.md#review-app)). A few curated ones are committed under
[boards/](boards/).
