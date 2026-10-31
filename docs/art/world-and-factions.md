# World and factions

**Status:** v0 draft, 2026-10-31. **Lead:** maintainer. Agents drafted this from the
maintainer's decisions of 2026-10-30. Names and lore are working placeholders unless a line
says *Decided*.

## Decided

| Topic | Decision (maintainer, 2026-10-30) |
|---|---|
| Fidelity | Full photoreal (roadmap §1). Fidelity is not a setting: nothing here is cyberpunk. |
| Setting | **Science fantasy**: ancient super-technology and magic coexist, and most people can't tell them apart. |
| Technology (2026-10-31) | **Technology so advanced it looks like magic.** Weapons, armour, architecture, units and the Builders' relics are all technology, and people treat them as sacred. See [the visual rule](#technology-that-looks-like-magic). |
| Tone | **Epic, weathered.** Heroic scale, with wear, dust, repair and history on every surface. Not grimdark: the playable space stays mid-value so units read at 40 px. |
| Sides | **Two opposed factions**, each with its own architecture and palette. |
| The pair | **The Solar Forge vs the Void.** |
| Team identity | **Factions by shape and material; teams by rim.** Ally/enemy is told only by rims, outlines, health bars and the HUD, and stays player-relative and colour-blind safe ([palette-and-value.md](palette-and-value.md)). |
| Map 1 | **Ember Crossing.** |

## Technology that looks like magic

The maintainer's direction (2026-10-31): science fantasy in the sense that technology this
advanced is indistinguishable from magic. It applies to weapons and armour, architecture, the
Builders' relics and the Ember crystals, and units and machines.

- **From a distance it reads as magic.** Things float silently and effortlessly, light appears
  without flame, power moves without wires. No jets, thrusters, exhaust, engines, pistons or
  vents in view.
- **Up close it is technology.** Precise machined seams, emitter rings, fine lattices, lenses
  and hard-light projections are the visible source of every "magical" effect.
- **Ancient and ceremonial forms.** Seamless advanced materials (ceramic composites,
  precision-machined alloys, light-conducting crystal) shaped as temples, regalia and
  ceremonial weapons, worn by millennia of use: dust, patina, scorching, chipped edges.
- **Never:** screens or holographic user interfaces, cables or hazard stripes, spaceships,
  modern guns, neon, bulky power armour.
- **Solar Forge = solar and fusion technology.** Sun discs are collector mirrors and fusion
  lenses; furnaces are contained fusion cores; light is near-white with a pale gold edge.
- **Void = gravity technology.** Levitation, gravity wells and lensing, with a faint shimmer of
  distorted air around floating parts and violet-white light only in seams and gaps.
- Soldiers' faces are hidden by helms or masks.

First previews (Gemini round G3, 2026-10-31) showed the two ways this goes wrong: bulky power
armour reads as conventional sci-fi, and levitation shown with jets of dust reads as rocket
thrusters. Both are fixed by the "from a distance" rule. Prompts: the review app's
`prompts/build_gemini3.py` (factions) and `prompts/build_gemini4.py` (everything else).

What the full rounds added (G3: 72 faction images on Nano Banana Pro; G4: 143 images on Nano
Banana 2.1 for the rest of the bible):

- **Collector discs turn into satellite dishes.** A sun disc must be drawn as a **flat disc
  with radial fins** set into the architecture, never a parabolic dish on a mount.
- **Generators add flying craft to skies** (saucer shapes in Forge B and Void C), even when
  told not to. Skies stay empty: anything that flies is a floating architectural piece.
- **Close-fitting armour reads as a sci-fi bodysuit.** Soldiers need ceremonial layering
  (tabards, wraps, mantles, stacked plates) so the silhouette says "order" before "suit".
- **Violet seam lines drift towards neon.** Void light stays in thin, sparse seams and gaps;
  never a continuous outline traced around the body.
- **"Ribs" draws a skeleton.** Asked for the fallen engine's curved ribs, the map concepts
  drew a whale's skeleton in bone. Builder wreckage is **machined hull arches** of pale alloy
  with panel seams and dead emitter rings: manufactured, never organic. Redrawn that way
  (round G5, [boards/g4-ember.jpg](boards/g4-ember.jpg)), the wreck reads as a row of
  half-buried hull rings: clearly a machine, and a strong landmark from the gameplay camera.
- **Glowing discs inside rings of fins read as jet turbines** (the Lens Gunner, the Crucible
  Walker). A Forge lens sits in a stepped collar, not a finned cowling.
- **Fusion described as "contained energy released"** (a near-white flash, a pale gold
  shockwave) finally stopped generators painting Forge heat as orange fire.

### What changes from the fantasy-era drafts

