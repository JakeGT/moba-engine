# Temporal foundation: gates 1–4, gate 6 and the tiered perf-gate split (N1a)

Captured 2026-11-03 for ADR-0021 (N1a) and its
[Amendment 2](../adr/0021-temporal-upscaling-overlay.md#amendment-2-2026-11-03-n1a-the-temporal-foundation-as-built).

- GPU: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08 (tiered path); software: lavapipe
  (Mesa llvmpipe) with the Khronos validation layer, `MOBA_VULKAN_VALIDATION=required`
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-5-cachyos, Wayland
- Toolchain: rustc 1.99.1 stable
- Branch `feat/n1a-temporal` (after the review fixes); baseline for gate 6: origin/main
  `620f11c` (with N1b stage 1 and N1d), built detached in its own worktree; Compat identity
  and goldens against the same commit

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
| camera (terrain) | 383 207 | 0.0044 | 382 692 | 0.0044 | 4.98 |
| static instance | 2 097 | 0.0010 | 2 097 | 0.0010 | 1.92 |
| skinned (one joint) | 1 709 | 0.0080 | 1 709 | 0.0080 | 9.27 |
| VAT (frame 0 → 1) | 2 163 | 0.0043 | 2 163 | 0.0043 | 6.52 |
| foliage | 3 729 | 0.0043 | 4 244 | 0.0043 | 5.72 |
| `motion_reset` | 1 802 | 0.0041 | 1 802 | 0.0041 | 4.45 |
| key absent from A | 2 300 | 0.0043 | 2 300 | 0.0043 | 5.72 |
| key new in B | 2 512 | 0.0044 | 2 512 | 0.0044 | 5.46 |

Pixels within a centimetre of an object's boundary are skipped and counted (7 327 on Low,
6 979 on High).

**Foliage pushed** (a second patch with a displacer in frame B only; no per-blade twin, so
checked by share and direction): on Low 2 068 of 2 594 pixels (79.7 %, need 30 %) moved
0.25 px or more beyond camera-only, and 1 691 of those (81.8 %, need 70 %) point back towards
the displacer; on High 2 393 of 2 942 (81.3 %) and 1 932 (80.7 %). Mean extra motion 2.4 px.
With the prepass's second evaluation removed from `foliage.vert` (a mutation) no pushed pixel
moved and the class failed.

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
native, where a missing vector barely smears (0.14 %), so native does not detect missing
hero motion keys; `scripts/ghost-probe.sh` runs Quality, Performance (the strongest
hero-smear check) and native. The wake's residue is the moving
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
- **Mid-stream history reset** (amendment 2, item 28): `scripts/tiered-goldens.sh` also runs
  40 frames of High with `MOBA_TEMPORAL_RESET_EVERY=5` under synchronization validation; 0
  messages. **It is a validation smoke, not a regression check for the reset barrier:**
  with the TAAU fresh barrier's source mutated back to `NONE`/`NONE`, synchronization
  validation (VVL 1.4.363) stays clean on lavapipe, on the RTX 5070 Ti and on RADV (Radeon
  610M), for 40 and 400 frames with a reset every fifth frame and for 1,000 frames with
  vsync off and a reset every second frame (`target/n1a/mutant/`, not committed). The
  barrier's source is pinned by unit tests on the barriers instead
  (`fresh_barrier_waits_for_earlier_compute`, `placeholder_barrier_waits_for_earlier_compute`);
  the first fails on the mutant.

## Gate 6: performance on the RTX 5070 Ti

The teamfight replay (`tests/replays/teamfight-5v5.mbr`, 330 warm-up ticks) at 1920×1080,
release builds, validation off, `MOBA_VSYNC=0`, `MOBA_PIPELINE_CACHE=off`, 3 000 frames (the
log covers the last 2 940). Three rounds, interleaved: in each round every configuration ran
on origin/main `620f11c` (N1b stage 1 and N1d merged) and then on the branch (the review
fixes, merged with it). Both builds run N1d's defaults: on Tiered, Reflex on and the 240 fps
limit. Values are the medians of the three rounds (range in brackets where it is wider
than 0.004 ms). The branch's tiered runs use their tier defaults: temporal AA, TAAU Quality,
render scale 0.656 (dynamic resolution held the ceiling throughout).

**GPU and memory:**

