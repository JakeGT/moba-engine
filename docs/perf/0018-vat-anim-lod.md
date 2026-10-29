# VAT minions, bone LOD and attachments (R4b, ADR-0012 amendment)

Date: 2026-10-29  
Host: AMD Ryzen 7 9800X3D (8 cores / 16 threads), NVIDIA GeForce RTX 5070 Ti, Linux 7.3.0
(CachyOS), rustc stable, release builds. The machine was shared with other agents' builds
during every run (load average 16–30 on 16 threads), so absolute CPU numbers are pessimistic;
the comparisons below were taken under the same load.

## Budget
The R4 exit criterion asks a 200-minion stress test to stay inside the animation CPU budget.
This note sets the budget for **minion animation at 0.5 ms of CPU per frame** for 200 minions
(7 % of the 6.9 ms frame at 144 Hz, half of the 1 ms R4a set for all animation in
[perf/0012](0012-animation-runtime.md)): every minion's player update, pose and instance build
on the main thread. The GPU budget is the tier's frame budget; VAT minions must cost no more
GPU time than a few hundred cubes did.

## CPU: benchmarks
`crates/assets/benches/vat_minions.rs`:

```bash
cargo bench -p moba-assets --bench vat_minions
cargo bench -p moba-assets --bench skin_sample -- animator/200   # R4a reference, same load
```

- `vat/200_minions`: 200 `VatPlayer`s on the shipped `content/vat/minion.toml`, half running
  (idle ↔ run by speed, rate by speed), half standing and attacking every 1.2 s (attack
  crossfaded over locomotion); per frame every minion's `VatPose` at High quality and its
  model-to-world transform — the client's whole per-minion CPU path.
- `vat/200_minions_low`: the same at Low quality (nearest frame, no crossfade).
- `skin_lod/gather_10_heroes`: copying LOD1 palettes out of 10 heroes' full skinning
  matrices, the CPU cost the bone LOD adds.

| Benchmark | Time (median, loaded host) |
|---|---|
| `vat/200_minions` | **8.7 µs** per frame (6.2–8.7 µs over three runs) |
| `vat/200_minions_low` | 8.4 µs (4.8–8.4 µs) |
| `skin_lod/gather_10_heroes` | 102 ns |
| `animator/200_units` (R4a, for comparison, same load) | 157.5 µs (115.6 µs in perf/0012) |
| `animator/200_units_lod4` (R4a) | 49.1 µs |

200 VAT minions use **under 2 % of the 0.5 ms budget** and are about **18× cheaper than 200
R4a animators** measured under the same load: a VAT player is a handful of float operations
and two clip lookups; the bones are baked. Low saves little CPU (the crossfade is two extra
multiplies); its saving is on the GPU (2 instead of 8 texel fetches per vertex).

## CPU: in the client
`MOBA_STRESS_MINIONS=200` (camera with `MOBA_LOCAL_CAMERA_DISTANCE`) adds 200 client-only VAT minions to the local bot preview (half
running in circles, half attacking on the spot, each with its own `VatPlayer`, through the
same player → pose → instance path as match minions) and times that loop every frame. The
mean and p95 go to stderr and the frame log (`stress_cpu_mean_us`, `stress_cpu_p95_us`).

| Run | Tier | Frames | Stress CPU mean | Stress CPU p95 |
|---|---|---|---|---|
| Pinned capture, RTX | Low | 90 | 7.9 µs | 12.9 µs |
| Pinned capture, RTX | Medium | 90 | 14.6 µs (one outlier frame) | 13.2 µs |
| Pinned capture, RTX | High | 90 | 9.4 µs | 13.1 µs |
| Pinned capture, RTX | Ultra | 90 | 9.2 µs | 16.8 µs |
| Free-running, RTX, median of 3 of 5 runs | High | 5,940 | **8.3 µs** | **10.9 µs** |
| The same with `MOBA_VAT=off` (cube instances, no player) | High | 5,940 | 2.0 µs | 3.6 µs |

The worst p95, 16.8 µs, is **3.4 % of the 0.5 ms budget**. The VAT path costs about 6 µs more
than drawing the same 200 minions as cubes.

