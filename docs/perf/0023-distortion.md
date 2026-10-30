# Distortion / heat haze: cost, validation and image evidence (R6 item 7)

Captured 2026-10-30 on the development workstation, not a CI runner, for
[ADR-0019](../adr/0019-distortion.md).

- Software: lavapipe (`llvmpipe (LLVM 23.1.3, 256 bits)`) from
  `/home/jaket/Projects/MOBA/.tools` with the bundled Khronos validation layer
- GPU: NVIDIA GeForce RTX 5070 Ti (16 GB), driver 615.78.08 — **not measured, see below**
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-2-cachyos, Wayland
- Build: release, this branch (`feat/r6-distortion`, based on `main` at `db29264`).
  "Off" is the same build with `MOBA_DISTORTION=off`, which removes both passes from the
  graph and skips every haze spawn.

## RTX 5070 Ti timings: pending

The brief asks for RTX timings (median of three interleaved on/off rounds of the teamfight
replay at High and Ultra). They could not be taken: another process held 15.6 of the GPU's
16.3 GB (`nvidia-smi --query-gpu=memory.used,memory.total`: `15623 MiB, 16303 MiB`), and the
client fails at start-up with `renderer initialisation failed: GPU allocation error: Out of
memory`. That process was left alone. **Follow-up:** rerun the commands below on the RTX
when its memory is free and add the table here.

```bash
MOBA_QUALITY=<high|ultra> MOBA_WINDOW_SIZE=1920x1080 MOBA_REPLAY=tests/replays/teamfight-5v5.mbr \
MOBA_WARMUP_TICKS=330 MOBA_SMOKE_FRAMES=3000 MOBA_VSYNC=0 MOBA_PIPELINE_CACHE=off \
MOBA_FRAME_LOG=/tmp/r19.log [MOBA_DISTORTION=off] cargo run --release -p moba-client
```

**What to expect, from the pass structure.** The teamfight at tick 330 has no haze in view
(see below), so on the RTX both passes are two empty timestamp pairs plus their barriers:
the `distortion_copy` records no copy and `distortion` no draw. With haze in view the copy
is one 1080p `R16G16B16A16_SFLOAT` region copy (16.6 MB read and written; R3b's
`water_copy`, the same copy, measured 0.01–0.02 ms on this RTX in
[perf/0021](0021-decals-foliage-water.md)), and the draw shades only the haze sprites'
pixels with five texture reads each (a bilinear copy tap, two depth reads, an ID read and
two 2×2 gathers), well under the particles' own `vfx_draw` cost at the same coverage.
On lavapipe, where every pass is CPU-rasterised, both together are 0.5 % of the frame.

## Lavapipe timings (1280×720)

Four scenes, three rounds interleaved on/off (on, off, on, off, …), validation off, no
pipeline cache, 90 frames each (the frame log covers the last 28). GPU ms at p50 from the
renderer's timestamp queries, **median of the three rounds**; `range` is the three rounds'
p50 totals.

| scene | distortion | total p50 / p95 | range | scene | vfx_draw | distortion_copy | distortion |
|---|---|---|---|---|---|---|---|
| teamfight, High | on | 124.02 / 134.26 | 123.5–124.1 | 107.83 | 0.58 | 0.032 (idle) | 0.032 (idle) |
| teamfight, High | off | 124.72 / 133.09 | 123.5–129.8 | 108.55 | 0.52 | — | — |
| teamfight, Ultra | on | 163.70 / 177.29 | 163.0–172.0 | 140.39 | 0.71 | 0.031 (idle) | 0.032 (idle) |
| teamfight, Ultra | off | 164.47 / 179.34 | 163.6–164.6 | 141.36 | 0.74 | — | — |
| `cinder-zone` preview, High | on | 157.23 / 168.38 | 157.0–158.2 | 139.30 | 0.57 | **0.156** | **0.842** |
| `cinder-zone` preview, High | off | 156.42 / 171.54 | 155.9–164.4 | 139.99 | 0.51 | — | — |
| `cinder-zone` preview, Ultra | on | 221.08 / 238.43 | 220.1–223.2 | 197.60 | 0.56 | **0.178** | **0.837** |
| `cinder-zone` preview, Ultra | off | 218.74 / 232.60 | 218.4–220.5 | 196.33 | 0.53 | — | — |

- **Teamfight:** the fight at tick 330 plays no hazy effect, so both passes run idle: no
  commands, ~0.03 ms each on lavapipe for the timestamp pair and the barriers. On/off totals
  are within the rounds' noise.
