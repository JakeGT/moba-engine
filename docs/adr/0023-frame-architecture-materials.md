# ADR-0023: Frame architecture and materials: thin G-buffer, compute deferred lighting, LTC area lights and the multi-layer material model (N2)

**Status:** proposed 2026-11-02. This is phase N2 of the
[next-gen graphics roadmap](../plans/next-gen-graphics-roadmap.md) (§3, §4, §5.2, §6.2, §6.6, §8
and §12), written before the code.
- **Builds on:**
  - [ADR-0020](0020-next-gen-foundations.md) (N0, accepted 2026-11-01, with Amendments 1–6). It
    settles the `Tiered`/`Compat` paths (§2), the bindless set and buffer device address (§3),
    Slang (§4), the tier budgets (§5) and the vendor-SDK features (§6). This ADR uses them and
    does not re-decide them.
  - ADR-0021 (N1, **proposed** on PR #350 as `docs/adr/0021-temporal-upscaling-overlay.md`,
    not yet on `main`, so it is not linked here). N2 takes over
    N1's render-resolution `prepass` and its `motion` attachment, writes N1's reactive and
    `tc_mask` targets from the new forward passes, and leaves N1's post-upscale overlay layer
    as it is. If ADR-0021 changes in review, this ADR follows it.
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
