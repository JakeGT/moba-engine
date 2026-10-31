# Heroes, minions and structures (v0)

**Status:** v0 draft, 2026-10-31. **Lead:** maintainer. **Decided:** the maintainer picks the
vertical-slice hero from three concepts covering both factions and different roles. Every
concept here maps onto an **existing kit**, so the slice needs new art, not new gameplay.

Names are working names. Concept art for every option is in the review app (several
variants per concept, plus 30 px and 40 px silhouettes); the best are in [boards/](boards/).

**Redesigned 2026-10-31 for [technology that looks like magic](world-and-factions.md#technology-that-looks-like-magic).**
Each concept keeps its role, kit, size class and 40 px tell; what changed is what things are
made of and how they work. Round G4 (Nano Banana 2.1) drew the redesigns; round G5 (Nano
Banana Pro) made model sheets of the picks. The fantasy-era rounds 1–3 and G1 stay in the
review app under *Earlier rounds*.

## The three slice-hero concepts

| | **1. The Kiln Warden** | **2. The Still Oracle** | **3. The Ash Strider** |
|---|---|---|---|
| Faction | Solar Forge | Void | Ashen (neutral nomads) |
| Role, size class | Melee bruiser, **Hero M/L** (2.0 m) | Caster, **Hero M** (1.9 m, floats 0.2 m) | Marksman, **Hero S** (1.75 m) |
| Existing kit | **Blade** (760 HP, 2 m range) | **Mage** (500 HP, 8 m range) | **Ranger** (540 HP, 9 m range) |
| One line | A guild-knight who carries a sun's collector array on his back and a fusion core in his hammer | A blind astronomer who folds space around her hands with gravity rings | A Crossing-born hunter armed with Builder relics scavenged from both empires |
| 40 px tell (top third) | **Sun-disc collector halo** (≈ 1 m, radial fins) above stacked pauldrons; hammer head over the shoulder | **Crescent crown** of three hovering composite segments; **three slabs** hovering at the shoulders, angled down, with clear gaps | **Bone-white bow arc** held diagonally; hood and a scarf streaming sideways |
| Silhouette | Base-heavy, wide stance, blocky | Tall, narrow, top-heavy, broken by gaps | Lean, asymmetric, a diagonal line of bow and scarf |
| Materials | Sun-alloy plate, glazed heat-shield ceramic breastplate, an emitter lattice in the hammer head, leather; one thin white-gold seam glow | Void composite, gravity-ring alloy and tarnished silver wrist rings, violet-black cloth; **bone-white mask** with engraved star-map lines | Charcoal and ash-grey cloth, bone, a scavenged sun-alloy vambrace, Void composite plates, a bow of Builder alloy with a hard-light string |
| Value read | Mid-light warm metal on mid ground | Dark body with **bone mask and silver rings** (≥ 10 % of area: see palette §5) | Mid-light cloth: **risk on sand** (see below) |
| Motion | Heavy: hammer arcs, planted feet, sparks falling | Weightless: hovering idle, slow orbits, robe strips drifting **up** | Quick: low crouch, cloth and scarf lag |
| VFX (see [vfx-language.md](vfx-language.md)) | Contained fusion released: white-gold flash, pale gold shockwave, sparks, scorch decals | Lensing, implosions, dark cores with violet-white rims | Pressure waves of ash, hard-light arrows, dust bursts |
| What the slice tests best | **Animation feel and impact timing** (roadmap §9.4, human checkpoint 3); hard-surface materials | **VFX language**, distortion and RT lighting of dark polished materials | **Cloth**, the neutral third vocabulary, readability on sand |
| AI-pipeline fit (roadmap §9.3) | **Good:** hard-surface armour decimates and bakes well | Medium: floating parts are separate meshes (easy); the robe needs hand topology | Hard: layered cloth needs hand topology and sim |
| Main risk | Looks generic "paladin" unless the collector halo and ceramic stay prominent | Dark blob on basalt without the bone/silver rule; slabs read as wings | Blends into sand and ash (CON-1): the charcoal cloak is required |

**Recommendation: the Kiln Warden for the slice.** It is the cheapest to produce well
through the pipeline. It tests the thing only a human can sign off (animation feel and
impact), it suits the Golden Crossing mood and the Forge's west base, and its silhouette is
the most robust at 30 px. The Still Oracle is the strongest identity and the best
second hero for A4.

> **Decision needed (maintainer): the slice hero** — Warden (recommended), Oracle or Strider.
> Pick in the review app; comments there feed v1.

### Round G4: the advanced-tech redesigns

18 images (4 full-body studio views and 2 at the gameplay camera's angle per hero; review app
*Slice heroes*; [boards/g4-heroes.jpg](boards/g4-heroes.jpg)). All three are a clear step up
from the fantasy-era rounds, and **every 40 px tell now survives**:

- **Kiln Warden:** the strongest silhouette of the three. The collector halo is huge and reads
  at once from the gameplay angle, and the lattice hammer head shows its fusion core. Two
  drifts to correct in the model sheet: the radial fins came out as **spikes** (a sunburst,
  "sun god", rather than a collector array), and the **cream ceramic breastplate vanished**:
  the whole figure is gold-bronze, which costs value contrast.
- **Still Oracle:** the hovering crown, smooth mask (no skull read) and floating hem all work.
  In two of four views the shoulder slabs still **fan out like wings** (take g2 keeps them
  close), and the robe strips hang down instead of drifting up.
- **Ash Strider:** the round-3 fix holds: charcoal cloak, and a thick bone-white bow held
  diagonally that is the clearest 40 px tell of all three. The hard-light string reads as
  technology without any extra glow.

**Model sheets (round G5, Nano Banana Pro;** [boards/g5-sheets.jpg](boards/g5-sheets.jpg)): three
turnarounds, two front A-poses and up to two detail callouts each, sent g1 (Warden), g2
(Oracle) and g2 (Strider) as references, with the corrections above written into the prompt.
They are the modeller's starting point once the slice hero is picked. What they settled, and
what is still for the modeller to decide:

- **Warden:** the cream ceramic breastplate and tabard are back, and the design holds across
  all four views. The collector fins are flat blades in the front A-pose but **still spikes in
  the turnarounds**: model them as flat blades (front A-pose g1 is the reference).
- **Oracle:** the hovering crown and the wrist rings are consistent. Only turnaround **g3**
  keeps the shoulder slabs hanging close to the body; the others still spread two or three
  per side like wings. Use g3, one slab per side, and decide the count in the model.
- **Strider:** consistent in every view; the hard-light string and the scavenged plates read.
  The bow is diagonal only in front views; the idle pose must keep it diagonal (SIL-4).
- One detail callout each for the Oracle and the Strider failed (the service returned no
  image, as in round G1); one of each survived.

**Recommendation unchanged: the Kiln Warden for the slice.** The redesign makes the case
stronger: the most robust silhouette, hard-surface throughout (best for the AI-assisted
pipeline), and the clearest "technology that looks like magic" read (a sun on his back, a
contained star in his hammer).

### Earlier rounds (fantasy-era look)

These findings are about readability and still apply; the look they describe is superseded.

#### First read of the generated concepts (round 1)

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

#### Rounds 2 and 3

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

**Round 3** ([boards/round3-fixes.jpg](boards/round3-fixes.jpg)) tried fixes directly:
- **Strider C**, with a charcoal cloak and a large bone-white bow held diagonally, is the best
  Strider so far. The dark cloak solves the sand contrast, and the bow survives in about half
  the 40 px cut-outs. A thicker limb would make it reliable.
- **Oracle C** fixes the mask: smooth, with no skull read. But the slabs still float out
  sideways (the generator ignores "angled down"), and the crown drops out in several cut-outs.
  The slab placement is a model-sheet decision, not a prompt one.

> **Decision needed (maintainer): does the slice hero keep its old name?** Blade, Mage and
> Ranger are kit names today. **Recommendation:** new display names; kit IDs unchanged.

### Concept detail: the Kiln Warden

- **Read order at 40 px:** halo disc → shoulder mass → hammer head → stance.
- **Halo:** a flat sun-disc **solar collector array** of sun-alloy with 9–12 radial fins,
  mounted on the backplate and tilted back 20° so the 54° camera sees it face-on. Never a
  dish. It glows **only** during the ultimate (the collector charges white-gold from the
  centre out). At rest it's metal: emissive budget kept for gameplay meaning.
- **Hammer:** long haft, a block head that holds a **contained fusion core** behind an emitter
  lattice, with one thin white-gold seam. Each strike releases a little of it (the shockwave).
  The head sits above the right shoulder at idle (SIL-4).
- **Armour:** stacked, banded sun-alloy plates (a stepped profile) with precise machined
  seams and edge wear. A glazed heat-shield ceramic breastplate in cream and ochre, crazed and
  chipped. A leather apron and a tabard, so the silhouette reads as an order, not a suit.
- **Face:** a sculpted helm with a closed face plate. At 40 px the head is the top of the helm.
- **Showcase extras:** heat shimmer from the seams; strand hair not needed (helmeted).

### Concept detail: the Still Oracle

- **Read order at 40 px:** crescent crown → orbiting slabs (the gaps) → narrow robe.
- **Crown:** five obsidian shards in a crescent, 15 cm above the head, not touching it.
- **Slabs:** three palm-sized basalt slabs on a slow orbit (period about 4 s) at shoulder
  height. They are the SIL-6 gaps, and they move to the casting hand on cast.
- **Mask:** bone-white, smooth, with engraved star-chart lines and no eye holes.
- **Wrists:** thin tarnished-silver **gravity rings**: the visible source of her spells.
- **Robe:** violet-black cloth, split into long strips that drift upwards (cloth sim on High+;
  baked wobble on Low).
- **Floating:** she hovers 0.2 m up with no visible support: a faint shimmer of distorted air
  under the hem, never a jet or a glow.

### Concept detail: the Ash Strider

- **Read order at 40 px:** bow arc → hood and scarf diagonal → crouch.
- **Bow:** as tall as the hunter, made from a curved **Builder alloy rib**, thick and
  bone-white so it survives at 40 px, held diagonally across the body. Its string is a thin
  line of **hard light**.
- **Cloak:** layered charcoal cloth (L ≈ 0.35–0.45), well below the Crossing's sand so CON-1
  holds on the desert layer. A dark ash-grey scarf. Smoked crystal lenses over the eyes.
- **Scavenged parts:** a sun-alloy vambrace on one arm, Void composite plates on the other
  shoulder: worn as charms, still working.
- **Kit:** hard-light arrows with obsidian tips (Rooting Shot pins with an obsidian spike), dust trails.

## Minions

Rules: SIL-2 (≤ 0.8× the smallest hero, no capes, halos or orbiting parts); repeated by the
dozen, so they are VAT-animated and have a 1–2.5k triangle budget (roadmap §9.1); team
identity from rims only.

| | **Forge: the Foundry Thralls** | **Void: the Hollowed** |
|---|---|---|
| Idea | Bronze-alloy automata, each run by a small contained fusion core | Masked acolytes whose armour is half held together by gravity |
| Melee | **Thrall:** squat sun-alloy barrel body, a fusion core glowing softly behind a chest lattice, a smooth mask-like head, a round shield with an emitter ring and a short broad blade. 1.4 m | **Hollow:** a narrow figure in Void composite and a smooth silver mask, with shoulder plates floating 5 cm off the body (fixed gaps, no orbit), a short glaive with an emitter tip. 1.4 m |
| Ranged | **Lens Gunner** (was Bellows Gunner): a short-legged automaton carrying a focusing lens that lobs bolts of white-gold light. 1.3 m | **Shard Caster:** hovers 30 cm up, a cracked void-glass shard held overhead inside a hovering silver ring. 1.3 m |
| Siege | **Crucible Walker:** a squat, wide four-legged walker carrying a contained fusion crucible in a ring of radial fins; no visible pistons. 2.4 m | **Monolith:** a floating basalt slab bound by two gravity rings, escorted by two Hollows. 2.4 m |
| Silhouette tell | Domes and cylinders, broad bases | Verticals with fixed gaps, masks |
| Death | Collapses; the core flares white and goes dark | Pieces fall as gravity lets go, a dust ring |

**Round G4 (advanced tech):**

18 images, drawn with the recommended faction readings' soldiers as style references (review
app *Minions*; [boards/g4-units.jpg](boards/g4-units.jpg)).

