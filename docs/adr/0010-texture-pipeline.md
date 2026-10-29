# ADR-0010: Texture pipeline and materials

**Status:** accepted 2026-10-28 (AAA graphics roadmap R1b).

## Context
Until R1b the renderer had no material textures: units were lit vertex colours, and the only
texture was the UI atlas. The [AAA graphics roadmap](../plans/aaa-graphics-roadmap.md) (R1.4,
§6) asks for a material system (shader permutation + parameter block + texture set) and an
offline baker that produces KTX2 containers with BC7 (colour), BC5 (normals) and BC4 (masks),
mip chains and explicit sRGB/linear rules. R2 then builds PBR on top, so the texture set and
parameter block must already have the slots R2 needs.

Constraints that shaped the decision:

- **Untrusted input.** Textures and materials live in the client-local asset root, which a
  player or a downloaded mod controls (ADR-0007 has the same threat model for meshes).
- **No C/C++ toolchain.** The workspace builds with `cargo` plus `glslc`; `deny.toml` checks
  licences. A baker that needs ISPC or a C++ compiler would be a new build dependency for
  every contributor and CI runner.
- **Every device we ship on.** Desktop GPUs, lavapipe and Apple Silicon expose
  `textureCompressionBC`, but it is an optional feature, so there must be a path without it.
- **Determinism is untouched.** Materials are cosmetic and client-local, like `MapVisuals`.

## Decision

### Container and formats
- **KTX 2.0, no supercompression.** One 2D image per file, a full or partial mip chain, the
  Khronos basic data format descriptor. The loader (`moba_assets::load_ktx2`) accepts eight
  formats, one per role and encoding:

  | Role (texture slot) | Shipped (BC) | 8-bit equivalent | Encoding |
  |---|---|---|---|
  | base colour | `BC7_SRGB_BLOCK` | `R8G8B8A8_SRGB` | sRGB colour, linear alpha |
  | normal (tangent XY) | `BC5_UNORM_BLOCK` | `R8G8_UNORM` | linear; Z reconstructed |
  | metallic (R) / roughness (G) | `BC5_UNORM_BLOCK` | `R8G8_UNORM` | linear |
  | team-colour mask | `BC4_UNORM_BLOCK` | `R8_UNORM` | linear |

  `BC7_UNORM_BLOCK` and `R8G8B8A8_UNORM` load too (for tools and non-colour RGBA data), but no
  material slot takes them.
- **sRGB/linear rules are in the format, not in a flag.** Colour is sRGB-encoded and every
  other channel is linear; the sampler decodes sRGB in hardware. Three checks enforce it: the
  KTX2 descriptor's transfer function must match the format's sRGB-ness (a mismatch is
  `TextureError::BadDescriptor`), each material slot accepts only its own formats
  (`TextureSlot::accepts`, checked by `content-check`, the client and the renderer), and the
  baker picks the format from `--kind`, so a normal map cannot be baked as sRGB colour.
- **Mips are baked, never generated at load.** The baker box-filters colour in *linear* light
  (decode sRGB, average, re-encode), averages and renormalises normals, and averages data and
  masks as stored. Files are written smallest level first with `lcm(block, 4)` alignment, as
  the specification asks.
