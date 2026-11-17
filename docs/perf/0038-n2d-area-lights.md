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
- **Branch:** `feat/n2d-ltc-area-lights` on `fa661a7` (N2c). The sections from "Gate 2" on
  are the first round's evidence, on the build of `70c3c5f` (code-identical to the reviewed
  head `08b06a0`; called "pre-review" below). Review round 1 changed the shading and the
  frame; its measurements are in "Review round 1" and are probes (two or three interleaved
  rounds), not the final campaign, which waits on the maintainer's decision below.
- **Baselines:** release builds in their own worktrees: `ab42718` (N1's tiered forward frame,
  ADR-0023 §9b's baseline) and `fa661a7` (N2c).
- **Evidence:** frame logs, captures and register logs, with the scripts that ran them, are
  kept locally in `~/Projects/MOBA/handoff/evidence/n2d-ab/` (not committed): `r1/` holds
  the first round's runs, `*-probe/` the probes quoted under "How the cost was recovered",
  `review1-fix/` review round 1's probes, register logs, bench, parity and Compat runs. The
  scripts are N2c's (`scripts/env.sh`, `perf.sh`, `perf-round.sh`, `perfsum.py`,
  `parity-x11.sh`, `goldens.sh`), pointed at this branch, with the area-light
  configurations added; `compat-n2d.sh` and `realgpu-n2d.sh` are N2c's with this branch's
  scenes.

Every lavapipe and RTX run below printed no `[vulkan` line.

## Summary