- **The Thrall is ready for a model sheet:** a squat bronze-alloy automaton with a fusion core
  glowing through a chest lattice, a mask-like head and an emitter shield. Dull alloy, not
  copper: the PAL-3 problem of round 3 is gone.
- **The Hollow finally has floating shoulder plates**, the first time any generator drew the
  gaps. But its body is a plain bodysuit; the model sheet adds a tabard (the layering rule).
- **The Lens Gunner and Crucible Walker** carry their lenses and crucibles inside rings of
  radial fins, which read as **jet turbines**. Both need a different housing (a flat lens in a
  stepped collar, an open crucible) before they're modelled.
- **The Shard Caster** reads well but is the thinnest silhouette in the game: check it at 30 px.
- **The Monolith** (a slab bound by two gravity rings, escorted by two Hollows) reads at once.

Round G5 made Pro model sheets of the Thrall and the Hollow. **The Thrall sheet is
consistent in all three views and could go to modelling as is. The Hollow sheet** keeps the
floating plates and adds the tabard; its g1 adds small emitter rings at the plates, which is a
good, cheap way to show what holds them up.

**Earlier rounds (fantasy-era look).** First concept round ([boards/units.jpg](boards/units.jpg)): the Thrall reads well as a
bronze automaton, though it renders **copper-orange**. Keep it in brass and bronze
(H ≈ 75–90°) to stay out of the enemy zone. The **Hollow lost its floating shoulder plates**
and came out as a plain silver knight, so the gaps must be drawn into the model sheet
explicitly. The Monolith siege (a slab carried by two acolytes) reads at once. The Crucible
Walker is the weakest; try a squatter, wider shape.

