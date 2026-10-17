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

## Still open for M6
- Hero levels/XP, gold, items and a shop.
- Lobby/start flow, pause/reconnect, end-of-match summary and rematch.
- Terrain line-of-sight blockers in fog (current vision is radius-only, same as the server).
- Minimap, floating health bars, targeting/selection affordances.
- Audio and settings.

## Acceptance
- [x] HUD renders under lavapipe with validation enabled.
- [x] Fog circles align with server visibility; unseen units are absent and unseen terrain is dark.
- [x] Tab scoreboard lists allied heroes and hides unseen enemies.
- [x] Kill credit increments the correct team; outcome travels in snapshots.
- [x] 113 workspace tests; protocol/delta tests cover scores, outcome and hero pick.
- [ ] Levels/items and the full lobby-to-rematch flow.

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
