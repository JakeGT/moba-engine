# Render graph, HDR target and tonemap: GPU cost (R1a)

Captured 2026-10-28 on the development workstation, not a CI runner.

- GPU: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08 (`nvidia_icd.json`)
- CPU: AMD Ryzen 7 9800X3D
- OS: Linux 7.3.0-2-cachyos, Wayland
- Window: `MOBA_WINDOW_SIZE=1920x1080` (frame log `extent=1920x1080`), vsync off, validation off
- Power state: the frame is so light that the GPU stays in **P8** (997 MHz graphics, 405 MHz
  memory, 3 % utilisation, 20 W, read with `nvidia-smi` mid-run). All µs figures below are at
  those clocks. Bandwidth-bound work, such as the fullscreen composite, is slower than it would
  be at full clocks.

## Method

- **After:** this branch. The `MOBA_FRAME_LOG` report now has per-pass GPU columns from
  timestamp queries (`gpu_scene`, `gpu_composite`, `gpu_ui`, `gpu_total`, `gpu_span`, each as
  p50/p95). Each pass writes a timestamp after its barriers and another after its commands, both
  at `ALL_COMMANDS`. `total` is the sum of the passes. `span` runs from the first pass's start
  to the last pass's end, so `span − total` is the time spent in the barriers between passes.
- **Before:** `origin/main` at `d9f279b`, plus a measurement-only patch that was never
  committed. It writes one timestamp pair the same way around today's single forward pass:
  after its initial barriers, and after `cmd_end_rendering`. That covers scene plus UI and
  excludes the capture and present transition, just as `total` does. It prints p50/p95 at exit
  over the same frames the frame log measures (the last 840 or 2,940 frames).
- **Scenes:** the two from [perf/0007](0007-real-hardware.md). Each binary ran three
  alternating rounds. Round 1 of the baseline's networked scene also sampled lobby frames, so it
  is left out.

```bash
# Local bot preview
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/nvidia_icd.json MOBA_VULKAN_VALIDATION=off \
MOBA_VSYNC=0 MOBA_WINDOW_SIZE=1920x1080 MOBA_WARMUP_TICKS=3000 MOBA_SMOKE_FRAMES=900 \
MOBA_FRAME_LOG=/tmp/frames.log ./target/release/moba-client
# Networked match, GPU-skinned heroes, whole-map camera
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/nvidia_icd.json MOBA_VULKAN_VALIDATION=off \
MOBA_VSYNC=0 MOBA_SERVER=local MOBA_CAMERA=overview MOBA_WINDOW_SIZE=1920x1080 \
MOBA_WARMUP_TICKS=960 MOBA_SMOKE_FRAMES=3000 MOBA_FRAME_LOG=/tmp/frames.log \
MOBA_HERO_MODELS=sentinel-skinned ./target/release/moba-client
```

## Results (GPU ms, p50 / p95)

The frame log prints GPU columns with three decimals (1 µs resolution). The baseline patch
printed four.

**Networked match** (2,940 measured frames per run):

| Build | scene | composite | ui | total | span |
|---|---|---|---|---|---|
| before, round 2 (single pass) | — | — | — | 0.0298 / 0.0300 | — |
| before, round 3 | — | — | — | 0.0297 / 0.0300 | — |
| after (High, `R16G16B16A16_SFLOAT`), rounds 1–3 | 0.027 / 0.027 | 0.014 / 0.014 | 0.005 / 0.005–0.008 | 0.046 / 0.046–0.049 | 0.046 / 0.046 |
| after (Low, `B10G11R11_UFLOAT_PACK32`) | 0.026 / 0.027 | 0.014 / 0.014 | 0.005 / 0.005 | 0.045 / 0.045 | 0.045 / 0.045 |

**Local bot preview** (840 measured frames per run):

