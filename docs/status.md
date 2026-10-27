# Engine status

Last updated: **2026-10-21**. Workspace **0.4.0** preview; protocol **v38**; replay format **v5**;
simulation logic **v7**; content packs are validated runtime TOML.

The engine is playable end-to-end and has a stable internal contract, but it is pre-1.0.
Two external-resource items remain open: real Windows/macOS GPU frame results (#9/#127) and
crates.io publication enablement (#21). The 2026-10-19 audit remediation is complete; post-1.0 asset/skinning work is tracked in #276.

## Milestones

| # | Milestone | State | Notes |
|---|---|---|---|
| M0 | Bootstrap | ✅ | Workspace, lints, CI, justfile, [agent instructions](../AGENTS.md), issue/PR templates. |
| M1 | Core runtime | ✅ | Fixed-point core, SoA ECS, work stealing, arenas/bitsets; Miri and the [Loom job-scope model](../.github/workflows/loom.yml) are green in CI. |
| M2 | Platform | 🟡 | Linux runtime verified; Windows/macOS compile and CI-checked, runtime GPU validation still open (#9). winit/input, 30 Hz step, settings and headless modes present. |
| M3 | Renderer | 🟡 | Vulkan 1.3 device/swapchain/dynamic rendering, instanced units, terrain fog/LOS, textured UI atlas, animated hero rig, map themes, [golden-image CI](../.github/workflows/render-golden.yml) and [NVIDIA timings](perf/0007-real-hardware.md). Windows/macOS real-GPU timing remains open (#127). |
| M4 | Simulation | ✅ | [Deterministic 5v5](perf/0004-simulation.md), 3 lanes, towers/nexuses, minions, vision-respecting bots, respawns, XP/gold/items, modes/rules; measured ~26.3 µs/tick steady benchmark (7.876 ms/300 ticks, 2026-10-19). |
| M5 | Networking | ✅ | [UDP, fragmentation](perf/0005-networking.md), delta snapshots, prediction/interpolation, reconnect, spectators, protocol v38, content updates, identity/attestation. Deterministic and sanitizer fuzz gates exist. |
| M6 | Vertical slice | 🟡 | Lobby/countdown/results/rematch, ready/surrender, reconnect, spectator, HUD, shop, minimap, scoreboard, fog/LOS, status icons, procedural audio, persistent settings/remapping/UI scale. A 10-client playtest on a real network has not been recorded. |
| M7 | Hardening | 🟡 | AEAD, per-client tokens, Ed25519 server/client identity with rotation/revocation, handshake replay window, per-address/global limits, graceful shutdown, results log, soak/fuzz/bench evidence, Loom model. 50 *concurrent* matches per box and independent review remain open. |
| M8 | Content pipeline | ✅ | [Runtime](tasks/0008-content.md) item/hero/ability/map packs, manifests, per-client catalogue updates (v36), manifest attestation, map names/rotation, visual themes and modes. Asset meshes/skinning assets remain future rendering work. |

## Core capabilities

- **Deterministic simulation**: Q32.32 fixed point, integer math, no wall clocks/hash maps in
  gameplay, seeded config, stable state hashing and replay verification.
- **Server authority**: clients send intent only; cooldowns, mana, range, vision, purchases and
  movement are validated by the server.
- **Netcode**: delta snapshots against acked baselines, team visibility filtering, local-hero
  prediction, remote interpolation, reconnect grace, spectators and between-match catalogue
  updates.
- **Security**: optional ChaCha20-Poly1305 sessions, per-client token ring, pinned Ed25519
  server identity, Ed25519 client identity with rotation/revocation, content-pack attestation,
  replay window and application-layer DoS budgets.
- **Content**: validated runtime TOML packs with hashes, signed manifests, hot reload in
  Lobby/Results, map names/themes, empty-lobby rotation and 1v1/ARAM modes.
- **Client**: textured UI atlas/icons, settings/key remapping/UI scale, procedural audio,
  animated hero rig, map themes, spectator camera, capture/frame-log modes.
- **Tooling**: replay verifier, match/network soak, wire fuzzers (deterministic + cargo-fuzz),
  content checker/signer, state hash, image fingerprint, atlas bake, Vulkan probe and Loom.

## Verification evidence

- `cargo test --workspace --locked`: **324 passed**, 0 failed, 3 ignored.
- `cargo test --workspace --release --locked`: passes (same suite; CI job `release tests + doctests`).
- `cargo fmt --all --check`; clippy `--workspace --all-targets -- -D warnings`; rustdoc with
  `RUSTDOCFLAGS="-D warnings"` are clean.
- Feature matrix (`moba-engine` no-default, `render`, all-features) and crate layering checks
  pass. `scripts/agent-verify.sh --full` runs these plus Loom/bench compile/soak/fuzz.
- Deterministic cross-OS golden state hash: `0x8db35a0f5aab5190`.
- Golden render workflow: 64×36 fingerprint on lavapipe with validation + sync validation;
  latest rebaseline after the animated hero rig is green.
- Deterministic wire fuzzer historical evidence: v11 **2,476,647,000** iterations clean and
  final v12 **2,384,035,000** clean. Current cargo-fuzz targets `wire`, `replay`, `content`
  run in the manual/nightly `fuzz` workflow; a 30 s-per-target run passed.
- Loom model: `loom.yml` weekly/manual run passes.
- 10-client soak (historical, v12): all connected, 9,117 ticks, avg tick 15 µs, max 304 µs,
  ~5.0 KiB/s down / 1.17 KiB/s up per client, no rate-limit drops.
- 50-match soak (historical): 509,820 ticks, unique hashes per seed, avg 6 µs/max 3,021 µs;
  outcomes blue 30 / red 20 (Wilson interval includes even odds).
- Allocation guards: 6/6 simulation budget tests pass (steady/teamfight/respawn/shop/status/
  wave); snapshot build+encode allocates **0** in steady state.
- Replay v5: round-trip, checkpoint, human-purchase, content-hash and unsupported-version
  tests pass; golden replay fixtures are checked in.
- Content: all six `content/*.toml` packs validate; 1v1/ARAM packs replay deterministically;
  signed-pack verify accepts valid signatures and rejects tampering/untrusted publishers.
- Renderer: validation-required lavapipe capture passes with zero messages. Real NVIDIA
  RTX 5070 Ti at 1920×1080: p50 4.181 ms, p95 4.197 ms, p99 4.212 ms, max 4.538 ms (vsync off).
  macOS hosted runner recorded p95 16.945 ms; Windows hosted runner has no Vulkan GPU.
- Release: `v0.1.0`–`v0.4.0` workflows build Linux/Windows/macOS archives;
  [v0.4.0](https://github.com/JakeGT/moba-engine/releases/tag/v0.4.0) is the current preview.
  crates.io publication remains opt-in (`publish = false`, #21).
- CI: Linux/Windows/macOS gates, Miri, cargo-deny, release tests/doctests, MSRV, no-op/real
  `renderer-gate`, docs link/contract check, golden/fuzz/Loom/platform workflows. Nightly
  failures open an issue automatically. Hosted-runner capability limits are documented, not treated as passes.

Screenshots are in [the gallery](screenshots.md). Renderer/network captures use lavapipe and
headless readback; real-GPU evidence is in [perf/0007](perf/0007-real-hardware.md).

## Tooling availability

The development container may omit `just`, `cargo-nextest`, `cargo-deny`, `cargo-fuzz` or
Miri. Prefer `scripts/agent-verify.sh` for portable gates; CI runs Miri, cargo-deny, cargo-fuzz,
Loom and the nightly soak. Record which optional tools were available when quoting local
evidence.

## Known gaps and tracked work

Open issues: <https://github.com/JakeGT/moba-engine/issues>
Second-audit plan: [`docs/plans/audit-remediation-2026-10-19.md`](plans/audit-remediation-2026-10-19.md)

- **#9 / #127** — real Windows/macOS GPU frame results and min-spec validation. Linux/NVIDIA
  evidence is complete; the hosted macOS result misses the frame target and Windows hosted
  runners expose no Vulkan device. Requires real platform hardware or a revised acceptance
  decision.
- **#21** — crates.io publication. Metadata, release artifacts and policy are ready; registry
  ownership plus `CARGO_REGISTRY_TOKEN` are required. `publish = false` remains intentional.
- **#45** — extensibility roadmap map, not unfinished implementation work.
- **#276** — post-1.0 asset-pipeline/GPU-skinning work and residual mirror/balance coverage.
- Future product work (not blocking the current preview): 50-concurrent-match load test,
  independent external review, larger balance sweeps.

## Open-source surface

Dual MIT/Apache-2.0. Repository: <https://github.com/JakeGT/moba-engine>.
README, architecture, content authoring, API stability, security, contributing, code of
conduct, release checklist, screenshot gallery, agent guide/runbook and GitHub issue/PR
templates are included. `publish = false` remains intentional until crate names, ownership and
per-crate metadata are finalized; see [RELEASE.md](../RELEASE.md) and [docs/agents.md](agents.md).
