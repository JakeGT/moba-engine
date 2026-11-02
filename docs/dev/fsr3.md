# FSR 3.1.6 upscaler port

The open build's vendor-neutral temporal upscaler is our port of the AMD FidelityFX **FSR
3.1.6 upscaler** (ADR-0020 §6a and Q9, ADR-0021 §4b, amendment 1). Frame generation is not
ported. It is the `fsr3` cargo feature of `moba-render` (on by default) and needs nothing
fetched.

| Piece | Where | Upstream |
|---|---|---|
| The HLSL passes and headers, unmodified | `third_party/fidelityfx/` ([README](../../third_party/fidelityfx/README.md), `MANIFEST.sha256`) | FidelityFX SDK v2.4.0 |
| The build of the passes | `crates/shaders/build.rs` (`FSR3_PASSES`, `compile_fsr3`) | the SDK's shader permutations |
| The pass and binding table | `crates/shaders/src/fsr3.rs` (`moba_shaders::fsr3::PASSES`) | `ffx_fsr3upscaler_resources.h`, the passes' `register`s |
| The constants | `crates/render/src/fsr3/constants.rs` | `fsr3upscalerDispatch` and `setupDeviceDepthToViewSpaceDepthParams` (`ffx_fsr3upscaler.cpp`), `ffxSpdSetup` (`ffx_spd.h`), `FsrRcasCon` (`ffx_fsr1.h`) |
| Resources, descriptors, clears, barriers and dispatches | `crates/render/src/fsr3/context.rs` (`Fsr3`) | `fsr3upscalerDispatch`, `getInternalResourceDescriptions`, the Vulkan back end's clear jobs and samplers (`ffx_vk.cpp`) |
| The smoke test | `crates/render/src/smoke/fsr3.rs`, `fsr3-smoke` | — |

## Building the passes

`moba-shaders`' `fsr3` feature compiles nine modules (`fsr3_<pass>.spv`) with the pinned
`slangc` and validates each with `spirv-val` (mandatory, as for every Slang module):

| Module | Entry file | Workgroup | Dispatch |
|---|---|---|---|
| `fsr3_prepare_inputs` | `ffx_fsr3upscaler_prepare_inputs_pass.hlsl` | 8×8 | render ÷ 8 |
| `fsr3_luma_pyramid` | `…_luma_pyramid_pass.hlsl` | 256 | SPD: render ÷ 64 |
| `fsr3_shading_change_pyramid` | `…_shading_change_pyramid_pass.hlsl` | 256 | SPD: render ÷ 64 |
| `fsr3_shading_change` | `…_shading_change_pass.hlsl` | 8×8 | half render ÷ 8 |
| `fsr3_prepare_reactivity` | `…_prepare_reactivity_pass.hlsl` | 8×8 | render ÷ 8 |
| `fsr3_luma_instability` | `…_luma_instability_pass.hlsl` | 8×8 | render ÷ 8 |
| `fsr3_accumulate` | `…_accumulate_pass.hlsl` (`APPLY_SHARPENING=0`) | 8×8 | output ÷ 8 |
| `fsr3_accumulate_sharpen` | `…_accumulate_pass.hlsl` (`APPLY_SHARPENING=1`) | 8×8 | output ÷ 8 |
| `fsr3_rcas` | `…_rcas_pass.hlsl` | 64 | output ÷ 16 |

The flags are our Slang modules' (`-target spirv -profile spirv_1_6`,
`-matrix-layout-column-major`, `-O2`, `-fvk-use-entrypoint-name`, `-warnings-as-errors all`,
`-reflection-json`), entry point `CS`, plus:

- **`-lang slang`.** `slangc` reads `.hlsl` with its HLSL front end, which keeps a `for`
  loop's variable in scope after the loop (the legacy HLSL rule). Two vendor headers declare
  the same loop variable in consecutive loops (`iSampleIndex` in
  `ffx_fsr3upscaler_prepare_reactivity.h`, `row` in `ffx_fsr3upscaler_upsample.h`), which DXC
  accepts and that front end rejects as ambiguous (E39999; `-Gec` does not change it). The
  Slang front end scopes them as DXC does and compiles the vendor HLSL unchanged.