| Path, tier | Build | AA | GPU total p50, ms | GPU total p95, ms | VRAM peak | `locked_uploads` / `locked_pipelines` |
|---|---|---|---|---|---|---|
| Tiered Low | main | FXAA, native | 0.496 (0.493–0.498) | 0.511 | 136.0 MB | 0 / 0 |
| Tiered Low | branch | TAAU Quality | 0.541 | 0.551 | 180.6 MB | 0 / 0 |
| Tiered High | main | MSAA 4× | 0.659 (0.656–0.663) | 0.718 | 292.9 MB | 0 / 0 |
| Tiered High | branch | TAAU Quality | 0.588 | 0.598 | 242.4 MB | 0 / 0 |
| Tiered Ultra | main | MSAA 4× | 0.658 (0.654–0.666) | 0.718 | 292.9 MB | 0 / 0 |
| Tiered Ultra | branch | TAAU Quality | 0.589 | 0.599 | 242.4 MB | 0 / 0 |
| Compat Ultra | main | MSAA 4× | 0.662 (0.659–0.663) | 0.724 | 292.9 MB | 0 / 0 |
| Compat Ultra | branch | MSAA 4× | 0.663 | 0.721 | 292.9 MB | 0 / 0 |

Per-pass lanes on the branch (median p50, ms; within 0.001 ms across rounds):

| Tier | `prepass` | `temporal` (TAAU) | `tc_mask` | `overlay_prepass` | `overlay` | `scene` (main → branch) |
|---|---|---|---|---|---|---|
| Low | 0.022 | 0.147 | 0.004 | 0.024 | 0.051 | 0.161 → 0.084 |
| High | 0.025 | 0.144 | 0.004 | 0.025 | 0.051 | 0.254 → 0.094 |
| Ultra | 0.025 | 0.144 | 0.004 | 0.025 | 0.051 | 0.254 → 0.094 |

**CPU** (`cpu_*`: the client's frame work before rendering; `render_*`: the renderer's
recording and submission, both from the frame log):

| Path, tier | Build | `cpu` p50, ms | `render` p50, ms | `render` p95, ms (rounds) |
|---|---|---|---|---|
| Tiered Low | main | 0.009 | 0.162 | 0.289 (0.282–0.304) |
| Tiered Low | branch | 0.010 | 0.182 | 0.338 (0.332–0.342) |
| Tiered High | main | 0.009 | 0.169 | 0.316 (0.310–0.318) |
| Tiered High | branch | 0.010 | 0.183 | 0.344 (0.327–0.344) |
| Tiered Ultra | main | 0.009 | 0.169 | 0.314 (0.271–0.315) |
| Tiered Ultra | branch | 0.010 | 0.183 | 0.327 (0.322–0.341) |
| Compat Ultra | main | 0.009 | 0.168 | 0.315 (0.314–0.318) |
| Compat Ultra | branch | 0.010 | 0.169 | 0.318 (0.317–0.318) |

- **GPU.** High and Ultra are 0.07 ms cheaper and 50.5 MB smaller than the MSAA 4× they
  replace; TAAU's own memory is 38.7 MB (`upscaler_vram_mb`: the history and previous-depth
  pairs at a 1080p output). Low costs 0.045 ms and 44.6 MB more: it had no MSAA to give back
  (it ran FXAA at native), and TAAU at a 1080p output (0.147 ms) costs more than rendering
  at 0.656 saves. Compat is unchanged within noise.
- **Session-to-session drift, asymmetric.** An earlier A/B of this branch against `e9d5f91`
  (before the N1d merge, the same day) measured lower GPU totals than here, and the two
  builds did not move together (GPU total p50, this session minus the earlier one, ms):

  | Build | Low | High | Ultra | Compat |
  |---|---|---|---|---|
  | main | +0.010 | +0.005 | +0.005 | +0.004 |
  | branch | +0.042 | +0.037 | +0.038 | +0.004 |

  Most of the branch's rise is TAAU (0.130 → 0.144–0.147 ms; water +0.004, bloom +0.003).
  **It is not a code change.** The earlier branch binary was `9d3562b`, which already had
  the review's mip-bias and foliage fixes. A three-round Tiered High A/B on the RTX 5070 Ti
  (`target/n1a/ab4/`) of `9d3562b`, this head with Reflex on, and this head with
  `MOBA_LATENCY=off` measured GPU total p50 0.586–0.587, 0.587 and 0.586–0.587 ms, with
  TAAU at 0.144 ms in all three: neither the later review fixes nor the N1d latency merge
  moved it. What changed is the GPU's state that session (perf/0022's memory-clock note),
  which was not logged; why it landed mostly on TAAU is not known.
  The comparisons within a session hold; the committed baselines come from this session,
  with every fix in. Render CPU drifts between sessions too: the head's Tiered High render
  p50 was 0.156–0.158 ms in that A/B against 0.183 ms here, so the render-CPU delta below
  holds only within a session.
