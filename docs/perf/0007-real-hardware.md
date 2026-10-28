# Real-hardware frame timings

Captured 2026-10-18 on the development workstation, not a CI runner.

- GPU: NVIDIA GeForce RTX 5070 Ti (GB203), driver 615.71.09, Vulkan 1.4.351
- CPU: AMD Ryzen 7 9800X3D
- OS: Linux (Wayland/XWayland), Mesa validation layer 1.3.x
- Resolution: 1920×1080 physical (1280×720 logical at 1.5 scale)
- Scene: local bot preview after 3,000 warmup ticks, 840 measured frames, vsync disabled
- Command:
  ```bash
  VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/nvidia_icd.json \
  MOBA_WARMUP_TICKS=3000 MOBA_SMOKE_FRAMES=900 MOBA_VSYNC=0 \
  MOBA_FRAME_LOG=/tmp/frame_nv.log cargo run -p moba-client
  ```
- Result: `mean 4.183 ms`, `p50 4.181 ms`, `p95 4.197 ms`, `p99 4.212 ms`, `max 4.538 ms`.
  540-frame repeat: `p95 4.198 ms`, `max 5.518 ms`.

Validation-required capture at the same scene produced zero validation messages; artifact
`docs/images/m8-nvidia-teamfight.png`. This is well inside the 6.9 ms p95 / 16.6 ms max budget.

## Networked match with skinned heroes (WP-4.2, #289)

Captured 2026-10-28 on the same workstation: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08,
Vulkan 1.4.351, Linux 7.3 (Wayland), Ryzen 7 9800X3D. Physical extent 1920×1080 (from the
frame log's `extent=`), vsync off (`MOBA_VSYNC=0`), validation off. Scene: `MOBA_SERVER=local`
(in-process loopback server, 9 bots plus the idle local hero), whole-map camera
(`MOBA_CAMERA=overview`), frame logging from match tick 960 (minion waves spawn at ticks 30 and
930), 3,000 logged frames, first 60 discarded. The client draws every unit the server sends it
(enemies are filtered by vision).

```bash
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/nvidia_icd.json MOBA_VULKAN_VALIDATION=off \
MOBA_VSYNC=0 MOBA_SERVER=local MOBA_CAMERA=overview MOBA_WINDOW_SIZE=1920x1080 \
MOBA_WARMUP_TICKS=960 MOBA_SMOKE_FRAMES=3000 MOBA_FRAME_LOG=/tmp/frames.log \
MOBA_HERO_MODELS=sentinel-skinned ./target/release/moba-client
```

`cpu_*` is the time from frame start to the render call; `render_*` is the time inside
`Renderer::render_with_ui` (fence wait, acquire, upload, recording, submit, present).

| Heroes | mean | p50 | p95 | p99 | max | cpu p50/p95 | render p50/p95 |
|---|---|---|---|---|---|---|---|
| GPU-skinned (`sentinel-skinned`) | 4.179 | 4.177 | 4.192 | 4.201 | 4.475 | 0.031 / 0.063 | 0.202 / 0.252 |
| Static mesh (`MOBA_NO_SKINNING=1`) | 4.181 | 4.178 | 4.195 | 4.205 | 4.251 | 0.030 / 0.062 | 0.200 / 0.250 |
| Procedural rig (no models) | 4.179 | 4.176 | 4.193 | 4.202 | 4.877 | 0.030 / 0.061 | 0.201 / 0.248 |

All times are in ms. All three configurations are within the 6.9 ms p95 and 16.6 ms max budgets.
The interval is pinned at the display's 240 Hz refresh (4.17 ms) even with vsync off, so the
interval is an upper bound on frame cost. The CPU does about 0.25 ms of work per frame, and
skinning costs nothing measurable against either fallback.

**Bug found and fixed while measuring.** The first runs on this branch were over budget, with or
without skinning:

| Run (before the fix) | mean | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| GPU-skinned | 7.340 | 7.189 | 8.334 | 9.809 | 14.082 |
| Procedural rig | 7.322 | 7.167 | 8.493 | 9.783 | 12.229 |
| GPU-skinned, repeat with the cpu/render split | 7.342 | 7.186 | 8.488 | 9.802 | 12.686 |
| Spectator, so no fog of war and a different camera | 5.733 | 5.512 | 7.325 | 8.257 | 12.276 |
| Diagnostic build without fog of war | 7.270 | 7.135 | 8.086 | 9.689 | 12.287 |

- The split showed `cpu_p50_ms=0.020` and `render_p50_ms=7.160`. A temporary, uncommitted timer
  inside `render_internal` placed 6.8 ms of that between acquire and present, with the fence wait
  at about 0 ms. So the time was CPU-side recording, not GPU work.
- The cause was the UI upload loop. It wrote each vertex into the persistently mapped
  (write-combined) UI buffer, then scaled it in place with `target.position[0] *= scale`. Every
  in-place scale reads back from uncached memory. The networked HUD (health bars, minimap) has
  about 3,200 vertices, against a few hundred in the local preview, which is why the older local
  measurement above never showed it.
- The fix scales a local copy and stores it once. The same diagnostic build then read
  `record_submit` 6.8 → 0.15 ms per frame, and the table at the top of this section is after the
  fix.
- The fix does not change the rendered image: the default golden fingerprint and the networked
  capture are byte-identical before and after.

## GPU skinning: CPU pose cost (WP-4.1)

`cargo bench -p moba-assets --bench skin_sample -- --warm-up-time 1 --measurement-time 3`,
AMD Ryzen 7 9800X3D, 2026-10-27. `Skeleton::sample` per hero per frame (joint chains with a
rotation track per joint plus a root translation track; `rest` = idle, `walk` = clip at full
weight):

| Rig | rest | walk |
|---|---|---|
| 7 joints (shipped `sentinel-skinned.glb` size) | 167.7 ns | 597.5 ns |
| 64 joints (cap) | 947.2 ns | 4.399 µs |

The default bone budget is 16 skinned heroes × 64 joints, so the worst case is about 70 µs of
CPU per frame (≈1 % of the 6.9 ms budget); 16 sample-sized heroes cost under 10 µs. The bone
upload is one `copy_from_slice` of at most 48 KiB into persistently mapped memory. The
networked GPU frame cost of skinned heroes on real hardware is WP-4.2 (#289).

## Windows / macOS

- Windows dedicated GPU results are not available in this environment. CI Windows runners have
  no Vulkan-capable GPU; the cross-OS `gates` job still compiles/tests the renderer.
- macOS: `macos-frame.yml` ran on GitHub's Apple runner (MoltenVK) and recorded
  `frames=240 mean_ms=11.650 p50_ms=9.169 p95_ms=16.945 p99_ms=33.187 max_ms=68.039`
  with vsync off. That exceeds the 6.9 ms p95 target on the hosted/virtualised runner; the
  artifact and command are retained, and the gap is tracked as follow-up #127 rather than
  claimed as a passing min-spec result.
- Windows: `#129/#130` fixed the Vulkan SDK/GLSLC PATH and the probe now builds. Run
  `39174113378` then fails at device creation with `VK_ERROR_INCOMPATIBLE_DRIVER`: the hosted
  runner has no Vulkan-capable GPU. No Windows frame result exists; follow-up #127 covers a
  real Windows GPU run.
- Commands are the same as above with the platform's `VK_ICD_FILENAMES`/MoltenVK ICD path.

## Headless/low-end path

The existing instanced-cube fallback and lavapipe golden path are unchanged; software Vulkan
validation is covered by the separate golden workflow.