- **Bounded parsing.** Width and height `1..=4096`, at most 13 levels (the full chain), one
  layer, one face, no depth, `typeSize` 1, no supercompression, a file of at most 64 MiB. Every
  level's offset and length is checked against the file and against the exact size its
  format and extent need **before** anything is allocated. A `levelCount` of 0 ("generate at
  load") is read as one level. `cargo-fuzz` target `ktx2` (with a corpus) and unit tests
  (every truncation, every single-bit flip of a sample file) cover it.

### Encoder: in-house BC7 mode 6, BC5 and BC4
`moba-tools texture-bake` (PNG → KTX2) encodes with a small in-house encoder
(`crates/tools/src/bin/texture-bake/bc.rs`, about 300 lines, safe Rust, deterministic):

- **BC7 mode 6 only:** one subset, 7-bit RGBA endpoints plus a p-bit each, 4-bit indices.
  Endpoints start from the block's principal axis, get two least-squares refinements against
  the chosen indices, and all four p-bit pairs are tried. This is the "quick" BC7 class
  (DirectXTex `BC7_QUICK` also uses mode 6). On the shipped sample set, level 0 decodes at
  **51.7 dB** PSNR (base colour), **50.6 dB** (BC5 normal), **53.9 dB** (BC5 metallic/
  roughness) and is exact for the BC4 mask, all measured with the same decoder the runtime
  fallback uses.
- **BC4:** the eight-value palette with endpoints inset from the block range, and the
  six-value palette (exact 0 and 255), keeping the lower error. **BC5** is two BC4 blocks.

Candidates evaluated (crates.io, 2026-10-28):

| Candidate | Licence | Why not (now) |
|---|---|---|
| `intel_tex_2`, `ctt-*` (`bc7e`, Compressonator, `bc7f`, etcpak bindings), `basis-universal` | MIT/Apache, Zlib | C/C++/ISPC bindings: a native toolchain for every contributor and CI runner |
| `rusty_dds` 0.9 | MIT | Pure Rust and multi-mode BC7 with measured quality at or above DirectXTex, but ~30 kLOC, about ten transitive dependencies, heavy `unsafe` SIMD, a DDS-centred API, a single maintainer and a few months old. Worth re-evaluating when art arrives (below). |
| `texpresso` | MIT | BC1–BC3 only |

The in-house encoder is adequate for engine development and the self-generated sample set.
It is **not** adequate for production art on high-contrast or multi-colour blocks, where BC7
partition modes (1, 3, 7) and separate alpha (modes 4, 5) matter. That is a documented
follow-up (below); the KTX2 container and the runtime do not depend on which encoder wrote
the file.

### Devices without `textureCompressionBC`: CPU decode at load
When the device lacks `textureCompressionBC`, cannot sample a BC format with linear
filtering, or `RenderConfig::compressed_textures` is `false` (client: `MOBA_COMPRESSED_TEXTURES=0`),
`Renderer::register_texture` decodes the texture on the CPU to its 8-bit equivalent (every
level) and uploads that. The decoder is `bcdec_rs` 0.2.1 (MIT, safe, `no_std`, no
dependencies; a port of `bcdec`, which decodes every BC7 mode), behind the `moba-assets`
`bc-decode` feature.

- **Why not bake RGBA8 alongside?** It doubles disk and download size for every user, to help
  a rare device class (every desktop GPU, lavapipe and Apple Silicon support BC). Two files
  per texture can also drift apart.
- **Cost:** load-time only, and only on those devices. On the sample set the fallback's
  output differs from the GPU's own BC decode by at most one 8-bit step in the final frame
  (lavapipe, `MOBA_COMPRESSED_TEXTURES=0`; [perf/0013](../perf/0013-materials.md)).
- VRAM grows 4× for colour (BC7 → RGBA8) and 2× for BC5/BC4 on those devices.

### Materials: a client-local descriptor
A material is a **shader permutation + parameter block + texture set**. A model named
`sentinel` uses `<asset root>/materials/sentinel.toml` if it exists (`moba_assets::MaterialDesc`):
`shader = "textured" | "vertex-color"`, up to four texture names resolved to
`<asset root>/textures/<name>.ktx2` (base colour, normal, metallic-roughness, mask), and
`base_color_factor`, `metallic`, `roughness`, `normal_scale`, `team_tint`, `emissive`. The
file is at most 4 KiB, unknown keys are rejected, every number is range-checked, and names
use the mesh-name rules, so a path can never leave the asset root.

- **Why not glTF materials?** A glTF material points at images that are usually embedded
  PNG/JPEG. Using them would put image decoders on the client's untrusted-input surface, or
  need `KHR_texture_basisu`. A descriptor next to the model also matches `MapVisuals` and the
  R4a animation sets, and keeps materials out of the content hash and the wire. Generating the
  descriptor from a glTF material is a tooling follow-up, not a runtime feature.
- **One material per model.** It applies to the model's static and skinned meshes. Per-
  primitive materials need the loader to keep primitive ranges; that is a follow-up.
- **Missing or invalid → vertex colour, logged once.** A bad descriptor, a missing or
  malformed texture, a texture in the wrong slot, a model without UVs or a renderer limit each
  log one `[assets]` line and leave that model on the vertex-colour look. A match never fails
  because of a material. `content-check` reports the same problems as errors, plus counts.

### Shading, descriptors and pipelines
- **Permutations.** `unit.vert`, `skinned.vert` and `unit.frag` also build with
  `MATERIAL_TEXTURED` (`crates/shaders/build.rs` `PERMUTATIONS`). The textured fragment shader
  samples base colour × factor, tints it by the unit colour where the mask is set (team
  readability), and applies the normal map with a tangent frame from screen-space derivatives
  (no tangent attribute until R2). Metallic/roughness are bound and wait for R2's PBR. The
  vertex-colour permutation is unchanged, and both golden captures are byte-identical to
  `main` on lavapipe.
- **Descriptor model (ADR-0009).** Set 2 of the scene pipeline layout is the material set: a
  64-byte std140 parameter block and four combined image samplers. Each
  `Renderer::register_materials` call creates **one descriptor pool sized for exactly the
  textured materials of that call** and one parameter buffer; the client registers every
  material of a content load in one call. Unbound slots use 1×1 defaults (white, flat normal,
  metallic 0 / roughness 1, full mask). Nothing is allocated or written per frame. Limits:
  256 textures, 64 materials.
- **Sampler:** trilinear, repeat; anisotropy by tier (Low off, Medium 4×, High 8×, Ultra 16×,
  clamped to the device and only when `samplerAnisotropy` is supported).
- **UVs.** `MeshData::with_uvs` and the `.glb` loader's `TEXCOORD_0` (`f32` or normalised
  `u8`/`u16`). The static vertex is 32 bytes and the skinned vertex 40 bytes; the
  vertex-colour pipelines do not declare the UV attribute. A textured material on a mesh
  without UVs is rejected.
