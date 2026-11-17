# The material model on Tiered (N2c)

Captured 2026-11-17 for ADR-0023 (N2c) and its
[Amendment 3](../adr/0023-frame-architecture-materials.md#amendment-3-2026-11-17-n2c-the-material-model-as-built-and-the-frozen-material-spec).

- **GPU:** NVIDIA GeForce RTX 5070 Ti (driver 620.70.06), used for timings and validation.
- **Software:** lavapipe (Mesa 26.2.5 llvmpipe, LLVM 23.1.3) with the Khronos validation layer
  and `MOBA_VULKAN_VALIDATION=required` (validation and synchronisation validation).
- **CPU:** AMD Ryzen 7 9800X3D. **OS:** Linux 7.3.4-1-cachyos, Wayland with an output scale of
  1.5. The lavapipe goldens and parity ran under XWayland with `WINIT_X11_SCALE_FACTOR=1`, so a
  640×360 or 1280×720 window is that many pixels, as on CI's Xvfb. Under Wayland the same
  windows are 960×540 and 1920×1080, and the HUD no longer matches the committed fingerprints:
  `ceca947` itself then fails the `low` fingerprint (mean 2.21, max 97, all in the HUD).
- **Toolchain:** rustc stable, the pinned `slangc`.
- **Branch:** `feat/n2c-material-model` at `7106fb2`, on origin/main `ceca947` (N2b).
- **Baselines:** detached release builds in their own temporary worktrees, deleted after the
  review's measurements: `ab42718` (the N1 tiered forward frame, ADR-0023 §9b's baseline) and
  `ceca947` (N2b).
- **Evidence:** the frame logs of every timing run below (the first build's and the review's),
  with the scripts that ran them, are kept locally in
  `~/Projects/MOBA/handoff/evidence/n2c-ab/` (not committed).

Every lavapipe and RTX run below printed no `[vulkan` line.

## Summary

