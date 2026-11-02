# Slang toolchain

New tiered passes are written in **Slang**; GLSL stays for every existing pass and for Compat
([ADR-0020](../adr/0020-next-gen-foundations.md) §4). `crates/shaders/build.rs` compiles the
Slang modules at build time with a **pinned, checksummed `slangc`**, so every build of the
renderer (client, tools, `cargo test --workspace`) needs it. A server-only build
(`-p moba-server`) does not build the shaders and needs no shader tools.

## One-time setup

```bash
scripts/fetch-slang.sh        # Linux, macOS
scripts/fetch-slang.ps1       # Windows (PowerShell / pwsh)
```

On Windows use the `.ps1`: the Windows archive is a `.zip`, which Git Bash's GNU `tar` cannot
read, so `fetch-slang.sh` there needs `unzip` on `PATH`.

The script reads `crates/shaders/slang-version.txt`, downloads the host's archive from the
official [shader-slang release](https://github.com/shader-slang/slang/releases), checks its
SHA-256, and unpacks it into `target/tools/slang-<version>/` (or under `$CARGO_TARGET_DIR`).

- **Relocated target directory.** The scripts read only the `CARGO_TARGET_DIR` variable. If
  you set `build.target-dir` in a Cargo config, run the script with `CARGO_TARGET_DIR` set to
  the same directory, or point `SLANGC` at the compiler.

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
   script puts it. `build.rs` walks up from its `OUT_DIR` and stops at the target directory:
   the outermost ancestor with a `CACHEDIR.TAG` (written by Cargo and the fetch scripts) at
   most five levels up, so `cargo build --target <triple>` finds it too. A `build.target-dir`
   therefore works on the build side. With Cargo's `build.build-dir`, `OUT_DIR` is not under
   the target directory: set `SLANGC`.
3. `slangc` on `PATH`, for example from the Vulkan SDK (which has shipped Slang since
   1.3.296.0).

`slangc -version` must report **exactly** the pinned version, and a mismatch fails the build.

- **Packagers** who build with a distribution `slangc` can set `MOBA_SLANG_ANY_VERSION=1`.
  The build then accepts any version and prints a warning.
- **Broken compilers.** A `slangc` whose `-version` exits non-zero fails the build, even with
  `MOBA_SLANG_ANY_VERSION=1`.
- **Rebuilds.** Changing `SLANGC`, `MOBA_SLANG_ANY_VERSION` or `GLSLC` rebuilds the shaders,
  and so does replacing the resolved `slangc` binary.
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
- **Bindings.** Every resource declares an explicit `[[vk::binding(binding, set)]]`.
  `slangc` has no flag that rejects implicit bindings, so `moba-shaders`'
  `slang_descriptor_bindings_match_the_table` test checks the reflected layout of every
  module against its `SLANG_BINDINGS` table, an exact `(name, set, binding)` list. The check
  covers globals, `lib/` imports and entry-point parameters. A new module, resource or
  binding means editing that table and the Rust descriptor layout that mirrors it. The test
  also rejects two things that would get implicit bindings:
  - loose `uniform` globals, which Slang gathers into an implicit globals uniform buffer;
  - uniform entry-point parameters.
- **`ParameterBlock<T>`** takes a whole descriptor set: `[[vk::binding(0, set)]]` picks the
  set. Inside it, Slang lays the bindings out in field order:
  - an implicit uniform buffer at binding 0 when `T` has ordinary (non-resource) data;
  - then one binding per resource field, in declaration order (an array takes one binding).

  The Rust set layout must follow that order. The table records a block as `Slot::Block`.
- **Layout check.** A structure shared with Rust has its layout compared with the
  `#[repr(C)]` mirror by a unit test against the reflection JSON. The test checks each
  field's name, order, scalar type, shape (rows × columns, array length), offset and size.
  For example, `moba-render`'s `slang_frame_constants_match_camera_uniform` test checks
  `FrameConstants` against `CameraUniform`. The JSON comes from
  `moba_shaders::reflection(name)`. Nothing at run time calls it, so the linker drops the
  JSON from shipped binaries.
- **Matrices: column-major, `mul(M, v)`.** With `-matrix-layout-column-major`, a Slang
  `float4x4` has the same memory layout as a GLSL `mat4` and glam's `to_cols_array`. `M[r][c]`
  is row `r`, column `c` of the mathematical matrix, so `mul(M, v)` in Slang is `M * v` in
  Rust and GLSL. Write `mul(view_proj, position)`, never `mul(position, view_proj)`.
  - **In the SPIR-V** this shows up as a `RowMajor` decoration. Slang treats a `float4x4` as
    four HLSL rows and emits it as the transposed SPIR-V type. Don't "fix" that.
  - **`slang_matrices_are_column_major`** (a `moba-shaders` test) checks the decorations.
  - **`vulkan-probe --slang-smoke`** checks the product on a GPU. It dispatches
    `toolchain_smoke.slang` once and compares `mul(view_proj, camera_pos)` with glam. CI runs
    it on lavapipe (render-golden) and on MoltenVK (the macOS probe), with validation
    required.
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

Vendor HLSL also goes through `slangc`: see the [RTXDI spike](rtxdi-slang-spike.md) for
RTXDI, and [fsr3.md](fsr3.md) for the FSR 3.1.6 passes, which `crates/shaders/build.rs`
compiles with `-lang slang` (the HLSL front end's legacy `for` scoping rejects two of them),
the quad-subgroup capability, register shifts for explicit bindings, and two warnings off.
