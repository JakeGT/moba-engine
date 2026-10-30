# Decals, foliage and water: cost, validation and image evidence (R3b)

Captured 2026-10-30 on the development workstation, not a CI runner, for
[ADR-0017](../adr/0017-decals-foliage-water.md).

- GPU: NVIDIA GeForce RTX 5070 Ti (16 GB), driver 615.78.08 (`nvidia_icd.json`); software:
  lavapipe from `/home/jaket/Projects/MOBA/.tools` with the bundled Khronos validation layer
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-2-cachyos, Wayland
- Builds, both release: **before** = `main` at `a0d2e10` (R2b merged over R6: clustered
  effect lights, outlines, baked AO and contact shadows, over R6's post chain, MSAA on Ultra
  and GTAO with the prepass on High and up, over R3a's terrain); **after** = this branch
  rebased on it. A first full set against `df2dea6` (R6 only, before R2b merged) gave the
  same deltas within 0.01 ms on Ultra (teamfight +0.101, view +0.091, overview +0.056 ms).
- **Re-run on `main` at `197857b`** (later the same day, no other GPU client): the per-feature
  costs ([below](#where-the-cost-goes-re-measured-on-main-rtx-5070-ti)), the RTX validation
  and `terrain-decal-check.sh`.
- The GPU was shared with another agent's builds and captures, so every configuration ran
  **three rounds**, interleaved before/after, and the tables give the **median** of the three
  (the `p50 range` column shows all three rounds' GPU totals).

## The evidence scenes

1. **Teamfight** (`replay`): R6's §7 scene, R5's scripted 5v5 teamfight
   `tests/replays/teamfight-5v5.mbr` from tick 330 (it is on the river's ford: every
   ability's telegraph decal, the scorch and splats, water and foliage are in view).
2. **View** (`view`): the local preview from 60 m (`MOBA_PREVIEW_CAMERA=-0.7,0.3,60`, a
   camera both builds understand): the base, lanes, the meadow and both river reaches.
3. **Overview** (`overview`): the networked whole-map camera (`MOBA_CAMERA=overview`): every
   foliage chunk within the draw distance, both reaches, the fog-of-war texture.
4. **River** (`river`, after only): the river golden's close-up
   (`MOBA_PREVIEW_CAMERA=0.9,0.3,14,4,-62`): the water filling the screen.

3,000 frames each at 1920×1080, vsync off, validation off, no pipeline cache:

```bash
MOBA_QUALITY=<tier> MOBA_WINDOW_SIZE=1920x1080 MOBA_REPLAY=tests/replays/teamfight-5v5.mbr \
MOBA_WARMUP_TICKS=330 MOBA_SMOKE_FRAMES=3000 MOBA_VSYNC=0 MOBA_PIPELINE_CACHE=off \
MOBA_FRAME_LOG=/tmp/r3b.log [MOBA_DECALS=off MOBA_FOLIAGE=off MOBA_WATER=…] cargo run --release -p moba-client
```

The frame interval stays at the client's 240 Hz cap (4.181–4.186 ms p50) in every run; the
GPU columns come from the renderer's timestamp queries.

## The GPU's power state decides the small numbers

At 0.2–1.0 ms of GPU work in a 4.18 ms frame the RTX is mostly idle, and its driver picks
the memory clock from the load. Sampling `nvidia-smi` (read-only, every 250 ms) through two
more interleaved rounds of the teamfight showed:

| Run | Dominant P-state during the run (memory clock) |
|---|---|
| Low before / after | P8 (405 MHz) / P8 (405 MHz) |
| Medium before / after | P8 / P8 |
| **High before / after** | **P8 (405 MHz) / P3–P0 (7–14 GHz)** |
| Ultra before / after | P3 (7 GHz) / P3 (7 GHz) |
| High after, `MOBA_WATER=simple` | P5–P0, varying by round |

The full water's `water_copy` (a transfer) is enough to lift High into a faster memory
clock, so **High "after" is faster than High "before"** in the table below, and High with
the full water off is slower than with it on. Those High numbers measure the power state,
not R3b. The passes R3b does not touch show it (High bloom 0.052 → 0.041 ms, composite
0.024 → 0.020 ms, the prepass 0.043 → 0.021 ms, GTAO 0.052 → 0.047 ms in the teamfight).
**Ultra ran in the same state in both builds**, and Low and Medium mostly did, so their
deltas are R3b's cost. Clocks cannot be locked without
root, and the client caps at 240 FPS, so this GPU cannot be loaded into one state for every
tier.

## Per-tier GPU time, before and after (RTX 5070 Ti, 1080p)

Measured with the review fixes in (the second section below has what they cost). GPU ms at
p50 per pass (median of three rounds), then the GPU total (sum of the passes) at
p50 / p95 and the range of the three rounds' p50 totals. `vfx` sums R5's three passes, `ao`
is GTAO + its blur and `bloom` every down- and up-sample. R2b's contact-shadow pass runs on
High and up in both builds and is in the total. Each tier runs its defaults (Low and Medium:
simple water in the scene pass; High and Ultra: the `water_copy` + `water` passes).

| run | shadow | prepass | ao | scene | water_copy | water | vfx | bloom | composite | fxaa | ui | GPU total p50 / p95 | p50 range |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| replay-low before | 0.005 | — | — | 0.100 | — | — | 0.011 | — | 0.018 | 0.017 | 0.004 | **0.173** / 0.182 | 0.172–0.315 |
| replay-low after | 0.005 | — | — | 0.136 | — | — | 0.011 | — | 0.018 | 0.018 | 0.005 | **0.211** / 0.222 | 0.208–0.345 |
| replay-medium before | 0.006 | — | — | 0.129 | — | — | 0.026 | 0.041 | 0.020 | 0.022 | 0.004 | **0.258** / 0.266 | 0.257–0.353 |
| replay-medium after | 0.006 | — | — | 0.170 | — | — | 0.026 | 0.041 | 0.020 | 0.023 | 0.004 | **0.300** / 0.307 | 0.299–0.426 |
| replay-high before | 0.006 | 0.021 | 0.047 | 0.131 | — | — | 0.024 | 0.041 | 0.020 | 0.021 | 0.004 | **0.412** / 0.419 | 0.410–0.607 |
| replay-high after | 0.006 | 0.021 | 0.047 | 0.166 | 0.019 | 0.025 | 0.023 | 0.041 | 0.020 | 0.024 | 0.004 | **0.493** / 0.500 | 0.491–0.509 |
| replay-ultra before | 0.011 | 0.065 | 0.059 | 0.196 | — | — | 0.030 | 0.041 | 0.022 | — | 0.004 | **0.564** / 0.572 | 0.560–0.620 |
| replay-ultra after | 0.011 | 0.066 | 0.059 | 0.260 | 0.019 | 0.027 | 0.029 | 0.041 | 0.022 | — | 0.004 | **0.673** / 0.745 | 0.673–0.762 |
| view-low before | 0.004 | — | — | 0.086 | — | — | 0.007 | — | 0.017 | 0.017 | 0.004 | **0.184** / 0.233 | 0.159–0.188 |
| view-low after | 0.005 | — | — | 0.110 | — | — | 0.007 | — | 0.017 | 0.017 | 0.004 | **0.201** / 0.256 | 0.183–0.252 |
| view-high before | 0.007 | 0.050 | 0.044 | 0.281 | — | — | 0.042 | 0.046 | 0.036 | 0.021 | 0.013 | **0.668** / 1.073 | 0.414–0.677 |
| view-high after | 0.006 | 0.027 | 0.044 | 0.184 | 0.019 | 0.022 | 0.023 | 0.041 | 0.020 | 0.022 | 0.005 | **0.501** / 0.518 | 0.485–0.505 |
| view-ultra before | 0.012 | 0.086 | 0.057 | 0.315 | — | — | 0.033 | 0.041 | 0.022 | — | 0.005 | **0.687** / 0.703 | 0.632–0.688 |
| view-ultra after | 0.012 | 0.097 | 0.057 | 0.393 | 0.021 | 0.025 | 0.031 | 0.042 | 0.022 | — | 0.005 | **0.820** / 0.843 | 0.731–0.821 |
| overview-high before | 0.006 | 0.040 | 0.041 | 0.169 | — | — | 0.025 | 0.041 | 0.020 | 0.019 | 0.008 | **0.425** / 0.872 | 0.418–0.775 |
| overview-high after | 0.006 | 0.040 | 0.040 | 0.206 | 0.019 | 0.017 | 0.021 | 0.041 | 0.020 | 0.020 | 0.010 | **0.504** / 0.532 | 0.489–0.506 |
| overview-ultra before | 0.012 | 0.118 | 0.054 | 0.342 | — | — | 0.032 | 0.042 | 0.022 | — | 0.010 | **0.699** / 0.722 | 0.638–0.701 |
| overview-ultra after | 0.012 | 0.126 | 0.054 | 0.440 | 0.022 | 0.018 | 0.031 | 0.042 | 0.022 | — | 0.010 | **0.844** / 0.875 | 0.756–0.848 |
| river-high after | 0.005 | 0.022 | 0.077 | 0.164 | 0.019 | 0.068 | 0.023 | 0.041 | 0.020 | 0.022 | 0.004 | **0.556** / 0.566 | 0.549–0.563 |
| river-ultra after | 0.010 | 0.087 | 0.104 | 0.360 | 0.021 | 0.083 | 0.031 | 0.042 | 0.022 | — | 0.005 | **0.896** / 0.903 | 0.815–0.898 |

**Against the §7 budgets** (Low ≤ 14 ms, Medium/High ≤ 6.0 ms, Ultra ≤ 7.5 ms):

| Tier | Budget (GPU) | Heaviest scene after R3b, p50 / p95 | Headroom at p95 |
|---|---|---|---|
| Low | 14.0 ms | teamfight 0.211 / 0.222 ms | 98 % |
| Medium | 6.0 ms | teamfight 0.300 / 0.307 ms | 95 % |
| High | 6.0 ms | river close-up 0.556 / 0.566 ms; overview 0.504 / 0.532 ms | 91 % |
| Ultra | 7.5 ms | river close-up 0.896 / 0.903 ms | 88 % |

The frame interval is the 240 Hz cap in every run, so on this GPU every tier has an order of
magnitude to spare. The budgets are written for the §7 reference machines, which this note
does not cover (see the end).

## Where the cost goes, re-measured on `main` (RTX 5070 Ti)

The per-feature breakdown was first measured on the PR's first version (before the review
fixes and before `terrain_nodecals.frag`, so `decals-off` still ran the decal code over an
empty buffer: teamfight decals 0.007 ms, foliage 0.030, full against simple water 0.029,
simple water against none 0.025 ms). It was re-run on `main` at `197857b` (R3b with R8's
culling and LODs and the distortion pass merged), release, 1920×1080, vsync off, validation
off, no pipeline cache, 3,000 frames, **three rounds interleaved over every row** (17:43–17:58
UTC), the median of the three in each cell (`p50 range`: the rounds' GPU totals).

- **The GPU had no other client.** Before and after the set: 1.5–2.3 GB of 16.3 GB used, all
  of it the desktop (`kwin_wayland`, `plasmashell`, a browser and, from 17:42, the session's
  screen locker `kscreenlocker_greet`), utilisation 0–3 %. The frame interval stayed at the
  240 Hz cap (4.18 ms p50) in every run, locker or not.
- **Clocks.** They cannot be locked without root (`nvidia-smi -lgc`); `nvidia-smi -q -d
  CLOCK` before the set: graphics 1762 MHz, memory 7001 MHz (P3; maxima 3090 / 14001 MHz). A
  1,500-frame Ultra warm-up ran first, and a sampler logged the P-state and clocks every
  250 ms during each run: the last column is each round's dominant memory clock. The
  driver held 7001 MHz for every High and Ultra row in rounds 2 and 3, and in all three
  rounds from `replay-ultra-foliage-off` on (round 1 of the first five rows ran at 14001 MHz
  after the warm-up), so **every Ultra median below is from the same memory clock** and
  their differences are the features. Low and Medium dropped to 405 MHz after
  round 1, which is why their p95 and ranges are wide.

| run (`main` 197857b) | shadow | prepass | ao | contact | scene | water_copy | water | vfx | bloom | composite | fxaa | ui | GPU total p50 / p95 | p50 range | mem clock per round (MHz) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| replay-low | 0.004 | — | — | — | 0.131 | — | — | 0.010 | — | 0.017 | 0.017 | 0.003 | **0.224** / 0.372 | 0.200–0.230 | 14001 / 405 / 405 |
| replay-medium | 0.006 | — | — | — | 0.215 | — | — | 0.028 | 0.040 | 0.020 | 0.023 | 0.004 | **0.367** / 0.740 | 0.288–0.415 | 14001 / 405 / 405 |
| replay-high | 0.006 | 0.018 | 0.047 | 0.052 | 0.158 | 0.019 | 0.024 | 0.024 | 0.040 | 0.020 | 0.024 | 0.004 | **0.481** / 0.494 | 0.481–0.490 | 14001 / 7001 / 7001 |
| replay-ultra | 0.011 | 0.062 | 0.059 | 0.091 | 0.275 | 0.022 | 0.030 | 0.037 | 0.042 | 0.022 | — | 0.005 | **0.712** / 0.724 | 0.658–0.716 | 14001 / 7001 / 7001 |
| replay-ultra-decals-off | 0.011 | 0.062 | 0.058 | 0.091 | 0.253 | 0.022 | 0.029 | 0.036 | 0.042 | 0.022 | — | 0.005 | **0.692** / 0.703 | 0.637–0.695 | 14001 / 7001 / 7001 |
| replay-ultra-foliage-off | 0.011 | 0.062 | 0.059 | 0.091 | 0.240 | 0.021 | 0.030 | 0.035 | 0.042 | 0.022 | — | 0.005 | **0.678** / 0.689 | 0.671–0.680 | 7001 / 7001 / 7001 |
| replay-ultra-water-simple | 0.011 | 0.060 | 0.059 | 0.091 | 0.301 | — | — | 0.038 | 0.049 | 0.022 | — | 0.005 | **0.691** / 0.703 | 0.683–0.695 | 7001 / 7001 / 7001 |
| replay-ultra-water-off | 0.011 | 0.060 | 0.059 | 0.091 | 0.275 | — | — | 0.038 | 0.049 | 0.022 | — | 0.005 | **0.668** / 0.679 | 0.660–0.668 | 7001 / 7001 / 7001 |
| replay-ultra-all-off | 0.011 | 0.060 | 0.059 | 0.091 | 0.223 | — | — | 0.038 | 0.048 | 0.022 | — | 0.005 | **0.614** / 0.624 | 0.611–0.614 | 7001 / 7001 / 7001 |
| replay-ultra-vfx8 | 0.011 | 0.062 | 0.059 | 0.091 | 0.282 | 0.023 | 0.030 | 0.040 | 0.042 | 0.022 | — | 0.005 | **0.717** / 0.731 | 0.712–0.721 | 7001 / 7001 / 7001 |
| view-low | 0.003 | — | — | — | 0.100 | — | — | 0.007 | — | 0.017 | 0.017 | 0.004 | **0.181** / 0.238 | 0.177–0.190 | 405 / 405 / 405 |
| view-high | 0.005 | 0.024 | 0.044 | 0.036 | 0.182 | 0.018 | 0.021 | 0.022 | 0.041 | 0.020 | 0.022 | 0.005 | **0.497** / 0.515 | 0.485–0.498 | 7001 / 7001 / 7001 |
| view-ultra | 0.011 | 0.083 | 0.057 | 0.061 | 0.372 | 0.020 | 0.025 | 0.032 | 0.042 | 0.022 | — | 0.005 | **0.783** / 0.806 | 0.783–0.786 | 7001 / 7001 / 7001 |
| overview-high | 0.006 | 0.038 | 0.040 | 0.010 | 0.201 | 0.018 | 0.013 | 0.022 | 0.041 | 0.020 | 0.020 | 0.009 | **0.492** / 0.512 | 0.491–0.497 | 7001 / 7001 / 7001 |
| overview-ultra | 0.010 | 0.125 | 0.054 | 0.011 | 0.415 | 0.021 | 0.017 | 0.031 | 0.042 | 0.022 | — | 0.010 | **0.816** / 0.841 | 0.815–0.820 | 7001 / 7001 / 7001 |
| overview-ultra-decals-off | 0.010 | 0.125 | 0.055 | 0.010 | 0.398 | 0.021 | 0.017 | 0.031 | 0.042 | 0.022 | — | 0.010 | **0.800** / 0.820 | 0.799–0.802 | 7001 / 7001 / 7001 |
| overview-ultra-foliage-off | 0.010 | 0.126 | 0.054 | 0.011 | 0.360 | 0.021 | 0.018 | 0.031 | 0.042 | 0.022 | — | 0.010 | **0.762** / 0.786 | 0.760–0.765 | 7001 / 7001 / 7001 |
| overview-ultra-water-simple | 0.011 | 0.123 | 0.055 | 0.010 | 0.427 | — | — | 0.033 | 0.046 | 0.022 | — | 0.010 | **0.793** / 0.817 | 0.792–0.800 | 7001 / 7001 / 7001 |
| overview-ultra-water-off | 0.010 | 0.123 | 0.055 | 0.011 | 0.415 | — | — | 0.033 | 0.047 | 0.022 | — | 0.010 | **0.783** / 0.804 | 0.777–0.788 | 7001 / 7001 / 7001 |
| overview-ultra-all-off | 0.011 | 0.122 | 0.055 | 0.011 | 0.346 | — | — | 0.033 | 0.047 | 0.022 | — | 0.010 | **0.715** / 0.732 | 0.709–0.715 | 7001 / 7001 / 7001 |
| overview-ultra-vfx8 | 0.010 | 0.126 | 0.055 | 0.010 | 0.414 | 0.021 | 0.017 | 0.031 | 0.042 | 0.022 | — | 0.010 | **0.817** / 0.841 | 0.816–0.821 | 7001 / 7001 / 7001 |
| river-high | 0.005 | 0.021 | 0.077 | 0.045 | 0.157 | 0.018 | 0.070 | 0.022 | 0.041 | 0.020 | 0.022 | 0.004 | **0.549** / 0.568 | 0.544–0.558 | 7001 / 7001 / 7001 |
| river-ultra | 0.009 | 0.073 | 0.104 | 0.077 | 0.346 | 0.021 | 0.081 | 0.032 | 0.042 | 0.022 | — | 0.005 | **0.865** / 0.872 | 0.865–0.873 | 7001 / 7001 / 7001 |

`contact` is R2b's contact-shadow pass; `vfx` sums R5's three passes. Distortion is idle in
all of these scenes (no hazy effect in view; [perf/0023](0023-distortion.md)).

- **R3b's whole cost on Ultra** (everything on against `MOBA_DECALS=off MOBA_FOLIAGE=off
  MOBA_WATER=off`): teamfight **+0.098 ms** (0.614 → 0.712), overview **+0.101 ms** (0.715 →
  0.816).
- **Decals** (now compiled out with `MOBA_DECALS=off`): 0.020 ms in the teamfight (every
  telegraph, scorch and splat of a 5v5 fight on screen, scene pass 0.253 → 0.275 ms) and
  0.016 ms on the overview, where no decal is in view: that is the decal code's fixed cost,
  scene 0.398 → 0.415 ms (1.04×, the same ratio `terrain-decal-check.sh` measured below).
- **Foliage**: 0.034 ms in the teamfight, 0.054 ms on the overview (it draws the grass and
  reeds around the ground focus, see the review fixes below).
- **Water**: full against simple 0.021 / 0.023 ms (teamfight / overview), simple against
  none 0.023 / 0.010 ms.
- **`MOBA_VFX_SCALE=8`** adds 0.005 ms in the teamfight and 0.001 ms on the overview (the
  decal budget is fixed at 128).
- **The river close-up**, the worst R3b case (water fills the screen, GTAO, contact shadows
  and refraction over it): 0.549 / 0.568 ms on High and **0.865 / 0.872 ms** on Ultra
  (p50 / p95), against 0.556 and 0.896 ms in the table above.
- **Budgets.** The heaviest row per tier against §7 at p95: Low 0.372 ms (teamfight, 97 %
  headroom of 14 ms), Medium 0.740 ms (teamfight, its 405 MHz rounds; 88 % of 6.0 ms), High
  0.568 ms (river; 91 %), Ultra 0.872 ms (river; 88 % of 7.5 ms). Every tier meets its
  budget with an order of magnitude to spare on this GPU.

### Reading the before/after table

**Ultra** is the clean comparison in the per-tier table (the same memory clock in every round
of both builds):

- With the review fixes in, the teamfight costs **+0.109 ms** (0.564 → 0.673), the 60 m
  view **+0.133 ms** and the whole-map overview **+0.145 ms**. The full water's two passes
  are 0.040–0.046 ms of that; the rest is the scene pass (+0.064 / +0.078 / +0.098 ms:
  foliage, the water's carved bed, the decals and the depth re-resolve) and the prepass (up
  to +0.011 ms, the river bed's extra relief). The first version measured +0.106 / +0.091 /
  +0.056 ms; the difference is the review fixes below.

**Low and Medium** ran mostly at the lowest memory clock, but a round now and then caught a
higher one, so their totals wander between rounds (the `p50 range` column). The scene pass
holds the R3b cost there: **+0.042 ms** on Low and **+0.066 ms** on Medium in the teamfight,
**+0.018 ms** on Low in the view.

**High** is the power-state artefact above: after is faster than before in the view, by the
memory clock and not by R3b (the teamfight and overview ran in one state this time: +0.081
and +0.079 ms, about half of it the full water's two passes).

### What the review fixes cost

With each fix the Ultra overview's scene pass went from 0.358 ms (first version) to 0.440 ms
(`main` 0.342 ms), measured in the same memory-clock state (bloom 0.042 ms in both):

- **Foliage distance from the ground focus**: 0.051 ms (the overview with `MOBA_FOLIAGE=off`
  is 0.389 ms, the same state). The overview camera sits 46 m up, so counting the 60 m
  foliage distance from the eye alone culled almost every chunk of the map it looks at; it
  now draws the grass and reeds the view is centred on, as the teamfight camera does. In the
  first version `MOBA_FOLIAGE=off` made no difference on the overview (0.360 ms).
- **The MSAA depth re-resolve** (the scene re-resolves its 4× depth into the single-sample
  depth so water and particles see foliage): about 0.03 ms at 1080p, the rest of the
  foliage-off overview's step from 0.360 to 0.389 ms.
- The ring rework, the role-split water glow and the view cull of the decal grid are inside
  the noise (`MOBA_DECALS=off` on the overview: 0.443 ms against 0.440).

The rows with features off in this second set ran partly in other memory-clock states, so
they were not quoted as deltas; the re-run on `main` above has every Ultra row in one state.

### A register step in the terrain shader

The first version of the decal shader copied each 64-byte record into a local
`DecalRecord` before evaluating its shape. On the RTX 5070 Ti that alone doubled the
terrain's cost on every tier that runs the full splat path, **whether or not any decal was
in view** and with `MOBA_DECALS=off` (the buffer then holds zero decals): the Ultra
overview's scene pass went from 0.30 to 0.62 ms, High's from 0.17 to 0.32 ms. Removing the
decal calls from the terrain shader restored it; a flat board (`MOBA_TERRAIN=flat`) showed
no difference, and Low (no triplanar or layer normals) only +0.02 ms. Variants placed the
cliff: a disc-only shape, or a disc and a ring selected without branches, were fast; adding
any third shape, branchy or branchless, `[[dont_unroll]]` or `[[dont_flatten]]`, was slow.
That points to register allocation: the inlined decal loop kept the whole record live
across the shape code, and the terrain shader crossed an occupancy step. (Measured on the
R6 base, before R2b merged.)

The shipped shader reads each field from the buffer where it is used (`MOBA_DECAL(index)`
in `decals.glsl`), which brought the overview's Ultra scene pass back to 0.351 ms (`main`
0.335 ms) and High's to 0.17 ms, with byte-identical lavapipe output. After R2b's lights
joined the same shader it still holds: 0.358 ms against `main`'s 0.345 ms. ADR-0017 records the
rule; any change to `decals.glsl` needs the Ultra overview measured again.

**The guard (review follow-up).** `MOBA_PIPELINE_STATS=1` enables
`VK_KHR_pipeline_executable_properties` where the driver has it and logs the terrain
pipeline's compiled statistics. On the RTX 5070 Ti the terrain fragment shader is at **64
registers** in both its single-sample and 4× MSAA pipelines, with the ring and water changes
below in. The record copy put back into today's shader (every loop and `decal_coverage`
reading a local `DecalRecord`) **also compiles to 64 registers** (a larger binary, 94 336
against 91 904 bytes), yet it still costs the Ultra overview's scene pass **0.70 ms against
0.44 ms** (three interleaved rounds each: 0.703/0.702/0.701 against 0.441/0.439/0.443; the
teamfight's 0.34 against 0.30). A register threshold alone would have passed it, so
`scripts/terrain-decal-check.sh` checks the cost directly: `MOBA_DECALS=off` now builds the
terrain from `terrain_nodecals.frag` (the decal code compiled out), and the script times the
local Ultra overview with decals on and off from one binary, failing above 1.15×; it then
checks the register count (≤ 64). On the shipped shader it passes: scene p50 0.250 ms with
decals against 0.245 ms without (1.02×), 64 registers at 1 and 4 samples. **Re-run on `main`
at `197857b`** (after R8 and distortion, the GPU otherwise idle): `terrain-decal-check: ok
scene p50 0.241 ms with decals, 0.232 ms without (<= x1.15)` (1.04×), `ok samples=1
registers=64 <= 64`, `ok samples=4 registers=64 <= 64`. The record-copy build is a modified
shader and was not rebuilt for this; its 1.6× on the networked overview above is the same
comparison the script makes, well past the 1.15× limit.

## Validation and synchronisation validation

`MOBA_VULKAN_VALIDATION=required` (the Khronos layer with synchronisation validation; the
client refuses to start without it), every message counted from the client's debug
messenger. **0 messages and exit 0 in every run on both devices**. On lavapipe, the final
code: the 35 matrix configurations below plus 13 extra ones, and every golden capture. On the
RTX 5070 Ti, the same 35 configurations with the review fixes in, before the last change
(`terrain_nodecals.frag` for `MOBA_DECALS=off`), and the 13 extra ones on the PR's first
version. **Re-run on `main` at `197857b`** with the final code on the RTX 5070 Ti: the river
showcase on all four tiers, the reeds over the full water on Ultra, the networked overview on
High and Ultra (Ultra also with `MOBA_DECALS=off`, the compiled-out terrain), the decal
teamfight on all four tiers and High with `MOBA_DECALS=off`, 300 frames each: 0 messages
([perf/0022](0022-r8-scaling-pacing.md#validation) has the whole 37-run matrix). The
configurations:

| Group | Configurations |
|---|---|
| Tiers | the local preview and the networked scene (loopback server, skinned heroes, tick 240) on Low, Medium, High and Ultra (8) |
| Switches | High with `MOBA_DECALS=off`, `MOBA_FOLIAGE=off`, `MOBA_WATER=off`, `MOBA_WATER=simple`, and all three off; Low with `MOBA_WATER=full`; the networked scene with all three off and with simple water |
| R6 × R3b | High + `MOBA_AA=msaa4` (foliage and simple water through the 4× pipelines, the full water after the resolve), MSAA + simple water, `MOBA_RENDER_SCALE=0.7` (refraction by texel in the sub-rectangle), Low at 0.5 without dynres, `MOBA_DYNRES=8`, Medium + MSAA + AO, Medium + `MOBA_AO=on`; the river close-up on Low and Ultra; Ultra's reeds over the full water (the depth re-resolve), and Ultra with simple water, water off and `MOBA_VFX=off` (the re-resolve's other triggers) |
| R5 × R3b | the teamfight replay at tick 360 with `MOBA_VFX_SCALE=8` on Low, High and Ultra, High with decals off and with MSAA; `MOBA_VFX_PREVIEW=nova` on Ultra, `binding-field` on Low; `MOBA_VFX=off`; the networked scene on Ultra at tick 1,500 (mid-fight) |
| R4b × R3b | `MOBA_STRESS_MINIONS=64` (camera 40 m: more drawn units than the 32 displacers every tier keeps) on High and Low |
| R3a × R3b | the networked overview; the horizon view on Ultra; `MOBA_TERRAIN=flat` (no terrain: the environment is cleared); `MOBA_FOW=legacy`; `MOBA_SHADOWS=off` |
| Maps | ember on Ultra and frost on High (their sparse foliage, no water) |
| R2b × R3b | every configuration above runs R2b's tier defaults (effect lights in the teamfight, outlines, contact shadows and the ID prepass on High and up); the ember golden (walls, baked AO, outlines) and the outline occlusion probe (`scripts/outline-probe.sh`, below) |

The frame-graph tests cover the new passes in every post topology
(`full_water_copies_then_draws_between_the_scene_and_the_particles`) and the depth
re-resolve (`the_scene_re_resolves_depth_after_the_prepass_for_water_and_particles`), and the
`descriptor_budget` test checks the scene, VAT, foliage and water layouts against the Vulkan
minimum, now including storage buffers per stage.

## Goldens

Twelve fingerprints and R2b's outline occlusion probe, captured with the exact
`.github/workflows/render-golden.yml` commands on lavapipe, validation and sync validation
required (0 messages in each run). Two runs of every capture were **byte-identical**
(`cmp`), and `image-check` of each run against the written fingerprint reports
`mean_abs_diff=0.000 max_diff=0`. The probe passes: a unit behind a wall is byte-identical
to outlines off on the hull (Low) and screen (High, Ultra) paths, and the open control
differs on each.

| Golden | Configuration | Against `main`'s fingerprint (a0d2e10) | Against the PR's first version |
|---|---|---|---|
| `render-fingerprint.rgb` | local preview, High | rebaselined: mean 0.461 / max 69 (grass and flowers off the lanes) | 0.060 / 9 |
| `render-network-fingerprint.rgb` | networked frame, tick 120, High | rebaselined: mean 0.157 / max 8 | 0.030 / 2 |
| `render-vat-fingerprint.rgb` | 64 VAT stress minions at 40 m, High | rebaselined: mean 2.207 / max 68 | 0.059 / 3 |
| `render-ultra-fingerprint.rgb` | the VAT scene on Ultra | rebaselined: mean 2.446 / max 70 | 0.046 / 2 |
| `render-low-fingerprint.rgb` | local preview, Low at render scale 0.75 | rebaselined: mean 0.315 / max 44 | 0.005 / 1 |
| `render-overview-fingerprint.rgb` | the networked whole-map overview | rebaselined: mean 0.220 / max 30 (both reaches) | 0.027 / 4 |
| `render-horizon-fingerprint.rgb` | the low horizon orbit | rebaselined: mean 0.726 / max 19 | 0.094 / 7 |
| `render-ember-fingerprint.rgb` | R2b's networked ember close-up at 24 m | rebaselined: mean 0.029 / max 5 (ember's sparse grass) | 0.004 / 1 |
| `render-river-fingerprint.rgb` | **new**: the river close-up on the default tier (full water, refraction, foam, reeds, grass) | — | 0.050 / 4 |
| `render-river-low-fingerprint.rgb` | **new**: the same on Low (simple water, one normal octave, thinner foliage) | — | 0.030 / 13 |
| `render-river-ultra-fingerprint.rgb` | **new (review)**: the bank's reeds in front of the full water on Ultra (MSAA + prepass), camera `4.04,0.2,9,4,-62` | — | new |
| `render-decals-fingerprint.rgb` | **new**: the teamfight replay at tick 360, 30 m camera (telegraph rings with their halo and progress fill, scorch and splats over grass, path and water, R2b's effect lights) | — | 0.359 / 33 (the ring rework) |

The second column's small differences come from the foliage distance now counting from the
ground focus (more distant grass) and the decals' ring rework. **`render-river-ultra`
guards the review's blocking bug**: with the depth re-resolve reverted, the capture fails
it (mean 0.076, max 39 > 24), the full water drawing over the reeds in front of it.

Five of the old eight exceed the CI threshold against `main` (mean 2.0 or max 24); all eight
were rebaselined so every fingerprint is exact. The local captures advance one simulation
tick and one VFX step per frame and the environment time is `render tick / 30`, so the
replay frame and the wind and flow are repeatable.

## Not measured here

- The other §7 reference GPUs (UHD 620, GTX 1060 and RTX 3060 classes, Apple M1) are
  deferred to pre-1.0 by the maintainer (#9/#127). Low's simple water and thinned foliage on a UHD 620 are the open
  question: the foliage vertex shader builds 3 blades per clump on Low, and the
  `MOBA_FOLIAGE=off` / `MOBA_WATER=simple` switches are the fall-backs.
- 1440p and 4K: the window caps at 2560×1380 on this display; the foliage and water costs
  are per-pixel (decals, water) and per-instance (foliage), so the 1080p deltas are the
  per-pixel lower bound.
- **Art sign-off** of the showcase map (roadmap R3 exit, "one fully art-directed map") is
  deferred to pre-1.0 by the maintainer: the sample maps are engine-authored. The screenshots
  (RTX 5070 Ti captures on `main` at `197857b`) are in
  [screenshots](../screenshots.md#decals-foliage-and-water-r3b).
