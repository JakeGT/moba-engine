# Dynamic lights, outlines, baked AO and contact shadows: cost and validation (R2b)

Captured 2026-10-29 on the development workstation, not a CI runner, for
[ADR-0016](../adr/0016-lights-outlines-ao.md).

- GPU: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08 (`nvidia_icd.json`); software: lavapipe
  from `/home/jaket/Projects/MOBA/.tools` with the bundled Khronos validation layer
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-2-cachyos, Wayland
- Build: this branch, release, on `main` at `74f3087`; "before" is `main` at `74f3087` itself
  (`git archive`, built release in a scratch directory). The host was shared with other
  agents' builds; the GPU columns come from timestamp queries and are not affected by host
  load. Frame intervals are display-paced (240 Hz) as in [perf/0011](0011-render-graph.md).

## GPU time before and after (RTX 5070 Ti, 1920×1080, vsync off, validation off)

The R5 evidence scene ([perf/0016](0016-vfx.md)): the scripted 5v5 teamfight replay
`tests/replays/teamfight-5v5.mbr` from tick 330, 9,540 logged frames (the first 60 of 9,600
are warm-up), each tier's defaults. Before and after runs are **interleaved** per tier
(before-low, after-low, before-medium, …) so a GPU clock change hits both, and the whole
matrix was run **3 times**; each cell is the **median of the rounds** (an outlier round
cannot move it), in ms at p50 (p95 below). `cpu` is the client's work before the render
call (effect lights are gathered there), `render` the time inside it (light binning,
uploads, submission, present). `after-ultra-off` sets all four R2b switches off;
`x8` is `MOBA_VFX_SCALE=8` (more particles; effect lights are per effect instance, so the
light count is the same).

| Run | GPU total | shadow | depth_prepass | contact_shadows | scene | vfx_draw | outline | cpu | render | lights max |
|---|---|---|---|---|---|---|---|---|---|---|
| before-low | 0.086 / 0.093 | 0.005 / 0.006 | — | — | 0.050 / 0.052 | 0.004 / 0.006 | — | 0.008 / 0.025 | 0.065 / 0.132 | — |
| after-low | 0.097 / 0.103 | 0.005 / 0.006 | 0.000 / 0.000 | 0.000 / 0.000 | 0.060 / 0.062 | 0.004 / 0.006 | 0.000 / 0.000 | 0.008 / 0.025 | 0.095 / 0.258 | 11 |
| before-medium | 0.111 / 0.118 | 0.006 / 0.007 | — | — | 0.057 / 0.059 | 0.010 / 0.012 | — | 0.008 / 0.023 | 0.072 / 0.137 | — |
| after-medium | 0.113 / 0.124 | 0.006 / 0.007 | 0.000 / 0.000 | 0.000 / 0.000 | 0.063 / 0.068 | 0.009 / 0.012 | 0.000 / 0.000 | 0.008 / 0.025 | 0.101 / 0.220 | 11 |
| before-high | 0.119 / 0.128 | 0.006 / 0.007 | — | — | 0.067 / 0.070 | 0.009 / 0.011 | — | 0.008 / 0.025 | 0.077 / 0.146 | — |
| after-high | 0.223 / 0.234 | 0.006 / 0.007 | 0.014 / 0.014 | 0.052 / 0.053 | 0.071 / 0.074 | 0.009 / 0.013 | 0.032 / 0.033 | 0.008 / 0.021 | 0.146 / 0.249 | 11 |
| before-ultra | 0.144 / 0.236 | 0.011 / 0.013 | — | — | 0.077 / 0.102 | 0.010 / 0.015 | — | 0.007 / 0.023 | 0.069 / 0.145 | — |
| after-ultra | 0.351 / 0.367 | 0.038 / 0.046 | 0.014 / 0.017 | 0.091 / 0.092 | 0.077 / 0.087 | 0.034 / 0.040 | 0.032 / 0.033 | 0.007 / 0.024 | 0.160 / 0.357 | 11 |
| after-ultra-off | 0.215 / 0.229 | 0.030 / 0.039 | 0.000 / 0.000 | 0.000 / 0.000 | 0.094 / 0.101 | 0.030 / 0.033 | 0.000 / 0.000 | 0.007 / 0.023 | 0.071 / 0.123 | 0 |
| before-high-x8 | 0.116 / 0.139 | 0.006 / 0.007 | — | — | 0.064 / 0.068 | 0.013 / 0.025 | — | 0.008 / 0.026 | 0.081 / 0.149 | — |
| after-high-x8 | 0.225 / 0.243 | 0.005 / 0.006 | 0.013 / 0.014 | 0.051 / 0.052 | 0.078 / 0.086 | 0.012 / 0.024 | 0.032 / 0.032 | 0.008 / 0.024 | 0.175 / 0.359 | 11 |

