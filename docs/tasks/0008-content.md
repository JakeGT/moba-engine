# 0008: Content pipeline (M8)

**Milestone:** M8   **Crates:** game, sim, net, server, client   **Status:** complete (runtime content, updates, attestation, themes)

## Goal
Let creators add and tune game content without forking simulation code, while keeping
deterministic playback and server authority.

## Landed (rounds 14–15)
- `GameContent`/`ItemSpec` owned runtime catalogue with validation (1–16 items, unique 1–16
  byte ASCII names, positive costs).
- `Simulation::new_with_content` reads purchase costs/bonuses from the catalogue; default
  content is derived from the compile-time table.
- `NetServer::bind_with_content` publishes the catalogue in the `Welcome` handshake
  (`WireItem` fixed-capacity encoding); clients use it for shop/inventory labels.
- `moba-server --items FILE` TOML loader, example `content/items.toml`.
- Integration test buys a custom item over UDP and catches progress-only delta omission;
  protocol regression test added.
- Runtime hero catalogue: `HeroSpec` stats and ability bindings, TOML `[[heroes]]`, validation,
  `WireHero` handshake entries for client HUD/vision; simulation spawns/casts from content.
- Runtime ability catalogue: `AbilitySpec` over the engine's skillshot/dash/heal/shield/burst
  kinds,
  TOML `[[abilities]]`, validation, a complete `WireCatalog` handshake projection (`WireAbility` carries kind/range/radius/duration) verified by `GameContent::hash`; a
  custom burst ability is exercised by a simulation test (content/ability-pack.toml).
- Runtime maps: `MapSpec` lanes/spawns/structures/walls, partial TOML `[map]` overlay,
  validation, simulation placement, and `WireMap` lane/wall geometry for the client. Map-only
  pack capture (`../images/m8-map-content.png`).
- Example `content/example-pack.toml` (8 items + Vanguard hero); custom hero UDP test and
  client capture (`../images/m8-hero-content.png`).
- Catalogue integrity: FNV hash in `Welcome`, bound into the PSK HMAC and rechecked by clients.
- Shared TOML loader in `moba-game`; packs carry name/version/author manifests. Replay
  format v3 carries the content hash plus pack name/version, and
  `moba-tools replay --items pack.toml` verifies custom matches. Live 555-tick replay with
  Example Pack 1.0.0 content hash `0x717e1ddd76abfabe` verified; state hashes were re-baselined by the v5/state-hash contract (#58)
  with 3 checkpoints over 555 ticks.

- Shield kind: temporary absorption with duration, snapshot flag and cyan client tint.
- Haste kind: self movement multiplier, stacking with slow and restored on expiry/respawn.
- Burn kind: radius/duration/per-tick damage with state-hash and client flag.
- Silence kind: radius/duration blocking ability casts, with expiry/respawn removal and flag.
- Root kind: radius/duration with move-command and dash blocking, expiry/respawn removal and a flag.
- Slow kind: radius/factor/duration, stacking with haste and restored on expiry/respawn.
- Per-hero level-growth stats in content (health/mana/damage/armour/MR), with defaults.
- Hot reload: `--reload-content` broadcasts a protocol-v24 catalogue update in Lobby/Results;
  connected clients atomically replace items/heroes/abilities/map/vision state and reconnect
  prediction. Live `Playing` matches are never hot-swapped. Tests cover empty-lobby reload,
  mid-match deferral, update round-trip and tamper rejection.

## Completed after the initial pipeline
- Per-client catalogue updates (`ContentUpdate`, protocol v24) in Lobby/Results; no live
  simulation hot-swap.
- Manifest signing/attestation (`content-sign`, `content-check --require-attestation`, server
  pinned-publisher enforcement).
- Per-map visual themes, map names and empty-lobby rotation.
- 1v1/ARAM/mode rules, team size/bot fill, minion/tower stats, vision/XP rules.
- Content-fuzz target and deterministic mode-pack replay tests.

## Future work (not blocking M8)
- Real asset meshes/materials and GPU skinning assets.
- Scripting hooks and more code-driven behaviours (summons, auras, complex triggers).
- Larger content-pack compatibility/version-migration fixtures.

## References
- [Data-driven RPG architecture](https://codesmith.hashnode.dev/architecting-a-data-driven-rpg-in-c)
- [Cocos Creator asset hot-update guide](https://docs.cocos.com/creator/3.8/manual/en/advanced-topics/hot-update.html)

## Publisher attestation

- [x] Optional top-level `publisher`/`signature` Ed25519 fields, `content-sign` tool,
  `content-check --require-attestation/--publisher`, server `--require-content-attestation`
  and pinned `--content-publisher`.
- [x] Attestation covers manifest + content hash and does not alter the catalogue hash.

## Map rotation

- [x] Repeated `--items FILE` (or `items_rotation` config) rotates packs between empty-lobby
  matches; servers log/announce the selected map and content hash.
- [x] Map display name is hashed and carried in protocol v23; client HUD shows it.
- [x] Replays already record content hash/name/version.

## Map visual themes

- [x] `[map] theme` selects a built-in terrain/wall palette (`grass`, `stone`, `snow`, `desert`,
  `arcane`) without recompiling the client; ids are validated and hashed.
- [x] Protocol v25 carries the theme id; renderer uploads a terrain tint and app uses it for
  walls/terrain.
- [x] `MOBA_CONTENT=<pack>` previews a pack in the local client; screenshot gallery includes
  the snow ARAM theme.
