# Capability fixtures (ADR-0020 §2b)

The admission unit tests in `../tests.rs` read these files. Each one is a `DeviceCapabilities`
value in JSON, in the format `vulkan-probe --json` prints.

## Recorded (`"source": "recorded"`)

These are dumps from real devices, taken on 2026-11-01 with `vulkan-probe --json` and the
Khronos validation layer loaded. The layer printed no message.

| File | Device | Driver | Vulkan |
|---|---|---|---|
| `rtx-5070-ti.json` | NVIDIA GeForce RTX 5070 Ti | NVIDIA 615.78.08 | 1.4.351 |
| `radeon-610m.json` | Radeon 610M, the Ryzen 7 9800X3D iGPU | RADV, Mesa 26.2.5 | 1.4.354 |
| `lavapipe.json` | llvmpipe (LLVM 23.1.3) | the bundled lavapipe in `.tools` | 1.4.354 |

To record a device, run `vulkan-probe --json`. Keep that device's object from the array and
commit it here with `"source": "recorded"`. When a reference GPU arrives (#9/#127), its dump
replaces the synthetic stand-in for its class.

## Synthetic (`"source": "synthetic"`)

These stand for devices we do not own, and they never close a target. Each one starts as a
copy of `rtx-5070-ti.json`. It then gets its own identity (name, vendor, device and driver
IDs, API version) and loses what public facts say its class lacks:

| File | Stands for | Removed from the RTX dump |
|---|---|---|
| `moltenvk-m1.json` | Apple M1 through MoltenVK (driver ID 14), 1.3 | RT and NVIDIA-only extensions, mesh shaders, fragment shading rate, the 1.4-promoted features. Adds `VK_KHR_portability_subset`. Sets 16 per-stage samplers |
| `kosmickrisp-m1.json` | Apple M1 through Mesa KosmicKrisp (driver ID 28), 1.3 | As MoltenVK, without the portability subset, because KosmicKrisp is conformant |
| `nvk-rtx-2060.json` | RTX 2060 on NVK (driver ID 24) | RT and NVIDIA-only extensions. Mesh shaders and fragment shading rate stay |
| `gtx-1060.json` | GTX 1060 (Pascal), NVIDIA proprietary | RT, mesh shaders, fragment shading rate, and NVIDIA-only extensions except `VK_NV_low_latency2` |
| `uhd-620.json` | Intel UHD 620 (Gen9.5) on ANV (driver ID 6) | RT and NVIDIA-only extensions, mesh shaders, fragment shading rate |
| `rt-no-mesh.json` | An RT GPU without mesh shaders | `VK_EXT_mesh_shader` and its features only |

"RT extensions" here means:
- acceleration structure;
- deferred host operations;
- ray query;
- the RT pipeline;
- RT maintenance1;
- position fetch;
- invocation reorder;
- opacity micromap.

When the RTX dump is re-recorded, re-derive the synthetic files the same way, then check the
diff.
