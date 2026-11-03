# FSR 3.1.6 upscaler port

The open build's vendor-neutral temporal upscaler is our port of the AMD FidelityFX **FSR
3.1.6 upscaler** (ADR-0020 §6a and Q9, ADR-0021 §4b, amendments 1 and 4). Frame generation is
not ported. It is the `fsr3` cargo feature of `moba-render` (on by default) and needs nothing
fetched. In the frame it is a `TemporalUpscaler` back end, chosen with `MOBA_UPSCALER=fsr3`
(see [In the renderer](#in-the-renderer)); `auto` keeps TAAU (ADR-0021 Q1, amendment 4).

| Piece | Where | Upstream |
|---|---|---|
| The HLSL passes and headers, unmodified | `third_party/fidelityfx/` ([README](../../third_party/fidelityfx/README.md), `MANIFEST.sha256`) | FidelityFX SDK v2.4.0 |
| The build of the passes | `crates/shaders/build.rs` (`FSR3_PASSES`, `compile_fsr3`) | the SDK's shader permutations |
| The pass and binding table | `crates/shaders/src/fsr3.rs` (`moba_shaders::fsr3::PASSES`) | `ffx_fsr3upscaler_resources.h`, the passes' `register`s |
| The constants | `crates/render/src/fsr3/constants.rs` | `fsr3upscalerDispatch` and `setupDeviceDepthToViewSpaceDepthParams` (`ffx_fsr3upscaler.cpp`), `ffxSpdSetup` (`ffx_spd.h`), `FsrRcasCon` (`ffx_fsr1.h`) |
| Resources, descriptors, clears, barriers and dispatches | `crates/render/src/fsr3/context.rs` (`Fsr3`, its `TemporalUpscaler` impl) | `fsr3upscalerDispatch`, `getInternalResourceDescriptions`, the Vulkan back end's clear jobs and samplers (`ffx_vk.cpp`) |
| Choosing it, and the fallback to TAAU | `crates/render/src/upscale/mod.rs` (`resolve`, `create`) | — |
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

Format features (Vulkan 1.3 format features 2, optimal tiling), read off the passes' SPIR-V:
every float storage format, and the caller's `RGBA16F` output, needs
`STORAGE_WRITE_WITHOUT_FORMAT` (they are declared `Unknown`); the two read back through
storage views without a format (`RGBA32F` frame info, `R16G16_SFLOAT` SPD mip 5) also
`STORAGE_READ_WITHOUT_FORMAT` (the new locks are declared `R8` and the reactive masks
`Rgba8`, so they need neither); the formats sampled through the linear sampler need
`SAMPLED_IMAGE_FILTER_LINEAR` (all but `R32_SFLOAT`, `RGBA32F` and `R32_UINT`, which are only
loaded); `R32_UINT` needs `STORAGE_IMAGE_ATOMIC`. The SPIR-V capabilities
`StorageImage{Read,Write}WithoutFormat` are allowed by Vulkan 1.3 itself, so no device feature
beyond the tiered set is enabled.

Each pass has one descriptor set per frame-in-flight slot and history parity (`SET_RING`
slots, 2, × 2 parities), and the constants one block of each kind per slot. The slot is the
trait's `UpscaleInputs::frame_slot` (ADR-0021 amendment 2, item 17); the parity is the
resource frame index's, because the history pairs swap by parity. The internal descriptors
(the constant blocks, the sampler, the internal images) are written once, at creation and
after `resize`. The descriptors that name the caller's images are rewritten in this frame's
slot and parity on every `record`, from fixed arrays, so a view the caller recreated is never
read through a stale descriptor, even one whose handle value was reused.

`record` (the `TemporalUpscaler` method) is an `unsafe fn` with the trait's contract: every
recorded command buffer is submitted exactly once, in `record` order, on one queue (the
context assumes its images were transitioned and cleared by the records before, and orders
each frame only against the previous one on that queue); at most `FRAMES_IN_FLIGHT` records
are pending and `frame_slot`'s last submission has finished (this record rewrites that slot's
sets and constants, which the renderer asserts at compile time with `SET_RING >=
FRAMES_IN_FLIGHT`); and the caller's images live until the GPU is done. A caller that drops a
recorded command buffer unsubmitted calls `reset_history` (the renderer's
`forget_unsubmitted`), which makes the next record a first frame: every internal image is
re-initialised, after a barrier that waits for every earlier compute and clear on the queue
(`first_frame_barrier`, pinned by a unit test, since synchronization validation does not flag a
`NONE` source). `resize` (new maximum sizes: new images, the pipelines kept; an `unsafe fn`
on the trait) and dropping the context both need the GPU to have finished all of its work; the
renderer idles the device first, as for every pass. `resize` builds the new images before
dropping the old ones, so it briefly holds both sets.

`record` follows `fsr3upscalerDispatch`:

1. **Constants** for this frame (`UpscalerConstants::advance`, the SPD set-up, `FsrRcasCon`)
   into this frame slot's blocks of one host-visible uniform buffer.
2. **The caller's descriptors** in this slot's and parity's sets, as above.
3. **Clears.** The first frame (at creation, after `resize` or `reset_history`) moves every
   internal image to `GENERAL` (they stay there) and initialises every one of them: zero, except the reconstructed depth (the far plane) and the
   frame info (−1, 1, 0, 0). The SDK leaves most of them uninitialised; clearing them once
   means no pass can read garbage on the first frame (the new locks, for one, are written only
   where a lock forms and read everywhere). Later frames: a reset zeroes this frame's
   accumulation and sets the frame info to (−1, 1, 0, 0), and every frame clears the
   reconstructed depth to the far plane (the bits of the float 1.0, as the SDK's Vulkan back
   end does on this `R32_UINT` image), the SPD counter and the SPD mips to 0, whole images, as
   the SDK does.
4. **The passes** in the order of the table above (accumulate, or accumulate-and-sharpen and
   RCAS when the sharpness is above 0), each after a global compute-to-compute memory barrier.
   The SPD pyramids hand data between work groups inside one dispatch (the last group, counted
   with an atomic, reads the others' `globallycoherent` level 5): the upstream pattern.
5. The **resource frame index** advances modulo 16; its parity picks the history images.

A context whose maximum render size is below 64 has fewer than six SPD levels; the views of
the missing levels alias the last one so every pass's six bindings stay bound. They are never
written: SPD writes level `i` only for `i < floor(log2(max(render)))`, which fits the image
for any render size up to the maximum.

The caller (the render graph, ADR-0021 §4a) moves the inputs to `SHADER_READ_ONLY_OPTIMAL` and
the output to `GENERAL` before `record`, and makes the output's compute writes visible to its
next reader.

Inputs as §4a defines them: motion vectors in render pixels to the previous position, so the
motion-vector scale is (1, 1) ÷ the render size; jitter in render pixels with FSR's sign (the
sample at pixel `i` shows the scene at `i + 0.5 − jitter`); a 1×1 linear exposure texture, so
the pre-exposure is 1; depth not inverted with a finite far plane. The tuning constants keep the
SDK's defaults (velocity factor 1, reactiveness and shading-change scales 1, accumulation
added per frame 1/3, minimum disocclusion accumulation −1/3).

