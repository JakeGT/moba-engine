# N0.3: tier presets and budgets; dynamic resolution holds a forced budget

Captured 2026-11-01 on the development workstation, not a CI runner, for
[ADR-0020](../adr/0020-next-gen-foundations.md) §5 and verification gate 4.

- **Branch and baseline:** branch `feat/n0-tiers-budgets` against N0.2
  (`feat/n0-capability-gate`, `4307b8b`).
- **Hardware and software:**
  - GPU: NVIDIA GeForce RTX 5070 Ti (16 GB), driver 615.78.08.
  - Software Vulkan: lavapipe from `/home/jaket/Projects/MOBA/.tools`, with the bundled
    Khronos validation layer.
  - CPU: AMD Ryzen 7 9800X3D. OS: Linux 7.3.0-5-cachyos, Wayland.
  - Build: release.

## Gate 4: dynamic resolution holds a forced budget (RTX 5070 Ti)

`scripts/dynres-check.sh` plays the teamfight replay (`tests/replays/teamfight-5v5.mbr`,
warm-up 330 ticks, 3,000 frames, the frame log covering the last 2,940) at 1920×1080, vsync
off, twice per tier:
1. dynamic resolution off, at the preset's ceiling (full scale; [Amendment 2](../adr/0020-next-gen-foundations.md#amendment-2-2026-11-01-n03-dynamic-resolution-before-the-upscalers)): the natural GPU p95;
2. dynamic resolution forced to 0.75 × that p95 (`MOBA_DYNRES=<ms>`).

It passes when the scale stepped down, changed at most twice over the second half of the
frames, and the GPU p95 is at or under the target. The new frame-log columns
`scale_first`, `scale_final`, `scale_min` and `scale_changes_late` carry the scale; the first
column is the first frame after the frame log's 60-frame warm-up, by which point the
controller has already taken a step.

| Path | Tier (`config_tier`) | Natural p95, ms | Target, ms | Forced p95, ms | Scale first → final | Late changes | Result |
|---|---|---|---|---|---|---|---|
| tiered | low (high) | 0.513 | 0.385 | 0.364 | 0.750 → 0.688 | 1 | ok |
| tiered | medium (ultra) | 0.675 | 0.506 | 0.470 | 0.750 → 0.688 | 0 | ok |
| tiered | high (ultra) | 0.714 | 0.535 | 0.499 | 0.781 → 0.719 | 0 | ok |
| tiered | ultra (ultra) | 0.716 | 0.537 | 0.498 | 0.781 → 0.688 | 1 | ok |
| tiered | overdrive (ultra) | 0.718 | 0.538 | 0.499 | 0.781 → 0.719 | 0 | ok |
| compat | ultra (ultra) | 0.677 | 0.508 | 0.477 | 0.750 → 0.688 | 0 | ok |

Every forced run had `locked_uploads=0` and `locked_pipelines=0`.

- **The first attempt failed, and found a real limit.** `PostConfig::sanitized` clamped the
  target to at least 1 ms, and this GPU renders the replay in 0.5–0.7 ms, so no forced target
  could take effect: the scale never moved. The clamp is now 0.1 ms (Amendment 2).
- **Tiered Low runs today's High configuration** (§5e), which is why its natural cost is
  above Compat Low's 0.22 ms ([perf/0025](0025-n0-ash-pin.md)). Medium and up run today's
  Ultra.
- These targets are far below the real budgets (6–15 ms). At the real budgets the
  controller stays at full scale on this GPU, which is the point of holding the ceiling
  until N1.

```bash
scripts/dynres-check.sh <out>                       # tiered Ultra
TIER=low scripts/dynres-check.sh <out>              # … medium, high, overdrive
RENDER_PATH=compat scripts/dynres-check.sh <out>
```

## Compat timings: within noise (RTX 5070 Ti)

Compat must not change in N0, so the A/B pins `MOBA_RENDER_PATH=compat` on both builds.
Teamfight replay, 1920×1080, vsync off, validation off, no pipeline cache, 3,000 frames per
run (the frame log covers the last 2,940); **three rounds, interleaved** N0.2 / branch for
each tier. Each cell is the median of the three rounds; the range is their spread.

| Tier | Build | GPU total p50, ms (range) | GPU total p95, ms | scene p50, ms | VRAM peak | `locked_uploads` / `locked_pipelines` |
|---|---|---|---|---|---|---|
| Low | N0.2 | 0.210 (0.209–0.211) | 0.223 | 0.134 | 84.3 MB | 0 / 0 |
| Low | branch | 0.210 (0.210–0.212) | 0.222 | 0.134 | 84.3 MB | 0 / 0 |
| High | N0.2 | 0.507 (0.506–0.508) | 0.520 | 0.165 | 136.0 MB | 0 / 0 |
| High | branch | 0.507 (0.499–0.507) | 0.519 | 0.165 | 136.0 MB | 0 / 0 |
| Ultra | N0.2 | 0.661 (0.658–0.662) | 0.673 | 0.252 | 292.9 MB | 0 / 0 |
| Ultra | branch | 0.659 (0.658–0.660) | 0.701 | 0.252 | 292.9 MB | 0 / 0 |

p50 is equal on every tier. Ultra's p95 is bimodal on this GPU for both builds: the rounds
gave 0.673, 0.722 and 0.672 ms on N0.2, and 0.713, 0.678 and 0.701 ms on the branch (perf/0026
saw the same 0.68/0.72 split), so the medians' 0.03 ms gap is noise. VRAM and the counters
are identical. The Compat frame log now also names `config_tier`, which equals the tier.
