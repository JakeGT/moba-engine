# Latency: Reflex and Anti-Lag (N1d)

Captured 2026-11-02 and 2026-11-03 on the development workstation for ADR-0021 §6,
verification gate 7 and
[amendment 3](../adr/0021-temporal-upscaling-overlay.md#amendment-3-2026-11-02-n1d-latency-as-built).

- GPU: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08, `VK_NV_low_latency2` revision 3
  (tiered path, `max_tier=overdrive`); Radeon 610M iGPU on RADV, Mesa 26.2.5
- Display: one 2560×1440 monitor at about 165 Hz (FIFO frames of 6.07 ms), KDE Plasma on
  Wayland; the client runs as a native Wayland window
- CPU: AMD Ryzen 7 9800X3D; OS: CachyOS, Linux 7.3.0-5-cachyos
- Build: release, rustc 1.99.1. "branch" is `feat/n1d-latency` with the review fixes
  (render-submit marker after the fence and acquire waits, the 90 % backstop, the resume and
  settle logic); every table below is on that code except
  [the first pacing run](#first-pacing-run-four-tiers-before-the-review-fixes), which says so.
  "main" is a detached `origin/main` at `e5f1451` built in its own worktree
- Validation: the bundled Khronos layer (`/home/jaket/Projects/MOBA/.tools/validation`) with
  `MOBA_VULKAN_VALIDATION=required`, which also turns on synchronization validation

## Summary

| Gate 7 item | Result |
|---|---|
| Reflex timings in the frame log (RTX) | **done**: `latency=reflex`, `paced_frames`, and p50/p95 of `latency_sim`, `latency_submit`, `latency_present`, `latency_gpu`, plus p50/p95/p99 of `pc_latency`, from `vkGetLatencyTimingsNV` |
| PC latency, Reflex on vs off, teamfight replay (RTX) | **measured, no gain**: vsync off 5.109 vs 5.072 ms p50, vsync on 12.882 vs 12.893 ms, uncapped and GPU-bound 4.214 vs 4.269 ms (medians of 3 interleaved rounds); see [below](#why-reflex-gains-nothing-here) |
| Anti-Lag smoke on RADV with native `VK_AMD_anti_lag` | **blocked**: needs Mesa 26.3. Mesa 26.2.5 has only the anti-lag layer, which is not the target. By default the Radeon logs `[latency] unavailable` and falls back; with the layer forced on, the Anti-Lag back end runs but validation flags the layer's own queries ([below](#anti-lag-on-radv-blocked)) |
| Pacing p99 ÷ p50 ≤ 1.5 with the platform limiter replaced | **pass**: 1.10 (median of 3 interleaved 10-minute High rounds; `origin/main` also 1.10), and 1.05–1.18 over the four tiers in one earlier run ([below](#pacing-10-minute-replay-platform-limiter-replaced)) |
| Validation and sync validation, extensions enabled (RTX) | **0 messages** with Reflex on, off and boost, vsync off and on, and uncapped |

## Validation (RTX, Reflex enabled)

The teamfight replay (`tests/replays/teamfight-5v5.mbr` from tick 330, 600 frames, tiered,
1920×1080) once per setting and vsync mode, plus one uncapped Overdrive run at 2560×1380:

```bash
VK_LAYER_PATH=/home/jaket/Projects/MOBA/.tools/validation/usr/share/vulkan/explicit_layer.d \
LD_LIBRARY_PATH=/home/jaket/Projects/MOBA/.tools/validation/usr/lib \
MOBA_VULKAN_VALIDATION=required MOBA_SETTINGS=/dev/null MOBA_RENDER_PATH=tiered \
MOBA_LATENCY=on|off|boost MOBA_REPLAY=tests/replays/teamfight-5v5.mbr MOBA_WARMUP_TICKS=330 \
MOBA_SMOKE_FRAMES=600 MOBA_VSYNC=0|1 MOBA_WINDOW_SIZE=1920x1080 MOBA_FRAME_LOG=v.log \
./target/release/moba-client
```

| Setting | Start-up line | Validation messages (vsync off / on) |
|---|---|---|
| `on` | `[latency] reflex: VK_NV_low_latency2 rev 3, setting on (minimumIntervalUs 4166); the latency sleep replaces the platform limiter` | 0 / 0 |
| `off` | `[latency] reflex: VK_NV_low_latency2 rev 3, setting off; markers and timings only, the platform limiter paces frames` | 0 / 0 |
| `boost` | `[latency] reflex: VK_NV_low_latency2 rev 3, setting boost (minimumIntervalUs 4166); the latency sleep replaces the platform limiter` | 0 / 0 |
| `on`, `MOBA_MAX_FPS=0`, Overdrive | `… setting on (minimumIntervalUs 0); …` | 0 |

The first run found one bug: passing the same `VkLatencyTimingsFrameReportNV` array to
`vkGetLatencyTimingsNV` twice failed `VUID-VkLatencyTimingsFrameReportNV-sType-sType` (the
driver writes the structures back whole). The back end now resets the array before each
call (amendment 3, item 8); every run above is after the fix.

## Reflex on and off (RTX, teamfight replay)

The R8 evidence replay from tick 330 for 2,400 frames (the first 60 are warm-up), tiered path,
1920×1080, the default tier (High, which runs the Ultra configuration on Tiered), 240 fps
limit, validation off. Per round the four variants run back to back for each vsync mode
(main, off, on, boost), three rounds; each cell is the median of the rounds. Frame times are
the client's wall-clock intervals; the `latency_*` columns are Reflex's own timings, so `main`
has none.

| vsync | Variant | Frame p50 / p95 / p99 ms | p99 ÷ p50 | Render call p50 | GPU total p50 | Paced by the sleep | Submit p50 | GPU (Reflex) p50 | **PC latency p50 / p95 / p99** |
|---|---|---|---|---|---|---|---|---|---|
| off | main | 4.178 / 4.190 / 4.200 | 1.005 | 0.167 | 0.655 | — | — | — | — |
| off | off | 4.177 / 4.190 / 4.195 | 1.004 | 0.171 | 0.655 | 0 / 2,340 | 0.140 | 0.675 | **5.072 / 5.225 / 5.272** |
| off | on | 4.220 / 4.239 / 4.251 | 1.007 | 0.169 | 0.656 | 2,339 / 2,340 | 0.139 | 0.676 | **5.109 / 5.230 / 5.308** |
| off | boost | 4.220 / 4.240 / 4.253 | 1.008 | 0.168 | 0.656 | 2,339 / 2,340 | 0.138 | 0.676 | **5.105 / 5.228 / 5.320** |
| on | main | 6.068 / 6.180 / 6.307 | 1.039 | 6.045 | 0.712 | — | — | — | — |
| on | off | 6.068 / 6.225 / 7.261 | 1.197 | 6.047 | 0.709 | 0 / 2,340 | 0.117 | 0.727 | **12.893 / 13.029 / 14.001** |
| on | on | 6.068 / 6.170 / 6.308 | 1.040 | 6.037 | 0.712 | 2,339 / 2,340 | 0.141 | 0.732 | **12.882 / 12.973 / 13.276** |
| on | boost | 6.067 / 6.613 / 7.540 | 1.243 | 6.037 | 0.708 | 2,339 / 2,340 | 0.140 | 0.728 | **12.879 / 13.367 / 14.397** |

The simulation span is 0.008–0.010 ms and the present call 0.018–0.021 ms in every Reflex
row. `pc_latency` is simulation start to the end of the frame's GPU work (amendment 3,
item 8). `Paced by the sleep` counts the logged frames (past the warm-up).

Reading it:
- **Nothing else changes.** `off` matches `main` at p50 and p95: the markers, the present
  IDs and the submit chain cost nothing measurable.
- **The render-submit span is now the render thread's work.** With vsync on it is
  0.12–0.14 ms; before the review fix (the marker set before the fence and acquire waits) it
  was 6.0 ms, the whole vsync block (amendment 3, item 5).
- **The sleep replaces the limiter.** With vsync off, `on` paces 2,339 of 2,340 logged frames
  and the 240 fps limit becomes `minimumIntervalUs = 4166`. The driver's interval comes out
  0.042 ms longer than the platform's `WaitUntil` (4.220 vs 4.178 ms, 237 vs 239 fps).
- **PC latency does not move.** On vs off is +0.037 ms with vsync off and −0.011 ms with
  vsync on at p50, inside the round-to-round spread. The vsync-on p99 values (frame and PC
  latency) swing by a millisecond from round to round in every variant, `main` included
  (`main` 6.25–7.21 ms frame p99, `off` 6.25–7.70, `on` 6.24–6.95, `boost` 6.29–7.66), so
  the p99 differences in that half of the table are noise, not an effect.

## GPU-bound and uncapped (RTX)

Gate 7's capped case has no render queue for Reflex to drain: the GPU is idle most of each
frame. To give it one, the same replay ran uncapped (`MOBA_MAX_FPS=0`, vsync off) on
Overdrive at 2560×1380 (the largest window the desktop gives), with
`MOBA_SHADOW_SIZE=8192 MOBA_VFX_SCALE=16 MOBA_CONTACT_SHADOWS=256`, 4,000 frames, three
interleaved rounds of off, on and boost (`main` has no uncapped switch; `off` matches it
above). Locking the GPU clocks low (`nvidia-smi -lgc`) needs root and was not done; this
machine has no heavier load than this to give the RTX 5070 Ti.

| Variant | Frame p50 / p95 / p99 ms | Render call p50 | GPU total p50 | Submit p50 | GPU (Reflex) p50 | Paced | **PC latency p50 / p95 / p99** |
|---|---|---|---|---|---|---|---|
| off | 1.419 / 1.986 / 2.883 | 1.404 | 1.244 | 0.106 | 1.266 | 0 / 3,940 | **4.269 / 5.187 / 5.919** |
| on | 1.402 / 1.722 / 1.802 | 1.380 | 1.244 | 0.077 | 1.270 | 3,939 / 3,940 | **4.214 / 4.524 / 4.805** |
| boost | 1.444 / 1.715 / 1.807 | 1.420 | 1.245 | 0.096 | 1.273 | 3,939 / 3,940 | **4.274 / 4.549 / 4.674** |

The GPU is busy about 88 % of each frame (1.24 of 1.40–1.42 ms) and a frame's PC latency is three GPU
frames: two frames are queued ahead of it. Reflex's sleep should remove that queue; it does
not. On vs off is −0.055 ms at p50, inside the spread (off 4.217–4.271, on 4.211–4.261). The
tails look lower with Reflex, but each variant has one or two rounds with hitches (`off`
frame p99 1.72–2.98 ms, `on` 1.79–2.28, `boost` 1.79–2.82), so three rounds do not separate
them.

### Why Reflex gains nothing here

Each frame's latency is spent where the extension does not reach:
- **Vsync off, capped.** The render call is 0.17 ms and the GPU work 0.68 ms, but the GPU work
  ends about 4.9 ms after the submit. The second batch waits on the swapchain image's acquire
  semaphore, which the compositor signals when it releases the image. The driver's sleep,
  timed in a temporary instrumented build of the first draft, averaged 4.0 ms per frame: it
  enforces `minimumIntervalUs` and nothing else.
- **Vsync on (FIFO).** The render call takes 6.04 ms, almost all of it blocked in
  `vkAcquireNextImageKHR`, so the frame is display-bound in the swapchain, not in the render
  queue. The same instrumented build measured a mean sleep of 0.04 ms. Moving
  `RENDERSUBMIT_START` after the waits (so the driver now sees a 0.14 ms render thread) did
  not change PC latency.
- **Uncapped and GPU-bound.** The render call blocks for 1.38 ms of a 1.40 ms frame (in the
  frame-slot fence and the swapchain waits; not split further) and the queue stays three
  frames deep with the sleep on. An XWayland run of the first draft gave 4.06 vs 4.27 ms, a
  single run recorded only as a lead.

So §6b's expected gain is **not demonstrated** on this desktop (amendment 3, item 9), and
on-by-default rests on the maintainer's answer to Q5, not on a measured benefit. The
integration does what the extension asks (validation-clean, timings reported, sleep mode
applied per swapchain, every frame attributed to its present ID). `pc_latency` also stops at
the end of the GPU work, so the compositor's and the display's share is invisible;
`VK_KHR_present_wait` timestamps would close that and are not done. The open follow-up is to
find a case where the NVIDIA driver sleeps (an X11 session, fullscreen direct scan-out) and to
look at the swapchain's own queueing (image count, present mode, present wait).

## Pacing: 10-minute replay, platform limiter replaced

`scripts/perf-replay.sh` with `RENDER_PATH=tiered` (the R8 pacing gate: the 18,000-tick
teamfight replay, about ten minutes, vsync off, 1920×1080), `TIERS=high`, three rounds, each
round running the branch and then `origin/main`, each build through its own script and
binary. On the branch the default setting is `on`, so the Reflex sleep paces the frames and
the platform limiter is idle except where the backstop hands a frame back. `perf-gate`
passed every run.

| Build | Frame p50 / p95 / p99 ms (median) | **p99 ÷ p50** (rounds) | Max ms (rounds) | GPU total p95 | Paced by the sleep | PC latency p50 / p95 / p99 |
|---|---|---|---|---|---|---|
| branch | 4.219 / 4.240 / 4.646 | **1.10** (1.10, 1.01, 1.14) | 90.6, 13.2, 11.4 | 0.716 | 94.4 %, 98.5 %, 94.2 % | 5.111 / 5.278 / 5.683 |
| main | 4.178 / 4.191 / 4.586 | 1.10 (1.01, 1.10, 1.24) | 7.2, 14.8, 8.4 | 0.716 | — | — |

No frames dropped, and `locked_uploads` and `locked_pipelines` are 0 in every run. The two
builds pace the same: the ratio's spread between rounds (1.01–1.24) is larger than any
difference between them. The branch's p50 is 0.04 ms longer for the reason in the on/off
table, so it runs about 1 % fewer frames in the same replay. Over ten minutes the 90 %
backstop hands 1.5–6 % of the frames back to the platform limiter: frames the driver let
through in under 90 % of the interval (which frames, and why, is not broken down here). These
runs timed the backstop from each frame's `SIMULATION_START`; it now runs from one sleep's end
to the next (amendment 3, item 6), which no longer counts the event-dispatch gap against the
frame, so this share is an upper bound for the current code (not re-measured over ten
minutes; a 2,400-frame validation run paces 2,339 of 2,340 logged frames). The single-frame maxima (the branch's 90.6 ms
in round 1) are one frame each, on both builds; p99 is the gate.

### First pacing run: four tiers, before the review fixes

One ten-minute run per tier and build, on the first draft (render-submit marker before the
waits, no backstop), recorded for the other tiers. Single runs, so the differences between
the builds are within the round-to-round spread shown above.

| Tier | Build | Frames | Frame p50 / p95 / p99 ms | p99 ÷ p50 | Max ms | GPU total p95 | PC latency p50 / p95 / p99 |
|---|---|---|---|---|---|---|---|
| Low | branch | 141,856 | 4.220 / 4.241 / 4.430 | 1.05 | 20.53 | 0.516 | 4.957 / 5.122 / 5.476 |
| Low | main | 142,674 | 4.180 / 4.203 / 4.996 | 1.20 | 8.75 | 0.512 | — |
| Medium | branch | 141,790 | 4.219 / 4.241 / 4.819 | 1.14 | 21.75 | 0.719 | 5.119 / 5.292 / 5.918 |
| Medium | main | 142,947 | 4.178 / 4.192 / 4.847 | 1.16 | 10.02 | 0.720 | — |
| High | branch | 141,813 | 4.219 / 4.244 / 4.819 | 1.14 | 8.42 | 0.713 | 5.113 / 5.292 / 5.791 |
| High | main | 142,749 | 4.178 / 4.195 / 4.988 | 1.19 | 50.86 | 0.719 | — |
| Ultra | branch | 141,676 | 4.219 / 4.245 / 4.958 | 1.18 | 12.64 | 0.715 | 5.117 / 5.313 / 6.034 |
| Ultra | main | 142,858 | 4.177 / 4.194 / 4.922 | 1.18 | 22.72 | 0.714 | — |

## Anti-Lag on RADV: blocked

Gate 7 asks for a smoke of native `VK_AMD_anti_lag`, which RADV gains in Mesa 26.3. This
machine's Radeon 610M runs Mesa 26.2.5, which offers the extension only through the implicit
`VK_LAYER_MESA_anti_lag`, off unless `ENABLE_LAYER_MESA_ANTI_LAG=1` is set. The teamfight
replay from tick 330, 300 frames, tiered, 1280×720, vsync off, validation required, through
`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/radeon_icd.json`:

| Run | Start-up line | Frame log | Validation |
|---|---|---|---|
| Default (layer off), setting `on` or `off` | `[latency] unavailable: the device enables neither VK_NV_low_latency2 (with presentId) nor VK_AMD_anti_lag; the platform limiter paces frames` | `latency=none paced_frames=0/240 latency_timing=unavailable`, p50 14.00 ms | 0 |
| Layer forced on, setting `on` | `[latency] anti-lag: VK_AMD_anti_lag rev 1, setting on (maxFPS 240); the latency sleep replaces the platform limiter` | `latency=anti-lag paced_frames=239/240`, p50 13.98 ms, p99 14.36 ms | **4 errors**, all `VUID-vkCmdWriteTimestamp-None-00830` on queries 126, 127, 254 and 255 of a pool the renderer does not own (its timer pools have 80 queries) |
| Layer forced on, setting `off` | `[latency] anti-lag: VK_AMD_anti_lag rev 1, setting off; the platform limiter paces frames` | `latency=anti-lag paced_frames=0/240`, p50 14.00 ms | 0 |

The fallback does what §6 asks: one line, nothing else changes. The forced-layer runs show the
Anti-Lag back end selects, calls `vkAntiLagUpdateAMD` at input and present, and reports itself
in the frame log; the validation errors come from the layer's own timestamp queries, which is
one more reason the layer is not the target. The smoke stays **blocked** until Mesa 26.3 is
installed; the iGPU is GPU-bound at 14 ms a frame here, so latency numbers wait for that too.

## Commands

```bash
# Gate 7 rounds (main, off, on, boost x vsync 0/1, 3 interleaved rounds)
MOBA_VULKAN_VALIDATION=off MOBA_SETTINGS=/dev/null MOBA_RENDER_PATH=tiered MOBA_LATENCY=<v> \
MOBA_REPLAY=tests/replays/teamfight-5v5.mbr MOBA_WARMUP_TICKS=330 MOBA_SMOKE_FRAMES=2400 \
MOBA_VSYNC=<0|1> MOBA_FRAME_LOG=<log> MOBA_WINDOW_SIZE=1920x1080 ./target/release/moba-client
# GPU-bound, uncapped (off, on, boost x 3 interleaved rounds)
MOBA_VULKAN_VALIDATION=off MOBA_SETTINGS=/dev/null MOBA_RENDER_PATH=tiered MOBA_QUALITY=overdrive \
MOBA_LATENCY=<v> MOBA_MAX_FPS=0 MOBA_SHADOW_SIZE=8192 MOBA_VFX_SCALE=16 MOBA_CONTACT_SHADOWS=256 \
MOBA_REPLAY=tests/replays/teamfight-5v5.mbr MOBA_WARMUP_TICKS=330 MOBA_SMOKE_FRAMES=4000 \
MOBA_VSYNC=0 MOBA_FRAME_LOG=<log> MOBA_WINDOW_SIZE=2560x1380 ./target/release/moba-client
# Pacing (each build through its own script; 3 rounds, branch then main)
TIERS=high RENDER_PATH=tiered BASELINE= scripts/perf-replay.sh <out>
# Radeon fallback and the forced Mesa layer (add ENABLE_LAYER_MESA_ANTI_LAG=1 for the latter)
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/radeon_icd.json MOBA_SETTINGS=/dev/null \
MOBA_RENDER_PATH=tiered MOBA_LATENCY=<on|off> MOBA_REPLAY=tests/replays/teamfight-5v5.mbr \
MOBA_WARMUP_TICKS=330 MOBA_SMOKE_FRAMES=300 MOBA_VSYNC=0 MOBA_WINDOW_SIZE=1280x720 \
MOBA_FRAME_LOG=<log> ./target/release/moba-client
```
