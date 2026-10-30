# Roadmap: from preview renderer to AAA-quality MOBA visuals

Written 2026-10-27. Status: **engine work complete** (2026-10-30): every phase R0–R8 is merged
on `main`. The remaining maintainer and art items, deferred to pre-1.0, are listed in
[Completion status](#completion-status-2026-10-30). Owner: maintainer.

This plan covers **graphics only**: the renderer, the asset and animation pipeline, VFX, art
production and the performance evidence a commercial release needs. Gameplay, matchmaking,
accounts and live operations are separate tracks (see [§9](#9-other-tracks-a-shippable-game-needs)).

---

## Completion status (2026-10-30)

The follow-on plan is the [next-gen graphics roadmap](next-gen-graphics-roadmap.md).

The engine is now AAA-*capable* in the sense of [§2](#2-what-aaa-means-for-a-moba): every
engineering item below is merged, tiered (Low/Medium/High/Ultra) with an off switch, clean
under validation and sync validation on lavapipe, and covered by goldens. Rendering stays
purely presentational: the only wire changes were R0's WP-3.1 (protocol v40, asset references)
and R4b-1 (protocol v41, basic-attack events for hit-frame sync); sim logic v8, replay v5 and
the golden state hash were untouched by every rendering phase. R4's exit needed a gameplay
feature, recall, which is protocol v42 and sim logic v9 (replay format still v5).

| Phase | Engine work | Merged in | Exit items left to the maintainer or the art team |
|---|---|---|---|
| R0 Asset foundation | done | #317, #309, #319, #316, #318 | — |
| R1 Renderer architecture | done; exit met under the maintainer's reading of the 5 % criterion (below) | #323 (R1a), #327 (R1b) | — |
| R2 Lighting and materials | done | #330 (R2a), #335 (R2b) | deferred to pre-1.0 by the maintainer: timings on the three §7 reference GPUs; art-direction readability sign-off |
| R3 Terrain and environment | done | #333 (R3a), #336 (R3b) | deferred to pre-1.0 by the maintainer: sign-off of "one fully art-directed map" (the sample maps are engine-authored) |
| R4 Animation runtime | done; exit met with recall ([perf/0024](../perf/0024-recall.md)) | #324 (R4a), #325 (R4b-1), #332 (R4b), #340 | — |
| R5 VFX | done | #329 | deferred to pre-1.0 by the maintainer: §7 reference-GPU timings (#9/#127) |
| R6 Post and image quality | done, including item 7 (distortion) | #334, #338 | deferred to pre-1.0 by the maintainer: the blind A/B readability sign-off (image set in perf/0019) |
| R7 UI and presentation | done | #326 (R7a), #328 (R7b) | — (champion select does not exist yet) |
| R8 Performance and scaling | done | #337 | deferred to pre-1.0 by the maintainer: reference-machine matrix and min-spec/MoltenVK evidence (#9/#127); a self-hosted GPU runner for `nightly-perf.yml` |

**R1's "≤ 5 % GPU-time overhead".** As [perf/0011](../perf/0011-render-graph.md) shows, the
literal relative figure cannot hold for any design that adds the HDR target R1 itself asks for:
the old frame did 13–30 µs of GPU work and one fullscreen copy is ~14 µs. **Maintainer
decision (2026-10-30):** the criterion means **graph overhead for the same work** — barriers
below 1 µs, CPU recording within noise — which is met; the HDR composite's cost is a feature
cost and counts against each tier's §7 GPU budget, which every tier meets on the RTX 5070 Ti
([perf/0022](../perf/0022-r8-scaling-pacing.md)).

**Left for the maintainer, all planned for just before 1.0:**
- **Art production (§8)**: an art director, the style guide sign-off and the heroes and maps
  themselves. The engine ships engine-authored sample content only.
- **Reference hardware** (§7, #9/#127): Low/min-spec (UHD 620, Apple M1 via MoltenVK),
  Medium and High machines, and one self-hosted `[self-hosted, gpu]` runner with
  `HAS_GPU_RUNNER=true` so `nightly-perf.yml` gates budgets every night. Every real-GPU number
  so far is from one RTX 5070 Ti.
- **Sign-offs**: R2's readability review, R3's art-directed map, R6's blind A/B.

**Deferred to the pre-1.0 milestone (maintainer decision, 2026-10-30).** The art-direction
sign-offs (R2 readability, R3 art-directed map, R6 blind A/B), the §7 reference-GPU matrix
(#9/#127) and the self-hosted GPU runner are explicitly deferred by the maintainer to the
pre-1.0 milestone, alongside the 10-client playtest and the independent external review. They
are not open engine work. The RTX 5070 Ti evidence that a full GPU blocked earlier the same
day (R8's validation matrix, the 10-minute `perf-replay.sh` and baseline refresh, R3b's
`terrain-decal-check.sh` and the distortion timings) has been re-run on `main`
([perf/0021](../perf/0021-decals-foliage-water.md), [perf/0022](../perf/0022-r8-scaling-pacing.md),
[perf/0023](../perf/0023-distortion.md)).

**Optional or deferred engine work** (not exit items): GPU-driven indirect culling (ADR-0018 §2
sets the triggers), TAA, SMAA and FSR (ADR-0015), hero and skin streaming at champion select
(waits for a champion-select flow), BC6H environments (ADR-0010), bindless descriptors
(ADR-0009), and moving contact shadows into the half-res GTAO pass to free a scene sampler
(the scene fragment stage is at 16 of 16).

## 1. Where we were (2026-10-27) and are now

The engine has a solid base to build on, but today's picture is a prototype. Compare
[the NVIDIA teamfight capture](../images/m8-nvidia-teamfight.png) with any commercial MOBA.

| Area | State (written against `main` @ 9a251a1; rows updated as each phase merged, last 2026-10-30) |
|---|---|
| Frame structure | **R1a:** a render graph ([ADR-0008](../adr/0008-render-graph.md)): `scene` → HDR colour (`R16G16B16A16_SFLOAT`, `B10G11R11_UFLOAT_PACK32` on Low) + sampled-capable depth, `composite` (tonemap) → swapchain, `ui` on top, with derived sync2 barriers, transient pooling and pass culling. `FRAMES_IN_FLIGHT = 2`. `QualityTier` exists (Low/Medium/High/Ultra). |
| Shaders | **R1b:** 9 GLSL sources sharing `#include` files (camera block, instance buffer, lighting, material set) plus `MATERIAL_TEXTURED` permutations and (R2a) two `SHADOW_PASS` depth-only casters: 14 SPIR-V modules validated with `spirv-val`. Every pipeline is built at load through a persistent `VkPipelineCache` (9 with shadows on). Hot reload of SPIR-V overrides in debug builds (`MOBA_SHADER_DIR`). |
| Lighting | **R2a** ([ADR-0011](../adr/0011-art-style-lighting.md)): stylised PBR (GGX, Smith-GGX, Schlick, energy-conserving diffuse) with wrap/ramp/painted-specular/rim set per map and material; per-map IBL (SH-L2 diffuse + prefiltered specular cube, split-sum LUT) from `env-bake`'s analytic skies; one fitted, texel-snapped directional shadow map with hard/PCF/PCSS-lite filtering by tier, skinned heroes casting. **R2b** ([ADR-0016](../adr/0016-lights-outlines-ao.md)): clustered forward point lights (ground-aligned 32×32 grid, per-tier caps) fed by effect `[light]` blocks, team/hover/target outlines (screen-space ID-buffer on High+, inverted hull on Low/Medium) in the colour-blind-capable team palette, baked top-down AO from `ao-bake` on the ambient line, and a depth pre-pass with screen-space contact shadows on High+. |
| Geometry | **R0:** `.glb` meshes with a procedural cube fallback (WP-3.2) and GPU-skinned heroes (WP-4.1); cubes remain only as fallbacks. **R8** ([ADR-0018](../adr/0018-culling-lods-memory.md)): CPU per-view frustum culling of units, shadow casters and terrain chunks; meshoptimizer LODs baked offline (`mesh-bake`) and picked by screen-space error; per-tier VRAM budgets. |
| Terrain | **R3a** ([ADR-0014](../adr/0014-terrain-fog-of-war.md)): a per-map heightfield mesh (board relief ≤ 0.5 m, hills or cliffs beyond the edge) with 1–8 splat-textured layers (texture arrays, triplanar on slopes, layer normal maps and cavity AO, baked relief AO), generated from the simulation's map by `moba_assets::generate_terrain`; `content-check` verifies the visuals against the simulation's map. Height fog and a sky close every view. **R3b** ([ADR-0017](../adr/0017-decals-foliage-water.md), 2026-10-30): ground decals binned in a 32×32 world grid (telegraphs with a guaranteed-contrast rim, scorch, splats), GPU-instanced wind-animated foliage pushed aside only by drawn units, and rivers and pools with flow, depth fade, foam and refraction on High; `content-check` keeps water wadeable. |
| Fog of war | **R3a:** a GPU visibility texture (`R8`, 128²–512² by tier) rasterised every frame from the own team's vision with line-of-sight walls, blurred and faded; no 16/32 caps; styled per map; also masks the minimap. |
| Materials and textures | **R1b** ([ADR-0010](../adr/0010-texture-pipeline.md)): client-local material descriptors (permutation + parameter block + base colour/normal/metallic-roughness/mask set), one descriptor set per material from a pool sized by the loaded content, KTX2 textures (BC7/BC5/BC4 with baked mips, CPU decode where BC is unsupported) from the `texture-bake` tool. One sample textured material (the static `sentinel`). R2a reads metallic/roughness and emissive, and normal maps use vertex tangents (glTF `TANGENT` or a MikkTSpace-style generator). |
| Animation | R4a ([ADR-0012](../adr/0012-animation-runtime.md), 2026-10-28): named clips, idle↔run blend space, crossfades, upper-body masks, additive hit reactions, a client-local content-defined state machine driven by snapshots and cast/death events (basic attacks inferred: no attack event on the wire), spring bones, sockets and a distance LOD; 200 units ≈ 0.12 ms CPU per frame. R4b (2026-10-29, [perf/0018](../perf/0018-vat-anim-lod.md)): VAT minions (baked by `vat-bake`, instanced, shadow-casting, cube fallback), tiered bone-count LOD, visible socket attachments, 2D blend spaces and the local preview/replays on the same runtime; 200 VAT minions ≈ 0.01 ms CPU per frame. Recall (2026-10-30, protocol v42, [perf/0024](../perf/0024-recall.md)): the recall loop from `EVENT_RECALL` and the recall flag, teleports snapped instead of interpolated. |
| VFX | **R5** ([ADR-0013](../adr/0013-vfx-runtime.md), 2026-10-29): compute-simulated GPU particles in a fixed per-tier buffer (sprites, flipbooks, trails, beams, ground rings, mesh particles), additive plus weighted blended OIT (unsorted on Low), soft particles from the scene depth, client-local effect descriptors bound to presentation events and animation sockets, per-tier particle/overdraw budgets with distance culling and enemy-telegraph/ally-dimming rules, and a live-reload preview. Distortion moved to R6. |
| Image quality | **R6** ([ADR-0015](../adr/0015-post-processing.md), 2026-10-29): AgX with per-map exposure and opt-in eye adaptation, per-map 3D colour-grading LUTs, a physically based 5-level bloom chain (Medium+), FXAA (Low–High) and MSAA 4× (Ultra), half-resolution GTAO from a depth prepass on the ambient terms (High+), render scale with dynamic resolution (Low) and a native-resolution HUD, and accessibility-toggleable death desaturation, damage vignette and low-health pulse. Heat-haze distortion particles (High+, [ADR-0019](../adr/0019-distortion.md), 2026-10-30): a refraction draw over a copy of the HDR target before the particles, capped per tier and resolution, depth-aware and masked off every unit. No TAA, SMAA or FSR yet. |
| UI | **R7a:** MSDF font (Open Sans Condensed, baked offline by `bake-ui-font`) with derivative anti-aliasing, outlines and soft shadows; the RGBA atlas keeps pixel-art icons. Styled health bars (damage ghost, shield, ticks, colour-blind palette), pooled floating combat text, cooldown sweeps, screen fades and a loading screen ([perf/0014](../perf/0014-ui.md)). **R7b:** render-to-texture through the graph (persistent offscreen images the UI samples): 3D hero portraits (animated idle above `Low`) and a minimap baked from a top-down render of the map at load. |
| Evidence | Fifteen lavapipe captures in `renderer-gate` with validation and sync validation (13 golden fingerprints, the distortion on/no-draw pair), plus the outline-occlusion and culling-exactness probes. Per-pass GPU timestamps in `MOBA_FRAME_LOG`; `perf-gate` checks p95/p99 and VRAM against `tests/perf/baseline.toml`. Real-GPU timings exist only for an RTX 5070 Ti ([perf/0011](../perf/0011-render-graph.md) to [perf/0023](../perf/0023-distortion.md)). |

**What helped from the start:**
- The simulation is deterministic, and **rendering is purely presentational**. Visuals can never desync a match, and fog-of-war visuals are cosmetic because the server already withholds unseen units.
- Presentation events give VFX and animation a clean trigger stream.
- Vulkan 1.3 with dynamic rendering and sync2 is a modern baseline.
- The engine has a strong evidence culture: golden images, validation-required captures and perf logs.

## 2. What "AAA" means for a MOBA

A MOBA is not an open-world shooter. The camera is high and angled down, the play space is fixed, and **readability beats realism**: players must parse 10 heroes, 100+ minions and dozens of overlapping abilities in a fraction of a second. The genre's best-looking games are **stylised PBR** — hand-painted or painterly textures on physically-based lighting — tuned for clarity, and they run on cheap laptops.

So the target is:

1. **Readability first.**
   - Silhouettes are distinct at gameplay zoom, and team identity is unmistakable (outline or rim, colour-blind-safe palettes).
   - Ability telegraphs and area-of-effect decals read clearly over any terrain.
   - VFX never hide enemies. Ally VFX can be dimmed, and enemy VFX are always legible.
2. **A rich, stylised world:**
   - textured, lit terrain with height, decals and foliage that sways and reacts to units;
   - water, props and atmosphere;
   - fog of war that looks authored, not computed.
3. **Characters that feel alive:**
   - skinned, animated heroes with blended locomotion, attack wind-ups synced to the simulation, and cast, death and recall animations;
   - secondary motion such as capes and hair.
4. **Spectacular but budgeted VFX:**
   - GPU particles, trails, mesh effects and distortion, all data-driven from content packs;
   - a hard per-frame budget that holds in a 10-hero teamfight.
5. **Polished image:**
   - HDR lighting, filmic tonemapping, bloom and colour grading;
   - anti-aliasing that keeps thin lines and text sharp;
   - stable frame pacing with no shader-compile hitches.
6. **Scales down:**
   - the same game is playable on the ADR-0002 min-spec (Intel UHD 620-class) at 1080p/60 on Low;
   - it reaches 144+ fps on mid-range GPUs on High.
7. **Production-ready pipeline:** artists can build, preview and hot-reload content without engineers, and content-check catches broken assets before a match does.

**The honest caveat:** points 1–7 are about 30 % engine work and **70 % art production**. The engine can be made AAA-*capable* by a small team. An AAA *look* needs an art director, character/environment/VFX/technical artists and animators. [§8](#8-art-production-the-long-pole) sizes that.

## 3. Principles and constraints

- **Never touch determinism.**
  - Animation, VFX and physics-for-looks run client-side from snapshots and presentation events. They never feed back into `moba-sim`.
  - Visual randomness uses client-only RNG streams.
  - Root motion is visual only: the simulation owns positions.
- **Network clients never see pack files.** Visuals are resolved by name against a client-local asset root, with procedural fallback (finish plan, Phase 3). New visual fields that affect the content hash follow the WP-0.1 parity-test pattern.
- **Every feature is tiered** (Low/Medium/High/Ultra). Low must hit min-spec. A feature without a Low path or an off switch doesn't merge.
- **Evidence per feature:** a validation-clean capture, a golden fingerprint where deterministic, and a GPU-time delta from timestamp queries on the reference GPUs ([§7](#7-performance-targets-and-evidence)).
- **No per-frame allocation** in the render loop (existing invariant). Resources are created at load or through transient render-graph pools.
- **Portability:** Linux, Windows and macOS (MoltenVK). Any feature that needs an extension ships with a fallback. Consoles are out of scope.
- **Decide architecture in ADRs** before writing code (render graph, bindless, texture formats, animation runtime).

## 4. Phases

Sizes are rough, for one experienced graphics engineer, unless noted: S ≤ 2 weeks, M ≈ 1 month, L ≈ 2–3 months, XL > 3 months. Phases overlap. The critical path is R0 → R1 → R2/R3 → R4, with art production (§8) running alongside from R1.

### R0 — Finish the asset foundation (already planned)
**Status:** done in the finish plan: WP-3.1 (#317), WP-3.2 (#309), WP-3.3 (#319), WP-4.1
(#316) and WP-4.2 (#318).

The finish plan's Phases 3–4 are the first step of this roadmap. Don't duplicate them:
- WP-3.1 packs reference assets (#285);
- WP-3.2 `.glb` mesh loading with fallback (#286);
- WP-3.3 per-map visuals (#287);
- WP-4.1 GPU skinning (#288);
- WP-4.2 networked animated golden and frame cost (#289).

**Exit:** a skinned, animated hero model and a custom-styled map render in a networked match, with validation-clean evidence.

### R1 — Renderer architecture (L)
Today's single pass can't hold shadows, post-processing or VFX. Rebuild the frame around:
1. **Render graph** (ADR-0008). Passes declare their reads and writes; the graph derives sync2 barriers, transient image aliasing and pass culling. Keep it small and hand-written (no external crate) and test it headless.
2. **HDR offscreen targets.** Colour `R16G16B16A16_SFLOAT` (or `B10G11R11_UFLOAT` on Low), depth with sampling, and a final tonemap/composite pass to the swapchain.
3. **Descriptor model** (ADR-0009): move to **bindless textures and buffers** (descriptor indexing, core-optional in 1.2+ and supported by MoltenVK through argument buffers), or keep plain sets with per-material sets. Prototype both and measure on min-spec and MoltenVK. ADR-0002 currently says "plain sets", so this is an explicit amendment.
4. **Material system.** Material = shader permutation + parameter block + texture set, defined in content and hashed like everything else. Keep the permutation count small and enumerated so pipelines can be precompiled.
5. **Pipeline cache and precompilation.** Persist `VkPipelineCache`, build all permutations at load, and never compile mid-match.
6. **GPU profiling.** Timestamp queries per pass, with a stats overlay and `MOBA_FRAME_LOG` columns per pass. Optional Tracy GPU zones.
7. **Shader tooling.** Include files, permutation macros, hot reload in dev builds, and `spirv-val` in CI (already present).

**Status (2026-10-28):** done. R1a (#323) delivered items 1–3 and 6
([ADR-0008](../adr/0008-render-graph.md), [ADR-0009](../adr/0009-descriptor-model.md),
[perf/0011](../perf/0011-render-graph.md)). R1b delivered items 4, 5 and 7
([ADR-0010](../adr/0010-texture-pipeline.md), [perf/0013](../perf/0013-materials.md)) with
two deliberate scope choices: materials are **client-local and not hashed** (like map visuals
and animation sets, so no wire or replay change), not "defined in content and hashed"; and
the GPU timings are in `MOBA_FRAME_LOG` but not yet in the HUD's stats line (the HUD is
R7's area). The baker's BC7 encoder is single-mode (ADR-0010 follow-up). The exit's ≤ 5 %
criterion is discussed in perf/0011; the maintainer resolved it on 2026-10-30 as graph
overhead for the same work, which is met (see [Completion status](#completion-status-2026-10-30)).

**Exit:**
- The existing scene renders through the graph at ≤ 5 % GPU-time overhead versus today (the graph's own overhead for the same work: maintainer decision, 2026-10-30).
- Validation and sync validation are clean.
- The golden fingerprint is rebaselined once, deliberately, from CI.

### R2 — Lighting and materials (L)
1. **PBR shading.** GGX specular, metallic/roughness, normal maps, with an energy-conserving diffuse term. Expose a **stylisation layer** (wrap lighting, ramp textures, painted-specular tint) so art direction can push it painterly.
2. **Image-based lighting.** Per-map prefiltered environment cubemap plus spherical-harmonics ambient, baked offline by a `moba-tools` baker.
3. **Shadows.** The MOBA camera allows one tightly fitted directional shadow map (no cascades needed at fixed zoom; add a second cascade only if spectator zoom requires it). Use PCF on Low/Medium and PCSS-lite on High. Add screen-space contact shadows on High.
4. **Baked lighting for the static map.** Lightmaps or baked AO for terrain and props, so Low-tier maps still look lit.
5. **Many dynamic lights.** Use clustered forward shading for ability lights (fireballs, auras), capped per cluster with a per-tier global cap.
6. **Team readability.** Per-team rim/outline (screen-space outline from a stencil or ID buffer on High, inverted-hull on Low), hover/selection outlines, and colour-blind palettes in settings.

**Exit:** a lit, textured, shadowed hero-and-map scene at the target budgets on all three reference GPUs, and a readability review signed off by art direction.

**Status (2026-10-29):** R2a done — items 1–3 ([ADR-0011](../adr/0011-art-style-lighting.md),
[perf/0015](../perf/0015-lighting.md)): PBR with the stylisation layer and vertex tangents,
per-map IBL baked by `moba-tools env-bake`, and one fitted directional shadow map (no
cascades: the sphere fit gives 4–9 cm texels at the player cameras and stays readable at the
overview) with hard/PCF/PCSS-lite by tier and off switches. Scope choices: screen-space
contact shadows (item 3) need a depth pre-pass and move to R2b; BC6H for environments is
deferred (ADR-0010 amendment). **R2b done (2026-10-29)** — items 3 (contact shadows), 4, 5
and 6 ([ADR-0016](../adr/0016-lights-outlines-ao.md), [perf/0020](../perf/0020-lights-outlines-ao.md)):
clustered forward dynamic lights on a ground-aligned grid with per-tier caps, driven by
effect `[light]` blocks (enemy lights first); team, hover and attack-target outlines in the
HUD team palette (colour-blind included), screen-space from a pre-pass ID target on High+
and an inverted hull on Low/Medium, depth-tested so they never show hidden units; baked
top-down AO for walls and props (`moba-tools ao-bake`, fingerprinted against the bound map)
on the ambient line, composable with R6's SSAO; and a depth pre-pass with screen-space
contact shadows on High+, reusable by R6's GTAO. Lightmaps were not needed (one sun, shadows
on every tier). **R2 is done** as engineering work. The exit's "all three reference GPUs"
and the **art-direction readability sign-off** are deferred to pre-1.0 by the maintainer: the
RTX 5070 Ti is the only measured GPU, and the review evidence (captures per tier, palette
checks) is in perf/0020 for an art director to sign.

### R3 — Terrain and environment (L)
1. **Terrain.** Heightfield or authored mesh with splat-map texturing (4–8 layers, triplanar on slopes), plus normal and AO maps. Walls and cliffs are props from the asset manifest. The simulation's map geometry stays authoritative and is validated against the visual mesh by content-check.
2. **Decals.** Projected decals for ability telegraphs, scorch marks and blood/goo, with clustered or deferred-decal binning. Telegraph decals get guaranteed contrast.
3. **Foliage and props.** GPU-instanced and wind-animated. **Brush that reacts to hidden units** (a MOBA staple) is driven only by entities the client can see, so it never leaks hidden information.
4. **Water and rivers.** Flow maps, depth fade, shoreline foam and cheap screen-space refraction on High.
5. **Fog of war v2.** Replace the 16-circle UBO loop with a **visibility texture**: the client rasterises vision from its visible units and the LOS walls each tick into a low-res R8 texture, then blurs and fades it over time. That is presentation only; the server keeps filtering units. This removes the 16/32 caps and lets art style the fog (desaturate, height fog, clouds).
6. **Atmosphere.** Height fog, simple sky/backdrop and map-edge treatment (no visible board edge).

**Exit:**
- One fully art-directed map renders at budget on all tiers.
- The fog-of-war texture handles 10 heroes, wards and structures.
- Golden images cover the map at several camera positions.

**Status (2026-10-29):** R3a done — items 1, 5 and 6 ([ADR-0014](../adr/0014-terrain-fog-of-war.md),
[perf/0017](../perf/0017-terrain-fow.md)):
- **Terrain:** a heightfield mesh per map with 1–8 splat layers (two texture arrays and a
  two-layer splat array, so three samplers whatever the layer count), triplanar on slopes,
  layer normal maps and cavity AO plus baked relief AO; the procedural generator in
  `moba-assets` makes the sample maps' heightfields and splats from the simulation's map, and
  their layers come from `texture-bake generate terrain`. Walls stay props along every
  simulation wall; cliffs can be props (`edge_mesh`). `content-check` enforces the
  visual-versus-simulation rules (board relief ≤ 0.5 m, rising edges, covered walls ≥ 2 m,
  props off the lanes as warnings).
- **Fog of war v2:** a GPU visibility texture from the own team's vision only (unit-tested:
  enemies in the snapshot never become sources), with line-of-sight walls, a blur and a
  reveal/fade; no 16/32 caps (a 158-source test with 10 heroes, wards and structures); it
  also masks the minimap. The legacy loop stays behind `MOBA_FOW=legacy`.
- **Atmosphere:** analytic height fog, a far-plane sky (environment cube or gradient) and
  hills/cliffs beyond the board, so no board edge shows from the player, overview or low
  spectator cameras.
- Every feature is tiered with an off switch (`MOBA_TERRAIN=flat`, `MOBA_FOW=legacy|off`,
  `MOBA_FOG=off`, `MOBA_SKY=off`). Goldens now cover the map from four cameras.
- **R3b done (2026-10-30)** — items 2, 3 and 4 and the exit
  ([ADR-0017](../adr/0017-decals-foliage-water.md),
  [perf/0021](../perf/0021-decals-foliage-water.md)):
  - **Decals:** projected in the terrain shader from a 32×32 world-grid binning of up to 128
    decals a frame (a set-3 storage buffer, no sampler, no extra pass): telegraph rings and
    discs with a progress sweep, scorch and splats stamped on impacts and deaths, from
    client-local `[[decals]]` in the effect descriptors. Enemy telegraphs are listed first,
    never dropped, keep R5's 35 % floor and get a two-tone rim whose contrast a unit test
    holds over any ground. They augment R5's particle rings.
  - **Foliage:** GPU-instanced procedural grass, brush, reeds and flowers from a per-map
    `[foliage]` table (`generate_foliage` scatters all three sample maps), wind-animated and pushed
    aside by the units the client draws only (a test: hidden units produce no displacement).
    Decorative: the simulation has no brush.
  - **Water:** `[[water]]` rivers and pools carved into the heightfield, with flow-map normals,
    depth fade, shoreline foam and (High and up) screen-space refraction; `content-check`
    rejects water a unit could not visibly wade through (the simulation lets every unit walk
    everywhere) and warns about water near structures and brush over lanes.
  - Tiered with switches (`MOBA_DECALS=off`, `MOBA_FOLIAGE=off`,
    `MOBA_WATER=off|simple|full`). The grass map is the showcase; goldens cover it from seven
    cameras (default, networked, overview, horizon, the river close up on High and Low, and
    the decal teamfight) plus the VAT, Ultra and Low-scale captures.
  - **Exit:** the showcase renders well inside every tier's budget (perf/0021). Art sign-off
    of "one fully art-directed map" is deferred to pre-1.0 by the maintainer: the sample maps
    are engine-authored.
- **Wards** do not exist in the simulation yet; the texture takes any source, and the test
  uses ward-like sources.

### R4 — Animation runtime (M–L, after WP-4.1)
**Status:** R4a (items 1–4, with a 1D blend space, and the update-rate half of 5) is done in
[ADR-0012](../adr/0012-animation-runtime.md). R4b-1 replicated basic attacks (`EVENT_ATTACK`,
protocol v41), so attack wind-up and release follow the simulation's attack timer and the hit
frame lands on the hit tick. **R4b is done** (2026-10-29, ADR-0012 amendment,
[perf/0018](../perf/0018-vat-anim-lod.md)): VAT minions, bone-count LOD, visible socket
attachments, 2D blend spaces and local preview/replay animation. 200 VAT minions cost
≈ 0.01 ms of CPU per frame against a 0.5 ms budget. **Recall exists and the exit is met**
(2026-10-30, protocol v42, simulation logic v9, ADR-0012 recall amendment,
[perf/0024](../perf/0024-recall.md)): in a networked match through a 150 ms RTT / 30 ms jitter
/ 5 % loss link, on lavapipe, the local skinned hero played idle, run, three casts, recall
(two cancels and a completed recall that snapped it to its spawn), a basic attack and death,
and the other nine played attacks, casts and deaths, in the order the server ran them.

1. **Clips and blending.** Multiple clips per hero, 1D/2D blend spaces for locomotion, crossfades, additive hit reactions and an upper/lower-body mask (cast while moving).
2. **Animation state machine** defined in content, driven by snapshot state and presentation events:
   - idle and run come from interpolated velocity;
   - attack wind-up and release are timed to the simulation's attack timers so the hit frame lines up;
   - casts are keyed to ability ids, and deaths and respawns to their events.
3. **Secondary motion.** Spring bones for capes, hair and tails (client-only, fixed timestep).
4. **Attachments.** Sockets for weapons and VFX, and item visuals if desired.
5. **LOD.** Reduce bone counts and the animation update rate for distant or off-screen units; animate minions with vertex-animation textures (VAT) so hundreds stay cheap.

**Exit:** a hero with idle, run, attack, three casts, death and recall plays correctly in a networked match at 150 ms RTT. A 200-minion stress test stays within the animation CPU budget.

### R5 — VFX system (L)
1. **GPU particles.** Compute-simulated emitters (spawn, update, sort-free additive or alpha with weighted blended OIT), with soft particles from scene depth.
2. **Primitive types:** sprites, flipbooks, ribbons/trails (projectiles, dashes), mesh particles and beams. Add distortion/heat haze on High.
3. **Data-driven effects.** Abilities reference VFX by name in content packs, resolved client-side with fallback, like meshes. Effects bind to presentation events and sockets.
4. **Budgets and LOD.** Global particle and overdraw budgets per tier, distance and importance culling, and **enemy-VFX legibility rules** (minimum alpha on enemy telegraphs, allied VFX dimming option).
5. **Authoring.** A live-reload effect preview tool (`moba-tools vfx-preview`) so artists iterate without a match.

**Exit:** a scripted 5v5 teamfight replay (deterministic, from a checked-in `.mbr`) renders at budget on every tier with all ability VFX. The fill-rate p95 is recorded.

**Status (2026-10-29):** done; distortion moved to R6 ([ADR-0013](../adr/0013-vfx-runtime.md),
[perf/0016](../perf/0016-vfx.md)). Items 1, 3, 4 as written; item 2 without distortion/heat
haze, which **moved to R6** (item 7 there); item 5 as a client mode (`MOBA_VFX_PREVIEW=<effect>`, reloads on save)
rather than a separate tool. Effects are client-local and unhashed like every other visual,
bound by name in `vfx/bindings.toml`. The exit scene is `tests/replays/teamfight-5v5.mbr`
(`moba-tools teamfight`, all 15 built-in abilities cast, verified by a test), played with
`MOBA_REPLAY` and measured on every tier on the RTX 5070 Ti; the other §7 reference GPUs are
deferred to pre-1.0 by the maintainer (#9/#127).

### R6 — Post-processing and image quality (M)
1. **Tonemapping** (AgX or ACES-fitted) with exposure, and per-map colour-grading LUTs.
2. **Bloom.** A physically-based downsample/upsample chain.
3. **Anti-aliasing.** MSAA 4× on the forward HDR pass where affordable, or SMAA 1×. Treat TAA as optional and High-only, because ghosting harms readability on fast units. Keep UI and text unaffected.
4. **Ambient occlusion.** GTAO/SSAO at half resolution on High; Low relies on baked AO.
5. **Resolution scaling.** Render-scale slider and dynamic resolution to hold the frame target. Optional AMD FSR 1/2 (MIT licence) upscaling.
6. **Effects for gameplay:** death desaturation, damage vignette and a low-health pulse, all accessibility-toggleable.
7. **Distortion / heat haze** (moved from R5, High and up, `MOBA_DISTORTION=off` switch): a copy of the HDR target before the particle pass, sampled with a refraction offset by distortion particles ([ADR-0013](../adr/0013-vfx-runtime.md#follow-ups)).

**Exit:** reference captures per tier, and a blind A/B readability check against the R2 build.

**Status (2026-10-29):** items 1–6 done ([ADR-0015](../adr/0015-post-processing.md),
[perf/0019](../perf/0019-post.md)): every feature tiered with an off switch, validation and sync
validation clean on lavapipe and the RTX 5070 Ti, per-pass GPU timings before/after on the R5
teamfight replay inside every §7 budget, reference captures per tier and the blind A/B image
set with its protocol in perf/0019. The A/B **sign-off is deferred to pre-1.0 by the
maintainer**. Item 7 done 2026-10-30 ([ADR-0019](../adr/0019-distortion.md), [perf/0023](../perf/0023-distortion.md)):
distortion particles (`blend = "distortion"`) refract a copy of the HDR target taken after the
water and before the particles, on High (6 px at 1080p) and Ultra (8 px), off on Low, Medium,
`MOBA_DISTORTION=off` and portability-subset devices; units are masked through the prepass ID
target and telegraph decals' bands through the scene's alpha, nearer geometry is never pulled
in, and the particles (telegraph rings), outlines and the UI draw after it. Heat haze ships on `cinder-zone`, `ember-curse` and the Ember map's crystals. Not done:
TAA (optional; not started for the readability reason above), and SMAA and FSR (ADR-0015 §4,
§6), which are alternatives to the MSAA and render scale that shipped. Low's AO is R2b's baked
AO (done).

### R7 — UI, HUD and presentation polish (M)
**Status:** done. R7a (2026-10-28) and R7b (2026-10-29), [perf/0014](../perf/0014-ui.md).
- Signed-distance-field or MSDF text and icons, so they stay crisp at any UI scale or 4K. Keep the atlas for pixel-art elements. **R7a: text done** (MTSDF atlas, outline and shadow styles); icons stay pixel art.
- 3D hero portraits rendered to texture, and an animated minimap from a baked top-down render of the map. **R7b: done.** Offscreen images are persistent render-graph imports with their own passes in the frames that re-render them; the UI samples them (`UiVertex::image`). Portraits: one cell per hero slot, idle-animated at 5–10 Hz (static on `Low`). Minimap: baked once per map bind; markers and vision darkening are drawn over it each frame from client data.
- Floating combat text, styled health bars with shield/damage-ghost segments, and cooldown sweeps — all batched, with no per-frame allocation. **R7a: done** (derived from snapshot health deltas; shield size and crit flags are not on the wire).
- Screen transitions for lobby, champion select and results; a stylised loading screen. **R7a: done** for loading, lobby, match and results (there is no champion select yet).

### R8 — Performance, scaling and platform evidence (M, continuous)
- **Culling.** GPU-driven frustum culling with indirect draws (`vkCmdDrawIndexedIndirectCount`, core in 1.2) once instance counts justify it. Mesh LODs from `meshoptimizer` (MIT).
- **Texture formats.** BC7/BC5 on desktop, with KTX2 containers produced by the asset baker. Optional Basis Universal transcoding (Apache-2.0) if the download size matters.
- **Memory.** VRAM budgets per tier, with streaming of hero and skin assets at champion select (load, never mid-match).
- **Hardware matrix.** Owned or lab hardware is mandatory: hosted CI has no real GPU on Windows/macOS ([#9](https://github.com/JakeGT/moba-engine/issues/9)/[#127](https://github.com/JakeGT/moba-engine/issues/127)). Use the three reference machines in §7. A nightly perf job on at least one self-hosted GPU runner fails on budget regressions.
- **Frame pacing.** No compile or allocation hitches. p99 within 1.5× p50 over a 10-minute replay.

**Status (2026-10-30):** engineering done ([ADR-0018](../adr/0018-culling-lods-memory.md),
[perf/0022](../perf/0022-r8-scaling-pacing.md)). Culling runs on the CPU per view: unit spheres
against the camera frustum, a separate shadow caster list, and 32×32-cell terrain chunks
(`MOBA_GPU_CULL=off` switches it off). It is checked byte-identical to unculled in five golden
scenes. GPU-driven culling is designed but deferred: at ≈ 700 instances the CPU path costs
microseconds, and ADR-0018 §2 sets the triggers for building it. meshoptimizer LODs are baked
offline (`mesh-bake`), picked by screen-space error per tier, with `MOBA_MESH_LOD=off|<px>`;
they combine with the R4b bone LOD. Per-tier VRAM and texture budgets are accounted in the
frame log and at startup; over-budget textures drop mips instead of failing. Loads happen
only at load time (`locked_uploads=0`, `locked_pipelines=0` in every run), and content-check
reports per-tier fits. The 10-minute replay holds p99 ≤ 1.18 × p50 on every tier on the RTX
5070 Ti (re-run on `main` after R3b and distortion, which refreshed `tests/perf/baseline.toml`).
`nightly-perf.yml` gates it on a `[self-hosted, gpu]` runner once
`HAS_GPU_RUNNER=true`. Texture formats (BC7/BC5 in KTX2) shipped in R1b; Basis transcoding is
not needed yet. Not done: hero and skin streaming at champion select waits for a
champion-select flow (today everything loads before the match). The reference-machine
matrix (§7) and the self-hosted runner are **deferred to pre-1.0 by the maintainer** (#9/#127);
perf/0022 has the checklist.

## 5. Sequencing at a glance

```text
R0 assets/skinning (finish plan) ──► R1 render graph + HDR + materials ──► R2 lighting ──► R6 post
                                   │                                    └─► R3 terrain/env + FoW v2
                                   ├─► R4 animation runtime (needs WP-4.1)
                                   └─► R5 VFX (needs R1 targets; R3 depth for soft particles)
R7 UI polish: any time after R1          R8 perf/evidence: continuous, gate on every phase
Art production (§8): style guide during R1; first hero + map during R2–R4; scale up after R5
```

Indicative engineering timeline, with 2–3 graphics/engine engineers: R0 ~2 months (in flight), R1 ~3 months, R2–R5 ~6–9 months in parallel, R6–R8 ~3 months, so **12–18 months** to an AAA-capable renderer. Art runs alongside and decides when the game *looks* AAA (§8).

## 6. Decisions needed (ADRs)

| # | Decision | Recommended default |
|---|---|---|
| ADR-0007 | Mesh format | `.glb` via `gltf` (being written in WP-3.2) |
| ADR-0008 | Render graph | **Accepted** (R1a): in-house, small, compile-time pass list; transient pooling |
| ADR-0009 | Descriptor model | **Accepted** (R1a): per-material plain sets now; bindless deferred until measured on MoltenVK + UHD 620 |
| ADR-0010 | Texture pipeline | **Accepted** (R1b, [ADR-0010](../adr/0010-texture-pipeline.md)): offline baker → KTX2 (BC7/BC5/BC4), baked mips, sRGB/linear fixed by format and slot; CPU-decode fallback; client-local material descriptors |
| ADR-0011 | Art style | **Accepted** (R2a, [ADR-0011](../adr/0011-art-style-lighting.md)): stylised PBR (GGX + wrap/ramp/painted specular/rim), per-map IBL, one fitted directional shadow map; readability rules as acceptance criteria |
| ADR-0012 | Animation runtime | **Accepted** ([ADR-0012](../adr/0012-animation-runtime.md)): in-house sampler + content-defined state machine; VAT for minions in R4b |
| ADR-0016 | Lights, outlines, baked AO | **Accepted** (R2b, [ADR-0016](../adr/0016-lights-outlines-ao.md)): clustered forward on a ground-aligned column grid with CPU binning; screen-space ID-buffer outlines (High+) and inverted hull (Low/Medium); baked height-field AO on the ambient line; a depth pre-pass shared by contact shadows and R6's GTAO |
| ADR-0013 | VFX runtime | **Accepted** (R5, [ADR-0013](../adr/0013-vfx-runtime.md)): GPU compute particles over a fixed per-tier buffer with a CPU slot allocator; WBOIT with log revealage (no `independentBlend`); client-local effects bound to presentation events and sockets |
| ADR-0014 | Terrain and fog of war | **Accepted** (R3a, [ADR-0014](../adr/0014-terrain-fog-of-war.md)): a generated heightfield with splat-textured layers, a GPU fog-of-war visibility texture from own-team vision, height fog and a sky; visual-vs-simulation rules in content-check |
| ADR-0015 | Post-processing | **Accepted** (R6, [ADR-0015](../adr/0015-post-processing.md)): post chain fixed per renderer; grading after AgX on display-referred `.cube` LUTs; FXAA below Ultra, MSAA 4× on Ultra (single-sample VFX after the resolve); GTAO from a depth prepass on ambient only; sub-rectangle dynamic resolution |
| ADR-0017 | Decals, foliage and water | **Accepted** (R3b, [ADR-0017](../adr/0017-decals-foliage-water.md)): decals projected in the receiving shaders from a world-grid binned storage buffer (no sampler, no pass), enemy telegraphs first with a two-tone contrast rim; procedural instanced foliage displaced only by drawn units; water carved into the heightfield, simple in the scene pass or full with a copy-based refraction pass |
| ADR-0019 | Distortion / heat haze | **Accepted** (R6 item 7, [ADR-0019](../adr/0019-distortion.md)): a particle blend mode drawn by its own pass as a direct refraction of a copy taken after the water and before the particles (no offset target); per-tier pixel cap scaled by render height; unit mask from the prepass ID target; High+ only |
| — | Quality tiers | Low = min-spec 1080p/60; High = mid-range 1440p/144 |

## 7. Performance targets and evidence

| Tier | Reference GPU (class) | Resolution | Target | GPU budget |
|---|---|---|---|---|
| Low | Intel UHD 620 / Apple M1 (MoltenVK) | 1080p (render-scale allowed) | 60 fps p95 | ≤ 14 ms |
| Medium | GTX 1060 / RX 580 class | 1080p | 144 fps p95 | ≤ 6.0 ms |
| High | RTX 3060 class | 1440p | 144 fps p95 | ≤ 6.0 ms |
| Ultra | RTX 4070+ class | 4K | 120 fps p95 | ≤ 7.5 ms |

Measured on a deterministic replay of a late-game 5v5 teamfight with all VFX (R5 exit scene).
Evidence goes in `docs/perf/` with GPU, driver, OS and per-pass timings, as in
[perf/0007](../perf/0007-real-hardware.md). Estimates never close a target.

## 8. Art production (the long pole)

The engine work above makes AAA visuals *possible*. Producing them takes a team and budget:

- **Roles:**
  - an art director, to own the style guide and readability rules;
  - character artists, riggers and animators;
  - environment artists;
  - VFX artists;
  - 1–2 technical artists, who bridge DCC tools, the baker, shaders and budgets;
  - a UI artist.
- **Per hero:** concept, high/low-poly model, textures, rig, roughly 15–25 animations, VFX for every ability, icons and a portrait. A launch roster of 20+ heroes is the biggest single cost.
- **Per map:** the terrain set, prop kit, foliage, water, lighting and grading, plus a fog-of-war style.
- **Pipeline deliverables (engineering):**
  - a Blender export preset and validation (`content-check` reports polycount, bones, textures and budgets per tier);
  - hot reload of meshes, materials and VFX;
  - an asset preview tool;
  - an asset licensing record in `THIRD_PARTY.md`.
- **Vertical-slice milestone:** one art-complete hero and one art-complete map section at Ultra *and* Low. This is the go/no-go point before scaling the roster.

Until an art team exists, use CC0 or permissively licensed sample assets for engine development. Never ship them as the game's identity.

## 9. Other tracks a shippable game needs

These are out of scope here; each needs its own plan:
- audio (spatial mixing, music systems);
- accounts, matchmaking and ranked play;
- anti-cheat (server authority already covers most of it);
- live operations and telemetry;
- localisation and accessibility beyond visuals;
- monetisation;
- a launcher and patching.

## 10. Next concrete steps

The original list (land R0, write ADR-0008/0009, adopt a style guide, open phase issues) is
done or superseded. What remains is the maintainer list in
[Completion status](#completion-status-2026-10-30):

1. Acquire or borrow the §7 reference hardware and register one self-hosted GPU runner
   (`HAS_GPU_RUNNER=true`), then run `scripts/perf-replay.sh` on each and refresh
   `tests/perf/baseline.toml` (#9/#127).
2. Hire or commission the art direction and production in §8, and sign off R2's readability
   review, R3's art-directed map and R6's blind A/B against the first art-complete hero and map
   (the §8 vertical slice).
