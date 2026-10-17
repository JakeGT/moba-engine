# Changelog

All notable changes are documented here. Format loosely follows Keep a Changelog; the
project is pre-1.0, so minor versions may contain breaking API changes (see
[docs/API-STABILITY.md](docs/API-STABILITY.md)).

## [Unreleased]

### Added
- Runtime item and hero content packs (`--items FILE`), validated and published to clients in
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
- Version 10: handshake carries a validated runtime hero catalogue; runtime hero stat and
  ability-binding packs.
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
