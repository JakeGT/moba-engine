# 0003: Vulkan renderer (M3)

**Milestone:** M3   **Crates touched:** shaders, render, client   **Reviewers:** gpu-safety + perf + /code-review

## Goal
A Vulkan 1.2+ renderer that draws the MOBA scene (terrain + hundreds of instanced units)
with zero validation messages, and a reproducible way to capture its output on machines
without a GPU.

## Scope (landed)
- GLSL shaders compiled to SPIR-V at build time with `glslc`/`glslangValidator`; every
  module validated by `spirv-val` when installed.
- Instance creation with optional Khronos validation layer (`Off`/`IfAvailable`/`Required`)
  and a debug messenger; surface creation via `ash-window` + raw window handles.
- Physical-device scoring, queue-family selection, Vulkan 1.2/1.3 feature negotiation,
  descriptor indexing + dynamic rendering + synchronization2 (core on 1.3, extension
  loaders on 1.2), `gpu_allocator` suballocation.
- Swapchain with format/present-mode/depth-format selection, per-image render-finished
  semaphores and resizing.
- Dynamic-rendering frame loop with explicit sync2 barriers, depth testing, instanced units
  (storage buffer) and procedural terrain grid.
- GPU readback capture (`Renderer::capture_frame`) and a `MOBA_CAPTURE` client mode that
  writes a PPM.

## Status

Core rendering, fog/LOS, HUD and readback are landed. The 2026-10 audit fixes (portable
instance enumeration, acquire ordering, capture safety, validation counting) landed in #32;
golden-image CI, skinning and hardware timing remain open. The open items below are tracked as
GitHub issues; this task is not exit-complete until they close.

## Non-goals / still open
- Skeletal skinning and animation (currently instanced cubes; M6 art pass).
- Golden-image comparison tolerances and CI wiring for lavapipe.
- 6.9 ms frame budget measurement on min-spec hardware (software rasteriser numbers are not
  representative).
- Bindless texture array (descriptor-indexing features are enabled; textures arrive with
  the asset pipeline).

## Acceptance tests
- [x] `moba-shaders::modules_are_well_formed_spirv` and build-time `spirv-val`.
- [x] `Renderer` runs 500 units + terrain at 1920×1080 under Mesa lavapipe with
      `MOBA_VULKAN_VALIDATION=required` and emits **zero** validation messages.
- [x] GPU readback contains the expected scene (2464 distinct colours; terrain, clear
      colour and all three team colours present). See `docs/images/m3-500-units.png`.
- [ ] Golden-image diff in CI.
- [ ] Skinned units and frame-time capture.

## Budgets touched
Renderer submission is designed for ≤ 6.9 ms on min-spec GPUs; not yet measured on real
hardware. Per frame there are two draw calls, two descriptor sets and two host-visible
buffer writes (camera + instances), with no per-frame allocation in `Renderer::render`.

## Review log
| Reviewer | Finding | Verdict | Resolution |
|---|---|---|---|
