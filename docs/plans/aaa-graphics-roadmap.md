# Roadmap: from preview renderer to AAA-quality MOBA visuals

Written 2026-10-27. Status: **proposal** — no work package below is started unless it is also in
[finish-plan-2026-10-20.md](finish-plan-2026-10-20.md). Owner: maintainer.

This plan covers **graphics only**: the renderer, the asset and animation pipeline, VFX, art
production and the performance evidence a commercial release needs. Gameplay, matchmaking,
accounts and live operations are separate tracks (see [§9](#9-other-tracks-a-shippable-game-needs)).

---

## 1. Where we are

The engine has a solid base to build on, but today's picture is a prototype. Compare
[the NVIDIA teamfight capture](../images/m8-nvidia-teamfight.png) with any commercial MOBA.

| Area | Today (checked against `main` @ 9a251a1; R1a rows updated 2026-10-28) |
|---|---|
| Frame structure | **R1a:** a render graph ([ADR-0008](../adr/0008-render-graph.md)): `scene` → HDR colour (`R16G16B16A16_SFLOAT`, `B10G11R11_UFLOAT_PACK32` on Low) + sampled-capable depth, `composite` (tonemap) → swapchain, `ui` on top, with derived sync2 barriers, transient pooling and pass culling. `FRAMES_IN_FLIGHT = 2`. `QualityTier` exists (Low/Medium/High/Ultra). |
| Shaders | 9 GLSL files (`crates/shaders/shaders/`), adding the fullscreen vertex shader and the composite. |
| Lighting | One directional light (Lambert), hemispheric ambient and a rim term (`unit.frag`). No shadows, no specular, no PBR, no image-based lighting. |
| Geometry | Instanced cubes. Heroes are a 6-cube rig posed on the CPU (`push_hero_rig`). The instance SSBO is capped at 4,096. Mesh loading is in progress (WP-3.2, #286). |
| Terrain | A flat plane with a procedural grid and per-map tint. Walls are only drawn on the network path. |
| Fog of war | The terrain shader loops over 16 vision circles and 32 wall segments held in the camera UBO. |
| Materials and textures | Vertex colour only. Textures exist only in the UI atlas. |
| Animation | R4a ([ADR-0012](../adr/0012-animation-runtime.md), 2026-10-28): named clips, idle↔run blend space, crossfades, upper-body masks, additive hit reactions, a client-local content-defined state machine driven by snapshots and cast/death events (basic attacks inferred: no attack event on the wire), spring bones, sockets and a distance LOD; 200 units ≈ 0.12 ms CPU per frame. VAT minions and bone LOD (R4b) are open. |
| VFX | None beyond unit tinting. Presentation events (cast, projectile spawn/hit, death, zone) already reach the client ([perf/0009](../perf/0009-presentation-events.md)). |
| Image quality | **R1a:** exposure + AgX tonemap (with a saturation look) in the composite pass. No anti-aliasing, bloom or colour grading. |
| UI | Baked bitmap font and RGBA atlas, with UI scale. |
| Evidence | Golden 64×36 fingerprints on lavapipe with validation (rebaselined for the tonemap in R1a). Per-pass GPU timestamps in `MOBA_FRAME_LOG` (R1a); real-GPU timings exist only for an RTX 5070 Ti (p95 4.2 ms @1080p, GPU work 0.046 ms, [perf/0011](../perf/0011-render-graph.md)). |

**What already helps:**
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

**Exit:**
- The existing scene renders through the graph at ≤ 5 % GPU-time overhead versus today.
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

### R4 — Animation runtime (M–L, after WP-4.1)
**Status:** R4a (items 1–4, with a 1D blend space, and the update-rate half of 5) is done in
[ADR-0012](../adr/0012-animation-runtime.md); exact attack sync needs a replicated attack event
(a protocol follow-up). R4b — VAT minions and renderer bone LOD — waits for R1.

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

### R6 — Post-processing and image quality (M)
1. **Tonemapping** (AgX or ACES-fitted) with exposure, and per-map colour-grading LUTs.
2. **Bloom.** A physically-based downsample/upsample chain.
3. **Anti-aliasing.** MSAA 4× on the forward HDR pass where affordable, or SMAA 1×. Treat TAA as optional and High-only, because ghosting harms readability on fast units. Keep UI and text unaffected.
4. **Ambient occlusion.** GTAO/SSAO at half resolution on High; Low relies on baked AO.
5. **Resolution scaling.** Render-scale slider and dynamic resolution to hold the frame target. Optional AMD FSR 1/2 (MIT licence) upscaling.
6. **Effects for gameplay:** death desaturation, damage vignette and a low-health pulse, all accessibility-toggleable.

**Exit:** reference captures per tier, and a blind A/B readability check against the R2 build.

### R7 — UI, HUD and presentation polish (M)
- Signed-distance-field or MSDF text and icons, so they stay crisp at any UI scale or 4K. Keep the atlas for pixel-art elements.
- 3D hero portraits rendered to texture, and an animated minimap from a baked top-down render of the map.
- Floating combat text, styled health bars with shield/damage-ghost segments, and cooldown sweeps — all batched, with no per-frame allocation.
- Screen transitions for lobby, champion select and results; a stylised loading screen.

### R8 — Performance, scaling and platform evidence (M, continuous)
- **Culling.** GPU-driven frustum culling with indirect draws (`vkCmdDrawIndexedIndirectCount`, core in 1.2) once instance counts justify it. Mesh LODs from `meshoptimizer` (MIT).
- **Texture formats.** BC7/BC5 on desktop, with KTX2 containers produced by the asset baker. Optional Basis Universal transcoding (Apache-2.0) if the download size matters.
- **Memory.** VRAM budgets per tier, with streaming of hero and skin assets at champion select (load, never mid-match).
- **Hardware matrix.** Owned or lab hardware is mandatory: hosted CI has no real GPU on Windows/macOS ([#9](https://github.com/JakeGT/moba-engine/issues/9)/[#127](https://github.com/JakeGT/moba-engine/issues/127)). Use the three reference machines in §7. A nightly perf job on at least one self-hosted GPU runner fails on budget regressions.
- **Frame pacing.** No compile or allocation hitches. p99 within 1.5× p50 over a 10-minute replay.

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
| ADR-0010 | Texture pipeline | Offline baker → KTX2 (BC7/BC5/BC4), mips, sRGB rules |
| ADR-0011 | Art style | Stylised PBR with a painterly layer; readability rules as acceptance criteria |
| ADR-0012 | Animation runtime | **Accepted** ([ADR-0012](../adr/0012-animation-runtime.md)): in-house sampler + content-defined state machine; VAT for minions in R4b |
| ADR-0013 | VFX runtime | GPU compute particles; content-defined effects bound to presentation events |
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

1. Land R0 (finish-plan Phases 3–4), which is in progress.
2. Write ADR-0008 (render graph) and ADR-0009 (descriptor model) with a small spike that measures both descriptor options on MoltenVK and a UHD 620-class device.
3. Acquire or borrow the §7 reference hardware, and set up one self-hosted GPU runner for nightly perf.
4. Commission or adopt an art style guide (ADR-0011) before R2 starts, so lighting is tuned to the target look.
5. Open one tracking issue per phase (R1–R8), linked to this document.
