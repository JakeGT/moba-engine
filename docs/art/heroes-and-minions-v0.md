# Heroes, minions and structures (v0)

**Status:** v0 draft, 2026-10-31. **Lead:** maintainer. **Decided:** the maintainer picks the
vertical-slice hero from three concepts covering both factions and different roles. Every
concept here maps onto an **existing kit**, so the slice needs new art, not new gameplay.

Names are working names. Concept art for every option is in the review app (several
variants per concept, plus 30 px and 40 px silhouettes); the best are in [boards/](boards/).

## The three slice-hero concepts

| | **1. The Kiln Warden** | **2. The Still Oracle** | **3. The Ash Strider** |
|---|---|---|---|
| Faction | Solar Forge | Void | Ashen (neutral nomads) |
| Role, size class | Melee bruiser, **Hero M/L** (2.0 m) | Caster, **Hero M** (1.9 m, floats 0.2 m) | Marksman, **Hero S** (1.75 m) |
| Existing kit | **Blade** (760 HP, 2 m range) | **Mage** (500 HP, 8 m range) | **Ranger** (540 HP, 9 m range) |
| One line | A foundry guild-knight who carries the forge's fire on his back | A blind astronomer who folds space around her hands | A Crossing-born hunter who scavenges from both empires |
| 40 px tell (top third) | **Sun-disc back-halo** (≈ 1 m, radial fins) above stacked brass pauldrons; hammer head over the shoulder | **Crescent crown** of floating obsidian; **three slabs orbiting** the shoulders, with clear gaps | **Long bow arc** above the head; hood and a scarf streaming sideways |
| Silhouette | Base-heavy, wide stance, blocky | Tall, narrow, top-heavy, broken by gaps | Lean, asymmetric, a diagonal line of bow and scarf |
| Materials | Sun-brass, forge iron, kiln ceramic chest plate, leather, soot; one thin white-gold seam glow | Void ceramic, obsidian, pewter rings, plum lacquered felt; **bone-white mask** with star-chart lines | Sand linen, bone, a scavenged brass vambrace, obsidian arrowheads, a bow of Builder alloy |
| Value read | Mid-light warm metal on mid ground | Dark body with **bone mask and silver rings** (≥ 10 % of area: see palette §5) | Mid-light cloth: **risk on sand** (see below) |
| Motion | Heavy: hammer arcs, planted feet, sparks falling | Weightless: hovering idle, slow orbits, robe strips drifting **up** | Quick: low crouch, cloth and scarf lag |
| VFX (see [vfx-language.md](vfx-language.md)) | White-gold heat: molten arcs, spark showers, scorch decals | Lensing, implosions, dark cores with violet-white rims | Ash and wind, obsidian glints, dust bursts |
| What the slice tests best | **Animation feel and impact timing** (roadmap §9.4, human checkpoint 3); hard-surface materials | **VFX language**, distortion and RT lighting of dark polished materials | **Cloth**, the neutral third vocabulary, readability on sand |
| AI-pipeline fit (roadmap §9.3) | **Good:** hard-surface armour decimates and bakes well | Medium: floating parts are separate meshes (easy); the robe needs hand topology | Hard: layered cloth needs hand topology and sim |
| Main risk | Looks generic "paladin" unless the halo and kiln ceramics stay prominent | Dark blob on basalt without the bone/silver rule | Blends into sand and ash (CON-1): needs a darker cloak or a rim boost |

**Recommendation: the Kiln Warden for the slice.** It is the cheapest to produce well
through the pipeline. It tests the thing only a human can sign off (animation feel and
impact), it suits the Golden Crossing mood and the Forge's west base, and its silhouette is
the most robust at 30 px. The Still Oracle is the strongest identity and the best
second hero for A4.

> **Decision needed (maintainer): the slice hero** — Warden (recommended), Oracle or Strider.
> Pick in the review app; comments there feed v1.

### First read of the generated concepts (round 1)

