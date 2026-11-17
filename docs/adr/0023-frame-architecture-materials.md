# ADR-0023: Frame architecture and materials: thin G-buffer, compute deferred lighting, LTC area lights and the multi-layer material model (N2)

**Status:** accepted 2026-11-02 by the maintainer, with every recommendation under
[Open questions](#open-questions-for-the-maintainer) taken (see
[Maintainer answers](#maintainer-answers-2026-11-02)). N2a is recorded in
[Amendment 1](#amendment-1-2026-11-03-n2a-reversed-z-and-the-g-buffer-targets-as-built), N2b in
[Amendment 2](#amendment-2-2026-11-15-n2b-deferred-lighting-for-standard-as-built) and N2c, with the
frozen material spec, in
[Amendment 3](#amendment-3-2026-11-17-n2c-the-material-model-as-built-and-the-frozen-material-spec).
Proposed 2026-11-02. This is phase N2 of the
[next-gen graphics roadmap](../plans/next-gen-graphics-roadmap.md) (§3, §4, §5.2, §6.2, §6.6, §8
and §12), written before the code.
- **Builds on:**
  - [ADR-0020](0020-next-gen-foundations.md) (N0, accepted 2026-11-01, with Amendments 1–6). It
    settles the `Tiered`/`Compat` paths (§2), the bindless set and buffer device address (§3),
    Slang (§4), the tier budgets (§5) and the vendor-SDK features (§6). This ADR uses them and
    does not re-decide them.
  - [ADR-0021](0021-temporal-upscaling-overlay.md) (N1, accepted 2026-11-02). N2 takes over
    N1's render-resolution `prepass` and its `motion` attachment, writes N1's reactive and
    `tc_mask` targets from the new forward passes, and leaves N1's post-upscale overlay layer
    as it is. If ADR-0021 is amended in its implementation, this ADR follows it.
- **Amends, for the Tiered path only:**
  - [ADR-0011](0011-art-style-lighting.md) §1, §2 and §6 rule 1: shading moves from the forward
    `shade()` to a compute pass; the stylisation layer (wrap, ramp, painted specular, map rim)
    leaves the tiered frame; team identity moves to the rim (§8);
  - [ADR-0016](0016-lights-outlines-ao.md) §1: the lights gain area shapes (§4), and the
    "why not deferred" reasoning no longer holds (§2);
  - [ADR-0017](0017-decals-foliage-water.md) §1: `Paint` decals become G-buffer decals (§7);
  - [ADR-0015](0015-post-processing.md) §1: the `scene` pass is replaced on Tiered;
  - [ADR-0008](0008-render-graph.md): the graph gains compute passes over G-buffer images and
    indirect dispatches (both already exist as access kinds; see §11);
  - [ADR-0020](0020-next-gen-foundations.md) §1e: `maxColorAttachments ≥ 8` joins the tiered
    admission limits (§1d).
- **Compat is unchanged by N2.** It keeps today's forward frame, `shade()`, forward decals and
  plain descriptor sets (ADR-0020 §2d). C1 later refreshes it from the material spec in §6 and
  §10, then freezes it.
- **ADR numbers.** ADR-0022 is taken by the A0 licence and provenance gate (PR #349,
  `docs/adr/0022-provenance-gate-mikktspace.md`), so N2 is ADR-0023.

External claims (papers, specifications, vendor SDK inputs, GPU format support and hardware
figures) were re-checked on 2026-11-02 against the primary sources in [Sources](#sources), as the
roadmap's preamble requires. Facts ADR-0020 or ADR-0021 already verified are cited as theirs and
not re-derived. Claims that could not be confirmed are marked **unverified**.

## Context

Roadmap §8 gives N2 seven items: a thin G-buffer, compute deferred lighting, LTC area lights,
forward transparents, G-buffer decals, the multi-layer material model, and team rims in the
lighting. Roadmap §5.2 explains why: the guide buffers that NRD and DLSS Ray Reconstruction
need (N4, N9), one primary-hit description for RT and path tracing, material cost decoupled
from geometry, far fewer pipelines, and a home for software VRS.

What the code does today (`main` at `a17823a`, which has N0.1–N0.3 and the Slang toolchain):

- **One forward frame on both paths.** ADR-0020 §2c: the Tiered path still runs today's frame
  under the new presets. Its order is `(prepass) → (contact_shadows, ao) → scene → water →
  distortion → vfx_draw → (vfx_resolve) → post` (ADR-0015 §1, ADR-0016 §5, ADR-0017, ADR-0019).
- **Shading.** Every lit shader calls `shade(Surface)` in `include/lighting.glsl` (ADR-0011):
  - GGX, height-correlated Smith, Schlick, Lambert `(1 − F)(1 − metallic)`;
  - SH-L2 diffuse IBL and a prefiltered cube through a 32² split-sum LUT (`F0·A + B`);
  - one fitted sun shadow map, PCF or PCSS-lite;
  - the stylisation layer: wrap, ramp, painted specular and a rim
    `(1 − N·V)³ × map rim × material rim × rim colour`, half of it taking the surface's hue;
  - up to 256 point lights in a CPU-binned 32 × 32 **ground-column grid**, at most 16 per
    column (ADR-0016 §1), with no shadows;
  - AO on the ambient line only, `min(surface, GTAO) × baked`.
- **Team colour lives in albedo,** through the material's `mask` texture and `team_tint`
  (ADR-0011 rule 1, ADR-0010). Outlines carry the team colour on heroes (ADR-0016 §2).
- **Height fog** is applied by `unit.frag`, `terrain.frag` and `foliage.frag`; **fog of war** by
  the terrain and foliage only (units are already filtered by the server).
- **Decals** (ADR-0017 §1) are inlined into the terrain, water and foliage shaders: `Paint`
  into albedo before lighting, `Glow` after shading. N1 moves `Glow` to the overlay layer
  (ADR-0021 §5b). The inlined code is why `scripts/terrain-decal-check.sh` guards the terrain
  shader's register count.
- **Materials** (`materials/<model>.toml`): base colour, normal, `metallic_roughness` (R metal,
  G roughness) and `mask` textures; factors; `team_tint`; a constant `emissive`; the
  stylisation keys. The GPU block is 80 bytes (`MaterialParamsGpu`). Materials are client-local
  and cosmetic: not in the content hash, the signature or the wire
  ([content-authoring.md](../content-authoring.md)).
- **Transparency.** Additive particles add to HDR; alpha particles use **weighted blended OIT**
  (McGuire and Bavoil [S11]) on Medium and up, with an `RGBA16F` accumulation and an `R16F`
  revealage target that both blend `(ONE, ONE)` (ADR-0013). Water refracts a copy of the
  opaque HDR (ADR-0017). There is no transmissive geometry.
- **Targets.** HDR is `B10G11R11_UFLOAT_PACK32` on Low and `R16G16B16A16_SFLOAT` above
  (`QualityTier::hdr_format`); depth is `D32_SFLOAT` (or `D16`) with no stencil, non-reversed
  (ADR-0021 §2a leaves reversed-Z to N2).
- **Presets.** `crates/render/src/preset.rs` lists "thin G-buffer, deferred", "clustered, 2–4
  shadowed", "plus LTC area lights" and the material row as `Planned(N2)`.
- **Graph limits.** `MAX_PASSES = 40`, `MAX_RESOURCES = 48` under a `u64` mask.

Constraints:
- **Presentation only.** Rendering never touches the simulation, the wire, replays or the
  state hash. Nothing here changes `PROTOCOL_VERSION` (v42), replay `VERSION` (v5),
  `SIM_LOGIC_VERSION` (v9), content hashes or `tests/golden-state-hash.txt` (§12).
- **Readability is non-negotiable** (roadmap §4). Rule 2 ("team-tinted rim light that survives
  darkness, fog and GI") is N2's to deliver. The art bible's rim rules (RIM-1 to RIM-6) and
  contrast rules (CON-1 to CON-5) are its numeric form (PR #347, read-only here).
- **Hidden information.** Nothing in the G-buffer, the rims or the lights may show a unit the
  scene would not draw (ADR-0016 §2).
- **The R8 rules hold:** no per-frame allocation, `locked_uploads=0`, `locked_pipelines=0`.
- **Compat is byte-identical** through N2. Every Compat golden stays as it is.
- **Open art decisions stay open.** The bible's open decisions that affect N2 become switches:
  rim vs outline on allies, and `team_tint` on photoreal content (§8c).

## Decision

### 1. The thin G-buffer

**1a. Layout.** Three colour targets of 4 bytes each at render resolution, plus N1's depth and
motion. The same layout on every tier.

| Target | Format | Channels | Bits |
|---|---|---|---|
| `gb0` | `R8G8B8A8_UNORM` | rgb base colour, **sRGB-encoded in the shader**; a metallic | 8 / 8 / 8 / 8 |
| `gb1` | `A2B10G10R10_UNORM_PACK32` | rg octahedral normal (world space); b perceptual roughness; a flags: bit 0 `UNIT`, bit 1 reserved | 10 / 10 / 10 / 2 |
| `gb2` | `R8G8B8A8_UNORM` | rgb model data (§6c); a: shading model (bits 4–7), `NO_FOG_OF_WAR` (bit 3), rim slot (bits 0–2) | 8 / 8 / 8 / 4+1+3 |
| `depth` | `D32_SFLOAT`, **reversed-Z** (§1c) | from N1 | 32 |
| `motion` | `R16G16_SFLOAT` | N1's motion vectors (ADR-0021 §2a) | 16 / 16 |
| `hdr` | `R16G16B16A16_SFLOAT` | the light accumulation target, cleared to 0; the G-buffer pass writes emissive and unlit radiance into it | 16 × 4 |

- **Why these formats.** All three are mandatory as colour attachments with blending and as
  sampled images in Vulkan's required-format tables [S12]. `R8G8B8A8_UNORM` is also a mandatory
  storage format; `A2B10G10R10_UNORM_PACK32` storage is only required with
  `shaderStorageImageExtendedFormats`, which Vulkan 1.3 does not mandate [S12][S13]. So nothing
  in N2 writes `gb1` from compute; the only writers are raster passes (§7).
- **sRGB in the shader, not an `_SRGB` format.** It keeps `gb0` a mandatory storage format, so
  N6's wetness pass may later rewrite it from compute. The 8-bit sRGB encoding keeps the dark
  end precise: the bible's Void bodies sit at luma 39–58 (art bible `material-vocabulary.md`
  §3.2).
- **Octahedral normals** follow Cigolle et al. [S10]. 10 + 10 bits; the round-trip error is
  measured by a unit test (verification gate 1) rather than assumed.
- **Roughness is perceptual** (`alpha = roughness²`), as ADR-0011 and the bible define it, in
  10 bits. The ≥ 0.045 clamp (ADR-0011) and the bible's ≥ 0.08 floor on units are applied when
  writing.
- **No emissive target.** Roadmap §5.2 step 1 lists emissive among the G-buffer targets. Here
  emissive goes straight into `hdr`, which the G-buffer pass already binds as its fifth
  attachment, and which the lighting pass then adds to (§2d). That saves 4–8 bytes per pixel,
  and an unlit surface (hard light) needs no G-buffer slot at all. See
  [Departures](#departures-from-the-roadmap).
- **Flags.** `UNIT` marks every unit pixel. It replaces N1's 1-bit unit mask for distortion
  (ADR-0021 §5c) and drives the rim (§8). `NO_FOG_OF_WAR` reproduces today's rule that only
  terrain and foliage take fog of war (`unit.frag` applies height fog only).
- **Two spare fields.** `gb1.a` bit 1 and model ID values 8–15 are kept for N3 (baked-lighting
  receivers) and N10 (skin, eyes).

**1b. Bytes per pixel and memory per tier.** The targets are `Render { shift: 0 }` images
(ADR-0015 §1), allocated at the **ceiling** of the tier's dynamic-resolution range (ADR-0020
§5c; ADR-0021 §4c), so a scale step never reallocates.

| Tier | Output | Ceiling (render size) | Pixels | New G-buffer (12 B/px) | Touched by the G-buffer pass (28 B/px) |
|---|---|---|---|---|---|
| Low | 1080p | 0.67 → 1280 × 720 | 0.92 M | 11.1 MB | 25.8 MB |
| Medium | 1080p | 0.67 → 1280 × 720 | 0.92 M | 11.1 MB | 25.8 MB |
| High | 1440p | 0.67 → 1707 × 960 | 1.64 M | 19.7 MB | 45.9 MB |
| Ultra | 1440p / 4K | 0.67 / 0.50 → 1707 × 960 / 1920 × 1080 | 1.64 / 2.07 M | 19.7 / 24.9 MB | 45.9 / 58.1 MB |
| Overdrive | 1440p–4K | 0.50 → up to 1920 × 1080 | ≤ 2.07 M | ≤ 24.9 MB | ≤ 58.1 MB |

- "Touched" counts `gb0`–`gb2`, depth, motion and `hdr`, written once (12 + 4 + 4 + 8 bytes).
  Only the 12 bytes of `gb0`–`gb2` are new memory; depth and motion exist from N1, and `hdr`
  from today.
- **On Low the lit target becomes `RGBA16F`.** Compute writes it (§2), and
  `B10G11R11_UFLOAT_PACK32` is neither a mandatory colour attachment nor a mandatory storage
  format [S12]. The cost is 3.7 MB at 720p, and Low's HDR then matches the other tiers'.
- **Bandwidth floor.** The opaque path moves about 80 bytes per pixel per frame (the G-buffer
  pass at 1.5× overdraw, classification, and the lighting pass's reads plus the `hdr`
  read-modify-write). At 720p that is about 75 MB per frame:
  - about 0.34 ms at the RX 6600's 224 GB/s [S16], before its 32 MB Infinity Cache, which can
    hold the whole 11 MB G-buffer;
  - about 0.21 ms at the RTX 3060 12 GB's 360 GB/s [S16];
  - on the RTX 2060 the commonly quoted 336 GB/s is **unverified** (NVIDIA's page gives 6 GB at
    14 Gbps, not the bus width) [S16].

  These are lower bounds at peak bandwidth, not estimates of the pass time. The §9 budgets
  allow 3–4× that.

**1c. Reversed-Z, on Tiered, in N2.** ADR-0021 §2a deferred it here, because N2 rewrites every
depth consumer anyway.
- Depth is cleared to 0, tested `GREATER_OR_EQUAL`, with a finite far plane (the upscalers take
  near and far, ADR-0021 §4a). Each upscaler back end is given its "inverted depth" flag
  (ADR-0021 [S1][S2][S5]).
- **Every tiered depth consumer switches together:** the G-buffer pass, decals (§7), GTAO and
  contact shadows, the sky draw, water depth fade, soft particles, TAAU's disocclusion test and
  the reconstruction helpers in `shaders/slang/lib/`. N1's output-resolution overlay prepass
  switches too, so one convention holds on Tiered.
- Compat keeps today's convention; its goldens stay byte-identical.

**1d. Admission.** The G-buffer pass binds 5 colour attachments. The Vulkan core minimum for
`maxColorAttachments` is **4** (7 under the Roadmap 2022 profile, 8 under Roadmap 2024 / 1.4)
[S13]. The tier-floor GPUs report 8 (ADR-0020 Amendment 1). So `maxColorAttachments ≥ 8` joins
the tiered admission limits (ADR-0020 §3b), leaving N3 room for one more target. A device below
it is Compat with `maxColorAttachments` in `missing`. `independentBlend` is already required by
N1 (ADR-0021 §2e) and is used by §7.

**1e. Who writes the G-buffer.** N1's render-resolution `prepass` (depth + motion) **becomes
the G-buffer pass**, as roadmap §5.2 step 1 and ADR-0021 §2b anticipate.
- It draws terrain, foliage, walls, props, structures, units (static, skinned, VAT) and the
  far-plane background-motion draw, all jittered (ADR-0021 §1).
- It runs in two groups, so `Paint` decals fall between them (§7): **receivers** (terrain and
  foliage), then **the rest**.
- **Fragment stages are new Slang shaders** (ADR-0020 §4a). **Vertex stages stay N1's GLSL
  prepass shaders**, which already compute the current and previous positions. SPIR-V modules
  from the two compilers link by location; a reflection test pins the interface (verification gate 1).
  Porting the vertex stages waits for N7, which rewrites them for GPU-driven geometry
  (Q8).
- **Materials come from a bindless table** (§6d). The fragment shader depends only on the
  vertex format and alpha test, not on the material, so the opaque tiered pipelines drop to
  one per vertex format × {opaque, alpha-tested}: about 10, against today's permutation set
  (counted before and after in the PR, verification gate 7).
- **Foliage is opaque in the G-buffer,** alpha-tested, with the `FOLIAGE` model (§6c). Hair
  cards stay forward (N10).

**1f. Debug views.** `MOBA_DEBUG_VIEW` gains `albedo|normal|roughness|metallic|model|rim-slot|
flags|tiles|light-count|lit-only`, next to N1's views.

### 2. Compute deferred lighting

**2a. Passes.** After the G-buffer pass, the decals and the AO passes:

```text
light_classify   compute, one 16 × 16 tile per workgroup → tile lists + indirect args
deferred_light   compute, vkCmdDispatchIndirect per tile class → hdr (+=)
sky              raster, depth EQUAL far (0) → hdr
```

- **`light_classify`** reads depth and `gb2` per tile and writes three tile lists and their
  `VkDispatchIndirectCommand`s:
  - **sky** tiles (every pixel at the far plane): skipped by `deferred_light`;
  - **standard** tiles (only `STANDARD` and `UNLIT` pixels): the fast permutation;
  - **extended** tiles (any other model): the full permutation, a `switch` on the model ID.

  It also records each tile's depth range and model mask for the debug views. The lists sit in
  two fixed buffers sized for the maximum tile count at the ceiling resolution, and are reset
  by a fill command each frame.
- **Two lighting permutations, not one per material.** The model set is bounded (§6), so the
  extended permutation's `switch` covers all of it. If the extended permutation's register
  count lowers occupancy on standard pixels, that is exactly what the split avoids.
- **The tiles are the VRS hook.** Software variable-rate shading in compute (roadmap §6.8) and
  HUD-mask tile skipping can later add tile classes here. N2 does not do either.
- **Dynamic resolution.** Classification covers only the render sub-rectangle; tiles at its
  edge clamp their reads.

**2b. Light lists: the existing ground-column grid.** Roadmap §5.2 step 2 says "the existing
clustered light lists". They are kept exactly:
- the CPU still bins up to 256 lights into the 32 × 32 ground columns, at most 16 per column,
  with the per-tier caps of ADR-0016 §5 (an ADR-0016 alternative, GPU binning, stays rejected
  until N8's many-light work);
- `deferred_light` reconstructs each pixel's world position from depth, finds its column and
  loops over that column's list, as `shade()` does today.
- **No per-tile light culling in N2.** A 16-pixel tile is about 0.9–1.2 m on the ground at the
  default camera (roadmap §2's 20–27 px/m at 1080p–1440p output, at the 0.67 render scale),
  against 4.6 m columns (ADR-0016 §1), so a tile almost always lies in one column. This is
  clustered shading (Olsson et al. [S18]) with clusters fitted to the board, as ADR-0016 chose. Where a tile straddles two, the
  loop diverges for that tile only.
- **Wave-uniform loop.** Where the device reports subgroup `BALLOT` and `ARITHMETIC` in compute,
  the loop is scalarised (each iteration takes the subgroup's smallest pending light index). The
  core minimum guarantees only `BASIC` in compute; the Roadmap 2022 profile raises it to ballot,
  arithmetic, shuffle and quad [S13]. So the scalarised loop is a permutation, chosen at
  creation; the plain loop is the fallback.
- **The same set of lights lights every surface.** The windowed falloff reaches zero at the
  radius (ADR-0016 §1), and the forward passes (§5) read the same column grid. So a deferred
  pixel and a forward pixel at the same point evaluate the same lights, with the same caps and
  the same per-fragment clamp.

**2c. What the lighting pass computes, per pixel:**
1. decode `gb0`–`gb2` (§6c) into `SurfaceParams`;
2. sun: shadow map (Low to Ultra, ADR-0011 §4) × contact shadows, PCSS per tier, the shadow
   strength floor (ADR-0011 rule 2);
3. clustered point and area lights (§4);
4. ambient: SH-L2 diffuse and the specular cube (until N3's probes), with
   `min(material AO, GTAO) × baked AO` on the ambient line only (ADR-0016 §6), and specular
   occlusion;
5. the multiscatter compensation (§6b);
6. height fog for every pixel, and fog of war unless `NO_FOG_OF_WAR` is set, with today's
   functions;
7. the team rim, **after fog** (§8);
8. `hdr += result`.

GTAO can now read real normals from `gb1` instead of reconstructing them from depth. That is
an option for the AO pass, measured separately, not a requirement of N2.

**2d. Bindless.** `deferred_light` binds only the global bindless set (ADR-0020 §3a) and a push
block:
- `gb0`–`gb2`, depth, AO, contact shadows, the shadow map, the environment cube, the split-sum
  LUT, the LTC LUTs (§4b), baked AO and the fog-of-war texture are **sampled-image handles**
  (binding 2), sampled through the fixed sampler palette (binding 0, including the compare
  sampler for the shadow map);
- `hdr` is a **storage-image handle** (binding 1); `RGBA16F` storage is mandatory [S12];
- the light list, the column grid, the tile lists and the rim palette are **buffer device
  addresses** in the frame constants (ADR-0020 §3a).

So the pass is outside the 16-sampler ceiling that today's scene layout has spent (ADR-0016
§6), which is the "arbitrary resource access" ADR-0020 §3c named for N2.

**2e. Why compute, not in-pass deferred.** Dynamic rendering local read helps only in-pass
deferred, and ADR-0020 Amendment 1 kept the tiered path on Vulkan 1.3 because N2 is a compute
pass. Compute also gives the tile classification and the VRS hook above. Amendment 1's
"revisit if N2 moves to in-pass deferred" does not trigger.

### 3. Shadowed local lights move to ray-query shadow rays (N4/N5)

The §3 feature matrix and `preset.rs` give Low "clustered, 2–4 shadowed" as `Planned(N2)`.
N2 does **not** build local-light shadow maps:
- every tier has hardware ray query (roadmap §1, ADR-0020 §1e), and N4 builds the TLAS;
- a shadow-map atlas for 2–4 point lights (cube or tetrahedral faces, a caster pass per face)
  would be built in N2 and thrown away in N4/N5, where High's "RT shadow rays, top-N per tile"
  replaces it anyway;
- so shadowed local lights become **ray-query shadow rays for the 2–4 strongest lights per
  tile, from N4**, on Low and Medium, and N5's top-N per tile on High.

N2 keeps local lights unshadowed, as today. `preset.rs` moves that cell to `Planned(N4)`. This
is [Q1](#open-questions-for-the-maintainer) and a [departure](#departures-from-the-roadmap).

### 4. LTC area lights

**4a. Shapes.** Roadmap §6.2 asks for "LTC area lights for beams and capsules". Linearly
transformed cosines (Heitz, Dupuy, Hill and Neubelt, SIGGRAPH 2016 [S1]) integrate a GGX lobe
over a polygon in closed form; Heitz and Hill extended them to line and disk lights (SIGGRAPH
2017 course) [S3]. The engine's shapes:

| Shape | LTC form | Used for |
|---|---|---|
| `Point` | none (today's punctual light) | hit flashes, small effects, Low's fallback for every shape |
| `Sphere` | disk facing the shaded point [S3] | orbs, cores, sun-glass and ember-crystal glow |
| `Capsule` | line integral [S3] plus two sphere ends, ours (the composition is not from the papers) | beams, lasers, the hard-light bowstring and arrows, projectile trails |
| `Rect` | polygon, 4 vertices, with horizon clipping [S1] | emitter-lattice panels, structure light panels |

- **Diffuse** uses the same LTC integral with the identity transform (a clamped cosine), as the
  paper does [S1].
- **Per tier:** LTC from Medium up, as the §3 matrix says. **Low** evaluates every shape as a
  point light at the shape's closest point to the reflection ray (Karis 2013's representative
  point, literature), with the same total power, so a capsule light on Low is still a light,
  only with a smaller highlight.
- **Shapes do not change the caps.** An area light is one entry in the light list and counts
  against `MAX_LIGHTS` and the per-column caps, binned by the bounding sphere of its shape.

**4b. The LUTs.**
- The reference implementation fits a **64 × 64** table over roughness and view angle, indexed
  by `sqrt(1 − cos θ)`, not plain `cos θ`, and stores two RGBA tables: the inverse matrix
  normalised by one element (4 coefficients), and the magnitude, the average Fresnel and a
  horizon-clipping term for spheres [S2].
- **Provenance.** The fitting code (`selfshadow/ltc_code`) is under a BSD-3-Clause-style licence
  (copyright 2017, Heitz, Dupuy, Hill and Neubelt) that also asks for the paper to be cited
  [S2]. We port `fitLTC.cpp` to a `moba-tools ltc-bake` tool, keep the licence notice in
  `THIRD_PARTY.md` and the source header, cite the paper, and commit only the generated table
  (2 × 64 × 64 `RGBA16F`, 64 KiB) under `crates/render/assets/`. Q9 offers a clean-room fit as
  the alternative.
- **Half precision is a hypothesis.** `RGBA16F` is checked against the CPU reference (verification gate 3).
  If it fails, the table becomes `RGBA32F` (128 KiB).
- The tables are two bindless handles, uploaded at renderer creation.

**4c. The light record.** `PointLight` (32 bytes: position and radius, colour) becomes
`Light` (48 bytes): position and influence radius; colour and shape; axis and length (capsule)
or half-extents (rect), plus the source radius. `Scene::lights` stays the per-frame API. The
effect descriptor's optional `[light]` table (ADR-0016 §1) gains `shape`, `length`, `width` and
`source_radius`, parsed with the same bounded, range-checked parser. Effects are client-local,
so this is no content-hash change.

**4d. Sheen and LTC.** A sheen LTC exists (Zeltner, Burley and Chiang, SIGGRAPH 2022 [S4]), and
OpenPBR 1.1 adopted it for fuzz [S8]. Its published tables state **no licence** [S4], so N2
does not ship them; sheen uses the analytic Charlie sheen under point and area lights (area
lights evaluated at the representative point for the sheen lobe only). See Q7.

### 5. Forward transparents and their consistency with deferred

**5a. What stays forward on Tiered:** water (ADR-0017), the new transmissive glass and crystal
(§5c), particles (ADR-0013), distortion (ADR-0019), and, later, hair cards (N10). Everything
else is deferred.

**5b. One lighting code path.** Forward and deferred surfaces must not look different under the
same light. So:
- the BRDFs, the light loop, the shadow lookup, the fog and the AO rules live in shared Slang
  modules (`shaders/slang/lib/{brdf,lights,shadow,fog,material}.slang`), imported by both
  `deferred_light` and the forward Slang shaders;
- forward shaders read the same column grid, light list, caps and clamps (§2b), the same
  shadow map and the same exposure;
- **tiered water and glass are Slang.** Water is a tiered rewrite of `water.frag`
  (ADR-0020 §4a) that keeps its look: flow, depth fade, foam, refraction;
- **particles stay GLSL** (`vfx.frag`): they are emissive or lit by the key light and ambient
  only, as today, and that does not change in N2. Mesh particles keep the key light. This is a
  deliberate gap, not a bug: a particle is a short-lived light source, not a surface;
- **the parity test** (verification gate 2) renders the same `STANDARD` surface once in the G-buffer and
  once through the forward path under every light shape, and requires agreement within the
  G-buffer's quantisation.

**5c. Transmissive glass and crystal (Medium and up).** The bible's sun-glass, void-glass and
ember crystal want transmission from Medium, and an opaque base with an emissive core on Low
and Compat (art bible `material-vocabulary.md` §3).
- **A new `glass` forward pass** draws transmissive instances sorted back to front by instance
  (they are a few props per screen), following `KHR_materials_transmission` and
  `KHR_materials_volume` (both ratified [S9]) in a thin-walled or thick form:
  - refraction samples the opaque copy that water already takes (`water_copy`, shared);
  - the surface has full specular from every light, with the same Fresnel;
  - the absorption colour comes from the volume's attenuation colour and distance.
- **It writes depth.** Particles behind a crystal are then hidden instead of refracted. The
  alternative (no depth write) composites particles behind the crystal on top of it, which is
  a worse error for a solid body. See Q5.
- **It writes `tc_mask`** (ADR-0021 §2d), so the upscalers keep history but drop locks on
  refracted pixels.
- **On Low,** transmissive materials are drawn in the G-buffer with their opaque fallback (§10)
  and no `glass` pass exists.

**5d. WBOIT stays for particles.** Particles keep ADR-0013's weighted blended OIT on Medium and
up, unchanged: the accumulation and revealage targets, the `(ONE, ONE)` blending, the clamps
and `vfx_resolve`. Glass is **not** put into the OIT targets:
- WBOIT cannot refract and its weights are tuned for particle depth ranges [S11];
- glass is few, large and sortable, and particles are many and small.

The order on Tiered is `deferred_light → sky → water_copy → water → glass → vfx_draw →
vfx_resolve → distortion_copy → distortion`. Particles in front of glass composite over it;
particles behind it are hidden by its depth.

**5e. Reactive and motion.** Water and glass write `tc_mask`; particles write `reactive` and,
where dominant, `motion`, as ADR-0021 §2d specifies. N2 changes none of that.

### 6. The multi-layer material model

**6a. Scope: a bounded set of shading models, not open-ended layering** (roadmap §6.6).
- Parameters are named after glTF `KHR_materials_*` and OpenPBR, so assets map directly. The
  current OpenPBR release is **v1.1.1** (2026-04-17, Apache-2.0, Academy Software Foundation)
  [S8]. Its structure is a metal/dielectric base, a specular interface, then optional coat and
  fuzz, with thin film as a Fresnel effect [S8].
- The glTF extensions used are ratified: `clearcoat`, `sheen`, `specular`, `ior`,
  `iridescence`, `anisotropy`, `transmission`, `volume`, `emissive_strength` [S9].
  `KHR_materials_diffuse_transmission` is still a **release candidate** [S9]; the `FOLIAGE`
  model uses its parameter name but no file depends on it until it is ratified.
- **Vertical layering is one optional top layer:** a base (metal or dielectric) plus at most one
  of coat, sheen or thin film. **Horizontal mixing** (worn paint, oxide in recesses, dust,
  chipped scale) is per texel in the textures, as the bible's G4 rule and its "Layers" column
  describe (art bible `material-vocabulary.md` §1, §4). The bible's materials need nothing more
  (§6e).
- **Macro modifiers** are evaluated when the G-buffer is written and cost no G-buffer bits:
  - a world-space **dust** layer (amount × `saturate(n · up)`, map dust colour), for the
    bible's "dust layer on up-facing ledges";
  - N6's **wetness** (albedo darkening, roughness drop);
  - the **team tint**, when honoured (§8c).

**6b. The BRDF core** (both deferred and forward, §5b):
- GGX, height-correlated Smith, Schlick Fresnel (unchanged from ADR-0011 §1).
- **F0 for dielectrics** comes from `ior` and `specular`:
  `F0 = ((ior − 1)/(ior + 1))² × specular`, capped at 1, as `KHR_materials_specular` and
  `KHR_materials_ior` define [S9]. Metals use base colour. The default IOR is 1.5 (F0 0.04),
  the bible's default.
- **Multiscatter energy compensation** (Kulla and Conty 2017 [S5]; Fdez-Agüera 2019 [S6]):
  the single-scatter specular is scaled by `1 + F0 (1/E_ss − 1)`, as Filament does from its DFG
  table [S7]. **No new LUT is needed:** today's split-sum LUT holds `(A, B)` with `F0·A + B`
  (ADR-0011 §3), so `E_ss = A + B`, its value at `F0 = 1`. Kulla and Conty's own correction
  says the energy lost at roughness 1 is close to 60 % [S5], which is why it matters for the
  bible's rough metals (sun-alloy worn, 0.35–0.60).
- **Diffuse** stays Lambert × `(1 − F)(1 − metallic)`. OpenPBR's rough diffuse is not adopted;
  at 46 m it is below the camera's resolution (roadmap §2).
- **The stylisation layer leaves Tiered.** Wrap, ramp, painted specular and the map rim are not
  evaluated on Tiered from N2. Maps keep their keys for Compat until C1, which drops the ramps
  (roadmap §8). The map rim's readability role passes to the team rim (§8). See Q12.
- **Specular tint is a scalar.** `KHR_materials_specular` also has `specularColor` [S9]; N2
  stores only the scalar, since metals take their tint from base colour and the bible's
  dielectrics are untinted. See [Departures](#departures-from-the-roadmap).

**6c. Shading models and their G-buffer encoding.** `gb2.a`'s high nibble is the model; `gb2.rgb`
is the model's data. Every model also has base colour, metallic, normal and roughness.

| ID | Model | `gb2.r` | `gb2.g` | `gb2.b` | Lighting | glTF / OpenPBR source |
|---|---|---|---|---|---|---|
| 0 | `UNLIT` | — | — | — | none: emissive only, written to `hdr` by the G-buffer pass | `KHR_materials_unlit` |
| 1 | `STANDARD` | dielectric F0, `0.16 x²` | material AO | — | GGX + Lambert + multiscatter | core, `specular`, `ior` |
| 2 | `CLEARCOAT` | coat weight | coat roughness | material AO | base + one GGX coat lobe (IOR 1.5, base normal), base attenuated by the coat's Fresnel | `KHR_materials_clearcoat`; OpenPBR coat |
| 3 | `SHEEN` | sheen weight | sheen roughness | sheen tint (0 white … 1 base colour) | base + Charlie sheen, base scaled by the sheen albedo | `KHR_materials_sheen` (Charlie, Conty Estevez and Kulla 2017 [S9]); OpenPBR fuzz |
| 4 | `ANISOTROPIC` | anisotropy strength | tangent angle about the normal (in a basis built from the normal) | material AO | anisotropic GGX | `KHR_materials_anisotropy` |
| 5 | `THIN_FILM` | film thickness (100–400 nm range per material) | film IOR (1.0–2.0) | material AO | GGX with the thin-film Fresnel of Belcour and Barla 2017, as `KHR_materials_iridescence` uses [S9] | `KHR_materials_iridescence`; OpenPBR thin film |
| 6 | `FOLIAGE` | translucency | material AO | — | two-sided Lambert with back lighting through the leaf | `KHR_materials_diffuse_transmission` (release candidate) |
| 7 | `TRANSMISSIVE_FALLBACK` | dielectric F0 | material AO | — | as `STANDARD`; marks a transmissive material drawn opaque (Low), for debug views and the readability heatmap | `KHR_materials_transmission` |
| 8–15 | reserved | | | | N10: pre-integrated skin, eyes; N3/N6 as needed | |

- **Dielectric F0 uses `0.16 x²`.** It covers F0 0–0.16 in 8 bits, with the precision at the
  bible's range of 0.018–0.077 (IOR 1.31–1.77, `PBR-GLASS-1`).
- **Material AO** is the authored cavity occlusion. It joins GTAO and the baked AO by `min`
  (ADR-0016 §6) and is dropped by models that need all three channels.
- **The anisotropy angle** is measured in an orthonormal basis built from the decoded normal
  (Duff et al. 2017, literature), so no tangent is stored.
- **No combinations.** A material picks one model. The bible's Void silk asks for sheen *and*
  anisotropy; it gets `SHEEN` only, since anisotropy is below a pixel at 46 m (bible
  `silhouettes-and-scale.md`, "the camera sets the rules"). Showcase variants can combine them
  in N10 (Q6).

**6d. Material records and the Slang interface.**
- **The record.** A `MaterialRecord` (64 bytes, `#[repr(C)]`) lives in a GPU table addressed by
  BDA and indexed by the instance's material index. It holds u32 bindless handles (base colour,
  normal, `metallic_roughness`, emissive, model texture), the factors, the model ID, its
  parameters, the dust amount, `team_tint` and the alpha cutoff. It joins the Slang reflection
  layout test, as ADR-0020 Amendment 4 planned for the material record.
- **The emissive mask lives in `metallic_roughness.b`,** today unused (R is metal and G
  roughness, ADR-0011 §1). It scales the material's emissive colour on **both** paths, so
  molten seams and emitter grilles need no new sampler on Compat (§10). A full-colour emissive
  texture is optional and Tiered-only.
- **Slang modules** in `shaders/slang/lib/`:

```slang
// material.slang
struct SurfaceParams {
    float3 base_color; float metallic;
    float3 normal;     float roughness;
    uint   model;      float3 model_data;   // §6c, decoded to physical units
    float  ao;         uint flags; uint rim_slot;
};
GBufferTexels encode_gbuffer(SurfaceParams s, float3 emissive);  // G-buffer fragment stage
SurfaceParams decode_gbuffer(GBufferTexels t);                    // deferred_light

interface IShadingModel {
    static float3 direct(SurfaceParams s, LightSample l, float3 v);
    static float3 ambient(SurfaceParams s, AmbientInputs a, float3 v);
}
struct Standard   : IShadingModel { ... }
struct Clearcoat  : IShadingModel { ... }
// ...one struct per model in §6c

// deferred_light.slang: the standard permutation calls shade<Standard>(); the extended one
// switches on s.model to shade<Clearcoat>(), shade<Sheen>() ... (generic specialisation,
// so each case is compiled without dynamic dispatch).
```

  - The forward `water` and `glass` shaders call the same `shade<M>()` with their own
    `SurfaceParams`.
  - `encode_gbuffer`/`decode_gbuffer` have a Rust twin in `moba-render` (pure functions over
    bytes), unit-tested for round trips and checked against the shader's output on lavapipe.

**6e. The art bible's vocabulary mapped to the models.** From the v0 draft on PR #347
(`docs/art/material-vocabulary.md` §3), which is not approved yet. The numbers stay the
bible's; this table only says how each is rendered.

| Bible material | Model | Notes |
|---|---|---|
| Sun-alloy, polished / worn; cast sun-alloy, patinated; gilt leaf; gravity-ring alloy; tarnished silver; Builder alloy | `STANDARD`, metal | Oxide, patina and tarnish are per-texel dielectric (metalness 0) in the same texture set (bible G2, `PBR-OXIDE-1`). Builder alloy's dust is the dust modifier (§6a) |
| Forge dark alloy, heat-tinted | `THIN_FILM`, metal | Temper colours from film thickness; the bible's straw-to-purple limit (its open "heat tint" decision) is a content-check range on thickness, not a shader change |
| Forge dark alloy, scaled; fused scorch | `STANDARD`, dielectric | |
| Heat-shield ceramic, unglazed; Forge sandstone and basalt masonry; basalt; ash, soil, lane grit, sand; bone; dead and charred wood | `STANDARD`, dielectric | The bible's environment materials are all single-layer |
| Heat-shield ceramic, glazed; Void composite, glazed; Void lacquer | `CLEARCOAT` | Coat roughness 0.05–0.25 from the bible |
| Void composite, matte; obsidian (polished, knapped); polished black stone | `STANDARD`, dielectric | Obsidian's F0 0.043 from IOR 1.52 |
| Linen; Void felt | `SHEEN` | |
| Void silk | `SHEEN` | Anisotropy dropped in gameplay (§6c, Q6) |
| Sun-glass; void-glass; ember crystal | forward `glass` (Medium+); `TRANSMISSIVE_FALLBACK` on Low | Emissive cores and edges through the emissive mask (§6d) |
| Molten seam | emissive mask on its host material | Counts towards the bible's emissive budget (`PBR-EMIT-*`) |
| Emitter lattice | `STANDARD`, metal, with an emissive mask behind the grille | Lit only in gameplay states: the client sets the material's emissive factor |
| Hard light | `UNLIT` | A thin emissive mesh; no G-buffer lighting |
| Dry scrub (foliage) | `FOLIAGE` | |
| Basalt, wet | `STANDARD` + N6 wetness | |
| Skin | `STANDARD` placeholder in N2 | Pre-integrated skin is N10 (reserved model ID) |

**6f. The content format.** `materials/<model>.toml` gains, all optional:
- `model = "standard" | "clearcoat" | "sheen" | "anisotropic" | "thin_film" | "foliage" |
  "unlit" | "transmissive"`, with a `[model]` table of that model's parameters in glTF units;
- `ior`, `specular`, `dust`, `emissive_texture` (Tiered only);
- `[transmission]` (`transmission`, `thickness`, `attenuation_color`, `attenuation_distance`);
- `[compat]` overrides (§10).

Unknown keys still fail validation. The `mesh-bake`/glTF importer maps the listed `KHR_materials_*`
extensions to these keys. Materials are client-local and cosmetic, so **no content hash,
signature or wire change** (content-authoring.md). The format change ships with Compat's
fallback in the same PR (ADR-0020 §2d).

### 7. G-buffer decals

**7a. What changes from ADR-0017 §1.** Today `Paint` decals are inlined into the terrain, water
and foliage shaders. On Tiered from N2:
- **`Paint` decals are a raster pass between the two G-buffer groups** (§1e):

```text
gbuffer_receivers   terrain, foliage                 → gb0–gb2, depth, motion, hdr
gbuffer_decals      one box per decal (instanced)    → gb0.rgb, gb1.b (blend), depth read-only + sampled
gbuffer_rest        walls, props, structures, units  → gb0–gb2, depth, motion, hdr
```

- **Each decal draws its footprint box** (±4 m in height, as today's projection range) from the
  same decal list buffer. The fragment reconstructs the ground position from depth, which is
  attached read-only and sampled in the same pass, as ADR-0013 already does for soft
  particles. It then evaluates the shape with the shared decal code (Slang port of
  `decals.glsl`, with `glow_reference`'s CPU twin as the oracle).
- **It blends base colour (`gb0.rgb`) and roughness (`gb1.b`)** with
  `(SRC_ALPHA, ONE_MINUS_SRC_ALPHA)`; the other channels are masked off. Different write masks
  per attachment need `independentBlend`, which the tiered path requires from N1 (ADR-0021 §2e).
  Normals are not blended: octahedral values do not blend linearly, and scorch and splats read
  through value and roughness at 46 m.
- **"Units never take decals" holds by order, not by a mask.** Units, walls and props are drawn
  after the decals, so they overwrite any decal under them by the depth test (ADR-0017 §1's
  rule). Depth has no stencil (ADR-0016 alternatives), so the order is the cheapest receiver
  test. The cost is that terrain under walls and units is decal-shaded and then overdrawn; a
  unit covers little of the screen, and walls are measured (verification gate 7).
- **Binning.** The 32 × 32 ground grid is no longer read per fragment: each decal is drawn once.
  The CPU list, its order and the tier caps (Low 48, Medium 96, High and Ultra 128) stay.
- **Water** is forward on Tiered, so it keeps evaluating `Paint` decals inline through the
  shared module.
- **The terrain register problem goes away on Tiered.** The tiered terrain G-buffer shader has
  no decal code. `scripts/terrain-decal-check.sh` keeps guarding Compat's `terrain.frag`.

**7b. `Glow` decals** (telegraphs, zones, ranges) are in N1's overlay layer (ADR-0021 §5b), not
in the G-buffer, and N2 does not touch them.

**7c. Why not compute decals.** A compute pass could write `gb0`, but not `gb1`:
`A2B10G10R10_UNORM_PACK32` storage is not mandatory on the 1.3 floor [S12][S13].

### 8. Team rims in the lighting pass

**8a. The rim.** For every pixel with `UNIT` set and a non-zero rim slot:

```text
rim_mask  = pow(saturate(1 − N·V), p_class)            // Fresnel-shaped, per size class
rim_lit   = rim_mask × palette[slot].color × palette[slot].strength
rim_floor = rim_mask × palette[slot].color × palette[slot].floor / exposure_linear
hdr      += max(rim_lit, rim_floor)                     // after fog (§2c step 7)
```

- **Slots and palette.** `UnitInstance` gains `rim: u8` (0 none, 1–7), alongside `outline`.
  `Scene::rims` (`RimPalette`) holds per slot a linear colour, a strength and a display floor.
  The **client** fills it from the `team_palette` setting (standard or colour-blind, R7a) and
  assigns slots, as it does for outlines (ADR-0016 §2). The renderer knows nothing about teams.
- **Rule RIM-1 (width ≈ 1.5–2.5 px at output resolution).** The exponent `p_class` is set per
  unit size class from the bible's table (minions, heroes S/M/L) so that the rim's measured
  width at the default camera falls in that range at 1080p and 1440p. The width is a
  measurement (verification gate 4), not a promise of the Fresnel shape.
- **RIM-2 (survives darkness, fog and GI).** The rim is added **after** height fog and fog of
  war, and the `rim_floor` term divides by the exposure multiplier (ADR-0021 §4a's
  `exposure_linear`, previous frame under auto exposure), so a rim's displayed value never drops
  below its floor, whatever the scene's light or exposure. With the gameplay default of fixed
  exposure (roadmap §4 rule 5) there is no lag.
- **RIM-3 (enemy ≥ 1.25× ally).** The client's default palette gives the enemy slots 1.25× the
  ally strength and floor; a unit test on the client's palette builder holds it. The self slot
  uses the self colour.
- **RIM-4 and RIM-6** are content and capture checks (the bible's PAL-4/PAL-5 and CON-2), run by
  the readability tests (verification gate 4).
- **RIM-5 (outlines follow the same colours).** The rim palette and the outline palette are
  filled from the same `team_palette`, in one function.
- **The rim is lighting, so it is in the upscaler's input and history.** ADR-0021 §5a says so:
  "team rims stay in the lighting". At render scale 0.67 a 2-pixel output rim is about 1.3
  render pixels, so its temporal stability is tested (verification gate 4, ghost energy) and is a risk.

**8b. Hidden information.** The rim slot comes only from `Scene::units`, which holds only what
the server sent, and is written only where the unit's own G-buffer pixels pass the depth test.
A unit fully behind a wall writes no pixels and gets no rim. `scripts/outline-probe.sh` gains a
rim case: the occluded unit's frame is byte-identical with rims on and off.

**8c. Switches for the open art decisions.** The art bible's README lists them as "Decisions
needed" items 9 and 11 (PR #347, `docs/art/README.md`; item 9 is decision 4 in
`material-vocabulary.md` §8, and item 11 is in `silhouettes-and-scale.md`, "Team rims"). N2
builds both options and leaves the defaults to the maintainer.

| Bible decision | Switch | Values | Default until decided |
|---|---|---|---|
| **11. Rim vs outline on allies** | `team_layer_allies` (setting and `MOBA_TEAM_LAYER_ALLIES`) | `rim` (rim on allies, outline only on enemies and the hovered unit: the bible's option (a), recommended there); `rim+outline` (option (b)); `outline` (option (c)) | `rim+outline`: additive, so no ally loses today's outline before the decision |
| **9. `team_tint` on photoreal content** | `team_tint_policy` on Tiered (setting and `MOBA_TEAM_TINT`) | `honour` (today's mask tint); `zero` (the bible's option (a), recommended there: the tint is ignored on Tiered and the mask kept for Compat) | `honour`, so today's content keeps its team colour until the decision and the CON-2 result |

- Enemies keep both rim and outline in every setting; the switch is about allies only. Minions
  have no outline today (ADR-0016 §2); they get a rim in every setting.
- **When decision 9 is taken as (a),** ADR-0011 rule 1 ("team colour lives in albedo") is
  amended for Tiered in the same PR, as the bible's recommendation says, and the readability
  test (CON-2 with rims) must pass first.
- Both switches are client-local presentation; ranked and tournament locks (roadmap §4 rule 6)
  may fix them like any visibility setting.

### 9. Per-tier budgets

**9a. N2's share of ADR-0020 §5b.** Planning estimates on each tier's reference GPU at its
internal resolution, from roadmap §6.8's High example (G-buffer 1.2, deferred direct 0.8,
transparents 1.0 ms). They are to be replaced by measurements; estimates never close a target.

| Pass (ms) | Low (RTX 2060, 720p) | Medium (RTX 3060, 720p) | High (RTX 4070, 960p) | Ultra (RTX 5070 Ti, 960p / 1080p) | Overdrive |
|---|---|---|---|---|---|
| `gbuffer_*` incl. decals | 1.2 | 0.9 | 1.2 | 0.9 | as Ultra (the primary hit for N9) |
| `light_classify` | 0.1 | 0.1 | 0.1 | 0.1 | 0.1 |
| `deferred_light` | 0.8 | 0.9 (with LTC) | 0.8 | 0.6 | replaced by path tracing in N9; kept as the fallback |
| `water`, `glass` | 0.6 | 0.8 | 0.9 | 0.8 | 0.8 |
| **N2 total** | **2.7 of 12.0** | **2.7 of 6.0** | **3.0 of 7.5** | **2.4 of 9.0** | — |

- **Medium is the tight tier:** 2.7 of 6.0 ms leaves 3.3 ms for shadows, SSGI (≤ 0.8),
  SSR, the upscaler, post, the overlay and the UI. The budget-driven dynamic resolution
  (ADR-0020 §5c) is what absorbs a miss.
- The particle passes are R5's and are not counted here.

**9b. What can be measured now.** Only the RTX 5070 Ti is owned. It runs today's 1080p
teamfight replay in about 0.5–0.7 ms of GPU time (ADR-0020 Amendment 3), so absolute
reference-GPU numbers are not available. The binding gates are therefore relative, on the 5070
Ti (verification gate 7):
- the N2 tiered frame at each preset costs **no more than 1.10×** the N1 tiered forward frame
  on the teamfight replay (median of 3 interleaved rounds);
- each N2 pass stays under its column's share scaled by the 5070 Ti / reference-GPU ratio from
  the R8 perf logs, labelled as an estimate;
- the reference GPUs replace both when they arrive (#9/#127).

**9c. VRAM.** The G-buffer adds 11–25 MB (§1b), and Low's lit target grows by 3.7 MB. The light
records grow by 4 KiB per frame in flight, and the LTC tables add 64 KiB. All of this is inside
ADR-0020 §5e's tier VRAM budgets by a wide margin, and is reported in `docs/perf/`.

### 10. How Compat (C1) consumes the material spec

C1 (roadmap §8) brings Compat to "GGX with multiscatter compensation and the N2 material
parameters (dropping the stylised ramps)", then freezes it. N2's material spec is the contract C1
implements; N2 itself changes no Compat pixel.

- **One description, two evaluators.** `moba_assets::MaterialDesc` parses the extended TOML
  (§6f) once. Tiered evaluates it in the G-buffer and `deferred_light`; Compat evaluates it in
  its forward `shade()`.
- **What C1 renders:** `STANDARD` in full, including `ior`/`specular` F0 and multiscatter
  (from the same split-sum LUT, §6b). No new Compat sampler is needed: the emissive mask is in
  `metallic_roughness.b` (§6d), and Compat's scene layout is full (16 of 16, ADR-0016 §6).
- **Collapse rules for the other models** (each overridable by the material's `[compat]` table,
  as the bible's tier notes ask, for example "Compat: base only, roughness 0.3"):

| Model | Compat fallback |
|---|---|
| `CLEARCOAT` | base only; roughness `mix(base, coat, 0.5 × weight)` unless `[compat] roughness` is set |
| `SHEEN` | base only |
| `ANISOTROPIC` | isotropic GGX at the mean roughness |
| `THIN_FILM` | base colour tinted by the film's Fresnel at normal incidence, baked into albedo by `content-check` within the bible's G2 metal range |
| `FOLIAGE` | today's foliage shader |
| transmissive | opaque base plus the emissive mask (`TRANSMISSIVE_FALLBACK`) |
| `UNLIT` | emissive only |

- **Team tint:** Compat keeps the mask and the tint whatever decision 9 says (that is the
  bible's own option (a)).
- **Team rims on Compat** are not part of N2. Whether C1 adds the rim to Compat's forward
  `shade()` is C1's decision; the formula in §8a needs only N·V, the palette and the exposure,
  all of which Compat has.
- **`content-check`** validates each material against both evaluators: every model has a defined
  fallback, every `[compat]` override is in range, and the bible's per-class ranges hold for
  the collapsed values too.

### 11. The tiered frame from N2

```text
(portraits, minimap) → (fow) → vfx_simulate → (shadow)
→ gbuffer_receivers  render res, jittered, reversed-Z: terrain, foliage → gb0–gb2, depth, motion, hdr (emissive)
→ gbuffer_decals     Paint decals → gb0.rgb, gb1.b
→ gbuffer_rest       walls, props, structures, units (static, skinned, VAT), background motion
→ (contact_shadows, ao, ao_blur)
→ light_classify     → tile lists, indirect args
→ deferred_light     sun, lights (LTC from Medium), ambient, AO, fog, fog of war, team rims → hdr
→ sky
→ (water_copy → water) → (glass, Medium+) → vfx_draw → (vfx_resolve)
→ (distortion_copy → distortion) → tc_mask
→ temporal → bloom, exposure → composite → overlay_prepass → overlay → overlay_vfx → ui → (capture)
```

From `temporal` on, the frame is ADR-0021 §5a's, unchanged. The `scene` pass leaves Tiered.
- **Graph accesses.** Compute storage read/write, indirect read and read-only depth plus
  fragment sampling already exist (ADR-0013). The new usage is compute sampling of G-buffer
  images, which is the existing sampled access in the compute stage.
- **Graph limits.** N2 adds about six passes and six resources over N1's topology. ADR-0021
  already warned that `MAX_RESOURCES` (48) is near the `u64` mask; if the largest tiered
  topology needs more, the mask widens to `u128` in the same PR, and the topology test compiles
  every variant against the limits.
- **Off switches.** `MOBA_DEFERRED=off` runs N1's tiered forward frame instead (kept until the
  parity gate has held for one release, then removed); `MOBA_AREA_LIGHTS=off` (point fallback);
  `MOBA_GLASS=off` (opaque fallback); `MOBA_DECALS=off` (as today); `MOBA_TEAM_RIMS=off`.

### 12. Determinism and the invariant

- Rendering still never touches the simulation, the wire, replays or the state hash. No version
  or hash moves: `PROTOCOL_VERSION` v42, replay v5, `SIM_LOGIC_VERSION` v9, content hashes and
  `tests/golden-state-hash.txt` are unchanged.
- Rim slots, the rim palette, light shapes and material records are presentation state, built
  by the client from data it already has. Materials and effects are client-local and outside
  the content hash.
- Nothing reads back from the GPU into gameplay. Picking is unchanged (ADR-0021 §8).

## Departures from the roadmap

| Roadmap says | This ADR decides | Why |
|---|---|---|
| §3 matrix and `preset.rs`: Low's local lights "clustered, 2–4 shadowed" in N2 | Unshadowed in N2; shadowed by ray-query shadow rays from N4 (§3) | Every tier has ray query; a shadow-map atlas would be thrown away in N4/N5 (Q1) |
| §5.2 step 1: emissive is a G-buffer target | Emissive is written straight into the light-accumulation `hdr` (§1a) | Saves 4–8 B/px; unlit surfaces need no G-buffer slot |
| §6.6: "specular tint" in the material model | Scalar `specular` and `ior` only (§6b) | Metals take tint from base colour; the bible's dielectrics are untinted; saves G-buffer bits |
| §6.6: clear coat, sheen, anisotropy … as layers | One optional top layer per material; no sheen + anisotropy combination in gameplay (§6c) | Bounded models, as §6.6 itself asks; anisotropy is sub-pixel at 46 m (Q6) |
| §5.2 step 4: G-buffer decals | Decals blend albedo and roughness, never normals (§7a) | Octahedral normals do not blend; `gb1` cannot be written from compute on the 1.3 floor |
| §6.2: LTC "for beams and capsules" on Medium+ | Shapes `Sphere`, `Capsule`, `Rect`, with Low evaluating them as representative points (§4a) | The §3 matrix starts LTC at Medium; Low still needs the light |

## Risks

| Risk | Mitigation |
|---|---|
| The team rim is about 1.3 render pixels at scale 0.67, and may flicker or ghost under TAA, DLSS or FSR | The rim width and ghost-energy gates (verification gate 4); the outline stays on enemies in every setting; the per-class exponent is tuned on captures |
| Losing the stylisation layer (white map rim, wrap) lowers unit-to-ground contrast before the team rims land | Both change in the same PR, N2e (Q12); CON-1/CON-5 must hold against Low and against the N1 frame |
| 8-bit base colour bands on the dark Void bodies (luma 39–58) | sRGB encoding in the shader (§1a); the material sheet golden includes the Void materials |
| The extended lighting permutation's registers lower occupancy | Tile classification keeps standard tiles on the small permutation (§2a); pipeline statistics logged as for the terrain shader (ADR-0017) |
| Decal ordering costs overdraw where walls cover decal'd terrain | Measured in verification gate 7; walls could move to the receivers' depth-only pre-draw if it matters |
| GLSL vertex and Slang fragment interfaces drift | The reflection test of verification gate 1; the port in N7 (Q8) |
| The deferred and forward paths drift apart in look | Shared Slang modules and the parity gate (§5b, verification gate 2) |
| Reversed-Z breaks a depth consumer that was missed | One switch for every tiered consumer (§1c), and the tiered goldens; Compat untouched |
| The art decisions (9, 11) stay open past N2e | Both options are built and switchable (§8c); the defaults are additive |
| The material spec changes after C1 starts | The spec freezes at N2c (see the work estimate under Consequences); later changes follow the content-format rule (a Compat fallback in the same PR, ADR-0020 §2d) |
| Reference-GPU budgets are estimates | Relative gates on the 5070 Ti (§9b); estimates never close a target |
| Area lights look wrong on Low (representative point) | Same total power; Low passes the same readability checks; LTC from Medium as the §3 matrix says |

## Alternatives considered

- **A visibility buffer now** (roadmap §5.2 step 5). Doom: The Dark Ages moved from clustered
  Forward+ to a hybrid with a visibility buffer, compute material dispatch, deferred texturing,
  tile classification and deferred lighting [S17]. It pays when material count or overdraw is
  high; our material count is small and the camera's overdraw is low. The G-buffer here is what
  a later visibility-buffer resolve would write, so the step stays open. Not now.
- **Keep Forward+.** No guide buffers for NRD or DLSS RR, no primary-hit description for N9, and
  the scene layout's 16-sampler ceiling stays the limit on materials. Rejected (roadmap §5.2).
- **A fat G-buffer** (5–6 targets, a full emissive target, custom data per model). About twice
  the bandwidth for parameters the bounded models do not need. Rejected.
- **An adaptive multi-layer G-buffer like Unreal's Substrate.** Substrate became production-ready
  in UE 5.7, with adaptive and blendable G-buffer modes, while some sub-features (material
  layers, rough refraction) are still experimental or beta [S19]. That machinery is far beyond
  a bounded set of models at a 46 m camera. Rejected.
- **In-pass deferred with dynamic rendering local read.** Would need Vulkan 1.4 or the KHR
  extension, which Amendment 1 declined, and gives no tile classification. Rejected (§2e).
- **The vendor denoisers' packing as our `gb1`.** NRD's recommended `R10G10B10A2` packing is its
  own octahedral variant with roughness and a 2-bit material ID [S14], and DLSS RR recommends
  RGB16F normals with roughness packed in alpha [S15]. Copying NRD's packing would put code from
  an RTX-SDK-licensed header into the MIT tree (ADR-0020 §6b), and the two denoisers disagree
  anyway. N4/N9 repack from our G-buffer in one small pass behind their features.
- **Per-tile GPU light culling.** Useful with thousands of lights; with at most 256 lights in
  4.6 m ground columns the CPU grid is already tile-sized at the default camera (§2b).
  Deferred to N8 (ReSTIR DI).
- **A shadow-map atlas for 2–4 local lights in N2.** Rejected for ray-query shadows (§3, Q1).
- **Stencil-masked decal receivers.** Depth has no stencil, and `D32_SFLOAT_S8_UINT` support is
  not guaranteed. Draw order does the same job (§7a).
- **Compute decals.** `gb1` storage is not mandatory (§7c). Rejected.
- **Glass in the WBOIT targets.** WBOIT cannot refract, and its weights suit particles [S11].
  Rejected (§5d).
- **The LTC sheen tables.** No licence is stated [S4]. Charlie sheen instead (Q7).
- **A full OpenPBR evaluator.** OpenPBR 1.1.1 explicitly allows approximations on low-power
  targets [S8]; a full evaluator (diffuse roughness, subsurface, coat and fuzz together) is a
  showcase-camera feature. N10 may add parts of it for showcase.

## Consequences

- **Wire, replays and determinism: no change** (§12).
- **Frame shape.** The `scene` pass leaves Tiered; the tiered frame gains `gbuffer_receivers`,
  `gbuffer_decals`, `gbuffer_rest`, `light_classify`, `deferred_light` and `glass`. Compat's
  frame is unchanged.
- **Pipelines.** The opaque tiered pipeline count drops to about one per vertex format × alpha
  mode (§1e); `locked_pipelines=0` still holds, since every pipeline is created with the
  renderer.
- **Shaders.** New Slang: the G-buffer fragment stages, `light_classify`, `deferred_light`, the
  decal pass, tiered `water`, `glass`, and the shared modules in `shaders/slang/lib/`. GLSL
  remains for the vertex stages (until N7), particles and Compat.
- **Readability changes that can be seen on Tiered:** the stylisation layer (wrap, ramp,
  painted specular, the white map rim) is gone; team rims arrive; with `team_tint_policy=zero`,
  team colour leaves albedo.
- **Content.** The material format gains the §6f keys and the emissive mask channel;
  `content-check` gains model and Compat-fallback validation. Every shipped material stays
  valid, since all new keys are optional and default to `STANDARD`.
- **Public API** (additive, `#[non_exhaustive]`):
  - `Light` and `LightShape` (with `PointLight` kept as a constructor for one release);
  - `UnitInstance::rim`, `RimPalette`, `Scene::rims`;
  - `TeamLayerAllies`, `TeamTintPolicy`, and the matching `LightingConfig` builders;
  - `moba_assets::{MaterialDesc, ShadingModel, CompatOverride}`, `VfxLight::shape`.
- **Licences.** `THIRD_PARTY.md` gains the LTC fitting code's BSD-3-Clause-style notice and
  citation (§4b). No vendor SDK is involved in N2.
- **Docs.** `docs/content-authoring.md` (material keys, the emissive mask), `docs/status.md`,
  `docs/architecture.md`'s renderer section and `preset.rs`'s cells move with the code.
- **Work estimate.** The roadmap's **L**, in six PRs on the critical path:
  1. **N2a:** reversed-Z on Tiered; the G-buffer targets written by N1's prepass; the encode and
     decode twins; debug views. The forward `scene` still shades, so pixels change only by
     reversed-Z precision.
  2. **N2b:** `light_classify` and `deferred_light` for `STANDARD` (sun, ambient, AO, clustered
     point lights, fog, fog of war), today's map rim reproduced; forward water behind it; the
     parity gate; `scene` leaves Tiered.
  3. **N2c:** the material model: the other shading models, multiscatter, the material table and
     bindless materials, the content format with Compat fallbacks, `content-check` rules; the
     G-buffer decals. The material spec freezes here, which **unblocks C1**.
  4. **N2d:** LTC area lights, the light record and `[light] shape`.
  5. **N2e:** team rims, the decision 9 and 11 switches, the stylisation layer removed, the
     readability gates.
  6. **N2f:** the `glass` pass (Medium+).

  N2b alone closes the frame-architecture and perf-gate requirements; N2e closes roadmap §4
  rule 2.

## Verification and evidence gates

These follow roadmap §10 and the gates of ADR-0020 and ADR-0021, applied to N2.

1. **Encoding and interfaces.**
   - Rust twin tests: `encode_gbuffer`/`decode_gbuffer` round trips for every model, the
     octahedral normal's maximum angular error over a dense sphere (the bound is recorded by the
     test, not assumed), and the F0, roughness and AO quantisation.
   - A lavapipe frame writes a fixed set of surfaces; the read-back G-buffer matches the Rust
     twin byte for byte.
   - The reflection test covers `MaterialRecord`, `Light`, the frame constants and the
     GLSL-vertex / Slang-fragment interface locations.
2. **Deferred–forward parity.** With the stylisation layer off on both, `STANDARD` only and rims
   off, the N2b deferred frame matches N1's tiered forward frame on lavapipe within 2/255 on at
   least 99.5 % of pixels and 6/255 everywhere, for the default scene, the R3b river and the R3b
   teamfight goldens. The same surface through the forward path (§5b) and the deferred path
   agrees within the G-buffer quantisation under every light shape.
3. **BRDF and lights.**
   - **White furnace:** a rough metal with F0 = 1 under a uniform white environment returns
     1 ± 0.02 at every roughness with multiscatter on.
   - **LTC:** each shape against a fixed-seed Monte Carlo integral of the same GGX lobe (CPU
     reference) over a grid of roughness, angle and light size, with a recorded tolerance; the
     `RGBA16F` table passes it or becomes `RGBA32F` (§4b). A sphere light's result tends to the
     point light's as its radius tends to 0.
   - **Light-set consistency:** a CPU test that the forward and deferred light loops visit the
     same lights for the same world position under every cap.
4. **Readability (merge-gating).**
   - **CON-1, CON-2, CON-4 and CON-5** (art bible `palette-and-value.md`) on the standard
     captures at every tier, both team palettes, with rims on: hero against its surround
     |ΔL| ≥ 0.15 with rims and rim pixels C ≥ 0.10 within ±15° of the team hue; and no tier
     below Low − 0.02.
   - **RIM-1:** measured rim width 1.5–2.5 output px per size class at 1080p and 1440p.
   - **RIM-2:** the same captures in a tower's shadow, under height fog and in fog of war
     (an allied unit in vision inside a fogged area), with the rim's display value at or above
     its floor.
   - **RIM-3:** the enemy-to-ally ratio from the captures ≥ 1.25.
   - **Hidden information:** `outline-probe.sh` with rims: an occluded unit's frame is
     byte-identical with rims on and off; and a unit standing in a `Paint` decal has
     byte-identical unit pixels with decals on and off.
   - **Ghost energy** (ADR-0021 gate 3) re-run with rims on.
   - Both values of `team_layer_allies` and `team_tint_policy` are captured and pass.
5. **Goldens.**
   - Every Compat golden is byte-identical (`MOBA_RENDER_PATH=compat`).
   - New tiered goldens on lavapipe, at small resolution with ADR-0021's warm-up: Low and High
     deferred, an LTC scene (Medium), a material-model sheet (one sphere per model), a `Paint`
     decal scene and a glass scene; each byte-identical across two runs, with validation and
     sync validation clean.
6. **Debug views** ship with N2a and are shown in `docs/screenshots.md`.
7. **Performance.**
   - Per-pass lanes for every N2 pass in the frame log; medians of 3 interleaved rounds.
   - On the RTX 5070 Ti, the relative gate of §9b (≤ 1.10× the N1 tiered forward frame per
     preset on the teamfight replay).
   - Tiered budgets keep failing `perf-gate` (from N1, ADR-0021 §7).
   - The tiered pipeline count before and after, in the PR.
   - Decal overdraw: the teamfight decal scene's `gbuffer_decals` lane, and the wall-heavy
     overview's `gbuffer_receivers` against N1's prepass.
   - `locked_uploads=0` and `locked_pipelines=0`.
   - VRAM against §1b.
8. **Compat contract (for C1).** `content-check` passes every shipped material under both
   evaluators, and a test loads each shading model's sample material on Compat.

## Open questions for the maintainer

1. **Should Low's "2–4 shadowed" local lights wait for ray-query shadow rays in N4, instead of a
   shadow-map atlas in N2?** *Recommendation:* yes. Every tier has ray query, and an atlas would
   be replaced within two phases.
2. **Art bible decision 11 (rim vs outline on allies): which default until you decide?**
   *Recommendation:* `rim+outline` (additive, nothing lost); switch to the bible's recommended
   `rim` once you take decision 11 and CON-2 passes.
3. **Art bible decision 9 (`team_tint` on photoreal content): which default on Tiered until you
   decide?** *Recommendation:* `honour` until the rims pass CON-2; then take the bible's
   option (a), `zero` on Tiered, with ADR-0011 rule 1 amended in the same PR.
4. **Reversed-Z on Tiered in N2a, as ADR-0021 deferred it?** *Recommendation:* yes, in N2a,
   since N2 touches every depth consumer.
5. **Should transmissive glass write depth (particles behind it are hidden) or not (particles
   behind it draw on top)?** *Recommendation:* write depth; a crystal is a solid body.
6. **Void silk's sheen plus anisotropy: sheen only in gameplay?** *Recommendation:* yes;
   anisotropy at the showcase camera in N10.
7. **Sheen: analytic Charlie sheen, or LTC sheen (OpenPBR 1.1's fuzz), whose reference tables
   state no licence?** *Recommendation:* Charlie now; revisit if the tables get a licence or we
   fit our own.
8. **Keep N1's GLSL vertex stages under the new Slang G-buffer fragments until N7?**
   *Recommendation:* yes; N7 rewrites the vertex path for GPU-driven geometry anyway.
9. **LTC tables: port the BSD-licensed fitting code (with its notice and citation), or write a
   clean-room fit?** *Recommendation:* port it; the licence is permissive and the notice is one
   entry in `THIRD_PARTY.md`.
10. **Require `maxColorAttachments ≥ 8` for the tiered path?** *Recommendation:* yes; the floor
    GPUs report 8, and N3 may need a sixth target.
11. **Put the emissive mask in `metallic_roughness.b` on both paths?** *Recommendation:* yes; it
    gives Compat emissive detail without a sampler, and the change ships its Compat fallback.
12. **Drop the stylisation layer (wrap, ramp, painted specular, map rim) from Tiered in N2?**
    *Recommendation:* yes, in N2e together with the team rims, so the map rim's readability role
    is never left empty.

### Maintainer answers (2026-11-02)

The maintainer accepted this ADR with **every recommendation above taken**, Q1 to Q12:
- Low's shadowed local lights wait for ray-query shadow rays in N4; no shadow-map atlas in N2;
- allied highlight defaults to `rim+outline` until art bible decision 11 is taken and CON-2
  passes, then the bible's `rim`;
- `team_tint` on Tiered defaults to `honour` until the rims pass CON-2, then option (a),
  `zero` on Tiered, with ADR-0011 rule 1 amended in that PR;
- reversed-Z on Tiered lands in N2a;
- transmissive glass writes depth;
- Void silk is sheen only in gameplay, with anisotropy at the showcase camera in N10;
- analytic Charlie sheen now, LTC sheen only if its tables get a licence or we fit our own;
- N1's GLSL vertex stages stay under the Slang G-buffer fragments until N7;
- the BSD-licensed LTC fitting code is ported, with its notice in `THIRD_PARTY.md`;
- the tiered path requires `maxColorAttachments ≥ 8`;
- the emissive mask goes in `metallic_roughness.b` on both paths, with its Compat fallback;
- the stylisation layer leaves Tiered in N2e, together with the team rims.

### Amendment 1 (2026-11-03, N2a): reversed-Z and the G-buffer targets as built

Recorded in the N2a PR (`feat/n2a-gbuffer-reversed-z`), the first of the six PRs under
[Consequences](#consequences). The evidence is in [perf/0035](../perf/0035-n2a-gbuffer.md).
§1 and §11 hold, with these departures and clarifications:

1. **Reversed-Z is one `DepthConvention` per path** (`crates/render/src/depth.rs`): `Forward`
   on Compat, `Reversed` on Tiered. It gives the clear value (0), the compare op
   (`GREATER_OR_EQUAL`), the far value and the sky's vertex stage. The **reversal happens
   only at the GPU boundary**: the camera uniform, the VFX uniforms, the environment's inverse
   view-projection, the overlay block and the probes' CPU twins. Culling, the shadow fit, the
   light grid, `Temporal::begin_frame` and picking keep the forward matrices, so nothing on
   the CPU side changed. The projection's depth row is rebuilt as
   `z' = n·f/(f − n) − n/(f − n)·w`, which maps near to 1 and the finite far plane to 0 and
   commutes with the jitter (a unit test).
2. **Shadow maps stay forward** (cleared to 1, `LESS_OR_EQUAL`). They are orthographic, where
   reversed-Z gains nothing, and they are shared with Compat. The first draft cleared them to
   0 with the main view, which removed every shadow on Tiered, so the shadow clear is now
   separate from the main view's.
3. **Every tiered depth consumer switched together** (§1c): the prepass and the scene, the
   overlay prepass, the sky (`sky_reversed.vert`), water (its depth fade reconstructs through
   the environment's inverse view-projection, which is already reversed, so it needs no
   define; `MOBA_REVERSED_Z` on `water_tiered`/`water_refraction_tiered` matters only to the
   refraction branch's depth compare; the scene picks those modules from the depth
   convention, not from the overlay), soft particles and the distortion draw (the VFX
   pipelines' compare op and reconstruction), GTAO (`gtao_reversed.frag`), contact shadows
   (`contact_shadows_reversed.frag`), TAAU's nearest-depth cross and linearisation, and the
   motion probe's twin. Decals need no change: on both paths they are applied inside the
   terrain and foliage shaders, not drawn as depth-tested geometry. **Compat's SPIR-V is
   byte-identical**, module by module, to origin/main's: the shared shaders take the
   convention from macros (`include/depth.glsl`) whose Compat expansion is the original
   expression, and only the new `*_reversed` and tiered permutations differ.
4. **The upscalers are told, explicitly.** `UpscaleInputs` gains a `depth_convention` field
   (a change to ADR-0021 §4a's inputs), set from the renderer's main-view convention, and
   **every back end must read it**. TAAU's linearisation and FSR 3.1's `device_to_view_depth`
   are computed from it (the SDK's plane swap for an inverted depth). Both back ends' shaders
   are built for reversed-Z only (TAAU's nearest-depth cross takes the largest depth; FSR 3.1
   builds with `FFX_FSR3UPSCALER_OPTION_INVERTED_DEPTH=1`, and its reconstructed-depth target
   is cleared to the bits of 0.0, its `InterlockedMax` far value), so each declares that
   convention through `TemporalUpscaler::depth_convention` (a required method with no
   default, so a back end cannot compile without one) and `record` refuses another
   (`expect_depth_convention`, a panic: a renderer bug). Unit tests tie each back end's
   declared convention to its shader source, its far value and the refusal. The vendor back ends' inverted-depth flags belong to #365 (N1c);
   see item 15.
5. **The G-buffer fragment stages are GLSL, not new Slang shaders** (departs from §1e). They
   are `GBUFFER` permutations of today's forward `unit.frag`, `terrain.frag` and
   `foliage.frag` (11 new modules in all, with the `GBUFFER` vertex permutations of `unit`,
   `skinned` and `vat`), and they encode the forward shading's own surface. N2a's forward
   `scene` still shades, so the encoder has to see the same albedo, normal, roughness and
   occlusion that `shade()` sees. A Slang port of those inputs would be thrown away in N2c,
   when the material table and the bindless materials (§6d) replace them. The **reference
   encoding is Slang** (`slang/lib/gbuffer.slang`, `encode_gbuffer`/`decode_gbuffer`). Its
   GLSL twin (`include/gbuffer.glsl`) is what the raster stages call, and its Rust twin
   (`crates/render/src/gbuffer.rs`) is what the tests use. `gbuffer-smoke` checks all three
   (item 11). The GLSL-vertex/Slang-fragment interface test in gate 1 therefore moves to N2c,
   with the first Slang fragment stage. Materials still bind through today's per-draw sets, so
   **the tiered pipeline count rises** in N2a, 46 → 48 on the RTX replay (the textured unit
   and skinned G-buffer variants), instead of dropping; §1e's "about 10" is N2c's. The counts
   are in perf/0035, with the G-buffer's cost (about +0.045 ms in the `prepass` lane on the
   RTX 5070 Ti) and reversed-Z's (none measurable).
6. **No `hdr` attachment in the G-buffer pass** (departs from §1a and §1e). Emissive and unlit
   radiance still come from the forward `scene`, which writes `hdr`. The pass binds motion and
   `gb0`–`gb2` (attachments 0–3), plus the outline IDs (attachment 4) when screen-space
   outlines are on: at most five, as §1d planned. N2b adds `hdr` when the `scene` leaves Tiered.
7. **One group, not `gbuffer_receivers` then `gbuffer_rest`** (departs from §1e). The split
   exists so that `Paint` decals can land between the groups. Those decals arrive in N2c (§7),
   and the split arrives with them. Until then, the foliage and terrain G-buffer stages apply
   today's paint decals to their albedo, as the forward shading does.
8. **The G-buffer exists only with temporal AA.** The temporal prepass is the G-buffer pass.
   A tiered frame without temporal AA (a device or setting that turns it off) has no G-buffer,
   and the G-buffer debug views then log one line and draw the frame. Compat never has one.
9. **What each surface writes in N2a.** Units (static, skinned, VAT; textured or not): model
   `STANDARD`, F0 0.04 (IOR 1.5, §6b), material AO, flags `UNIT | NO_FOG_OF_WAR`. Terrain:
   `STANDARD`, F0 0.04, AO from the surface's occlusion, no flags. Foliage: `FOLIAGE`, with the
   forward back light's `0.35 · height` as its translucency. Every rim slot is 0 until N2e (§8).
   **`UNIT` is set on exactly what N1's unit mask covers**: everything drawn by the unit,
   skinned and VAT pipelines, which includes structures and props. The sky draws in the pass
   for its background motion only (its G-buffer writes are masked), so the far plane keeps
   the clear value 0, which decodes as `UNLIT`.
10. **Quantisation is explicit**: `floor(x · (2ᵇ − 1) + 0.5) / (2ᵇ − 1)` in the shader, so the
    stored bytes do not depend on the device's float-to-UNORM rounding. The octahedral map
    folds about +y in the (x, z) plane, because the map's normals are mostly +y. Measured by
    the twin tests over a dense sphere: maximum angular error 0.237°, mean 0.084° (bounds 0.25° and 0.088°).
    sRGB 8-bit relative step at most 4.6 %. The F0 0.04 default stores 128. The shader
    therefore hands the float-to-UNORM conversion a value within an ulp of an integer, never
    one halfway between two. Vulkan still lets that conversion round to either neighbouring
    integer, so a conformant device may store `k ± 1` where the scaled value lands an ulp off
    `k`. Lavapipe, CI's device, stores `k` for every value `gbuffer-smoke` draws, so the smoke's
    byte-for-byte comparison holds there; on another device a one-LSB difference is a
    conformant rounding, not an encoding bug.
11. **Gate 1's lavapipe check is `gbuffer-smoke`** (`crates/tools/src/bin/gbuffer-smoke.rs`, in
    the `tiered-temporal` job). It raster-encodes 4,096 surfaces over the eight models, every
    flag combination and rim slots 0–15 through the GLSL encoder. It reads the three targets
    back and compares them byte for byte with the Slang encoder (a compute dispatch) and the
    Rust twin, and checks the Slang decode against the Rust decode. A value within 0.01 of a
    step of a rounding boundary is redrawn, so the check does not depend on last-ulp float
    differences between compilers. Its push and bindings, and those of `gbuffer_debug.slang`,
    are pinned by reflection tests.
12. **Debug views** (§1f): `MOBA_DEBUG_VIEW=albedo|normal|roughness|metallic|model|rim-slot|
    flags` are new `TemporalDebugView` variants. `tiles`, `light-count` and `lit-only` come
    with N2b's passes. The view is a compute pass, `gbuffer_debug`, after `temporal`. It
    overwrites `hdr_out` with the decoded channel (nearest render pixel, divided by the
    exposure as TAAU's views are), and it is in the graph only while a G-buffer view is
    selected. The pass only writes `hdr_out`; the graph records that write as a new
    `StorageImageReadWrite` access. A plain write would make the upscaler's write of
    `hdr_out` dead, and the graph would cull the `temporal` pass. The read-write access
    keeps it live, so its history and timings stay the frame's.
13. **Admission:** `maxColorAttachments ≥ 8` joined the tiered limits (§1d), with a capability
    test.
14. **Graph:** the G-buffer adds three render-resolution transients and, with a debug view,
    one pass. Every frame variant still shares its transients and start states (unit tests).
15. **#365 (N1c) wires the vendor flags, not this PR.** #365 (head `783bc82`) is open and
    blocked on a maintainer decision, so N2a merges first. When #365 merges main it must set,
    from `UpscaleInputs::depth_convention` (reversed on every tiered frame):
    - **DLSS:** `NVSDK_NGX_DLSS_Feature_Flags_DepthInverted` in the feature-create flags;
    - **FSR 4:** `FFX_UPSCALE_ENABLE_DEPTH_INVERTED` in the context-creation flags;
    - **XeSS:** `XESS_INIT_FLAG_INVERTED_DEPTH` in the init flags.

    Each back end must also implement `TemporalUpscaler::depth_convention`, which is
    required and has no default, so a DLSS, FSR 4 or XeSS back end that has not decided its
    convention does not compile. It returns the convention its flag was created with, and
    `record` refuses other inputs through `expect_depth_convention`. Each back end's depth
    handling then gets the same test as TAAU's and FSR 3.1's (item 4). Without those flags,
    a vendor upscaler would read the far plane as the near one and reject the history
    everywhere.
16. **The G-buffer is stored only for a reader, and its content is checked on lavapipe.**
    - **Stored only when read.** Nothing reads `gb0`–`gb2` in N2a except a debug view and the
      motion probe's readback. Without one of those, the prepass stores them `DONT_CARE`, so
      a tile-based or compressing GPU need not write them out. The graph still records the
      pass's colour writes. N2b's lighting will need `STORE`.
    - **Write masks.** On lavapipe (Mesa 26.0.8 on CI and 26.2.5 locally), the prepass
      pipelines' per-attachment colour write masks were not honoured: attachment 0's mask
      (motion's R|G) reached every attachment. That dropped `gb0`–`gb2`'s blue and alpha (the
      roughness, the flags and the model), so on lavapipe every surface read as `UNLIT`. The
      RTX 5070 Ti honoured the masks. The trigger is inferred, not traced in Mesa: these
      pipelines differ only in their masks, while pipelines that also differ in blending (the
      VFX, `tc_mask`, the overlay) behave.
    - **The fix.** Motion's mask is now RGBA. On its two-channel `R16G16_SFLOAT` format that
      writes exactly R and G on any device. Every output a pipeline has an attachment for is
      now written, with a mask covering it, except one:
      - **Units:** the ID's R mask on the one-channel `R8_UINT` target writes the whole
        texel, so attachment 0's RGBA is the same mask there.
      - **Terrain:** writes ID 0 (no unit) explicitly, RGBA mask (`terrain_gbuffer_ids.frag`).
        The terrain draws first, onto the ID clear of 0, so this changes no pixel on any
        device.
      - **Sky:** writes zero (`UNLIT`, no flags) to `gb0`–`gb2` and, with IDs, ID 0, all RGBA
        masks (`sky_motion.frag`, `sky_motion_ids.frag`). It draws only where nothing else
        did, onto the same clear values, so the sky's G-buffer is written as zero, not left
        unwritten, with the same bits.
      - **Foliage (the one exception):** writes no ID and keeps an empty mask on the ID
        attachment, so a unit under the brush keeps its outline slot. On a conformant device
        that is defined: an empty mask leaves the attachment unchanged, whatever the shader
        did not write. On lavapipe the mask is not honoured, so attachment 0's RGBA mask
        applies and the unwritten output's value is undefined. Only the outline-ID pixels
        where foliage covers a unit can differ, and only on lavapipe (CI's software renderer,
        never a shipping device). Those pixels decide whether a unit's screen-space outline
        shows through the brush. Whether outlines *should* show under brush is a gameplay
        decision for the maintainer and stays open; this PR keeps the real-GPU behaviour.
    - **The check.** The motion probe (`scripts/motion-probe.sh`, CI `renderer-gate`) reads
      `gb0`, `gb1` and `gb2` back. It checks each draw class against item 9:
      - units, skinned and VAT are `STANDARD`, F0 0.04, `UNIT | NO_FOG_OF_WAR`, and `gb0`
        holds their instance colour within one step per channel (its blue is a channel the
        fault dropped);
      - terrain is `STANDARD`, F0 0.04, no flags;
      - foliage is `FOLIAGE`;
      - every class has metallic 0 and rim slot 0.

      A third frame at the horizon checks that the sky's pixels are zero in all three
      targets (`UNLIT`). This is gate 1's in-frame complement to `gbuffer-smoke`, which tests
      the encoder alone.

### Amendment 2 (2026-11-15, N2b): deferred lighting for `STANDARD` as built

Recorded in the N2b PR (`feat/n2b-deferred-lighting`), the second of the six PRs under
[Consequences](#consequences). The evidence is in [perf/0036](../perf/0036-n2b-deferred.md).
§2 and §11 hold for N2b's scope (`STANDARD`, sun, ambient, AO, clustered point lights, fog
and fog of war, today's map rim; the `scene` pass leaves Tiered), with these departures and
clarifications:

1. **`deferred_light` is GLSL and shades with the forward `shade()`** (departs from §2d and
   from "New Slang: … `deferred_light`" under Consequences). It includes the forward
   `include/lighting.glsl`, so the deferred pixel runs the same BRDF, sun shadow (PCF/PCSS by
   tier), contact shadows, column-grid light loop, SH and specular ambient, AO, specular
   occlusion and map stylisation as the forward fragment did, and the parity gate measures
   the G-buffer's quantisation and the depth reconstruction, not two lighting codes. The only
   change to the shared code is `MOBA_PIXEL_CENTER` (the pixel centre `shade()` reads where
   the fragment stages read `gl_FragCoord.xy`; their expansion is unchanged, so every forward
   and Compat module's SPIR-V is byte-identical). N2c ports the lighting to Slang with the
   material model, when the forward path stops being the reference. `light_classify` is
   Slang (`shaders/slang/light_classify.slang`).
2. **Plain descriptor sets, not the bindless table** (departs from §2d). `deferred_light`
   binds set 0 (the camera block, the existing layout), set 1 (its own: `gb0`–`gb2`, depth,
   `hdr` as an `rgba16f` storage image and the tile buffer) and set 3 (the scene's lighting
   set: shadow map, environment, BRDF LUT, light grid, AO, contact shadows, fog of war). The
   camera and lighting layouts gain the `COMPUTE` stage. The pass's sampler count is checked
   against the device by `descriptor_budget` before the frame is chosen (item 17). Both
   passes' sets are written once per transient binding (at creation and after a resize, with
   the device idle), not per frame. The bindless move comes with N2c's material table, which
   is what needs it.
3. **Tile buffer.** One `GpuOnly` storage buffer (`deferred-tiles`, imported into the graph as
   a persistent buffer), not two: a 12-word header (the sky, standard and extended
   `VkDispatchIndirectCommand`s, then the three classes' tile counts), then the standard and
   the extended lists of `capacity` packed tiles (`x | y << 16`). Sky tiles are classified but
   not appended (their header words keep the reset values), and §2a's per-tile depth range is
   not stored: nothing reads it until a pass needs it. `light_classify` ORs the tile's models
   with `WaveActiveBitOr` and one shared atomic per subgroup (item 17). `capacity` is the tile
   count at the swapchain extent, the render resolution's ceiling; the buffer is recreated
   on a swapchain resize. `TileLayout` in `crates/render/src/passes/deferred.rs` is the Rust
   twin, checked against both shaders by a source test. **The arguments are rows of
   `tiles_x` groups, not one long row:** a class can hold more tiles than
   `maxComputeWorkGroupCount[0]`'s guaranteed 65,535 (79,712 at 6016 × 3384 and native
   scale), so `light_classify` raises each class's `x` and `y` to `min(n, tiles_x)` and
   `ceil(n / tiles_x)` with atomic maxima as it appends, and `deferred_light` skips the
   last row's groups past the class's count (a CPU test covers the formula up to a
   32,768² image). The same check bounds every list read by `capacity`.
4. **The reset is a `vkCmdUpdateBuffer` of the 12 header words, in `mask_clear`** (departs
   from §2a's fill). `mask_clear` already opens the tiered frame with transfers, so the reset
   needs no new pass; the lists themselves need no clearing (each is read only up to its
   count).
5. **No scalarised loop in N2b** (defers §2b's wave-uniform permutation). The loop is
   `shade()`'s own, so a scalarised variant would fork the shared code that gate 2 depends
   on. It returns with the Slang port in N2c, measured against the plain loop.
6. **Emissive is loaded and replaced, not `hdr +=`** (§2c step 8), and only the units write
   or read it. The G-buffer pass writes `hdr` (attachment 4, or 5 with the outline IDs: §1a,
   closing amendment 1 item 6) from the units (their emissive) and the sky (item 7); the
   terrain and the foliage have an empty write mask there, since they emit nothing. The
   attachment is not loaded (`DONT_CARE`): every pixel is either the sky or lit. For a unit
   pixel (`GBUFFER_FLAG_UNIT`) `deferred_light` reads the emissive, shades, applies the fogs
   (in the forward order: emissive before fog, as `shade()` adds it) and stores the result;
   any other lit pixel is stored without a read. An `UNLIT` pixel off the far plane keeps
   what the G-buffer pass wrote (`deferred_light` returns early). No shipped surface but the
   sky is `UNLIT`; the early return is sound only while the units, which write `hdr`, are the
   only pipelines that may write `UNLIT` off the far plane. An `UNLIT` terrain or foliage
   pixel would keep an undefined `hdr`, so a new `UNLIT` surface must write `hdr` too (a
   comment at the return says so). Lavapipe ignores the empty mask (amendment 1 item 16) and stores the terrain's
   and foliage's zero; that is safe only because nothing reads those texels before the
   lighting overwrites them. Without the world's sky (`MOBA_SKY=off`) the sky's draw writes
   the main view's clear colour itself: `sky_motion`'s `SKY_RADIANCE` specialisation
   constant is false and constants 1–4 carry `RenderConfig::clear_color`, so `hdr` stays
   `DONT_CARE` either way. (The first build cleared `hdr` and kept the clear colour with a
   (`ZERO`, `ONE`) blend on the sky's attachment; lavapipe does not honour that blend and
   stored the sky's radiance, so the sky-off frame was 93.2 % within 2/255, max 140. The
   parity script now captures the default scene with `MOBA_SKY=off`: 100 % within 2, max 2.)
   As with amendment 1 item 16's masks, lavapipe lost a state set on one attachment only
   (the trigger is again not traced in Mesa). Within the G-buffer pass and the other
   `TargetsSpec` pipelines nothing else depends on lavapipe honouring per-attachment state
   (the VFX's per-attachment blends are honoured there, amendment 1 item 16). Every G-buffer
   pipeline now blends nothing and writes every output with a mask covering it, except the
   two empty `hdr` masks above (the terrain and the foliage, safe because unread) and the
   foliage's unit ID (amendment 1 item 16's documented exception, whose texel lavapipe
   leaves undefined). The other `TargetsSpec` pipelines (`tc_mask`'s `MAX`, the overlay
   prepass's IDs) have a single attachment.
7. **The sky is drawn once, by the G-buffer pass.** Its far-plane draw (§2a's "depth EQUAL
   far", through the sky's own vertex stage and compare) already wrote the background's
   motion and an `UNLIT` G-buffer; with `GBUFFER_HDR` it also writes the sky's radiance
   (`include/sky_radiance.glsl`, shared with the forward `sky.frag`) or, without the world's
   sky, the clear colour (item 6), and `deferred_light` skips the far plane. On Low/Medium, where water is the simple, non-refracting draw, a
   `simple_water` pass after the lighting draws it (the forward `scene` did); with refraction,
   `water_copy → water` follows as in N1 and there is no pass between. (The first build drew
   the sky a second time in a `sky` pass after the lighting, with an `hdr` round trip between
   the colour and the general layout; the review removed it.)
8. **`hdr` is `RGBA16F` whenever the frame is deferred**, since it is a storage image (§2d's
   mandatory format). Where N1 used the packed `B10G11R11` (`QualityTier::hdr_format` for a Low
   configuration tier), a deferred frame's `hdr` doubles in size. The gate 7 runs were
   already `RGBA16F` on both sides (their configuration tier is High), so perf/0036's
   numbers do not include that cost. Nor do they include two costs the RTX cannot show:
   `deferred_light` writing `hdr` in `GENERAL` as a storage image, which on some AMD and
   Intel drivers turns off the image's colour compression for the passes that read it
   (re-checked with #9/#127's devices), and the light loop under many lights (the teamfight
   replay peaks at 10 lights in view).
9. **What the G-buffer does not carry, and so leaves Tiered with the forward `scene`:**
   - **per-material style overrides** on textured units (the material's wrap, rim scale,
     ramp scale and specular tint): every unit pixel gets the map's wrap, ramp and specular
     tint and a rim of 1, the untextured units' values. The one shipped textured material
     (`content/materials/sentinel.toml`) sets none of these keys, so it has the defaults
     and no shipped frame changes for it; N2c's material table carries them;
   - **the stored F0**: the lighting takes F0 from `default_surface` (0.04, which is what
     every N2a surface stores);
   - **the shadow normal offset uses the shading normal**: the G-buffer has no geometric
     normal. Foliage offsets along +y, as `foliage.frag` does; for terrain and units the
     shading normal's offset moves the lookup by at most the normal-map deviation times the
     offset (a few centimetres);
   - **the units' roughness below 0.08**: the G-buffer floors a unit's stored roughness at
     0.08 (`gbuffer.rs`, `include/gbuffer.glsl`, amendment 1), while the forward `shade()`
     floors it at 0.045. No shipped unit material is below 0.08, so no frame changes; N2c's
     material model revisits the floor.
10. **`FOLIAGE` is lit in the extended permutation** (`deferred_light_extended.comp`): the
    `STANDARD` lighting plus `foliage.frag`'s translucent back light from the stored
    translucency. The other models do not exist until N2c and shade as `STANDARD` there.
11. **Team rims are not in N2b** (§2c step 7). They are N2e's (§8); the `rim_slot` channel
    stays 0. The map rim is the forward one, through `shade()`.
12. **`MOBA_STYLISATION=off`** (client): the map's wrap, rim and ramp go to 0 and its painted
    specular to white at scale 1, on both paths. It exists for gate 2 ("stylisation off on
    both") and for N2e's comparison; the default is on.
13. **Six fragment outputs, checked per frame, not at admission.** With screen-space outlines
    the G-buffer pass writes motion, `gb0`–`gb2`, the IDs and `hdr`. Admission keeps
    `maxFragmentOutputAttachments` and `maxFragmentCombinedOutputResources` at 5 (the forward
    frame's need); a Tiered device with fewer than 6 lights forward (item 17), so
    `MOBA_DEFERRED=off` never costs a device its Tiered path. Every recorded device reports 8
    and more. **Graph limit.** The largest deferred topology (with a G-buffer debug view) has
    39 passes; `MAX_PASSES` is now 48, and the topology test asserts the largest variant's
    count. `MAX_RESOURCES` stays 48 (N2b adds one resource, the tile buffer).
14. **Debug views.** `MOBA_DEBUG_VIEW=tiles` (standard tiles green, extended orange),
    `light-count` (the pixel's column light count, a heat ramp over 0–16) and `lit-only`
    (white albedo, no emissive) are drawn by `deferred_light` itself, so they need no pass.
    The view is the pipelines' specialisation constant `DEBUG_VIEW` (fixed at start-up, like
    `MOBA_DEBUG_VIEW`), so the shipped pipelines carry no debug branches. Their colours are
    scene-linear, before exposure: the pass runs before the frame's exposure is measured, and
    the composite's exposure scales them with the frame.
15. **Off switch.** `MOBA_DEFERRED=off` (and `RenderConfig::with_deferred(false)`) runs N1's
    forward frame (§11). The tiered start-up log prints `[render] lighting: deferred` or
    `forward`, which the parity script checks. The forward `scene` pipelines are still built
    on a deferred frame (the simple water draws from them); the tiered pipeline count is in
    perf/0036.
16. **Gate 2 is met, after a TAAU fix ([ADR-0021 amendment 7](0021-temporal-upscaling-overlay.md#amendment-7-2026-11-15-n2b-the-history-clip-on-a-degenerate-box)).**
    `scripts/deferred-parity.sh` (CI `tiered-temporal`) runs the gate as written: High (the
    default tier, Ultra configuration), TAAU Quality mode (render scale 0.667, sharpness
    0.25), 1280×720, stylisation and foliage off; within 2/255 on at least 99.5 % of pixels
    and 6/255 everywhere. The script also captures the default scene without the world's sky
    (`MOBA_SKY=off`, item 6). Each cell gives the share within 2/255, the worst pixel and the
    number of pixels past 2/255 (of 921,600).

    | Scene | Local (Mesa 26.2.5) | CI (Mesa 26.0.8, LLVM 21.1.8; AMD EPYC 9V74 and Intel Xeon 6975P-C alike) |
    |---|---|---|
    | Default | 100.000 %, max 2, 0 pixels past 2 | 100.000 %, max 4, 2 pixels past 2 |
    | Default, `MOBA_SKY=off` | 100.000 %, max 2, 0 pixels past 2 | — (added after these CI runs) |
    | R3b river | 100.000 %, max 3, 3 pixels past 2 | 100.000 %, max 3, 2 pixels past 2 |
    | R3b teamfight | 99.999 %, max 5, 9 pixels past 2 | 99.999 %, max 5, 7 pixels past 2 |

    **The first build failed the river** (locally max 15/255 at 7 pixels; on CI max 9 at 11
    pixels, and the teamfight 7 at one pixel), all at high-contrast edges of the river's
    bank. The review found the cause in TAAU, not in the deferred frame:
    - **On the river, before the upscaler, the two frames agree within 2/255 everywhere.**
      With TAAU patched to output its nearest raw sample (no history, native scale,
      sharpness 0), the river is 100 % within 2, max 2, and every earlier outlier is within
      2 over its 7 × 7 neighbourhood. The lighting is exact up to the G-buffer's
      quantisation, which is the case gate 2's second sentence allows. The teamfight is not:
      through the same raw passthrough it is max 8/255, at a cast shadow's edge
      ((744–747, 350–351)) and at a terrain crack ((1180, 665)); their cause was not
      isolated, and through TAAU both are within the gate.
    - **TAAU's variance clip amplified sub-step differences.** It pulled the history toward
      the box centre along its whole offset, scaled by the most-violated axis
      (`h = centre + offset / largest`), with the box's extent floored at only 1e-5. On
      near-uniform chroma the box is a few 1e-5 wide, so a chroma change far below one
      8-bit step rescaled the luma offset too and moved the pixel by up to 15/255. This was
      also the earlier "the frame is unstable" control: scaling the forward terrain albedo by
      1.001 moved a pixel by 13–14/255 through TAAU, and by 2 through the raw passthrough.
      Sharpening was not the cause (native, sharpness 0: still 7), nor the water
      (`MOBA_WATER=off`: a pixel at 8, on dry sand).
    - **The fix** (ADR-0021 amendment 7) keeps the clip on every box whose half-extent is
      at least 0.004 and blends it into a per-channel clamp where the box is narrower. The
      clamp alone met
      gate 2 (2, 2 and 4) but failed ghost energy (particle smear 8.20 % at Performance,
      bound 7.5 %); a floor on the clip's extent of 0.004 left the teamfight at 5/255, and
      one of 0.01 ghosted the hero. The blend measures as the 0.004 floor does on gate 2,
      fails safe if its constant is widened, and passes every ADR-0021 probe. It moves the forward tiered frame too (TAAU runs on
      both), so the tiered hashes were retaken from CI.

    The teamfight's worst pixel (5/255, at (516, 475)) is on a unit's silhouette edge, just
    inside its outline. The review isolated it: through the raw passthrough the 9 × 9 block
    around it is within 1/255, and the pixel is unchanged with sharpness 0 and with either
    RCAS `hit_min` floor (ADR-0021 amendment 7 item 6), so it is the clip's residual gain
    on a box that is not degenerate (wider than 0.004 at the silhouette), which amendment 7
    deliberately keeps. Native scale was not an option: on the first build, at frame 90, it
    measured 7.
17. **A Tiered device that cannot run the deferred passes lights forward** (one log line,
    `[render] deferred lighting unavailable (…); lighting forward`): fewer than 6 fragment
    outputs (item 13), a compute-stage descriptor budget below the pass's sets (item 2),
    `maxComputeWorkGroupInvocations` below 256 (the 16 × 16 tiles; the core minimum is 128),
    or no subgroup arithmetic in compute shaders (`light_classify`'s `WaveActiveBitOr`).
    TAAU's 16 × 16 workgroups likewise fall back to FXAA below 256 invocations. These are
    runtime checks rather than admission limits because the recorded device fixtures
    (`vulkan-probe --json`) do not record them, so an admission test could not hold them;
    they can join `REQUIRED_LIMITS` when the fixtures are re-recorded on #9/#127's devices.

### Amendment 3 (2026-11-17, N2c): the material model as built, and the frozen material spec

N2c builds §6 (the shading models past `STANDARD`, multiscatter, the material table and the
content format with its Compat fallbacks), §7 (`Paint` decals in the G-buffer), §10's
`content-check` rules and gate 1's GLSL-vertex / Slang-fragment interface test. It **freezes the
material spec** (below, and `docs/content-authoring.md`, Materials and textures), which unblocks
C1. Every departure from §1–§12 is an item here.

**The frozen material spec** (C1's contract, §10). A change to any of this from now on is an
amendment that names C1's consequence.

- **Descriptor:** `<root>/materials/<model>.toml`, at most 4 KiB, unknown keys rejected, every
  number range-checked. The ADR-0010/0011 keys are unchanged. The material-model keys, all
  optional and for `shader = "textured"` only:

  | Key | Range (default) | Meaning |
  |---|---|---|
  | `model` | one of §6c's eight names, `thin_film` and `transmissive` spelled so (`standard`) | the shading model |
  | `ior` | 1..=2.5 (1.5) | `standard` and `transmissive` only: dielectric F0 = `((ior − 1)/(ior + 1))² × specular` on Tiered, above 0.16 an error; every other model's base is F0 0.04 on every path, and a non-default `ior` or `specular` on it is an error (item 24) |
  | `dust` | 0..=1 (0) | world-space dust on up-facing surfaces, Tiered only |
  | `emissive_texture` | a texture name | full-colour emissive, sRGB `bc7-srgb`/`rgba8-srgb`, times `emissive`, Tiered only |
  | `model_texture` | a texture name | linear `bc7`/`rgba8`; R, G and B scale the model's data channels per texel, the AO being G on `standard`, `transmissive` and `foliage` and B on `clearcoat` and `anisotropic` (`sheen` has no AO channel); thin film: R picks the thickness between min and max, G scales the film's IOR, B is the AO (item 27); Tiered only |
  | `[clearcoat]` | `weight` 0..=1 (1), `roughness` 0..=1 (0) | `KHR_materials_clearcoat` |
  | `[sheen]` | `weight` 0..=1 (1), `roughness` 0..=1 (0.5), `tint` 0..=1 (0 white, 1 base colour) | `KHR_materials_sheen` |
  | `[anisotropic]` | `strength` 0..=1 (0.5), `rotation` −π..=π rad (0) | `KHR_materials_anisotropy`: the mesh's tangent turned by `rotation` about the normal (item 26) |
  | `[thin_film]` | `thickness_min`, `thickness_max` 100..=400 nm (100, 400; max ≥ min), `ior` 1..=2 (1.3) | `KHR_materials_iridescence` |
  | `[foliage]` | `translucency` 0..=1 (0.35) | `KHR_materials_diffuse_transmission` |
  | `[transmission]` | `transmission` 0..=1 (1), `thickness` 0..=10 m (0), `attenuation_color` 0..=1 (white), `attenuation_distance` 0.001..=1000 m (infinite) | `model = "transmissive"` only; parsed now, drawn by N2f |
  | `[compat]` | `base_color_factor`, `metallic`, `roughness` 0..=1; `emissive` 0..=16 | overrides of the Compat fallback |

  A model table that is not the material's model is an error, as is any model key on a
  `vertex-color` material.
- **The emissive mask** is `metallic_roughness.b`, on both paths, when the bound map has a third
  channel (`bc7` or `rgba8`, `moba_assets::has_emissive_mask`); a `bc5`/`rg8` map keeps the
  full emissive. `metallic_roughness` therefore accepts `bc7` and `rgba8` as well as `bc5`/`rg8`.
- **The Compat fallback** is `MaterialDesc::compat_fallback()`, computed at load, then the
  `[compat]` overrides:

  | Model | Compat (and the forward Tiered frame) draws |
  |---|---|
  | `standard`, `sheen`, `foliage`, `transmissive` | the base as it is (`transmissive`: opaque, with the emissive mask) |
  | `clearcoat` | roughness `mix(base, coat, 0.5 × weight)` |
  | `anisotropic` | isotropic GGX at `sqrt((α + α_t) / 2)`, `α_t = mix(α, 1, strength²)` |
  | `thin_film` | base colour × `1 + (t − 1) × metallic`, where `t` is the film's Fresnel at normal incidence (at `thickness_max`) over the base's F0, per channel, clamped to 0..=1: only a metal's base colour is its F0 |
  | `unlit` | base colour 0, metallic 0, painted specular and rim 0, the emissive |

- **`content-check` rules** (both evaluators, gate 8): the collapsed values are finite and in
  range; an `unlit` material has a non-zero `emissive` (Tiered's) and a non-zero Compat
  emissive, and no `model_texture`; `ior` and `specular` keep their defaults (1.5, 1) on every
  model but `standard` and `transmissive`; an
  `emissive_texture` with a non-zero `emissive` on a lit model has the mask or a
  `[compat] emissive`. Every descriptor under `materials/` and one level of subdirectories is
  checked, named by a unit or not (`moba_assets::check_material_dir`). The report counts the
  units' materials per model: text `models     standard 1`, JSON `"material_models"`.
- **The record and the table** (Tiered only): `MaterialRecord`, 64 bytes, as
  `shaders/slang/lib/material.slang`; 64 records (`MAX_MATERIALS`), host-written at load. The
  model word's flags (bits 28–31) are the emissive mask (1), a bound emissive texture (2) and a
  bound model texture (4).

**Items.**

1. **`model_texture` is added** (§6d lists a model-texture handle that §6f did not name).
2. **The emissive mask is read on Compat now**, not from C1 (§10): `unit_textured.frag`'s
   SPIR-V changes. No Compat pixel moves for content whose `metallic_roughness` has two
   channels, which is all shipped content but the N2c samples (gate 5, measured below).
3. **The Compat collapse is computed at load** by `compat_fallback()` and fills the UBO of
   every textured material; `STANDARD`'s is its own factors (a unit test). Departures from §10's
   table: `THIN_FILM`'s tint is computed there, not baked into the albedo by `content-check`;
   `FOLIAGE` on a unit material is the base under the unit shader (the map's foliage keeps
   `foliage.frag`); `UNLIT` is a black dielectric plus its emissive, so Compat still adds the
   0.04 specular of a black base: Compat's `shade()` has no unlit branch, and adding one would
   move Compat SPIR-V for every textured unit. C1 adds it.
4. **`specular` is reused.** It is ADR-0011's painted-specular scale on Compat and
   `KHR_materials_specular`'s factor in Tiered's F0 (§6b): one key, as §6f lists it. Content
   that sets it for the painted highlight also scales its Tiered F0. Only `standard` and
   `transmissive` may set it (item 24).
5. **`deferred_light` stays GLSL** (§6d's `deferred_light.slang` with `IShadingModel` generics
   is not built, as amendment 2 item 1 began). The models past `STANDARD` are
   `include/shading_models.glsl`, compiled only into `deferred_light_extended.comp`
   (`MOBA_SHADING_MODELS`), with a `switch` on the model; `shade()` calls `model_direct` per
   light and `model_ambient` once, so a `STANDARD` pixel in an extended tile shades as in a
   standard one.
6. **The first Slang fragment stage is the textured units' G-buffer stage**
   (`slang/unit_material_gbuffer.slang`, four permutations: plain, `PREPASS_IDS`,
   `GBUFFER_HDR`, both), behind the GLSL `unit_textured_gbuffer.vert` and
   `skinned_textured_gbuffer.vert`. Vertex-colour units, terrain, walls, props, foliage and the
   forward Tiered frame stay GLSL. The interface test
   (`glsl_vertex_and_slang_fragment_interfaces_match`) reflects both stages' SPIR-V: every
   location the fragment reads is written by the vertex stage with the same type, integer
   inputs are `Flat`, and the fragment reads at least locations 0, 1, 3, 4, 14 and 15 (13 in the
   `_ids` permutations). Slang strips unread varyings, so the test does not require equal sets.
7. **Bindless materials.** The material table is read by device address; the draw's material
   index is a push constant (`MaterialPush`, 32 bytes: the vertex stage's 16-byte skin words,
   the table address, the index, a reserved word), **per mesh, not per instance** (§6d): a
   material belongs to a model today (item 20's gap). Textures are `BindlessTable` handles in
   set 2 of the stage's layout (samplers at binding 0, textures at binding 2). The material
   sampler is bindless sampler slot 12 (`MATERIAL_SAMPLER`, after the 12-entry palette): the
   Compat `Materials` sampler (repeat, trilinear, the tier's anisotropy). The stage takes the
   slot and the table's length as specialisation constants 0 and 1, which the renderer sets
   from `bindless::MATERIAL_SAMPLER` and `MAX_MATERIALS` (a test checks both are decorated), and
   clamps the draw's index to the table. Empty slots point at the default textures (white, flat,
   metallic 0 / roughness 1, full mask, white emissive and model data); the stage skips the
   emissive and model samples when the record's flags say those slots are the defaults. Only
   the textured units read the table.
8. **The record has no alpha cutoff and no style words.** No shipped material is a cutout
   (reserved for when one is); per-material wrap, rim and ramp stay off Tiered (§6b), which
   corrects amendment 2 item 9's "N2c's material table carries them".
9. **Multiscatter and the stored F0** (`MOBA_TIERED_SHADING`) are in every Tiered lighting
   module, forward and deferred; Compat's expansion is unchanged (its SPIR-V is byte-identical
   but for item 2).
10. **Tiered gets its own split-sum LUT, with its texels on the end points** (§6b said no new
    LUT). With the 128-sample LUT and a fetch clamped at the last row's centre, the `E_ss` read
    near roughness 1 was far enough from the lobe's integral that the white furnace read 0.93.
    Tiered's table (`brdf_lut_tiered`) integrates the same `(A, B)` with 1,024 samples per
    texel and puts texel `i` at `i / (size − 1)` on both axes, so `brdf_lut_sample` reads N·V
    and roughness 0..=1 with one bilinear fetch, `uv = (x · (size − 1) + 0.5) / size`, and
    nothing is clamped or extrapolated. Every Tiered lit stage is built with
    `MOBA_TIERED_SHADING` (the offscreen views share the main view's pipelines), so none reads
    this table with Compat's fetch. The table is integrated once per process, on scoped threads,
    and kept for a renderer rebuilt on a settings change: 5–8 ms of start-up on the 9800X3D's 16
    threads, 43–77 ms on one. Compat keeps its 128-sample table with centred texels and its
    fetch. **C1 must adopt both** when it brings multiscatter to Compat, or its furnace will
    fail the same way. Gate 3's furnace is a CPU model of the shader's fetch (item 28).
11. **`FOLIAGE` and `TRANSMISSIVE_FALLBACK` are lit in the standard tiles** (reverses amendment 2
    item 10). The foliage's back light needs no model code beyond `STANDARD`'s, and the map's
    foliage covers much of the frame: in the extended class it pushed gate 7 to 1.108× at Low.
    The extended class is now only `CLEARCOAT`, `SHEEN`, `ANISOTROPIC` and `THIN_FILM`.
12. **The roughness floors stay.** The lighting's floor (`MOBA_MIN_ROUGHNESS`, 0.045) applies
    to every lobe, the coat's included. The G-buffer's floor on a unit's stored base roughness
    (0.08, amendment 2 item 9) is kept: amendment 2 left it for N2c to revisit, and moving it
    would move the encoding that `gbuffer-smoke` and C1's spec rely on. Below 0.08 a unit's
    base lobe is therefore rougher on the deferred frame than on the forward one. The coat,
    sheen and film keep their own roughness in the model data.
13. **The dust colour is a constant** (ash, `(0.30, 0.28, 0.25)`) until a map has a dust colour.
14. **`UNLIT` is unfogged** on both Tiered frames: the deferred pass leaves its `hdr` (amendment 2
    item 6) and the forward Tiered frame writes the emissive alone, before height fog.
15. **No scalarised light loop yet** (amendment 2 item 5 stands).
16. **G-buffer decals as built** (departs from §7a):
    - **GLSL** (`decal_gbuffer.vert`, `decal_gbuffer.frag`), sharing `decals.glsl` with the
      forward path, not a Slang port.
    - **The receivers are the terrain only.** Foliage is drawn after the decals, in the rest, as
      the forward frame draws grass over the terrain's inlined decals. The passes are
      `prepass` (receivers; the lane keeps N2b's name), `gbuffer_decals` and `gbuffer_rest`.
    - **Depth is attached read-only and sampled** (§7a): `DEPTH_STENCIL_READ_ONLY_OPTIMAL`,
      loaded and not stored, and bound in the same layout as the texture the pass
      reconstructs the ground from.
    - **Base colour only**, blended `(SRC_ALPHA, ONE_MINUS_SRC_ALPHA)` into `gb0`'s sRGB view,
      so the blend is linear as in the forward `apply_paint_decals`; the write mask is RGB, so
      `gb0.a` (metallic) is kept. `gb0` gains
      `MUTABLE_FORMAT` with a two-format list (UNORM, sRGB). `gb1.b` (roughness) is not
      blended: the forward path changes only albedo, and gate 2 compares with it. So
      `independentBlend` is not needed for decals.
    - **The 32 × 32 grid is still read per fragment.** A pixel is shaded only when its forward
      grid cell lists the decal, so a decal clips where the forward terrain clips it. Without
      it, splat droplets outside their cell drew in deferred only (teamfight parity max 28/255).
      The grid bins a decal by its radius, so droplets past it are clipped at cell edges on both
      paths: a known R3b limit, kept so the two frames agree.
    - **Back faces only, behind the ground.** Front faces are culled and the back faces are
      tested `LESS_OR_EQUAL` against the reversed-Z depth, so a box face in front of the
      ground rasterises nothing, the sky fails the test, and each covered pixel blends once,
      with the camera inside a box too.
    - **`terminateInvocation`, not `discard`**, for pixels outside the decal's grid cell or
      shape: glslc maps `discard` to `OpDemoteToHelperInvocation` under SPIR-V 1.6, which
      validation rejects without the demote feature. The device enables
      `shaderTerminateInvocation` on Tiered (Vulkan 1.3 requires it, so admission is
      unchanged).
    - `Glow` decals in the same buffer (N1's overlay, §7b) are collapsed by the vertex stage,
      every vertex outside the clip volume, so they rasterise nothing.
    - **The split is only for frames with a `Paint` decal.** The frame graph has a `paint`
      axis: without a live `Paint` decal, `gbuffer_decals` and `gbuffer_rest` are reserved and
      `prepass` draws everything, as N2b's one pass did (twice the plans, the same resources).
    - `MOBA_DECALS=off` (§11) keeps one G-buffer pass, as in N2b, and no sRGB view on `gb0`.
17. **The graph's largest tiered topology is 41 resources** (of `MAX_RESOURCES` 48; no `u128`
    mask). The frame log has the lanes `prepass`, `gbuffer_decals` and `gbuffer_rest` (38
    lanes in all).
18. **Samples and the sheet.** `content/materials/samples/` holds one material per model
    (textures `material-sample-*`, baked by `texture-bake generate material-sample`; the MR and
    model maps are `rgba8`, so the mask is exercised). `MOBA_MATERIAL_SHEET=<set>` draws one
    sphere per descriptor in `materials/<set>/` (client-only instances, at most 16). The tiered
    goldens gain `materials` (the sheet) and `decals` (the teamfight replay).
19. **Not built in N2c** (still in §6f's scope): the `mesh-bake`/glTF importer's mapping of
    `KHR_materials_*` to these keys. One material per model (item 7) leaves nowhere to put a
    glTF file's per-primitive materials, so the mapping waits for multi-material models
    (item 20).
20. **Art-slice gaps** (art-source `PIPELINE.md` §6 and `export/structures/HOOKS.md`):
    - **Closed:** the emissive slot (`emissive_texture`, and the mask in `metallic_roughness.b`;
      an asset whose MR is `bc5` must bake it as `bc7`/`rgba8` to carry the mask, or use
      `emissive_texture` with a `[compat] emissive`); the dust part of weathering (`dust`).
    - **Open, with owners:**
      - **Weathering masks** on a second UV (the structures' unique masks): need `TEXCOORD_1`
        (ADR-0007's vertex format) and a material key; a later ADR-0023 amendment with the
        asset track.
      - **`TEXCOORD_1`:** ADR-0007 (the loader and the vertex layout).
      - **One material per model** (`<asset>-glass`, `-emit`, `-crystal` slots): the loader's
        per-primitive split plus item 19; an asset-track (ADR-0007) change, needed before N2f
        can draw a model's glass slot.
      - **Props cannot take a material:** the map-visuals descriptor (ADR-0014); the material
        table can index them once it has a per-instance index.
      - **VAT has no faction key:** ADR-0012.
      - **Terrain import and wall tiling** (walls stretch `wall_mesh` along a segment):
        ADR-0014.
      - **FX hooks:** ADR-0013.
      - **N6's wetness** (§6a): N6.

21. **`content-check` does not check the bible's per-class ranges** (§6e: coat roughness
    0.05–0.25, the glass F0 range, the rough metals' roughness, the thin film's straw-to-purple
    limit on thickness) and does not bake `THIN_FILM`'s tint into the albedo (§10's G2 bake;
    item 3 computes it at load instead). Its rules are the frozen spec's checks above. The
    ranges wait for the bible's accepted numbers (PR #347); C1 or the asset track adds them.
22. **The forward Tiered frame draws every model but `STANDARD` and `UNLIT` with its Compat
    fallback** (`MOBA_DEFERRED=off`, and a device that lights forward under amendment 2 item
    17): `unit_textured_tiered.frag` reads the parameter block, which is the fallback, and
    draws `UNLIT` as its emissive alone, as the deferred frame does. §5b's one shared shade,
    and so gate 2's parity, hold for `STANDARD` (and `UNLIT`) only. The material sheet is not a parity
    scene, since its other seven spheres differ by design; it is a golden (`materials`).
23. **Gate 7's wall-heavy overview is not measured as such** (§9b: `gbuffer_receivers`
    against N1's prepass). The shipped maps have four walls each, so no view is wall-heavy. A
    low orbit across the whole map (`MOBA_PREVIEW_CAMERA=-0.7,0.12,170`: terrain, towers and
    structures, little wall) costs what N2b's frame does (0.991× N1's; N2b 0.996×): with the `paint`
    variant (item 16) it runs the single G-buffer pass except on the minority of frames with a
    live `Paint` decal, so the receivers' pass cannot be read from it. On the teamfight, which
    always splits, the receivers' `prepass` (terrain alone) is 0.045 ms at Low and 0.052 ms at
    High, against N1's whole prepass of 0.020 and 0.024 ms, which writes no G-buffer. Walls
    stay in `gbuffer_rest`; their overdraw of decal-shaded terrain waits for a map with walls
    in view.
24. **`ior` and `specular` shape the F0 of `standard` and `transmissive` only** (§6b gave every
    model the stored F0). Only those two models store an F0 in the G-buffer; every other
    model's channel holds its own data, so the deferred frame lit their base at 0.04 while the
    forward Tiered frame and the thin film's Compat tint used the authored F0. Now all three
    use `MaterialDesc::model_dielectric_f0()`: the authored F0 for `standard` and
    `transmissive`, IOR 1.5's 0.04 for the rest, and `content-check` refuses a non-default
    `ior` or `specular` on the rest, so the spec says what is drawn. A model that needs its own
    F0 later is an amendment that names C1's consequence.
25. **The models' light-invariant terms are computed once per pixel** (§6c's per-model BRDFs
    are unchanged otherwise). `model_prepare` (`include/shading_models.glsl`), called by
    `shade()` after the energy compensation, builds the anisotropic frame, the sheen colour
    and its albedo scaling and the thin film's Fresnel; `model_direct` and the ambient read
    them. **The thin film's Fresnel is evaluated once, at N·V** (the
    `KHR_materials_iridescence` form evaluates it per light at V·H): each light's specular is
    scaled by `film(N·V) / Schlick(F0, N·V)` and the ambient takes `film(N·V)` as its F0. The
    film's colour therefore does not shift across a highlight, an approximation that is
    largest where V·H and N·V differ most (wide highlights, grazing views). The Compat
    collapse (normal incidence) is unchanged.
26. **The anisotropy's direction is the mesh's tangent turned by `rotation`**
    (`KHR_materials_anisotropy`; the first build took the angle in the stored normal's
    orthonormal basis, which flips at `n.z = 0`, so a direction authored on a sphere jumped
    there). The G-buffer still stores an angle modulo π in that basis, now of the tangent's
    direction: the Slang stage quantises the normal as the G-buffer will, rebuilds the basis
    the lighting uses and measures the turned tangent in it. The basis still flips at `n.z = 0`,
    and the stored angle flips with it, so the lit direction is as continuous as the mesh's
    tangents (`anisotropy_direction_is_continuous_across_the_basis_seam`). A mesh without a
    usable tangent falls back to the rotation in the basis.
27. **The thin film's `model_texture` B is the material AO**, as the clearcoat's and the
    anisotropic model's B is (the first build left it unread, and the film's AO at 1).
28. **Gate 3 stays a CPU model until N2e.** The furnace models the fetch and the compensation
    with the painted-specular scale at 1 and no specular occlusion; a rendered furnace, with
    the painted specular and the specular occlusion N2e's stylisation brings, is N2e's. **N2e's
    gate 4 needs a `Paint`-only switch**, `MOBA_PAINT_DECALS=off` (not built in N2c):
    `MOBA_DECALS=off` also removes the overlay's `Glow` decals, so it cannot isolate the
    `Paint` pass.
29. **Review hardening** (no behaviour change for valid content): a textured skinned pipeline
    that fails to build, or a Tiered prepass without the material layout, is an error, not a
    silent fallback; `gb0`'s sRGB view is asserted present when decals are on, and the UNORM
    format's sRGB twin is a checked table (`srgb_twin`); a test checks that the material
    G-buffer layout fits the Tiered admission minimums.
30. **Timing evidence names what it measured.** The frame log reports `paint_decals_avg` (live
    `Paint` decals per frame) and, with `MOBA_TILE_COUNTS=1`, `tiles_standard_avg`,
    `tiles_extended_avg` and `extended_frames` (`light_classify`'s counts, copied to a
    host-readable buffer in `mask_clear` and read a frame later). The copy's barriers cost
    0.004–0.005 ms on the RTX 5070 Ti, so it is off by default and in gate 7's timings.

**Gates** ([perf/0037](../perf/0037-n2c-materials.md)):

- **1. Encoding and interfaces:** the record and push block against the Slang reflection, and
  the interface test (item 6). The Rust twin (`every_model_round_trips`,
  `stored_texels_are_fixed_points`) and `gbuffer-smoke` cover all eight models. Met.
- **2. Parity** (`STANDARD` and `UNLIT` only, item 22): `deferred-parity.sh` at 1280×720 on lavapipe
  gives the worst pixels 3 (default), 3 (sky off), 3 (river) and 6/255 (teamfight), each
  within 2/255 on at least 99.999 % of pixels (re-run on the review's head: 1, 1, 3 and 7
  pixels past 2/255). Met. Without item 16's grid test the teamfight read 28/255.
- **3. White furnace:** worst 0.9861 over roughness 0–1 and N·V 0.05–1, a CPU model of the
  shader's fetch (item 10). Met; a rendered furnace is not measured.
- **4. Hidden information (decals):** with bloom off, all 38,813 unit pixels of the teamfight
  keep their G-buffer albedo bytes with decals on and off, while 35,010 other pixels change. `MOBA_DECALS=off` also removes the
  overlay's glow decals, so the `Paint` pass is not isolated. Partial; N2e runs the full gate
  with rims.
- **5. Goldens:**
  - Every Compat golden scene, and both sentinel heroes, are byte-identical to `ceca947`.
  - The `materials` and `decals` tiered goldens are two-run identical with TAAU and FSR 3.1.
  - Validation and sync validation are clean on lavapipe and on the RTX 5070 Ti.
- **7. Performance** on the RTX 5070 Ti against `ab42718`, the teamfight replay, the review's
  head: GPU total 1.036–1.073×, GPU span 1.053–1.086× (Low, High, Ultra, Overdrive, Low and
  High with FSR 3.1), 1.010–1.020× N2b's; Compat Ultra 0.999×. **`gpu_total` is the gate
  metric** (perf/0036's gate column); `gpu_span`, which adds the gaps between passes, is
  advisory. Met on the replay the gate is defined on, which has no extended-model pixel. The
  overview: item 23.
  - **Headroom** against 1.10× N1 (`gpu_total`, ms): Low 0.0138, High 0.0209, Ultra 0.0209,
    Overdrive 0.0322, Low with FSR 3.1 0.0276, High with FSR 3.1 0.0367 (span: 0.0074 at
    Low, 0.0146 at High).
  - **Extended-model pixels reach the gate:** the material sheet, with 6.9 % and 14.1 % of
    its tiles extended, is 1.102× and 1.117× N1's frame without the spheres at High (N1 cannot
    draw them; an estimate of N1 drawing them puts the close view at about 1.10×). The
    teamfight's split G-buffer costs 0.007–0.011 ms over N2b's single pass.
  - **Budget for N2d–N2f** (measured as now: the teamfight, plus the sheet's close view as the
    extended-model check): N2d (LTC area lights and the light record) at most 0.005 ms at Low
    and 0.008 ms at High; N2e (team rims, with the stylisation layer removed) at most 0.005 ms
    at Low and 0.008 ms at High, net of what the removal saves; N2f (the `glass` pass,
    Medium+) nothing at Low and at most 0.005 ms at High. That leaves about 0.004 ms at Low for
    noise, and nothing for heroes with extended models, so **N2d opens with a recovery step**
    before its own work, in this order: the wave-uniform (scalarised) light loop amendment 2
    item 5 deferred (`deferred_light` is 0.053 ms at Low); folding the `Paint` decals back
    into the receivers' G-buffer stage from the decal grid, as N2b's terrain did (a departure
    from §7a's decal pass, so an amendment), which removes the split's second pass boundary;
    and `light_classify` (0.009 ms) folded into the deferred dispatch. A phase that cannot
    meet its budget after that is an amendment that names the preset and the overrun.
- **8. Compat contract:** `content-check` passes every shipped material under both evaluators,
  and `every_sample_material_loads_on_compat` builds each model's sample on Compat. Met.

## Corrections to the roadmap

These were re-checked on 2026-11-02. This PR only adds a link to this ADR from roadmap §12;
the maintainer may fold the corrections in when this ADR is accepted.

- **§5.2, "the path Doom: The Dark Ages took from clustered Forward+".** id's GDC 2026 talk
  describes a move from pure clustered Forward+ to a **hybrid** with a visibility buffer, compute
  material dispatch, deferred texturing, tile classification and deferred lighting, plus
  variable-rate compute shaders [S17]. That is closer to the roadmap's optional step 5 than to a
  thin G-buffer alone. Its G-buffer contents are **unverified** (no public source).
- **§6.6, OpenPBR.** The current version is **v1.1.1** (2026-04-17), Apache-2.0, hosted by the
  Academy Software Foundation; 1.1 adopted the LTC sheen for fuzz, and 1.1.1 allows
  approximations on low-power targets [S8].
- **§6.6, glTF `KHR_materials_*`.** All the extensions the roadmap lists are ratified, but
  `KHR_materials_diffuse_transmission` (needed for foliage translucency) is still a release
  candidate [S9].
- **§6.2, LTC.** LTC covers polygons (2016) and lines and disks (2017) [S1][S3]. "Capsules" are
  our composition of a line and two spheres, not a published LTC form. The reference tables are
  indexed by `sqrt(1 − cos θ)` [S2].
- **§9.6 and the bible, physicallybased.info.** Confirmed CC0 1.0 for its data, with the shader
  ball renders under CC BY 4.0 [S20].
- **Confirmed unchanged:** Kulla–Conty multiscatter compensation and Filament's DFG-table form
  [S5][S7]; WBOIT [S11]; NRD v4.18.0 is the latest release, with the guide inputs N4 needs
  (motion, normal and roughness, view Z) [S14]; DLSS SDK v310.9.16 and RR's guide buffers
  (diffuse and specular albedo, normals, roughness, depth, motion, specular hit distance or
  specular motion) [S15].

## Sources

All accessed 2026-11-02.

- [S1] E. Heitz, J. Dupuy, S. Hill, D. Neubelt, "Real-time polygonal-light shading with linearly
  transformed cosines", ACM TOG 35(4), 2016, DOI 10.1145/2897824.2925895:
  <https://api.crossref.org/works/10.1145/2897824.2925895>,
  <https://eheitzresearch.wordpress.com/415-2/>,
  <https://blog.selfshadow.com/publications/s2016-advances/>
- [S2] LTC reference code (fitting, tables, `LICENSE`):
  <https://github.com/selfshadow/ltc_code>,
  <https://raw.githubusercontent.com/selfshadow/ltc_code/master/fit/fitLTC.cpp>,
  <https://raw.githubusercontent.com/selfshadow/ltc_code/master/LICENSE>
- [S3] E. Heitz, S. Hill, "Real-Time Line- and Disk-Light Shading with Linearly Transformed
  Cosines", SIGGRAPH 2017 course "Physically Based Shading in Theory and Practice":
  <https://blog.selfshadow.com/publications/s2017-shading-course/>,
  <https://blog.selfshadow.com/publications/s2017-shading-course/heitz/s2017_pbs_ltc_lines_disks.pdf>.
  The GPU Zen chapter on linear lights by the same authors is cited by secondary sources only;
  its exact title is **unverified** and it is not relied on.
- [S4] T. Zeltner, B. Burley, M. J.-Y. Chiang, "Practical Multiple-Scattering Sheen Using
  Linearly Transformed Cosines", SIGGRAPH 2022 Talks, DOI 10.1145/3532836.3536240:
  <https://api.crossref.org/works/10.1145/3532836.3536240>; tables (32 × 32, no licence stated):
  <https://raw.githubusercontent.com/tizian/ltc-sheen/master/README.md>
- [S5] C. Kulla, A. Conty, "Revisiting Physically Based Shading at Imageworks", SIGGRAPH 2017
  PBS course (energy compensation; erratum "close to 60 %"):
  <https://blog.selfshadow.com/publications/s2017-shading-course/imageworks/s2017_pbs_imageworks_slides_v2.pdf>
- [S6] C. J. Fdez-Agüera, "A Multiple-Scattering Microfacet Model for Real-Time Image Based
  Lighting", JCGT 8(1), 2019: <https://jcgt.org/published/0008/01/03/>
- [S7] Filament, "Physically Based Rendering in Filament", §4.7.2 energy compensation from the DFG
  table: <https://google.github.io/filament/main/filament.html>
- [S8] OpenPBR Surface v1.1.1 (2026-04-17), Academy Software Foundation, Apache-2.0:
  <https://academysoftwarefoundation.github.io/OpenPBR/>,
  <https://github.com/AcademySoftwareFoundation/OpenPBR/releases>,
  <https://raw.githubusercontent.com/AcademySoftwareFoundation/OpenPBR/main/LICENSE>
- [S9] glTF extension registry and the Khronos material extensions:
  <https://raw.githubusercontent.com/KhronosGroup/glTF/main/extensions/README.md>,
  <https://raw.githubusercontent.com/KhronosGroup/glTF/main/extensions/2.0/Khronos/KHR_materials_sheen/README.md>
  (Charlie sheen, Conty Estevez and Kulla 2017),
  <https://raw.githubusercontent.com/KhronosGroup/glTF/main/extensions/2.0/Khronos/KHR_materials_iridescence/README.md>
  (Belcour and Barla 2017),
  <https://raw.githubusercontent.com/KhronosGroup/glTF/main/extensions/2.0/Khronos/KHR_materials_specular/README.md>
- [S10] Z. H. Cigolle, S. Donow, D. Evangelakos, M. Mara, M. McGuire, Q. Meyer, "A Survey of
  Efficient Representations for Independent Unit Vectors", JCGT 3(2), 2014:
  <https://jcgt.org/published/0003/02/01/>
- [S11] M. McGuire, L. Bavoil, "Weighted Blended Order-Independent Transparency", JCGT 2(2), 2013:
  <https://jcgt.org/published/0002/02/09/>
- [S12] Vulkan specification, "Required Format Support" (mandatory sampled, colour-attachment,
  blend and storage features; `‡` storage formats conditional on
  `shaderStorageImageExtendedFormats`):
  <https://docs.vulkan.org/spec/latest/chapters/formats.html#features-required-format-support>
- [S13] Vulkan specification, limits (`maxColorAttachments` minimum 4, Roadmap 2022 7, Roadmap
  2024 8; subgroup minimum `BASIC` in compute, Roadmap 2022 raises it) and features
  (`independentBlend` and `shaderStorageImageExtendedFormats` optional before 1.4):
  <https://docs.vulkan.org/spec/latest/chapters/limits.html>,
  <https://docs.vulkan.org/spec/latest/chapters/features.html>
- [S14] NVIDIA NRD v4.18.0 (2026-10-30): inputs and normal/roughness encodings:
  <https://raw.githubusercontent.com/NVIDIA-RTX/NRD/master/README.md>,
  <https://raw.githubusercontent.com/NVIDIA-RTX/NRD/master/Include/NRDDescs.h>,
  <https://github.com/NVIDIA-RTX/NRD/releases>. The SDK's licence is ADR-0020 [S19].
- [S15] NVIDIA DLSS SDK v310.9.16 (2026-10-16), "DLSS-RR Integration Guide" (August 2026):
  <https://github.com/NVIDIA/DLSS/blob/main/doc/DLSS-RR%20Integration%20Guide.pdf>,
  <https://github.com/NVIDIA/DLSS/releases>. The SDK's licence is ADR-0020 [S16].
- [S16] GPU memory figures from the vendors:
  - RX 6600: 8 GB GDDR6, 128-bit, up to 224 GB/s, 32 MB Infinity Cache:
    <https://www.amd.com/en/products/graphics/desktops/radeon/6000-series/amd-radeon-rx-6600.html>
  - RTX 3060 12 GB: 192-bit GDDR6 at 15 Gbps, 360 GB/s:
    <https://www.nvidia.com/en-us/geforce/news/game-on-you-asked-we-answered-qa/>
  - RTX 5070 Ti: 16 GB GDDR7, 256-bit, 896 GB/s:
    <https://www.nvidia.com/en-us/geforce/graphics-cards/compare/>
  - RTX 4070: 12 GB GDDR6 or GDDR6X, 192-bit; NVIDIA publishes no bandwidth, so 504 GB/s
    (GDDR6X at 21 Gbps) is a calculation and **unverified**:
    <https://www.nvidia.com/en-us/geforce/graphics-cards/40-series/rtx-4070-family/>
  - RTX 2060: 6 GB GDDR6 at 14 Gbps; the 192-bit bus and 336 GB/s are **unverified** on NVIDIA's
    pages: <https://www.nvidia.com/en-us/geforce/news/nvidia-geforce-rtx-2060/>
- [S17] id Software and Doom: The Dark Ages:
  - P. Hammer, D. Lazarek, "Rip & Tear: Breaking Down the Rendering of 'DOOM: The Dark Ages'",
    GDC 2026 (abstract): <https://gdcvault.com/play/1035744/Rip-Tear-Breaking-Down-the>
  - Microsoft Game Dev, "Variable Rate Compute Shaders in DOOM: The Dark Ages" (2026-04-09):
    <https://developer.microsoft.com/en-us/games/articles/2026/04/variable-rate-compute-shaders-doom-the-dark-ages/>
  - T. Sousa, "Fast as Hell: idTech 8 Global Illumination", SIGGRAPH 2025 Advances:
    <https://advances.realtimerendering.com/s2025/index.html>
- [S18] O. Olsson, M. Billeter, U. Assarsson, "Clustered Deferred and Forward Shading", HPG 2012,
  DOI 10.2312/EGGH/HPG12/087-096: <https://doi.org/10.2312/EGGH/HPG12/087-096>
- [S19] Unreal Engine 5.7 release (Substrate production-ready) and the Substrate overview:
  <https://forums.unrealengine.com/t/unreal-engine-5-7-released/2673913>,
  <https://dev.epicgames.com/documentation/unreal-engine/overview-of-substrate-materials-in-unreal-engine>
- [S20] physicallybased.info (data CC0 1.0): <https://physicallybased.info/>,
  <https://github.com/AntonPalmqvist/physically-based-api>
- **Literature (not vendor claims, not re-checked):** B. Karis, "Real Shading in Unreal Engine
  4", SIGGRAPH 2013 course (split sum, representative point); T. Duff et al., "Building an
  Orthonormal Basis, Revisited", JCGT 2017; E. Heitz, "Understanding the Masking-Shadowing
  Function in Microfacet-Based BRDFs", JCGT 2014.
- **Project inputs:** the art bible v0 draft on PR #347 (`docs/art/README.md` decisions 9 and 11;
  `material-vocabulary.md` §1, §3, §8; `silhouettes-and-scale.md` RIM-1 to RIM-6;
  `palette-and-value.md` CON-1 to CON-5), read-only.