| Gate | Result |
|---|---|
| 7. Performance | **Not met on feature-bearing frames, and a maintainer decision is pending (amendment 4 item 14).** Review round 1's rule: N2d's own cost (each area mode minus `MOBA_AREA_LIGHTS=off`, same build, same rounds, 1920×1080) at most 0.005 ms at Low, 0.008 at High, and 0.005 on the material sheet under the area-light sheet. After every lever (below), the area-light sheet at High costs +0.055 ms with LTC and +0.018 with the representative point; at Low the representative point (Low's default) +0.017; the combined sheet +0.055 (LTC) and +0.019 (point); the teamfight's frames with a shaped light +0.012 at High (LTC) and +0.006 at Low (p50 over those frames). Every mode fails every sheet, so neither LTC nor the representative point fits as the default at Medium and up, and amendment 4 item 14's fallback ("the representative point by default, LTC opt-in") does not fit either. The teamfight over all its frames is unchanged by the feature (+0.000 ms at High, +0.001 at Low) and, with the scalarised loop, about 0.01 ms faster than the pre-review build (`b7probe/`: 0.639 → 0.628 ms at High). Medium and Ultra were not timed in this round; Medium read as High in the first |
| 3. LTC | **Met** (the CPU twins of `ltc.glsl` and the per-light term): the three regression metrics within their tolerances (evaluation, diffuse, specular as a fraction of the lobe's energy: sphere 0.058, 0.0002, 0.099; capsule 0.145, 0.042, 0.056; rect 0.116, 0.002, 0.105); the plain relative errors recorded (specular sphere 0.537, capsule 0.587, rect 0.509); `RGBA16F` against the full-precision table at most 6.5e-4 absolute; a shrinking sphere becomes the engine's point light at the span cut-off (exact below a span of 0.08); the near-circular disk stable; the cut-off's errors recorded (amendment 4 item 15) |
| 2. Deferred–forward parity | **Met** on the head after review round 1, at 1280×720 on lavapipe with validation (`review1-fix/parity/`): worst pixel 3, 3, 3, 6 and 2/255 (default, default without the sky, river, teamfight, area-light sheet), at least 99.999 % within 2/255; identical with `MOBA_PAINT_DECAL_PASS=1`. The teamfight still reads 6/255 (five at 3, one at 4, one at 6), as in the first round |
| 5. Goldens | [[GOLDENS]] |
| Validation | [[VALIDATION]] |

## Review round 1: the levers and the decision rule

Review round 1 asked for the shading fixes first (amendment 4 items 1, 3, 8 and 15: the
near-field floor under LTC, the span cut-off with the engine's point light below it, the
stable disk, the representative point's normalisation, the radius at twice the extent, the
capsule's inside), then the performance levers, then one timing campaign against a stated
rule. The rule, on frames that carry the feature (the area-light sheet; the material sheet
under the area-light sheet, "combined"; the teamfight's frames with a shaped light, from the
frame log's new `gpu_area_frames`, `gpu_total_area_p50_ms` and `gpu_total_area_p95_ms`
columns, flagged per measured frame by the shaped lights that frame binned): **each area
mode minus `MOBA_AREA_LIGHTS=off`, in the same build and rounds, at most 0.005 ms at Low and
0.008 at High, and at most 0.005 on the combined sheet**. The teamfight's p50 over all frames
does not count, and the sheet's radii stay. If Medium and up fail after the levers, the
default there becomes whichever path fits, with LTC opt-in; if none fits, the decision is
the maintainer's.

**The probes** (`review1-fix/probe/`, `b7probe/`, `leverprobe/`): RTX 5070 Ti, 1920×1080,
3,000 frames per run, each round under the GPU lock with no other client, builds
interleaved. Their numbers are medians of two or three rounds; within a configuration the
rounds agree within 0.003 ms. The final campaign (five rounds, every preset, N1 and N2c
alongside) was not run: the rule is failed by every mode, so the default is the
maintainer's to choose first.

**After every lever** (`leverprobe/`, three rounds, the head as committed: `s2`; `gpu_total`
p50, ms):

| Feature-bearing frames | off | point | LTC | point − off | LTC − off | budget |
|---|---|---|---|---|---|---|
| Area-light sheet, High (far) | 0.708 | 0.726 | 0.763 | +0.018 | **+0.055** | 0.008 |
| Area-light sheet, Low (far; Low's default is the point) | 0.631 | 0.648 | — | **+0.017** | — | 0.005 |
| Combined sheet, High (close) | 0.715 | 0.734 | 0.770 | +0.019 | **+0.055** | 0.005 |
| Teamfight, frames with a shaped light (327 of 3,000), High: p50 (p95) | 0.589 (0.641) | — | 0.601 (0.655) | — | **+0.012** (+0.014) | 0.008 |
| Teamfight, frames with a shaped light, Low: p50 (p95) | 0.544 (0.595) | 0.550 (0.603) | — | **+0.006** (+0.008) | — | 0.005 |
| Teamfight, every frame, High (Low) | 0.629 (0.583) | — (0.584) | 0.629 | (+0.001) | +0.000 | not counted |

Without the scalarised loop (`s0`, the same rounds), whose gain is in the mode-0 pipelines
only (so it lowers `off` and widens every difference): area sheet High point +0.010, LTC
+0.048; Low point +0.007; combined point +0.014, LTC +0.052; teamfight's area frames High
+0.012, Low +0.004. Those fail too, on every sheet. The pre-review build (`probe/`, two
rounds): area sheet High point +0.008, LTC +0.048; Low point +0.007; combined point +0.007,
LTC +0.044.

**The levers and what each recovered** (on the configurations where it acts):

| Lever | What was built | Measured recovery |
|---|---|---|
| B1: per-tile area classes | `light_classify` sorts tiles into five classes: the standard and extended tiles split by whether a shaped light's screen bounds (up to 12 rectangles from the lights' bounding spheres, in the classify push block) touch them; only the area tiles run the area-mode pipelines, the rest the mode-0 ones, and a frame without a rectangle skips the area dispatches | none on the sheets, whose lights' bounds cover the screen ([[TILES]]); on the teamfight with B4 and B6, against the pre-review build, −0.002 ms p50 and −0.006 ms p95 at High (0.639 → 0.637, 0.657 → 0.651), 0.000 at Low; `light_classify` +0.001 ms on area frames |
| B2: lazy per-pixel invariants | the LTC lobe, its magnitude and Fresnel and the shading frame fetched once per pixel, on its first LTC light | register-neutral (127–128 hoisted or not); not timed apart from the rest |
| B3: the span cut-off | amendment 4 item 15 | none measurable on the sheets: their capsule spans at least 0.21 and their rect 0.11 wherever they light, and the sphere's far pixels are a small share; LTC − off moved +0.048 → +0.048 (`s0` against the pre-review build), with the cut-off's and the fixes' arithmetic together |
| B4: the water's mode-0/mode-1 pair | `water_tiered` and `water_refraction_tiered` built twice, `WATER_SHAPED_LIGHTS` (specialisation constant 0) on and off, picked per frame by whether it binned a shaped light | the teamfight's `water` lane 0.023 → 0.021 ms at High and 0.021 → 0.019 at Low (the pre-review build's water carried the representative point's code on every frame); registers 78 → 64 (simple water) and 72 → 64 (full) |
| B5: the forward Tiered frame | timed, not split | [[FORWARD]] |
| B6: no shape store for points | `bin` writes a light's shape `vec4` only when it has a shape | −0.12 µs of CPU per frame at 256 lights (7.48 → 7.38 µs, two interleaved bench runs); the bench gains `256_shaped` (8.8 µs), `256_rect` (9.5 µs) and `area_tiles/256_shaped` (7.8 µs, the per-frame rectangles) |
| B7: the scalarised light loop | each pass of `deferred_light`'s loop takes the subgroup's lowest pending light (`subgroupMin`), so the light's loads are uniform; per pixel the lights and their order are unchanged | in the mode-0 pipelines −0.006 to −0.008 ms of `deferred_light` (teamfight High 0.060 → 0.052, Low 0.056 → 0.049; the sheet with `off` 0.065 → 0.059) and 74 → 64 registers (extended 105 → 101); in the area-mode pipelines +0.002 ms (`b7probe/`), so it is compiled into the mode-0 pipelines only (`MOBA_SCALAR_LIGHT_LOOP` is `AREA_MODE == 0`) |
| A4's cost | the representative point's normalisation | +0.003 to +0.004 ms (point mode with it against without, same rounds: High 0.726 against 0.723, Low 0.648 against 0.644, combined 0.734 against 0.731) |

**Why nothing fits.** The sheet's three lights each cover several metres of ground in view,
so every lit pixel evaluates three shaped lights. The representative point's own arithmetic
(the closest point, the rect's cosine, the normalisation) on those pixels already costs more
than Low's 0.005 ms, before LTC; LTC's two quad integrals and a disk per light cost ten times
the budget. B1, B3 and B4 act where shaped lights are small or absent, which the sheets are
not. What is left for the maintainer, with the measured costs above (the error of each path
against the Monte Carlo reference is amendment 4 item 15's table):

| Default at Medium and up (and Low) | Cost on the sheets over `off` | Fidelity |
|---|---|---|
| `off`: every shape as the point at its centre | 0 | a point light's highlight; a rect lights behind itself |
| The representative point without its normalisation | +0.013 to +0.016 | highlights too bright for small rough lobes (relative error up to 8.69 at a span of 0.08) |
| The representative point with it (Low's default now) | +0.017 to +0.019 | relative error up to 0.449 at a span of 0.08 |
| LTC (Medium and up now) | +0.055 | relative error up to 0.280 at a span of 0.12, the point light below 0.08 |

Or a larger budget for N2d where shaped lights fill the screen, or a smaller sheet (which the
review ruled out for the measurement).

## Gate 3: LTC against Monte Carlo

The Rust twin of `include/ltc.glsl` (`crates/render/src/ltc/twin.rs`, step for step: the
bilinear fetch of the half-float table, the shading frame, the clipless quad, the sphere's
disk and cubic) is held against a fixed-seed Monte Carlo reference
(`ltc::tests::ltc_matches_monte_carlo`), and the per-light term around it (the span cut-off,
the representative point and its normalisation, the near-field floor, the radiance scales)
has its own twin, `ltc/shade.rs`. The reference integrates, over the true shape (ray hits
against the sphere, the capsule and the one-sided rect), the clamped cosine (the diffuse),
the engine's GGX specular at F0 0.04 and 1, and the LTC lobe itself, by cone sampling with up
to 400 strata. The cases: roughness 0.15, 0.3, 0.5, 0.75 and 1; view angles 0°, 40° and 75°;
four light directions (elevation 80°, 45°, 30° and 20°); two sizes, 0.25 m at 3 m and 0.75 m
at 2.5 m (the sphere's radius; the capsule a segment four times as long, of 0.4 times the
radius, tilted across the view; the rect turned 30° from facing the point).

Three regression metrics, worst over the 120 cases per shape, each under a tolerance ~15 %
above it:

- **evaluation error:** the LTC lobe's integral against Monte Carlo of the same lobe (the
  evaluation and, for the capsule, its quad, without the table's fit), `|LTC − MC| /
  max(MC, 0.02)`: relative above 0.02, absolute below, which is most cases;
- **diffuse error:** the form factor, `|LTC − MC| / max(MC, 0.001)`;
- **specular error as a fraction of the lobe's energy:** `|LTC − MC|` at F0 0.04 and 1 over
  that lobe's whole reflected energy; the table's fit and the evaluation together, not a
  relative error.

| Shape | Evaluation error | Diffuse error | Specular (fraction of lobe energy) | Tolerance | Plain relative: evaluation, diffuse, specular |
|---|---|---|---|---|---|
| Sphere | 0.058 | 0.0002 | 0.099 | 0.07, 0.002, 0.12 | 0.079, 0.000, 0.537 |
| Capsule | 0.145 | 0.042 | 0.056 | 0.17, 0.05, 0.07 | 0.145, 0.042, 0.587 |
| Rect | 0.116 | 0.002 | 0.105 | 0.13, 0.003, 0.12 | 0.141, 0.002, 0.509 |

The plain relative errors are `|LTC − MC| / MC` over the cases whose reference is at least
0.01; the specular's are the table's fit of GGX at grazing view, where a lobe's peak sits
about 5° from GGX's.

- **What the errors are:** the evaluation's is the clipless method's horizon (the sphere
  table stands in for clipping the polygon or ellipse in LTC space; worst on glossy lobes at
  grazing view), plus, for the capsule, its quad's square ends.
- **`RGBA16F` is kept:** `the_half_table_integrates_as_the_full_one` (run with
  `MOBA_LTC_FULL_TABLE` pointing at `ltc-bake`'s full-precision output, which the bake now
  writes as an optional second file) integrates every case with both tables: the largest
  absolute differences are 1.1e-5 (diffuse), 6.5e-4 (lobe × magnitude) and 1.5e-5 (lobe ×
  Fresnel), relative 2.5e-4, 1.9e-3 and 8.8e-4. The bake's own half-float rounding is at most
  4.9e-4 relative, and `the_table_is_well_formed` checks every texel. The baked table against
  the upstream's `ltc_1.dds`/`ltc_2.dds`: median difference 0 per channel, differing at the
  grazing row and at roughness 0 (largest relative difference 1.59).
- **The disk where it is near-circular:** the eigen decomposition of the transformed disk is
  now the guarded symmetric rotation (and the cubic's square roots are clamped), in the
  shader and the twin. `the_sphere_is_stable_where_its_disk_is_circular` read 0.80, 0.59 and
  1.00 off before the fix (N·V 0.8 at radii 0.85 and 1.0, N·V 1 at 0.5) and 0.000 after.
- **Radius → 0** (`a_shrinking_sphere_tends_to_the_engine_point_light`): against the
  engine's point light itself (GGX `D·Vis·F·N·L` with Fresnel at the half vector, the wrapped
  diffuse, the 1 m near-field floor), worst over 4 roughness, view, wrap and metal cases, 3
  directions, at 0.5 and 2 m:

  | span R/d | 0.3 | 0.2 | 0.12 | 0.1 | 0.08 and below |
  |---|---|---|---|---|---|
  | relative difference | 0.235 | 0.232 | 0.230 | 0.116 | 0 |

  Above the span cut-off (amendment 4 item 15) the difference is the sphere's size; at and
  below 0.08 the sphere is the point light. Before review this test compared with the LTC
  lobe's own point value (`the_ltc_sphere_tends_to_its_lobe_point_value`, kept), and the
  engine's point light read 0.43–1.11 of the result.
- **The span cut-off** (`the_span_cut_off_is_recorded`, amendment 4 item 15): the whole
  term against Monte Carlo at the thresholds, worst over the cases: at 0.08 (the point light
  at the centre; a rect's representative point) sphere 0.349, capsule 0.176, rect 0.307; at
  0.12 (LTC alone) 0.267, 0.273 and 0.280. The representative point (Low's path) reads 0.449,
  0.303 and 0.307 at 0.08 with its normalisation (item 8) and 8.69, 0.62 and 4.35 without.
- **The near-field floor under LTC** (`ltc_keeps_the_near_field_floor`): a sphere within
  8 % of the point light from 0.3 to 2 m. A capsule lights its inside as the whole
  hemisphere, with no step at its surface (`a_capsule_lights_its_inside`).
- **Power and geometry:** `every_shape_has_the_point_light_power`,
  `rect_axes_follow_the_normal`, `the_representative_point_is_on_the_shape`, and
  `a_long_thin_capsule_is_lit_along_its_length` (the light's radius is twice the shape's
  extent, item 1).
- **A capsule without length is its sphere** within 5 % (`a_capsule_without_length_is_its_sphere`).
- **The white furnace** (`white_furnace_with_multiscatter_is_one`) is unchanged: worst 0.9861.
  The split-sum LUT moved into the atlas texel for texel
  (`the_atlas_holds_both_tables`, and `the_shader_constants_match` pins the atlas size, the
  shapes' offset in the light buffer, the cut-off, the normalisation's constant and the
  near-field line).
- **Light-set consistency:** `forward_and_deferred_visit_the_same_lights` gives three lights
  of every four a shape (a sphere, a capsule, a rect) and every eighth a long capsule (its radius raised to twice its extent) and
  asserts the bins hold shaped lights; the forward and deferred loops visit the same lights
  under every cap.

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

*The first round, on the pre-review build. Review round 1's rule and probes are under
"Review round 1"; the five-round campaign on the head waits on the maintainer's decision.
Where the two disagree on a cost (Low's representative point read +0.004 ms here and
+0.007 ms in review round 1's probes on the same build), the probes' same-round differences
are the better figure: within a configuration the rounds agree within 0.003 ms, which is
the noise floor of any single difference below.*

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

**Medium** (the lowest preset with LTC; perf/0037 did not time it) ran afterwards in three
more interleaved rounds of the three builds (`r1/medium/`), the same binaries:

| Configuration | N1 total | N2c total | branch total | branch p95 | branch span | `deferred_light` N2c → branch | `water` N2c → branch | ÷ N1 total (span) | branch − N2c, p50 (p95) |
|---|---|---|---|---|---|---|---|---|---|
| Tiered Medium (TAAU) | 0.604 | 0.635 | 0.638 | 0.657 | 0.653 | 0.059 → 0.060 | 0.021 → 0.023 | **1.056** (1.070) | +0.003 (+0.010) |

Medium reads as High does (0.638 against 0.638 ms): a Tiered device runs the High
configuration tier at every preset (amendment 4 item 7), and Medium takes LTC as High does.

- **Every preset is within 1.10× N1** on `gpu_total` (1.020–1.067×, Medium 1.056×; span
  1.036–1.082×).
- **N2d's budget is met on p50** (amendment 3: at most 0.005 ms at Low, 0.008 ms at High):
  +0.002 at Low (TAAU and FSR 3.1), +0.003 at Medium, +0.004 at High and Ultra, +0.005 at
  Overdrive, +0.002 at High with FSR 3.1. **On p95 Medium and the High presets read
  +0.010–0.011 ms**: the 11 % of frames with a shaped light run the LTC pipelines, which is
  the cost a budget measured on p50 does not show. Most of the p50 cost is the water lane (above).
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
| Tiered Medium (TAAU), `r1/medium/` | 0.6644 | 0.638 | +0.0264 | +0.0294 | +0.0134 (High's budgets) |
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
| Medium, far, `r1/medium/` | 0.703 | 0.715 | — | 0.760 | 0.065 → 0.111 (off → LTC) | **+0.045** | — |

(Low's row is `area-low`, which runs the representative point by default, against
`areaoffsheet-low`; p95 moves with p50 in every row, within 0.003 ms.)

- **The feature's cost is LTC minus off: +0.046 ms far and +0.049 ms close at High**, all of
  it in `deferred_light` (0.065 → 0.111 ms far). Two causes are mixed in it, and **the split
  is not measured**: LTC's arithmetic on the pixels inside the three lights' radii (two quad
  integrals and a sphere's cubic per light per pixel), and the LTC pipeline's occupancy on
  every pixel of the pass (117 registers against the mode-0 pipeline's 74, which ran every
  off frame). The probe that ran the LTC pipeline on every teamfight frame, where almost no
  pixel is inside a shaped light, put the occupancy alone at 0.009 ms of `deferred_light` at
  1280×720 ("How the cost was recovered"); it was not measured at 1920×1080 (with the lights
  culled out of view the frame switches to the mode-0 pipeline). The representative point
  costs +0.007–0.008 ms at High and +0.004 ms at Low. Off itself is the three point lights
  the baselines do not have: +0.010–0.011 ms over N2c.
- **Against 1.10× N1** (`r1/gate7/`): N1 cannot draw the lights, so, as N2c's
  extended-model check did, the criterion is an estimate of N1 drawing them, here as point
  lights: N1's frame of the same view (0.665 ms far, 0.632 close) plus what the three point
  lights cost the branch (off − N2c, +0.010), so 0.675 and 0.642 ms. The estimate is not a
  measurement. **The LTC sheet at High is about 1.126× it far and 1.140× close (1.143× and
  1.158× N1's frame without the lights): over gate 7's 1.10× by about 0.018 ms far and
  0.026 ms close** (0.029 and 0.037 ms against N1 without the lights). It is recorded as
  amendment 4 item 14, with the levers below untried. Low (the representative point) is
  1.082× N1's frame without the lights, and the point mode at High 1.081×: both within.
  Medium is High's case (`r1/medium/`, N1 0.664 ms): LTC 1.145× N1's frame without the
  lights, about 1.128× the estimate (0.674 ms), over by about 0.019 ms.
  N2e and N2f have no headroom on this scene at High.
- **Per light:** the sheet's three lights each cover a few metres of open ground at both
  cameras, so about 0.015 ms per large shaped light on screen at 1920×1080, both causes
  together. The teamfight's
  bolts (radius 0.06 m, on 11 % of its frames) cost +0.010 ms of p95 at High.
- **The levers left, none tried or measured:** amendment 3's (the scalarised light loop,
  the terrain without decal code per frame, `light_classify` folded into the dispatch;
  "How the cost was recovered"), which act on the whole pass and so on the occupancy share;
  and, for the arithmetic, LTC only within a shaped light's inner radius (the representative
  point beyond it) or a lower area mode at High.

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
  the dispatch. The teamfight met the budget without them; the area-light sheet does not
  meet gate 7 at High (above), and what they would recover there is not measured. They stay
  open for N2e and N2f.

## Register counts on the RTX 5070 Ti

`MOBA_PIPELINE_STATS=1` at High and Low, the first round's build (`r1/stats/`) against the
head after review round 1 (`review1-fix/registers/s2-*.log`; the `deferred_light` lines also
for a build with the LTC invariants fetched per light, `nohoist-high.log`):

| Pipeline | First round | After review round 1 |
|---|---|---|
| `deferred_light`, `AREA_MODE` 2 (LTC) | 117 | 127 (128 with the light loop scalarised; 127 without the lazy invariants) |
| `deferred_light_extended`, LTC | 128 | 127 |
| `deferred_light_plain` (mode 0) | 74 | 64 (the scalarised loop) |
| `deferred_light_extended_plain` | 105 | 101 |
| `deferred_light`, Low (mode 1, the representative point) | 77 | 78 |
| `deferred_light_extended`, Low | 105 | 109 |
| `light_classify` | 17 | 17 |
| `water` (`water_tiered`), with and without the shaped-light code | 78 (always with it) | 78 and 64 |
| `water_full` (`water_refraction_tiered`), with and without | — | 72 and 64 |
| `terrain` (the forward `terrain_tiered`) | 80 | 96 |

The forward Tiered terrain's growth is the cut-off, the normalisation and the near-field
scale in its LTC branch; it draws the offscreen views and the `MOBA_DEFERRED=off` frame
(B5 above). `scripts/terrain-decal-check.sh`'s register limits are Compat's terrain and
Tiered's `terrain_gbuffer` (48, unchanged).

## SPIR-V

Against `fa661a7`'s build, after review round 1, 116 of the 126 modules are byte-identical,
every Compat module among them (`water.frag` and `water_refraction.frag` too: the water's
specialisation constant is Tiered-only). The ten that change are Tiered: `deferred_light`,
`deferred_light_extended`, `light_classify` (the area classes), and the Tiered forward stages
that include the lighting (`unit_tiered`, `unit_textured_tiered`, `terrain_tiered`,
`terrain_nodecals_tiered`, `foliage_tiered`, `water_tiered`, `water_refraction_tiered`). No
module is new or gone.

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

- **The final campaign on the head:** review round 1's numbers are two- and three-round
  probes; the five-round campaign at every preset against N1 and N2c, and the ratios to N1,
  wait on the maintainer's decision (amendment 4 item 14). Medium and Ultra were not timed
  in review round 1.
- **Exact tiered hashes for the head:** review round 1 changed the Tiered shading, so the
  committed hashes are the first round's (CI run 44083568862) until the one `cpu_sample`
  dispatch after the decision.
- No reference GPU (#9/#127): the gates are relative, on the RTX 5070 Ti.
- No rendered white furnace or rendered Monte Carlo comparison: gate 3 is the CPU twin.
- The bolts' capsules are small (radius 0.06 m) and short-lived; the area-light sheet is the
  scene with shaped lights on every frame, and its numbers are the cost of LTC where it is
  used.
