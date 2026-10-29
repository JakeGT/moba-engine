# ADR-0008: Render graph

**Status:** accepted 2026-10-28 (AAA graphics roadmap R1a).

## Context
Until now the whole frame was one function (`Renderer::render_internal`, ~670 lines): one
forward pass straight into the swapchain image with a per-image depth buffer, the HUD drawn in
the same pass, and hand-written `synchronization2` barriers around it. The
[AAA graphics roadmap](../plans/aaa-graphics-roadmap.md) (R1–R6) adds shadow maps, HDR,
post-processing, decals and GPU particles. Each of them is another pass that reads what an
earlier pass wrote. Hand-placing every barrier and layout transition in one function does not
scale, and a wrong barrier is a silent GPU race that only sync validation catches.

## Decision
- **An in-house, small render graph** in `moba-render` (`crates/render/src/graph.rs`). No
  external crate.
- **Explicit, ordered pass list.** The frame is declared in code
  (`crates/render/src/frame_graph.rs`), in execution order. There is no automatic reordering,
  no async compute and no multi-queue scheduling. Passes run in the order they are declared.
- **Passes declare accesses, not barriers.** Each pass lists the images and buffers it uses
  and how: `ColorWrite`, `ColorReadWrite` (load + blend), `DepthWrite`, `SampledFragment`,
  `TransferRead`, `TransferWrite`, `VertexStorageRead`. Imported resources (the swapchain
  image, per-frame host-written buffers, the capture buffer) also declare their state at graph
  start and an optional final access (`Present`, `HostRead`).
- **`compile` derives the plan.** Compilation is pure logic over `ash::vk` flag types. It never
  touches a device, so it is unit-tested headless (`graph/tests.rs`, `frame_graph.rs`). It
  rejects a read before any write of a transient **or** of an import whose start layout is
  `UNDEFINED` (the swapchain image), including through a final access (since R1b). It:
  1. **Culls** passes whose outputs no later live pass, final access or side-effect pass
     consumes. It walks the list backwards, so a write that is overwritten before it is read
     is dead.
  2. **Pools transients.** Graph-owned images live for one frame. Transients with identical
     descriptions (format, aspect, size, extra usage) whose live ranges do not overlap share
     one physical image. The image's usage is the union of what its aliases need. Culled
     transients get no memory.
  3. **Derives `vkCmdPipelineBarrier2` barriers.** For each resource it tracks the layout, the
     last write and the stages that have read since. Read-after-write makes the write
     available and visible with exact stage/access masks. Write-after-read is an execution
     dependency (no access mask). Write-after-write makes the first write available. A layout
     change is always a barrier. The barriers before each pass are grouped into one
     `vkCmdPipelineBarrier2`. The final transitions (swapchain → `PRESENT_SRC_KHR`, capture
     buffer → host read) are recorded after the last pass.
  4. **Orders against the previous frame.** A transient's first barrier in a frame starts from
     `UNDEFINED` (contents discarded). Its source scope is every stage and write that touches
     that slot during the frame, because the same plan ran in the previous submission on the
     same queue. One HDR target and one depth buffer are therefore shared by both frames in
     flight, with no per-frame copies.
- **Compile once, record many.** The renderer compiles its graph variants at creation: the
  normal frame and the capture frame (which adds a copy pass). Because the variants alternate
  on the same physical transients, creation checks that they agree on every slot's
  description, usage **and** cross-frame start `(stage, access)` (`PhysicalImage` equality,
  since R1b), not just the slot list. Physical images are created at
  creation and on resize. Per frame, the renderer fills a fixed-size binding table
  (`Bindings`, 16 entries) with this frame's handles. It then records the precompiled barriers
  from stack arrays (at most 16 barriers per pass). **Nothing on the frame path allocates.**
- **Limits:** 8 passes, 16 resources and 16 barriers per pass. `compile` returns
  `GraphError::TooLarge` past them, rather than growing at run time.
- **Per-pass GPU timing.** Each pass writes two `vkCmdWriteTimestamp2(ALL_COMMANDS)` queries:
  one after its barriers and one after its commands. They go into a query pool per frame in
  flight, created at renderer creation. A pass's time excludes waits on earlier commands.
  **Since R1b** the frame is two batches in one `vkQueueSubmit`: the passes before the first
  swapchain access (the scene) go in a batch with no semaphore wait, and the rest wait on the
  acquire semaphore at `COLOR_ATTACHMENT_OUTPUT`, so the scene's time can no longer include
  that wait (the composite's still can when the presentation engine holds the image; measure
  with vsync off). Results are read after that frame's fence with no wait, and carry the
  measured frame's sequence number (`GpuTimings::frame`) so a caller polling every frame logs
  each sample once. If the graphics queue reports `timestampValidBits == 0` or
  `timestampPeriod` is zero, no pools are created and `Renderer::gpu_timings()` returns
  `None`.

### The R1a frame

```text
scene      terrain, units, skinned meshes → hdr (ColorWrite, cleared) + depth (DepthWrite,
           sampled-capable); bone buffer (VertexStorageRead, imported host-written)
composite  hdr (SampledFragment) → exposure + AgX → swapchain (ColorWrite)
ui         HUD batch → swapchain (ColorReadWrite): after the tonemap, so text is never tonemapped
capture    swapchain (TransferRead) → readback buffer (TransferWrite), capture variant only
final      swapchain → PRESENT_SRC_KHR; readback buffer → HostRead
```

