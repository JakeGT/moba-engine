# Local agent environment (template)

Copy to `agents.local.md` (gitignored) and fill in machine-specific details. Do not commit
secrets or absolute paths tied to your machine.

## Toolchain

```bash
rustc --version   # Rust 1.85+
cargo --version
```

## Software Vulkan (optional, Linux)

Set these only when using the bundled lavapipe/validation artifacts:

```bash
export VK_ICD_FILENAMES="$PWD/.tools/swrast/usr/share/vulkan/icd.d/lvp_icd.json"
export VK_LAYER_PATH="$PWD/.tools/validation/usr/share/vulkan/explicit_layer.d"
export LD_LIBRARY_PATH="$PWD/.tools/swrast/usr/lib:$PWD/.tools/validation/usr/lib:$PWD/.tools/llvm/usr/lib:$LD_LIBRARY_PATH"
export MOBA_VULKAN_VALIDATION=required   # or off
```

## Local gates

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Record which optional tools are available (`just`, `cargo-nextest`, `cargo-deny`, Miri) and
how you ran the equivalent commands.

## Capture

```bash
MOBA_SERVER=local MOBA_SMOKE_FRAMES=240 \
  MOBA_CAPTURE=/tmp/frame.ppm MOBA_CAPTURE_FRAME=220 \
  cargo run -p moba-client
```
