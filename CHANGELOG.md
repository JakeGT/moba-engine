# Changelog

All notable changes are documented here. Format loosely follows Keep a Changelog; the
project is pre-1.0, so minor versions may contain breaking API changes (see
[docs/API-STABILITY.md](docs/API-STABILITY.md)).

## [Unreleased]

### Added
- Release-audit evidence for optimized tests, feature matrix, layering, docs links and packs.
- `haste` self movement-buff ability kind.
- Rooted units can no longer bypass control with a dash.
- Burn state hashing now includes applying entity and damage type.
- Free-pan/rotate/zoom spectator camera (WASD, arrows, Shift boost).
- Refreshed sim/snapshot benchmark capture after the status-effect work.
- Shop pagination (arrows) so packs with more than eight items are purchasable.
- Health-bar icons for shield/slow/root/silence/burn statuses.
- One-hour wire fuzz completed 2,476,647,000 iterations clean (protocol v11).
- Optional Ed25519 client identities with address-bound server challenges and public-key
  allowlists (`--allow-client-key`, `MOBA_CLIENT_IDENTITY`).
- Graceful SIGINT/SIGTERM shutdown that flushes an in-progress replay.
- Documented 50-match bot-balance confidence interval.
- CI tools gate now runs a 30-second wire-decoder fuzz pass.
- `--json` summaries for `replay` and `content-check` tools.
- `burn` damage-over-time ability kind.
- `silence` ability kind blocking casts until expiry.
- `root` ability kind blocking movement until expiry.
- Optional JSONL match-results log (`--results-log`).
- `slow` ability kind with movement multiplier, expiry/respawn restore and client tint.
- Client FPS/frame-time HUD and wall segments on the minimap.
- Per-hero level-growth fields in content packs.
- Bounded handshake nonce replay window (same-address retransmits allowed).
- Replay format v4 periodic state checkpoints and first-divergence tick reporting.
- Per-client token key ring for authenticated/encrypted per-player sessions and rotation.
- Content pack manifests (name/version/author) and replay format v3 carrying them.
- Allocation-regression test and hot-path fix: command ordering no longer allocates a `Vec`
  per tick (312→12 allocations per 300 ticks).
- Global UDP packet budget (`--max-total-pps`) complementing per-address limits.
- TOML dedicated-server configuration (`--config server.example.toml`) with CLI override.
- `shield` ability kind with timed damage absorption, snapshot flag and client tint.
- Content hot reload (`--reload-content`) applied only in an empty lobby, with a server test.
- CI release gate for content validation, deterministic 10-match soak and benchmark
  compilation.
- Optional pinned Ed25519 server identity (`--server-key`, `MOBA_SERVER_PUBKEY`) with a
  signed Welcome challenge.
- Criterion snapshot-codec benchmarks and a profiling capture log
  (`docs/perf/0007-profiling.md`).
- Runtime map vision blockers: TOML `[[map.walls]]`, server LOS filtering, shader
  per-pixel wall shadows, aligned wall geometry and wall data in the handshake map.
- `match-soak` 50-match deterministic harness with composition rotation and `content-check`
  validator; mirrored hero spawn/pick fairness fixes and alternating wave/command order.
- Per-address UDP token-bucket rate limiting (`--max-pps`) with bounded/LRU buckets, drop
  counters and average/max simulation tick telemetry.
- Replay format v2 with content-hash manifests; `moba-tools replay --items`; shared
  `moba-game` TOML loader used by server and tools.
- Runtime item, hero, ability and map content packs (`--items FILE`), validated and published to clients in
  the handshake; fixed progress-only delta encoding bug found by the content integration test.
- Deterministic replay recording (`moba-server --record`) and verification
  (`moba-tools replay`), including a server recording test and decoder fuzz coverage.
- Optional pre-shared-key authenticated encryption: HMAC handshake proofs, HKDF session keys,
  ChaCha20-Poly1305 payloads, encrypted reconnect and privacy-preserving debug output.
- Open-source project surface: dual MIT/Apache-2.0 license, README, architecture,
  content-authoring, API-stability and security docs, contributions guide and templates.
- `cargo doc` quality gate and CI documentation job.
- `moba-engine` umbrella crate with `sim`, `net` and optional `render` features.
- Match flow: lobby/countdown/playing/results/rematch state machine, match overlays, minimap,
  floating health bars and protocol v4 (phase/countdown).
- Runnable embedding examples (`headless_match`, `scripted_orders`), release/security/contributing
  guidance, third-party notices and issue/PR templates.
- `missing_docs` lint for the public API.

## [0.1.0] — preview

### Added
- Deterministic 30 Hz 5v5 MOBA simulation: three lanes, 5 heroes × 3 abilities, minions,
  towers, nexus, respawns, bots, events and state hashing.
- Hero levels/XP with shared experience, gold bounties, passive gold, eight items, six-slot
  inventories, validated shop and bot purchases.
- Custom UDP stack: handshake, session slots, intent input, fragmentation, visibility-filtered
  delta snapshots, client prediction, interpolation, link conditioner and soak/fuzz tools.
- Server-authoritative dedicated server with bot fill, timeouts and bandwidth counters.
- Vulkan 1.2 client: instanced units, procedural terrain, vision-circle fog of war, bitmap
  HUD (health/mana/XP/cooldowns/items/gold), Tab scoreboard, shop, VICTORY/DEFEAT and capture.
- ECS, job system, core fixed-point primitives, spatial grid and shader build validation.

### Protocol
- Version 12: Ed25519 client identity fields and server `Challenge` packet.
- Version 11: Ed25519 server identity public key/signature in `Welcome`; pinned clients.
- Version 10: handshake carries validated runtime item, hero, ability and map catalogues,
  with a catalogue hash bound into the Welcome HMAC.
- Version 9: handshake carries a validated runtime item catalogue.
- Version 8: ready-up and surrender vote commands, lobby ready/player masks.
- Version 7: read-only spectator handshake/slots and full-map observation; spectator welcome
  flag.
- Version 6: optional authenticated, encrypted transport (PSK handshake + ChaCha20-Poly1305
  payloads); handshake auth fields; reconnect token; snapshots carry phase/countdown,
  team scores, outcome, hero pick and per-hero progression; shop intents are one-shot
  commands applied immediately by the server.

### Known limitations
- No encryption/authentication yet; trusted networks only.
- Content is compile-time; runtime content packs are planned for M8.
- Lobby/rematch/reconnect flow, minimap, floating health bars and line-of-sight fog remain.
- Windows/macOS builds are CI-checked but rendering has been validated primarily on Linux
  (lavapipe); min-spec hardware timings are not yet recorded.
