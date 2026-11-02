# RTXDI through `slangc`: N0 compile spike (2026-11-01)

[ADR-0020](../adr/0020-next-gen-foundations.md) §4d says RTXDI's HLSL library is compiled with
`slangc`, and that this was **unverified** (upstream compiles it with DXC). If it failed, DXC
would be used for that vendor library only, behind the `rtxdi` feature. Verification gate 3
requires this spike to be recorded, pass or fail. This is that record.

**Verdict: PASS for ReSTIR DI, ReGIR, presampling and ReSTIR GI; FAIL for ReSTIR PT.**

- The unmodified RTXDI-Library headers for DI, ReGIR (grid and onion), presampling and GI
  compile with the pinned `slangc` **2026.20** to SPIR-V 1.6, using the engine's flags. The
  output passes `spirv-val --target-env vulkan1.3`.
- The library's ReSTIR PT headers do **not** compile. The errors are inside the library and
  come from Slang's HLSL-compatibility gaps (see [ReSTIR PT](#restir-pt)).
- **Consequence for the `rtxdi` feature:** DI and GI need no DXC fallback. A DXC fallback, or
  ported headers, would only be needed if ReSTIR PT is ever adopted. The roadmap does not
  schedule it.

Nothing from RTXDI is vendored in this repository. The spike ran in a scratch directory
outside it, and no upstream script, CMake or build tool was run.

## Sources

| What | Where | Revision |
|---|---|---|
| RTXDI v3.1.0 (2026-09-03) | <https://github.com/NVIDIA-RTX/RTXDI> | `a6efab966b7c3b272da0461578eb56ac61c7cbff` |
| `Libraries/Rtxdi` submodule: the shader library, the only part DI/GI need | <https://github.com/NVIDIA-RTX/RTXDI-Library> | `f12037fa8e97ebc08e9e3edfd2de528ed1772a4b` |
| `External/donut` submodule (sample shaders only) | <https://github.com/NVIDIA-RTX/Donut> | `2c1077673edb0e0d814c202e6ce8e502d245c2da` |

```bash
git clone --branch v3.1.0 --depth 1 https://github.com/NVIDIA-RTX/RTXDI ~/.cache/moba-spikes/rtxdi
cd ~/.cache/moba-spikes/rtxdi
git submodule update --init --depth 1 Libraries/Rtxdi External/donut   # non-recursive
```

The library is header-only HLSL under `Libraries/Rtxdi/Include/Rtxdi/`:

- `DI/`, `GI/`, `ReGIR/`, `LightSampling/`, `PT/` and `Utils/`;
- the C++/HLSL parameter headers `RtxdiParameters.h`, `DI/ReSTIRDIParameters.h` and
  `GI/ReSTIRGIParameters.h`.

The application supplies the "RTXDI Application Bridge" (`RAB_*`), described in
`Doc/RtxdiApplicationBridge.md`:

- the types `RAB_Surface`, `RAB_Material`, `RAB_LightInfo` and `RAB_LightSample`;
- about 30 functions (surface and material loads, light sampling and PDFs, BRDF sampling,
  visibility rays);
- the reservoir, neighbour-offset and RIS buffer macros.

## Flags and tools

- `slangc` 2026.20, the [pinned toolchain](slang.md), run with the engine's Slang flags
  (`crates/shaders/build.rs`) and the RTXDI include path:

  ```bash
  slangc <file>.hlsl -target spirv -profile spirv_1_6 -matrix-layout-column-major -O2 \
    -fvk-use-entrypoint-name -I ~/.cache/moba-spikes/rtxdi/Libraries/Rtxdi/Include \
    [extra] -o out.spv
  ```

- `spirv-val --target-env vulkan1.3 out.spv`, with SPIRV-Tools v2026.4.

## Results

### A. Stub bridge, ReSTIR DI and presampling

The test unit is a stub bridge of about 150 lines, with explicit `[[vk::binding]]`s and inline
`RayQuery` visibility, plus two `[shader("compute")]` entry points, emitted in one module:

- `PresampleLights` covers local, environment-map and ReGIR presampling.
- `ReSTIRDI` (8×8) covers:
  - `RTXDI_SampleLightsForSurface` (local, infinite, environment and BRDF);
  - `RTXDI_DITemporalResampling`;
  - the boiling filter;
  - `RTXDI_DISpatialResampling`, plain and `WithPairwiseMIS`;
  - `RTXDI_DISpatioTemporalResampling`, both variants;
  - the reservoir store.

The library knobs are defined first:

- `RTXDI_ALLOWED_BIAS_CORRECTION` and `RTXDI_GI_ALLOWED_BIAS_CORRECTION`;
- `RTXDI_ENABLE_BOILING_FILTER` and `RTXDI_BOILING_FILTER_GROUP_SIZE 8`.

The first two errors were bugs in our stub: a `Texture2D<float>` where `RTXDI_TEX2D` is
`Texture2D`, and a missing boiling-filter define.

| Variant (extra args) | Result | Warnings | SPIR-V | `spirv-val` |
|---|---|---|---|---|
| default (`RTXDI_REGIR_MODE=0`) | pass | 19 | 162,028 B | ok |
| `-DRTXDI_REGIR_MODE=1` (grid) | pass | 19 | 173,540 B | ok |
| `-DRTXDI_REGIR_MODE=2` (onion) | pass | 19 | 177,628 B | ok |
| bridge without `RayQuery` | pass | 19 | 159,444 B | ok |
| `-lang hlsl` | pass | 19 | identical bytes | ok |

- **Warnings.**
  - 18 are `30081` ("implicit conversion from 'uint' to 'bool' is not recommended"). All of
    them are in the library, which uses `uint` flags as booleans, for example
    `Utils/Math.hlsli:41`.
  - One is `41012` ("profile implicitly upgraded") for subgroup ballot and arithmetic, which
    come from the boiling filter's `WaveActiveSum`, and for `RayQueryKHR`.
- **Ray query** comes only from the bridge's visibility callbacks. The library itself issues
  no ray queries.

### B. Stub bridge, ReSTIR GI

The unit covers the GI reservoir make, temporal, boiling-filter, spatial, spatiotemporal and
store functions. It passes with 7 warnings (6 × `30081` and 1 × `41012`). The output is
56,808 B, and `spirv-val` passes.

### C. Under the engine's `-warnings-as-errors all`

With that flag, DI and GI fail on the warnings above. Two additions make every DI variant
(ReGIR modes 0–2) and GI build with **zero warnings** and the same bytes:

- `-warnings-disable 30081`. It must be the numeric id: `E30081` is rejected as an unknown
  diagnostic.
- An explicit `-capability spvRayQueryKHR -capability spvGroupNonUniform -capability
  spvGroupNonUniformBallot -capability spvGroupNonUniformArithmetic`.

### D. Parameter layout

A `ConstantBuffer` holding every DI, GI and ReGIR parameter struct has 161 fields and is
1,712 B. `-reflection-json` reports identical offsets with and without `-fvk-use-dx-layout`,
so the shared C++ parameter headers need no DX-layout flag.

### E. NVIDIA's MinimalSample (`Samples/MinimalSample/Shaders/Render.hlsl`, unmodified)

The sample combines a real bridge with spatiotemporal DI. It was compiled with `-entry main
-stage compute -D SPIRV -I External/donut/include`:

- **Without `-unscoped-enum`:** it fails with 12 errors, beginning with `error[E30015]:
  undefined identifier 'GeomAttr_Position'` (`SceneGeometry.hlsli:79`). Slang enums are scoped
  by default.
- **With `-unscoped-enum`:** it passes and `spirv-val` passes (162,148 B). 56 of the 84
  warnings are bindings: `register()` without a Vulkan binding, and t0, u0 and b0 overlapping
  on binding 0. That overlap is a real descriptor clash which `spirv-val` does not catch.
- **With NVRHI's shifts as well** (`-fvk-t-shift 0 all -fvk-s-shift 128 all -fvk-b-shift 256
  all -fvk-u-shift 384 all`): 28 warnings remain. They include `15601`, which reports that the
  sample's `#pragma pack_matrix(row_major)` is ignored.

### F. NVIDIA's FullSample, unmodified

