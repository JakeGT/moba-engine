# Agent guide — MOBA Engine

This file is the authoritative development guide for coding agents. The operational runbook
(work selection, PR lifecycle, verification matrix, handoff template) is
[`docs/agents.md`](docs/agents.md). Machine-specific setup lives in `agents.local.md`
(gitignored); the portable template is [`agents.local.example.md`](agents.local.example.md).
Claude Code reads [`CLAUDE.md`](CLAUDE.md), which points here; GitHub Copilot uses
[`.github/copilot-instructions.md`](.github/copilot-instructions.md).

Repository: <https://github.com/JakeGT/moba-engine>  
Issue tracker: <https://github.com/JakeGT/moba-engine/issues>

## Current contract

A server-authoritative MOBA engine in Rust 2024 with a Vulkan 1.3+ client. Deterministic 30 Hz
simulation, UDP netcode, runtime TOML content, replay tooling and a layered workspace intended
for embedding and extension. This is a **pre-1.0 preview**, not a finished commercial title.

| Item | Current value | Source of truth |
|---|---|---|
| Workspace version | `0.4.0` | `Cargo.toml` |
| Wire protocol | **v41** | `crates/net/src/protocol/mod.rs::PROTOCOL_VERSION` |
| Replay format | **v5** | `crates/sim/src/replay.rs::VERSION` |
| Simulation logic | **v8** | `crates/sim/src/replay.rs::SIM_LOGIC_VERSION` |
| Content schema | runtime TOML, hashed; optional Ed25519 attestation | `crates/game/src/content/` (`schema.rs`, `hash.rs`) |
| Golden state hash | `0x8db35a0f5aab5190` | `tests/golden-state-hash.txt` |
| Golden image | 64×36 fingerprints (default scene + networked skinned capture), lavapipe, validation required | `.github/workflows/render-golden.yml` |
| Tests | **677 passed**, 3 ignored | `cargo test --workspace --locked` |

Re-run the commands before quoting these values; update this table when a contract changes.

## Read first

1. [`README.md`](README.md) — quick start and feature overview.
2. [`docs/plans/audit-remediation-2026-10-19.md`](docs/plans/audit-remediation-2026-10-19.md) —
   completed remediation plan. [`docs/plans/finish-plan-2026-10-20.md`](docs/plans/finish-plan-2026-10-20.md)
   is the current plan (all agent work packages done; see its §2a).
3. [`docs/architecture.md`](docs/architecture.md) — layers, data flow, determinism, netcode.
4. [`docs/status.md`](docs/status.md) — what is done, measured and still open.
5. [`docs/agents.md`](docs/agents.md) — agent workflow and verification matrix.
6. [`docs/building-on-moba.md`](docs/building-on-moba.md) — embedding and extension patterns.
7. [`CONTRIBUTING.md`](CONTRIBUTING.md), [`docs/API-STABILITY.md`](docs/API-STABILITY.md) and
   [`SECURITY.md`](SECURITY.md).

## Repository map

```text
crates/core       fixed-point math, RNG, arena, bitsets, time, stable hashing
crates/ecs        generational SoA archetype ECS, change ticks, queries
crates/jobs       scoped work-stealing pool
crates/nav        deterministic spatial grid
crates/game       defaults + runtime content, abilities, items, map, mitigation, attestation
crates/sim        deterministic simulation, status effects, bots, replay
crates/net        UDP transport, protocol, crypto, snapshots, prediction, server/client
crates/netsim     lossy-link conditioner
crates/assets     asset manifests, fallback resolution, bounded .glb mesh loader (ADR-0007),
                  animation sampling/blending/state machine (ADR-0012), bounded KTX2
                  textures and material descriptors (ADR-0010), VFX descriptors and the
                  procedural particle atlas (ADR-0013)
crates/render     ash/Vulkan renderer: render graph, HDR + tonemap, terrain/fog, MSDF UI text,
                  animated rig, GPU skinning, materials, pipeline cache, per-pass GPU timestamps,
                  compute GPU particles (ADR-0013), post chain: bloom, exposure, grading,
                  FXAA/MSAA, GTAO, render scale (ADR-0015)
crates/shaders    GLSL -> SPIR-V build (shared #include files, enumerated permutations)
crates/platform   winit window/input/fixed step
crates/client     playable client, HUD, spectator, settings, audio, capture modes
crates/server     headless dedicated server
crates/tools      content-check/sign, replay, match-soak, network soak, fuzz-wire,
                  state-hash, image-check, vulkan-probe, bake-ui-atlas, sample-mesh,
                  texture-bake, teamfight (the R5 VFX evidence replay), vat-bake (R4b
                  minion VATs), link-proxy (RTT/jitter/loss UDP proxy), grade-bake (LUTs)
                  (font-bake/: standalone MSDF font baker, own lockfile)
crates/engine     umbrella crate + embedding examples
content/          validated example runtime TOML packs (5v5, 1v1, ARAM, abilities, maps, economy)
docs/             architecture, status, tasks, ADRs, perf logs, screenshots, agent guide
fuzz/             standalone cargo-fuzz crate (wire/replay/content/glb/ktx2/vfx/grading)
scripts/          agent-verify.sh and operational helpers
.github/          CI, golden/fuzz/loom/platform workflows, issue/PR templates
```

