# N0.2: the capability gate changes no image and no GPU time

Captured 2026-11-01 on the development workstation, not a CI runner, for
[ADR-0020](../adr/0020-next-gen-foundations.md) §2 and verification gate 2.

- **Branch and baseline:** branch `feat/n0-capability-gate` at `4307b8b` against N0.1
  (`feat/n0-ash-pin`, `5c0d245`). The review fixes that followed (`96d7487`) touch only the
  probe and Tiered device creation; [what they change](#after-the-review-fixes) is below.
- **Hardware and software:**
  - GPU: NVIDIA GeForce RTX 5070 Ti (16 GB), driver 615.78.08.
  - Software Vulkan: lavapipe from `/home/jaket/Projects/MOBA/.tools`, with the bundled
    Khronos validation layer.
  - CPU: AMD Ryzen 7 9800X3D. OS: Linux 7.3.0-5-cachyos, Wayland.
  - Build: release.

## Admission on the local devices

Recorded with `vulkan-probe --json` and the validation layer loaded (no message). The dumps
are the recorded fixtures in `crates/render/src/caps/fixtures/`; the unit tests check every
fixture's path, `max_tier` and `missing` list.

| Device | Driver, Vulkan | `auto` path | `max_tier` | Notes |
|---|---|---|---|---|
| RTX 5070 Ti | NVIDIA 615.78.08, 1.4.351 | tiered | overdrive | Every 1.4-promoted feature, read through its KHR/EXT struct |
| Radeon 610M (RADV) | Mesa 26.2.5, 1.4.354 | tiered | overdrive | An RT correctness device, not a performance reference |
| lavapipe | `.tools`, 1.4.354 | compat | none (overdrive with `tiered`) | A CPU device with the whole tiered set: `auto` gives Compat, and `MOBA_RENDER_PATH=tiered` admits it, which gives N0.5's `rt_smoke` a CI device |

The six synthetic fixtures (MoltenVK and KosmicKrisp on M1, NVK on an RTX 2060, GTX 1060,
UHD 620, an RT GPU without mesh shaders) are labelled `"source": "synthetic"` and close no
target. The macOS MoltenVK workflow asserts `path=compat` on the real runner.

## Goldens: byte-identical on both paths (lavapipe)

All fifteen of the golden workflow's scenes were captured once on N0.1 and twice on the branch. Every
capture ran at 1280×720 on lavapipe with `MOBA_VULKAN_VALIDATION=required`, which includes
synchronization validation. The full PPMs were compared with `cmp`. Every run exited cleanly
with no validation message. The branch ran twice: once on its default path (`auto`, Compat on
lavapipe) and once with `MOBA_RENDER_PATH=tiered`.

| Scene (golden workflow step) | Compat | Tiered | SHA-256 (first 16) |
|---|---|---|---|
| default (frame 90) | identical | identical | `5a476005e52ac38b` |
| networked, skinned heroes (tick 120) | identical | identical | `e7e6198ad2260eb3` |
| VAT minions, 64 (frame 90) | identical | identical | `09b8ef828a3eed7b` |
| Ultra, 64 minions (frame 90) | identical | identical | `4b8beb4c337bd8be` |
| Low at render scale 0.75 (frame 90) | identical | identical | `25aa95a501ac7ad1` |
| Ember map, camera 24 (tick 120) | identical | identical | `342a3fa53facf1df` |
| teamfight decals (replay, frame 30) | identical | identical | `8a07fb957cb153b2` |
| overview camera, skinned heroes (tick 120) | identical | identical | `b0c819a49b7f3843` |
| horizon (frame 90) | identical | identical | `79f126b8576a82bc` |
| river (frame 90) | identical | identical | `b34379f7ad633a4d` |
| river, Low (frame 90) | identical | identical | `6d4feae28f6f3160` |
| river, Ultra (frame 90) | identical | identical | `217e397eef618b00` |
| heat haze, cinder zone, Ultra (frame 90) | identical | identical | `cedc451ffedcc6f3` |
| heat haze, bloom off | identical | identical | `ba6f715199a7c375` |
| heat haze, bloom off, distortion no-draw | identical | identical | `a3846daa3f69e8cf` |

The hashes equal N0.1's ([perf/0025](0025-n0-ash-pin.md)), so they equal `main`'s too.

## RTX 5070 Ti timings: within noise

The teamfight replay (`tests/replays/teamfight-5v5.mbr`, warm-up 330 ticks) at 1920×1080,
vsync off, validation off, no pipeline cache, 3,000 frames per run with the frame log covering
the last 2,940; **three rounds, interleaved** N0.1 / branch for each tier. The branch runs on
its default path, which on this GPU is **tiered**: the capability gate enables the extra
features it admits. Each cell is the median of the three rounds; the range is their spread.
Clocks cannot be locked without root.

| Tier | Build | GPU total p50, ms (range) | GPU total p95, ms | scene p50, ms | VRAM peak | `locked_uploads` / `locked_pipelines` |
|---|---|---|---|---|---|---|
| Low | N0.1 | 0.210 (0.209–0.210) | 0.223 | 0.134 | 84.3 MB | 0 / 0 |
| Low | branch (`path=tiered`) | 0.211 (0.210–0.211) | 0.222 | 0.135 | 84.3 MB | 0 / 0 |
| High | N0.1 | 0.503 (0.502–0.506) | 0.517 | 0.164 | 136.0 MB | 0 / 0 |
| High | branch (`path=tiered`) | 0.504 (0.503–0.505) | 0.516 | 0.164 | 136.0 MB | 0 / 0 |
| Ultra | N0.1 | 0.660 (0.659–0.662) | 0.710 | 0.252 | 292.9 MB | 0 / 0 |
| Ultra | branch (`path=tiered`) | 0.661 (0.661–0.663) | 0.720 | 0.252 | 292.9 MB | 0 / 0 |

The branch is within 0.001 ms of N0.1 at p50 on every tier, inside the rounds' spread. Ultra's
GPU p95 is bimodal on this GPU for both builds (rounds 0.719/0.678/0.710 ms on N0.1,
0.720/0.720/0.704 ms on the branch), so its 0.01 ms gap is noise. VRAM and the upload and
pipeline counters are identical.

Enabling features mostly changes CPU-side driver work, which GPU time cannot show, so the same
runs' CPU columns (medians of the three rounds, ms):

| Tier | Build | Frame p50 | Frame p95 | CPU before the render call, p95 | Inside the render call, p95 |
|---|---|---|---|---|---|
| Low | N0.1 | 4.181 | 4.688 | 0.036 | 0.140 |
| Low | branch | 4.182 | 4.759 | 0.034 | 0.137 |
| High | N0.1 | 4.181 | 4.689 | 0.034 | 0.193 |
| High | branch | 4.182 | 4.674 | 0.034 | 0.191 |
| Ultra | N0.1 | 4.181 | 4.647 | 0.034 | 0.201 |
| Ultra | branch | 4.181 | 4.694 | 0.033 | 0.188 |

The frame time is the compositor's pacing (vsync off, 1920×1080 on Wayland), not the
renderer's cost. The render call (fence wait, acquire, recording, submit, present) is no
slower on the tiered device.

## After the review fixes

`96d7487` (ADR-0020 amendment 2) changes:
- which bits the probe records: `rayTracingInvocationReorder`, `presentId` and `antiLag`;
- which feature structures Tiered device creation chains with those extensions;
- that only `MOBA_RENDER_PATH=tiered` admits a CPU device.

Compat device creation is unchanged. On lavapipe the re-recorded fixture is identical (it has
none of the three extensions), so lavapipe's tiered device is created exactly as in the golden
runs above. The RTX gains `presentId` and SER and the Radeon 610M gains `presentId`.

Re-checked on the RTX with a debug build at the fix commit, with `MOBA_VULKAN_VALIDATION=required`:
- `MOBA_RENDER_PATH=tiered`: 90 frames, exit 0, no validation message,
  `path=tiered max_tier=overdrive missing=none admission=admitted`;
- lavapipe with `MOBA_RENDER_PATH=tiered`: 30 frames, clean;
- lavapipe with only `render_path = "tiered"` in the settings file: start-up fails with
  `llvmpipe …: not admitted to the tiered path (cpu-device; missing nothing)`, as intended.


## Radeon 610M (RADV): CPU cost of the tiered features

The perf review asked whether enabling the tiered features (buffer device address,
descriptor indexing, ray query, acceleration structures) makes RADV's submit path slower,
since RADV tracks buffer objects per submission. Same replay and settings as the RTX A/B,
N0.1 (`main`) against the branch (`path=tiered`), three interleaved rounds per tier; each
cell is the median.

| Tier | Build | Frame p50, ms | Render call p95, ms | CPU p95, ms | GPU total p95, ms |
|---|---|---|---|---|---|
| Low | N0.1 | 8.023 | 9.185 | 0.059 | 8.204 |
| Low | branch | 8.023 | 9.235 | 0.057 | 8.201 |
| High | N0.1 | 20.288 | 22.111 | 0.082 | 20.594 |
| High | branch | 20.274 | 22.003 | 0.080 | 20.594 |

Within noise on every column. These runs shared the CPU with lavapipe golden jobs, so the
absolute frame times are pessimistic; both builds ran under the same load, interleaved.

## Pipeline cache: an N0.1 cache stays warm

The tiered device enables more features, which a driver may fold into its pipeline cache
key. On the RTX, a cache written by N0.1 (`MOBA_PIPELINE_CACHE`, 30 frames, validation off):

| Run | Pipelines built in | Cache |
|---|---|---|
| N0.1, first run (writes) | 69.3 ms | cold |
| N0.1, second run | 3.7 ms | warm |
| branch, `path=tiered` | 3.3 ms | warm |
| branch, again | 3.4 ms | warm |

The upgrade does not cost players a cold pipeline build.
