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
- Example `content/example-pack.toml` (8 items + Vanguard hero); custom hero UDP test and
  client capture (`../images/m8-hero-content.png`).
- Catalogue integrity: FNV hash in `Welcome`, bound into the PSK HMAC and rechecked by clients.

## Next
- Move hero stats, ability tables and map/lane/structure definitions into validated content.
- Content hash in handshake/replay manifests; reject mismatched packs.
- Hot-reload for development and a `moba-tools content check` command.
- Asset/model mappings and per-map rendering.

## References
- [Data-driven RPG architecture](https://codesmith.hashnode.dev/architecting-a-data-driven-rpg-in-c)
- [Cocos Creator asset hot-update guide](https://docs.cocos.com/creator/3.8/manual/en/advanced-topics/hot-update.html)
