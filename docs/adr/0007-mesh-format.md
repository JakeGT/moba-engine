# ADR-0007: Mesh asset format

**Status:** accepted 2026-10-27 (WP-3.2, #286).

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
- **Scope:** static triangle meshes. `POSITION` (`f32` VEC3), optional `NORMAL` (`f32` VEC3)
  and optional `u8`/`u16`/`u32` indices, all widened to `u32`. Node transforms of the default
  scene are baked into the vertices, and all primitives merge into one mesh. Missing normals
  become flat face normals, as the glTF specification requires. Materials, textures, skins,
  animations and morph targets are ignored. Sparse accessors and any `extensionsRequired`
  entry (for example Draco or meshopt compression) are rejected.
- **Untrusted-input limits** (`moba-assets`, feature `glb`):
  - file ≤ 16 MiB (checked from metadata before reading) and JSON chunk ≤ 1 MiB, so the
    `serde_json` heap stays a small multiple of the input;
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
  centred, and it faces +Z (the glTF convention), rotated to the movement direction. Until
  packs name a model per hero (#285), `MOBA_HERO_MODELS=hero=model,...` (or a bare model name
  for every hero) provides the names.

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

## Consequences and the path to skinning (WP-4.1)
- `MeshData` grows optional per-vertex streams (`JOINTS_0` as `R8G8B8A8_UINT`, `WEIGHTS_0` as
  `R8G8B8A8_UNORM`) plus one skin and one clip. The loader reads them with the same accessor
  helper and the same caps; joints get a cap (64) of their own.
- Skinned meshes need a second vertex layout and `skinned.vert`. `pipeline::create` gains a
  vertex-layout parameter. The bucketing stays the same, keyed by mesh, with a per-draw push
  constant `{first_bone, joint_count}` on the shared pipeline layout.
- Mesh slots are never freed. A future unload path needs deferred deletion keyed by frame
  fence.
- Model files are cosmetic and differ per client by design. They are not hashed, signed or
  sent on the wire; #285 decides whether model *names* in packs are.
