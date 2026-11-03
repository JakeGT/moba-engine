# FSR 3.1 in the frame: gates 3, 4 and 6 and Q1 (N1b stage 2)

Captured 2026-11-03 for ADR-0021 (N1b stage 2) and its
[Amendment 4](../adr/0021-temporal-upscaling-overlay.md#amendment-4-2026-11-03-n1b-stage-2-fsr-31-in-the-frame).
The port itself is [perf/0030](0030-fsr3-port.md) and [dev/fsr3.md](../dev/fsr3.md).

- GPUs: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08 (timings, ghost probe, smoke); AMD
  Radeon 610M on RADV (Low-class timing, ghost probe, smoke); software: lavapipe (Mesa llvmpipe) with the Khronos validation layer,
  `MOBA_VULKAN_VALIDATION=required`
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-5-cachyos, Wayland
- Toolchain: rustc 1.99.1 stable, the pinned `slangc` 2026.20
- Branch `feat/n1b-fsr3-stage2`; baseline: origin/main `761beb3` (N1a merged), built detached
  in its own worktree, for gate 6 and the Compat goldens

Every lavapipe run below printed no `[vulkan` line: validation and synchronisation validation
were clean.

## Gate 3: readability

**Ghost energy** (`scripts/ghost-probe.sh` over `UPSCALERS="taau fsr3"`, 1280×720, Tiered
High, fixed exposure, dynamic resolution off; the probe itself is
[perf/0031](0031-n1a-temporal.md#gate-3-readability)), **on three devices**: lavapipe, the
RTX 5070 Ti and the Radeon 610M on RADV, with validation required. Worst frame per mode; the
gated modes are Quality, Performance and native, and Balanced and Ultra Performance were run
for the trend.

**Before the fix (review round 1 of #364): CI's lavapipe passed what both GPUs failed.**
Particle core smear, the share of the projectile's core differing from a no-history render:

| Device | TAAU native | TAAU Quality | TAAU Performance | FSR 3.1 native | FSR 3.1 Quality | FSR 3.1 Performance |
|---|---|---|---|---|---|---|
| lavapipe | 1.40 % | 3.77 % | 5.88 % | 4.19 % | 6.63 % | 20.26 % |
| RTX 5070 Ti | **19.52 %** | **27.55 %** | **35.78 %** | 6.61 % | **32.70 %** | **39.72 %** |
| Radeon 610M | **18.71 %** | **27.75 %** | **35.40 %** | 6.11 % | **29.13 %** | **40.25 %** |

N1a's rule had particle fragments write motion through the colour draw, blended by an output
alpha of `step(0.5, α)` into the two-channel `R16G16_SFLOAT` motion target. Both GPUs take
that source alpha from the shader and kept the ground's motion under most of a soft sprite;
lavapipe wrote every fragment's motion regardless (and wrote the target even with a zero
write mask while it shared the draw with other targets). Amendment 4 item 13 replaces it with
a motion-only draw of the particles in its own rendering scope, blending off, each fragment
writing its own motion where its opacity reaches 0.1 and clearing its sample mask below.

**After the fix** (wake bound 3 %, hero interior 1.75 %, particle bound in the header):

| Back end | Mode | Wake, lavapipe / RTX / RADV | Hero interior | Particle core | Bound |
|---|---|---|---|---|---|
| TAAU | Native | 0.573 / 0.611 / 0.579 % | 0.00 / 0.00 / 0.00 % | 1.40 / 1.23 / 1.27 % | 7.5 % |
| TAAU | Quality | 1.123 / 1.222 / 1.259 % | 0.73 / 0.73 / 0.69 % | 3.77 / 3.52 / 3.75 % | 7.5 % |
| TAAU | Balanced | 1.304 / 1.289 / 1.296 % | 1.02 / 1.16 / 1.05 % | 5.37 / 5.41 / 5.70 % | (7.5 %) |
| TAAU | Performance | 1.927 / 2.031 / 1.936 % | 1.37 / 1.33 / 1.35 % | 5.88 / 6.52 / 6.56 % | 7.5 % |
| TAAU | Ultra Performance | **4.373 / 4.375 / 4.409 %** | **4.30 / 4.76 / 4.51 %** | **8.64 / 8.41 / 8.80 %** | (7.5 %) |
| FSR 3.1 | Native | 0.317 / 0.307 / 0.310 % | 0.00 / 0.00 / 0.00 % | 4.19 / 3.81 / 3.94 % | 7.5 % |
| FSR 3.1 | Quality | 0.390 / 0.466 / 0.384 % | 0.00 / 0.00 / 0.02 % | 6.63 / 6.70 / 6.82 % | 7.5 % |
| FSR 3.1 | Balanced | 0.462 / 0.542 / 0.455 % | 0.02 / 0.04 / 0.02 % | 7.35 / 6.96 / 7.23 % | (7.5 %) |
| FSR 3.1 | Performance | 0.842 / 0.966 / 0.814 % | 0.49 / 0.46 / 0.49 % | 11.64 / 11.51 / 12.33 % | 15 % |
| FSR 3.1 | Ultra Performance | 1.893 / 2.035 / 1.909 % | 1.04 / 1.04 / 0.99 % | 21.81 / 22.34 / 21.78 % | (15 %) |

Bounds in brackets are not gated. The three devices now agree within about 0.7 points.

- **TAAU** keeps its 7.5 % tripwire: its worst gated share is 6.56 % (RADV, Performance).
- **FSR 3.1** leaves less wake than TAAU and smears the hero's interior no more, but smears the
  particle core more at every ratio. It keeps 7.5 % at Quality and native (worst 6.82 %) and
  gets `FSR3_PARTICLE_BOUND` = **15 %** from Performance down (worst 12.33 %; the bound was
  25 % before the fix, when lavapipe's 20.26 % was the only figure). Why it smears more is
  **not known** (amendment 4 item 6): the review found the smear byte-identical with FSR's
  motion-vector dilation reduced to the centre pixel, so the nearest-depth dilation is ruled
  out, and the reactive mask is wired as §2 says (zeroing it smears more; binding it as the
  transparency-and-composition mask too changed nothing).
- **The mutation: losing particle motion still fails.** With the motion draw's coverage
  threshold set to 2.0 (no fragment writes motion), on lavapipe / RTX / RADV:
  TAAU native 18.36 / 20.72 / 19.84 %, Quality 29.96 / 31.59 / 32.23 %, Performance
  45.17 / 45.55 / 44.84 %; FSR 3.1 Quality 29.19 / 33.84 / 29.64 %, Performance
  57.97 / 54.41 / 55.26 %, all failing. FSR 3.1 native does not catch it
  (4.67 / 6.17 / 5.24 %, passing).
- **The threshold** (RTX, TAAU Quality / Performance / native): 0.25 gives
  8.95 / 11.00 / 8.13 %; 0.1, 0.02 and 0.004 all give 3.52 / 6.52 / 1.23 %. FSR 3.1
  Performance is 11.51 % at 0.1, 12.04 % at 0.02 and 16.85 % at 0.004, so 0.1 it is.

**The ghost probe must run on a real GPU, not only lavapipe** (amendment 4 item 14): the bug
above passed CI for N1a and stage 2. CI's lavapipe run stays as a tripwire; the gate is the
hardware run, repeated whenever particle, motion or upscaler code changes. The script also
fails a run whose log does not name the back end it asked for.

**Overlay invariance** (`scripts/overlay-invariance.sh`): the overlay-ID view on Tiered High
with the cast-flash preview is byte-identical across FSR 3.1 Quality, native and Performance
and the six earlier cases (TAAU Quality, native, Performance, FXAA, FXAA at render scale 0.6
and no AA): nine captures, one hash (`4adde3f7af91db8c`). Each FSR case checks its log for
`upscaler fsr3`, so a build that fell back to TAAU fails it.

## Gate 4: goldens and determinism

- **Compat:** the seven scenes of the stage-1 review (default, network, Ember, VAT, Ultra,
  Low scaled, decals) are byte-identical between origin/main `761beb3` and the branch, run
  twice (lavapipe, SHA-256 of the PPMs; `golden/summary.txt` in the evidence).
- **Tiered, TAAU:** `scripts/tiered-goldens.sh` renders low, high, native and overlay twice
  each; every pair is byte-identical and matches its committed fingerprint, unchanged from
  N1a.
- **Tiered, FSR 3.1:** the script renders the same four scenes with `MOBA_UPSCALER=fsr3`,
  twice each, requires each pair to be byte-identical and the log to name `upscaler fsr3`,
  and **compares them with committed fingerprints**
  (`tests/golden/render-tiered-fsr3-{low,high,native,overlay}-fingerprint.rgb`, Q8, amendment
  4 item 7). The fingerprints were written from CI's output for `c0ff307`, the reference.
- **Why CI's bytes differ from a local run.** SHA-256 prefixes of the captures:

  | Scene | CI (lavapipe, LLVM 21.1.8) | Local (lavapipe, LLVM 23.1.3) |
  |---|---|---|
  | FSR 3.1 low | `3a0c1b01…` | `7a7bd71f…` |
  | FSR 3.1 high | `41d3f992…` | `f2afb134…` |
  | FSR 3.1 native | `80729430…` | `09723141…` |
  | FSR 3.1 overlay | `bf074c91…` | `65662194…` |
  | TAAU low | `31b525c6…` | `91f51690…` |

  llvmpipe compiles shaders with LLVM, and the two versions generate different code, so the
  raw bytes differ for TAAU as much as for FSR 3.1; both are byte-identical run to run on
  either. The fingerprints absorb it: the local captures compare with CI's fingerprints at a
  mean of 0.066–0.088 and a max of 1–2 for FSR 3.1 (TAAU 0.059–0.074, max 2), where the check
  allows 2.0 and 24. TAAU's fingerprints were written locally in N1a and pass on CI the same
  way. The particle-motion fix leaves every tiered capture unchanged (TAAU still matches at
  mean 0.000 locally; the FSR 3.1 hashes above are the same before and after it).
- **Mid-stream history reset:** 40 frames of High with `MOBA_TEMPORAL_RESET_EVERY=5`, now
  under TAAU and FSR 3.1 each, with synchronization validation: 0 messages. As for TAAU, this
  is a validation smoke, not a regression check for the reset barrier (perf/0031); FSR 3.1's
  first-frame barrier is pinned by `first_frame_barrier_waits_for_earlier_work` instead.

## `fsr3-smoke`: the stage-1 gaps (amendment 4, item 10)

Two occluder runs join the smoke: a box at depth 0.3 crosses the still field at depth 0.6 for
60 frames, with motion on the box only, so the field it uncovers is disoccluded. The second
run steps the render size from Performance to Quality while the box moves, over a coarse
checkerboard. Two checks per run, each tested by injecting the fault it is for:

| Device | Run | Old trail ÷ one-frame error (≤ 0.9) | Below the box after the step (within 5 %) |
|---|---|---|---|
| lavapipe | clean | 0.785× | −0.97 %, −1.66 % |
| lavapipe | previous depth not cleared each frame | **0.969×** | **−28.86 %, −27.92 %** |
| lavapipe | previous render size set to the current one | 0.785× | **−10.89 %, −11.92 %** |
| RTX 5070 Ti | clean | 0.784× | −0.97 %, −1.65 % |
| RTX 5070 Ti | previous depth not cleared | **0.969×** | **−28.86 %, −27.92 %** |
| RTX 5070 Ti | previous render size set to the current one | 0.784× | **−10.89 %, −11.92 %** |
| Radeon 610M (RADV) | clean | 0.786× | −0.97 %, −1.65 % |
| Radeon 610M (RADV) | previous depth not cleared | **0.969×** | **−28.86 %, −27.92 %** |
| Radeon 610M (RADV) | previous render size set to the current one | 0.786× | **−10.99 %, −12.01 %** |

The lavapipe rows were re-run for review round 1 with temporary environment-gated hooks
(removed again; logs in `r1/smoke-faults/` of the evidence). A previous render size that is
never written at all (left at zero) is **not** caught (lavapipe −1.81 %, −3.41 %): the check
catches a size that misses the step, not one the SDK may treat as unknown.

The "old trail" is the box's trail over 40 frames old; its error falls to 0.78× of a fresh
frame's when FSR's reconstructed previous depth is cleared, and stays near 1× when it is not.
Every faulted run failed the smoke and every clean run passed it, with validation at 0
errors and 0 warnings on all three devices. The smoke is now 526 frames and 133 checks; its
output is bit-identical across two runs, the first with poisoned internal images, with
digests `d2cb4f37fed22fec` (lavapipe), `eac7fe4415f24edc` (RTX 5070 Ti) and
`0e09d3d96bf481ba` (RADV).

## Gate 6 and Q1: FSR 3.1 against TAAU on the RTX 5070 Ti

The teamfight replay (`tests/replays/teamfight-5v5.mbr`, 330 warm-up ticks) at 1920×1080,
release builds of the round-1 code, validation off, `MOBA_VSYNC=0`,
`MOBA_PIPELINE_CACHE=off`, `MOBA_SETTINGS=/dev/null`, 3 000 frames. Three rounds,
interleaved: in each round every configuration ran once, in the order main, branch TAAU,
branch FSR 3.1 (High, then Low), then Compat Ultra on main and the branch. Values are medians
of the three rounds (range in brackets where wider than 0.004 ms). Every tiered run used its
tier defaults: temporal AA, Quality, render scale 0.656 (1260×709; dynamic resolution held
the ceiling), Reflex on.

| Path, tier | Build, back end | GPU total p50 / p95, ms | `temporal` p50 / p95, ms | `vfx_draw` p50 / p95, ms | VRAM peak | `upscaler_vram_mb` |
|---|---|---|---|---|---|---|
| Tiered High | main, TAAU | 0.585 / 0.598 | 0.144 / 0.148 | 0.006 / 0.010 | 242.4 MB | 38.7 (formula) |
| Tiered High | branch, TAAU | 0.587 / 0.595 | 0.144 / 0.148 | 0.006 / 0.010 | 242.4 MB | 41.2 |
| Tiered High | branch, FSR 3.1 | 0.695 / 0.743 (0.709–0.748) | 0.278 / 0.305 (0.290–0.306) | 0.006 / 0.010 | 276.6 MB | 75.4 |
| Tiered Low | main, TAAU | 0.539 / 0.549 | 0.146 / 0.150 | 0.006 / 0.010 | 180.6 MB | 38.7 (formula) |
| Tiered Low | branch, TAAU | 0.541 / 0.550 | 0.147 / 0.151 | 0.007 / 0.010 | 180.6 MB | 41.2 |
| Tiered Low | branch, FSR 3.1 | 0.648 / 0.706 | 0.279 / 0.310 | 0.006 / 0.010 | 214.8 MB | 75.4 |
| Compat Ultra | main | 0.660 / 0.720 | – | 0.009 / 0.013 | 292.9 MB | 0 |
| Compat Ultra | branch | 0.660 / 0.720 | – | 0.009 / 0.013 | 292.9 MB | 0 |

`locked_uploads` and `locked_pipelines` were 0 in every run and no frame was dropped. Frame
time was paced (Reflex on Tiered, the compositor on Compat): p50 4.19–4.21 ms on every run.

- **The particle motion draw costs nothing measurable here.** `vfx_draw` (which now holds
  the second, motion-only particle pass) is 0.006–0.007 ms on the branch against 0.006 ms on
  main, and the branch's TAAU GPU total is within 0.002 ms of main's on both tiers; Compat is
  unchanged (it has no motion target). The teamfight peaks at about 690 live particles on the RTX runs
  (`vfx_alive_max`); the cost is one more vertex pass over them, so a heavier scene costs
  more.
- **TAAU's `upscaler_vram_mb` is now its allocations** (41.2 MB, the sum of its images'
  `allocated_bytes()`, the basis FSR 3.1's 75.4 MB uses), not the 38.7 MB formula main
  reports; the frame's peak is the same on both.
- **FSR 3.1 costs 0.134 ms more than TAAU at p50 and 0.157 ms more at p95** in the
  `temporal` lane at High (0.278 / 0.305 against 0.144 / 0.148 ms), and 0.132 / 0.159 ms at
  Low; 0.108 ms more in GPU total on both tiers. (The c0ff307 timings were 0.281 against
  0.145 ms; the review's p95 deltas from them were +0.144 High, +0.158 Low.) The other lanes
  are slightly lower under FSR 3.1, probably the GPU's clocks under the heavier load; this was
  not measured. **Q1 is decided on the lane**, and its p95 is within the 0.2 ms.
- **Memory.** FSR 3.1's own memory is 75.4 MB at a 1080p output against TAAU's 41.2 MB; the
  frame's peak is 34.2 MB higher, because TAAU's images are not allocated while FSR 3.1 runs.
  A resize builds FSR 3.1's new images before dropping the old ones, so it briefly holds
  about 2 × 75 MB.

**Q1: `auto` stays TAAU.** Q1 gives FSR 3.1 the open build's `auto` slot when it passes the
same readability tests as TAAU and costs within 0.2 ms of it on this GPU. It is within the
cost (+0.134 ms p50, +0.157 ms p95 in the lane), but on all three devices it smears the
particle core more than TAAU at every ratio and needs its own 15 % tripwire from Performance
down (gate 3 above), so `AUTO_OPEN_BUILD` stays `Taau` and FSR 3.1 is selectable with
`MOBA_UPSCALER=fsr3`.

## The Low class: FP32 FSR 3.1 against TAAU on the Radeon 610M

The SDK's 16-bit permutation does not compile unmodified with the pinned `slangc` (amendment
4 item 9), and no published AMD figure for its saving was found, so the 32-bit permutation
was timed against TAAU on the Radeon 610M (RADV, RDNA 2 with two compute units; a part on
which the SDK would choose the 16-bit path, so this is an upper bound for FSR 3.1 there).
Same replay and settings, Tiered Low, 1920×1080 output; dynamic resolution sat at its floor,
render scale 0.5 (960×540), for both back ends throughout. Three interleaved rounds, medians
(ranges within 0.06 ms):

| Back end | GPU total p50 / p95, ms | `temporal` p50 / p95, ms | `vfx_draw` p50, ms | VRAM peak | `upscaler_vram_mb` |
|---|---|---|---|---|---|
| TAAU | 19.96 / 20.21 | 9.51 / 9.52 | 0.091 | 187.3 MB | 39.4 |
| FSR 3.1 | 23.49 / 23.74 | 13.05 / 13.21 | 0.089 | 222.1 MB | 74.2 |

**FSR 3.1 costs +3.54 ms at p50 and +3.69 ms at p95 in the lane on this part**, about 37 %
more than TAAU, whose own 9.5 ms is already half the frame. **FSR 3.1 must not enter `auto` or
a Low preset without a measurement on a Low-class part** (the RTX 2060 or an RDNA 2 discrete
card), with FP16 if it is built by then; the review estimates the RTX 2060's lane delta at
0.3–0.5 ms, which was not measured.

## Not verified here

- **FP16:** not built, so not timed, and no AMD figure is cited (above).
- **The RTX 2060 (Low reference class), 1440p, Windows and macOS** were not run (#9, #127).
  Every RTX timing is at 1080p on the RTX 5070 Ti; macOS is Compat-only.
- **The fallback to TAAU on a device without FSR 3.1's features** is code-reviewed and logged,
  but no device here lacks them, so it was not observed running.
- **Why FSR 3.1 smears the particle core more** is not known (gate 3); tuning its
  reactiveness or velocity factors, or giving particles depth, is the follow-up if FSR 3.1 is
  to take `auto`.
- **The ghost probe on hardware is not in CI**: CI's lavapipe run is a tripwire; the hardware
  runs here are by hand.
- **`tiered-temporal`**, the new CI job that holds the ADR-0021 probes, is not a required
  check; making it one is a repository setting.
- **Machine state:** the timings ran on a working desktop, not an idle one; the three rounds
  agree within the ranges shown. The tiered baselines' flip stays with the last N1 part, on
  a quiet machine.
