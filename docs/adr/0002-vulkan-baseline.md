# ADR-0002: Vulkan 1.3 baseline

**Status:** accepted 2026-10-14 (Vulkan 1.2 baseline); **amended 2026-10-20** (raised to 1.3).

## Decision
- Require **Vulkan 1.3** on the instance and device. The device must expose the core `dynamicRendering` and `synchronization2` features.
- Bindings: `ash` and `gpu-allocator`.
- macOS: runs through MoltenVK or KosmicKrisp. The instance enables `VK_KHR_portability_enumeration` when advertised; the device enables `VK_KHR_portability_subset` when present, and the renderer honours its limits.
- Optional features stay behind capability checks with fallbacks.
- Shaders: GLSL compiled to SPIR-V for `--target-env=vulkan1.3`, and checked by `spirv-val --target-env vulkan1.3`.
- Vulkan 1.4 is **not** required. It is revisited when the conditions under Revisit are met.

## Why 1.3 (amendment, 2026-10-20)
The original decision kept 1.2 as the minimum because some MoltenVK and older-driver configurations lacked 1.3. That is no longer true for the platforms we target:

| Platform | Vulkan 1.3 | Vulkan 1.4 |
|---|---|---|
| Linux (Mesa 25+: RADV, ANV, NVK, lavapipe; NVIDIA proprietary) | yes | yes |
| Windows: NVIDIA, AMD RDNA, Intel Arc/Xe | yes | yes |
| Windows: Intel Gen9–Gen11 iGPUs (UHD 620/630, a typical MOBA min-spec laptop) | yes | **no** (driver tops out at 1.3) |
| macOS: MoltenVK 1.4.x | yes | reported; `dynamic_rendering_local_read` partial |
| macOS: KosmicKrisp (Vulkan SDK, Apple Silicon + macOS 26) | yes | conformant |

The only hardware a 1.3 minimum excludes is roughly decade-old GPUs that are limited to 1.2 (for example NVIDIA Kepler on its legacy driver). In exchange we delete the second code path: the KHR extension loaders, a parallel feature chain, and the `expect`s that guarded them. Rendering and barriers now go straight through core entry points.

## Why not 1.4 yet
- **Bindings.** The latest published `ash` (0.38) ships Vulkan **1.3.281** headers. Vulkan 1.4 core types are only on ash `master` (the "0.39 with Vulkan 1.4" milestone). Requiring 1.4 would mean depending on a git revision or a fork.
- **Min-spec hardware.** Intel Gen9–Gen11 iGPUs on Windows do not expose 1.4.
- **Need.** Nothing the renderer uses today requires 1.4. The useful 1.4 features (push descriptors, maintenance5/6, dynamic-rendering local read, host image copy) can be adopted as optional extensions on 1.3 when a feature needs them.

## Revisit when
- `ash` 0.39 (or later) with Vulkan 1.4 headers is published. Then 1.4-only features become available behind capability checks.
- The min-spec target moves past Intel Gen11 iGPUs. Then a 1.4 minimum can be considered.

## Rejected
- wgpu: it would cost us explicit control over sync, memory and bindless, and the project explicitly wants Vulkan.
- Keeping the 1.2 minimum: the extra path costs code and test surface but serves almost no hardware we target.

## Consequences
- More unsafe code in `render`, which `gpu-safety-reviewer` and the validation-layer CI gates offset.
- Drivers that only report Vulkan 1.2 now fail with `UnsupportedApiVersion` or `NoSuitableDevice`, and the error names the 1.3 requirement.
