# 0008: Content pipeline (M8)

**Milestone:** M8   **Crates:** game, sim, net, server, client   **Status:** in progress

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
  Example Pack 1.0.0 verified at content `0xbb6dc5bd0e5ab91a` / state `0x64844168ba624c84`
  with 3 checkpoints over 554 ticks.

- Shield kind: temporary absorption with duration, snapshot flag and cyan client tint.
- Hot reload: `--reload-content` swaps packs only in an empty lobby; connected clients keep
  their handshake catalogue. Test covers reload with zero clients.

## Next
- Move map/lane/structure definitions into validated content.
- Add more ability behaviour kinds (projectile arcs, buffs, summons) and scripting hooks.
- Per-client content-update packets; manifest signing/attestation.
- Hot-reload for development and a `moba-tools content check` command.
- Asset/model mappings and per-map rendering.

## References
- [Data-driven RPG architecture](https://codesmith.hashnode.dev/architecting-a-data-driven-rpg-in-c)
- [Cocos Creator asset hot-update guide](https://docs.cocos.com/creator/3.8/manual/en/advanced-topics/hot-update.html)