| Gate | Result |
|---|---|
| 1. Encoding and interfaces | **Met.** `MaterialRecord` and `MaterialPush` against the Slang reflection; the GLSL-vertex / Slang-fragment interface test; the record's model word; the Rust twin and `gbuffer-smoke` cover all eight models |
| 2. Deferred–forward parity | **Met** locally at 1280×720: worst pixel 3, 3, 3 and 6/255 on the review's head (default, default without the sky, river, teamfight), `STANDARD` and `UNLIT` only (amendment 3 item 22); CI below |
| 3. White furnace | **Met** (CPU model of the shader's fetch): worst 0.9861 over roughness 0–1 and N·V 0.05–1 |
| 4. Hidden information (decals) | **Partly:** with bloom off, every unit pixel keeps its G-buffer albedo with decals on and off (35,010 other pixels change); the `Paint` pass is not isolated from the glow decals; see below |
| 5. Goldens | **Met.** Compat byte-identical (11 scenes); `materials` and `decals` tiered goldens two-run identical; exact hashes from CI |
| 7. Performance (≤ 1.10× N1) | **Met** on the teamfight, every preset: GPU total 1.036–1.073× `ab42718` (GPU span 1.053–1.086×), 1.010–1.020× `ceca947`; Compat 0.999×; least headroom Tiered Low, 0.0138 ms. **Extended-model pixels reach the gate:** the material sheet is 1.102–1.117× N1's frame without the spheres (6.9–14.1 % of tiles extended; about 1.10× an estimate of N1 drawing them). The overview 0.991× |
| 8. Compat contract | **Met.** `content-check` passes every shipped material under both evaluators; a test builds each model's sample on Compat |

## Gate 1: encoding and interfaces

- **`material_record_and_push_match_the_slang_layout`** compiles `unit_material_gbuffer.slang`
  with `slangc -reflection-json` and compares every member offset of `MaterialRecord` (64 bytes)
  and `MaterialPush` (32 bytes, through the push block's `table` pointer) with the Rust structs.
- **`material_record_packs_the_model_word`** pins the model word for `STANDARD`
  (`128 | 255 << 8 | 1 << 24 | 1 << 28`: F0 0.04, AO 1, the model, the mask flag), thin film
  and anisotropy at −π/2 (wrapped into 0..π).
- **`glsl_vertex_and_slang_fragment_interfaces_match`** reflects the SPIR-V of
  `unit_textured_gbuffer.vert`, `skinned_textured_gbuffer.vert` and the four
  `unit_material_gbuffer` permutations: every fragment input location has a vertex output of
  the same type, integer inputs are `Flat`, and the fragment reads locations 0, 1, 3, 4, 14
  and 15 (and 13 in the `_ids` permutations).

## Gate 2: deferred–forward parity

`scripts/deferred-parity.sh` at 1280×720 (High, TAAU Quality, `MOBA_STYLISATION=off`,
`MOBA_FOLIAGE=off`), locally on lavapipe, 0 validation messages. Pixels past 2/255 of 921,600:

| Scene | Within 2/255 | Worst | Past 2/255 |
|---|---|---|---|
| default | 100.000 % | 3 | 1 (3/255) |
| default, `MOBA_SKY=off` | 100.000 % | 3 | 1 |
| R3b river | 100.000 % | 3 | 3 |
| R3b teamfight | 99.999 % | 6 | 7 (five at 3, one at 4, one at 6) |

These are the review's head (re-run after the review's changes: the read-only depth, the
front-face culling, the end-point LUT, the hoisted model terms); the first build read 2, 2, 2
and 7 pixels past 2/255 with the same worst values.

One fix made the teamfight pass. Before it, the worst pixel was 28/255: splat droplets outside
the forward decal grid's cell drew only in the deferred frame. The decal pass now reads the
grid (amendment 3 item 16).

The 6/255 pixel is on a unit's edge inside a glow decal's ring (N2b's teamfight worst was
5/255). CI's `tiered-temporal` check runs the same script on the PR's head; its numbers are
in the PR.

## Gate 3: the white furnace

`white_furnace_with_multiscatter_is_one` integrates the GGX lobe (4,096 samples, F0 = 1) at
roughness 0 to 1 in steps of 0.05 and N·V 0.05 to 1. It applies the shader's compensation
`1 + F0 (1/E − 1)`, with E read from Tiered's `R16G16_SFLOAT` LUT the way `brdf_lut_sample`
reads it. The worst value is **0.9861** (gate: 1 ± 0.02). With the 128-sample LUT and the
clamped fetch it was 0.93. This is a CPU model of the fetch, not a rendered furnace: no scene
renders a uniform white environment.

## Gate 4: hidden information (decals)

The teamfight replay with `MOBA_DEBUG_VIEW=albedo` (the G-buffer's base colour, nearest, before
lighting), Native mode at 1280×720 on lavapipe, with decals on and off, and
`MOBA_DEBUG_VIEW=flags` for the `UNIT` mask. Pixels of the pure unit colour in the flags view
number 38,813: eight units and a structure.

- With `MOBA_BLOOM=off`, **all 38,813 keep every albedo byte**, while 35,010 other pixels
  change (up to 239/255): the decals are in the view, and the units are drawn over them.
- With bloom on, three units and the structure keep every byte; the five units inside or at
  the edge of a glow decal's ring differ on 830 pixels by at most 1/255 (775 of them on the two
  units inside the rings). That is the glow's bloom on top of the debug view, not `gb0`.

`MOBA_DECALS=off` also removes the `Glow` decals, which the overlay draws over the debug view,
so this does not isolate the `Paint` pass. By construction the units overwrite `gb0` after the
decals (§7a). The full gate, with rims, is N2e's.

## Goldens and probes

