# FSR 3.1.6 upscaler port: standalone smoke (N1b stage 1)

Captured 2026-11-02 on the development workstation for ADR-0021 §4b–§4c
([amendment 1](../adr/0021-temporal-upscaling-overlay.md#amendment-1-2026-11-02-n1b-the-fsr-316-port-as-built)),
and again after the PR #358 review (the tables below are the post-review run). This is stage 1
only: the port runs standalone in `fsr3-smoke`. It is not yet behind the `TemporalUpscaler`
trait (N1a), so there are no readability, golden or timing results here (gates 3, 4 and 6 are
stage 2).

- GPUs: lavapipe (`llvmpipe (LLVM 23.1.3, 256 bits)`, the software-Vulkan set in
  `agents.local.md`), NVIDIA GeForce RTX 5070 Ti (driver 615.78.08) and the AMD Radeon 610M
  iGPU (RADV `RAPHAEL_MENDOCINO`); all on the tiered path, `max_tier=overdrive`
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-5-cachyos
- Toolchain: rustc 1.99.1 stable, `slangc` v2026.20 (pinned), `spirv-val` from the pinned set
- Build: release, branch `feat/n1b-fsr3`
- Command: `MOBA_RENDER_PATH=tiered MOBA_VULKAN_VALIDATION=required target/release/fsr3-smoke`
  (validation and synchronization validation on), two processes per device; the RTX runs under
  the workstation's RTX lock
- Evidence: `~/.cache/moba-evidence/n1b-fsr3/review/` (`{lavapipe,rtx,radv}-{1,2}.stdout`,
  empty `.stderr` files); the fault-injection runs in `~/.cache/moba-evidence/n1b-fsr3/faults/`;
  the first submission's runs in `~/.cache/moba-evidence/n1b-fsr3/`

## What the smoke does

One FSR 3.1 context at 192×108 output with a 128×72 maximum render size (Quality) upscales a
synthetic scene whose ideal output is known:

- a flat colour;
- a smooth analytic field: static, moving one render pixel a frame with matching motion
  vectors, and with RCAS sharpening;
- a sweep of render sizes inside the same context without a reset (§4c), checking the first
  frame after each change as well as the converged one, then 9×5, 2×1 and 1×1, and back;
- a fine field near the render resolution's limit, still and moving.

Inputs carry sentinel values (colour 1000) outside the frame's render rectangle. Each
checkpoint bounds the mean and largest error over the interior (6-texel margin) and the
largest error over the whole image, so a read outside the rectangle would show.

It runs the way a renderer does: two frames in flight (each frame submitted without waiting),
and the output alternating between two images every two frames. Each process runs the
406-frame script twice. The first run fills every internal image with garbage (NaN, all ones)
before its first frame. The second starts from fresh allocations. Every output frame of the
two must be bit-identical.

## Results

Interior mean / max error per channel, and the largest error over the whole image:

| Checkpoint | lavapipe | RTX 5070 Ti | Radeon 610M | Bounds (mean, max; whole) |
|---|---|---|---|---|
| flat | 0.00014 / 0.00039; 0.0004 | 0.00006 / 0.00039; 0.0004 | 0.00011 / 0.00039; 0.0004 | 0.002, 0.004; 0.5 |
| static (Quality, 128×72) | 0.00560 / 0.02682; 0.057 | 0.00564 / 0.02731; 0.058 | 0.00575 / 0.02682; 0.059 | 0.012, 0.060; 0.5 |
| moving | 0.00426 / 0.02982; 0.047 | 0.00426 / 0.03006; 0.047 | 0.00457 / 0.03654; 0.056 | 0.012, 0.070; 0.5 |
| sharpened (RCAS) | 0.00655 / 0.03171; 0.059 | 0.00659 / 0.03219; 0.059 | 0.00670 / 0.03171; 0.060 | 0.015, 0.070; 0.5 |
| Balanced 112×63, first frame | 0.00568 / 0.02682; 0.059 | 0.00573 / 0.02731; 0.060 | 0.00584 / 0.02682; 0.061 | 0.025, 0.120; 0.5 |
| Balanced, converged | 0.00737 / 0.02731; 0.061 | 0.00744 / 0.02780; 0.061 | 0.00770 / 0.02829; 0.063 | 0.025, 0.100; 0.5 |
| Performance 96×54, first frame | 0.00741 / 0.02731; 0.061 | 0.00748 / 0.02731; 0.061 | 0.00774 / 0.02780; 0.063 | 0.025, 0.120; 0.5 |
| Performance, converged | 0.00899 / 0.03024; 0.067 | 0.00911 / 0.03268; 0.067 | 0.00961 / 0.03268; 0.067 | 0.025, 0.100; 0.5 |
| Ultra Performance 64×36, first frame | 0.00904 / 0.03171; 0.074 | 0.00916 / 0.03317; 0.074 | 0.00968 / 0.03512; 0.074 | 0.025, 0.120; 0.5 |
| Ultra Performance, converged | 0.01093 / 0.03410; 0.109 | 0.01104 / 0.03610; 0.109 | 0.01187 / 0.03946; 0.109 | 0.025, 0.100; 0.5 |
| odd 77×43, first frame | 0.01083 / 0.03459; 0.088 | 0.01093 / 0.03644; 0.087 | 0.01173 / 0.03995; 0.087 | 0.025, 0.120; 0.5 |
| odd 77×43, converged | 0.01180 / 0.05155; 0.081 | 0.01187 / 0.05130; 0.082 | 0.01279 / 0.05081; 0.084 | 0.025, 0.100; 0.5 |
| back to Quality, first frame | 0.01178 / 0.05277; 0.080 | 0.01184 / 0.05252; 0.082 | 0.01274 / 0.05203; 0.083 | 0.025, 0.120; 0.5 |
| back to Quality, converged | 0.01165 / 0.04422; 0.075 | 0.01169 / 0.04373; 0.076 | 0.01240 / 0.04373; 0.077 | 0.025, 0.100; 0.5 |
| tiny 9×5 | 0.147 / 0.466; 0.466 | 0.147 / 0.466; 0.466 | 0.147 / 0.466; 0.466 | finite; 1.5 |
| tiny 2×1 | 0.180 / 0.475; 0.475 | 0.180 / 0.475; 0.475 | 0.180 / 0.475; 0.475 | finite; 1.5 |
| tiny 1×1 | 0.167 / 0.433; 0.433 | 0.167 / 0.433; 0.433 | 0.167 / 0.432; 0.432 | finite; 1.5 |
| Quality after 1×1, converged | 0.01206 / 0.05263; 0.057 | 0.01206 / 0.05409; 0.058 | 0.01211 / 0.05263; 0.058 | 0.025, 0.100; 0.5 |
| fine, first frame after reset | 0.03921 / 0.12204; 0.149 | 0.03921 / 0.12204; 0.149 | 0.03921 / 0.12204; 0.149 | 0.050, —; 0.5 |
| fine, converged | 0.03111 / 0.09958; 0.268 | 0.03108 / 0.10007; 0.269 | 0.03110 / 0.09958; 0.268 | ≤ 0.9× first; 0.5 |
| fine moving, first frame after reset | 0.03921 / 0.12204; 0.149 | 0.03921 / 0.12204; 0.149 | 0.03921 / 0.12204; 0.149 | 0.050, —; 0.5 |
| fine moving, converged | 0.03305 / 0.14743; 0.187 | 0.03305 / 0.14694; 0.186 | 0.03468 / 0.16062; 0.227 | ≤ 0.95× first; 0.5 |

| | lavapipe | RTX 5070 Ti | Radeon 610M |
|---|---|---|---|
| Result | PASS, checked=94, mismatches=0 | PASS, checked=94, mismatches=0 | PASS, checked=94, mismatches=0 |
| Validation / sync validation messages | 0 errors, 0 warnings | 0 errors, 0 warnings | 0 errors, 0 warnings |
| Output digest (406 frames) | `330153dfef8bcc19` | `bd7496c74ea960fc` | `20596e75b1b323c9` |
| Poisoned run = fresh run, every frame | yes | yes | yes |
| Identical across the two processes | yes | yes | yes |
| Fine detail, converged ÷ first frame (still / moving) | 0.79× / 0.84× | 0.79× / 0.84× | 0.79× / 0.88× |
| Local contrast, sharpened ÷ unsharpened | 1.242× | 1.243× | 1.245× |
| `vram_bytes` (128×72 → 192×108 context) | 743 104 | 1 111 168 | 939 456 |

The digests differ between devices, as expected for floating-point compute on different
hardware. Each device is deterministic run to run. `vram_bytes` is the sum of the context's
allocations, which each driver rounds differently.

**§4c:** one context accepted every render size from 1×1 to its maximum, plus an odd 77×43,
from frame to frame without a reset. Each step stayed inside its bounds, with no validation
message. No sweep checkpoint has an error above 0.11 anywhere in the image; a leaked sentinel
would be 1000. A size one pixel over the maximum is refused (`accepts` returned false for
129×72 and true for 77×43).

## What the checks catch

Each fault below was injected alone into a debug build. Each ran once on the RTX, at the
review's head plus these checks: the tighter moving-fine bound (0.95×) came after, and every
ratio below is 1.0 or more.

| Fault | Result | What fails it |
|---|---|---|
| none | PASS | — |
| motion-vector scale (1, 1) instead of (1, 1) ÷ render size | FAIL | fine moving converged 1.00× its first frame |
| motion vectors ignored (scale 0) | FAIL | fine moving 1.66×; moving 0.029 mean, 0.17 max |
| jitter flipped on y only | FAIL | fine first frame 0.056 mean; moving; odd-size step |
| jitter flipped on x only | FAIL | fine first frame 0.071 mean; Performance max |
| RCAS not run at sharpness 0.25 | FAIL | contrast 1.00×; sharpened output equal to unsharpened |
| render size in the constants always the maximum | FAIL | whole-image errors up to 762 (sentinels) at every sweep size below the maximum |
| previous-frame render size stale (= the current one) | PASS | not caught, see below |
| new locks not cleared on the first frame (poisoned) | PASS | not observable here, see below |
| first frame's clears limited to the original port's (poisoned) | PASS | not observable here, see below |
| no first-frame clears and no first-frame reset (poisoned) | digest changes | NaN history reaches the output; unpoisoned, the two runs differ |

- **Stale previous render size.** The previous size feeds only the luma-instability,
  reactivity and shading-change paths. These scenes have no change in shading, so those masks
  stay near zero whatever the size. A size-step test on the fine field moved the first frame's
  error by under 0.001. Catching this needs a scene whose shading changes across a size step:
  stage 2's readability scenes, or a later smoke phase.
- **Uninitialised internal images.** With the first frame a reset, as it always is, the
  passes did not read the poisoned images in a way that reached the output, so the original
  port's narrower first-frame clears were not observably wrong in this scene. The port now
  initialises every image anyway (amendment 1). The poison-versus-fresh comparison is the
  regression check: the last row shows that garbage does reach the output once history is read
  without a clear.

## Memory at real sizes

`vram_bytes` (the context's own images and constant buffer, not the shared inputs) reported by
the smoke, with one context resized through these sizes:

| Output, mode (render) | lavapipe | RTX 5070 Ti | Radeon 610M |
|---|---|---|---|
| 1920×1080 Quality (1280×720) | 70.6 MiB | 75.4 MiB | 74.2 MiB |
| 2560×1440 Quality (1706×960) | 125.9 MiB | 134.3 MiB | 133.5 MiB |
| 2560×1440 NativeAA (2560×1440) | 207.7 MiB | 221.7 MiB | 216.9 MiB |
| 3840×2160 Performance (1920×1080) | 217.7 MiB | 225.0 MiB | 225.7 MiB |

ADR-0021 §7 estimates TAAU's additions at 1440p Quality at about 130 MB in all. That includes
targets FSR shares (the inputs, `hdr_out`, the overlay), so FSR 3.1's own 126–134 MiB comes on
top of the shared ones. Stage 2 must free TAAU's history while FSR 3.1 runs (amendment 1).

## Compat goldens

The branch's release `moba-client` was run against a release build of `origin/main`
(`34bc098`, in its own detached worktree), on lavapipe with validation required. The captures
were the default, network, ember, VAT, Ultra, Low-scaled and decals ones, with the
render-golden environments. Every capture is byte-identical between main and two branch runs,
and the branch runs logged no validation message. This held at the first submission and again
after the review fixes. Evidence: `~/.cache/moba-evidence/n1b-fsr3/golden/summary.txt` and
`~/.cache/moba-evidence/n1b-fsr3/golden-review/summary.txt`. The port is not wired into the
renderer yet, so no tiered golden changes either.

## Not measured here

- Timings: no GPU time against TAAU (gate 6), which needs the trait wiring of stage 2.
- Readability tests (gate 3) and goldens (gate 4): stage 2.
- Windows and macOS (MoltenVK): not run. The context refuses a device without quad subgroup
  operations in compute or the listed format features (`unavailable`). That refusal has not
  been exercised on such a device.
- Only the 32-bit permutation (`FFX_HALF=0`) is built, so there is no FP16 comparison. Stage 2
  times FP32 against FP16 on the 610M or cites AMD's deltas (amendment 1).