## GPU
RTX 5070 Ti, 1280×720, High, vsync off, validation off, `MOBA_FRAME_LOG`. Five runs per
configuration of 6,000 frames; the fastest and slowest run (by GPU total p50) are discarded and
the median of the other three is shown. `main` is origin/main at 74f3087 (R5 merged), built
from the same tree without R4b.

| Configuration | GPU total p50 | GPU total p95 | scene p50 | shadow p50 | CPU p50 |
|---|---|---|---|---|---|
| `main`, networked (`MOBA_SERVER=local`, skinned heroes, follow camera) | 0.083 ms | 0.150 ms | 0.045 ms | 0.007 ms | 0.044 ms |
| R4b, networked | 0.087 ms | 0.095 ms | 0.045 ms | 0.007 ms | 0.056 ms |
| R4b, networked, `MOBA_VAT=off` | 0.088 ms | 0.094 ms | 0.045 ms | 0.006 ms | 0.056 ms |
| R4b, networked, `MOBA_BONE_LOD=off` | 0.088 ms | 0.095 ms | 0.045 ms | 0.007 ms | 0.070 ms |
| `main`, local preview (235 m overview) | 0.060 ms | 0.063 ms | 0.021 ms | 0.006 ms | 0.009 ms |
| R4b, local preview (animated heroes, VAT minions) | 0.064 ms | 0.076 ms | 0.021 ms | 0.007 ms | 0.018 ms |
| R4b, 200 stress minions at 40 m, VAT | **0.121 ms** | 0.130 ms | 0.065 ms | 0.021 ms | 0.019 ms |
| R4b, 200 stress minions at 40 m, `MOBA_VAT=off` (cubes) | 0.083 ms | 0.095 ms | 0.045 ms | 0.008 ms | 0.009 ms |

- **200 VAT minions filling the view cost 0.038 ms more GPU time than 200 cubes** (scene
  +0.020 ms, shadow +0.013 ms), all 974-vertex meshes with eight texel fetches per vertex on
  High. On the RTX that is under 1 % of a 144 Hz frame.
- In the networked match the follow camera sees a few minions, and the GPU difference from
  `main` (+0.004 ms p50) is inside run-to-run noise. `MOBA_VAT=off` and `MOBA_BONE_LOD=off`
  make no measurable difference there.
- The main-thread CPU p50 rose by about 0.01 ms in both views. The local preview now runs
  the animation drivers, and the networked view adds LOD classification, attachments and
  the minion driver. The host was loaded by other builds, so the CPU columns are noisy
  (the `MOBA_BONE_LOD=off` row is higher, not lower).
- Frame time is not shown. With vsync off the frame loop still paced at about 4.18 ms
  (p50) on this display in every configuration, so frame time does not separate them.

## Bone-count LOD
LOD1 keeps ≤ 2 influences and ≤ 75 % of the joints, LOD2 1 influence and ≤ 4 joints. On the
10-joint `sentinel-skinned` the levels are 8 joints / 2 influences and 4 joints / 1
influence (`models::tests`). The CPU cost is the palette gather (≈ 10 ns per hero); the
saving is bone-buffer bandwidth (8 or 4 instead of 10 matrices per hero) and vertex work
(1–2 instead of up to 4 influences; `skinned.vert` skips zero weights). With 10 heroes of 974
vertices the skinned work is far below the timer's resolution on this GPU, so the table shows
no measurable difference; the LOD matters on the min-spec GPUs of the roadmap and for rigs with
64 joints, where it was designed to scale.

## Memory
- The sample VAT (`content/vat/minion.ktx2`): 974 vertices × 88 frames × 2 (position, normal)
  × 8 bytes = **1.38 MB** of `RGBA16F`, 4096 × 42 texels, uploaded once.
- `GpuInstance` grows from 80 to 112 bytes (two `vec4`s of VAT frame data). At the 4,096
  instances the unit buffer holds, that is +128 KiB per frame in flight.
- Each LOD level is one more skinned mesh registration per skinned model (two per model).

## Allocation
Steady state allocates nothing (`moba-client` tests, counting allocator):

- `app::minions::tests::steady_state_minion_frames_do_not_allocate`: 150 minions over 119
  snapshots with attack and death events, corpses and poses.
