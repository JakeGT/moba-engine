# ADR-0021: Temporal foundation, upscaling, the gameplay overlay layer and latency (N1)

**Status:** proposed 2026-10-31. This is phase N1 of the
[next-gen graphics roadmap](../plans/next-gen-graphics-roadmap.md) (§4, §5.3, §7, §8 and §12
step 3), written before the code.
- **Builds on:** [ADR-0020](0020-next-gen-foundations.md) (N0, proposed). That ADR settles the
  capability gate, the `Tiered`/`Compat` paths, Slang, bindless, the tier budgets and
  budget-driven dynamic resolution, and the vendor-SDK cargo features. This ADR uses all of
  them and does not re-decide them.
- **Amends, for the Tiered path only:**
  - [ADR-0015](0015-post-processing.md) §1, §4 and §6: the post chain moves after a temporal
    upscaler, and MSAA and FXAA leave the tiered presets;
  - [ADR-0016](0016-lights-outlines-ao.md) §2: outlines move to output resolution, and hull
    outlines become Compat-only;
  - [ADR-0017](0017-decals-foliage-water.md) §1: `Glow` decals (telegraphs and zones) move to
    the overlay layer;
  - [ADR-0013](0013-vfx-runtime.md): enemy-telegraph particles move to the overlay layer, and
    particles write the reactive mask;
  - [ADR-0020](0020-next-gen-foundations.md) §1e: `independentBlend` joins the tiered
    required set (§2e).
- **Compat is unchanged.** It keeps FXAA, MSAA 4× on Compat Ultra, hull outlines, decal
  telegraphs and today's frame (ADR-0020 §2d).

