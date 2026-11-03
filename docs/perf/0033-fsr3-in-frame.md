# FSR 3.1 in the frame: gates 3, 4 and 6 and Q1 (N1b stage 2)

Captured 2026-11-03 for ADR-0021 (N1b stage 2) and its
[Amendment 4](../adr/0021-temporal-upscaling-overlay.md#amendment-4-2026-11-03-n1b-stage-2-fsr-31-in-the-frame).
The port itself is [perf/0030](0030-fsr3-port.md) and [dev/fsr3.md](../dev/fsr3.md).

- GPUs: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08 (timings, smoke); AMD Radeon 610M on
  RADV (smoke); software: lavapipe (Mesa llvmpipe) with the Khronos validation layer,
  `MOBA_VULKAN_VALIDATION=required`
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-5-cachyos, Wayland
- Toolchain: rustc 1.99.1 stable, the pinned `slangc` 2026.20
- Branch `feat/n1b-fsr3-stage2`; baseline: origin/main `761beb3` (N1a merged), built detached
  in its own worktree, for gate 6 and the Compat goldens

Every lavapipe run below printed no `[vulkan` line: validation and synchronisation validation
were clean.

## Gate 3: readability

**Ghost energy** (`scripts/ghost-probe.sh`, now over `UPSCALERS="taau fsr3"`; lavapipe,
1280×720, Tiered High, fixed exposure, dynamic resolution off; the probe itself is
[perf/0031](0031-n1a-temporal.md#gate-3-readability)). Worst frame per mode; the gated modes
are Quality, Performance and native, and Balanced and Ultra Performance were run once by hand
for the trend:

| Back end | Mode | Render | Wake (bound 3 %) | Hero interior (bound 1.75 %) | Particle core (bound) |
|---|---|---|---|---|---|
| TAAU | Native | 1280×720 | 0.573 % | 0.00 % | 1.40 % (7.5 %) |
| TAAU | Quality | 853×480 | 1.135 % | 0.73 % | 3.77 % (7.5 %) |
| TAAU | Balanced | 753×424 | 1.328 % | 1.02 % | 5.37 % (7.5 %) |
| TAAU | Performance | 640×360 | 1.975 % | 1.37 % | 5.88 % (7.5 %) |
| TAAU | Ultra Performance | 427×240 | **4.539 %** | **4.30 %** | **8.64 %** (7.5 %) |
| FSR 3.1 | Native | 1280×720 | 0.402 % | 0.00 % | 4.19 % (7.5 %) |
| FSR 3.1 | Quality | 853×480 | 0.411 % | 0.00 % | 6.63 % (7.5 %) |
| FSR 3.1 | Balanced | 753×424 | 0.575 % | 0.02 % | **12.80 %** (7.5 %) |
| FSR 3.1 | Performance | 640×360 | 0.840 % | 0.49 % | 20.26 % (25 %) |
| FSR 3.1 | Ultra Performance | 427×240 | 1.921 % | 1.04 % | **30.57 %** (25 %) |

TAAU's rows are N1a's numbers again, unchanged. FSR 3.1 leaves less wake and smears the
hero's interior less than TAAU at every mode, but its particle core smears more as the ratio
grows, past TAAU's 7.5 % tripwire from Balanced down. Why (amendment 4, item 6): FSR 3.1
dilates motion vectors by the nearest depth in each 3×3 neighbourhood, and particles write no
depth, so a projectile's core takes the ground's motion in places; and its reactive input
lowers the accumulation only through the shading-change term. A local experiment that bound
the reactive mask as the transparency-and-composition mask too left every share unchanged
(`diag/` in the evidence, not committed). The probe gives FSR 3.1 its own particle bound,
`FSR3_PARTICLE_BOUND` = 25 %, from ratio 2 (Performance) down, and keeps 7.5 % at Quality
and native. **This is a readability shortfall of FSR 3.1**, and it decides Q1 (below).

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
- **Tiered, FSR 3.1:** the script also renders the same four scenes with
  `MOBA_UPSCALER=fsr3`, twice each, and requires each pair to be byte-identical and the log
  to name `upscaler fsr3`. **They are not committed as fingerprints** (Q8, amendment 4
  item 7: the coordinator's reading keeps the committed goldens to TAAU). What was checked
  instead, SHA-256 prefixes of the two runs:

  | Scene | Run 1 | Run 2 |
  |---|---|---|
  | low | `7a7bd71f0b20329c` | `7a7bd71f0b20329c` |
  | high | `f2afb1344a2df894` | `f2afb1344a2df894` |
  | native | `097231414773d520` | `097231414773d520` |
  | overlay | `656621943b313140` | `656621943b313140` |

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
| lavapipe | stale previous render size | 0.785× | **−10.89 %, −11.92 %** |
| RTX 5070 Ti | clean | 0.784× | −0.97 %, −1.65 % |
| RTX 5070 Ti | previous depth not cleared | **0.969×** | **−28.86 %, −27.92 %** |
| RTX 5070 Ti | stale previous render size | 0.784× | **−10.89 %, −11.92 %** |
| Radeon 610M (RADV) | clean | 0.786× | −0.97 %, −1.65 % |
| Radeon 610M (RADV) | previous depth not cleared | **0.969×** | **−28.86 %, −27.92 %** |
| Radeon 610M (RADV) | stale previous render size | 0.786× | **−10.99 %, −12.01 %** |

The "old trail" is the box's trail over 40 frames old; its error falls to 0.78× of a fresh
frame's when FSR's reconstructed previous depth is cleared, and stays near 1× when it is not.
Every faulted run failed the smoke and every clean run passed it, with validation at 0
errors and 0 warnings on all three devices. The smoke is now 526 frames and 133 checks; its
output is bit-identical across two runs, the first with poisoned internal images, with
digests `d2cb4f37fed22fec` (lavapipe), `eac7fe4415f24edc` (RTX 5070 Ti) and
`0e09d3d96bf481ba` (RADV).

## Gate 6 and Q1: FSR 3.1 against TAAU on the RTX 5070 Ti

The teamfight replay (`tests/replays/teamfight-5v5.mbr`, 330 warm-up ticks) at 1920×1080,
release builds, validation off, `MOBA_VSYNC=0`, `MOBA_PIPELINE_CACHE=off`,
`MOBA_SETTINGS=/dev/null`, 3 000 frames. Three rounds, interleaved: in each round every
configuration ran once, in the order main, branch TAAU, branch FSR 3.1 (High, then Low), then
Compat Ultra on main and the branch. Values are medians of the three rounds (range in
brackets where wider than 0.004 ms). Every tiered run used its tier defaults: temporal AA,
Quality, render scale 0.656 (1260×709; dynamic resolution held the ceiling), Reflex on.

| Path, tier | Build, back end | GPU total p50, ms | p95, ms | `temporal` p50, ms | VRAM peak | `upscaler_vram_mb` | render p50 / p95, ms |
|---|---|---|---|---|---|---|---|
| Tiered High | main, TAAU | 0.591 (0.588–0.594) | 0.602 | 0.145 | 242.4 MB | 38.7 | 0.184 / 0.340 |
| Tiered High | branch, TAAU | 0.591 (0.589–0.595) | 0.601 | 0.145 | 242.4 MB | 38.7 | 0.185 / 0.340 |
| Tiered High | branch, FSR 3.1 | 0.699 | 0.715 | 0.281 | 276.6 MB | 75.4 | 0.196 / 0.367 (0.312–0.367) |
| Tiered Low | main, TAAU | 0.542 | 0.552 | 0.145 | 180.6 MB | 38.7 | 0.184 / 0.342 |
| Tiered Low | branch, TAAU | 0.542 | 0.552 | 0.145 | 180.6 MB | 38.7 | 0.184 / 0.334 (0.320–0.345) |
| Tiered Low | branch, FSR 3.1 | 0.650 | 0.700 (0.691–0.706) | 0.280 | 214.8 MB | 75.4 | 0.195 / 0.345 (0.338–0.362) |
| Compat Ultra | main | 0.662 | 0.712 (0.676–0.713) | – | 292.9 MB | 0 | 0.169 / 0.319 |
| Compat Ultra | branch | 0.662 | 0.722 (0.671–0.724) | – | 292.9 MB | 0 | 0.169 / 0.319 |

`locked_uploads` and `locked_pipelines` were 0 in every run and no frame was dropped. Frame
time was paced (Reflex at the 240 fps limit on Tiered, the compositor on Compat): p50
4.220 ms on every tiered run and 4.178–4.179 ms on Compat, p99 at most 4.256 ms.

- **The branch's default frame is main's.** With `auto` still TAAU, the branch's TAAU runs
  are within 0.001 ms and 0 MB of main's on both tiers, and Compat is unchanged, so the
  tiered baselines N1a recorded still describe the default frame (amendment 4, item 12).
- **FSR 3.1 costs 0.136 ms more than TAAU** in the `temporal` lane (0.281 against 0.145 ms;
  each within 0.002 ms across rounds), and 0.108 ms more in GPU total on both tiers. The
  other lanes were each 0.001–0.005 ms lower under FSR 3.1 (bloom −0.005, water −0.004,
  scene −0.003 on High), together about 0.028 ms; nothing else in the frame differs between
  the two runs, so this is probably the GPU's clocks under the heavier load, but it was not
  measured. **Q1 is decided on the 0.136 ms of the lane**, not on the smaller total.
- **Memory.** FSR 3.1's own memory is 75.4 MB at a 1080p output against TAAU's 38.7 MB; the
  frame's peak is 34.2 MB higher, because TAAU's history and previous-depth pairs are not
  allocated while FSR 3.1 runs (amendment 4, item 5).
- **Render CPU** is 0.011 ms higher at p50 under FSR 3.1 (0.195–0.196 against 0.184–0.185 ms):
  its record dispatches up to nine compute passes and rewrites every descriptor in them that
  names one of the frame's images (at most 32), where TAAU writes 12 (perf/0031). How the
  0.011 ms splits was not profiled. The p95s spread by up to 0.055 ms across rounds.

**Q1: `auto` stays TAAU.** Q1 gives FSR 3.1 the open build's `auto` slot when it passes the
same readability tests as TAAU and costs within 0.2 ms of it on this GPU. It is within the
cost (+0.136 ms in the lane, +0.108 ms in total), but it does not pass the particle tripwire
at Balanced and below (gate 3 above), so `AUTO_OPEN_BUILD` stays `Taau` and FSR 3.1 is
selectable with `MOBA_UPSCALER=fsr3`.

**FP32 against FP16 was not measured** (amendment 4, item 9): the SDK's 16-bit permutation
does not compile unmodified with the pinned `slangc`, so every figure here is the 32-bit
permutation's.

## Not verified here

- **FP16:** not built, so not timed (above). The Low tier's FSR cost on the parts that would
  take the 16-bit path (Turing, RDNA 2) is unknown.
- **The RTX 2060 (Low reference class), 1440p, Windows and macOS** were not run (#9, #127).
  Every timing is at 1080p on the RTX 5070 Ti; macOS is Compat-only.
- **The fallback to TAAU on a device without FSR 3.1's features** is code-reviewed and logged,
  but no device here lacks them, so it was not observed running.
- **FSR 3.1's particle smear** is measured, not fixed: tuning its reactiveness or velocity
  factors, or giving particles depth, is the follow-up if FSR 3.1 is to take `auto`.
- **FSR 3.1's tiered fingerprints** are checked for run-to-run identity but not committed
  (Q8 above), so a change to FSR's output would not fail CI unless it also broke determinism
  or the smoke's error bounds.
- **Machine state:** the timings ran on a working desktop, not an idle one; the three rounds
  agree within the ranges shown. The tiered baselines' flip stays with the last N1 part, on
  a quiet machine.
