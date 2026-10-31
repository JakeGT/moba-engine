# Roadmap: next-gen photoreal graphics

Written 2026-10-30. Status: **accepted plan**; maintainer decisions in §1. Owner: maintainer.

**Follows:** [the AAA graphics roadmap](aaa-graphics-roadmap.md), whose engine work (R0–R8) is
complete.

**Goal:** Cyberpunk 2077-class visual fidelity in a MOBA, built in Rust on Vulkan, as optimised
as possible. Today's "Ultra" becomes roughly the new Medium, and the scale goes up from there to
a path-traced Overdrive tier.

**Scope:**
- **Covered:** the renderer, the asset pipeline, AI-assisted content creation, vendor
  technology (DLSS, FSR, XeSS, Reflex), and the performance evidence.
- **Unchanged:**
  - Rendering stays purely presentational. No phase touches the simulation, the wire format,
    replays or the state hash.
  - Every feature is client-local, tiered, has an off switch, and allocates nothing per frame.
  - Each phase writes an ADR before its code.

This plan comes from four research passes done on 2026-10-30, covering:
- NVIDIA's stack;
- the vendor-neutral stack;
- photoreal rendering techniques;
- assets and AI generation.

Their sources are cited inline. Status claims (shipped / preview / DX12-only) are as of that
date and **must be re-checked when each phase starts**: this area moves monthly.

---

## 1. Decisions taken (maintainer, 2026-10-30)