- **Render CPU rose by 0.014–0.020 ms at p50 on every tiered tier** (0.162–0.169 →
  0.182–0.183 ms, +8–12 %); Compat is flat (0.168 → 0.169). The p95s rise by 0.01–0.05 ms
  and spread by up to 0.04 ms across rounds. Where the p50 delta goes, from one more run
  each of the pre-merge branch's Tiered Ultra with `MOBA_AA=fxaa` (0.169 ms) and with
  temporal AA (0.180 ms): about 0.002 ms is the overlay (its prepass and pass are recorded
  under every AA mode) and about 0.011 ms the temporal path. Its parts are the motion history
  and the previous-state upload, the prepass permutations with motion and the `tc_mask`
  pass, and TAAU's and `exposure_linear`'s descriptor writes (12 and 2 descriptors a frame).
  The motion part is measured on its own by `benches/motion_history.rs` (below): the frame
  log does not count keyed units, but at the bench's costs (about 17 ns a unit unskinned,
  59 ns with 40 joints) even 150 skinned units are 0.009 ms, and a 5v5 teamfight keys fewer
  (10 heroes and the minion waves), so the motion work is at most about 0.009 of the
  0.011 ms; how the rest splits is not measured. Nothing was trimmed: no single part is
  large enough to be worth a change without a profiler on the render thread, and the delta
  is about 0.2 % of Ultra's 9 ms budget.
- **Overlay prepass at 1080p on Low:** 0.024 ms on the 5070 Ti. Scaled by the RTX 2060's
  ≈ 7× lower shader throughput and ≈ 2.7× lower bandwidth, the estimate is about 0.17 ms,
  under the 0.4 ms gate. This is an **estimate**: per the ADR it does not close the target
  until an RTX 2060 (the Low reference class) is measured.
- **Overlay VFX filter.** The overlay re-draws the particle buffer and keeps only enemy
  telegraphs in the vertex stage, so its vertex work scales with live particles, not
  telegraphs. This replay peaks at 663–693 live particles (`vfx_alive_max`); Ultra's cap is
  49,152. The cost at the cap was not measured; the `overlay` lane above (0.051 ms) is
  dominated by its full-screen work at this load.
- **Frame time is paced and symmetric across builds.** Frame p50 was 4.220 ms on Tiered and
  4.178 ms on Compat in every run on both builds although `MOBA_VSYNC=0`: Tiered runs N1d's
  Reflex pacing at the 240 fps limit, and Compat's presentation is paced by the compositor on
  this 240 Hz display, so frame time does not measure this change. p99 was 4.196–4.257 ms
  everywhere (p99 ÷ p50 ≤ 1.01). In the first A/B of this branch (before the review fixes,
  with other agents' builds on the machine) the R8 pacing rule (p99 ≤ 1.5× p50) was breached
  once on each build: branch Tiered High in round 1 (1.62×) and main Tiered Low in round 2
  (1.52×); that was load on the machine, not this change. **Before the last N1 part flips
  the Tiered baseline check, the baselines are re-recorded on a quiet machine.**
- **1440p was not run**; every figure here is at 1080p. TAAU's memory and cost scale with the
  output (its history is output-sized), so a 1440p run belongs with N1b's FSR 3.1 comparison.

**`perf-gate` (amendment 2, item 16).** The High, Ultra and Overdrive budgets are defined
at 1440p (`GPU_BUDGETS`; Low and Medium at 1080p) and these runs are at 1080p, so the
absolute check is lenient on those tiers until a 1440p run. The branch's logs pass
`perf-gate --baseline tests/perf/baseline.toml` with every Tiered budget checked as a failure
(Low, High and Ultra at 0.55–0.60 ms against 12, 7.5 and 9 ms) and the new
`[tiered.low|high|ultra]` baselines compared report-only. Those baselines are the branch
medians above (GPU total p95 and render p95); every later N1 part re-records them, and the
last flips the regression check to failing. Compat Ultra's render p95 sits on its
committed limit (0.318 ms against `[ultra]`'s 0.254 ms + 25 %) on **both** builds: main
`620f11c` fails it in round 1 and the branch in rounds 1 and 2, at 0.314–0.318 ms. These
short replays are not `perf-replay.sh`'s conditions (18,000 ticks, a warm pipeline cache)
that the Compat baseline was recorded under, and the branch's Compat render p50 is within
0.001 ms of main's, so this is recorded, not attributed to N1a; the nightly perf run is the
check.

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
- **The reset barrier's hazard** is not observed by any tool here (item 28): no validation
  run, on lavapipe, NVIDIA or RADV, distinguishes the fixed barrier from a `NONE` source.
- **The Tiered baseline regression check** is report-only until the last N1 part flips it;
  the absolute budgets fail now (Amendment 2 item 16).
- **1440p**, the overlay's VFX filter at Ultra's particle cap, and a render-thread profile
  of the 0.015 ms CPU delta were not run.