## Non-negotiable invariants

1. **Layering:** `moba-server`, `moba-sim`, `moba-game`, `moba-nav`, `moba-net` must not depend
   on `moba-render`, `moba-platform`, `ash`, `winit` or `gpu-allocator`. The server must build
   without Vulkan; `scripts/agent-verify.sh` checks this.
2. **Server authority:** clients send intent only. Validate cooldowns, mana, range, vision,
   movement, purchases and input sequence in the server path. Never accept client state.
3. **Determinism:** gameplay code must not use wall clocks, floats, environment reads,
   `HashMap`/`HashSet` iteration or randomness outside the seeded RNG. Iterate ECS archetypes
   deterministically, break ties by entity handle, and hash every gameplay-affecting component.
4. **Hot-path allocation:** per tick/frame/entity paths should not allocate. The allocation
   guards enforce the steady-state tick and snapshot budgets; reuse scratch buffers and
   pre-size pools.
5. **`unsafe`:** only in `core`, `ecs`, `jobs`, `platform`, `render`; every block needs a
   `// SAFETY:` proof and one unsafe operation per block. Other crates are
   `#![forbid(unsafe_code)]`. The client keeps its test-only counting allocator as a
   documented test exception; non-test client code is covered by crate-level `forbid`.
6. **Versioning:** bump `PROTOCOL_VERSION` for any wire change, replay `VERSION` for replay
   layout changes and `SIM_LOGIC_VERSION` for deterministic behavior changes. Update content
   hashing/docs for schema changes. Never reinterpret an existing version.
7. **Content/replay compatibility:** replays record content hash + pack name/version. Adding a
   gameplay-affecting content field changes the hash and invalidates old replays; call this
   out. Attestation fields are excluded from the catalogue hash but covered by the signature.
8. **Docs and screenshots are part of the change** when user-visible behavior or contracts
   change. Keep `docs/status.md`, task logs, `CHANGELOG.md`, `AGENTS.md` and protocol/replay/
   content docs consistent.

## Commands

Use the repository script for the fast gate; it avoids PATH/ordering mistakes:

```bash
scripts/check-docs.sh            # relative links + contract version/test-count consistency
scripts/agent-verify.sh          # docs, fmt, clippy, tests, docs, layering, state hash, content packs
scripts/agent-verify.sh --full   # + release tests, feature matrix, Loom, bench compile, soak, fuzz
```

