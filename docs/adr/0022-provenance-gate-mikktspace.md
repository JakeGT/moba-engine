# ADR-0022: Asset licence and provenance gate, and MikkTSpace tangents (A0)

**Status:** accepted 2026-10-31, agent-drafted for maintainer review (next-gen roadmap
[§9.4](../plans/next-gen-graphics-roadmap.md#94-the-automated-pipeline) and §12 step 4,
"A0"). Written before the code. Amends [ADR-0011](0011-art-style-lighting.md) §8 (the tangent
generator). ADR-0020 and ADR-0021 are reserved for N0 and N1. Amended 2026-11-14 after review
([Amendment 1](#amendment-1-2026-11-14-a0-review-bounded-tangents-and-a-harder-gate)): runtime tangents are hard-bounded, shipped meshes carry baked
tangents, and the gate hashes third-party files and refuses special files.

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
Every file below the content root must be covered by exactly one `[[asset]]` entry of the
`provenance.toml` in **its own directory**. The root itself may hold only pack files
(`*.toml`, gameplay data) and the generated `CREDITS.txt` and `AI-DISCLOSURE.md`; anything
else there is an error. Dot-files, dot-directories and symlinks are errors at every level
and symlinks are never followed, so no file in a content root escapes the gate by being
hidden, by sitting at the root, or by living behind a link:

```toml
[[asset]]
files = ["terrain-rock.ktx2"]       # names or `*` patterns in this directory
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
human_modified = false              # true (with notes) permits CC-BY
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
manifest and a pattern that matches nothing (a stale entry) are all errors. Patterns are plain
ASCII names (letters, digits, `.`, `_`, `-`, `*`). The shipped manifests list binary assets by
name and keep `*` only for families of text descriptors, so each new binary is a reviewed
manifest change. `provenance.toml` is never an asset (`check_vfx_dir` skips
`vfx/provenance.toml`); nothing rejects `provenance` as an asset name, so the docs say not
to use it.

### 2. The licence policy
- **Allowed:** `CC0-1.0`, `CC-BY-3.0`, `CC-BY-4.0`, `MIT`, `Apache-2.0`, `MIT OR Apache-2.0`
  (the repository's own licence, which every current asset carries) and `OFL-1.1` (fonts).
- **Exceptions** (`moba_assets::provenance::LICENCE_EXCEPTIONS`), each with its approval and
  scope, both from §9.2: `LicenseRef-CMU-Mocap` (the CMU motion-capture database) only as an
  asset `licence` under `animations/`, and `LicenseRef-NVIDIA-Open-Model-License` (Kimodo)
  only as `ai.weights_licence`. Approving an exception is a human step (§9.4), so the list,
  its `uses` and its `dirs` are code: adding or widening one is a reviewed PR, not a data
  edit an agent can make alongside the asset that needs it.
- **Rejected:** non-commercial and no-derivatives (`NC`, `ND`), share-alike, unknown or
  `NOASSERTION`, and all-rights-reserved or proprietary, each with its own message; anything
  else not listed is "not on the allow-list". Brand marks (§9.6) will need an exception when
  they arrive.
- **Concept-only services** (`moba_assets::provenance::CONCEPT_ONLY_SERVICES`): the
  maintainer approved the Gemini API ("Nano Banana") for concept art and model sheets only
  (2026-10-31). Its output is a reference people model from by hand, so its licence id
  `LicenseRef-Gemini-API-Output` is rejected in every field, and so is an `ai.model` naming
  it. Using a concept as a reference is recorded in the asset's `notes`; the asset itself is
  original work. Allowing a generator to consume such output would be a new decision.
- **CC-BY needs attribution text**, which goes verbatim into the notices.
- **AI-generated assets** (an `[asset.ai]` table) follow §9.5: the asset itself is `CC0-1.0`,
  or `CC-BY-*` only with `ai.human_modified = true` and non-empty `notes` saying what a person
  changed (raw model output carries no copyright to license); model, version, weights licence
  (allowed or an exception), seed and at least one tool are required, and either a prompt or
  an input; a third-party input must be `CC0-1.0`, and our own inputs any allowed licence.
- **Manifest text is inert.** Control characters (line breaks included) and `<!--`/`-->` are
  errors in every string, so no entry can end the generated section or forge a marker, and
  table cells escape `|`, backticks, brackets and `<`.

### 3. Generated notices, checked in CI
`content-check --provenance [--content DIR] [--third-party FILE]` runs the gate and writes:

- the **content section of `THIRD_PARTY.md`**, between `<!-- BEGIN content provenance … -->`
  and `<!-- END content provenance -->` markers. The file already covers code dependencies and
  embedded fonts by hand, so the generated table is appended to it rather than replacing it,
  and the release archives that already ship `THIRD_PARTY.md` carry it. The file must hold
  exactly one `BEGIN` then one `END` marker, each starting a line (or neither, and the
  section is appended); anything else is a hand edit the tool will not guess at, so it exits
  1 without writing;
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
the reference's edge-sort and welding quirks. *(Superseded by [amendment 1](#amendment-1-2026-11-14-a0-review-bounded-tangents-and-a-harder-gate): both
`corrected-*` features are on, because the legacy weld and sort are quadratic on crafted
input.)* Our adapter:

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

The `.glb` loader generates tangents **once, at load**, for a mesh without `TANGENT` and with
UVs, so every consumer (the renderer, bone-count skin LODs, tools) sees the same tangents
and none regenerates them. A node whose transform mirrors (negative determinant) reverses
the triangle winding (glTF 2.0 §3.7.4), so the loader writes its corners as `(i0, i2, i1)`
and takes a missing flat normal from the reordered corners; front faces then stay
counter-clockwise and MikkTSpace gives the mirrored handedness, as file tangents do.
Because that puts MikkTSpace on the untrusted-file path (ADR-0007: `content-check`, the
`glb` fuzz target), generation is bounded. MikkTSpace's time is quadratic in the triangle
corners sharing one position: measured in release, a fan whose 8 Ki triangles share a centre
vertex takes 0.42 s, and 16 Ki overflows its recursion's stack (an abort, not an error); a
centre split per triangle (same position, distinct UVs, so not welded) or triangles collapsed
onto one point are quadratic too (1.1 s and 6.4 s at 32 Ki). So corners are counted per
position bit pattern (with `-0.0` as `0.0`), which over-approximates MikkTSpace's welded
vertices, and the mesh skips MikkTSpace and takes the fallback tangent everywhere when one
position has more than **1024** corners (`MeshData::TANGENT_MAX_SHARED_CORNERS`, about 7 ms
of quadratic work, 16× below the stack overflow and far above the ~6 corners per position of
a real mesh or the few hundred at a UV-sphere pole) or the sum of corners² over positions
exceeds **2^24** (`TANGENT_MAX_SHARING_COST`; the worst case measured under it is about 1 s, from 16
twisted-UV fans of 1020 corners; a dense mesh at the vertex cap is
about 2.4 M), so many positions just under the cap cannot add up. *(Superseded by [amendment 1](#amendment-1-2026-11-14-a0-review-bounded-tangents-and-a-harder-gate):
the caps are now 512 corners, Σk² ≤ 2^22 and Σk³ ≤ 2^28. Under the old caps, 16 random-UV
fans of 1020 corners took 0.9–1.2 s, and a cluster of adjacent floats took 59 s in the legacy
weld, which no cap tracked.)* Degenerate triangles (two
corners at one position, MikkTSpace's own test) escape both, because each of their corners
scans every good corner for a tangent to copy (upstream: "O(N^2)"): a 181 × 181 grid plus
16 Ki degenerate triangles on their own vertices takes 0.88 s, about 0.1 ns per (degenerate
corner, good corner) pair. So the mesh also falls back when that product exceeds **2^28**
(`TANGENT_MAX_DEGENERATE_COST`, about 30 ms), which still allows about 220 degenerate
triangles at the vertex cap and about 1500 on a 10 k-vertex hero, enough for decimated or
exported slivers; a tighter 2^26 would drop real meshes with a few dozen to the fallback.
The checks are one pass over the triangles and a sort of the positions, within the bench's
noise.
`mesh/generate_tangents/{10k,65k}` (`crates/assets/benches/mesh_tangents.rs`) measures the
cost: about 13 ms for a 10 k-vertex grid and 92 ms at the 65 536-vertex cap on the
reference machine, load-time only. *(Amendment 1 adds the cases at the caps and the comparison
with the old generator, about 100× slower.)*

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
  `LicenceException`, `LicenceUse`, `ALLOWED_LICENCES`, `LICENCE_EXCEPTIONS`,
  `licence_status`, `needs_attribution`, `third_party_section`, `splice_third_party`,
  `credits_text`, `ai_disclosure`, `PROVENANCE_FILE`, `CREDITS_FILE`, `AI_DISCLOSURE_FILE`,
  `MAX_PROVENANCE_BYTES`, `THIRD_PARTY_BEGIN`, `THIRD_PARTY_END`; amendment 1 adds
  `ConceptOnlyService`, `CONCEPT_ONLY_SERVICES`, `read_plain_file` and
  `AssetProvenance::sha256`), and
  `content-check --provenance [--check]` (exit 0 pass, 1 gate failure or stale notice, 2
  usage).
- New `MeshData::TANGENT_MAX_SHARED_CORNERS`, `MeshData::TANGENT_MAX_SHARING_COST` and
  `MeshData::TANGENT_MAX_DEGENERATE_COST` (amendment 1 adds `TANGENT_MAX_SUBGROUP_COST` and
  lowers the first two); a mesh over any gets fallback tangents (`w = +1`, perpendicular to the normal) everywhere.
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
  ember golden scene are byte-identical before and after (`main` @ 0745fc3). That shows no
  regression, not that MikkTSpace works: every shipped mesh is flat-faceted, where the old
  and new generators agree to rounding. The unit tests above are the evidence for smooth
  meshes.
- No wire, replay, simulation-logic, golden-state-hash or content-hash change; tangents are
  render-only.
- Every new asset needs a provenance entry before `scripts/agent-verify.sh` passes, which is
  the point; the format is in [content-authoring](../content-authoring.md#provenance).

## Amendments

### Amendment 1 (2026-11-14, A0 review): bounded tangents and a harder gate

Recorded in the A0 PR (`feat/a0-licence-gate`, #349) after its three reviews. §4's bound did
not hold: the caps counted corners, but MikkTSpace's cost also depends on welding, UVs and
the shape of each fan. The gate also read files it should refuse, and it trusted directory
names and unhashed bytes.

- **Both `corrected-*` features are on** (`corrected-vertex-welding`,
  `corrected-edge-sorting`). The legacy weld (`merge_verts_fast`) falls back to an O(k²) scan
  when the widest axis spans adjacent floats: a 255 × 255 grid on adjacent `f32`s near 10^6
  took 59 s. The legacy edge quicksort uses a fixed pivot. The corrected versions are a
  `BTreeMap` weld keyed by `total_cmp` and `sort_unstable`, both O(n log n); the same cluster
  now takes 0.1–0.2 s. The output is therefore no longer byte-identical to the C in general.
  For the shipped samples it is: `sample-mesh` regenerates `content/models/{sentinel,
  skinned,crystal,rampart}.glb` and `fuzz/corpus/glb/sentinel.glb` byte for byte. The
  reference test against the platform `acos` builds with the same features, so it now checks
  only our `acos`, not the welding.
- **The caps follow the cost.** Within one group of g faces sharing a position, MikkTSpace
  filters faces in O(g²), runs one `acos` per face per new subgroup, and compares subgroups in
  O(g³) at most. The degenerate pass is O(degenerate × good corners). So the guard now checks,
  over corners k per position (counted by bit pattern, `-0.0` as `0.0`, as before):
  - k ≤ **512** (`TANGENT_MAX_SHARED_CORNERS`, was 1024; a 256-segment UV-sphere pole has
    256);
  - Σk² ≤ **2^22** (`TANGENT_MAX_SHARING_COST`, was 2^24; about 56–78 ns per pair in the
    worst case, so about 0.3 s);
  - Σk³ ≤ **2^28** (new `TANGENT_MAX_SUBGROUP_COST`; the subgroup compares, at most k³ per
    position, about 0.1 s at the cap; a dense mesh at the vertex cap is about 14 M, a
    256-segment pole about 17 M);
  - degenerate × good corners ≤ 2^28 (`TANGENT_MAX_DEGENERATE_COST`, unchanged).

  The pair term dominates, so the ceiling is about 0.3 s. A frontier sweep (release, best of 3) of sliver fans
  (0.01 rad wide, random UVs, so every pair of faces costs an `acos`) at the Σk² cap found the
  worst single-cap case at 964 fans of 64 triangles, 0.23 s:

  | Triangles per fan | Fans | Time | ns per pair |
  |---|---|---|---|
  | 8 | 6553 | 37 ms | 60 |
  | 16 | 3640 | 87 ms | 75 |
  | 32 | 1927 | 130 ms | 58 |
  | 64 | 964 | 234 ms | 56 |
  | 128 | 127 | 113 ms | — |
  | 256 | 15 | 52 ms | — |
  | 510 | 2 | 41 ms | 78 |

  A fan of t triangles has t corners at its centre. A 255-side grid with 230 degenerate triangles, the most under the
  degenerate cap, takes 0.12–0.23 s. One mesh can also sit just under two caps at once:
  940 sliver fans of 64 plus 495 degenerate triangles (1485 × 180 480 corners ≤ 2^28) takes
  about **0.25 s with two caps combined**, the slowest accepted mesh known (from the
  reviewers' re-check). One consequence: a double-sided mesh at the 65 536-vertex
  cap (12 corners per position, Σk² about 4.7 M) now takes the fallback at runtime. That is
  acceptable, because shipped meshes no longer depend on runtime generation (next item).
- **Runtime generation is about 100× the old generator.** The bench
  (`crates/assets/benches/mesh_tangents.rs`) measures 13 ms for the 10 k-vertex grid and
  88 ms at the 65 536-vertex cap, against 0.15 ms and 0.8 ms for the pre-A0 area-weighted
  generator. It gains `sliver-fans-at-cap` (0.23 s), `big-fans-at-cap` (two fans of 510,
  26 ms), `degenerates-at-cap` (0.12 s) and `two-caps-combined` (0.25 s, the worst case). §4's 92 ms and "about 1 s" are replaced by these
  numbers.
- **Shipped meshes carry baked tangents.** `content-check --provenance` loads every `.glb`
  under the content root without generating tangents and fails one that has `TEXCOORD_0` but
  lacks `TANGENT` on any primitive (`sample-mesh` already writes them). The rule is library
  code (`check_baked_tangents`, `check_baked_tangents_file`,
  `provenance::baked_tangent_errors`), unit-tested on every CI platform on a fixture and on
  the shipped content, not only through the binary. Runtime MikkTSpace
  is now a bounded fallback for mods and development files, not the shipping path.
- **Loading without tangents.** New `GlbTangents { Generate, FileOnly }`, `load_glb_with` and
  `load_glb_file_with`; `load_glb` and `load_glb_file` are unchanged (`Generate`). `FileOnly`
  keeps a file's own `TANGENT` and generates none. `mesh-bake`, `vat-bake`, `ao-bake`,
  `content-check`'s footprint and the pack validation in `moba_assets` use it, because none
  of them reads tangents. VFX `unit_mesh` and the client keep `Generate`: the renderer asks for
  tangents, so skipping them at load would only move the cost.
- **Regression tests** (`moba_assets::mesh`), with a time bound of 1.5 s in release builds:
  964 sliver fans of 64 (under the caps; 965 are over), two fans of 510 with disc and sliver
  UVs (three are over), the adjacent-float cluster, the 255 grid with 230 degenerate
  triangles (232 are over), and the two-caps case (496 degenerates are over). The bound is
  about 6× the 0.25 s worst case on purpose: shared CI runners are several times slower and
  noisier, and the tests guard against the unbounded cases (seconds to minutes, or a stack
  overflow), not against tens of percent, which the bench tracks. The `glb` fuzz target now checks the tangents the loader returns
  (`mesh.tangents()`, present whenever UVs are) instead of generating its own.
- **Special files are never read.** The gate reads `provenance.toml`, `CREDITS.txt`,
  `AI-DISCLOSURE.md` and `THIRD_PARTY.md` only through the new `provenance::read_plain_file`:
  `symlink_metadata` must say a plain file; the open uses `O_NOFOLLOW | O_NONBLOCK` on unix,
  so a path swapped for a symlink or FIFO after that check is refused rather than followed or
  blocked on; the opened handle must be a regular file (`fstat`, every platform); and at most
  `limit + 1` bytes are read. The SHA-256 check opens files the same way. A symlink to `/dev/zero` or a FIFO is an error, not an
  out-of-memory or a hang. A refused manifest (for example a directory named
  `provenance.toml`) is reported and never read, and a notice path that is not a plain file
  fails the command before anything is written. Notices are capped at 4 MiB.
- **Directory names are plain.** Every directory under the content root must match
  `[A-Za-z0-9._-]` (the rule file patterns already had), so a name cannot forge a
  `THIRD_PARTY.md` marker or a Markdown line. Directory names also go through the Markdown
  escaping in the generated notices. The `--content` path is normalised (`./content/` is
  written as `content`).
- **SHA-256 pins (maintainer decision).** An entry may record
  `sha256 = { "rock-042-color.ktx2" = "…" }`, 64 lower-case hex digits per covered file, and
  the gate verifies each one. It is **required** for every file an entry covers when its
  `source` is not `"original"`, so a third-party file cannot change after its licence was
  reviewed; original work stays hash-optional. A key naming a file the entry does not cover
  is an error. The hash is of the committed bytes: `.gitattributes` normalises text to LF, so
  record it after `git add`, or mark a third-party text file `-text` to keep its upstream bytes.
- **Smaller gate fixes.** A `*` pattern may match only `*.toml` files (list binaries by name).
  The Gemini screen (`ConceptOnlyService`, `CONCEPT_ONLY_SERVICES`) now also checks every
  `ai.tools` entry and each AI input's name and source, not only `ai.model` and the input
  licence. Manifest text also rejects the Unicode line and paragraph separators (U+2028,
  U+2029) and the bidirectional controls U+202A–U+202E and U+2066–U+2069.
- **What the gate does not prove.** Provenance is self-declared: an entry that omits its
  `[asset.ai]` block, or claims `source = "original"`, is taken at its word, and nothing in
  the files can show otherwise. Review of the manifest change is the control. The gate covers
  only the content root it is given (`content/` in CI); code, shaders and any file outside
  that root are outside it.
- **Layering.** `bevy_mikktspace` reaches `moba-sim`, `moba-server` and `moba-net` through
  `moba-game` → `moba-assets`. That is not a layering violation: the rule (AGENTS.md, checked
  by `scripts/agent-verify.sh`) keeps `moba-render`, `moba-platform`, `ash`, `winit` and
  `gpu-allocator` out of those graphs, and this is a
  dependency-free `no_std` crate that never runs on a simulation path; tangents stay
  render-only.
- **Not done.** A const `acos` table in place of the series was considered and skipped: it
  would change tangent bits and so every committed `.glb`, for a load-time cost the baked
  tangents now avoid.
- **API additions** (all additive): `ConceptOnlyService`, `CONCEPT_ONLY_SERVICES`,
  `read_plain_file`, `baked_tangent_errors` and `AssetProvenance::sha256` in
  `moba_assets::provenance`; `GlbTangents`, `load_glb_with`, `load_glb_file_with`,
  `check_baked_tangents`, `check_baked_tangents_file`; `MeshData::TANGENT_MAX_SUBGROUP_COST`.
  `moba-assets` gains `libc` on unix (already in the graph) for the open flags.
  No wire, replay, simulation-logic, state-hash or content-hash change; the committed meshes
  are byte-identical, so captures are too (compared against `main`'s CI artefacts in the PR).
