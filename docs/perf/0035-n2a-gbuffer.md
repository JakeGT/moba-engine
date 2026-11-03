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
  (Mesa 26.0.8, LLVM 21.1.8). They are regenerated from CI's artifacts for this branch, with
  CI's device line kept, and are never written locally.
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

*Pending.* The RTX 5070 Ti A/B (three interleaved rounds against origin/main, Tiered
Low/High/Ultra and Compat) is queued behind another measurement on the same GPU and will
be added to this PR.
