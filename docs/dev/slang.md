# Slang toolchain

New tiered passes are written in **Slang**; GLSL stays for every existing pass and for Compat
([ADR-0020](../adr/0020-next-gen-foundations.md) §4). `crates/shaders/build.rs` compiles the
Slang modules at build time with a **pinned, checksummed `slangc`**, so every build of the
renderer (client, tools, `cargo test --workspace`) needs it. A server-only build
(`-p moba-server`) does not build the shaders and needs no shader tools.

## One-time setup

```bash
scripts/fetch-slang.sh        # Linux, macOS (and Git Bash on Windows)
scripts/fetch-slang.ps1       # Windows PowerShell / pwsh
```

The script reads `crates/shaders/slang-version.txt`, downloads the host's archive from the
official [shader-slang release](https://github.com/shader-slang/slang/releases), checks its
SHA-256, and unpacks it into `target/tools/slang-<version>/` (or under `$CARGO_TARGET_DIR`).

- **Re-running is safe.** The script does nothing while that version is present, and it works
  offline once the archive or the toolchain is there.
- **Hosts.** It supports `linux-x86_64`, `linux-aarch64`, `windows-x86_64`, `macos-aarch64`
  and `macos-x86_64`.
- **After `cargo clean`.** `cargo clean` deletes `target/tools/`, including the downloaded
  archive, so run the script again afterwards. That run needs the network.

You also need **`spirv-val`** (SPIRV-Tools) on `PATH`. It is mandatory for Slang modules, and
a missing validator fails the build. For GLSL modules it stays optional.

| OS | Package |
|---|---|
| Debian/Ubuntu | `sudo apt install spirv-tools` |
| Arch | `sudo pacman -S spirv-tools` |
| macOS | `brew install spirv-tools` |
| Windows | the Vulkan SDK's `Bin` directory (already needed for `glslc`) |

## How the build finds `slangc`

1. `SLANGC=/path/to/slangc`.
2. `tools/slang-<version>/bin/slangc` in the Cargo target directory, which is where the fetch
   script puts it.
3. `slangc` on `PATH`, for example from the Vulkan SDK (which has shipped Slang since
   1.3.296.0).

`slangc -version` must report **exactly** the pinned version, and a mismatch fails the build.

- **Packagers** who build with a distribution `slangc` can set `MOBA_SLANG_ANY_VERSION=1`.
  The build then accepts any version and prints a warning.
- **Rebuilds.** Changing `SLANGC`, `MOBA_SLANG_ANY_VERSION` or `GLSLC` rebuilds the shaders.
- **No downloads.** `build.rs` never downloads anything, so offline and sandboxed builds work
  once the toolchain is present.

## Writing Slang modules

- **Layout.** Entry-point files are `crates/shaders/shaders/slang/*.slang`. Each file becomes
  one module, `<file>.slang.spv`, holding every `[shader("...")]` entry point under its source
  name. It is exposed in `moba_shaders::MODULES` as `<file>.slang`.
- **Shared code.** Shared modules live in `shaders/slang/lib/` and are `import`ed, never
  compiled on their own. `lib/frame_constants.slang` mirrors the GLSL camera block.
- **Flags.**
  - `-target spirv -profile spirv_1_6` (SPIR-V 1.6, Vulkan 1.3 core);
  - `-matrix-layout-column-major`;
  - `-O2`;
  - `-fvk-use-entrypoint-name`;
  - `-warnings-as-errors all`;
  - `-reflection-json`, written as `<file>.slang.json` next to the module.
- **Bindings.** Every resource declares an explicit `[[vk::binding(binding, set)]]` on the
  line before it. `slangc` has no flag that rejects implicit bindings, so a `moba-shaders` test
  checks the sources instead.
- **Layout check.** A structure shared with Rust has its layout compared with the
  `#[repr(C)]` mirror by a unit test against the reflection JSON. For example,
  `moba-render`'s `slang_frame_constants_match_camera_uniform` test checks
  `FrameConstants` against `CameraUniform`.
- **Permutations.** Permutations use the same `PERMUTATIONS` table as GLSL, with the source
  `slang/<file>.slang` and the output `<name>.slang`.

## Bumping the pin

1. Pick a release, then record every host archive's SHA-256 in
   `crates/shaders/slang-version.txt`.
2. **Cross-check** those values against the release API. Upstream publishes no checksum
   files, so this check is the evidence:
   `gh api repos/shader-slang/slang/releases/tags/v<version> --jq '.assets[] | "\(.digest) \(.name)"'`.
3. Re-run `scripts/fetch-slang.sh`, then `scripts/agent-verify.sh --full`. CI's cache is keyed
   on the version file, so CI fetches the new compiler automatically.

Vendor HLSL (RTXDI) also goes through `slangc`; see the
[RTXDI spike](rtxdi-slang-spike.md) for what compiles and what flags it needs.