- **`-capability spvGroupNonUniformQuad`.** The two SPD passes use quad shuffles.
- **Register shifts** `-fvk-{b,t,u,s}-shift 0 16 32 48 0`: the HLSL `register(bN)`, `tN`,
  `uN` and `sN` become bindings 0+, 16+, 32+ and 48+ of set 0. No pass declares more than 16 of
  a kind.
- **Two warnings off:** 15205 (an undefined macro in `#if`, which the headers use for their
  console paths) and 41000 (unreachable code after a commented-out `return` in `ffx_spd.h`).
- **The permutation** (`FSR3_DEFINES`): `FFX_GPU`, `FFX_HLSL`, `FFX_HLSL_SM=62` and
  `FFX_IMPLICIT_SHADER_REGISTER_BINDING_HLSL=0` as the SDK's own build defines them;
  `FFX_HALF=0` (32-bit math everywhere: one permutation for every device, see amendment 1);
  HDR colour; render-resolution motion vectors without jitter; non-inverted depth
  (ADR-0021 §2a); and the reference Lanczos kernel (`REPROJECT_USE_LANCZOS_TYPE=0`, the SDK's
  choice on every device except 32- and 64-wide AMD ones, so the Lanczos lookup table is
  never bound).

`moba_shaders::fsr3::PASSES` lists every binding each pass declares, with `used: false` on
those its SPIR-V does not reference (the samplers but one, the lookup table, the SPD mips the
luma pyramid skips). Three tests hold the table to the build: the reflected parameters equal
it (name, binding, set 0, kind), the SPIR-V's `Binding` decorations equal its used bindings,
and the entry point and workgroup size match. A fourth checks the constant buffers' sizes, and
`moba-render` checks every field offset of its `#[repr(C)]` mirrors against the reflection.

## The host side

`Fsr3::new(device, max_render, max_output)` refuses a device without the tiered path, quad
subgroup operations in compute, or the format features below (the caller then falls back to
TAAU), and creates every internal image at the maximum sizes, as the SDK does:

| Image | Format | Size | Notes |
|---|---|---|---|
| accumulation ×2 | `R8_UNORM` | render | parity pair |
| luma ×2 | `R16_SFLOAT` | render | current and previous by parity |
| intermediate | `R16_SFLOAT` | render | the farthest depth, then the luma instability |
| shading change | `R8_UNORM` | half render | |
| new locks | `R8_UNORM` | output | |
| upscaled ×2 | `RGBA16F` | output | the history, parity pair; RCAS reads this frame's |
| SPD mips | `R16G16_SFLOAT` | half render, every level | storage views of levels 0–5 |
| farthest depth mip 1 | `R16_SFLOAT` | half render | |
| luma history ×2 | `RGBA16F` | render | parity pair |
| SPD counter | `R32_UINT` | 1×1 | |
| dilated reactive masks | `RGBA8_UNORM` | render | |
| frame info | `RGBA32F` | 1×1 | |
| dilated depth | `R32_SFLOAT` | render | |
| dilated motion | `R16G16_SFLOAT` | render | |
| reconstructed previous depth | `R32_UINT` | render | atomic min |

Format features (Vulkan 1.3 format features 2, optimal tiling): every float storage format,
and the caller's `RGBA16F` output, needs `STORAGE_WRITE_WITHOUT_FORMAT`; the three read back
through storage views (`R8_UNORM` new locks, `RGBA32F` frame info, `R16G16_SFLOAT` SPD mip 5)
also `STORAGE_READ_WITHOUT_FORMAT`; `R32_UINT` needs `STORAGE_IMAGE_ATOMIC`. The SPIR-V
capabilities `StorageImage{Read,Write}WithoutFormat` are allowed by Vulkan 1.3 itself, so no
device feature beyond the tiered set is enabled.

`record` follows `fsr3upscalerDispatch`:

1. **Constants** for this frame (`UpscalerConstants::advance`, the SPD set-up, `FsrRcasCon`)
   into this parity's slot of one host-visible uniform buffer.
