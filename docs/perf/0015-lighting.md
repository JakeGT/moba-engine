# Lighting: PBR, image-based lighting and shadows — cost and evidence (R2a)

Captured 2026-10-29 on the development workstation, not a CI runner, for
[ADR-0011](../adr/0011-art-style-lighting.md).

- GPU: NVIDIA GeForce RTX 5070 Ti (`nvidia_icd.json`, the frame log's `gpu=`); software:
  lavapipe from `/home/jaket/Projects/MOBA/.tools` with the bundled Khronos validation layer.
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-2-cachyos, Wayland. The host and the GPU were
  shared with other agents' builds and runs (see "Noise" below).
- **Before:** `main` at `503bbad` (R1b), exported and built in release. **After:** this branch
  rebased on `6fd8672` (R7a). Both builds ran from their own checkout, so each used its own
  `content/` (before: no `map-grass.toml`, no environments). The timings predate the rebase
  onto R7b (`476305d`); that rebase adds the offscreen views' lighting set and keeps the main
  view's shadow, scene and composite work unchanged, so they were not re-measured. The
  validation runs below were repeated on the rebased branch.

## Per-pass GPU time (RTX 5070 Ti, 1920×1080, vsync off, validation off)

Commands as in [perf/0011](0011-render-graph.md) (`MOBA_FRAME_LOG`, `MOBA_PIPELINE_CACHE=off`),
with `MOBA_QUALITY=<tier>` and, for "off", `MOBA_SHADOWS=off`. Scenes:

- **overview:** networked match, GPU-skinned heroes, whole-map camera
  (`MOBA_SERVER=local MOBA_CAMERA=overview MOBA_HERO_MODELS=sentinel-skinned`, 2,940 frames);
- **player:** the same match from the default 46 m player camera (no `MOBA_CAMERA`);
- **textured:** the overview with every hero drawn by the textured static `sentinel`;
- **local:** the local bot preview (840 frames).

GPU ms, p50 / p95. Ranges are over the clean rounds (see "Noise"); a single value means the
rounds agreed.

| Scene | Build / tier | shadow | scene | composite | ui | total |
|---|---|---|---|---|---|---|
| overview | before (High) | — | 0.027–0.028 / 0.041–0.078 | 0.014 | 0.005 | 0.046–0.048 / 0.072–0.130 |
| | Low (hard, 1024², no specular cube) | 0.004 / 0.006–0.008 | 0.042 / 0.047–0.067 | 0.014 | 0.006 | 0.066 / 0.075–0.112 |
| | Medium (3×3 PCF, 2048²) | 0.006 / 0.007–0.011 | 0.048 / 0.051–0.087 | 0.014–0.016 | 0.006–0.007 | 0.074–0.077 / 0.078–0.159 |
| | High (PCSS 16 + 16, 2048²) | 0.006 / 0.007 | 0.052–0.053 / 0.052–0.058 | 0.014–0.017 | 0.006–0.007 | 0.079–0.082 / 0.083–0.085 |
| | Ultra (PCSS 16 + 32, 4096²) | 0.012–0.045 / 0.013–0.053 | 0.064–0.086 / 0.065–0.093 | 0.014–0.016 | 0.006–0.007 | 0.099–0.152 / 0.101–0.163 |
| | High, `MOBA_SHADOWS=off` | — | 0.042–0.043 / 0.043–0.045 | 0.014–0.016 | 0.006–0.007 | 0.063–0.066 / 0.067 |
| player | before (High) | — | 0.049–0.052 / 0.054–0.110 | 0.014–0.015 | 0.006 | 0.069–0.073 / 0.075–0.164 |
| | Low | 0.005 / 0.006 | 0.079 / 0.080 | 0.015 | 0.006 | 0.106 / 0.107 |
| | High | 0.007 / 0.008 | 0.096–0.097 / 0.097–0.098 | 0.015 | 0.007 | 0.125–0.126 / 0.127–0.128 |
| | Ultra | 0.012–0.013 / 0.013 | 0.106 / 0.107 | 0.015 | 0.007 | 0.139–0.141 / 0.142 |
| textured | before (High) | — | 0.030 / 0.031–0.034 | 0.014–0.016 | 0.005–0.006 | 0.049–0.052 / 0.053 |
| | High | 0.006 / 0.007–0.018 | 0.050–0.052 / 0.052–0.055 | 0.014–0.016 | 0.006–0.008 | 0.076–0.081 / 0.082–0.092 |
| local | before (High) | — | 0.016 / 0.016–0.029 | 0.015 | 0.003 | 0.034 / 0.035–0.046 |
| | Low | 0.004 / 0.004–0.009 | 0.028–0.031 / 0.029–0.033 | 0.014–0.015 | 0.003–0.004 | 0.051–0.052 / 0.052–0.059 |
| | High | 0.006 / 0.006–0.017 | 0.036–0.038 / 0.037–0.040 | 0.014–0.016 | 0.003–0.004 | 0.062 / 0.062–0.074 |

`span` equals `total` within 1 µs in every run (the new shadow → scene barrier costs nothing
measurable). Frame interval p50 is 4.172–4.184 ms in every run of both builds (display-paced
at 240 Hz, as in perf/0011). Pipelines built at creation: 9 with shadows on (the two
`SHADOW_PASS` casters added to R1b's 7), 7 with `MOBA_SHADOWS=off`.

**Reading:**
- **The lit scene costs about 0.02–0.05 ms more on this GPU.** At the overview the scene pass
  goes from 0.027 ms to 0.042 ms (shadows off) and 0.052 ms (High); at the 46 m player camera,
  where units and terrain fill more of the screen, from 0.050 ms to 0.096 ms. The added cost
  is per pixel (GGX, SH, the BRDF LUT, the cube fetch and the shadow taps), so it scales with
  resolution and screen coverage, not unit count.
- **The shadow pass is cheap:** 4–7 µs at 1024²–2048² (a few hundred instanced cubes and the
  skinned heroes, depth only). Ultra's 4096² map costs 12–45 µs: it is 64 MiB of D32 to clear
  and write, and the spread is GPU clock state (below).
- **Filtering:** at the player camera PCSS-lite (High) costs 0.017 ms over Low's single tap,
  and Ultra's 32-tap disk another 0.010 ms. At the overview, where few pixels are in penumbra,
  the tiers differ by under 0.01 ms.
- **Low path:** Low's scene is cheapest (0.042 ms overview, 0.079 ms player) — one shadow tap,
  SH ambient without the cube fetch — and its whole frame (0.066 ms) is within 0.003 ms of High
  with shadows off. Low's budget is the Intel UHD 620 class at 1080p/60 (≤ 14 ms, roadmap §7);
  it has **not** been measured on such a device (no reference hardware, as for R1).
- **Textured materials** cost the same as vertex colour (the overview "textured" row matches
  the skinned overview), as in perf/0013.
- Against the High-tier GPU budget (≤ 6 ms at 1440p on an RTX 3060 class) the measured whole
  frame is ≈ 0.13 ms at 1080p on a much faster GPU; that says nothing about the reference
  GPUs, which remain unmeasured (roadmap §7, #9/#127).

**Noise.** At 0.05–0.15 ms of GPU work per 4.2 ms frame the GPU idles most of the time and its
clock governor (observed at 180 MHz between runs) dominates run-to-run variation; other agents
also used the GPU. The composite pass is identical code in both builds, so a run whose
composite p50 left its usual 0.014–0.017 ms (0.031–0.058 ms in five round-2 overview runs and
0.051 ms in the round-1 textured baseline) was GPU-contended; those runs were discarded and
re-run as round 3. The table uses rounds 1 and 3 for the overview scenes and rounds 1–3 where
clean for the rest. Raw frame logs are not committed; each row is one `MOBA_FRAME_LOG` line.

## Load-time costs

- **BRDF LUT:** 32² `R16G16_SFLOAT`, 128 GGX samples per texel, generated on the CPU in
  `Renderer::new` and uploaded once (4 KiB).
- **Environment:** one 64² `RGBA16F` cube with 7 levels (256 KiB file and VRAM) plus a 1 KiB
  SH descriptor per map, uploaded once per map change (`set_environment` waits for the device
  to idle). Low does not upload the cube. Baking the three shipped environments with
  `env-bake` takes about 15 s in release.
- **Shadow map:** a fixed-size render-graph transient, allocated once with the other
  transients and kept across swapchain resizes (only swapchain-sized slots are reallocated):
  4 MiB (Low, 1024² D32), 16 MiB (Medium/High), 64 MiB (Ultra).
- **Vertices:** static vertices grow from 32 to 48 bytes and skinned ones from 40 to 56 (the
  tangent); tangents missing from a file are generated at registration in O(vertices).

Nothing new is allocated per frame: the shadow fit, the lighting uniform values and the frame
graph's extra barriers are computed into fixed-size values, and the lighting descriptor set is
written only at creation, on resize and on `set_environment`/`set_lighting_ramp`.

## Validation, goldens and captures

- **After the rebase onto R7b** (`476305d`), with validation + synchronisation validation
  required, on **lavapipe** (1280×720) and on the **RTX 5070 Ti**: the default capture, the
  networked skinned capture and the textured sentinel capture on each of the four tiers, plus
  the networked capture with `MOBA_SHADOWS=off`, with `MOBA_IBL=0`, and on the ember and frost
  maps — **0** messages in all 16 runs on each device. Every networked run kept the R7b
  offscreen passes running with shadows on (`MOBA_PORTRAITS=animated MOBA_MINIMAP_BAKE=30`),
  so portrait and minimap frames share frames with the shadow pass.
- Before the rebase the same matrix (without the offscreen passes) was also clean on both
  devices.
- **Goldens:** both fingerprints (`tests/golden/render-fingerprint.rgb`,
  `tests/golden/render-network-fingerprint.rgb`) were rebaselined deliberately from the
  `render-golden.yml` captures on lavapipe after the rebase; the networked frame differed from
  R7b's fingerprint by mean 3.95 / max 34 before the rebaseline (the default scene, which has
  no offscreen views, matched this branch's earlier fingerprint exactly). Two runs gave
  byte-identical PPMs and fingerprints (mean 0.000, max 0).
- **Screenshots:** [screenshots.md](../screenshots.md#r2a-lighting).

## Unit tests added

Shadow fit (covers the visible ground and casters; tight for a close camera; texel-stable while
panning; texel sizes at the client cameras), the split-sum LUT against known values, lighting
sanitising of untrusted map parameters, tier defaults and resolution clamping, the shadowed
frame graph's barriers (depth write → fragment sampled read, one bone-buffer barrier for both
passes), the std140 camera block layout, the 80-byte material block, vertex layouts with
tangents, built-in mesh tangents, the client's lighting overrides and the shadow lane of the
frame log. The asset side adds KTX2 cube/RGBA16F, environment-descriptor, tangent and
`env-bake` tests.
