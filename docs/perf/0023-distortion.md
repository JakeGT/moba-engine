# Distortion / heat haze: cost, validation and image evidence (R6 item 7)

Captured 2026-10-30 on the development workstation, not a CI runner, for
[ADR-0019](../adr/0019-distortion.md).

- Software: lavapipe (`llvmpipe (LLVM 23.1.3, 256 bits)`) from
  `/home/jaket/Projects/MOBA/.tools` with the bundled Khronos validation layer
- GPU: NVIDIA GeForce RTX 5070 Ti (16 GB), driver 615.78.08, measured on `main` at `197857b`
  ([below](#rtx-5070-ti-timings))
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-2-cachyos, Wayland
- Build: release, this branch (`feat/r6-distortion`, based on `main` at `db29264`).
  "No-draw" is the same build with `MOBA_DISTORTION=nodraw`, which spawns and simulates every
  haze particle exactly as "on" does but never draws it, so the two differ by the
  refraction alone. (An earlier draft compared against `MOBA_DISTORTION=off`, which skips
  the haze spawns: that shifts the allocator and the effects' random streams, so its image
  diff mixed different particles with the real refraction. Every number below is against
  no-draw.)

## RTX 5070 Ti timings

The PR's RTX run was blocked (another process held 15.6 of the card's 16.3 GB and the client
could not allocate). Re-run on `main` at `197857b` the same day (17:58–18:06 UTC),
release, 1920×1080, vsync off, validation off, no pipeline cache, 3,000 frames per
run, **three rounds interleaved on / no-draw over every row**, the median of the three in
each cell; `p50 range` is the rounds' GPU totals.

- **The GPU had no other client**: 2.3 GB of 16.3 GB used before the set (the desktop:
  `kwin_wayland`, `plasmashell`, a browser and the session's screen locker), 0 % utilisation.
  The frame interval was the 240 Hz cap (4.18 ms) in every run.
- **Clocks** cannot be locked without root. `nvidia-smi -q -d CLOCK` was recorded before the
  set, a 1,500-frame Ultra warm-up ran first, and a sampler logged the clocks every 250 ms;
  the last column is each round's dominant memory clock. Every row ran at 7001 MHz except the
  High Ember crystal pair (810 and 405 MHz, a lighter scene the driver clocks down): its
  totals wander between rounds, so only its pass times are quoted.

```bash
MOBA_QUALITY=<high|ultra> MOBA_WINDOW_SIZE=1920x1080 MOBA_SMOKE_FRAMES=3000 MOBA_VSYNC=0 \
MOBA_PIPELINE_CACHE=off MOBA_FRAME_LOG=r19.log [MOBA_DISTORTION=nodraw] <scene> \
  ./target/release/moba-client
# <scene>: MOBA_REPLAY=tests/replays/teamfight-5v5.mbr MOBA_WARMUP_TICKS=330
#          MOBA_VFX_PREVIEW=cinder-zone
#          MOBA_CONTENT=content/ember-map.toml MOBA_PREVIEW_CAMERA=0.6,0.35,24,-7,10
```

| scene, tier, distortion | scene | water_copy | water | vfx | distortion_copy | distortion | bloom | composite | fxaa | GPU total p50 / p95 | p50 range | mem clock per round (MHz) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| teamfight, High, on | 0.159 | 0.019 | 0.023 | 0.025 | — | — | 0.042 | 0.020 | 0.024 | **0.493** / 0.506 | 0.487–0.494 | 7001 / 7001 / 7001 |
| teamfight, High, no-draw | 0.158 | 0.019 | 0.026 | 0.024 | — | — | 0.042 | 0.020 | 0.024 | **0.491** / 0.503 | 0.489–0.491 | 7001 / 7001 / 7001 |
| `cinder-zone`, High, on | 0.168 | 0.018 | 0.019 | 0.029 | 0.007 | 0.010 | 0.042 | 0.020 | 0.024 | **0.524** / 0.546 | 0.521–0.527 | 7001 / 7001 / 7001 |
| `cinder-zone`, High, no-draw | 0.168 | 0.019 | 0.019 | 0.029 | — | — | 0.042 | 0.020 | 0.024 | **0.508** / 0.526 | 0.506–0.509 | 7001 / 7001 / 7001 |
| Ember crystal, High, on | 0.219 | — | — | 0.029 | 0.011 | 0.005 | 0.058 | 0.026 | 0.028 | **0.638** / 0.769 | 0.481–0.656 | 810 / 810 / 810 |
| Ember crystal, High, no-draw | 0.228 | — | — | 0.030 | — | — | 0.060 | 0.024 | 0.029 | **0.633** / 1.042 | 0.587–0.710 | 405 / 810 / 810 |
| teamfight, Ultra, on | 0.282 | 0.022 | 0.030 | 0.036 | — | — | 0.042 | 0.022 | — | **0.719** / 0.730 | 0.718–0.719 | 7001 / 7001 / 7001 |
| teamfight, Ultra, no-draw | 0.283 | 0.022 | 0.030 | 0.037 | — | — | 0.042 | 0.022 | — | **0.720** / 0.731 | 0.720–0.721 | 7001 / 7001 / 7001 |
| `cinder-zone`, Ultra, on | 0.285 | 0.022 | 0.025 | 0.040 | 0.007 | 0.012 | 0.042 | 0.022 | — | **0.758** / 0.776 | 0.757–0.758 | 7001 / 7001 / 7001 |
| `cinder-zone`, Ultra, no-draw | 0.285 | 0.022 | 0.026 | 0.041 | — | — | 0.042 | 0.022 | — | **0.742** / 0.756 | 0.742–0.742 | 7001 / 7001 / 7001 |
| Ember crystal, Ultra, on | 0.306 | — | — | 0.032 | 0.011 | 0.008 | 0.045 | 0.023 | — | **0.751** / 0.761 | 0.751–0.751 | 7001 / 7001 / 7001 |
| Ember crystal, Ultra, no-draw | 0.306 | — | — | 0.033 | — | — | 0.048 | 0.022 | — | **0.737** / 0.747 | 0.736–0.738 | 7001 / 7001 / 7001 |

- **Teamfight:** no hazy effect plays from tick 330, so every frame runs the idle variant (no
  distortion lanes in the log); on and no-draw agree within 0.002 ms on both tiers.
- **`cinder-zone` with the haze live** (68–71 live particles, `vfx_distortion_max` 9–10,
  the distortion passes in 2,522–2,598 of the 2,940 logged frames as the zone cycles): the
  bounding-box copy costs **0.007 ms** and the draw **0.010 ms (High) / 0.012 ms (Ultra)**;
  the total moves by +0.016 ms on both tiers. The scene pass is unchanged (0.168 / 0.168,
  0.285 / 0.285 ms).
- **Ember crystal** (the map's heat vents, `vfx_distortion_max` 21, distortion in 2,938
  of 2,940 frames): copy 0.011 ms and draw 0.005 ms (High) / 0.008 ms (Ultra); Ultra's total +0.014 ms.
- **Against the budgets:** the heaviest row, `cinder-zone` on Ultra, is 0.758 / 0.776 ms (p50
  / p95) against 7.5 ms; High's is 0.524 / 0.546 ms against 6.0 ms. The haze costs at most
  0.02 ms of either. This matches the expectation from the pass structure (one region copy
  of the haze's box, a scissored draw of its sprites; R3b's full-frame `water_copy` is
  0.018–0.022 ms on the same GPU in [perf/0021](0021-decals-foliage-water.md)).
- **Memory:** `vram_mb` 134.5 (High) and 286.7 (Ultra) with the haze on and with no-draw
  alike (`targets_mb` 87.5 / 231.3); `locked_uploads=0`, `locked_pipelines=0` in every run.

## Lavapipe timings (1280×720)

Four scenes, three rounds interleaved on/no-draw (on, no-draw, on, no-draw, …), validation
off, no pipeline cache, 90 frames each (the frame log covers the last 28). GPU ms at p50 from
the renderer's timestamp queries, **median of the three rounds**; `range` is the three
rounds' p50 totals. Live particles and `vfx_distortion_max` are identical between on and
no-draw in every scene (the same spawns).

| scene | distortion | total p50 / p95 | range | scene | vfx_draw | distortion_copy | distortion |
|---|---|---|---|---|---|---|---|
| teamfight, High | on | 123.64 / 136.42 | 123.6–124.0 | 107.53 | 0.59 | — (idle variant) | — (idle variant) |
| teamfight, High | no-draw | 123.59 / 131.50 | 123.4–123.7 | 107.41 | 0.56 | — | — |
| teamfight, Ultra | on | 164.54 / 180.64 | 163.7–169.4 | 141.33 | 0.81 | — (idle variant) | — (idle variant) |
| teamfight, Ultra | no-draw | 164.63 / 175.63 | 164.2–164.7 | 141.16 | 0.72 | — | — |
| `cinder-zone` preview, High | on | 157.14 / 168.00 | 157.1–157.4 | 139.57 | 0.57 | **0.094** | **0.896** |
| `cinder-zone` preview, High | no-draw | 156.25 / 168.36 | 156.2–156.3 | 139.55 | 0.54 | — | — |
| `cinder-zone` preview, Ultra | on | 221.19 / 238.07 | 221.1–232.4 | 197.59 | 0.58 | **0.121** | **0.883** |
| `cinder-zone` preview, Ultra | no-draw | 220.06 / 233.17 | 219.6–220.1 | 197.62 | 0.59 | — | — |

- **Teamfight:** the fight at tick 330 plays no hazy effect, so every frame runs the idle
  variant: the frame log has no `distortion_copy` or `distortion` lane at all (no
  timestamps, no commands, no barriers; the frame-graph unit test pins two fewer passes and
  at least three fewer barriers than the haze variant, with the same transient slots).
  On/no-draw totals are within the rounds' noise.
- **`cinder-zone` with the haze live** (78 live particles, a 3 m zone at the preview
  camera): the bounding-box copy costs 0.09–0.12 ms (the full-frame copy of the first
  draft cost 0.16–0.18 ms) and the draw 0.88–0.90 ms, together **1.0 ms of a 157–221 ms
  frame (0.5–0.6 %)**. The on/no-draw total delta (+0.9 / +1.1 ms) is the two passes; the
  scene pass is unchanged (139.57 / 139.55, 197.59 / 197.62).

## Memory (R8 accounting)

| tier | `vram_mb` on / no-draw | `targets_mb` on / no-draw | `buffers_mb` on / no-draw |
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

## Validation and sync validation (`MOBA_VULKAN_VALIDATION=required`)

**RTX 5070 Ti** (`main` at `197857b`, 1280×720, 300 frames each, part of the 37-run matrix in
[perf/0022](0022-r8-scaling-pacing.md#validation)): **0 messages and exit 0** for
`cinder-zone` on High and Ultra (MSAA 4×), each also with `MOBA_DISTORTION=nodraw` and at
`MOBA_RENDER_SCALE=0.75`; the Ember crystal (`MOBA_CONTENT=content/ember-map.toml
MOBA_PREVIEW_CAMERA=0.6,0.35,24,-7,10`) on High and Ultra, drawn and no-draw; the networked
Ember map on High and Ultra; and the teamfight replay on every tier. Every live run logged
the distortion passes in 238 of its 240 measured frames, every no-draw run none.

**Lavapipe:** zero `VUID`/`SYNC-` messages in every run below (validation and synchronisation
validation on, as in `render-golden.yml`):

- all twelve existing render-golden captures (default High, network, ember, VAT, Ultra
  with MSAA 4×, Low at 0.75 render scale, overview, horizon, river on High, Low and Ultra,
  teamfight decals) — distortion idle on High/Ultra, off on Low;
- the distortion golden (`cinder-zone`, Ultra, MSAA, haze live) and the same frame with
  `MOBA_DISTORTION=nodraw`;
- `cinder-zone` on High (live) and its no-draw twin, High at `MOBA_RENDER_SCALE=0.75`
  (960×540 sub-rectangle, live), **Ultra (MSAA 4×) at 0.75** (live), Medium (off) and
  `ember-curse` on Low at 0.75 (off); every live capture logs `vfx_distortion_max=8.000`
  and 27 frames with the distortion passes, every no-draw capture the same
  `vfx_distortion_max` and none;
- the Ember map on High and Ultra (networked, the vents in the map's `[[effects]]`, none in
  view; also with no-draw) and a local camera on an Ember crystal, on and no-draw (vents
  live, `vfx_distortion_max=14.000`);
- `scripts/outline-probe.sh` (all six cases ok: hidden units give zero outline pixels on the
  hull, High screen and Ultra MSAA screen paths; the new ID value 8 for unoutlined units
  does not change the outlines) and `scripts/cull-probe.sh` (all five scenes byte-identical
  with culling on and off).

## Goldens

| golden | result |
|---|---|
| the twelve existing fingerprints | **unchanged**: every capture matches with `mean_abs_diff=0.000 max_diff=0` |
| `render-distortion-fingerprint.rgb` | **new**: the `cinder-zone` preview on Ultra at frame 90 (MSAA resolved, prepass IDs, the stand-in units beside the haze); two lavapipe runs gave byte-identical captures |

The fingerprint (64×36, tolerance mean 2.0 / max 24) is too coarse to see a few-pixel
shimmer: the no-draw capture of the same frame still passes it (`mean 0.008, max 3`). So
`render-golden.yml` captures the frame twice more with `MOBA_BLOOM=off`, drawn
(`render-distortion-flat.ppm`) and with `MOBA_DISTORTION=nodraw`, and requires all of:

- the golden and the drawn bloom-off logs show `vfx_distortion_max` > 0 and at least one
  frame with the distortion passes, and no `distortion off` line;
- the no-draw log shows `vfx_distortion_max` > 0 (the haze was there) and no distortion
  pass;
- the pair is not byte-identical (`cmp -s`), and `image-check --against
  render-distortion-nodraw.ppm --min-changed 5000` passes;
- the stand-in units' boxes are byte-identical in both (`--same-box 640,448,684,505` for the
  red caster at the haze's edge, `--same-box 700,245,732,300` for the blue target).

**Why bloom is off in the pair.** The first CI run of the pair (with bloom on, run
41052647919, the runner's lavapipe on LLVM 21.1.8) found **one** changed pixel in the red
stand-in's box: (648, 477), a flat face pixel (`221,149,133` vs `220,149,133`), 27 px right
of the nearest refracted pixel and well inside the silhouette, so neither a mask leak at the
silhouette edge nor an outline. It is bloom: bloom blurs the HDR target, so the haze's
change reaches tens of pixels past the haze at 1–2 LSB after tonemapping. On that run the
1–2 LSB changes span x 424–664, y 276–529, while every change above 2 LSB stays inside
x 437–621, y 330–492; with bloom off (locally) every change, of any size, stays inside
x 424–621, y 316–498, and both boxes are untouched. Whether a 1-LSB bloom change rounds over
on a given pixel depends on the driver's float rounding, which is why the local LLVM 23
build happened to give 0. Every other pass after the haze is per-pixel at Ultra (MSAA, no
FXAA), so with bloom off the boxes test the unit mask alone.

**Refraction evidence (full resolution, 1280×720, on vs no-draw).** Ultra: **18,833 pixels**
change (18,873 with bloom off), High: **18,001**, all inside x 384–637, y 295–520 (the zone,
its haze and its bloom). Both unit boxes: **0 changed pixels** locally. In the diff image the zone's telegraph ring band is a dark gap
inside the changed area: the telegraph mask keeps every rim and halo pixel of the enemy
zone's decal in place (the ring particles draw after the haze anyway). Two lavapipe runs of
each capture are byte-identical, so the CI check is stable on one driver. The golden guards
against gross breakage (garbage taps, NaNs, a wrong copy region); the cap, the masks and the
bounds are unit-tested on the CPU twin (`moba_render::distortion`, `telegraph_mark`).

**Readability check (Ember crystal, High, on vs no-draw).** 3,049 pixels change, in the
vents' haze around the crystal; the crystal (a prop, prepass ID 8) keeps its pixels: the
unit mask keeps every unit's and prop's silhouette in place.

## Reproduce

```bash
# golden + evidence captures (validation on)
MOBA_VFX_PREVIEW=cinder-zone MOBA_QUALITY=ultra MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90 \
MOBA_CAPTURE=render-distortion-actual.ppm MOBA_FRAME_LOG=render-distortion.frames \
MOBA_VULKAN_VALIDATION=required ./target/release/moba-client
./target/release/image-check render-distortion-actual.ppm \
  --expected tests/golden/render-distortion-fingerprint.rgb
MOBA_BLOOM=off MOBA_VFX_PREVIEW=cinder-zone MOBA_QUALITY=ultra MOBA_SMOKE_FRAMES=120 \
MOBA_CAPTURE_FRAME=90 MOBA_CAPTURE=render-distortion-flat.ppm ./target/release/moba-client
MOBA_BLOOM=off MOBA_DISTORTION=nodraw MOBA_VFX_PREVIEW=cinder-zone MOBA_QUALITY=ultra \
MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90 MOBA_CAPTURE=render-distortion-nodraw.ppm \
./target/release/moba-client
./target/release/image-check render-distortion-flat.ppm --against render-distortion-nodraw.ppm \
  --min-changed 5000 --same-box 640,448,684,505 --same-box 700,245,732,300 --diff haze.ppm
# timings (validation off), with and without MOBA_DISTORTION=nodraw
MOBA_VFX_PREVIEW=cinder-zone MOBA_QUALITY=high MOBA_SMOKE_FRAMES=90 MOBA_VSYNC=0 \
MOBA_PIPELINE_CACHE=off MOBA_FRAME_LOG=/tmp/r19.frames ./target/release/moba-client
```
