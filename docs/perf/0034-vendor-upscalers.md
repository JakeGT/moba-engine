# Vendor upscalers: DLSS SR on the RTX, FSR 4 and XeSS compiled (N1c)

Captured 2026-11-03 for ADR-0021 §4, verification gates 3, 5 and 6, and
[Amendment 5](../adr/0021-temporal-upscaling-overlay.md#amendment-5-2026-11-03-n1c-the-vendor-upscalers-as-built).

- GPU: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08 (tiered path); DLSS SDK 310.9.16 with
  `libnvidia-ngx-dlss.so.310.9.16` copied beside the executable (fetched by
  `scripts/fetch-sdks.sh --only dlss`, SHA-256 pinned in `vendor-sdks.toml`; not committed,
  not packaged). Software: lavapipe (Mesa llvmpipe)
- Display: one 2560×1440 monitor, KDE Plasma on Wayland
- CPU: AMD Ryzen 7 9800X3D; OS: CachyOS, Linux 7.3.0-5-cachyos; rustc 1.99.1
- Branch `feat/n1c-vendor-upscalers` on `761beb3`; the cost table and the resize runs are on
  the review-round-1 code (`42c810f`, the PR's code from then on: later commits change docs
  only unless this file says otherwise). "main" is a detached `origin/main` at `761beb3`
  built in its own worktree
- Validation: the bundled Khronos layer (`/home/jaket/Projects/MOBA/.tools/validation`) with
  `MOBA_VULKAN_VALIDATION=required`, which also turns on synchronization validation

## Summary

| Back end | Status |
|---|---|
| DLSS SR (`dlss`) | **runs on the RTX 5070 Ti under Linux (gate 5)**: validation clean in Native (DLAA), Quality, Balanced, Performance and Ultra Performance, with and without the sharpen pass, and across window resizes under X11 and Wayland ([below](#resizes)). Gate 3 at Performance is **pending** ([below](#ghost-probe-on-the-rtx-gate-3)); the `windows-vm` run is **pending** |
| FSR 4 (`fsr4`) | **compiled and unit-tested only**; blocked at run time on AMD's Windows driver and an RDNA card (`windows-vm` plus an AMD GPU). Not chosen by `auto` |
| XeSS (`xess`) | **compiled and unit-tested only**; its Vulkan runtime is Windows-only, so blocked on `windows-vm` |
| Fallback | one line, then TAAU, for each case [below](#fallbacks-gate-5); 0 validation messages |

## DLSS: validation (gate 5)

The teamfight replay and the pre-match scene, 1920×1080 and 1280×720, once per mode
(`MOBA_UPSCALE_MODE=native|quality|balanced|performance|ultra-performance`), plus
`MOBA_SHARPNESS=0.5` (the sharpen pass): **0 validation messages**, re-checked on the
review-round-1 code. Without the barriers of Amendment 5 item 8, synchronization validation
reports, in 60 frames at 1280×720:

| Barriers removed | Mode | Messages | Hazard | Issue |
|---|---|---|---|---|
| before and after the evaluate (the output's) | native; Quality with the sharpen pass | 10 | `vkCmdClearColorImage[nv.ngx.dlss.Evaluate]` write-after-write against the graph's layout transitions | #367 |
| the global one before the evaluate | Performance | 10 | `vkCmdFillBuffer[nv.ngx.dlss.Evaluate]` write-after-write across two frames' submissions | #366 |

(Quality without the sharpen pass reports none of the first kind on this driver.) The clear
also needs `TRANSFER_DST` on the output, which a `dlss` build sets.

The start-up lines on the RTX at 1280×720:

```text
[render] anti-aliasing: temporal, upscaler dlss (quality mode, render scale 0.500..0.667, back-end range 640x360..1280x720, sharpness 0.00, off)
[vendor] selected: upscaler=dlss denoiser=builtin direct_lighting=builtin radiance_cache=none frame_generation=off latency=reflex compiled=dlss dlss=[DLSS SDK 310.9.16]
```

DLSS's reported ranges: at 1280×720, Quality and Performance 640×360..1280×720, DLAA
1267×713..1280×720; at 1920×1080, Quality, Balanced and Performance 960×540..1920×1080,
DLAA 1901×1069..1920×1080 and Ultra Performance 640×360 only (so dynamic resolution is
pinned there). The last per-mode pass was on the final code: the teamfight replay at
1920×1080, 300 frames per mode, each mode exiting 0 with 0 validation messages and no
evaluation failure (`~/.cache/moba-evidence/n1c/r2/modes-*.log`).

**DLAA at a low render scale** (`MOBA_UPSCALE_MODE=native MOBA_RENDER_SCALE=0.6`, 1280×720):
before review round 1 the render size was 768×432, outside DLSS's range, and every evaluate
failed (`InvalidParameter`, the output never written, validation silent). Now the
allocation is raised and the frame renders at DLSS's minimum:

```text
[render] render allocation 768x432 raised to 1267x713: dlss's smallest render size for native mode at 1280x720
[render] anti-aliasing: temporal, upscaler dlss (native mode, render scale 0.990..0.990, back-end range 1267x713..1280x720, sharpness 0.00, off)
```

with no evaluation failure and 0 validation messages over 240 frames.

## Resizes

`scripts/resize-smoke.sh` (new; `MOBA_SMOKE_RESIZE` makes the client request window sizes
mid-run) under X11 (XWayland), validation `required`, AO on, sizes 800×450 → 480×270 →
640×360 from a 640×360 start; and longer replay runs at 1280×720 → 1600×900 → 960×540 →
1280×720:

| Run | GPU | Swapchain recreations | Validation messages |
|---|---|---|---|
| tiered TAAU, tiered FXAA, Compat | RTX 5070 Ti | every size reached | **0** |
| tiered DLSS Quality, and with the sharpen pass | RTX 5070 Ti | every size reached | **0** |
| tiered DLSS Performance, resized to 8×8 (the window manager makes it 8×28) | RTX 5070 Ti | 8×28, then 640×360 | **0**; DLSS falls back to TAAU with one line and the run continues |
| tiered TAAU, tiered FXAA, Compat | lavapipe | every size reached | **0** |
| tiered DLSS Performance, 8×8 then back to 1280×720, Wayland | RTX 5070 Ti | as asked | **0**, same fallback |

The fallback line:

```text
[render] upscaler dlss cannot run at 8x28 (the temporal upscaler failed: creating the DLSS feature (performance mode, 4x14 to 8x28) failed (NVSDK_NGX_Result_InvalidParameter, 0xbad00005)); using taau
```

Before the fix, that resize ended the client with a misleading `no suitable GPU found`.

**The resize validation error was a bug on main** (`761beb3`), not DLSS's: lighting rewrote its
descriptor sets with the destroyed AO view before re-binding it, which gave
`vkUpdateDescriptorSets … pDescriptorWrites[9] … Invalid VkImageView` after every resize
(and at some X11 starts) with AO on, under any AA mode. The earlier "validation clean across a
window resize" claim in this file was wrong: those resize logs had 4–10 of these errors. With
the old two-write binding restored for one run, `resize-smoke.sh`'s TAAU case reports 11
errors; with the fix, 0.

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
The bounds are N1a's (wake 3.0 %, hero 1.75 %, particle 7.5 %). Re-run on the final code
(`42c810f`) with the same `dlss` build for both back ends: every number below repeated
exactly, with 0 validation messages (`~/.cache/moba-evidence/n1c/ghost-final/`).

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
set on lavapipe, where `vfx.frag`'s alpha-gated motion write is ignored (TAAU's particles
smear 1.4–5.9 % there: 3.77 / 5.88 / 1.40 % at Quality / Performance / native on this
branch). Lavapipe's particle numbers are a tripwire only.

**Gate 3 is pending for DLSS Performance: particle smear 8.44 % against the 7.5 % bound.**
DLSS has no reactive mask, so its particles rest entirely on their motion vectors, which
N1b's stage 2 (#364) fixes on real GPUs while re-deriving the bounds. After it merges, this
PR merges main and re-runs the probe on the RTX for DLSS in all three modes (Amendment 5,
item 16).

## Overlay invariance (gate 3)

`VENDOR_UPSCALERS=dlss scripts/overlay-invariance.sh` on the RTX: the overlay ID target is
byte-identical under DLSS Quality, Performance and native, TAAU at each render scale, FXAA and
temporal off. Each vendor case checks the log names the back end it asked for, so a silent
fallback to TAAU fails the case.

## Fallbacks (gate 5)

| Build, case | Log |
|---|---|
| `dlss`, `auto` on the RTX | `upscaler=dlss`, DLSS Quality |
| `dlss`, `MOBA_UPSCALER=auto`, DLSS binary removed | `[vendor] dlss unavailable: NGX found no DLSS 310.9.16 feature binary … (NVSDK_NGX_Result_FAIL_FeatureNotFound)`, then TAAU |
| `dlss`, `MOBA_UPSCALER=dlss`, DLSS binary removed | the same `[vendor]` line, plus `[render] upscaler dlss cannot run here (…); using taau` |
| `dlss`, `MOBA_UPSCALER=dlss` on lavapipe | `DLSS needs an NVIDIA RTX GPU on NVIDIA's driver, and the device is llvmpipe …`, then TAAU |
| `fsr4,xess` on the RTX, `MOBA_UPSCALER=fsr4` | `[vendor] fsr4 unavailable: needs an AMD GPU on AMD's driver …`, `[vendor] xess unavailable: …/libxess.so not found (XeSS 3.0.2 ships Windows binaries only)`, `[render] upscaler fsr4 cannot run here (…); using taau`, `[vendor] selected: upscaler=builtin …` |
| `fsr4,xess` on the RTX, `MOBA_UPSCALER=xess` | the same two `[vendor]` lines, `[render] upscaler xess cannot run here (…libxess.so not found…); using taau` |
| `fsr4,xess` on the RTX, `auto` | the two `[vendor]` lines, then TAAU |
| open build, `MOBA_UPSCALER=dlss`, `fsr4` or `xess` | `[render] upscaler X cannot run here (the \`X\` feature is not compiled in); using taau` |

Each ran with validation required and logged 0 messages. The `[vendor] selected:` summary now
names the upscaler that runs (`builtin` above), and is logged once the upscaler exists.

## Cost (gate 6)

The teamfight replay with the frame log on, 1920×1080 window (render 1260×709 at Quality in
every tiered run), release builds of `42c810f`, 3 interleaved rounds, medians (range in
brackets). "open" is the branch built **without** vendor features (the build that ships);
"dlss" is a `dlss` build. Nothing else was building or checking during the runs, apart from
the last three (round 3's sharpen and Compat runs), during which another agent's build raised
the load to about 4.

| Path, tier, build | GPU total p50 / p95 (ms) | temporal p50 (ms) | render p50 / p95 (ms) | upscaler VRAM | VRAM peak |
|---|---|---|---|---|---|
| Low, main (TAAU) | 0.502 (0.501–0.535) / 0.511 | 0.130 (0.130–0.145) | 0.182 / 0.243 | 38.7 MB | 180.6 MB |
| Low, open (TAAU) | 0.502 (0.501–0.538) / 0.510 | 0.130 (0.130–0.145) | 0.183 / 0.251 | 38.7 MB | 180.6 MB |
| Low, dlss, DLSS Quality | 0.941 (0.934–0.941) / 0.949 | **0.567** (0.565–0.567) | 0.216 / 0.266 | 85.7 MB | 225.1 MB |
| Low, dlss, DLSS Quality, sharpness 0.5 | 0.967 (0.966–0.974) / 0.979 | **0.600** (0.600–0.604) | 0.219 / 0.412 | 101.6 MB | 242.0 MB |
| High, main (TAAU) | 0.551 (0.550–0.585) / 0.592 | 0.130 (0.130–0.144) | 0.183 / 0.253 | 38.7 MB | 242.4 MB |
| High, open (TAAU) | 0.585 (0.552–0.586) / 0.595 | 0.144 (0.130–0.144) | 0.184 / 0.339 | 38.7 MB | 242.4 MB |
| High, dlss, DLSS Quality | 0.986 (0.985–0.988) / 0.996 | **0.567** (0.565–0.567) | 0.217 / 0.333 | 85.7 MB | 286.9 MB |
| Compat, main | 0.661 (0.659–0.662) / 0.720 | — | 0.168 / 0.314 | 0 | 292.9 MB |
| Compat, open | 0.660 (0.659–0.662) / 0.717 | — | 0.167 / 0.274 | 0 | 292.9 MB |

- **The open build matches main.** TAAU's timings are bimodal on this GPU (GPU total about
  0.50 or 0.535 ms on Low, 0.551 or 0.585 ms on High, from run to run in either build:
  rounds main-high 0.585/0.550/0.551, open-high 0.585/0.552/0.586). The High medians differ only
  in which level two of the three rounds landed on. Compat is identical.
- **DLSS costs about +0.43 ms of GPU** at 1080p Quality on the 5070 Ti: its evaluate is
  0.567 ms against TAAU's 0.130–0.144 ms, the rest of the frame unchanged.
- **The sharpen pass costs 0.033 ms** (temporal 0.600 against 0.567 ms) and a 15.9 MB
  output-resolution intermediate (upscaler VRAM 101.6 against 85.7 MB).
- **VRAM:** NGX's 85.7 MB now counts in the peak (`sdk_mb=` in the `[vram]` line,
  Amendment 5 item 13): DLSS's peak is **44.5 MB above TAAU's** (225.1 against 180.6 MB on
  Low, 286.9 against 242.4 MB on High), NGX's memory less TAAU's 38.7 MB of history and the
  other targets DLSS does not need. (The first draft of this file, without that accounting,
  showed DLSS 41 MB *below* TAAU.)
- `locked_uploads=0` and `locked_pipelines=0` in every run.
- **Not measured:** DLSS on the Low and Medium reference classes (RTX 2060, RTX 3060). Gate 6
  says estimates never close a target, so DLSS's cost there stays open. FSR 3.1 is not in the
  table: N1b's stage 2 had not merged.

## Not verified

- FSR 4 and XeSS at run time (above), including their jitter signs and barriers.
- DLSS in `windows-vm`.
- DLSS Performance's particle smear against the bound N1b's stage 2 sets (gate 3, pending).
- DLSS's cost on the Low and Medium reference cards.
