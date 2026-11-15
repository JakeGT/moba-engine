# Deferred lighting for `STANDARD` on Tiered (N2b)

Captured 2026-11-15 for ADR-0023 (N2b) and its
[Amendment 2](../adr/0023-frame-architecture-materials.md#amendment-2-2026-11-15-n2b-deferred-lighting-for-standard-as-built).

- **GPU:** NVIDIA GeForce RTX 5070 Ti (driver 620.70.06), used for timings and the debug-view
  captures.
- **Software:** lavapipe (Mesa 26.2.5 llvmpipe, LLVM 23.1.3) with the Khronos validation layer
  and `MOBA_VULKAN_VALIDATION=required` (validation and synchronisation validation).
- **CPU:** AMD Ryzen 7 9800X3D. **OS:** Linux 7.3.4-1-cachyos, Wayland.
- **Toolchain:** rustc stable, the pinned `slangc`.
- **Branch:** `feat/n2b-deferred-lighting`, on origin/main `fdc0a5d` (N2a and #377 merged).
- **Baselines:** detached release builds in their own temporary worktrees, deleted afterwards:
  `ab42718` (the N1 tiered forward frame, before N2a; the baseline ADR-0023 §9b's gate is
  against) and `790d9be` (N2a). Every release build uses the default `fsr3` feature.

Every lavapipe and RTX run below printed no `[vulkan` line.

## Summary

| Gate | Result |
|---|---|
| 1. Encoding and interfaces | **Met.** `gbuffer-smoke` adds the GLSL decode; push blocks pinned by SPIR-V and reflection tests |
| 2. Deferred–forward parity | **Met** locally and on CI after a TAAU fix (ADR-0021 amendment 7): worst pixel 2, 3 and 5/255 locally on the default scene, the river and the teamfight; on CI (AMD) 4, 3 and 5. No Intel runner ran the parity step. See [below](#gate-2-deferredforward-parity) |
| 3. Light-set consistency | **Met** under all five caps |
| 7. Performance (≤ 1.10× N1) | **Met**: GPU total 1.014–1.065× against `ab42718` (GPU span 1.031–1.078×), 0.959–0.981× against `790d9be`; Tiered Overdrive included |

## Gate 1: encoding and interfaces

- **`gbuffer-smoke`** (CI `tiered-temporal`) now also runs the GLSL decode that
  `deferred_light` uses (`include/gbuffer.glsl`, through `gbuffer_smoke_decode.comp`) on the
  same 4,096 surfaces, after the Slang decode, and compares both with the Rust twin in one
  loop. On lavapipe: **PASS, 16,384 values checked, 0 mismatches**; the largest absolute
  difference of either decode against Rust is 3.05e-5; 0 validation messages.
- **Push blocks.** `DeferredPush` (24 bytes) is checked against the compiled
  `deferred_light.comp` by parsing its SPIR-V for the push-constant block's member offsets
  (the names are stripped, so the block is found by its storage class); `ClassifyPush`
  (16 bytes) against `light_classify.slang`'s reflection. `TileLayout` (the header and the
  standard and extended lists) is tied to both shaders' constants by a source test. The
  debug view is the pipelines' specialisation constant, checked by
  `debug_views_are_a_specialisation_constant`.
- **The light loop's twin.** `visited()` in `crates/render/src/passes/deferred.rs` mirrors
  `dynamic_lights` in `include/lighting.glsl` (the column, the count clamp, the radius test);
  a source test pins the shader lines it mirrors.

## Gate 2: deferred–forward parity

`scripts/deferred-parity.sh` (CI `tiered-temporal`): the same binary with
`MOBA_DEFERRED=off` (N1's tiered forward frame) and on, `MOBA_STYLISATION=off` on both,
`MOBA_FOLIAGE=off` (`STANDARD` only), rims off (there are none before N2e), High (the default
tier, Ultra configuration), TAAU Quality mode (render scale 0.667, sharpness 0.25),
1280×720. `image-check --parity` takes the largest channel difference per pixel. The gate:
within 2/255 on at least 99.5 % of pixels, and within 6/255 everywhere.

**Met**, with TAAU's history fix (ADR-0021 amendment 7), locally (Mesa 26.2.5, 0 validation
messages) and on CI's lavapipe (Mesa 26.0.8, LLVM 21.1.8; `tiered-temporal` in run 43720150585, an AMD EPYC 9V74 runner):

| Scene | Local: within 2/255 | Local: max | CI: within 2/255 | CI: max |
|---|---|---|---|---|
| Default (frame 90) | 100.000 % | 2 | 100.000 % | 4 (2 pixels past 2) |
| R3b river (frame 90) | 100.000 % | 3 (3 pixels) | 100.000 % | 3 (2 pixels) |
| R3b teamfight (tick 360) | 99.999 % | 5 (1 pixel) | 99.999 % | 5 (1 pixel) |

**The first build failed the river**, and on CI the teamfight, before the fix:

| Scene | Local: within 2/255 | Local: max | CI: max (run 43693581282) |
|---|---|---|---|
| Default | 99.999 % | 3 | 6 |
| R3b river | 99.966 % | **15** (7 pixels past 6) | **9** (11 pixels) |
| R3b teamfight | 99.989 % | 6 | **7** (1 pixel) |

Every outlier was on the river's bank: the shoreline foam line and terrain cracks meeting
it, seen through the refracting water.

**The cause was TAAU's history clip, not the deferred frame.** Each row is one change
against the gate's comparison, locally:

| Comparison | Within 2/255 | Max |
|---|---|---|
| The gate's comparison, the first build (river, frame 90) | 99.966 % | 15 |
| Both frames through a TAAU patched to output its nearest raw sample (no history, native scale, sharpness 0) | 100.000 % | **2** |
| Forward against forward with the terrain albedo scaled by 1.001 (a tenth of an 8-bit step), through TAAU | 99.996 % | **14** |
| ... the same, through the raw passthrough | 100.000 % | 2 |
| Deferred against forward at native scale (`MOBA_UPSCALE_MODE=native`, sharpness 0) | 99.987 % | 6–7 |
| ... TAAU Quality with sharpness 0 | 99.980 % | 12 |
| ... with `MOBA_WATER=off` (frame 90) | 99.976 % | 8 |
| Deferred against a forward build that round-trips its terrain surface through `encode_gbuffer`/`decode_gbuffer` (river, frame 2) | 100.000 % | 2 |
| The fixed TAAU (amendment 7), the gate's comparison (river, frame 90) | 100.000 % | **3** (3 pixels) |

- **Before the upscaler the frames agree within 2/255 everywhere**, and every earlier
  outlier is within 2 over its 7 × 7 neighbourhood: the deferred lighting computes what the
  forward lighting computes, up to the G-buffer's quantisation (gate 2's second sentence).
- **TAAU's clip amplified sub-step differences.** It scaled the history's whole offset by
  the most-violated axis of a box whose extent was floored at 1e-5; on flat chroma a change
  far below one 8-bit step rescaled the luma too. That is also why a 0.1 % albedo change
  moved the forward frame by 14/255. Sharpening and the water were not the cause.
- **The fix and its alternatives** are measured in ADR-0021 amendment 7: a per-channel
  clamp alone met this gate (2, 2, 4) but failed ghost energy; a floor on the clip's extent
  passes at 0.004 (2, 3, 5) and ghosts the hero at 0.01. The shipped blend keeps the clip on
  every box at least 0.004 wide and clamps the narrower; it measures as the 0.004 floor
  does here, and fails safe if its constant is widened.
- **The teamfight's worst pixel** (5/255, one pixel at (516, 475)) is on a unit's
  silhouette edge, just inside its outline; its cause was not isolated.

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

- **Compat is byte-identical** to `790d9be` on lavapipe (#377, merged since, changes no
  render code): the Compat scenes of `render-golden.yml` (default, Ultra, Low at 0.75, VAT,
  horizon, river, river Low, river Ultra and the teamfight decals) compared with `cmp`,
  re-run on the review's code.
- **SPIR-V:** 103 of the 105 modules that exist on both sides are byte-identical. The two
  that differ are `sky.frag`, whose body moved into `include/sky_radiance.glsl` so the
  G-buffer pass's sky can share it (a function call in the module; every Compat capture is
  byte-identical), and `temporal_taau.slang` (ADR-0021 amendment 7; Tiered only). The 14
  new modules are `deferred_light`, `deferred_light_extended`, `light_classify`,
  `gbuffer_smoke_decode` and the ten `_hdr` G-buffer permutations.
