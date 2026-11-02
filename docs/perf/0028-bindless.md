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
| lavapipe | PASS, 8,197 values, 0 mismatches | PASS, 4,097 values, 0 mismatches | 0 / 0 |
| RADV (Radeon 610M) | PASS, 8,197, 0 | PASS, 4,097, 0 | 0 / 0 |
| RTX 5070 Ti | PASS, 8,197, 0 | PASS, 4,097, 0 | 0 / 0 |

Every device was admitted `path=tiered max_tier=overdrive missing=none`.

- **`bindless-smoke`:** 222 registrations (212, then 10 reusing the lowest of 12 slots
  released in frame 1 and retired), leaving 210 live textures and 2 stale handles that read
  magenta. 215 distinct texture handles and 6 samplers are sampled across the 64×64 pixels,
  once by the fragment pass and once by the compute pass (8,192 texels, plus 5 checks of the table: pending releases before and after retirement, slot reuse, no writes in the frame, the storage handle retired).
  Descriptor writes: **17,440 at creation** (the fallback in every slot), **235 at load**
  (registrations and the retirement's rewrites) and **0 in the frame**.
- **`rt-smoke`:** a BLAS of 3 triangles and a TLAS of 3 instances (one mirrored, with custom
  indices); 2,432 rays miss and the hits per instance and primitive are
  `[[325, 276, 153], [99, 118, 125], [325, 156, 87]]`, identical on all three drivers. No ray
  passes closer than 0.0110 to an edge line, so no result depends on a driver's watertightness.
  Acceleration-structure sizes differ by driver (BLAS + TLAS: 320 + 544 bytes on lavapipe,
  2,560 + 2,432 on the RTX), the results do not.
- **They fail when they should.** A deliberately wrong CPU slot hash failed with
  `mismatches=7810` and exit 1; `auto` or `compat` on lavapipe exits 2 with a message naming
  `MOBA_RENDER_PATH=tiered`.
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
branch captures, no validation message. The smoke table above is from the final build.

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

No pass binds the table, so the tiered frame must cost what N0.3's does. Both builds run
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
  branch does not touch; the branch adds no per-frame work, so this is not a gain.
- Ultra's p95 is bimodal on this GPU (as in [perf/0027](0027-tiers-budgets.md)): the branch's
  rounds gave 0.730, 0.689 and 0.724 ms, N0.3's 0.698, 0.692 and 0.693 ms. The p50 is equal.
- VRAM peak is identical: the table's images are 1×1 and its pool is under 1 MB. The branch
  makes 36 uploads to N0.3's 33, the three fallback textures, all at start-up.

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

Left for N1, when a pass first evicts textures in bulk: `retire` writes one descriptor per
recycled slot rather than merging runs of slots into one write.