## In the renderer

`upscale::create` builds the back end `MOBA_UPSCALER` resolves to (`upscale::resolve`): with
`fsr3`, an `Fsr3` for the swapchain extent with the dynamic-resolution ceiling as its maximum
render size; when the device cannot run it (`Fsr3::new`'s refusal above), TAAU with one log
line. Only the chosen back end exists, so TAAU's history is never allocated while FSR 3.1 runs
(`upscaler_vram_mb` in the frame log is FSR's own). `auto` is TAAU in the open build
(`AUTO_OPEN_BUILD`, ADR-0021 Q1; the decision and its numbers are amendment 4 and
[perf/0033](../perf/0033-fsr3-in-frame.md)). The client's mode maps by ratio (`Native` is FSR's
native AA). FSR reports its own range, the SDK's truncated sizes (1706 at 2560 Quality, where
TAAU and the controller round to 1707), but the controller is not clamped to it: the context
accepts every size up to its maximum, which is the controller's ceiling. `MOBA_DEBUG_VIEW`'s
temporal views are drawn by TAAU only (one log line under FSR 3.1).

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
sharpened), the render-size sweep (with the first frame after each change checked, then 9×5,
2×1 and 1×1, and back), and a fine field near the render resolution's limit, still and moving,
where the converged error must be at most 0.9× (0.95× moving) the first frame's: the
accumulation recovers detail, and when moving only if the reprojection is right. Two
**occluder** runs follow (ADR-0021 amendment 4): a box at a nearer depth crosses the still
fine field with motion vectors on the box only, so there is a depth edge and a disoccluded
trail. The trail the box left over 40 frames before the last must converge to at most 0.9× the
fine field's single-frame error; and over a coarse checkerboard of the fine field, stepping
from Performance to Quality halfway without a reset, the background's error must stay within
5 % of the frame before for the two frames after the step. Each
checkpoint bounds the mean and largest error over the interior and the largest over the whole
image (a read outside the render rectangle brings in a sentinel of 1000), and the sharpened
output must have more local contrast than the unsharpened one.

