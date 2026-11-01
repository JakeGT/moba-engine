# N0.2: the capability gate changes no image and no GPU time

Captured 2026-11-01 on the development workstation, not a CI runner, for
[ADR-0020](../adr/0020-next-gen-foundations.md) §2 and verification gate 2.

- **Branch and baseline:** branch `feat/n0-capability-gate` against N0.1
  (`feat/n0-ash-pin`, `5c0d245`).
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

The golden workflow's scenes were captured once on N0.1 and twice on the branch. Every
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
p95 is 0.01 ms higher; the p95 values are medians of the rounds and their spread is not
tabulated. VRAM and the upload and pipeline counters are identical.
