# Quality gates. `just ci` must be green before any review agent runs.
set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

# Everything CI runs on all three OSes.
ci-core: fmt-check clippy test layering

# Full local gate (adds supply-chain checks).
ci: ci-core deny

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-targets --locked -- -D warnings

test:
    cargo nextest run --workspace --locked --no-tests=pass

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