- **`cinder-zone` with the haze live** (7 distortion particles of 78 live, a 3 m zone at the
  preview camera): the copy costs 0.16–0.18 ms and the draw
  0.84 ms, together **1.0 ms of a 157–221 ms frame (0.5–0.6 %)**. The total's on/off delta
  (+0.8 / +2.3 ms) is inside the run-to-run range of the scene pass.
- **Idle frames** cost the two empty passes only (ADR-0019 §2): the renderer records the copy
  and the draw only while the CPU's lifetime bookkeeping says a haze can be live.

## Memory (R8 accounting)

| tier | `vram_mb` on / off | `targets_mb` on / off | `buffers_mb` on / off |
|---|---|---|---|
| High | 85.5 / 85.5 | 49.1 / 49.1 | 15.8 / 15.8 |
| Ultra | 184.7 / 184.7 | 139.8 / 139.8 | 24.2 / 24.2 |

The copy is a graph transient that shares the water's refraction copy or the OIT
accumulation target, so **no target is added** on High or Ultra (the frame-graph unit test
pins that no topology with water or OIT gains a slot). The `alive` buffer is allocated with
the distortion region on every tier (2 / 6 / 12 / 24 KB more, Low … Ultra), which the
accounting rounds away. `uploads` stays at the load-time count and `locked_uploads=0`,
`locked_pipelines=0` in every run: the distortion pipeline is built with the other VFX
pipelines at renderer creation.

## Validation and sync validation (lavapipe, `MOBA_VULKAN_VALIDATION=required`)

Zero `VUID`/`SYNC-` messages in every run below (validation and synchronisation validation
on, as in `render-golden.yml`):

- all twelve existing render-golden captures (default High, network, ember, VAT, Ultra
  with MSAA 4×, Low at 0.75 render scale, overview, horizon, river on High, Low and Ultra,
  teamfight decals) — distortion idle on High/Ultra, off on Low;
- the new distortion golden (`cinder-zone`, Ultra, MSAA, haze live);
- `cinder-zone` on High (live), High at `MOBA_RENDER_SCALE=0.75` (960×540 sub-rectangle,
  live), Medium (off) and `ember-curse` on Low at 0.75 (off);
- the Ember map on High and Ultra (networked, the vents in the map's `[[effects]]`) and a
  local camera on an Ember crystal (vents live);
- `scripts/outline-probe.sh` (all six cases ok: hidden units give zero outline pixels on the
  hull, High screen and Ultra MSAA screen paths; the new ID value 8 for unoutlined units
  does not change the outlines) and `scripts/cull-probe.sh` (all five scenes byte-identical
  with culling on and off).

## Goldens

| golden | result |
|---|---|
| the twelve existing fingerprints | **unchanged**: every capture matches with `mean_abs_diff=0.000 max_diff=0` |
| `render-distortion-fingerprint.rgb` | **new**: the `cinder-zone` preview on Ultra at frame 90 (MSAA resolved, prepass IDs, the stand-in units beside the haze); two lavapipe runs gave byte-identical captures |

The fingerprint (64×36, tolerance mean 2.0 / max 24) is too coarse to see a 5-pixel
shimmer: the same frame with `MOBA_DISTORTION=off` still passes it (`mean 0.136, max 21`).
The full-resolution frames do differ (26,805 pixels, max channel delta 71, all inside the
zone), so the CI step also requires the capture log to show the distortion passes ran and
no `distortion off` line. The golden guards against gross breakage (garbage taps, NaNs,
a wrong copy region); the cap and mask logic are unit-tested on the CPU twin
(`moba_render::distortion`).

**Readability check (Ember crystal, High, on vs off).** 457 pixels change, max delta 24,
in two bands either side of the crystal; the crystal itself (a prop, prepass ID 8) is
untouched pixel for pixel: the unit mask keeps every unit's silhouette in place.

## Reproduce

```bash
# golden + evidence captures (validation on)
MOBA_VFX_PREVIEW=cinder-zone MOBA_QUALITY=ultra MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90 \
MOBA_CAPTURE=render-distortion-actual.ppm MOBA_FRAME_LOG=render-distortion.frames \
MOBA_VULKAN_VALIDATION=required ./target/release/moba-client
./target/release/image-check render-distortion-actual.ppm \
  --expected tests/golden/render-distortion-fingerprint.rgb
# timings (validation off), with and without MOBA_DISTORTION=off
MOBA_VFX_PREVIEW=cinder-zone MOBA_QUALITY=high MOBA_SMOKE_FRAMES=90 MOBA_VSYNC=0 \
MOBA_PIPELINE_CACHE=off MOBA_FRAME_LOG=/tmp/r19.frames ./target/release/moba-client
```