- **Compat is byte-identical** to `ceca947` on lavapipe, compared with `cmp`:
  - the Compat scenes of `render-golden.yml` (default, Ultra, Low at 0.75, VAT, horizon,
    river, river Low, river Ultra and the teamfight decals);
  - the textured `sentinel` hero and the skinned sentinel over the local server.

  All 11 are identical, with 0 validation messages. The material sheet also draws on Compat:
  8 spheres, each "drawn with its material".
- **SPIR-V:** 105 of the 115 modules on both sides are byte-identical.
  - Compat's one change is `unit_textured.frag`, which reads the emissive mask (amendment 3
    item 2). The N2c samples carry a mask (their MR maps are `rgba8`, item 18); no material
    in the Compat golden scenes does, so their images do not move.
  - The other nine are Tiered: `deferred_light`, `deferred_light_extended`, `light_classify`,
    the tiered terrain, foliage, water and water-refraction stages, and the two terrain
    G-buffer `_hdr` stages, which have no decal code now.
  - Nine modules are new: the two decal stages, the four `unit_material_gbuffer` Slang
    stages, `unit_tiered`, `unit_textured_tiered` and `terrain_nodecals_tiered`.
  - Four are gone: the GLSL `unit_textured_gbuffer` fragment permutations.
- **Tiered goldens** at 640×360:
  - All twelve cases are two-run byte-identical, with no validation messages: the four
    existing ones and `materials` and `decals`, each with TAAU and FSR 3.1. Both mid-stream
    resets are clean.
  - The eight existing fingerprints still pass (re-run on the review's head: mean ≤ 0.145,
    max ≤ 5), so they are not rewritten.
  - The four new ones were written locally at 640×360. On the review's head the two `decals`
    fingerprints still pass (mean ≤ 0.004, max 1) and are kept; the two `materials` ones
    were rewritten (mean 0.133, max 39 against the first build's): the anisotropic sphere's
    direction now follows its tangent (amendment 3 item 26), the thin film's Fresnel is taken
    at N·V and its AO is read (items 25 and 27), and the LUT's texels moved (item 10).
  - The exact hashes come from CI only: `render-golden.yml` dispatched on `7106fb2` with
    `cpu_sample=true` (run 43992116303), eight sampling legs, its `tiered-hash-sections`
    artefact pasted with both the `cpu=amd` and `cpu=intel` sections of all twelve names,
    sorted with `LC_ALL=C`. Every later commit touches only docs and the hash file.

## Gate 7: performance

Re-measured on the review's head (the review's changes: the `paint` graph variant, the
read-only depth, the culled front faces, the hoisted model terms, the end-point LUT and the
per-record texture flags). The first build's numbers (`7106fb2`) are at the end of this section.

- **Builds:** `ab42718`, `ceca947` and the branch: release builds, the first two in detached
  worktrees.
- **Machine:** the RTX 5070 Ti (driver 620.70.06, Linux 7.3.4).
- **Replay:** the teamfight replay (`MOBA_WARMUP_TICKS=330`), its `Paint` decals included, at
  1920×1080. Vsync off, the 240 fps limit, validation off, the pipeline cache off, 3,000 frames
  per run.
- **Configurations:** Tiered Low, High and Ultra with TAAU, Tiered Overdrive (Ultra, TAAU
  Performance mode), Tiered Low and High with FSR 3.1, and Compat Ultra; for the review, the
  material sheet at two distances with a no-sheet control each, and the overview.
- **Rounds:** three, interleaved. Each held `flock handoff/gpu.lock` and ran every
  configuration on the three builds in turn, with no other `moba-client` running.
- **Results:** medians of the three rounds, in ms. Within each configuration and build, the
  three GPU totals agree within 0.005 ms.
- **The gate metric is `gpu_total`** (the sum of the pass timestamps), the column perf/0036
  marked as the gate. `gpu_span` (first to last timestamp, so also the gaps between passes) is
  reported beside it and treated as advisory (ADR-0023 amendment 3, gate 7).