| Was (rounds 1–3, local) | Now |
|---|---|
| Brass, bronze and forged iron | **Sun-alloy**: aged, dull gold-bronze alloy with precise machined seams (brass stays the measured stand-in for its PBR numbers) |
| Kiln ceramic and terracotta tiles | **Heat-shield ceramic**: crazed cream and ochre tiles on stepped forms |
| Furnaces, chimneys, bellows, steam, slag | **Contained fusion cores** behind lattice grilles; heat-radiator fins and spires; no smoke, steam or exhaust |
| Sun discs as emblems | Sun discs as **solar collector arrays and fusion lenses** (flat, finned) |
| Brass automata with furnace bellies | Bronze-alloy **automata with fusion cores**, moving without visible pistons |
| Void ceramic and obsidian | **Void composite** (matte black-violet, engraved) and obsidian-like metamaterial |
| Pewter astrolabe rings | **Gravity rings**: hovering pewter-like alloy and tarnished silver |
| Ashen cloth and scavenged brass | Ashen cloth over **salvaged Builder panels**; scavenged devices that still work, worn as charms |

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
> world must work. It is an empire of guild-priests who keep the Builders' solar collectors
> and fusion furnaces running without understanding them, served by bronze-alloy automata.
> It mines the ember shards to keep its furnaces — and, it believes, the sun — alight.
>
> **The Void** learned the other half of the Builders' craft: the dark engines that bent
> gravity and kept the sky in its place. It is an old, quiet order of astronomers and
> gravity-binders who think the dying sun is a cage, and that the shards the Forge burns
> belong to the night.

Why this premise works for the game:
- It explains **why everything is old and repaired**: weathered tone without grimdark.
- It explains **why technology looks like magic**: both factions inherited machines they
  operate by ritual, not understanding. The Builders' relics are as far beyond them as they
  are beyond us.
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
| Technology | Solar and fusion: collector arrays, fusion lenses, contained cores behind lattices | Gravity: levitation, gravity wells, lensing rings |
| Materials | Sun-alloy (dull gold-bronze), heat-shield ceramic, sandstone and basalt masonry, fusion lenses (sun-glass), emitter lattices | Void composite (black-violet), obsidian-like metamaterial, gravity-ring alloy (pewter-like and tarnished silver), levitating basalt, void-glass, bone-white inlay |
| Value | Mid to light, warm highlights | Mid to dark bodies with **strong cool speculars** and bone-white accents, never a black blob |
| Light motif | Light comes **out of** things: glowing seams, sun-glass cores, radiant halos | Light **bends round** things: lensing, dark cores with bright rims, starfield glints |
| Motion | Heavy, grounded: planted strikes, power building in seams, molten droplets falling | Weightless, slow orbits, debris drifting **upwards** |
| Sound (for audio) | A deep resonant hum of contained fusion, heavy strikes, metal ticking as it cools | Low hum, glass chimes, pressure drops |

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
- **Upward triangles**, heat-radiator fins and spires on architecture (never chimneys).
- Surfaces are **plated, with precise machined seams**; seams glow on special units only.

### Void: suspension and gaps

- **Top-heavy or floating.** Mass sits high or floats free: hovering pauldrons, a crown of
  stones, a tower whose upper blocks hang above its base.
- **Constant gaps.** Pieces are separated by clear, regular air gaps that read as negative
  space in the silhouette. It's the faction's 40 px tell, alongside orbiting fragments.
- **Arcs, crescents and rings**, plus long slender verticals.
- **Smooth monolithic slabs** of void composite, with fine engraved star-map lines (zoom camera
  only) and hovering gravity rings.
- Debris and cloth drift **upwards** around Void units (a VFX and animation rule, not geometry).

### Shared rules

- **No faction may use team colour as identity.** See the hue/chroma rules in
  [palette-and-value.md](palette-and-value.md).
- Hero silhouettes must still differ **within** a faction (role first: see the hero
  concepts in [heroes-and-minions-v0.md](heroes-and-minions-v0.md)).
- Faction tells are **secondary**: role and threat must read before faction.

## Architecture

### Solar Forge

- **Massing:** stepped heat-shield ceramic and basalt platforms (ziggurats that are solar
  megastructures); squat colonnades that are radiator fins; banded drum-shaped fusion cores;
  heat-radiator spires.
- **Details:** flat, finned sun-disc collectors (never dishes), one per building at most;
  crazed cream and ochre heat-shield tiles; gold-bronze banding with machined seams; fine
  lattice grilles; heat-stained stone around radiator fins.
- **Light:** white-gold light through lattice grilles; fusion lenses on tower crowns that fire
  the tower's attack as a beam.
- **Wear:** heat-scorching above radiator fins, sand drifts against the windward side, edge
  wear on every step, tiles replaced with a different glaze.

### Void

- **Massing:** dark monoliths and **ring gates**; terraces whose top courses float a metre
  above the courses below; observatory domes open to the sky.
- **Details:** star-chart lines engraved in black composite; hovering gravity rings of
  pewter-like alloy and tarnished silver; ring gates that lens the view through them;
  suspended basalt fragments locked in slow orbits.
- **Light:** almost none emitted. Thin violet-white light only in the seams and gaps of
  floating parts, and occasional void-glass cores (low chroma). Never a traced neon outline.
