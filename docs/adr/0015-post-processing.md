# ADR-0015: Post-processing chain, anti-aliasing and resolution scaling

**Status:** accepted 2026-10-29 (AAA graphics roadmap R6). Builds on
[ADR-0008](0008-render-graph.md) (render graph; this ADR amends its limits and adds image
sizes, sample counts and accesses), [ADR-0009](0009-descriptor-model.md) (plain sets) and
[ADR-0011](0011-art-style-lighting.md) (the lighting model whose ambient term AO occludes).
It composes with [ADR-0013](0013-vfx-runtime.md) (R5 VFX), whose passes run between `scene`
and the post chain on single-sample, render-resolution targets.

## Context
After R2a the frame ends in one composite pass: exposure (a constant) and the AgX tonemap
with a fixed saturation look, straight into the swapchain. There is no bloom, no colour
grading, no anti-aliasing, no ambient occlusion and no way to render below native resolution.
Roadmap R6 asks for all of these, each **tiered** with a **Low** path or an off switch (§3),
plus three gameplay effects (death desaturation, a damage vignette, a low-health pulse) that
players can turn off for accessibility. Readability beats realism (§2): nothing here may
blur team colours, smear fast units or touch the HUD.

Constraints that shaped the choices:
- **A forward renderer** (ADR-0011): there is no G-buffer, so normals for AO are not stored.
- **R5 VFX** (#329) draws particles after the scene into the HDR target with depth tested
  read-only and **sampled** (soft particles), and alpha particles through weighted blended
  OIT targets the size of the HDR target. Whatever anti-aliasing we pick must leave those
  passes single-sample, or they need multisampled pipelines and a depth resolve.
- **Determinism and goldens:** rendering is presentation only, but the golden fingerprints
  must stay byte-identical across runs. Anything with a feedback loop (eye adaptation,
  dynamic resolution) must be pinned in captures.
- **No per-frame allocation** and a precompiled graph (ADR-0008).

## Decision

### 1. The frame
```text
portraits/minimap  (R7b, unchanged)
shadow             (R2a, unchanged)
prepass   AO on    main view depth only → depth (msaa-depth + SAMPLE_ZERO resolve with MSAA)
ao        AO on    depth → ao-raw (half render res, RG16F: occlusion, view distance), GTAO
ao_blur   AO on    ao-raw → ao (half render res, R8): 4×4 depth-aware blur
scene              → hdr at render resolution (msaa-hdr resolved into hdr with MSAA); loads
                   the prepass depth (LESS_OR_EQUAL) and samples ao for the ambient term
[vfx_*]            (R5) single-sample, into hdr, depth read-only + sampled
bloom_d0..d4       hdr → bloom0 (½) … bloom4 (1/32): 13-tap down-sample, Karis average on d0
exposure  auto     bloom4 → exposure (1×1 R16F, persistent): metered, clamped, adapted
bloom_u3..u0       bloom[i+1] (3×3 tent) added into bloom[i]
composite          hdr + bloom0 + exposure + grading LUT + screen effects → ldr | swapchain
fxaa      FXAA     ldr → swapchain | ldr2
upscale   scaled   ldr(2) → swapchain: bilinear (Low) or Catmull-Rom bicubic
ui                 HUD at native resolution, never tonemapped, graded or anti-aliased
capture            (capture variant only)
```
Each block exists only when its feature is on: the topology is fixed per renderer at
creation (tier + switches), so every compiled variant shares the same transients, as
ADR-0008 requires. Changing a post setting recreates the renderer, like the tier.

**Render-graph changes (ADR-0008 amendment):**
- `ImageSize::Render { shift }`: an image at the *render* allocation (the swapchain extent ×
  the maximum render scale) divided by `2^shift` (bloom mips, half-resolution AO). HDR, depth
  and the R5 OIT targets are `Render { shift: 0 }`; only the post chain's last pass and the UI
  are swapchain-sized.
- `ImageDesc::samples` (MSAA attachments).
- New accesses: `DepthReadWrite` (load, test, write: the scene after the prepass) and
  `DepthResolveWrite` (the single-sample depth written by a multisample resolve at
  `COLOR_ATTACHMENT_OUTPUT` with `COLOR_ATTACHMENT_WRITE`, as the spec defines resolves).
- Limits: `MAX_PASSES` 12 → 32, `MAX_RESOURCES` 24 → 48 (R5 had raised them from 8/16; the
  pass-use mask becomes `u64`). A frame with everything on — both offscreen passes, the
  three VFX passes, shadows, AO, bloom with auto exposure, FXAA, an upscale and the capture
  — has 25 passes and 31 resources; a unit test compiles every topology against the limits.
  The GPU profiler's query pools grow with `MAX_PASSES`.

### 2. Tonemapping, exposure and grading
- **AgX stays** (ADR-0008), with its saturation look.
- **Exposure** is `RenderConfig::exposure` × `2^EV`, where EV is the map's compensation
  (`[post] exposure`, client-local map visuals) plus, when on, **auto exposure**.
- **Auto exposure (eye adaptation)** meters the last bloom mip (1/32 resolution): one
  fragment writes the 1×1 exposure image from an 8×8 grid of bilinear taps over that mip —
  the log-average luminance, centre-weighted, with non-finite texels counted as black (and
  the bloom down-samples treat them the same way). The target exposure `log2(0.18 / L)` is
  **clamped** to the map's `[exposure_min, exposure_max]` EV, then **blended** into the
  stored value with the fixed-function blend (`CONSTANT_COLOR`):
  `k = 1 − exp(−dt · adaptation)`. The first frame and every map change **clear** the image
  and use `k = 1`, so nothing (a NaN included) survives a reset. No compute and no
  read-back: the composite reads the 1×1 image. It is opt-in per map and needs the bloom
  down-chain (Medium and up; the chain runs for metering even when bloom itself is off). The
  image is an imported persistent resource (like R7b's offscreen images), **`R32_SFLOAT`**
  where that format can be blended into (desktop GPUs, lavapipe), else `R16_SFLOAT`. In half
  precision a small `k` (slow adaptation at a high frame rate: 0.004 at 240 fps) rounds the
  blend's step away and adaptation stalls up to ~0.6 EV short, so on `R16_SFLOAT` steps are
  deferred until `k ≥ 1/32` (the frame time accumulates), which bounds the residual to
  ≤ 0.03 EV below 4 EV (unit-tested at 240 fps for 60 s).
- **Grading** is a per-map **3D LUT** (`[post] grading = "<name>"` →
  `<asset root>/grading/<name>.cube`, the Adobe/Resolve text format, parsed and bounded by
  `moba_assets::GradingLut`), applied in the composite after the tonemap: it maps the
  display-encoded AgX output to display-encoded colour, blended by `grading_strength`. The
  LUT is uploaded once per map as an `R16G16B16A16_SFLOAT` 3D texture (size ≤ 64, sampled
  with the half-texel offset so trilinear filtering is exact on the lattice); without one a
  2³ identity is bound. `moba-tools grade-bake` generates the sample LUTs from documented
  lift/gamma/gain, contrast, saturation and white-balance parameters.
- **Team colours survive the grade** (ADR-0011 rule 1, now checked):
  `GradingLut::check_team_readability` holds a LUT, at the map's `grading_strength` and at
  its fixed EV ± 1 stop, to the standard red/blue unit tints and the colour-blind palette's
  orange/sky blue: the grade shifts no team hue by 20° or more, keeps each team's chroma
  ≥ 0.2 and keeps the two teams ≥ 90° apart in hue. `grade-bake bake` refuses a LUT that
  fails (`info` reports it) and the client, loading a map's grade, logs the rule and draws
  ungraded instead. The shipped grades pass with their maps' settings (unit-tested).
- **Why after the tonemap:** artists grade display images, `.cube` files from any grading
  tool assume display-referred input, and a [0, 1] domain needs no shaper LUT.

### 3. Bloom
A physically based down/up chain (Jimenez, "Next generation post processing in Call of
Duty: Advanced Warfare", SIGGRAPH 2014): five down-samples with the 13-tap filter (the first
with the Karis average, which removes single-pixel fireflies from GGX peaks), then up-samples
with a 3×3 tent, each added into the next larger mip by blending. The composite mixes the
result with the scene in linear HDR before exposure (`mix(hdr, bloom, strength)`, strength
0.04 by default, per map `[post] bloom`, **capped at 0.1** by the descriptor parser and the
renderer, since more washes team colours and silhouettes out), so bloom is energy
conserving: there is no threshold, and dim pixels contribute almost nothing visible. Five levels reach 1/32 of
the render resolution (60×34 at 1080p), which is wide enough at MOBA zoom; a sixth level
would add a pass for an invisible radius. **Low: off** (`MOBA_BLOOM=off` everywhere).

### 4. Anti-aliasing
Options measured in [perf/0019](../perf/0019-post.md); defaults:

| | Low | Medium | High | Ultra |
|---|---|---|---|---|
| AA | FXAA (fast preset) | FXAA | FXAA | MSAA 4× |

- **FXAA** (our implementation of Lottes' FXAA 3.11 algorithm, quality presets by tier) runs
  on the tonemapped, graded LDR image at render resolution, before the upscale and the UI: it
  sees perceptual luma, and the HUD is never filtered. It costs one fullscreen pass and no
  memory beyond an RGBA8 target, it is compatible with everything after the scene (R5
  particles included) and it works at any render scale.
- **MSAA 4×** renders the scene (and the AO prepass) into 4× colour and depth attachments,
  resolved at the end of the pass with dynamic rendering's resolve attachments (colour
  average, depth `SAMPLE_ZERO`). The resolved, single-sample HDR and depth are what R5's
  particles, AO and the post chain see, so none of them change. It needs a multisample copy
  of every main-view scene pipeline (portraits and the minimap keep the single-sample ones)
  and about 4 × (8 + 4) bytes per pixel: ~100 MB at 1080p, ~180 MB at 1440p, ~400 MB at 4K.
  It is the best edge quality and the only option that keeps thin geometry stable, so it is
  the **Ultra** default (RTX 4070-class, 12 GB+); on lower tiers the VRAM and bandwidth go
  further elsewhere.
- **Not chosen:** SMAA 1× needs the reference area/search textures (MIT) as assets and two
  extra passes; FXAA covers the cheap path and MSAA the quality path. **TAA** (optional and
  High-only in the roadmap) is not implemented: it needs motion vectors for skinned and VFX
  geometry and a history buffer, and its ghosting on fast units is exactly the readability
  hazard the roadmap warns about. Revisit with art direction.
- The multisampled colour is never stored (resolved, then `DONT_CARE`), so it is created
  `TRANSIENT_ATTACHMENT` (tilers can keep it on chip; gpu-allocator does not expose lazily
  allocated memory, so it is still backed). A depth access that is a resolve **source**
  (`DepthWriteResolveSource`: the prepass, or the scene without one) includes the resolve's
  `COLOR_ATTACHMENT_OUTPUT` read, so a later writer of the multisampled depth waits for it.
- `MOBA_AA=off|fxaa|msaa4`; a device whose **actual** HDR or depth format has no 4×
  sample count (`vkGetPhysicalDeviceImageFormatProperties`, not just the device-wide
  framebuffer limits) falls back to FXAA with one log line.
- R4b's VAT minions draw through the same per-sample-count pipeline sets as every other
  scene mesh, with a depth-only prepass twin (`vat.vert` declares `invariant gl_Position`,
  like `terrain.vert`, `unit.vert` and `skinned.vert`; a shader test checks all four).

### 5. Ambient occlusion
- **GTAO** (Jimenez et al., "Practical real-time strategies for accurate indirect
  occlusion", 2016) at **half render resolution**, High and Ultra: per pixel, 2 slices (3 on
  Ultra) with 4 steps on each side over a 1.5 m world radius, the cosine-weighted analytic
  integral, a 4×4 interleaved slice rotation and a 4×4 depth-aware blur that removes the
  pattern. Our formulation works in world space and needs nothing beyond the camera the
  scene already has: a pixel's position is the eye plus its camera ray (interpolated from the
  four frustum-corner rays at unit view depth) times the linear view depth recovered from the
  depth buffer. Unprojecting through the inverse view-projection instead loses most of fp32's
  precision near depth 1 and made the reconstructed normals of grazing ground noisy. Each
  slice's visibility is the cosine-weighted integral over its two horizons, weighted by the
  length of the normal projected into the slice plane and averaged over the slices (divided
  by the slice count, not by the summed lengths): over all slice directions an open
  hemisphere then integrates to exactly 1 for any normal, where normalising by the summed
  lengths biased tilted faces dark.
- **Normals are reconstructed from depth** (per pixel, the smaller of the left/right and
  up/down depth differences, so silhouettes do not smear). A forward renderer has no normal
  buffer; adding one would mean a second colour output on every scene pipeline (8 bytes per
  pixel at full resolution) and touching every scene shader, while AO at half resolution is
  low-frequency and does not need normal-map detail at MOBA zoom.
- **Applied to the ambient term only.** AO must be known while shading (direct light, rim
  and emissive must not darken), so AO needs depth **before** the scene: High and Ultra add
  a **depth prepass** of the main view (the scene's vertex shaders with no fragment stage;
  every scene vertex shader declares `invariant gl_Position`, so the scene pass passes
  `LESS_OR_EQUAL` exactly). The scene loads that depth and samples AO (set 3, binding 5) for
  the diffuse and specular ambient terms (specular occlusion from Lagarde & de Rousiers 2014).
  The prepass also removes shading overdraw. Offscreen views bind a white 1×1 image.
- **One sampler in the scene's budget.** The AO image is the only R6 binding a scene
  pipeline sees (one combined image sampler in the fragment stage). Bloom, exposure, the
  composite, FXAA, GTAO and the upscale run in their own passes with their own small
  descriptor sets, so they do not count against the scene pipelines'
  `maxPerStageDescriptorSamplers` (16 on the Vulkan minimum and MoltenVK, which set 3 and
  the material set approach once R3a and R2b land). Applying AO in the composite instead
  would remove that sampler but darken direct light (see Alternatives).
- **Composition with R3a and R2b.** R6 merges after R3a and before R2b. On `main` today AO
  is set 3 binding 5, the first free one; R3a takes set-3 bindings 5–9, so when R6 catches
  up with it AO moves to **binding 10** (`lighting.glsl` and the set-3 layout's binding
  list) and keeps R3a's per-stage sampler-budget check passing; both PRs append `vec4`s
  after `sh[9]` in `CameraBlock`, merged in the same std140 order as `FrameLighting`, and
  R3a's sky gets an MSAA twin through `view_pipelines` like every scene pipeline. R2b then
  appends its bindings after AO. **The frame has one depth prepass, R6's** (main view,
  High and up; multisample depth with a `SAMPLE_ZERO` resolve under MSAA): R2b's contact
  shadows reuse it rather than adding their own. `FrameSpec::prepass` requests it without
  AO (AO implies it), and an extra prepass output such as R2b's `R8_UINT` outline IDs is
  one more graph access and one colour attachment in `ScenePass::record_prepass`.