All 19 DI, GI and presampling compute shaders were built with the flags in E plus `-D
USE_RAY_QUERY=1`, without NRD or NVAPI. All 19 fail.

- **The single error:** every one of them is `error[E30050]: mutating method cannot be called
  on immutable value`, at 23 sites in **sample** code. For example:
  - `PolymorphicLight.hlsli:823`: `SphereLight::Create(lightInfo).CalcSample(...)`, a method
    called on a temporary;
  - `PSRUtils.hlsli:167`: a method called on a `const` parameter.
- **Why:** Slang treats HLSL struct methods as mutating, and DXC accepts these calls.
- **The fix:** in a scratch copy, binding each temporary to a local (20 lines) and dropping one
  `const` makes all 19 compile and pass `spirv-val`. They range from 26,988 B (PrepareLights)
  to 523,052 B (DI FusedResampling with onion ReGIR).
- **Upstream quirks shown by Slang's warnings:**
  - `RTXDI_ALLOWED_BIAS_CORRECTION` is used without a default in
    `DI/TemporalResampling.hlsli:173`, so it is 0 unless defined. DXC behaves the same way,
    silently.
  - The sample has a typo, `RTXDI_REGIR_MODE_DISABLED` for `RTXDI_REGIR_DISABLED`.

### ReSTIR PT

The library's `PT/` headers fail, through FullSample's `LightingPasses/PT/*`, for two reasons:

- **No templates.** `Rtxdi/PT/PathTracerContext.hlsli:30` declares
  `template<typename PTContextType>`, and the error is `error[E20009]: expected token ... '>'
  expected`. Slang has generics, not C++-style templates. This breaks GenerateInitialSamples,
  TemporalResampling and SpatialResampling.
- **Loop-variable scope.** `Rtxdi/PT/SpatialNeighborSelection.hlsli:71` gives
  `error[E39999]: ambiguous reference to 'k'`: two sibling `for (uint k ...)` loops depend on
  the legacy HLSL loop-variable scope.

Both fixes are mechanical (template → generic, renaming the loop variable). They would sit
inside vendor headers, though, so ReSTIR PT means either DXC or carrying a patch.

## What the `rtxdi` integration should do

- **Pinning.** Pin the RTXDI-Library commit and include its headers unmodified for DI, ReGIR
  and GI. Write our own bridge in Slang-friendly HLSL, or in Slang:
  - don't call methods on temporaries or on `const` values;
  - use no unscoped enums;
  - use explicit `[[vk::binding]]` rather than `register()`.
- **Defines.** Define `RTXDI_ALLOWED_BIAS_CORRECTION`, `RTXDI_GI_ALLOWED_BIAS_CORRECTION` and
  the boiling-filter macros before the includes.
- **Warnings.** Add `-warnings-disable 30081` for the vendor modules only, and declare the
  capabilities explicitly. The engine's `-warnings-as-errors all` then stays clean.
- **Device requirements.**
  - Subgroup ballot and arithmetic are needed only with the boiling filter.
  - Ray query is needed only through the bridge.
- **Matrices.** The library has no matrices in its buffers, so `-matrix-layout-column-major`
  is harmless. Code ported from the samples (Donut view constants) must choose its matrix
  convention deliberately, because Slang ignores `#pragma pack_matrix`.
- **Compile time.** About 0.8 s for the stub DI module and about 3 s for FullSample's fused DI
  shader.

## Reproducing

The spike files are in `~/.cache/moba-spikes/rtxdi-spike/` on the maintainer's Linux machine:

| File | What it is |
|---|---|
| `stub_bridge.hlsli` | the stub bridge |
| `rtxdi_di_spike.hlsl`, `rtxdi_gi_spike.hlsl` | the test units |
| `run.sh` | compile with the flags above, count warnings, run `spirv-val` |
| `full_sample.sh` | build the FullSample shaders |
| `patch_fullsample.py` | write the scratch FullSample copy |
| `repro.sh` | run everything except the PT and layout checks |
| `out/*.log` | full diagnostics |

The files are not committed. They are throwaway, and the bridge is not the one the `rtxdi`
feature will ship.
