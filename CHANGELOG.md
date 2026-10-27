# Changelog

All notable changes are recorded here. The workspace is pre-1.0; minor releases may include
breaking protocol, replay or content-schema changes.

## [0.4.1] — 2026-10-27

Security patch release cut from `v0.4.0` on the `release/0.4.x` branch. Protocol **v37**, replay
**v5**, simulation logic **v7**: wire-, replay- and content-compatible with 0.4.0, and no public
API change. Lockfile-only dependency updates.

### Security
- `ctutils` 0.4.3 → 0.4.6 ([RUSTSEC-2026-0385](https://rustsec.org/advisories/RUSTSEC-2026-0385)).
  Before 0.4.4, `ctutils::Choice` mask generation could compile to a branch on common targets,
  and `u128` comparisons branched on 32-bit targets, which can leak secret values through timing.
  `ctutils` is used by the RustCrypto stack under `moba-net`: `sha2`/`hmac`/`hkdf` (per-client
  tokens, key derivation), `curve25519-dalek`/`ed25519-dalek` (server/client identity) and
  `poly1305`/`chacha20poly1305` (session AEAD). Also updated in `fuzz/Cargo.lock`.
- `tokio` 1.54.0 → 1.54.1 ([RUSTSEC-2026-0384](https://rustsec.org/advisories/RUSTSEC-2026-0384);
  1.54.0 was yanked). Only reached through `winit`'s wasm path; native builds do not run it.

## [0.4.0] — 2026-10-21

Security release. Protocol **v37** (not wire-compatible with 0.3.x), replay **v5**, simulation logic **v7**. Content hashes and replays are unchanged.

### Security
- **GHSA-jw7c-q9x9-cgg3:**
  - Reconnects and re-handshakes are bound to the credential that opened the slot: the same token/PSK, and in identity mode the same client key.
  - Session ids are random.
  - Keyed handshakes answer an address-bound server challenge, which the Hello authenticator (`moba-hello-v2`) covers, so a captured Hello can't be replayed to another server or match.
  - Hello rejects are sent unsealed, and connected clients only act on a `Disconnect` for their own session.
- The client fails closed on malformed `MOBA_SERVER_PUBKEY`, `MOBA_SECRET` and `MOBA_CLIENT_IDENTITY`.
- The server validates allow-listed and rotated identity keys at startup.
- Server and client identity signatures use `verify_strict`.
- AEAD keys are zeroized again.

### Platform and toolchain
- Vulkan **1.3** is the minimum. The 1.2 extension fallback is removed (ADR-0002 amended), and shaders target `vulkan1.3`.
- Rust MSRV **1.88**. The CI check is now named `MSRV`.
- Dependencies are at their latest releases:
  - RustCrypto/dalek: chacha20poly1305 0.11, hmac/hkdf 0.13, sha2 0.11, ed25519-dalek 3, getrandom 0.4. Wire and signature compatibility is proven by known-answer tests.
  - glam 0.34, toml 1.1, gpu-allocator 0.28, cpal 0.18, criterion 0.8.
  - GitHub Actions bumped to their current majors.

### Client
- Audio fixes:
  - Tones play to completion across device buffers.
  - Voices mix instead of queueing.
  - The audio callback no longer allocates.
  - Only the stream for the device's sample format is opened.

### Content
- The built-in content's manifest version is now `BUILTIN_CONTENT_VERSION` ("0.3.0") instead of the crate version. Engine releases no longer change the default content hash, so existing default-content replays stay valid.

### Tooling
- `just bench-baseline` / `bench-compare` now pass criterion flags only to bench targets.
- The docs consistency check also covers CLAUDE.md and docs/agents.md.

## [0.3.0] — 2026-10-20

Preview release after the 2026-10-19 audit remediation. Protocol **v36**, replay **v5**,
simulation logic **v7**.

### Security
- Hardened session transport (independent AEAD counters, replayed-packet rejection, keyed peers
  reject unsealed application packets/fragments).
- Replaced the weak pack digest with domain-separated SHA-256 attestation, strict signature
  verification and a unified trusted loader.
- Hardened content parsing/hashing, catalogue updates and client validation.
- Closed the public unsafe job-scope lifetime issue with scoped ownership, blocking waits and a
  Loom model.

### Simulation
- Deterministic targeting tie-breaks, symmetric fixed-point division, team-size-agnostic bot
  seeds and fast-projectile swept collision.
- Rules-driven starting/passive gold, radii, shared XP, unit rewards, aggro and win conditions.
- Item actives/cooldowns, on-hit/on-damaged passives, HUD cooldowns and bot item use.
- Generic modifier stat bonuses, replicated status bits, data-driven expiry/on-tick/on-damaged
  triggers and a Stun modifier kind.
- Replay decode memory cap and seed-normalized state hash for cross-seed comparisons.

### Networking
- Reliable catalogue delivery with generation checks, acknowledgements, retries and
  connected-client map rotation.
- Snapshot/event allocation guards, transport send-buffer reuse and per-thread allocation
  accounting.

### Renderer
- Capture extent/format snapshots, corrected present/atlas barriers and validation suppression
  removal.
- Setup-failure RAII for buffers, images, samplers and swapchains; resize skip on out-of-date
  rebuilds.
- Correct inverse-transpose normals and explicit texture LOD.
- Downloadable/known-issue validation now counts every error.

### Client
- HUD split into scoreboard/shop/abilities modules; centralized theme colors.
- Input routing, NetworkView, scene-build and launch modules extracted; `app.rs` under 800 lines.
- Procedural audio formats, mute, device recovery, bounded lock-free scheduling and offline
  sound.
- Networked hero rig with interpolated facing/gait; zero-allocation HUD frame and UI-click
  routing tests.

### Content and tooling
- Validated asset manifests with safe paths, duplicate checks and fallback chains.
- Stable map-theme asset keys and map-rotation coverage.
- Fuzzed catalogue decoding; checked replay/fixture and golden-state rebaselines.

### Documentation
- Completed the audit remediation plan and closed tracking issues.
- Replaced the duplicated plan text in this changelog; refreshed status, release and API docs.
- Recorded a 15,000,000-tick determinism sweep.

### Deferred
- Asset-pipeline integration, GPU skinning and remaining mirror/balance coverage are tracked
  post-1.0 in [#276](https://github.com/JakeGT/moba-engine/issues/276).

## [0.2.0] — 2026-10-18

Preview release with runtime content packs and protocol/content hardening.

- Runtime TOML item/hero/ability/map packs, manifests, signed pack attestation and catalogue
  updates.
- Map-driven world extent, arcing projectiles, rules for respawn/wave cadence and per-map
  themes.
- Prediction/interpolation, reconnect, spectators, bot policy seam and dense net-id tables.
- Golden-image renderer gate, replay v5, allocation/soak/fuzz evidence and macOS/Linux CI
  coverage.
- Protocol v16–v25; see the `v0.2.0` tag for generated PR notes.

## [0.1.0] — 2026-10-18

First preview tag.

- Deterministic 30 Hz simulation, ECS/jobs runtime, Vulkan renderer, UDP netcode, server and
  client binaries.
- Golden state hash, replay format v5 and initial release workflow.
- Protocol v13–v15; see the `v0.1.0` tag for generated PR notes.

[0.3.0]: https://github.com/JakeGT/moba-engine/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/JakeGT/moba-engine/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/JakeGT/moba-engine/releases/tag/v0.1.0
