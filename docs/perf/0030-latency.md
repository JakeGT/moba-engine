# Latency: Reflex and Anti-Lag (N1d)

Captured 2026-11-02 on the development workstation for ADR-0021 §6, verification gate 7 and
[amendment 1](../adr/0021-temporal-upscaling-overlay.md#amendment-1-2026-11-02-n1d-latency-as-built).

- GPU: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08, `VK_NV_low_latency2` revision 3
  (tiered path, `max_tier=overdrive`); Radeon 610M iGPU on RADV, Mesa 26.2.5
- Display: one 2560×1440 monitor at about 165 Hz (FIFO frames of 6.07 ms), KDE Plasma on
  Wayland; the client runs as a native Wayland window
- CPU: AMD Ryzen 7 9800X3D; OS: CachyOS, Linux 7.3.0-5-cachyos
- Build: release, rustc 1.99.1. "branch" is `feat/n1d-latency` (the on/off rounds ran on an
  earlier draft whose only code difference is that a failed `vkSetLatencySleepModeNV` was then
  fatal; the 10-minute replays, the validation runs and the Radeon runs are on the code as
  submitted); "main" is a detached `origin/main` at `e5f1451` built in its own worktree
- Validation: the bundled Khronos layer (`/home/jaket/Projects/MOBA/.tools/validation`) with
  `MOBA_VULKAN_VALIDATION=required`, which also turns on synchronization validation

## Summary

| Gate 7 item | Result |
|---|---|
| Reflex timings in the frame log (RTX) | **done**: `latency=reflex`, `paced_frames`, and p50/p95 of `latency_sim`, `latency_submit`, `latency_present`, `latency_gpu`, plus p50/p95/p99 of `pc_latency`, from `vkGetLatencyTimingsNV` |
| PC latency, Reflex on vs off, teamfight replay (RTX) | **measured, no gain**: vsync off 5.121 vs 5.076 ms p50, vsync on 12.881 vs 12.893 ms p50 (medians of 3 interleaved rounds); see [below](#why-reflex-gains-nothing-here) |
| Anti-Lag smoke on RADV with native `VK_AMD_anti_lag` | **blocked**: needs Mesa 26.3. Mesa 26.2.5 has only the anti-lag layer, which is not the target. By default the Radeon logs `[latency] unavailable` and falls back; with the layer forced on, the Anti-Lag back end runs but validation flags the layer's own queries ([below](#anti-lag-on-radv-blocked)) |
| Pacing p99 ÷ p50 ≤ 1.5 with the platform limiter replaced | **pass**: 1.05x–1.18x over four 10-minute tiered replays with Reflex pacing every frame but the first; `origin/main` gives 1.16x–1.20x ([below](#pacing-10-minute-replay-platform-limiter-replaced)) |
| Validation and sync validation, extensions enabled (RTX) | **0 messages** with Reflex on, off and boost |

## Validation (RTX, Reflex enabled)

The teamfight replay (`tests/replays/teamfight-5v5.mbr` from tick 330, 600 frames, tiered,
1920×1080, vsync off) once per setting:

```bash
VK_LAYER_PATH=/home/jaket/Projects/MOBA/.tools/validation/usr/share/vulkan/explicit_layer.d \
LD_LIBRARY_PATH=/home/jaket/Projects/MOBA/.tools/validation/usr/lib \
MOBA_VULKAN_VALIDATION=required MOBA_SETTINGS=/dev/null MOBA_RENDER_PATH=tiered \
MOBA_LATENCY=on|off|boost MOBA_REPLAY=tests/replays/teamfight-5v5.mbr MOBA_WARMUP_TICKS=330 \
MOBA_SMOKE_FRAMES=600 MOBA_VSYNC=0 MOBA_WINDOW_SIZE=1920x1080 MOBA_FRAME_LOG=v.log \
./target/release/moba-client
```

| Setting | Start-up line | Validation messages |
|---|---|---|
| `on` | `[latency] reflex: VK_NV_low_latency2 rev 3, setting on (minimumIntervalUs 4166); the latency sleep replaces the platform limiter` | 0 |
| `off` | `[latency] reflex: VK_NV_low_latency2 rev 3, setting off; markers and timings only, the platform limiter paces frames` | 0 |
| `boost` | `[latency] reflex: VK_NV_low_latency2 rev 3, setting boost (minimumIntervalUs 4166); the latency sleep replaces the platform limiter` | 0 |

The first run found one bug: passing the same `VkLatencyTimingsFrameReportNV` array to
`vkGetLatencyTimingsNV` twice failed `VUID-VkLatencyTimingsFrameReportNV-sType-sType` (the
driver writes the structures back whole). The back end now resets the array before each
call (amendment 1, item 7); the runs above are after the fix.

## Reflex on and off (RTX, teamfight replay)

The R8 evidence replay from tick 330 for 2,400 frames (the first 60 are warm-up), tiered path,
1920×1080, the default tier (High, which runs the Ultra configuration on Tiered), validation
off. Per round the four variants run back to back for each vsync mode (main, off, on, boost),
three rounds; each cell is the median of the rounds. Frame times are the client's wall-clock
intervals; the `latency_*` columns are Reflex's own timings, so `main` has none.

| vsync | Variant | Frame p50 / p95 / p99 ms | p99 ÷ p50 | Render call p50 | GPU total p50 | Paced by the sleep | Submit p50 | GPU (Reflex) p50 | **PC latency p50 / p95 / p99** |
|---|---|---|---|---|---|---|---|---|---|
| off | main | 4.179 / 4.191 / 4.199 | 1.005 | 0.168 | 0.657 | — | — | — | — |
| off | off | 4.178 / 4.191 / 4.200 | 1.005 | 0.172 | 0.655 | 0 / 2,400 | 0.150 | 0.675 | **5.076 / 5.228 / 5.279** |
| off | on | 4.220 / 4.241 / 4.256 | 1.009 | 0.171 | 0.657 | 2,399 / 2,400 | 0.150 | 0.676 | **5.121 / 5.280 / 5.344** |
| off | boost | 4.221 / 4.241 / 4.253 | 1.008 | 0.171 | 0.658 | 2,399 / 2,400 | 0.150 | 0.678 | **5.125 / 5.281 / 5.343** |
| on | main | 6.068 / 6.226 / 6.289 | 1.036 | 6.044 | 0.715 | — | — | — | — |
| on | off | 6.067 / 6.234 / 6.303 | 1.039 | 6.045 | 0.717 | 0 / 2,400 | 6.017 | 0.736 | **12.893 / 13.024 / 13.092** |
| on | on | 6.068 / 6.243 / 6.313 | 1.040 | 6.036 | 0.716 | 2,399 / 2,400 | 6.008 | 0.736 | **12.881 / 13.022 / 13.077** |
| on | boost | 6.067 / 6.233 / 6.313 | 1.041 | 6.036 | 0.715 | 2,399 / 2,400 | 6.008 | 0.735 | **12.883 / 13.000 / 13.088** |

The simulation span is 0.010–0.011 ms and the present call 0.020–0.026 ms in every Reflex
row. `pc_latency` is simulation start to the end of the frame's GPU work (amendment 1,
item 7).

Reading it:
- **Nothing else changes.** `off` matches `main` in every column: the markers, the present
  IDs and the submit chain cost nothing measurable.
- **The sleep replaces the limiter.** With vsync off, `on` paces 2,399 of 2,400 frames
  (the first frame has no sleep before it) and the 240 fps limit becomes
  `minimumIntervalUs = 4166`. The driver's interval comes out 0.042 ms longer than the
  platform's `WaitUntil` (4.220 vs 4.178 ms, 237 vs 239 fps).
- **PC latency does not move.** On vs off is +0.045 ms with vsync off and −0.012 ms with
  vsync on, inside the round-to-round spread.

### Why Reflex gains nothing here

Each frame's latency is spent where the extension does not reach:
- **Vsync off.** The render call is 0.17 ms and the GPU work 0.68 ms, but the GPU work ends
  about 4.9 ms after the submit. The second batch waits on the swapchain image's acquire
  semaphore, which the compositor signals when it releases the image. The driver's sleep, timed
  in a temporary instrumented build, averaged 4.0 ms per frame: it enforces
  `minimumIntervalUs` and nothing else.
- **Vsync on (FIFO).** The render call takes 6.04 ms, almost all of it blocked in
  `vkAcquireNextImageKHR`, so the frame is display-bound in the swapchain, not in the render
  queue. The same instrumented build measured a mean sleep of 0.04 ms: the driver does not
  hold the frame back.
- **Uncapped and GPU-heavier.** A temporary local build with the frame limit set to 0
  (≈ 830 fps, Overdrive, 2560×1380, GPU 1.03 ms per frame) gave a mean sleep of 0.007 ms and
  PC latency 3.53 ms on vs 3.55 ms off. Through XWayland it was 0.08 ms and 4.06 vs 4.27 ms.
  With `MOBA_SHADOW_SIZE=8192 MOBA_VFX_SCALE=16` it was 0.008 ms and 3.86 vs 3.86 ms. None of
  these builds is part of the PR; they are single runs, recorded only as diagnosis.

So §6b's expected gain is **not demonstrated** on this desktop (amendment 1, item 8). The
integration does what the extension asks (validation-clean, timings reported, sleep mode
applied per swapchain, every frame attributed to its present ID). The open follow-up is to find
a case where the NVIDIA driver sleeps (a GPU-bound scene, an X11 session, fullscreen direct
scan-out) and to look at the swapchain's own queueing (image count, present mode,
`VK_KHR_present_wait`), which dominates the latency measured here.

## Pacing: 10-minute replay, platform limiter replaced

`scripts/perf-replay.sh` with `RENDER_PATH=tiered` (the R8 pacing gate: the 18,000-tick
teamfight replay, about ten minutes per tier, vsync off, the client's own window) once per
build, the branch first and then `origin/main`, each through its own script and binary. On
the branch the default setting is `on`, so the Reflex sleep paces every frame after the first
(`paced_frames` 141,915 of 141,916 on Low) and the platform limiter is idle. These are single
ten-minute runs, not medians of rounds; `perf-gate` passed every tier of both builds.

| Tier | Build | Frames | Frame p50 / p95 / p99 ms | **p99 ÷ p50** | Max ms | GPU total p95 | PC latency p50 / p95 / p99 |
|---|---|---|---|---|---|---|---|
| Low | branch | 141,856 | 4.220 / 4.241 / 4.430 | **1.05** | 20.53 | 0.516 | 4.957 / 5.122 / 5.476 |
| Low | main | 142,674 | 4.180 / 4.203 / 4.996 | 1.20 | 8.75 | 0.512 | — |
| Medium | branch | 141,790 | 4.219 / 4.241 / 4.819 | **1.14** | 21.75 | 0.719 | 5.119 / 5.292 / 5.918 |
| Medium | main | 142,947 | 4.178 / 4.192 / 4.847 | 1.16 | 10.02 | 0.720 | — |
| High | branch | 141,813 | 4.219 / 4.244 / 4.819 | **1.14** | 8.42 | 0.713 | 5.113 / 5.292 / 5.791 |
| High | main | 142,749 | 4.178 / 4.195 / 4.988 | 1.19 | 50.86 | 0.719 | — |
| Ultra | branch | 141,676 | 4.219 / 4.245 / 4.958 | **1.18** | 12.64 | 0.715 | 5.117 / 5.313 / 6.034 |
| Ultra | main | 142,858 | 4.177 / 4.194 / 4.922 | 1.18 | 22.72 | 0.714 | — |

No frames dropped, and `locked_uploads` and `locked_pipelines` are 0 in every run. The
branch's p50 is 0.04 ms longer for the reason in the on/off table (the driver's interval is a
little longer than `WaitUntil`), so it runs about 0.8 % fewer frames in the same replay. The
single-frame maxima differ run to run on both builds (main's 50.9 ms High spike, the branch's
21.7 ms Medium one) and are one frame each; p99 is the gate.

## Anti-Lag on RADV: blocked

Gate 7 asks for a smoke of native `VK_AMD_anti_lag`, which RADV gains in Mesa 26.3. This
machine's Radeon 610M runs Mesa 26.2.5, which offers the extension only through the implicit
`VK_LAYER_MESA_anti_lag`, off unless `ENABLE_LAYER_MESA_ANTI_LAG=1` is set. The teamfight
replay from tick 330, 300 frames, tiered, 1280×720, vsync off, through
`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/radeon_icd.json`:

| Run | Start-up line | Frame log | Validation |
|---|---|---|---|
| Default (layer off), setting `on` | `[latency] unavailable: the device enables neither VK_NV_low_latency2 (with presentId) nor VK_AMD_anti_lag; the platform limiter paces frames` | `latency=none paced_frames=0/300 latency_timing=unavailable`, p50 13.99 ms | not run |
| Layer forced on, setting `on` | `[latency] anti-lag: VK_AMD_anti_lag rev 1, setting on (maxFPS 240); the latency sleep replaces the platform limiter` | `latency=anti-lag paced_frames=299/300`, p50 13.99 ms, p99 15.38 ms | **4 errors**, all `VUID-vkCmdWriteTimestamp-None-00830` on queries 126, 127, 254 and 255 of a pool the renderer does not own (its timer pools have 80 queries) |
| Layer forced on, setting `off` | `[latency] anti-lag: VK_AMD_anti_lag rev 1, setting off; the platform limiter paces frames` | `latency=anti-lag paced_frames=0/300`, p50 13.98 ms | 0 |

The fallback does what §6 asks: one line, nothing else changes. The forced-layer runs show the
Anti-Lag back end selects, calls `vkAntiLagUpdateAMD` at input and present, and reports itself
in the frame log; the validation errors come from the layer's own timestamp queries, which is
one more reason the layer is not the target. The smoke stays **blocked** until Mesa 26.3 is
installed; the iGPU is GPU-bound at 14 ms a frame here, so latency numbers wait for that too.

## Commands

```bash
# Gate 7 short rounds (main, off, on, boost x vsync 0/1, 3 interleaved rounds)
MOBA_VULKAN_VALIDATION=off MOBA_SETTINGS=/dev/null MOBA_RENDER_PATH=tiered MOBA_LATENCY=<v> \
MOBA_REPLAY=tests/replays/teamfight-5v5.mbr MOBA_WARMUP_TICKS=330 MOBA_SMOKE_FRAMES=2400 \
MOBA_VSYNC=<0|1> MOBA_FRAME_LOG=<log> MOBA_WINDOW_SIZE=1920x1080 ./target/release/moba-client
# Pacing (each build through its own script)
RENDER_PATH=tiered BASELINE= scripts/perf-replay.sh <out>
# Radeon fallback and the forced Mesa layer (add ENABLE_LAYER_MESA_ANTI_LAG=1 for the latter)
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/radeon_icd.json MOBA_SETTINGS=/dev/null \
MOBA_RENDER_PATH=tiered MOBA_LATENCY=<on|off> MOBA_REPLAY=tests/replays/teamfight-5v5.mbr \
MOBA_WARMUP_TICKS=330 MOBA_SMOKE_FRAMES=300 MOBA_VSYNC=0 MOBA_WINDOW_SIZE=1280x720 \
MOBA_FRAME_LOG=<log> ./target/release/moba-client
```
