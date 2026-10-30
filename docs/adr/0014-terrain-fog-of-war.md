# ADR-0014: Terrain, fog of war v2 and atmosphere

**Status:** accepted 2026-10-29 (R3a of the
[AAA graphics roadmap](../plans/aaa-graphics-roadmap.md#r3--terrain-and-environment-l):
items 1, 5 and 6). Builds on [ADR-0008](0008-render-graph.md) (render graph),
[ADR-0010](0010-texture-pipeline.md) (KTX2 textures) and
[ADR-0011](0011-art-style-lighting.md) (lighting). Decals, foliage and water are R3b.

## Context
Until R3a the ground was one 300 m plane with a procedural grid and a per-map tint. Fog of
war was a loop in `terrain.frag` over 16 vision circles and 32 wall segments in the camera
uniform, so a 5v5 with towers and minions ran out of circles: the client dropped minions
first and then everything past 16. The board's edge was hidden only by a distance fade to the
ambient colour, and there was no sky. The R7b minimap used a 24×24 CPU grid that ignored
vision-blocking walls.

Constraints:
- **The simulation's map is authoritative and unchanged.** It is a flat 2D board. Walls block
  line of sight only; units walk through them. Every map is fully walkable inside its extent.
- **Presentation only.** Terrain, fog of war and atmosphere are client-local visuals. They are
  never hashed, never sent and never fed back into the simulation. The server still filters
  what each client sees; the fog texture only draws the boundary of what the client already
  knows.
- **Roadmap §3:** every feature is tiered (Low/Medium/High/Ultra), has an off switch and
  allocates nothing per frame.
- **Readability (ADR-0011 §6)** applies: visual relief must never suggest an obstacle, ramp or
  cliff that the simulation does not have.

## Decision

### 1. Terrain: a heightfield mesh with splat-map texturing
- **Data** lives in the map visuals descriptor's new `[terrain]` table
  (`maps/<name>.toml`, `moba_assets::MapTerrain` in `MapVisuals::ground`): 1–8 named layers
  (`textures/<layer>.ktx2` albedo, BC7 sRGB with ambient occlusion in alpha, and an optional
  `textures/<layer>-normal.ktx2` BC5 normal map), a tint and a tiling distance per layer, an
  optional splat map (`splat`, and `splat2` for layers 5–8: linear RGBA KTX2, baked with
  `texture-bake --kind data`), the play-area relief, the map-edge style and height, and a seed.
  With `splat` alone the array has one layer and layers 5–8 get no weight (the shader reads
  the second splat layer only when `TerrainBlock` says it exists).
- **A procedural generator** in `moba-assets` (`generate_terrain`, pure and deterministic,
  so `content-check` runs the same code as the client) builds the heightfield and, when the
  descriptor names no splat map, the splat weights. It reads only the simulation's map
  (extent, lanes, walls, structures, spawns; `TerrainSite`). Layers have roles by position:
  1. base ground; 2. paths (along lanes, around structures and spawns); 3. rock (slopes, the
     map edge, the feet of walls); 4. variation patches on the base; 5–8 extra variation
     patches. The sample maps therefore need nothing external: their layer textures come from
     `texture-bake generate terrain` (self-generated patterns) and their splat from the
     generator.
- **Relief rules** (`content-check` enforces them; the generator clamps to them):
  - **Inside the board** `|height| ≤ MAX_PLAY_RELIEF` (0.5 m). Units stand at `y = 0`; half a
    metre keeps feet on the ground and can never read as a ramp, ridge or pit.
  - **Outside the board** the ground only rises (`height ≥ 0`), into hills or cliffs up to
    `edge_height` (≤ 48 m). The walkable board reads as the basin; the edge of the
    simulation's space is the foot of the slope. The rise starts one Low-tier grid cell
    (`(extent + 2 × margin) / 128`) outside the board, so a grid cell that straddles the edge
    interpolates nothing taller than the play relief on any tier.
- **Mesh.** One indexed grid over the board plus a margin (0.6 × extent, 96–240 m), with
  heights and normals in the vertices and the baked relief ambient occlusion in the UV slot
  of the static vertex layout (no new vertex format). Grid size by tier (below). Uploaded at
  map bind (`Renderer::set_terrain`), never per frame.
- **Shading** (`terrain.frag`, the same `shade()` as everything else): per layer, the splat
  weight × a height-blend of the layer's AO, tinted albedo; top-down projection on flat
  ground and **triplanar on slopes** (blend weight from the geometric normal, only where
  `N.y < 0.85`, so the board pays for one projection); the layers' normal maps in a tangent
  frame aligned with the projection (whiteout blend); the layer AO and the baked relief AO
  multiply the ambient term. The layers are two `2D_ARRAY` textures (albedo, normal) and the
  splat a two-layer `RGBA8` array, so the terrain adds three samplers to set 3 whatever the
  layer count (the per-stage minimum is 16 samplers; units already use 9).
- **Walls and cliffs are props.** Walls are still drawn along every simulation wall segment
  (`wall_mesh` or the built-in block; the placement math moved to `moba_assets::wall_box` so
  the client and `content-check` share it). The descriptor can add `[terrain] edge_mesh`, a
  model from the asset root placed around the board edge as cliffs; it counts against the
  four-mesh limit.
- **Fallback.** No `[terrain]` table, a missing or mismatched layer texture, or
  `MOBA_TERRAIN=flat`: the R2a plane with its procedural grid.

### 2. Visual-versus-simulation consistency (`content-check`)
`moba_assets::check_map_consistency(&MapVisuals, &TerrainSite)` runs for every
`[[assets.maps]]` descriptor whose name the pack's map uses, and for
`maps/<visuals>.toml` next to the pack whenever it exists, listed or not, because the client
loads that file by name. **Errors** fail the check:
1. Board relief above `MAX_PLAY_RELIEF`, or edge ground below 0, checked on the generated
   heightfield of every tier's grid (`TERRAIN_GRID_SIZES`: 128, 192, 256 and 384 cells),
   at the vertices and at bilinear samples across the cells that straddle the board edge
   (`board_edge_relief`: the surface the GPU draws, not only its vertices), so a future
   generator change cannot slip past.
2. A simulation wall without a visual wall covering it: the wall box must contain both
   endpoints, and `wall_height` must be at least `MIN_WALL_HEIGHT` (2 m: lower walls do not
   read as line-of-sight blockers next to a 2 m hero).
3. A simulation wall outside the board (its visual would stand on the edge slope).
4. An edge prop (`edge_mesh`) that reaches into the board. The client places edge props by
   the mesh's real fitted footprint (`edge_footprint_radius`, half the diagonal of its
   scaled XZ bounds; `edge_placements` takes it); `content-check` loads the same
   `models/<edge_mesh>.glb` and fails when the gap to the board is smaller than that
   footprint (`check_map_consistency_with_edge_footprint`).
