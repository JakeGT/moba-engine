# 0006: Vertical slice (M6)

**Milestone:** M6   **Crates touched:** sim, net, render, client   **Reviewers:** gameplay + UI + netcode

## Goal
A playable 5v5 match from connection to nexus end: legible HUD, fog of war, scoreboard,
levels/items, and a complete match flow.

## Landed in round 5
- **UI renderer** (`moba-render::ui`): screen-space batched quads, alpha blending, a built-in
  5x7 bitmap font (Adafruit-GFX, BSD), no texture/sampler/descriptors, 65,536-vertex budget,
  drawn in the same dynamic-rendering pass after the 3D scene.
- **HUD**: hero name, health and mana bars, Q/W/E cooldown boxes with remaining seconds,
  match timer, team kill score, slot/ping/snapshot telemetry, control legend, dead-hero
  respawn banner, and a local-preview HUD.
- **Scoreboard**: Tab overlay (or `MOBA_SCOREBOARD=1` for captures) listing visible heroes by
  team, slot, hero name and health; enemy rows only appear while server visibility permits.
- **Fog of war rendering**: the client reconstructs allied vision circles from the
  visibility-filtered snapshots and uploads up to 16 circles in the camera UBO; the terrain
  shader darkens everything outside them. This matches the server's distance-based vision
  and adds no packets.
- **Team scores and match outcome**: kill credit (`LastHitBy`) increments blue/red hero
  kills; snapshots carry scores and outcome; the HUD shows VICTORY/DEFEAT.
- **Protocol v2**: snapshots add scores, outcome and hero pick on spawn (+9 bytes/snapshot).

## Landed in round 6
- **Hero levels/XP**: level 1–15 with the League-style rising threshold (280 XP to level 2,
  +100 per level), shared minion/structure XP (130% split among nearby allies), and per-level
  health/mana/damage/armour/magic-resistance growth from a data table (`LEVEL_GROWTH`).
- **Gold and bounties**: last-hit gold for minions/heroes/towers/nexus, passive gold every
  15 ticks, starting gold, and per-team kill credit retained.
- **Items and shop**: eight data-driven items with flat stat bonuses, six inventory slots,
  purchase validation (own nexus radius, gold, free slot), and deterministic bot purchases.
- **Protocol v3 + HUD**: level/XP/gold/inventory travel in snapshots (gold/inventory are
  private to the owning client), HUD shows level, XP bar, gold and item slots; B opens a
  shop with number-key purchases.
- **Extensibility**: item catalogue, level growth, XP curve and economy constants live in
  `moba-game`; the simulation only applies them through small helpers, so new items or
  balance changes do not alter the core tick loop.

## Landed in round 8
- **Match flow**: authoritative `Lobby → Countdown → Playing → Results → rematch` state
  machine in `moba-net`; snapshots carry phase/countdown; a rematch derives a new seed,
  resets client snapshot baselines/net ids and sends a full snapshot while connections stay up.
- **Match overlays**: waiting, countdown, victory/defeat and next-match countdown HUD.
- **Minimap**: lane paths plus visible unit dots, own hero highlighted, derived from the same
  visibility-filtered snapshots.
- **Floating health bars**: world→screen projected bars above visible heroes and structures,
  with team-coloured borders and an own-hero marker.
- **Protocol v4** for phase/countdown; server `--countdown/--results` tuning flags.
- Server match-flow test covers countdown, nexus end, results timer, rematch reset and the new
  seed; client capture verifies waiting/countdown/playing overlays and minimap.

## Landed in round 12
- **Protocol v7**: spectator handshake flag/slot and welcome flag; peek into a spectator-selected
  all-map observation path.
- **Protocol v8 ready/surrender**: `Ready { ready }` gates the countdown when
  `ServerConfig::require_ready` is set; snapshot ready/player masks drive the lobby HUD.
  `Surrender` is a deterministic simulation command and is recorded in replays; the server
  requires every connected human on a team to vote before it takes effect.
- **Spectator mode**: read-only connections in extra server slots receive all-team snapshots
  (no fog), zeroed private gold/inventory, cannot send inputs, and render with a free overview
  camera, minimap and scoreboard. `MOBA_SPECTATE=1` client mode; handshake/protocol v7.
- Spectator integration test verifies both teams and more than the 26 base entities are
  visible without a player slot.
- Ready/surrender integration test starts in Lobby, sends `Ready`, reaches Playing and gets a
  red win after a blue surrender. Capture: `../images/m6-ready-lobby.png`.

## Landed in round 9
- **Reconnect**: disconnected slots are reserved for a configurable grace period (60 s
  default) while bots cover; clients resume with their welcome session token, rebind the
  address, keep the same slot/hero and receive a full snapshot. Input sequence and snapshot
  baselines reset safely; net ids are retained. Covered by a real-UDP reconnect test.
- **Handshake v5**: `Hello` carries an optional reconnect token; protocol version bumped.
- **Server tuning**: `client_timeout`/`reconnect_grace` are configuration fields.
- **Protocol v5**: `Hello` carries an optional reconnect token.

## Closeout
- Hero levels/XP, gold/items/shop, lobby/rematch, reconnect, spectator, ready/surrender,
  minimap, health bars and line-of-sight walls all landed.
- Deferred beyond M6: audio, settings, richer targeting affordances.

## Acceptance
- [x] HUD renders under lavapipe with validation enabled.
- [x] Fog circles align with server visibility; unseen units are absent and unseen terrain is dark.
- [x] Tab scoreboard lists allied heroes and hides unseen enemies.
- [x] Kill credit increments the correct team; outcome travels in snapshots.
- [x] 216 workspace tests; protocol/delta tests cover scores, outcome, phase, hero pick,
      progression and reconnect handshake; UDP tests cover shop purchase and slot resumption.
- [x] Levels/XP, gold/items/shop and lobby-to-rematch flow.
- [x] Ready-up/surrender, spectator mode and reconnect grace.
- [x] Wall line-of-sight filtering on the server and per-pixel wall shadows in the terrain
      shader, with geometry rendered in the same centered world space.

## Research notes
- Small pixel text: a bitmap font is cheaper and crisper than SDF at fixed HUD scales
  ([Sascha Willems' Vulkan text sample](https://github.com/SaschaWillems/vulkan_slim)); SDF
  becomes worthwhile for arbitrary scale/localisation.
- Dota-style fog is a visibility mask over the terrain derived from team vision
  ([Dota vision simulation](https://github.com/ItsCoolOnMars/dota-vision-simulation)); this
  slice uses the same authoritative vision radii as the server instead of a second grid.
- HUD layout follows the MOBA convention: abilities/health bottom-left, timer/score top,
  Tab scoreboard.

## Review log
| Reviewer | Finding | Verdict | Resolution |
|---|---|---|---|
