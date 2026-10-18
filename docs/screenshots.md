# Screenshots

Captures from the Linux client with headless readback (PPM/PNG). Most were taken under Mesa
lavapipe; the renderer captures in M3 were taken with validation required. UI and art are
preview-quality and will evolve.

## Renderer and simulation

### 500 instanced units + terrain
![500 instanced units and terrain](images/m3-500-units.png)

Renderer stress capture at 1920×1080: instanced units, terrain grid, team colours and clear
colour. Validation produced zero messages.

### Simulation slice
![Simulation slice](images/m4-simulation-slice.png)

Headless-produced match rendered by the client, showing lanes, structures, heroes and
projectiles.

## Networked client and HUD

### Networked client
![Networked client](images/m5-network-client.png)

Client connected over UDP with predicted local hero and interpolated remote units.

### HUD and fog of war
![HUD and fog of war](images/m6-hud-fog.png)

Health/mana/XP, ability cooldowns, items, timer and team score, with terrain fog of war.

### Countdown
![Countdown](images/m6-countdown.png)

Lobby-to-match countdown, ready state and roster.

### Minimap and floating health bars
![Minimap and health bars](images/m6-minimap-healthbars.png)

Minimap with lanes and team dots, plus world-space health bars.

### Scoreboard
![Scoreboard](images/m6-scoreboard.png)

Tab scoreboard with both teams, levels and health.

### Shop
![Shop](images/m6-shop.png)

Runtime item catalogue, costs and affordability; the live shop supports arrow-key paging for
packs with more than eight items.

### Progression
![Progression](images/m6-progression.png)

XP, gold, level and purchased item state after combat.

### Ready lobby
![Ready lobby](images/m6-ready-lobby.png)

Ready-up gating before the countdown starts.

## Vision, status and spectator

### Wall line-of-sight fog
![Wall line-of-sight](images/m6-los-fog.png)

Custom map wall geometry and wall-aware fog: vision circles are clipped where a wall crosses
the source-to-fragment ray.

### Minimap walls and frame diagnostics
![Minimap walls and FPS](images/m6-minimap-fps.png)

Walls rendered on the minimap, the smoothed FPS/frame-time diagnostic, and the full HUD.

### Status icons
![Health-bar status icons](images/m6-status-icons.png)

Health-bar icons for shield, slow, root, silence, burn and haste; units also receive a
status tint.

### Spectator view
![Spectator view](images/m6-spectator.png)

Read-only spectator with both teams visible and no player slot.

### Free-pan spectator camera
![Free-pan spectator camera](images/m6-spectator-camera.png)

Spectator camera panned/zoomed with WASD and arrows; full-map visibility is retained.

## Runtime content

### Custom hero content
![Custom hero content](images/m8-hero-content.png)

Client using a runtime pack's custom Vanguard hero, stats and name.

### Custom map content
![Custom map content](images/m8-map-content.png)

One-lane map pack with custom structures/spawns and minimap geometry.

## Reproducing a capture

The client supports headless readback without changing the renderer:

```bash
MOBA_SERVER=local \
MOBA_SMOKE_FRAMES=240 \
MOBA_CAPTURE=/tmp/frame.ppm \
MOBA_CAPTURE_FRAME=220 \
cargo run -p moba-client
```

For the software Vulkan setup used here, see [software Vulkan](dev/software-vulkan.md).