5. Layer, tiling and splat values out of range (also rejected by the parser), and a splat or
   layer texture that is missing, has the wrong slot format (albedo sRGB, normal BC5/RG8,
   splat linear RGBA) or whose layers disagree on size or mip count.

**Warnings** (reported, not fatal; art direction may accept them):
6. A prop at least 2 m tall whose footprint overlaps a lane corridor (`lane_width / 2` from
   the lane polyline) or comes within 4 m of a structure: it looks like an obstacle units walk
   through. (Props elsewhere are decoration by design: the simulation has no obstacles, and
   flagging every tall prop would make the warning noise.)

### 3. Fog of war v2: a visibility texture built on the GPU
- **Inputs** (`Scene::fog_of_war`, `FogOfWar`): the client's **own-team** vision sources
  (`VisionSource`: position and radius; heroes, minions, structures and, when the simulation
  gains them, wards) and the map's static line-of-sight walls (`VisionWall`), plus the board
  rectangle and the frame time. Enemy units are never sources: the client builds the list
  from its own team only (`hud::FogInput`, unit-tested with enemies in the snapshot), so the
  texture is built only from what the team sees. It is a presentation of the server's
  vision, not the server's grid: the disc is inset by one texel before the blur so the
  blurred edge ends within half a texel of the vision radius, the wall test blocks every
  sight line the simulation blocks (a property test against the simulation's rule), and
  lost vision drops to half at once, yet bilinear sampling can still show static ground up
  to about one texel (0.25–1 m by tier) past what the server sees, and a just-lost area
  stays half visible for a moment. Only the static ground is affected: units are never
  drawn from the texture (the server decides which ones the client receives). Capacity
  `RenderConfig::max_vision_sources` (default 1,024, clamped to 65,536 and to the device's
  `maxStorageBufferRange / 16`) and 256 walls; extra sources are dropped and counted, never
  silently capped at 16.
- **Two render-graph passes** before the shadow pass:
  1. `fow-raster`: one instanced quad per source into a fixed-size `R8_UNORM` transient,
     cleared to 0 and blended with `MAX`. The fragment shader tests the segment from the
     source to the texel against every wall (hard line-of-sight shadows) and fades the disc's
     last 1.5 texels, ending one texel inside the vision radius.
  2. `fow-resolve`: a fullscreen pass that blurs the raw texture (a 3×3 tent from four
     bilinear taps) and eases the previous frame's result towards it:
     `mix(previous, blurred, 1 - exp(-dt / tau))`, with `tau` 0.08 s when an area is revealed
     and 0.3 s when it fades back to fog, moving at least one 8-bit step per frame so a slow
     fade never stalls on the quantisation. Where vision is lost the value first drops at once
     to at most half visible (or the current vision, if higher), so the area reads as fogged
     the moment the server stops seeing it. A capture pinned to a tick passes
     `FOG_OF_WAR_SETTLE_DT`, which settles the texture in one frame, so goldens never depend
     on how many frames led up to the tick. The result goes to one of two persistent images
     (ping-pong by frame-in-flight index, so every descriptor is static); the other holds the
     previous frame. Both are imports that start and end every frame in
     `SHADER_READ_ONLY_OPTIMAL` (ADR-0008's R7b amendment); the graph derives the barriers.
- **Consumers:** the terrain shader samples it through set 3, and the UI pass samples it for
  the minimap (`UiDrawList::fog_mask`), which now respects walls. The 24×24 CPU grid remains
  for `MOBA_FOW=legacy` (and when there is no renderer); `MOBA_FOW=off` shows no fog on the
  minimap either. Units, walls and props are **not** darkened: the server decides which units the
  client sees, so fogging a unit could only hide a visible one; the ground mist covers the
  feet of walls instead. Offscreen views bind a white image: portraits and the minimap bake
  are never fogged.
- **Style** per map (`[fog_of_war]` table, `MapFogStyle`, into `SceneAtmosphere`): colour,
  darkening, desaturation, and a ground mist (`mist_height`, `mist`) that thickens unseen
  ground and leaves the tops of tall walls readable.
- **Switches:** `MOBA_FOW=texture` (default), `legacy` (the 16-circle uniform loop, kept for
  comparison) or `off`; `MOBA_FOW_SIZE=<64..1024>`. Spectators get no fog.
- **Frames in flight.** The ping-pong and the per-frame lighting sets assume two frames in
  flight; a compile-time assertion ties `FOG_IMAGES`, `MAIN_SETS` and `FRAMES_IN_FLIGHT`.

### 4. Atmosphere: height fog, sky and the map edge
- **Height fog** (terrain and unit shaders, `atmosphere.glsl`): analytic exponential height fog
  integrated along the view ray (density at the ground, height falloff, start distance,
  maximum opacity, colour), per map from `[atmosphere]` (`MapAtmosphere`). Off
  (`MOBA_FOG=off`) keeps the R2a distance fade, so the board edge still never shows.
- **Sky** (a fullscreen triangle at the far plane, drawn after the opaque geometry with depth
  test `LESS_OR_EQUAL` and no depth write): the environment's prefiltered cube at a blurred
  level when the map has one, else a zenith/horizon gradient with a sun glow from
  `[atmosphere]`. The horizon colour is the fog colour, so distant terrain fades into the
  sky without a seam. `MOBA_SKY=off` keeps the clear colour.
- **Map edge:** the terrain margin rises into hills or cliffs (or `edge_mesh` props), the
  height fog thickens towards the low horizon, and the sky closes the view, so neither the
  player camera nor the spectator's low orbit sees a board edge.

### 5. Tiers and switches

| | Low | Medium | High | Ultra |
|---|---|---|---|---|
| Terrain grid (cells per side) | 128 | 192 | 256 | 384 |
| Layers sampled | 4 | 8 | 8 | 8 |
| Layer normal maps | off | on | on | on |
| Triplanar on slopes | off (top projection) | off | on | on |
| Fog-of-war texture | 128² | 256² | 256² | 512² |
| Height fog | on | on | on | on |
| Sky | gradient | environment | environment | environment |

`RenderConfig::world` (`WorldConfig::for_tier`, `with_*`) holds them; the client maps
`MOBA_TERRAIN=flat|splat`, `MOBA_FOW=texture|legacy|off`, `MOBA_FOW_SIZE`, `MOBA_FOG=off` and
`MOBA_SKY=off` onto it. Tier differences are uniforms, not shader permutations: the terrain
and sky add one pipeline each and fog of war two, all built at creation.

### 6. Descriptor budget and the set-3 binding map
Every scene pipeline shares one layout, so each sampler a later feature adds to set 3 counts
against every scene pipeline. Vulkan guarantees only 16 samplers and 16 sampled images per
stage (`maxPerStageDescriptorSamplers`/`SampledImages`; also `MoltenVK`'s limit), and a
combined image sampler counts against both. Renderer creation sums them over the scene
layout's sets, and the VAT layout's (R4b: set 2 is one vertex-stage VAT texture, sets 0, 1
and 3 are shared), from the same binding arrays the layouts are created from, and fails with
`RenderError::DescriptorLimit` naming the stage and the count instead of a driver error; a
unit test pins the current total. Fragment stage today:

| Set | Binding | Descriptor | Added by | Samplers |
|-----|---------|------------|----------|----------|
| 0 | 0 | `CameraBlock` uniform | R2a | – |
| 1 | 0, 1 | instances, joint matrices (storage, vertex only) | R2a, ADR-0012 | – |
| 2 | 0 | `MaterialBlock` uniform | ADR-0010 | – |
| 2 | 1–4 | base colour, normal, metallic-roughness, mask | ADR-0010 | 4 |
| 3 | 0 | `shadow_map` (`sampler2DShadow`) | R2a | 1 |
| 3 | 1 | `shadow_depth` | R2a | 1 |
| 3 | 2 | `environment_specular` (cube) | R2a | 1 |
| 3 | 3 | `brdf_lut` | R2a | 1 |
| 3 | 4 | `lighting_ramp` | R2a | 1 |
| 3 | 5 | `fog_visibility` | R3a | 1 |
| 3 | 6 | `terrain_splat` (2D array) | R3a | 1 |
| 3 | 7 | `terrain_albedo` (2D array) | R3a | 1 |
| 3 | 8 | `terrain_normal` (2D array) | R3a | 1 |
| 3 | 9 | `TerrainBlock` uniform | R3a | – |
| 3 | 10 | `ambient_occlusion` (screen-space AO) | R6 ([ADR-0015](0015-post-processing.md)) | 1 |
| 3 | 11, 12 | light list, light grid (storage) | R2b ([ADR-0016](0016-lights-outlines-ao.md)) | – |
| 3 | 13 | `baked_ao_map` | R2b | 1 |
| 3 | 14 | `contact_map` | R2b | 1 |
| | | | **Total** | **16 of 16** |

R3a left three samplers for later set-3 work; R6's AO took one and R2b the other two, so the
budget is full (ADR-0016 §6 has the follow-up that frees one).
ADR-0016 §6 plans the headroom back: the contact term moves into R6's AO target. The terrain already packs its layers
into arrays (three samplers for up to eight layers); the next step, if more room is needed,
is to put the splat weights into the albedo array or to merge set-3 bindings into arrays of
samplers, rather than adding a binding per texture.

## Alternatives considered
- **CPU rasterisation of the visibility texture.** Simple and testable, but shadow casting per
  source against every wall at 256² is tens of millions of operations per tick on one core;
  the GPU does it in microseconds. A CPU reference rasteriser (`fog::rasterize_reference`)
  still exists for tests and documents the shader's maths.
- **One pass with `max(previous × decay, new)` blending.** Saves a pass but reveals instantly
  and cannot blur; two passes cost a few microseconds more.
- **A compute pass.** Equivalent; R5 introduces compute passes separately, and fragment passes
  reuse the graph's existing accesses and the blend hardware for `MAX`.
- **Terrain layers as eight separate textures.** Eight samplers per array would exceed the
  16-sampler per-stage minimum with the material set bound; texture arrays cost three.
- **A displaced heightfield in the vertex shader.** Needs vertex texture fetch and a height
  texture; the static mesh is generated once per map, costs one draw, and keeps the shadow fit
  and the minimap bake unchanged.
- **Moving units onto the terrain height.** The simulation is flat; the 0.5 m relief rule
  makes it unnecessary and keeps unit placement untouched.
- **KTX2 texture arrays from the baker.** The renderer assembles arrays from per-layer 2D
  KTX2 files at load, so the baker and the bounded loader stay as they are.

## Consequences
- New public API, all additive and `#[non_exhaustive]` with constructors and `with_*`
  builders: in `moba-render` `WorldConfig`, `FogOfWarMode`, `FogOfWar`, `VisionSource`,
  `VisionWall`, `SceneAtmosphere`, `TerrainData`, `Scene::fog_of_war`/`atmosphere`,
  `RenderConfig::world`/`max_vision_sources`, `Renderer::{set_terrain, clear_terrain,
  has_terrain, world_config}`, `vision_sources_dropped`, `UiDrawList::fog_mask`; in
  `moba-assets` `MapTerrain`, `TerrainLayer`, `EdgeStyle`, `MapAtmosphere`, `MapFogStyle`,
  `MapVisuals::{ground, atmosphere, fog_of_war}` (the `[terrain]` table is `ground`, because `MapVisuals::terrain` was already the flat tint, which moves into the table as `tint`; the old `terrain = [r, g, b]` form still parses), `TerrainSite`, `Heightfield`,
  `GeneratedTerrain`, `generate_terrain`, `wall_box`, `check_map_consistency` and its report,
  `check_map_consistency_with_edge_footprint`, `edge_footprint_radius`, `edge_placements`,
  `board_edge_relief` and `TERRAIN_GRID_SIZES`; `RenderError::DescriptorLimit` and
  `MAX_VISION_SOURCES` in `moba-render`.
- The camera uniform grows by seven `vec4`s (fog-of-war and atmosphere parameters); set 3
  gains five bindings (fog-of-war visibility, splat, layer albedo, layer normal and the
  `TerrainBlock` uniform with the terrain rectangle, per-layer tiling and tints and the splat
  layer count), bringing the scene's fragment stage to 13 of the 16 guaranteed samplers (§6).
- The graph limits of 12 passes and 24 resources (raised by R5, ADR-0013) hold the largest
  frame: both offscreen passes, the two fog-of-war passes, the three VFX passes, shadow, scene,
  composite, UI and capture are exactly 12 passes over 22 resources (a unit test compiles
  every such variant).
- No wire, replay, simulation-logic or content-hash change: every new field is client-local.
  The default, networked and VAT goldens are rebaselined deliberately, and new goldens
  cover the map from several cameras.
- Costs and evidence: [perf/0017](../perf/0017-terrain-fow.md).