- **R3a's baked relief AO and GTAO** are not stacked on the terrain: on High and Ultra the
  terrain's ambient takes `min(baked, gtao)`, since both see the same crevices and their
  product would darken them twice; units take GTAO only. Low and Medium keep the baked
  term alone. (Implemented when R6 catches up with R3a.)
- **Low/Medium:** off; Low will rely on baked AO (R2b). `MOBA_AO=off`.

### 6. Resolution scaling
- **Render scale** (`MOBA_RENDER_SCALE=0.5..1`, settings `render_scale`): the scene and the
  post chain up to FXAA run at `scale × native`; one **upscale** pass writes the swapchain
  (bilinear on Low, Catmull-Rom bicubic on Medium and up: sharper than bilinear at 0.67–0.85,
  no ringing worth a clamp at these ratios), then the UI draws at native resolution.
- **Dynamic resolution** (`MOBA_DYNRES`, Low by default with a 14 ms GPU target): images are
  allocated at the maximum scale and each frame renders a sub-rectangle (viewport and
  scissor), so changing the scale never allocates. Every post shader maps its output pixel
  to the source sub-rectangle and clamps its taps to it. A controller reads the profiler's
  GPU total (two frames late), keeps an exponential average, steps the scale down by
  `sqrt(target / measured)` above 95 % of the target and up by 0.05 below 75 %, at most once
  every 15 frames, quantised to 1/32, within `[min_render_scale, render_scale]`.
