# Bindless descriptors on the tiered path

[ADR-0020](../adr/0020-next-gen-foundations.md) §3 (and amendment 5) is the design; this page
is the working summary. Evidence: [perf/0028](../perf/0028-bindless.md).

## What exists (N0.5)

- **`BindlessTable`** (`crates/render/src/bindless.rs`) is created by the renderer on the
  **tiered path only** and held for its lifetime. Compat keeps plain descriptor sets
  (ADR-0009) and never creates it. In N0 no production pass binds it yet; the smoke tests do.
- **One set, bound at index 0** of every tiered pipeline layout, `UPDATE_AFTER_BIND_POOL`,
  every binding `PARTIALLY_BOUND | UPDATE_AFTER_BIND | UPDATE_UNUSED_WHILE_PENDING`:

| Binding | Type | Slots | Fallbacks |
|---|---|---|---|
| 0 | `SAMPLER[]` | 32 | the 12-entry palette (`SamplerKey`); slots 12–31 hold slot 0 (linear/wrap) |
| 1 | `STORAGE_IMAGE[]` | 1,024 | slot 0: a 1×1 image cleared to zero, in `GENERAL` |
| 2 | `SAMPLED_IMAGE[]` | 16,384, variable count | 0 magenta (`MISSING`), 1 white, 2 flat normal |

- **Buffers are device addresses** in push constants (`GL_EXT_buffer_reference`), not
  descriptors. Tiered memory is allocated with `DEVICE_ADDRESS`; create the buffer with
  `SHADER_DEVICE_ADDRESS` and call `GpuBuffer::device_address`.
- **Acceleration structures** go in a small set at index 1 (the RT smoke does this).

## Rules

- **Write at load time only.** `register_texture(view)` / `register_storage_image(view)` hand
  out typed `u32` handles and write one descriptor each. Nothing is written per frame;
  `BindlessTable::writes()` counts every descriptor written, so a test can prove it. Writes
  may land while frames that bind the set are pending: they touch only free slots, which no
  pending frame indexes (`UPDATE_AFTER_BIND | UPDATE_UNUSED_WHILE_PENDING`).
- **Release with the last frame that can use the handle; it is `unsafe`.**
  `release_texture(handle, frame)`: `frame` is the last frame, **submitted or being
  recorded**, that may index the handle, and no frame recorded after the release may index
  it. Call `retire(completed_frame)` only once that frame's fence has signalled; the slot then
  points at the fallback again and goes back on the free list (lowest slot first). Both are
  `unsafe fn`s: a wrong `frame` or `completed` lets the rewrite or the slot's reuse write a
  descriptor a pending frame reads (VUID-vkUpdateDescriptorSets-None-03047), which is
  undefined behaviour. Keep the image alive until the retirement. `retire` with nothing
  pending writes and allocates nothing.
- **Never record a released handle.** It does not read the fallback: once its frame retires
  the slot can be reused, and rewritten while the late frame is pending. Shaders index only
  live handles and the reserved fallbacks (`MISSING`, `WHITE`, `FLAT_NORMAL`, storage slot 0).
- **The free lists are sized up front.** `allocate`, `release` and `retire` never allocate and
  cost O(log n) per slot, so a pass may call `retire` every frame. `retire` still writes one
  descriptor per recycled slot; when a pass first calls it per frame (N1, bulk eviction),
  merge adjacent slots into one write per run and add a criterion bench.
- **Every slot holds a written descriptor, for 2D float reads only.** Creation writes the
  fallback into all 17,440 slots, so a zero or bad index reads magenta (or the zero storage
  image, or linear/wrap) instead of an unwritten descriptor. Every fallback is a 2D
  `R8G8B8A8_UNORM` image: through a `texture3D`/`textureCube` alias, a `uimage2D` or a
  comparison sampler, a fallback is an invalid descriptor. Typed fallbacks come with N1.
- **Compare only depth textures.** `BINDLESS_SAMPLER2DSHADOW_NONUNIFORM(texture, sampler)`
  takes a comparison palette slot (6–11) and a depth texture.
- **Index with `nonuniformEXT`** wherever a handle can diverge. In GLSL include
  `bindless.glsl` and sample with `BINDLESS_SAMPLER2D_NONUNIFORM(texture, sampler)`: the
  combined `sampler2D(...)` needs the qualifier too, so that the `OpSampledImage` the sample
  reads is decorated `NonUniform` (VUID-RuntimeSpirv-NonUniform-06274). Decorating only the
  indices compiles and passes validation, but a driver may then sample one descriptor for the
  whole wave.
- **Check layouts with `descriptor_budget::check_tiered`.** It counts against the device's
  update-after-bind limits, in every stage a binding is visible to (`ALL` covers mesh and
  ray tracing too); the 16-sampler Compat ceiling does not apply.
- **The capacities are constants** (`caps::BINDLESS_*`), raised in one place; admission
  refuses a device whose limits are smaller (it runs Compat, never a smaller table).

## Smoke tests

`bindless-smoke` and `rt-smoke` (`moba-tools`) each render one fixed 64×64 frame on a
headless tiered device, read it back and compare every value exactly with a CPU expectation.
They exit 0 on a pass, 1 on a mismatch, any validation error or warning, or a run without
validation that was not asked for (only `MOBA_VULKAN_VALIDATION=off` passes without the
layer), and 2 when they cannot run (no tiered device, no validation layer under `required`).

```bash
# lavapipe (a CPU device needs MOBA_RENDER_PATH=tiered; see software-vulkan.md for the env)
MOBA_RENDER_PATH=tiered MOBA_VULKAN_VALIDATION=required cargo run --release -p moba-tools --bin bindless-smoke
MOBA_RENDER_PATH=tiered MOBA_VULKAN_VALIDATION=required cargo run --release -p moba-tools --bin rt-smoke
# a GPU admitted to the tiered path picks it under `auto`
MOBA_VULKAN_VALIDATION=required cargo run --release -p moba-tools --bin rt-smoke
```

- **`bindless_smoke`** registers 212 2×2 textures and a 2×2 depth texture, releases 12
  textures in frame 1, retires them, reuses 10 of the slots and keeps 2 recycled slots free. A
  256-entry handle list in a device-address buffer then feeds a fragment pass and a compute
  pass (the latter writes through a bindless storage image without a format). Neighbouring
  pixels pick unrelated handles and samplers, and every pixel samples at a coordinate where
  wrap/clamp and point/linear give four different exact values, so the palette is checked
  too; the compute pass also compares the depth texture through each comparison sampler
  (not the fragment pass: lavapipe crashes on a second sample after a `nonuniformEXT` one in
  a fragment shader). Zero handles and the recycled slots must read magenta. While the
  frame is pending the smoke writes four descriptors into the bound set, to slots the frame
  does not index (update after bind; validation fails the run without the flags).
- **`rt_smoke`** builds a BLAS of three triangles and a TLAS of three instances (one
  mirrored), traces one ray per pixel with `GL_EXT_ray_query`, and stores the closest hit's
  instance, custom index, primitive and `round(4t)`. Every ray stays at least 0.011 from every
  edge line (a unit test), so no result depends on watertightness rules.

The `renderer-gate` job runs both on lavapipe with validation and synchronization validation.
