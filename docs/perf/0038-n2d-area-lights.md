# LTC area lights on Tiered (N2d)

Captured 2026-11-17 for ADR-0023 (N2d) and its
[Amendment 4](../adr/0023-frame-architecture-materials.md#amendment-4-2026-11-17-n2d-ltc-area-lights-and-the-light-record-as-built).

- **GPU:** NVIDIA GeForce RTX 5070 Ti (driver 620.70.06), used for timings, register counts and
  validation.
- **Software:** lavapipe (the local Mesa llvmpipe in `~/Projects/MOBA/.tools`) with the Khronos
  validation layer and `MOBA_VULKAN_VALIDATION=required` (validation and synchronisation
  validation), under XWayland with `WINIT_X11_SCALE_FACTOR=1` as in perf/0037.
- **CPU:** AMD Ryzen 7 9800X3D. **OS:** Linux 7.3.4-1-cachyos.
- **Toolchain:** rustc 1.99.1, the pinned `slangc`.
- **Branch:** `feat/n2d-ltc-area-lights` on `fa661a7` (N2c). Every GPU measurement below ran on
  the build of `70c3c5f`; the later commits change docs, the goldens script, the hash and
  fingerprint files, a doc comment and the preset matrix's text, not a shader or the frame.
- **Baselines:** release builds in their own worktrees: `ab42718` (N1's tiered forward frame,
  ADR-0023 §9b's baseline) and `fa661a7` (N2c).
- **Evidence:** frame logs, captures and register logs, with the scripts that ran them, are
  kept locally in `~/Projects/MOBA/handoff/evidence/n2d-ab/` (not committed): `r1/` holds
  the final runs, `*-probe/` the probes quoted under "How the cost was recovered". The
  scripts are N2c's (`scripts/env.sh`, `perf.sh`, `perf-round.sh`, `perfsum.py`,
  `parity-x11.sh`, `goldens.sh`), pointed at this branch, with the area-light
  configurations added; `compat-n2d.sh` and `realgpu-n2d.sh` are N2c's with this branch's
  scenes.

Every lavapipe and RTX run below printed no `[vulkan` line.

## Summary

| Gate | Result |
|---|---|
| 2. Deferred–forward parity | **Met** locally at 1280×720 on lavapipe, with the area-light sheet as a fifth scene lit with LTC on both paths: worst pixel 3, 3, 3, 6 and 3/255 (default, default without the sky, river, teamfight, area-light sheet), at least 99.999 % within 2/255; the same with `MOBA_PAINT_DECAL_PASS=1`, and on the RTX 5070 Ti |
| 3. LTC | **Met** (the CPU twin of `ltc.glsl`): each shape within a recorded tolerance of a Monte Carlo reference (worst evaluation, diffuse and specular: sphere 0.058, 0.0002, 0.099; capsule 0.145, 0.042, 0.056; rect 0.116, 0.002, 0.105); `RGBA16F` kept (rounding ≤ 4.9e-4); a shrinking sphere tends to the point light (2.4e-4 diffuse, 6.6e-4 specular); the forward and deferred loops visit the same shaped lights; the white furnace unchanged (0.9861) |
| 5. Goldens | **Met.** Compat byte-identical to `fa661a7` (11 scenes) and its SPIR-V unchanged; all fourteen tiered goldens two-run identical, the new `ltc` and `fsr3-ltc` among them; exact hashes from CI (only `decals` and `fsr3-decals` moved, the teamfight's bolts now being capsules) |
| 7. Performance (≤ 1.10× N1) | **Met on the teamfight, every preset:** GPU total 1.020–1.067× `ab42718` (span 1.036–1.082×); N2d's budget met on p50 (+0.002 ms at Low, +0.004 at High; amendment 3: 0.005 and 0.008), +0.010–0.011 on p95 at High; Compat 1.003×; least headroom after N2e and N2f's budgets Tiered High, 0.0123 ms. Extended-model check +0.001 ms. **Not met on the area-light sheet at High:** LTC costs +0.046 ms (far) and +0.049 ms (close) over the same lights as points, 1.143× and 1.158× N1 (about 1.13× and 1.14× an estimate of N1 drawing the lights); Low (the representative point) 1.082×. Recorded in amendment 4 |
| Validation | **Met:** 0 messages on lavapipe and on the RTX 5070 Ti (36 captures, both pipelines of the per-frame choice) |

## Gate 3: LTC against Monte Carlo

The Rust twin of `include/ltc.glsl` (`crates/render/src/ltc/twin.rs`, step for step: the
bilinear fetch of the half-float table, the shading frame, the clipless quad, the sphere's
disk and cubic) is held against a fixed-seed Monte Carlo reference
(`ltc::tests::ltc_matches_monte_carlo`). The reference integrates, over the true shape (ray
hits against the sphere, the capsule and the one-sided rect), the clamped cosine (the
diffuse), the engine's GGX specular at F0 0.04 and 1, and the LTC lobe itself, by cone
sampling with up to 400 strata. The cases: roughness 0.15, 0.3, 0.5, 0.75 and 1; view angles
0°, 40° and 75°; four light directions (elevation 80°, 45°, 30° and 20°); two sizes, 0.25 m at
3 m and 0.75 m at 2.5 m (the sphere's radius; the capsule a segment four times as long, of
0.4 times the radius, tilted across the view; the rect turned 30° from facing the point). The
errors are relative, worst over the 120 cases per shape:

| Shape | Evaluation (LTC lobe over the shape) | Diffuse | Specular (against GGX) | Tolerance (evaluation, diffuse, specular) |
|---|---|---|---|---|
| Sphere | 0.058 | 0.0002 | 0.099 | 0.07, 0.002, 0.12 |
| Capsule | 0.145 | 0.042 | 0.056 | 0.17, 0.05, 0.07 |
| Rect | 0.116 | 0.002 | 0.105 | 0.13, 0.003, 0.12 |

- **What the errors are:** the evaluation's is the clipless method's horizon (the sphere
  table stands in for clipping the polygon or ellipse in LTC space; worst on glossy lobes at
  grazing view), plus, for the capsule, its quad's square ends. The specular adds the table's
  fit of GGX, loosest at grazing view, where the lobe's peak sits about 5° from GGX's.
- **`RGBA16F` is kept:** the bake reports a relative rounding of at most 4.9e-4, and every
  number above is with the half-float table (`the_table_is_well_formed` checks every texel is
  finite, every inverse matrix keeps its orientation, and the magnitudes, Fresnel and sphere
  table are in 0..=1). The baked table matches the upstream's `ltc_1`/`ltc_2` (median
  difference 0 per channel).
- **Radius → 0** (`a_shrinking_sphere_tends_to_the_point_light`): a sphere of the same
  power tends to the point light at its centre, worst over 3 roughness/view pairs and 3
  directions at 2 m:

  | R | 0.2 | 0.05 | 0.02 | 0.01 | 0.001 | 1e-5 |
  |---|---|---|---|---|---|---|
  | diffuse | 2.4e-4 | 2.4e-4 | 2.4e-4 | 2.4e-4 | 2.4e-4 | 2.4e-4 |
  | specular | 0.094 | 0.0054 | 6.6e-4 | 6.6e-4 | 6.6e-4 | 6.6e-4 |

  The test requires convergence and diffuse < 2e-3, specular < 0.02 at the end. Below a span
  of 1 % (R/d 0.01 here) a shape is grown and scaled back (amendment 4 item 4): without it a
  quad's `f32` evaluation was 10 % off at a span of 1e-3.
- **A capsule without length is its sphere** within 5 % (`a_capsule_without_length_is_its_sphere`).
- **The white furnace** (`white_furnace_with_multiscatter_is_one`) is unchanged: worst 0.9861.
  The split-sum LUT moved into the atlas texel for texel
  (`the_atlas_holds_both_tables`, and `the_shader_constants_match` pins the atlas size and
  the shapes' offset in the light buffer).
- **Light-set consistency:** `forward_and_deferred_visit_the_same_lights` now gives every
  fourth light each shape and asserts the bins hold shaped lights; the forward and deferred
  loops visit the same lights under every cap.

## Gate 2: deferred–forward parity

`scripts/deferred-parity.sh` at 1280×720 (High, TAAU Quality, the stylisation and foliage off)
on lavapipe, 0 validation messages, now with the area-light sheet as a fifth scene: a sphere,
a capsule and a rect light over open ground, lit with LTC on both paths. Pixels past 2/255 of
921,600:

| Scene | Within 2/255 | Worst | Past 2/255 |
|---|---|---|---|
| default | 100.000 % | 3 | 1 |
| default, `MOBA_SKY=off` | 100.000 % | 3 | 1 |
| R3b river | 100.000 % | 3 | 3 |
| R3b teamfight (its bolts are capsule lights) | 99.999 % | 6 | 7 (five at 3, one at 4, one at 6) |
| area-light sheet | 100.000 % | 3 | 2 |

With the G-buffer decal pass forced on (`MOBA_PAINT_DECAL_PASS=1`, CI's second parity step)
every scene reads the same. N2c's four scenes read exactly as in perf/0037 (worst pixel and
count alike).

## Goldens

- **Compat is byte-identical** to `fa661a7` on lavapipe (`compat-n2d.sh`, `cmp`): the default,
  Ultra, Low at 0.75, VAT, horizon, river, river Low, river Ultra and teamfight-decals scenes
  and the textured and skinned sentinel heroes over the local server, **all 11 identical**,
  0 validation messages. Compat's SPIR-V is byte-identical (below), and Compat lights every
  shape as the point at its centre: the area-light sheet on Compat draws with 3 shaped lights
  on every frame and no validation message. The material sheet still draws its 8 spheres.
- **Tiered goldens** at 640×360 (`goldens.sh`, `r1/goldens/`): all fourteen cases are two-run
  byte-identical with no validation messages, the twelve existing ones and the new `ltc`
  with TAAU and FSR 3.1 (the material sheet under the area-light sheet at Medium, the lowest
  preset with LTC; the extended models' spheres lit at the representative point). Both
  mid-stream resets are clean. The twelve existing fingerprints still pass (mean ≤ 0.145,
  max ≤ 5) and are kept; the two `ltc` fingerprints are new.
- **Exact hashes from CI only:** `render-golden.yml` dispatched on `164d590` with
  `cpu_sample=true` (run 44083568862, all eight sample legs green), its
  `tiered-hash-sections` artefact pasted whole, both the `cpu=amd` and `cpu=intel` sections,
  sorted with `LC_ALL=C`. Besides the new `ltc` and `fsr3-ltc`, only `decals` and
  `fsr3-decals` moved, in both sections: the teamfight's bolts are capsule lights now. `low`,
  `high`, `native`, `overlay` and `materials` (and their FSR 3.1 twins) keep N2c's hashes, so
  the atlas and the per-frame mode-0 pipelines change no byte of those captures. The `ltc` fingerprints pass on CI's Mesa (26.0.8). An earlier dispatch (run
  44078987962, on `277c09e`) failed only because the `ltc` fingerprints were not committed
  yet.

## Gate 7: performance

- **Builds:** `ab42718` (N1), `fa661a7` (N2c) and the branch, release builds.
- **Machine:** the RTX 5070 Ti, a 1920×1080 window (perf/0037's extent; the probes below ran
  at 1280×720).
- **Replay:** the teamfight replay (`MOBA_WARMUP_TICKS=330`). Vsync off, the 240 fps limit,
  validation off, the pipeline cache off, 3,000 frames per run.
- **Rounds:** three, interleaved, each under `flock handoff/gpu.lock`, no other `moba-client`
  running (`perf.sh`, `r1/gate7/`). Medians of the rounds' p50 (and p95), ms.
- **What the frames held:** the teamfight's bolts are capsule lights: **a shaped light on
  326 of 3,000 frames** (`area_frames`, 0.16 per frame on average; Compat counts 330). Those
  frames run the LTC pipelines (the representative point on Low), the rest the mode-0 ones.
- **The gate metric is `gpu_total`**; `gpu_span` is advisory (amendment 3, gate 7).

| Configuration | N1 total | N2c total | branch total | branch p95 | branch span | `deferred_light` N2c → branch | `water` N2c → branch | ÷ N1 total (span) | branch − N2c, p50 (p95) |
|---|---|---|---|---|---|---|---|---|---|
| Tiered Low (TAAU) | 0.554 | 0.589 | 0.591 | 0.603 | 0.606 | 0.055 → 0.056 | 0.019 → 0.021 | **1.067** (1.082) | +0.002 (+0.003) |
| Tiered High (TAAU) | 0.603 | 0.634 | 0.638 | 0.655 | 0.652 | 0.058 → 0.060 | 0.020 → 0.023 | **1.058** (1.069) | +0.004 (+0.010) |
| Tiered Ultra (TAAU) | 0.604 | 0.635 | 0.639 | 0.656 | 0.654 | 0.058 → 0.060 | 0.020 → 0.023 | **1.058** (1.070) | +0.004 (+0.010) |
| Tiered Overdrive | 0.545 | 0.551 | 0.556 | 0.570 | 0.572 | 0.037 → 0.038 | 0.016 → 0.019 | **1.020** (1.036) | +0.005 (+0.007) |
| Tiered Low, FSR 3.1 | 0.712 | 0.753 | 0.755 | 0.767 | 0.769 | 0.055 → 0.056 | 0.019 → 0.021 | **1.060** (1.070) | +0.002 (+0.003) |
| Tiered High, FSR 3.1 | 0.764 | 0.799 | 0.801 | 0.822 | 0.816 | 0.058 → 0.060 | 0.021 → 0.023 | **1.048** (1.060) | +0.002 (+0.011) |
| Compat Ultra | 0.731 | 0.732 | 0.733 | 0.744 | 0.738 | — | 0.031 → 0.032 | **1.003** (1.003) | +0.001 (+0.001) |

Within each configuration and build the three rounds agree within 0.006 ms.

- **Every preset is within 1.10× N1** on `gpu_total` (1.020–1.067×; span 1.036–1.082×).
- **N2d's budget is met on p50** (amendment 3: at most 0.005 ms at Low, 0.008 ms at High):
  +0.002 at Low (TAAU and FSR 3.1), +0.004 at High and Ultra, +0.005 at Overdrive, +0.002 at
  High with FSR 3.1. **On p95 the High presets read +0.010–0.011 ms**: the 11 % of frames
  with a shaped light run the LTC pipelines, which is the cost a budget measured on p50
  does not show. Most of the p50 cost is the water lane (above).
- **The extended-model check** (the close material sheet, High): the branch is +0.001 ms
  over N2c on p50 in the same rounds (0.706 against 0.705; +0.007 on p95), so amendment 3's
  0.005 ms of headroom becomes about 0.004 ms. The sheet holds a shaped light on 258 of
  3,000 frames (the preview's own effects). This round's absolute numbers are about 9 %
  above perf/0037's on every build (N1 Tiered Low 0.554 against 0.508) while the ratios
  reproduce (N2c Low 1.063× both times), so only the delta is quoted.

**Headroom against 1.10× N1 after N2d** (ms; `gpu_total`, and `gpu_span` advisory):

| Configuration | 1.10× N1 total | branch | headroom | N2c's headroom, same rounds | after N2e and N2f's budgets |
|---|---|---|---|---|---|
| Tiered Low (TAAU) | 0.6094 | 0.591 | +0.0184 | +0.0204 | +0.0134 (N2e 0.005, N2f 0) |
| Tiered High (TAAU) | 0.6633 | 0.638 | +0.0253 | +0.0293 | +0.0123 (N2e 0.008, N2f 0.005) |
| Tiered Ultra (TAAU) | 0.6644 | 0.639 | +0.0254 | +0.0294 | +0.0124 |
| Tiered Overdrive | 0.5995 | 0.556 | +0.0435 | +0.0485 | +0.0305 |
| Tiered Low, FSR 3.1 | 0.7832 | 0.755 | +0.0282 | +0.0302 | +0.0232 |
| Tiered High, FSR 3.1 | 0.8404 | 0.801 | +0.0394 | +0.0414 | +0.0264 |
| Compat Ultra | 0.8041 | 0.733 | +0.0711 | +0.0721 | — |

Span headroom: Low +0.0100, High +0.0190, Ultra +0.0181, Overdrive +0.0352, Low FSR 3.1
+0.0219, High FSR 3.1 +0.0310. N2e and N2f keep their amendment 3 budgets; the least
headroom after them is Tiered High's 0.0123 ms of total.

### The area-light sheet: what LTC costs where it is used

`MOBA_AREA_LIGHT_SHEET=1`: a sphere, a capsule and a rect light over open ground, on every
frame (`area_lights_avg` 3.09, `area_frames` 3,000 of 3,000), from the material sheet's far and
close cameras with no units. The baselines ignore the variable and draw the same ground with
no light there, so the branch's `MOBA_AREA_LIGHTS=off` run (the three shapes as point lights
at their centres) is the frame the feature is measured against. Two runs:

- `r1/gate7/`, interleaved with the teamfight rows above (N1, N2c and the branch);
- `r1/area/`, three more interleaved rounds of N2c and the branch with every area mode, so
  that LTC, the representative point and off are compared within the same rounds. The LTC
  rows reproduce across the two runs within 0.005 ms (0.760 and 0.760 far, 0.732 and 0.737
  close).

| Area-light sheet, `r1/area/` | N2c total (no light) | branch, off | branch, point | branch, LTC | `deferred_light` off → point → LTC | **LTC − off** | point − off |
|---|---|---|---|---|---|---|---|
| High, far | 0.704 | 0.714 | 0.721 | 0.760 | 0.065 → 0.072 → 0.111 | **+0.046** | +0.007 |
| High, close | 0.678 | 0.688 | 0.696 | 0.737 | 0.059 → 0.066 → 0.107 | **+0.049** | +0.008 |
| Low, far (the representative point) | 0.629 | 0.640 | 0.644 | — | 0.060 → 0.067 | — | +0.004 |

(Low's row is `area-low`, which runs the representative point by default, against
`areaoffsheet-low`; p95 moves with p50 in every row, within 0.003 ms.)

- **The feature's cost is LTC minus off: +0.046 ms far and +0.049 ms close at High**, all of
  it in `deferred_light` (0.065 → 0.111 ms far). It is LTC's arithmetic on the pixels inside
  the three lights' radii (two quad integrals and a sphere's cubic per light per pixel), not
  the registers: the mode-0 pipeline ran every off frame, and the LTC pipeline costs the
  teamfight's frames without a shaped light nothing (above). The representative point costs
  +0.007–0.008 ms at High and +0.004 ms at Low. Off itself is the three point lights the
  baselines do not have: +0.010–0.011 ms over N2c.
- **Against 1.10× N1** (`r1/gate7/`): N1 0.665 far and 0.632 close. The LTC sheet at High is
  **1.143× N1 far and 1.158× close**, against N1 without the lights; against an estimate of N1
  drawing them as points (N1 plus off's +0.010) it is about 1.126× far and 1.140× close.
  **This is over gate 7's 1.10×**, and is recorded as an amendment 4 item. Low (the
  representative point) is 1.082× N1 without the lights, and the point mode at High 1.081×:
  both within.
- **Per light:** the sheet's three lights each cover a few metres of open ground at both
  cameras, so about 0.015 ms per large shaped light on screen at 1920×1080. The teamfight's
  bolts (radius 0.06 m, on 11 % of its frames) cost +0.010 ms of p95 at High.
- **The levers left** (not tried, "How the cost was recovered"): the scalarised light loop
  and folding `light_classify` act on the whole pass, not on LTC's per-pixel work; the
  levers that would act on it are LTC only within a shaped light's inner radius (the
  representative point outside it), one integral for the diffuse of a small shape (the
  sphere's form factor is already analytic), or a lower area mode at High. None is measured.

## How the cost was recovered

The first LTC build put `deferred_light` at 117 registers on the RTX 5070 Ti (74 before; 80 is
the occupancy boundary) and the extended permutation at 128 with a spill. On the teamfight at
High (1280×720, one round, `quad-probe/`) that cost 0.009 ms of `deferred_light` and 0.015 ms
of GPU total against `fa661a7`, although only 11 % of its frames hold a shaped light:

| Probe (one round, 1280×720, teamfight High) | `deferred_light` | GPU total | ÷ `fa661a7` total |
|---|---|---|---|
| LTC everywhere (first build) | 0.039 | 0.376 | 1.042 |
| `MOBA_AREA_LIGHTS=point` | 0.031 | 0.369 | 1.017 |
| `MOBA_AREA_LIGHTS=off` | 0.030 | 0.368 | 1.017 |
| per-frame mode-0 pipelines (`plain-probe/`) | 0.031 | 0.368 | 1.022 |
| and the water at the representative point (`water-probe/`) | 0.030 | 0.366 | 1.008 |

- **Not register work in the shader.** The shapes as one quad each (a square for the
  sphere), the corners rotated once into the shading frame with the lobe's four terms applied
  inline, and the two integrals in a loop moved the count between 117 and 122. Stubbing the
  integrals showed why: the diffuse alone needs 97, the specular alone 80, both 117–121. The
  square sphere was timed against the disk on the area-light sheet over three rounds
  (`sphere-ab/`): 0.433 against 0.433 ms (far) and 0.425 against 0.424 ms (close), so the disk
  stays (it is three times closer to the reference).
- **A frame without a shaped light runs the mode-0 pipelines** (amendment 4 item 10): 74 and
  105 registers, N2c's.
- **The water pass** was the other 0.006 ms at 1280×720: `water.frag` includes the lighting
  and carried the LTC code on every frame. It now lights shapes at their representative
  point (item 11). At 1920×1080 that code still costs the water lane 0.002–0.003 ms on every
  teamfight frame (0.020 → 0.023 ms at High and Ultra, 0.019 → 0.021 at Low, 0.016 → 0.019
  at Overdrive), most of N2d's teamfight cost. Compiling the water at mode 0 (shapes as
  points at their centres) is the lever left for it; it was not done here, since it would
  invalidate every capture and hash above for 0.003 ms inside the budget.