- **ADR-0021's TAAU probes** pass on the final code; before and after amendment 7 they are
  in that amendment. FSR 3.1's ghost probe is unchanged (it does not run TAAU's shader).
- **Tiered goldens move**, deliberately: the tiered frame is deferred, and amendment 7 moves
  every TAAU frame. Their fingerprints and hashes follow ADR-0021 amendment 6. The 64×36 fingerprints pass
  unchanged, locally (max_diff ≤ 5) and on CI, so they are not rewritten. The eight TAAU
  exact hashes (`low`, `high`, `native`, `overlay` × the AMD and Intel sections) moved and
  were taken from CI only: the `cpu_sample=true` dispatch run 43722069812 at `3131b7f`
  (seven AMD legs on EPYC 7763 and 9V74, one Intel leg on a Xeon 6975P-C), pasted as its
  `tiered-hash-sections` artefact gives them, sorted with `LC_ALL=C`. The eight FSR 3.1
  hashes did not move (FSR 3.1 does not run TAAU's shader); the PR's `tiered-temporal` (run
  43720150585, AMD) printed `exact hash` for all four of its own. Cross-check: the same
  run's `renderer-gate` (AMD EPYC 7763) printed exactly the new AMD TAAU hashes against
  the old file.
  Locally all eight cases are two-run byte-identical with 0 validation messages, and both
  mid-stream resets are clean.

## Gate 7: performance

- **Builds:** `ab42718`, `790d9be` and the branch's final code (merged with `fdc0a5d`, the
  review's fixes and ADR-0021 amendment 7), release, detached worktrees for the first two.
- **Machine:** the RTX 5070 Ti (driver 620.70.06, Linux 7.3.4).
- **Replay:** the teamfight replay (`MOBA_WARMUP_TICKS=330`) at 1920×1080, vsync off, the
  240 fps limit, validation off, the pipeline cache off, 3,000 frames per run.
- **Configurations:** Tiered Low, High and Ultra with TAAU, **Tiered Overdrive** (the Ultra
  configuration, TAAU Performance mode: render scale 0.5 on every frame of every build),
  Tiered Low and High with FSR 3.1, and Compat Ultra.
- **Rounds:** three, interleaved: each held `flock handoff/gpu.lock` and ran every
  configuration on `ab42718`, `790d9be` and the branch in turn, with no other `moba-client`
  running (checked before each run).
- **Results:** medians of the three rounds, in ms. Within each configuration and build the
  three GPU totals agree within 0.002 ms. `gpu_total` is the sum of the pass timestamps;
  `gpu_span` is the frame's first to last timestamp, so it also counts the gaps between
  passes. The raw logs are kept locally in `~/Projects/MOBA/handoff/evidence/n2b-ab3/`
  (not committed).

| Configuration | Build | GPU total p50 | p95 | GPU span p50 | `prepass` | `scene` | `light_classify` | `deferred_light` | `temporal` |
|---|---|---|---|---|---|---|---|---|---|
| Tiered Low (TAAU) | `ab42718` | 0.505 | 0.515 | 0.512 | 0.020 | 0.079 | — | — | 0.125 |
| | `790d9be` | 0.550 | 0.560 | 0.556 | 0.063 | 0.077 | — | — | 0.125 |
| | branch | 0.538 | 0.549 | 0.552 | 0.063 | — | 0.009 | 0.053 | 0.127 |
| Tiered High (TAAU) | `ab42718` | 0.558 | 0.570 | 0.564 | 0.024 | 0.090 | — | — | 0.125 |
| | `790d9be` | 0.604 | 0.616 | 0.612 | 0.069 | 0.089 | — | — | 0.125 |
| | branch | 0.583 | 0.595 | 0.598 | 0.069 | — | 0.010 | 0.056 | 0.127 |
| Tiered Ultra (TAAU) | `ab42718` | 0.559 | 0.571 | 0.566 | 0.024 | 0.091 | — | — | 0.125 |
| | `790d9be` | 0.606 | 0.618 | 0.613 | 0.069 | 0.089 | — | — | 0.125 |
| | branch | 0.585 | 0.597 | 0.600 | 0.069 | — | 0.010 | 0.056 | 0.127 |
| Tiered Overdrive (TAAU, 0.5) | `ab42718` | 0.503 | 0.514 | 0.509 | 0.027 | 0.068 | — | — | 0.125 |
| | `790d9be` | 0.532 | 0.543 | 0.539 | 0.055 | 0.067 | — | — | 0.124 |
| | branch | 0.510 | 0.523 | 0.525 | 0.055 | — | 0.006 | 0.036 | 0.127 |
| Tiered Low (FSR 3.1) | `ab42718` | 0.657 | 0.671 | 0.664 | 0.020 | 0.079 | — | — | 0.278 |
| | `790d9be` | 0.701 | 0.714 | 0.708 | 0.063 | 0.077 | — | — | 0.279 |
| | branch | 0.688 | 0.702 | 0.703 | 0.064 | — | 0.010 | 0.053 | 0.279 |
| Tiered High (FSR 3.1) | `ab42718` | 0.707 | 0.720 | 0.714 | 0.024 | 0.090 | — | — | 0.277 |
| | `790d9be` | 0.754 | 0.767 | 0.761 | 0.069 | 0.090 | — | — | 0.278 |
| | branch | 0.730 | 0.743 | 0.745 | 0.069 | — | 0.010 | 0.056 | 0.278 |
| Compat Ultra | `ab42718` | 0.675 | 0.687 | 0.679 | 0.058 | 0.247 | — | — | — |
| | `790d9be` | 0.675 | 0.687 | 0.679 | 0.058 | 0.247 | — | — | — |
| | branch | 0.675 | 0.687 | 0.679 | 0.058 | 0.247 | — | — | — |

**Against ADR-0023 §9b's 1.10× gate** (branch ÷ baseline, p50):

| Preset | GPU total vs `ab42718` (gate) | GPU span vs `ab42718` | GPU total vs `790d9be` (N2a) | GPU span vs `790d9be` | `790d9be` ÷ `ab42718` (total) |
|---|---|---|---|---|---|
| Tiered Low (TAAU) | **1.065** | 1.078 | 0.978 | 0.993 | 1.089 |
| Tiered High (TAAU) | **1.045** | 1.060 | 0.965 | 0.977 | 1.082 |
| Tiered Ultra (TAAU) | **1.047** | 1.060 | 0.965 | 0.979 | 1.084 |
| Tiered Overdrive (TAAU) | **1.014** | 1.031 | 0.959 | 0.974 | 1.058 |
| Tiered Low (FSR 3.1) | **1.047** | 1.059 | 0.981 | 0.993 | 1.067 |
| Tiered High (FSR 3.1) | **1.033** | 1.043 | 0.968 | 0.979 | 1.066 |
| Compat Ultra | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 |

- **Nothing exceeds 1.10×**, on either metric. The least headroom is Tiered Low's span
  (1.078×). `gpu_span` runs 1–1.5 points above `gpu_total` against `ab42718`: the deferred
  frame has three more passes, and so more gaps between them.
- **N2a's own cost** (`790d9be` ÷ `ab42718`) measured 1.066–1.089× on `gpu_total` (1.058× at
  Overdrive), consistent with #369's projection of at most 1.092×.
