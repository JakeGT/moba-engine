# Deferred lighting for `STANDARD` on Tiered (N2b)

Captured 2026-11-15 for ADR-0023 (N2b) and its
[Amendment 2](../adr/0023-frame-architecture-materials.md#amendment-2-2026-11-15-n2b-deferred-lighting-for-standard-as-built).

- **GPU:** NVIDIA GeForce RTX 5070 Ti (driver 620.70.06), used for timings and the debug-view
  captures.
- **Software:** lavapipe (Mesa 26.2.5 llvmpipe, LLVM 23.1.3) with the Khronos validation layer
  and `MOBA_VULKAN_VALIDATION=required` (validation and synchronisation validation).
- **CPU:** AMD Ryzen 7 9800X3D. **OS:** Linux 7.3.4-1-cachyos, Wayland.
- **Toolchain:** rustc stable, the pinned `slangc`.
- **Branch:** `feat/n2b-deferred-lighting`, on origin/main `790d9be` (N2a merged).
- **Baselines:** detached release builds in their own temporary worktrees, deleted afterwards:
  `ab42718` (the N1 tiered forward frame, before N2a; the baseline ADR-0023 §9b's gate is
  against) and `790d9be` (N2a). Every release build uses the default `fsr3` feature.

Every lavapipe and RTX run below printed no `[vulkan` line.

## Summary

| Gate | Result |
|---|---|
| 1. Encoding and interfaces | **Met.** `gbuffer-smoke` adds the GLSL decode; push blocks pinned by SPIR-V and reflection tests |
| 2. Deferred–forward parity | **Not met on the river** (locally max 15/255 at 7 of 921,600 pixels, 99.966 % within 2/255; on CI max 9 at 11 pixels) **nor, on CI's lavapipe, the teamfight** (1 pixel at 7/255). Default scene met on both. Every outlier is on the river's bank (the foam line, or terrain cracks meeting it). See [below](#gate-2-deferredforward-parity) |
| 3. Light-set consistency | **Met** under all five caps |
| 7. Performance (≤ 1.10× N1) | **Met**: 1.061–1.087× against `ab42718`, 0.990–1.001× against `790d9be` |

## Gate 1: encoding and interfaces

- **`gbuffer-smoke`** (CI `tiered-temporal`) now also runs the GLSL decode that
  `deferred_light` uses (`include/gbuffer.glsl`, through `gbuffer_smoke_decode.comp`) on the
  same 4,096 surfaces, after the Slang decode, and compares both with the Rust twin in one
  loop. On lavapipe: **PASS, 16,384 values checked, 0 mismatches**; the largest absolute
  difference of either decode against Rust is 3.05e-5; 0 validation messages.
- **Push blocks.** `DeferredPush` (32 bytes) is checked against the compiled
  `deferred_light.comp` by parsing its SPIR-V for the push-constant block's member offsets
  (the names are stripped, so the block is found by its storage class); `ClassifyPush`
  (16 bytes) against `light_classify.slang`'s reflection. `TileLayout` (the header, the
  three lists and the info triples) is tied to both shaders' constants by a source test.
- **The light loop's twin.** `visited()` in `crates/render/src/passes/deferred.rs` mirrors
  `dynamic_lights` in `include/lighting.glsl` (the column, the count clamp, the radius test);
  a source test pins the shader lines it mirrors.

## Gate 2: deferred–forward parity

`scripts/deferred-parity.sh` (CI `tiered-temporal`): the same binary with
`MOBA_DEFERRED=off` (N1's tiered forward frame) and on, `MOBA_STYLISATION=off` on both,
`MOBA_FOLIAGE=off` (`STANDARD` only), rims off (there are none before N2e), High (the default
tier, Ultra configuration), TAAU Quality mode (render scale 0.667, sharpness 0.25),
1280×720. `image-check --parity` takes the largest channel difference per pixel.
Locally (Mesa 26.2.5), re-run on the final code with identical results and 0 validation
messages:

| Scene | Within 2/255 | Max | Pixels past 6/255 | Result |
|---|---|---|---|---|
| Default (frame 90) | 99.999 % | 3 | 0 | met |
| R3b river (frame 90) | 99.966 % | **15** | 7 (7, 7, 9, 9, 10, 13, 15) | **not met** |
| R3b teamfight (tick 360) | 99.989 % | 6 | 0 | met, at the bound |

With the foliage (`PARITY_FOLIAGE=1`, the extended tiles; outside the gate): default 99.999 %
max 5, river 99.964 % max 15, teamfight 99.989 % max 6.

**On CI's lavapipe** (`tiered-temporal` on the PR, run 43693581282: Mesa 26.0.8, LLVM 21.1.8,
an AMD EPYC runner), same script and settings:

| Scene | Within 2/255 | Max | Pixels past 6/255 | Result |
|---|---|---|---|---|
| Default | 99.998 % | 6 | 0 | met, at the bound |
| R3b river | 99.959 % | **9** | 11 | **not met** |
| R3b teamfight | 99.991 % | **7** | 1 | **not met** |

The CI outliers are on the river's bank too, but not all on the foam line itself (each
cluster cropped from the run's artefact and inspected):

- on the shoreline foam line: river (173, 388–389), (955, 407) and (461, 710), and the
  teamfight's one at (402, 282), where the replay's camera sees the river's edge;
- on a terrain crack where it meets the foam's end: river (612–616, 601–602), shifted by 6–8
  in all three channels rather than mostly blue;
- on a crack under the shallow water: river (410, 676);
- on the same crack in the dry bank, 1–5 rows from the frame's bottom edge: river
  (524–532, 715–719).

The pixels differ from the local ones, as the two lavapipes round differently. The controls
below (the G-buffer round trip and the perturbed forward frame) were run on the local Mesa
only; that CI's outliers have the same cause, high-contrast edges amplified by TAAU's upscale
and sharpening, is inferred from where they are, not measured.

**Where the river's outliers are.** All seven sit on the shoreline foam line, where the
river bed is seen through the refracting water, in single pixels that appear in either
frame. The water's transmission tint (up to twice the shallow colour) amplifies the blue
channel, which is where the large differences are.

**What causes them.** Each row is one change against the default comparison, on the early
river frame (frame 2, whose outliers are the same pixels as frame 90's) unless marked:

| Comparison | Within 2/255 | Max |
|---|---|---|
| Deferred against forward (the gate) | 99.965 % | 15 |
| Deferred against a forward build that round-trips its terrain surface through `encode_gbuffer`/`decode_gbuffer` before `shade()` | 100.000 % | **2** |
| ... round-tripping only the albedo | 99.996 % | 4 |
| Forward against forward with only the normal round-tripped (0.24° at most) | 99.994 % | 14 |
| **Forward against forward with the terrain albedo scaled by 1.001** (a tenth of an 8-bit step) | 99.996 % | **14** |
| Deferred against forward at native scale (`MOBA_UPSCALE_MODE=native`, sharpness 0) | 99.987 % | 6 |
| ... TAAU Quality with sharpness 0 | 99.980 % | 12 |
| ... with `MOBA_WATER=simple` (frame 90) | 99.980 % | 6 |
| ... with `MOBA_WATER=off` (frame 90) | 99.976 % | 8 |
| `tc` and `reactive` mask views, deferred against forward | 100.000 % | 0 |

- **The lighting is exact.** With the G-buffer's quantisation applied to the forward
  surface, the two paths agree within 2/255 everywhere: the deferred lighting computes what
  the forward lighting computes, which is gate 2's second sentence.
- **The frame is unstable at those pixels.** Changing the forward frame's terrain albedo by
  0.1 %, far below one G-buffer step, moves the same shoreline pixel (126, 399) by 14/255.
  So no G-buffer precision can meet "6/255 everywhere" on this capture, and the encoding is
  left as amendment 1 built it. TAAU's upscale and sharpening at the foam edge do the
  amplifying: at native scale without sharpening the worst pixel is 6.
- **Ruled out:** the depth (the forward `scene` without depth writes gives a byte-identical
  frame) and the TAAU masks (identical). The shadows-off and
  contact-shadows-off runs were byte-identical to the default ones (this view has no casters
  near the bed), so they rule nothing out.

The gate is kept as written, and CI reports the failing scenes (the river, and on CI the
teamfight; run 43699326756). ADR-0023 amendment 2 item 16
lists the options for the maintainer.

## Gate 3: light-set consistency

`forward_and_deferred_visit_the_same_lights` (`crates/render/src/passes/deferred.rs`):
- a camera at the default angle (reversed-Z, unjittered), 2 × 30 clustered lights listed
  first (so the clusters overflow their columns) and 180 spread lights;
- 320 × 180 pixel centres hitting the ground and a slope;
- for each tier's caps, the lights are binned by the renderer's own `bin()`, each pixel's
  world position is reconstructed from its `f32` reversed depth as `deferred_light` does
  (worst error **2.36e-5 m** against the analytic hit; the bound is 1e-4 m), and the light
  set the forward loop visits at the true position is compared with the set the deferred
  loop visits at the reconstructed one. Points within 1e-4 m of a column edge or a light's
  radius are skipped (21 of 57,600).

| Caps | Binned / submitted | Column overflows | Points compared | Points lit by ≥ 1 light | Mismatches |
|---|---|---|---|---|---|
| Low | 16 / 240 | 78 | 57,579 | 1,332 | 0 |
| Medium | 64 / 240 | 459 | 57,579 | 8,140 | 0 |
| High | 128 / 240 | 527 | 57,579 | 32,828 | 0 |
| Ultra | 240 / 240 | 1,093 | 57,579 | 37,522 | 0 |
| Overdrive | 240 / 240 | 1,093 | 57,579 | 37,522 | 0 |

## Goldens and probes

- **Compat is byte-identical** to origin/main (`790d9be`) on lavapipe: the Compat scenes of
  `render-golden.yml` (default, Ultra, Low at 0.75, VAT, horizon, river, river Low, river
  Ultra and the teamfight decals) compared with `cmp`, re-run on the final code (after the
  AO rebind fix in the shared `lighting.rs`).
- **SPIR-V:** every one of the 120 modules that exists on both sides is byte-identical,
  forward, Compat and tiered alike (`MOBA_PIXEL_CENTER` expands to `gl_FragCoord.xy` in the
  fragment stages). The 15 new modules are `deferred_light`, `deferred_light_extended`,
  `light_classify`, `gbuffer_smoke_decode` and the ten `_hdr` G-buffer permutations.
- **Tiered goldens move**, deliberately: the tiered frame is now deferred. Their fingerprints
  and hashes follow ADR-0021 amendment 6. The 64×36 fingerprints pass unchanged on CI's
  lavapipe and locally (max_diff ≤ 5), so they are not rewritten. All 16 exact hashes (8
  captures × the AMD and Intel sections) moved and were taken from CI only: the
  `cpu_sample=true` dispatch run 43697497468 at `e4d139b` (three AMD and three Intel CPU
  models; the legs agreed per class), cross-checked by the PR's own `renderer-gate` and
  `tiered-temporal` (run 43693581282, AMD), which printed the same AMD hashes. Locally all
  eight cases are two-run byte-identical with 0 validation messages, and both mid-stream
  resets are clean.

## Gate 7: performance

- **Builds:** `ab42718`, `790d9be` and the branch, release, detached worktrees for the first two.
- **Machine:** the RTX 5070 Ti (driver 620.70.06, Linux 7.3.4).
- **Replay:** the teamfight replay (`MOBA_WARMUP_TICKS=330`) at 1920×1080, vsync off, the
  240 fps limit, validation off, the pipeline cache off, 3,000 frames per run.
- **Rounds:** three, interleaved: each held `flock handoff/gpu.lock` and ran every
  configuration on `ab42718`, `790d9be` and the branch in turn, with no other `moba-client`
  running (checked before each run).
- **Results:** medians of the three rounds, in ms, from the final code (the rows-of-tiles
  dispatch and the AO rebind fix included). Within each configuration and build the three GPU
  totals agree within 0.005 ms. The raw logs are kept locally in
  `~/Projects/MOBA/handoff/evidence/n2b-ab2/` (not committed). An earlier A/B on the branch
  before those two changes measured 0.001–0.004 ms less on the branch and the same baselines.

| Configuration | Build | GPU total p50 | p95 | `prepass` | `scene` | `light_classify` | `deferred_light` | `sky` | `temporal` |
|---|---|---|---|---|---|---|---|---|---|
| Tiered Low (TAAU) | `ab42718` | 0.506 | 0.516 | 0.020 | 0.079 | — | — | — | 0.125 |
| | `790d9be` | 0.550 | 0.560 | 0.063 | 0.077 | — | — | — | 0.125 |
| | branch | 0.550 | 0.561 | 0.072 | — | 0.009 | 0.055 | 0.005 | 0.126 |
| Tiered High (TAAU) | `ab42718` | 0.557 | 0.566 | 0.024 | 0.090 | — | — | — | 0.125 |
| | `790d9be` | 0.604 | 0.615 | 0.069 | 0.089 | — | — | — | 0.125 |
| | branch | 0.599 | 0.609 | 0.080 | — | 0.009 | 0.059 | 0.005 | 0.125 |
| Tiered Ultra (TAAU) | `ab42718` | 0.557 | 0.567 | 0.024 | 0.090 | — | — | — | 0.125 |
| | `790d9be` | 0.604 | 0.614 | 0.069 | 0.089 | — | — | — | 0.125 |
| | branch | 0.598 | 0.608 | 0.080 | — | 0.009 | 0.059 | 0.005 | 0.125 |
| Tiered Low (FSR 3.1) | `ab42718` | 0.654 | 0.666 | 0.020 | 0.079 | — | — | — | 0.276 |
| | `790d9be` | 0.698 | 0.712 | 0.063 | 0.077 | — | — | — | 0.277 |
| | branch | 0.699 | 0.711 | 0.072 | — | 0.009 | 0.055 | 0.005 | 0.278 |
| Tiered High (FSR 3.1) | `ab42718` | 0.705 | 0.717 | 0.024 | 0.091 | — | — | — | 0.276 |
| | `790d9be` | 0.754 | 0.766 | 0.069 | 0.089 | — | — | — | 0.278 |
| | branch | 0.748 | 0.760 | 0.080 | — | 0.009 | 0.059 | 0.005 | 0.278 |
| Compat Ultra | `ab42718` | 0.672 | 0.683 | 0.058 | 0.246 | — | — | — | — |
| | `790d9be` | 0.672 | 0.683 | 0.058 | 0.246 | — | — | — | — |
| | branch | 0.672 | 0.683 | 0.058 | 0.245 | — | — | — | — |

**Against ADR-0023 §9b's 1.10× gate** (GPU total, branch ÷ baseline):

| Preset | p50 vs `ab42718` (gate) | p95 vs `ab42718` | p50 vs `790d9be` (N2a) | Headroom to 1.10× at p50 |
|---|---|---|---|---|
| Tiered Low (TAAU) | **1.087** | 1.087 | 1.000 | 0.007 ms (1.3 % of N1) |
| Tiered High (TAAU) | **1.075** | 1.076 | 0.992 | 0.014 ms (2.5 %) |
| Tiered Ultra (TAAU) | **1.074** | 1.072 | 0.990 | 0.015 ms (2.6 %) |
| Tiered Low (FSR 3.1) | **1.069** | 1.068 | 1.001 | 0.020 ms (3.1 %) |
| Tiered High (FSR 3.1) | **1.061** | 1.060 | 0.992 | 0.028 ms (3.9 %) |
| Compat Ultra | 1.000 | 1.000 | 1.000 | not gated |

- **The deferred lighting replaces the forward `scene` and costs a little less.** The three
  new lanes add 0.069 ms at Low (classify 0.009, lighting 0.055, sky 0.005) and 0.073 ms at
  High and Ultra, against the 0.077–0.089 ms `scene` they remove. The G-buffer pass's `hdr`
  attachment (emissive) adds 0.009–0.011 ms to `prepass`. Net against N2a: 0.000 to
  +0.001 ms at Low, −0.005 to −0.006 ms at High and Ultra.
- **Low has the least headroom** (1.087×, 0.007 ms), as with N2a: its `scene` was the
  cheapest to remove. N2c–N2f find their cost in that headroom and in what the Slang port of
  the lighting saves (the scalarised loop, amendment 2 item 5).
- **`temporal`, `water` and the post chain are unchanged.** So is Compat, to the microsecond.
- **Frame time** is the 240 fps limit's: `p95_ms` 4.233–4.237 on every tiered run of every
  build.
- **perf-gate** passes every log of every build (no regression flagged); `frames_dropped=0`,
  `locked_uploads=0` and `locked_pipelines=0` in every run.
- **Pipelines** on the replay: tiered TAAU 46 (`ab42718`) → 48 (`790d9be`) → **51**; FSR 3.1
  54 → 56 → **59**; Compat 42 throughout. The three new ones are `light_classify` and the two
  `deferred_light` permutations; the ten `_hdr` G-buffer permutations replace the N2a ones in
  the deferred frame. The forward `scene` pipelines are still built (amendment 2 item 15).
- **VRAM:** `vram_peak_mb` +0.1–0.2 MB against N2a (the tile buffer: 4 × (12 + 6 × 8,160)
  bytes at 1920×1080), and +11.3–11.5 MB against `ab42718` (N2a's G-buffer). `hdr` was
  already `RGBA16F` in these runs (amendment 2 item 8).

## Debug views

`tiles`, `light-count` and `lit-only` are shown in
[screenshots.md](../screenshots.md#tiered-deferred-lighting-debug-views-n2b), captured on the
RTX with validation required.
