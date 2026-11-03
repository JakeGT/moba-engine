# AMD FidelityFX FSR 3.1.6 upscaler (vendored)

The GPU sources of the **FSR 3.1.6 upscaler** from the AMD FidelityFX SDK, copied
**unmodified** with their SDK-relative paths, so that `moba-shaders` (feature `fsr3`) compiles
them with the pinned `slangc` (ADR-0020 §6a and Q9, ADR-0021 §4b). Frame generation is not
vendored. How the port is built and wired is in [docs/dev/fsr3.md](../../docs/dev/fsr3.md).

- **Source:** FidelityFX SDK v2.4.0,
  <https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK>, tag `v2.4.0`, commit
  `7539ae8b6568ad29ed78a753404351daf3b5cca7`; archive
  `https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/archive/refs/tags/v2.4.0.zip`,
  SHA-256 `27120e3bbe0d0b30213825ad40f19077563f04f04572e252109a392d5fb522cb` (the `fidelityfx`
  entry of `vendor-sdks.toml`). The upscaler reports version 3.1.6
  (`FFX_FSR3UPSCALER_VERSION_*`).
- **Licence:** `Kits/FidelityFX/docs/license.md` (SHA-256
  `32357259b965d6a8001e9e508ba60eda6034027c5923a7d226ffbaff813c2bfb`), copied whole. Its
  default terms are binary-only, but it lists every file here under the **MIT** licence it
  quotes; `THIRD_PARTY.md` carries the notice.
- **Integrity:** `MANIFEST.sha256` lists every file. To check it against a fetched SDK:

  ```bash
  scripts/fetch-sdks.sh --only fidelityfx     # into target/vendor-sdks/fidelityfx-v2.4.0
  (cd target/vendor-sdks/fidelityfx-v2.4.0 && sha256sum -c ../../../third_party/fidelityfx/MANIFEST.sha256)
  ```

The files: the eight `ffx_fsr3upscaler_*_pass.hlsl` entry points
(`upscalers/fsr3/internal/shaders/`), the upscaler's `fsr3upscaler/` headers, the SPD and FSR 1
(RCAS) headers it includes (`upscalers/fsr3/include/gpu/{spd,fsr1}/`), and the FidelityFX core
GPU headers (`api/internal/gpu/`). Do not edit them: a change to the port goes in the build
flags, the Rust host code or an ADR-0021 amendment, and a new SDK version is a new copy with a
new manifest.
