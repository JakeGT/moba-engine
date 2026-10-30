# Terrain, fog of war v2 and atmosphere: cost and validation (R3a)

Captured 2026-10-29 on the development workstation, not a CI runner, for
[ADR-0014](../adr/0014-terrain-fog-of-war.md).

- GPU: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08 (`nvidia_icd.json`); software: lavapipe
  from `/home/jaket/Projects/MOBA/.tools` with the bundled Khronos validation layer
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-2-cachyos, Wayland
- Builds, both release: **before** is `main` at `74f3087` (R5 VFX merged; flat procedural
  terrain plane, 16-circle fog-of-war loop, distance fade); **after** is this branch rebased
  on it. The host and the GPU were shared with other agents' builds and captures; GPU columns
  come from timestamp queries, so host load moves them little, and each row is the median of
  three interleaved rounds (before and after alternate) to spread GPU sharing evenly.

## Method

The client runs at 1920×1080 with vsync off and validation off, `MOBA_SERVER=local` (an
in-process loopback server; this client's hero with the `sentinel-skinned` model, the minion
waves and the structures, fog of war from the blue team's vision), 2,400 frames of which the first 60 are warm-up, so 2,340
logged frames per run. Two cameras:

- **player**: the networked gameplay camera following the own hero (about 40 m up, looking at
  points 18–80 m away; the usual frame);
- **overview** (`MOBA_CAMERA=overview`): the whole map from high above, so the terrain, its
  margin and the fog of war cover the entire screen. This is the worst case for the terrain
  shader, not a gameplay view.

Each tier runs its defaults (ADR-0014 §5; shadows as ADR-0011). Switch rows run High with one
feature off. Columns are GPU ms p50 from `MOBA_FRAME_LOG` (`gpu_<pass>_p50_ms`); the GPU
total is the sum of all passes (UI, composite and the R5 VFX passes included), with the
min..max over the three rounds in brackets, and its p95. Raw logs:
`timings.sh` output, kept with the scratch evidence, not committed.

## Per-pass GPU time (RTX 5070 Ti, 1080p)

### player camera

| Run | shadow | scene | fow_raster | fow_resolve | GPU total p50 (spread) | GPU total p95 |
|---|---:|---:|---:|---:|---:|---:|
| before Low | 0.004 | 0.079 | — | — | 0.113 (0.110..0.114) | 0.122 |
| **after Low** | 0.004 | 0.084 | 0.006 | 0.002 | 0.127 (0.121..0.128) | 0.137 |
| before Medium | 0.006 | 0.083 | — | — | 0.133 (0.127..0.134) | 0.143 |
| **after Medium** | 0.006 | 0.121 | 0.006 | 0.003 | 0.178 (0.170..0.179) | 0.186 |
| before High | 0.006 | 0.096 | — | — | 0.148 (0.141..0.148) | 0.156 |
| **after High** | 0.006 | 0.131 | 0.006 | 0.003 | 0.190 (0.185..0.191) | 0.200 |
| before Ultra | 0.011 | 0.106 | — | — | 0.165 (0.164..0.167) | 0.175 |
| **after Ultra** | 0.011 | 0.154 | 0.007 | 0.004 | 0.223 (0.222..0.224) | 0.233 |
| after High, `MOBA_TERRAIN=flat` | 0.006 | 0.075 | 0.006 | 0.003 | 0.136 (0.127..0.136) | 0.147 |
| after High, `MOBA_FOW=legacy` | 0.006 | 0.160 | — | — | 0.213 (0.206..0.216) | 0.222 |
| after High, `MOBA_FOG=off` | 0.006 | 0.131 | 0.006 | 0.003 | 0.190 (0.183..0.191) | 0.200 |
| after High, all off¹ | 0.006 | 0.104 | — | — | 0.157 (0.155..0.164) | 0.165 |

### overview camera (whole map, worst case)

| Run | shadow | scene | fow_raster | fow_resolve | GPU total p50 (spread) | GPU total p95 |
|---|---:|---:|---:|---:|---:|---:|
| before Low | 0.004 | 0.043 | — | — | 0.077 (0.077..0.086) | 0.090 |
| **after Low** | 0.004 | 0.098 | 0.006 | 0.002 | 0.140 (0.140..0.155) | 0.144 |
| before Medium | 0.005 | 0.045 | — | — | 0.096 (0.089..0.096) | 0.104 |
| **after Medium** | 0.005 | 0.143 | 0.006 | 0.003 | 0.200 (0.192..0.201) | 0.210 |
| before High | 0.005 | 0.050 | — | — | 0.103 (0.103..0.166) | 0.110 |
| **after High** | 0.005 | 0.156 | 0.006 | 0.003 | 0.215 (0.215..0.216) | 0.223 |
| before Ultra | 0.010 | 0.062 | — | — | 0.124 (0.122..0.229) | 0.139 |
| **after Ultra** | 0.010 | 0.194 | 0.007 | 0.004 | 0.262 (0.258..0.263) | 0.278 |
| after High, `MOBA_TERRAIN=flat` | 0.005 | 0.050 | 0.006 | 0.003 | 0.109 (0.107..0.120) | 0.119 |
| after High, `MOBA_FOW=legacy` | 0.005 | 0.191 | — | — | 0.243 (0.236..0.245) | 0.250 |
| after High, `MOBA_FOG=off` | 0.005 | 0.153 | 0.006 | 0.003 | 0.212 (0.211..0.212) | 0.221 |
| after High, all off¹ | 0.005 | 0.054 | — | — | 0.108 (0.108..0.109) | 0.114 |

¹ `MOBA_TERRAIN=flat MOBA_FOW=legacy MOBA_FOG=off MOBA_SKY=off`: the pre-R3a look (the
legacy loop still fogs the flat plane). The `before` spreads with a high maximum (High and
Ultra overview) are one round each that shared the GPU with another agent's capture; the
median discards them.

Frame intervals are display-paced (p50 ≈ 4.18 ms at 240 Hz before and after, as in
[perf/0011](0011-render-graph.md)); the GPU is idle for over 90 % of every frame on every row.

### Re-check after the review fixes, on `1c44dec`

The PR review changed the fog of war (the disc is inset one texel, lost vision drops at once,
the fade is 0.3 s), the terrain apron and the edge-prop placement, and the branch was rebased
on `1c44dec` (R4b's VAT minions merged; the networked run's minions are VAT-drawn in both
builds). The whole table was measured again the same way (three interleaved rounds); its
High and Ultra rows were disturbed by another GPU client (the shadow pass, unchanged by R3a,
ran at 2.5× its usual time in both builds), so those four rows were measured again in three
more interleaved rounds, shown here. GPU total p50 (spread):

| Tier | player before | player after | overview before | overview after |
|---|---:|---:|---:|---:|
| Low | 0.113 (0.111..0.114) | 0.125 (0.125..0.130) | 0.075 (0.075..0.076) | 0.137 (0.136..0.137) |
| Medium | 0.130 (0.128..0.130) | 0.172 (0.172..0.173) | 0.091 (0.091..0.092) | 0.205 (0.192..0.206) |
| High | 0.152 (0.143..0.158) | 0.199 (0.191..0.216) | 0.097 (0.097..0.110) | 0.216 (0.215..0.240) |
| Ultra | 0.167 (0.165..0.181) | 0.229 (0.229..0.246) | 0.126 (0.120..0.133) | 0.283 (0.264..0.287) |

The fog-of-war passes stay at 0.006–0.007 ms (raster) and 0.002–0.004 ms (resolve): the inset
and the instant drop add no work per texel beyond one `min`. Within the noise these match the
first table; the switch rows (High, player: flat 0.131, legacy 0.215, all off 0.151 ms) also
keep their order. The analysis below uses the first table.

## Reading the numbers

- **The splat terrain is the cost.** Per frame on High it adds 0.035 ms to the scene pass in
  the player view (0.096 → 0.131) and 0.106 ms in the full-screen overview (0.050 → 0.156):
  five layers sampled with explicit gradients, normal maps, triplanar on slopes, and a
  heightfield mesh of 256² cells. `MOBA_TERRAIN=flat` returns the scene pass to the baseline
  (0.075 / 0.050 ms). Low (four layers, top-down only, no normal maps, 128² cells) costs
  +0.005 ms in the player view.
- **The fog-of-war texture is cheaper than the loop it replaces.** The two passes take
  0.008–0.011 ms together at every tier (Ultra's 512² texture included), and the scene then
  samples one texel per fragment. `MOBA_FOW=legacy` (the 16-circle loop with per-circle wall
  tests in every terrain fragment) makes the scene pass 0.020 ms (player) and 0.026 ms
  (overview) slower than the texture's scene pass plus both of its passes (GPU total
  +0.023 / +0.028 ms), and it caps vision at 16 sources.
- **Height fog and the sky are almost free:** `MOBA_FOG=off` saves at most 0.003 ms; the sky
  covers only the pixels the terrain does not.
- **Totals.** The worst row, Ultra overview, is 0.262 ms GPU per frame at 1080p (p95
  0.278 ms); the gameplay camera on High is 0.190 ms. The RTX 5070 Ti numbers say nothing
  about the min-spec (ADR-0002, UHD 620-class) at Low; that measurement is still owed with the
  other reference machines (roadmap §7, #127).
- **No per-frame allocation.** Vision sources and walls go into fixed per-frame-slot storage
  buffers (`DEFAULT_MAX_VISION_SOURCES` 1,024, `MAX_VISION_WALLS` 256; overflow is dropped
  and counted by `vision_sources_dropped`, never reallocated), the client's
  `hud::FogInput` reuses its buffers (unit test with an allocation probe), and the graph and
  descriptors are compiled and written at creation.
- **Map bind.** Generating the heightfield and splat and uploading the terrain takes
  19–53 ms once per map bind (`[assets] terrain: 5 layers, <n> cells, built in <t> ms`; the
  spread is host load from parallel builds, larger grids at the top of it), never during a
  match.

## Validation and synchronisation validation

Every run below used `MOBA_VULKAN_VALIDATION=required` (Khronos validation plus
synchronisation validation) and exited 0 with **0 validation messages**. `net` is the
networked local server with skinned heroes pinned to tick 240; `local` is the offline
preview (frame 90 of 120).

| Configuration | lavapipe | RTX 5070 Ti |
|---|---|---|
| net + local, Low / Medium / High / Ultra (8 runs) | 0 | 0 |
| net `MOBA_FOW=legacy`, `MOBA_FOW=off`, `MOBA_FOW_SIZE=1024` | 0 | 0 |
| net `MOBA_TERRAIN=flat`, `MOBA_FOG=off`, `MOBA_SKY=off` | 0 | 0 |
| net + local, all off (`flat`, `off`, `off`, `off`) | 0 | 0 |
| net overview camera, local horizon (`MOBA_PREVIEW_CAMERA=-0.7,0.12,170`) | 0 | 0 |
| ember and frost sample maps (`content/{ember,frost}-map.toml`) | 0 | 0 |
| net with a VFX cast (`MOBA_DEMO_CAST=0@150`, R5 passes plus fog of war) | 0 | 0 |
| local, 64 VAT stress minions at 40 m (R4b), default tier and Low | 0 | 0 |
| 1920×1080 screenshot captures, High, before and after (12 runs) | — | 0 |
| the five golden captures, twice (below) | 0 | — |

## Goldens

All five 64×36 fingerprints were captured on lavapipe with the
[render-golden.yml](../../.github/workflows/render-golden.yml) commands and validation
required, twice: each pair of PPMs is byte-identical, and the second run compares with
`mean_abs_diff=0.000 max_diff=0` against the fingerprint written from the first.

| Golden | Change | Capture |
|---|---|---|
| `render-fingerprint.rgb` | rebaselined (terrain, sky, height fog) | local default, frame 90 |
| `render-network-fingerprint.rgb` | rebaselined (terrain, fog-of-war texture, height fog) | networked, skinned heroes, tick 120 |
| `render-overview-fingerprint.rgb` | **new**: the whole map with the fog of war | networked, `MOBA_CAMERA=overview`, tick 120 |
| `render-horizon-fingerprint.rgb` | **new**: the sky, haze and map-edge hills | local, `MOBA_PREVIEW_CAMERA=-0.7,0.12,170` |
| `render-vat-fingerprint.rgb` | rebaselined (R4b's VAT close-up: the preview's terrain, sky and height fog) | local, 64 stress minions at 40 m, frame 90 |

Pinned captures (`MOBA_CAPTURE_TICK`) resolve the fog of war with
`FOG_OF_WAR_SETTLE_DT` (10 s, past every ease), so the fingerprint does not depend on how
many frames the runner drew before the capture tick.

## Reproducing

```bash
cargo build --release --locked -p moba-client -p moba-tools
# timings (repeat and interleave with a build of the base commit):
MOBA_VULKAN_VALIDATION=off MOBA_VSYNC=0 MOBA_WINDOW_SIZE=1920x1080 MOBA_SERVER=local \
  MOBA_HERO_MODELS=sentinel-skinned MOBA_SMOKE_FRAMES=2400 MOBA_QUALITY=high \
  MOBA_FRAME_LOG=/tmp/r3a.txt ./target/release/moba-client      # add MOBA_CAMERA=overview
# validation: the same with MOBA_VULKAN_VALIDATION=required, plus the switches above
```