- **Pipeline cache and precompilation.** Every pipeline, each permutation included, is built
  when the renderer is created, through a persistent `VkPipelineCache` (wrapped file with a
  checksum; vendor, device and `pipelineCacheUUID` checked before the driver sees it; a
  corrupt or foreign file is a cold start; a corrupt one is deleted, a foreign one kept). On the RTX 5070 Ti, 7 pipelines take 90–98 ms cold
  and 0.5 ms warm with the driver's own disk cache off
  ([perf/0013](../perf/0013-materials.md)).

## Alternatives considered
- **Basis Universal / UASTC with transcoding.** Smaller downloads and one file for desktop and
  mobile, but a C++ transcoder (or a young pure-Rust one) and transcoding work at load. Revisit
  when download size or mobile matters (roadmap R8).
- **Zstd supercompression in KTX2.** Smaller files, but a decompressor dependency on the
  untrusted-input path. The engine ships a few hundred KiB of textures today.
- **ASTC for Apple.** Apple Silicon samples BC, so BC covers every target we test. ASTC is an
  R8 option for VRAM on Apple GPUs.
- **RGBA8 only (no compression).** Simplest, but four times the VRAM and bandwidth for colour,
  against the min-spec budgets of roadmap §7.

## Consequences
- New public API (additive, ADR-0008 style): in `moba-assets` `TextureData`, `TextureFormat`,
  `TextureError`, `load_ktx2`, `load_ktx2_file`, `write_ktx2`, `MaterialDesc`, `MaterialError`,
  `MaterialShader`, `TextureSlot`, `MeshData::with_uvs`/`uvs` and `MaterialReport`; in
  `moba-render` `Renderer::register_texture`, `register_materials`, `set_mesh_material`,
  `material_stats`, `pipeline_stats`, `reload_shaders` and `compressed_textures_supported`,
  `MaterialData` (texture slots set with `with_texture`, read with `texture`), `MaterialId`,
  `TextureId`, `MaterialStats`, `PipelineStats`,
  `PipelineCacheState`, `RenderConfig::with_pipeline_cache`, `with_shader_dir` and
  `with_compressed_textures`, and three `RenderError` variants. New types are
  `#[non_exhaustive]`.
- No wire, replay, simulation-logic or content-hash change.
- **Follow-ups:**
  1. A production BC7 encoder (partition and alpha modes), either `rusty_dds` once it has
     matured or a partition search added to `bc.rs`, before art production starts (roadmap §8).
  2. glTF material import into the descriptor (a `texture-bake`/`sample-mesh` tooling step).
  3. Per-primitive materials, tangents (`TANGENT` or MikkTSpace) and PBR (R2).
  4. Texture streaming and VRAM budgets per tier (R8).
