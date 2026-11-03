# ADR-0024: GPU-driven geometry: meshlets, compute culling, mesh shaders, two-phase occlusion and cluster LOD (N7)

**Status:** proposed 2026-11-03. This is phase N7 of the
[next-gen graphics roadmap](../plans/next-gen-graphics-roadmap.md) (§3, §5.4, §8, §9.1, §10
and §12), written before the code.
- **Builds on:**
  - [ADR-0020](0020-next-gen-foundations.md) (N0, accepted 2026-11-01, with Amendments 1–6). It
    settles the capability gate and the `Tiered`/`Compat` paths (§2), mesh shaders and VRS as
    **optional** capabilities with a compute and indirect fallback (§1e, Q3), the bindless set
    and buffer device address (§3), Slang (§4) and the tier budgets (§5). This ADR uses them
    and does not re-decide them.
  - [ADR-0021](0021-temporal-upscaling-overlay.md) (N1, accepted 2026-11-02): jitter, motion
    keys, the previous-instance buffer, the previous joint palettes and the output-resolution
    overlay prepass.
  - [ADR-0023](0023-frame-architecture-materials.md) (N2, accepted 2026-11-02): the G-buffer
    pass in two groups, reversed-Z, bindless materials and its Q8, "N1's GLSL vertex stages stay
    until N7; N7 rewrites the vertex path for GPU-driven geometry".
  - [ADR-0018](0018-culling-lods-memory.md) (R8): CPU frustum culling, index-only meshoptimizer
    LODs selected by projected error, VRAM budgets, and the GPU-culling design it deferred
    "until instance counts justify it" (§2).
- **Amends, for the Tiered path only:**
  - [ADR-0018](0018-culling-lods-memory.md) §1–§3: culling and LOD selection move to the GPU on
    Tiered (§3–§5); `CullMode` gains `Gpu`; meshes gain baked meshlets (§2);
  - [ADR-0007](0007-mesh-format.md): two new client-local sidecar files next to `.lod`
    (`.meshlets`, `.clod`), validated like it (§2d, §7b);
  - [ADR-0012](0012-animation-runtime.md): skinned meshes are skinned once per frame in compute
    (§6a); VAT textures move to the bindless table (§6b);
  - [ADR-0017](0017-decals-foliage-water.md) §2: foliage culling moves to the GPU (§6d);
  - [ADR-0020](0020-next-gen-foundations.md) §1e: the optional geometry group needs `meshShader`
    only, not `taskShader` (§4d).
- **Compat is unchanged by N7.** It keeps CPU culling, discrete LODs, vertex input bindings,
  GLSL and its goldens (ADR-0020 §2d). Nothing in this ADR runs on Compat.