```bash
MOBA_QUALITY=<tier> MOBA_WINDOW_SIZE=1920x1080 MOBA_REPLAY=tests/replays/teamfight-5v5.mbr \
MOBA_WARMUP_TICKS=330 MOBA_SMOKE_FRAMES=9600 MOBA_VSYNC=0 MOBA_FRAME_LOG=/tmp/r2b.log \
[MOBA_LIGHTS=off MOBA_OUTLINES=off MOBA_BAKED_AO=off MOBA_CONTACT_SHADOWS=off] \
cargo run --release -p moba-client
```

Cells are GPU ms p50 / p95 (`cpu` and `render` are CPU ms); `lights max` is the most live
effect lights in any frame of any round (`Renderer::light_stats`, binned after culling).

**Reading:**
- **Every tier stays far inside its §7 GPU budget.** At 1080p the frame is 0.097 ms of GPU
  work at p50 on Low, 0.113 ms on Medium, 0.223 ms on High and 0.351 ms on Ultra, with p95
  at most 0.367 ms, against budgets of 6–14 ms.
- **Low and Medium: +0.011 and +0.002 ms.** They have no new pass; the cost is in `scene`
  (+0.010 / +0.006 ms): the light loop, the baked AO fetch and the hull outline draws. The
  light loop costs almost nothing while a column holds no light, which is most of the screen.
- **High: +0.104 ms**, almost all in the three new passes: `depth_prepass` 0.014 ms,
  `contact_shadows` (8 steps, full resolution) 0.052 ms and `outline` (8-direction
  dilation, full resolution) 0.032 ms. `scene` itself grows by only 0.004 ms, plausibly
  because the loaded pre-pass depth keeps hidden fragments from being shaded.
- **Ultra: +0.136 ms against `after-ultra-off`** (the same build with the four switches off):
  `contact_shadows` at 16 steps is 0.091 ms. Rounds 2 and 3 of `after-ultra` and
  `after-ultra-off` ran with the GPU in a slower clock state (their `shadow` and
  `vfx_draw`, which R2b does not touch, are three times round 1's and `before-ultra`'s, as in
  perf/0016), so for Ultra compare against `after-ultra-off`, which shares that state, not
  against `before-ultra`.
