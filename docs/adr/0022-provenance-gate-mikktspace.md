# ADR-0022: Asset licence and provenance gate, and MikkTSpace tangents (A0)

**Status:** accepted 2026-10-31, agent-drafted for maintainer review (next-gen roadmap
[§9.4](../plans/next-gen-graphics-roadmap.md#94-the-automated-pipeline) and §12 step 4,
"A0"). Written before the code. Amends [ADR-0011](0011-art-style-lighting.md) §8 (the tangent
generator). ADR-0020 and ADR-0021 are reserved for N0 and N1.

## Context
The next-gen roadmap brings in third-party CC0 and CC-BY art (§9.2) and AI-generated assets
(§9.3), and makes three promises about them (§9.4, §9.5): every shipped asset has recorded
provenance, a licence gate rejects anything we may not ship, and `THIRD_PARTY.md`, the game's
credits and Steam's AI disclosure are generated from that record rather than kept by hand.
The roadmap also lists an early correctness fix: tangents must be MikkTSpace.

**Today's content.** `content/` holds 76 asset files in nine directories (models and LODs,
textures, a material, environments, grading LUTs, an animation set, a VAT, baked AO, VFX
descriptors and their atlas) next to eleven pack files. Git history, the bake scripts and the
generators show every one is the project's own: produced from code by `sample-mesh`,
`texture-bake`, `env-bake`, `grade-bake`, `ao-bake`, `vat-bake` and `mesh-bake`, or written by
hand as TOML. None is third-party or AI-generated. `THIRD_PARTY.md` says so in prose, and
nothing checks that the prose stays true.

**Today's tangents.** `MeshData::generate_tangents` (R2a, ADR-0011 §8) is a hand-written
"MikkTSpace-style" generator: Lengyel's per-triangle `sdir`, accumulated **un-normalised and
area-weighted**, orthogonalised once per vertex, with no orientation groups. MikkTSpace
normalises each triangle's tangent, projects it into each vertex's tangent plane, weights it by
the **corner angle**, and groups corners by handedness and edge connectivity. On flat-shaded
faces the two agree to rounding, which is all the engine ships today. On a smooth mesh they
disagree (by 0.06 in a tangent component on the test fan below), and a normal map baked
against MikkTSpace, which every baker and glTF assume, then shades with visible seams and
skew. Nothing in the repository calls meshoptimizer's tangent generator: the `meshopt` crate is
linked only by `mesh-bake` for LODs, which are index-only and never touch tangents.

## Decision

### 1. One `provenance.toml` per asset directory
Every file below the content root (not the pack files at the root itself, which are gameplay
data, and not dot-files) must be covered by exactly one `[[asset]]` entry of the
`provenance.toml` in **its own directory**:

```toml
[[asset]]
files = ["terrain-*.ktx2"]          # names or `*` patterns in this directory
title = "Terrain layer textures"
source = "original"                 # or the http(s) page it came from
author = "MOBA Engine contributors"
licence = "MIT OR Apache-2.0"       # SPDX-like id
made_with = "texture-bake generate terrain (scripts/bake-sample-textures.sh)"   # optional
attribution = "…"                   # required for CC-BY-*
notes = "…"                         # optional

[asset.ai]                          # only for AI-generated assets
model = "TRELLIS.2-4B"
model_version = "2025-12"
weights_licence = "MIT"
seed = 1234
prompt = "…"                        # may be empty when inputs drive it
tools = ["ComfyUI 0.3.x", "mesh-bake"]
[[asset.ai.inputs]]
name = "concept.png"
source = "original"
licence = "CC0-1.0"
```

Per-directory manifests keep related files (a model and its LOD, a texture set) in one entry,
need ten files for today's content instead of 76 sidecars, and sit where an agent downloading
or generating an asset will write it. Unknown keys are errors (`deny_unknown_fields`), so a
typo such as `license` fails. A file covered twice, an uncovered file, a directory without a
manifest and a pattern that matches nothing (a stale entry) are all errors. `provenance` is
therefore a reserved asset name; `check_vfx_dir` skips `vfx/provenance.toml`.

