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
- **DLSS available:** not tested; it needs the DLSS feature binary beside the executable, which
  belongs to N1.