24 full-body concepts (8 seeds per hero) were cut out as flat silhouettes at 30 px and 40 px
(the review app's *Slice heroes* section; [boards/hero-silhouettes.jpg](boards/hero-silhouettes.jpg)):

- **Kiln Warden:** the sun-disc halo survives at 30 px as a spiky crown on a blocky,
  base-heavy body. **The tell works.** Risk confirmed: without the halo it reads as a generic
  armoured knight.
- **Still Oracle:** the crescent crown and the orbiting slabs read at 40 px, and the hover gap
  under the robe shows. But the slabs often read as **wings**, and the bone-white mask reads
  close to a **skull** in the portraits: both pull towards "angel" or "death" clichés. Take B
  (one astrolabe ring) tests a cleaner tell.
- **Ash Strider:** **fails SIL rules at 40 px.** The bow is too thin to survive and is lost
  in the cloak, leaving a generic hooded figure. The scarf is the only distinct shape. It
  needs a thicker, brighter bow limb, or take B's spear-thrower held as a diagonal.

Each concept also has a **take B** in the review app, so the pick isn't tied to one rendering:

| | Take B | What it changes |
|---|---|---|
| Kiln Warden | Lighter | A cream ceramic cuirass and a **sun-disc crest on the helm** instead of the back halo; the hammer held low. Tests whether the radial tell still reads at 30 px when it is smaller and higher |
| Still Oracle | Monastic | One pewter **astrolabe ring** behind the head instead of the crescent crown; two slabs at the waist. Fewer floating parts (cheaper to rig) |
| Ash Strider | Scavenger | A **spear-thrower and glass darts** instead of the bow; smoked goggles. A different long diagonal for the 40 px tell |

**What round 2 showed** (18 take-B images; [boards/hero-silhouettes.jpg](boards/hero-silhouettes.jpg)):
**take A wins for the Warden and the Oracle.**
- **Warden B** loses the base-heavy mass, and its crest shrinks to a bump at 40 px. It reads
  as a generic knight, and the model drifted to an axe.
- **Oracle B's** astrolabe ring merges with the head into one round blob, and the waist ring
  becomes a horizontal line. The crescent crown and the floating slabs (take A) give a far
  stronger tell, so keep A and solve the wing and skull reads in the model sheet: slabs
  angled down, a smooth mask without eye sockets.
- **Strider B** holds the spear upright, parallel to the body, which adds nothing at 40 px,
  and its pale cloak risks CON-1 on sand. **Neither Strider take has a working 40 px tell**,
  which strengthens the case for the Warden as the slice hero.

> **Decision needed (maintainer): does the slice hero keep its old name?** Blade, Mage and
> Ranger are kit names today. **Recommendation:** new display names; kit IDs unchanged.

### Concept detail: the Kiln Warden

- **Read order at 40 px:** halo disc → shoulder mass → hammer head → stance.
- **Halo:** a bronze sun disc with 9–12 radial fins, mounted on the backplate and tilted
  back 20° so the 54° camera sees it face-on. It glows **only** during the ultimate (the
  white-gold core heats up). At rest it's metal: emissive budget kept for gameplay meaning.
- **Hammer:** long haft, a block head with a kiln-ceramic insert. The head sits above the
  right shoulder at idle (SIL-4).
- **Armour:** stacked, banded brass plates (a stepped profile), riveted, with edge wear. A
  ceramic breastplate in cream and ochre glaze, crazed and chipped. Leather apron.
- **Face:** an open helm with a soot-dark face. It's for the zoom camera only: at 40 px the
  head is the top of the helm.
- **Showcase extras:** heat shimmer from the seams, falling ash; strand hair not needed
  (helmeted).

### Concept detail: the Still Oracle

- **Read order at 40 px:** crescent crown → orbiting slabs (the gaps) → narrow robe.
- **Crown:** five obsidian shards in a crescent, 15 cm above the head, not touching it.
- **Slabs:** three palm-sized basalt slabs on a slow orbit (period about 4 s) at shoulder
  height. They are the SIL-6 gaps, and they move to the casting hand on cast.
- **Mask:** bone-white, smooth, with engraved star-chart lines and no eye holes.
- **Robe:** plum felt, split into long strips that drift upwards (cloth sim on High+; baked
  wobble on Low).

### Concept detail: the Ash Strider

- **Read order at 40 px:** bow arc → hood and scarf diagonal → crouch.
- **Bow:** a recurve as tall as the hunter, made of a curved Builder rib, matte grey alloy
  with a bone grip.
- **Cloak:** layered linen, but **one tone darker than the Crossing's sand** (L ≈ 0.50) so
  CON-1 holds on the desert layer. A dark ash-grey scarf.
- **Kit:** obsidian-tipped arrows (Rooting Shot pins with an obsidian spike), dust trails.

## Minions

Rules: SIL-2 (≤ 0.8× the smallest hero, no capes, halos or orbiting parts); repeated by the
dozen, so they are VAT-animated and have a 1–2.5k triangle budget (roadmap §9.1); team
identity from rims only.

| | **Forge: the Foundry Thralls** | **Void: the Hollowed** |
|---|---|---|
| Idea | Bronze automata, wound and stoked by the foundries | Masked acolytes whose bodies are half held together by gravity |
| Melee | **Thrall:** squat bronze barrel-body, a furnace grille in the chest (small glow), a shield-arm and a stub blade. 1.4 m | **Hollow:** a narrow figure in a pewter mask, with shoulder plates floating 5 cm off the body (fixed gaps, no orbit). 1.4 m |
| Ranged | **Bellows Gunner:** a kiln-bellied mortar on short legs, lobbing slag. 1.3 m | **Shard Caster:** hovers 30 cm up, a cracked void-glass shard held overhead. 1.3 m |
| Siege | **Crucible Walker:** a four-legged walking crucible, pouring. 2.4 m, wide | **Monolith:** a floating basalt slab carried by two Hollows. 2.4 m |
| Silhouette tell | Domes and cylinders, broad bases | Verticals with fixed gaps, masks |
| Death | Collapses, steam vents, the furnace goes dark | Pieces fall as gravity lets go, a dust ring |

**First concept round** ([boards/units.jpg](boards/units.jpg)): the Thrall reads well as a
bronze automaton, though it renders **copper-orange**. Keep it in brass and bronze
(H ≈ 75–90°) to stay out of the enemy zone. The **Hollow lost its floating shoulder plates**
and came out as a plain silver knight, so the gaps must be drawn into the model sheet
explicitly. The Monolith siege (a slab carried by two acolytes) reads at once. The Crucible
Walker is the weakest; try a squatter, wider shape.

## Structures

| | **Forge** | **Void** |
|---|---|---|
| Tower (12–16 m) | A **stepped sandstone pylon** with brass banding and a **sun-glass lens crown** that focuses its attack into a beam. Basalt foot, soot streaks from vents | A **monolith in four blocks**, the top two hovering above the lower two; a void-glass core in the gap that lenses its attack |
| Core (18–24 m) | **The Sunheart Crucible:** a domed foundry drum, a ring of chimneys and a white-gold core visible through grilles | **The Still Well:** a ring of floating stones around a dark gravity well; dust spirals inward |
| Base walls | Sandstone ramparts with brass sun discs; the wall mesh follows today's `rampart` placement | Black ceramic slabs, each course floating slightly above the last |
| Destruction | Collapses outward, with fire and slag | Implodes: the blocks fall inward, then drop |

**First concept round:** the **Void tower** (stacked blocks, the top two floating over a
glowing core) and the **Void core** (a ring of floating stones around a gravity well) match
the shape language closely and are the strongest structures. The Forge tower (a lens-crowned
obelisk) and core (a domed foundry drum) are serviceable but less distinctive. Push the
stepped Sun-Kiln forms (Forge B).

> **Decision needed (maintainer): the neutral monsters.** The slice needs at least one camp.
> **Recommendation:** **ember scarabs**, glass-backed beetles that feed on the ember shards
> (small camp), plus a dormant **Builder warden machine** as the large objective (A4, not
> the slice). This keeps the Builders as machines only.

## Next steps after the pick

1. Turn the chosen concept into a **model sheet** (front, side, back and top-down at 54°),
   generated and then painted over.
2. Run it through the §9.4 pipeline: image → TRELLIS.2 → remesh and bake → review renders.
3. Readability sign-off at 46 m (the human checkpoint) against SIL and CON rules.