- **FSR 1** (MIT) is **not** included: at the scales a MOBA needs (≥ 0.67 for readable
  units) the bicubic upscale is close, and EASU + RCAS is two more passes. It is a drop-in
  replacement for the upscale pass if measurements on the §7 Low hardware ask for it.

### 7. Gameplay effects
`Scene::effects` (`ScreenEffects`) carries three values the client sets each frame from data
it already has: **death desaturation** (0..1, while the local hero is dead), a **damage
vignette** (an edge flash scaled by the health lost in the latest snapshots) and a
**low-health pulse** (a vignette pulsing at ~1 Hz below 30 % health). The composite applies
them after grading, so they never touch the HUD. Nothing may blur team colours: death
desaturation stops at **0.6** (teams stay distinguishable while spectating), and the
vignette is a **neutral near-black** (not red, the default enemy colour, and not a hue that
fights the colour-blind palettes), only in the outer corners (from 0.6 of the half
diagonal) and at most **0.45** strong. Each has its own client setting
(`death_desaturation`, `damage_vignette`, `low_health_pulse`, all on by default) and
`MOBA_SCREEN_EFFECTS=off` turns them all off. They are cosmetic and client-local.

### 8. Tiers and switches

| | Low | Medium | High | Ultra |
|---|---|---|---|---|
| Exposure | map EV, fixed | map EV + auto (map opt-in) | same | same |
| Grading LUT | on | on | on | on |
| Bloom | off | on | on | on |
| AA | FXAA fast | FXAA | FXAA | MSAA 4× |
| AO (+ depth prepass) | off | off | GTAO, 2 slices | GTAO, 3 slices |
| Render scale / dynres | 1.0, dynres on (0.6–1.0, 14 ms) | 1.0, off | 1.0, off | 1.0, off |
| Upscale (when scaled) | bilinear | bicubic | bicubic | bicubic |
| Screen effects | on | on | on | on |

