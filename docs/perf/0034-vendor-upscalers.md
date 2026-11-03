# Vendor upscalers: DLSS SR on the RTX, FSR 4 and XeSS compiled (N1c)

Captured 2026-11-03 for ADR-0021 §3, verification gates 3, 5 and 6, and
[Amendment 5](../adr/0021-temporal-upscaling-overlay.md#amendment-5-2026-11-03-n1c-the-vendor-upscalers-as-built).

- GPU: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08 (tiered path); DLSS SDK 310.9.16 with
  `libnvidia-ngx-dlss.so.310.9.16` copied beside the executable (fetched by
  `scripts/fetch-sdks.sh --only dlss`, SHA-256 pinned in `vendor-sdks.toml`; not committed,
  not packaged). Software: lavapipe (Mesa llvmpipe)
- Display: one 2560×1440 monitor, KDE Plasma on Wayland
- CPU: AMD Ryzen 7 9800X3D; OS: CachyOS, Linux 7.3.0-5-cachyos; rustc 1.99.1
- Branch `feat/n1c-vendor-upscalers` on `761beb3`; "main" is a detached `origin/main` at
  `761beb3` built in its own worktree
- Validation: the bundled Khronos layer (`/home/jaket/Projects/MOBA/.tools/validation`) with
  `MOBA_VULKAN_VALIDATION=required`, which also turns on synchronization validation

## Summary

| Back end | Status |
|---|---|
| DLSS SR (`dlss`) | **runs on the RTX 5070 Ti under Linux (gate 5)**: validation clean in Native (DLAA), Quality, Balanced, Performance and Ultra Performance, with and without the sharpen pass, and across a window resize |
| FSR 4 (`fsr4`) | **compiled and unit-tested only**; blocked at run time on AMD's Windows driver and an RDNA card (`windows-vm` plus an AMD GPU). Not chosen by `auto` |
| XeSS (`xess`) | **compiled and unit-tested only**; its Vulkan runtime is Windows-only, so blocked on `windows-vm` |
| Fallback | one line, then TAAU, for each case below; 0 validation messages |

## DLSS: validation (gate 5)

Teamfight replay and the pre-match scene, 1920×1080 and 1280×720, once per mode
(`MOBA_UPSCALE_MODE=native|quality|balanced|performance|ultra-performance`), plus
`MOBA_SHARPNESS=0.5` (sharpen pass) and a window resize under Wayland and XWayland:
**0 validation messages** after the two barrier fixes in Amendment 5 item 7. Before them,
synchronization validation reported a write-after-write on the output (NGX's
`vkCmdClearColorImage`, which also needed `TRANSFER_DST` usage) and, in Performance mode only,
a write-after-write between frames on NGX's internal buffers (its `vkCmdFillBuffer`).

The start-up line on the RTX at 1280×720:

```text
[vendor] selected: upscaler=dlss denoiser=builtin direct_lighting=builtin radiance_cache=none frame_generation=off latency=reflex compiled=dlss dlss=[DLSS SDK 310.9.16]
[render] anti-aliasing: temporal, upscaler dlss (quality mode, render scale 0.500..0.667, back-end range 640x360..1280x720, sharpness 0.00, off)
```

DLSS's reported ranges: at 1280×720, Quality and Performance 640×360..1280×720, DLAA
1267×713..1280×720; at 1920×1080 Quality, 960×540..1920×1080.

## DLSS: jitter sign

Static pre-match scene, 1920×1080, frame 220, PSNR (dB) against a native FXAA render, per
region, with the jitter passed to NGX in each of the four sign combinations:

| Jitter passed | terrain | centre | tower |
|---|---|---|---|
| (+x, +y), as built | **40.70** | **38.91** | **38.15** |
| (+x, −y) | 35.02 | 34.08 | 34.89 |
| (−x, +y) | 36.61 | 34.95 | 35.74 |
| (−x, −y) | 34.53 | 33.44 | 34.59 |

Performance mode; Quality shows the same order (43.60 against 36.59 dB on terrain for
`(+,+)` against `(−,−)`). The sign is FSR's, so DLSS takes `FrameInputs::jitter` unchanged.

## Ghost probe on the RTX (gate 3)

`scripts/ghost-probe.sh` at 1280×720 on the RTX 5070 Ti; TAAU on the same GPU for comparison.
The bounds are N1a's (wake 3.0 %, hero 1.75 %, particle 7.5 %).

| Back end, mode | render | worst wake | hero smear | particle smear |
|---|---|---|---|---|
| DLSS Quality | 853×480 | 0.183 % | 0.40 % | 5.46 % |
| DLSS Performance | 640×360 | 0.166 % | 0.71 % | **8.44 %** |
| DLSS native (DLAA) | 1280×720 | 0.073 % | 0.12 % | 3.95 % |
| TAAU Quality | 853×480 | 1.202 % | 0.73 % | 27.55 % |
| TAAU Performance | 640×360 | 1.989 % | 1.33 % | 35.78 % |
| TAAU native | 1280×720 | 0.611 % | 0.00 % | 19.52 % |

DLSS ghosts far less than TAAU in every mode: the wake 7–12× less, the particle smear 4–5×
less. On the RTX TAAU's particle smear is over the 7.5 % bound in every mode; that bound was
set on lavapipe, where `vfx.frag` gates alpha differently (3.8–5.9 % there), and N1b's stage 2
re-derives it and changes the particle motion. DLSS Performance is 8.44 % against the current
bound; it is re-measured once N1b's stage 2 merges. Lavapipe's particle numbers are a tripwire
only.

## Overlay invariance (gate 3)

`VENDOR_UPSCALERS=dlss scripts/overlay-invariance.sh` on the RTX: the overlay ID target is
byte-identical under DLSS Quality, Performance and native, TAAU at each render scale, FXAA and
temporal off. Each vendor case checks the log names the back end it asked for, so a silent
fallback to TAAU fails the case.

## Fallbacks (gate 5)

| Case | Log |
|---|---|
| `auto` on the RTX, `dlss` build | `upscaler=dlss`, DLSS Quality |
| `MOBA_UPSCALER=auto`, DLSS binary removed | `[vendor] dlss unavailable: NGX found no DLSS 310.9.16 feature binary … (NVSDK_NGX_Result_FAIL_FeatureNotFound)`, then TAAU |
| `MOBA_UPSCALER=dlss`, DLSS binary removed | the same `[vendor]` line, plus `[render] upscaler dlss cannot run here (…); using taau` |
| `MOBA_UPSCALER=dlss` on lavapipe, `dlss` build | `DLSS needs an NVIDIA RTX GPU on NVIDIA's driver, and the device is llvmpipe …`, then TAAU |
| `MOBA_UPSCALER=dlss`, `fsr4` or `xess`, open build | `[render] upscaler X cannot run here (the \`X\` feature is not compiled in); using taau` |

Each ran with validation required and logged 0 messages.

## Cost (gate 6)

The teamfight replay with the frame log on at 1920×1080, release builds,
3 interleaved rounds, medians (range in brackets), render size 1260×709 in every tiered run:

| Path, tier, build | GPU total p50 / p95 (ms) | temporal p50 (ms) | render p50 / p95 (ms) | upscaler VRAM | VRAM peak |
|---|---|---|---|---|---|
| Low, main (TAAU) | 0.535 / 0.547 | 0.144 | 0.152 / 0.264 | 38.7 MB | 180.6 MB |
| Low, branch TAAU | 0.539 / 0.548 | 0.145 | 0.147 / 0.295 | 38.7 MB | 180.6 MB |
| Low, branch DLSS Quality | 0.936 / 0.946 | **0.567** | 0.215 / 0.369 | 85.7 MB | 139.4 MB |
| High, main (TAAU) | 0.588 / 0.598 | 0.144 | 0.183 / 0.315 | 38.7 MB | 242.4 MB |
| High, branch TAAU | 0.588 / 0.598 | 0.144 | 0.182 / 0.318 | 38.7 MB | 242.4 MB |
| High, branch DLSS Quality | 0.986 / 0.996 | **0.567** | 0.216 / 0.352 | 85.7 MB | 201.2 MB |
| Compat, main | 0.661 / 0.719 | — | 0.134 / 0.312 | 0 | 292.9 MB |
| Compat, branch | 0.658 / 0.670 | — | 0.133 / 0.233 | 0 | 292.9 MB |

- The branch's TAAU and Compat match main within the round-to-round spread.
- DLSS's evaluate costs 0.567 ms at 1260×709 → 1920×1080 on the 5070 Ti, about 0.42 ms more
  than TAAU. Upscaler VRAM is NGX's own figure (85.7 MB); NGX allocates it itself, so it is
  not in the renderer's VRAM peak, which drops by 41.2 MB (TAAU's 38.7 MB of history is gone).
- `locked_uploads=0` and `locked_pipelines=0` in every run.
- FSR 3.1 is not in the table: N1b's stage 2 had not merged.
- CPU p95 is noisy in these runs: an editor hook ran `cargo check` during some of them.

## Not verified

- FSR 4 and XeSS at run time (above), including their jitter signs and barriers.
- DLSS Performance's particle smear against the bound N1b's stage 2 sets.
- Found on `main` (`761beb3`), not caused by this change: a window resize under XWayland
  (and sometimes an X11 start) reports `vkUpdateDescriptorSets … Invalid VkImageView` on a
  combined image sampler with TAAU; DLSS's own recreate adds no messages.
