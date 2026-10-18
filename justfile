# Quality gates. `just ci` must be green before any review agent runs.
set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

# Everything CI runs on all three OSes.
ci-core: fmt-check clippy test layering doc features state-hash-check

# Full local gate (adds supply-chain checks).
ci: ci-core deny ci-tools

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-targets --locked -- -D warnings

test:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v cargo-nextest >/dev/null 2>&1; then
      cargo nextest run --workspace --locked --no-tests=pass
    else
      cargo test --workspace --locked
    fi

doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked

# Prove the umbrella crate works headless and with the optional renderer layer.
features:
    cargo check -p moba-engine --no-default-features --locked
    cargo check -p moba-engine --no-default-features --features render --locked
    cargo check -p moba-engine --all-features --locked

deny:
    cargo deny check

# UB check for the crates that own unsafe containers (needs nightly + miri).
# core/ecs run under Stacked Borrows with strict provenance. crossbeam-deque (used by the
# scheduler) casts integers to pointers internally and leaves deferred epoch bags behind,
# so the jobs crate runs under Tree Borrows with the external leak check disabled.
miri:
    MIRIFLAGS="-Zmiri-strict-provenance" cargo +nightly miri test -p moba-core -p moba-ecs
    MIRIFLAGS="-Zmiri-tree-borrows -Zmiri-ignore-leaks" cargo +nightly miri test -p moba-jobs

# The server and the simulation must never pull in rendering or windowing.
layering:
    #!/usr/bin/env bash
    set -euo pipefail
    fail=0
    for krate in moba-server moba-sim moba-game moba-nav moba-net; do
      bad=$(cargo tree -p "$krate" -e normal --prefix none --locked | grep -E '^(moba-render|moba-platform|ash|winit|gpu-allocator) ' || true)
      if [ -n "$bad" ]; then echo "layering violation: $krate depends on:"; echo "$bad" | sort -u; fail=1; fi
    done
    exit $fail

state-hash:
    cargo run -q --release -p moba-tools --bin state-hash

# Cross-platform deterministic hash; must match tests/golden-state-hash.txt.
state-hash-check:
    #!/usr/bin/env bash
    set -euo pipefail
    actual=$(cargo run -q --release -p moba-tools --bin state-hash)
    expected=$(cat tests/golden-state-hash.txt)
    if [ "$actual" != "$expected" ]; then
      echo "state hash mismatch: expected $expected, got $actual"
      exit 1
    fi

bench:
    cargo bench --workspace

# Save the current bench results as the baseline to compare PRs against.
bench-baseline:
    cargo bench --workspace -- --save-baseline main

bench-compare:
    cargo bench --workspace -- --baseline main

# Five-minute ten-client network soak (150 ms RTT, 5% loss, 30 ms jitter).
soak:
    cargo run --release -p moba-tools --bin soak -- --seconds 300

# Deterministic wire-decoder fuzzing (default one hour; pass seconds for CI).
fuzz-wire:
    cargo run --release -p moba-tools --bin fuzz-wire -- --seconds 3600

# Verify a deterministic replay file.
replay FILE:
    cargo run -p moba-tools --bin replay -- {{FILE}}

# Run the 50-match deterministic soak.
match-soak:
    cargo run --release -p moba-tools --bin match-soak

# Validate a content pack.
content-check FILE:
    cargo run -p moba-tools --bin content-check -- {{FILE}}

# Release-gate tooling: content validation, deterministic match soak, bench compilation.
ci-tools:
    cargo run --release -p moba-tools --bin content-check -- content/example-pack.toml
    cargo run --release -p moba-tools --bin content-check -- content/map-pack.toml
    cargo run --release -p moba-tools --bin match-soak -- --matches 10 --ticks 12000
    cargo run --release -p moba-tools --bin fuzz-wire -- --seconds 30
    cargo bench --workspace --no-run
