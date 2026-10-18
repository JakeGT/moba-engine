# Agent guide — MOBA Engine

This file is the authoritative development guide for coding agents. Machine-specific setup is
kept in `agents.local.md` (gitignored); a portable template is
[`agents.local.example.md`](agents.local.example.md). Claude Code also reads `CLAUDE.md`,
which points here.

Repository: <https://github.com/JakeGT/moba-engine>  
Issue tracker: <https://github.com/JakeGT/moba-engine/issues>

## What this is

A server-authoritative 5v5 MOBA engine in Rust (edition 2024) with a Vulkan 1.2 client.
Deterministic 30 Hz simulation, UDP netcode, runtime TOML content, replay tooling and a
layered workspace intended to be embedded or extended.

Current contract versions:

- wire protocol **v14** (`crates/net/src/protocol.rs::PROTOCOL_VERSION`)
- replay format **v5** (simulation logic v3; purchases are first-class inputs) (`crates/sim/src/replay.rs::VERSION`)
- runtime content schema as documented in `docs/content-authoring.md`
- workspace version **0.1.0**, pre-1.0 preview

Read first:

1. [`README.md`](README.md) — quick start and feature overview.
2. [`docs/architecture.md`](docs/architecture.md) — layers, data flow, determinism, netcode.
3. [`docs/status.md`](docs/status.md) — what is done, measured and still open.
4. [`docs/building-on-moba.md`](docs/building-on-moba.md) — embedding and extension patterns.
5. [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`docs/API-STABILITY.md`](docs/API-STABILITY.md).

## Repository map

```text
crates/core       fixed-point math, RNG, arena, bitsets, time, stable hashing
crates/ecs        generational SoA archetype ECS, change ticks, queries
crates/jobs       scoped work-stealing pool
crates/nav        deterministic spatial grid
crates/game       defaults + runtime content, abilities, items, map, mitigation
crates/sim        deterministic simulation, status effects, bots, replay
crates/net        UDP transport, protocol, crypto, snapshots, prediction, server/client
crates/netsim     lossy-link conditioner
crates/assets     asset/model stub (client depends on it; real assets are planned)
crates/render     ash/Vulkan renderer, terrain/fog, HUD batch
crates/shaders    GLSL -> SPIR-V build
crates/platform   winit window/input/fixed step
crates/client     playable client, HUD, spectator, capture modes
crates/server     headless dedicated server
crates/tools      content-check, replay, match-soak, network soak, fuzz-wire
crates/engine     umbrella crate + embedding examples
content/          example runtime TOML packs
docs/             architecture, status, tasks, ADRs, perf logs, screenshots
```

## Non-negotiable invariants

1. **Layering:** `moba-server`, `moba-sim`, `moba-game`, `moba-nav`, `moba-net` must not depend
   on `moba-render`, `moba-platform`, `ash`, `winit` or `gpu-allocator`. The server must build
   without Vulkan.
2. **Server authority:** clients send intent only. Validate cooldowns, mana, range, vision,
   movement, purchases and input sequence in the server path. Never accept client state.
3. **Determinism:** gameplay code must not use wall clocks, floats, environment reads,
   `HashMap`/`HashSet` iteration or randomness outside the seeded RNG. Iterate ECS archetypes
   deterministically, break ties by entity handle, and hash every gameplay-affecting component
   (see `docs/status.md` and `crates/sim/tests/allocations.rs`).
4. **Hot path allocation:** per tick/frame/entity paths should not allocate. The allocation
   guard enforces the steady-state tick budget; reuse scratch buffers and pre-size pools.
5. **`unsafe`:** only in `core`, `ecs`, `jobs`, `platform`, `render`; every block needs a
   `// SAFETY:` proof and one unsafe operation per block. Other crates are
   `#![forbid(unsafe_code)]`.
6. **Versioning:** bump `PROTOCOL_VERSION` for any wire change, bump replay `VERSION` for
   replay layout changes, and update content hashing/docs for schema changes. Never reinterpret
   an existing protocol version.
7. **Content/replay compatibility:** replays record content hash + pack name/version. Adding a
   gameplay-affecting content field changes the hash and invalidates old replays; call this out.
8. **Docs and screenshots are part of the change** when user-visible behavior changes.

## Commands

From the repository root:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test --workspace --release --locked
cargo run -q --release -p moba-tools --bin state-hash   # must match tests/golden-state-hash.txt
```

Feature/layering checks:

```bash
cargo check -p moba-engine --no-default-features --locked
cargo check -p moba-engine --no-default-features --features render --locked
cargo check -p moba-engine --all-features --locked
for k in moba-server moba-sim moba-game moba-nav moba-net; do
  cargo tree -p "$k" -e normal --prefix none --locked |
    grep -E '^(moba-render|moba-platform|ash|winit|gpu-allocator) ' && exit 1