Equivalent explicit gates:

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
cargo run -p moba-tools --bin content-sign -- content/1v1-mid.toml   # MOBA_SIGNING_SEED
cargo run -p moba-tools --bin replay -- --items content/example-pack.toml match.mbr
cargo run --release -p moba-tools --bin match-soak -- --matches 10 --ticks 12000
cargo run --release -p moba-tools --bin match-soak -- --content content/aram.toml
cargo run --release -p moba-tools --bin soak -- --seconds 300
cargo run --release -p moba-tools --bin fuzz-wire -- --seconds 60
cargo run -p moba-tools --bin bake-ui-atlas -- crates/render/assets/ui-atlas.rgba
cargo run --release -p moba-tools --bin texture-bake -- bake in.png out.ktx2 --kind color
scripts/bake-sample-textures.sh                       # regenerate content/textures/, the VFX atlas, content/grading/
cargo run --release -p moba-tools --bin grade-bake -- bake --preset ember out.cube   # colour-grading LUT (R6)
cargo run --release --locked --manifest-path crates/tools/font-bake/Cargo.toml   # MSDF HUD font (R7a)
cargo run --release -p moba-tools --bin teamfight     # regenerate tests/replays/teamfight-5v5.mbr
MOBA_VFX_PREVIEW=nova cargo run -p moba-client        # live-reload effect preview (content/vfx/)
cargo run -p moba-tools --bin image-check -- actual.ppm --expected tests/golden/render-fingerprint.rgb
cargo run -p moba-tools --bin vulkan-probe
```

Fuzz and model checking:

```bash
cargo check --manifest-path fuzz/Cargo.toml
cargo install cargo-fuzz --locked
cargo +nightly fuzz run wire -- -max_total_time=60
cargo +nightly fuzz run replay -- -max_total_time=60
cargo +nightly fuzz run content -- -max_total_time=60
cargo +nightly fuzz run glb -- -max_total_time=60
cargo +nightly fuzz run ktx2 -- -max_total_time=60
cargo +nightly fuzz run vfx -- -max_total_time=60
cargo +nightly fuzz run grading -- -max_total_time=60
RUSTFLAGS="--cfg loom" cargo test -p moba-jobs --lib loom -- --test-threads=1
```

`just` recipes mirror the gates (`just docs-check`, `just agent-check`, `just ci-core`, `just ci`,
`just soak`, `just fuzz`, `just fuzz-wire`, `just loom`, `just content-check FILE`,
`just replay FILE`, `just match-soak`, `just miri`), but `just` may not be installed in agent
containers. Prefer the explicit commands/script when in doubt.

Embedding examples:

```bash
cargo run -p moba-engine --example headless_match
cargo run -p moba-engine --example custom_content
```

## Fresh checkout / GPU

- Rust **1.88+** (workspace MSRV) and a `glslc`/`glslangValidator` for shader builds.
- Vulkan 1.3+ driver on Linux and Windows; macOS uses MoltenVK through the windowing stack.
- CI goldens use lavapipe; local software Vulkan setup is in
  [`docs/dev/software-vulkan.md`](docs/dev/software-vulkan.md).
- Real GPU frame logging: `MOBA_VSYNC=0 MOBA_FRAME_LOG=/tmp/frames.log` (see
  [`docs/perf/0007-real-hardware.md`](docs/perf/0007-real-hardware.md)).
- Linux/NVIDIA evidence exists (RTX 5070 Ti, p95 4.197 ms at 1920×1080). macOS hosted-runner
  p95 is 16.945 ms and Windows hosted runners expose no Vulkan GPU; this is tracked in #9/#127.

## Performance and quality budgets

| Budget | Target/evidence |
|---|---|
| Server tick (one 5v5) | ≤2 ms; measured **~26.3 µs/tick steady** (7.876 ms/300 ticks, 2026-10-19) |
| Downstream/client | ≤20 KB/s typical, ≤40 KB/s teamfight; soak ~5.0 KiB/s |
| Upstream/client | ≤4 KB/s; soak ~1.17 KiB/s |
| Network condition | 150 ms RTT, 5% loss, 30 ms jitter playable |
| Client frame | ≤6.9 ms p95, never >16.6 ms; Linux/NVIDIA p95 4.197 ms |
| Allocation guard | ≤8 allocations / ≤1 KiB per 300-tick window (steady/teamfight/respawn/shop/modifiers expected 0; wave spawn ≤2) |
| Snapshot build/encode | zero steady-state allocations; scaling benchmark in `docs/perf/0005-networking.md` |

If a hot path changes, run the relevant benchmark and record evidence in `docs/perf/` rather
than asserting a win. Do not copy stale numbers from old docs without re-running.

## Workflow

1. Check the GitHub issue tracker first; claim/link the issue for non-trivial work.
2. Synchronize `main` (`fetch` + `reset --hard origin/main`), then branch:
   `feat/…`, `fix/…`, `docs/…`, `perf/…`, `ci/…`, `security/…`.
3. Add a failing test/benchmark first for behavioral or performance changes.
4. Implement; update docs/changelog, contract versions and screenshots where applicable.
5. Run `scripts/agent-verify.sh`; use `--full` for contract/performance/security changes.
6. Commit with conventional messages (`feat(area): ...`, `fix(area): ...`, `docs: ...`).
7. Open a PR using `.github/PULL_REQUEST_TEMPLATE.md`; include commands and evidence.
   `main` is protected (no direct/force pushes; required checks `gates` ×3 OS,
   `tools + deterministic soak`, `release tests + doctests`, `MSRV`, `renderer-gate`,
   `deny`, `miri`; linear history). Use a body file for Markdown
   with backticks. Renaming a CI job requires updating the ruleset's required checks.
8. Watch checks, squash-merge, delete the branch, then `fetch` + `reset --hard origin/main`.
9. Do not mark an issue complete while any acceptance criterion lacks linked evidence.

Manual workflows:

```bash
gh workflow run render-golden.yml --repo JakeGT/moba-engine --ref main  # manual full renderer gate
gh workflow run macos-frame.yml --repo JakeGT/moba-engine --ref main
gh workflow run macos-vulkan.yml --repo JakeGT/moba-engine --ref main
gh workflow run windows-probe.yml --repo JakeGT/moba-engine --ref main
gh workflow run fuzz.yml --repo JakeGT/moba-engine --ref main
gh workflow run loom.yml --repo JakeGT/moba-engine --ref main
```

### Tests that need care

- `cargo test -p moba-sim --test allocations` is a global allocator test; run it as its own
  binary and avoid adding parallel allocator tests.
- `crates/net/tests/snapshot_allocations.rs` counts allocations; warm tick-zero paths before
  resetting counters.
- Real-UDP tests bind loopback with short timeouts; avoid fixed ports and flaky sleeps.
- Server tests use ephemeral ports and temp files.
- Replay tests intentionally corrupt checkpoints/headers; keep failure modes panic-free.
- `fuzz/` is a separate workspace; `cargo check --manifest-path fuzz/Cargo.toml` is the stable
  compile gate. Do not commit `fuzz/target` or `fuzz/artifacts`.

## Security expectations

- Plaintext remains the LAN default; optional PSK/token, Ed25519 identity (rotation/revocation),
  pinned server identity and content attestation must remain additive and tested.
- Bound queues, challenge/nonce caches and snapshot history. Never panic on malformed wire
  input; run `fuzz-wire`/cargo-fuzz for decoder changes.
- Treat client input as hostile. Add tests for wrong key/token/identity/pin/attestation.
- Do not commit secrets, private keys, player tokens, replay files with sensitive data or
  machine-specific `agents.local.md`.

## Known blockers / deferred work

- **#9 / #127:** real Windows/macOS GPU results. Linux/NVIDIA and validation evidence exist;
  hosted macOS is below target and hosted Windows has no Vulkan device. Do not claim a platform
  pass from a compile-only or probe-only job.
- **#21:** crates.io publication needs registry ownership and `CARGO_REGISTRY_TOKEN`.
  `publish = false` is intentional; tagged GitHub releases are the distribution path.
- **#276:** asset pipeline, GPU skinning and mirror/balance work is done (#308–#319); items have
  no asset references yet. The M6 playtest (WP-5.2) and the external review (WP-6.3) need people and are
  scheduled just before the 1.0 release, with #9/#127 and #21.
- Product roadmap items may be opened for future work; do not close a tracking issue merely
  because its wave landed. Update `docs/status.md` and the plan instead.

## Screenshots

Docs include running-program captures in [`docs/screenshots.md`](docs/screenshots.md). For a
user-visible client change, capture a new frame with `MOBA_CAPTURE`/`MOBA_CAPTURE_FRAME` and add
it to the gallery. Keep images reasonably sized (PNG) and describe what the screenshot proves.
Golden rebaselines must use the CI-produced 1280×720 capture, not a differently sized local
window.

## Issue labels

Use the tracker's labels: `bug`, `enhancement`, `documentation`, `renderer`, `simulation`,
`networking`, `security`, `content`, `product`, `infrastructure`, `release`, `agent-task`.
Link related issues/PRs instead of duplicating work.

## Definition of done

- Linked issue/acceptance criteria complete.
- Tests/benchmarks/docs updated and all required gates green.
- Protocol/content/replay compatibility noted and versioned.
- No new hot-path allocations or layering violations.
- Evidence (test outputs, capture, benchmark/soak/fuzz, hardware) recorded in the PR or docs.
- Docs (status/plan/task/agent guide) reflect reality; no stale counts or claims.