| Build | scene | composite | ui | total | span |
|---|---|---|---|---|---|
| before, rounds 1–3 (single pass) | — | — | — | 0.0133 / 0.0137–0.0248 | — |
| after (High), rounds 1–3 | 0.016 / 0.028–0.030 | 0.015 / 0.016 | 0.003 / 0.003 | 0.034 / 0.045–0.047 | 0.034 / 0.045–0.047 |
| after (Low) | 0.013 / 0.020 | 0.015 / 0.016 | 0.003 / 0.003 | 0.031 / 0.038 | 0.031 / 0.038 |

**Frame interval and CPU** (ms, p50/p95, unchanged within noise). The interval is pinned at the
display's 240 Hz (4.17 ms) even with vsync off, as in perf/0007:

| Scene | before interval | after interval | before `render_*` | after `render_*` |
|---|---|---|---|---|
| networked | 4.177–4.180 / 4.191–4.216 | 4.176–4.177 / 4.189–4.192 | 0.200–0.212 / 0.251–0.313 | 0.207–0.211 / 0.260–0.270 |
| local | 4.176–4.178 / 4.187–4.194 | 4.176–4.177 / 4.188–4.190 (round 3: 4.184 / 4.879) | 0.046–0.049 / 0.096–0.105 | 0.048–0.049 / 0.094–0.097 (round 3: 0.115 / 0.161) |

The round-3 local outlier of the after build occurred in one run only, with the CPU-side
`render_*` time raised too. It reads as host noise, not GPU work: its GPU columns match the
other runs.

## Reading the numbers

- **Graph barriers cost nothing measurable.** `span − total` is below the 1 µs resolution in
  every run. The barriers the graph derives (three before the scene, two before the composite,
  one before the UI, one at the end) do not show up as GPU time.
- **The new work is the composite.** The fullscreen exposure plus AgX pass costs 14–15 µs at
  1920×1080 in P8. Low's packed target does not make it cheaper: the pass is not limited by
  reading the target at this size.
- **The HDR scene pass costs a little more than writing BGRA8.** Networked: scene plus UI is
  0.032 ms against 0.0298 ms for the single pass (+2 µs, +7 %). Local: 0.019 against 0.0133 ms
  (+6 µs). The 64-bit target doubles colour bandwidth. The UI now loads the swapchain image in a
  separate rendering scope. On Low, the packed format brings the local scene to 0.013 ms.
- **Overall:** +16 µs per frame (networked, +54 %) and +21 µs (local, +156 %) of GPU time, for
  HDR and a tonemap. That is 0.2–0.3 % of the 6.9 ms p95 budget, and the frame interval and the
  CPU render time are unchanged.

### Against the roadmap exit criterion (≤ 5 % GPU-time overhead)
**Not met as written.** Today's frame does 13–30 µs of GPU work. An HDR target needs at least
one extra fullscreen read and write to reach the swapchain, and that alone is about 14 µs here.
No design that adds the HDR target and tonemap R1 asks for can stay within 5 % of a 30 µs
frame. The parts that are *graph* overhead are within it:

- inter-pass barriers: below 1 µs;
- CPU-side `render_*`: within run-to-run noise.

The maintainer should decide whether the criterion means one of these:

1. graph and barrier overhead for the same work, which is met;
2. an absolute budget, which is met for any reasonable value: +0.016–0.021 ms;
3. the literal relative figure, which is not met and cannot be with HDR.

The relative figure should be re-measured on the R5 teamfight scene and on the §7 reference
GPUs at their real clocks. Low/min-spec numbers (UHD 620, MoltenVK) do not exist yet (#9/#127).

## Validation

- Lavapipe, `MOBA_VULKAN_VALIDATION=required` (validation plus synchronisation validation), the
  `render-golden.yml` commands: the default capture and the networked capture each reported 0
  messages. A 200-frame Low run and a 200-frame High run also reported 0.
- NVIDIA 615.78.08 with the Khronos validation layer and synchronisation validation: the default
  capture, the networked capture and a 600-frame Low-tier run each reported 0 messages.
