#!/usr/bin/env bash
# One-shot local verification for coding agents.
#
# Usage:
#   scripts/agent-verify.sh          # fast gate: fmt, clippy, tests, docs, layering, hash, packs,
#                                    # asset provenance
#   scripts/agent-verify.sh --full   # adds release tests, feature matrix, fuzz/loom/bench compile
#
# The script never pushes, publishes, records, or edits files. Run it from the repository root.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

FULL=0
if [[ ${1:-} == "--full" ]]; then
  FULL=1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo is not on PATH" >&2
  exit 2
fi

section() { printf '\n==> %s\n' "$*"; }

section "docs consistency"
"$ROOT/scripts/check-docs.sh"

section "format"
cargo fmt --all --check

section "clippy"
cargo clippy --workspace --all-targets --locked -- -D warnings

section "tests"
cargo test --workspace --locked

section "rustdoc"
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked

section "layering"
for krate in moba-server moba-sim moba-game moba-nav moba-net; do
  bad=$(cargo tree -p "$krate" -e normal --prefix none --locked \
    | grep -E '^(moba-render|moba-platform|ash|winit|gpu-allocator) ' || true)
  if [[ -n "$bad" ]]; then
    echo "layering violation: $krate depends on:" >&2
    echo "$bad" | sort -u >&2
    exit 1
  fi
done

section "state hash"
actual=$(cargo run -q --release -p moba-tools --bin state-hash)
expected=$(cat tests/golden-state-hash.txt)
if [[ "$actual" != "$expected" ]]; then
  echo "state hash mismatch: expected $expected, got $actual" >&2
  exit 1
fi

section "content packs"
for pack in content/*.toml; do
  cargo run -q -p moba-tools --bin content-check -- "$pack" >/dev/null
  echo "ok $pack"
done

section "content provenance (A0 licence gate)"
cargo run -q -p moba-tools --bin content-check -- --provenance --check

section "fuzz crate (compile only)"
cargo check --manifest-path fuzz/Cargo.toml

if [[ "$FULL" == "1" ]]; then
  section "release tests"
  cargo test --workspace --release --locked

  section "feature matrix"
  cargo check -p moba-engine --no-default-features --locked
  cargo check -p moba-engine --no-default-features --features render --locked
  cargo check -p moba-engine --all-features --locked
  cargo check --manifest-path crates/tools/font-bake/Cargo.toml --locked

  section "mesh LOD baker (optional meshoptimizer)"
  cargo clippy -p moba-tools --features mesh-bake --bin mesh-bake --locked -- -D warnings
  cargo test -p moba-tools --features mesh-bake --bin mesh-bake --locked

  section "Loom model"
  RUSTFLAGS="--cfg loom" cargo test -p moba-jobs --lib loom -- --test-threads=1

  section "benches (compile only)"
  cargo bench --workspace --no-run

  section "deterministic match soak"
  cargo run --release -p moba-tools --bin match-soak -- --matches 2 --ticks 6000

  section "short wire fuzz"
  cargo run --release -p moba-tools --bin fuzz-wire -- --seconds 10

  section "multi-match load test"
  cargo run --release -p moba-tools --bin load-test -- --matches 4 --clients 10 --seconds 10

  section "multi-match flood isolation"
  cargo run --release -p moba-tools --bin load-test -- --matches 4 --clients 10 --seconds 10 \
    --workers 2 --flood-port 0 --flood-pps 50000
fi

printf '\nagent-verify: ok\n'