- **What the frames held:** the frame log's `paint_decals_avg` (live `Paint` decals per frame)
  and, with `MOBA_TILE_COUNTS=1`, `tiles_standard_avg`, `tiles_extended_avg` and
  `extended_frames` (amendment 3 item 30). The tile counts below come from an otherwise
  identical round with the read-back on; the timings ran without it.

| Configuration | Build | GPU total p50 | p95 | GPU span p50 | `prepass` | `gbuffer_decals` | `gbuffer_rest` | `deferred_light` | ÷ N1 total (span) | ÷ N2b total |
|---|---|---|---|---|---|---|---|---|---|---|
| Tiered Low (TAAU) | `ab42718` | 0.508 | 0.520 | 0.514 | 0.020 | — | — | — |  |  |
|  | `ceca947` | 0.538 | 0.551 | 0.553 | 0.063 | — | — | 0.053 | 1.059 (1.076) |  |
|  | branch | 0.545 | 0.558 | 0.558 | 0.046 | 0.005 | 0.021 | 0.053 | **1.073 (1.086)** | 1.013 |
| Tiered High (TAAU) | `ab42718` | 0.559 | 0.572 | 0.566 | 0.024 | — | — | — |  |  |
|  | `ceca947` | 0.585 | 0.597 | 0.599 | 0.069 | — | — | 0.056 | 1.047 (1.058) |  |
|  | branch | 0.594 | 0.607 | 0.608 | 0.052 | 0.005 | 0.024 | 0.057 | **1.063 (1.074)** | 1.015 |
| Tiered Ultra (TAAU) | `ab42718` | 0.559 | 0.571 | 0.566 | 0.024 | — | — | — |  |  |
|  | `ceca947` | 0.584 | 0.596 | 0.598 | 0.069 | — | — | 0.056 | 1.045 (1.057) |  |
|  | branch | 0.594 | 0.606 | 0.608 | 0.052 | 0.005 | 0.024 | 0.057 | **1.063 (1.074)** | 1.017 |
| Tiered Overdrive | `ab42718` | 0.502 | 0.513 | 0.508 | 0.026 | — | — | — |  |  |
|  | `ceca947` | 0.510 | 0.521 | 0.525 | 0.055 | — | — | 0.036 | 1.016 (1.033) |  |
|  | branch | 0.520 | 0.532 | 0.535 | 0.042 | 0.005 | 0.022 | 0.036 | **1.036 (1.053)** | 1.020 |
| Tiered Low, FSR 3.1 | `ab42718` | 0.656 | 0.668 | 0.662 | 0.020 | — | — | — |  |  |
|  | `ceca947` | 0.687 | 0.700 | 0.701 | 0.063 | — | — | 0.053 | 1.047 (1.059) |  |
|  | branch | 0.694 | 0.707 | 0.707 | 0.045 | 0.005 | 0.021 | 0.054 | **1.058 (1.068)** | 1.010 |
| Tiered High, FSR 3.1 | `ab42718` | 0.707 | 0.720 | 0.713 | 0.024 | — | — | — |  |  |
|  | `ceca947` | 0.730 | 0.744 | 0.746 | 0.069 | — | — | 0.056 | 1.033 (1.046) |  |
|  | branch | 0.741 | 0.754 | 0.755 | 0.052 | 0.005 | 0.024 | 0.057 | **1.048 (1.059)** | 1.015 |
| Compat Ultra | `ab42718` | 0.673 | 0.684 | 0.677 | 0.058 | — | — | — |  |  |
|  | `ceca947` | 0.672 | 0.684 | 0.676 | 0.058 | — | — | — | 0.999 (0.999) |  |
|  | branch | 0.672 | 0.684 | 0.677 | 0.058 | — | — | — | **0.999 (1.000)** | 1.000 |

**Headroom against 1.10× N1** (ms; negative is over), and what the branch's frames held:

| Configuration | 1.10× N1 total | branch total | headroom (ms) | 1.10× N1 span | branch span | headroom (ms) | paint decals | standard tiles | extended tiles |
|---|---|---|---|---|---|---|---|---|---|
| Tiered Low (TAAU) | 0.5588 | 0.545 | +0.0138 | 0.5654 | 0.558 | +0.0074 | 3.280 | 3554 | 0 |
| Tiered High (TAAU) | 0.6149 | 0.594 | +0.0209 | 0.6226 | 0.608 | +0.0146 | 3.280 | 3554 | 0 |
| Tiered Ultra (TAAU) | 0.6149 | 0.594 | +0.0209 | 0.6226 | 0.608 | +0.0146 | 3.280 | 3554 | 0 |
| Tiered Overdrive | 0.5522 | 0.520 | +0.0322 | 0.5588 | 0.535 | +0.0238 | 3.280 | 2039 | 0 |
| Tiered Low, FSR 3.1 | 0.7216 | 0.694 | +0.0276 | 0.7282 | 0.707 | +0.0212 | 3.280 | 3554 | 0 |
| Tiered High, FSR 3.1 | 0.7777 | 0.741 | +0.0367 | 0.7843 | 0.755 | +0.0293 | 3.280 | 3554 | 0 |
| Compat Ultra | 0.7403 | 0.672 | +0.0683 | 0.7447 | 0.677 | +0.0677 | 3.260 | 0 | 0 |
| Sheet, far (High) | 0.6710 | 0.672 | -0.0010 | 0.6776 | 0.688 | -0.0104 | 0.590 | 3310 | 244 |
| Sheet camera, no sheet | 0.6721 | 0.648 | +0.0241 | 0.6776 | 0.662 | +0.0156 | 0.590 | 3554 | 0 |
| Sheet, close (High) | 0.6380 | 0.648 | -0.0100 | 0.6435 | 0.664 | -0.0205 | 0.000 | 2627 | 433 |
| Close camera, no sheet | 0.6369 | 0.621 | +0.0159 | 0.6435 | 0.634 | +0.0095 | 0.000 | 3061 | 0 |

- **Every teamfight preset is within 1.10× N1** on `gpu_total`, and closer to N2b than the
  first build was (1.010–1.020× against 1.012–1.026×). The least headroom is Tiered Low:
  0.0138 ms of total, 0.0074 ms of span. The teamfight has no extended-model pixel
  (`extended_frames=0/2998`) and about 3.3 live `Paint` decals per frame.
- **The split G-buffer still costs 0.007–0.011 ms on the teamfight** over N2b's single pass
  (at Low, `prepass`, `gbuffer_decals` and `gbuffer_rest` total 0.072 ms against N2b's
  0.063 ms). The read-only depth attachment and the culled front faces did not recover it:
  the decal pass is 0.005 ms, and the rest is the second pass boundary (the G-buffer
  attachments are loaded again). The `paint` variant recovers it on frames with no live
  `Paint` decal: the close sheet camera without the sheet, which has none, is 0.621 ms against
  N2b's 0.625 ms.
- **The tile-count read-back costs 0.004–0.005 ms**, which is why it is opt-in. The review's
  first re-measure had it always on and read Low 0.551 ms (1.085×) and High 0.599 ms
  (1.070×); a build without the copy, interleaved with it over three rounds, read 0.545
  against 0.550 at Low and 0.594 against 0.598 at High. Its barriers drain the queue at the
  frame's start.

**Extended-model pixels: the material sheet** (`MOBA_MATERIAL_SHEET=samples`, local preview,
Tiered High with TAAU). `ab42718` and `ceca947` cannot draw these materials (they ignore the
variable), so their rows show the same view without the spheres, and each sheet configuration
has a control with the same camera and no sheet. "Far" is the golden's camera
(`1.5708,0.45,11,0,-12`): 244 of 3,554 tiles extended (6.9 %). "Close" is
`1.5708,0.3,4.5,0,-12`, with the anisotropic and thin-film spheres in the middle: 433 of 3,061
tiles extended (14.1 %).

