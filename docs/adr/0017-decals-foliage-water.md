# ADR-0017: Decals, foliage and water (R3b)

**Status:** accepted 2026-10-30 (R3b of the
[AAA graphics roadmap](../plans/aaa-graphics-roadmap.md#r3--terrain-and-environment-l):
items 2, 3 and 4, and the R3 exit). Written before the code; the costs in
[Consequences](#consequences) come from [perf/0021](../perf/0021-decals-foliage-water.md).
Builds on [ADR-0014](0014-terrain-fog-of-war.md) (terrain, fog of war, the set-3 binding map
and the descriptor budget), [ADR-0013](0013-vfx-runtime.md) (effects, telegraph rules),
[ADR-0011](0011-art-style-lighting.md) (`shade()`, readability) and
[ADR-0008](0008-render-graph.md) (the graph). It composes with R6
(ADR-0015, #334: depth prepass, MSAA on Ultra, render scale) and R2b (ADR-0016, #335:
clustered lights, outlines, baked AO, contact shadows), both merged before it (§7).

## Context
R3a gave the board a heightfield, splat layers, fog of war v2 and an atmosphere. The R3 items
left are projected decals with guaranteed telegraph contrast, wind-animated foliage that
reacts to units without leaking hidden ones, and water with flow, depth fade, shoreline foam
and cheap refraction on High; and the exit: one fully art-directed map at budget on every
tier, with goldens from several cameras.

Constraints:
- **Presentation only, client-local** (roadmap §3): no wire, replay, simulation-logic or
  content-hash change. Decals come from effects the client already plays; foliage and water
  come from the client-local map visuals. The simulation's map stays authoritative: it is a
  flat, fully walkable board with no brush, bushes or water (checked: `moba-sim` and
  `moba-game` have no such areas), so foliage is **decorative** and water is **shallow and
  walkable by construction**.
- **The scene's sampler budget is spent.** Every scene pipeline shares one layout and its
  fragment stage uses 13 of the 16 samplers Vulkan guarantees per stage (ADR-0014 §6); R6
  adds one and R2b two more. Nothing in R3b may add a sampler to the scene layout.
- **Readability:** telegraphs stay legible over any ground (ADR-0013's enemy-telegraph rules
  apply to decals too); foliage never hides a unit and never moves for a unit the client does
  not draw; water never suggests ground the simulation would not let a unit walk on.
- Every feature is tiered, has an off switch and allocates nothing per frame.

## Decision

### 1. Decals: forward projected decals binned on a ground grid
- **API.** `Scene::decals` is this frame's `&[Decal]`: a ground position and height, a yaw,
  a shape (`Disc`, `Ring`, `Cone`, `Line`, `Splat`), a radius (the line's length), a band
  width (ring and line), a cone angle, a linear HDR colour with alpha, a style, a role, a
  `progress` (an inner fill that grows to the edge as a cast timer runs out), a `fade` and a
  seed (the splat's outline). The renderer never invents decals.
- **Styles.** `Paint` (scorch, blood, goo) blends into the surface's **albedo before
  lighting**, so it is shadowed and fogged like the ground. `Glow` (telegraphs, zones) is
  composited **after** shading, fog of war and height fog, so nothing dims it.
- **Projection.** Decals project straight down onto everything that runs the decal code
  within ±4 m of their height: the terrain (both the splat and the flat plane), water and
  foliage. Units, walls and props never take decals: a telegraph must not paint the enemy
  standing in it. There is no decal box geometry and no extra pass: every receiving shader
  calls `apply_paint_decals()` / `apply_glow_decals()` from `decals.glsl`.
- **Binning.** Per frame the CPU sorts nothing and allocates nothing: it takes the frame's
  decals into a fixed list of at most `MAX_DECALS` (128) — **enemy telegraphs first**, then
  the rest in the caller's order, up to the tier's cap (enemy telegraphs may use the whole
  128) — computes the xz bounding box of their footprints, divides it into a **32 × 32
  grid** and sets, per cell, a 128-bit mask of the decals whose footprint overlaps it. List,
  grid and box go into one host-visible storage buffer per frame in flight (24,608 bytes). A
  fragment finds its cell and walks the set bits (highest index first, so enemy telegraphs,
  at the lowest indices, are composited last, on top). Outside the box no decal is tested.
  A 2D world grid suits a ground-projected decal on a MOBA camera better than view-space
  clusters, and needs no depth.
- **Register pressure.** The decal code is inlined into the terrain shader, so it shares its
  register allocation. The shader reads each record's fields from the buffer where it uses
  them: copying the whole 64-byte record into a local kept sixteen floats live through the
  shape code and pushed the terrain shader past a register step on NVIDIA, doubling every
  terrain pixel's cost with or without decals in view (the Ultra overview's scene pass
  0.30 → 0.62 ms, [perf/0021](../perf/0021-decals-foliage-water.md)). Any change to
  `decals.glsl` needs that overview measured again.
- **Telegraph contrast (enemy telegraphs).** The client marks an enemy's telegraph decal
  `DecalRole::EnemyTelegraph` (an ally's `Ally`, the rest `Cosmetic`). For that role the
  renderer (1) never drops it before any other decal, (2) floors its alpha at
  `VfxBudget::telegraph_floor` (35 %), multiplied by `fade` only, which the client keeps at 1
  until the last 15 % of the telegraph's life (ADR-0013), and (3) draws a **two-tone rim**: a
  bright band whose colour is scaled up until its luminance is at least `1.8 ×` the
  background's plus 0.05, and a dark halo just inside it that takes the background to 20 %
  (both at `min(2 × alpha, 1)`, so the rim reads at the floor). One of the two always
  contrasts with any ground, lit or fogged, bright snow or dark ash;
  `moba_render::glow_reference` is the CPU twin of the shader, and a unit test holds the
  luminance ratio over backgrounds from black to 16× white. Ally telegraphs keep
  the rim but not the floor and are dimmed by the allied-VFX opacity, like their particles.
- **From effects** (`[[decals]]` in `vfx/<effect>.toml`, up to four per effect, parsed and
  range-checked by `moba_assets::VfxDecal`): shape, style, anchor (`target`, `source`, or
  `line` from the source to the target), radius, width, angle, delay, lifetime (default:
  the effect's emit window, so a zone's decal lasts the zone), fade in/out, colour, team
  tint, `scale_with_radius`, `telegraph`, `progress`, and `stamp`. A **stamped** decal (a
  scorch mark, a goo splat; never a telegraph) is copied at its delay into a fixed ring of 96
  persistent decals with its own lifetime (≤ 30 s), so it outlives the effect without holding
  one of the 256 effect instances; when the ring is full the oldest stamp is replaced. The
  client's `VfxSystem` builds the frame's decal list next to its particle spawns, with the
  same rules: enemy telegraph decals are exempt from distance, visibility and budget culling
  and listed first; others are culled by distance from the camera focus like their effect.
  **Decals reveal nothing:** they exist only for effect instances the client plays, which
  come only from events the server sent (or the local simulation in the local view). The
  R5 ground rings stay (decals augment them): a ring reads well on flat ground, the decal
  hugs the relief and carries the contrast rim.
- **Tiers:** decal cap Low 48, Medium 96, High and Ultra 128 (enemy telegraphs exempt).
  `MOBA_DECALS=off` binds an empty list (the effects still play their particles).

### 2. Foliage: GPU-instanced, wind-animated, displaced only by drawn units
- **Data.** A map visuals descriptor may have a `[foliage]` table (`moba_assets::MapFoliage`):
  scatter density, grass height range, base and tip colours, a flower colour, wind direction
  and strength, gust speed, reeds along water, a seed, and up to 32 `[[foliage.patches]]`
  (centre, radius, kind `grass`, `brush`, `reeds` or `flowers`, density). It needs the
  `[terrain]` table (instances stand on the heightfield).
- **Placement** is procedural and deterministic (`moba_assets::generate_foliage`, from the
  descriptor, the simulation's map and the generated heightfield; `content-check` runs the
  same code): a jittered grid over the board, kept where a low-frequency noise mask allows
  and **never** within the lane corridors, 6 m of structures, 10 m of spawns, 2 m of walls or
  in water; reeds line the water's banks; patches add brush and flowers where the
  descriptor says. Instances are `(position, yaw, scale, kind, tint)`, 16 bytes on the GPU.
  The sample maps need no authored placement: the generator places them, and the showcase
  map adds a few patches.
- **Rendering.** One storage buffer of instances, uploaded at map bind
  (`Renderer::set_foliage`), sorted into 16 m chunks. Each frame the CPU frustum-tests the
  chunks (and drops those beyond the tier's draw distance from the camera focus) and records
  one non-indexed instanced draw per run of visible chunks inside the **scene pass**, after
  the opaque units. There is no vertex buffer: `foliage.vert` builds each clump's blades
  (2 segments, 7 vertices as 5 triangles each) from `gl_VertexIndex`; blade count, width and
  stiffness by kind; far clumps collapse their extra blades to zero area. **Wind** is two
  sine gusts along the map's wind direction plus a per-instance phase, scaled by the square
  of the height along the blade. Shading is `shade()` with a normal bent towards up (soft,
  painterly grass), a base-to-tip gradient, back-light translucency, the fog of war, height
  fog and both decal styles. Foliage receives shadows but casts none (the tufts are under
  half a metre and brush is at most 1.4 m; a shadow pass over them costs more than it
  reads).
- **Its own pipeline layout** `[camera, instances, foliage, lighting]`: set 2 holds the
  foliage instance buffer (vertex storage) and a per-frame uniform with the time, the wind and
  the displacers. Sets 0/1 stay compatible with the scene layout, so the scene's bound sets
  survive; the foliage draws bind sets 2 and 3 with their own layout after the VAT draws.
  **No sampler is added to any layout.**
- **Reacting to units.** Up to `MAX_FOLIAGE_DISPLACERS` (32) displacers per frame (`xz`,
  radius, strength) push the blade tips away and flatten them. **The client builds them only
  from the units it draws this frame** (`foliage::Displacers::build` over the frame's unit
  anchors — the units the server sent and the client interpolates; nothing else), nearest to
  the camera focus first, structures excluded. A hidden enemy is not in the snapshot, so it
  is not drawn, so it bends nothing; a unit test feeds a snapshot whose hidden enemy is
  withheld (and one with a dead unit) and asserts no displacement near it. The renderer only
  uses what `Scene::environment` carries. Brush therefore never reveals anyone; and because the
  simulation has no brush areas there is also no "unit inside a bush" state to hide:
  foliage is decorative (if the simulation gains brush, its areas become `brush` patches and
  the server's vision rules decide what is drawn, as today).
- **Tiers:** density Low 0.3, Medium 0.6, High 1.0, Ultra 1.0 of the descriptor's; most
  instances 8k / 24k / 48k / 64k; blades per clump 3 / 4 / 5 / 6; draw distance 60 / 90 / 130 /
  180 m; displacers 8 / 16 / 32 / 32. `MOBA_FOLIAGE=off` draws none.

### 3. Water: shallow rivers and pools over a carved bed
- **Data.** Up to eight `[[water]]` bodies in the map visuals (`moba_assets::MapWater`):
  a centre polyline (1–32 points; one point is a round pool), width, surface `level`, bed
  `depth`, `flow` speed along the polyline, deep and shallow colours, `clarity` and `foam`.
  It needs the `[terrain]` table.
- **The bed is carved into the terrain** by the generator
  (`moba_assets::generate_terrain_with_water`): inside a body the ground falls smoothly to
  `level − depth` at the centre line and meets the surface exactly at the banks; just outside,
  the banks rise a few centimetres above the surface. Everything stays inside the board's
  ±0.5 m relief rule (`MAX_PLAY_RELIEF`), which therefore also bounds the water: the surface
  is at most `MAX_WATER_LEVEL` (0.3 m) above the ground units stand on and at most
  `MAX_WADE_DEPTH` (0.8 m) above the bed, so a hero (2 m) visibly wades and never swims. The
  splat puts the path layer (pebbles, sand) on the bed.
- **Mesh** (`moba_assets::generate_water`): the heightfield cells a body covers, at the
  surface level, with the **same vertices and diagonal split as the terrain grid**, so the
  per-vertex bed depth (surface minus terrain height) interpolates to exactly the terrain
  surface the GPU draws underneath. Per vertex: position, the flow vector (along the nearest
  polyline segment, slowing towards the banks: the flow map, baked into the mesh) and the
  bed depth. Uploaded at map bind (`Renderer::set_water`).
- **Shading** (`water.frag`): flow-advected procedural normals (two value-noise octaves
  sampled at two phases of the flow, cross-faded, so the pattern never stretches); depth fade
  from the bed depth (`1 − exp(−depth / clarity)` between the shallow and deep colours);
  `shade()` for the sun, IBL reflection, shadows and the fog of war; shoreline foam where the
  bed depth falls below a few centimetres, broken by noise and pushed by the flow; glow
  decals on the surface only (what shows through already has them); fragments over dry
  ground (negative bed depth) write nothing new (transparent black for `Simple`, the
  unchanged copied pixel for `Full`; no `discard`, which a Vulkan 1.3 target compiles to a
  demote-to-helper the device need not enable), which gives the shoreline without any
  authored edge. The per-vertex depth is also clipped to `width / 2` minus the distance from
  the centre line, so relief that dips below the surface past the carved bank stays dry.
- **Two modes.** `Simple` draws the water at the end of the scene pass, alpha-blended over
  the carved bed (real transparency; no extra pass). `Full` (High and up) adds
  **screen-space refraction**: a `water_copy` graph pass copies the HDR target, then a
  `water` pass, loading HDR and depth read-only (`Access::DepthReadSampled`, as the R5
  particles do), samples the copy offset by the normal, scaled by depth, and rejects offsets
  that land on geometry in front of the water (scene depth nearer than the surface), plus
  foam where the scene depth meets the surface (around units' legs). The minimap bake always
  draws the simple path in its offscreen pass. On portability-subset devices (`MoltenVK`)
  `Full` falls back to `Simple` (same reason as soft particles, ADR-0013).
- **Its own pipeline layout** `[camera, instances, water, lighting]`: set 2 holds the water
  style uniform (up to eight bodies' colours, clarity and foam; the time) and, for `Full`,
  the refraction copy and the scene depth: two samplers in a layout that is not the scene's.
  Its fragment stage sees set 3's 10 (with R6's AO; 12 after R2b) plus 2, within 16; the
  budget check covers it. The refraction copy and the depth are read by texel at
  `gl_FragCoord` (the render region may be a corner of the targets, R6).
- **Tiers:** Low and Medium `Simple` (Low with one normal octave), High and Ultra `Full`.
  `MOBA_WATER=off|simple|full`. With water off the carved bed still shows (a dry
  stream bed; the terrain is generated once per map).

### 4. Visual-versus-simulation rules for water and foliage (`content-check`)
`check_map_consistency` gains these, on the same generated heightfields as ADR-0014 §2
(every tier's grid, with the water carved):
- **Errors:** a body's level outside `[−MAX_PLAY_RELIEF, MAX_WATER_LEVEL]`; any part of a
  body (its polyline widened by half its width) outside the board, where the ground only
  rises; any board sample deeper than `MAX_WADE_DEPTH` below a surface (no water a unit could
  not visibly wade through: the simulation lets every unit walk everywhere). The relief rule
  is re-checked with the bed carved.
- **Warnings:** water within 4 m of a structure or over a spawn; water crossing a wall;
  overlapping bodies; a `brush` patch over a lane corridor or within 6 m of a structure (it
  would read as cover the simulation does not have).

### 5. Frame
```text
(portraits, minimap) → (fow-raster, fow-resolve) → vfx_simulate → (shadow)
  → scene [terrain (paint+glow decals), units, skinned, sky, VAT, foliage, water if Simple]
  → (water_copy → water)   [Full only]
  → vfx_draw → (vfx_resolve) → composite → ui → (capture)
```
The scene stores depth when either the water pass or the particles read it (single-sampled:
with MSAA they read the resolve or the prepass's depth). R3b adds two passes and one
resource to R6's graph (`MAX_PASSES` 32, `MAX_RESOURCES` 48 with a `u64` mask): the
largest frame (both offscreen passes, fog of war, shadow, the prepass and AO, the scene, the
two water passes, the three VFX passes, the bloom chain, exposure, composite, FXAA, upscale,
UI and capture) has 29 passes.

### 6. Set 3 and the descriptor budget
Set 3 gains **binding 20**, the decal storage buffer (fragment), numbered past the bindings
R6 (AO at 10) and R2b (11 and up) take, so the three merge without renumbering. It is a
storage buffer, not a sampler: R3b adds no sampler (the scene fragment stage is at 16 of 16
with R6's AO and R2b's baked AO and contact shadows). The budget
check (`descriptor_budget`) now also counts **storage buffers per stage** against
`maxPerStageDescriptorStorageBuffers` (4 on the Vulkan minimum): scene vertex 2 (instances,
joints), fragment 3 (R2b's light list and grid, the decals); foliage vertex 3, fragment 3.
The foliage
and water layouts are checked at creation like the scene's and VAT's.

| Layout | Fragment samplers | Vertex storage | Fragment storage |
|---|---|---|---|
| scene (sets 0–3) | 13 (R3a) → 14 (R6) → 16 (R2b) | 2 | 3 |
| VAT | 12 (R2b) | 2 | 3 |
| foliage `[camera, instances, foliage, lighting]` | 12 | 3 | 3 |
| water `[camera, instances, water, lighting]` | 14 | 2 | 3 |

### 7. Composition with R6 and R2b (both merged first; as implemented)
- **MSAA 4× (R6, Ultra):** the foliage and the `Simple` water pipelines are scene-pass
  pipelines, so they get multisample twins through R6's `view_pipelines`, like the sky. The
  `Full` water pass runs after R6's resolve on single-sample HDR and depth, like the
  particles; the `water_copy` copies the resolved HDR.
- **Depth prepass (R6/R2b, High and up):** foliage and water are not in the prepass (GTAO
  and the outline IDs ignore them; foliage still writes depth in the scene pass, which tests
  `LESS_OR_EQUAL` against the loaded prepass depth, so it shades correctly). They write no
  outline IDs, and the terrain's decals change only its colour. A unit standing in brush
  therefore keeps its screen-space outline (High and up) over the blades: deliberate, since
  the brush is decorative and the simulation does not hide units in it, so foliage must
  never cost readability. The inverted-hull outline (Low and Medium) is depth-tested in the
  scene pass like any geometry, so tall brush can cover part of it there.
- **Render scale (R6):** the `water` pass sets its viewport and scissor to the frame's render
  sub-rectangle and the `water_copy` copies only that rectangle; the copy target is
  `ImageSize::Render { shift: 0 }`.
- **Clustered lights (R2b):** the decal and light buffers are separate set-3 bindings; R2b
  binds lights at 11 and 12, so the scene's fragment stage has three storage buffers, within
  the minimum of four. Effect lights shine on foliage and water through `shade()`, and
  the baked AO and contact shadows apply to them too.
- **Graph limits:** all three raise `MAX_PASSES` to 32; R6 and R2b also raise resources to 48
  with a `u64` mask, which R3b adopts.
- **GPU culling (R8):** the foliage draw is self-contained (one instanced vertexless draw per
  visible 16 m chunk from a static instance buffer, chunks culled on the CPU), so R8 can move
  the chunk test to a compute pass and an indirect draw without touching the shaders.

## Alternatives considered
- **Deferred (box) decals after the opaque pass**, sampling scene depth. The classic choice:
  it decals any geometry and needs no shader changes. But it needs a pass with read-only
  depth on every tier (Low included), cannot change albedo before lighting (paint decals
  would not be shadowed), and would paint units standing in a telegraph. Forward decals in
  the three receiving shaders cost a buffer and a loop.
- **A decal texture atlas.** Painted decals look richer, but an atlas is a sampler in the
  scene layout, which has none to spare; procedural shapes (signed distances and hash noise)
  cover telegraph shapes exactly and scorch/goo well enough for the sample content. If art
  wants textures, the atlas can become a layer range of the terrain albedo array (same
  sampler).
- **View-space clustered decals** (sharing R2b's grid). R2b's grid is the camera's ground
  footprint for lights; decals want their own extent (a dense box around where they are), and
  sharing would couple two caps.
- **Alpha-tested foliage cards** (textured quads). Cheaper geometry, but they need a texture
  (a sampler), alpha-to-coverage under MSAA and sorting-free alpha; procedural blades need
  nothing and stay crisp at MOBA zoom.
- **A trample texture** (units rendered into a small texture the foliage samples). Scales to
  any unit count, but needs a pass and a sampler; 32 displacers in a uniform cover the units
  near the camera, where bending is visible.
- **Water as a flat plane at a global level** with a depth test against the terrain. Simple,
  but needs scene depth for any fade on Low, cannot have per-river levels or flow, and shows
  water wherever the relief dips. The carved bed plus a mesh on the terrain grid gives exact
  depth on every tier with no depth sampling.
- **Planar reflections.** A second scene render; the environment cube and the sun's GGX lobe
  read as water at MOBA zoom.

## Consequences
- New public API (additive, `#[non_exhaustive]` with constructors and builders): in
  `moba-render` `Decal`, `DecalShape`, `DecalStyle`, `DecalRole`, `MAX_DECALS`,
  `EnvironmentConfig`, `WaterMode`, `FoliageData`, `FoliageInstanceData` (`kind` a `u32`
  0–3), `EnvironmentFrame`, `FoliageDisplacer`, `MAX_FOLIAGE_DISPLACERS`,
  `MAX_FOLIAGE_INSTANCES`, `MAX_WATER_BODIES`, `MAX_WATER_VERTICES`, `WaterData`,
  `WaterVertexData`, `WaterStyle`, `glow_reference`, `RenderError::InvalidWater`,
  `Scene::decals`/`with_decals`/`environment`/`with_environment`,
  `RenderConfig::environment`/`with_environment`/`effective_environment`,
  `Renderer::{set_foliage, clear_foliage, foliage_instances, set_water, clear_water,
  has_water, environment_config, decals_dropped}`; in `moba-assets` `VfxDecal`,
  `VfxDecalShape`, `VfxDecalStyle`,
  `VfxEffect::decals`, `MapFoliage`, `FoliagePatch`, `FoliageKind`, `MapWater`,
  `MapVisuals::{foliage, water}`, `FoliageInstance`, `generate_foliage`, `WaterMesh`,
  `WaterVertex`, `generate_water`, `generate_terrain_with_water`, `MAX_WATER_LEVEL`,
  `MAX_WADE_DEPTH`.
- No wire, replay, simulation-logic or content-hash change: effects and map visuals are
  client-local.
- Goldens: the default grass map becomes the showcase (a river with fords across the middle,
  pools, reeds, brush and grass), so every golden that shows it is rebaselined deliberately;
  new goldens cover the river from a low camera on High and on Low, and the decals (telegraph
  rings, scorch and splats) with water and foliage from the 5v5 teamfight replay at tick 360.
- Costs and validation: [perf/0021](../perf/0021-decals-foliage-water.md).
