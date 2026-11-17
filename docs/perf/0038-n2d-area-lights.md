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

[[SUMMARY]]

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

[[GOLDENS]]

## Gate 7: performance

[[GATE7]]

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
- **The water pass** was the other 0.006 ms: `water.frag` includes the lighting and carried
  the LTC code on every frame. It now lights shapes at their representative point (item 11).
- **The amendment 3 levers** were not needed for the teamfight: the scalarised light loop,
  the terrain without decal code on frames without a live `Paint` decal (the teamfight has
  about 3.3 per frame, so it would not run there) and `light_classify` folded into the
  dispatch are left for N2e and N2f.

## Register counts on the RTX 5070 Ti

`MOBA_PIPELINE_STATS=1`, High, the area-light sheet (`r1/rtx`'s stats run and the probes'
logs):

| Pipeline | Registers | Local memory |
|---|---|---|
| `deferred_light`, `AREA_MODE` 2 (LTC) | 117 | none |
| `deferred_light_extended`, LTC | 128 | none |
| `deferred_light_plain` (mode 0, frames without a shaped light) | 74 | none |
| `deferred_light_extended_plain` | 105 | none |
| `deferred_light`, Low (mode 1, the representative point) | 80 | none |
| `deferred_light_extended`, Low | 102 | none |
| `light_classify` | 17 | none |

## SPIR-V

Against `fa661a7`'s build, 117 of the 126 modules are byte-identical, every Compat module
among them. The nine that change are Tiered: `deferred_light`, `deferred_light_extended`, and
the Tiered forward stages that include the lighting (`unit_tiered`, `unit_textured_tiered`,
`terrain_tiered`, `terrain_nodecals_tiered`, `foliage_tiered`, `water_tiered`,
`water_refraction_tiered`). No module is new or gone.

## Validation on the RTX 5070 Ti

[[RTX]]

## What is not measured

- No reference GPU (#9/#127): the gates are relative, on the RTX 5070 Ti.
- No rendered white furnace or rendered Monte Carlo comparison: gate 3 is the CPU twin.
- The bolts' capsules are small (radius 0.06 m) and short-lived; the area-light sheet is the
  scene with shaped lights on every frame, and its numbers are the cost of LTC where it is
  used.