Vendor, version and licence claims were re-checked on 2026-10-31 against the sources in
[Sources](#sources). Facts that ADR-0020 already verified that day are cited as
"ADR-0020 [Sn]" and not re-derived. Where a claim could not be confirmed it says
**unverified**.

## Context

Roadmap §8 gives N1 six items:
- jitter and motion vectors for everything;
- TAA/TAAU;
- one upscaler interface over FSR 3.1.6, DLSS SR (NGX), FSR 4 and XeSS;
- the **post-upscale gameplay overlay layer** (outlines, telegraphs, the HUD);
- Reflex and Anti-Lag;
- retiring MSAA.

Readability (§4) is the hard constraint. Temporal reconstruction is the largest readability
hazard in the roadmap (§11, "ghosting from TAA, DLSS or FG on fast units"). So every decision
below is judged by whether a fast hero, a projectile or an enemy telegraph stays sharp and
in place.

What the code does today (`origin/docs/adr-0020-n0`, which is `main` plus ADR-0020):

- **No temporal state.**
  - Nothing is jittered, and nothing writes motion vectors.
  - No pass reads the previous frame's colour, except the fog-of-war ping-pong (R3a) and
    auto exposure (R6).
  - `FRAMES_IN_FLIGHT = 2` (`renderer.rs`).
- **Camera and picking.**
  - `moba_render::Camera` holds one `view_proj` (`scene.rs`), built by the client.
  - `input::cursor_ground` and `hovered_unit` (`crates/client/src/input.rs`) unproject the
    cursor through **that same matrix** onto `y = 0` and convert the hit to `FixVec2`.
  - That hit becomes `WireCommand::MoveTo`/`CastAbility` coordinates. The server applies them
    and records them in the replay (`moba-sim` `replay.rs` stores post-resolution inputs).
  - The renderer copies `view_proj` and its inverse into `CameraUniform` (`scene.rs`; the
    GLSL side is `CameraBlock` in `include/camera.glsl`). The VFX and distortion passes get
    their own copy in `VfxUniform::view_proj` (`passes/vfx.rs`), water reconstructs depth
    from `EnvironmentBlock::inverse_view_proj` (`environment.rs`), and GTAO builds its view
    rays from `PostFrame::view_proj` (`passes/post.rs`).
- **Instances have no identity.**
  - `UnitInstance` is a transform, a colour, a mesh, a `BoneRange` into this frame's
    `Scene::bones`, an outline slot and a `VatPose`.
  - The client rebuilds the list every frame (`scene_build.rs`). Positions come from
    `Interpolator::position(net_id, render_tick)` at a fractional render tick between the
    30 Hz snapshots. The own hero is predicted ahead, and teleports are flagged by
    `teleported_between`.
- **Animated geometry.**
  - Skinned meshes read this frame's joint palette.
  - VAT minions sample a `VatPose` (R4b).
  - Foliage bends with `environment.time` (two sine gusts) and up to 32 displacers built from
    the drawn units (`foliage.vert`).
  - Water normals scroll procedurally (`water.frag`).
  - Distortion offsets are procedural noise scrolled by `distortion_speed` (ADR-0019).
  - Particles are simulated on the GPU from position and velocity (`vfx_simulate.comp`), and
    they never write depth.
- **The frame** (`frame_graph.rs`), in order:
  - `prepass` (depth, plus `R8_UINT` outline IDs) at render resolution;
  - `scene`, which shades and composites `Glow` decals inside the terrain, water and foliage
    shaders (`apply_glow_decals`);
  - water, distortion and VFX;
  - bloom and exposure;
  - `composite`, then `outline` on the tonemapped `ldr` image **at render resolution**;
  - `fxaa`, then `upscale` (bilinear or bicubic) to the swapchain;
  - `ui` at native resolution.

  So today outlines and telegraphs are upscaled with the scene. The HUD (`ui`), health bars
  included, is already at output resolution.
- **Two telegraph forms.** `DecalRole::EnemyTelegraph` decals (`Glow` style, two-tone rim)
  and `ParticleRole::EnemyTelegraph` ring particles (ADR-0013's opacity floor).
- **Frame loop** (`crates/platform/src/app.rs`).
  - winit delivers the input events, then `RedrawRequested` calls `App::on_frame`.
  - That one call runs input, the `FixedStep` ticks, prediction, the scene build, and
    `Renderer::render_*` (acquire, record, submit, present).
  - A `max_frame_rate` limiter (default 240) sets `ControlFlow::WaitUntil`. `MOBA_VSYNC`
    picks FIFO, or else MAILBOX/IMMEDIATE (`swapchain.rs`).
- **Graph limits.** `MAX_PASSES = 40`, and `MAX_RESOURCES = 48` under a `u64` resource mask
  (`graph.rs`).

Constraints:
- **Presentation only.**
  - No change to `PROTOCOL_VERSION` (v42), replay `VERSION` (v5), `SIM_LOGIC_VERSION` (v9),
    content hashes or `tests/golden-state-hash.txt`.
  - Jitter must never reach picking, orders or any value that goes on the wire.
- **The R8 rules hold:** no per-frame allocation, `locked_uploads=0` and `locked_pipelines=0`.
  Upscaler contexts are created with the renderer, never mid-match.
- **The open build stays whole** (ADR-0020 §6c). With default features, Linux and Windows get
  a complete temporal path.
- **Determinism of captures.** Two lavapipe runs of a golden must stay byte-identical.

## Decision

### 1. Jitter

- **The sequence.** Halton (2, 3), index starting at 1, with offsets in render pixels in
  `[-0.5, 0.5)`.
  - **Phase count** = `ceil(8 × r²)`, where `r` is output ÷ render per axis, rounded to three
    decimals first (so 2560 ÷ 1707 counts as 1.5), recomputed every frame from the current
    render size. That gives 8 at native, 18 at 1.5×, **24** at 1.7× and at DLSS's 1.724×, 32 at
    2× and 72 at 3×.
  - **Rounding up is deliberate.** DLSS's table gives these values as minimums, 24 for
    Balanced [S1]. FSR's manual states `ceil(8n²)` for custom ratios but lists 23 at 1.7×,
    which is the truncated value [S2]. One extra phase is harmless to FSR and meets DLSS's
    minimum. XeSS asks for a Halton sequence whose length grows with the ratio [S5].
  - The index runs continuously and is taken modulo the current count, so a dynamic-resolution
    step does not restart the pattern.
- **Where it is applied.** Only inside `moba-render`, as a clip-space translation of the
  projection used for rasterisation: `+2·jx / render_w` and `−2·jy / render_h`, with the sign
  fixed by a unit test against the Vulkan y-down convention.
- **The rule for each matrix.** The render-resolution depth is jittered, so anything that
  rasterises into it, or reconstructs a position from it, uses the **jittered** matrix or its
  inverse. The unjittered matrices are used only to compute motion, by the output-resolution
  overlay (§5), and on the CPU.

| Consumer | Today | From N1 on Tiered |
|---|---|---|
| `CameraUniform` (Rust, `scene.rs`) / `CameraBlock` (GLSL, `include/camera.glsl`): `view_proj`, `inv_view_proj` | client matrix | jittered, and the inverse of the jittered matrix (scene, prepass, contact shadows, fog and decal reconstruction) |
| `CameraUniform`, new members | — | `view_proj_unjittered` and `prev_view_proj` (motion), `jitter` (current and previous, in pixels), `mip_bias` (below) |
| `VfxUniform::view_proj` (`passes/vfx.rs`: sprites, mesh particles, distortion sprites) | client matrix | jittered; gains `view_proj_unjittered` and `prev_view_proj` for particle motion (§2d). The billboard basis (`camera_right`, `camera_up`) stays unjittered |
| `EnvironmentBlock::inverse_view_proj` (`environment.rs`: water depth reconstruction) | inverse client matrix | inverse of the jittered matrix |
| GTAO's view rays (`PostFrame::view_proj`, `passes/post.rs`) | client matrix | jittered |
| Overlay prepass and overlay (§5) | — | unjittered, at output resolution, with its own depth |
| CPU: culling, decal and light binning, the distortion region (`distortion.rs`) | client matrix | unjittered (jitter is under one render pixel) |

- **Compat.** The jitter is zero and both matrices are equal. The members are new, so
  `camera.glsl` (shared with Compat) changes layout; Compat's **pixels** stay identical, and
  its goldens check that.
- **What jitter never touches:**
  - **`Camera` itself.** The client's `Camera::view_proj` is never jittered and never mutated
    by the renderer.
  - **Picking.** `cursor_ground`, `hovered_unit`, the HUD's world anchors (`hud/geometry.rs`)
    and the VFX billboard basis keep reading the unjittered client matrix.
  - **CPU culling** (`cull.rs`, `lights::ground_footprint`), which uses the unjittered matrix.
    Jitter is under one render pixel, and the bounding spheres already have more slack than
    that.
  - **Offscreen views** (portraits, minimap). They are not jittered and not temporal.
- **The jitter index is a presentation counter,** advanced once per presented main-view frame.
  It is never derived from the tick or the wall clock.
- **Captures.** `MOBA_CAPTURE` resets the index and the history, then renders a fixed
  warm-up: two full phase cycles at the pinned tick before the captured frame. The image is
  therefore a pure function of the scene and the settings.
- **Texture LOD bias.** Material texture samples on the Tiered path add a per-frame shader
  bias, `CameraBlock::mip_bias`. Its formula is set **per back end**, as each SDK documents:
  - **DLSS, FSR 3.1, FSR 4 and TAAU:** `log2(render_w / output_w) − 1`, which gives −1.58 at
    1.5×, −2 at 2× and −2.58 at 3× [S1][S2]. FSR 4 is assumed to follow FSR 3.1
    (**unverified**).
  - **XeSS:** `log2(render_w / output_w)`, with no −1: −1 at 2×, −1.202 at 2.3× and −1.585 at
    3× [S5].
  - **Temporal off:** 0.
  - It is per frame because dynamic resolution changes the ratio, and the DLSS and XeSS
    guides both say the bias must follow it [S1][S5].
  - The bindless sampler palette (ADR-0020 §3a) stays fixed. The bias is a `texture(…, bias)`
    argument.
  - Spec clamping to `maxSamplerLodBias` (at least 2) limits Ultra Performance to −2 on
    minimum devices.
  - LUTs, VAT, UI, data textures and every post pass take no bias.

### 2. Motion vectors and masks

**2a. Format and resolution.**
- `motion` is `R16G16_SFLOAT` at **render resolution**, in the render sub-rectangle.
- Each value is a screen-space offset **in render pixels from the current sample to its
  position in the previous frame**, without jitter. Every back end takes that directly, with
  a **scale of (1, 1)**:
  - **FSR 3.1** expects screen-space vectors in the range `±(width, height)`.
    `motionVectorScale` exists to convert other spaces; its render-size example is for NDC
    vectors [S2].
  - **XeSS** takes low-resolution vectors in pixels, R16G16_FLOAT, and dilates them itself
    [S5].
  - **DLSS** takes low-resolution vectors pointing to the previous frame, with
    `InMVScaleX/Y` = 1.0 when no conversion is needed [S1].
- FSR's manual notes that 16-bit precision is what its internals use anyway [S2].
- Each back end receives the same image, with its scale and flags set in its adapter (§4b).
- No back end gets jittered or display-resolution motion vectors. The FSR and XeSS flags for
  those stay off.
- **Depth** is today's `D32_SFLOAT`, non-inverted and with a finite far plane. Every back end
  has flags for inverted depth [S1][S2][S5]. Reversed Z is left to N2, whose G-buffer
  rewrite touches every depth consumer anyway.

**2b. Who writes motion vectors.** The render-resolution `prepass`, which every tiered
preset already has (tiered Low runs today's High configuration, which has the prepass;
ADR-0020 §5e), gains the `motion` colour attachment. Every opaque main-view draw goes through
it, and the scene pass keeps loading its depth with `LESS_OR_EQUAL`. N2's G-buffer pass
takes the attachment over (roadmap §5.2, step 1). Each source works as follows:

| Source | Previous position |
|---|---|
| Terrain, walls, props, structures (motion key 0) | `prev_view_proj × model × p`: camera motion only |
| Units and static meshes with a key | the previous model matrix from the motion history (§2c) |
| Skinned | previous joint palette × previous model matrix (§2c) |
| VAT minions | the previous `VatPose` sampled in the same vertex shader. The prepass's VAT permutation fetches twice; the scene's does not |
| Foliage | `foliage.vert` evaluated a second time at `time − dt` with the previous frame's displacers. The foliage uniform keeps both. **Foliage joins the prepass**, so GTAO and contact shadows now see brush (a tiered-only visual change) |
| Sky and background (depth = 1) | a final fullscreen draw in the prepass at the far plane: the view direction through `prev_view_proj`, which gives camera rotation only |
| Particles | see §2d |
| Water, distortion, `Paint` decals | static geometry, so no motion of their own. Their changing shading goes into the masks (§2d) |
| `Glow` decals, outlines | not in the scene; drawn post-upscale (§5) |

**2c. Previous transforms: motion keys and the history.**
- **The key.** `UnitInstance` gains `motion_key: u32` (0 means static, with no history) and
  `motion_reset: bool`.
  - The client sets `key = net_id << 8 | part` for every unit part.
  - It sets key 0 for map instances.
  - It sets `motion_reset` on the first frame a unit is drawn, and when
    `teleported_between(net_id, previous_render_tick, render_tick)` is true. That query covers
    respawns and recall completions only (`is_teleport`, `crates/net/src/interpolation.rs`).
    Blinks and dashes are interpolated like any movement, so they get exact motion vectors and
    need no reset.
  - **The client keeps one new value for this:** the previous frame's `render_tick` (one
    `f32`).
  - **A renderer-side guard** also treats a key as reset when its drawn position moved more
    than 20 m since the previous frame. No interpolated move can do that, so it only catches
    teleports the client missed.
- **The history.** The renderer keeps a fixed-capacity `MotionHistory` (open addressing,
  4,096 entries, allocated at creation). Each entry maps a key to the **transform, joint
  matrices and VAT pose the renderer drew on the previous presented frame**.
  - An entry not seen this frame is dropped.
  - A key that is missing, or has `motion_reset`, uses its current state as its previous
    state.
- **What the history holds is what was displayed,** not a re-query of the interpolator at
  `render_tick − dt`. The 30 Hz snapshots are interpolated (and the own hero predicted) per
  presented frame, so the drawn path is piecewise-linear between snapshots and bends at
  prediction corrections. The motion that the reprojection needs is exactly that drawn path,
  so the history captures it whatever the frame rate or the tick phase. Apart from the
  previous render tick above, the client needs no new state.
- **Upload.**
  - `GpuInstance` stays **128 bytes** (pinned by `instances_are_128_bytes_with_the_outline_slot_last`).
    The previous state goes into a parallel per-frame **previous-instance buffer**, indexed
    like the instances: a 3×4 previous model matrix and the previous VAT samples (64 bytes).
    Only the prepass's vertex shaders read it.
  - This frame's bone buffer gets the previous palettes **copied in by the CPU**, next to the
    current ones.
  - Reading last frame's slot instead would race: with two frames in flight, frame N+1
    rewrites the slot that frame N, still on the GPU, reads as "previous".
  - The copy costs one more palette upload per skinned unit (about 30 KB for ten heroes).
- **Reset of everything.** `Scene::history_reset` (set by the client on spectator switches,
  minimap camera jumps, map loads and resizes) clears the motion history and every temporal
  history. The renderer also resets automatically when the eye moves more than 8 m, or the
  view turns more than 15°, in one frame.

**2d. Reactive and transparency masks.**
- **The targets.** `reactive` and `tc_mask` (transparency and composition) are `R8_UNORM` at
  render resolution, cleared each frame. That matches FSR 3.1's masks; DLSS and XeSS take
  the equivalents, or ignore them (§4b) [S1][S2][S5].
- **Particles.**
  - `vfx_draw` writes `reactive` with `MAX` blending:
    - alpha particles write their alpha;
    - additive particles write `min(1, luminance × k)`.

    Both are clamped to 0.9, as FSR advises; XeSS clamps at 0.8 itself [S2][S5].
  - **Dominant particle fragments also write motion vectors.** A fragment whose opacity is
    at least 0.5 (projectile cores, opaque mesh particles) writes its own motion into
    `motion`. The blend is `SRC_ALPHA`/`ONE_MINUS_SRC_ALPHA` with an output alpha of
    `step(0.5, α)`, so the result is an exact replace or an exact keep, with no `discard`.
  - The previous position is `position − velocity·dt`. That inverts the integrator's position
    step; whether the inversion is exact is to be confirmed against `vfx_simulate.comp`, and if
    it is not, the 96-byte record stores the previous position.
  - **Why this matters:** DLSS's reactive-mask equivalent (`BiasCurrentColor`) "should not be
    used" with its current models [S1]. So for DLSS, motion vectors are the only defence
    against particle ghosting.
- **Distortion** (ADR-0019) writes `reactive` from `|offset| / cap`.
- **`tc_mask`.** A small `tc_mask` raster pass after the VFX draws two kinds of surface
  depth-tested into the mask (`MAX` blending): water surfaces (flow-scrolled normals and
  refraction) and any material flagged `animated_uv`.
- **Required feature.** The extra attachments blend differently from the HDR and OIT targets,
  so the Tiered path **requires `independentBlend`** (§2e).
- **Disocclusion** is detected by each back end. TAAU (§3) keeps the previous frame's linear
  depth for it. The application supplies no disocclusion mask, since none of the back ends
  asks for one.

**2e. Amendment to ADR-0020 §1e.** The tiered required set gains `independentBlend` (a core
feature). It is expected on every RT-capable desktop driver, but that is unverified per
device until the reference fixtures (ADR-0020 §2b) record it. A device without it is Compat.

### 3. In-house TAA/TAAU: the baseline on every tier

- **Where it runs.** `temporal_taau.slang`, one compute pass at **output** resolution after
  `vfx_resolve` and `tc_mask` and before bloom. It runs on every tiered tier and is never
  used on Compat. As "TAA" it runs at native resolution (ratio 1.0); as "TAAU" it runs at
  any ratio from 1.0 to 3.0.
- **The algorithm.** Karis 2014 (SIGGRAPH Advances) with Playdead's INSIDE TAA as reference.
  Playdead's TAA is MIT [S10]; it is a reference only and no code is copied.
  1. **Dilated motion.** The motion vector of the nearest-depth pixel in a 3×3 cross keeps
     the edges of moving units attached to the unit.
  2. **History fetch.** A 5-tap Catmull-Rom of the output-resolution history at the
     reprojected position.
  3. **Upsampled current sample.** The render samples around the output pixel are weighted by
     a Blackman-Harris kernel of their *jittered* distance. The total weight is also the
     pixel's confidence.
  4. **Neighbourhood clip.** A variance clip in YCoCg (γ 1.0, or 1.25 on static pixels),
     clipping toward the mean.
  5. **Blend.** Luminance-weighted (`1/(1+L)`) to stop flicker from fireflies.
- **History.**
  - `RGBA16F` at output resolution, a persistent ping-pong pair indexed by frame parity, plus
    the previous linear depth (`R32F`, render resolution, ping-pong) for disocclusion.
  - Both are imported persistent resources, like the fog-of-war images.
  - A reset (§2c), a resize or the first frame writes the upsampled current colour with
    weight 1.
- **Ghosting rules,** tuned for fast MOBA units and VFX:
  - **Base history weight:** 0.9 at native, rising with the ratio to 0.95 at 2×. The current
    sample's confidence lowers it.
  - **Velocity weighting.** The weight falls linearly to 0.8 at 8 output pixels per frame of
    motion. A hero dashing 10 m in 0.2 s at 1440p and 120 fps moves about 11 px per frame.
  - **Disocclusion.** Where the reprojected previous depth differs by more than 1 %
    (relative), the history is rejected and the pixel restarts from the current sample.
  - **Reactive pixels.** The current sample's weight is at least the reactive value.
  - **`tc_mask` pixels.** The clip box narrows: γ is lowered by `0.5 × mask`.
  - **Telegraphs and outlines never enter the history** (§5), so none of these rules is what
    keeps them readable.
- **Sharpening.**
  - RCAS (MIT, FidelityFX) runs on the output in a reversible tonemapped space. For TAAU it
    is fused into the same pass.
  - One `sharpness` setting (0–1) is shared by every back end:
    - FSR 3.1 uses its built-in RCAS [S2];
    - DLSS and XeSS get our separate `sharpen` pass, since DLSS's own sharpening is
      deprecated [S1].
  - Defaults: 0.25 for TAAU and FSR 3.1, 0 for DLSS and XeSS. These are starting points; the
    readability review tunes them.
- **Debug views:** `MOBA_DEBUG_VIEW=motion|reactive|tc|disocclusion|confidence|overlay-ids`,
  shipped with N1 (roadmap §4, rule 8).

### 4. The upscaler interface

**4a. One trait.** All back ends sit behind one trait in `crates/render/src/upscale/`,
chosen at renderer creation. Vendor code stays behind `vendor/` (ADR-0020 §6a), and call
sites never `cfg`.

```rust
pub(crate) trait TemporalUpscaler {
    fn kind(&self) -> UpscalerKind;               // Taau, Fsr3, Dlss, Fsr4, Xess
    fn render_size_range(&self, output: Extent, mode: UpscaleMode) -> (Extent, Extent);
    fn jitter_phases(&self, render: Extent, output: Extent) -> u32;
    fn has_sharpening(&self) -> bool;
    fn record(&mut self, cmd: vk::CommandBuffer, inputs: &UpscaleInputs, out: &UpscaleOutput);
    fn reset_history(&mut self);
    fn vram_bytes(&self) -> u64;
}
```

- **`UpscaleInputs`** carries, all at render resolution in the sub-rectangle:
  - the jittered linear HDR colour;
  - depth;
  - `motion`, `reactive` and `tc_mask`;
  - a 1×1 `R32_SFLOAT` **linear exposure multiplier**. Today's 1×1 image holds the adapted
    **EV** (log2), possibly as `R16_SFLOAT` (`frame_graph.rs`), and the composite applies
    `push.exposure × exp2(EV)` (`composite.frag`). So a new one-texel `exposure_linear`
    compute pass, just before `temporal`, writes that product into a separate `R32_SFLOAT`
    image (or `push.exposure` alone when auto exposure is off);
  - the jitter in render pixels;
  - the render size this frame;
  - `dt` in milliseconds;
  - the camera's near and far planes and vertical field of view. `Camera` holds only
    `view_proj` today, and `Camera::orbit` hard-codes 60°, 0.1 and 1,000 (`scene.rs`). So
    `Camera` gains `fov_y`, `near` and `far`, set by `perspective` and `orbit` (0 for the
    orthographic offscreen views, which never upscale);
  - the reset flag;
  - the sharpness.
- **`UpscaleOutput`** is the `hdr_out` storage image (`RGBA16F`, output resolution).
- **The exposure lags by one frame under auto exposure.** The exposure pass meters bloom's
  last mip, which is built from `hdr_out`, so it runs after `temporal`. The multiplier the
  upscaler sees is therefore the previous frame's adapted value, while the composite uses
  this frame's.
  - The difference is one adaptation step, `k = 1 − exp(−dt · adaptation)` of the gap (ADR-0015
    §2), which is small at gameplay frame rates.
  - It is **zero with fixed exposure**, which is the gameplay default (roadmap §4, rule 5:
    exposure locked or limited in play).
  - DLSS wants the current frame's value [S1]. Its presets L and M auto-expose anyway (L
    verified, M **unverified**: the guide names only L) [S1].
