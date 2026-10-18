# 0008: Content pipeline (M8)

**Milestone:** M8   **Crates:** game, sim, net, server, client   **Status:** in progress (core pipeline landed; see Next)

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
  TOML `[[abilities]]`, validation, `WireAbility` handshake entries for HUD cooldowns; a
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
- Hot reload: `--reload-content` swaps packs only in an empty lobby; connected clients keep
  their handshake catalogue. Test covers reload with zero clients.

## Next
- Per-client content-update packets (connected clients currently keep their handshake pack).
- Manifest signing/attestation for third-party pack distribution.
- Asset/model mappings and per-map rendering.
- Scripting hooks and more code-driven behaviours (projectile arcs, summons, auras).
- Larger content-pack compatibility/fuzz fixtures.

## References
- [Data-driven RPG architecture](https://codesmith.hashnode.dev/architecting-a-data-driven-rpg-in-c)
- [Cocos Creator asset hot-update guide](https://docs.cocos.com/creator/3.8/manual/en/advanced-topics/hot-update.html)
