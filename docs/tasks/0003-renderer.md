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

Core rendering, fog/LOS, textured HUD, map themes, animated hero rig, golden-image CI and
readback are landed. The 2026-10 audit fixes (portable instance enumeration, acquire ordering,
capture safety, validation counting) landed in #32; real Windows/macOS GPU timing remains open
(#9/#127). Linux/NVIDIA timing is recorded below; the task remains partially open only for
those platform results.

## Non-goals / still open
- GPU joint-weight/skeletal meshes and real asset imports (current hero animation uses a
  deterministic multi-part instance rig; minions/structures remain instanced cubes).
- 6.9 ms frame-budget validation on real Windows/macOS GPUs (Linux/NVIDIA is measured;
  hosted macOS misses the target and Windows hosted runners have no Vulkan device).
- Bindless texture array (one UI atlas is sampled today; descriptor-indexing features remain
  enabled for a future asset pipeline).

## Acceptance tests
- [x] `moba-shaders::modules_are_well_formed_spirv` and build-time `spirv-val`.
- [x] `Renderer` runs 500 units + terrain at 1920×1080 under Mesa lavapipe with
      `MOBA_VULKAN_VALIDATION=required` and emits **zero** validation messages.
- [x] GPU readback contains the expected scene (2464 distinct colours; terrain, clear
      colour and all three team colours present). See `docs/images/m3-500-units.png`.
- [x] Golden-image diff in CI (`render-golden.yml`, 64×36 fingerprint, validation + sync).
- [x] Animated hero path, textured UI atlas/icons, map themes and real-GPU frame capture.

## Budgets touched
Renderer submission targets ≤ 6.9 ms p95. Linux/NVIDIA RTX 5070 Ti at 1920×1080 measured
p95 **4.197 ms**, max **4.538 ms** with vsync off and zero validation messages. Per frame there
are terrain/unit/UI draw batches, pre-sized descriptor sets and host-visible buffer writes,
with no per-frame heap allocation in `Renderer::render`. Windows/macOS real-GPU runs remain
open (#127).

## Review log
| Reviewer | Finding | Verdict | Resolution |
|---|---|---|---|

## macOS runtime probe

`vulkan-probe` + the `macos vulkan probe` workflow exercise instance creation, portability
extension selection and physical-device enumeration under MoltenVK. See
[software Vulkan](../dev/software-vulkan.md).

## Real-hardware frame timings

- [x] RTX 5070 Ti / NVIDIA 615.71.09 / 1920×1080: p50 4.181 ms, p95 4.197 ms,
  p99 4.212 ms, max 4.538 ms with vsync off; zero validation messages.
- [x] `MOBA_FRAME_LOG`/`MOBA_VSYNC` benchmark path documented in
  `docs/perf/0007-real-hardware.md`; Windows/macOS commands recorded there (no real GPU in CI).
- [x] Capture artifact `docs/images/m8-nvidia-teamfight.png`.

## Animated hero rig

- [x] Procedural multi-part hero rig (torso/head/arms/legs) animated from ticks; no per-frame
  heap allocation, existing instance pipeline.
- [x] Animated hero acceptance: deterministic multi-part rig, validation-required golden capture,
  no per-frame allocation. GPU joint-weight meshes remain a future asset-pipeline optimization.
