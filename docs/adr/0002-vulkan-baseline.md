# ADR-0002: Vulkan 1.2 baseline with targeted extensions

**Status:** accepted (2026-10-14)

## Decision
Target Vulkan 1.2 core plus `VK_KHR_dynamic_rendering`, `VK_KHR_synchronization2` and descriptor indexing (bindless), through `ash` and `gpu-allocator`.
On macOS we run through MoltenVK: we enable `VK_KHR_portability_enumeration` on the instance, honour `VK_KHR_portability_subset` limits, and gate every optional feature behind a capability check with a fallback.
Shaders are GLSL, compiled to SPIR-V at build time and checked with `spirv-val` in CI.

## Rejected
- wgpu: it would cost us explicit control over sync, memory and bindless, and the project explicitly wants Vulkan.
- Vulkan 1.3 as a hard minimum: it would exclude some MoltenVK and older-driver configurations for little gain, since the extensions above give the same features.

## Consequences
- More unsafe code in `render`, which `gpu-safety-reviewer` and validation-layer CI gates offset.