Switches (`RenderConfig::post`, `PostConfig::with_*`; the client reads the environment):
`MOBA_BLOOM=off|on`, `MOBA_AA=off|fxaa|msaa4`, `MOBA_AO=off|on`, `MOBA_RENDER_SCALE=0.5..1`,
`MOBA_DYNRES=off|on|<target ms>`, `MOBA_GRADING=off`, `MOBA_EXPOSURE=auto|fixed|<EV>`,
`MOBA_UPSCALE=bilinear|bicubic`, `MOBA_SCREEN_EFFECTS=off`.

**Captures and goldens:** with `MOBA_CAPTURE` (or a map-switch capture) set the client pins
auto exposure, dynamic resolution and the **screen effects** off (fixed exposure, the
configured render scale, no wall-clock-driven effect), unless `MOBA_EXPOSURE=auto`,
`MOBA_DYNRES` or `MOBA_SCREEN_EFFECTS_PREVIEW` is given explicitly, so two capture runs stay
byte-identical. Five goldens cover the post topologies: the default (High), the networked
frame, R4b's VAT minions, **Ultra** (MSAA 4× + GTAO, with VAT minions) and **Low at render
scale 0.75** (FXAA fast + the bilinear upscale).

## Alternatives considered
- **ACES-fitted tonemap:** rejected in ADR-0008 (mid-grey lift, hue skew of team colours).
- **Compute luminance histogram** for exposure: more robust metering (percentile clipping),
  but it needs compute passes, storage images and a buffer the composite reads; the 1/32 bloom
  mip already holds a well-filtered image. Revisit if art direction wants highlight-weighted
  metering.