done
```

Tools (release recommended for soak/fuzz):

```bash
cargo run -p moba-tools --bin content-check -- content/example-pack.toml
cargo run -p moba-tools --bin content-check -- --json content/example-pack.toml
cargo run -p moba-tools --bin replay -- --items content/example-pack.toml match.mbr
cargo run --release -p moba-tools --bin match-soak -- --matches 10 --ticks 12000
cargo run --release -p moba-tools --bin soak -- --seconds 300
cargo run --release -p moba-tools --bin fuzz-wire -- --seconds 60
```

`just` recipes mirror these if installed: `just ci-core`, `just ci-tools`, `just ci`,
`just soak`, `just fuzz-wire`, `just content-check FILE`, `just replay FILE`.

Embedding examples:

```bash
cargo run -p moba-engine --example headless_match
cargo run -p moba-engine --example custom_content
```

## Performance and quality budgets

| Budget | Target/evidence |
|---|---|
| Server tick (one 5v5) | ≤ 2 ms; measured ~13.3 µs/tick steady (far under) |
| Downstream/client | ≤ 20 KB/s typical, ≤ 40 KB/s teamfight; soak ~5.0 KiB/s |
| Upstream/client | ≤ 4 KB/s; soak ~1.17 KiB/s |
| Network condition | 150 ms RTT, 5% loss, 30 ms jitter playable |
| Client frame | ≤ 6.9 ms min-spec, never >16.6 ms (hardware validation still open) |
| Allocation guard | ≤30 allocations / ≤4 KiB per 300 steady ticks |

If a hot path changes, run the relevant benchmark and record evidence in
`docs/perf/` rather than asserting a win.

## Workflow

1. Check the GitHub issue tracker first; claim/link the issue for non-trivial work.
2. Branch from `main` (e.g. `fix/replay-divergence`). Keep unrelated refactors out.
3. Add a failing test/benchmark first for behavioral or performance changes.
4. Implement; update docs/changelog and screenshots where user-visible.
5. Run the full gates above. For content/protocol/replay changes, additionally run a replay,
   content-check and a short fuzz/soak.
6. Commit with conventional messages (`feat(area): ...`, `fix(area): ...`, `docs: ...`).
7. Open a PR using `.github/PULL_REQUEST_TEMPLATE.md`; include commands and evidence.
   `main` is protected by a ruleset (no direct or force pushes; required checks `gates` ×3 OS,
   `tools + deterministic soak`, `deny`, `miri`; linear history). Never push to `main`; push a
   branch and open a PR. Renaming a CI job requires updating the ruleset's required checks.
8. Do not mark work complete while a linked issue has unfinished acceptance criteria.

### Tests that need care

- `cargo test -p moba-sim --test allocations` is a global allocator test; run it on its own
  test binary (it is already an integration test) and avoid adding parallel allocator tests.
- Real-UDP tests bind loopback and use short timeouts; keep them deterministic and avoid
  fixed ports.
- Server tests use ephemeral ports and temp files.
- Replay tests intentionally corrupt checkpoints/headers; keep failure modes panic-free.

## Security expectations

- Plaintext remains the LAN default; optional PSK/token and Ed25519 identity modes must remain
  additive and tested.
- Bound queues, challenge/nonce caches and snapshot history. Never panic on malformed wire
  input; use `fuzz-wire` for decoders.
- Treat client input as hostile. Add tests for wrong key/token/identity/pin when touching auth.
- Do not commit secrets, private keys, player tokens, replay files with sensitive data or
  machine-specific `agents.local.md`.

## Screenshots

Docs include running-program captures in [`docs/screenshots.md`](docs/screenshots.md). For a
user-visible client change, capture a new frame with `MOBA_CAPTURE`/`MOBA_CAPTURE_FRAME` and add
it to the gallery. Keep images reasonably sized (PNG) and describe what the screenshot proves.

## Issue labels

Use the tracker’s labels: `bug`, `enhancement`, `documentation`, `renderer`, `simulation`,
`networking`, `security`, `content`, `product`, `infrastructure`, `release`. Link related issues/PRs instead of duplicating work.

## Definition of done

- Linked issue/acceptance criteria complete.
- Tests/benchmarks/docs updated and all gates green.
- Protocol/content/replay compatibility noted.
- No new hot-path allocations or layering violations.
- Evidence (test outputs, capture, benchmark/soak/fuzz) recorded in the PR or docs.