- `app::stress::tests::two_hundred_stress_minions_animate_without_allocating`: 300 frames of
  the 200-minion stress scene.
- `app::local_feed::tests::bot_ticks_become_snapshots_and_events_without_allocating`: the
  local preview's snapshot and event capture.

## Validation
`MOBA_VULKAN_VALIDATION=required` (validation + synchronisation validation) on lavapipe and on
the RTX 5070 Ti; **0 messages in every run** (38 runs):

| Configuration | Tiers | lavapipe | RTX 5070 Ti |
|---|---|---|---|
| Local preview + 200 stress minions, pinned capture | Low, Medium, High, Ultra | 0 | 0 |
| Networked golden configuration (`MOBA_SERVER=local`, skinned heroes, `MOBA_CAPTURE_TICK=120`) | Low, Medium, High, Ultra | 0 | 0 |
| The same with `MOBA_VAT=off`, `MOBA_BONE_LOD=off`, `MOBA_ATTACHMENTS=off`, `MOBA_ANIM_LOD=off` (one each) | High | 0 | 0 |
| Stress scene with `MOBA_VAT=off` (cubes) | High | 0 | 0 |
| Unpinned network smoke, `MOBA_BONE_LOD=0,0` (every hero on LOD2), 600 frames | High | 0 | 0 |
| Unpinned stress smoke, `MOBA_BONE_LOD=0,0`, 600 frames | Medium | 0 | 0 |
| `MOBA_REPLAY=tests/replays/teamfight-5v5.mbr`, skinned heroes | High | 0 | 0 |

Golden fingerprints (the `render-golden.yml` commands on lavapipe, two byte-identical runs
each): the networked fingerprint changed (VAT minions, attachments) and was rebaselined; the
default-scene fingerprint is within tolerance (mean 0.010, max 4) and was left alone. A new
**VAT golden** (`tests/golden/render-vat-fingerprint.rgb`: `MOBA_STRESS_MINIONS=64`,
`MOBA_LOCAL_CAMERA_DISTANCE=40`, pinned frame 90) checks the VAT path pixel-wise; the same
frame with `MOBA_VAT=off` fails it (mean 0.918, max 70 > 24).

## Exit run: 150 ms RTT
`moba-server --listen 127.0.0.1:17977 --seconds 240 --countdown 1` (10 bot heroes), `moba-tools
link-proxy --upstream 127.0.0.1:17977 --listen 127.0.0.1:17978 --rtt 150 --jitter 30 --loss 5`,
and a spectator client through the proxy (`MOBA_SPECTATE=1 MOBA_SPECTATE_FOLLOW=0:0
MOBA_HERO_MODELS=sentinel-skinned MOBA_ANIM_LOG=<file>`, RTX 5070 Ti). The proxy forwarded
13,907 packets and dropped 715 (5.1 %); the server ran 7,166 ticks.

The animation log records every animated hero's state transitions by render tick. Over the
run (all 10 heroes):

| Animation | Transitions logged |
|---|---|
| basic attack | 1,413 |
| cast 1 / 2 / 3 | 192 / 52 / 77 |
| death | 51 |
| locomotion run / idle | 493 / 2,632 |
| recall | 0 — the simulation has no recall; the state is bound but never triggered |

The followed hero (blue slot 0) played cast 3 at render tick 275, cast 1 at 286, a basic
attack at 291, cast 2 at 598, death at 669 and its respawned run from 963. With only bots and
a spectator the match is deterministic, so a second and third run reproduced the same ticks;
the third captured the sequence in [docs/screenshots.md](../screenshots.md#vat-minions-attachments-and-the-150-ms-rtt-run-r4b)
(`MOBA_CAPTURE_SERIES`). The death clip starts a few ticks after the replicated dead flag, by
design (R4a: the clip's release is aligned to the death event's tick, which arrives with the
snapshot).

## Not measured here
- Min-spec GPUs (roadmap §7): the VAT fetch count per tier and the bone LOD were chosen for
  them, but only the RTX 5070 Ti and lavapipe were available.
- A late-game minion count in a real network match (≈ 150 alive): the stress scene covers 200.
