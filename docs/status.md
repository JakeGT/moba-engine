# Engine status

Last updated: 2026-10-18. Protocol **v13**; replay format **v5** (sim logic v1); content packs are runtime
TOML. The engine is a playable preview (`0.1`), not a finished commercial title.

## Milestones

| # | Milestone | State | Notes |
|---|---|---|---|
| M0 | Bootstrap | ✅ [evidence](../README.md) | Workspace, lints, CI, justfile, agent instructions. |
| M1 | Core runtime | 🟡 [evidence](tasks/0001-core-runtime.md) | Fixed-point core, SoA ECS, work stealing, arenas/bitsets; Miri is green in CI. The Loom model of the job-scope counter is still open (#16), so the milestone is not complete. |
| M2 | Platform | 🟡 [evidence](tasks/0002-platform.md) | Linux runtime verified; Windows/macOS are compile/CI checked but not runtime-validated. winit/input, 30 Hz step and headless modes are present. |
| M3 | Renderer | 🟡 [evidence](tasks/0003-renderer.md) | Vulkan 1.2 device/swapchain/dynamic rendering, instanced units, terrain fog/LOS, bitmap UI and readback; validated on lavapipe. Skinning, golden-image CI, min-spec frame timings and Windows/macOS validation remain. |
| M4 | Simulation | ✅ [evidence](tasks/0004-simulation.md) | Deterministic 5v5, 3 lanes, towers/nexuses, minions, bots, respawns, XP/gold/items, vision filtering; **13.3 µs/tick** steady benchmark. |
| M5 | Networking | ✅ [evidence](tasks/0005-networking.md) | UDP, fragmentation, delta snapshots, prediction/interpolation, reconnect, spectators, protocol v13 complete-catalogue handshake/identity; v12 one-hour fuzz passed and v13 passed a short fuzz smoke. |
| M6 | Vertical slice | 🟡 [evidence](tasks/0006-vertical-slice.md) | Lobby/countdown/results/rematch, ready/surrender, reconnect, spectator, HUD, shop, minimap, scoreboard, fog/LOS, status icons. Audio/settings deferred; a 10-client playtest on a real network has not been recorded. |
| M7 | Hardening | 🟡 [evidence](tasks/0007-hardening.md) | AEAD, per-client tokens, Ed25519 server/client identity, handshake replay window, per-address/global limits, graceful shutdown, results log, soak/fuzz/bench evidence. 50 *concurrent* matches per box and independent code-review evidence remain open. |
| M8 | Content pipeline | 🟡 [evidence](tasks/0008-content.md) | Runtime item/hero/ability/map packs, manifests, hot reload (empty lobby), 10 ability kinds, per-hero growth. Per-client live updates, manifest attestation and asset mapping remain. |

## Core capabilities

- **Deterministic simulation**: Q32.32 fixed point, integer math, no wall clocks/hash maps in
  gameplay, seeded config, stable state hashing and replay verification.
- **Server authority**: clients send intent only; cooldowns, mana, range, vision, purchases and
  movement are validated by the server.
- **Netcode**: delta snapshots against acked baselines, team visibility filtering, local-hero
  prediction, remote interpolation, reconnect grace and spectators.
- **Security**: optional ChaCha20-Poly1305 sessions, per-client token ring, pinned Ed25519
  server identity, Ed25519 client identity with address-bound challenges, replay window and
  application-layer DoS budgets.
- **Content**: validated runtime TOML packs with name/version/author manifests, content hashes,
  hot reload in an empty lobby, and content carried in `Welcome`.
- **Tooling**: replay verifier with checkpoints, deterministic match soak, lossy network soak,
  wire fuzzer, content checker, JSON output and an embedding guide/examples.

## Verification evidence

- `cargo test --workspace --locked`: **195 passed**, 0 failed, 3 ignored.
- `cargo test --workspace --release --locked`: **195 passed**, 0 failed.
- `cargo fmt --all --check`; clippy `--workspace --all-targets -- -D warnings`; rustdoc with
  `RUSTDOCFLAGS="-D warnings"` are clean.
- Feature matrix (`moba-engine` no-default, `render`, all-features) and crate layering checks
  pass.
- One-hour wire fuzz:
  - v11 shape: **2,476,647,000 iterations clean**.
  - final v12 shape: **2,384,035,000 iterations clean**.
- Final 10-client soak, 300 s at 150 ms RTT / 5% loss / 30 ms jitter: all connected,
  **9,117 ticks**, avg tick **15 µs**, max **304 µs**, ~**5.0 KiB/s down** and
  **1.17 KiB/s up** per client, `rate_limited=0`.
- 50-match deterministic soak: 509,820 ticks, unique hashes per seed, avg 6 µs / max
  3,021 µs; outcomes blue 30 / red 20. The 95% Wilson interval (46.2%–72.4%) includes even
  odds, so this sample does not establish a side bias.
- Benchmarks: sim **11.0 µs/tick** full, **13.3 µs** steady, **1.43 µs** idle; snapshot full
  encode/decode **207/253 ns**, delta encode/decode/apply **36/93/84 ns**.
- Allocation guard: **12 allocations / 490 bytes per 300 steady ticks**.
- Auth smoke: pinned server + client identity + per-client token produced a verified
  314-tick replay. Missing/wrong identity/token/pin paths are covered by tests.
- Replay v5: round-trip, checkpoint, human-purchase and unsupported-version tests pass;
  cross-OS golden state hash `0xb852fe4bba4e7c28`. The old custom-pack state hash is
  intentionally retired by the #26/#27 hash/format change.
- Release audit: 0 broken local Markdown links; every example/map/ability content pack
  validates.
- Renderer audit: validation-required + sync-validation client capture passes with zero
  messages (Linux/lavapipe); macOS instance creation is fixed but awaiting macOS runtime CI.
- CI: cross-OS golden state hash, release tests/doctests, MSRV 1.85 and nightly fuzz/soak/
  match-soak/bench workflows are configured.
- GitHub CI is green on Ubuntu, Windows and macOS, plus Miri, cargo-deny and the `tools` job
  (deterministic match soak/bench compilation). The recurring network soak runs in the nightly
  workflow; see [Actions](https://github.com/JakeGT/moba-engine/actions).

Screenshots are in [the gallery](screenshots.md). Renderer/network captures use lavapipe and
headless readback; see [software Vulkan](dev/software-vulkan.md).

## Tooling availability

The local build container does not provide `just`, `cargo-nextest`, `cargo-deny` or Miri.
Equivalent cargo commands were run manually; CI runs Miri over the unsafe containers and
`cargo-deny`, and the nightly workflow runs the one-hour fuzz/network soak. Local Miri
evidence predates the final round; the authoritative result is the green CI `miri` job.

## Known gaps and tracked work

Open work is tracked in [GitHub Issues](https://github.com/JakeGT/moba-engine/issues).
Current themes:

- Renderer: [skinning/animation](https://github.com/JakeGT/moba-engine/issues/7),
  [golden-image CI](https://github.com/JakeGT/moba-engine/issues/8),
  [min-spec/Windows/macOS validation](https://github.com/JakeGT/moba-engine/issues/9), and
  [asset pipeline/per-map rendering](https://github.com/JakeGT/moba-engine/issues/12).
- Content: [per-client updates](https://github.com/JakeGT/moba-engine/issues/10),
  [manifest attestation](https://github.com/JakeGT/moba-engine/issues/11),
  [scripting/advanced behaviours](https://github.com/JakeGT/moba-engine/issues/13), and
  [map selection](https://github.com/JakeGT/moba-engine/issues/20).
- Hardening: [account/key-transparency/player-count](https://github.com/JakeGT/moba-engine/issues/14),
  [sanitizer fuzzing](https://github.com/JakeGT/moba-engine/issues/15),
  [Loom job model](https://github.com/JakeGT/moba-engine/issues/16), and
  [larger balance sweeps](https://github.com/JakeGT/moba-engine/issues/17).
- Product: [audio](https://github.com/JakeGT/moba-engine/issues/18) and
  [settings/remapping/UI scale](https://github.com/JakeGT/moba-engine/issues/19).
- Release: [crates.io publishing/metadata](https://github.com/JakeGT/moba-engine/issues/21) and
  [replace unmaintained ttf-parser](https://github.com/JakeGT/moba-engine/issues/22).

## Open-source surface

Dual MIT/Apache-2.0. Repository: <https://github.com/JakeGT/moba-engine>.
README, architecture, content authoring, API stability, security, contributing, code of
conduct, release checklist, screenshot gallery, agent instructions and GitHub issue/PR
templates are included. `publish = false` remains intentional until crate names, ownership and
per-crate metadata are finalized; see [RELEASE.md](../RELEASE.md).