It runs as a renderer would: two frames in flight, the output alternating between two images.
The first of its two runs fills every internal image with garbage (NaN, all ones) before the
first frame (`Fsr3::poison`), the second starts from fresh allocations, and every output frame
of the two must be bit-identical. Any validation or synchronization-validation message fails
it. It runs in CI (render-golden) on lavapipe. Each of these faults, injected one at a time,
fails it: the motion-vector scale wrong or zero, the jitter flipped on x or y alone, RCAS not
run, a wrong render size in the constants, and (since the occluder runs) a stale
previous-frame render size (the step's background error drops 11 %: FSR reads the previous
luma and accumulation at the wrong scale) and skipping the every-frame clear of the
reconstructed previous depth (the old trail stays at 0.97× the single-frame error: everywhere
the box ever was stays disoccluded). Do not loosen the moving fine field's 0.95× bound:
RADV's clean 0.88 sits about 7 % under it, and misscaled motion vectors give 1.003. The smoke
also reports `vram_bytes` at real sizes. Results on lavapipe, an RTX 5070 Ti and a Radeon
610M: [docs/perf/0030](../perf/0030-fsr3-port.md) and, with the occluder runs,
[docs/perf/0033](../perf/0033-fsr3-in-frame.md).

In the frame, FSR 3.1 also runs in the readability gates (ADR-0021 gate 3: the ghost probe
and overlay invariance) and in `scripts/tiered-goldens.sh` (two lavapipe runs byte-identical,
committed fingerprints written from CI's lavapipe, and a mid-stream reset every fifth frame
under synchronization validation). Run the ghost probe on a real GPU as well as lavapipe
(ADR-0021 amendment 4, item 14): its particle bound is 7.5 % at Quality and native and 15 %
from Performance down.

## Updating the SDK

1. Fetch the new SDK (`vendor-sdks.toml`'s `fidelityfx` entry), re-read its licence, and check
   every vendored file is still under its MIT list.
2. Replace `third_party/fidelityfx/` with the new files, unmodified, and regenerate
   `MANIFEST.sha256` (`find Kits -type f | sort | xargs sha256sum`).
3. Diff the upstream `ffx_fsr3upscaler.cpp` dispatch, resource list and constants against the
   Rust host code, and the passes' bindings against `moba_shaders::fsr3::PASSES` (the tests
   name every difference).
4. Run `fsr3-smoke` on lavapipe and a GPU, and record the result in the PR.
