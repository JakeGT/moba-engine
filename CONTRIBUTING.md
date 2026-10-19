# Contributing

Thanks for helping build a reusable MOBA engine. The project is pre-1.0 and evolving fast;
small, focused pull requests with tests and docs are the easiest to review.

By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md). Contributions are
dual-licensed under MIT OR Apache-2.0, matching the project.

## Setup

- Rust **1.85+** (edition 2024). `rust-toolchain.toml` requests the floating `stable` channel;
  CI additionally checks the 1.85 MSRV job. Use `scripts/agent-verify.sh` for a portable gate
  if `just`/`cargo-nextest` are unavailable.
- A Vulkan **1.2+** driver for the client; `glslc` (Vulkan SDK) or `glslangValidator` to
  compile shaders. The dedicated server and tests need no GPU.
- Optional: `just` and `cargo-nextest`, which the `justfile` prefers. All gates also work
  with plain `cargo`.
- Linux window build deps (when not using the headless tests): X11 or Wayland development
  headers, `libxkbcommon`.

```bash
git clone <this repository>
cd engine
cargo build --workspace
cargo run -p moba-server -- --ticks 1000
```

## Workflow

1. Open an issue or describe the problem in a draft PR. For non-trivial design, add a short
   spec under `docs/tasks/` and link it.
2. Branch from `main` (`feat/...`, `fix/...`).
3. Write a failing test or benchmark where practical, then implement.
4. Run the gates below and include the results in the PR.
5. Keep the PR scoped. Mention protocol/content/determinism changes explicitly.
6. Update docs and `CHANGELOG.md` when behavior or public API changes.

## Gates

```bash
scripts/check-docs.sh     # links and contract/test-count consistency
scripts/agent-verify.sh   # fmt, clippy, tests, docs, layering, state hash, content, fuzz check
# or explicitly:
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
just layering          # or scripts/agent-verify.sh
just state-hash-check  # fixed simulation hash must match tests/golden-state-hash.txt
```

Before merging hot-path or netcode changes also run a short soak or benchmark:

```bash
cargo run --release -p moba-tools --bin soak -- --seconds 30
cargo run --release -p moba-tools --bin fuzz-wire -- --seconds 30

# Sanitizer fuzzing (nightly + cargo-fuzz):
cargo install cargo-fuzz --locked
cargo +nightly fuzz run wire -- -max_total_time=60
cargo +nightly fuzz run replay -- -max_total_time=60
cargo +nightly fuzz run content -- -max_total_time=60
# Crashes land in fuzz/artifacts/<target>/; reproduce with
# `cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<file>`.
cargo bench -p moba-sim --bench tick
```

## Architecture rules

- **Layering:** `moba-sim`, `moba-game`, `moba-nav`, `moba-net` and `moba-server` must not
  depend on rendering, windowing, `ash` or `winit`. CI enforces this.
- **Determinism:** simulation code uses fixed-point or integer math, seeded RNG, no clocks,
  no `HashMap` iteration. Tie-break by entity id. Update hash tests deliberately.
- **Authority:** clients send intent only; the server validates. Do not trust client state.
- **Allocation:** per-tick/per-frame hot paths should not allocate after warm-up. Use the
  provided scratch buffers, arenas and chunk queries.
- **Unsafe:** only `moba-core`, `moba-ecs`, `moba-jobs`, `moba-platform` and `moba-render`;
  one operation per block and a `// SAFETY:` proof. Other crates forbid unsafe.
- **Protocol:** bump `PROTOCOL_VERSION` on wire changes; keep decoders bounds-checked and add
  malformed-input tests.
- **Public API:** document new public items; follow the stability tiers in
  [docs/API-STABILITY.md](docs/API-STABILITY.md).

## Commit and PR style

Conventional commits are preferred (`feat:`, `fix:`, `perf:`, `docs:`, `refactor:`, `test:`).
PRs should state:

- what changed and why;
- tests/benchmarks run;
- determinism/protocol/content impact;
- screenshots for visual changes.

The PR template mirrors this checklist.

## Reviews

Expect review for correctness, determinism, allocation costs, unsafe proofs, wire
compatibility and public API. "It works" is not sufficient for hot paths; show the budget or
benchmark. See [AGENTS.md](AGENTS.md), [docs/agents.md](docs/agents.md) and the reviewer
prompts in [`.claude/agents/`](.claude/agents) for the detailed workflow and milestone gates.

## Reporting security issues

Do **not** open a public issue for a vulnerability. See [SECURITY.md](SECURITY.md).

## Issues

Check the [issue tracker](https://github.com/JakeGT/moba-engine/issues) before starting
non-trivial work. Claim or open an issue that describes the acceptance criteria; PRs should link
it. Renderer/content/security work has component labels and milestone groups.

## Pull requests

`main` is protected: direct pushes, force-pushes and deletion are blocked, history must be
linear, and every change lands through a pull request (squash or rebase merge) once the
required CI checks pass: `gates` on Linux, Windows and macOS, `tools + deterministic soak`,
`deny` and `miri`. Branches must be up to date with `main` before merging. Release tags
(`v*`) cannot be moved or deleted. If you rename a CI job, update the required checks in the
repository ruleset in the same PR.

Use the repository PR template and include the commands you ran. Changes that touch the
simulation, protocol, content schema or replay format must include determinism/replay evidence
and call out compatibility/migration impact. Keep unrelated refactors out of feature PRs.

## Cross-platform determinism

`just state-hash-check` runs a fixed scripted simulation and compares the state hash to
`tests/golden-state-hash.txt`; CI requires the match on Linux, Windows and macOS. Update the
golden value deliberately with simulation/replay changes.
