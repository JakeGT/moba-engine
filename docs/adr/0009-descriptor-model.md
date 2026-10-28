# ADR-0009: Descriptor model

**Status:** accepted 2026-10-28 (AAA graphics roadmap R1a). It **confirms** ADR-0002, which
keeps optional features behind capability checks; it does not amend it.

## Context
The [AAA graphics roadmap](../plans/aaa-graphics-roadmap.md) (R1.3) asks for a decision
between two models:

- **Bindless.** Every texture and buffer goes in one large descriptor array, indexed from
  shaders. This needs descriptor indexing (`descriptorIndexing` features, core-optional since
  Vulkan 1.2), plus `runtimeDescriptorArray`, partially-bound and update-after-bind.
- **Plain descriptor sets.** Small sets bound per pass and per material.

The roadmap's recommendation was "bindless if MoltenVK and UHD 620 measurements are
acceptable". Its §10 asked for a spike on those two targets before deciding.

Today the renderer binds very little. The scene pass has a per-frame camera UBO (set 0) and the
instance and bone SSBOs (set 1). The UI pass has the atlas sampler. The composite pass has the
HDR target. There are no material textures yet: R1b adds the material system and R2 adds PBR
texture sets.

## Decision
- **Plain descriptor sets, with one set per material, for now.** Pipeline layouts stay
  shared where passes share data, as the scene pass already does across the terrain, unit and
  skinned pipelines. When R1b's material system arrives, each material gets its own descriptor
  set: texture set plus parameter block. That set is allocated at load time from a pool sized
  by the content, and bound per draw batch. No descriptor set is allocated or written per
  frame, except where a set names a per-frame buffer and is pre-allocated per frame in flight,
  as today.
- **Bindless is deferred, not rejected.** It is revisited with measurements, behind a
  capability check. It is not a requirement.

## Why
- **The target hardware is not available to measure.** The decision hinges on MoltenVK
  (argument buffers, Apple M1) and an Intel UHD 620-class GPU. Neither is in this environment,
  and hosted CI has no real GPU on Windows or macOS (#9/#127). The maintainer will collect that
  hardware evidence just before 1.0. Under this project's evidence rules (roadmap §3,
  `docs/agents.md`), estimates never close a target, so bindless cannot be chosen on faith.
- **Plain sets work everywhere we ship.** They are core Vulkan 1.0 with no feature bits, so
  MoltenVK, Intel Gen9–Gen11, lavapipe and every desktop driver behave alike. The
  validation-required lavapipe goldens keep exercising the same path that ships.
- **Nothing needs bindless yet.** With a handful of sets per frame and no material textures,
  binding cost is not measurable. R1a's measured frame spends 3–27 µs per pass on the GPU
  ([perf/0011](../perf/0011-render-graph.md)), and the whole render call (upload, recording,
  submit, present) takes about 0.2 ms of CPU.
- **Migration is contained.** Materials will be referenced by id and own their descriptors
  (R1b). Switching the material backend later to "index into a bindless array" changes the
  material module and its shaders' resource declarations, not callers, content or the
  render graph.

## Revisit when
Any of these happens:

- **Measurements exist.** Bindless and per-material sets have been prototyped on MoltenVK
  (Apple M1 class) *and* a UHD 620-class GPU, and the evidence is in `docs/perf/`. Choose
  bindless if it is supported on both, with no extra per-frame CPU or GPU cost at the R5
  teamfight scene.
- **Binding cost shows up.** Descriptor binding or set updates appear in a CPU profile at
  more than about 2 % of the client's frame budget, for example from a material count that
  forces many set binds per frame.
- **A feature needs arbitrary resource access.** GPU-driven culling with indirect draws
  (R8) or a visibility buffer would need shaders to reach any texture from a draw index.

## Rejected (for now)
- **Bindless as the only path.** It would drop or emulate devices that lack the
  descriptor-indexing features, or where MoltenVK maps them to argument buffers with
  restrictions, before we can measure what we lose.
- **Both paths at once.** That doubles shader permutations and test surface before either is
  needed.
- **Push descriptors (`VK_KHR_push_descriptor`, core in 1.4).** They are attractive for
  per-draw material data, but ADR-0002 keeps 1.4 out of the baseline. They can be an optional
  path later, behind a capability check.

## Consequences
- R1b's material system is designed around per-material sets allocated at load time. The pool
  is sized from the content and reported by `content-check`.
- The descriptor set layouts stay small and per pass. Pipeline layouts do not reserve a
  bindless set.
- When the maintainer runs the pre-1.0 hardware matrix, the bindless spike should be part of
  it, so this ADR can be revisited with data.
