# N0.1: the `ash` bump changes no image and no GPU time

Captured 2026-11-01 on the development workstation, not a CI runner, for
[ADR-0020](../adr/0020-next-gen-foundations.md) §1 and verification gate 1.

- **Branch and baseline:** branch `feat/n0-ash-pin` (`ash` master `8844f6b`, headers 1.4.352;
  `gpu-allocator` `ash-next` `6a68a5b4`) against `main` at `2b44b64` (`ash` 0.38.0+1.3.281,
  `gpu-allocator` 0.28.0). The two builds differ only in the bindings and their call sites.
- **Hardware and software:**
  - GPU: NVIDIA GeForce RTX 5070 Ti (16 GB), driver 615.78.08.
  - Software Vulkan: lavapipe from `/home/jaket/Projects/MOBA/.tools`, with the bundled
    Khronos validation layer.
  - CPU: AMD Ryzen 7 9800X3D. OS: Linux 7.3.0-5-cachyos, Wayland.
  - Build: release.

## Goldens: byte-identical (lavapipe)

Seven of the golden workflow's scenes were captured once on `main` and twice on the branch.
Every capture ran at 1280×720 on lavapipe with `MOBA_VULKAN_VALIDATION=required`, which
includes synchronization validation. The full PPMs were compared with `cmp`, not the 64×36
fingerprints. Every run exited cleanly with no validation message.

| Scene (golden workflow step) | Result | SHA-256 (first 16) |
|---|---|---|
| default (frame 90) | main = branch run 1 = branch run 2 | `5a476005e52ac38b` |
| networked, skinned heroes (tick 120) | identical | `e7e6198ad2260eb3` |
| VAT minions, 64 (frame 90) | identical | `09b8ef828a3eed7b` |
| Ultra, 64 minions (frame 90) | identical | `4b8beb4c337bd8be` |
| Low at render scale 0.75 (frame 90) | identical | `25aa95a501ac7ad1` |
| Ember map, camera 24 (tick 120) | identical | `342a3fa53facf1df` |
| teamfight decals (replay, frame 30) | identical | `8a07fb957cb153b2` |

CI's `renderer-gate` runs the complete fingerprint set on this PR.

## RTX 5070 Ti timings: within noise

The run used the teamfight replay (`tests/replays/teamfight-5v5.mbr`, warm-up 330 ticks)
with these settings:
- 1920×1080, vsync off, validation off, no pipeline cache;
- 3,000 frames per run, with the frame log covering the last 2,940;
- **three rounds, interleaved** main / branch for each tier.

Each cell is the median of the three rounds; the range is the rounds' spread. Clocks cannot be
locked without root. `nvidia-smi -q -d CLOCK` reported a memory clock of 7,001 MHz before the
set and 13,801 MHz just after it. No other client used the GPU: 1.7 of 16.3 GB was in use
before the set, by the desktop.

```bash
MOBA_QUALITY=<low|high|ultra> MOBA_WINDOW_SIZE=1920x1080 MOBA_SMOKE_FRAMES=3000 MOBA_VSYNC=0 \
MOBA_PIPELINE_CACHE=off MOBA_VULKAN_VALIDATION=off \
MOBA_REPLAY=tests/replays/teamfight-5v5.mbr MOBA_WARMUP_TICKS=330 MOBA_FRAME_LOG=<log> \
  ./target/release/moba-client
```

| Tier | Build | GPU total p50, ms (range) | GPU total p95, ms | scene p50, ms | VRAM | `locked_uploads` / `locked_pipelines` |
|---|---|---|---|---|---|---|
| Low | main | 0.207 (0.206–0.209) | 0.222 | 0.133 | 82.2 MB | 0 / 0 |
| Low | branch | 0.207 (0.207–0.207) | 0.224 | 0.133 | 82.2 MB | 0 / 0 |
| High | main | 0.499 (0.499–0.500) | 0.519 | 0.163 | 134.5 MB | 0 / 0 |
| High | branch | 0.498 (0.498–0.501) | 0.519 | 0.163 | 134.5 MB | 0 / 0 |
| Ultra | main | 0.661 (0.657–0.661) | 0.671 | 0.252 | 286.7 MB | 0 / 0 |
| Ultra | branch | 0.660 (0.660–0.661) | 0.670 | 0.252 | 286.7 MB | 0 / 0 |

The branch is within 0.002 ms of `main` on every tier, at p50 and at p95. At p50 that is
inside the rounds' own spread, shown in the table. The p95 values are medians of the rounds,
and their spread is not tabulated. VRAM is identical, and so are the upload and pipeline counters.
