# Temporal foundation: gates 1–4 and 6 (N1a)

Captured 2026-11-03 for ADR-0021 (N1a) and its
[Amendment 1](../adr/0021-temporal-upscaling-overlay.md#amendment-1-2026-11-03-n1a-the-temporal-foundation-as-built).

- GPU: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08 (tiered path); software: lavapipe
  (Mesa llvmpipe) with the Khronos validation layer, `MOBA_VULKAN_VALIDATION=required`
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-5-cachyos, Wayland
- Toolchain: rustc 1.99.1 stable
- Branch `feat/n1a-temporal`; baseline: origin/main `34bc098`, built detached in its own
  worktree

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
dynamic resolution off). A hero-sized box dashing 10 m in 0.2 s at 120 fps and a projectile
at 1 m per frame; the share of wake pixels (covered in the previous 12 frames, more than 2 px
from the object now, antialiasing excluded) that differ from the no-history reference by more
than 4/255:

| Mode | Render | Worst frame | Bound |
|---|---|---|---|
| Quality | 853×480 | 2.388 % | 3 % |
| Native | 1280×720 | 1.204 % | 3 % |

The residue is the moving box's shadow trailing edge and thin grid lines re-converging; the
3 % bound is recorded in Amendment 1 with how it was set.

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
on origin/main and then on the branch. Values are the medians of the three rounds' p50s
(range in brackets). The branch's tiered runs use their tier defaults: temporal AA, TAAU
Quality, render scale 0.656 (dynamic resolution held the ceiling throughout).

| Path, tier | Build | AA | GPU total p50, ms | GPU total p95, ms | VRAM peak | `locked_uploads` / `locked_pipelines` |
|---|---|---|---|---|---|---|
| Tiered Low | main | FXAA, native | 0.497 (0.496–0.499) | 0.512 | 136.0 MB | 0 / 0 |
| Tiered Low | branch | TAAU Quality | 0.543 (0.539–0.543) | 0.552 | 180.6 MB | 0 / 0 |
| Tiered High | main | MSAA 4× | 0.655 (0.654–0.657) | 0.710 | 292.9 MB | 0 / 0 |
| Tiered High | branch | TAAU Quality | 0.590 (0.589–0.590) | 0.600 | 242.4 MB | 0 / 0 |
| Tiered Ultra | main | MSAA 4× | 0.657 (0.656–0.659) | 0.713 | 292.9 MB | 0 / 0 |
| Tiered Ultra | branch | TAAU Quality | 0.589 (0.589–0.589) | 0.600 | 242.4 MB | 0 / 0 |
| Compat Ultra | main | MSAA 4× | 0.664 (0.660–0.664) | 0.725 | 292.9 MB | 0 / 0 |
| Compat Ultra | branch | MSAA 4× | 0.663 (0.662–0.666) | 0.723 | 292.9 MB | 0 / 0 |

Per-pass lanes on the branch (median p50, ms; identical within 0.001 ms across rounds):

| Tier | `prepass` | `temporal` (TAAU) | `tc_mask` | `overlay_prepass` | `overlay` | `scene` (main → branch) |
|---|---|---|---|---|---|---|
| Low | 0.022 | 0.146 | 0.004 | 0.023 | 0.051 | 0.163 → 0.084 |
| High | 0.025 | 0.144 | 0.004 | 0.025 | 0.051 | 0.253 → 0.094 |
| Ultra | 0.025 | 0.145 | 0.004 | 0.025 | 0.051 | 0.253 → 0.094 |

- **High and Ultra** are 0.065 ms cheaper on the GPU and 50.5 MB smaller than the MSAA 4×
  they replace. TAAU's own memory is 38.7 MB (`upscaler_vram_mb`: the history and
  previous-depth pairs at a 1080p output).
- **Low** costs 0.046 ms more and 44.6 MB more. Low had no MSAA to give back (it ran FXAA at
  native resolution), and TAAU at a 1080p output (0.146 ms) costs more than the scene work
  saved by rendering at 0.656. This is within Low's budget and is recorded, not hidden; a
  cheaper Low option is N1b's FSR 3.1 or a lighter TAAU variant.
- **Compat** is unchanged within noise (0.664 → 0.663 ms, VRAM identical).
- **Overlay prepass at 1080p on Low:** 0.023 ms on the 5070 Ti. Scaled by the RTX 2060's
  ≈ 7× lower shader throughput and ≈ 2.7× lower bandwidth, the estimate is about 0.16 ms,
  under the 0.4 ms gate. This is an **estimate**: per the ADR it does not close the target
  until an RTX 2060 (the Low reference class) is measured.
- **Frame time** p50 was 4.18 ms in every run on both builds (presentation-paced on this
  display). The p95/p99 tails varied between 4.19 and 6.8 ms on both builds and from round to
  round, with other agents' builds running on the machine at the same time; they are not
  a measurement of this change. The GPU timestamps above are unaffected by that noise
  (their round-to-round range is ≤ 0.004 ms).

## Not verified here

- **Windows and macOS** were not run (#9, #127). macOS is Compat-only, so its path is the
  byte-identical Compat path.
- **The RTX 2060 (Low reference class)**: the overlay prepass figure above is an estimate.
- **FSR 3.1 and the vendor back ends** (N1b, N1c) are not part of N1a; gate 5 is theirs.
- **Latency** (gate 7) is N1d's.
- **Particle motion** is not twinned by the motion probe (not in gate 2's list), and wind
  *animation* between the two probe frames is not twinned (the wind is steady).
- **`perf-gate` on Tiered** stays report-only until the last N1 part merges (Amendment 1
  item 16).
