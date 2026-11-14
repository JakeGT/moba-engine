# ADR-0026: Baked lighting: `light-bake`, a top-down terrain lightmap, an irradiance probe volume and reflection probes (N3)

**Status:** proposed 2026-11-14. This is phase N3 of the
[next-gen graphics roadmap](../plans/next-gen-graphics-roadmap.md) (§1, §3, §6.1, §6.4, §8,
§9.6, §10 and §12), written before the code. Docs only; nothing here is built yet. Every
choice the maintainer has to make is marked **Decision needed (maintainer)** in the text and
collected, with its recommendation, under
[Decisions needed](#decisions-needed-maintainer).
- **Builds on** (each accepted; this ADR uses them and does not re-decide them):
  - [ADR-0020](0020-next-gen-foundations.md) (N0, accepted 2026-11-01, Amendments 1–6): the
    `Tiered`/`Compat` paths and the Compat freeze (§2d), the bindless set and buffer device
    address (§3), Slang for new RT passes (§4), the tier budgets (§5b) and fetched, pinned
    vendor SDKs (§6b, `vendor-sdks.toml`).
  - [ADR-0021](0021-temporal-upscaling-overlay.md) (N1, accepted 2026-11-02): nothing in N3
    moves, so nothing new enters the upscalers' history or the overlay layer.
  - [ADR-0023](0023-frame-architecture-materials.md) (N2, accepted 2026-11-02): the thin
    G-buffer and its two spare fields kept for N3 (§1a: `gb1.a` bit 1, model IDs 8–15), the
    spare colour attachment (§1d), `deferred_light`'s ambient step "SH-L2 diffuse and the
    specular cube (until N3's probes)" (§2c step 4), the shared Slang modules that forward
    water and glass also call (§5b), and the material model (§6) whose base colour the bake
    reads.
  - If any of these is amended in its implementation, this ADR follows it.
- **Amends, for the Tiered path only, and only where a valid bake is loaded:**
  - [ADR-0011](0011-art-style-lighting.md) §3: the SH-L2 diffuse ambient and the single
    environment cube are replaced by the bake (§5);
  - [ADR-0016](0016-lights-outlines-ao.md) §6: the top-down baked AO map is no longer
    multiplied into the ambient diffuse, because the bake's sky visibility already contains it
    (§5c);
  - [ADR-0010](0010-texture-pipeline.md): the KTX2 loader accepts 3D images, still without
    arrays or supercompression (§4d);
  - [ADR-0023](0023-frame-architecture-materials.md) §1a and §2c step 4: `gb1.a` bit 1 becomes
    `LIGHTMAP` (§5b), and step 4 becomes the bake lookup.
- **Compat is unchanged by N3.** Every N3 PR leaves every Compat golden byte-identical. N3
  only *specifies and tests* the subset C1 will sample (§8); the C1 PR wires it in and
  rebaselines Compat deliberately, then Compat freezes (ADR-0020 §2d).
- **Not dependent on unaccepted ADRs.** ADR-0022 (A0 provenance gate, PR #349) is accepted but
  not merged; ADR-0024 (N7, PR #362) and ADR-0025 (N6, PR #361) are proposed. N3 names where it
  would meet them, but no N3 phase waits for them, and they are cited as file names, not links,
  until they are on `main`.
- **ADR number.** ADR-0022 is the A0 gate (#349), ADR-0024 is N7 (#362) and ADR-0025 is N6
  (#361). No other open PR adds an ADR file, so N3 is ADR-0026.

External claims (OIDN, Unity's probe-volume documentation, Poly Haven) were checked on
2026-11-14 against the sources in [Sources](#sources). Papers are cited as literature and were
not re-checked. Anything that could not be confirmed is marked **unverified**.

## Context

Roadmap §8 gives N3 six items: `light-bake` (a Vulkan RT path tracer), lightmaps and probes,
reflection probes, OIDN, `content-check` validation, and bakes for every map. It depends on N2
and "shares BVH with N4". Roadmap §6.1 calls it "the largest visual jump for Low and Medium",
and §1 fixes the premise: **lighting is fixed per map and baked**, with ray tracing adding only
what moves.

Two later phases need it:
- **A3, the vertical slice** (roadmap §8 A-track: "A3 needs N2 + N3 and art bible v0"): one
  Ember Crossing map section, the Kiln Warden and both factions' minions, shown at Medium and
  Overdrive. At Medium the §3 matrix's static GI *is* the bake, and the dynamic GI is
  "probe-lit dynamics" plus N5's SSGI.
- **C1, the Compat refresh** (roadmap §8): "sample N3's baked lightmaps, probes and reflection
  probes (the biggest win, and it needs no RT)".

What the code does today (`main` at `657a7ab`):

- **Ambient light is one sky for the whole map.** `crates/shaders/shaders/include/lighting.glsl`
  evaluates `ambient_irradiance(n)` from nine SH-L2 coefficients (`sh_irradiance`) times
  `ibl_params.x`, and `ambient_specular(r, roughness)` from one prefiltered cube. Both come from
  `moba-tools env-bake` (`crates/tools/src/bin/env-bake/`): an analytic zenith/horizon/ground
  gradient with a sun glow and **no sun disk**, written as a 64² `RGBA16F` cube with 7 mips and
  an SH-L2 sidecar (`moba_assets::EnvironmentDesc`, `crates/assets/src/environment.rs`). A unit
  in a wall's shadow gets the same sky as one in the open.
- **The only bake is top-down AO.** `moba-tools ao-bake` (`crates/tools/src/bin/ao-bake/`)
  rasterises walls and props into a height field and computes **height-field horizon AO**
  (16 directions, 12 m) for a ground-level receiver; it casts no rays. It writes a square
  `R8_UNORM` KTX2 plus a sidecar (`moba_assets::BakedAoDesc`, `crates/assets/src/baked_ao.rs`)
  with an input fingerprint (`ao_fingerprint`). The client recomputes the fingerprint and
  ignores a stale bake. **Structures are not baked: "they are destroyed in play."** The shader
  maps world `xz` straight to the texture (`lighting.glsl`, `baked_ao_map`, binding 13) and
  fades it out with height.
- **Ray tracing exists only as a smoke test.** `crates/render/src/smoke/rt.rs` builds one BLAS
  and one TLAS and traces `GL_EXT_ray_query` rays from `shaders/rt_smoke.comp`; CI runs it on
  lavapipe with validation (`.github/workflows/render-golden.yml`, "Tiered bindless and
  ray-query smokes"). There is no BLAS/TLAS manager (that is N4).
- **The static map is generated, not authored as one mesh.** `moba_assets::terrain` builds a
  heightfield over the board plus a margin of `0.6 × extent`, clamped to 96–240 m
  (`terrain_margin`), with cliffs or hills rising in the margin; the board's relief is at most
  0.5 m. Walls are boxes (or `wall_mesh` stretched into them), props are meshes from the map
  visuals (`content/maps/ember.toml`: five ember crystals), foliage is procedural.
- **No second UV set.** `moba_assets::MeshData` carries `TEXCOORD_0` only
  (`crates/assets/src/mesh.rs`). The KTX2 loader rejects 3D images, arrays and supercompression
  (`crates/assets/src/texture.rs`), and `TextureFormat` has no BC6H.
- **Map lighting is in relative units.** `[lighting]` holds `sun_color × sun_intensity`
  (Ember 1.4, Golden Crossing 1.5) and `ibl_intensity` (0.3–0.35, an art override that keeps a
  warm sky from greying blue units, ADR-0011 rule 1). The art bible's `lighting-mood.md` (PR
  #347) lists *physical* targets for `light-bake` (Golden Crossing: about 55 klx of sun, EV100
  13.5, "sky turbidity 4").
- **Presets.** `crates/render/src/preset.rs` marks five cells `Planned(N3)`: Static GI on Low
  ("baked (lightmaps and SH probes)"), Medium and High ("baked"), Dynamic GI on Low
  ("probe-lit dynamics"), and Reflections on Low ("baked probes").
- **Compat's scene layout is full.** ADR-0016 §6 pins the fragment stage at **16 of 16**
  samplers; the next set-3 sampler needs a binding retired first.

The art direction (bible v0 on PR #347, approved by the maintainer on 2026-11-14 with every
recommendation taken) sets what the bake must deliver for the slice:
- **Golden Crossing** is map 1's mood: sun at 28° from the west-south-west, warm, "GI bounce off
  sand" named as its photoreal payoff. Forge west, Void east. No weather or time-of-day variant
  for the slice.
- **SHD-1:** no fully unlit area in playable space; bounce from the sand and the VAL-3 floor
  (L ≥ 0.18) hold in every structure's shadow. **SHD-2:** a unit in a structure's shadow still
  passes CON-1. **SHD-3:** no long shadows straight up the main lane.
- **G3 / PBR-DELIGHT-1:** no baked lighting in albedo, because "the renderer bakes GI and AO
  itself".
- Reference HDRIs (`references.md` §1.1, `rogland_sunset` closest to Golden Crossing) are
  "references for the analytic sky and the future `light-bake`. They don't ship with the map
  unless the maintainer approves one."

Constraints:
- **Presentation only.** No change to `PROTOCOL_VERSION` (v42), replay `VERSION` (v5),
  `SIM_LOGIC_VERSION` (v9), content hashes or `tests/golden-state-hash.txt` (§10).
- **Readability is non-negotiable** (roadmap §4). Baked shadowing must not take the board below
  the bible's value floors, and a higher tier may never hide what Low shows (CON-5).
- **The R8 rules hold:** no per-frame allocation, `locked_uploads=0`, `locked_pipelines=0`. A
  bake is loaded with the map, like today's baked AO.
- **Compat is byte-identical** until C1.
- **Storage.** The maintainer decided on 2026-11-14: no Git LFS; each committed file ≤ 10 MB;
  ≤ 100 MB added to the repository for the slice in total. The roadmap's "about 100–500 MB per
  map" (§6.1) cannot be committed, so the bake's size is a design input here, not an
  afterthought (§4e).
- **Downloads** only from sources an ADR or the bible's `references.md` names, pinned by
  SHA-256.

## Decision

### 1. What is baked

**1a. Baked indirect, not baked direct.** The bake stores, for static receivers:
- **sky light** from the map's environment, with its visibility (what today's IBL ambient
  lacks, and what today's baked AO approximates);
- **every indirect bounce** of the sun and the sky off static geometry (up to 4 bounces, then
  Russian roulette);
- **light from static emissive surfaces** (the ember crystals, molten seams on static props).

It does **not** store the sun's direct light. The sun stays real time on every tier, as the §3
matrix has it: today's shadow map with PCSS (`Built`), RT sun shadows from N5. A baked sun
would disagree with the units' real-time shadows and with N5's, and would need a shadow mask to
let units cast into it. This is **Decision needed (maintainer) Q2**, recommended as stated.

**1b. What is in the baked scene.** Static map geometry only, built by the same code the client
uses to draw it:
- the terrain heightfield, board and margin (`moba_assets::terrain`);
- walls (`wall_mesh` or the box), at `wall_height`;
- props with their meshes (`[[props]]`);
- foliage as a thin, mostly transmissive layer (it darkens the ground slightly, and is never an
  occluder above its height of 0.2–0.45 m).

**Not in the scene:** units of any kind, VFX, water surfaces (the bed is terrain) and
**structures** (towers, inhibitors, the core). Structures are destroyed in play: baking them in
would leave their bounce and occlusion on the ground after they fall. They are lit at run time
as dynamic receivers through the probe volume (§3), cast their sun shadow through the real-time
shadow map, and their glow (a Forge tower's lens crown) is a clustered or LTC light (ADR-0023
§4), not baked emission. This matches `ao-bake`, which excludes them for the same reason. It
also means the bake never depends on match state, so it cannot leak information (§10). This is
**Decision needed (maintainer) Q3**: exclude structures (recommended); bake them in and accept
ghost lighting after they fall; or bake two variants and switch per structure (rejected:
doubles storage and adds a state-dependent lighting path).

**1c. Hit shading in the baker.** Each hit is shaded with the material's **mean base colour**
(from the KTX2 mip tail times the factor, or the factor alone), its metallic and its emissive
term, and for the terrain the splat weights at the hit times each layer's mean albedo and tint.
At 0.5 m lightmap texels and 2 m probes, texture detail in a bounce is invisible. Albedo is
taken *as authored*: bible rule G3 forbids light in albedo, and `content-check` already flags
it (PBR-DELIGHT-1).

**1d. The light sources are exactly the run time's.** The bake's sun is `[lighting]`'s
`sun_direction` and `sun_color × sun_intensity`; its sky is level 0 of the environment cube the
map names, times `ibl_intensity`. So, in the open, the baked sky light equals today's IBL
ambient (gate 1, "open sky"), and the bake and the real-time sun can never disagree about
where the light comes from.
- **Decision needed (maintainer) Q8: light units.** Keep these relative units and check the
  bible's physical targets (Golden Crossing's ~55 klx and EV100 13.5) as ratios (recommended),
  or move to physical units now. See [Decisions needed](#decisions-needed-maintainer).
- **Decision needed (maintainer) Q9: the slice's sky.** Because the bake reads the sky only
  through the environment, the choice of sky (mood A's analytic sky, an HDRI-derived cube from
  `rogland_sunset`, or ADR-0025's Hillaire sky if accepted) is made in `env-bake`, and N3 works
  with any of them. The recommendation is mood A's analytic sky; see the list at the end.

### 2. Representation: a top-down terrain lightmap and an irradiance probe volume

**Decision needed (maintainer) Q1: how static GI is stored.** The roadmap says "SH-L1/L2
lightmaps or a probe volume for static geometry" (§6.1). Three options:

| Option | What it is | For | Against |
|---|---|---|---|
| **(a) Hybrid (recommended)** | The terrain gets a **top-down lightmap** in world `xz`, like today's baked AO; everything else (walls, props, structures, units, glass) samples **one 3D irradiance probe volume** | No second UV set, no mesh-format change, no unwrap tool. Both are looked up from world position and normal, which `deferred_light` already reconstructs, so deferred, forward and Compat sample them the same way. The terrain is most of the screen at 46 m | Walls and props get probe-resolution light (2 m); leaks need validity handling (§3b) |
| (b) UV2 lightmaps everywhere | xatlas-unwrapped `TEXCOORD_1` on every static mesh, plus probes for dynamics | Per-texel detail on walls and props | A mesh-format change with a Compat fallback (ADR-0020 §2d); an unwrap step in the §9.4 pipeline; `deferred_light` cannot see UV2, so it needs a sixth G-buffer target (8 B/px, ADR-0023 §1d) written by the G-buffer pass; seams and texel budgets per asset |
| (c) Probe volume only | No lightmap at all | Simplest | The ground, which is most of the frame, gets 2 m resolution and leaks under every wall edge |

The terrain lightmap is a natural fit: the terrain is a heightfield, its relief on the board is
at most 0.5 m, and `ao-bake` already proved the world-`xz` mapping. Steep cliff faces in the
margin stretch it; there the lookup blends to the probe volume above a slope threshold (§5b).

**2a. The terrain lightmap.** Two levels, both square, both in world `xz`:

| Level | Covers | Texel | Size (Ember, 256 m board, 153.6 m margin) |
|---|---|---|---|
| `near` | the board, `[0, extent]²` (the baked AO's square) | 0.5 m (`--texel`) | 512² |
| `far` | the whole terrain, `[−margin, extent + margin]²` | 2.0 m | 288² (563 m, rounded up to a multiple of 16) |

The lookup takes `near` inside the board and `far` outside, cross-fading over the last 4 m.

**2b. Encoding.** Each texel stores **SH-L1 irradiance** (the same convention as
`EnvironmentDesc`: convolved with the clamped cosine and divided by π, so the diffuse term is
`albedo × max(0, Σ sh·Y(n))`), packed as:

| Texture | Format | Channels |
|---|---|---|
| `L0` | `RGBA16F` | rgb: the L0 coefficient per colour; a: **sky visibility**, the cosine-weighted fraction of the upper hemisphere that sees the sky (0–1) |
| `L1` | `RGBA8_UNORM` | rgb: the **luminance** L1 vector divided by `√3 × L0 luminance`, mapped from [−1, 1] to [0, 1]; a: spare |

12 bytes per texel. Colour comes from L0; direction from the luminance L1, scaled per channel
by L0. The `√3` normalisation bounds a physical irradiance's L1/L0 ratio, so the stored value
is in [−1, 1], which is exactly the range OIDN's directional lightmap mode expects (§6c, [S1]).
**Decision needed (maintainer) Q6:** this luminance-L1 encoding (recommended, 12 B per texel);
full RGB L1 (24 B, coloured directionality, double the size); or L0 only (8 B, no normal-map
response to baked light).

**2c. Why not SH-L2 like the environment.** L2 adds five coefficients per channel for detail
that indirect light at 0.5–2 m resolution does not have. The environment keeps SH-L2: it is
nine numbers per map, not per texel.

### 3. The irradiance probe volume

**3a. The grid.** One world-aligned 3D grid over the board, used by every non-terrain receiver,
static or dynamic:
- **2 m spacing** in `x` and `z` (128 × 128 on Ember's 256 m board);
- **9 layers** at 1, 3, 5 … 17 m above the board's zero height (the board's relief is at most
  0.5 m, and the tallest receivers are 15 m towers);
- outside the grid, the lookup clamps to its edge; nothing outside the board needs probes but
  the terrain, which uses the lightmap.

147,456 probes. Each probe stores the same L0/L1 pair as a lightmap texel (§2b), with `L0.a`
holding **validity** instead of sky visibility: 12 bytes per probe, 1.8 MB.

**3b. Leaks.** A probe inside a wall or prop, or right against one, sees back faces and would
light the wrong side. The bake follows the established validity approach (Unity's adaptive
probe volumes document the same three tools [S2]):
1. **Validity.** A probe whose rays hit back faces more than a set fraction of the time (25 %,
   tuned on the fixture) is **invalid**, `L0.a = 0`.
2. **Virtual offset.** Before an invalid probe is discarded, its capture point is pushed out of
   the geometry along the shortest free direction (at most half the spacing) and re-baked. If
   it is still invalid, it stays so.
3. **Dilation.** An invalid probe's coefficients are filled from its valid neighbours, so a
   trilinear lookup that cannot avoid it still gets plausible light.

At run time the lookup weights the eight surrounding probes by trilinear weight × validity ×
a back-face term (probes behind the surface along its normal get less weight), renormalised,
with the sample point offset along the normal and towards the eye by a quarter of the spacing.

**Rejected for N3: per-probe visibility (DDGI's octahedral depth moments, Majercik et al.,
literature).** It needs a depth map per probe (hundreds of bytes each, about 30× the
irradiance), and its purpose is *dynamic* probe updates. High's RT probe volume (N5) is the
place for it.

**3c. Dynamic receivers.** Units and structures sample the same volume per pixel in
`deferred_light`, so a hero walking into a tower's GI shadow darkens smoothly and matches the
ground under it. The roadmap's "irradiance probes for dynamic objects" and "a probe volume for
static geometry" are the same volume here.

### 4. `light-bake`: the tool

**4a. Shape.** A new `moba-tools light-bake` (`crates/tools/src/bin/light-bake/`), modelled on
`ao-bake`:

```text
light-bake bake [--pack <pack.toml>] [--root <asset root>] [--visuals <name>]
                [--texel M] [--probe-spacing M] [--samples N] [--bounces N]
                [--denoise oidn|filter|none] [--reflection-size N] [<out-stem>]
light-bake info <out-stem>
```

It loads the pack and its visuals with the client's own loaders, builds the static scene
(§1b), bakes on the GPU (§4b), denoises (§6), and writes the files of §4e.

**4b. Where it runs: the GPU, with a CPU oracle.** **Decision needed (maintainer) Q4.**
- **(a) Recommended: a Vulkan ray-query compute path tracer** in Slang (ADR-0020 §4: new RT
  passes are Slang), on a headless Tiered device. One thread per lightmap texel or probe,
  cosine-weighted hemisphere samples for L0/L1 projection, next-event estimation towards the
  sun and towards the sky. The sun's direct light *at the receiver itself* is not stored (§1a);
  sunlight enters from the first bounce on. Samples are seeded per texel from a fixed hash, accumulated in a fixed order: **the
  output is byte-identical for one GPU and driver**, not across them.
- **A pure-Rust CPU reference tracer** of the same estimator, on small scenes only, is the
  oracle for the verification gates (gate 1). It is not a production path.
- **(b) CPU only.** Deterministic everywhere and runs in CI, but a 512² + 288² + 147k-probe
  bake at 1,024 samples is about 0.5 billion paths; hours on a desktop CPU against seconds to
  minutes on the RTX 5070 Ti (an **estimate**, to be measured in gate 7).
- **(c) Blender Cycles bakes.** Blender is installed, and the slice uses it for asset bakes.
  But the map is generated by engine code (terrain, wall boxes, props), so every bake would
  first need an engine-to-Blender export; Cycles' Principled BSDF is not our material model;
  and it cannot run in CI. Rejected.

The bake is a developer-machine step. On the dev box it runs under
`flock /home/jaket/Projects/MOBA/handoff/gpu.lock`, like every real-GPU job. CI runs only the
tiny fixture on lavapipe, which supports ray query (it already runs `rt-smoke`).

**4c. The BVH it shares with N4.** **Decision needed (maintainer) Q5.** The roadmap says N3
"shares BVH with N4", but N4 has no ADR yet. Recommended: N3 promotes the BLAS/TLAS code of
`crates/render/src/smoke/rt.rs` into a `moba_render::rt` module that builds **compacted,
static BLASes** for the static scene (terrain chunks, walls, props) and one TLAS, used
headless by `light-bake`. N4's TLAS/BLAS manager starts from that module and adds per-frame
hero refits, VAT pose buckets, SER and OMM; the static proxy BLASes it wants (roadmap §5.5,
"built once and cached") are the same objects. The alternative, a baker-only BVH (CPU-built,
uploaded as a buffer, traversed in software), duplicates work N4 needs anyway.

**4d. Formats and the loader.**
- The lightmaps are 2D KTX2 (`RGBA16F`, `RGBA8_UNORM`): both are existing `TextureFormat`s.
- The probe volume is **two 3D KTX2 images** (128 × 128 × 9, the same two formats). The KTX2
  loader learns 3D (`pixelDepth > 0`), bounded like everything else, with no arrays and no
  supercompression; that is the [ADR-0010](0010-texture-pipeline.md) amendment above. The
  Vulkan core minimum for `maxImageDimension3D` is believed to be 256 (**unverified** here;
  N3a's admission test records it), well above 128.
- Reflection probes are cube KTX2, which the loader already reads (§7).
- **No BC6H in N3.** The sizes fit the storage rule uncompressed (§4e), and a BC6H encoder is
  new code; N11 (content scale, NTC) is where compression of bakes belongs.

**4e. Files, sizes and the storage rule.** Under `<root>/maps/light/`, named by the visuals:

| File | Ember size |
|---|---|
| `<name>.toml` | sidecar: extents, texel sizes, grid origin and spacing, probe boxes (§7), the fingerprint, the settings; under 4 KiB |
| `<name>-near-l0.ktx2`, `<name>-near-l1.ktx2` | 2.0 MB + 1.0 MB |
| `<name>-far-l0.ktx2`, `<name>-far-l1.ktx2` | 0.66 MB + 0.33 MB |
| `<name>-probes-l0.ktx2`, `<name>-probes-l1.ktx2` | 1.2 MB + 0.6 MB |
| `<name>-refl-<i>.ktx2`, at most 8 | 0.26 MB each at 64² with mips (2.1 MB for 8) |
| **Total per map** | **about 8 MB**; every file far under 10 MB |

`content-check` enforces **≤ 16 MB per map** (headroom for 128² reflection probes on a map that
needs them) and ≤ 10 MB per file. Ember's bake counts against the slice's 100 MB.

**Decision needed (maintainer) Q11: which maps are baked in N3.** Recommended: **Ember only**
(the slice's map, about 8 MB). Frost and the default grass map keep today's IBL ambient on
Tiered until their visuals are reworked for photoreal content; nothing about them changes. The
alternative is all three now (about 24 MB of the slice's 100 MB), which spends space on maps
the slice does not show. The roadmap's "baked for all maps" then closes when those maps are
reworked.

**Decision needed (maintainer) Q12: commit the bakes, or bake at build time.** Recommended:
**commit** the baked KTX2 files, as `ao-bake`'s are. A GPU bake is not reproducible across
GPUs, and neither CI nor a player's build has the developer's GPU. Baking at build time would
make the output depend on whoever builds.

**4f. The fingerprint and stale bakes.** `moba_assets::light_bake_fingerprint` hashes
everything the bake depends on: `ao_fingerprint`'s inputs (extent, walls, wall height, props and
their mesh shapes), the terrain parameters, the foliage parameters, `[lighting]`'s sun
direction, colour and intensity, the environment's name, its SH and a hash of its cube file,
`ibl_intensity`, each static material's mean albedo and emissive, and the bake settings and
tool version.
- **At run time** the client recomputes it, and on a mismatch **ignores the bake** with a
  warning and falls back to today's IBL ambient and cube, as it does for baked AO.
- **In CI** `content-check` fails a shipped map whose bake is stale.

**Decision needed (maintainer)** (part of Q12): this ignore-and-fall-back rule (recommended),
or refusing to start a match with a stale bake (stricter, but a cosmetic file would then block
play).

**4g. Provenance.** `content/maps/light/provenance.toml` records the bake like `ao-bake`'s
entry in ADR-0022's format (PR #349): `source = "original"`, the project's licence, and
`made_with` naming `light-bake`, its settings, the environment it used, and the denoiser and
its version (§6c). Until #349 merges, the file is written in that format and checked when the
gate lands.

### 5. The run-time lookup on Tiered

**5a. Where.** One shared Slang module, `shaders/slang/lib/baked.slang`, imported by
`deferred_light` and by the forward `water` and `glass` shaders (ADR-0023 §5b, one lighting
code path):

```slang
struct BakedLight { float3 irradiance; float sky_visibility; };  // irradiance already ÷ π
BakedLight baked_terrain(float3 p, float3 n);   // near/far lightmap, §2a
BakedLight baked_volume(float3 p, float3 n, float3 v);  // probe volume, §3b weighting
float3 baked_specular(float3 p, float3 r, float roughness, float3 irradiance_here);  // §7
```

`deferred_light`'s step 4 (ADR-0023 §2c) becomes: if a bake is bound, the diffuse ambient is
`baked_terrain` or `baked_volume`; otherwise today's SH-L2. Everything after it (AO, specular
occlusion, fog, rims) is unchanged. No new pass and no new render target.

**5b. Which pixels use the lightmap.** ADR-0023 kept `gb1.a` bit 1 for N3. It becomes
**`LIGHTMAP`**: set by the G-buffer fragment stages for terrain and foliage pixels, and by the
forward water shader for itself (its bed is terrain). `deferred_light` takes `baked_terrain` for
those, cross-fading to `baked_volume` where the terrain normal is steeper than 60° (cliff faces
in the margin); every other pixel takes `baked_volume`. **Decision needed (maintainer) Q13:**
use bit 1 this way (recommended), leaving model IDs 8–15 for N10 and N6 and declining the spare
colour attachment of ADR-0023 §1d, which the hybrid (Q1 a) does not need. Q1 (b) would need
that attachment instead.

**5c. AO.** With a valid bake, the diffuse ambient line is
`baked × min(material AO, GTAO)`. The baked AO map (`baked_ao_map`, ADR-0016) is **not**
applied to it: the bake's sky visibility already contains the same occluders at higher
fidelity, and applying both would darken wall bases twice. The baked AO map stays loaded for
Compat and as a fallback when the bake is stale. Specular occlusion (Lagarde and de Rousiers,
already in `lighting.glsl`) takes the bake's sky visibility on lightmap pixels.

**5d. Bindless.** The six lightmap and probe images and the reflection cubes are sampled-image
handles in the global bindless set (ADR-0020 §3a), through the existing linear-clamp sampler;
their extents and handles go in the frame constants. The tiered path's 16-sampler ceiling does
not apply (ADR-0023 §2d).

**5e. What does not use the bake.** Particles keep the key light and today's SH ambient
(ADR-0023 §5b keeps them GLSL). A particle is a short-lived light source, not a surface, so this
is a deliberate gap, as in N2.

### 6. Denoising

**6a. Why.** At 1,024 samples per texel the L1 band is still visibly noisy at wall edges; noise
in a lightmap reads as blotches on the ground, which the bible's VAL rules measure.

**6b. The in-house filter.** A deterministic, edge-aware à-trous filter over the lightmap, guided
by the terrain normal and height and by the sky visibility, and over the probe grid, guided by
validity. Pure Rust, no dependency, runs in CI on the fixture.

**6c. OIDN.** Intel Open Image Denoise is **Apache-2.0**; the current release is **v2.5.1
(2026-08-18)**, with an official `oidn-2.5.1.x86_64.linux.tar.gz` asset [S1]. Its
**`RTLightmap`** filter denoises HDR lightmaps and, with `directional = true`, normalised
directional coefficients in [−1, 1] such as an L1 band with L0 divided out [S1], which is our L1
encoding (§2b). Devices are CPU, SYCL (Intel), CUDA (NVIDIA), HIP (AMD) and Metal [S1].

**Decision needed (maintainer) Q7.** Recommended:
- OIDN is an **optional `oidn` feature of `moba-tools` only**; the client never links it. It is
  fetched like the vendor SDKs (ADR-0020 §6b): a `vendor-sdks.toml` entry pinning the release
  asset above by SHA-256 and its `LICENSE.txt` by SHA-256, unpacked under `target/vendor-sdks/`,
  nothing installed system-wide.
- It runs on the **CPU device**, so a bake denoises identically on any x86-64 machine and does
  not depend on CUDA. The bake is small (about 0.5 M texels), so the CPU is fast enough
  (**estimate**, gate 7).
- Without the feature, `--denoise filter` (§6b) is the default; the committed Ember bake uses
  OIDN.
- **Provenance:** `made_with` names OIDN and its version. OIDN's filters use trained weights,
  but it filters our own render rather than generating content; the recommendation is not to
  list the bake in `AI-DISCLOSURE.md`. That classification is the maintainer's call.

Alternatives: in-house filter only (no dependency, visibly softer or noisier result), or OIDN
mandatory for every bake (breaks the open build's ability to bake).

### 7. Reflection probes

**7a. What.** At most 8 **box-projected cube probes** per map, each a 64² `RGBA16F` cube with a
full GGX-prefiltered mip chain (`env-bake`'s prefilter code, reused), path-traced by
`light-bake` at its capture point with the baked lighting on every surface and the sun's direct
light included (a reflection sees the sunlit ground). Units and structures are not in them.

**7b. Placement.** **Decision needed (maintainer) Q10.** Recommended: **authored** in the map
visuals, a few lines each:

```toml
[[reflection_probes]]
position = [64.0, 64.0]   # world x, y (map coordinates)
height = 3.0               # capture height, metres
box = [48.0, 48.0, 20.0]   # projection box: width, depth, height, metres
```

with one map-wide fallback probe (the environment cube, as today) where none covers a pixel.
Four to eight boxes (each base, the river, the jungle quadrants) cover a MOBA board. The
alternative is an automatic grid; it costs nothing to author but spends probes on empty
ground.

**7c. Run time.** `deferred_light` selects at most two probes per 16 × 16 tile (the tile
classification ADR-0023 §2a already runs), blends them by box distance, and box-projects the
reflection vector. Each sample is **renormalised by irradiance**: scaled by the ratio of the
shading point's baked irradiance to the irradiance at the probe's capture point (stored in the
sidecar), as idTech 8 does (roadmap §6.1). That keeps a probe captured in sunlight from
brightening a reflection in a wall's shadow. Water and glass use the same function.

**7d. Tiers.** Low's "baked probes" cell is this. Medium adds SSR in front of it (N5); High and
up replace it below roughness 0.45 with RT reflections (N5, N8), and keep it as the fallback
above.

### 8. Compat's share (for C1)

C1 samples a subset, so the format is designed for Compat's full sampler budget (16 of 16,
ADR-0016 §6):
- **the `near` and `far` L0 lightmaps,** which carry sky visibility in alpha, **replace binding
  13** (the baked AO map) one for one, with the far level folded into the same image as a
  second region if C1 needs to save a sampler;
- **the probe volume's L0** needs one more sampler. ADR-0016 §6 already describes how to free
  one: move contact shadows into the GTAO pass and retire binding 14. C1 does that, or retires
  another binding;
- **reflection probes**: C1 binds the probe nearest each draw in place of the environment
  cube, or keeps the cube. C1 decides.
- Compat skips the L1 band, the validity weighting beyond a single tap, and the renormalisation.

N3 tests that every shipped bake loads and that the Compat subset can be evaluated by a GLSL
function (a Rust twin), but changes no Compat pixel. **The format freezes at N3c**, which with
N2c (the material spec) is what C1 waits for.

### 9. Per-tier use and budgets

**9a. Use per tier.**

| Tier | Diffuse ambient | Specular ambient | Added by later phases |
|---|---|---|---|
| Low | lightmap + probe volume | reflection probes | — |
| Medium | same | same | N5: half-res SSGI, SSR |
| High | same | same | N5: RT probe volume near the camera, RT reflections |
| Ultra | same, as the seed and fallback | same, as the fallback | N8: hash radiance cache, per-pixel gather |
| Overdrive | fallback only | fallback only | N9: path tracing; the bake can seed SHaRC |

**9b. Run-time cost: planning estimates** on each tier's reference GPU, inside
`deferred_light` (no new pass). Estimates never close a target.

| | Low (RTX 2060, 720p) | Medium (RTX 3060, 720p) | High (RTX 4070, 960p) | Ultra (RTX 5070 Ti) | Overdrive |
|---|---|---|---|---|---|
| Lightmap or probe lookup | 0.08 ms | 0.06 ms | 0.08 ms | 0.05 ms | 0 with N9 on |
| Reflection probes | 0.07 ms | 0.06 ms | 0.07 ms (above roughness 0.45) | 0.05 ms | 0 with N9 on |
| **N3 total** | **0.15 of 12.0** | **0.12 of 6.0** | **0.15 of 7.5** | **0.10 of 9.0** | — |

The roadmap's "runtime cost about 0.1 ms" (§6.1) is consistent with this. A probe lookup is
16 texel fetches (8 probes × 2 images) for non-terrain pixels and 2–4 for terrain pixels; a
reflection lookup is at most 2 cube samples.

**9c. VRAM.** About 8 MB per baked map (§4e), loaded with the map and inside ADR-0020 §5e's
tier VRAM budgets by a wide margin. Reported in `docs/perf/`.

**9d. Off switches and debug views.** `MOBA_BAKED_LIGHTING=off` (today's SH ambient and cube);
`MOBA_REFLECTION_PROBES=off` (the environment cube). `MOBA_DEBUG_VIEW` gains
`baked-irradiance|sky-visibility|lightmap-texels|probe-validity|reflection-probe`.

### 10. Determinism and the invariant

- Rendering still never touches the simulation, the wire, replays or the state hash. No version
  or hash moves: `PROTOCOL_VERSION` v42, replay v5, `SIM_LOGIC_VERSION` v9, content hashes and
  `tests/golden-state-hash.txt` are unchanged.
- Bakes are map visuals: client-local, cosmetic, never hashed or sent, like baked AO and
  environments.
- **No hidden information.** The bake contains only static map geometry, never units or
  structures, so it is the same for every player at every moment of every match, and nothing
  in it depends on what the server sends. Fog of war is still applied after lighting, as today.
- Nothing reads back from the GPU into gameplay.

## What this ADR does not touch

- The simulation, `moba-sim`, the wire format and `PROTOCOL_VERSION`, replays and their
  version, `SIM_LOGIC_VERSION`, content hashes and signatures, and
  `tests/golden-state-hash.txt`.
- **Compat's pixels.** Every Compat golden stays byte-identical in every N3 PR
  (`MOBA_RENDER_PATH=compat`). C1 rebaselines Compat when it adopts §8.
- **Tiered pixels without a bake.** With no bake bound (Frost and grass under Q11, or
  `MOBA_BAKED_LIGHTING=off`), every existing tiered golden is byte-identical.
- The N1 temporal path and overlay layer, the upscalers, and the G-buffer layout apart from
  `gb1.a` bit 1.
- The sun: direction, colour and real-time shadows are unchanged (§1a).
- The mesh format (no `TEXCOORD_1`) and the material format.
- Weather and the sky model (N6), the RT infrastructure beyond the static BLAS module (N4), and
  any dynamic GI (N5, N8, N9).

## Departures from the roadmap

| Roadmap says | This ADR decides | Why |
|---|---|---|
| §6.1: "SH-L1/L2 lightmaps or a probe volume for static geometry" | A top-down SH-L1 lightmap for the terrain, one probe volume for everything else (§2, Q1) | No UV2, no mesh-format change, no new G-buffer target; the terrain is a heightfield |
| §6.1: "irradiance probes for dynamic objects" as a separate set | The same volume lights static non-terrain surfaces and dynamic ones (§3c) | One lookup, and a unit matches the wall beside it |
| §6.1: "about 100–500 MB per map" | About 8 MB per map, ≤ 16 MB enforced (§4e) | The board is one 256 m square; the storage rule caps files at 10 MB and the slice at 100 MB |
| §8: "baked for all maps" | Ember in N3; other maps when their visuals are reworked (Q11) | Storage; they keep today's look meanwhile |
| §6.1: OIDN | Optional `oidn` tools feature on the CPU device, with an in-house fallback (§6, Q7) | Keeps the open build able to bake; reproducible across machines |
| §6.1: "SH-L1/L2" encoding | L0 RGB with luminance L1 (Q6) | Half the size of RGB L1; matches OIDN's directional mode |
| §6.1: shares BVH with §5.5 | N3 builds the static BLAS module N4 inherits (§4c, Q5) | N4 has no ADR yet; static proxies are the same objects |
| (implicit) structures in the bake | Structures excluded (§1b, Q3) | They are destroyed in play, as `ao-bake` already decided |

## Risks

| Risk | Mitigation |
|---|---|
| Probe leaks through thin walls light the wrong side of a unit | Validity, virtual offset, dilation and the back-face weight (§3b); the leak gate (gate 3); walls are 2.5 m thick boxes against 2 m spacing |
| Baked shadowing breaks the value floors (VAL-3, SHD-1) under towers and walls | Structures are not occluders (§1b); readability gates on the baked frame (gate 6); the floors are art rules the bake must meet, not tune around |
| The bake and the run-time sun or sky disagree (a changed `[lighting]` without a rebake) | The fingerprint covers them (§4f); `content-check` fails stale bakes; the client falls back |
| GPU bakes differ between developers' GPUs, so diffs are noisy | Commit the bake (Q12); one bake machine (the RTX 5070 Ti) recorded in `made_with`; the CPU oracle checks the estimator, not the bytes |
| Relative light units make the bible's physical targets unmeasurable | Ratio checks (Q8); physical units stay a later, separate decision |
| The map section for the slice changes after the bake | Re-bake per map PR; `scripts/bake-map-light.sh` (N3d); bake time gate |
| The KTX2 3D change loosens a hardened loader | Same bounds and fuzz targets as 2D; dimensions capped; no arrays or supercompression |
| OIDN's licence or release changes | Pinned by SHA-256 with its licence file's SHA-256 (ADR-0020 §6b); the in-house filter remains |
| N2b slips, delaying the run-time side | N3a (tool and formats) does not need N2b and can run in parallel |

## Alternatives considered

- **UV2 lightmaps on every static mesh** (Q1 b) and **probes only** (Q1 c): see §2.
- **A fully dynamic GI on every tier** (no bake). Roadmap §1 rejects it: the fixed map is "the
  biggest lever".
- **Baking direct sunlight with a shadow mask** (Q2): see §1a.
- **Baking structures, or two variants per structure** (Q3): see §1b.
- **CPU-only or Blender Cycles baking** (Q4 b, c): see §4b.
- **DDGI-style per-probe visibility** in the bake: see §3b; N5's job.
- **SH-L2 per texel:** see §2c.
- **BC6H-compressed bakes:** sizes fit without them; N11 (§4d).
- **Automatic reflection-probe placement:** see §7b.
- **Baking at build or install time:** see Q12.

## Consequences

- **Wire, replays and determinism: no change** (§10).
- **Frame shape:** none. N3 adds no pass; `deferred_light` and the forward water and glass
  shaders gain the bake lookup.
- **Shaders:** new Slang `shaders/slang/lib/baked.slang`; the baker's Slang compute kernels;
  the G-buffer fragments set `LIGHTMAP`.
- **Tools:** `light-bake` (new); `env-bake` and `ao-bake` unchanged; `content-check` gains the
  bake rules (fingerprint, sizes, per-map budget, probe boxes, provenance).
- **Content** (client-local): `[lighting] light_bake = "<name>"` and `[[reflection_probes]]` in
  map visuals, both optional; `content/maps/light/` for Ember.
- **Public API** (additive, `#[non_exhaustive]`): `moba_assets::{LightBakeDesc,
  light_bake_fingerprint, light_bake_paths, ReflectionProbeDesc}`; `moba_render::rt` (static
  BLAS/TLAS); `Renderer::set_light_bake`; `LightingConfig` off switches.
- **Licences:** `THIRD_PARTY.md` gains OIDN's Apache-2.0 notice (tools only, when built with
  `oidn`).
- **Docs:** `docs/content-authoring.md` (the keys, the bake workflow), `docs/status.md`,
  `docs/architecture.md`, `docs/screenshots.md` (debug views) and `preset.rs`'s cells move with
  the code. If Q1 (a) is taken, Low's Static GI label becomes "baked (terrain lightmap and
  probe volume)".

## Rollout

The roadmap's **L**, in four PRs. N3a can start as soon as this ADR is accepted; N3b needs
N2b's `deferred_light` on `main`.

| Phase | Scope | `preset.rs` cells it flips | Needs |
|---|---|---|---|
| **N3a** | `moba_render::rt` static BLAS/TLAS (from `smoke/rt.rs`); `light-bake` with the GPU tracer and the CPU oracle; terrain lightmap (both levels) and probe volume with validity, virtual offset and dilation; the in-house filter; KTX2 3D (ADR-0010 amendment); `LightBakeDesc`, the fingerprint, `content-check`, provenance; the CI fixture bake on lavapipe. **No run-time pixel changes.** | none | acceptance |
| **N3b** | The run-time lookup on Tiered (`baked.slang` in `deferred_light`, water, glass); `LIGHTMAP`; the fallback and off switches; debug views; the `oidn` feature; **the Ember bake in the Golden Crossing mood, committed** | Static GI Low, Medium, High → `Built`; Dynamic GI Low → `Built` | N2b merged |
| **N3c** | Reflection probes: bake, prefilter, box projection, renormalisation, per-tile selection; `[[reflection_probes]]`; **the bake format freezes (unblocks C1 with N2c)** | Reflections Low → `Built` | N3b |
| **N3d** | Incremental bake cache; `scripts/bake-map-light.sh`; other maps if Q11 says so; RTX perf evidence in `docs/perf/`; docs | none | N3c |

**How this unblocks the vertical slice.** A3 needs N2 and N3 (roadmap §8). After N3b and N3c,
the Ember map section in Golden Crossing is lit at Medium the way the matrix says: baked sky and
bounce on the ground and walls, units lit by the probe volume as they move between sun and
shadow, and probe reflections on the bible's metals and obsidian. That is what lets the slice
check SHD-1/SHD-2 and the bible's "GI bounce off sand" payoff, and judge the look at Medium.
N3 does **not** deliver Medium's SSGI and SSR (N5) or the Overdrive path tracer (N9): the
slice's Overdrive view uses the bake as its fallback until N9.

The slice's map geometry (A3's map-section PR) will change after N3b. Each such PR re-bakes
Ember with the committed tool and settings; the fingerprint makes a forgotten re-bake fail CI.

## Gates

These follow roadmap §10 and the gates of ADR-0020, ADR-0021 and ADR-0023, applied to N3.

1. **The estimator (N3a).**
   - **CPU oracle vs GPU** on the fixture scene (a 32 m board with one wall, one prop and an
     emissive block): mean relative error of L0 ≤ 2 % and of the stored L1 ≤ 0.02, at 4,096
     samples, on lavapipe.
   - **Open sky:** an empty flat board with albedo 0 bakes to the environment's SH irradiance
     for an up-facing normal within 1 %, with sky visibility 1.
   - **Furnace:** a closed box with uniform albedo ρ = 0.5 and uniform emission E converges to
     the analytic `E / (1 − ρ)` within 1 % (exercising multiple bounces and Russian roulette).
   - **View factor:** a long wall's sky visibility at a set of distances matches its analytic
     view factor within 1 %.
2. **Determinism (N3a).** The fixture bake on lavapipe is byte-identical across two runs; the
   Ember bake on the RTX 5070 Ti is byte-identical across two runs; the fingerprint changes when
   any listed input changes (one unit test per input) and not otherwise.
3. **Leaks (N3a, N3b).** In the fixture, every probe inside the wall is invalid before virtual
   offset; a receiver on the wall's shadowed side gets at most 10 % more irradiance from the
   volume lookup than the CPU oracle computes at that point.
4. **Run-time parity (N3b, N3c).**
   - The Slang lookups match their Rust twins within 1/255 over a grid of positions and
     normals, read back on lavapipe.
   - The same surface through `deferred_light` and through the forward path (water, glass)
     agrees within the G-buffer's quantisation (ADR-0023 gate 2).
   - **Fallback parity:** with no bake, or `MOBA_BAKED_LIGHTING=off`, every existing tiered
     golden is byte-identical.
5. **Goldens.**
   - **Every Compat golden is byte-identical** (`MOBA_RENDER_PATH=compat`) in every N3 PR.
   - New tiered goldens on lavapipe at small resolution with ADR-0021's warm-up: Low and High
     with the fixture bake; a reflection-probe scene (N3c); each byte-identical across two
     runs, with validation and sync validation clean. The fixture bake is committed under
     `tests/`, so the goldens never depend on a GPU bake.
6. **Readability (merge-gating, N3b on).** On the standard captures at every tier, both team
   palettes, rims on, Ember in Golden Crossing with its bake:
   - **CON-1, CON-2, CON-4, CON-5** (no tier below Low − 0.02);
   - **VAL-2** (ground mean L 0.30–0.58) and **VAL-3** (shadow floor L ≥ 0.18);
   - **SHD-1** in each structure's and wall's shadow, and **SHD-2** with a hero standing in it;
   - **RIM-2** in a tower's shadow.
7. **Performance.**
   - Bake time: the full Ember bake ≤ 10 min on the RTX 5070 Ti under the GPU lock (proposed;
     recorded with and without OIDN); the CI fixture ≤ 2 min on lavapipe.
   - Run time, on the RTX 5070 Ti, teamfight replay, each preset at its output resolution,
     median of 3 interleaved rounds against a detached `origin/main` build: the
     `deferred_light` lane grows by **≤ 0.05 ms** with the bake on (a proposed absolute gate:
     today's whole 1080p frame on the 5070 Ti is about 0.5–0.7 ms, so a ratio means little).
   - Each §9b cell scaled by the 5070 Ti / reference-GPU ratio from the R8 perf logs, labelled
     as an estimate. Estimates never close a target.
   - Map load: the bake's upload time recorded; `locked_uploads=0` and `locked_pipelines=0`
     hold.
   - VRAM and file sizes against §4e.
   - The Radeon 610M (RADV) runs the fixture bake with validation clean, as an AMD ray-query
     correctness smoke; not a performance reference.
8. **Content.** `content-check` passes the Ember bake (fingerprint, ≤ 10 MB per file, ≤ 16 MB per
   map, probe boxes inside the map, provenance entry), and fails a deliberately stale copy.
9. **Compat contract (for C1, N3c).** Every shipped bake loads; the Rust twin of the Compat
   subset (§8) evaluates it; the format is frozen from here.

## Blockers

**Maintainer:**
- Accepting this ADR and answering [Decisions needed](#decisions-needed-maintainer).
- Q9: whether an HDRI-derived sky (for example `rogland_sunset`) may ship in the slice.
- Q7: whether an OIDN-denoised bake is listed in `AI-DISCLOSURE.md`.
- The slice's readability sign-off at 46 m, which will include the baked lighting.

**Code dependencies:**
- N3b needs N2b (`deferred_light`) on `main`; N2a is PR #369, still open.
- The Golden Crossing bake needs the mood in Ember's map visuals (A3's map-section PR) and,
  for N3b's final bake, the reworked map geometry.
- ADR-0022's provenance gate (#349) for checking `provenance.toml`; N3a writes the file in its
  format either way.

**Hardware:**
- Only the RTX 5070 Ti is owned, so §9b's per-tier costs stay estimates until the reference
  GPUs arrive (#9/#127).
- The bake is Linux-only until the Windows VM exists; nothing in N3 needs Windows.
- Bakes and RTX timings run under the GPU lock, one holder at a time.

## Decisions needed (maintainer)

Each is also marked where it arises. The recommendation comes first.

1. **Q1, how static GI is stored.** *Recommendation:* **(a)** a top-down terrain lightmap plus
   one irradiance probe volume. Alternatives: (b) UV2 lightmaps on every static mesh; (c) probes
   only. (§2)
2. **Q2, bake the sun?** *Recommendation:* no. Bake sky light, bounces and static emission, and
   keep the sun real time on every tier. (§1a)
3. **Q3, structures in the bake?** *Recommendation:* exclude them; they are dynamic receivers.
   Alternatives: bake them in (ghost lighting after they fall); two variants per structure. (§1b)
4. **Q4, where the bake runs.** *Recommendation:* the GPU, with Vulkan ray query in Slang and a
   CPU oracle for tests. Alternatives: CPU only; Blender Cycles. (§4b)
5. **Q5, the BVH.** *Recommendation:* N3 builds `moba_render::rt` (static BLAS/TLAS), and N4
   inherits it. Alternative: a baker-only software BVH. (§4c)
6. **Q6, encoding.** *Recommendation:* L0 RGB with luminance L1, 12 B per texel or probe.
   Alternatives: full RGB L1 (24 B); L0 only (8 B). (§2b)
7. **Q7, the denoiser.** *Recommendation:* OIDN v2.5.1 as an optional `oidn` tools feature,
   fetched and pinned through `vendor-sdks.toml`, on the CPU device, with an in-house filter as
   the default without it; recorded in `made_with`, not listed as AI-generated. Alternatives:
   the in-house filter only; OIDN mandatory. (§6)
8. **Q8, light units.** *Recommendation:* keep the engine's relative units (`sun_color ×
   sun_intensity`, `ibl_intensity`), so the bake matches what the run time already draws, and
   check the bible's physical targets as **ratios** (sun to sky illuminance at the board,
   measured by the bake's open-sky gate) rather than absolute klx and EV100. Alternative:
   physical units now, which also moves the exposure and every map's tuning, and belongs with a
   physical-camera change of its own (roadmap §6.7). (§1d)
9. **Q9, the slice's sky.** The bake reads the sky only through the map's environment, so the
   choice lives in `env-bake`, not here. *Recommendation:* **(a)** Golden Crossing's analytic
   sky from the bible (`env-bake` with mood A's parameters): reproducible, consistent with the
   28° sun by construction, and no download. **(b)** An `env-bake --hdri` import of
   `rogland_sunset` (CC0 [S3], named in the bible's `references.md`, fetched at a modest
   resolution and pinned by SHA-256, its sun clipped and replaced by mood A's analytic sun,
   rotated to the mood's azimuth), used only if you approve shipping an HDRI-derived cube, as
   the bible requires. **(c)** ADR-0025's Hillaire sky (`env-bake --sky hillaire`) if that ADR
   is accepted; N3 needs no change for it, which meets ADR-0025's constraint that the bake and
   the IBL use the same sky. (§1d)
10. **Q10, reflection-probe placement.** *Recommendation:* authored boxes in map visuals, at
    most 8, with the environment cube as the fallback. Alternative: an automatic grid. (§7b)
11. **Q11, which maps.** *Recommendation:* Ember only in N3 (about 8 MB); Frost and the grass map
    keep today's ambient until reworked. Alternative: all three now (about 24 MB). (§4e)
12. **Q12, commit the bakes, and the stale-bake rule.** *Recommendation:* commit them; at run
    time ignore a stale bake and fall back with a warning, while `content-check` fails it in CI.
    Alternatives: bake at build time; refuse to play with a stale bake. (§4e, §4f)
13. **Q13, the G-buffer.** *Recommendation:* use ADR-0023's spare `gb1.a` bit 1 as `LIGHTMAP`;
    decline the spare colour attachment and leave model IDs 8–15 to N10 and N6. (§5b)


## Corrections to the roadmap

These were checked on 2026-11-14. This PR only adds a link to this ADR from roadmap §12; the
maintainer may fold the corrections in when this ADR is accepted.

- **§6.1, storage "about 100–500 MB per map".** For one 256 m board with a 2 m probe grid,
  0.5 m terrain texels and 8 small reflection probes, the bake is about 8 MB uncompressed (§4e).
  The roadmap's figure fits multi-kilometre levels or per-texel lightmaps on every mesh, not
  this board.
- **§9.4, "`normal-bake`: a Rust high→low baker on the `ao-bake` ray core".** `ao-bake` has no
  ray core: it rasterises a height field and computes horizon AO over it
  (`crates/tools/src/bin/ao-bake/bake.rs`). A ray core arrives with N3's `moba_render::rt` (GPU)
  and the CPU oracle; `normal-bake` can build on either.
- **§6.1, OIDN.** Current release v2.5.1 (2026-08-18), Apache-2.0, with an official x86-64
  Linux binary and a directional lightmap mode suited to SH-L1 [S1].
- **§6.1, "SH-L1/L2 lightmaps".** L2 per texel is not needed for indirect light at this
  resolution (§2c).
- **Art bible (`lighting-mood.md`, not the roadmap), "sky turbidity (Hillaire/Preetham)".**
  ADR-0025 already notes that Hillaire's model has no turbidity parameter. N3 needs neither:
  under Q8 the bake takes the sky from the environment and checks ratios.

## Sources

All accessed 2026-11-14.

- [S1] Intel Open Image Denoise: README (licence; `RTLightmap` and its `directional`
  parameter; device types) and the latest release (v2.5.1, published 2026-08-18; assets include
  `oidn-2.5.1.x86_64.linux.tar.gz`):
  <https://raw.githubusercontent.com/RenderKit/oidn/master/README.md>,
  <https://github.com/RenderKit/oidn/releases>,
  <https://api.github.com/repos/RenderKit/oidn/releases/latest>
- [S2] Unity, Adaptive Probe Volumes: probe validity, virtual offset, dilation and leak
  reduction:
  <https://docs.unity3d.com/Packages/com.unity.render-pipelines.universal@17.0/manual/probevolumes-fixissues.html>,
  <https://docs.unity3d.com/Manual/urp/probevolumes-troubleshoot-artefacts.html>,
  <https://unity.com/en/blog/engine-platform/new-ways-of-applying-global-illumination-in-unity-6>
- [S3] Poly Haven, `rogland_sunset` (CC0, up to 24K, EXR and HDR):
  <https://polyhaven.com/a/rogland_sunset>
- **Literature (not re-checked):** Z. Majercik, J.-P. Guertin, D. Nowrouzezahrai, M. McGuire,
  "Dynamic Diffuse Global Illumination with Ray-Traced Irradiance Fields", JCGT 2019 (DDGI);
  P.-P. Sloan, "Stupid Spherical Harmonics (SH) Tricks", GDC 2008; S. Lagarde, C. de Rousiers,
  "Moving Frostbite to Physically Based Rendering", SIGGRAPH 2014 (specular occlusion); the
  idTech 8 irradiance renormalisation of reflection probes is as roadmap §6.1 cites it.
- **Project inputs:** the art bible v0 on PR #347 (`docs/art/lighting-mood.md`: moods,
  SHD-1 to SHD-3, physical targets; `palette-and-value.md`: VAL-2, VAL-3, CON-1 to CON-5;
  `material-vocabulary.md` G3; `references.md` §1.1), approved 2026-11-14; ADR-0022
  (`docs/adr/0022-provenance-gate-mikktspace.md`, PR #349); ADR-0025
  (`docs/adr/0025-atmosphere-weather.md`, PR #361, proposed) §1e–§1f; ADR-0024
  (`docs/adr/0024-gpu-driven-geometry.md`, PR #362, proposed).