- **Wear:** pitted, sand-scoured polish; cracks lit faintly from inside; dust collecting
  **under** floating stones in a circle (gravity has moved it).

### Neutral (the Crossing itself)

- Builder wreckage: huge curved ribs of a fallen sky-engine in seamless pale grey **Builder
  alloy**, with dead emitter rings, half buried and fused with glassed rock.
- Ember shards growing out of fractures in the basalt: smoky crystal with a white-gold glow
  deep inside.
- Nomad camps and shrines of the Crossing's own people (the **Ashen**, placeholder): layered
  charcoal and ash-grey cloth over salvaged Builder panels, small lights hovering above the
  camp, and scavenged Forge alloy and Void composite worn as charms. Their devices still work
  and look like talismans. This is a third, small visual vocabulary for neutral heroes, camps
  and monsters.
- **Builder machines** are the only Builder presence: ember scarabs (small maintenance
  machines) and a dormant Builder warden, all seamless pale alloy with hovering plates.

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

Each faction has three readings of the same identity, plus the mixes the earlier rounds
suggested. All of them keep the shape rules and the [technology rule](#technology-that-looks-like-magic);
they differ in culture, era and emphasis. The review app shows each one, drawn as advanced
technology (round G3), as an establishing shot, a tower, a soldier and a material close-up.

| | Reading | What it emphasises | Risk |
|---|---|---|---|
| Forge A | Foundry Theocracy | Temple-foundries of heat-shield ceramic and gold-bronze around contained fusion furnaces; sun-disc collector emblems; white-gold light through lattices | Busy ornament; collector emblems drawn as dishes |
| Forge B | Sun-Kiln Empire | Ziggurats that are solar megastructures: crazed cream and ochre heat-shield terraces, colossal collector mirrors, radiator colonnades | Mirrors dominate and read as satellite dishes |
| Forge C | Iron Dawn | Colossal fusion engines and kinetic presses in basalt cliffs; banded drum reactors; lens towers | Industrial: beams and heat haze everywhere; darkest Forge |
| **Forge rec** | **B + A emblems + C machinery** (recommended) | Sun-Kiln ziggurats, at most one collector disc per building, Iron Dawn machinery on towers and gates | Needs the "flat finned disc" rule |
| Void A | Observatory Order | Black composite monoliths that are gravity sensors, hovering rings round open domes, engraved star maps | Static; literal stars creep in |
| Void B | Gravity Monastery | Terraces whose upper courses float with clear gaps, orbiting blocks, lensing ring gates, dust drifting up | Floating masses cost RT/BLAS refits if they move (static is fine) |
| Void C | Obsidian Court | Mirror-smooth black metamaterial, tarnished silver, plum banners, floating crescents, bone-white masks | Highly reflective black (VAL-1); reads as a villain's court |
| **Void rec** | **B + A ornament** (recommended) | The Gravity Monastery with the Observatory's hovering rings and fine engraved star maps | Violet seams drifting towards neon |

**What round G3 showed** (72 images on Nano Banana Pro, review app *Factions*):

- **Forge rec is the strongest Forge reading**: stepped cream ceramic ziggurats with drum
  machinery, and lens towers that fire beams read at once as "a tower that shoots light".
  Its collector discs, like Forge A's and B's, came out as dishes: see the rule above.
- **Forge C** makes the best towers (a lens on a column) but its bases are a haze of beams and
  heat: take its towers, not its massing.
- **Void B and Void rec** are the most original readings of the whole set: tower segments
  hanging above each other with violet rings in the gaps survive as a silhouette at any size.
- **Void A** reads as an observatory, but its details went back to literal painted stars.
- **Void C** produced saucer craft and a villain's-court look. Not recommended.
- **Soldiers** in every reading came out as close-fitting bodysuits: the layering rule above.

**Earlier rounds (fantasy-era look, kept for the record).** What the first concept round
showed (96 images, review app *Factions*;
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
- **Literal star motifs turn the Void into a space station.** The B + A mix produced pods
  with painted stars and porthole rings. Keep Void A's inlay as fine engraving, visible only
  at the zoom camera. (This is about clichés, not technology: the Void *is* advanced
  technology; see [above](#technology-that-looks-like-magic).)
- **The 46 m base mock-ups are far too busy for a play surface** in both factions: gilt,
  domes and machinery everywhere. Ornament belongs on structures' upper thirds and the
  out-of-bounds skyline, not on the walkable base floor (VAL-2/VAL-4).

> **Decision needed (maintainer): faction readings.** Pick one reading per faction, or a mix.
> **Recommended (unchanged by the advanced-tech rounds): Forge rec** (B with A's emblems and
> C's machinery) and **Void rec** (B with A's ornament). Rounds G4's structures and minions
> were drawn with these two as style references.

## Concept boards

Generated concept boards for each faction (several interpretations each) are in the review
app (see [README.md](README.md#review-app)). A few curated ones are committed under
[boards/](boards/).
