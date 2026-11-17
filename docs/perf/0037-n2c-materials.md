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
- **Baselines:** detached release builds in their own temporary worktrees, deleted afterwards:
  `ab42718` (the N1 tiered forward frame, ADR-0023 §9b's baseline) and `ceca947` (N2b).

Every lavapipe and RTX run below printed no `[vulkan` line.

## Summary

| Gate | Result |
|---|---|
| 1. Encoding and interfaces | **Met.** `MaterialRecord` and `MaterialPush` against the Slang reflection; the GLSL-vertex / Slang-fragment interface test; the record's model word |
| 2. Deferred–forward parity | **Met** locally at 1280×720: worst pixel 3, 3, 3 and 6/255 (default, default without the sky, river, teamfight); CI below |
| 3. White furnace | **Met** (CPU model of the shader's fetch): worst 0.9861 over roughness 0–1 and N·V 0.05–1 |
| 4. Hidden information (decals) | **Partly:** unit pixels away from a glow decal keep their G-buffer albedo with decals on and off; see below |
| 5. Goldens | **Met.** Compat byte-identical (11 scenes); `materials` and `decals` tiered goldens two-run identical; exact hashes from CI |
| 7. Performance (≤ 1.10× N1) | **Met** on every preset: GPU total 1.040–1.077× `ab42718` (GPU span 1.057–1.091×), 1.012–1.026× `ceca947`; Compat 0.999× |
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
| default | 100.000 % | 3 | 2 (3/255) |
| default, `MOBA_SKY=off` | 100.000 % | 3 | 2 |
| R3b river | 100.000 % | 3 | 2 |
| R3b teamfight | 99.999 % | 6 | 7 (five at 3, one at 4, one at 6) |

Two fixes made the teamfight pass. Before them, the worst pixel was 28/255:

- splat droplets outside the forward decal grid's cell drew only in the deferred frame. The
  decal pass now reads the grid (amendment 3 item 16);
- the white furnace's LUT (amendment 3 item 10).

The 6/255 pixel is on a unit's edge inside a glow decal's ring (N2b's teamfight worst was
5/255). CI: PARITYCI.

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

- Three units and the structure keep every albedo byte.
- The five units inside or at the edge of a glow decal's ring differ on 830 pixels by at most
  1/255. 775 of those pixels are on the two units inside the rings.

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
    item 2); no shipped material has a mask.
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
  - The eight existing fingerprints still pass (mean ≤ 0.146, max ≤ 5), so they are not
    rewritten.
  - The four new ones were written locally at 640×360.
  - The exact hashes come from CI only: HASHES.

## Gate 7: performance

- **Builds:** `ab42718`, `ceca947` and the branch at `7106fb2`: release builds, the first two in
  detached worktrees.
- **Machine:** the RTX 5070 Ti (driver 620.70.06, Linux 7.3.4).
- **Replay:** the teamfight replay (`MOBA_WARMUP_TICKS=330`), its `Paint` decals included, at
  1920×1080. Vsync off, the 240 fps limit, validation off, the pipeline cache off, 3,000 frames
  per run.
- **Configurations:** Tiered Low, High and Ultra with TAAU, Tiered Overdrive (Ultra, TAAU
  Performance mode), Tiered Low and High with FSR 3.1, and Compat Ultra.
- **Rounds:** three, interleaved. Each held `flock handoff/gpu.lock` and ran every
  configuration on the three builds in turn, with no other `moba-client` running.
- **Results:** medians of the three rounds, in ms. Within each configuration and build, the
  three GPU totals agree within 0.006 ms.

| Configuration | Build | GPU total p50 | p95 | GPU span p50 | `prepass` | `gbuffer_decals` | `gbuffer_rest` | `deferred_light` | ÷ N1 total (span) | ÷ N2b total |
|---|---|---|---|---|---|---|---|---|---|---|
| Tiered Low (TAAU) | `ab42718` | 0.508 | 0.520 | 0.514 | 0.020 | — | — | — | | |
| | `ceca947` | 0.540 | 0.551 | 0.554 | 0.063 | — | — | 0.053 | 1.063 (1.078) | |
| | branch | 0.547 | 0.559 | 0.561 | 0.045 | 0.006 | 0.021 | 0.054 | **1.077 (1.091)** | 1.013 |
| Tiered High (TAAU) | `ab42718` | 0.560 | 0.572 | 0.566 | 0.024 | — | — | — | | |
| | `ceca947` | 0.583 | 0.596 | 0.598 | 0.069 | — | — | 0.056 | 1.041 (1.057) | |
| | branch | 0.596 | 0.608 | 0.609 | 0.052 | 0.006 | 0.024 | 0.058 | **1.064 (1.076)** | 1.022 |
| Tiered Ultra (TAAU) | `ab42718` | 0.559 | 0.572 | 0.566 | 0.024 | — | — | — | | |
| | `ceca947` | 0.584 | 0.597 | 0.599 | 0.069 | — | — | 0.056 | 1.045 (1.058) | |
| | branch | 0.596 | 0.608 | 0.609 | 0.052 | 0.006 | 0.024 | 0.058 | **1.066 (1.076)** | 1.021 |
| Tiered Overdrive | `ab42718` | 0.502 | 0.513 | 0.508 | 0.026 | — | — | — | | |
| | `ceca947` | 0.509 | 0.521 | 0.524 | 0.055 | — | — | 0.035 | 1.014 (1.031) | |
| | branch | 0.522 | 0.533 | 0.537 | 0.042 | 0.005 | 0.022 | 0.036 | **1.040 (1.057)** | 1.026 |
| Tiered Low, FSR 3.1 | `ab42718` | 0.656 | 0.669 | 0.662 | 0.020 | — | — | — | | |
| | `ceca947` | 0.687 | 0.700 | 0.701 | 0.063 | — | — | 0.053 | 1.047 (1.059) | |
| | branch | 0.695 | 0.708 | 0.708 | 0.045 | 0.006 | 0.021 | 0.054 | **1.059 (1.069)** | 1.012 |
| Tiered High, FSR 3.1 | `ab42718` | 0.706 | 0.720 | 0.713 | 0.024 | — | — | — | | |
| | `ceca947` | 0.731 | 0.744 | 0.747 | 0.069 | — | — | 0.056 | 1.035 (1.048) | |
| | branch | 0.744 | 0.757 | 0.758 | 0.052 | 0.006 | 0.024 | 0.058 | **1.054 (1.063)** | 1.018 |
| Compat Ultra | `ab42718` | 0.676 | 0.686 | 0.680 | 0.058 | — | — | — | | |
| | `ceca947` | 0.675 | 0.685 | 0.680 | 0.058 | — | — | — | 0.999 (1.000) | |
| | branch | 0.675 | 0.686 | 0.679 | 0.058 | — | — | — | **0.999 (0.999)** | 1.000 |

- **Every preset is within 1.10× N1**. The closest is Tiered Low's GPU span, at 1.091×.
- **The split G-buffer** costs 0.009 ms more than N2b's single pass at Low:
  - `prepass` (now the terrain alone), `gbuffer_decals` and `gbuffer_rest` total 0.072 ms,
    against N2b's 0.063 ms `prepass`;
  - the decal pass itself is 0.006 ms on the teamfight's decals;
  - the rest is the extra pass boundary and `gb0`'s load.
- **Before `FOLIAGE` moved to the standard tiles** (amendment 3 item 11), round 1 of the same
  replay measured Tiered Low at 1.108× total and 1.123× span: over the gate. That is why it
  moved.
- **Pipelines:** Tiered 52 (N2b 51, N1 46), FSR 3.1 60 (59, 54), Compat 42 (unchanged). The one
  added is the decal pipeline. The Slang material G-buffer stage replaces the GLSL textured one
  in the same pipeline.
- `locked_uploads=0` and `locked_pipelines=0` in every run.
- **VRAM at High:** 247.5 MB, the same as N2b (N1 236.2). The material table is 4 KiB and
  `gb0`'s sRGB view adds no memory.
- The raw logs are kept locally, not committed.

## Validation on the RTX 5070 Ti

`MOBA_VULKAN_VALIDATION=required` (validation and synchronisation validation), 1280×720
(1920×1080 swapchain), on the branch's release build. Each of the seven gate-7 presets
(Tiered Low, High, Ultra and Overdrive with TAAU, Low and High with FSR 3.1, Compat Ultra)
captured:

- the default scene;
- the material sheet;
- the teamfight decal scene.

Two more captures: the sheet on the forward Tiered frame (`MOBA_DEFERRED=off`), and the
teamfight with `MOBA_DECALS=off`. All 23 ran on the RTX with **0 validation messages**.

`deferred-parity.sh` on the RTX (information only; the gate is lavapipe's):

| Scene | Within 2/255 | Worst |
|---|---|---|
| default | 100.000 % | 3 |
| default, sky off | 100.000 % | 3 |
| river | 100.000 % | 3 |
| teamfight | 99.998 % | 15 (one pixel at 15, one at 13) |

The teamfight's two pixels are at (1716–1717, 964), on a unit's edge. N2b's own RTX run
(`ceca947`'s review) had the same two pixels at 13 and 15/255, so N2c did not introduce them.
They are NVIDIA's arithmetic, not a path difference: lavapipe and CI read 6/255 at most.
