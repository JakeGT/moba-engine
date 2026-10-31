# ADR-0020: Next-gen foundations: bindings, capability gate, bindless, Slang, tiers and vendor SDKs (N0)

**Status:** proposed 2026-10-31. This is phase N0 of the
[next-gen graphics roadmap](../plans/next-gen-graphics-roadmap.md) (§5.1, §8 and §12 step 2),
and it is written before the code.
- **Builds on:** [ADR-0002](0002-vulkan-baseline.md), which stays in force: Vulkan 1.3 is
  still the minimum.
- **Amends, for the tiered renderer only:**
  - [ADR-0009](0009-descriptor-model.md): the tiered renderer goes bindless, and plain sets
    stay for Compat;
  - [ADR-0015](0015-post-processing.md) §6: dynamic resolution is driven by a per-tier budget.
- **Keeps the R8 rules** from [ADR-0018](0018-culling-lods-memory.md): VRAM budgets,
  `locked_uploads=0` and `locked_pipelines=0`.

Vendor, version and licence claims were re-checked on 2026-10-31 against the sources listed in
[Sources](#sources), as the roadmap's preamble requires. Where they differ from the roadmap (written
2026-10-30), this ADR says so in [Corrections to the roadmap](#corrections-to-the-roadmap).

## Context

The roadmap replaces today's four-tier forward renderer with five tiers, Low to Overdrive. Every
tier uses hardware ray tracing, and Low starts at RTX 2060 / RX 6600 class (§1, §3). Today's
renderer is kept as **Compat**: it gets one refresh (C1) and is then frozen. Before any of the
N1–N11 rendering work starts, N0 must settle six things: the bindings, the capability gate,
the descriptor model, the shader language, the tier presets and budgets, and how the closed
vendor SDKs plug in.

What the code does today (`main` at 9a487f2):

- **Bindings.**
  - `ash = "0.38"` resolves to `0.38.0+1.3.281` from crates.io (`Cargo.lock`).
  - It is joined by `ash-window = "0.13"` and `gpu-allocator = "0.28"`.
  - `deny.toml` has `unknown-git = "deny"` and no git allow-list.
  - Only `moba-render` (and `moba-platform` through `ash-window`) touches `ash`. The layering
    check keeps it out of the server crates.
- **Device selection** (`crates/render/src/device.rs`).
  - Each device must meet three conditions:
    - API ≥ 1.3;
    - a graphics queue (preferably one that also does compute) and a present queue;
    - `VK_KHR_swapchain`, plus core `dynamicRendering` and `synchronization2`.
  - Optional extras: `VK_KHR_portability_subset` (MoltenVK), `VK_KHR_pipeline_executable_properties`
    (statistics), and the BC, anisotropy and depth-clamp core features.
  - Devices are scored by device type (discrete 1000, integrated 500, virtual 200, CPU 100),
    plus 10 when graphics and present share a queue family. There is no capability table.
  - `maintenance4` is not enabled today.
  - `vulkan-probe` prints the API version, the device count and whether portability is in use.
- **Shaders** (`crates/shaders/build.rs`).
  - 37 GLSL stage sources plus enumerated permutations are compiled at build time by the system
    `glslc` or `glslangValidator`, with `--target-env=vulkan1.3`.
  - Each output is checked with `spirv-val` when one is installed.
  - CI installs `glslang-tools` (apt), `glslang` (brew) or the Vulkan SDK (choco, Windows).
- **Descriptors** (ADR-0009).
  - Rendering uses plain descriptor sets, one per material, allocated at load time.
  - The scene fragment stage is at **16 of 16 samplers**, the Vulkan and MoltenVK minimum
    (ADR-0017 §6, ADR-0019). `descriptor_budget.rs` enforces this when the renderer is created.
- **Tiers** (`crates/render/src/quality.rs`).
  - `QualityTier { Low, Medium, High, Ultra }` is `#[non_exhaustive]`, with an `ALL` slice.
  - It is chosen by the settings, and `MOBA_QUALITY` overrides them.
  - Dynamic resolution (`dynres.rs`) is on by default only on Low, with a fixed
    `dynamic_target_ms = 14.0` on every tier (`post.rs`).
  - `perf-gate` enforces the old roadmap's §7 budgets: Low 14, Medium 6, High 6, Ultra 7.5 ms.
  - The VRAM budgets are `moba_assets::TIER_BUDGETS`.

Constraints:

- **Presentation only.** Rendering never touches the simulation, the wire format, replays,
  content hashes or the golden state hash. Nothing in this ADR changes `PROTOCOL_VERSION`,
  replay `VERSION`, `SIM_LOGIC_VERSION` or `tests/golden-state-hash.txt`.
  - The capability gate, tier choice, SDK features and dynamic resolution are all
    client-local.
  - None of them may feed back into gameplay. Fog-of-war visibility stays the server's.
- **Layering and `unsafe`** (AGENTS.md invariants 1 and 5).
  - Vulkan and SDK FFI stay in `moba-render`, which is allowed `unsafe`. Every unsafe block
    has a `// SAFETY:` proof.
  - The server must still build without Vulkan.
- **Builds stay open.** An open-source checkout builds and plays with no proprietary SDK
  (roadmap §7). The MIT/Apache tree never contains SDK source or binaries (RTX SDKs License
  §4e).
- **MSRV 1.88**, and every CI platform stays green (Linux, Windows, macOS).
- **No per-frame allocation and no pipeline built mid-match.**

## Decision

### 1. Bindings: `ash` on a reviewed git revision

**1a. The pin.** `ash` and `ash-window` come from a single reviewed revision of
`ash-rs/ash` master through a workspace `[patch.crates-io]` entry. The dependency lines stay
`ash = "0.38"` and `ash-window = "0.13"`.

```toml
[patch.crates-io]
# ADR-0020 §1: Vulkan 1.4.352 headers; no ash release after 0.38.0 (2024-04-01).
ash        = { git = "https://github.com/ash-rs/ash", rev = "8844f6b4eefb16d08cd73c8f3d340e1ac12deb16" }
ash-window = { git = "https://github.com/ash-rs/ash", rev = "8844f6b4eefb16d08cd73c8f3d340e1ac12deb16" }

[workspace.dependencies]
# ADR-0020 §1: the ash-next branch, which builds against ash master; 0.28.0 does not.
gpu-allocator = { git = "https://github.com/Traverse-Research/gpu-allocator", rev = "6a68a5b4c233361c07dfa8865043a16ade7512db", default-features = false, features = ["vulkan", "std"] }
```

- **Why `[patch]` and not `ash = { git = … }`.**
  - Master still calls itself `0.38.0+1.4.352`, so a patch satisfies every `^0.38` requirement
    in the graph and leaves exactly **one** `ash` crate.
  - A direct git dependency would leave `gpu-allocator` linking crates.io `ash`. That gives
    two `ash` crates and mismatched `vk::Device` types.
- **`gpu-allocator` must move too.**
  - Ash master renamed `push_next()` to the unsafe `extend()` (#909) and renamed the extension
    `::new` constructors to `::load` (#1021).
  - Released `gpu-allocator` 0.28.0 therefore **fails to compile** against the pin:
    `no method named push_next found for MemoryAllocateInfo`.
  - Upstream's `ash-next` branch (rev `6a68a5b4c233`, 2025-12-03, "Prepare for next `ash`
    release") does compile.
  - Both results come from a scratch crate on 2026-10-31, with `cargo check` on stable and on
    **Rust 1.88.0**. The branch is based on 0.28.0, so it lacks the 6 later `main` commits.
    Each bump reviews that gap.
- **`deny.toml`** gains `[sources] allow-git` with exactly these two URLs, and a comment that
  names this ADR and the reason. `unknown-git = "deny"` stays for everything else.

**1b. Why a git revision rather than crates.io.**
- **No newer release exists.**
  - The newest release is still `0.38.0+1.3.281`, published 2024-04-01; no 0.39 exists
    [S1].
  - The "Ash 0.39 with Vulkan 1.4" milestone is open, with 21 open and 68 closed items [S3].
  - Master is at `8844f6b` (2026-10-28), generated from Vulkan headers **1.4.352** [S2].
- **N0's own required set is in 0.38.** It covers:
  - `VK_KHR_acceleration_structure`, ray-pipeline and ray-query types;
  - `VK_EXT_mesh_shader` and `VK_KHR_fragment_shading_rate` (raw entry points);
  - descriptor indexing and buffer device address (core 1.2);
  - `VK_NV_low_latency2`.
- **The phases right after N0 need extensions that 0.38 does not have:**

| Extension | Needed in | In 0.38? | On the pinned master? |
|---|---|---|---|
| `VK_AMD_anti_lag` | N1 (Anti-Lag) | no | wrapper |
| `VK_EXT_ray_tracing_invocation_reorder` (SER) | N4, N9 | no (NV only) | types |
| `VK_KHR_opacity_micromap` | N4 | no (EXT only) | types |
| `VK_EXT_descriptor_heap` | later (§3) | no | wrapper |
| `VK_NV_cluster_acceleration_structure`, `VK_NV_partitioned_acceleration_structure` | N9, optional | no | wrapper |
| `VK_NV_cooperative_vector` | later, optional | no | wrapper |

- **The API break is cheapest to take now.** Master renames constructors, reshapes pNext
  chaining (`push`/`extend` on `TaggedStructure`) and merges the `Extends*` traits.
  - Today `moba-render` has 16 `push_next` call sites and 6 extension-loader `::new` calls
    across 4 loader types (surface, swapchain, debug utils, pipeline executable properties).
  - Taking the break once, mechanically, before the new RT code is written means none of N1–N4
    is written twice.

**1c. How the pin is updated.**
- **A deliberate PR.** A bump is its own PR, titled `deps(render): bump ash to <short-sha>
  (headers 1.4.N)`. It is made at a phase start, or when a needed extension lands.
- **What the PR records:**
  - the header version;
  - the ash changelog delta and any API breaks;
  - the matching `gpu-allocator` revision.
- **What the PR runs:**
  - `scripts/agent-verify.sh --full`;
  - the lavapipe golden and validation jobs;
  - the macOS MoltenVK probe;
  - one RTX 5070 Ti run.
- **Revisions only.** The pin is always a full `rev =`, never a branch or tag. `Cargo.lock`
  records it, and `--locked` keeps CI reproducible.
- **Return to crates.io** (remove the patch) once `ash` 0.39 or later and a `gpu-allocator`
  release that accepts it are published.
- **Watch item.** Open ash PR #1043 would move master to headers 1.4.366. Khronos headers are
  at 1.4.367 (2026-10-30) [S5].

**1d. MSRV.**
- The pinned `ash` declares `rust-version = "1.69.0"`, and `gpu-allocator`'s `ash-next`
  branch declares 1.71.
- The pair builds with Rust 1.88.0, as checked above.
- Open ash PR #1059 would raise ash's MSRV to 1.83, which is still under ours.
- **The workspace MSRV stays 1.88.** A bump that needs a newer compiler is an MSRV change and
  amends this ADR, and the CI `msrv` job (1.88.0) enforces it.

**1e. Extension plumbing.**
- **One table drives everything.** A new `crates/render/src/caps.rs` holds a static table of
  `ExtensionSpec { name, requirement, feature_query }`, where requirement is `Required`,
  `Optional` or `Sdk(feature)`.
- **One probe per physical device.** It reads:
  - the extension list;
  - one `vkGetPhysicalDeviceFeatures2` chain;
  - one `vkGetPhysicalDeviceProperties2` chain, covering the acceleration-structure,
    ray-tracing-pipeline, mesh-shader, fragment-shading-rate, descriptor-indexing and driver
    properties.
- The chain contains only structures whose extension is advertised.
- **The result is plain data,** `DeviceCapabilities`. It has no lifetimes and no pNext
  pointers, and it is printed by `vulkan-probe`.
- **The enabled set** is the required set, plus the optional extensions that are available,
  plus what each compiled-in vendor back end asks for.
- **Vendor back ends declare their extensions before instance and device creation.**
  - NGX reports its required instance and device extensions, for example
    `VK_NVX_binary_import` and `VK_NVX_image_view_handle`.
  - XeSS has a matching query.
  - A back end whose needs are not met is disabled with one log line. The device is never
    rejected because of a back end.

Extensions and features enabled on the **tiered** path:

| Group | Requirement | Contents |
|---|---|---|
| Core 1.3 | required | `dynamicRendering`, `synchronization2` (both enabled today); `maintenance4` (new); `VK_KHR_swapchain` |
| Core 1.2 | required | `bufferDeviceAddress`, `descriptorIndexing` (§3 list), `runtimeDescriptorArray`, `scalarBlockLayout`, `timelineSemaphore`, `drawIndirectCount`, `hostQueryReset` |
| Ray tracing | required | `VK_KHR_acceleration_structure` (+ `VK_KHR_deferred_host_operations`), `VK_KHR_ray_query` |
| Ray tracing | optional; required for Overdrive | `VK_KHR_ray_tracing_pipeline`, `VK_KHR_ray_tracing_maintenance1`, `VK_KHR_ray_tracing_position_fetch` |
| SER | optional (separate SER and non-SER pipeline permutations; see below) | `VK_EXT_ray_tracing_invocation_reorder` |
| Opacity micromaps | optional (N4; fallback: any-hit alpha test) | `VK_KHR_opacity_micromap` (its `VK_KHR_device_address_commands` dependency), else `VK_EXT_opacity_micromap` |
| Geometry | optional (fallback: compute culling + indirect draws, N7) | `VK_EXT_mesh_shader` (`meshShader`, `taskShader`) |
| Shading rate | optional (fallback: software VRS in compute lighting, §6.8) | `VK_KHR_fragment_shading_rate` |
| Latency | optional, default build | `VK_NV_low_latency2` (+ `VK_KHR_present_id`), `VK_AMD_anti_lag` |
| NVIDIA-only | optional, later phases | `VK_NV_cluster_acceleration_structure`, `VK_NV_partitioned_acceleration_structure`, `VK_NV_cooperative_vector` |
| Existing | unchanged | `VK_KHR_portability_subset` (Compat only), `VK_KHR_pipeline_executable_properties` |

**SER needs its own permutation.** SPIR-V that declares the `ShaderInvocationReorderEXT`
capability is invalid on a device without the extension, so it cannot be "always emitted"
as roadmap §5.5 puts it. Each ray-generation shader that reorders is built twice by the
permutation table: with SER and without it (`hitObject` calls compiled out). The renderer
creates only the pipeline whose capabilities the device enables. The same rule holds for
every optional capability (OMM, mesh shaders, fragment shading rate, cooperative vectors):
a module is created only on a device that enables what it declares.

**Mesh shaders and VRS are optional, not required.** This is a deliberate deviation from
roadmap §5.1, which listed them as required.
- **Low does not use them.** In the §3 feature matrix, Low uses "GPU-driven, discrete LODs",
  and the mesh-shader path starts at Medium.
- **Each has a planned fallback:** compute culling with indirect count (N7), and software VRS
  in the compute lighting (§6.8).
- **Requiring them gains nothing in N0.** No N0 code uses them, and requiring them only
  turns a missing extension into a Compat device.
- **The cost of requiring them is probably small, though.** Secondary sources consistently
  say Turing supports `VK_EXT_mesh_shader`, and RADV exposes both on RDNA 2. No primary source
  (driver notes or a recorded dump) for RTX 20 was found in this pass, so it is not treated as
  verified. If the RTX 2060 and RX 6600 dumps confirm both, requiring them costs nothing and
  removes two fallbacks.
- **The final call** is left to the maintainer as [question 3](#open-questions-for-the-maintainer).

Local evidence on 2026-10-31 (`vulkaninfo`):
- **RTX 5070 Ti** (driver 615.78.08, Vulkan 1.4.351) exposes every extension in the table
  except the NVIDIA-only optional row, which was not checked.
  - The list includes `VK_EXT_ray_tracing_invocation_reorder`, both opacity-micromap
    extensions and `VK_EXT_descriptor_heap`.
  - It reports `maxPerStageDescriptorUpdateAfterBindSampledImages` = 1,048,576.
- **The Radeon 610M iGPU** (RADV, Mesa 26.2.5, Vulkan 1.4.354) exposes:
  - acceleration structures, ray query and the ray-tracing pipeline;
  - mesh shaders, fragment shading rate and descriptor heap;
  - an update-after-bind sampled-image limit of 8,388,606.
- `VK_AMD_anti_lag` shows on both devices. That is likely Mesa's anti-lag implicit layer
  [S23], not the driver.

### 2. Capability gate, render paths and the Compat freeze

**2a. Two paths.** A new enum, `RenderPath { Tiered, Compat }`, is chosen when the renderer is
created.
- **Tiered.** The renderer that N1–N11 build. It needs the tiered required set (§1e) and the
  bindless limits (§3).
- **Compat.** Today's renderer: the forward frame of ADR-0011 to ADR-0019, plain descriptor
  sets (ADR-0009), GLSL only, and today's four presets with today's numbers.

**2b. Probing and admission.**
- **A pure function decides the path.** `caps::admit(&DeviceCapabilities, &Overrides) ->
  Admission { path, max_tier, missing: Vec<&'static str> }` is unit-tested against fixture
  capability sets.
- **Fixtures.** Recorded fixtures cover the RTX 5070 Ti, Radeon 610M and lavapipe dumps from
  `vulkan-probe --json`. Synthetic fixtures, labelled as such, cover:
  - MoltenVK and KosmicKrisp;
  - NVK;
  - GTX 1060;
  - UHD 620;
  - a device with RT but no mesh shaders.
- **Admission rules:**
  - A device that misses any required item is **Compat**, and `missing` names what it lacks.
  - **macOS is always Compat** (§2d), even if a future MoltenVK exposes ray tracing behind an
    opt-in. The gate checks three things, any one of which is enough:
    - `cfg!(target_os = "macos")`;
    - a driver ID of `MOLTENVK` or `MESA_KOSMICKRISP` (`VkPhysicalDeviceDriverProperties`);
    - `VK_KHR_portability_subset` enabled.
    The portability subset alone is not enough: KosmicKrisp is a conformant driver and does not
    advertise it.
  - **A CPU device (`PhysicalDeviceType::CPU`, such as lavapipe) is never admitted to Tiered
    automatically.** `MOBA_RENDER_PATH=tiered` admits it (for CI's RT goldens).
  - Overdrive also needs `VK_KHR_ray_tracing_pipeline`. A device without it tops out at
    Ultra.
  - Ultra and Overdrive need the SDK features (roadmap §7), **once those features exist**.
    - **Until then they are selectable in every build.** That covers N0 up to the phases that
      land the SDK-backed features: ReSTIR DI and SHaRC (N8), DLSS RR and the path tracer
      (N9). Neither tier yet has a feature that needs an SDK.
    - **From the phase that makes a tier depend on an SDK feature,** that tier is shown
      disabled in a build without the feature, or on a GPU it does not support. The reason is
      given ("built without `rtxdi`", "needs an NVIDIA RTX GPU"). It is not selectable with the
      feature silently off.
- **Device choice: device type first, path second.** Devices are ranked by:
  1. device type: discrete > integrated > virtual > CPU (today's order);
  2. then, within one type, admitted to Tiered over Compat-only;
  3. then the existing +10 for a shared graphics and present family.

  So a non-RT discrete GPU (a GTX 1060) beats an RT-capable iGPU (the Radeon 610M) and runs
  Compat. A GPU always beats lavapipe. On a hybrid laptop with an RT dGPU, the dGPU wins on
  type anyway.
- **Overrides.** `MOBA_RENDER_PATH=auto|tiered|compat`, with a matching settings entry:
  - `auto` (default): the ranking above, then the chosen device's own admission. CPU devices
    are Compat.
  - `compat`: the same ranking; the chosen device runs Compat. This is for testing and player
    choice.
  - `tiered`: the ranking is applied **only to devices admitted to Tiered**, CPU devices
    included. With none, renderer creation fails with an error that lists each device and
    what it lacks. It never yields a half-working frame.
  - The ranking never overrides a user's choice of device. The existing loader and driver
    mechanisms for that (for example `MESA_VK_DEVICE_SELECT`) keep working, because they
    filter the device list before the ranking.
- **The default tier.**
  - A small table keyed by PCI vendor and device ID maps the reference GPUs (§10) to their §3
    tier.
  - Any other admitted device defaults to **Low**, and Compat devices default to Compat
    Medium.
  - The tier is detected once, then saved in the settings. `MOBA_QUALITY` still overrides it.
- **Logging.** The frame log and `vulkan-probe` print `path=`, `max_tier=` and `missing=`, so
  evidence always names the path it ran on.

**2c. What the paths share during N0–N2.**
- The tiered renderer does not exist as separate code yet. In N0 the Tiered path runs
  today's frame, configured by the new presets (§5), plus whatever N0 adds: bindless, the
  capability gate, the Slang smoke passes and budget-driven dynamic resolution.
- Each later phase replaces a part of the Tiered frame behind its own switches. N2 is where
  the frames diverge (G-buffer and compute deferred lighting).
- From then on, Compat keeps its own copy of what Tiered replaced.

**2d. Compat: the Vulkan 1.3 fallback, refreshed once, then frozen** (roadmap §1).
- **Who uses it:**
  - macOS (MoltenVK and KosmicKrisp);
  - NVK, which has mesh shaders and fragment shading rate but no ray tracing [S10];
  - Pascal, Polaris, and Intel Gen9–Gen11 iGPUs;
  - anything without hardware RT;
  - players who choose it.
- **What it keeps:**
  - the Vulkan 1.3 minimum (ADR-0002);
  - plain descriptor sets and the 16-sampler ceiling (ADR-0009, `descriptor_budget`);
  - GLSL and glslang only (§4);
  - today's presets, which keep their names in code (`CompatPreset::{Low, Medium, High,
    Ultra}`, today's `QualityTier` values) and show as "Compat Low … Compat Ultra" in the
    menu;
  - today's `perf-gate` budgets (14, 6, 6 and 7.5 ms). They are checked against the RTX 5070
    Ti baseline, as today. That is not the hardware the Compat presets target (GTX 1060 / RX
    580, Apple M1), so they guard against regressions rather than proving those targets.
- **Timeline:**
  1. **N0 to C1:** Compat changes only where shared code moves, for example the `ash` bump.
     Its goldens stay byte-identical.
  2. **C1:** the one refresh (roadmap §8).
  3. **After C1 merges, Compat is frozen.**
- **What is allowed after the freeze:**
  - bug, security and driver-workaround fixes;
  - content-format compatibility. A PR that changes a content format ships Compat's fallback
    in the same PR, and Compat must load every content pack.
  - Compat's goldens stay in CI for good (roadmap §11 risk "Compat bit-rot").
- **Not allowed after the freeze:** new Compat features. A PR that adds one needs an ADR
  amendment.
- **MoltenVK and macOS constraints that stay in force:**
  - the portability-subset rules (ADR-0002);
  - full water, soft particles and distortion off (ADR-0017, ADR-0019).
- **No macOS change is pending upstream.**
  - MoltenVK ray tracing is still the open WIP PR #2771, and mesh shaders the open PR #2842.
  - The latest MoltenVK release is v1.4.2 (2026-07-24) [S9].
  - KosmicKrisp lists no RT or mesh extension [S10].
  - Revisit only if one ships *and* Apple hardware can hold Low's budget, which roadmap §1
    doubts.

### 3. Bindless (tiered path)

**3a. The model.** One global **bindless set** is bound at set index 0 of every tiered
pipeline layout and never rebound within a frame. Its layout uses
`UPDATE_AFTER_BIND_POOL`, and its bindings use `PARTIALLY_BOUND`,
`UPDATE_AFTER_BIND` and `UPDATE_UNUSED_WHILE_PENDING`.

| Binding | Type | Capacity | Notes |
|---|---|---|---|
| 0 | `SAMPLER[]` | 32 | A fixed palette: wrap/clamp × point/linear/aniso × compare; written once at creation |
| 1 | `STORAGE_IMAGE[]` | 1,024 | Compute writes; needs `shaderStorageImageWriteWithoutFormat` |
| 2 | `SAMPLED_IMAGE[]` | 16,384 (`VARIABLE_DESCRIPTOR_COUNT`) | Every 2D/3D/cube texture: materials, terrain layers, VAT, LUTs, baked lighting, render targets read as textures |

- **The variable-count array is the last binding.** The spec allows
  `VARIABLE_DESCRIPTOR_COUNT` only on the highest-numbered binding of a set
  (VUID-VkDescriptorSetLayoutBindingFlagsCreateInfo-pBindingFlags-03004). That is why the
  texture array is binding 2.

- **Buffers use buffer device address, not a descriptor array.** This departs from roadmap
  §5.1, which describes texture, sampler **and buffer** arrays. Instance, material, light,
  meshlet and BVH-instance buffers are passed as 64-bit addresses in push constants or in a
  per-frame constant block.
  - BDA is already mandatory with `VK_KHR_acceleration_structure`.
  - It removes the per-stage storage-buffer ceiling (4 on the minimum device) that ADR-0017
    had to count.
- **Acceleration structures** (TLAS) go in a small per-frame set at index 1 (N4).
- **Handles.** `BindlessTable` hands out typed `u32` handles from free lists at load time.
  Slot 0 of each array is a valid fallback (a 1×1 magenta, white or flat-normal image, and a
  linear sampler), so a stale or zero index never reads an unwritten descriptor.
- **Writes happen only at load or stream-in** (N11). Freed slots are recycled only after the
  frames in flight have retired. No descriptor is written per frame, so `locked_uploads` is
  unaffected.
- **Shaders** index with `NonUniformResourceIndex` in Slang or `nonuniformEXT` in GLSL
  wherever the index can diverge.

**3b. Limits relied on.** These are checked at admission. A failure means Compat, never a
smaller table.

| Feature / limit | Needed | Spec minimum when supported |
|---|---|---|
| `shaderSampledImageArrayNonUniformIndexing`, `shaderStorageImageArrayNonUniformIndexing` | true | — |
| `descriptorBindingSampledImageUpdateAfterBind`, `…StorageImageUpdateAfterBind`, `…PartiallyBound`, `…VariableDescriptorCount`, `…UpdateUnusedWhilePending`, `runtimeDescriptorArray` | true | — |
| `maxPerStageDescriptorUpdateAfterBindSampledImages`, `maxDescriptorSetUpdateAfterBindSampledImages` | ≥ 16,384 | 500,000 [S11] |
| `maxPerStageDescriptorUpdateAfterBindSamplers` | ≥ 32 | 500,000 |
| `maxPerStageDescriptorUpdateAfterBindStorageImages` | ≥ 1,024 | 500,000 |
| `maxUpdateAfterBindDescriptorsInAllPools` | ≥ 17,440 | 500,000 |
| `maxPerStageUpdateAfterBindResources` | ≥ 17,440 + per-pass sets | 500,000 |

- Every conformant device that has the features meets these limits by a wide margin. The
  RTX 5070 Ti reports 1,048,576 and RADV 8,388,606.
- 16,384 textures is headroom, not a measurement. A match's real texture count is measured
  in A3 (roadmap §9.1). The capacity is a constant, raised in one place.
- `descriptor_budget` gains the update-after-bind limits for tiered layouts. The 16-sampler
  ceiling applies only to Compat.

**3c. Fallbacks.**
- **No third path.** A device without the descriptor-indexing features is Compat (plain sets,
  ADR-0009). In practice every RT-capable desktop driver has them.
- **ADR-0009's revisit condition is met for the tiered path.** It said "a feature needs
  arbitrary resource access", and that now applies: GPU-driven culling (N7), deferred material
  resolve (N2) and RT hit shading (N4) all need it.
- **ADR-0009's MoltenVK and UHD 620 measurement condition is moot.** Both are Compat-only now.

**3d. Descriptor heap later.**
- `VK_EXT_descriptor_heap` (ratified, spec 1.4.340, 2026-01-23) deprecates
  `VK_EXT_descriptor_buffer` [S6].
- Both local devices already expose it, but cross-vendor driver coverage is not established.
- `BindlessTable` is the only code that creates or writes the bindless set, so a descriptor-heap
  back end can replace it later without touching shaders' handle math.
- `VK_EXT_descriptor_buffer` is **not** adopted, because it is deprecated.

### 4. Slang toolchain

**4a. Scope.**
- GLSL and glslang stay for every existing pass and for all of Compat. The current release is
  glslang 16.6.0 (2026-09-11), which supports ray query, ray tracing, mesh shaders, fragment
  shading rate and `GL_EXT_shader_invocation_reorder` [S15].
- **New tiered passes are written in Slang:** RT, denoising, ReSTIR, the radiance cache,
  neural passes, and any tiered rewrite of an existing pass. For example, N2's deferred
  lighting is a new Slang shader, not an edit of `unit.frag`.
- **Vendor HLSL shader libraries** (RTXDI, SHaRC) are compiled through `slangc` too.
- Existing shaders are **not** ported wholesale. Compat's GLSL freezes with Compat.

**4b. Licence.**
- Slang is `Apache-2.0 WITH LLVM-exception` (repository `LICENSE`) [S12], which is already on
  `deny.toml`'s allow-list.
- `slangc` is a build tool. Neither it nor `libslang` is linked into the client.
- Generated SPIR-V may embed code from Slang's core module, and the LLVM exception covers that.
- Governance: Khronos has hosted the Slang Initiative since 2024-11-21. The repository is
  still in the `shader-slang` GitHub organisation.

**4c. How it compiles: at build time, with a pinned and checksummed `slangc`.**
- **`crates/shaders/build.rs`** compiles every `shaders/slang/*.slang` entry point to SPIR-V
  next to the GLSL modules, with the same permutation table and the same
  `include_bytes!` consumer. Nothing compiles at run time, so `locked_pipelines=0` holds.
- **Compiler lookup.** It looks for `slangc` in `SLANGC`, then in
  `target/tools/slang-<version>/bin`, then on `PATH`.
- **Version check.** It runs `slangc -version` and **requires the exact version** pinned in
  `crates/shaders/slang-version.txt`. The proposed initial pin is **v2026.20**
  (2026-10-17) [S12].
  - A mismatch fails the build.
  - `MOBA_SLANG_ANY_VERSION=1` relaxes the check for distribution packagers and prints a
    warning.
- **Fetching.** `scripts/fetch-slang.sh` (with `.ps1` for Windows) downloads the pinned release
  archive into `target/tools/` for the host, from these builds:
  - `linux-x86_64`, `linux-aarch64`;
  - `windows-x86_64`;
  - `macos-aarch64`, `macos-x86_64`.
- **Checksums.** Upstream publishes no checksum files or attestations. GitHub's release API
  does report a SHA-256 `digest` for each asset. For example, `slang-2026.20-linux-x86_64.tar.gz`
  is `sha256:e863e568…347c9`. The script checks the SHA-256 we record per archive in
  `crates/shaders/slang-version.txt`. The PR that pins a version cross-checks those values
  against the API digests.
- **No network access in build scripts.** `build.rs` never downloads anything; offline and
  sandboxed builds work once the toolchain is present.
- **The Vulkan SDK** has shipped Slang since 1.3.296.0, and its latest version is 1.4.363.0
  [S13]. The SDK's `slangc` is accepted only if its version matches the pin.
- **Flags.**
  - Every module is built with `-target spirv`, a SPIR-V 1.6 / Vulkan 1.3 profile, explicit
    `[[vk::binding]]` layout, column-major matrices and `-O2`.
  - **`spirv-val --target-env vulkan1.3` becomes mandatory** for Slang modules: a missing
    validator is a build error. For GLSL it stays optional, as today.
  - **`build.rs` declares `cargo:rerun-if-env-changed`** for `SLANGC` and
    `MOBA_SLANG_ANY_VERSION` (and keeps watching `GLSLC`), so switching compilers rebuilds the
    shaders.
- **Device safety.** The tiered-only modules (bindless, ray query, SER) are built on every
  platform, but only a device admitted to Tiered creates them (§1e). A Compat device never
  loads a module that declares a capability it lacks.
- **Layout check.** `slangc -reflection-json` output for the shared structures (frame
  constants, the instance record, the material record) is compared in a unit test with the
  Rust `#[repr(C)]` sizes and offsets. Layouts therefore have one source of truth per path,
  and drift fails a test, not a frame.
- **CI.** Every workflow that builds the shaders runs `fetch-slang`, cached by version:
  `ci.yml`, `release.yml`, `nightly.yml`, `render-golden.yml`, `macos-vulkan.yml`,
  `macos-frame.yml`, `windows-probe.yml` and `nightly-perf.yml` (self-hosted). Those jobs also
  install `spirv-val`, since today the Linux and macOS jobs install only glslang:
  - `spirv-tools` (apt) on Linux;
  - `spirv-tools` (brew) on macOS;
  - the Vulkan SDK, which already ships it, on Windows.
- **macOS also compiles the Slang modules,** although it only runs Compat. Shader errors are
  therefore caught on every platform, and the build has one shape everywhere.

**4d. Migration path.**
- **N0 adds the pipeline plus two smoke shaders, not a port:**
  - `bindless_smoke.slang`, a compute pass that samples 64 bindless slots into a buffer;
  - `rt_smoke.slang`, a compute pass that ray-queries a 4-triangle TLAS into a 64×36 hit mask.
- They run in CI on lavapipe with validation, and on the RTX. They prove Slang, BDA, bindless
  and ray query end to end before N1 depends on them.
- From N1 on, every new tiered pass is Slang, and shared Slang modules live in
  `shaders/slang/lib/`.
- **Vendor HLSL.** RTXDI's library is HLSL that upstream compiles with DXC [S20]. Compiling
  it with `slangc` is **unverified**, so N0 runs a spike to check it. If it fails, DXC (LLVM
  licence) is used for that vendor library only, behind the `rtxdi` feature.

### 5. Tier presets and budgets

**5a. The tier scale.**
- `QualityTier` gains `Overdrive`. This is additive, since the enum is `#[non_exhaustive]` and
  `ALL` is a slice.
- On the Tiered path the five tiers mean what roadmap §3 says. On Compat, Overdrive clamps to
  Compat Ultra.
- `MOBA_QUALITY` and the settings accept `overdrive`.
- A settings file written before N0 is re-detected once (§2b) instead of reinterpreting the
  old "ultra" on new hardware.

**5b. The budget table.** There is one source of truth: `moba_assets::budget::GPU_BUDGETS`.
It sits next to `TIER_BUDGETS` as pure data with no Vulkan, so `perf-gate` and `moba-render`
read the same numbers.

| Tier | Reference GPU class | Output | Upscaler mode (from N1) | Target, p95, FG off | **GPU budget** | Dynres scale range |
|---|---|---|---|---|---|---|
| Low | RTX 2060, RX 6600 | 1080p | Quality | 60 fps | **12.0 ms** | 0.50–0.67 |
| Medium | RTX 3060, RX 6700 XT | 1080p | Quality | 144 fps | **6.0 ms** | 0.50–0.67 |
| High | RTX 4060 Ti / 4070, RX 7700 XT | 1440p | Quality | 120 fps | **7.5 ms** | 0.50–0.67 |
| Ultra | RTX 4070 Ti Super / 5070 Ti, RX 9070 XT | 1440p (4K) | Quality (Performance at 4K) | 120 (90 at 4K) fps | **9.0 ms** | 0.50–0.67 |
| Overdrive | RTX 4080 / 5080 and up | 1440p–4K | Performance + RR | 60 fps rendered | **15.0 ms** | 0.33–0.50 |
| Compat (frozen) | today's targets | — | today's render scale | — | 14 / 6 / 6 / 7.5 ms (today) | today's |

**5c. Dynamic resolution follows the budget.**
- On the Tiered path, dynamic resolution is **on by default on every tier**.
  - Its target is the tier's GPU budget.
  - The controller's existing 0.95 high-water and 0.75 low-water marks provide the headroom.
  - It replaces today's fixed 14 ms.
- **The scale range follows the upscaler modes.** The ceiling is the preset's mode at the
  output resolution:
  - Quality (0.67) at 1080p and 1440p;
  - Performance (0.50) for Ultra at 4K and for Overdrive.

  The floor is **Performance (0.50)** where the ceiling is Quality, which is two modes down,
  skipping Balanced (0.58). Where the ceiling is already Performance, the floor is **Ultra
  Performance (0.33)**, one mode down. The table shows the 1440p ranges. Ultra at 4K uses
  0.33–0.50, like Overdrive.
- **Until N1 ships the upscalers,** the range is applied to today's render scale with
  bicubic upscaling. The 0.5 floor is today's lower bound.
- The budget is measured as today:
  - on the R5 teamfight replay (`tests/replays/teamfight-5v5.mbr`);
  - with frame generation off;
  - as p95.
- Frame generation is reported separately (roadmap §3).

**5d. The perf gate.**
- **Budgets.** `perf-gate` checks Tiered logs against `GPU_BUDGETS` and Compat logs against
  today's table. Frame logs gain `path=`.
- **Baselines.** Baselines are keyed by `(gpu, path, tier)`, so there is one baseline per
  reference GPU (roadmap §3, §10). The baseline file already names its GPU.
- **Phasing.**
  - Tiered budgets are **report-only until N1 merges.** Before upscaling, the tiered presets
    are today's frame at native resolution, and a failing gate would only measure the absence
    of N1.
  - From N1 they fail the gate.
  - Compat budgets keep failing the gate, as today.

**5e. Presets in N0.**
- `TierPreset` is a table of the §3 feature matrix. Each feature is a level with an off
  switch, and features not built yet are recorded as `Planned(phase)`, so the settings menu can
  show them greyed out.
- Until the phases land, the tiered presets map onto today's switches. Today's "Ultra" is
  roughly the new Medium:
  - **Tiered Low** uses today's High post and lighting configuration, with FXAA.
  - **Medium, High, Ultra and Overdrive** use today's Ultra configuration, **MSAA 4×
    included**.
  - MSAA is retired in N1, when TAA and the upscalers replace it (roadmap §5.2, §8), not in N0.
    Retiring it earlier would leave tiered Medium and up on FXAA only, below today's Ultra.
- **VRAM budgets** gain an Overdrive entry. The tiered values are re-derived from the
  smallest card in each tier's reference class, keeping ≥ 25 % headroom for the driver and
  compositor, so a budget is at most 75 % of that card:

| Tier | Smallest reference card | 75 % of it | Proposed budget |
|---|---|---|---|
| Low | RTX 2060, 6 GB (RX 6600: 8 GB) | 4.5 GiB | 4 GiB |
| Medium | RTX 3060 8 GB variant (12 GB model, RX 6700 XT: 12 GB) | 6 GiB | 6 GiB |
| High | RTX 4060 Ti 8 GB (4070: 12 GB, RX 7700 XT: 12 GB) | 6 GiB | 6 GiB |
| Ultra | RTX 4070 Ti Super, 5070 Ti, RX 9070 XT: 16 GB | 12 GiB | 12 GiB |
| Overdrive | RTX 4080 / 5080: 16 GB | 12 GiB | 12 GiB |

  - These are to be validated by A3 and N11, not taken as measured.
  - If a tier needs more than its smallest card allows, the fix is to drop that card from
    the tier's class, not to break the headroom rule. High and Medium are both capped by
    8 GB cards today.

### 6. Cargo features for the NVIDIA, AMD and Intel SDKs

**6a. Layout.**
- The features live on `moba-render` and are forwarded with the same names by `moba-client`
  and by `moba-engine` (under `render`).
- The FFI code is in `crates/render/src/vendor/{ngx,streamline,nrd,rtxdi,sharc,fsr,xess,latency}.rs`.
  Only `moba-render` may use `unsafe`, so no new crate needs an invariant exception.
- No server crate can reach these features. The existing layering check covers that, because
  they sit behind `moba-render`.

| Feature | Default | SDK (pinned, verified 2026-10-31) | Platforms | Licence and obligations | Source of the binary |
|---|---|---|---|---|---|
| *(none)*: `VK_NV_low_latency2` (Reflex), `VK_AMD_anti_lag` | **on** (no feature) | Vulkan extensions only | Linux, Windows | Registry extensions; no SDK code is used [S18][S23] | — |
| `fsr3` | **on** | FSR 3.1.6 upscaler shaders from FidelityFX SDK **v2.4.0** (2026-10-28) [S22]; they are **HLSL only** there | Linux, Windows (our own Vulkan back end) | **MIT**: the one SDK whose source is vendored, under `third_party/fidelityfx/` with its licence and listed in `THIRD_PARTY.md` | HLSL compiled in-tree by `slangc`; fallback: FSR 3.1.4's GLSL from SDK v1.1.4, compiled by glslang |
| `dlss` | off | DLSS SDK **v310.9.16** (2026-10-16): SR presets L/M ("DLSS 4.5"), RR transformer preset F [S16] | Linux (`.so`), Windows (`.dll`); NVIDIA RTX only | NVIDIA RTX SDKs License (DLSS `LICENSE.txt`): object code only (§1c); not subject to an open-source licence (§4e); NVIDIA GPUs only (supplement §1); **notify NVIDIA before commercial release** (supplement §4); attribution and NVIDIA marks (supplement §7.1(b)); NVIDIA may update or disable over the air (supplement §6) | Fetched |
| `streamline` (implied by `dlss-fg`) | off | Streamline **v2.14.16** (2026-10-16) [S17] | **Windows only** | Streamline itself is **MIT**; the DLSS-G binaries it loads fall under the RTX SDKs License above | Fetched |
| `nrd` | off | NRD **v4.18.0** (2026-10-30), built from source [S19] | Linux, Windows; any vendor | NVIDIA RTX SDKs License (NRD copy): §4e; credits attribution (supplement §7.1(c)); must stay interoperable with NVIDIA hardware; **not** NVIDIA-only | Fetched source, built by `build.rs` (CMake) |
| `rtxdi` | off | RTXDI **v3.1.0** (2026-09-03), HLSL library [S20] | Linux, Windows; any vendor | RTX SDKs License: §4e, attribution, interoperability; not NVIDIA-only | Fetched source → `slangc` (DXC fallback) |
| `sharc` | off | SHaRC **1.8.3** in RTXGI **v2.8.0** (2026-10-13) [S21] | Linux, Windows; any RT GPU | RTX SDKs License (RTXGI `License.md`): as for RTXDI | Fetched source (HLSL/GLSL headers) |
| `fsr4` | off | FSR 4.1.2 (ML) in FidelityFX SDK **v2.4.0**: signed `upscaler_vk.dll` [S22] | **Windows only**; RDNA 2+ (INT8 on RDNA 2/3, FP8 on RDNA 4) | **Ambiguous:** the SDK's `license.md` sets binary-only redistribution without reverse engineering as the default, but also lists the signed DLLs under its MIT text. Treated as binary-only until a legal reading ([Q6](#open-questions-for-the-maintainer)) | Fetched |
| `xess` | off | XeSS SDK **3.0.2** (2026-07-24) [S24] | **Windows only** (no `.so`); Intel, plus a DP4a path on other vendors | Intel Simplified Software License: redistribute binaries unmodified, no reverse engineering (also at run time); headers MIT. XeSS-FG and XeLL are DX12-only and excluded | Fetched |

Aggregate features:
- `nvidia = ["dlss", "nrd", "rtxdi", "sharc"]`;
- `vendor-sdks = ["nvidia", "fsr4", "xess"]`;
- `dlss-fg = ["dlss", "streamline"]`.

NRC (Windows, CUDA and `NRC_Vulkan.dll`, experimental) and RTX NTC get feature names only
when their phases start.

**6b. Closed SDKs are fetched, never vendored.** The only exception is MIT-licensed FSR 3.1
source (§6a), which may sit in the tree like any other permissive dependency. Everything
under the RTX SDKs, Intel or AMD binary licences is fetched.
- **The manifest.** `vendor-sdks.toml` at the repository root lists each SDK with:
  - its name and pinned tag;
  - the archive URL (an official GitHub release or tag archive);
  - **its git submodules and CMake downloads, each pinned as an entry of its own,** because
    GitHub tag archives leave submodules out. Each entry has the repository, the commit and a
    SHA-256 of the archive at that commit:
    - **RTXDI** v3.1.0 keeps its shader library in a submodule, `Libraries/Rtxdi` →
      `NVIDIA-RTX/RTXDI-Library` at `f12037fa8e97` ("3.1 Update", 2026-09-03). The `rtxdi`
      feature pins that library directly, not the sample repository.
    - **SHaRC** 1.8.3 lives in the standalone `NVIDIA-RTX/SHARC` repository as an **untagged
      commit**, `4e21b585c33c` ("v1.8.3", 2026-08-02). RTXGI v2.8.0's `Libraries/Sharc`
      submodule points at it. The `sharc` feature pins it directly.
    - **NRD** v4.18.0's CMake fetches ShaderMake (archive of `3f01623a76b0`) and MathLib
      (`v11`) with `FetchContent`, and NRI (`v191`) when `NRD_NRI` is on. All are MIT. The
      fetch script pre-fetches ShaderMake and MathLib. `build.rs` configures NRD with
      `FETCHCONTENT_FULLY_DISCONNECTED=ON`, `FETCHCONTENT_SOURCE_DIR_SHADERMAKE`,
      `FETCHCONTENT_SOURCE_DIR_MATHLIB` and `NRD_NRI=OFF`.
    - **ShaderMake's own downloads are turned off as well.** As a subproject it enables
      `SHADERMAKE_FIND_COMPILERS`. Its `SHADERMAKE_FIND_DXC` defaults to ON on non-Apple hosts
      and `FetchContent`-downloads DXC v1.9.2609. Its `SHADERMAKE_FIND_FXC` defaults to ON on
      Windows. This is in ShaderMake@`3f01623`'s `CMakeLists.txt`, checked 2026-10-31.
      `build.rs` therefore sets:
      - `SHADERMAKE_FIND_DXC=OFF` and `SHADERMAKE_FIND_FXC=OFF`;
      - `SHADERMAKE_FIND_DXC_VK=ON`, which uses the Vulkan SDK's DXC;
      - `NRD_EMBEDS_SPIRV_SHADERS=ON`, with `NRD_EMBEDS_DXIL_SHADERS=OFF` and
        `NRD_EMBEDS_DXBC_SHADERS=OFF`. Those two default to ON on Windows and would need
        DXIL and FXC.
    - With these settings the configure step never downloads. The Vulkan SDK's DXC becomes a
      build requirement of the `nrd` feature only.
  - the archive's **SHA-256**;
  - the path of its licence file inside the archive, and **that file's SHA-256**;
  - its platforms.
- **The fetch command.** `scripts/fetch-sdks.sh [--only dlss,…]` (`.ps1` on Windows) downloads
  into `target/vendor-sdks/<name>-<tag>/`, which is gitignored and outside the source tree, and
  verifies every hash. It unpacks submodules and CMake dependencies at the paths the SDK's
  build expects.
  - **A changed licence text fails the fetch.** That forces the re-read roadmap §7 asks for
    at each bump; the PR that updates the hash records the re-read.
- **Building with a feature.** The feature's `build.rs` code reads `MOBA_SDK_DIR` (default
  `target/vendor-sdks`). If the SDK is missing, it fails with a message naming the fetch
  command. Build scripts never download.
- **FFI declarations are generated at build time** from the fetched headers with `bindgen`.
  It is a build dependency only, under the feature, and BSD-3-Clause. Nothing derived from
  an SDK's text is ever committed (§4e; [Q5](#open-questions-for-the-maintainer)).
- **Linking.**
  - NGX links its static library: `libnvsdk_ngx.a` on Linux, the `.lib` on Windows.
  - DLLs and shared objects that the SDK loads itself (the DLSS feature binaries, Streamline
    plug-ins) are copied next to the executable by the packaging script.
  - FSR 4 and XeSS are loaded at run time with `libloading` (ISC), so a feature build still
    starts, and falls back, when a DLL is absent.
- **Notices.** `THIRD_PARTY.md` and the in-game credits gain the required notices and marks
  for each feature that is compiled in. The release checklist gains "notify NVIDIA" (DLSS
  supplement §4) before any commercial release.

**6c. The open build stays whole.**
- `cargo build` with default features needs no proprietary SDK. That build contains Reflex
  and Anti-Lag through extensions, the FSR 3.1 MIT upscaler, and in-house TAA (N1),
  denoising and shadow-ray lighting.
- It **runs Low to High** (roadmap §7). Ultra and Overdrive stay selectable until N8 and N9
  land their SDK-backed features. After that they are shown disabled in this build, with the
  missing features named (§2b), as roadmap §7 says.
- **Upscaler and denoiser selection** is a run-time trait object chosen from what is both
  compiled in and supported by the device. It never depends on `cfg` at call sites outside
  `vendor/`.
- **Commands that change:**
  - `cargo check -p moba-engine --all-features` (AGENTS.md, `agent-verify --full`) would now
    need the SDKs. The verify script uses `--features render,…` with every non-vendor feature,
    and runs the vendor features only when `MOBA_SDK_DIR` is populated.
  - A new CI job, `vendor-sdks` (Linux and Windows), fetches the pinned SDKs and builds and
    links `--features vendor-sdks`. Run-time evidence comes from the self-hosted GPU runner
    and the Windows VM (§10).
  - `cargo-deny`'s `all-features` graph gains `bindgen` and its transitive build-time crates
    (`clang-sys` and others). `libloading` is already in the graph through `ash`. `bindgen`
    also needs **libclang** on the build machine, but only for the vendor features.

## Departures from the roadmap

These are the places where this ADR decides differently from the roadmap's text. Each needs
the maintainer's agreement when the ADR is accepted.

| Roadmap says | This ADR decides | Why |
|---|---|---|
| §5.1: mesh shaders and `VK_KHR_fragment_shading_rate` are required for the tiered renderer | Optional, with fallbacks (§1e) | Low does not use them, and the fallbacks are planned anyway. To be revisited with the reference-GPU dumps (Q3) |
| §5.1: bindless is "large texture, sampler and buffer arrays" | Texture, sampler and storage-image arrays. Buffers go through buffer device address (§3a) | BDA is already mandatory with acceleration structures, and it removes the per-stage storage-buffer limits |
| §5.1: the budget table "fails `perf-gate`" | Tiered budgets are report-only until N1 merges; Compat budgets keep failing it (§5d) | Before upscaling, tiered presets render natively, so failures would only measure N1's absence (Q11) |
| §5.5: SER "always emitted, a no-op where the hardware lacks it" | Separate SER and non-SER permutations (§1e) | SPIR-V declaring `ShaderInvocationReorderEXT` is invalid without the extension |
| §7: the SDK feature list has no FSR 3.1 entry, and SDKs are fetched | A default-on `fsr3` feature with the MIT FSR 3.1 shaders vendored in-tree (§6a, §6b) | They are MIT and the only cross-platform vendor upscaler. SDK 2.4 cannot build them on Linux |

The two points below follow the roadmap, after review:
- **MSAA stays on tiered Medium and up until N1** (roadmap §8), not retired in N0.
- **Ultra and Overdrive follow roadmap §7.** Once their SDK-backed features land (N8, N9),
  they are not selectable in a build without those features; they are shown disabled with
  the reason. Until then they are selectable.

## Alternatives considered

- **Stay on `ash` 0.38.0 and hand-write the missing extensions.**
  - This would avoid both git dependencies.
  - The cost is our own copies of the `vk.xml`-generated structs, constants and `Extends*`
    impls for SER, KHR OMM, anti-lag and the NV extensions, which would grow every phase.
  - It also defers the master API break to an unknown 0.39 date, when it would hit far more
    RT code.
  - It remains the **fallback** if the bump PR hits a blocker.
- **Generate our own bindings** from `vk.xml`. This is the most control and the most code to
  own, which is exactly what `ash` already maintains. Rejected.
- **A direct `ash = { git = … }` dependency without `[patch]`.** This duplicates `ash` in the
  graph and gives `gpu-allocator` the wrong `vk` types. Rejected.
- **Replace `gpu-allocator` with an in-house allocator.** This is possible later, but it is
  out of scope for N0. The `ash-next` branch builds today.
- **Mesh shaders and VRS required for admission** (roadmap §5.1). This excludes devices for
  features Low does not use. Rejected for now; see question 3.
- **One renderer without Compat.** The maintainer already decided against it (roadmap §1).
- **Bindless everywhere, including Compat.** That would reopen ADR-0009 on MoltenVK and old
  iGPUs for a frozen path. Rejected.
- **Vulkan descriptor buffers.** Deprecated by `VK_EXT_descriptor_heap`. Rejected.
- **Slang through the `shader-slang` crate** (0.1.0, MIT OR Apache-2.0), linking `libslang`
  into the build. That is a large native dependency with an early-stage API, for no gain over
  a pinned CLI. Rejected.
- **Committing precompiled SPIR-V.**
  - It is reproducible without the toolchain, but reviews would see binaries and the source
    would drift from the output.
  - The pinned, checksummed `slangc` gives reproducibility without this.
- **Downloading in `build.rs`.** That breaks offline, sandboxed and docs.rs builds and hides
  network access in `cargo build`. Rejected.
- **rust-gpu.** It is still pinned to nightly (roadmap §5.1). Rejected.
- **A separate crate per SDK** (`moba-dlss`, …). Each would need `unsafe`, which AGENTS.md
  invariant 5 confines to five crates. Modules under `moba-render` keep the rule. This can be
  revisited if `moba-render`'s build time suffers.
- **Committing FFI declarations written by hand from SDK headers.** It is smaller than
  `bindgen`, but its licence status under RTX SDKs §4e is unclear. Generating them at build
  time avoids the question; see Q5.

## Consequences

- **Determinism, wire and replays: no change.**
  - No change to `PROTOCOL_VERSION` (v42), replay `VERSION` (v5), `SIM_LOGIC_VERSION` (v9),
    content hashes or `tests/golden-state-hash.txt`.
  - Rendering never touches the simulation, the wire, replays or the state hash. Capability,
    path, tier and SDK choices are client-local presentation.
- **Dependencies.**
  - Two git sources (`ash-rs/ash`, `Traverse-Research/gpu-allocator`) enter `Cargo.lock` and
    `deny.toml`'s allow-list.
  - Crates.io publishing (#21) becomes harder. A published `moba-render` would resolve
    crates.io `ash` 0.38 and fail to compile against the master API. Publishing waits for
    `ash` 0.39 (or the fallback above). It is already `publish = false`.
- **MSRV:** 1.88 is unchanged (§1d).
- **Build tools and CI.**
  - Every build needs the pinned `slangc` in addition to glslang. Contributors run
    `scripts/fetch-slang.sh` once.
  - CI gains a fetch step on all three operating systems.
  - `spirv-val` becomes mandatory wherever Slang modules build.
  - `AGENTS.md` "Fresh checkout" and `docs/dev/` are updated when the code lands.
- **Licences.**
  - The repository stays MIT/Apache plus the vendored MIT FidelityFX shaders.
  - Proprietary SDKs exist only in `target/vendor-sdks/` and in binaries built with their
    features.
  - Public GitHub release artefacts stay the **open build** until the maintainer decides
    otherwise (Q7).
  - Shipping a DLSS build commercially triggers the NVIDIA notification and the marks
    obligations.
- **The Compat freeze.**
  - Compat is the only path on macOS, NVK and non-RT GPUs, for good. Its feature set ends at
    C1.
  - Its goldens and `perf-gate` budgets stay in CI.
  - Content-format PRs carry a Compat fallback.
- **Existing goldens.**
  - Existing goldens are captured with `MOBA_RENDER_PATH=compat` and stay byte-identical.
    N0 changes no Compat pixel.
  - Under `auto`, lavapipe is a CPU device and always runs Compat (§2b), so the existing
    goldens keep their path anyway. The explicit `compat` pins them against later changes
    to the admission rules. The new tiered smoke goldens run with `MOBA_RENDER_PATH=tiered`.
- **Public API (additive, `#[non_exhaustive]`).**
  - `QualityTier::Overdrive`;
  - `RenderPath`, `DeviceCapabilities`, `Admission`;
  - `RenderConfig::path`;
  - `TierPreset`, `moba_assets::budget::GPU_BUDGETS`;
  - `CompatPreset`, a rename of today's tier semantics behind a deprecated alias for one
    release.
- **Settings.** A settings re-detect happens once.
- **Work estimate.** This matches roadmap N0's **M** size. The largest single items are the
  `ash` API migration and the CI toolchain steps.

## Verification and evidence gates

These follow roadmap §10, applied to N0.

1. **The bindings bump PR.**
   - `scripts/agent-verify.sh --full` is green.
   - The 1.88.0 `msrv` job is green, and `cargo-deny` is green with the allow-git entries.
   - All goldens are byte-identical. They are captured twice on lavapipe, with validation and
     sync validation.
   - The macOS MoltenVK probe is green.
   - On an RTX 5070 Ti run, the Compat GPU timings (the median of 3 interleaved rounds) are
     within noise of `main`.
2. **The capability gate.**
   - Unit tests on every fixture: path, `max_tier` and the `missing` list.
   - `vulkan-probe --json` dumps from the RTX 5070 Ti, the Radeon 610M (RADV, an RT
     correctness smoke test, not a performance reference) and lavapipe are committed as
     recorded fixtures.
   - The macOS CI probe must report `path=compat`.
   - Each reference GPU adds its dump when it arrives (#9/#127). Until then its admission is
     **synthetic and labelled so**: estimates never close a target.
   - The RTX 2060 dump specifically settles `VK_EXT_mesh_shader` on Turing.
3. **Bindless and Slang.**
   - `bindless_smoke` and `rt_smoke` run on lavapipe (small resolution, fixed frame) and on
     the RTX with validation and sync validation clean, and their outputs are checked
     exactly.
   - The Slang modules compile with the pinned `slangc` and pass `spirv-val` on Linux,
     Windows and macOS CI.
   - The reflection layout test passes.
   - The RTXDI-through-`slangc` spike is recorded, pass or fail.
4. **Budgets.**
   - The `GPU_BUDGETS` table is unit-tested to be ordered and to match roadmap §3.
   - Dynamic resolution holds a forced budget in a frame-log test: an artificial target on the
     teamfight replay, with the scale settling and p95 under the target.
   - `perf-gate` reads `path=`. Tiered logs are report-only until N1.
5. **Vendor SDKs.**
   - The `vendor-sdks` CI job fetches, verifies both hashes, and builds and links on Linux and
     Windows.
   - The default build passes with `MOBA_SDK_DIR` unset.
   - Run-time checks on the RTX (Linux) and in the Windows VM (`windows-vm`): with every
     back end compiled in but its binary removed, the client must start and fall back with
     one log line.
6. **Every phase:**
   - `locked_uploads=0` and `locked_pipelines=0`;
   - the readability tests (roadmap §4), once they exist (N1);
   - the evidence recorded in a new `docs/perf/` entry.

## Open questions for the maintainer

1. **Take the `ash` git pin in N0, or defer it to N1, when `VK_AMD_anti_lag` first needs it?**
   *Recommendation:* N0. The API break is mechanical now, and N1–N4 would otherwise be
   written against an API about to change.
2. **For `gpu-allocator`, track upstream's `ash-next` branch revision, or fork it to
   `JakeGT/gpu-allocator`?** *Recommendation:* the upstream revision. Fork only if upstream
   stops following ash master. Either way it is one `rev =` line.
3. **Make mesh shaders and fragment shading rate required for the tiered path (roadmap
   §5.1), or keep them optional with fallbacks (this ADR)?** *Recommendation:* optional.
   Reconsider once the RTX 2060 and RX 6600 dumps are in. If both expose them, requiring them
   costs nothing and removes two fallbacks.
4. **Slang for every build, including macOS (Compat-only at run time)?**
   *Recommendation:* yes, with a single build shape and the shader errors caught everywhere.
   The cost is one cached download per CI job.
5. **FFI to closed SDKs: generate it at build time with `bindgen` (this ADR), or commit
   declarations written by hand?** *Recommendation:* `bindgen` at build time unless a legal
   reading says hand-written declarations are fine under RTX SDKs §4e and the Intel and AMD
   licences.
6. **FSR 4's DLL licence is ambiguous** (binary-only by default, yet listed under MIT).
   *Recommendation:* keep `fsr4` out of any distributed build until AMD or a lawyer confirms
   redistribution terms. It is development-only until then.
7. **Should public GitHub releases include the SDK features?** *Recommendation:* no, ship
   the open build only until the commercial-release decision. Then build the release with
   `vendor-sdks`, add the notices and marks, and notify NVIDIA first.
8. **Is Linux DLSS frame generation in scope?** The DLSS SDK ships a Linux `dlssg` shared
   object (§6), but the roadmap assumed FG is Windows-only through Streamline.
   *Recommendation:* don't promise it. Run a spike in N9 using NGX directly on Linux, and keep
   Streamline as the Windows path.
9. **FSR 3.1 on Linux needs our own port of the MIT shaders,** because the SDK 2.4 build is
   Windows-only. *Recommendation:* port it in N1 into `third_party/fidelityfx/`, as Godot
   did, behind the default `fsr3` feature.
10. **The default-tier detection table and the proposed VRAM budgets** (§2b, §5e).
    *Recommendation:* accept them as provisional. A3 (vertical slice) and the reference cards
    replace them with measurements.
11. **Tiered `perf-gate` budgets report-only until N1?** *Recommendation:* yes. Before
    upscaling they would only measure the absence of N1.

## Corrections to the roadmap

These were re-checked on 2026-10-31. This PR only adds a link to this ADR from roadmap §12;
the corrections themselves are not folded into the roadmap text. The maintainer
may fold the corrections in when this ADR is accepted.

- **§5.1 bindings.** "Pin `ash` to a git revision" is not enough on its own: released
  `gpu-allocator` 0.28.0 does not compile against ash master, so it needs a git revision too
  (§1a). Ash master is at headers 1.4.352, as the roadmap says, but Khronos headers are at
  1.4.367 [S5].
- **§5.1 capability gate.** This ADR makes mesh shaders and VRS optional (§1e). See
  [Departures from the roadmap](#departures-from-the-roadmap). `VK_EXT_mesh_shader` on RTX 20
  is widely reported by secondary sources but has no primary source yet. The RTX 2060 dump
  settles it.
- **§5.3 and §7, FSR 3.1.6.**
  - The version, the MIT licence and Vulkan support are correct (FidelityFX SDK v2.4.0,
    2026-10-28).
  - "Linux and Windows" is **out of date for the SDK itself**: it builds only on Windows and
    ships only `.dll`s. Linux needs our own port of the MIT source (Q9).
- **§5.3 and §7, FSR 4.1.2.**
  - Correct: Vulkan since SDK v2.4.0, RX 6000+ via INT8 models, signed DLLs, Windows.
  - Two additions:
    - the SDK's own "Limitations" line still says RX 7000/9000 only;
    - the DLL licence is ambiguous (Q6).
- **§5.5 and §6.3, AMD.** Ray Regeneration and Radiance Caching are DX12-only. The roadmap is
  correct. The AMDVLK driver was discontinued on 2025-09-15 [S25], so RADV is the AMD driver on
  Linux.
- **§7 Anti-Lag.** Native `VK_AMD_anti_lag` in RADV lands in Mesa 26.3 (not yet released).
  Mesa 25.3 provides it as an implicit layer [S23].
- **§7 DLSS FG.** The DLSS SDK also ships Linux `.so` files for frame generation (`dlssg`).
  The roadmap's "Windows only" holds for Streamline, but NGX-direct on Linux is open (Q8).
- **§6.3 SHaRC.** SHaRC 1.8.3 is in the standalone `NVIDIA-RTX/SHARC` repository, as the
  untagged commit `4e21b585c33c` ("v1.8.3"). The repository's latest *tag* is older, v1.6.5.0.
  RTXGI v2.8.0 pulls it in as a submodule. Pin that commit (§6b).
- **§6.2 RTXDI.** Upstream compiles its HLSL with DXC. Compiling it through Slang is
  unverified (§4d).
- **§6.3 NRC.** "Windows plus CUDA only" is correct. It *does* have a Vulkan DLL
  (`NRC_Vulkan.dll`), with no Linux binaries.
- **§5.1 Slang.** The licence and Khronos hosting are correct. The repository lives in the
  `shader-slang` organisation, not `KhronosGroup`.
- **§5.5 OMM.** `VK_KHR_opacity_micromap` (spec 1.4.351, 2026-05-08) is correct. It depends
  on `VK_KHR_device_address_commands`, and `VK_EXT_opacity_micromap` is promoted to it.
- **Confirmed unchanged:**
  - SER cross-vendor since 2025-11 (spec 1.4.333);
  - `VK_EXT_descriptor_heap` ratified in January 2026;
  - MoltenVK PR #2771 still a WIP;
  - NVK without RT;
  - Mega Geometry extensions NVIDIA-only, with no EXT or KHR version;
  - no public DLSS 5 SDK;
  - glslang 16.6;
  - XeSS-FG and XeLL DX12-only.

## Sources

All accessed 2026-10-31.

- [S1] crates.io, `ash` versions: <https://crates.io/api/v1/crates/ash/versions>
- [S2] ash master `8844f6b`, `ash/Cargo.toml` and changelog:
  <https://github.com/ash-rs/ash/commit/8844f6b4eefb16d08cd73c8f3d340e1ac12deb16>,
  <https://github.com/ash-rs/ash/blob/master/Changelog.md>
- [S3] ash milestones: <https://github.com/ash-rs/ash/milestones>
- [S4] `gpu-allocator` 0.28.0 dependencies and the `ash-next` branch:
  <https://crates.io/api/v1/crates/gpu-allocator/0.28.0/dependencies>,
  <https://github.com/Traverse-Research/gpu-allocator/tree/ash-next>
- [S5] Vulkan-Headers: <https://github.com/KhronosGroup/Vulkan-Headers>
- [S6] Vulkan registry `vk.xml`: <https://github.com/KhronosGroup/Vulkan-Docs/blob/main/xml/vk.xml>
- [S7] Khronos, SER extension:
  <https://www.khronos.org/blog/boosting-ray-tracing-performance-with-shader-execution-reordering-introducing-vk-ext-ray-tracing-invocation-reorder>
- [S8] `VK_KHR_opacity_micromap`:
  <https://github.com/KhronosGroup/Vulkan-Docs/blob/main/appendices/VK_KHR_opacity_micromap.adoc>
- [S9] MoltenVK: <https://github.com/KhronosGroup/MoltenVK/pull/2771>,
  <https://github.com/KhronosGroup/MoltenVK/pull/2842>,
  <https://github.com/KhronosGroup/MoltenVK/releases>
- [S10] Mesa `features.txt`: <https://gitlab.freedesktop.org/mesa/mesa/-/blob/main/docs/features.txt>
- [S11] Vulkan required limits:
  <https://docs.vulkan.org/refpages/latest/refpages/source/Required_Limits.html>
- [S12] Slang: <https://github.com/shader-slang/slang/blob/master/LICENSE>,
  <https://github.com/shader-slang/slang/releases/tag/v2026.20>,
  <https://www.khronos.org/news/permalink/khronos-group-launches-slang-initiative-hosting-open-source-compiler-contributed-by-nvidia>
- [S13] LunarG Vulkan SDK: <https://vulkan.lunarg.com/sdk/latest.json>
- [S14] `shader-slang` crate: <https://crates.io/crates/shader-slang>
- [S15] glslang 16.6.0: <https://github.com/KhronosGroup/glslang/releases/tag/16.6.0>
- [S16] DLSS SDK and `LICENSE.txt`: <https://github.com/NVIDIA/DLSS>;
  <https://developer.nvidia.com/rtx/dlss>; notification form:
  <https://developer.nvidia.com/sw-notification>
- [S17] Streamline: <https://github.com/NVIDIA-RTX/Streamline>
- [S18] `VK_NV_low_latency2`:
  <https://registry.khronos.org/vulkan/specs/1.3-extensions/man/html/VK_NV_low_latency2.html>
- [S19] NRD: <https://github.com/NVIDIA-RTX/NRD>
- [S20] RTXDI: <https://github.com/NVIDIA-RTX/RTXDI>
- [S21] RTXGI (SHaRC, NRC): <https://github.com/NVIDIA-RTX/RTXGI>,
  <https://github.com/NVIDIA-RTX/RTXGI/blob/main/Docs/NrcGuide.md>
- [S22] FidelityFX SDK: <https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/releases/tag/v2.4.0>,
  <https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/releases/tag/v1.1.4>
- [S23] Anti-Lag: <https://github.com/KhronosGroup/Vulkan-Docs/blob/main/appendices/VK_AMD_anti_lag.adoc>,
  <https://gpuopen.com/anti-lag-2/>, <https://www.phoronix.com/news/Mesa-26.3-RADV-VK_AMD_anti_lag>,
  <https://github.com/GPUOpen-LibrariesAndSDKs/AntiLag2-SDK>
- [S24] XeSS: <https://github.com/intel/xess>
- [S25] AMDVLK discontinued: <https://github.com/GPUOpen-Drivers/AMDVLK>,
  <https://www.phoronix.com/news/AMDVLK-Discontinued>
- **Local evidence (2026-10-31, this machine):**
  - `vulkaninfo` on the RTX 5070 Ti (driver 615.78.08) and the Radeon 610M (RADV, Mesa 26.2.5);
  - a scratch crate checked with `cargo check` (stable and 1.88.0): `ash` at `8844f6b` with
    `gpu-allocator` `ash-next` at `6a68a5b4c233` builds, and with crates.io `gpu-allocator`
    0.28.0 it fails on `push_next`.