- **The deferred lighting is now cheaper than the forward `scene` it replaces.** The two
  new lanes add 0.062 ms at Low and 0.066 ms at High and Ultra, against the 0.077–0.090 ms
  `scene` they remove; `prepass` costs the same as N2a's, since the terrain and the foliage
  no longer write `hdr` and the sky's radiance is drawn there instead of in a second sky
  pass. The first build measured 0.990–1.001× N2a; the review's changes (amendment 2 items
  3, 6 and 7) bring it to 0.959–0.981×.
- **`temporal` costs 0.002 ms more** with TAAU (0.125 → 0.127 ms): ADR-0021 amendment 7's
  clip and clamp blend. FSR 3.1's is unchanged.
- **Compat is unchanged**, to the microsecond.
- **Frame time** is the 240 fps limit's: `p95_ms` 4.236–4.240 on every tiered run of every
  build.
- **perf-gate** passes all 63 logs; `frames_dropped=0`, `locked_uploads=0` and
  `locked_pipelines=0` in every run.
- **Pipelines** on the replay: tiered TAAU 46 (`ab42718`) → 48 (`790d9be`) → **51**; FSR 3.1
  54 → 56 → **59**; Compat 42 throughout. The three new ones are `light_classify` and the
  two `deferred_light` permutations; the ten `_hdr` G-buffer permutations replace the N2a
  ones in the deferred frame. The forward `scene` pipelines are still built (amendment 2
  item 15).
- **VRAM:** `vram_peak_mb` equal to N2a's to 0.1 MB (the tile buffer, now 4 × (12 + 2 ×
  8,160) bytes at 1920×1080), and +7.1–11.3 MB against `ab42718` (N2a's G-buffer).
- **What this replay does not measure:**
  - **`deferred_light` under many lights.** The teamfight peaks at 10 lights in view
    (`lights_max=10`), so a pixel whose column holds many lights is not timed; the cost
    grows with the column's count, which the light-count debug view shows.
  - **`simple_water`**: this replay runs full water on every tiered configuration, so the
    lane does not appear.
  - **`hdr` as a storage image on AMD and Intel.** `deferred_light` writes `hdr` in the
    `GENERAL` layout, which on some AMD and Intel drivers disables colour compression for
    the image and can slow the passes that read it (the transparents, `temporal`). The RTX
    shows no cost; it is to be re-checked with #9/#127's devices.

## Debug views

`tiles`, `light-count` and `lit-only` are shown in
[screenshots.md](../screenshots.md#tiered-deferred-lighting-debug-views-n2b), captured on the
RTX with validation required.