**Round 3:**
- **The Hollow's floating plates failed again**, even with an explicit "gap of air" prompt.
  Current generators don't draw detached parts reliably, so the gaps must be drawn in the
  model sheet and built in the mesh.
- **Thrall C in brass is too saturated.** Measured, its body sits at median hue 77° with p90
  C 0.114. That is the centre of the enemy colour-blind zone and over the 0.11 accent cap
  (PAL-3). Bright brass is a PAL-3 risk for the whole Forge: keep it worn and dull, like
  sun-brass `#A98C62` (C 0.067).

## Structures

| | **Forge** | **Void** |
|---|---|---|
| Tower (12–16 m) | A **stepped heat-shield ceramic pylon** with sun-alloy banding and a **fusion lens crown** in a radial-finned collar that fires its attack as a beam. Basalt foot, heat scorch at the base | A **monolith in four blocks**, the top two hovering above the lower two; a gravity lens core in the gap that bends light and fires its attack |
| Core (18–24 m) | **The Sunheart Crucible:** a domed, stepped drum ringed by heat-radiator fins, a white-gold fusion core visible through a lattice | **The Still Well:** a ring of floating stones around a dark gravity well; dust spirals inward |
| Base walls | Stepped heat-shield ceramic and basalt with sun-alloy banding and one collector disc per segment; the wall mesh follows today's `rampart` placement | Void composite slabs, each course floating slightly above the last |
| Destruction | The core flares white, then collapses outward with molten seams | Implodes: the blocks fall inward, then drop |

