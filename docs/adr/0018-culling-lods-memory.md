# ADR-0018: Culling, mesh LODs, memory budgets and frame pacing (R8)

**Status:** accepted 2026-10-30, revised the same day after implementation (§2: the
GPU-driven path is deferred, not built; §3: attribute-weighted permissive simplification) (AAA graphics roadmap
[R8](../plans/aaa-graphics-roadmap.md#r8--performance-scaling-and-platform-evidence-m-continuous),
the last engineering phase). Builds on [ADR-0008](0008-render-graph.md) (the graph),
[ADR-0011](0011-art-style-lighting.md) (the fitted shadow map), [ADR-0012](0012-animation-runtime.md)
(bone LOD, VAT minions), [ADR-0014](0014-terrain-fog-of-war.md) (the heightfield) and
[ADR-0015](0015-post-processing.md) (render scale). Written before the code; the numbers in
[Consequences](#consequences) come from [perf/0022](../perf/0022-r8-scaling-pacing.md).

## Context
Every phase so far drew everything it was given. The R3a review noted that the Ultra
heightfield (385² samples: 148 k vertices, about 295 k triangles) is drawn whole by the
prepass, the scene and the minimap bake, although the default 46 m camera sees a fraction of
it, and the shadow pass draws every unit of the main view whether or not it can cast into the
fitted shadow box. Roadmap R8 asks for culling, mesh LODs, VRAM budgets with load-time-only
streaming, pacing evidence (p99 within 1.5× p50 over a 10-minute replay) and a nightly perf
job that fails on budget regressions.

Constraints: presentation only (no wire, replay, simulation or content-hash change: protocol
v41, replay v5, logic v8), every feature tiered with an off switch, nothing allocated per
frame, and one rule specific to culling: **culling must never change what a player sees.**

### What the instance counts are today (measured before deciding)
On the RTX 5070 Ti at 1920×1080 (`main` @ a0d2e10, release, Ultra):

| Scene | Instances drawn | CPU before the render call | Render call (bucket, upload, record, submit, present) | GPU `scene` / `prepass` |
|---|---|---|---|---|
| teamfight replay (tick 330) | ≈ 150 | 0.006 ms | 0.101 ms p50 | 0.194 / 0.066 ms |
| local preview + 512 VAT stress minions | ≈ 700 | 0.033 ms | 0.107 ms p50 | 0.364 / 0.147 ms |

The whole render call, including fence wait, acquire, submission and present, is about a
tenth of a millisecond, and adding 550 instances moves it by 6 µs. Instances move every frame,
so whichever side culls, the CPU uploads every candidate's 128-byte instance every frame.

## Decision

### 1. Per-view CPU frustum culling is the default
- **Bounds.** Every registered mesh gets a model-space bounding sphere at registration
  (centre of its AABB, radius to the farthest vertex). A **VAT** mesh's sphere covers every
  baked frame's positions (read from the texture at registration: exact). A **skinned** mesh's
  sphere (`MeshData::cull_sphere`) is centred on its bind-pose sphere. Its radius is
  `max(farthest vertex over every clip × 1.05, bind radius × 2)`, where the clips are skinned
  on the CPU at load, 15 fps, at most 256 poses. Measuring makes it hold for a modded hero
  with root motion or long reach. The ×2 floor covers what the clips cannot show: spring
  bones, additive hit reactions and blends between clips. A bone-LOD reduced skin uses its
  full mesh's sphere, and a hero is one instance, so a loose sphere costs nothing. The sample
  sentinel's clips reach 1.47× its bind radius, so its floor applies. Culling a unit that is
  on screen, or that casts a visible shadow, would be a fairness bug, which is why the floor
  stays. The world sphere is the model sphere through the instance transform
  (radius times the largest column length).
- **Views.** The main view culls against the camera frustum (six planes, Vulkan depth 0..1).
  The offscreen views (portraits, the minimap bake) cull against their own cameras. The
  **shadow pass gets its own caster list**: the main view's instances culled against the four
  side planes of the fitted light box only. The near and far planes are never used for shadows,
  because casters there are pancaked by `depthClamp` (ADR-0011) and still write the map.
- **Outlined instances are never culled from the camera list.** The inverted hull extrudes a
  silhouette by up to `MAX_OUTLINE_WIDTH` pixels in clip space, so a unit just outside the
  frustum could draw a ring at the screen edge. They are a handful (heroes, hover, target).
- **Exactness.** The tests are conservative (a sphere is dropped only when it is wholly
  outside a plane), culled instances are removed without reordering the survivors inside their
  mesh bucket, and every pass that reads an instance (prepass, outline IDs, scene, hulls) uses
  the same list, so a culled frame is **byte-identical** to an unculled one. The golden
  workflow captures with culling on and off and requires `max_diff=0`.
- **Terrain chunks.** The heightfield's index buffer is reordered at upload into chunks of
  32 × 32 cells (Ultra: 12 × 12 chunks), each with an AABB over its heights. Per view the visible
  chunks are merged into contiguous index runs (row-major chunk order, so a visible row is one
  run) and drawn with one `vkCmdDrawIndexed` each; the prepass and the scene draw the same runs.
  With culling off the whole reordered buffer is one draw, so off and on draw the same
  triangles in the same order. The terrain casts no shadow (ADR-0014), so there is no shadow
  terrain list.
- **Switch.** `RenderConfig::culling` (`CullMode::{Off, Cpu}`), from
  `MOBA_GPU_CULL=off|cpu` in the client (`on` is accepted as `cpu`); the default is `cpu` on
  every tier. `off` draws everything, the reference for the exactness check
  (`scripts/cull-probe.sh`).

### 2. GPU-driven culling: designed, deferred until instance counts justify it
The roadmap asks for GPU-driven frustum culling "once instance counts justify it". The table
above says they do not yet: the CPU path's whole per-instance cost is a sphere test (a few
nanoseconds), the upload is the same on both paths, and the render call is 0.1 ms. A GPU path
can only remove the sphere tests while adding a compute dispatch and its barriers, and with the
CPU path culling a 700-instance scene costs microseconds (perf/0022). The first draft of this
ADR planned to build it opt-in anyway; that was dropped during implementation, because an
unexercised second path is a maintenance and validation cost (every tier, culling on and off,
lavapipe and real GPU) with no measured gain. `CullMode` is `#[non_exhaustive]`, so the variant
can be added without an API break.

The design, when it is needed:
- the CPU buckets every candidate of the main view's camera and caster lists by mesh and LOD
  (as for `cpu`, without the sphere test) and writes one `VkDrawIndexedIndirectCommand` per
  bucket with `instanceCount = 0`;
- a `cull` compute pass (one thread per instance, before the shadow pass) tests each world
  sphere against the list's planes, appends survivors into a compacted instance buffer at
  `firstInstance + atomicAdd(instanceCount)`, and raises the mesh's draw count with
  `atomicMax`;
- static and VAT buckets draw with **`vkCmdDrawIndexedIndirectCount`** (core in 1.2, the
  `drawIndirectCount` feature): one call per mesh over its LOD commands, so trailing LODs with
  no survivors are skipped. Skinned instances (one draw per instance with a bone push
  constant) stay CPU-culled and are copied through unchanged; hulls read the uncompacted list;
- the graph declares the compacted buffer `VertexStorageRead` and the commands `IndirectRead`
  for the shadow, prepass and scene passes, so the barriers are derived like every other one;
- devices without `drawIndirectCount` fall back to `cpu`. Offscreen views stay on the CPU path.

The triggers to build it: a measured render call above 0.5 ms from instance work, more than
about 10 k instances, or GPU-resident instance data (static props uploaded once).

### 3. Mesh LODs: offline index-only levels, screen-space error selection
- **Baking.** `moba-tools mesh-bake lod <model.glb>` simplifies with
  [`meshoptimizer`](https://github.com/zeux/meshoptimizer) (MIT, through the `meshopt` crate,
  MIT OR Apache-2.0) — `meshopt_simplifyWithAttributes` at 50 %, 25 % and 12.5 % of the
  triangles, keeping a level only if it removes at least 20 % of the previous one — and writes
  `models/<model>.lod`: a small binary file (`MLOD`, version 1) with the source mesh's
  `mesh_hash`, and per level its object-space error (in model units) and its indices. The baker
  is the only place meshoptimizer is linked, and only behind the `moba-tools` feature
  `mesh-bake` (`required-features`): the default workspace build, the client and the renderer
  never compile it; `just ci-tools` builds it, re-bakes every sample and fails on any diff.
- **Seams and attributes.** The sample meshes are flat-shaded (split vertices per face), so
  every edge is an attribute seam and meshoptimizer's default (seam-preserving) mode removes
  almost nothing; the baker runs in `Permissive` mode, which may collapse across seams, and
  weights the collapses by the normals (0.5) and, on textured meshes, the UVs (1.0), so a
  collapse that would smear the texture mapping or bend the shading costs error. Locking every
  UV-seam vertex instead (`--protect-uv-seams`, meshoptimizer's `Protect` flag) is opt-in: on
  the sample meshes 83–100 % of the vertices sit on a UV seam (per-face palette mapping) and it
  bakes no levels at all. `--keep-seams` (no `Permissive`) and `--lock-border` are there for
  smooth-shaded meshes and open borders. A flat-shaded untextured mesh is weighted by its
  normals only.
- **Error.** A level's error is the largest of meshoptimizer's own estimate and a measured one:
  the distance from every LOD0 vertex the level dropped to the level's surface, over the bind
  pose and, for a skinned mesh, every pose sampled from its clips (10 fps, at most 31 poses),
  because a level that is close at bind can fold at a joint. On the samples the measured error
  dominates and is about twice meshoptimizer's (perf/0022).
- **Index-only levels.** A level is a new index list over the LOD0 vertex buffer, so one file
  serves the static mesh, the GPU-skinned mesh, every bone-LOD reduced skin (same vertices, same
  order) and the VAT minion (whose texture is per LOD0 vertex). `moba_assets::MeshLods` loads
  and validates it (hash match, indices in range, at most three levels, no level larger than
  LOD0) and the loader attaches it to the `MeshData`; a missing file is no LODs, a bad one is
  logged and ignored. The renderer appends the levels to the mesh's index buffer after LOD0
  and draws a level by its `firstIndex` range.
- **Selection.** Per instance and view, the coarsest level whose error projected to pixels
  (`error × scale × ½·height·|row₁(view_proj)| / w`, which covers perspective and the
  orthographic offscreen views) is at most the tier's tolerance: Low 2 px, Medium 1 px,
  High 0.75 px, Ultra 0.5 px. The caster list reuses the camera list's level for the same
  instance. `MOBA_MESH_LOD=off|<px>` turns it off or overrides the tolerance; captures keep it.
  `height` is the view's height at its configured render scale, not the frame's
  dynamic-resolution extent, so dynamic resolution never flips a unit between levels (the only
  input that changes a level is the unit's distance and scale).
- **Tier budgets.** When LOD0 exceeds the tier's triangle budget (§4) and a baked level fits, the
  finest level that tier draws is that one (a content-check warning says so).
- **Bone LOD.** The two are independent and compose: bone LOD picks the reduced skin by camera
  distance (ADR-0012), mesh LOD picks the index list by projected error, and a reduced skin
  draws the same levels as the full one.

### 4. Memory: per-tier budgets, accounting, load-time-only uploads
- **Budgets** live in `moba_assets::budget` so the renderer and `content-check` share them:

  | Tier | Total VRAM | Textures | Hero triangles | Hero joints | Texture bytes per model |
  |---|---|---|---|---|---|
  | Low | 1 GiB | 384 MiB | 8 k | 32 | 4 MiB |
  | Medium | 2 GiB | 1 GiB | 16 k | 48 | 16 MiB |
  | High | 4 GiB | 2.5 GiB | 32 k | 64 | 32 MiB |
  | Ultra | 6 GiB | 4 GiB | 64 k | 64 | 64 MiB |

  (Low is a UHD 620 sharing system memory, Medium a 3 GB GTX 1060, High and Ultra 8–12 GB
  cards, with headroom for the driver, the compositor and other applications.)
- **Accounting.** Every `GpuImage` and `GpuBuffer` adds its allocation size to the renderer's
  counters when created and removes it when dropped, by class: textures (sampled images
  uploaded from assets), render targets (attachments, transients, offscreen images) and buffers;
  the swapchain adds its images (width × height × texel size × image count, owned by the WSI
  and estimated). The total counts all four classes. `MemoryStats::reserved_bytes` is the
  allocator's reserved block bytes (`gpu_allocator::Allocator::capacity`), which is what the
  driver has actually handed out (allocations round up into 256 MiB device blocks, so it is
  usually above the total). `Renderer::memory_stats()` reports them with the budgets; the
  client prints them at startup and in the frame log (`vram_mb`, `texture_mb`, `targets_mb`,
  `buffers_mb`, `swapchain_mb`, `vram_reserved_mb`, `vram_over_budget` with the budgets), and
  the first allocation that takes the total over the tier's budget logs a warning.
  `VK_EXT_memory_budget` (the heap's live budget across processes) is not queried: it is not
  this process's footprint, and the shared desktop GPU makes it noise.
- **Degrading, never crashing.** A texture registration that would exceed the texture budget
  drops its largest mip levels until it fits (a mip bias, logged); a texture that cannot fit even
  at its smallest level is refused with `RenderError::OverBudget`, and the material keeps its
  fallback for that map (an optional map dropped). Going past the total budget only logs.
- **Load-time only.** There is no champion select, so every hero, skin, map and effect asset
  loads at match or lobby start, as today. `Renderer::lock_uploads()` marks the start of play
  (the client calls it on the first measured frame while the match is `Playing` past its
  warm-up, or the local view's first frame) and `unlock_uploads()` the end (the match leaving
  `Playing` — results, the next lobby or countdown — a content reload or map rotation, or a dev
  shader/VFX reload, unlocks and rebinds; the next measured `Playing` frame locks again). The
  lock is process-wide, so `Renderer::new` starts unlocked and dropping a renderer unlocks
  (a renderer rebuilt mid-session never inherits the old lock); `Renderer::uploads_locked`
  reports it. While locked, every staging upload and every pipeline creation is counted
  (`MemoryStats::locked_uploads`, `MemoryStats::locked_pipelines`) and the first is logged; the
  lock is evidence, not a gate (the upload still happens). The frame log reports both, and
  `perf-gate` and the evidence runs require 0.
- **content-check** reports, per unit model, LOD0 and baked-level triangles, joints and texture
  bytes against each tier's budget (text and `--json`): `fits`, `lod<N>` (the tier draws that
  baked level as its finest) or `over`. Over a tier is a warning, over Ultra an error. A `.lod`
  that is stale (another `mesh_hash`) or malformed is a warning naming the problem, since the
  model then draws LOD0 only (`ModelBudgetReport::lod_problem`).
- **Instance cap.** The renderer holds `max_instances` instances per view; the cap applies after
  culling, in scene order, to the units a list draws (camera or casters), so off-screen units
  never push visible ones out. `CullStats::dropped` and the frame log's `cull_dropped` count the
  ones that did not fit.

### 5. Frame pacing evidence
- The frame log keeps up to `MOBA_FRAME_LOG_FRAMES` samples (default 10 000, up to 1 000 000,
  allocated once at start) so a 10-minute run is logged whole, and it reports `p99_ms`, the pipeline
  cache state, the locked upload and pipeline counts, the VRAM counters and `frames_dropped`
  (frames past the log's capacity, which `perf-gate` requires to be 0: the percentiles cover
  the whole run).
- The 10-minute scene is the R5 teamfight script run for 18 000 ticks
  (`moba-tools teamfight --ticks 18000`): deterministic, regenerated on demand, not committed.
- Pacing target (roadmap R8): frame-time p99 ≤ 1.5 × p50 per tier on the replay.

### 6. Nightly perf job
`.github/workflows/nightly-perf.yml` runs on `[self-hosted, gpu]` from `schedule` and
`workflow_dispatch`, only when the repository variable `HAS_GPU_RUNNER` is `true` (otherwise the
job is skipped, never queued), with a timeout. It builds release, records the replay, plays it
per tier and runs `moba-tools perf-gate`, which compares each frame-log line with the tier's §7
GPU budget, the pacing ratio and the committed baseline (`tests/perf/baseline.toml`, default
threshold +25 % on GPU p95), and fails on any breach. It also fails on dropped frames, a VRAM
peak over the tier's budget, a cold pipeline cache (with `--warm-cache`) and any locked upload
or pipeline. The baseline is strict: it names the GPU it was recorded on (`gpu = "…"`, which
must match the log's `gpu=`, or the gate fails asking for a baseline recorded on that GPU),
has a table for every tier the log ran, with exactly `gpu_total_p95_ms` and
`render_p95_ms`, and any other key or table is an error (a typo never silently skips a
check). `perf-gate` is unit-tested and runs on any machine.

## Consequences
- The prepass, scene and minimap bake draw only the terrain chunks they can see; the shadow
  pass draws only units that can cast into its box. On the RTX 5070 Ti the 512-minion stress
  scene's GPU total fell 0.816 → 0.730 ms p50 (shadow −40 %, scene −13 %) and the Ultra
  teamfight's 0.621 → 0.593 ms, for 9–27 µs more CPU in the render call
  ([perf/0022](../perf/0022-r8-scaling-pacing.md)).
- Culling is exact by construction and checked by the golden workflow (five scenes
  byte-identical on and off). LODs are not exact: they changed 0.01–0.86 % of pixels in the
  golden captures, all on silhouettes, so all eight fingerprints were deliberately rebaselined.
- Baked levels are appended to each mesh's index buffer (no new buffers), one bounding sphere
  per mesh, and 2× the instance writes for the main view (camera and caster lists).
- A skinned mesh's culling sphere is measured from its clips at load (§1): it costs a CPU
  skinning pass over at most 256 poses per model at load time, and a model whose motion is not
  in its clips (spring bones, additive layers) relies on the ×2 floor.
- Over-budget content degrades visibly (blurrier textures, missing optional maps) and is
  reported at load and by content-check, instead of failing to load.
- The reference-GPU matrix (§7) stays a maintainer item (#9/#127): perf/0022 has the RTX 5070 Ti
  evidence and a checklist for the other machines.

## Alternatives considered
- **GPU-driven culling now (default or opt-in):** no measured CPU win at today's counts (§2);
  designed and deferred.
- **Hi-Z occlusion culling:** a MOBA camera looks down at a mostly flat board; little occludes.
- **Separate LOD meshes (vertices and indices):** would need one VAT texture and one reduced skin
  per level; index-only levels share both.
- **Continuous terrain LOD (CDLOD, geomorphing):** the visible terrain is already a fraction of
  the mesh after chunk culling; revisit if a map needs larger terrain.
- **Streaming textures mid-match:** forbidden by the roadmap (load, never mid-match).