External claims (the Vulkan specification, driver support, vendor guidance, meshoptimizer and
published talks) were re-checked on 2026-11-03 against the primary sources in
[Sources](#sources), as the roadmap's preamble requires. Facts ADR-0020, ADR-0021 or ADR-0023
already verified are cited as theirs. Claims that could not be confirmed are marked
**unverified**.

## Context

Roadmap §8 gives N7 these items: meshlets, compute culling, indirect count, mesh shaders, HiZ
occlusion, cluster LOD for static assets, and an optional visibility buffer. It depends on N0
and runs alongside the critical path. Roadmap §5.4 says why: photoreal density is what
triggers the GPU-driven path that ADR-0018 §2 designed and deferred.

What the code does today (`main` at `e9d5f91`, which has N0 and the FSR 3.1 port; N1a is open
as #360 and N2 is not yet built):

- **Instances.** The client rebuilds `Scene::units` every frame (`client/src/app/scene_build.rs`),
  including the map's walls and props, which `MapLibrary::push_instances` copies from a prebuilt
  slice. Each `UnitInstance` uploads as a 128-byte `GpuInstance` (`scene.rs`, pinned by
  `instances_are_128_bytes_with_the_outline_slot_last`). `RenderConfig::max_instances` is 4,096
  per view.
- **Culling** (`cull.rs`, ADR-0018 §1). Per view, on the CPU: a bounding sphere per mesh
  (skinned spheres widened from their clips with a ×2 floor; VAT spheres covering every baked
  frame) against the frustum; the shadow caster list against the four side planes of the
  fitted light box. `CullMode::{Off, Cpu}`, `MOBA_GPU_CULL=off|cpu`. Culled frames are
  byte-identical to unculled ones, which `scripts/cull-probe.sh` checks.
- **LODs** (`mesh.rs`, ADR-0018 §3). Up to three index-only levels per mesh (`MAX_LODS` = 4
  with LOD0), baked offline by `moba-tools mesh-bake` with meshoptimizer into
  `models/<model>.lod`, appended to the mesh's index buffer. Selection is per instance and
  view: the coarsest level whose projected error is within the tier's tolerance (Low 2 px,
  Medium 1, High 0.75, Ultra 0.5).
- **Draw submission** (`passes/scene.rs`). Instances are bucketed by mesh slot × level
  (`MAX_MESHES` = 64). Static and VAT meshes are one instanced `vkCmdDrawIndexed` per
  non-empty bucket; **skinned meshes are one draw per instance**, with a 16-byte push constant
  selecting the joint range in the per-frame bone buffer (three `vec4` rows per joint). VAT
  meshes bind a per-mesh descriptor set at 2 (`vat.rs`, `MAX_VAT_MESHES` = 8).
- **Vertex formats.** `Vertex` is 48 bytes (position, normal, UV, tangent); `SkinnedVertex`
  is 56 bytes (adds four `u8` joints and four `unorm8` weights). Both are fed through vertex
  input bindings.
- **Terrain** (`terrain.rs`). The heightfield is reordered at upload into 32 × 32-cell chunks
  with AABBs; visible chunks merge into index runs, one draw each.
- **Foliage** (`environment.rs`, ADR-0017). Up to 65,536 procedural clump instances (up to 8
  blades of 9 vertices each), CPU-culled in 16 m chunks with a distance cap, drawn non-indexed.
- **Back faces.** Every scene pipeline is created with `CullModeFlags::NONE`
  (`passes/scene.rs`, `pipeline.rs`), so back faces are rasterised today.
- **meshoptimizer** is linked only into `moba-tools` behind the `mesh-bake` feature, through the
  `meshopt` crate, **0.7.0** in `Cargo.lock`. That release wraps meshoptimizer 1.3, including
  the `clusterlod.h` bindings and the meshlet codec [S12].
- **Measured cost of today's path** (ADR-0018, perf/0022, RTX 5070 Ti): the whole render call
  is about 0.1 ms for 150–700 instances. ADR-0018 §2's triggers for a GPU path were "a render
  call above 0.5 ms from instance work, more than about 10 k instances, or GPU-resident
  instance data".

What changes the picture is content, not today's scenes:
- **Photoreal density.** Roadmap §2 expects **1–3M visible triangles per frame**; §9.1 budgets
  structures at 15–40k triangles (or cluster LOD from 1–2M-triangle sources), props at
  0.3–4k, foliage clusters at 1–6k of real geometry, and terrain at 0.5 m spacing.
- **GPU-resident static instances.** The map's decoration does not move. Uploading it once
  meets ADR-0018's third trigger by itself.
- **The vertex path is being rewritten anyway.** ADR-0023 Q8 left N1's GLSL vertex stages
  under N2's Slang fragments until N7.
- **N4 needs skinned positions in a buffer** for its per-frame hero BLAS refits (roadmap §5.5).

Local evidence (`vulkaninfo`, 2026-11-03):

| Device | Driver | `VK_EXT_mesh_shader` | Max mesh output vertices / primitives | Preferred mesh workgroup | `prefersLocalInvocation…Output` (vertex / primitive) | Subgroup |
|---|---|---|---|---|---|---|
| RTX 5070 Ti | NVIDIA 615.78.08, Vulkan 1.4.351 | `meshShader`, `taskShader`, `meshShaderQueries`, `primitiveFragmentShadingRateMeshShader` | 256 / 256 | 32 | false / false | 32 |
| Radeon 610M (RDNA2) | RADV, Mesa 26.2.5 | the same, without `meshShaderQueries` | 256 / 256 | 128 | true / true | 64 |
| llvmpipe | bundled lavapipe, Mesa 26.2.5, LLVM 23.1.3 | yes | 256 / 256 | 128 | — | 8 |

All three report `drawIndirectCount`, `multiDrawIndirect`, `drawIndirectFirstInstance` and
`shaderDrawParameters`, with `maxDrawIndirectCount` = 2^32 − 1 on both GPUs. Both GPUs report
subgroup ballot and arithmetic. Lavapipe running mesh shaders means CI can run **both**
raster back ends of §4.

Constraints:
- **Presentation only.** Rendering never touches the simulation, the wire, replays or the
  state hash. No change to `PROTOCOL_VERSION` (v42), replay `VERSION` (v5),
  `SIM_LOGIC_VERSION` (v9), content hashes or `tests/golden-state-hash.txt` (§10).
- **Culling must never change what a player sees** (ADR-0018). That includes fairness: a unit
  on screen, or casting a visible shadow, is never culled.
- **Hidden information.** Nothing may draw a unit the scene would not draw (ADR-0016 §2).
- **The R8 rules hold:** no per-frame allocation, `locked_uploads=0`, `locked_pipelines=0`.
- **Mesh shaders stay optional** (ADR-0020 Q3). Every Tiered device must get the full N7
  feature set through a compute and indirect fallback.
- **Determinism of captures.** Two lavapipe runs of a golden stay byte-identical, on both back
  ends.

## Decision

### 1. Shape: compute culls, the raster back end only draws

```text
geo_instances   compute: instance cull for every view (frustum, LOD, cluster-LOD cut),
                ordered compaction → per-view candidate meshlets
geo_skin        compute: skin the visible skinned instances once (current + previous) → skinned buffer
geo_meshlets    compute: meshlet cull per view (frustum, cone, HiZ in phase 2), ordered compaction
                → per-view, per-bucket visible-meshlet lists + indirect arguments
raster          mesh back end:   vkCmdDrawMeshTasksIndirectEXT, one mesh workgroup per visible meshlet
                vertex back end: geo_expand (compute) writes a packed index buffer,
                                 then one vkCmdDrawIndexedIndirect per bucket
```

- **The shape is the published GPU-driven pipeline** (instance culling, cluster culling,
  compaction, indirect draws) of Haar and Aaltonen [S17], with meshlets as the cluster.
- **One culling implementation for both back ends.** The visible-meshlet list a view produces
  is the same buffer, byte for byte, whichever back end draws it. Only the last step differs.
  That makes the back ends testable against each other (verification gate 2).
- **No task shaders.** Culling runs in compute, not in a task (amplification) shader. See
  [Alternatives](#alternatives-considered) for why.
- **One instance pass for every view of the frame.** Each instance's LOD (or cluster cut) is
  computed once and tested against every view that wants it: the camera, the sun-shadow
  casters, the output-resolution overlay prepass (ADR-0021 §5c) and the offscreen views. The
  casters keep the camera's level, as ADR-0018 §3 does today.
- **Ordered compaction, no atomics.** Every compaction preserves input order (§3d), so the
  draw order of the survivors equals their order in the unculled list. That is what keeps
  culling exact (ADR-0018's argument) and goldens deterministic.

### 2. Meshlets

**2a. Size: 64 vertices, 124 triangles.** One size on every vendor, chosen from the vendors'
own guidance:
- NVIDIA recommends 64 vertices and 126 primitives, with 84 and 40 as other sweet spots
  [S9]. Its guidance is written for `VK_NV_mesh_shader`; no EXT-specific NVIDIA figure was
  found, so it is assumed to carry over (**unverified**).
- AMD recommends 128 vertices and 256 triangles on RDNA, and says 64/126 gives "similar
  performance at the expense of duplicating more vertices" [S8].
- meshoptimizer's README suggests 64/126 for NVIDIA, equal limits on earlier AMD GPUs, and
  64/96 otherwise [S10]. Kapoulkine measured occlusion-culling efficiency falling from about
  80 % at 64 triangles to about 66 % at 256 [S13].
- **124 rather than 126** keeps the triangle count a multiple of 4, so the packed `u8`
  triangle indices load as whole `u32` words. meshoptimizer 1.0 removed its own alignment
  requirement, so this is our choice, not a constraint [S10].
- Both limits are well inside the spec minimums: `maxMeshOutputVertices` and
  `maxMeshOutputPrimitives` must be at least 256 [S2].

**2b. Building.** `moba-tools mesh-bake meshlets <model.glb>` (the existing `mesh-bake`
feature, meshoptimizer 1.3 through `meshopt` 0.7.0):
- per discrete level (LOD0 and each `.lod` level), `meshopt_buildMeshlets` with
  `max_vertices = 64`, `max_triangles = 124` and `cone_weight = 0.25` (the README's suggested
  value), then `meshopt_optimizeMeshlet` on each meshlet [S10];
- bounds from `meshopt_computeMeshletBounds`: a sphere, and a cone (apex, axis, cutoff) in
  model space [S10];
- built with `-ffp-contract=off` through `scripts/bake-mesh-lods.sh`, as ADR-0018 §3 requires
  for byte-identical bakes, and checked by CI against the committed files.

**Meshlets stay index-only.** A meshlet's vertex list holds indices into the mesh's existing
vertex buffer, exactly as an ADR-0018 level does. So one set of meshlets serves the static
mesh, the skinned mesh, every bone-LOD reduced skin and the VAT minion (whose texture is per
LOD0 vertex).

**2c. Records** (GPU, std430-compatible, one global meshlet pool per renderer):

| Record | Size | Fields |
|---|---|---|
| `MeshletRecord` | 32 B | `vertex_offset: u32` (into the pool's meshlet-vertex list), `triangle_offset: u32` (into the packed `u8` triangle list), `vertex_count: u8`, `triangle_count: u8`, `flags: u16`; sphere `center: f32×3`, `radius: f32`; cone `axis: s8×3`, `cutoff: s8` |
| meshlet vertex | 4 B | `u32` vertex index into the mesh's vertex buffer |
| meshlet triangle | 3 B, padded per meshlet to 4 | three `u8` local vertex indices |
| `MeshLevel` | 16 B | first meshlet, meshlet count, LOD error (model units), reserved |
| `VisibleMeshlet` | 8 B | `instance: u32`, `meshlet: u32` (global pool index) |

- The `s8` cone is meshoptimizer's own quantised form [S10].
- The records get Slang mirrors in `shaders/slang/lib/geometry.slang` and join ADR-0020 §4's
  reflection layout test.
- Everything is reached through buffer device address (ADR-0020 §3a), so no pool is bound by
  `maxStorageBufferRange` (at least 2^27 bytes [S2]).

**2d. The `.meshlets` file.** A new client-local sidecar, `models/<model>.meshlets` (magic
`MMLT`, version 1), next to the `.lod` file:
- it holds the source mesh's `mesh_hash`, the hash of the `.lod` file it was built from, a
  `closed` flag (§3c), and per level its meshlets, vertex lists and triangles;
- `moba_assets::MeshMeshlets` loads and validates it, as `MeshLods` does: hashes match, every
  vertex index is below the mesh's vertex count, every local index is below its meshlet's
  vertex count, at most 64 vertices and 124 triangles per meshlet, each level's triangles
  equal to its index list's. A stale or broken file is logged and ignored;
- `content-check` reports a missing or stale file as a warning (it never fails a pack, since
  Compat does not need it).

**2e. Models without a baked file.** A modded model, or one whose `.meshlets` file is stale,
is meshletised **at load** by an in-tree Rust builder:
- it walks the level's index list in order, starting a new meshlet whenever the next triangle
  would exceed 64 unique vertices or 124 triangles;
- it computes a bounding sphere and a conservative cone (cutoff from the widest normal
  deviation, or "no cone" when the normals span more than a hemisphere);
- it is deterministic, allocation-bounded and unit-tested, and it runs only at load
  (`locked_uploads` is unaffected).

Its meshlets are less compact than meshoptimizer's, so culling is less effective, never wrong.
This keeps meshoptimizer's C++ out of the client build (Q9).

### 3. Compute culling

**3a. Instance data.**
- **Static instances become GPU-resident.** A new `Renderer::set_static_instances(&[StaticInstance])`,
  called at map bind like `set_foliage`, uploads the map's decoration (walls, props, structures,
  rocks, mesh foliage clusters) once. On Tiered, the client stops pushing them into
  `Scene::units` every frame; on Compat it keeps doing so, so Compat's pixels cannot change.
  Static instances have motion key 0 (camera motion only, ADR-0021 §2c).
- **Dynamic instances** (units, and anything the simulation can destroy, such as towers) stay
  in `Scene::units`, uploaded per frame as today, with N1's parallel previous-instance buffer
  (64 bytes per instance, ADR-0021 §2c).
- Both use the 128-byte `GpuInstance`. The instance index space is **dynamic first, then
  static**, so a capacity overflow (§3e) can only ever drop decoration, never a unit.
- **Compaction writes instance indices, never copies.** The previous-instance buffer is
  indexed like the instances, so the G-buffer's motion vectors read `prev_instances[i]` with
  the same `i` as today, whatever was culled.

**3b. `geo_instances`.** One thread per instance (static and dynamic), for every active view:
- **Frustum.** ADR-0018's tests unchanged: the world sphere against the view's planes,
  dropped only when wholly outside one plane; the caster view uses the light box's four side
  planes only; non-finite inputs are never culled.
- **LOD.** ADR-0018 §3's projected-error rule, the same formula as `cull.rs::LodMetric`, with
  the view's height at its configured render scale (so dynamic resolution never flips a level).
  The caster view reuses the camera's level.
- **Cluster-LOD cut** for cluster-LOD instances on the tiers that enable it (§7).
- **Output:** for each surviving `(view, instance)`, its meshlet range at the chosen level. An
  ordered scan of the ranges gives each view a flat list of **candidate meshlets**.

**3c. `geo_meshlets`.** One thread per candidate meshlet. The thread finds its instance by a
binary search in the scanned ranges (so load balance does not depend on mesh size), then:
- **Frustum:** the meshlet's world sphere against the view's planes, as above.
- **Cone (back-face) culling**, only where it is exact:
  - the mesh is **closed**: the bake (or §2e's builder) found every position-welded edge shared
    by exactly two triangles, and its material is opaque (not alpha-tested);
  - the mesh is static (not skinned, VAT or wind-deformed, whose cones do not hold);
  - the view's pipelines cull back faces.

  A closed opaque mesh seen from outside never shows a back face, so removing back-facing
  meshlets changes no pixel, except possibly a depth tie exactly on a silhouette edge (gate 2).
  The test uses the eye position. Jitter is a clip-space translation that keeps the centre of
  projection, so it does not change which side of a triangle's plane the eye is on. The
  orthographic caster view would use the light direction, but the shadow pipelines keep
  `CullModeFlags::NONE` (back faces cast), so the caster view does no cone culling.
- **Back-face culling in the pipelines.** The Tiered geometry pipelines use `BACK` culling
  for closed opaque meshes and `NONE` for everything else (open, alpha-tested, double-sided,
  foliage). This is the same exactness argument and a deliberate Tiered rebaseline.
- **Occlusion** in phase 2 (§5).
- **Skinned and VAT instances skip per-meshlet tests.** Their meshlet bounds do not hold under
  deformation, so all meshlets of their chosen level pass. Heroes are 40–60 px tall at the
  gameplay camera (roadmap §2), so per-meshlet culling would save little.

**3d. Ordered compaction.** Every list (candidates, visible meshlets per bucket, the skinning
work list) is compacted **in input order**, with a reduce-then-scan in three dispatches: count
per workgroup, scan the counts, scatter.
- **Why not `atomicAdd` appends:** the order of the survivors would depend on scheduling, so
  ties in depth would resolve differently between runs and goldens would not be
  byte-identical. Ordered output is also what makes a culled frame draw "a subset of the
  unculled frame's primitives in the same order" (ADR-0018 §1), so the exactness argument
  carries over to the GPU.
- **Why not a single-pass scan with decoupled look-back:** it relies on progress between
  workgroups, and the Vulkan specification makes almost no guarantee about inter-workgroup
  forward progress [S18].
- **Workgroups of 128 threads,** the core minimum for `maxComputeWorkGroupInvocations` [S2],
  with the scan in shared memory. No subgroup operation is required: core 1.3 guarantees only
  `BASIC` subgroup operations in compute [S2]. A subgroup-ballot permutation may be added
  later where it measures faster, with the same output.
- The scan's cost is a few microseconds at these counts (planning estimate).

**3e. Capacities and overflow.** Fixed at renderer creation, never grown in a frame:
- `RenderConfig::max_static_instances` (default 16,384), `max_instances` (dynamic, 4,096 as
  today), `max_visible_meshlets` per view (default 262,144, about 32M triangles, 2 MiB of
  `VisibleMeshlet`s) and, for the vertex back end, `max_visible_triangles` per view (§4c).
- An overflow drops the tail of the ordered list, which is static decoration by construction
  (§3a), and increments a counter. The frame log reports `geo_overflow`; `perf-gate` and the
  evidence runs require it to be 0.

**3f. Synchronisation.** The culling buffers are graph resources (ADR-0008): written as
`ComputeStorageReadWrite`, read as `IndirectRead` (`VK_PIPELINE_STAGE_2_DRAW_INDIRECT_BIT`,
`VK_ACCESS_2_INDIRECT_COMMAND_READ_BIT`) and as storage reads in the mesh, vertex and
compute stages [S5]. Every barrier is derived by the graph. The persistent ones (the static
instances, the meshlet pool, the visibility bits of §5) are imported persistent buffers, as
the TAA history is. N7 adds about eight graph resources; if a topology passes `MAX_RESOURCES`
(48), the mask widens to `u128` in the same PR, as ADR-0023 §11 already allows.

### 4. The two raster back ends

**4a. Vertex pulling on both.** Neither back end uses vertex input bindings. The shaders read
`Vertex` and `SkinnedVertex` records (today's 48- and 56-byte layouts, unchanged in N7) through
buffer device address; the vertex buffers gain `SHADER_DEVICE_ADDRESS` usage on Tiered.
Compressed, split vertex streams are a separate later step (Q7).

**4b. The mesh back end** (devices that enable `meshShader`):
- the culling pass writes one `VkDrawMeshTasksIndirectCommandEXT` per bucket whose group
  count is the bucket's visible-meshlet count, laid out as `(65,535, ⌈n / 65,535⌉, 1)`. The
  spec's minimum `maxMeshWorkGroupCount` is 65,535 per dimension and `maxMeshWorkGroupTotalCount`
  is 2^22 meshlets (about 520M triangles) [S2];
- one `vkCmdDrawMeshTasksIndirectEXT` per bucket with `drawCount = 1`. The count variant
  (`vkCmdDrawMeshTasksIndirectCountEXT`, which needs `drawIndirectCount`) is not needed. Its
  refpage now marks it superseded by `vkCmdDrawMeshTasksIndirectCount2EXT` from
  `VK_KHR_device_address_commands` [S3];
- each mesh workgroup reads its `VisibleMeshlet`, fetches and transforms up to 64 vertices,
  writes up to 124 primitives, and culls small primitives (§4d);
- **workgroup size is a build-time permutation picked at creation:** 128 threads, one vertex
  and one primitive per thread, on devices reporting `prefersLocalInvocationPrimitiveOutput`
  (RADV here), as AMD advises for RDNA [S8]; otherwise 32 threads, NVIDIA's preferred size
  here, looping over vertices and primitives. Both are compiled; one is created. Neither is
  compiled at run time, so `locked_pipelines=0` holds;
- **the fragment interface is shared with the vertex back end.** The mesh shader writes only
  per-vertex outputs (flat values on every vertex), never `PerPrimitiveEXT` ones, so N2's Slang
  G-buffer fragment modules serve both back ends unchanged;
- **output size** is checked at creation against the device's `maxMeshOutputMemorySize`
  (spec minimum 32,768 bytes) and `maxMeshOutputComponents` (minimum 128) [S2]. The G-buffer
  outputs need a few kilobytes per meshlet (planning estimate);
- **rasterisation order:** the spec orders all primitives of one mesh workgroup before those
  of later workgroups, and primitives within a workgroup by the primitive index buffer [S4].
  With the ordered visible list (§3d) the primitive order is therefore deterministic.

**4c. The vertex back end** (every Tiered device; the only one without mesh shaders):
- `geo_expand`: one 64-thread workgroup per visible meshlet writes its triangles into a
  per-view **packed index buffer**, each index being `visible_slot << 6 | local_vertex`;
- the vertex shader decodes the index, reads the `VisibleMeshlet`, the meshlet's vertex list
  and the vertex, and runs the same shared Slang transform as the mesh shader. Repeated
  indices inside one meshlet keep post-transform vertex reuse;
- one `vkCmdDrawIndexedIndirect` per bucket, whose `indexCount` the expand pass writes (0 for
  an empty bucket). Indexed draws are processed in their indirect buffer's storage order [S4];
- small-primitive and back-face tests per triangle happen in the expand pass (§4d), which
  also lowers the index count;
- **capacity:** `max_visible_triangles` per view, default 2M on Low and Medium and 4M on High
  and up: 24 or 48 MB of `u32` indices per view. Only devices on the vertex back end allocate
  it. The buffer is one persistent buffer per view, ordered across frames by the graph's
  persistent-buffer barrier rather than duplicated per frame in flight, since N7 does not use
  async compute;
- **the compacted index buffer, not one indirect draw per meshlet.** At 1–3M visible
  triangles that would be 8k–24k draws of 124 triangles. The N7b spike measures both on the
  RTX 5070 Ti and the Radeon 610M; this ADR's choice stands unless per-meshlet multi-draw is
  at least 10 % faster on both, in which case the spike records an amendment.

**4d. Small-primitive culling.** A triangle is dropped when its screen-space bounding box,
widened by one sub-pixel step (`subPixelPrecisionBits`, at least 4 [S2]), contains no pixel
centre of the **jittered** viewport, or when its area is zero. Such a triangle produces no
fragment (MSAA is retired on Tiered, ADR-0021 §7), so the test is exact. In the mesh back end
it sets `CullPrimitiveEXT`; in the vertex back end the expand pass skips it.

**4e. Which back end runs.** `MOBA_MESH_SHADERS=auto|off` (and a settings entry):
- `off` uses the vertex back end on any device;
- `auto` uses the mesh back end where `meshShader` is enabled **and** the device's vendor is
  on the measured allow-list (Q2), otherwise the vertex back end;
- the frame log prints `geo=mesh|vertex`, so evidence always names its back end.

### 5. Two-phase HiZ occlusion

**5a. The phases** (main camera view only). Following Nanite's and niagara's two-pass
scheme [S13][S14]:
1. **Phase 1** draws the meshlets that were visible last frame (by a persistent visibility
   bit), after frustum and cone tests, with no occlusion test.
2. `hiz_build` reduces phase 1's depth into a pyramid.
3. **Phase 2** tests every frustum-passing meshlet **not** drawn in phase 1 against the
   pyramid (the instance's sphere first, as an early out), draws the survivors, and rewrites
   every visibility bit.

This is exact up to the pyramid's conservativeness: anything phase 2 rejects is behind
geometry already in the depth buffer, so the depth test would have rejected all its fragments.

**5b. What is never occlusion-culled.**
- **Dynamic instances** (units). They draw in phase 1 after frustum culling only. They are few,
  poor occluders and the fairness-critical case, so no unit can be culled by a stale or
  conservative pyramid (Q4).
- **Receivers** (terrain and foliage). ADR-0023 §1e draws them first, then the `Paint` decals,
  then the rest, and "units never take decals" holds by that order. Occlusion culling them
  would put receivers after the decals. They are frustum-culled only.
- **The overlay prepass** (ADR-0021 §5c). Its outline IDs decide hidden information at output
  resolution, so it uses frustum and cone culling only; an occluded unit's ID pixels are still
  removed by its own depth test, as today.
- **The caster and offscreen views.**

So the main view's order is `gbuffer_receivers → gbuffer_decals → gbuffer_rest (phase 1) →
hiz_build → geo_meshlets (phase 2) → gbuffer_rest (phase 2)`.

**5c. The pyramid.**
- `R32_SFLOAT`, mip 0 at half the render resolution, reduced from the render-resolution
  depth; allocated at the dynamic-resolution ceiling like the G-buffer (ADR-0023 §1b) and
  built over the active sub-rectangle.
- With reversed-Z (ADR-0023 §1c) each texel keeps the **minimum** (farthest) depth of its
  footprint; a sphere is occluded when its nearest depth is below the pyramid value over its
  projected rectangle, at the mip where that rectangle covers at most 2 × 2 texels.
- Built in compute with explicit 2 × 2 (and 3-wide at odd edges) reductions, so it needs no
  `samplerFilterMinmax`. `R32_SFLOAT` as a storage image is checked at creation (the required
  format table lists it, but the column mapping was **unverified** in this pass; both local
  GPUs report `STORAGE_IMAGE` for it).
- It is built from the **jittered** depth and tested with the jittered matrix, so the test
  and the raster agree.

**5d. Persistent state and resets.** One visibility bit per (static instance, meshlet) of the
main view: about 12 KB for 5,000 instances of 20 meshlets. Dynamic instances need none. On
`Scene::history_reset` (camera cuts, spectator switches, map loads, resizes; ADR-0021 §2c) the
bits are cleared, so phase 1 draws nothing and phase 2 tests everything against an empty
(far) pyramid: correct, and as slow as no occlusion culling for one frame.

**5e. Default.** HiZ is built in N7d behind `MOBA_HIZ=on|off|spectator`. ADR-0018 rejected it
because "a MOBA camera looks down at a mostly flat board; little occludes", and roadmap §5.4
expects it to pay "mainly at low spectator angles". So its default is set by measurement
(Q3, verification gate 6).

### 6. Animated and procedural geometry

**6a. Skinned meshes: skinned once per frame in compute.**
- `geo_skin` runs after `geo_instances` on the **visible** skinned instances (any view), with
  an indirect dispatch over an ordered work list.
- For each vertex of the instance's mesh it writes, in **model space**, the skinned position,
  normal and tangent with this frame's palette, and the position with the previous palette
  (ADR-0021 §2c's copied previous palette, so nothing new is kept).
- The output is a per-frame-in-flight buffer of `RenderConfig::max_skinned_vertices` (default
  131,072; 64 bytes each, 8 MiB per slot). An instance that does not fit is not drawn and is
  counted, like `skinned_instances_dropped` today.
- Every pass then draws skinned instances as if static, reading the skinned buffer: the
  shadow casters, the G-buffer, the overlay prepass and the offscreen views. The instance
  transform is applied after skinning, so motion is `view_proj × model × p` against
  `prev_view_proj × prev_model × p_prev`, exactly ADR-0021 §2b's rule.
- **Why compute:** today each view skins in its vertex shader, and N1's prepass skins twice.
  Skinning once covers three to four views. It also gives N4 the skinned positions its hero
  BLAS refits need (roadmap §5.5); N4 decides whether it reads them.
- **Meshlets for skinned meshes: yes,** built on the bind-pose topology (§2b), so skinned
  instances share the meshlet path and both back ends. They are culled per instance only
  (§3c), with ADR-0018's widened cull sphere and its ×2 floor unchanged.
- **Discrete LODs stay** for skinned heroes (roadmap §5.4). Bone LOD composes as before.

**6b. VAT minions.**
- VAT textures move from the per-mesh set at 2 into the bindless sampled-image array
  (ADR-0020 §3a already lists VAT there). `MAX_VAT_MESHES` stops limiting Tiered.
- The mesh or vertex shader samples the VAT texture for the current pose and, in the G-buffer
  pass, the previous pose from the previous-instance buffer (ADR-0021 §2b), as N1's prepass
  does.
- VAT is not pre-expanded in compute: minions are many instances of few meshes, and N4 plans
  pose-bucketed shared BLASes for them, not per-instance positions (roadmap §5.5).
- Culled per instance only, with ADR-0018's every-frame sphere.

**6c. Terrain.** At `set_terrain` the heightfield is cut into **7 × 7-cell meshlets**: 8 × 8 =
64 vertices and 98 triangles, which fit §2a's limits exactly. Each gets an AABB-derived
sphere. Terrain meshlets are frustum-culled per view, never occlusion-culled (§5b), and have
no cone (heightfields face up). The terrain stays a single level (ADR-0018 rejected CDLOD;
revisit with the spectator camera).

**6d. Foliage.**
- **Today's procedural blades** keep their vertex shader (blade geometry from the vertex
  index). Culling moves to `geo_instances`: one thread per clump, frustum plus the distance
  cap, ordered compaction, and one `vkCmdDrawIndirect` whose `instanceCount` the GPU writes.
  The clump's sphere is widened by the largest wind and displacer bend, so wind never pushes a
  blade outside its bound. The 16 m CPU chunks remain on Compat.
- **Mesh foliage clusters** (roadmap §9.1, 1–6k triangles) are static instances in the
  general meshlet path: alpha-tested, `NONE` culling, no cone, bounds widened by the material's
  wind amplitude bound. Their wind runs in the shared transform, evaluated twice for motion as
  ADR-0021 §2b says.

**6e. Water, particles, decals and sky.** Unchanged by N7: water is one small mesh, particles
and decals have their own passes, and the sky is a full-screen draw.

### 7. Cluster LOD for static assets

**7a. Scope.** Static, opaque or alpha-tested meshes whose sources are dense: structures,
rocks, props and mesh foliage (roadmap §5.4, §9.1). **Never skinned or VAT meshes.** On the
**Ultra and Overdrive** tiers, and in the zoom, spectator and showcase camera configurations,
as roadmap §3's matrix says (Q5). Every cluster-LOD asset **also ships discrete LODs**
(15–40k-triangle gameplay LOD0 plus `.lod` levels), because Compat, C1 and the Low to High
tiers draw those.

**7b. Building.** `moba-tools mesh-bake clusters <model.glb>` uses meshoptimizer's
`clusterlod.h` through `meshopt::clusterlod::clod_build` [S11][S12]:
- `clusterlod.h` is an MIT single-header library shipped with meshoptimizer since 1.0, "a
  small library/example" for cluster LOD hierarchies, usable as is or as a reference [S11];
- its clusters are 64 vertices / 124 triangles to match §2a (the header accepts 4–256) [S11];
- simplification never adds vertices, so all levels index the LOD0 vertex buffer (index-only,
  as §2b);
- each cluster stores its own error and its group's parent error, both monotonic up the DAG,
  with conservative bounds, as clusterlod.h produces them;
- output `models/<model>.clod` (`MCLD`, version 1): the cluster DAG flattened into §2c records
  plus a 16-byte `ClusterLod` record per cluster (own error, parent error, parent sphere);
  same hash checks and fallbacks as §2d. A missing or stale `.clod` falls back to the discrete
  LODs, never to an error.

**7c. Selection.** In `geo_instances`, per cluster of a visible cluster-LOD instance: the
cluster is drawn when its own projected error is within the tier's tolerance **and** its
parent's is not. That is Nanite's cut rule [S14], with the error projected by ADR-0018 §3's
formula from the cluster's sphere. Monotonic errors make the cut crack-free.
- **N7e starts with a flat test** of every cluster of every visible cluster-LOD instance.
- If that costs more than 0.2 ms on the vertical-slice map on the RTX 5070 Ti, N7e switches to
  a level-by-level traversal of `clodBuildHierarchy`'s BVH (meshoptimizer 1.2) [S11], one
  indirect dispatch per level, recorded as an amendment. A persistent-thread traversal is
  rejected for the forward-progress reason in §3d.

**7d. Memory.** Cluster-LOD data is **resident**: N7 streams nothing.
- **Estimate per source triangle** (to be measured in N7e): vertices at about 0.5 per triangle
  × 48 B (24 B), plus per level about 5.4 B of meshlet indices and records, about twice over
  for the whole DAG (11 B): **about 35 B**, or about 25 B with Q7's 28-byte vertices. That is
  about 35 MB for a 1M-triangle structure. Nanite's own format averages 14.4 B per input
  triangle [S14], with compression and streaming that N7 does not have.
- **Truncated per tier at load.** Levels finer than what the tier's tolerance needs at the
  closest distance its cameras reach are dropped when uploading. The gameplay camera then
  pays only for the levels it can select.
- **A geometry budget per tier** joins `moba_assets::budget`: provisionally 1 GiB of cluster
  data on Ultra and Overdrive, inside ADR-0020 §5e's 12 GiB (Q10). Content over budget drops
  further levels, logged, as textures drop mips today (ADR-0018 §4).
- **Disk.** The `.clod` file may use meshoptimizer's meshlet codec (`meshopt_encodeMeshlet`,
  stable since 1.2, about 5–7 bits per triangle of topology [S10]), decoded at load.
- Streaming the finest levels is not N7's (Q6).

**7e. Readability.** The cut is at the tier's pixel tolerance, so silhouettes change by at
most that error, as discrete LODs do today. Cluster LOD is static decoration only; no unit's
geometry ever changes by it.

### 8. Shaders, pipelines and the vertex path

- **N7 replaces N1's GLSL vertex stages** on Tiered, closing ADR-0023 Q8. New Slang:
  `geometry_cull.slang` (the instance, meshlet, expand and scan entry points), `geometry_skin.slang`,
  `hiz_build.slang`, `geometry_ms.slang` (both workgroup permutations), `geometry_vs.slang`, and
  `shaders/slang/lib/geometry.slang` (records, vertex decode, transform, jitter, motion, wind,
  VAT). Both back ends call the same transform function.
- **Fragment stages** are N2's Slang G-buffer fragments, unchanged, plus depth-only and
  overlay-ID fragments. Their input locations are pinned by the reflection test.
- **Pipelines per back end:** geometry kind (static-or-skinned, VAT, terrain, foliage blades)
  × alpha mode × cull mode × pass (G-buffer, caster, overlay prepass, offscreen), and only the
  device's back end is created. All are created with the renderer.
- **Buckets** are pipeline × pass group (receivers, rest), so the G-buffer group order of
  ADR-0023 §1e is kept.
- **Offscreen views** (portraits, the minimap bake) run `geo_instances` and `geo_meshlets`
  with frustum culling only and no persistent state, so Tiered has one geometry path. Their
  shading is whatever ADR-0023 gives them; only their vertex side changes.
- **Compat's GLSL vertex shaders are untouched** and freeze with Compat.

### 9. Per-tier budgets

**9a. N7's share of ADR-0020 §5b.** Planning estimates on each tier's reference GPU at its
internal resolution, to be replaced by measurements; estimates never close a target.

| Pass (ms) | Low (RTX 2060, 720p) | Medium (RTX 3060, 720p) | High (RTX 4070, 960p) | Ultra (RTX 5070 Ti, 960p / 1080p) |
|---|---|---|---|---|
| `geo_instances` (all views, incl. cluster cut on Ultra) | 0.06 | 0.05 | 0.05 | 0.10 |
| `geo_skin` | 0.03 | 0.02 | 0.02 | 0.02 |
| `geo_meshlets` (both phases, all views) | 0.10 | 0.08 | 0.08 | 0.08 |
| `hiz_build` (when on) | 0.04 | 0.03 | 0.04 | 0.03 |
| `geo_expand` (vertex back end only) | 0.15 | 0.12 | 0.12 | 0.10 |
| **N7 overhead** | **≤ 0.38** | **≤ 0.30** | **≤ 0.31** | **≤ 0.33** |

- The overhead has to be paid for by fewer triangles rasterised and shaded, and by removing
  the per-view CPU bucketing. At today's scene sizes it will not be, which is ADR-0018's
  finding. That is why the binding gate is relative and on two scenes (9b).
- **Medium remains the tight tier** (ADR-0023 §9a).

**9b. What can be measured now.** Only the RTX 5070 Ti is owned. The binding gates are
relative, medians of 3 interleaved rounds (verification gate 6):
- on the **teamfight replay** (today's density), the N7 Tiered frame costs **no more than
  1.03×** the N2 Tiered frame with `MOBA_GPU_CULL=cpu`, on each back end;
- on a **density scene**, a new deterministic `moba-tools` stress scene at roadmap §2's
  photoreal budget (the R5 teamfight units plus about 2,000 static props and structures at
  §9.1's triangle counts, 2–3M triangles in view), the N7 frame is **faster** than the CPU
  path, and the CPU render call shrinks;
- the reference GPUs replace both when they arrive (#9/#127).

**9c. VRAM.** Per renderer: the meshlet pool (about 6 B per triangle per level over discrete
LODs), 2 MiB of visible lists per view, 16 MiB of skinned output (two slots), the pyramid
(about 2 MB at 960p) and, on the vertex back end only, 24–48 MB of packed indices per view. Plus
§7d's cluster data on Ultra and Overdrive. All of it is reported in `docs/perf/`.

### 10. Determinism and the invariant

- Rendering still never touches the simulation, the wire, replays or the state hash. No
  version or hash moves: `PROTOCOL_VERSION` v42, replay v5, `SIM_LOGIC_VERSION` v9, content
  hashes and `tests/golden-state-hash.txt` are unchanged.
- Meshlets, cluster LOD, static instances and visibility bits are client-local presentation
  data. `.meshlets` and `.clod` files are client-local and outside the content hash, like
  `.lod` (ADR-0007).
- Culling only removes what the client passed; the server already withholds hidden units.
  Nothing reads back from the GPU into gameplay. The culling statistics are read back two
  frames late for the frame log only.
- Picking, HUD anchors and the client's `Camera` are untouched (ADR-0021 §8).

## Departures from the roadmap

| Roadmap says | This ADR decides | Why |
|---|---|---|
| §3 matrix: Low "GPU-driven, discrete LODs"; the mesh-shader path from Medium | The back end follows the device and measurement on **every** tier; the vertex back end is everywhere (§4e) | A raster back end is not a quality feature. Gating it by tier would leave a Low device with mesh shaders on the slower path for no reason |
| §5.4: `vkCmdDrawIndexedIndirectCount` | One `vkCmdDrawIndexedIndirect` per bucket over a compacted index buffer (§4c); `drawIndirectCount` stays required by ADR-0020 §1e | Meshlets and cluster LOD need meshlet-level draws, which compaction turns into one command per bucket |
| §5.4: two-phase HiZ | Built; not applied to units, receivers or the overlay prepass; default by measurement (§5b, §5e, Q3) | Fairness, the decal draw order and ADR-0018's finding that little occludes at the gameplay camera |
| §8: N7 depends on N0 and runs in parallel | N7a (bakes, files, static instances, probe) in parallel; N7b onwards after N2a | N7b rewrites the G-buffer pass's vertex stages; porting N1's prepass and the forward scene first would be done twice |
| §5.4: meshlets with cone culling (everywhere) | Cone culling only on closed, opaque, static meshes (§3c) | Every pipeline renders back faces today; cone culling is exact only where back faces cannot be seen |

## Risks

| Risk | Mitigation |
|---|---|
| GPU-driven overhead exceeds its savings at today's counts | The relative gates of §9b; `MOBA_GPU_CULL=cpu` stays on Tiered for one release as the reference (Q11) |
| The mesh back end is slower than the vertex back end on a vendor (RDNA2's task and mesh handling) | No task shaders (§1); per-vendor allow-list set by measurement (Q2); the vertex back end is always available |
| A culled unit (fairness) | Units are never occlusion-culled (§5b); dynamic instances lead the index space, so overflow drops only decoration (§3a, §3e); ADR-0018's widened skinned spheres unchanged |
| Exactness lost (cone, back-face or small-primitive culling changes pixels) | Applied only where provably exact (§3c, §4d); `cull-probe.sh` on Tiered (gate 2) |
| Non-deterministic goldens from compaction order | Ordered reduce-then-scan everywhere (§3d); two lavapipe runs per back end (gate 4) |
| Cluster-LOD VRAM on 8 GB cards | Ultra and Overdrive only; per-tier truncation and a geometry budget (§7d); A3 measures |
| Cluster selection cost | The 0.2 ms trigger for the hierarchical traversal (§7c) |
| Unbaked or stale meshlets (mods) | The in-tree builder (§2e); `content-check` warnings |
| Mesh and vertex back ends drift apart | One shared transform in `lib/geometry.slang`, one culling pass, the back-end parity gate (gate 2) |
| Graph resource limits | Widen the mask to `u128` (§3f), as ADR-0023 already allows |
| Reference-GPU behaviour unknown (mesh shaders on the RTX 2060, Windows AMD) | Synthetic fixtures until the dumps arrive; the vertex back end covers any gap |

## Alternatives considered

- **Task-shader culling** (niagara's design: the task shader culls meshlets and emits mesh
  workgroups [S13]). It saves the visible-list round trip through memory. Rejected for N7:
  culling would exist twice (compute for the vertex back end, task shader for the mesh back
  end) and drift; AMD reports amplification shaders running as compute on the async compute
  queue on RDNA [S8], so the saving there is unclear; and task payloads should stay small on
  NVIDIA (below 108 bytes preferred [S9]). Revisit if the round trip measures above 0.05 ms.
- **One indirect draw per meshlet** for the vertex back end. Kept as the spike's comparison
  (§4c).
- **Per-instance multi-draw with discrete LODs only** (niagara's non-mesh path). It cannot
  draw cluster LOD, so the fallback would lack a feature. Rejected.
- **Atomic (unordered) compaction.** Simpler, but non-deterministic goldens and no exactness
  argument. Rejected (§3d).
- **`VK_EXT_device_generated_commands`** (ratified, revision 1 dated 2024-02-23 [S20]). It
  switches pipelines and state from the GPU; N7 has a few fixed buckets and needs none of
  that. Not adopted.
- **A software rasteriser for small triangles** (Nanite [S14]). Roadmap §2 puts photoreal at
  1–3M visible triangles at 720p–1080p internal, with LOD tolerances of 0.5–2 px. Rejected for
  N7; revisit if quad-overdraw measurements on A3 content say otherwise.
- **A visibility buffer in N7** (roadmap §5.2 step 5). ADR-0023 kept the step open. The
  `VisibleMeshlet` slot plus a 7-bit triangle index fits a 32-bit visibility ID, so N7 keeps
  it cheap to add. Not now (Q8).
- **Linking meshoptimizer into the client** to meshletise at load. Adds a C++ build to the
  default client. Rejected for §2e's builder (Q9).
- **Pre-expanding VAT minions in compute.** Per-instance memory for many instances of few
  meshes, and not what N4 plans. Rejected (§6b).
- **Per-meshlet bounds for skinned meshes** recomputed after skinning. Possible, but heroes are
  small on screen and cone bounds still would not hold. Not now.
- **RTX Mega Geometry clusters** (`VK_NV_cluster_acceleration_structure`, in the registry
  since 2024 [S19]). An N4 option for ray tracing, not a raster decision. Out of scope.

## Consequences

- **Wire, replays and determinism: no change** (§10).
- **Frame shape (Tiered).** New passes `geo_instances`, `geo_skin`, `geo_meshlets`,
  `hiz_build` and, on the vertex back end, `geo_expand`; `gbuffer_rest` splits into two phases
  when HiZ is on; the shadow, overlay prepass and offscreen passes draw from the GPU lists.
  Compat's frame is unchanged.
- **CPU.** The per-view bucketing and LOD selection leave the Tiered render call; it uploads
  the dynamic instances and records a fixed number of indirect draws.
- **Capabilities.** N7 **adds no required capability.** `meshShader` stays optional and
  `taskShader` is no longer needed by the optional group (amending ADR-0020 §1e). As ADR-0020
  Amendment 2 planned, N7a adds `VkPhysicalDeviceMeshShaderPropertiesEXT` to the probe chain
  and the recorded fixtures, and the `rt-no-mesh` synthetic fixture covers the vertex back end.
- **Content.** Two optional sidecar files per model (`.meshlets`; `.clod` for cluster-LOD
  statics), baked by `mesh-bake` and checked by CI like `.lod`. `content-check` gains meshlet
  validation, the `closed` report, cluster-LOD source caps from roadmap §9.1 (structures ≤ 2M,
  props ≤ 300k source triangles) and the geometry budget. No content hash changes.
- **Public API** (additive, `#[non_exhaustive]`):
  - `CullMode::Gpu` (the Tiered default), `MOBA_GPU_CULL=off|cpu|gpu`;
  - `Renderer::set_static_instances`, `StaticInstance`;
  - `RenderConfig::{max_static_instances, max_visible_meshlets, max_visible_triangles,
    max_skinned_vertices, mesh_shaders, hiz}`;
  - `CullStats` gains per-test meshlet counts, `geo_overflow` and the back end;
  - `moba_assets::{MeshMeshlets, MeshClusterLod}`.
- **Licences.** meshoptimizer and `clusterlod.h` are MIT [S10][S11]; `meshopt` is MIT OR
  Apache-2.0 [S12]; both are already in the tree for `mesh-bake`. No vendor SDK is involved.
- **Docs.** `docs/content-authoring.md` (the sidecars and cluster-LOD caps),
  `docs/architecture.md`'s renderer section, `docs/status.md` and `preset.rs`'s geometry
  cells move with the code.
- **Work estimate.** The roadmap's **L**, in six PRs:
  1. **N7a** (parallel with N1/N2): meshlet bake and `.meshlets`, the Rust builder, the
     `closed` flag, terrain meshlets, `set_static_instances` and the client's Tiered switch,
     the mesh-shader property probe and fixtures. No frame change.
  2. **N7b** (after N2a): compute culling with ordered compaction, GPU LOD selection, the
     vertex back end, compute skinning, VAT through bindless, GPU foliage culling, the Slang
     vertex path; `CullMode::Gpu` default on Tiered. Includes the §4c spike.
  3. **N7c:** the mesh back end and its workgroup permutations; small-primitive culling in
     both back ends; the back-end parity gate.
  4. **N7d:** two-phase HiZ and its default (Q3).
  5. **N7e:** cluster LOD for static assets, `.clod`, truncation and the geometry budget.
  6. **N7f** (if Q7 is taken): compressed, split vertex streams on Tiered.

  N7b alone closes "compute culling, indirect draws, meshlets"; N7c closes "mesh shaders";
  N7e closes "cluster LOD".

## Verification and evidence gates

These follow roadmap §10 and the gates of ADR-0018, ADR-0020, ADR-0021 and ADR-0023, applied
to N7.

1. **Data and twins.**
   - Bakes: two runs of `mesh-bake meshlets` and `mesh-bake clusters` are byte-identical, and CI
     re-bakes every sample against the committed files.
   - Unit tests: every meshlet's triangles lie inside its sphere; every triangle of a meshlet
     with a cone faces away from every eye position the cone rejects (sampled); `.meshlets` and
     `.clod` validation rejects each broken invariant; the `closed` flag on fixtures (a cube is
     closed, a plane is not); terrain meshlets tile the heightfield exactly; the §2e builder's
     limits and determinism.
   - The GPU LOD selection matches `cull.rs::LodMetric` on a fixture of instances, read back on
     lavapipe, except within 10^-4 relative of a threshold.
   - The cluster cut has no cracks: on a fixture mesh, every LOD0 triangle is covered exactly
     once by the selected clusters at a sweep of tolerances.
   - The reflection test covers the §2c records and the shared vertex outputs.
2. **Exactness (merge-gating).**
   - `scripts/cull-probe.sh` gains Tiered runs: `MOBA_GPU_CULL=off` (the GPU path with every
     test disabled) against `gpu` must be **byte-identical** on the five probe scenes, with
     frustum, cone, back-face and small-primitive culling on.
   - `MOBA_HIZ=off` against `on` is byte-identical on the probe scenes and on a low-angle
     spectator scene.
   - **Back-end parity:** mesh against vertex back end on lavapipe and the RTX. The target is
     byte-identical; any difference is recorded (pixel count, maximum) and must be at most
     0.01 % of pixels with none on a unit's silhouette in the outline-probe scene.
   - Fairness: a unit at the frustum edge and a unit behind a wall are drawn (or depth-rejected)
     exactly as with `MOBA_GPU_CULL=cpu`.
3. **Readability.**
   - The §4 contrast tests and N2's readability gates pass unchanged; culling must not move
     them.
   - `outline-probe.sh` on Tiered: an occluded unit's frame is byte-identical with HiZ on and off.
4. **Goldens.**
   - Every Compat golden is byte-identical (`MOBA_RENDER_PATH=compat`).
   - Tiered goldens rebaseline once, deliberately (back-face culling of closed meshes, compute
     skinning's operation order), with the reasons in the PR.
   - New tiered goldens on lavapipe for **both back ends** (lavapipe has mesh shaders): the
     default scene, the density scene at small resolution and a cluster-LOD scene; each
     byte-identical across two runs, with validation and sync validation clean.
5. **Validation.** Clean (0 messages) with validation and sync validation on lavapipe and the
   RTX 5070 Ti, for both back ends, HiZ on and off; and a correctness smoke run of both back
   ends on the Radeon 610M (RADV), which is not a performance reference.
6. **Performance.**
   - Per-pass lanes for every N7 pass in the frame log; medians of 3 interleaved rounds on the
     RTX 5070 Ti against a detached build of `origin/main`.
   - §9b's relative gates: ≤ 1.03× on the teamfight replay per back end; faster than the CPU
     path on the density scene.
   - **Mesh against vertex back end** on the density scene: the result sets the NVIDIA entry
     of Q2's allow-list.
   - **HiZ:** saving on the low-angle spectator scene and cost at the gameplay camera, which set
     Q3's default.
   - Cluster selection under 0.2 ms on the vertical-slice map, or the §7c switch.
   - `geo_overflow=0`, `locked_uploads=0`, `locked_pipelines=0`, VRAM against §9c, and the
     pacing rule (p99 ≤ 1.5 × p50, R8).
   - Tiered budgets keep failing `perf-gate` (from N1).
7. **Capabilities.** Admission tests on every fixture, including `rt-no-mesh` (Tiered, vertex
   back end) and the recorded fixtures with mesh-shader properties. Each reference GPU's dump
   is added when it arrives; the RTX 2060 and RX 6600 dumps settle ADR-0020 Q3 for the floor.

## Open questions for the maintainer

1. **Keep mesh shaders optional now that both back ends exist?** *Recommendation:* yes, for
   good. The vertex back end is also the reference and the measurement baseline, and Alan
   Wake 2 had to add a non-mesh-shader path after launch [S15]. Revisit only if the vertex back
   end becomes a maintenance burden.
2. **Where should `auto` pick the mesh back end?** *Recommendation:* per vendor, only once
   that vendor's real hardware measures it at least at parity with the vertex back end:
   NVIDIA from the RTX 5070 Ti in N7c; AMD only when an RDNA card is on hand (the 610M checks
   correctness, not speed); Intel when tested. Until then, the vertex back end.
3. **Two-phase HiZ by default?** *Recommendation:* on wherever it saves at least 0.05 ms on the
   low-angle spectator scene and costs at most 0.02 ms at the gameplay camera on the 5070 Ti;
   otherwise on only for the spectator, zoom and showcase configurations.
4. **Should dynamic units ever be occlusion-culled?** *Recommendation:* no. They are few, poor
   occluders and the fairness-critical case.
5. **Cluster LOD on High as well as Ultra and Overdrive?** *Recommendation:* not yet. High's
   cards have 8 GB (ADR-0020 §5e); decide after A3 measures the geometry memory.
6. **Geometry streaming of the finest cluster levels: N11's scope?** *Recommendation:* not in
   N7, which keeps cluster data resident and truncated per tier. Add it to N11 only if A3's map
   exceeds the geometry budget.
7. **Compress the Tiered vertex data (N7f)?** A 12-byte position stream plus a 16-byte
   attribute stream (octahedral 16-bit normal and tangent, `f32` UVs), against today's 48
   bytes. *Recommendation:* yes, after N7c, gated by a normal-error test (it must stay well
   below the G-buffer's 10-bit normals, ADR-0023 §1a). It cuts static vertex memory by about
   40 %, and a tight position stream suits N4's BLAS builds.
8. **Build the visibility buffer (roadmap §5.2 step 5) in N7?** *Recommendation:* no. N7
   keeps the ID cheap to add; build it only when measured G-buffer overdraw or material cost
   justifies it, as ADR-0023 said.
9. **Unbaked models: an in-tree Rust meshletiser, or meshoptimizer linked into the client?**
   *Recommendation:* the Rust builder (§2e). It keeps C++ out of the client build, and baked
   content still gets meshoptimizer's quality.
10. **The cluster-LOD geometry budget: 1 GiB on Ultra and Overdrive?** *Recommendation:* accept
    it as provisional; A3 replaces it with a measurement, as ADR-0020 Q10 did for VRAM.
11. **Keep `MOBA_GPU_CULL=cpu` on Tiered?** *Recommendation:* yes, as the reference for one
    release after N7b, then remove it from Tiered, as ADR-0023 does with `MOBA_DEFERRED=off`.

## Corrections to the roadmap

These were re-checked on 2026-11-03. This PR only adds a link to this ADR from roadmap §12;
the maintainer may fold the corrections in when this ADR is accepted.

- **§5.4, "Skeletal Nanite is still experimental in UE 5.8".** The UE 5.8 Nanite documentation
  has a "Nanite Skeletal Mesh" section with no Experimental or Beta label, and no primary
  source for its maturity was found; secondary sources disagree [S14]. Its status is
  **unverified**. The decision (discrete LODs for skinned heroes) does not depend on it.
- **§2 and §9.1, "about 14 bytes per source triangle in Nanite's format".** Epic's documentation
  gives **14.4 bytes** per input triangle on average [S14]. That figure is Nanite's
  compressed, streamed format; N7's resident format is estimated at about 25–35 bytes (§7d).
- **§5.4, meshoptimizer `clusterlod.h`.** meshoptimizer is at **v1.3** (2026-09-25).
  `clusterlod.h` is an official MIT single-header library in `demo/` since v1.0, not an
  experiment; `clodBuildHierarchy` arrived in v1.2 [S10][S11]. The `meshopt` crate 0.7.0
  (2026-10-17) binds it and is already in `Cargo.lock` [S12].
- **§5.1 and ADR-0020 Q3, mesh shaders at the tier floor.**
  - **RX 6600 (RDNA2):** RADV enabled `VK_EXT_mesh_shader` by default on RDNA2 in Mesa 23.0.0
    (2023-02-22) [S7]. The Windows driver version that added it is **unverified**.
  - **RTX 2060 (Turing):** NVIDIA shipped `VK_EXT_mesh_shader` in beta drivers 517.22
    (Windows) and 515.49.15 (Linux) on 2022-09-01, and Turing introduced mesh shading [S6]. User-submitted reports show
    the extension from that day [S21], but no primary source names the RTX 2060 itself, and the first non-beta driver is
    **unverified**.
  - So the floor very likely has mesh shaders, but the reference dumps still decide Q3.
- **Confirmed unchanged:** `VK_EXT_mesh_shader` is ratified (Khronos release 2022-08-31) and
  depends on Vulkan 1.2 or `VK_KHR_spirv_1_4` [S1]; `drawIndirectCount` is optional in Vulkan
  1.2–1.4 and in Roadmap 2022 and 2024, which is why ADR-0020 gates it [S3]; Alan Wake 2
  shipped GPU-driven meshlet rendering with mesh shaders and later added a path for GPUs
  without them [S15]; id's DOOM: The Dark Ages talk describes a visibility buffer and
  "GPU-based triangle culling", without mentioning meshlets [S16].

## Sources

All accessed 2026-11-03.

- [S1] `VK_EXT_mesh_shader` (extension 329, ratified, revision 1 2022-03-08; dependencies;
  features; commands):
  <https://docs.vulkan.org/refpages/latest/refpages/source/VK_EXT_mesh_shader.html>,
  <https://docs.vulkan.org/refpages/latest/refpages/source/VkPhysicalDeviceMeshShaderFeaturesEXT.html>;
  Khronos, "Khronos releases Vulkan mesh shader pipeline extension" (2022-08-31):
  <https://www.khronos.org/news/permalink/khronos-releases-vulkan-mesh-shader-pipeline-extension>;
  C. Kubisch, "Mesh Shading for Vulkan" (2022-09-01): <https://www.khronos.org/blog/mesh-shading-for-vulkan>.
  The release version 1.3.226 is from secondary sources only (**unverified**) and not relied on.
- [S2] Vulkan specification, limits: the mesh-shader required limits (`maxMeshOutputVertices`
  and `maxMeshOutputPrimitives` 256, `maxMeshOutputMemorySize` 32,768,
  `maxMeshOutputComponents` 128, `maxMeshWorkGroupCount` 65,535 per dimension,
  `maxMeshWorkGroupTotalCount` 2^22, `maxTaskPayloadSize` 16,384), `subPixelPrecisionBits` ≥ 4,
  `maxComputeWorkGroupInvocations` ≥ 128, `maxStorageBufferRange` ≥ 2^27,
  `maxDrawIndirectCount`, and the core subgroup minimums (`BASIC`, compute only):
  <https://docs.vulkan.org/spec/latest/chapters/limits.html>,
  <https://docs.vulkan.org/refpages/latest/refpages/source/VkPhysicalDeviceMeshShaderPropertiesEXT.html>
- [S3] Indirect draws: <https://docs.vulkan.org/refpages/latest/refpages/source/vkCmdDrawIndexedIndirectCount.html>,
  <https://docs.vulkan.org/refpages/latest/refpages/source/vkCmdDrawMeshTasksIndirectCountEXT.html>,
  <https://docs.vulkan.org/refpages/latest/refpages/source/vkCmdDrawMeshTasksIndirectCount2EXT.html>,
  <https://docs.vulkan.org/refpages/latest/refpages/source/VK_VERSION_1_2.html>,
  <https://docs.vulkan.org/refpages/latest/refpages/source/VK_VERSION_1_4.html>,
  <https://docs.vulkan.org/spec/latest/appendices/roadmap.html>
- [S4] Vulkan specification, primitive order (indirect commands in storage order) and mesh
  shading primitive ordering (workgroups in order; primitives by the primitive index buffer):
  <https://docs.vulkan.org/spec/latest/chapters/drawing.html>,
  <https://docs.vulkan.org/spec/latest/chapters/VK_NV_mesh_shader/mesh.html>
- [S5] Synchronisation names and pipeline restrictions:
  <https://docs.vulkan.org/refpages/latest/refpages/source/VkPipelineStageFlagBits2.html>,
  <https://docs.vulkan.org/refpages/latest/refpages/source/VkAccessFlagBits2.html>,
  <https://docs.vulkan.org/spec/latest/chapters/pipelines.html> (mesh and vertex stages cannot
  mix, VUID-VkGraphicsPipelineCreateInfo-pStages-02095)
- [S6] NVIDIA Vulkan driver page ("New: VK_EXT_mesh_shader", 2022-09-01, 517.22 / 515.49.15
  betas): <https://developer.nvidia.com/vulkan-driver>; C. Kubisch, "Introduction to Turing
  Mesh Shaders" (2018-09-17): <https://developer.nvidia.com/blog/introduction-turing-mesh-shaders/>
- [S7] Mesa 23.0.0 release notes ("Enabled VK_EXT_mesh_shader by default on RADV/RDNA2"):
  <https://docs.mesa3d.org/relnotes/23.0.0.html>; Mesa 23.1.0 (GFX11):
  <https://docs.mesa3d.org/relnotes/23.1.0.html>; AMDVLK v-2023.Q2.1:
  <https://github.com/GPUOpen-Drivers/AMDVLK/releases/tag/v-2023.Q2.1>
- [S8] M. Oberberger, M. Kuth, Q. Meyer, "Mesh shaders on AMD RDNA graphics cards" (GPUOpen,
  2023–2024), in particular "Optimization and best practices" (2024-01-16): 128/256 meshlets,
  64/126 similar, workgroups of 128, one vertex and one primitive per thread, amplification
  shaders running as compute: <https://gpuopen.com/learn/mesh_shaders/mesh_shaders-optimization_and_best_practices/>,
  <https://gpuopen.com/learn/mesh_shaders>
- [S9] NVIDIA, "Advanced API Performance: Mesh Shaders" (2021-10-25): 64 vertices / 126
  primitives, task payload below 108 bytes preferred:
  <https://developer.nvidia.com/blog/advanced-api-performance-mesh-shaders/>; C. Kubisch, "Using
  Mesh Shaders for Professional Graphics" (2020-12-08):
  <https://developer.nvidia.com/blog/using-mesh-shaders-for-professional-graphics>
- [S10] meshoptimizer (MIT): v1.3 (2026-09-25) and release history; README (meshlet sizes,
  `cone_weight`, the meshlet codec); `meshoptimizer.h` (limits ≤ 256 vertices and ≤ 512
  triangles; v1.0 removed the triangle alignment rule):
  <https://github.com/zeux/meshoptimizer/releases/tag/v1.3>,
  <https://api.github.com/repos/zeux/meshoptimizer/releases>,
  <https://raw.githubusercontent.com/zeux/meshoptimizer/master/README.md>,
  <https://raw.githubusercontent.com/zeux/meshoptimizer/master/src/meshoptimizer.h>
- [S11] meshoptimizer `demo/clusterlod.h` (MIT; `clodBuild`, `clodBuildHierarchy`, defaults):
  <https://raw.githubusercontent.com/zeux/meshoptimizer/master/demo/clusterlod.h>
- [S12] The `meshopt` crate 0.7.0 (2026-10-17, MIT OR Apache-2.0; meshoptimizer 1.3, clusterlod
  bindings): <https://crates.io/api/v1/crates/meshopt>,
  <https://raw.githubusercontent.com/gwihlidal/meshopt-rs/master/CHANGES.md>,
  <https://docs.rs/meshopt/latest/meshopt/all.html>
- [S13] A. Kapoulkine: niagara (MIT; task-shader meshlet culling and two-phase occlusion in
  `meshlet.task.glsl`): <https://github.com/zeux/niagara>,
  <https://raw.githubusercontent.com/zeux/niagara/master/src/shaders/meshlet.task.glsl>;
  "Meshlet size tradeoffs" (2023-01-16): <https://zeux.io/2023/01/16/meshlet-size-tradeoffs/>
- [S14] B. Karis, R. Stubbe, G. Wihlidal, "A Deep Dive into Nanite Virtualized Geometry",
  SIGGRAPH 2021 Advances in Real-Time Rendering: <https://advances.realtimerendering.com/s2021/index.html>
  (the slides were not read; slide-level details such as 128-triangle clusters and the
  software rasteriser are **unverified** here and not relied on); Epic, "Nanite Technical
  Details" (14.4 bytes per input triangle; two-pass occlusion culling):
  <https://dev.epicgames.com/documentation/unreal-engine/nanite-technical-details>; Epic, "Nanite
  Virtualized Geometry" (UE 5.8, "Nanite Skeletal Mesh"):
  <https://dev.epicgames.com/documentation/en-us/unreal-engine/nanite-virtualized-geometry-in-unreal-engine>
- [S15] Remedy: E. Jansson, "GPU-driven rendering with mesh shaders in Alan Wake 2", Digital
  Dragons 2024: <https://presentations.remedy.fi/gpu-driven-rendering-with-mesh-shaders-in-alan-wake-2-dd2024.ppsx>
  (listed in <https://remedygames.com/article/explore-remedys-gdc2024-talks-on-creating-alan-wake-2>);
  "How Northlight makes Alan Wake 2 shine" (2023-11-06, meshlet and occlusion culling):
  <https://www.remedygames.com/article/how-northlight-makes-alan-wake-2-shine>; Alan Wake 2
  patch notes, PC 1.0.16.1 (2024-03-06, "Optimized rendering for GPU's that don't support mesh
  shaders"): <https://www.alanwake.com/story/alan-wake-2-patch-notes/>. That launch excluded
  GPUs without mesh shaders is from press reports only (**unverified**).
- [S16] P. Hammer, D. Lazarek, "Rip & Tear: Breaking Down the Rendering of 'DOOM: The Dark
  Ages'", GDC 2026 (abstract): <https://gdcvault.com/play/1035744/Rip-Tear-Breaking-Down-the>
- [S17] U. Haar, S. Aaltonen, "GPU-Driven Rendering Pipelines", SIGGRAPH 2015 Advances in
  Real-Time Rendering: <https://advances.realtimerendering.com/s2015/index.html>; G. Wihlidal,
  "Optimizing the Graphics Pipeline With Compute", GDC 2016:
  <https://gdcvault.com/play/1023109/Optimizing-the-Graphics-Pipeline-With> (the deck's
  specific culling techniques were not read: **unverified**, cited as background only)
- [S18] T. Sorensen, L. F. Salvador, H. Mittal, H. Evrard, A. F. Donaldson, "Specifying and
  Testing GPU Workgroup Progress Models", OOPSLA 2021 (Vulkan and Metal say almost nothing about
  inter-workgroup forward progress): <https://multicore.doc.ic.ac.uk/publications/gpu-oopsla-21.html>,
  <https://arxiv.org/abs/2109.06132>. The current spec text on forward progress was not read.
- [S19] `VK_NV_cluster_acceleration_structure` (revisions 2024-08-29 to 2025-07-16):
  <https://docs.vulkan.org/refpages/latest/refpages/source/VK_NV_cluster_acceleration_structure.html>;
  NVIDIA, "RTX Mega Geometry Now Available with New Vulkan Samples" (2025-02-06):
  <https://developer.nvidia.com/blog/nvidia-rtx-mega-geometry-now-available-with-new-vulkan-samples>
- [S20] `VK_EXT_device_generated_commands` (ratified, revision 1 2024-02-23):
  <https://docs.vulkan.org/refpages/latest/refpages/source/VK_EXT_device_generated_commands.html>
- [S21] Vulkan Hardware Database (user-submitted reports; evidence, not a vendor statement):
  `VK_EXT_mesh_shader` first reported 2022-09-01; `maxMeshOutputVertices` 256 in most reports:
  <https://vulkan.gpuinfo.org/displayextensiondetail.php?extension=VK_EXT_mesh_shader>
- **Local evidence:** `vulkaninfo` on 2026-11-03: RTX 5070 Ti (NVIDIA 615.78.08), Radeon 610M
  (RADV, Mesa 26.2.5) and the bundled lavapipe (Mesa 26.2.5, LLVM 23.1.3); the recorded
  capability fixtures in `crates/render/src/caps/fixtures/` (2026-11-01).
- **Project inputs:** `crates/render/src/{cull.rs, mesh.rs, scene.rs, passes/scene.rs, terrain.rs,
  environment.rs, vat.rs, pipeline.rs}`, `crates/shaders/shaders/{skinned,vat}.vert`,
  `crates/client/src/app/{scene_build.rs, map_visuals.rs}` at `e9d5f91`.