**Round G4 (advanced tech):**

18 images, drawn with the recommended readings' towers and architecture as style references
(review app *Structures*; [boards/g4-structures.jpg](boards/g4-structures.jpg)).

- **The Forge tower is solved:** a stepped heat-shield pylon whose lens crown fires a
  white-gold beam. It reads as "a tower that shoots light" with no explanation.
- **The Void tower** works when its blocks hang apart with clear gaps (g1). Two of three
  filled the gaps with **glass tubes**, which reads as a sci-fi capsule; the gaps must stay
  empty air.
- **The Sunheart Crucible** (a domed drum with a lattice core) works; one take added a
  hovering ring above it, which is the Void's language. Keep the Forge grounded.
- **The Still Well** (a ring of floating stones round a dark well with dust spiralling in) is
  the most striking structure, but its swirl reads as a portal at close range; keep the
  spiral subtle at the gameplay camera.
- **Walls:** the Forge wall (stepped ceramic, one collector disc) is right. The Void wall's
  floating courses work, but continuous violet light strips between them read as neon:
  light only at a few points per course.

**Earlier rounds (fantasy-era look).** First concept round: the **Void tower** (stacked blocks, the top two floating over a
glowing core) and the **Void core** (a ring of floating stones around a gravity well) match
the shape language closely and are the strongest structures. The Forge tower (a lens-crowned
obelisk) and core (a domed foundry drum) are serviceable but less distinctive. Push the
stepped Sun-Kiln forms (Forge B).

> **Decision needed (maintainer): the neutral monsters.** The slice needs at least one camp.
> **Recommendation:** **ember scarabs**, small Builder maintenance machines shaped like
> beetles, with ember-crystal backs, that graze on the ember shards (small camp), plus a dormant
> **Builder warden machine** as the large objective (A4, not the slice). This keeps the
> Builders as machines only.

## Next steps after the pick

1. Turn the chosen concept into a **model sheet** (front, side, back and top-down at 54°):
   round G5 made Pro sheets for all three redesigns, so this starts from the picked one.
   Write down the answer to every inconsistency between the views.
2. Run it through the §9.4 pipeline: image → TRELLIS.2 → remesh and bake → review renders.
3. Readability sign-off at 46 m (the human checkpoint) against SIL and CON rules.