- **HDR target:** `R16G16B16A16_SFLOAT` on Medium/High/Ultra and `B10G11R11_UFLOAT_PACK32` on
  Low (`QualityTier`, `RenderConfig::with_quality_tier`). Low falls back to `R16G16B16A16_SFLOAT`
  when the device cannot render to, blend into and sample the packed format.
- **Depth:** `D32_SFLOAT` (or `D16_UNORM`), with `SAMPLED` usage so later passes can read it.
  Its store op stays `DONT_CARE` until a pass reads it.
- **Swapchain:** imported with discarded contents. Its first barrier's source stage is
  `COLOR_ATTACHMENT_OUTPUT`, the stage the acquire semaphore is waited on, so the semaphore
  wait and the layout transition chain correctly. The swapchain no longer tracks layouts or
  owns depth images.

### Tonemap: AgX
The composite applies exposure (`RenderConfig::exposure`, default 1.0) and then **AgX**. It uses
Troy Sobotka's AgX with the polynomial sigmoid fit published by Benjamin Wrensch ("Minimal AgX
implementation", 2023): inset matrix, log2 encoding over [−12.47, 4.03] EV, a 6th-order
polynomial, then the outset matrix. Then it linearises with a 2.2 power, because the swapchain
is `*_SRGB` and encodes on store.

- **Why AgX over ACES-fitted.** At exposure 1, AgX keeps shadows and mid-tones almost as they
  are: 0.03 → 0.032, 0.18 → 0.21. Most of a MOBA frame is dark terrain and fog. ACES-fitted
  (Narkowicz) brightens mid-grey by about 50 % (0.18 → 0.27) and skews hue per channel in bright
  saturated colours. Those are exactly the team colours and, from R5, the ability VFX.
  Readability comes first in this genre (roadmap §2).
- **Saturation look.** The base AgX curve desaturates bright colours. In the first capture red
  team units turned pink. The composite therefore applies the AgX look stage with identity
  slope, offset and power and the "punchy" preset's saturation (1.4). With it, sample lit team colours move by at most about 20 sRGB steps
  per channel from the pre-tonemap renderer, against about 35 with the base look, and keep
  their hue (`TONEMAP_SATURATION` in `passes/composite.rs`).
- **Cost.** One fullscreen triangle (no vertex buffer), one `texelFetch` per pixel and a few
  dozen ALU operations. Per-map grading, LUTs, bloom and anti-aliasing are R6.
- **Golden images.** This is a deliberate rebaseline: highlights now roll off (a lit yellow
  structure is dimmer), and the background clear colour goes through the same curve. UI pixels
  are unchanged.

## Alternatives considered
- **Keep hand-written barriers.** This is fine for one pass and fragile for ten. Every new
  pass would need a full re-audit of the barrier chain, and the barriers could not be tested
  without a device.
- **An external crate** (for example a general Vulkan render-graph crate, or adopting `wgpu`'s
  automatic tracking). Rejected for the same reasons as ADR-0002. We want explicit control over
  sync and memory, the dependency surface stays small (`deny.toml`), and there is no crate on
  `ash` 0.38 / Vulkan 1.3 dynamic rendering that is small, maintained and lets us keep the
  no-per-frame-allocation invariant.
- **A fully dynamic graph rebuilt every frame**, with automatic pass reordering, as in large
  engines. It allocates or needs arena bookkeeping every frame, makes the barrier plan depend
  on run-time data, and gives nothing at our pass count (3–10). If a later phase needs
  per-frame variation, it gets another precompiled variant, like the capture frame.
- **Memory aliasing across different formats** (placing transients of unrelated descriptions
  in one `VkDeviceMemory` region). This is deferred. Today's frame has two overlapping
  transients, so there is nothing to alias. Pooling identical descriptions covers the
  ping-pong chains of R6 (bloom, post). Revisit when transient memory shows up in the VRAM
  budgets (R8).
- **Render passes / subpasses** (`VkRenderPass`) for tile-local tonemapping. ADR-0002 already
  chose dynamic rendering. `dynamic_rendering_local_read` (Vulkan 1.4, partial on MoltenVK) can
  later be an optional path behind a capability check.
- **Merging the UI into the composite rendering scope.** This would save one
  begin/end-rendering pair and a load of the swapchain image. We keep it separate so it has its
  own GPU timing and so UI-only passes (R7) slot in without touching the tonemap. Measured cost
  of the separate pass: 3–5 µs ([perf/0011](../perf/0011-render-graph.md)).

## Consequences
- Adding a pass means declaring its accesses. Its barriers are derived and covered by the
  headless tests. Validation plus sync validation stays the run-time backstop (lavapipe golden
  CI and `MOBA_VULKAN_VALIDATION=required`).
- `renderer.rs` shrinks from 1,872 to about 900 lines. Pass code lives in
  `crates/render/src/passes/` (`scene`, `composite`, `overlay`, `capture`).
- The frame now has one extra fullscreen pass. On the RTX 5070 Ti at 1920×1080 it costs about
  14 µs. That is +16–21 µs of GPU time per frame over the old single pass, against a 6.9 ms
  budget. The relative change on this tiny scene is large: see
  [perf/0011](../perf/0011-render-graph.md) for the numbers and the discussion of the roadmap's
  ≤ 5 % criterion.
- Both golden fingerprints were rebaselined once, deliberately, for the tonemap.
- `RenderConfig` gains `quality_tier` and `exposure`. `Renderer` gains `gpu_timings`,
  `gpu_timing_supported`, `quality_tier` and `hdr_format_name`. `GpuTimings`, `PassTiming` and
  `QualityTier` are new public types. `RenderError` gains `RenderGraph`. All changes are
  additive: the types are `#[non_exhaustive]`.