- **Contact shadows are the most expensive feature** (half of High's increase). If a
  reference GPU needs it, the march can run at half resolution or drop to 4 steps on High;
  `MOBA_CONTACT_SHADOWS=<steps>` makes that a tuning knob, not a code change.
- **CPU.** The work before the render call is unchanged (`cpu`, 0.007–0.008 ms at p50: the
  effect lights are gathered with the effect update, into fixed buffers). The render call
  grows by 0.03 ms at p50 on Low/Medium (light binning, the 68 KiB grid written into the
  mapped buffer) and by 0.07–0.09 ms on High/Ultra (also recording the pre-pass, contact and
  outline passes); with every switch off it is back to `main`'s 0.07 ms. The render call's
  p95 is about twice its p50 on every row after the change (0.22–0.36 ms), still far below a
  millisecond.
- **`x8`** (about 8× the particles) changes nothing R2b adds: lights are per effect
  instance, and the outline and contact passes do not see particles.
- **Frame intervals** are display-paced at 4.17–4.19 ms p50 in every run, before and after;
  a few runs on both sides have a frame-interval p95 of 4.6–5.8 ms in one of the three
  rounds (host load from other agents' builds), which the median discards.

## Validation and synchronisation validation

`MOBA_VULKAN_VALIDATION=required` (the Khronos layer with synchronization validation
enabled by the renderer) on both devices, with animated 3D portraits and a minimap re-bake
every 30 frames (`MOBA_PORTRAITS=animated MOBA_MINIMAP_BAKE=30`) on the local, networked
and replay runs. Every run exits 0 with **0 validation messages**:

| Run (per tier: Low, Medium, High, Ultra) | lavapipe | RTX 5070 Ti |
|---|---|---|
| local preview, capture at frame 90 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 |
| network (`MOBA_SERVER=local`, skinned heroes, ember map with its AO bake, `MOBA_VFX_SCALE=8`), capture at tick 120 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 |
| network, frost map with its AO bake, tick 120 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 |
| teamfight replay, 600 frames from tick 330, `MOBA_VFX_SCALE=8` (up to 8–9 live lights, ~960 particles) | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 |

Switch and override runs (network ember frame unless noted), each 0 messages on both devices:
High with all four switches off; Ultra all off on the teamfight replay; Low with lights,
outlines and AO off; Low with `MOBA_OUTLINES=screen` (adds the pre-pass on Low); Ultra with
`MOBA_OUTLINES=hull`; High with `MOBA_OUTLINES=off`; High with `MOBA_CONTACT_SHADOWS=off`
(outlines keep the pre-pass); Medium with `MOBA_CONTACT_SHADOWS=32` (adds the pre-pass on
Medium); High with `MOBA_LIGHTS=256` on the teamfight replay; High with `MOBA_BAKED_AO=off`.
The before/after screenshot captures (including the effect previews `cinder-zone` and
`nova` and the colour-blind palette) and both golden captures were also validation-clean on
lavapipe.

The graph's new access kind (`DepthReadWrite`: the scene loads and tests the pre-pass depth)
and the pre-pass → contact shadows → scene and composite → outline → UI orders have headless
barrier tests (`crates/render/src/frame_graph.rs`); light binning (caps, list-order drops,
per-cell overflow, culling of NaN, far and black lights) is unit-tested in
`crates/render/src/lights.rs`.

**Light caps under load.** The shipped content keeps at most 8–9 effect lights alive in the
teamfight at 1280×720 (p50 1, p95 5) and 11 at 1920×1080 (a wider ground footprint), under
Low's cap of 16; no run hit a global or per-cell cap (`Renderer::light_stats`). The cap paths are covered by the unit tests, not by a scene.

## Goldens

Both `render-golden.yml` captures, run as the workflow does (lavapipe, validation and sync
validation required, release build, the default tier High) without `xvfb-run` (not
installed; the capture is the swapchain image either way). First, `main` at `74f3087`
reproduces the checked-in fingerprints here exactly (`mean_abs_diff=0.000 max_diff=0` for
both), so this machine's lavapipe matches CI's. This branch then differs from them:

| Golden | vs the old fingerprint | what changed |
|---|---|---|
| local (frame 90) | `mean_abs_diff=0.031 max_diff=23` | hero outlines (screen-space), contact shadows at units' feet |
| networked (tick 120) | `mean_abs_diff=0.024 max_diff=9` | the own hero's outline, its contact shadow |

Two runs of each capture were **byte-identical** (local
`sha256 f0d3ed7a…5b1c1da`, networked `3c68bd3c…367a645a`), and both fingerprints
(`tests/golden/render-fingerprint.rgb`, `render-network-fingerprint.rgb`) were rewritten with
`image-check --write` from the first run; the second run then matches them with
`max_diff=0`. The default grass map has no walls or props, so it has no AO bake and the AO
term is 1 in both goldens; no effect is live at frame 90 or tick 120, so no light is either.

## Baked AO

`scripts/bake-map-ao.sh` runs `moba-tools ao-bake` for the two maps with static occluders.
Each bake is 512 × 512 texels (0.5 m) of `R8_UNORM`, 262,332 bytes of KTX2 plus a 353-byte
sidecar; the client loads and uploads it once per map bind. Re-running the bake gives
byte-identical files (the jitter is a hash of the texel, not a random stream).

| Map | fingerprint | bake time (release) |
|---|---|---|
| ember (`content/ember-map.toml`, `maps/ember.toml`) | `0x1abaf6e3147d9360` | 0.47 s |
| frost (`content/frost-map.toml`, `maps/frost.toml`) | `0x01a84e44294da8b8` | 0.45 s |

The frost overview with AO off and on differs on 1.4 % of the pixels, by at most 22 of 255
([screenshot](../screenshots.md#r2b-lights-outlines-baked-ao-and-contact-shadows)): the
ambient darkens at the wall and prop bases and inside the wall square. It multiplies only
the ambient line, so in direct sun it is subtle by design. A bake whose fingerprint does not
match the map the client bound (another pack reusing the visuals name) is refused with one
`[assets] baked AO '<name>': stale …; no baked AO` line, tested in
`crates/client/src/app/map_visuals.rs`.

## Readability review (R2 exit; for art-direction sign-off)

The R2 exit asks for "a readability review signed off by art direction". The evidence is
prepared here and in [screenshots.md](../screenshots.md#r2b-lights-outlines-baked-ao-and-contact-shadows);
**the sign-off itself is a maintainer item** (there is no art director on the project).
Checked against ADR-0011's readability rules:

1. **Team colour survives the lighting.** Lights add light and never replace albedo; the
   cinder and nova previews keep the red and blue units' hues inside the light pools.
   Outlines draw outside the silhouette only, so the unit's own pixels are untouched.
2. **Shadows never black out a unit.** Contact shadows multiply the shadow term *before* the
   map's shadow-strength floor, and baked AO only the ambient line, so a unit in contact
   shadow and AO keeps at least the floor's light.
3. **Outline colours are the HUD's.** On High (screen-space, after the tonemap) the own-hero
   outline measures exactly the palette colour: standard `#59DB52` → (89, 219, 82) on screen,
   colour-blind `#F0E442` → (240, 228, 66). On Low (inverted hull, before the tonemap) it
   measures (90, 206, 82): the tonemap darkens the green channel by 13 of 255, still clearly
   the own-hero green next to the same-coloured health bar.
4. **No hidden information.** Outlines and lights only exist for units and effects the client
   already has from the server, and both are depth-tested: no outline through walls, brush
   or fog (no "x-ray", by design, ADR-0016).
5. **Clutter.** Team outlines are on heroes only (minion waves are not outlined); hover
   (the enemy a click there would attack, `nearest_enemy`) and the attack target are wider
   and lighter.

Open for the reviewer: outline widths (1.5 px team, 2.5 px hover, 3 px target at 1080p),
whether allied heroes should be outlined at all, the per-map `ao_strength` (0.85 ember,
0.8 frost) and effect light intensities.

## Review round 1, on `main` with R4b and R3a

Captured 2026-10-30, same machine and method, after the review fixes and the merges of R4b
(#332) and R3a (#333): "before" is `main` at `760bd01`, "after" this branch merged with it.
R6 (#334) has not landed; its merge is measured separately.

### GPU and CPU time (RTX 5070 Ti, 1080p, teamfight replay, median of 3 interleaved rounds)

| Run | GPU total | shadow | depth_prepass | contact_shadows | scene | vfx_draw | outline | cpu | render | lights max |
|---|---|---|---|---|---|---|---|---|---|---|
| before-low | 0.115 / 0.118 | 0.004 / 0.004 | — | — | 0.075 / 0.078 | 0.003 / 0.005 | — | 0.005 / 0.026 | 0.068 / 0.118 | — |
| after-low | 0.127 / 0.132 | 0.004 / 0.004 | 0.000 / 0.000 | 0.000 / 0.000 | 0.086 / 0.090 | 0.003 / 0.005 | 0.000 / 0.000 | 0.006 / 0.026 | 0.132 / 0.258 | 10 |
| before-medium | 0.188 / 0.209 | 0.005 / 0.006 | — | — | 0.121 / 0.141 | 0.008 / 0.015 | — | 0.006 / 0.027 | 0.075 / 0.132 | — |
| after-medium | 0.212 / 0.237 | 0.005 / 0.006 | 0.000 / 0.000 | 0.000 / 0.000 | 0.131 / 0.159 | 0.008 / 0.015 | 0.000 / 0.000 | 0.006 / 0.029 | 0.137 / 0.264 | 10 |
| before-high | 0.177 / 0.190 | 0.005 / 0.006 | — | — | 0.118 / 0.123 | 0.009 / 0.016 | — | 0.006 / 0.029 | 0.074 / 0.131 | — |
| after-high | 0.338 / 0.355 | 0.005 / 0.006 | 0.019 / 0.024 | 0.051 / 0.051 | 0.169 / 0.186 | 0.009 / 0.014 | 0.032 / 0.032 | 0.006 / 0.030 | 0.139 / 0.291 | 10 |
| before-ultra | 0.464 / 0.483 | 0.050 / 0.056 | — | — | 0.242 / 0.260 | 0.074 / 0.082 | — | 0.006 / 0.030 | 0.074 / 0.131 | — |
| after-ultra | 0.766 / 0.805 | 0.052 / 0.058 | 0.096 / 0.107 | 0.091 / 0.095 | 0.312 / 0.340 | 0.060 / 0.068 | 0.032 / 0.038 | 0.006 / 0.031 | 0.111 / 0.269 | 10 |
| after-ultra-off | 0.441 / 0.464 | 0.051 / 0.057 | 0.000 / 0.000 | 0.000 / 0.000 | 0.231 / 0.245 | 0.069 / 0.078 | 0.000 / 0.000 | 0.006 / 0.029 | 0.075 / 0.173 | 0 |
| before-high-x8 | 0.181 / 0.201 | 0.005 / 0.006 | — | — | 0.118 / 0.123 | 0.014 / 0.026 | — | 0.006 / 0.030 | 0.073 / 0.135 | — |
| after-high-x8 | 0.343 / 0.368 | 0.006 / 0.006 | 0.019 / 0.022 | 0.051 / 0.051 | 0.173 / 0.190 | 0.013 / 0.022 | 0.032 / 0.032 | 0.006 / 0.031 | 0.131 / 0.277 | 10 |

- **Every tier is still far inside its budget**: at most 0.77 ms of GPU work at p50 (Ultra),
  against 6–14 ms.
- **Low +0.012 ms, Medium +0.024 ms** (`scene`: light loop, baked AO fetch, hull outlines).
- **High +0.161 ms**, where it was +0.104 ms before R3a. The three passes cost what they
  did (`depth_prepass` 0.019, `contact_shadows` 0.051, `outline` 0.032 ms); the change is
  `scene`, +0.051 ms instead of +0.004 ms. Switching R2b's features off one at a time on High
  (4,800 frames, one run each) isolates it: `scene` is 0.176 ms with everything on, 0.163
  with lights off, 0.154 with baked AO off, **0.127 with contact shadows off** and 0.125 with
  all four off, against 0.118 ms on `main`. On R2a's flat terrain (`MOBA_TERRAIN=flat`) the
  same build's `scene` is 0.071 ms, as before R3a. So the cost is R2b's terms, mainly the
  contact-shadow read, on top of R3a's splat shader (several array fetches per layer,
  triplanar on slopes), not the pre-pass: the pre-pass does not make `scene` cheaper on this terrain either
  (0.127 ms with it, 0.129 ms without). The plan in ADR-0016 §6 moves the contact term into
  R6's half-resolution AO target, so the scene reads one texture it reads anyway; that
  removes the `contact_shadows` pass and this extra `scene` time together.
- **Ultra +0.325 ms against `after-ultra-off`.** The Ultra rows ran in the GPU's slower
  clock state in all three rounds (`shadow` 0.050 ms on both sides, 0.011 ms in round 0's
  fast state), so compare them with each other. The pre-pass is 0.096 ms here against
  0.019 ms on High: it now draws R3a's Ultra heightfield (384² cells, about 295,000
  triangles, against 256² on High); with flat terrain it is 0.015 ms. A coarser terrain
  LOD for the pre-pass is the lever, if a reference GPU needs it; R6's GTAO will need the
  same depth, so the cost is shared after that merge, not doubled.
- **CPU.** The work before the render call is unchanged (0.006 ms). The render call grows by
  0.04–0.06 ms at p50 on every tier: the 68 KiB grid copy into mapped memory, which happens
  even when few lights are live (binning itself is microseconds, below). Its p95 is about
  twice its p50 (0.26–0.29 ms), still well under a millisecond.
- **Lights**: at most 10 live in the teamfight at 1080p, under every tier's global cap.

### Light binning on the CPU (criterion, `cargo bench -p moba-render --bench light_binning`)

| Case | time per frame |
|---|---|
| 256 lights of 4 m over the player camera's footprint (the global cap) | 6.3 µs |
| 256 lights of 24 m (the largest effect radius; most columns at the 16-light cap) | 33.6 µs |
| 16 lights of 4 m with Low's caps (16, 8 per column) | 0.80 µs |

The worst case by work is 0.034 ms of one core, a fraction of the render call's own p50.

### Readability under effect lights (the 1 m near-field floor and the 1.5× sun cap)

A unit stood a metre behind the light (`MOBA_OUTLINE_PROBE=hit`), High, lavapipe; mean
colour over the unit's 4,700 pixels, HSV saturation and the share of near-white pixels
(every channel above 225):

| Effect (frame) | lights off: mean RGB, sat | before (0.25 m² floor, no cap) | after (1 m² floor, 1.5× sun cap) |
|---|---|---|---|
| judgement-hit-3 | (193, 135, 125), sat 0.36, 0 % near-white | (227, 167, 142), sat 0.38, 0 % near-white | (227, 164, 138), sat 0.39, 0 % near-white |
| collapse-4 | (225, 195, 175), sat 0.23, 1 % near-white | (241, 205, 182), sat 0.25, 2 % near-white | (240, 204, 181), sat 0.25, 1 % near-white |
| collapse-8 | (229, 197, 172), sat 0.25, 0 % near-white | (239, 202, 177), sat 0.26, 0 % near-white | (239, 201, 176), sat 0.27, 0 % near-white |
| arc-hit-3 | (192, 134, 127), sat 0.36, 0 % near-white | (210, 161, 162), sat 0.26, 2 % near-white | (209, 158, 160), sat 0.26, 0 % near-white |
| nova-8 | (198, 142, 145), sat 0.32, 0 % near-white | (222, 157, 173), sat 0.31, 1 % near-white | (221, 156, 172), sat 0.31, 0 % near-white |
| hero-death-6 | (197, 147, 138), sat 0.30, 0 % near-white | (215, 158, 147), sat 0.32, 0 % near-white | (213, 157, 146), sat 0.32, 0 % near-white |
| cinder-zone-20 | (193, 134, 124), sat 0.37, 0 % near-white | (222, 149, 129), sat 0.42, 0 % near-white | (222, 147, 128), sat 0.43, 0 % near-white |

The unit brightens and keeps its saturation in every case; the near-white share falls to
at most 1 % (it was up to 2 % with the old falloff).

### Relief AO × baked AO on R3a's terrain

R3a darkens the terrain's albedo by its relief occlusion; R2b's baked AO multiplies the
ambient line. Per pixel, baked AO off against on (High, lavapipe, the 3D view between the
HUD panels), networked at tick 120 on the hero close-up (24 m) and the whole-map overview:

| View | pixels baked AO changes | largest darkening (of 255) | darkest changed pixel, AO off → on | darkest pixel of the frame, off → on |
|---|---|---|---|---|
| ember | 0.0 % | 0 | 1000000000 → 1000000000 | 22 → 22 |
| ember-ov | 0.4 % | 6 | 75 → 70 | 5 → 5 |
| frost | 0.0 % | 0 | 1000000000 → 1000000000 | 49 → 49 |
| frost-ov | 2.8 % | 15 | 133 → 131 | 3 → 3 |

Baked AO darkens at most 15 of 255, on 2.8 % of the frost overview's pixels (wall and prop
bases), and never changes the frame's darkest pixel, which is R3a's relief and the sun
shadow. The close-ups have no wall base in view. The stack is not over-dark, so no floor on
the product was added. R6's GTAO, the third factor, is checked on that merge.

### Validation, goldens and the outline probe

`MOBA_VULKAN_VALIDATION=required` (validation and synchronisation validation) after the
merge. On the RTX 5070 Ti, the matrix of the first section (16 tier runs and the 10 switch
and override runs), the VAT minions through the pre-pass and the hull (Low, High, Ultra,
Ultra with hull outlines) and five R3a views (the High overview with the fog of war, the
Ultra and Low horizon with sky and map-edge cliffs, High with `MOBA_FOW=legacy`, High with
`MOBA_TERRAIN=flat`): **35 runs, 35 exit 0 with 0 messages**. On lavapipe, the VAT and R3a
sets: **9 runs, 9 clean**, plus the six golden captures twice, the four probe cases and the
ten AO-stack captures above, all clean.

All six render goldens (default, networked, VAT, ember, R3a's overview and horizon) were
regenerated from this merge: two lavapipe runs per capture are byte-identical, with no
validation or sync-validation message. The outline occlusion probe
(`scripts/outline-probe.sh`) passes in all four cases (a unit behind a wall draws no
outline pixel with the hull on Low or the screen-space pass on High; an open unit does).
Both bakes were re-run after the merge. Ember's fingerprint changed to `0x1abaf6e3147d9360`
because R3a moved a crystal off the mid lane (from (128, 128) to (121, 138)), and its
texture changed where the crystal was and is. Frost's inputs, and so its fingerprint, did not
change, but its texture moved by 1–8 levels of 255 around the wall square (a few texels at
wall ends by up to 28). The likely cause (not isolated): `ao-bake` now places walls with
`moba_assets::wall_box`, whose `cos(atan2)` axis differs from the old normalised direction in
the last bit, and the axis-aligned walls' faces lie exactly on texel centres, so a row of
texels along a face can flip in or out of the wall. Both are within the bake's 0.5 m texel; the client draws the
walls with the same `wall_box`.

## Not measured

- The first table is this branch on `main` at `74f3087`; [the review round](#review-round-1-on-main-with-r4b-and-r3a)
  re-measures it on `main` at `760bd01` (R4b and R3a). R6 has not landed: the integration
  with its pre-pass, MSAA and GTAO (ADR-0016 §6) changes where the pre-pass cost is counted
  and adds an MSAA ID target on Ultra, and is re-measured on that merge.

- The other §7 reference GPUs (UHD 620, GTX 1060, RTX 3060 classes, Apple M1) are still
  unowned (#9/#127); every number above is the RTX 5070 Ti at 1080p.
- No shipped scene reaches a light cap. The 256-light CPU worst case is benchmarked (the
  review round); the GPU fragment loop at 16 lights per cell is bounded by construction.