| Sheet, far (High) | `ab42718` | 0.610 | 0.626 | 0.616 | 0.044 | — | — | — |  |  |
|  | `ceca947` | 0.650 | 0.665 | 0.663 | 0.129 | — | — | 0.052 | 1.066 (1.076) |  |
|  | branch | 0.672 | 0.707 | 0.688 | 0.133 | 0.002 | 0.054 | 0.060 | **1.102 (1.117)** | 1.034 |
| Sheet camera, no sheet | `ab42718` | 0.611 | 0.626 | 0.616 | 0.045 | — | — | — |  |  |
|  | `ceca947` | 0.650 | 0.666 | 0.663 | 0.129 | — | — | 0.052 | 1.064 (1.076) |  |
|  | branch | 0.648 | 0.692 | 0.662 | 0.122 | 0.003 | 0.052 | 0.054 | **1.061 (1.075)** | 0.997 |
| Sheet, close (High) | `ab42718` | 0.580 | 0.595 | 0.585 | 0.044 | — | — | — |  |  |
|  | `ceca947` | 0.626 | 0.640 | 0.639 | 0.124 | — | — | 0.045 | 1.079 (1.092) |  |
|  | branch | 0.648 | 0.663 | 0.664 | 0.128 | — | — | 0.053 | **1.117 (1.135)** | 1.035 |
| Close camera, no sheet | `ab42718` | 0.579 | 0.594 | 0.585 | 0.044 | — | — | — |  |  |
|  | `ceca947` | 0.625 | 0.641 | 0.638 | 0.124 | — | — | 0.045 | 1.079 (1.091) |  |
|  | branch | 0.621 | 0.636 | 0.634 | 0.119 | — | — | 0.047 | **1.073 (1.084)** | 0.994 |

- **A frame with extended-model pixels reaches the gate against N1's frame without them:**
  1.102× (far) and 1.117× (close) on `gpu_total`. The spheres cost the branch 0.024–0.027 ms
  over the same view without them: their G-buffer draws (`prepass`, about 0.010 ms), the
  extended tiles in `deferred_light` (0.006 ms for 433 tiles), and their shadow, AO and
  overlay prepass.
- **No like-for-like N1 number exists.** As an estimate, the branch's forward Tiered frame
  (`MOBA_DEFERRED=off`, three rounds of the close sheet) draws the spheres with their Compat
  fallbacks for 0.005 ms more in its `scene` pass than N1's scene without them, so N1 drawing
  them would be about 0.59 ms, and the deferred frame's 0.648 ms is about 1.10× that.
- So the gate holds on the replay it is defined on, which has no extended model, and is at or
  past 1.10× at High on a view with 7–14 % extended-model pixels. When heroes ship with
  extended models, gate 7 must include them; amendment 3's gate 7 budgets N2d–N2f and opens
  N2d with a recovery step for this reason.

**The overview** (§9b gate 7's decal-overdraw check asks for a wall-heavy view; the shipped
maps have four walls each, so none is): a low orbit across the whole map
(`MOBA_PREVIEW_CAMERA=-0.7,0.12,170`, local preview: terrain, towers and structures, little
wall), Tiered High with TAAU, the same settings, builds and three interleaved rounds, under the
GPU lock:

| Build | GPU total p50 | GPU span p50 | `prepass` | `gbuffer_decals` | `gbuffer_rest` | `deferred_light` | ÷ N1 total (span) |
|---|---|---|---|---|---|---|---|
| `ab42718` | 0.698 | 0.705 | 0.122 | — | — | — | |
| `ceca947` | 0.695 | 0.710 | 0.240 | — | — | 0.036 | 0.996 (1.007) |
| branch | 0.692 | 0.706 | 0.230 | 0.005 | 0.145 | 0.037 | 0.991 (1.001) |

