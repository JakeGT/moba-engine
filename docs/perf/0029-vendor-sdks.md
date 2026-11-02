# Vendor SDK features: build, link and fallback (N0.6)

Captured 2026-11-02 on the development workstation for ADR-0020 §6 and gate 5
([amendment 6](../adr/0020-next-gen-foundations.md#amendment-6-2026-11-02-n06-vendor-sdk-features-in-n0)).

- GPU: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08 (tiered path, `max_tier=overdrive`)
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-5-cachyos, Wayland
- Toolchain: rustc 1.99.1 stable, libclang 23.1 (for `bindgen`); no CMake, no `dxc`
- Build: debug, branch `feat/n0-vendor-sdks` (based on `main` plus N0.1–N0.3 at `4278eab`)
- Evidence: `~/.cache/moba-evidence/n0-6-rtx/` (`fallback.stdout`, `default.stdout`,
  `default-frames.csv`)

## Fetch

`scripts/fetch-sdks.sh` fetched and verified all ten manifest entries. Every archive's SHA-256
and every licence file's SHA-256 matched the pins in `vendor-sdks.toml`; the two release assets
(Streamline, XeSS) also matched GitHub's published `digest`. A second run left every entry in
place ("present"), and a deliberately altered licence hash failed the fetch as designed.

## Build and link (Linux)

| Feature | bindgen | Built | Linked into `moba-client` | Note |
|---|---|---|---|---|
| `dlss` | yes (C) | yes | yes, `libnvsdk_ngx.a` + `stdc++` + `dl` | |
| `rtxdi` | yes (C++17) | yes | yes (no library) | host parameter structs only |
| `sharc` | — | yes | yes (no library) | header presence check |
| `fsr4` | yes (C, `libloading` struct) | yes | yes | run-time library is Windows-only |
| `xess` | yes (C, `libloading` struct) | yes | yes | run-time library is Windows-only |
| `streamline` | yes (C++17) | yes | yes | run-time library is Windows-only |
| `nrd` | — | **not built** | — | needs CMake 3.30+ and the Vulkan SDK's `dxc`, neither installed here; the build fails with a message saying so. First build is the `vendor-sdks` CI job |

`cargo clippy -p moba-render --all-targets --features dlss,rtxdi,sharc,fsr4,xess,streamline --
-D warnings` is clean, and the five selection tests pass with those features (the test binary
links NGX). With `MOBA_SDK_DIR` pointing at an empty directory, `--features sharc` fails with
the fetch command to run. Windows was not built (no Windows target or MSVC here).

## Fallback on the RTX

`cargo build -p moba-client --features dlss,rtxdi,sharc,fsr4,xess,streamline`, then
`MOBA_SERVER=local MOBA_SMOKE_FRAMES=240 target/debug/moba-client` under the RTX lock. No
run-time binary was placed beside the executable: there is no `libnvidia-ngx-dlss.so.*` on the
system (only the driver's NGX core, `libnvidia-ngx.so.615.78.08`), and the Windows-only SDKs
have no Linux library. The client started, ran its 240 frames and exited 0. The `[vendor]` lines,
one per unavailable back end, then the summary:

```text
[vendor] dlss unavailable: NGX found no DLSS 310.9.16 feature binary (libnvidia-ngx-dlss.so.310.9.16) beside the executable or in the driver (NVSDK_NGX_Result_FAIL_FeatureNotFound); upscaler falls back to builtin
[vendor] fsr4 unavailable: needs an AMD RDNA 2 or newer GPU, and the device is NVIDIA GeForce RTX 5070 Ti (vendor 0x10de); upscaler falls back to builtin
[vendor] xess unavailable: …/target/debug/libxess.so not found (XeSS 3.0.2 ships Windows binaries only); upscaler falls back to builtin
[vendor] streamline unavailable: …/target/debug/libsl.interposer.so not found (Streamline 2.14.16 ships Windows binaries only); frame_generation falls back to off
[vendor] selected: upscaler=builtin denoiser=builtin direct_lighting=rtxdi radiance_cache=sharc frame_generation=off latency=reflex compiled=dlss,fsr4,xess,rtxdi,sharc,streamline rtxdi=[RTXDI 3.1.0, ReSTIR DI parameters 272 bytes, reservoir block 16] sharc=[SHaRC 1.8.3]
```

- **DLSS:** NGX initialised on the renderer's device, answered the capability query with
  `FeatureNotFound`, and shut down again before the device was destroyed. NGX's data directory
  stayed empty: no over-the-air download was attempted or observed in this run.
- **RTXDI and SHaRC** have no run-time binary to remove; they are available because the device
  is on the tiered path with ray queries, so they are selected rather than logged.
- **Latency:** `VK_NV_low_latency2` is exposed, so `latency=reflex` with no feature.
- The same run's `[vram]` line: `locked_uploads=0 locked_pipelines=0`.
- **Validation:** this run did not record whether the Khronos validation layer was on, and its
  lines predate the GPU-safety review fixes (extensions declared before creation, the driver
  checks, the per-user NGX directory). The re-run below has both.

### RTX re-run with validation required (after the review fixes)

The same build at `20c550d`, then `MOBA_VULKAN_VALIDATION=required MOBA_SERVER=local
MOBA_SMOKE_FRAMES=240 target/debug/moba-client` under the RTX lock (driver 615.78.08, the
Khronos layer from `.tools/validation`), twice:

1. **As shipped** (no DLSS binary beside the executable): exit 0, **0 validation messages**.

   ```text
   [vendor] dlss unavailable: NGX found no DLSS 310.9.16 feature binary (libnvidia-ngx-dlss.so.310.9.16) beside the executable or in the driver (NVSDK_NGX_Result_FAIL_FeatureNotFound); upscaler falls back to builtin
   [vendor] fsr4 unavailable: needs an AMD GPU on AMD's driver (FSR 4 runs on RDNA 2 or newer), and the device is NVIDIA GeForce RTX 5070 Ti (vendor 0x10de, driver NVIDIA); upscaler falls back to builtin
   [vendor] xess unavailable: …/target/debug/libxess.so not found (XeSS 3.0.2 ships Windows binaries only); upscaler falls back to builtin
   [vendor] streamline unavailable: …/target/debug/libsl.interposer.so not found (Streamline 2.14.16 ships Windows binaries only); frame_generation falls back to off
   [vendor] selected: upscaler=builtin denoiser=builtin direct_lighting=rtxdi radiance_cache=sharc frame_generation=off latency=reflex compiled=dlss,fsr4,xess,rtxdi,sharc,streamline rtxdi=[RTXDI 3.1.0, ReSTIR DI parameters 272 bytes, reservoir block 16] sharc=[SHaRC 1.8.3]
   ```

2. **With the SDK's `lib/Linux_x86_64/rel/libnvidia-ngx-dlss.so.310.9.16` copied beside the
   executable** (removed afterwards): NGX found the feature, the probe's re-run of NGX's
   instance and device extension requirements found them all enabled, and DLSS was selected.
   Exit 0, **0 validation messages**. DLSS winning ends the upscaler search, so FSR 4 and XeSS
   are not probed and log nothing:

   ```text
   [vendor] streamline unavailable: …/target/debug/libsl.interposer.so not found (Streamline 2.14.16 ships Windows binaries only); frame_generation falls back to off
   [vendor] selected: upscaler=dlss denoiser=builtin direct_lighting=rtxdi radiance_cache=sharc frame_generation=off latency=reflex compiled=dlss,fsr4,xess,rtxdi,sharc,streamline dlss=[DLSS SDK 310.9.16] rtxdi=[RTXDI 3.1.0, ReSTIR DI parameters 272 bytes, reservoir block 16] sharc=[SHaRC 1.8.3]
   ```

   N0 creates no DLSS feature, so this checks admission and extension setup only; creating and
   evaluating the feature is N1.

### RTX re-run after the second review round (`8a61eda`)

The same two runs at `8a61eda` (the second round's fixes, including the vendor
device-extension checks and the create retry), with the same command, lock, driver and layer:

1. **As shipped:** exit 0, **0 validation messages**, no create-retry line. The `[vendor]`
   lines match run 1 above: DLSS, FSR 4, XeSS and Streamline fall back, and the summary is
   `upscaler=builtin … direct_lighting=rtxdi radiance_cache=sharc … latency=reflex`.
2. **With the DLSS feature binary beside the executable** (removed afterwards): exit 0,
   **0 validation messages**, no create-retry line, and `upscaler=dlss` with
   `dlss=[DLSS SDK 310.9.16]`, as in run 2 above.

The re-check edits after this round change XeSS's probe (it no longer requests or checks
extensions) and comments only; XeSS's library is absent on Linux, so neither run reaches that
code.

## Lavapipe run after the review fixes

`cargo build -p moba-client --features dlss,rtxdi,sharc,fsr4,xess,streamline`, then
`MOBA_VULKAN_VALIDATION=required MOBA_SERVER=local MOBA_SMOKE_FRAMES=60 target/debug/moba-client`
on Mesa lavapipe (LLVM 23.1.3, the bundled `.tools` ICD and validation layer), with the default
path (Compat, CPU device) and with `MOBA_RENDER_PATH=tiered`. Both exited 0 after 60 frames with
no validation message. The tiered run's lines:

```text
[vendor] dlss unavailable: DLSS needs an NVIDIA RTX GPU on NVIDIA's driver, and the device is llvmpipe (LLVM 23.1.3, 256 bits) (vendor 0x10005, driver llvmpipe); upscaler falls back to builtin
[vendor] fsr4 unavailable: needs an AMD GPU on AMD's driver (FSR 4 runs on RDNA 2 or newer), and the device is llvmpipe (LLVM 23.1.3, 256 bits) (vendor 0x10005, driver llvmpipe); upscaler falls back to builtin
[vendor] xess unavailable: …/target/debug/libxess.so not found (XeSS 3.0.2 ships Windows binaries only); upscaler falls back to builtin
[vendor] streamline unavailable: DLSS frame generation needs an NVIDIA RTX GPU on NVIDIA's driver, and the device is llvmpipe (LLVM 23.1.3, 256 bits) (vendor 0x10005, driver llvmpipe); frame_generation falls back to off
[vendor] selected: upscaler=builtin denoiser=builtin direct_lighting=rtxdi radiance_cache=sharc frame_generation=off latency=none compiled=dlss,fsr4,xess,rtxdi,sharc,streamline rtxdi=[RTXDI 3.1.0, ReSTIR DI parameters 272 bytes, reservoir block 16] sharc=[SHaRC 1.8.3]
```

On Compat, RTXDI and SHaRC log "the device runs the compat path, which has no ray-traced
lighting" instead, and the summary has `direct_lighting=builtin radiance_cache=none`. The
`dlss` build's instance also enabled NGX's one instance request,
`VK_KHR_get_physical_device_properties2`; no device extension was requested on lavapipe.

## Lavapipe run after the second review round

The fixes for the second review round (NRD's `ShaderMakeBlob` link and source copy, the
per-feature clippy fixes, XeSS rejecting until N1 merges its device features, FSR 4 tiered-only,
the vendor device-extension checks with the create retry, and NGX's data directory checked
against the effective user). Same build and environment as above:
`cargo build -p moba-client --features dlss,rtxdi,sharc,fsr4,xess,streamline`, then
`MOBA_VULKAN_VALIDATION=required MOBA_SERVER=local MOBA_SMOKE_FRAMES=60 target/debug/moba-client`
on lavapipe, once on the default path (Compat) and once with `MOBA_RENDER_PATH=tiered`. Both
exited 0 after 60 frames with **0 validation messages** and `locked_uploads=0
locked_pipelines=0`, one `[vendor]` line per probed back end, then the summary. Compat:

```text
[vendor] dlss unavailable: DLSS needs an NVIDIA RTX GPU on NVIDIA's driver, and the device is llvmpipe (LLVM 23.1.3, 256 bits) (vendor 0x10005, driver llvmpipe); upscaler falls back to builtin
[vendor] fsr4 unavailable: needs an AMD GPU on AMD's driver (FSR 4 runs on RDNA 2 or newer), and the device is llvmpipe (LLVM 23.1.3, 256 bits) (vendor 0x10005, driver llvmpipe); upscaler falls back to builtin
[vendor] xess unavailable: …/target/debug/libxess.so not found (XeSS 3.0.2 ships Windows binaries only); upscaler falls back to builtin
[vendor] rtxdi unavailable: the device runs the compat path, which has no ray-traced lighting; direct_lighting falls back to builtin
[vendor] sharc unavailable: the device runs the compat path, which has no ray-traced lighting; radiance_cache falls back to none
[vendor] streamline unavailable: DLSS frame generation needs an NVIDIA RTX GPU on NVIDIA's driver, and the device is llvmpipe (LLVM 23.1.3, 256 bits) (vendor 0x10005, driver llvmpipe); frame_generation falls back to off
[vendor] selected: upscaler=builtin denoiser=builtin direct_lighting=builtin radiance_cache=none frame_generation=off latency=none compiled=dlss,fsr4,xess,rtxdi,sharc,streamline
```

Tiered: the same `dlss`, `fsr4`, `xess` and `streamline` lines, then
`[vendor] selected: upscaler=builtin denoiser=builtin direct_lighting=rtxdi radiance_cache=sharc
frame_generation=off latency=none …`. No vendor device extension was requested on lavapipe
(NGX is not asked off NVIDIA's driver, and the XeSS library is absent), so the create retry
did not run. The default build (`cargo build -p moba-client`) on the tiered path logged no
`[vendor]` line. The RTX runs for this round are under "RTX re-run after the second review
round" above.

## Open build frame log

`env -u MOBA_SDK_DIR cargo build -p moba-client` (default features), then
`MOBA_SERVER=local MOBA_SMOKE_FRAMES=600 MOBA_VSYNC=0 MOBA_FRAME_LOG=default-frames.csv`. No
`[vendor]` line is logged. Summary (debug build, 1280×720, High on the tiered path):

| Frames | mean | p50 | p95 | p99 | GPU total p95 | locked_uploads | locked_pipelines |
|---|---|---|---|---|---|---|---|
| 540 | 4.248 ms | 4.190 ms | 4.605 ms | 5.276 ms | 0.370 ms | **0** | **0** |

## Not covered here

- **Windows run-time check** (`windows-vm`): blocked; the VM is set up by the maintainer.
- **NRD** build and link: CI only (see above).
- **DLSS feature creation and evaluation:** N1. DLSS *availability* was tested (re-run 2 above:
  with the SDK's feature binary beside the executable, NGX reported DLSS available and the
  probe selected it), but N0 creates no DLSS feature.
- **XeSS, FSR 4 and Streamline at run time:** their libraries are Windows-only, so on Linux each
  probe stops at the missing library. XeSS would reject even with its library until N1 merges
  its device features.
