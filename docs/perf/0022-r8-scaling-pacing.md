# Culling, mesh LODs, VRAM budgets and frame pacing (R8)

Captured 2026-10-30 on the development workstation, not a CI runner, for
[ADR-0018](../adr/0018-culling-lods-memory.md).

- GPU: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08 (`nvidia_icd.json`); software: lavapipe
  from `/home/jaket/Projects/MOBA/.tools` with the bundled Khronos validation layer
- CPU: AMD Ryzen 7 9800X3D; OS: CachyOS, Linux 7.3.0-2-cachyos, Wayland
- Build: this branch, release, on `main` at `a0d2e10`; "before" is `main` at `a0d2e10` itself
  (`git archive`, built release in a scratch directory). The host and the GPU were shared with
  other agents' builds and clients during every run (another session was running the client
  on the same GPU), so every comparison is interleaved and taken as the median of three
  rounds. Frame intervals are display-paced (240 Hz, 4.18 ms) as in
  [perf/0011](0011-render-graph.md), so frame p50 does not show GPU savings at this size.

## Summary

| Item | Result |
|---|---|
| Validation + sync validation | 0 messages, every tier, culling on and off, on the RTX 5070 Ti and lavapipe ([below](#validation)) |
| Culling exactness | byte-identical frames with culling on and `MOBA_GPU_CULL=off` in five scenes on lavapipe (`scripts/cull-probe.sh`, now a render-golden step) |
| Culling and LOD gains | 512-minion stress (Ultra): GPU total 0.816 → 0.730 ms p50 (−11 %), shadow −40 %, scene −13 %; teamfight Ultra 0.621 → 0.593 ms (−5 %) |
| CPU cost | the render call grows by 0.009–0.027 ms p50 (culling, level selection, the caster list); the stress scene's whole per-frame cull + select over 612 units is inside that |
| VRAM | 58 MB (Low) to 263 MB (Ultra) at 1080p, every tier far inside its budget; texture bytes 6.8–7.5 MB |
| Mid-match uploads / pipeline builds | 0 / 0 in every run (`locked_uploads`, `locked_pipelines`) |
| 10-minute pacing (p99 ≤ 1.5 × p50) | p99 ÷ p50 = 1.006 (Low), 1.044 (Medium), 1.289 (High), 1.248 (Ultra); GPU p99 0.44–1.08 ms, every tier inside its §7 budget ([below](#frame-pacing-10-minute-replay)) |
| Goldens | culling: unchanged; LODs: deliberate rebaseline of every fingerprint (silhouette pixels only; twelve after R3b) |

## GPU-driven culling: the measurement that decided it

Measured before any code (ADR-0018 §"What the instance counts are today"), `main` at
`a0d2e10`, Ultra, 1920×1080:

| Scene | Instances | CPU before the render call | Render call p50 | GPU scene / prepass |
|---|---|---|---|---|
| teamfight replay (tick 330) | ≈ 150 | 0.006 ms | 0.101 ms | 0.194 / 0.066 ms |
| 512 VAT stress minions | ≈ 700 | 0.033 ms | 0.107 ms | 0.364 / 0.147 ms |

Adding 550 instances moves the whole render call (bucketing, upload, recording, submission and
present) by 6 µs. A GPU-driven path can only remove the per-instance sphere tests, which cost
nanoseconds each; it would add a compute dispatch and its barriers and a second path to
validate on every tier. It is designed and deferred (ADR-0018 §2) with explicit triggers: a
render call above 0.5 ms from instance work, more than about 10 k instances, or GPU-resident
instance data.

## Before and after (RTX 5070 Ti, 1920×1080, vsync off, validation off)

The R5 evidence scene: `tests/replays/teamfight-5v5.mbr` from tick 330 for 2,400 frames (the
first 60 are warm-up), each tier's defaults, and the local preview with 512 VAT stress
minions on Ultra (`MOBA_STRESS_MINIONS=512`). Runs are interleaved per tier (before-low,
after-low, before-medium, …), the whole matrix 3 times; each cell is the median of the
rounds. GPU ms p50 (p95 for `total`); `render` is the CPU time inside the render call.

| Run | GPU total p50 / p95 | shadow | prepass | scene | render p50 / p95 | units drawn / casters / in | terrain chunks drawn | LOD0/1/2/3 units |
|---|---|---|---|---|---|---|---|---|
| before-low | 0.182 / 0.407 | 0.004 | — | 0.098 | 0.097 / 0.262 | all | 1 mesh | — |
| after-low | 0.182 / 0.374 | 0.004 | — | 0.097 | 0.106 / 0.276 | 53 / 62 / 64 | 5 / 16 | 53/0/0/0 |
| before-medium | 0.254 / 0.653 | 0.006 | — | 0.126 | 0.125 / 0.267 | all | 1 mesh | — |
| after-medium | 0.261 / 0.612 | 0.006 | — | 0.126 | 0.139 / 0.297 | 53 / 62 / 64 | 7 / 36 | 53/0/0/0 |
| before-high | 0.411 / 0.709 | 0.006 | 0.021 | 0.127 | 0.117 / 0.299 | all | 1 mesh | — |
| after-high | 0.404 / 0.915 | 0.006 | 0.020 | 0.124 | 0.144 / 0.310 | 53 / 62 / 64 | 9 / 64 | 53/0/0/0 |
| before-ultra | 0.621 / 0.632 | 0.012 | 0.075 | 0.222 | 0.116 / 0.196 | all | 1 mesh | — |
| after-ultra | 0.593 / 0.604 | 0.012 | 0.062 | 0.207 | 0.136 / 0.209 | 53 / 62 / 64 | 14 / 144 | 53/0/0/0 |
| before-stress | 0.816 / 0.831 | 0.043 | 0.150 | 0.402 | 0.130 / 0.209 | all | 1 mesh | — |
| after-stress | 0.730 / 0.743 | 0.026 | 0.135 | 0.348 | 0.147 / 0.232 | 612 / 607 / 612 | 99 / 144 | 76/0/527/9 |

```bash
MOBA_QUALITY=<tier> MOBA_WINDOW_SIZE=1920x1080 MOBA_REPLAY=tests/replays/teamfight-5v5.mbr \
MOBA_WARMUP_TICKS=330 MOBA_SMOKE_FRAMES=2400 MOBA_VSYNC=0 MOBA_FRAME_LOG=/tmp/r8.log \
[MOBA_GPU_CULL=off] [MOBA_MESH_LOD=off] cargo run --release -p moba-client
MOBA_QUALITY=ultra MOBA_STRESS_MINIONS=512 MOBA_SMOKE_FRAMES=2400 ...   # the stress rows
```

Reading it:
- **Terrain.** The default 46 m camera sees 5–14 of the heightfield's chunks (9–31 %); the
  prepass and scene draw only those runs. On this GPU the terrain was never the bottleneck,
  so it shows as a few hundredths of a millisecond (Ultra prepass −17 %, scene −7 %); it
  matters on the UHD 620 class, where vertex throughput is scarce.
- **Units.** At the default camera the teamfight keeps 53 of 64 units on screen and 62 in the
  shadow box; the stress scene keeps all 612 in view. Culling earns little in a camera that
  frames the fight, which is what a MOBA camera does; it bounds the cost when the camera is
  elsewhere.
- **LODs.** The teamfight draws every unit at LOD0 at 1080p (heroes and minions are tens of
  pixels tall, so the 0.5 px Ultra tolerance keeps them full detail). The stress scene's
  minions at 40–80 m draw LOD2 (527 of 612), which is most of its saving: the shadow pass
  (−40 %) and the scene (−13 %). The attribution runs below confirm it: in the stress scene
  the saving is all LODs, in the teamfight all terrain culling.
- **High p95.** `after-high` GPU p95 is higher than `before-high` (0.915 vs 0.709) while the
  p50s agree (0.404 vs 0.411) and every pass's p50 is equal or lower. The Low–High p95s move
  by 0.1–0.3 ms between rounds in both builds: the GPU was shared with another session's
  client, whose frames land in some of ours. This is why `perf-gate` allows +25 % over a
  baseline taken on the runner itself, and why the baseline must come from the same machine.
- **CPU.** The render call grows by 9–27 µs p50: per-unit sphere tests and level selection,
  writing the caster list's copy of each instance and the terrain runs. It stays far below
  the 0.5 ms trigger.

## Culling and LOD attribution

This branch's build only, Ultra, interleaved per round (on, culling off, LODs off, both off),
3 rounds, median. `MOBA_GPU_CULL=off` and `MOBA_MESH_LOD=off` are the off switches.

| Scene, config | GPU total p50 / p95 | shadow | prepass | scene | render p50 | drawn / casters | chunks | LOD0/1/2/3 |
|---|---|---|---|---|---|---|---|---|
| teamfight, on | 0.588 / 0.603 | 0.012 | 0.062 | 0.206 | 0.120 | 53 / 62 | 14/144 | 53/0/0/0 |
| teamfight, culling off | 0.622 / 0.633 | 0.012 | 0.076 | 0.220 | 0.121 | 64 / 64 | 144/144 | 64/0/0/0 |
| teamfight, LODs off | 0.595 / 0.607 | 0.013 | 0.062 | 0.207 | 0.121 | 53 / 62 | 14/144 | 53/0/0/0 |
| teamfight, both off | 0.621 / 0.634 | 0.012 | 0.077 | 0.221 | 0.098 | 64 / 64 | 144/144 | 64/0/0/0 |
| stress 512, on | 0.734 / 0.748 | 0.027 | 0.135 | 0.350 | 0.137 | 612 / 607 | 99/144 | 76/0/527/9 |
| stress 512, culling off | 0.737 / 0.750 | 0.025 | 0.135 | 0.354 | 0.159 | 612 / 612 | 144/144 | 76/0/527/9 |
| stress 512, LODs off | 0.816 / 0.831 | 0.045 | 0.158 | 0.393 | 0.125 | 612 / 607 | 99/144 | 612/0/0/0 |
| stress 512, both off | 0.820 / 0.833 | 0.044 | 0.159 | 0.396 | 0.132 | 612 / 612 | 144/144 | 612/0/0/0 |

- **Both off reproduces `main`** (teamfight 0.621 vs 0.621 ms, stress 0.820 vs 0.816 ms): the
  switches are real off switches.
- **Culling** pays in the teamfight (−0.033 ms, all of it terrain: prepass −19 %, scene −6 %)
  and nothing in the stress scene, whose camera keeps every unit and two-thirds of the
  terrain in view.
- **LODs** pay in the stress scene (−0.082 ms: shadow −40 %, prepass −15 %, scene −11 %) and
  nothing in the teamfight, where every unit is close enough for LOD0.
- **CPU.** Culling and level selection cost ≈ 22 µs in the teamfight render call (0.098 →
  0.120 ms) and ≈ 5 µs in the stress scene, whose render call is dominated by other work. The
  stress rows' render p50 spread (0.125–0.159 ms) is larger than that and does not follow
  the switches; it is CPU contention from the shared host.

## Mesh LOD bake

`scripts/bake-mesh-lods.sh` (meshoptimizer 1.3 through `meshopt` 0.7.0, `--features
mesh-bake`, ratios 50/25/12.5 %): `Permissive` with normal (0.5) and UV (1.0) attribute
weights, and each level's error the largest of meshoptimizer's estimate and the measured
distance from every dropped LOD0 vertex to the level's surface (over the bind pose and 31
sampled clip poses for the skinned model). Errors are object-space, in model units; two
bakes are byte-identical.

| Model | Vertices | UV-seam vertices | LOD0 | LOD1 (error) | LOD2 (error) | LOD3 (error) | `.lod` bytes |
|---|---|---|---|---|---|---|---|
| crystal | 288 | 184 | 138 | 68 (0.191) | 34 (0.327) | 16 (0.577) | 1,464 |
| rampart | 144 | 144 | 72 | 36 (0.982) | 18 (1.218) | 8 (2.123) | 792 |
| sentinel | 902 | 750 | 462 | 230 (0.206) | 114 (0.372) | 56 (0.555) | 4,848 |
| sentinel-skinned (heroes, VAT minions) | 974 | 0 (no UVs) | 498 | 248 (0.206) | 124 (0.430) | 62 (0.550) | 5,256 |

The first bake (plain `--permissive`, meshoptimizer's error only) had the same triangle
counts with errors of 0.084–0.765: the measured errors are 2–3× meshoptimizer's own estimate,
so levels now switch about twice as far away (and the golden captures changed accordingly).
Seam-preserving mode (`--keep-seams`) removes almost nothing from these flat-shaded meshes
(every face edge is an attribute seam), and locking the UV-seam vertices
(`--protect-uv-seams`) bakes no levels for rampart and sentinel and one 88-triangle level for
crystal, because 64–100 % of their vertices are on a UV seam (per-face palette mapping). The
UV weight is checked by a test that measures the texture-coordinate distortion of a level
(it fails with the weight set to 0). The sample meshes are tiny; the point is the pipeline.
A production hero at 20–40 k triangles gets the same three levels.

## VRAM accounting (1920×1080, teamfight replay)

| Tier | Total | Textures | Render targets | Buffers | Peak | Budget (total / textures) | Uploads at load |
|---|---|---|---|---|---|---|---|
| Low | 58.3 MB | 6.8 MB | 41.8 MB | 9.7 MB | 60.5 MB | 1 GiB / 384 MiB | 28 |
| Medium | 99.9 MB | 7.5 MB | 80.4 MB | 12.0 MB | 101.7 MB | 2 GiB / 1 GiB | 30 |
| High | 110.4 MB | 7.5 MB | 87.5 MB | 15.3 MB | 112.2 MB | 4 GiB / 2.5 GiB | 30 |
| Ultra | 262.5 MB | 7.5 MB | 231.3 MB | 23.7 MB | 269.1 MB | 6 GiB / 4 GiB | 30 |

The counters are the sum of `vkGet*MemoryRequirements` sizes of every live image and buffer
(the swapchain images, owned by the WSI, and the allocator's unused block space are not
included). Ultra's targets are dominated by the 4096² shadow map and the MSAA 4× attachments.
The sample content is small; the budgets are sized for production heroes and maps (ADR-0018
§4). Over-budget textures drop their largest mips (unit-tested in
`memory::tests::over_budget_textures_drop_their_largest_mips_until_they_fit`); no sample
texture comes near the budget, so no run degraded one (`degraded_textures=0`).

**Load-time-only uploads.** Every run above and below reports `locked_uploads=0` and
`locked_pipelines=0`: after the first measured frame no mesh, texture or terrain was uploaded
and no pipeline was built. Every pipeline is built at renderer creation through the
persistent cache (`pipeline_cache=warm` in every run here).

## Frame pacing (10-minute replay)

`scripts/perf-replay.sh` on the RTX 5070 Ti: `teamfight --ticks 18000` recorded once (a
10-minute, 5v5 scripted match), then played once per tier at 1920×1080 with vsync off and
validation off, each after a 30-frame run that warms the pipeline cache. Every frame of the
10 minutes is in the log (≈ 142 k frames per tier). `perf-gate --warm-cache` checked all four
logs: **ok**.

These runs predate the review fixes (pose-measured skin bounds, re-baked LODs, the cap after
culling) and R3b. Their logs have no `frames_dropped`, which the stricter gate now requires,
although every frame was logged (the counts above are the whole run). The re-run on the
merged build, with `tests/perf/baseline.toml` refreshed from it, is pending: at the time of
the review the shared RTX 5070 Ti had 15.6 of its 16 GiB held by another process, and the
client could not allocate. The committed baseline keeps these p95 values, now bound to
`gpu = "NVIDIA GeForce RTX 5070 Ti"`.

| Tier | Frames | Frame p50 / p95 / p99 / max (ms) | p99 ÷ p50 | GPU total p50 / p95 / p99 (ms) | §7 GPU budget | CPU p95 | render p95 | mid-match uploads / pipelines | cache |
|---|---|---|---|---|---|---|---|---|---|
| Low | 143,427 | 4.177 / 4.194 / 4.203 / 13.957 | 1.006 | 0.382 / 0.423 / 0.439 | 14 ms | 0.032 | 0.279 | 0 / 0 | warm |
| Medium | 143,208 | 4.177 / 4.193 / 4.359 / 60.571 | 1.044 | 0.525 / 0.578 / 0.600 | 6.0 ms | 0.029 | 0.302 | 0 / 0 | warm |
| High | 142,035 | 4.181 / 4.375 / 5.391 / 92.207 | 1.289 | 0.673 / 1.037 / 1.078 | 6.0 ms | 0.028 | 0.315 | 0 / 0 | warm |
| Ultra | 142,357 | 4.181 / 4.279 / 5.217 / 9.844 | 1.248 | 0.551 / 0.599 / 0.604 | 7.5 ms | 0.027 | 0.171 | 0 / 0 | warm |

```bash
cargo build --release --locked -p moba-client -p moba-tools
BASELINE= scripts/perf-replay.sh perf-replay           # 4 × 10 minutes, then perf-gate
./target/release/perf-gate --warm-cache --baseline tests/perf/baseline.toml perf-replay/frames-*.log
```

- **p99 ≤ 1.5 × p50 holds on every tier** (1.006–1.289). The frame interval is
  compositor-paced at 240 Hz (4.18 ms) even with vsync off, so p50 is the display interval
  and p99 counts the frames that missed it.
- **No compile or allocation hitch.** `locked_pipelines=0` and `locked_uploads=0` over all
  four 10-minute runs: no pipeline was built and no mesh, texture or terrain uploaded after
  the first measured frame, and every tier started from a warm cache. CPU p95 is ≤ 0.032 ms
  and GPU p99 ≤ 1.08 ms, so neither side of the frame came near the interval.
- **The max spikes** (Medium 60.6 ms and High 92.2 ms, one or a few frames each) are not the
  client's own work. CPU and GPU p99 stay flat through them, and no upload or pipeline build
  happened. The GPU and the compositor were shared with another session's client running at
  the same time; a dedicated runner should not show them. `perf-gate` gates p99, not max.
- **The committed baseline** (`tests/perf/baseline.toml`) is these runs' `gpu_total_p95_ms`
  and `render_p95_ms`, and the same logs pass the gate against it. The High p95 (1.037 ms) is
  the noisiest baseline value (see the High p95 note above).
- 10 minutes of the scripted teamfight draws 46 of 58 units in view and LOD0 throughout at
  1080p (the camera follows the fight); terrain 5/16 (Low) to 12/144 (Ultra) chunks.


## Validation

`MOBA_VULKAN_VALIDATION=required` (Khronos validation with synchronization validation, which
the renderer enables; the run fails if the layer is missing), 1280×720, LODs on:

| Device | Scene | Low | Medium | High | Ultra |
|---|---|---|---|---|---|
| RTX 5070 Ti | teamfight replay, culling on / off, 300 frames | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| RTX 5070 Ti | 64 VAT minions, culling on / off | | | | 0 / 0 |
| lavapipe | teamfight replay, culling on / off, 90 frames | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| lavapipe | 64 VAT minions, culling on / off | | | | 0 / 0 |

Every cell is the number of `[vulkan …]` messages. Also 0 on lavapipe: `scripts/cull-probe.sh`
(five scenes, culling on and off) and the eight golden captures with LODs on and off. The nightly perf workflow repeats the
teamfight rows on its runner before the timed runs.

## Goldens

Culling never changes a pixel: `scripts/cull-probe.sh` captured five scenes on lavapipe with
validation (the local preview, the low horizon orbit, Ultra with 64 VAT minions, Low with hull
outlines at render scale 0.75, and the networked match at tick 120) with culling on and off;
all five pairs are byte-identical, with 0 validation messages, and each culled run did cull
(units 131/164 and chunks 10/144 in the Ultra VAT scene, units 32/43 and chunks 8/64 in the
networked match, chunks 38/64 at the horizon).

LODs do change pixels, at silhouettes. Every golden capture with `MOBA_MESH_LOD=off` matched
the old fingerprint exactly (`max_diff=0`), so the LODs are the only change; with LODs on,
the full-resolution 1280×720 captures differ from LOD-off in 0.01–0.86 % of pixels (Ultra
0.01 %, Low 0.06 %, overview 0.06 %, default 0.10 %, ember 0.13 %, horizon 0.17 %, networked
0.57 %, the High VAT close-up 0.86 %), all on silhouette edges, and the fingerprints by at most 7/255. All eight fingerprints were rebaselined
deliberately with the render-golden commands on lavapipe; two complete runs gave
byte-identical captures. The comparison image is in [screenshots](../screenshots.md).

**Review re-bake.** The attribute-weighted, pose-measured levels (larger errors, so each unit
keeps a finer level to a greater distance) moved the fingerprints against the first R8
baseline by mean 0.000–0.026 and at most 7/255 (the networked match and the VAT close-up most);
all eight were rebaselined again on lavapipe, two complete runs byte-identical with 0
validation messages, and `scripts/cull-probe.sh` still reports all five scenes byte-identical
with culling on and off with the measured skinned spheres (Ultra VAT units 131/164, networked
32/43).

**After R3b.** Merged with R3b's decals, foliage and water (ADR-0017, twelve goldens), the
twelve captures against R3b's fingerprints differ by at most 2/255 (default, low, overview,
horizon, networked; the other seven match exactly at fingerprint resolution); all twelve were
written from one of two lavapipe runs that were byte-identical (24 files), with 0 validation
messages; `scripts/cull-probe.sh` and `scripts/outline-probe.sh` pass on the merged build.

## Hardware matrix (#9/#127)

The roadmap's §7 reference machines are maintainer items: this environment has one real GPU
(the RTX 5070 Ti above) and hosted CI has none on Windows or macOS. Each machine below gets a
copy of this checklist filled in, committed as a section of this note (or a follow-up perf
note), before its tier's target can be called met. Estimates never close a target.

| Machine | Tier | Target (§7) | Status |
|---|---|---|---|
| Intel UHD 620 (Windows or Linux) | Low | 60 fps p95, GPU ≤ 14 ms, 1080p (render scale allowed) | open (#127) |
| Apple M1, MoltenVK | Low | 60 fps p95, GPU ≤ 14 ms, 1080p | open (#9, #127) |
| GTX 1060 / RX 580 class | Medium | 144 fps p95, GPU ≤ 6.0 ms, 1080p | open (#127) |
| RTX 3060 class | High | 144 fps p95, GPU ≤ 6.0 ms, 1440p | open (#127) |
| RTX 4070+ class | Ultra | 120 fps p95, GPU ≤ 7.5 ms, 4K | open (#127); the RTX 5070 Ti above is above this class at 1080p only |

Per machine:

1. **Record the machine.** GPU and VRAM, driver version, OS and build, CPU, RAM, display
   refresh rate, Vulkan ICD (`vulkan-probe` output), power plan (plugged in, performance).
2. **Build.** `cargo build --release --locked -p moba-client -p moba-tools` at a recorded
   commit.
3. **Validation, every tier, culling on and off** (0 messages):
   ```bash
   for tier in low medium high ultra; do for cull in cpu off; do
     MOBA_VULKAN_VALIDATION=required MOBA_QUALITY=$tier MOBA_GPU_CULL=$cull \
     MOBA_REPLAY=tests/replays/teamfight-5v5.mbr MOBA_WARMUP_TICKS=330 MOBA_SMOKE_FRAMES=300 \
     MOBA_VSYNC=0 ./target/release/moba-client 2>&1 | grep -c '^\[vulkan'
   done; done
   ```
4. **The 10-minute replay per tier** at the tier's §7 resolution, validation off, vsync off:
   `WINDOW=<w>x<h> TIERS=<tier> BASELINE= scripts/perf-replay.sh perf-<machine>`. Keep every
   frame-log line.
5. **Gate.** `perf-gate perf-<machine>/frames-<tier>.log` must print `ok`: GPU p95 within the
   §7 budget, frame p99 ≤ 1.5 × p50, `locked_uploads=0`, `locked_pipelines=0`.
6. **Report** in the table format of the RTX 5070 Ti section: frame p50/p95/p99, GPU total
   p50/p95/p99, the per-pass GPU columns, `vram_mb`/`texture_mb`, `cull_drawn`/
   `cull_candidates`, `terrain_chunks`, `lod_instances`, the pipeline cache state, and whether
   the fps target was met. On a fail, the most expensive pass and the next step.
7. **Runner (optional).** To make the machine the nightly perf runner, register it as a
   self-hosted runner with the labels `self-hosted, gpu`, set the repository variable
   `HAS_GPU_RUNNER=true`, run step 4 there with `BASELINE=` and commit its
   `gpu_total_p95_ms`/`render_p95_ms` per tier to `tests/perf/baseline.toml` (the committed
   baseline is the RTX 5070 Ti's).