- **The amendment 3 levers were not tried:** the scalarised light loop (amendment 2 item 5);
  the terrain without decal code on frames without a live `Paint` decal (the teamfight has
  about 3.3 per frame, so it would rarely run there); `light_classify` (0.005 ms) folded into
  the dispatch. The teamfight met the budget without them; on the area-light sheet the cost
  is LTC's arithmetic on the pixels the shapes reach (below). They stay open for N2e and
  N2f; what they would recover on the area-light sheet is not measured.

## Register counts on the RTX 5070 Ti

`MOBA_PIPELINE_STATS=1` on the area-light sheet at High and Low (`r1/stats/`):

| Pipeline | Registers | Local memory |
|---|---|---|
| `deferred_light`, `AREA_MODE` 2 (LTC) | 117 | none |
| `deferred_light_extended`, LTC | 128 | none |
| `deferred_light_plain` (mode 0, frames without a shaped light) | 74 | none |
| `deferred_light_extended_plain` | 105 | none |
| `deferred_light`, Low (mode 1, the representative point) | 77 | none |
| `deferred_light_extended`, Low | 105 | none |
| `light_classify` | 17 | none |

## SPIR-V

Against `fa661a7`'s build, 117 of the 126 modules are byte-identical, every Compat module
among them. The nine that change are Tiered: `deferred_light`, `deferred_light_extended`, and
the Tiered forward stages that include the lighting (`unit_tiered`, `unit_textured_tiered`,
`terrain_tiered`, `terrain_nodecals_tiered`, `foliage_tiered`, `water_tiered`,
`water_refraction_tiered`). No module is new or gone.

