# Reversed-Z and the thin G-buffer on Tiered (N2a)

Captured 2026-11-03 for ADR-0023 (N2a) and its
[Amendment 1](../adr/0023-frame-architecture-materials.md#amendment-1-2026-11-03-n2a-reversed-z-and-the-g-buffer-targets-as-built).

- **GPU:** NVIDIA GeForce RTX 5070 Ti, used for timings, the ghost probe and the debug-view
  captures.
- **Software:** lavapipe (Mesa 26.2.5 llvmpipe, LLVM 23.1.3) with the Khronos validation layer
  and `MOBA_VULKAN_VALIDATION=required`.
- **CPU:** AMD Ryzen 7 9800X3D. **OS:** Linux 7.3.0-5-cachyos, Wayland.
- **Toolchain:** rustc stable, the pinned `slangc`.
- **Branch:** `feat/n2a-gbuffer-reversed-z`.
- **Baseline:** origin/main `657a7ab` (N1b stage 2 merged), built detached in its own
  worktree. Both release builds use the default `fsr3` feature.

Every lavapipe and RTX run below printed no `[vulkan` line. Validation and synchronisation
validation were clean.

## Gate 1: encoding and interfaces

- **Twin tests** (`crates/render/src/gbuffer.rs`):
  - **round trips for all eight models.** IDs, flags and rim slot come back exactly; every
    value is within its quantisation step; re-encoding the decoded surface gives the same
    texels.
  - **stored texels are fixed points** of decode then encode, for every model, flag and rim
    slot.
  - **the octahedral normal** over 10⁶ Fibonacci-sphere normals at 10 + 10 bits: maximum
    **0.2373°**, mean **0.0836°**, recorded as bounds of 0.25° and 0.088°. The six axes come
    back within 0.12°.
  - **quantisation:**
    - sRGB 8-bit, whose relative step at linear 0.02 is under 5 % (4.6 %; a linear store
      would be 20 %);
    - F0 0.018–0.077 in steps under 0.0012, with F0 0.04 storing 128 (`sqrt(F0 / 0.16)`);
    - roughness and AO within half a step, with the ≥ 0.045 and unit ≥ 0.08 roughness floors;
    - NaN and out-of-range inputs clamped.
  - **the alpha bit layouts** of `gb1.a` and `gb2.a`.
- **`gbuffer-smoke` on lavapipe** checked 12,288 values with 0 mismatches and no validation
  messages. It covers 4,096 surfaces over the 8 models, the 8 flag combinations and rim slots
  0–15:
  - the GLSL raster encode (the three real target formats) is compared byte for byte with the
    Slang compute encode;
  - the Slang decode matches the Rust decode within 1e-5 or 4 ulps (largest difference
    3.05e-5);
  - 719 surfaces with a value within 0.01 of a step of a rounding boundary were redrawn.
  CI runs it in `tiered-temporal`.
- **Reflection tests:** `gbuffer_debug.slang`'s push block against `DebugPush` (24 bytes),
  and the smoke's bindings and push. The GLSL-vertex / Slang-fragment interface test moves to
  N2c with the first Slang G-buffer fragment stage (amendment 1 item 5).

## Gate 5: goldens and probes

- **Compat** is byte-identical to origin/main.
  - Each of the 15 Compat scenes in `render-golden.yml` was captured with both builds on
    lavapipe and compared with `cmp`: default, network, Ember, VAT, Ultra, Low scaled,
    overview, horizon, river, river Low, river Ultra, decals, distortion, outline High and
    outline Low.
  - Every compiled SPIR-V module that exists on both sides is byte-identical, except three
    that are tiered-only: `water_refraction_tiered.frag`, `temporal_taau.slang` and
    `fsr3_prepare_inputs`, which are the reversed-Z consumers.
- **Tiered against origin/main** (lavapipe, 640×360, `MOBA_RENDER_PATH=tiered`): pixels change
  only by reversed-Z precision. The table compares PPM channel values (0–255).

  | Back end, scene | Max | Mean | > 2 | > 16 |
  |---|---|---|---|---|
  | TAAU low | 15 | 0.032 | 0.339 % | 0 % |
  | TAAU high | 15 | 0.035 | 0.383 % | 0 % |
  | TAAU native | 37 | 0.039 | 0.427 % | 0.0025 % |
  | TAAU overlay | 24 | 0.010 | 0.025 % | 0.0010 % |
  | FSR 3.1 low | 20 | 0.039 | 0.389 % | 0.0020 % |
  | FSR 3.1 high | 26 | 0.045 | 0.446 % | 0.0026 % |
  | FSR 3.1 native | 30 | 0.053 | 0.525 % | 0.0127 % |
  | FSR 3.1 overlay | 27 | 0.021 | 0.022 % | 0.0004 % |

  The differences sit on the water: its depth fade and shoreline, where the bed and the
  surface are close in depth. The likely cause is that reversed-Z stores those depths with
  different float precision, so the fade's thresholds land a few texels differently
  (inferred, not instrumented). The geometry and the shadows match. The G-buffer writes add no change of their own: these numbers are the same as a
  reversed-Z-only build of the branch, captured before the G-buffer pass existed.
- **Tiered goldens** (`scripts/tiered-goldens.sh`, `UPSCALERS="taau fsr3"`, local lavapipe):
  - every pair of runs is byte-identical;
  - the 64×36 fingerprints pass unchanged (worst `max_diff` 5, mean ≤ 0.093);
  - the mid-stream history reset (40 frames, every 5th reset) is validation-clean for both
    back ends.

  The exact hashes in `tests/golden/render-tiered-hashes.txt` are keyed to CI's lavapipe
  (Mesa 26.0.8, LLVM 21.1.8), so they were regenerated from CI rather than locally:
  - the source is the `render-golden` (TAAU) and `tiered-temporal` (FSR 3.1) artifacts of
    this PR's first CI run (render golden run 41679033229);
  - each value is a `sha256sum` of the `-1.ppm` capture, and every capture was byte-identical
    to its `-2.ppm` on CI;
  - the device line is unchanged, and `WRITE_HASHES` was not run locally.

  All eight hashes changed (reversed-Z). On CI the 64×36 fingerprints passed unchanged, and so
  did every other renderer-gate and tiered-temporal step, `gbuffer-smoke` included.
- **Probes on lavapipe**, all passing:
  - `motion-probe.sh`: every class is within 0.0044 render px, against a bound of 1/64.
  - `ghost-probe.sh` with TAAU and FSR 3.1.
  - `overlay-invariance.sh`: off and FSR 3.1 Quality, native and Performance are identical to
    TAAU Quality.
  - `outline-probe.sh` on Tiered and on Compat.
- **Ghost probe on the RTX 5070 Ti** (validation required, 1280×720), branch against
  origin/main:

  | Back end | Mode | Wake | Hero interior | Particle core | Bound |
  |---|---|---|---|---|---|
  | TAAU | Quality | 1.224 / 1.222 % | 0.73 / 0.73 % | 3.52 / 3.52 % | 7.5 % |
  | TAAU | Performance | 2.039 / 2.031 % | 1.33 / 1.33 % | 6.52 / 6.52 % | 7.5 % |
  | TAAU | Native | 0.611 / 0.611 % | 0.00 / 0.00 % | 1.23 / 1.23 % | 7.5 % |
  | FSR 3.1 | Quality | 0.478 / 0.466 % | 0.00 / 0.00 % | 6.70 / 6.70 % | 7.5 % |
  | FSR 3.1 | Performance | 0.961 / 0.966 % | 0.49 / 0.46 % | 11.51 / 11.51 % | 15 % |
  | FSR 3.1 | Native | 0.310 / 0.307 % | 0.00 / 0.00 % | 3.94 / 3.81 % | 7.5 % |

  Every mode passes. All differences are within 0.013 points, except the FSR 3.1 native
  particle core, which moves by 0.13.

## Gate 6: debug views

The debug views are shown in [screenshots.md](../screenshots.md#tiered-g-buffer-debug-views-n2a).
They were captured on the RTX with validation required.

## Gate 7: performance

RTX 5070 Ti, the teamfight replay (`MOBA_WARMUP_TICKS=330`) at 1920×1080, vsync off, the 240 fps
limit, validation off, the pipeline cache off, 3000 frames per run (the log covers the last
2940). Three rounds, each running every configuration on origin/main and then the branch, run
under the shared GPU lock with no other GPU client running. Medians of the three rounds, in ms.
A first A/B overlapped another agent's measurement on the same GPU and was discarded.

| Configuration | GPU total p50 | GPU total p95 | `prepass` p50 | `scene` p50 | `temporal` p50 | Frame p95 |
|---|---|---|---|---|---|---|
| Tiered Low, main | 0.539 | 0.547 | 0.022 | 0.083 | 0.144 | 4.239 |
| Tiered Low, branch | 0.585 | 0.595 | 0.065 | 0.081 | 0.146 | 4.240 |
| Tiered High, main | 0.588 | 0.597 | 0.026 | 0.095 | 0.144 | 4.238 |
| Tiered High, branch | 0.638 | 0.648 | 0.073 | 0.093 | 0.146 | 4.240 |
| Tiered Ultra, main | 0.588 | 0.597 | 0.025 | 0.095 | 0.144 | 4.241 |
| Tiered Ultra, branch | 0.638 | 0.648 | 0.073 | 0.092 | 0.146 | 4.239 |
| Compat Ultra, main | 0.715 | 0.727 | 0.068 | 0.275 | — | 4.193 |
| Compat Ultra, branch | 0.715 | 0.727 | 0.068 | 0.276 | — | 4.193 |

- **Reversed-Z costs nothing measurable.** The `scene` lane, which runs the same draws with
  the reversed compare, is 0.001–0.003 ms faster. `temporal` (TAAU's reversed depth
  linearisation) is 0.002 ms slower. Both are at the noise floor.
- **The G-buffer writes cost about +0.045 ms in the `prepass` lane** (0.022 → 0.065 at Low,
  0.025 → 0.073 at High and Ultra), which is all of the +0.046–0.051 ms in the GPU total.
  The pass now writes 12 more bytes per pixel at render resolution and binds the textured
  materials. The frame time does not move: these frames are limited at 240 fps.
- **perf-gate** against `tests/perf/baseline.toml` passes every branch log: Tiered High and
  Ultra at 0.648 ms against an allowed 0.747 and 0.749, Low at 0.595 against 0.689, and Compat
  Ultra at 0.727 ms against an allowed 0.830. No pacing or run-integrity failure. Tiered
  baselines are still report-only (`TIERED_BASELINE_FAILS`), but no tiered log breaches one.
- **`locked_uploads=0`, `locked_pipelines=0` and `frames_dropped=0`** in every run.
- **Pipelines:**
  - On the RTX the replay builds **46 tiered pipelines on main and 48 on the branch**.
    Compat stays at 42.
  - On lavapipe at 640×360 High it is 45 → 47 with TAAU and 53 → 55 with FSR 3.1.
  - The two new pipelines are the textured unit and skinned G-buffer variants, because the
    textured materials now feed the G-buffer.
  - §1e's drop to about one pipeline per vertex format comes with N2c's bindless materials
    (amendment 1 item 5).
- **VRAM against §1b:**
  - `targets_mb` rises by **11.2–11.3 MB** at every tiered preset (132.0 → 143.2 at Low and
    180.5 → 191.8 at High and Ultra), and `vram_peak_mb` by 11.3 MB.
  - §1b predicts 11.1 MB at the 1280×720 ceiling of a 1080p output (12 B/px × 0.92 M px).
  - On lavapipe the same growth is 1.2 MB at 427×240 and 2.7 MB at 640×360. Compat does not
    change.