The local preview's bots leave a `Paint` decal live on some frames (0.59 per frame on
average), so the branch splits the G-buffer on those frames and runs N2b's single pass on the
rest; each lane's median is over the frames that ran it. The frame now costs what N2b's does
(the first build, which always split, measured 0.709 ms, 1.016×, and its overview ran without
the GPU lock). **The receivers' pass cannot be read from this view any more** (§9b's
`gbuffer_receivers` against N1's prepass): it is separate only on frames with a live `Paint`
decal, a minority here. On the teamfight, where it always is, the receivers' `prepass` (the
terrain alone) is 0.046 ms at Low and 0.052 ms at High, against N1's whole-scene `prepass` of
0.020 and 0.024 ms, which writes no G-buffer. Walls in view are not measured (amendment 3 item
23), so the ADR's decal-ordering risk stays open.

**The first build** (`7106fb2`, the same method; its logs are in the evidence directory):
Tiered Low 0.547 ms (1.077×, span 1.091×), High 0.596 (1.064×), Ultra 0.596 (1.066×),
Overdrive 0.522 (1.040×), Low with FSR 3.1 0.695 (1.059×), High with FSR 3.1 0.744 (1.054×),
Compat Ultra 0.675 (0.999×); the overview 0.709 ms (1.016×).

- **Before `FOLIAGE` moved to the standard tiles** (amendment 3 item 11), round 1 of the same
  replay measured Tiered Low at 1.108× total and 1.123× span: over the gate. That is why it
  moved.
- **Pipelines:** Tiered 52 (N2b 51, N1 46), FSR 3.1 60 (59, 54), Compat 42 (unchanged). The one
  added is the decal pipeline. The Slang material G-buffer stage replaces the GLSL textured one
  in the same pipeline.
- `locked_uploads=0` and `locked_pipelines=0` in every run.
- **VRAM at High:** 247.5 MB, the same as N2b (N1 236.2). The material table is 4 KiB and
  `gb0`'s sRGB view adds no memory.
- The raw logs are in `~/Projects/MOBA/handoff/evidence/n2c-ab/` (local, not committed).

## Validation on the RTX 5070 Ti

`MOBA_VULKAN_VALIDATION=required` (validation and synchronisation validation), 1280×720
(1920×1080 swapchain), on the branch's release build. Each of the seven gate-7 presets
(Tiered Low, High, Ultra and Overdrive with TAAU, Low and High with FSR 3.1, Compat Ultra)
captured:

- the default scene;
- the material sheet;
- the teamfight decal scene.

Four more captures: the sheet on the forward Tiered frame (`MOBA_DEFERRED=off`), the
teamfight with `MOBA_DECALS=off`, and the teamfight (High) and the sheet (Low) with the tile
counts read back (`MOBA_TILE_COUNTS=1`). All 25 ran on the RTX with **0 validation messages**,
on the review's head. The default scene has no `Paint` decal and the teamfight has them, so
both of the frame graph's `paint` variants are covered. On lavapipe the teamfight with
`MOBA_TILE_COUNTS=1` is also clean (synchronisation validation included), as is
`gbuffer-smoke` (`MOBA_RENDER_PATH=tiered`: 16,384 checks, 0 mismatches).

`deferred-parity.sh` on the RTX (information only; the gate is lavapipe's):

| Scene | Within 2/255 | Worst |
|---|---|---|
| default | 100.000 % | 3 |
| default, sky off | 100.000 % | 3 |
| river | 100.000 % | 3 |
| teamfight | 99.998 % | 15 (one pixel at 15, one at 13) |

The teamfight's two pixels are at (1716–1717, 964), on a unit's edge. N2b's own RTX run
(`ceca947`'s review) had the same two pixels at 13 and 15/255, so N2c did not introduce them.
They pre-exist at N2b (the same two pixels); the cause is not known. Lavapipe reads 6/255 at most.
