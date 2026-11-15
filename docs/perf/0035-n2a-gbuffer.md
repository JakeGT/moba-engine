# Reversed-Z and the thin G-buffer on Tiered (N2a)

Captured 2026-11-03, with gate 7 re-timed and the G-buffer content check added on 2026-11-15
after the reviews, for ADR-0023 (N2a) and its
[Amendment 1](../adr/0023-frame-architecture-materials.md#amendment-1-2026-11-03-n2a-reversed-z-and-the-g-buffer-targets-as-built).

- **GPU:** NVIDIA GeForce RTX 5070 Ti, used for timings, the ghost probe and the debug-view
  captures.
- **Software:** lavapipe (Mesa 26.2.5 llvmpipe, LLVM 23.1.3) with the Khronos validation layer
  and `MOBA_VULKAN_VALIDATION=required`.
- **CPU:** AMD Ryzen 7 9800X3D. **OS:** Linux 7.3.0-5-cachyos, Wayland.
- **Toolchain:** rustc stable, the pinned `slangc`.
- **Branch:** `feat/n2a-gbuffer-reversed-z`.
- **Baseline:** origin/main, built detached in its own worktree: `657a7ab` (N1b stage 2
  merged) for gates 1, 5 and 6, and `ab42718` (after #373, #347, #374 and #349) for gate 7 and
  the 2026-11-15 tiered comparison. Every release build uses the default `fsr3` feature.

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
- **In the frame, on lavapipe:** the motion probe reads `gb0`, `gb1` and `gb2` back
  (amendment 1 item 16). Every class must also hold metallic 0 in `gb0`, and the units their
  instance colour (0.75, 0.35, 0.2) within one step per channel. `scripts/motion-probe.sh`,
  at Low and High, 853×480, validation required:

  | Class | What it must hold | Pixels (Low / High) | Mismatches |
  |---|---|---|---|
  | unit (static, reset, unkeyed) | `STANDARD`, F0 0.04, `UNIT \| NO_FOG_OF_WAR`, rim 0, instance colour | 8,711 / 8,711 | 0 |
  | skinned | as unit | 1,709 / 1,709 | 0 |
  | vat | as unit | 2,163 / 2,163 | 0 |
  | terrain (outside the foliage patches) | `STANDARD`, F0 0.04, no flags | 369,616 / 369,616 | 0 |
  | foliage | `FOLIAGE`, no flags | 6,323 / 7,186 | 0 |
  | sky (frame C, at the horizon) | written as zero: `UNLIT`, every bit of `gb0`–`gb2` 0 | 216,662 / 216,662 | 0 |

  **The check found a real fault.** Its first run failed every class on lavapipe: `gb1`'s
  roughness and flags and `gb2`'s model read 0, so every surface decoded as `UNLIT`. The
  in-frame `model` and `flags` debug views showed the same on lavapipe, so the readback was
  not at fault. The cause was the per-attachment write masks: lavapipe applied attachment 0's
  (motion's R|G) to every attachment. With motion's mask set to RGBA, which on its
  `R16G16_SFLOAT` format writes the same R and G, every class passes. The RTX debug views
  (gate 6) were right before the fix, because the NVIDIA driver honours the masks. The
  terrain and the sky now also write their ID (0) and the sky its G-buffer (zero)
  explicitly, so only the foliage's ID slot still relies on an empty mask, which lavapipe
  leaves undefined (item 16). Neither change moved a tiered hash.
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

  **Re-checked on 2026-11-15** with the review fixes, against `ab42718`:
  - **Method.** The `-1.ppm` captures of `scripts/tiered-goldens.sh` (`UPSCALERS="taau fsr3"`)
    from both builds, compared per channel. `image-check <branch> --against <main>` counts
    the changed pixels.
  - **Unchanged by the fixes.** All eight rows above came out identical to the digit. So the
    review fixes, including the motion write mask (amendment 1 item 16), move no tiered
    pixel.
  - **How much changes.** 3.0–5.1 % of pixels change: `image-check` counts 7,000 (TAAU low)
    to 11,729 (FSR 3.1 overlay) of 230,400. Most changes are one or two steps. In TAAU low,
    5,114 of the 7,000 changed pixels move by 1–2, and 1,886 by more than 2.
  - **Where.** Mapping the changes (FSR 3.1 native, kept with the raw logs) puts every
    change above 2 on the two water strips, apart from a few isolated pixels. The 1–2 step
    changes are spread over the ground, which fits the depth-reconstructing passes (GTAO,
    contact shadows, the water fade) reading a reversed `D32_SFLOAT` depth whose precision
    falls differently.
  - **The cause** is inferred, not instrumented. Reversed-Z stores the near-coplanar water
    surface and bed with different float precision, so the fade's thresholds land a few
    texels differently. The geometry and the shadows match.
  - **The G-buffer adds no change of its own.** These numbers equal those of a
    reversed-Z-only build of the branch, captured before the G-buffer pass existed.
- **Tiered goldens** (`scripts/tiered-goldens.sh`, `UPSCALERS="taau fsr3"`, local lavapipe):
  - every pair of runs is byte-identical;
  - the 64×36 fingerprints pass unchanged (worst `max_diff` 5, mean ≤ 0.093);
  - the mid-stream history reset (40 frames, every 5th reset) is validation-clean for both
    back ends.

  The exact hashes in `tests/golden/render-tiered-hashes.txt` are keyed to CI's lavapipe
  (Mesa 26.0.8, LLVM 21.1.8) and the runner's CPU class
  ([ADR-0021 amendment 6](../adr/0021-temporal-upscaling-overlay.md#amendment-6-2026-11-14-the-tiered-exact-hashes-per-cpu-class)).
  Both sections were filled from CI per amendment 6 item 5, after the branch merged main
  (`9325ea3`), never from a local run:
  - **Both classes:** the `tiered-hash-sections` artifact of the dispatch-only sampler, render
    golden run 43657910589 at `ebd43a7`. Its eight legs agreed within each class, and every
    capture of both back ends was present (none `# FAILED`).
  - **Cross-check:** the PR's own `tiered-temporal` run on `ebd43a7` (render golden run
    43657881765, an AMD EPYC 9V74) wrote the same AMD FSR 3.1 section. The `sha256sum` of its
    downloaded `fsr3-*-1.ppm` captures gives the same values.
  - **No local hashes:** `WRITE_HASHES` was not run locally.

  All eight hashes of both classes changed (reversed-Z), and the 64×36 fingerprints pass
  unchanged. Before amendment 6, this PR's runs flipped between two values per capture. The
  flip is the CPU split and nothing else:
  - The Intel section is exactly the set first committed from run 41679033229 (TAAU low
    `64e7f21c…`, FSR 3.1 low `7028ce84…`).
  - The AMD section is exactly what `tiered-temporal` printed when it failed on `504013a`
    (FSR 3.1 low `90364321…`; TAAU low `02f3c849…`).
  - Every run was byte-identical to its own second run.
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

Re-timed on 2026-11-15, after the reviews, at the branch's review-fix commit (merged with
origin/main `ab42718`), against a detached build of `ab42718` in its own temporary worktree.
- **Builds:** both are release builds with the default `fsr3` feature.
- **Machine:** the RTX 5070 Ti (driver 620.70.06, Linux 7.3.4).
- **Replay:** the teamfight replay (`MOBA_WARMUP_TICKS=330`) at 1920×1080, vsync off, the
  240 fps limit, validation off, the pipeline cache off, 3000 frames per run (the log covers
  the last 2940).
- **Rounds:** three. Each held `flock handoff/gpu.lock` and ran every configuration on main
  and then the branch, with no other `moba-client` running (checked before each run).
- **Results:** medians of the three rounds, in ms; the three rounds' GPU totals agree within
  0.003 ms. The raw logs are retained locally in `~/Projects/MOBA/handoff/evidence/n2a-ab/`
  (not committed).
- **The earlier A/B** (2026-11-03, against `657a7ab`) measured the same deltas at about 0.03 ms
  higher absolute totals. The drop is the machine's stack: this A/B ran on driver 620.70.06
  and Linux 7.3.4, the earlier one on Linux 7.3.0. Both arms of each A/B ran on the same
  stack, so the ratios stand. The earlier A/B is superseded by this one.

| Configuration | GPU total p50 | GPU total p95 | `prepass` p50 | `scene` p50 | `temporal` p50 | Frame p95 |
|---|---|---|---|---|---|---|
| Tiered Low (TAAU), main | 0.506 | 0.519 | 0.020 | 0.079 | 0.125 | 4.241 |
| Tiered Low (TAAU), branch | 0.548 | 0.560 | 0.062 | 0.076 | 0.125 | 4.242 |
| Tiered High (TAAU), main | 0.557 | 0.570 | 0.024 | 0.090 | 0.125 | 4.242 |
| Tiered High (TAAU), branch | 0.604 | 0.617 | 0.068 | 0.089 | 0.125 | 4.238 |
| Tiered Ultra (TAAU), main | 0.558 | 0.571 | 0.024 | 0.090 | 0.125 | 4.240 |
| Tiered Ultra (TAAU), branch | 0.604 | 0.615 | 0.068 | 0.089 | 0.125 | 4.242 |
| Tiered Low (FSR 3.1), main | 0.656 | 0.670 | 0.020 | 0.079 | 0.278 | 4.238 |
| Tiered Low (FSR 3.1), branch | 0.698 | 0.711 | 0.062 | 0.077 | 0.278 | 4.241 |
| Tiered High (FSR 3.1), main | 0.706 | 0.720 | 0.024 | 0.090 | 0.277 | 4.241 |
| Tiered High (FSR 3.1), branch | 0.753 | 0.767 | 0.068 | 0.089 | 0.278 | 4.240 |
| Compat Ultra, main | 0.672 | 0.684 | 0.058 | 0.246 | — | 4.189 |
| Compat Ultra, branch | 0.672 | 0.684 | 0.058 | 0.246 | — | 4.190 |

**Against ADR-0023 §9b's 1.10× gate** (the N2 tiered frame against the N1 tiered forward
frame; GPU total, branch ÷ main):

| Preset | p50 ratio | p95 ratio | Headroom to 1.10× at p50 |
|---|---|---|---|
| Tiered Low (TAAU) | 1.083 | 1.079 | 0.009 ms (1.7 % of N1) |
| Tiered High (TAAU) | 1.084 | 1.082 | 0.009 ms (1.6 %) |
| Tiered Ultra (TAAU) | 1.082 | 1.077 | 0.010 ms (1.8 %) |
| Tiered Low (FSR 3.1) | 1.064 | 1.061 | 0.024 ms (3.6 %) |
| Tiered High (FSR 3.1) | 1.067 | 1.065 | 0.024 ms (3.3 %) |
| Compat Ultra | 1.000 | 1.000 | not gated (Compat is not N2) |

- **The gate is for the finished N2 frame**, not for N2a alone. N2a passes it at every
  preset, but it spends most of the margin: about **0.01 ms of headroom is left at the TAAU
  presets** (0.024 ms with FSR 3.1, whose own cost makes the ratio smaller).
- **Where N2b–N2f find their cost.** N2b's deferred lighting has to replace the forward
  `scene` shading (0.076–0.089 ms here) rather than add to it, and N2c–N2f (materials,
  LTC lights, rims, transmission) then fit in what that frees plus the 0.01 ms. N2b should
  measure against this table before it lands. The G-buffer writes can only get cheaper
  (N2c's bindless materials drop the per-draw binds).
- **Reversed-Z costs nothing measurable.** The `scene` lane, which runs the same draws with
  the reversed compare, is 0.001–0.003 ms faster. `temporal` is unchanged for TAAU (0.125)
  and for FSR 3.1, which now builds its inverted-depth permutation (0.277–0.278).
- **The G-buffer writes cost +0.042–0.044 ms in the `prepass` lane** (0.020 → 0.062 at Low,
  0.024 → 0.068 at High and Ultra). That is all of the +0.042–0.047 ms in the GPU total. The
  pass writes 12 more bytes per pixel at render resolution and binds the textured materials.
  The frame time does not move: these frames are limited at 240 fps.
- **`DONT_CARE` stores (amendment 1 item 16) change nothing on this GPU.** The prepass lane
  is 0.062/0.068 against 0.065/0.073 in the earlier A/B, which is within that A/B's higher
  absolute level. The NVIDIA driver writes the targets out either way; the store op is for
  tile-based and compressing GPUs, which are not measured here.
- **perf-gate** against `tests/perf/baseline.toml` passes every TAAU branch log:
  - Tiered High at 0.618 ms against an allowed 0.747, Ultra at 0.615 against 0.749;
  - Tiered Low at 0.563 ms against an allowed 0.689; only its report-only CPU-render line
    flags;
  - the FSR 3.1 logs are checked against the TAAU baselines (FSR 3.1 has none of its own)
    and flag only report-only lines.
- **Compat Ultra trips perf-gate's CPU-render p95 limit (0.318 ms) on both builds**: main in
  rounds 1 and 3 (0.320, 0.321), the branch in rounds 1–3 (0.318–0.319). That is this
  machine's CPU today, not the branch: Compat's GPU numbers are identical, and N2a changes no
  Compat code.
- **`locked_uploads=0`, `locked_pipelines=0` and `frames_dropped=0`** in every run.
- **Pipelines:**
  - On the RTX the replay builds **46 tiered pipelines on main and 48 on the branch** with
    TAAU, and 54 → 56 with FSR 3.1. Compat stays at 42.
  - On lavapipe at 640×360 High it is 45 → 47 with TAAU and 53 → 55 with FSR 3.1.
  - The two new pipelines are the textured unit and skinned G-buffer variants, because the
    textured materials now feed the G-buffer.
  - §1e's drop to about one pipeline per vertex format comes with N2c's bindless materials
    (amendment 1 item 5).
- **VRAM against §1b:**
  - `targets_mb` rises by **11.2–11.3 MB** at every tiered preset (132.0 → 143.2 at Low and
    180.5 → 191.8 at High and Ultra), and `vram_peak_mb` by 11.3 MB.
  - §1b predicts 11.1 MB at the 1280×720 ceiling of a 1080p output (12 B/px × 0.92 M px).
  - **At native scale** (TAAU or FSR 3.1 native, render = output), the G-buffer is the output
    size:

    | Output | G-buffer at native scale |
    |---|---|
    | 1920×1080 | 24.9 MB |
    | 2560×1440 | 44.2 MB |
    | 3840×2160 | 99.5 MB |

    §1b's 11–25 MB envelope assumes a render-scale ceiling of 0.67 or less at 1440p and 4K.
    Native at 4K is about 4× that. Even so, it stays far inside the per-preset VRAM budgets
    perf-gate checks (4–12 GB).
  - On lavapipe the same growth is 1.2 MB at 427×240 and 2.7 MB at 640×360. Compat does not
    change.
