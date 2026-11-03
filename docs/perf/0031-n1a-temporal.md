# Temporal foundation: gates 1–4, gate 6 and the tiered perf-gate split (N1a)

Captured 2026-11-03 for ADR-0021 (N1a) and its
[Amendment 2](../adr/0021-temporal-upscaling-overlay.md#amendment-2-2026-11-03-n1a-the-temporal-foundation-as-built).

- GPU: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08 (tiered path); software: lavapipe
  (Mesa llvmpipe) with the Khronos validation layer, `MOBA_VULKAN_VALIDATION=required`
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-5-cachyos, Wayland
- Toolchain: rustc 1.99.1 stable
- Branch `feat/n1a-temporal` (after the review fixes); baseline: origin/main `e9d5f91` (with
  N1b stage 1), built detached in its own worktree

Every lavapipe run below printed no `[vulkan` line: validation and synchronisation validation
were clean.

## Gate 1: jitter and picking

Unit tests, run by `scripts/agent-verify.sh`:

- `temporal.rs`: `halton_values_match_the_sequence`, `offsets_stay_in_the_half_open_pixel`,
  `phase_counts_follow_the_ceil_rule` (`ceil(8 r²)`), `the_index_survives_a_scale_step`,
  `jitter_moves_samples_right_and_down_in_pixels` (the sign test),
  `mip_bias_follows_each_back_end` and `camera_cuts_reset_the_history`.
- `upscale/frame.rs`: the jittered matrix never replaces the client's (`unjittered` is the
  client's `view_proj`), and `restart_returns_to_the_first_phase`.
- Client: `the_same_cursor_sends_the_same_command_under_every_aa_and_scale`. The same cursor
  sends byte-identical `WireCommand`s under temporal AA, FXAA and no AA, at render scales 0.5
  and 1.0.
- Capture warm-up: the two-tick capture under gate 4.

## Gate 2: motion against a CPU twin

`scripts/motion-probe.sh` (`MOBA_MOTION_PROBE=1`), lavapipe, render 853×480 (Quality,
1280×720 output), frame B jitter (−0.375, −0.056). Every class is within 1/64 px
(0.0156):

| Class | Low: pixels | Low: max error, px | High: pixels | High: max error, px | Mean motion, px |
|---|---|---|---|---|---|
| camera (terrain) | 393 043 | 0.0044 | 392 524 | 0.0044 | 4.97 |
| static instance | 2 097 | 0.0010 | 2 097 | 0.0010 | 1.92 |
| skinned (one joint) | 1 709 | 0.0080 | 1 709 | 0.0080 | 9.27 |
| VAT (frame 0 → 1) | 2 163 | 0.0043 | 2 163 | 0.0043 | 6.52 |
| foliage | 3 723 | 0.0043 | 4 242 | 0.0043 | 5.72 |
| `motion_reset` | 1 802 | 0.0041 | 1 802 | 0.0041 | 4.45 |
| key absent from A | 2 300 | 0.0043 | 2 300 | 0.0043 | 5.72 |
| key new in B | 2 512 | 0.0044 | 2 512 | 0.0044 | 5.46 |

91 pixels were left unclassified (silhouette pixels within the shell of two objects).

## Gate 3: readability

**Ghost energy** (`scripts/ghost-probe.sh`, lavapipe, 1280×720, High, fixed exposure,
dynamic resolution off). A hero-sized box dashing 10 m in 0.2 s at 120 fps, built as three
slabs (dark, light, dark) over mid-tone tiles so the clip cannot hide a wrong history fetch
on it, and a box projectile and a particle projectile at 0.7 m per frame. Over the last 20 of
24 frames, three shares of pixels that differ from the no-history reference by more than
4/255: the **wake** (covered in the previous 12 frames, more than 2 px from the object now,
antialiasing excluded), the **hero's interior** (off its silhouette and stripe edges) and the
**particle's core**. Worst frame per mode:

| Mode | Render | Wake (bound 3 %) | Hero interior (bound 1.75 %) | Particle core (bound 7.5 %) |
|---|---|---|---|---|
| Native | 1280×720 | 0.573 % | 0.00 % | 1.40 % |
| Quality | 853×480 | 1.135 % | 0.73 % | 3.77 % |
| Balanced | 753×424 | 1.328 % | 1.02 % | 5.37 % |
| Performance | 640×360 | 1.975 % | 1.37 % | 5.88 % |

**The bounds are regression tripwires, calibrated about 25 % above these measurements; they
are not perceptual thresholds.** The wake's 3 % was set over the first version of the probe
(a solid box and a 1 m/frame projectile: 2.39 % at Quality, 1.20 % native). Each was checked
by mutation on lavapipe:

| Mutation | Mode | Wake | Hero interior | Particle core | Result |
|---|---|---|---|---|---|
| Hero motion keys removed (no motion vectors) | Quality | 1.130 % | **2.59 %** | 3.77 % | fails |
| Hero motion keys removed | Performance | 1.927 % | **7.30 %** | 5.88 % | fails |
| Hero motion keys removed | Native | 0.577 % | 0.14 % | 1.40 % | passes |
| Particle motion output zeroed (`vfx.frag`) | Quality | 1.090 % | 0.73 % | **29.96 %** | fails |

The wake does not see a missing motion vector (it is unchanged under the first mutation):
TAAU's clip and disocclusion test rescue the trail. The on-object measures do, except at
native, where a missing vector barely smears (0.14 %). The wake's residue is the moving
box's shadow trailing edge and thin grid lines re-converging.

**Overlay invariance** (`scripts/overlay-invariance.sh`): the overlay-ID debug view on Tiered
High with the cast-flash preview is byte-identical across TAAU Quality, Native and
Performance, FXAA, FXAA at render scale 0.6 and no AA (6 cases, IDs include the slot-2 hero's
2 719 pixels).

**Outlines** (`scripts/outline-probe.sh` with `MOBA_RENDER_PATH=tiered`): all six open/hidden
cases pass.

**Telegraph contrast** (`enemy_telegraphs_contrast_in_display_space`): over display
backgrounds 0–1 and three enemy tints the rim or the halo reaches 1.5:1 everywhere; worst
contrast 1.511, worst rim lift 1.266 where the tint has headroom.

## Gate 4: goldens and determinism

- **Compat:** all 15 render-golden scenes are byte-identical between origin/main and the
  branch (lavapipe, `cmp` of the PPMs).
- **Tiered:** `scripts/tiered-goldens.sh` renders low, high, native and overlay (the two-tick
  capture, network scene with a skinned hero) at 640×360 twice each; every pair is
  byte-identical and matches its committed fingerprint (`mean_abs_diff=0.000 max_diff=0`).
  The tiered goldens are new: MSAA is retired on Tiered, so there is no earlier tiered image
  to compare with.

## Gate 6: performance on the RTX 5070 Ti

The teamfight replay (`tests/replays/teamfight-5v5.mbr`, 330 warm-up ticks) at 1920×1080,
release builds, validation off, `MOBA_VSYNC=0`, `MOBA_PIPELINE_CACHE=off`, 3 000 frames (the
log covers the last 2 940). Three rounds, interleaved: in each round every configuration ran
on origin/main `e9d5f91` and then on the branch (after the review fixes). Values are the
medians of the three rounds (range in brackets where it is wider than 0.002 ms). The
branch's tiered runs use their tier defaults: temporal AA, TAAU Quality, render scale 0.656
(dynamic resolution held the ceiling throughout).

**GPU and memory:**

| Path, tier | Build | AA | GPU total p50, ms | GPU total p95, ms | VRAM peak | `locked_uploads` / `locked_pipelines` |
|---|---|---|---|---|---|---|
| Tiered Low | main | FXAA, native | 0.486 | 0.494 | 136.0 MB | 0 / 0 |
| Tiered Low | branch | TAAU Quality | 0.499 | 0.507 | 180.6 MB | 0 / 0 |
| Tiered High | main | MSAA 4× | 0.654 (0.652–0.655) | 0.662 | 292.9 MB | 0 / 0 |
| Tiered High | branch | TAAU Quality | 0.551 | 0.558 | 242.4 MB | 0 / 0 |
| Tiered Ultra | main | MSAA 4× | 0.653 (0.653–0.655) | 0.662 | 292.9 MB | 0 / 0 |
| Tiered Ultra | branch | TAAU Quality | 0.551 | 0.558 | 242.4 MB | 0 / 0 |
| Compat Ultra | main | MSAA 4× | 0.658 | 0.665 | 292.9 MB | 0 / 0 |
| Compat Ultra | branch | MSAA 4× | 0.659 | 0.667 | 292.9 MB | 0 / 0 |

Per-pass lanes on the branch (median p50, ms; identical within 0.001 ms across rounds):

| Tier | `prepass` | `temporal` (TAAU) | `tc_mask` | `overlay_prepass` | `overlay` | `scene` (main → branch) |
|---|---|---|---|---|---|---|
| Low | 0.020 | 0.130 | 0.004 | 0.022 | 0.050 | 0.159 → 0.079 |
| High | 0.024 | 0.130 | 0.004 | 0.023 | 0.050 | 0.250 → 0.091 |
| Ultra | 0.024 | 0.130 | 0.004 | 0.022 | 0.050 | 0.250 → 0.091 |

**CPU** (`cpu_*`: the client's frame work before rendering; `render_*`: the renderer's
recording and submission, both from the frame log):

| Path, tier | Build | `cpu` p50 / p95, ms | `render` p50, ms | `render` p95, ms (rounds) |
|---|---|---|---|---|
| Tiered Low | main | 0.009 / 0.028 | 0.159 | 0.246 (0.195–0.270) |
| Tiered Low | branch | 0.010 / 0.028 | 0.181 | 0.307 (0.249–0.340) |
| Tiered High | main | 0.009 / 0.028 | 0.167 | 0.280 (0.234–0.301) |
| Tiered High | branch | 0.010 / 0.028 | 0.182 | 0.310 (0.272–0.336) |
| Tiered Ultra | main | 0.009 / 0.028 | 0.167 | 0.249 (0.228–0.301) |
| Tiered Ultra | branch | 0.010 / 0.028 | 0.181 | 0.297 (0.250–0.335) |
| Compat Ultra | main | 0.009 / 0.028 | 0.166 | 0.273 (0.248–0.297) |
| Compat Ultra | branch | 0.010 / 0.028 | 0.167 | 0.242 (0.234–0.298) |

- **GPU.** High and Ultra are 0.103 ms cheaper and 50.5 MB smaller than the MSAA 4× they
  replace; TAAU's own memory is 38.7 MB (`upscaler_vram_mb`: the history and previous-depth
  pairs at a 1080p output). Low costs 0.013 ms and 44.6 MB more: it had no MSAA to give back
  (it ran FXAA at native), and TAAU at a 1080p output costs about what rendering at 0.656
  saves. Compat is unchanged within noise. Against the first measurement of this branch
  (before the review fixes) TAAU fell from 0.145 to 0.130 ms and the tiered scene pass from
  0.084–0.094 to 0.079–0.091 ms; the one change on that path is that `depth` and `motion`
  no longer carry `TRANSFER_SRC` outside the motion probe (amendment 2 item 13), which is
  consistent with the driver keeping them compressed. That attribution is an inference,
  not measured separately.