| Topic | Decision |
|---|---|
| Art direction | **Full photoreal.** Readability comes from HUD-borne text and numbers, outlines, team rims, telegraph overlays and VFX rules (§4), not from stylisation. |
| Vendor policy | **NVIDIA-first, vendor-neutral fallbacks.** DLSS SR/RR/FG and Reflex on NVIDIA; FSR, XeSS and in-house TAA elsewhere; hardware RT through Khronos KHR/EXT extensions. Closed NVIDIA binaries are acceptable in the client. |
| Tier scale | **Five tiers:** Low, Medium, High, Ultra, Overdrive. Each tier is a preset over per-feature toggles (§3). |
| Hardware | **Low = RTX 2060 / RX 6600 class**, so **every tier has hardware ray tracing.** The top tiers target RTX 4070 Ti / 5070 Ti and up. Pascal and older GPUs are dropped: GTX 10 moved to security-only drivers in Oct 2025. |
| Lighting | **Fixed lighting per map, baked**: GPU path-traced GI on every tier. Ray tracing adds the dynamic parts: characters, ability and VFX light, and reflections. |
| Frame generation | **Allowed, off by default,** on Ultra and Overdrive, with Reflex or Anti-Lag. Budgets are met **without** it. |
| DLSS 5 | **Optional in gameplay**, as an Overdrive opt-in, once a public Vulkan SDK exists. It must pass the readability tests and is **locked off in ranked and tournament play**. Also used for showcase and cinematics. |
| Camera range | Invest in **zoomed views, spectator cinematics and a hero showcase** (champion select, victory). Gameplay budgets are still set by the 46 m camera. |
| Asset licences | **CC0 and CC-BY**, with attribution in THIRD_PARTY.md and credits. No NC, ND or SA licences, and no engine-restricted terms such as Megascans/Fab Standard. **Listed exceptions:** CMU mocap; outputs from the NVIDIA Open Model License (Kimodo). |
| AI 3D generation | **Local open-weight models** on the RTX 5070 Ti, with permissive licences that hold in the UK (Hunyuan3D's licence excludes the UK). Every output passes the remesh and bake pipeline (§9). |
| Compat renderer | **Keep today's renderer as Compat**, give it one refresh (C1, §8) so photoreal content still looks good on it, then **freeze** it. It serves macOS, NVK and GPUs without hardware RT. |
| NVIDIA SDKs | **Use NVIDIA's SDKs** (RTXDI, SHaRC, NRD, and RTXPT as reference) rather than clean-room versions: matching their quality would take years. |
| Art bible | **Owned by the maintainer and their team** (possibly a graphic designer and a marketing executive); agents draft and keep it testable (§9.6). |
| Platforms and hardware | **Linux and Windows** are the targets for the tiered renderer. macOS is Compat only: its hardware can't reach the new Medium, or arguably Low. The reference-GPU matrix is **brought forward** from pre-1.0, with Windows tested in a GPU-passthrough VM (§10). |

**Consequence: macOS and non-RT GPUs.**
- MoltenVK and KosmicKrisp expose neither ray tracing nor mesh shaders on Apple Silicon
  (MoltenVK PR #2771 is still a work-in-progress opt-in). NVK has no RT either.
- With RT required on every tier, these platforms can't meet any tier.
- **Decided:** today's renderer becomes the **Compat** path. It gets one refresh (C1, §8), then
  freezes: bug fixes and content-format compatibility only. It covers macOS, NVK and the older
  GPUs (Pascal, Polaris) that main supports today.

## 2. What "next-gen" means at a MOBA camera

The camera sets the budget.

**At the default 46 m view** (60° FOV, ~54° pitch):

| Resolution | Ground resolution | Height of a 1.8 m hero |
|---|---|---|
| 1080p | 20 px/m | — |
| 1440p | 27 px/m | about 40 px |
| 4K | 41 px/m | about 60 px |

One 4K pixel at the focus point covers about 2.5 cm. The consequences:

- **High value at the gameplay camera:** lighting, GI, shadows, reflections, wet surfaces,
  volumetrics, VFX light, materials (roughness and metal response), and temporal stability.
  These are what make a top-down frame read as photoreal.
- **Low value at the gameplay camera:** micro-geometry. Above about **1–2k triangles a hero's
  triangles are sub-pixel**, and texture density above about 64–128 px/m is never sampled.
  Strand hair, path-traced skin SSS, parallax occlusion and Nanite-style geometry go to the
  **zoom (8 m), spectator and showcase cameras**, which get their own render configuration.
- **"Meshy-class" AI meshes** of 0.5–2M triangles add nothing at 46 m and cost real GPU,
  memory and BVH time. Their detail is baked into normal and AO maps (§9). Dense sources are
  kept only as bake sources, or as cluster-LOD sources for static assets.
- **The fixed map is the biggest lever.** It lets us bake path-traced GI and keep real-time RT
  for what moves. id dropped baking only because it didn't scale to ~5,000 km² of levels; we
  have one board.
- **Reference points:**
  - LoL renders a whole frame in about 200k triangles.
  - Dota 2 heroes are about 4.4–6k triangles in game.
  - Photoreal at 46 m needs roughly **1–3M visible triangles per frame**, not tens of millions.

## 3. The new tier scale

| Tier | Reference GPU class | Output | Internal resolution and upscaler | Target (p95, no frame generation) | GPU budget |
|---|---|---|---|---|---|
| Low | RTX 2060, RX 6600 | 1080p | DLSS / FSR / XeSS Quality (~720p) | 60 fps | ≤ 12 ms |
| Medium (≈ today's Ultra, plus RT) | RTX 3060, RX 6700 XT | 1080p | Quality | 144 fps | ≤ 6.0 ms |
| High | RTX 4060 Ti / 4070, RX 7700 XT | 1440p | Quality (~960p) | 120 fps | ≤ 7.5 ms |
| Ultra | RTX 4070 Ti Super / 5070 Ti, RX 9070 XT | 1440p–4K | Quality at 1440p; Performance at 4K | 120 fps at 1440p, 90 at 4K | ≤ 9 ms |
| Overdrive | RTX 4080 / 5080 and up | 1440p–4K | Performance plus Ray Reconstruction | 60 fps rendered (FG optional) | ≤ 15 ms |

**Measurement rules:**
- Budgets are measured on the R5 teamfight replay (`tests/replays/teamfight-5v5.mbr`), with
  frame generation off, as today.
- Frame generation is reported separately, as presented fps plus added latency (Reflex/PCL).
- The per-tier perf-gate baselines become **one baseline per reference GPU**. That needs the
  reference-GPU matrix (§10), which is now brought forward from pre-1.0 (#9/#127). The runner
  must hold a fixed power state (the lesson of perf/0022).

### Feature matrix

| Area | Low | Medium | High | Ultra | Overdrive |
|---|---|---|---|---|---|
| Frame | Thin G-buffer, deferred (§5.2) | same | same | same | same, plus path-traced lighting |
| Anti-aliasing / upscaling | TAA or DLSS/FSR/XeSS | same | same | same, DLAA option | DLSS RR (NRD + upscaler elsewhere) |
| Static GI | baked (lightmaps / SH probes) | baked | baked | baked + RT GI (§6.3) | path-traced |
| Dynamic GI | probe-lit dynamics | + half-res SSGI | + RT probe volume near the camera | hash radiance cache + per-pixel gather | path-traced + SHaRC (NRC optional) |
| Sun shadows | tight shadow map + PCSS | + RT contact | RT sun shadows (SIGMA) | same | path-traced |
| Local and ability lights | clustered, 2–4 shadowed | + LTC area lights | + RT shadow rays, top-N per tile | ReSTIR DI | ReSTIR DI |
| Reflections | baked probes | SSR + probes | half-res RT below 0.45 roughness | full-res RT, multi-bounce through the cache | path-traced |
| AO | baked + GTAO | same | RT AO | from GI | from path tracing |
| Materials | multi-layer PBR (§6.6) | same | same | same | same |
| Skin | pre-integrated | same | separable SSS | same | path-traced SSS (showcase too) |
| Hair | cards | cards | cards | cards | cards; strands in showcase |
| Atmosphere | analytic height fog, Hillaire sky | + froxel volumetrics | same | + volumetric shadows from local lights | same |
| Weather / wetness | puddle masks | + ripples, streaks | same | same | same |
| Geometry | GPU-driven, discrete LODs | + mesh-shader path | same | same, cluster LOD for static | same, Mega Geometry optional |
| Frame generation | — | — | — | opt-in | opt-in |
| DLSS 5 | — | — | — | — | opt-in, not in ranked (RTX 50) |

**Showcase and cinematic modes** have their own preset, independent of tier. It adds:
- showcase LODs (60–150k triangles) and streamed mip 0;
- strand hair, path-traced SSS, eyes;
- depth of field, motion blur and film grain;
- DLSS 5 where available.

## 4. Readability contract

These rules are non-negotiable on every tier.

Photoreal raises the readability risk: Riot's Summoner's Rift update, Dota 2's art guide and
Diablo IV all put readability first. The rules:

1. **The gameplay overlay layer.** Outlines, telegraphs, range indicators, health bars,
   selection rings and the HUD draw **after upscaling, at output resolution**. They are never
   in TAA, DLSS or FG history, so they don't ghost and stay sharp. Frame generation gets them
   through the HUD-less colour and UI-alpha inputs. Today's decal telegraphs move into this
   layer.
2. **Team rims in the lighting pass.** Team-tinted rim light that survives darkness, fog and GI.
3. **Environment value and saturation budget per map,** clamped in playable space. A debug
   "readability heatmap" view.
4. **Automated contrast tests per tier.**
   - Headless captures of standard scenes measure the luminance and colour contrast of units
     against the background, plus telegraph contrast.
   - A higher tier (fog, GI, DLSS 5, weather) may **never** hide what Low shows.
   - These tests run in CI on lavapipe, plus on the GPU runner.
5. **Exposure** is locked or limited during gameplay. Full eye adaptation only in cinematics.
6. **Locks in ranked and tournament play:**
   - DLSS 5 is off;
   - frame generation follows the competitive rules (off by default);
   - weather and volumetric density are capped;
   - any setting that changes visibility is fixed.
7. **VFX rules from R5 carry over:** enemy-telegraph floors, ally dimming, and overdraw and
   light budgets per team.
8. **Motion vectors for everything** (§5.3) and reactive masks on particles, so fast units
   don't ghost. Debug views ship with them.

## 5. Architecture changes

### 5.1 Tooling and bindings (first)

- **Vulkan bindings.**
  - The released `ash` crate (0.38, Apr 2024) predates the extensions we need: cluster AS,
    `VK_EXT_ray_tracing_invocation_reorder`, `VK_KHR_opacity_micromap`, cooperative vectors,
    `VK_NV_low_latency2` updates.
  - Pin `ash` to a reviewed **git revision** (master is at headers 1.4.352), or generate our
    own bindings.
  - `deny.toml` has `unknown-git = "deny"`, so add an explicit allow-git entry with a recorded
    reason.
- **Shaders.**
  - Keep GLSL and glslang for existing passes: glslang 16.6 supports every RT and mesh
    extension we need.
  - Write new RT, denoise and neural passes in **Slang** (Khronos-hosted, Apache-2.0 with LLVM
    exception), compiled by `slangc` from `build.rs` and validated by `spirv-val`. Slang gives
    `HitObject`/SER, cooperative vector and matrix, autodiff, and can ingest vendor HLSL.
  - Skip rust-gpu: it's still pinned to nightly, with breaking changes every release.
- **Bindless descriptors.** Use `VK_EXT_descriptor_indexing` (core in 1.2): one global set of
  large texture, sampler and buffer arrays. That removes the 16-sampler ceiling the scene stage
  hits today.
  - Abstract the binding layer so it can later target `VK_EXT_descriptor_heap` (ratified Jan
    2026; it deprecates descriptor buffer).
- **Device capability gate.** Ray tracing (`VK_KHR_acceleration_structure`,
  `VK_KHR_ray_query`), `VK_KHR_fragment_shading_rate` and `VK_EXT_mesh_shader` become required
  for the tiered renderer. Without them the client falls back to Compat (§1).
- **Budgets.** Per-pass GPU timers already exist. Add a per-tier budget table that drives
  dynamic resolution and fails `perf-gate`.

### 5.2 Frame architecture: forward clustered → thin G-buffer + compute deferred

This is the path Doom: The Dark Ages took from clustered Forward+. It gives us:
- the guide buffers DLSS Ray Reconstruction and the denoisers need;
- one primary-hit description for RT and path tracing;
- decoupled material cost;
- far fewer pipelines;
- a home for software variable-rate shading.

Migration steps:
1. The forward pass also writes G-buffer targets: albedo, oct normal, roughness/metal,
   shading-model ID and flags, motion vectors, emissive.
2. Opaque lighting moves to tile-classified compute deferred lighting, using the existing
   clustered light lists plus LTC area lights.
3. Forward stays for transparents, water, particles and hair cards.
4. Decals become G-buffer decals: scorch and splats. Gameplay telegraphs move to the overlay
   layer (§4).
5. **Later and optional:** a visibility buffer with a material-binned resolve that writes the
   same G-buffer. Do it only if material count or overdraw justifies it.

**MSAA 4× is retired.** TAA, DLSS, FSR or XeSS replaces it on every tier, and outlines move
post-upscale.

### 5.3 Temporal foundation

- Halton jitter, with exact motion vectors for:
  - skinned meshes (previous bone palette);
  - VAT minions (previous frame time);
  - particles (previous position);
  - foliage wind;
  - water and scrolling materials (reactive masks).
- History resets on camera cuts and spectator switches.
- An in-house **TAA/TAAU**, based on Karis 2014, variance clipping, and Playdead's MIT TAA as
  reference.
- **One upscaler interface with these back ends:**

| Back end | Status | Platforms | Licence |
|---|---|---|---|
| **FSR 3.1.6** (universal) | Shipped | Any vendor; Linux and Windows (Godot ships a GLSL port) | MIT source |
| **DLSS SR and RR** | Shipped | NVIDIA. NGX-direct on Windows and Linux; the `dlss_wgpu` and `nvidia-rs` crates are prior art. | Binaries under the NVIDIA RTX SDKs License |
| **FSR 4.1.2** (ML) | Shipped; Vulkan since FSR SDK 2.4 (2026-10-28) | AMD RX 6000+, Windows | Signed DLLs |
| **XeSS-SR** | Shipped (Vulkan) | Intel; also a DP4a path elsewhere | Windows DLL |

### 5.4 GPU-driven geometry

- An instance buffer with compute frustum culling, meshoptimizer meshlets with cone and
  small-primitive culling, and `vkCmdDrawIndexedIndirectCount`.
- A mesh-shader fast path.
- Two-phase HiZ occlusion, which pays off mainly at low spectator angles.
- **Cluster LOD** (meshoptimizer `clusterlod.h`, Nanite-style) for **static** assets only:
  structures, rocks, props, foliage. Skinned heroes keep discrete LODs. (Skeletal Nanite is
  still experimental in UE 5.8.)
- This replaces R8's CPU culling, which ADR-0018 §2 deferred until instance counts justified
  it. Photoreal density does.

### 5.5 Ray-tracing infrastructure

- A **TLAS/BLAS manager:**
  - compacted proxy BLAS for the static map (LOD1–2), built once and cached;
  - per-frame refits for the 10 heroes;
  - **pose-bucketed shared BLAS for VAT minions**: N quantised poses per minion type, a few
    refits per frame;
  - no particles in the BVH (they feed the light list instead).
- `VK_EXT_ray_tracing_invocation_reorder` (SER, cross-vendor since Nov 2025, a no-op where
  the hardware lacks it), always emitted.
- `VK_KHR_opacity_micromap` (May 2026; NVIDIA only in practice), with alpha-test any-hit as
  the fallback, for foliage.
- **RTX Mega Geometry** (`VK_NV_cluster_acceleration_structure` / partitioned TLAS) is an
  optional NVIDIA path later. It's NV-only on Vulkan, and DXR's cross-vendor version is only a
  preview (2026-10-28).
- A **world light grid** (idTech 8-style cascaded grid) shared by GI, reflections and
  many-light sampling.
- **Denoising:**
  - NRD (ReBLUR or ReLAX for GI and reflections, SIGMA for shadows) on every vendor;
  - DLSS Ray Reconstruction replaces it on NVIDIA at Overdrive, and optionally at Ultra;
  - AMD's ML Ray Regeneration is DX12-only, so it's not available to us.

## 6. Rendering features

Costs are estimates for 1440p on an RTX 4070-class GPU unless measured. Every feature gets
measured on the reference GPUs in its phase.

### 6.1 Baked lighting: the largest visual jump for Low and Medium

- A `moba-tools light-bake` that path-traces each map on the developer GPU with Vulkan RT,
  sharing BVH code with §5.5.
- It produces:
  - SH-L1/L2 lightmaps or a probe volume for static geometry;
  - irradiance probes for dynamic objects;
  - box-projected reflection probes, renormalised by local irradiance (idTech 8);
  - the existing baked AO.
- Denoised offline with Open Image Denoise (Apache-2.0).
- Runtime cost about 0.1 ms.
- `content-check` validates that the bake matches the map (fingerprint, as ao-bake does).
- **Risks:**
  - leaks at walls (dilation and visibility-aware probes);
  - about 100–500 MB per map (compress, and stream by tier);
  - bake times of hours (incremental bakes, cached).

### 6.2 Direct lighting and shadows

- **Sun:** one texel-snapped 4096² shadow map with PCSS. The gameplay frustum is small, which
  gives about 1.5 cm texels.
  - RT sun shadows with SIGMA from High up: about 0.8–1.5 ms.
  - Cascades only for spectator and cinematic cameras.
- **Local and ability lights:**
  - clustered deferred lighting with LTC area lights for beams and capsules;
  - RT shadow rays for the top-N lights per tile (High);
  - **ReSTIR DI** for hundreds of emissive VFX lights on Ultra and Overdrive, using **RTXDI**
    (HLSL shader library compiled to SPIR-V through Slang or DXC). About 1.5–3 ms.
- VFX register light-list entries under a hard per-frame budget (e.g. 256 lights, 16
  shadowed).

### 6.3 Global illumination

- **Low, Medium, High:**
  - baked static GI plus probe-lit dynamics;
  - **Medium** adds half-res SSGI (≤ 0.8 ms) for dynamic bounce;
  - **High** adds an RT-updated probe volume (DDGI-lite) around the camera focus (~0.7 ms),
    so heroes and VFX inject bounce.
- **Ultra:** the idTech 8 "Fast as Hell" design (SIGGRAPH Advances 2025): a spatially hashed
  world radiance cache plus a ½–¼-res per-pixel final gather with a bilateral upscale.
  - It's **~1.7 ms measured** on PC in Doom: The Dark Ages, and less when seeded from the
    baked GI.
  - Built on **SHaRC**, NVIDIA's packaged hash radiance cache (any RT GPU, RTX SDKs License),
    with the idTech 8 per-pixel gather on top.
- **Overdrive:** path-traced GI with SHaRC. NRC (neural radiance cache) is optional: it's
  Windows plus CUDA only, experimental, and Turing+.
  - ReSTIR GI is **A/B-tested, not assumed**: Bevy 0.20 (Oct 2026) made it off by default
    once DLSS RR improved enough that it rarely paid.

### 6.4 Reflections

| Tier | Approach |
|---|---|
| Low | Baked probes |
| Medium | SSR + probes |
| High | Half-res RT below roughness ~0.45, probe fallback above |
| Ultra | Full-res RT with multi-bounce through the cache |
| Overdrive | Path-traced |

- Hit shading uses a cheap material, the world light grid with one shadow ray, and indirect
  light from the cache or probes. A Lumen-style surface cache is rejected as XL complexity.
- Water and puddles reflect through RT or SSR. The screen-space refraction from R3b stays.

### 6.5 Atmosphere and weather

- **Sky:** Hillaire 2020 LUT sky (~0.1–0.2 ms).
- **Fog:** froxel volumetric fog (~160×90×64, temporally reprojected), shadowed by the sun
  map and clustered lights (~0.4–0.8 ms, Medium and up).
- **Clouds:** volumetric clouds for backdrops, showcase and cinematics only (Nubis-style,
  quarter-res).
- **Wetness:** porosity darkening, a roughness drop, puddle masks, ripples and streaks
  (Lagarde 2013).
- Weather is client-side and gameplay-neutral, capped by the readability tests (§4).

### 6.6 Materials and characters

- **Material model:**
  - GGX with multiscatter energy compensation;
  - clear coat, sheen, anisotropy, specular tint, transmission and iridescence;
  - parameters follow glTF `KHR_materials_*` / OpenPBR, so assets map directly;
  - a bounded set of shading-model IDs (not open-ended layering).
- **Skin:**

| Where | Approach |
|---|---|
| Low, Medium | Pre-integrated |
| High and up | Separable SSS (~0.2–0.4 ms on masked pixels) |
| Showcase and Overdrive | Path-traced or diffusion SSS (RTXCR as reference) |

- **Hair:** cards with a Marschner-style shader and alpha-to-coverage, stabilised by TAA.
  Strands (RTXCR, LSS on Blackwell) in the showcase only.
- **Eyes:** refractive cornea and parallax iris (showcase).
- **Cloth:** sheen.
- **POM and tessellation:** showcase terrain only. Displacement micromaps are deprecated by
  NVIDIA, so skip them.

### 6.7 Post and camera

- **Gameplay:** physical camera (EV100) and AgX with LUTs (kept), graded so gameplay contrast
  is identical on every tier.
- **Cinematics only:** depth of field (diaphragm/circular separable), motion blur, subtle lens
  effects, and film grain applied post-upscale (never in history).

### 6.8 Selective rendering and optimisation

- **Variable-rate shading:**
  - software VRS in the compute lighting (idTech 8 VRCS, CoD-style);
  - hardware `VK_KHR_fragment_shading_rate` for raster passes;
  - full rate around your hero, the cursor and active fights, lower at the periphery.
- **HUD masking:** skip shading under opaque HUD panels (the minimap and ability bar cover
  ~15–25 % of pixels) using a stencil or tile mask.
- **Async compute:** GI and cache updates, BVH refits, froxels and particle simulation overlap
  the shadow and G-buffer raster. id measured 0.4–0.5 ms saved.
- **Half resolution** with depth-aware upsampling for GI, reflections, AO, volumetrics and
  clouds.
- **Texture streaming:**
  - in a match, mips ≤ 512² stay resident;
  - mip 0–1 stream in for the zoom, spectator and showcase cameras;
  - software feedback from the material pass (Vulkan has no sampler feedback);
  - full virtual texturing only if needed.
- **Neural texture compression** (RTX NTC, beta):
  - load mode (transcode to BCn at load) for disk and download size;
  - sample mode is NVIDIA-specific (`VK_NV_cooperative_vector`), so it's later and optional.
- **Pipelines:**
  - a build-time pipeline manifest, a warmed pipeline cache, and graphics pipeline libraries
    or shader objects for fast linking;
  - the deferred resolve collapses most permutations;
  - R8's `locked_pipelines=0` rule still holds: no mid-match compiles.

**Example High-tier budget** (1440p, upscaled from ~960p; planning estimates to be replaced by
measurements):

| Pass | ms |
|---|---|
| Sun shadow | 0.6 |
| G-buffer | 1.2 |
| Deferred direct | 0.8 |
| RT shadows | 0.8 |
| GI probes | 0.5 |
| RT reflections | 1.5 |
| GTAO | 0.3 |
| Fog and sky | 0.5 |
| Transparents, particles and water | 1.0 |
| Upscaler | 0.8 |
| Post and overlay | 0.6 |
| UI | 0.3 |
| **Total** | **≈ 8.9** |

That total is over High's 7.5 ms budget, which is exactly why the budget table and dynamic
resolution are part of N0.

## 7. Vendor technology and licences

| Technology | Use | Vulkan | Licence and obligations |
|---|---|---|---|
| **DLSS SR and RR** (4.5 transformer; RR preset F) | NVIDIA upscaler; RR is the Overdrive denoiser | Yes (NGX-direct on Windows and Linux, or Streamline) | NVIDIA RTX SDKs License: object-code redistribution allowed; **notify NVIDIA before commercial release**; NVIDIA marks on the splash screen, About box and credits; NVIDIA GPUs only; no open-source relicensing of the SDK |
| **DLSS FG / MFG** | Opt-in on Ultra and Overdrive | Through **Streamline, Windows only**: interposer-loaded `vkGetInstanceProcAddr`, an optical-flow queue, swapchain hooks, HUD-less and UI-alpha inputs, Reflex via `sl.reflex`. Dynamic MFG is DX12-only. | Streamline is MIT; DLSS binaries as above |
| **Reflex** | Every tier, to cut latency | `VK_NV_low_latency2` directly; it moves to `sl.reflex` once FG is on | Behind a latency trait |
| **Anti-Lag** | AMD latency | `VK_AMD_anti_lag` | — |
| **DLSS 5** | Overdrive opt-in plus showcase (§1) | **SDK not public**; Vulkan support unknown | RTX 50 only for now; ~50–60 % fps cost. Revisit when an SDK ships. |
| **FSR 3.1.6** | Universal upscaler and FG on Windows | Yes | MIT source |
| **FSR 4.1.2 / FSR 4 FG** | AMD ML upscaler (RX 6000+) and FG (RX 9000, Windows 11) | Yes, since SDK 2.4 | Signed DLLs, binary-only |
| **XeSS-SR** | Intel upscaler | Yes (XeSS-FG and XeLL are DX12-only) | Intel licence, binary |
| **NRD** | Cross-vendor denoiser | Yes | NVIDIA RTX SDKs License (attribution) |
| **RTXDI, SHaRC** | ReSTIR DI and the hash radiance cache (Ultra, Overdrive) | Yes (HLSL headers → SPIR-V) | RTX SDKs License. Unlike DLSS and Reflex, they are **not** restricted to NVIDIA GPUs; the app must stay interoperable with NVIDIA hardware. Credits attribution. |
| **RTX NTC, RTXNS, NRC** | Later or optional | NV-specific paths | RTX SDKs License |

**How closed SDKs are handled.**
- No proprietary SDK source or binary is committed. The RTX SDKs License (clause 4e) forbids
  making the SDK subject to an open-source licence, so the SDKs stay out of the MIT/Apache tree.
- They are fetched at build time behind cargo features (`dlss`, `streamline`, `rtxdi`,
  `sharc`, `nrd`, `fsr4`, `xess`), with checksums pinned, and compiled into release builds.
- **The default open build** (no SDK features) runs Low to High with in-house denoising and
  shadow-ray lighting. Ultra and Overdrive need the SDK features and say so in the settings
  menu.
- Licence check (RTXDI and RTXGI `LICENSE.txt`, read 2026-10-30): the "NVIDIA GPUs only"
  clause covers the DLSS, Reflex, NGX and Dynamic Vibrance SDKs, not RTXDI, SHaRC or NRD.
  Re-read the licence at each SDK bump.
- `THIRD_PARTY.md` and the in-game credits gain the required notices.
- The release checklist gains "notify NVIDIA".

**Linux.** Every vendor's ML frame generator and the FSR 4 and XeSS DLLs are Windows-only.
Linux gets:
- DLSS SR/RR (NGX `.so`);
- FSR 3.1.6;
- TAA.

That is acceptable, and it is documented per tier.

## 8. Phases

Sizes assume agent teams like those that delivered R1–R8:
- S < 2 weeks;
- M 2–6 weeks;
- L 1.5–4 months;
- XL > 4 months.

Rendering never touches determinism. Every phase:
- has an ADR;
- has tier switches and off switches;
- has validation and sync-validation evidence;
- rebaselines goldens deliberately;
- records GPU timings as the median of 3 interleaved rounds;
- passes the readability tests (§4).

| Phase | Work | Size | Depends on |
|---|---|---|---|
| **N0 Foundations** | `ash` git pin and extension plumbing; the RT, mesh-shader and VRS capability gate and the Compat freeze; bindless; Slang toolchain; per-tier budget table driving dynamic resolution; new tier presets | M | — |
| **N1 Temporal and upscaling** | Jitter and motion vectors for everything; TAA/TAAU; upscaler interface with FSR 3.1.6, DLSS SR (NGX), FSR 4, XeSS; **post-upscale gameplay overlay layer** (outlines, telegraphs, HUD); Reflex and Anti-Lag; MSAA retired | L | N0 |
| **N2 Frame architecture and materials** | Thin G-buffer, compute deferred lighting, LTC area lights, forward transparents, G-buffer decals, multi-layer material model, team rims in lighting | L | N1 |
| **N3 Baked lighting** | `light-bake` (Vulkan RT path tracer), lightmaps/probes, reflection probes, OIDN, content-check validation, baked for all maps | L | N2; shares BVH with N4 |
| **N4 RT infrastructure** | TLAS/BLAS manager (static proxies, hero refits, VAT pose buckets), SER, OMM, world light grid, NRD integration | L | N0, N2 |
| **N5 Hybrid RT (Low–High)** | RT sun shadows, RT AO, RT reflections, RT probe volume, SSGI | M–L | N3, N4 |
| **N6 Atmosphere and weather** | Hillaire sky, froxel volumetrics, backdrop clouds, wetness and puddles | M | N2 (parallel with N4/N5) |
| **N7 GPU-driven geometry** | Meshlets, compute culling, indirect count, mesh shaders, HiZ, cluster LOD for static assets; optional visibility buffer | L | N0 (parallel) |
| **N8 Ultra GI and many lights** | Hash radiance cache with per-pixel gather; ReSTIR DI for VFX and ability lights | L | N4, N5 |
| **N9 Overdrive** | Path tracer reusing hit shading; SHaRC (NRC optional); ReSTIR DI (GI A/B); DLSS RR (NRD elsewhere); FG via Streamline / FSR 3.1 FG; DLSS 5 opt-in once its SDK exists | L–XL | N1, N4, N8 |
| **N10 Characters and showcase** | Skin, eyes, cloth, hair cards; showcase mode (LOD class, streamed mip 0, strands, path-traced SSS, DOF, motion blur); champion select and victory screens | L–XL | N2; parts on N9 |
| **N11 Content scale** | Mip streaming (≤ 512² resident in match), software feedback, NTC load mode; virtual texturing only if needed | M–L | N0 |
| **C1 Compat refresh, then freeze** | Bring today's forward renderer up to photoreal content: GGX with multiscatter compensation and the N2 material parameters (dropping the stylised ramps); sample N3's baked lightmaps, probes and reflection probes (the biggest win, and it needs no RT); Hillaire sky and improved height fog from N6; loading the shared content packs at capped LODs and mips; the §4 readability tests. Targets: 1080p60 on GTX 1060 / RX 580; Apple M1-class at a reduced render scale. Then **freeze**: bug fixes and content-format compatibility only. | M | N2 (material spec), N3 (bakes), N6 (sky) |
| **A-track** (parallel from N0) | Asset pipeline and content (§9): A0 licence gate, A1 ingest tools, A2 AI workstation, A3 vertical slice, A4 roster | XL, continuous | A3 needs N2 + N3 and art bible v0; A4 needs bible v1 (§9.6) |

**Critical path:** N0 → N1 → N2 → (N3 ∥ N4) → N5 → N8 → N9.

N6, N7, N11 and C1 run alongside. **The vertical slice (A3)** is one photoreal map section, one
hero and the minions, shown at Medium and Overdrive. It is the go/no-go point, as §8 of the
previous roadmap had it: it proves the look and the budgets before scaling the roster.

**Indicative timeline** with 2–3 parallel agent teams and maintainer review:
- N0–N2: 4–6 months;
- N3–N5: 3–4 months;
- N7–N9: 4–6 months.

That is **12–18 months of engine work** to Overdrive, with N10 and content continuing beyond.
The art side decides when it *looks* Cyberpunk-class.

## 9. Asset pipeline and AI generation

### 9.1 Budgets: avoiding the "obscenely high-poly" trap

These extend `content-check`'s `TIER_BUDGETS` with a **showcase class** that is never resident
in a match.

| Asset | Gameplay LOD0 (46 m) | Zoom / spectator LOD | Showcase (on demand) | Textures (authored → resident in match) |
|---|---|---|---|---|
| Hero | 5–8k tris | 15–30k | 60–150k (source ≤ 1M) | 2048² body + 2048² head → ≤ 512² resident; mips 0–1 streamed |
| Minion (VAT) | 1–2.5k | 4–6k | 15–30k | 1024² shared atlas → 256² resident |
| Boss / large monster | 10–20k | 30–50k | 100–150k | 2048² |
| Structure | 15–40k, or cluster LOD (source ≤ 1–2M) | cluster LOD | — | Tileables and trim at 256–512 px/m, plus a unique 1–2k mask/AO |
| Prop | 0.3–4k (source ≤ 300k for cluster LOD) | — | — | Atlas/trim at 128–256 px/m |
| Foliage cluster | 1–6k (geometry, minimal alpha, OMM) | cluster LOD | — | 1024² atlas |
| Terrain | 0.5 m height spacing | adaptive | — | Layers at 256–512 px/m + macro maps at 32–64 px/m |

**How to read these numbers:**
- Detail finer than ~2.5 cm at the focus point (one 4K pixel at 46 m) is decimated away and
  baked into normal and AO maps.
- Dense AI or scan meshes are **bake sources**, never runtime assets. Static assets are the
  exception, where cluster LOD makes source density a disk and bake-time cost, not a frame
  cost: about 14 bytes per source triangle in Nanite's format, ~2.8 GB for 200 assets of 1M
  triangles.
- `content-check` enforces source caps, per-class budgets and texel density.

### 9.2 Sources (CC0 and CC-BY, plus the listed exceptions)

| Need | Sources | Licence | Notes |
|---|---|---|---|
| Environment materials, HDRIs, props | **Poly Haven** | CC0 | 997 HDRIs to 24k, 869 scanned textures, 521 models. The API allows bulk download with a unique User-Agent. |
| | **ambientCG** | CC0 | ~2,900 assets; API v2/v3. Throttle requests: it has one maintainer. |
| | CGBookcase, TextureCan | CC0 | — |
| | Smithsonian and the Met | CC0, per item | Statue and artefact scans |
| | Sketchfab | `cc0` and `by` only | Curated per model via the Download API, with a licence snapshot. New KitBash ToS (27 Oct 2026) bans scraping. |
| Human bases | **MPFB2 / MakeHuman** | CC0 | — |
| | Anny data | CC0 | Not its NC smplx topology |
| | SMPL-Body, FLAME 2023 Open | CC-BY | FLAME adds use clauses |
| | Meta MHR | Apache/MIT | — |
| | Rocketbox | MIT | 115 rigged NPC-grade avatars |
| | Blender Studio characters | CC-BY | — |
| Animation | **Quaternius UAL 1+2** | CC0 | ~250 clips: locomotion, combat, hits, deaths |
| | 100STYLE | CC-BY | — |
| | ACCAD | CC-BY | Taken from ACCAD directly; AMASS re-licenses it NC |
| | Mesh2Motion | CC0, per file | — |
| | **CMU** | Listed exception | — |
| | **Kimodo** text-to-motion | NVIDIA Open Model License, listed exception | — |

**Rejected:**
- Mixamo: no raw redistribution, so unusable in an open repo.
- Rokoko.
- Bandai Namco (NC).
- LAFAN1 (NC-ND).
- AMASS and HumanML3D (NC).
- Share Textures: no automated download, no redistribution in collections.
- Textures.com, Free PBR, 3D Scan Store samples.
- Megascans/Fab Standard: no standalone redistribution.
- MetaHuman, Daz, Reallusion.

**Gaps that no free source fills:**
- Cyberpunk-class rigged humans;
- pore-level skin texture sets;
- spell-casting mocap.

Those come from the AI pipeline, procedural detail, hand-keying, or commissioned or scanned
work. Real-person scans need **likeness releases**: CC licences don't grant them.

### 9.3 Local AI generation (RTX 5070 Ti, 16 GB)

| Purpose | Model | Licence | Notes |
|---|---|---|---|
| Image → PBR mesh | **TRELLIS.2-4B** (Dec 2025) | MIT | Fits 16 GB (~12–13 GiB peak measured on a 5070 Ti). Run it through **ComfyUI's native port**, which removes the NC nvdiffrast/nvdiffrec. Feed pre-cut alpha PNGs to avoid RMBG (NC). Outputs ~1M faces with 4K PBR, so decimate hard. |
| | Pixal3D | MIT | An alternative |
| | TripoSG | MIT | Light shape-only fallback |
| Tileable PBR materials | StableMaterials | OpenRAIL, commercial OK | Fed by albedo from Z-Image-Turbo or Qwen-Image (Apache) |
| | DeepBump / Materialize | GPL tools; outputs are free | Normal and height maps |
| Mesh texturing | MV-Adapter | Apache | — |
| Concepts | **Z-Image-Turbo** (6B) | Apache | Fits natively |
| | Qwen-Image-2512 | Apache | FP8 |
| | FLUX.1 schnell, FLUX.2 klein 4B | Apache | **Not** FLUX dev (NC) |
| Rigging | MPFB2 / Quaternius skeleton with weight transfer for humanoids | — | The whole CC0 animation library retargets free |
| | UniRig / Puppeteer for creatures | MIT / Apache code | Weights licences to confirm |
| Motion | **Kimodo** | NVIDIA OML (listed exception) | < 3 GB with the text encoder on CPU; retarget from its 30-joint skeleton. Avoid every model trained on AMASS or HumanML3D. |
| Delighting scans | Marigold IID | Apache code, OpenRAIL++ weights | — |

**Excluded:**
- Hunyuan3D / HY-Motion / HunyuanWorld (the licence excludes the UK);
- Step1X-3D's texture stage (Tencent code);
- MeshAnything (NC);
- Stable Fast 3D / SPAR3D (US$1M revenue cap);
- FLUX dev (NC);
- RMBG 1.4/2.0 (NC);
- any repo that pulls in nvdiffrast or Inria's rasteriser.

**Blackwell setup:** the RTX 5070 Ti needs CUDA ≥ 12.8 and PyTorch ≥ 2.7 (cu128). Several
repos need their CUDA extensions rebuilt, and flash-attn crashes nvcc on sm_120, so use
xformers or SDPA.

**The AI workstation lives outside the repo.** Model weights are never committed. The repo
holds only scripts, prompt sheets and provenance manifests.

### 9.4 The automated pipeline

```text
source / generate → licence gate → repair (voxel/SDF manifold) → simplify (meshopt: Permissive, Regularize, seam-protect)
  → UV (xatlas, texel density per class) → bake high→low (normal, AO, curvature, thickness)
  → material validation (delight, albedo 30–240 sRGB, metals ≥ 180, roughness variance, normal length)
  → LODs (discrete for skinned, clusterlod for static) → rig/skin/retarget → KTX2 BC7/BC5/BC4 + streamable mips
  → content-check (budgets, licence, PBR, texel density, source caps) → review renders → human approval → pack
```

**New tools** in `moba-tools`:
- `normal-bake`: a Rust high→low baker on the `ao-bake` ray core, with jittered AA, a cage and
  MikkTSpace. No mature open Linux CLI baker exists.
- a `clusterlod` mode for `mesh-bake`;
- a licence and provenance gate in `content-check`, which **auto-generates THIRD_PARTY.md,
  the credits and a Steam AI-disclosure list**;
- review-render contact sheets (46 m at 1440p and 4K, 8 m zoom, showcase turntable, lineup
  under each map's grading) with numeric readability checks.

**A correctness fix to make early:** tangents must be MikkTSpace. `meshopt_generateTangents`
only matches it with `meshopt_TangentCompatible`.

**What an agent automates end to end:**
- licence filtering;
- download with provenance;
- running the generators;
- repair, UVs, baking, LODs and KTX2;
- the budget, PBR and licence gates;
- credits and disclosure;
- review renders and diffs.

**Where a human is unavoidable:**
1. the **art bible** (palette, material vocabulary, faction shape language);
2. **choosing** among generated variants;
3. hero face and deformation topology, and **animation feel and ability timing**;
4. the **readability sign-off** at 46 m;
5. approving licence exceptions and likeness releases.

Without the human checkpoint, the pipeline produces technically valid but stylistically
incoherent assets. That is the main failure mode of pure-AI content.

### 9.5 Ownership of AI output

- **US:** purely AI-generated output is very likely uncopyrightable. *Thaler v. Perlmutter*
  cert was denied on 2026-03-02.
- **UK:** CDPA s9(3) still assigns computer-generated works to the arranger, but the government
  proposed removing it in March 2026. *Getty v Stability* is on appeal.
- **Policy:**
  - AI-generated assets are licensed **CC0** in the repo, with a per-asset provenance
    manifest: model, version, weights licence, seed, prompt, inputs and their licences, tools.
  - Inputs to image-to-3D must be our own or CC0.
  - Human-modified assets may be CC0 or CC-BY.
  - Steam's AI disclosure is generated from the manifests.

### 9.6 Art bible

**Owner:** the maintainer, working with their team, which may include a graphic designer and
a marketing executive. Agents draft, research and turn decisions into checks; people decide.

| Part | Contents | Lead | What becomes a check |
|---|---|---|---|
| World and factions | Setting, faction identity, shape language, architecture | Maintainer | — |
| Palette and value | Per-map palette; value and saturation budget in playable space; team colours | Maintainer + graphic designer | The §4 value and saturation clamps and contrast tests |
| Lighting mood per map | Sun angle and colour, sky/HDRI, fog density, exposure range | Maintainer | `light-bake` inputs; the gameplay exposure lock |
| Material vocabulary | The allowed materials and their look ("worn bronze", "wet basalt") | Agent draft from physicallybased.info and references; maintainer approves | `content-check` PBR ranges per material class |
| Silhouettes and scale | Hero, minion and monster size classes; silhouette rules at 40 px tall; team rims | Maintainer | Silhouette and contrast checks in the review renders (§9.4) |
| VFX language | Colour by team and ability type; telegraph styles | Maintainer + graphic designer | R5 readability rules; telegraph contrast tests |
| UI, HUD and typography | HUD layout and style, icons, fonts, number formatting, outlines | Graphic designer | HUD contrast tests; font licences (SIL OFL, as today's HUD font) |
| Brand and key art | Logo, key art, store capsules, trailer look, showcase and victory framing | Marketing executive + graphic designer | — |

**How agents help:**
- reference boards from CC0 photos, Poly Haven HDRIs and Apache-licensed concept models
  (§9.3);
- first drafts of each part, revised from the team's feedback;
- turning each decision into a numeric rule in `content-check` or the readability tests, so
  the bible is enforced on every asset rather than remembered;
- review-render contact sheets for every new asset against the bible.

**Where it lives:** `docs/art/`, holding the text, the rules and CC0 or team-owned reference
images. Work made by team members goes in the repo only under CC0 or CC-BY (or a written
assignment), so it can sit alongside the rest of the content. Brand marks (logo, name) are the
exception: they stay all-rights-reserved and are listed as such.

**Versions:**
- **v0**, before the vertical slice (A3): enough to build one map section, one hero and the
  minions.
- **v1**, signed off by the maintainer after the slice's go/no-go: the gate for the roster (A4).

## 10. Evidence and performance gates

- Every phase keeps the existing bar:
  - validation and sync validation clean on lavapipe **and** the RTX;
  - goldens rebaselined deliberately (two byte-identical lavapipe runs);
  - GPU timings as the median of 3 interleaved rounds;
  - `locked_uploads=0` and `locked_pipelines=0`.
- **Lavapipe** supports ray query, but slowly. Golden captures for RT tiers use small
  resolutions and fixed frames. CI keeps a raster-only Low golden set plus a few RT goldens.
- **Platforms:** Linux and Windows for the tiered renderer. macOS is Compat only (C1).
- **Reference GPUs**, needed per tier:
  - RTX 2060, RX 6600;
  - RTX 3060;
  - RTX 4070, RX 7700 XT;
  - RTX 5070 Ti (owned), RX 9070 XT;
  - RTX 4080 / 5080.
  - This is the pre-1.0 hardware matrix (#9/#127), **brought forward** (decided 2026-10-30).
    Until a card is on hand, its tier is estimated from the 5070 Ti. **Estimates never close a
    target**, as before.
  - The cheapest route is used cards for Low and Medium (RTX 2060, RTX 3060, RX 6600) plus an
    AMD card for the RX 7700 XT / 9070 XT rows, because the AMD paths (FSR 4, Anti-Lag, NRD on
    RDNA) need real AMD hardware.
- **Windows: a GPU-passthrough VM on the dev box.**
  - The box suits it: the Ryzen 9800X3D's Radeon 610M iGPU can drive the Linux desktop while
    the RTX 5070 Ti is bound to `vfio-pci` and passed to a Windows 11 VM (KVM/QEMU, OVMF, a
    pinned CPU set). IOMMU is already enabled (27 groups); check that the 5070 Ti and its audio
    function sit in their own group. NVIDIA has allowed GeForce passthrough since driver R465.
  - While the VM owns the 5070 Ti, Linux runs on the iGPU, so Linux-side RT work pauses.
  - **VM timings carry a small overhead** (CPU scheduling, interrupts), so evidence from the VM
    is labelled `windows-vm`. A target closes only on a bare-metal run: dual-boot, or a
    dedicated Windows test machine.
  - The Windows run covers what Linux can't: Streamline frame generation, FSR 4, XeSS, the
    DLSS DLL path and Windows present and pacing.
  - The Radeon 610M (RDNA2, RADV ray query) doubles as a correctness smoke test for the AMD RT
    path. It is not a performance reference.
- **Frame-generation evidence:** presented fps, added latency (Reflex PCL), and a ghosting
  review of fast units and telegraphs.
- **Readability tests** (§4) run every phase and gate merges.

## 11. Risks

| Risk | Mitigation |
|---|---|
| Photoreal hurts readability | The §4 contract, automated contrast tests, and ranked locks; vertical-slice go/no-go |
| Ghosting from TAA, DLSS or FG on fast units | Exact motion vectors, reactive masks, the post-upscale overlay layer, debug views |
| Vendor SDK churn (DLSS 5 SDK, Mega Geometry, cooperative vectors are NV-only, DX12-first features) | Abstractions per feature; re-verify status and licences at each phase start; SDKs pinned by checksum |
| Dependence on NVIDIA SDK licences (RTX SDKs License terms can change) | SDKs isolated behind cargo features and traits; the open build stays playable up to High; licence re-read on every bump |
| Compat bit-rot once frozen | Compat keeps its golden captures in CI and must load every content pack; content-format changes ship a Compat fallback in the same PR |
| `ash` on a git revision | Pinned revision, `deny.toml` allow entry, periodic bumps |
| Bake times and disk per map | Incremental bakes, compression, per-tier bake resolution |
| Raising min-spec to RTX 2060 drops players | The Compat path (C1) keeps older GPUs and macOS playable |
| AI asset licence drift (repos pulling NC dependencies) | A dependency grep in the AI workstation scripts; provenance manifests; the licence gate |
| Scope: photoreal content is the long pole | The A-track starts at N0; the vertical slice before the roster; art direction stays a human role |

## 12. Next concrete steps

1. Draft art bible v0 (§9.6) with the maintainer and team, starting with palette, lighting
   mood and material vocabulary for the first map.
2. **ADR-0020 (N0):** bindings, capability gate and Compat, bindless, Slang, tier presets and
   budgets, and the cargo-feature layout for the NVIDIA, AMD and Intel SDKs. Drafted as
   [ADR-0020](../adr/0020-next-gen-foundations.md) (proposed, 2026-10-31), with the vendor
   claims re-checked and its corrections to this plan listed there.
3. **ADR-0021 (N1):** the temporal foundation, the upscaler interface and the overlay layer.
4. **A0:** the licence and provenance gate in `content-check`, and the MikkTSpace tangent fix.
5. Set up the local AI workstation (ComfyUI + TRELLIS.2, Z-Image, StableMaterials, Kimodo) and
   produce a first test hero through the full §9.4 pipeline. That measures where the human
   effort actually goes.
6. Set up the Windows passthrough VM (§10), and source used Low and Medium reference cards plus
   one AMD card.

## 13. Open questions for the maintainer

All resolved on 2026-10-30 and recorded in §1: the Compat path (keep, refresh, freeze),
NVIDIA SDKs over clean-room versions, the reference-GPU matrix brought forward for Linux and
Windows, and the art bible owned by the maintainer and team with agent drafting (§9.6).
