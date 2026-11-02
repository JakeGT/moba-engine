# N0.5: the bindless table and the tiered smokes

Captured 2026-11-02 on the development workstation, not a CI runner, for
[ADR-0020](../adr/0020-next-gen-foundations.md) §3 ([amendment 5](../adr/0020-next-gen-foundations.md#amendment-5-2026-11-02-n05-the-bindless-table-as-built))
and verification gates 3 and 6. The design summary is [dev/bindless.md](../dev/bindless.md).

- **Branch and baseline:** branch `feat/n0-bindless` against N0.3 (`feat/n0-tiers-budgets` before its rebase onto main for #354,
  `a71877a`).
- **Hardware and software:**
  - GPU: NVIDIA GeForce RTX 5070 Ti (16 GB), driver 615.78.08.
  - Integrated GPU: AMD Radeon 610M (RADV `RAPHAEL_MENDOCINO`), Mesa 26.2.5.
  - Software Vulkan: lavapipe (`llvmpipe`, LLVM 23.1.3) from `/home/jaket/Projects/MOBA/.tools`.
  - Validation: the Khronos validation layer 1.4.363 bundled in `.tools`, used on every
    device, with synchronization validation (`MOBA_VULKAN_VALIDATION=required`).
  - CPU: AMD Ryzen 7 9800X3D. OS: Linux 7.3.0-5-cachyos, Wayland.
  - Build: release (the smokes also passed in debug on the two GPUs).

## Gate 3: the smokes, exact on three drivers

Each smoke renders one fixed 64×64 frame on a headless tiered device and compares every value
with its CPU expectation; any validation message fails it. lavapipe ran with
`MOBA_RENDER_PATH=tiered` (it is a CPU device); both GPUs are admitted under `auto`.

| Device | `bindless-smoke` | `rt-smoke` | Validation errors / warnings |
|---|---|---|---|
| lavapipe | PASS, 8,198 values, 0 mismatches | PASS, 4,096 values, 0 mismatches | 0 / 0 |
| RADV (Radeon 610M) | PASS, 8,197, 0 (first version) | PASS, 4,097, 0 (first version) | 0 / 0 |
| RTX 5070 Ti | PASS, 8,197, 0 (first version) | PASS, 4,097, 0 (first version) | 0 / 0 |

Every device was admitted `path=tiered max_tier=overdrive missing=none`. The lavapipe row is
the smokes after the correctness review ([below](#correctness-review)): 2×2 textures that
check the sampler palette, the comparison samplers and descriptor writes while the frame is
pending. The RADV and RTX rows are the first version (1×1 textures, one tautological check
each). Re-run on the review build: TODO(maintainer-run).

- **`bindless-smoke`** (review build, lavapipe): 223 registrations before the frame (212
  2×2 textures and a 2×2 depth texture, then 10 reusing the lowest of 12 slots released in
  frame 1 and retired), leaving 211 live textures and 2 recycled slots that read magenta. 214
  distinct texture handles and the 6 non-comparison samplers are sampled across the 64×64
  pixels at uv (−0.125, 0.625), where wrap/clamp and point/linear give four different exact
  values; the compute pass also compares the depth texture through the 6 comparison samplers
  (alpha 0, 240, 45 and 180 for point/wrap, point/clamp, linear/wrap and linear/clamp). That
  is 8,192 texels, plus 6 CPU-side checks of the table: pending releases before and after
  retirement, slot reuse order, the slot reused while the frame was pending, the descriptor
  writes made while it was pending, and the output's release and retirement after it.
  Descriptor writes: **17,440 at creation** (the fallback in every slot), **236 before the
  frame** (registrations and the retirement's rewrites) and **4 while it was pending**
  (a texture slot the frame does not index released, retired and reused; a storage image
  registered, released and retired), none of them to a slot the frame indexes.
- **`rt-smoke`:** 4,096 rays (the first version also counted a tautological "no descriptor
  writes in the frame" check). A BLAS of 3 triangles and a TLAS of 3 instances (one mirrored, with custom
  indices); 2,432 rays miss and the hits per instance and primitive are
  `[[325, 276, 153], [99, 118, 125], [325, 156, 87]]`, identical on all three drivers. No ray
  passes closer than 0.0110 to an edge line, so no result depends on a driver's watertightness.
  Acceleration-structure sizes differ by driver (BLAS + TLAS: 320 + 544 bytes on lavapipe,
  2,560 + 2,432 on the RTX), the results do not.
- **They fail when they should.** A deliberately wrong CPU slot hash failed with
  `mismatches=7810` and exit 1; `auto` or `compat` on lavapipe exits 2 with a message naming
  `MOBA_RENDER_PATH=tiered`. On the review build (lavapipe, each a temporary edit, reverted):
  - every palette slot holding slot 0's sampler: `mismatches=5986`, exit 1;
  - wrap and clamp swapped in the sampler create info: `mismatches=7435`, exit 1;
  - point and linear swapped: `mismatches=5712`, exit 1;
  - the bindings without `UPDATE_AFTER_BIND | UPDATE_UNUSED_WHILE_PENDING`: 0 mismatches but
    `validation_errors=1` (VUID-vkUpdateDescriptorSets-None-03047, a write into a set a
    pending command buffer uses), exit 1.
  - no validation layer on the layer path, `MOBA_VULKAN_VALIDATION` unset: `rt-smoke` prints
    `FAIL ... validation=missing` and exits 1; with `MOBA_VULKAN_VALIDATION=off` it passes.
- **Reviewer finding fixed before these runs:** decorating only the array indices
  `nonuniformEXT` passed validation and these exact checks on all three drivers, but leaves the
  `OpSampledImage` undecorated (VUID-RuntimeSpirv-NonUniform-06274), so a driver may sample one
  descriptor per wave. The `BINDLESS_SAMPLER2D_NONUNIFORM` macro decorates the combined
  sampler too (checked in the SPIR-V). A passing smoke cannot prove this; the macro does.

```bash
MOBA_RENDER_PATH=tiered MOBA_VULKAN_VALIDATION=required ./target/release/bindless-smoke   # lavapipe env: dev/software-vulkan.md
MOBA_VULKAN_VALIDATION=required ./target/release/rt-smoke                                 # a tiered GPU
```

## Compat unchanged: goldens byte-identical (lavapipe)

The fifteen golden scenes (the seven of `render-golden.yml` plus overview, horizon, the
three river views and the three distortion views) at 1280×720 on lavapipe, with validation
and synchronization validation required, compared as whole PPMs with `cmp`: each scene once
on N0.3 (`a71877a`) and twice on the branch.

| Scenes | Path | Result |
|---|---|---|
| all 15 | default (`auto`, which is Compat on lavapipe) | N0.3 = branch run 1 = branch run 2, byte for byte |
| all 15 | `MOBA_RENDER_PATH=tiered` on both builds | N0.3 = branch, byte for byte; the two branch runs equal |

| Scene | SHA-256 (first 16), default | Scene | SHA-256 (first 16), default |
|---|---|---|---|
| default | `5a476005e52ac38b` | overview | `b0c819a49b7f3843` |
| network | `e7e6198ad2260eb3` | horizon | `79f126b8576a82bc` |
| vat | `09b8ef828a3eed7b` | river | `b34379f7ad633a4d` |
| ultra | `4b8beb4c337bd8be` | river-low | `6d4feae28f6f3160` |
| low-scaled | `25aa95a501ac7ad1` | river-ultra | `217e397eef618b00` |
| ember | `342a3fa53facf1df` | distortion | `cedc451ffedcc6f3` |
| decals | `8a07fb957cb153b2` | distortion-flat / -nodraw | `ba6f715199a7c375` / `a3846daa3f69e8cf` |

No run printed a validation message. On the tiered path the branch creates the table in every
run (the log line `bindless table: 32 samplers, 1024 storage images, 16384 textures; 17440
descriptors written at creation`) and the pixels still equal N0.3's tiered pixels, because
no pass binds it. Tiered captures differ from Compat ones where §5e maps a tier to a
different configuration (tiered High runs Ultra), as on N0.3.

The goldens ran on a branch build from before the free-list change ([below](#perf-review)) (it only changes
`SlotAllocator`'s containers; Compat never creates one). The final build was rechecked on
four scenes (default on both paths, decals and distortion on tiered): identical to the earlier
branch captures, no validation message. The correctness-review build was rechecked on all fifteen scenes on
the default path (Compat on lavapipe), once each: every SHA-256 above, byte for byte, and no
validation message. That build changes no Compat code and no production shader (only the
smoke shaders include `bindless.glsl`). The smoke table above is from the final build.

## Frame log on the RTX 5070 Ti

The teamfight replay (`tests/replays/teamfight-5v5.mbr`, warm-up 330 ticks, 3,000 frames,
the log covering the last 2,940) at 1920×1080, vsync off, default quality (High):

| Path | `config_tier` | Bindless table | Uploads | `locked_uploads` / `locked_pipelines` | VRAM peak |
|---|---|---|---|---|---|
| `auto` → tiered | ultra | created (17,440 descriptors at creation) | 36 | 0 / 0 | 301.4 MB |
| `compat` | high | none | 33 | 0 / 0 | 136.0 MB |

The tiered path's three extra uploads are the 1×1 fallback textures, made at start-up. The
VRAM difference is the tiered path's Ultra configuration (§5e), not the table, whose images
are 1×1.

## Tiered timings: within noise (RTX 5070 Ti)

No pass binds the table, so the tiered frame does no new per-frame work. The branch does
change one thing every tiered frame touches: every tiered memory block now carries
`DEVICE_ADDRESS` (amendment 5), so the comparison is not free by construction. Both builds run
`auto` (tiered on this GPU). Teamfight replay, 1920×1080, vsync off, validation off, no
pipeline cache, 3,000 frames per run (the log covers the last 2,940); **three rounds,
interleaved** N0.3 / branch for each tier. Each cell is the median of the three rounds; the
range is their spread.

| Tier (`config_tier`) | Build | GPU total p50, ms (range) | GPU total p95, ms | scene p50, ms | VRAM peak | `locked_uploads` / `locked_pipelines` |
|---|---|---|---|---|---|---|
| Low (high) | N0.3 | 0.530 (0.528–0.530) | 0.541 | 0.163 | 144.4 MB | 0 / 0 |
| Low (high) | branch | 0.522 (0.521–0.523) | 0.534 | 0.163 | 144.4 MB | 0 / 0 |
| High (ultra) | N0.3 | 0.682 (0.681–0.684) | 0.692 | 0.251 | 301.4 MB | 0 / 0 |
| High (ultra) | branch | 0.680 (0.678–0.680) | 0.690 | 0.252 | 301.4 MB | 0 / 0 |
| Ultra (ultra) | N0.3 | 0.682 (0.682–0.684) | 0.693 | 0.251 | 301.4 MB | 0 / 0 |
| Ultra (ultra) | branch | 0.679 (0.679–0.680) | 0.724 | 0.251 | 301.4 MB | 0 / 0 |

- The scene pass is equal on every tier. Low's 0.008 ms lower total held in all three rounds
  and sits almost entirely in the VFX simulation dispatch (p50 0.013 → 0.009 ms), which the
  branch does not touch; the branch records no new per-frame work, so this is not a gain.
- Ultra's p95 is bimodal on this GPU (as in [perf/0027](0027-tiers-budgets.md)): the branch's
  rounds gave 0.730, 0.689 and 0.724 ms, N0.3's 0.698, 0.692 and 0.693 ms. The p50 is equal.
  Because every tiered block now carries `DEVICE_ADDRESS`, three rounds cannot rule out a
  real p95 cost (0.724 against 0.693 ms); it needs more rounds. Extra Ultra rounds:
  TODO(maintainer-run).
- The VRAM peak column cannot show the table's cost: `crate::memory` counts each resource's
  `requirements.size` only, so neither the descriptor pool's memory nor any cost of
  `DEVICE_ADDRESS` allocations appears in it. Estimates: the table's images are 1×1 (bytes),
  and the pool holds 17,440 descriptors, about 16,384 × 32 B ≈ 0.5 MB for the sampled images
  on RADV (descriptor sizes are driver-specific; not measured). The branch makes 36 uploads
  to N0.3's 33, the three fallback textures, all at start-up (one submission since the
  review, together with the storage fallback's clear).

```bash
rtx-ab.sh <out> <N0.3 worktree> <branch worktree> "low high ultra"   # the shared A/B script
```

## Perf review

The `perf-reviewer` pass found nothing per frame (the table is created once; the 17,440
fallback descriptors are written in one `vkUpdateDescriptorSets` call with three writes), but
flagged what a future per-frame `retire` would pay. Fixed before the final build:
- `SlotAllocator` re-sorted the whole free list (up to 16,381 slots) whenever one slot
  retired. It is now a min-heap (`BinaryHeap<Reverse<u32>>`), O(log n) per slot.
- Its pending queue and the table's recycle buffer grew on first use. Both are now sized for
  every slot at creation (`VecDeque`, `Vec::with_capacity`); a unit test churns every slot ten
  times and checks that no capacity changes.
- `live()` scanned 16,384 flags; it is now a counter.

Left for N1, when a pass first calls `retire` per frame (bulk eviction): it writes one
descriptor per recycled slot. Then merge adjacent slots into one write per run, and add a
criterion bench for it; the code comment on `BindlessTable::retire` says the same.

## Correctness review

An independent correctness review found three should-fixes, fixed on the review build:
- **The stale-handle promise was wrong.** The first version said a stale handle recorded
  after its retirement reads the fallback, or the slot's next texture. If that frame is still
  pending when the slot is reused, the reuse writes a descriptor a pending command buffer
  uses (VUID-vkUpdateDescriptorSets-None-03047): undefined behaviour, reachable from safe
  functions given a wrong frame number. `release_texture`, `release_storage_image` and
  `retire` are now `unsafe fn`s whose `# Safety` sections state the contract (`frame` is the
  last frame, submitted or being recorded, that may index the handle; `completed` has
  retired), and recording a released handle is documented as a caller bug.
- **The sampler palette was never checked on the GPU.** With 1×1 textures every sampler
  returns the texel, so the smoke passed with the sampler ignored. The 2×2 textures and the
  coordinate above make every address/filter pair, and every comparison entry, read a
  different exact value (unit-tested on the CPU), and the experiments above fail.
- **Update after bind was never exercised.** Every write happened before the frame was
  recorded and the frame was waited for at once, so the smokes passed without the flags, and
  the "no descriptor writes in the frame" checks were tautological. The frame is now
  submitted and waited for separately, with four writes in between (above); the tautological
  checks are gone.

Also from that review: the fallbacks are documented as valid for 2D float reads only,
`check_tiered` counts every stage a binding reaches (it counted vertex, fragment and compute
only), `DescriptorSetLayout::with_binding_flags` states its create-info requirements, and a
smoke run without validation fails unless `MOBA_VULKAN_VALIDATION=off` was given. The perf
review's start-up nit is fixed too: the fallback textures and the storage fallback's clear
share one submission (they were two blocking submissions).

**lavapipe limit found on the way:** on Mesa 26.2.5 a fragment shader that takes a second
texture sample after a `nonuniformEXT` one crashes (SIGSEGV in the JIT'd shader, with or
without validation, with uniform or divergent indices for the second sample); the same two
samples in a compute shader work. So the comparison samplers are checked in the compute pass
only. N1 fragment shaders that sample several bindless textures will hit this on lavapipe;
re-check on the next Mesa update.