- **Render CPU rose by about 0.015 ms at p50 on every tiered tier** (0.159–0.167 →
  0.181–0.182 ms, +9–14 %); Compat is flat (0.166 → 0.167). The p95s move by +0.03–0.06 ms,
  but they spread by up to 0.1 ms across rounds on both builds, so the p95 delta is within
  that spread. Where the p50 delta goes, from one more run each of the branch's Tiered Ultra
  with `MOBA_AA=fxaa` (0.169 ms) and with temporal AA (0.180 ms): about 0.002 ms is the
  overlay (its prepass and pass are recorded under every AA mode), and about 0.011 ms is the
  temporal path. Its parts are the motion history and the previous-state upload, the prepass
  permutations with motion and the `tc_mask` pass, and TAAU's and `exposure_linear`'s
  descriptor writes (12 and 2 descriptors a frame). The motion part is measured on its own
  by `benches/motion_history.rs` (below): the frame log does not count keyed units, but at
  the bench's costs (about 17 ns a unit unskinned, 59 ns with 40 joints) even 150 skinned
  units are 0.009 ms, and a 5v5 teamfight keys fewer (10 heroes and the minion waves), so
  the motion work is at most about 0.009 of the 0.011 ms; how the rest splits is not
  measured.
  Nothing was trimmed: no single part is large enough to be worth a change without a
  profiler on the render thread, and the delta is about 0.2 % of Ultra's 9 ms budget.