- **Inside the render graph,** the upscaler is one pass. The graph moves each input to
  shader-read and the output to general before it, as the SDKs require [S1][S2]. The back
  end records only its own dispatches.
- **Creation.** Contexts are created with the renderer at the **maximum** render size of the
  dynamic-resolution range. They are re-created with it on resize or a settings change,
  never mid-match.
  - DLSS presets L and M allocate for the creation size, and reallocate with "severe frame
    time spikes" if an input exceeds it [S1].

**4b. Back ends.**

| Back end | Feature | Platforms | Inputs used | Modes (render ÷ output per axis) | Notes |
|---|---|---|---|---|---|
| **TAAU** (in-house, Slang) | always built | Linux, Windows, every tiered device | colour, depth, motion, reactive, tc, exposure | any, 1.0–3.0 | The baseline and the final fallback. Used in goldens |
| **FSR 3.1.6** | `fsr3` (default on) | Linux, Windows; any vendor | colour, depth, motion (scale 1, 1), reactive, T&C, exposure | NativeAA 1.0, Quality 1.5, Balanced 1.7, Performance 2.0, Ultra Performance 3.0 [S2] | Our port of the MIT shaders under `third_party/fidelityfx/` (ADR-0020 §6a, Q9). The SDK build itself is Windows-only. Its manual puts tonemapping and bloom after FSR [S2] |
| **DLSS SR** (NGX direct) | `dlss` | Linux `.so`, Windows `.dll`; NVIDIA RTX (ADR-0020 [S16]) | colour, depth, motion (`InMVScale` 1, 1), jitter, exposure (presets J/K only; preset L always auto-exposes; M not stated, so **unverified**) [S1] | Ultra Performance 3.0, Performance 2.0, Balanced 1.724, Quality 1.5, DLAA 1.0 [S1] | No reactive mask: `BiasCurrentColor` must not be used with the current models [S1]. Default presets per the 310.6.0 guide: K (DLAA, Quality, Balanced), M (Performance), L (Ultra Performance) [S1]. Must run before tonemapping [S1]. Streamline is not used for SR |
| **FSR 4.1.2** | `fsr4` | **Windows only**; RX 6000+ (ADR-0020 [S22]) | as FSR 3.1, through the FidelityFX API | as FSR 3.1 | The signed DLL is loaded with `libloading`. The FidelityFX API picks an effect version at context creation (`ffxQueryDescGetVersions`) [S3]. That FSR 4 is reached through the same Vulkan upscale descriptors as FSR 3.1 is consistent with the v2.4.0 notes [S4] but **unverified** line by line. Licence ambiguous: development-only (ADR-0020 Q6) |
| **XeSS SR 3.0.2** | `xess` | **Windows only** (the guide lists Windows 10/11 only [S5]); Intel, plus DP4a elsewhere | colour, motion (low-res, R16G16_FLOAT), depth, exposure, responsive mask (clamped at 0.8) [S5] | XeSS 1.3+ names: Native AA 1.0, Ultra Quality Plus 1.3, Ultra Quality 1.5, Quality 1.7, Balanced 2.0, Performance 2.3, Ultra Performance 3.0 [S5] | `xessVK*` needs Vulkan 1.1, `shaderStorageImageWriteWithoutFormat` and `mutableDescriptorType` [S5]. Its instance and device extensions are declared before device creation (ADR-0020 §1e) |