### 2. The licence policy
- **Allowed:** `CC0-1.0`, `CC-BY-3.0`, `CC-BY-4.0`, `MIT`, `Apache-2.0`, `MIT OR Apache-2.0`
  (the repository's own licence, which every current asset carries) and `OFL-1.1` (fonts).
- **Exceptions** (`moba_assets::provenance::LICENCE_EXCEPTIONS`), each with its approval:
  `LicenseRef-CMU-Mocap` (the CMU motion-capture database) and
  `LicenseRef-NVIDIA-Open-Model-License` (Kimodo), both from §9.2. Approving an exception is a
  human step (§9.4), so the list is code: adding one is a reviewed PR, not a data edit an
  agent can make alongside the asset that needs it.
- **Rejected:** non-commercial and no-derivatives (`NC`, `ND`), share-alike, unknown or
  `NOASSERTION`, and all-rights-reserved or proprietary, each with its own message; anything
  else not listed is "not on the allow-list". Brand marks (§9.6) will need an exception when
  they arrive.
- **CC-BY needs attribution text**, which goes verbatim into the notices.
- **AI-generated assets** (an `[asset.ai]` table) follow §9.5: the asset itself is `CC0-1.0`
  (or `CC-BY-*` once a person modified it); model, version, weights licence (allowed or an
  exception), seed and at least one tool are required, and either a prompt or an input; a
  third-party input must be `CC0-1.0`, and our own inputs any allowed licence.

### 3. Generated notices, checked in CI
`content-check --provenance [--content DIR] [--third-party FILE]` runs the gate and writes:

- the **content section of `THIRD_PARTY.md`**, between `<!-- BEGIN content provenance … -->`
  and `<!-- END content provenance -->` markers. The file already covers code dependencies and
  embedded fonts by hand, so the generated table is appended to it rather than replacing it,
  and the release archives that already ship `THIRD_PARTY.md` carry it;
- **`content/CREDITS.txt`**, plain text the game can show: original, third-party and
  AI-generated sections, one line per entry and its attribution;
- **`content/AI-DISCLOSURE.md`**, the Steam AI-disclosure list: every AI entry's files, model,
  version, weights licence, asset licence and tools ("no shipped asset was generated with AI"
  today).

`--check` writes nothing and fails when the gate fails or any of the three is stale.
`scripts/agent-verify.sh` and `just ci-tools` run it, and a `moba-assets` unit test repeats
it on all three CI platforms. Output is deterministic (sorted directories, manifest order,
LF line endings). The library is `moba_assets::provenance`, so a future downloader or AI
pipeline writes the same types it is checked against.

Provenance is metadata only: never hashed, signed or sent on the wire, never read by the
client, and pack hashes are unchanged.

### 4. MikkTSpace tangents through `bevy_mikktspace`
`MeshData::generate_tangents` runs the reference algorithm through
[`bevy_mikktspace`](https://crates.io/crates/bevy_mikktspace) 1.0.0, pinned exactly: safe
Rust (`forbid(unsafe_code)`), no dependencies, MSRV 1.85, Zlib AND (MIT OR Apache-2.0), and
fuzz-tested by its authors to be byte-identical to Mikkelsen's C. Its legacy behaviour is kept
(the `corrected-*` features stay off) because matching the bakers matters more than fixing
the reference's edge-sort and welding quirks. Our adapter:

- passes **`(u, -v)`**: glTF `v` runs down the image, so negating it gives the space a DCC tool
  computes with `v` up before its exporter flips `v`, the convention ADR-0011 §8 already
  documents (bitangent up the image, `w = -1` on mirrored islands). Negation keeps every UV
  difference and the welding's exact equality;
- passes **unit normals** (MikkTSpace assumes them);
- uses `default-features = false` and supplies `Ops`: `sqrt` is IEEE-exact, and `acos` is a
  30-term `asin` series in `f64` using only `+ - * /` and `sqrt`. The result is bit-identical
  on every platform, which the committed sample meshes need (their bytes are tested on Linux,
  Windows and macOS). It agrees with the platform `f64::acos` to a few `f64` ulps and within
  one `f32` ulp after rounding;
- **collapses corners to vertices**: MikkTSpace answers per triangle corner, and `MeshData`
  holds one tangent per vertex. A vertex whose corners disagree (shared across a mirrored seam,
  or two fans meeting at one index) takes its first corner's answer in index order. Exporters
  split such vertices, so a mesh with split seams gets exact MikkTSpace everywhere. A vertex
  with no tangent space (only degenerate triangles) keeps the old fallback, a fixed unit vector
  perpendicular to its normal with `w = +1`.

Tests pin the result against an independent `f64` implementation of Mikkelsen's definition
on a smooth fan, where the old area weighting is 0.06 away, and against `bevy_mikktspace`
with the platform `acos` (the reference build) on a fan, a mirrored island, a UV-degenerate
triangle and a position-degenerate one.

## Alternatives considered
- **meshoptimizer's `meshopt_generateTangents` with `meshopt_TangentCompatible`.** The roadmap
  named it. In that mode it groups and weights corners as MikkTSpace does, with its own
  approximate `acos`, so it is close to the reference rather than identical; and it would put
  C++ into `moba-assets` and so into the client, which today never links meshoptimizer
  (ADR-0018 keeps it behind `mesh-bake`).
- **gltf-rs `mikktspace` 0.3.** A c2rust translation with `unsafe`, nalgebra 0.26 by default,
  and NaN divergences from the C that `bevy_mikktspace`'s fuzzing found.
- **Porting MikkTSpace by hand.** The reference is about 1,900 lines of C to re-verify; the dependency already
  is that port, tested against the original.
- **Keeping the old generator and documenting the gap** (ADR-0011's choice). It was fine while
  every mesh was flat-faceted and generated here; the roadmap's baked normal maps are what
  make baker parity necessary.
- **REUSE-style `.license` sidecars or a `dep5` file.** A standard, but one sidecar per file
  (76 today, thousands later) and no place for the AI fields. Per-directory TOML matches the
  rest of `content/`.
- **One manifest at the content root.** `content/*.toml` are packs, which the gates and docs
  glob; a root manifest would need special-casing everywhere and would conflict on every
  asset PR.
- **A separate `THIRD_PARTY_ASSETS.md`.** One notices file is what the release archives
  already ship; a generated section inside it keeps that.

## Consequences
- New public API (additive): the `moba_assets::provenance` module (`check_provenance`,
  `ProvenanceReport`, `ProvenanceEntry`, `AssetProvenance`, `AiProvenance`, `AiInput`,
  `LicenceException`, `ALLOWED_LICENCES`, `LICENCE_EXCEPTIONS`, `licence_status`,
  `needs_attribution`, `third_party_section`, `splice_third_party`, `credits_text`,
  `ai_disclosure`, `PROVENANCE_FILE`, `MAX_PROVENANCE_BYTES`, `THIRD_PARTY_BEGIN`,
  `THIRD_PARTY_END`), and `content-check --provenance [--check]`.
- New dependency `bevy_mikktspace =1.0.0` in `moba-assets` (so the client, the tools and the
  fuzz crate); `cargo-deny`'s allow-list already has Zlib, MIT and Apache-2.0.
- The committed sample meshes `content/models/{sentinel,crystal,rampart}.glb` (and
  `fuzz/corpus/glb/sentinel.glb`) are regenerated: they are flat-faceted, so their tangents
  move by at most 1.8e-7 per component (rounding), with no handedness change. The skinned
  sample has no UVs and is unchanged, and so are every `.lod` (re-baked and diffed), the VAT
  and the baked AO, which do not read tangents.
- MikkTSpace takes a triangle's handedness from its UV winding and assumes front faces are
  counter-clockwise about the normal (glTF's convention, which the sample meshes follow). The
  renderer's built-in cube and ground plane were wound clockwise, harmless while nothing culls
  back faces but enough to give the plane `w = -1`; their index order is reversed, and a test
  now checks every built-in triangle's winding and bitangent direction.
- Local lavapipe captures of the default scene, the textured `sentinel-pack` scene and the
  ember golden scene are byte-identical before and after (`main` @ 0745fc3).
- No wire, replay, simulation-logic, golden-state-hash or content-hash change; tangents are
  render-only.
- Every new asset needs a provenance entry before `scripts/agent-verify.sh` passes, which is
  the point; the format is in [content-authoring](../content-authoring.md#provenance).