- **Overlay prepass at 1080p on Low:** 0.022 ms on the 5070 Ti. Scaled by the RTX 2060's
  ≈ 7× lower shader throughput and ≈ 2.7× lower bandwidth, the estimate is about 0.15 ms,
  under the 0.4 ms gate. This is an **estimate**: per the ADR it does not close the target
  until an RTX 2060 (the Low reference class) is measured.
- **Overlay VFX filter.** The overlay re-draws the particle buffer and keeps only enemy
  telegraphs in the vertex stage, so its vertex work scales with live particles, not
  telegraphs. This replay peaks at 663–693 live particles (`vfx_alive_max`); Ultra's cap is
  49,152. The cost at the cap was not measured; the `overlay` lane above (0.050 ms) is
  dominated by its full-screen work at this load.
- **Frame time is presentation-paced and symmetric across builds.** Frame p50 was
  4.177–4.178 ms in every run on both builds although `MOBA_VSYNC=0`: the compositor paces
  presentation on this display (240 Hz), so frame time does not measure this change. In this
  rerun p99 was 4.192–4.202 ms everywhere (p99 ÷ p50 ≤ 1.01). In the first run of this A/B
  (before the review fixes, with other agents' builds on the machine) the R8 pacing rule
  (p99 ≤ 1.5× p50) was breached once on each build: branch Tiered High in round 1 (1.62×)
  and main Tiered Low in round 2 (1.52×). Those were load on the machine, not this change.
  **Before the last N1 part flips the Tiered baseline check, the baselines are re-recorded on
  a quiet machine.**
- **1440p was not run**; every figure here is at 1080p. TAAU's memory and cost scale with the
  output (its history is output-sized), so a 1440p run belongs with N1b's FSR 3.1 comparison.

**`perf-gate` (amendment 2, item 16).** The branch's round-2 logs pass
`perf-gate --baseline tests/perf/baseline.toml` with every Tiered budget checked as a failure
(Low 0.506 of 12 ms, High 0.558 of 7.5 ms, Ultra 0.558 of 9 ms) and the new
`[tiered.low|high|ultra]` baselines compared report-only. Those baselines are the branch
medians above (GPU total p95 and render p95); every later N1 part re-records them, and the
last flips the regression check to failing.

**Motion history bench** (`cargo bench -p moba-render --bench motion_history`, 9800X3D;
per frame, every lookup a hit):

| Case | Time |
|---|---|
| 1,000 keyed units × 40 joints | 66.1 µs |
| 1,000 keyed units, unskinned | 16.6 µs |
| 150 keyed units × 40 joints | 8.8 µs |
| 3,072 keyed units (the 3/4-load key cap), unskinned | 55.8 µs |

The 40-joint case is dominated by copying the palettes twice (into the history, then into
the previous-bone upload: 1.9 MB each way at 1,000 units).

## Not verified here

- **Windows and macOS** were not run (#9, #127). macOS is Compat-only, so its path is the
  byte-identical Compat path.
- **The RTX 2060 (Low reference class)**: the overlay prepass figure above is an estimate.
- **FSR 3.1 and the vendor back ends** (N1b, N1c) are not part of N1a; gate 5 is theirs.
- **Latency** (gate 7) is N1d's.
- **Particle motion** is not twinned by the motion probe (not in gate 2's list); the ghost
  probe's particle projectile measures it. Wind *animation* between the two probe frames is
  not twinned (the wind is steady), and the pushed foliage is checked by direction and share,
  not to 1/64 px.
- **The Tiered baseline regression check** is report-only until the last N1 part flips it;
  the absolute budgets fail now (Amendment 2 item 16).
- **1440p**, the overlay's VFX filter at Ultra's particle cap, and a render-thread profile
  of the 0.015 ms CPU delta were not run.