- **One naming scheme.** The menu always shows our five modes by **ratio**: Native 1.0,
  Quality 1.5, Balanced 1.7, Performance 2.0, Ultra Performance 3.0. FSR's manual asks for
  consistent naming [S2].
  - XeSS gets its preset with the same ratio: our Quality is XeSS "Ultra Quality". Each
    frame it is also given the explicit input size, which XeSS accepts within its min–max
    range [S5].
  - DLSS gets the nearest NGX mode; its optimal-settings query returns the render-size range.
- **Per-tier defaults** follow ADR-0020 §5b:
  - Quality on Low, Medium and High;
  - Ultra: Quality at 1440p, Performance at 4K;
  - Overdrive: Performance (Ray Reconstruction arrives in N9).
- **Back-end order under `auto`.** It takes the first back end that is compiled in and
  supported by the device:
  1. DLSS on NVIDIA RTX;
  2. FSR 4 on RDNA 2+ under Windows, once it has been verified on AMD hardware ([verification gate 5](#verification-and-evidence-gates)) and
     its licence is settled (ADR-0020 Q6);
  3. XeSS on Intel Arc;
  4. **TAAU** (see Q1 for when FSR 3.1 takes this slot).

  FSR 3.1 is always selectable.
- **Fallbacks.** A back end that fails to load (a missing DLL, an NGX init error, an
  unsupported mode) falls to the next one with one log line. The device is never rejected,
  and the frame never runs without temporal AA unless the player turned it off.
- **The AA switch.** `MOBA_AA` accepts `temporal|fxaa|off|msaa4`. Today's parser
  (`AntiAliasing::from_name`, used by `crates/client/src/app/launch.rs`) accepts
  `off|fxaa|msaa4`; `temporal` is added with `AntiAliasing::Temporal`.
  - **`temporal`** is the tiered default.
  - **`fxaa` and `off`** are the off switch on Tiered: no jitter, no history, and today's
    render-resolution chain (composite → `ldr`, FXAA when chosen, bicubic `upscale`). The
    overlay still runs at output resolution, after the upscale (§5a).
  - **`msaa4`** is Compat-only. On Tiered it is refused with one log line (§7).
- **Settings and logs.**
  - `MOBA_AA=temporal|fxaa|off|msaa4` (above);
  - `MOBA_UPSCALER=auto|taau|fsr3|dlss|fsr4|xess`;
  - `MOBA_UPSCALE_MODE=native|quality|balanced|performance|ultra-performance`;
  - `MOBA_SHARPNESS=0..1`;
  - frame logs gain `upscaler=`, `render_scale=` and `jitter_phases=`.

**4c. Dynamic resolution** (ADR-0020 §5c).
- The controller and its range are unchanged from ADR-0020 §5c, and this ADR does not amend
  them:
  - the ceiling is the preset mode's scale;
  - where the ceiling is Quality (0.67), the floor is Performance (0.50), two modes down,
    skipping Balanced;
  - where the ceiling is already Performance (Ultra at 4K, Overdrive), the floor is Ultra
    Performance (0.33), one mode down.
- Each frame's render size goes to the back end. Jitter phases and `mip_bias` follow it.
- **DLSS.** The controller's floor is clamped to the minimum that NGX reports. If NGX
  reports no dynamic-resolution support (minimum = maximum) for a mode, dynamic resolution is
  off while DLSS runs, and the frame log says so [S1].
- **XeSS** accepts any input size between the minimum and maximum given at init [S5].
- **FSR 3.1.** That its dispatch accepts any render size up to the context maximum is
  **unverified** (the manual pages read do not cover it); the port settles it.
- **Aspect ratio.** The render aspect ratio always equals the output's. DLSS requires it, and
  XeSS recommends it [S1][S5]. The controller scales both axes together.

### 5. The post-upscale gameplay overlay layer

Roadmap §4 rule 1: outlines, team rims, telegraphs, range indicators, selection and the HUD
are drawn **after upscaling, at output resolution**, and never enter any history.

**5a. The tiered frame from N1:**

```text
(portraits, minimap) → (fow) → vfx_simulate → (shadow)
→ prepass         render res, jittered: depth + motion (+ unit mask when distortion is on)
                  terrain, units, skinned, VAT, foliage; far-plane background-motion draw
→ (contact_shadows, ao, ao_blur)
→ scene           render res, jittered; no Glow decals, no hull outlines
→ (water_copy → water) → (distortion_copy → distortion [+ reactive])
→ vfx_draw [+ reactive, + dominant-particle motion; telegraph particles excluded] → (vfx_resolve)
→ tc_mask         water surfaces and animated-UV materials → tc_mask
→ temporal        TAAU | FSR 3.1 | DLSS | FSR 4 | XeSS → hdr_out (output res) [→ sharpen]
→ bloom_d*, exposure, bloom_u*   from hdr_out
→ composite       AgX, grading, screen effects → ldr (output res)
→ overlay_prepass output res, unjittered: terrain, water surfaces, walls, props, units
                  → overlay depth (D32) + overlay IDs (R8_UINT)
→ overlay         ldr + overlay depth/IDs + decal bins + outline palette → swapchain:
                  Glow decals (telegraphs, zones, range indicators), outlines, selection
→ overlay_vfx     enemy-telegraph particles, depth-tested against the overlay depth
→ ui → (capture)
```

With temporal off (`MOBA_AA=fxaa|off`), the middle of the frame is today's chain: `scene` …
`vfx_resolve`, then bloom and exposure at render resolution, `composite` → `ldr`, `fxaa` and
`upscale` → an output-resolution `ldr_out`. The overlay passes then read `ldr_out` and write
the swapchain exactly as above, so the overlay never depends on the AA mode.

- **Composite moves to output resolution.** Bloom, exposure and composite now run on
  `hdr_out`. FSR places tonemapping and bloom after the upscaler [S2], and DLSS must run
  before tonemapping [S1].
- **Team rims stay in the lighting** (rule 2, N2). They are lighting, and they reconstruct
  with the scene.
- **The overlay prepass has no data dependency on the scene,** so the graph may schedule it
  anywhere after the shadow pass.

**5b. What moves, and from where.**

| Item | Today | From N1 on Tiered |
|---|---|---|
| Enemy and ally telegraph decals, zones, future range indicators (`DecalStyle::Glow`) | composited in the terrain, water and foliage shaders, in HDR before the tonemap | the `overlay` fullscreen pass. Each output pixel's ground position comes from the overlay depth. Decals are evaluated with the same 32×32 ground bins and `decals.glsl` shape code. The scene shaders' `apply_glow_decals` is compiled out of the tiered permutations |
| Enemy-telegraph ring particles (`ParticleRole::EnemyTelegraph`) | `vfx_draw` into HDR | `vfx_simulate` compacts them into their own list, which `overlay_vfx` draws at output resolution with the composite's exposure and AgX applied in-shader (as the UI's `MODE_IMAGE` does), so they look the same |
| Team, hover and selection outlines (slots 1–6) | `outline` at render resolution before FXAA and the upscale; hull outlines on Low and Medium | the `overlay` pass dilates the **overlay IDs** at output resolution. Widths are output pixels, with no render-scale factor. Hull outlines are Compat-only |
| Health bars, HUD, minimap, portraits | `ui` at native resolution | unchanged |
| `Paint` decals (scorch, splats) | in the albedo | unchanged: surface detail, not readability |

**5c. Depth for the overlay: a full-resolution, unjittered overlay prepass.**
- **What it draws.** A depth pass at output resolution with the **unjittered** matrix,
  writing `D32_SFLOAT` plus an `R8_UINT` class/ID, for:
  - the terrain, as class `GROUND`;
  - the water surfaces, as class `GROUND`, so a telegraph lands on the surface as it does
    today;
  - walls, props and structures, as `OCCLUDER`;
  - every unit: its outline slot, or `OCCLUDER` when it has none.
- **What it leaves out.** Foliage (decorative; brush is at most 1.4 m and the simulation has
  none), particles and the sky.
- **Telegraphs** draw only on `GROUND` pixels. So a unit standing in a telegraph still covers
  it, as "units never take decals" requires today (ADR-0017 §1).
- **Outlines** draw only the exterior of a slot's silhouette, as today.
- **Hidden information.** The ID is depth-tested against walls and terrain, so a unit fully
  behind a wall has no ID pixels and no outline (ADR-0016 §2). `scripts/outline-probe.sh`
  gains a tiered run.
- **Telegraph contrast moves to display space.**
  - The rim rule (luminance at least 1.8× the background's plus 0.05, a dark halo at 20 %,
    ADR-0017 §1) is evaluated against `ldr`, the exact displayed background.
  - `glow_reference`'s CPU twin and its tests are re-derived for display-referred input.
  - Telegraphs no longer take bloom, the grade or the screen effects (death desaturation, the
    vignette). Losing bloom is a visible change. The overlay draws its own fixed soft halo
    instead (Q4).
- **The overlay IDs depend only on geometry and the unjittered camera.** That makes
  **overlay coverage identical whichever back end runs**, which is what the readability test
  checks ([verification gate 3](#verification-and-evidence-gates)).
- **Distortion's unit mask** (ADR-0019 §3) stays at render resolution. The jittered prepass
  keeps a 1-bit unit mask when distortion is on, because the distortion pass is jittered too.

### 6. Latency: Reflex and Anti-Lag

**6a. Back ends.** A `LatencyBackend` trait (roadmap §7) has three implementations. Both
extensions are default-build items (ADR-0020 §6a), and both are on by default when present.

- **NVIDIA: `VK_NV_low_latency2`** (revision 3, last modified 2026-08-12) [S6].
  - It needs Vulkan 1.2 or `VK_KHR_timeline_semaphore`, and `VK_KHR_present_id` or
    `VK_KHR_present_id2` [S6].
  - The swapchain is created with `VkSwapchainLatencyCreateInfoNV`. Every submit chains
    `VkLatencySubmissionPresentIdNV`, which only revision 3 respects [S6].
  - `vkSetLatencySleepModeNV` sets `lowLatencyMode`, `lowLatencyBoost` and
    `minimumIntervalUs`. `vkLatencySleepNV` enforces `minimumIntervalUs` even when low-latency
    mode is off [S6].
  - All of these exist in `ash` 0.38. The structures were checked in its generated
    `definitions.rs`.
- **AMD: `VK_AMD_anti_lag`** (revision 1, 2024-06-06) [S7].
  - It provides `vkAntiLagUpdateAMD` with `VkAntiLagDataAMD { mode, maxFPS,
    pPresentationInfo { stage, frameIndex } }`. The modes are
    `DRIVER_CONTROL/ON/OFF` and the stages `INPUT/PRESENT` [S7].
  - It needs the `ash` git pin (ADR-0020 §1b).
- **Null:** no latency extension.
- **When both are present, NVIDIA's wins.** On this machine Mesa 26.2's
  `VK_LAYER_MESA_anti_lag` implicit layer also advertises `VK_AMD_anti_lag` on the NVIDIA
  device (local `vulkaninfo`).
- **Mesa 26.3 deletes that layer.** Its MR !42048 ("radv: VK_AMD_anti_lag", merged
  2026-10-08) adds a shared `vk_frame_pacer` framework, implements `VK_AMD_anti_lag` natively
  in RADV, and includes the commit "vulkan: delete anti-lag layer" (`b99595dedb85`). That
  commit is on the `26.3` branch and in tags `mesa-26.3.0-rc2` and `rc3` [S8].
  - So from Mesa 26.3, `VK_AMD_anti_lag` exists only on Mesa drivers that implement it (RADV
    first), and the NVIDIA proprietary driver offers only `VK_NV_low_latency2`.
  - The selection rule above covers both cases. The back end never depends on the layer.

**6b. Frame pacing and the frame loop.**
- **The sleep.** It must happen before input is gathered, and exactly once between presents
  [S6][S7].
  - winit delivers input events before `RedrawRequested`, so the sleep cannot sit at the top
    of `on_frame`: the events would already be stale.
  - Instead `moba-platform`'s `Runner::update` calls a new `App::pace()` hook **after**
    `on_frame` returns, so after this frame's present.
  - The client implements it with `Renderer::latency_sleep()`:
    - Reflex: `vkLatencySleepNV`, then `vkWaitSemaphores` on its timeline semaphore;
    - Anti-Lag: `vkAntiLagUpdateAMD` at stage `INPUT`.
  - The OS queues the events that arrive during the sleep, and winit delivers them before
    the next `RedrawRequested`. The next frame therefore samples the freshest input.
  - `moba-platform` stays Vulkan-free; the hook is a plain trait method.
- **Markers.** For Reflex, using the frame's present ID (one `u64` counter owned by the
  renderer):
  - `SIMULATION_START` and `INPUT_SAMPLE` at the top of `on_frame`;
  - `TRIGGER_FLASH` on a left click;
  - `SIMULATION_END` after the scene build;
  - `RENDERSUBMIT_START` and `RENDERSUBMIT_END` inside `Renderer::render_*`, around
    recording and `vkQueueSubmit2`;
  - `PRESENT_START` and `PRESENT_END` around `vkQueuePresentKHR` [S6].

  Anti-Lag gets its `PRESENT` stage call before the present, with the same frame index [S7].
- **The 30 Hz simulation is unaffected.**
  - `FixedStep` still turns wall-clock deltas into whole ticks, so the sleep only moves
    *when* a frame starts.
  - Network receive and send stay in `on_frame`. The sleep is bounded by one frame interval,
    so a snapshot waits at most that long, as it does under today's frame limiter.
  - The gain is the render queue: the cursor, the camera, the predicted own hero and the
    click feedback reach the screen sooner. Server round trips do not change.
- **One limiter, not two.** With Reflex or Anti-Lag active, the platform's `WaitUntil`
  limiter is disabled. Today nothing can do that: `Runner::new` fixes `frame_interval` from
  `WindowConfig::max_frame_rate` (`crates/platform/src/app.rs`).
  - So `App::pace()` returns a `Pacing` value: `Platform` keeps the `WaitUntil` limiter, and
    `External` means the app paced the frame itself, so the next frame is due at once.
  - The default implementation returns `Platform`, so other apps are unchanged. `max_frame_rate` becomes `minimumIntervalUs = 10⁶ / max_frame_rate`
  (Reflex) or `maxFPS` (Anti-Lag) [S6][S7].
- **Settings and evidence.**
  - `MOBA_LATENCY=off|on|boost`. Boost sets `lowLatencyBoost` and is off by default.
  - Frame logs record `vkGetLatencyTimingsNV` (simulation, render-submit, present, GPU and
    total) where available, as roadmap §3's latency reporting needs.
- **Frame generation is N9.** There, Reflex moves to `sl.reflex` on Windows (roadmap §7).

### 7. Retiring MSAA

- **When.** In the N1 change that makes the temporal pass the tiered default, not before
  (ADR-0020 §5e). Until then tiered Medium and up keep MSAA 4×.
- **What replaces it:**

| Path | Low | Medium | High | Ultra | Overdrive |
|---|---|---|---|---|---|
| Tiered | TAAU or vendor, Quality | same | same | same; Performance at 4K | Performance (+ RR in N9) |
| Tiered, temporal off | FXAA + bicubic | same | same | same | same |
| Compat (frozen) | FXAA fast | FXAA | FXAA | **MSAA 4×** (unchanged) | clamps to Compat Ultra |

- **The code.**
  - On the Tiered path, `AntiAliasing::Msaa4` is refused with one log line (temporal is used
    instead), and `AntiAliasing` gains `Temporal`.
  - The tiered renderer creates no multisample pipelines and no 4× attachments.
  - The MSAA code stays because Compat uses it.
  - Compat's MSAA and hull-outline paths freeze with Compat after C1.
- **Goldens.**
  - Every Compat golden, the MSAA-on Ultra one included, is captured with
    `MOBA_RENDER_PATH=compat` and stays **byte-identical**. N1 changes no Compat pixel. The
    `mip_bias` and jitter fields are zero there; any difference fails the PR.
  - New tiered goldens on lavapipe, at small resolution with the §1 warm-up:
    - TAAU at Low (Quality) and at High;
    - TAAU at native;
    - FSR 3.1 at Quality;
    - the overlay over moving and occluded units. A single pinned tick has no motion, so this
      golden pins **two** ticks: the warm-up runs at tick `n`, then the captured frame is at
      tick `n + 1`, with the interpolation and animation pinned to each. This extends today's
      single-tick capture mode (`app.rs`) to a two-tick script. Units move by one tick's step, and the camera
      pans by a scripted fixed offset between the two.
  - Vendor back ends are not in goldens: they are not on lavapipe, and DLSS models update
    over the air (ADR-0020 §6a).
- **The perf gate.**
  - Tiered budgets **start failing `perf-gate` when N1 merges**, as ADR-0020 §5d planned.
  - Tiered baselines are re-recorded per `(gpu, path, tier)`, with the upscaler named in the
    log.
  - Compat budgets and baselines are unchanged.
- **VRAM** (estimates, to be replaced by `docs/perf/` measurements):
  - **Removed:** MSAA 4× colour and depth, about 180 MB at native 1440p (ADR-0015).
  - **Added at 1440p Quality** (render 1707×960), about 130 MB in all:
    - the history pair (59 MB);
    - `hdr_out` (29.5 MB);
    - the overlay depth and IDs (18.4 MB);
    - the previous-depth pair (13 MB);
    - `motion` (6.6 MB);
    - the two masks (3.3 MB).
  - The vendor back ends' own memory is reported by `vram_bytes()`.

### 8. Determinism and the invariant

- Rendering still never touches the simulation, the wire, replays or the state hash.
- **Jitter, motion keys and histories are presentation state inside `moba-render`.** Motion
  keys are derived from `net_id`s the client already has, read-only.
- **Picking is jitter-free by construction:**
  - jitter lives only in the renderer's GPU uniforms (`CameraUniform`, `VfxUniform`,
    `EnvironmentBlock` and GTAO's push block, §1);
  - the client's `Camera` has no jitter field;
  - `cursor_ground` and `hovered_unit` read only the client's matrix.

  So a cursor position produces the same `FixVec2`, the same `WireCommand` bytes and the same
  replay frame whatever the AA, upscaler or render scale
  ([verification gate 1](#verification-and-evidence-gates)).
- **Overlay IDs never widen what a player sees.** They come from `Scene::units` only, which
  holds only what the server sent, and they are depth-tested (ADR-0016's hidden-information
  rule).
- The latency sleep changes when a frame starts, never what the frame contains or what the
  client sends.

## Departures from the roadmap

| Roadmap says | This ADR decides | Why |
|---|---|---|
| §5.3: exact motion vectors for particles (previous position) | Only fragments with opacity ≥ 0.5 write motion. All particles write the reactive mask (§2d) | Most particles are translucent layers without depth. For those, a reactive mask is what every SDK asks for; a single motion vector would be wrong for the layers behind it |
| §5.3: water and scrolling materials in "reactive masks" | The separate transparency-and-composition mask (§2d) | FSR distinguishes the two: T&C keeps history while removing locks, reactive reduces history [S2]. Water would flicker under a reactive mask |
| §5.2: "MSAA 4× is retired … on every tier" | Retired on the five tiered tiers. **Compat Ultra keeps MSAA 4×** (§7) | Compat is frozen and has no temporal path (ADR-0020 §2d) |
| §4 rule 1 lists the overlay contents | Also moves `ParticleRole::EnemyTelegraph` particles and every `Glow` decal, not only "decal telegraphs" (§5b) | Particle telegraphs carry the same readability rule (ADR-0013). Zones and ranges are gameplay information |

## Corrections to the roadmap

These were re-checked on 2026-10-31. Only a link from roadmap §12 is added here; the
maintainer may fold these in when this ADR is accepted.

- **§5.3, the FSR 3.1.6 row, "Godot ships a GLSL port".** Godot vendors **FSR 2.2.1** (MIT,
  `thirdparty/amd-fsr2`) and FSR 1.0.2. It has no FSR 3.x entry [S9]. Our FSR 3.1 port is
  our own work, from SDK v2.4.0's MIT HLSL, with FSR 3.1.4's GLSL from SDK v1.1.4 as the
  fallback (ADR-0020 §6a). ADR-0020 Q9's "as Godot did" means the approach, not a port we
  can reuse.
- **§5.3 and §7, DLSS SR** [S1]:
  - Exposure input is honoured only by presets J and K; preset L always auto-exposes.
  - The `BiasCurrentColor` (reactive) mask must not be used with the current models.
  - DLSS's own sharpening is deprecated; NVIDIA points to NIS.
  - The default presets (guide 310.6.0) are K for DLAA, Quality and Balanced, M for
    Performance and L for Ultra Performance.
  - DLSS must run before tonemapping.
  - With L and M, dynamic resolution costs as much as the maximum size it was created with.
- **§7, XeSS.**
  - The ratios changed in XeSS 1.3: "Quality" is 1.7× and "Ultra Quality" 1.5×, so the
    roadmap's "Quality (~720p)" at 1080p is XeSS Ultra Quality [S5].
  - Windows-only is confirmed by the guide's requirements [S5].
- **§7, Reflex.** `VK_NV_low_latency2` is now at revision 3 (2026-04-20, frame attribution
  clarified; `VkLatencySubmissionPresentIdNV` only respected from revision 3) [S6].
- **Confirmed unchanged:**
  - FSR 3.1's inputs, its 1.5/1.7/2.0/3.0 ratios, and tonemapping and bloom after the
    upscaler [S2];
  - FidelityFX SDK v2.4.0 with FSR 3.1.6 and FSR 4.1.2 (Vulkan) [S4];
  - Anti-Lag's API [S7];
  - Mesa 26.3 bringing native RADV Anti-Lag, still unreleased (rc3 tagged 2026-10-30). It
    also deletes Mesa's anti-lag layer [S8].

## Alternatives considered

- **Keep MSAA under TAA.** About 180 MB at 1440p, every scene pipeline doubled, and no
  upscaling benefit. The vendor upscalers expect single-sample input. Rejected.
- **The overlay at render resolution, kept out of history through the reactive mask.** It
  would still be upscaled, and so softened, and every back end treats the mask differently
  (DLSS ignores it). It fails §4 rule 1. Rejected.
- **Overlay depth from the upsampled render depth** (nearest sample, with the jitter removed)
  instead of the overlay prepass. It saves one output-resolution geometry pass, but the
  silhouette edges are quantised to `1/scale` output pixels, which is 1.5 px at Quality, and
  outlines step and crawl. It is kept as the measured fallback for Low if the overlay
  prepass misses its cost gate (Q2).
- **Analytic heightfield intersection for telegraphs** (no overlay depth). It is exact for
  the ground, but it still needs a unit mask at output resolution, so a prepass is needed
  anyway. Rejected.
- **The client supplies the previous transforms.** That would need new client state per
  `net_id` for transforms, palettes and VAT poses. The renderer's history keyed by the client
  records exactly what was drawn, with no new client state. Rejected.
- **Motion vectors from the scene pass** instead of the prepass. They would be written under
  shading, with no benefit before N2's G-buffer. Rejected.
- **Motion vectors in NDC or UV units.** Every SDK accepts a scale, but pixels match XeSS's
  default and FSR's documented range without conversion [S2][S5]. Rejected.
- **DLSS SR through Streamline.** Streamline is Windows-only, and NGX-direct covers Linux and
  Windows (roadmap §5.3). Streamline arrives with frame generation in N9.
- **The Reflex SDK instead of the extension.** It is a closed SDK under the RTX licence, for
  no functional gain over `VK_NV_low_latency2` (ADR-0020 §6a). Rejected.
- **FSR 1 or a spatial upscaler as the tiered baseline.** It has no temporal stability, and
  the roadmap's tiers assume temporal reconstruction. It stays as the "temporal off" bicubic
  path.
- **Reversed-Z now.** Every back end supports it, but it touches every depth consumer, and
  N2 rewrites those anyway. Deferred to N2.

## Consequences

- **Wire, replays and determinism: no change.** No version or hash moves.
- **Frame shape.**
  - The tiered post chain runs at output resolution (bloom, exposure, composite), which costs
    more at 4K than today's render-resolution chain.
  - FXAA and `upscale` leave the tiered default.
  - The graph gains `tc_mask`, `temporal`, `sharpen`, `overlay_prepass`, `overlay` and
    `overlay_vfx`, and loses `outline`, `fxaa` and `upscale` on Tiered.
  - `MAX_RESOURCES` (48) is near the `u64` mask's 64. If the largest tiered topology needs
    more, the mask widens to `u128` in the same PR. The topology test compiles every variant
    against the limits.
- **Shaders.**
  - Every new pass is Slang: `temporal_taau`, the masks, the overlay, the overlay prepass and
    `sharpen`.
  - The tiered permutations of today's GLSL scene shaders gain the `motion` output (prepass),
    the `mip_bias` argument and a compiled-out `apply_glow_decals`.
  - `include/camera.glsl` and `include/vfx.glsl` are shared, so Compat's GLSL gains the new
    members too. That is a **layout change only**: Compat writes zero jitter and equal
    matrices, and its pixels stay byte-identical (checked by its goldens).
- **Instance and upload data.**
  - `GpuInstance` stays 128 bytes. A parallel 64-byte-per-instance previous-instance buffer
    holds the previous model matrix and VAT samples (§2c).
  - The bone upload carries the previous palettes too.
  - A one-texel `exposure_linear` pass is added (§4a).
  - `MotionHistory` holds a fixed 4,096 entries.
- **Readability changes that can be seen:**
  - telegraphs lose bloom and the grade;
  - brush enters GTAO;
  - outlines become crisp at output resolution on every tier, and hull outlines are gone from
    Tiered.
- **Public API** (additive, `#[non_exhaustive]`):
  - `UnitInstance::{motion_key, motion_reset}`;
  - `Scene::history_reset`;
  - `AntiAliasing::Temporal`;
  - `UpscalerKind`, `UpscaleMode`, `PostConfig::{upscaler, upscale_mode, sharpness}`;
  - `LatencyMode`;
  - `Renderer::{latency_sleep, latency_marker, latency_report}`;
  - `Camera::{fov_y, near, far}`, set by the constructors (§4a);
  - `moba_platform::App::pace() -> Pacing` and `moba_platform::Pacing { Platform, External }`.
    The default does nothing and returns `Platform`.
- **Work estimate.** This is the roadmap's **L**. It lands as four PRs on the critical path:
  1. N1a: jitter, motion vectors and masks, TAAU, the overlay layer, MSAA retired;
  2. N1b: the FSR 3.1 port;
  3. N1c: DLSS, FSR 4 and XeSS;
  4. N1d: latency.

  N1a alone closes the readability and perf-gate requirements.

## Verification and evidence gates

These follow roadmap §10 and ADR-0020's gates, applied to N1.

1. **Jitter and picking.**
   - Unit tests: the Halton values, the phase counts (8, 18, 24 at 1.7× and at 1.724×, 32,
     72) under the `ceil` rule of §1, the sign convention,
     and the index surviving a scale step.
   - A client test: the same cursor gives byte-identical `WireCommand`s with
     `MOBA_AA=temporal`, `fxaa` and `off` (§4b), and at render scales 0.5 and 1.0.
   - A renderer test: `Scene::camera` is unchanged after `render`.
2. **Motion vectors.**
   - A CPU twin computes the expected motion for:
     - a translating static instance;
     - a skinned instance;
     - a VAT instance across a pose change;
     - foliage;
     - the camera.
   - A lavapipe frame pair must match it within 1/64 render pixel.
   - `motion_reset` and absent keys give camera-only motion.
3. **Readability (merge-gating):**
   - **Ghost energy.** A hero dashes at maximum speed and a projectile crosses the screen.
     Over 20 frames, the area behind the unit that differs from a no-history render by more
     than 4/255 stays under a fixed bound, for TAAU and FSR 3.1 on lavapipe, and for DLSS on
     the RTX.
   - **Overlay invariance.** The overlay ID target is byte-identical under TAAU, FSR 3.1,
     temporal off and every render scale.
   - **Telegraph contrast** in display space, over backgrounds from black to 16× white.
   - **`outline-probe.sh` on Tiered.**
4. **Goldens.**
   - Every Compat golden is byte-identical.
   - The new tiered goldens (§7) are byte-identical across two lavapipe runs, with
     validation and sync validation clean.
5. **Back ends.**
   - DLSS SR on the RTX 5070 Ti (Linux) and in `windows-vm`.
   - XeSS (DP4a) in `windows-vm`.
   - FSR 4 only on an AMD card. Until one is on hand it is **unverified** at run time and
     not enabled by `auto`.
   - With each binary removed, the client starts and falls back with one log line.
   - Validation is clean around each vendor dispatch. Any message inside an SDK is recorded
     and reported upstream, never filtered silently.
6. **Performance.**
   - Tiered budgets fail `perf-gate` from N1. Medians of 3 interleaved rounds.
   - Per-pass lanes for `prepass`, `temporal`, `overlay_prepass`, `overlay` and `tc_mask`.
   - The overlay prepass must cost **≤ 0.4 ms at 1080p on the Low reference class**. It is
     estimated from the 5070 Ti until an RTX 2060 is on hand, and estimates never close a
     target.
   - `locked_uploads=0` and `locked_pipelines=0` hold with every back end.
   - VRAM is reported against the MSAA it replaced.
7. **Latency.**
   - Reflex timings in the frame log on the RTX: PC latency with Reflex on and off, over the
     teamfight replay.
   - Anti-Lag: a functional smoke test on RADV with **native** `VK_AMD_anti_lag` (Mesa 26.3,
     a release candidate until it ships; the 26.2 layer is not a target, since 26.3 deletes it),
     and numbers on an AMD card.
   - Pacing (p99 ÷ p50 ≤ 1.5, R8) holds with the platform limiter replaced.

## Open questions for the maintainer

1. **Which back end should `auto` fall back to in the open build: in-house TAAU or FSR 3.1?**
   *Recommendation:* TAAU first. It ships in N1a and is ours to tune for the ghosting tests.
   FSR 3.1 takes the slot when its port passes the same readability tests and costs within
   0.2 ms of TAAU on the 5070 Ti. Both stay selectable.
2. **Is the overlay prepass's cost acceptable on Low?** *Recommendation:* yes, with the
   ≤ 0.4 ms gate (verification gate 6). If Low misses it, Low alone uses upsampled depth, and the other tiers
   keep the prepass.
3. **Should dominant particle fragments write motion vectors, or should every particle use
   only the reactive mask?** *Recommendation:* write them. DLSS has no usable reactive mask,
   so without motion vectors projectiles ghost under DLSS.
4. **Telegraphs lose bloom, the grade and the screen effects. Is that acceptable?**
   *Recommendation:* accept it. Display-space contrast is the stronger readability guarantee,
   and the overlay's own fixed halo replaces the glow.
5. **Should Reflex and Anti-Lag be on by default, with boost off, and should they replace the
   platform's frame limiter when active?** *Recommendation:* yes to all three.
6. **Should the motion history be keyed in the renderer from `motion_key` (this ADR), or
   should the client pass previous transforms?** *Recommendation:* keyed in the renderer.
   It needs no new client state, and it records what was drawn.
7. **Should `independentBlend` be required on the tiered path?** *Recommendation:* yes. Check
   it in every recorded fixture as the dumps arrive.
8. **Should vendor back ends be in goldens?** *Recommendation:* no. Goldens cover TAAU and
   FSR 3.1, and the vendor back ends are covered by verification gate 5.

## Sources

All accessed 2026-10-31.

- [S1] NVIDIA DLSS Programming Guide (`doc/DLSS_Programming_Guide_Release.pdf`, revision
  310.6.0, dated 31 March 2026): pipeline placement §3.1, dynamic resolution §3.2.2, mip bias
  §3.5, jitter phases §3.7, exposure §3.9, sharpening §3.11, presets §3.12,
  `BiasCurrentColor` §3.15:
  <https://github.com/NVIDIA/DLSS/blob/main/doc/DLSS_Programming_Guide_Release.pdf>. The SDK
  version (v310.9.16), licence and platforms are ADR-0020 [S16].
- [S2] AMD FSR 3.1 upscaler manual (the page documents 3.1.4; SDK v2.4.0 ships 3.1.6):
  <https://gpuopen.com/manuals/fidelityfx_sdk/techniques/super-resolution-upscaler/>
- [S3] FidelityFX API, version override:
  <https://gpuopen.com/manuals/fidelityfx_sdk/getting-started/ffx-api/>
- [S4] FidelityFX SDK v2.4.0 release notes (ADR-0020 [S22]):
  <https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/releases/tag/v2.4.0>
- [S5] Intel XeSS-SR developer guide:
  <https://github.com/intel/xess/blob/main/doc/xess_sr_developer_guide_english.md>. The SDK
  version (3.0.2) and licence are ADR-0020 [S24].
- [S6] `VK_NV_low_latency2`:
  <https://docs.vulkan.org/refpages/latest/refpages/source/VK_NV_low_latency2.html>,
  <https://docs.vulkan.org/refpages/latest/refpages/source/VkLatencyMarkerNV.html>,
  <https://docs.vulkan.org/refpages/latest/refpages/source/VkLatencySleepModeInfoNV.html>,
  <https://docs.vulkan.org/refpages/latest/refpages/source/vkLatencySleepNV.html>,
  <https://github.com/KhronosGroup/Vulkan-Docs/blob/main/appendices/VK_NV_low_latency2.adoc>
- [S7] `VK_AMD_anti_lag`:
  <https://docs.vulkan.org/refpages/latest/refpages/source/VK_AMD_anti_lag.html>,
  <https://docs.vulkan.org/refpages/latest/refpages/source/VkAntiLagDataAMD.html>,
  <https://docs.vulkan.org/refpages/latest/refpages/source/vkAntiLagUpdateAMD.html>,
  <https://docs.vulkan.org/refpages/latest/refpages/source/VkAntiLagStageAMD.html>,
  <https://docs.vulkan.org/refpages/latest/refpages/source/VkAntiLagModeAMD.html>
- [S8] Mesa 26.3 RADV Anti-Lag: <https://www.phoronix.com/news/Mesa-26.3-RADV-VK_AMD_anti_lag>
  (ADR-0020 [S23]). The layer's deletion was checked through the GitLab API: MR
  <https://gitlab.freedesktop.org/mesa/mesa/-/merge_requests/42048> (merged 2026-10-08) and
  commit `b99595dedb85` "vulkan: delete anti-lag layer", which is on the `26.3` branch and in
  `mesa-26.3.0-rc2` and `rc3`.
- [S9] Godot third-party components:
  <https://github.com/godotengine/godot/blob/master/thirdparty/README.md>
- [S10] Playdead INSIDE TAA (MIT): <https://github.com/playdeadgames/temporal>
- **Literature (not vendor claims, not re-checked):** B. Karis, "High Quality Temporal
  Supersampling", SIGGRAPH 2014 Advances in Real-Time Rendering; M. Salvi, "An Excursion in
  Temporal Supersampling", GDC 2016; J. Jimenez, "Filmic SMAA", SIGGRAPH 2016.
- **Local evidence (2026-10-31, this machine):**
  - `vulkaninfo`: the RTX 5070 Ti (driver 615.78.08) exposes `VK_NV_low_latency2` revision 3.
    Both it and the Radeon 610M (Mesa 26.2.5) expose `VK_KHR_present_id`/`present_id2` and
    `VK_KHR_present_wait`/`present_wait2`, and `VK_AMD_anti_lag` revision 1 through
    `VK_LAYER_MESA_anti_lag`.
  - `ash` 0.38.0's `definitions.rs` contains `SwapchainLatencyCreateInfoNV`,
    `LatencySleepInfoNV`, `LatencySleepModeInfoNV`, `SetLatencyMarkerInfoNV` and
    `LatencySubmissionPresentIdNV`.