- **Grading in linear HDR (before the tonemap) with a log shaper:** more physically correct,
  but artists' tools and `.cube` files assume display input.
- **Dual-filter (Kawase) bloom:** cheaper per pass, but blockier at our 5 levels; the 13-tap
  filter's cost is in texture fetches the hardware filters for free.
- **SSAO (normal-oriented hemisphere):** simpler, noisier and over-darkens creases; GTAO costs
  about the same at half resolution.
- **AO applied in the composite** (no prepass): darkens direct light and VFX, which breaks
  the "ambient only" rule and team-colour readability in shadowed corners.
- **AO from the previous frame's depth** (no prepass): ghosting under moving units.
- **Separate render-scale allocations per step:** allocation on the frame path; the
  sub-rectangle approach costs a clamp per tap.

## Consequences
- More passes: up to 16 post/AO passes with everything on (prepass to upscale, the
  composite included), each cheap at 1080p ([perf/0019](../perf/0019-post.md) has the
  per-pass numbers against the §7 budgets).
  `MOBA_FRAME_LOG` gets `prepass`, `ao` (GTAO + blur), `bloom` (every bloom pass),
  `exposure`, `fxaa` and `upscale` lanes.
- Transient VRAM added per tier at 1080p (render allocation = the window at scale 1):
  Low +17 MB (two RGBA8 LDR images: FXAA and the dynres upscale), Medium +11 MB (one LDR
  image, five `B10G11R11` bloom mips), High +14 MB (+ the half-resolution AO pair), Ultra
  +105 MB (the 4× colour and depth attachments, bloom, AO; no LDR image). At 1440p and 4K
  that is 30/20/24/187 MB and 66/44/55/420 MB. The LUT (17³ RGBA16F, 39 KB) and the 1×1
  exposure image are negligible.
- Both golden fingerprints were rebaselined deliberately (grading, bloom and FXAA change
  every pixel of the scene; the UI is unchanged).
- New public API (additive, `#[non_exhaustive]`): `PostConfig`, `AntiAliasing`,
  `UpscaleFilter`, `PostLook`, `ScreenEffects`, `RenderConfig::post`/`with_post`/
  `effective_post`, `Renderer::{post_config, set_post_look, set_grading_lut, render_scale}`,
  `Scene::effects`/`with_effects`, `GpuTimings::group_ms`; `moba_assets::{GradingLut,
  MapPost}`.
- No wire, replay, simulation-logic or content-hash change: map post settings and LUTs are
  client-local map visuals.