## Validation on the RTX 5070 Ti

`MOBA_VULKAN_VALIDATION=required` (validation and synchronisation validation), 1280×720, the
branch's release build (`realgpu-n2d.sh`, `r1/rtx/`). Each of the seven gate-7 presets (Tiered
Low, High, Ultra and Overdrive with TAAU, Low and High with FSR 3.1, Compat Ultra) captured
the default scene, the material sheet, the teamfight and the area-light sheet (the material
sheet under the three shaped lights, every frame with a shaped light). Eight more: the
material sheet and the area-light sheet on the forward Tiered frame (`MOBA_DEFERRED=off`), the
area-light sheet at Medium and at High with `MOBA_AREA_LIGHTS=point` and `off`, the
teamfight with the tile counts read back, and the teamfight with the decal pass at Low and
High. **All 36 ran with 0 validation messages.** The default scene and the material sheet
hold no shaped light (`area_frames=0`), so they ran the mode-0 pipelines; the area-light
captures ran LTC (or the representative point, Low) on every frame, so both pipelines of the
per-frame choice were validated. `deferred-parity.sh` on the RTX (information only): worst
pixel 3, 3, 3, 6 and 3/255 (default, sky off, river, teamfight, area-light sheet).

## What is not measured

- No reference GPU (#9/#127): the gates are relative, on the RTX 5070 Ti.
- No rendered white furnace or rendered Monte Carlo comparison: gate 3 is the CPU twin.
- The bolts' capsules are small (radius 0.06 m) and short-lived; the area-light sheet is the
  scene with shaped lights on every frame, and its numbers are the cost of LTC where it is
  used.
