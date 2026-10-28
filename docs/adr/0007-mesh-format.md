# ADR-0007: Mesh asset format

**Status:** accepted 2026-10-27 (WP-3.2, #286); amended 2026-10-27 for GPU skinning (WP-4.1,
#288; see [Skinning](#skinning-wp-41)).

## Context
The renderer drew only procedural shapes: instanced cubes and the six-cube hero rig. M8 needs
packs to reference real models (#285), and M3 needs skinned meshes (#288, WP-4.1). Model files
live in a **client-local** asset directory: network clients never receive pack files, only the
wire catalogue, so a model is a *name* that each client resolves on its own disk. That makes
every model file untrusted input (a player, or a mod they downloaded, controls it) and makes
visuals purely cosmetic: nothing about a model reaches the simulation, the wire, replays or the
content hash.

## Decision
- **Format:** binary glTF 2.0 (`.glb`) only. One self-contained file per model, with the
  geometry in the embedded `BIN` chunk. Text `.gltf`, external `.bin` buffers, data URIs and
  every other buffer `uri` are rejected; the loader never opens a second file.
- **Parser:** the [`gltf`](https://crates.io/crates/gltf) crate, pinned exactly
  (`gltf = "=1.4.1"`) with `default-features = false`. That drops the `import` feature
  (file-system and URI loading, `base64`, `urlencoding`) and `image` (texture decoding), so the
  only parsing dependency is `gltf-json` on `serde_json`. Licences, checked by hand against
  `deny.toml` (CI's `cargo deny` re-checks):

  | Crate | Version | Licence |
  |---|---|---|
  | `gltf`, `gltf-json`, `gltf-derive` | 1.4.1 | MIT OR Apache-2.0 |
  | `byteorder` | 1.5.0 | Unlicense OR MIT |
  | `inflections` | 1.1.1 | MIT |
  | `lazy_static` | 1.5.1 | MIT OR Apache-2.0 |
  | `serde_json` (already in the tree) | 1.0.154 | MIT OR Apache-2.0 |

  The loader uses only `gltf::json` (the deserialised document). It splits the GLB container
  itself and reads accessors itself through bounds-checked slices, because `gltf`'s own GLB
  splitter computes `header.length - 12` unchecked (a debug-build overflow panic on a short
  length) and its accessor helpers `unwrap()` on validation assumptions.
- **Scope:** triangle meshes, optionally skinned. `POSITION` (`f32` VEC3), optional `NORMAL` (`f32` VEC3)
  and optional `u8`/`u16`/`u32` indices, all widened to `u32`. Node transforms of the default
  scene are baked into the vertices, and all primitives merge into one mesh. Missing normals
  become flat face normals, as the glTF specification requires. Materials, textures and
  morph targets are ignored; skins and one animation clip are read since WP-4.1 (see
  [Skinning](#skinning-wp-41)). Sparse accessors and any `extensionsRequired`
  entry (for example Draco or meshopt compression) are rejected.
- **Untrusted-input limits** (`moba-assets`, feature `glb`):
  - file ≤ 16 MiB (checked from metadata before reading) and JSON chunk ≤ 256 KiB. A hostile
    chunk of empty objects still expands to tens of MiB of parsed structs, but only
    transiently and only at load time;
  - ≤ 65,536 vertices and ≤ 393,216 indices (128 Ki triangles), checked *before* reserving;
  - node walk ≤ 1,024 visits and depth ≤ 32 (cycles fail), ≤ 4,096 primitives inspected;
  - every offset, stride (≤ 252) and count is checked arithmetic; all indices must be in
    range and all values finite.
  The result is `moba_assets::MeshData`, whose private fields can only be built through a
  constructor that enforces these invariants, so the renderer uploads it without re-checking.
  `load_glb` never panics: unit tests cover each malformed case, a deterministic test
  truncates and corrupts the sample file, and the `glb` cargo-fuzz target exercises it.
- **Renderer:** `Renderer::register_mesh(&MeshData) -> MeshId` interleaves the mesh into the
  existing `{pos, normal}` vertex (stride 24), uploads through the existing staging +
  `synchronization2` path, and waits for the queue. It is load-time only (startup or content
  reload). Slot 0 is the built-in cube; 64 slots in total; meshes are never freed while the
  renderer lives, so a frame in flight cannot reference a freed buffer. `UnitInstance.mesh`
  selects the mesh (default `MeshId::CUBE`). Each frame the renderer counting-sorts instances
  by mesh into the existing instance SSBO using fixed-size arrays (no allocation), then issues
  one indexed draw per non-empty mesh with `firstInstance` at that mesh's run. The shader
  already indexes the SSBO with `gl_InstanceIndex`, which includes `firstInstance`, so the
  pipeline, descriptors and shaders are unchanged. `RenderConfig::max_instances` is shared by
  all meshes. A scene that uses only the cube produces the same single draw and the same
  instance order as before, so the default golden image is unchanged.
- **Client resolution:** a model name (1–32 of `[A-Za-z0-9_-]`) resolves to
  `<MOBA_ASSET_ROOT>/models/<name>.glb`, with `content` as the default root. Each distinct
  name is loaded once. Any failure (bad name, missing or malformed file, upload error, slot
  exhaustion) is logged once and that hero keeps the procedural rig; a missing asset never
  fails a match. The model is fitted to the rig's height (3.6 m), standing on the ground and
  centred, and it faces +Z (the glTF convention), rotated to the movement direction. The names
  come from the catalogue's per-hero `model` (#285, `[assets]` in the pack); the dev override
  `MOBA_HERO_MODELS=hero=model,...` (or a bare model name for every hero) wins for the heroes it
  matches.
- **Map meshes (#287):** a map visuals descriptor (`<root>/maps/<name>.toml`, TOML rather than
  glTF because it carries colours and placements, not geometry) names wall and prop meshes
  that resolve the same way, `<root>/models/<mesh>.glb`, through the same loader and
  `register_mesh`, at most 4 per descriptor. They are uploaded only when the map changes,
  cached per name, and a failure draws the built-in block. See
  [content authoring](../content-authoring.md#map-visuals).

## Rejected
- **Text `.gltf` or URI buffers:** the loader would need file-system or URI access driven by
  untrusted JSON, which means path-traversal and resource-exhaustion surface for no benefit.
- **`gltf` default features:** `import` reads arbitrary paths and data URIs, and `image`
  pulls in the full image-decoder stack. Neither is needed for geometry.
- **A baked `.mesh` format with a `moba-tools` baker:** it is optional in the plan. A baker
  still needs a glTF reader, and a second format doubles the validation surface. Revisit if
  load time or file size matters.
- **OBJ/FBX:** OBJ has no skinning (WP-4.1 needs it), and FBX is proprietary.
- **Per-mesh instance buffers or indirect draws:** extra descriptor sets or buffers per mesh
  for no gain at this scale. One SSBO plus `firstInstance` keeps the descriptor layout fixed.

## Skinning (WP-4.1)
- **Data.** `MeshData` carries an optional `MeshSkin`: `JOINTS_0` (`u8`/`u16` in the file,
  stored as `R8G8B8A8_UINT`) and `WEIGHTS_0` (`f32` or normalised `u8`/`u16`, re-normalised
  and quantised to `R8G8B8A8_UNORM` summing to exactly 255, or all zero) per vertex, plus a
  `Skeleton`: ≤ 64 joints, parent links (any order; a parents-first order is computed),
  rest TRS, the static transform of non-joint ancestors, inverse bind matrices (identity if
  absent) and at most one `AnimationClip`. Everything is built through validating
  constructors, like `MeshData`.
- **Loader.** All mesh nodes must share one skin (or none); skinned vertices stay in bind
  space (the skinned node's transform is ignored, per glTF). The clip is the first animation
  that drives a joint: translation/rotation/scale, `f32` keys, `LINEAR`/`STEP`
  (`CUBICSPLINE` keeps only its values and plays linearly). Caps: 64 joints, 4,096 keys per
  track and 65,536 per clip (checked before reserving), 600 s, finite non-decreasing times.
  Animation channels count against the 4,096-entry inspection cap. `JOINTS_1`/`WEIGHTS_1`,
  morph targets and joints below a non-joint node inside the skeleton are not supported.
  Animation names are not read (`gltf`'s `names` feature stays off), so the first clip is
  treated as locomotion.
- **Renderer.** `register_skinned_mesh` uploads the 32-byte `SkinnedVertex`
  `{pos, normal, joints, weights}` through the same staging path. `skinned.vert` reuses
  `unit.frag`; `pipeline::create` takes a vertex layout. Set 1 gains binding 1, a per-frame
  `CpuToGpu` bone SSBO of three `vec4` rows per joint (default 16 instances × 64 joints =
  48 KiB per frame); the pool's `STORAGE_BUFFER` count is `2 × FRAMES_IN_FLIGHT`. A 16-byte
  vertex-stage push constant `{first_bone, joint_count}` lives on the **shared** pipeline
  layout, so the bound descriptor sets stay compatible across terrain, unit and skinned
  pipelines. Skinned instances still occupy their mesh's bucket in the instance SSBO but are
  drawn one per draw (instance count 1, `firstInstance` = slot) so each can push its range.
  An instance whose range is empty or reaches past this frame's bones is skipped and counted
  (`skinned_instances_dropped`); bones past `max_bones` are counted (`bones_truncated`). The
  shader clamps every joint index below `joint_count`, and the renderer guarantees
  `joint_count ≥ 1` and `first_bone + joint_count ≤ uploaded`, so no bone read leaves the
  frame's upload. One `BufferMemoryBarrier2` (HOST/HOST_WRITE → VERTEX_SHADER/
  SHADER_STORAGE_READ) precedes `cmd_begin_rendering`; the frame fence covers
  write-after-read. `RenderConfig::skinning = false`, or a skinned pipeline that fails to
  build, makes `register_skinned_mesh` return `SkinningUnavailable`.
- **Client.** A model with a skin is uploaded twice (static fallback + skinned, two of the
  64 slots). The pose comes from presentation state the client already has (interpolated
  movement → facing and gait, and the render tick): the clip at weight `gait`, the rest pose
  when idle, with a per-unit phase offset. No wire fields change. A preallocated
  `BonePool` (the renderer's bone budget) is cleared each frame; a hero that does not fit is
  counted and drawn static for that frame. `MOBA_NO_SKINNING=1` skips the skinned upload.
- **Sample.** `sample-mesh` also writes `content/models/sentinel-skinned.glb`: the sentinel's
  geometry bound to a 7-joint skeleton with a 0.8 s walk clip, generated from `+ - * / sqrt`
  only and byte-compared by a test.

## Consequences
- Mesh slots are never freed. A future unload path needs deferred deletion keyed by frame
  fence.
- Model files are cosmetic and differ per client by design. They are not hashed, signed or
  sent on the wire; #285 decides whether model *names* in packs are.