2. **Descriptor sets**, one per pass and parity, rewritten only when the caller's views change.
   Because the history pairs swap by parity, two sets per pass cover every frame; with at most
   two frames in flight (the renderer's `FRAMES_IN_FLIGHT`), the GPU never uses the parity
   being rewritten.
3. **Clears.** The first frame moves every internal image to `GENERAL` (they stay there) and
   zeroes both accumulation and luma images. A reset zeroes this frame's accumulation and sets
   the frame info to (−1, 1, 0, 0). Every frame clears the reconstructed depth to the far plane
   (the bits of the float 1.0, as the SDK's Vulkan back end does on this `R32_UINT` image), the
   SPD counter and the SPD mips to 0.
4. **The passes** in the order of the table above (accumulate, or accumulate-and-sharpen and
   RCAS when the sharpness is above 0), each after a global compute-to-compute memory barrier.
5. The **resource frame index** advances modulo 16; its parity picks the history images.

The caller (the render graph, ADR-0021 §4a) moves the inputs to `SHADER_READ_ONLY_OPTIMAL` and
the output to `GENERAL` before `record`, and makes the output's compute writes visible to its
next reader.

Inputs as §4a defines them: motion vectors in render pixels to the previous position, so the
motion-vector scale is (1, 1) ÷ the render size; jitter in render pixels with FSR's sign (the
sample at pixel `i` shows the scene at `i + 0.5 − jitter`); a 1×1 linear exposure texture, so
the pre-exposure is 1; depth not inverted with a finite far plane. The tuning constants keep the
SDK's defaults (velocity factor 1, reactiveness and shading-change scales 1, accumulation
added per frame 1/3, minimum disocclusion accumulation −1/3).

## Render sizes (ADR-0021 §4c)

**Any render size from 1×1 to the context's maximum is accepted, frame to frame, without a
reset.** Every pass takes the frame's size from the constants (`iRenderSize`, and
`iPreviousFrameRenderSize` for reprojection), clamps its samples to that rectangle and sizes
its dispatch from it; the internal images are only allocated at the maximum.
`render_size_range(output, mode)` reports Ultra Performance's size to the mode's size, both
clamped to the maximum. The smoke checks it: inside one Quality context (192×108 output,
128×72 maximum), the render size steps through Balanced (112×63), Performance (96×54), Ultra
Performance (64×36), an odd 77×43 and back to 128×72 with no reset, and every step converges
within its error bound with no validation message, while every input holds sentinel values
outside the frame's rectangle (so a read outside it would show). A size above the maximum is
refused (`accepts`).

## The smoke test

```bash
MOBA_RENDER_PATH=tiered MOBA_VULKAN_VALIDATION=required cargo run --release -p moba-tools --bin fsr3-smoke
```

It upscales a synthetic scene whose ideal output is known exactly: a flat colour, a smooth
analytic field (static, moving one render pixel a frame with matching motion vectors, and
sharpened), the render-size sweep, and a fine field near the render resolution's limit, where
the converged error must be at most 0.9× the first frame's (the accumulation recovers detail).
Each phase's mean and largest error must stay under its bound, every output frame of a second
run on a fresh context must be bit-identical to the first, and any validation or
synchronization-validation message fails it. It runs in CI (render-golden) on lavapipe.
Flipping the jitter sign fails it (the moving and Ultra Performance phases exceed their bounds).
Results on lavapipe and an RTX 5070 Ti: [docs/perf/0030](../perf/0030-fsr3-port.md).

## Updating the SDK

1. Fetch the new SDK (`vendor-sdks.toml`'s `fidelityfx` entry), re-read its licence, and check
   every vendored file is still under its MIT list.
2. Replace `third_party/fidelityfx/` with the new files, unmodified, and regenerate
   `MANIFEST.sha256` (`find Kits -type f | sort | xargs sha256sum`).
3. Diff the upstream `ffx_fsr3upscaler.cpp` dispatch, resource list and constants against the
   Rust host code, and the passes' bindings against `moba_shaders::fsr3::PASSES` (the tests
   name every difference).
4. Run `fsr3-smoke` on lavapipe and a GPU, and record the result in the PR.
