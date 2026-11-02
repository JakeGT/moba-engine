# FSR 3.1.6 upscaler port: standalone smoke (N1b stage 1)

Captured 2026-11-02 on the development workstation for ADR-0021 §4b–§4c
([amendment 1](../adr/0021-temporal-upscaling-overlay.md#amendment-1-2026-11-02-n1b-the-fsr-316-port-as-built)).
This is stage 1 only: the port runs standalone in `fsr3-smoke`. It is not yet behind the
`TemporalUpscaler` trait (N1a), so there are no readability, golden or timing results here
(gates 3, 4 and 6 are stage 2).

- GPUs: lavapipe (`llvmpipe (LLVM 23.1.3, 256 bits)`, the software-Vulkan set in
  `agents.local.md`) and NVIDIA GeForce RTX 5070 Ti, driver 615.78.08; both on the tiered path,
  `max_tier=overdrive`
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-5-cachyos
- Toolchain: rustc 1.99.1 stable, `slangc` v2026.20 (pinned), `spirv-val` from the pinned set
- Build: release, branch `feat/n1b-fsr3`
- Command: `MOBA_RENDER_PATH=tiered MOBA_VULKAN_VALIDATION=required target/release/fsr3-smoke`
  (validation and synchronization validation on), two processes per device; the RTX runs under
  the workstation's RTX lock
- Evidence: `~/.cache/moba-evidence/n1b-fsr3/` (`lavapipe-{1,2}.stdout`, `rtx-{1,2}.stdout`,
  empty `.stderr` files)

## What the smoke does

One FSR 3.1 context at 192×108 output with a 128×72 maximum render size (Quality) upscales a
synthetic scene whose ideal output is known: a flat colour, a smooth analytic field (static,
moving one render pixel a frame with matching motion vectors, and with RCAS sharpening), a
sweep of render sizes inside the same context without a reset (§4c), and a fine field near the
render resolution's limit. Inputs carry sentinel values outside the frame's render rectangle, so
a read outside it would show as error. Each process runs the 310-frame script twice on fresh
contexts and compares every output frame bit for bit.

## Results

| Phase | lavapipe mean / max | RTX 5070 Ti mean / max | Bounds (mean, max) |
|---|---|---|---|
| flat | 0.00014 / 0.00039 | 0.00006 / 0.00039 | 0.002, 0.004 |
| static (Quality, 128×72) | 0.00560 / 0.02682 | 0.00564 / 0.02731 | 0.012, 0.060 |
| moving | 0.00426 / 0.02982 | 0.00426 / 0.03006 | 0.012, 0.070 |
| sharpened (RCAS) | 0.00655 / 0.03171 | 0.00659 / 0.03219 | 0.015, 0.070 |
| sizes: Balanced 112×63 | 0.00737 / 0.02731 | 0.00744 / 0.02780 | 0.025, 0.100 |
| sizes: Performance 96×54 | 0.00899 / 0.03024 | 0.00911 / 0.03268 | 0.025, 0.100 |
| sizes: Ultra Performance 64×36 | 0.01093 / 0.03410 | 0.01104 / 0.03610 | 0.025, 0.100 |
| sizes: odd 77×43 | 0.01180 / 0.05155 | 0.01187 / 0.05130 | 0.025, 0.100 |
| sizes: back to Quality 128×72 | 0.01165 / 0.04422 | 0.01169 / 0.04373 | 0.025, 0.100 |
| fine: first frame after reset | 0.03885 / 0.12155 | 0.03885 / 0.12155 | — |
| fine: converged | 0.03068 / 0.10397 | 0.03067 / 0.10349 | ≤ 0.9× first frame |

| | lavapipe | RTX 5070 Ti |
|---|---|---|
| Result | PASS, checked=36, mismatches=0 | PASS, checked=36, mismatches=0 |
| Validation / sync validation messages | 0 errors, 0 warnings | 0 errors, 0 warnings |
| Output digest (310 frames) | `2f896f88d99212e2` | `fed947ce861f272e` |
| Identical across the two in-process runs | yes | yes |
| Identical across the two processes | yes | yes |
| Fine detail, converged ÷ first frame | 0.79× | 0.79× |
| `vram_bytes` (128×72 → 192×108 context) | 743 104 | 1 111 168 |

The digests differ between devices, as expected for floating-point compute on different
hardware; each device is deterministic run to run. `vram_bytes` is the sum of the context's
allocations, which each driver rounds differently.

**§4c:** one context accepted every render size from Ultra Performance's to its maximum, plus an
odd 77×43, from frame to frame without a reset, and each step converged inside its bound with
no validation message. A size one pixel over the maximum is refused (`accepts` returned false
for 129×72 and true for 77×43).

## Not measured here

- Timings: no GPU time against TAAU (gate 6), which needs the trait wiring of stage 2.
- Readability tests (gate 3) and goldens (gate 4): stage 2.
- Windows and macOS (MoltenVK): not run. The context refuses a device without quad subgroup
  operations in compute or the listed format features (`unavailable`); that refusal has not been
  exercised on such a device.
- Only the 32-bit permutation (`FFX_HALF=0`) is built; there is no FP16 comparison.
