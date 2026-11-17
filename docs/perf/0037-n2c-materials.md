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
- **Branch:** `feat/n2c-material-model`, on origin/main `ceca947` (N2b): the first build at
  `7106fb2`, the first review's code at `5e7062b`, and the second review's at `84e0904`, where
  every measurement below ran unless it says otherwise. The third review's commit adds two
  start-up log lines, stops compiling the `paint` frame-graph variants when the decal pass is
  off, and adds a CI step; gate 2 on all four scenes, the tiered goldens and Compat were
  re-run on it on lavapipe, and all 48 of its captures are byte-identical to `84e0904`'s
  (`review3/` in the evidence directory). Other later commits change only docs and the
  hash file.
- **Baselines:** detached release builds in their own temporary worktrees
  (`build-baselines.sh`), deleted after the measurements: `ab42718` (the N1 tiered forward
  frame, ADR-0023 §9b's baseline) and `ceca947` (N2b).
- **Evidence:** the frame logs and captures of every run below (the first build's, the first
  review's in `review/`, the second review's in `review2/`), with the scripts that ran them,
  are kept locally in `~/Projects/MOBA/handoff/evidence/n2c-ab/` (not committed). Each script
  takes its output directory as an argument, refuses to overwrite one, and reads its paths
  from `scripts/env.sh`, so the runs can be repeated (`final-chain.sh OUT` runs them all).

Every lavapipe and RTX run below printed no `[vulkan` line.

## Summary

| Gate | Result |
|---|---|
| 1. Encoding and interfaces | **Met.** `MaterialRecord` and `MaterialPush` against the Slang reflection; the GLSL-vertex / Slang-fragment interface test; the record's model word; the Rust twin and `gbuffer-smoke` cover all eight models |
| 2. Deferred–forward parity | **Met** locally at 1280×720: worst pixel 3, 3, 3 and 6/255 (default, default without the sky, river, teamfight), with the inline decals and with the decal pass alike, `STANDARD` and `UNLIT` only (amendment 3 item 22); CI 4, 4, 3 and 6/255 |
| 3. White furnace | **Met** (CPU model of the shader's fetch): worst 0.9861 over roughness 0–1 and N·V 0.05–1 |
| 4. Hidden information (decals) | **Partly:** with bloom off, every unit pixel keeps its G-buffer albedo with decals on and off, with the inline decals and with the decal pass; the `Paint` decals are not isolated from the glow decals; see below |
| 5. Goldens | **Met.** Compat byte-identical (11 scenes); all twelve tiered goldens two-run identical; exact hashes from CI |
| 7. Performance (≤ 1.10× N1) | **Met** on the teamfight, every preset: GPU total 1.014–1.063× `ab42718` (GPU span 1.029–1.074×), 0.998–1.002× `ceca947` on p50 (0.998–1.003× on p95); Compat 0.999×; least headroom Tiered Low, 0.0188 ms. **The extended-model check is met:** the close material sheet (14.1 % of tiles extended) is 1.091× an estimate of N1 drawing the spheres (criterion 1.10×; 1.123× N1's frame without them in the same rounds). The overview 0.999× (p95 1.000×) |
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

These are the second review's head, with the terrain's inline decals (amendment 3 item 31);
with the decal pass forced on (`MOBA_PAINT_DECAL_PASS=1`) every scene reads the same. The
first review's head (the split G-buffer) read the same, and the first build 2, 2, 2 and 7
pixels past 2/255 with the same worst values.

One fix made the decal pass's teamfight pass. Before it, the worst pixel was 28/255: splat
droplets outside the forward decal grid's cell drew only in the deferred frame. The pass now
reads the grid (amendment 3 item 16); the inline decals read it by construction.

The 6/255 pixel is on a unit's edge inside a glow decal's ring (N2b's teamfight worst was
5/255). CI's `tiered-temporal` check runs the same script on its own lavapipe (Mesa 26.0.8,
LLVM 21.1.8): 4, 4, 3 and 6/255, with 2, 2, 1 and 8 pixels past 2/255.

## Gate 3: the white furnace

`white_furnace_with_multiscatter_is_one` integrates the GGX lobe (4,096 samples, F0 = 1) at
roughness 0 to 1 in steps of 0.05 and N·V 0.05 to 1. It applies the shader's compensation
`1 + F0 (1/E − 1)`, with E read from Tiered's `R16G16_SFLOAT` LUT the way `brdf_lut_sample`
reads it. The worst value is **0.9861** (gate: 1 ± 0.02). With the 128-sample LUT and the
clamped fetch it was 0.93. This is a CPU model of the fetch, not a rendered furnace: no scene
renders a uniform white environment.

## Gate 4: hidden information (decals)

The teamfight replay with `MOBA_DEBUG_VIEW=albedo` (the G-buffer's base colour, nearest, before
lighting), Native mode at 1280×720 on lavapipe, bloom off (`MOBA_BLOOM=off`), with decals on and
off, and `MOBA_DEBUG_VIEW=flags` for the `UNIT` mask (`hidden.sh`). Pixels of the pure unit
colour in the flags view number 38,838: eight units and a structure.

- **With the terrain's inline decals** (the default, amendment 3 item 31): **all 38,838 keep
  every albedo byte**, while 35,063 other pixels change (up to 239/255).
- **With the decal pass** (`MOBA_PAINT_DECAL_PASS=1`): all 38,838 keep every byte, while 35,010
  other pixels change. The two frames differ on 6,271 pixels by at most 3/255, none of them a
  unit's (the pass blends into `gb0`'s stored 8-bit value through its sRGB view; the inline
  decals blend before the value is stored).
- The first review, with bloom on, found the units inside or at the edge of a glow decal's ring
  differing by at most 1/255: the glow's bloom on top of the debug view, not `gb0`.

`MOBA_DECALS=off` also removes the `Glow` decals, which the overlay draws over the debug view,
so this does not isolate the `Paint` decals. By construction the units overwrite `gb0` after
the decals (§7a). The full gate, with rims, is N2e's.

## Goldens and probes

- **Compat is byte-identical** to `ceca947` on lavapipe, compared with `cmp`:
  - the Compat scenes of `render-golden.yml` (default, Ultra, Low at 0.75, VAT, horizon,
    river, river Low, river Ultra and the teamfight decals);
  - the textured `sentinel` hero and the skinned sentinel over the local server.

  All 11 are identical, with 0 validation messages. The material sheet also draws on Compat:
  8 spheres, each "drawn with its material".
- **SPIR-V** (`84e0904` against `ceca947`): 107 of the 119 modules on `ceca947`'s side are
  byte-identical.
  - Compat's one change is `unit_textured.frag`, which reads the emissive mask (amendment 3
    item 2). The N2c samples carry a mask (their MR maps are `rgba8`, item 18); no material
    in the Compat golden scenes does, so their images do not move.
  - The other seven are Tiered: `deferred_light`, `deferred_light_extended`, `light_classify`
    and the tiered terrain, foliage, water and water-refraction stages. The terrain's G-buffer
    `_hdr` stages are N2b's again, byte for byte (item 31).
  - Eleven modules are new: the two decal stages, the terrain's two G-buffer stages without
    decal code (the decal pass's, item 31), the four `unit_material_gbuffer` Slang stages,
    `unit_tiered`, `unit_textured_tiered` and `terrain_nodecals_tiered`.
  - Four are gone: the GLSL `unit_textured_gbuffer` fragment permutations.
- **Tiered goldens** at 640×360:
  - All twelve cases are two-run byte-identical, with no validation messages: the four
    existing ones and `materials` and `decals`, each with TAAU and FSR 3.1. Both mid-stream
    resets are clean.
  - The eight existing fingerprints still pass (re-run on the second review's head: mean
    ≤ 0.145, max ≤ 5), so they are not rewritten.
  - The four new ones were written locally at 640×360. On the second review's head all four
    pass the committed ones and are kept: `decals` mean ≤ 0.006, max 1 (the inline decals);
    `materials` mean 0.000, max 0 (the packed model context changes no sampled pixel). The
    first review rewrote the `materials` pair (mean 0.133, max 39 against the first build's):
    the anisotropic sphere's direction follows its tangent (amendment 3 item 26), the thin
    film's Fresnel is taken at N·V and its AO is read (items 25 and 27), and the LUT's texels
    moved (item 10).
  - The exact hashes come from CI only: `render-golden.yml` dispatched on `84e0904` with
    `cpu_sample=true` (run 44028236577), its `tiered-hash-sections` artefact pasted with
    both the `cpu=amd` and `cpu=intel` sections of all twelve names, sorted with
    `LC_ALL=C` (earlier: run 44013048172 on `5e7062b`, run 43992116303 on `7106fb2`). Only
    the `decals` and `fsr3-decals` lines moved, in both sections (the inline decals blend in
    the terrain's shader again); the `materials` lines are `5e7062b`'s, so the packed model
    context (item 32) changes no byte. Every later commit touches only docs and the hash file.

## Gate 7: performance

Re-measured on the second review's head, `84e0904`: the `Paint` decals inline in the terrain's
G-buffer stage again (amendment 3 item 31), the packed model context (item 32) and the first
review's changes (the hoisted model terms, the end-point LUT, the per-record texture flags).
The first review's numbers (`5e7062b`, with the split G-buffer) and the first build's
(`7106fb2`) are at the end of this section.

- **Builds:** `ab42718`, `ceca947` and the branch: release builds, the first two in detached
  worktrees (`build-baselines.sh`).
- **Machine:** the RTX 5070 Ti (driver 620.70.06, Linux 7.3.4).
- **Replay:** the teamfight replay (`MOBA_WARMUP_TICKS=330`), its `Paint` decals included, at
  1920×1080. Vsync off, the 240 fps limit, validation off, the pipeline cache off, 3,000 frames
  per run.
- **Configurations:** Tiered Low, High and Ultra with TAAU, Tiered Overdrive (Ultra, TAAU
  Performance mode), Tiered Low and High with FSR 3.1, Compat Ultra, and Low and High with
  the decal pass forced on (`MOBA_PAINT_DECAL_PASS=1`; the baselines ignore it); the material
  sheet at two distances with a no-sheet control each; the overview.
- **Rounds:** three, interleaved (`perf.sh`). Each held `flock handoff/gpu.lock` and ran every
  configuration on the three builds in turn; a run is skipped if another `moba-client` is
  running (none was).
- **Results:** medians of the three rounds' p50 (and p95), in ms. Within each configuration
  and build, the three GPU totals agree within 0.005 ms.
- **The gate metric is `gpu_total`** (the sum of the pass timestamps), the column perf/0036
  marked as the gate. `gpu_span` (first to last timestamp, so also the gaps between passes) is
  reported beside it and treated as advisory (ADR-0023 amendment 3, gate 7).
- **One frame-graph variant per configuration,** except the two with the decal pass on: there
  the split runs on the 2,806 of 2,940 frames with a live `Paint` decal (`gbuffer_decals`
  runs), and its lanes' medians are over those frames.
- **What the frames held:** about 3.3 live `Paint` decals per frame on the teamfight, none
  extended-model tiles (`extended_frames=0`); the sheet's tile counts are below
  (`MOBA_TILE_COUNTS=1`, amendment 3 item 30, from a separate counts run, since the read-back
  costs time).

| Configuration | Build | GPU total p50 | p95 | GPU span p50 | `prepass` | `gbuffer_decals` | `gbuffer_rest` | `deferred_light` | ÷ N1 total (span) | ÷ N2b total p50 (p95) |
|---|---|---|---|---|---|---|---|---|---|---|
| Tiered Low (TAAU) | `ab42718` | 0.508 | 0.521 | 0.515 | 0.020 | — | — | — |  |  |
|  | `ceca947` | 0.540 | 0.551 | 0.554 | 0.063 | — | — | 0.053 | 1.063 (1.076) |  |
|  | branch | 0.540 | 0.551 | 0.553 | 0.063 | — | — | 0.053 | **1.063 (1.074)** | 1.000 (1.000) |
| Tiered High (TAAU) | `ab42718` | 0.558 | 0.570 | 0.564 | 0.024 | — | — | — |  |  |
|  | `ceca947` | 0.585 | 0.597 | 0.600 | 0.069 | — | — | 0.056 | 1.048 (1.064) |  |
|  | branch | 0.585 | 0.597 | 0.598 | 0.069 | — | — | 0.057 | **1.048 (1.060)** | 1.000 (1.000) |
| Tiered Ultra (TAAU) | `ab42718` | 0.558 | 0.571 | 0.565 | 0.024 | — | — | — |  |  |
|  | `ceca947` | 0.584 | 0.596 | 0.598 | 0.069 | — | — | 0.056 | 1.047 (1.058) |  |
|  | branch | 0.585 | 0.598 | 0.599 | 0.069 | — | — | 0.057 | **1.048 (1.060)** | 1.002 (1.003) |
| Tiered Overdrive | `ab42718` | 0.502 | 0.514 | 0.509 | 0.027 | — | — | — |  |  |
|  | `ceca947` | 0.510 | 0.521 | 0.524 | 0.055 | — | — | 0.036 | 1.016 (1.029) |  |
|  | branch | 0.509 | 0.520 | 0.524 | 0.055 | — | — | 0.036 | **1.014 (1.029)** | 0.998 (0.998) |
| Tiered Low, FSR 3.1 | `ab42718` | 0.656 | 0.668 | 0.663 | 0.020 | — | — | — |  |  |
|  | `ceca947` | 0.686 | 0.701 | 0.701 | 0.063 | — | — | 0.053 | 1.046 (1.057) |  |
|  | branch | 0.687 | 0.701 | 0.700 | 0.064 | — | — | 0.053 | **1.047 (1.056)** | 1.001 (1.000) |
| Tiered High, FSR 3.1 | `ab42718` | 0.706 | 0.721 | 0.713 | 0.024 | — | — | — |  |  |
|  | `ceca947` | 0.731 | 0.745 | 0.745 | 0.069 | — | — | 0.056 | 1.035 (1.045) |  |
|  | branch | 0.732 | 0.746 | 0.746 | 0.069 | — | — | 0.057 | **1.037 (1.046)** | 1.001 (1.001) |
| Compat Ultra | `ab42718` | 0.674 | 0.685 | 0.678 | 0.058 | — | — | — |  |  |
|  | `ceca947` | 0.674 | 0.684 | 0.678 | 0.058 | — | — | — | 1.000 (1.000) |  |
|  | branch | 0.673 | 0.684 | 0.677 | 0.058 | — | — | — | **0.999 (0.999)** | 0.999 (1.000) |

**Headroom against 1.10× N1** (ms; negative is over):

| Configuration | 1.10× N1 total | branch total | headroom (ms) | 1.10× N1 span | branch span | headroom (ms) |
|---|---|---|---|---|---|---|
| Tiered Low (TAAU) | 0.5588 | 0.540 | +0.0188 | 0.5665 | 0.553 | +0.0135 |
| Tiered High (TAAU) | 0.6138 | 0.585 | +0.0288 | 0.6204 | 0.598 | +0.0224 |
| Tiered Ultra (TAAU) | 0.6138 | 0.585 | +0.0288 | 0.6215 | 0.599 | +0.0225 |
| Tiered Overdrive | 0.5522 | 0.509 | +0.0432 | 0.5599 | 0.524 | +0.0359 |
| Tiered Low, FSR 3.1 | 0.7216 | 0.687 | +0.0346 | 0.7293 | 0.700 | +0.0293 |
| Tiered High, FSR 3.1 | 0.7766 | 0.732 | +0.0446 | 0.7843 | 0.746 | +0.0383 |
| Compat Ultra | 0.7414 | 0.673 | +0.0684 | 0.7458 | 0.677 | +0.0688 |
| Sheet, far (High) | 0.6743 | 0.679 | -0.0047 | 0.6798 | 0.695 | -0.0152 |
| Sheet camera, no sheet | 0.6732 | 0.654 | +0.0192 | 0.6798 | 0.668 | +0.0118 |
| Sheet, close (High) | 0.6413 | 0.657 | -0.0157 | 0.6468 | 0.672 | -0.0252 |
| Close camera, no sheet | 0.6413 | 0.629 | +0.0123 | 0.6479 | 0.642 | +0.0059 |

- **Every teamfight preset is within 1.10× N1** on `gpu_total` and costs what N2b's does:
  0.998–1.002× `ceca947` on p50 (0.998–1.003× on p95, Ultra's 0.598 against 0.596 ms), with
  `prepass` equal to N2b's at every preset. The least headroom is Tiered Low: 0.0188 ms of
  total, 0.0135 ms of span (the first review's split G-buffer left 0.0138 and 0.0074).
- **Before `FOLIAGE` moved to the standard tiles** (amendment 3 item 11), round 1 of the first
  build measured Tiered Low at 1.108× total (1.123× span), High at 1.100× and Ultra at 1.098×:
  over or at the gate. That is why it moved.

**The decal pass, forced on** (amendment 3 item 31; the cost of turning it on when a receiver
other than the terrain needs it):

| Configuration | Build | GPU total p50 | p95 | GPU span p50 | `prepass` | `gbuffer_decals` | `gbuffer_rest` | `deferred_light` | ÷ N1 total (span) | ÷ N2b total p50 (p95) |
|---|---|---|---|---|---|---|---|---|---|---|
| Low, decal pass on | `ab42718` | 0.508 | 0.521 | 0.515 | 0.020 | — | — | — |  |  |
|  | `ceca947` | 0.539 | 0.552 | 0.554 | 0.063 | — | — | 0.053 | 1.061 (1.076) |  |
|  | branch | 0.546 | 0.559 | 0.559 | 0.045 | 0.005 | 0.021 | 0.053 | 1.075 (1.085) | 1.013 (1.013) |
| High, decal pass on | `ab42718` | 0.558 | 0.570 | 0.565 | 0.024 | — | — | — |  |  |
|  | `ceca947` | 0.584 | 0.596 | 0.599 | 0.069 | — | — | 0.056 | 1.047 (1.060) |  |
|  | branch | 0.594 | 0.607 | 0.608 | 0.052 | 0.005 | 0.024 | 0.057 | 1.065 (1.076) | 1.017 (1.018) |

- It costs **0.006 ms of total at Low and 0.009 ms at High** over the inline decals (p95:
  0.008 and 0.010 ms). By lane, `prepass` + `gbuffer_decals` + `gbuffer_rest` are 0.071 ms at
  Low and 0.081 ms at High against the single pass's 0.063 and 0.069 ms: 0.008 and 0.012 ms.
  The decal pass itself is 0.005 ms; the rest is the second pass boundary (the G-buffer
  attachments are loaded again). The first review measured the same split at 0.007–0.011 ms
  of total, with the read-only depth and the culled front faces already in.
- **The receivers' pass** (§9b's `gbuffer_receivers` against N1's prepass) exists only with it:
  the terrain alone is 0.045 ms at Low and 0.052 ms at High, against N1's whole-scene
  `prepass` of 0.020 and 0.024 ms, which writes no G-buffer.

**Extended-model pixels: the material sheet** (`MOBA_MATERIAL_SHEET=samples`, local preview,
Tiered High with TAAU). `ab42718` and `ceca947` cannot draw these materials (they ignore the
variable), so their rows show the same view without the spheres, and each sheet configuration
has a control with the same camera and no sheet. "Far" is the golden's camera
(`1.5708,0.45,11,0,-12`): 244 of 3,554 tiles extended (6.9 %). "Close" is
`1.5708,0.3,4.5,0,-12`, with the anisotropic and thin-film spheres in the middle: 433 of 3,061
tiles extended (14.1 %). (Tile counts from the first review's counts runs; the classification
has not changed since. On this head, the far sheet at Low reads 3,358 standard and 242
extended tiles per frame on the RTX, every frame extended.)

| Configuration | Build | GPU total p50 | p95 | GPU span p50 | `prepass` | `gbuffer_decals` | `gbuffer_rest` | `deferred_light` | ÷ N1 total (span) | ÷ N2b total p50 (p95) |
|---|---|---|---|---|---|---|---|---|---|---|
| Sheet, far (High) | `ab42718` | 0.613 | 0.627 | 0.618 | 0.044 | — | — | — |  |  |
|  | `ceca947` | 0.653 | 0.669 | 0.665 | 0.129 | — | — | 0.052 | 1.065 (1.076) |  |
|  | branch | 0.679 | 0.694 | 0.695 | 0.140 | — | — | 0.060 | **1.108 (1.125)** | 1.040 (1.037) |
| Sheet camera, no sheet | `ab42718` | 0.612 | 0.627 | 0.618 | 0.044 | — | — | — |  |  |
|  | `ceca947` | 0.653 | 0.668 | 0.666 | 0.129 | — | — | 0.052 | 1.067 (1.078) |  |
|  | branch | 0.654 | 0.670 | 0.668 | 0.129 | — | — | 0.054 | **1.069 (1.081)** | 1.002 (1.003) |
| Sheet, close (High) | `ab42718` | 0.583 | 0.597 | 0.588 | 0.044 | — | — | — |  |  |
|  | `ceca947` | 0.628 | 0.643 | 0.641 | 0.124 | — | — | 0.045 | 1.077 (1.090) |  |
|  | branch | 0.657 | 0.671 | 0.672 | 0.134 | — | — | 0.053 | **1.127 (1.143)** | 1.046 (1.044) |
| Close camera, no sheet | `ab42718` | 0.583 | 0.597 | 0.589 | 0.044 | — | — | — |  |  |
|  | `ceca947` | 0.626 | 0.641 | 0.640 | 0.124 | — | — | 0.045 | 1.074 (1.087) |  |
|  | branch | 0.629 | 0.644 | 0.642 | 0.124 | — | — | 0.047 | **1.079 (1.090)** | 1.005 (1.005) |

- **Against N1's frame without the spheres**, the sheet is 1.108× (far) and 1.127× (close) in
  these rounds; in the sheet-forward rounds the extended-model check uses, the close view is
  1.123× (0.658 against 0.586 ms).
  The spheres cost the branch 0.025–0.028 ms over the same view without them: their G-buffer
  draws (`prepass`, about 0.010 ms), the extended tiles in `deferred_light` (0.006–0.007 ms)
  and their shadow, AO and overlay prepass. The views without the spheres are 1.069× and
  1.079× N1 (N2b: 1.067× and 1.074×).
- **The close view without the spheres has no live `Paint` decal**, and there the first
  review's frame was 0.005 ms cheaper (0.621 against 0.629 ms): it drew the terrain's
  permutation without decal code. The inline permutation is N2b's (`prepass` 0.124 ms, as
  `ceca947`'s), so that 0.005 ms is not lost against N2b, but a per-frame choice of the
  permutation would win it back on such views; amendment 3 item 31 leaves it as an N2d lever.
- **The extended-model check** (amendment 3, gate 7). N1 cannot draw the spheres, so the
  check is against an estimate of N1 drawing them: N1's close view without them plus what the
  spheres cost the forward Tiered frame (`MOBA_DEFERRED=off`, the branch) in the lanes N1 has.
  Three more interleaved rounds (`sheet-forward`) measured, in ms:

| Lane | N1, no sheet | branch forward, no sheet | branch forward, sheet | spheres' forward cost |
|---|---|---|---|---|
| `scene` | 0.093 | 0.092 | 0.097 | +0.005 |
| `shadow` | 0.012 | 0.012 | 0.015 | +0.003 |
| `ao` | 0.049 | 0.050 | 0.053 | +0.003 |
| `overlay_prepass` | 0.025 | 0.025 | 0.031 | +0.006 |
| `prepass` | 0.044 | 0.124 | 0.133 | +0.009 |
| GPU total | 0.586 | 0.665 | 0.692 | +0.027 |

  - N1 drawing the spheres is about 0.586 + 0.005 + 0.003 + 0.003 + 0.006 = **0.603 ms**. The
    forward `prepass` delta is left out, since it writes the G-buffer and N1's prepass does
    not; with it in, 0.612 ms.
  - The deferred frame's close sheet in the same rounds is 0.658 ms: **1.091×** the estimate
    (1.075× with the prepass delta). **Criterion: within 1.10×.** Met, with 0.005 ms to
    spare.
- So the gate holds on the replay it is defined on, which has no extended model, with
  0.0188–0.0446 ms of headroom, and the extended-model check holds with 0.005 ms. When heroes
  ship with extended models, gate 7 must include them; amendment 3's gate 7 budgets N2d–N2f
  against both.

**The overview** (§9b gate 7's decal-overdraw check asks for a wall-heavy view; the shipped
maps have four walls each, so none is): a low orbit across the whole map
(`MOBA_PREVIEW_CAMERA=-0.7,0.12,170`, local preview: terrain, towers and structures, little
wall), Tiered High with TAAU, three interleaved rounds under the GPU lock (`overview.sh`):

| Configuration | Build | GPU total p50 | p95 | GPU span p50 | `prepass` | `gbuffer_decals` | `gbuffer_rest` | `deferred_light` | ÷ N1 total (span) | ÷ N2b total p50 (p95) |
|---|---|---|---|---|---|---|---|---|---|---|
| Overview (High) | `ab42718` | 0.700 | 0.715 | 0.707 | 0.123 | — | — | — |  |  |
|  | `ceca947` | 0.698 | 0.713 | 0.713 | 0.240 | — | — | 0.036 | 0.997 (1.008) |  |
|  | branch | 0.699 | 0.715 | 0.714 | 0.240 | — | — | 0.037 | **0.999 (1.010)** | 1.001 (1.003) |

The local preview's bots leave a `Paint` decal live on some frames (0.59 per frame on
average); every frame now runs the single G-buffer pass, so the lanes are whole-run medians.
The frame costs what N2b's does: 0.999× N1's total at p50 and 1.000× at p95, 1.001× N2b's.
The first review's frame, which split on the frames with a live `Paint` decal, read 0.692 ms
p50 (0.991×), but its p95 was 0.725 ms against N2b's 0.710 (medians of the three rounds): the
second review put its split frames at 0.013–0.017 ms more, which the p50, taken mostly over
single-pass frames, hid. Walls in view are not measured (amendment 3 item 23), so the
ADR's decal-ordering risk stays open.

**The first review's head** (`5e7062b`, the split G-buffer on every frame with a live
`Paint` decal; logs in the evidence directory's `review/`): Tiered Low 0.545 ms (1.073×, span
1.086×), High 0.594 (1.063×), Ultra 0.594 (1.063×), Overdrive 0.520 (1.036×), Low with FSR 3.1
0.694 (1.058×), High with FSR 3.1 0.741 (1.048×), Compat Ultra 0.672 (0.999×); 1.010–1.020×
N2b. The sheet 1.102× (far) and 1.117× (close).

**The first build** (`7106fb2`, the same method; its logs are in the evidence directory):
Tiered Low 0.547 ms (1.077×, span 1.091×), High 0.596 (1.064×), Ultra 0.596 (1.066×),
Overdrive 0.522 (1.040×), Low with FSR 3.1 0.695 (1.059×), High with FSR 3.1 0.744 (1.054×),
Compat Ultra 0.675 (0.999×); the overview 0.709 ms (1.016×).

- **Pipelines:** Tiered 51, as N2b (N1 46), FSR 3.1 59 (N1 54), Compat 42 (unchanged); 52 and
  60 with the decal pass on (its pipeline). The Slang material G-buffer stage replaces the
  GLSL textured one in the same pipeline.
- `locked_uploads=0` and `locked_pipelines=0` in every run.
- **VRAM at High:** 247.5 MB, the same as N2b (N1 236.2). The material table is 4 KiB.
- The raw logs are in `~/Projects/MOBA/handoff/evidence/n2c-ab/review2/` (local, not
  committed), with the scripts that ran them (`scripts/`; each takes its output directory).

## Register counts on the RTX 5070 Ti

`MOBA_PIPELINE_STATS=1` (amendment 3 item 32) logs `VK_KHR_pipeline_executable_properties`
statistics; NVIDIA reports `Register_Count`. The logs are in the evidence directory's
`review2/r8/`.

| Pipeline stage | Registers | Log |
|---|---|---|
| `deferred_light` (standard tiles) | 74 | `final.log` |
| `deferred_light_extended`, the first build (`7106fb2`, before the hoisting) | 111 | `old.log` |
| `deferred_light_extended`, hoisted, one field per term (`5477882`) | 113 | `cur.log` |
| `deferred_light_extended`, packed into three `vec4`s (`84e0904`) | 105 | `packed.log`, `final.log` |
| `light_classify` | 17 | `final.log` |
| `terrain_gbuffer` fragment, inline decals (the default) | 48 | `final.log` |
| `terrain_gbuffer` fragment without decal code (`MOBA_PAINT_DECAL_PASS=1`) | 64 | `pstats-pass.log` |

`scripts/terrain-decal-check.sh` now fails if the inline stage needs more than 48
(`GBUFFER_REGISTERS`).

## Validation on the RTX 5070 Ti

`MOBA_VULKAN_VALIDATION=required` (validation and synchronisation validation), 1280×720
(1920×1080 swapchain), on the branch's release build. Each of the seven gate-7 presets
(Tiered Low, High, Ultra and Overdrive with TAAU, Low and High with FSR 3.1, Compat Ultra)
captured:

- the default scene;
- the material sheet;
- the teamfight decal scene.

Seven more captures: the sheet on the forward Tiered frame (`MOBA_DEFERRED=off`), the
teamfight with `MOBA_DECALS=off`, the teamfight (High) and the sheet (Low) with the tile
counts read back (`MOBA_TILE_COUNTS=1`), and the teamfight with the opt-in decal pass
(`MOBA_PAINT_DECAL_PASS=1`, amendment 3 item 31) on Low, on High, and on High with the tile
counts. All 28 ran on the RTX with **0 validation messages**, on `84e0904`. The default frame
inlines the terrain's decals, so the frame graph's `paint` variant is built only under
`MOBA_PAINT_DECAL_PASS=1`; those three captures cover it, and the other teamfight captures
cover the inline path. The tile-count captures read from frame 0, so the first copy after
creation is skipped (item 33); a resize was not exercised by a capture. On lavapipe the
teamfight with `MOBA_TILE_COUNTS=1` is also clean with and without the pass (synchronisation
validation included), as is `gbuffer-smoke` (`MOBA_RENDER_PATH=tiered`: 16,384 checks,
0 mismatches).

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
