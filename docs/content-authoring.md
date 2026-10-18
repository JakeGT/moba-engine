# Content authoring

Content is **runtime TOML**. The server validates a pack once, hashes it, publishes the
catalogue in the handshake, and the simulation/replays use the same definitions. Omitted
sections fall back to the engine's default pack. Example packs:

- [`content/example-pack.toml`](../content/example-pack.toml) — items + one hero
- [`content/ability-pack.toml`](../content/ability-pack.toml) — all 10 ability kinds
- [`content/map-pack.toml`](../content/map-pack.toml) — one-lane map + vision wall

Validate and inspect a pack:

```bash
cargo run -p moba-tools --bin content-check -- content/example-pack.toml
cargo run -p moba-tools --bin content-check -- --json content/example-pack.toml
```

## Pack identity

```toml
name = "Example Pack"     # 1..=32 ASCII bytes
version = "1.0.0"         # 1..=32 ASCII bytes
author = "MOBA Engine"    # 1..=32 ASCII bytes
```

Identity is part of the content hash. Replays record the pack name/version and hash, so a
mismatched or missing pack is rejected.

## Limits

| Element | Limit |
|---|---|
| Items | 16 (`name` 1..=16 ASCII bytes, `cost` > 0) |
| Heroes | 8 (`name` 1..=16 ASCII bytes) |
| Abilities | 24 |
| Lanes | 4, 2..=16 waypoints each |
| Structures/walls | 32 total |
| Map | one blue and one red nexus; coordinates 0..=256 |
| Stats | health/damage/gold ≤10,000,000; item bonuses ≤1,000,000; speed ≤100 u/s; ranges ≤1,000; resist ≤100,000 |

Unknown fields and unknown enum values are **rejected** (no silent fallbacks). Use
`content-check` to catch schema errors before starting a server.

Hero `abilities = [q, w, e]` are ability ids. Slot count is currently three.

## Items

```toml
[[items]]
name = "Ironblade"
cost = 350
attack_damage = 12
# Optional flat stats:
# health, mana, armor, magic_resist, move_speed (world units/s)
```

Purchases apply stats once in `Simulation::buy_item`. The shop draws the runtime catalogue;
number keys buy the visible page and Left/Right arrows paginate. If you add a new stat, extend
`ItemSpec`, the wire item entry, the purchase application and the shop summary together.

## Heroes

```toml
[[heroes]]
name = "Vanguard"
max_health = 800
max_mana = 280
health_regen_per_tick = 2
mana_regen_per_tick = 3
attack_damage = 50
attack_range = 2.0
attack_interval_ticks = 34
move_speed = 6.5
armor = 34
magic_resist = 28
vision_range = 13.0
abilities = [0, 1, 2]
# Optional per-level growth (defaults shown):
# health_per_level = 70
# mana_per_level = 30
# attack_damage_per_level = 4
# armor_per_level = 1
# magic_resist_per_level = 1
```

## Modifiers

Status modifiers are content definitions. A pack that supplies `[[modifiers]]` must define
exactly one entry for each built-in kind (`shield`, `slow`, `haste`, `root`, `silence`,
`burn`); omitting the table uses the six defaults. Names are 1..=16 ASCII bytes and unique.

```toml
[[modifiers]]
name = "Frost"
kind = "slow"
```

Modifier definitions are hashed and carried in the handshake catalogue. Abilities apply a
modifier through an `ApplyModifier` effect, which carries the per-application duration,
movement factor, shield/burn amount and damage type.

## Abilities

Common fields: `name`, `cooldown_ticks`, `mana_cost`, `cast_range`, `kind`.

Abilities are compiled into a `targeting` mode plus a fixed-capacity effect list:
`damage`, `heal`, `apply-modifier`, `displace`, `projectile { on_hit }`, `area` and
`zone`. The `kind` strings below are the authoring shorthand for the same data.

`cast_range` is validated for every kind. Skillshots/dashes must target a point within range;
point-targeted effects (burst/slow/root/silence/burn) accept a target point within range. A
point-targeted `cast_range = 0` is self-centred and `content-check` prints a warning. Kinds:

| Kind | Fields | Behaviour |
|---|---|---|
| `skillshot` | `speed`, `range`, `radius`, `damage`, `damage_type` | projectile along caster→cursor |
| `dash` | `range` | move toward cursor; blocked by root |
| `heal` | `amount`, `radius` | heal self |
| `burst` | `radius`, `damage`, `damage_type` | instant area damage at target |
| `shield` | `amount`, `duration_ticks` | timed damage absorption |
| `slow` | `radius`, `factor` (0..1), `duration_ticks` | area movement slow |
| `root` | `radius`, `duration_ticks` | blocks move commands and dashes |
| `silence` | `radius`, `duration_ticks` | blocks ability casts |
| `burn` | `radius`, `duration_ticks`, `damage` (per tick), `damage_type` | damage over time |
| `haste` | `factor` (>1), `duration_ticks` | self movement boost |

`damage_type` is `"physical"` or `"magic"` (omitted defaults to physical). Status effects
restore movement/protection on expiry and respawn. Slow and haste stack multiplicatively and
expire independently; base speed + item bonuses are preserved.

Composition extras:

| Field | Meaning |
|---|---|
| `arc` | visual mid-flight height for a skillshot projectile (world units) |
| `on_hit = "root"` | skillshot also applies that status on hit |
| `on_hit_duration_ticks` | on-hit status duration (defaults to `duration_ticks`) |
| `kind = "burn_zone"` | point-targeted periodic zone using `radius`, `duration_ticks`, `period_ticks`, `damage`, `damage_type` |

```toml
[[abilities]]
name = "Rooting Shot"
cooldown_ticks = 240
mana_cost = 30
cast_range = 9.0
kind = "skillshot"
speed = 24.0
range = 9.0
radius = 1.0
arc = 2.5
damage = 70
on_hit = "root"
on_hit_duration_ticks = 45

[[abilities]]
name = "Cinder Zone"
cooldown_ticks = 300
mana_cost = 40
cast_range = 6.0
kind = "burn_zone"
radius = 100.0
duration_ticks = 180
period_ticks = 30
damage = 12
damage_type = "magic"
```

Adding a *new effect primitive* still requires engine code and a protocol bump; combining
existing primitives and modifiers is data-only.

## Maps

```toml
[map]
blue_spawn = [32, 128]
red_spawn = [224, 128]

[[map.lanes]]
name = "Mid"
path = [[32, 128], [128, 112], [224, 128]]

[[map.structures]]
team = "blue"          # blue | red
kind = "nexus"         # nexus | tower
position = [20, 128]

[[map.walls]]
a = [110, 100]
b = [110, 156]
```

Walls block line of sight, not movement. The server filters snapshots by vision; the terrain
shader gets the same segments and clips vision circles; the client minimap draws the walls.

## Hot reload

```bash
cargo run -p moba-server -- \
  --listen 127.0.0.1:7777 --seconds 600 \
  --items content/example-pack.toml --reload-content
```

The pack is re-read at most once per second **only when the lobby has no connected clients**;
the new catalogue is used by the next handshake. Connected clients are never hot-swapped
mid-match.

## Replays

Record and verify:

```bash
cargo run -p moba-server -- --listen 127.0.0.1:7777 --seconds 600 \
  --items content/example-pack.toml --record /tmp/match.mbr
cargo run -p moba-tools --bin replay -- --items content/example-pack.toml /tmp/match.mbr
```

Replay format v5 stores the content hash/manifest, resolved inputs (including purchases), state-hash checkpoints every
150 ticks and the final hash; the first divergent checkpoint tick is reported.

## Determinism checklist

- No floats, wall clocks, environment reads or unordered iteration in gameplay paths.
- Break ties by entity id; use fixed-point math for positions/damage.
- Keep new content values in the hash; bump tests deliberately.
- Run `cargo test --workspace`, the allocation guard and a short soak after content changes.
- Keep a pack with any published replay.

## Visual references

See the [screenshot gallery](screenshots.md) for runtime hero/map content and HUD examples.
