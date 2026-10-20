# Local agent environment (template)

Copy to `agents.local.md` (gitignored) and fill in machine-specific details. Do not commit
secrets or absolute paths tied to your machine.

## Toolchain

```bash
rustc --version   # Rust 1.88+
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
scripts/agent-verify.sh          # fast portable gate
scripts/agent-verify.sh --full   # + release/features/Loom/soak/fuzz
# explicit equivalents:
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
```

Record which optional tools are available (`just`, `cargo-nextest`, `cargo-deny`, Miri) and
how you ran the equivalent commands.

## Capture / frame log

```bash
MOBA_SERVER=local MOBA_SMOKE_FRAMES=240 \
  MOBA_CAPTURE=/tmp/frame.ppm MOBA_CAPTURE_FRAME=220 \
  cargo run -p moba-client

# Real-GPU timing (vsync off; writes p50/p95/p99/max on exit):
MOBA_VSYNC=0 MOBA_WARMUP_TICKS=3000 MOBA_SMOKE_FRAMES=900 \
  MOBA_FRAME_LOG=/tmp/frames.log cargo run -p moba-client
```

Golden rebaselines must use the CI-produced 1280×720 capture, not a differently sized local
window. Fuzz/Loom commands and optional tools are listed in `AGENTS.md`/`docs/agents.md`.
