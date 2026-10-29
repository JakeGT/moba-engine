# Content authoring

Content is **runtime TOML**. The server validates a pack once, hashes it, publishes the
catalogue in the handshake, and the simulation/replays use the same definitions. Omitted
sections fall back to the engine's default pack. Example packs:

- [`content/example-pack.toml`](../content/example-pack.toml) — items + one hero
- [`content/ability-pack.toml`](../content/ability-pack.toml) — all 10 ability kinds
- [`content/map-pack.toml`](../content/map-pack.toml) — one-lane map + vision wall
- [`content/sentinel-pack.toml`](../content/sentinel-pack.toml) — `[assets]` manifest and a hero `model` with a [material](#materials-and-textures)
- [`content/ember-map.toml`](../content/ember-map.toml) and
  [`content/frost-map.toml`](../content/frost-map.toml) — maps with their own
  [visuals descriptors](#map-visuals)

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
| Map | one blue and one red nexus; `extent` 1..=256; every coordinate a whole number in 0..=`extent`; `name` ≤32 ASCII bytes, lane `name` 1..=16 ASCII bytes |
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
existing primitives and modifiers is data-only; targeting and effect selection remain code-defined.

## Publisher attestation

Packs may carry an optional top-level Ed25519 attestation:

```toml
publisher = "<64 hex chars>"
signature = "<128 hex chars>"
```

Sign with `MOBA_SIGNING_SEED=<64 hex> cargo run -p moba-tools --bin content-sign -- pack.toml`
and paste the printed fields **before any TOML table**. Verify with:

```bash
cargo run -p moba-tools --bin content-check -- --require-attestation --publisher <pubkey-hex> pack.toml
cargo run -p moba-server -- --listen 127.0.0.1:7777 --items pack.toml \
  --require-content-attestation --content-publisher <pubkey-hex>
```

The signature covers name/version/author plus the content hash (attestation fields are not
hashed, so signing does not change the catalogue hash). Unsigned packs still load for local
development unless `--require-content-attestation` is set. Trust is pinned per server key;
rotate by restarting with the new key. Revocation is out of band (remove the key from the
server config); there is no online revocation list.

## Rules

```toml
[rules]
team_size = 5
bot_fill = true
hero_respawn_ticks = 300
respawn_per_level = 0
wave_interval_ticks = 900
first_wave_ticks = 30
level_cap = 15
xp_base = 180
xp_per_level = 100
melee_vision = 9.0
ranged_vision = 10.0
tower_vision = 13.0
nexus_vision = 14.0
starting_gold = 500
gold_per_interval = 1
gold_tick_interval = 15
xp_radius = 30.0
shop_radius = 30.0
shared_xp_percent = 130
structure_heroes_first = false
nexus_win = true
score_limit = 0

[rules.rewards.melee_minion]
xp = 32
gold = 20

[rules.rewards.ranged_minion]
xp = 45
gold = 25

[rules.rewards.tower]
xp = 220
gold = 250

[rules.rewards.nexus]
xp = 300
gold = 500

[rules.rewards.hero]
xp = 180
gold = 300

[rules.melee_minion]
max_health = 320
attack_damage = 14
attack_range = 2.0
attack_interval_ticks = 30
move_speed_per_sec = 4.0
armor = 10
magic_resist = 5
vision_range = 9.0

[rules.ranged_minion]
max_health = 220
attack_damage = 20
attack_range = 7.0
attack_interval_ticks = 32
move_speed_per_sec = 4.0
armor = 5
magic_resist = 5
vision_range = 10.0

[rules.tower]
max_health = 2200
attack_damage = 105
attack_range = 10.0
attack_interval_ticks = 30
armor = 40
magic_resist = 40
vision_range = 13.0

[rules.nexus]
max_health = 4800
attack_damage = 85
attack_range = 9.0
attack_interval_ticks = 30
armor = 30
magic_resist = 30
vision_range = 14.0
```

`[map] theme` selects a built-in visual palette; unknown themes fail validation. All rules are optional (defaults shown), validated and hashed/carried in the handshake.
`team_size` is 1..=5; `bot_fill = false` leaves empty player slots empty. Example packs:
`content/1v1-mid.toml`, `content/aram.toml`; both are exercised by
`cargo test -p moba-sim --test mode_packs`. Servers rotate packs when repeated `--items`
paths are supplied (empty-lobby rotation). Minion/tower combat stat blocks are exposed under
`[rules.melee_minion]`, `[rules.ranged_minion]`, `[rules.tower]` and `[rules.nexus]`.

Every hashed `[rules]` field is carried in the handshake (since protocol **v38**): the economy fields
(`starting_gold`, `gold_per_interval`, `gold_tick_interval`, `xp_radius`, `shop_radius`,
`shared_xp_percent`, `[rules.rewards.*]`) and the win-condition fields (`structure_heroes_first`,
`nexus_win`, `score_limit`) reach a joining client, so its rules match the server's instead of
falling back to defaults. `content/economy-override.toml` is the regression pack;
`cargo test -p moba-net --test content` joins a real UDP client against it and compares the
client's rules and content hash with the server's. Adding a `RulesSpec` field fails to compile
until it is hashed (`feed_catalogue` in `crates/game/src/content/hash.rs`) and carried (`impl From<&GameContent> for WireCatalog` in `crates/net/src/protocol/catalog_from.rs`),
and the per-field parity test in `crates/net/src/protocol/tests.rs` fails until the wire conversion
round-trips it.

## Maps

```toml
[map]
name = "1v1 Mid"
theme = "stone"        # grass | stone | snow | desert | arcane
extent = 128
blue_spawn = [16, 64]
red_spawn = [112, 64]

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

### Map coordinate rules

Map geometry crosses the network as whole world units (`extent` as `u16`; spawns, lane
waypoints, structure positions and wall endpoints as `i16` pairs), and the client recomputes
the content hash from what it receives. `GameContent::validate` therefore requires:

- `extent` and every coordinate to be a whole number — `ContentError::BadMapCoordinate(field)`
  names the offending field (`map.extent`, `map.blue_spawn`, `map.red_spawn`,
  `map.lanes.path`, `map.structures.position`, `map.walls.a`, `map.walls.b`);
- `extent` in 1..=256 (`BadMap`) and every point inside 0..=`extent` on both axes
  (`BadMapData`, or `BadWalls` for wall endpoints);
- at most 4 lanes with 1..=16 waypoints each and at most 32 structures (`BadMap`), and a map
  `name` of ≤32 ASCII bytes and lane names of 1..=16 ASCII bytes (`BadMapData`), so nothing
  is truncated on the wire.

TOML packs can only express integer coordinates, so these rules matter mostly for content
built in code (`MapSpec::wire_point` / `MapSpec::wire_extent` give the exact wire value).
`NetServer::bind_with_content` validates its pack and returns `InvalidInput` for one that
fails, and clients validate the catalogue in both the Welcome and `ContentUpdate`.

## Assets

A pack can name visual assets: a hero `model` and a map `visuals`. The names are declared in an
optional `[assets]` manifest that maps each name to a file in the pack
([`content/sentinel-pack.toml`](../content/sentinel-pack.toml) is the example):

```toml
[[assets.units]]           # hero models
name = "knight"
path = "models/knight.glb"
fallback = "sentinel"      # optional; another [[assets.units]] name

[[assets.units]]
name = "sentinel"
path = "models/sentinel.glb"

[[assets.maps]]            # per-map visuals (a descriptor, see "Map visuals" below)
name = "lava"
path = "maps/lava.toml"

[[heroes]]
name = "Knight"
# ...stats...
model = "knight"           # optional; omitted draws the procedural rig

[map]
theme = "stone"
visuals = "lava"           # optional; defaults to the theme's name, e.g. "map-stone"
# ...geometry...
```

**Names.** `model` and `visuals` are 1..=16 of `[A-Za-z0-9_-]`: names, never paths. Every
`model` must name an `[[assets.units]]` entry and every explicit `visuals` an
`[[assets.maps]]` entry of the same pack; the load fails otherwise. An omitted `visuals` is the
theme's built-in name (`map-grass`, `map-stone`, `map-snow`, `map-desert`, `map-arcane`) and
needs no entry. Writing the theme's own name explicitly is the same content (and hash) as
omitting it, but then it must be declared. Manifest entry names are unique within each list.
A `fallback` must name an entry in the same list, and chains may not loop or exceed 16 steps.

**Paths** are relative to the pack file's directory: ASCII, no leading `/`, no `.` or `..`
component, and no `\`, `:` (drive letters, alternate streams) or control characters, so a path
means the same thing on every OS and can never leave the pack. `content-check` also checks
every manifest entry's file. It must exist as a regular file and not resolve outside the pack
directory through a symlink. A unit entry must end in `.glb` (the only mesh format the engine
loads, see [ADR-0007](adr/0007-mesh-format.md)) and parse with the bounded loader. A map entry
must end in `.toml` and be a valid [map visuals descriptor](#map-visuals), and every mesh the
descriptor names must pass the unit checks at `models/<mesh>.glb` in the pack directory.
Entries nothing references are checked too. The asset count and any errors appear in both
outputs:

```text
$ cargo run -p moba-tools --bin content-check -- --json content/sentinel-pack.toml
{"file":"content/sentinel-pack.toml",...,"assets":1,...,"valid":true,"warnings":0,"errors":[]}
```

A pack that fails to load (unknown name, bad path, fallback cycle, TOML error) prints
`{"file":...,"valid":false,"errors":["..."]}`. A pack that loads but has an unusable file
prints the full summary with `"valid":false` and one error per file. Both exit with status 1.

**What travels, and what is hashed.** Network clients never see pack files: they receive
only the catalogue in the handshake. The `model` and `visuals` **names** are part of the
content hash, the publisher signature and the catalogue (protocol v40), because they are part
of the pack's identity and every client should ask for the same visual. The `[assets]`
manifest (paths and fallbacks) is pack-local. It is not hashed, not signed and never sent, so
moving a file inside a pack changes nothing on the wire. Packs that name no visuals hash
exactly as before #285, so their replays and signatures stay valid.

**Resolution is local and cosmetic.** A client resolves a model name against its own asset
root (`MOBA_ASSET_ROOT`, default `content`) as `<root>/models/<name>.glb`. Lay unit files out
as `models/<name>.glb` so the pack directory doubles as an asset root. A missing, unreadable or
malformed file is logged once and that hero keeps the procedural rig; a missing asset never
fails a match. Two players with different local files see different meshes and the same
match. Visuals never affect the simulation, the state hash or server authority. The client
override `MOBA_HERO_MODELS=hero=model,...` (dev only) replaces the catalogue's model for the
heroes it matches. Map visuals resolve the same way, to a descriptor (next section).

### Map visuals

A map's visuals name (its explicit `visuals`, else the theme's `map-<theme>`) resolves on each
client to a small TOML **descriptor** at `<root>/maps/<name>.toml` (#287). Lay map entries out
as `maps/<name>.toml` so the pack directory doubles as an asset root.
[`content/maps/ember.toml`](../content/maps/ember.toml) and
[`content/maps/frost.toml`](../content/maps/frost.toml) are the examples:

```toml
terrain = [0.24, 0.09, 0.05]   # required: terrain tint, linear RGB, each 0..=1
wall = [0.55, 0.20, 0.08]      # required: wall colour
wall_height = 6.0              # optional: metres, 0.5..=32 (default 4)
wall_mesh = "rampart"          # optional: <root>/models/rampart.glb along every wall

[[props]]                      # optional decoration, at most 32
position = [96, 72]            # required: world units, the pack's map coordinates
color = [1.0, 0.42, 0.10]      # required: linear RGB, each 0..=1
height = 11.0                  # optional: metres, 0.1..=32 (default 3)
yaw = 45.0                     # optional: degrees about the vertical axis, -360..=360
mesh = "crystal"               # optional: <root>/models/crystal.glb, else a built-in block
```

- **Walls** are the map's `[[map.walls]]` segments, the same data the server sends, so the
  local preview and a networked client draw the same walls. Each is the built-in block
  (2.5 m thick, `wall_height` tall) or, with `wall_mesh`, that mesh fitted to the same box: a
  wall mesh is authored along +X and stretched to the segment's length.
- **Props** are cosmetic only. They never block movement or vision; walls do the gameplay. A
  prop mesh is scaled uniformly to `height`, standing on the ground and centred on `position`.
  Meshes are static (a skin is ignored).
- **Mesh names** are 1..=32 of `[A-Za-z0-9_-]`, and a descriptor may reference at most 4
  distinct meshes (wall and props together). A client uploads at most 16 distinct map meshes
  per session (renderer mesh slots are shared with hero models and never freed); later ones
  draw the built-in block.
- **Limits.** A descriptor is at most 16 KiB. Unknown keys, wrong types and out-of-range or
  non-finite numbers are errors. The client reads it with a bounded read and never panics on
  it.

**Fallback.** A client logs each problem once per name and never fails a match:

| Problem | What the client draws |
|---|---|
| no descriptor for a theme's built-in name (`map-grass`, ...) | the built-in theme, silently (the default case) |
| a missing, oversized or invalid descriptor | the built-in theme of the map's `theme`, exactly as before #287 |
| a missing, malformed or unuploadable wall or prop mesh | the built-in block for that wall or prop, in the descriptor's colours |

The default packs ship no `map-*` descriptors, so the built-in themes and the default golden
image are unchanged. A shipped descriptor for a built-in name (for example
`maps/map-stone.toml`) restyles every pack that uses that theme without a `visuals` name.

**Switching maps.** Descriptors and meshes are read only when the map changes: at startup, on
a `ContentUpdate` (hot reload) and on map rotation. Each name is loaded once per session, so
rotating back to a map touches neither the disk nor the GPU. A live `Playing` match is never
restyled. To see two maps back to back in one client session, rotate packs on the server
(`--items content/ember-map.toml --items content/frost-map.toml`) or hot-reload one with
`--reload-content`. `MOBA_CAPTURE_MAPS=<prefix>` (network mode) writes `<prefix>-<n>.ppm` after
`MOBA_CAPTURE_FRAME` frames on each map and keeps playing; see
[screenshots](screenshots.md#per-map-visuals).

**Nothing about a descriptor is hashed or sent.** Only the `visuals` name is (see above).
Descriptors are client-local and cosmetic like models: two players with different files see
different colours and props in the same match.

### Animation sets

A skinned model can have an **animation set**: a small TOML descriptor at
`<root>/animations/<model>.toml` that names the model's clips, how they blend and which
events play them ([ADR-0012](adr/0012-animation-runtime.md)).
[`content/animations/sentinel-skinned.toml`](../content/animations/sentinel-skinned.toml) is
the example for `content/models/sentinel-skinned.glb`. Like map visuals it is client-local and
cosmetic: never hashed, signed, sent or replayed.

**Clips** come from the `.glb`: every glTF animation that drives a joint is one clip, named by
the animation's `name` (at most 32 clips, 4,096 keys per track, 65,536 per clip, 262,144 per
model and 8,192 animation channels per file, enough for 64 joints × translation, rotation and
scale × 32 clips). Joints are named by their nodes' `name`s. Names the descriptor uses are 1..=32 of
`[A-Za-z0-9_-]`, matched ignoring ASCII case; other names in the file are ignored. Export one
named glTF animation per action.

```toml
move_speed = 1.5           # optional: m/s above which upper-body actions leave the legs to
                           #   locomotion (0.05..=100, default 1.5)
upper_body = "chest"       # optional: this joint and everything below it is the upper body

[[locomotion]]             # optional blend space, 1..=8 points, speeds increasing (0..=100 m/s)
clip = "idle"
speed = 0.0

[[locomotion]]
clip = "run"
speed = 4.0                # the run clip has full weight from 4 m/s

[[states]]                 # at most 16 action states
name = "attack"            # required, unique
clip = "attack"            # required: a clip of the model
mode = "once"              # once (default) | loop | hold (stays until respawn: death)
layer = "upper"            # full (default) | upper (upper_body only while moving) | additive
fade_in = 0.06             # seconds, 0..=2 (default 0.1)
fade_out = 0.15            # seconds, 0..=2 (default 0.15)
speed = 1.0                # playback rate, 0.1..=4 (default 1)
release = 0.35             # seconds into the clip that the trigger marks: the hit or the
                           #   release of a cast (0..=10, default 0)
interrupt_on_move = true   # stop (fade out) when the unit moves faster than move_speed

[triggers]                 # which state each event plays (all optional)
attack = "attack"          # basic attacks
casts = ["cast1", "cast2", "cast3"]   # ability slots Q, W, E
death = "death"
death_linger = 1.5         # seconds the body stays after the death clip (0..=30, default 1.5)
recall = "recall"
hit = "hit"                # taking a hit (use an additive state)

[[abilities]]              # optional, at most 16: by catalogue ability name, wins over the slot
name = "Fireball"
state = "cast2"

[[springs]]                # optional secondary motion, at most 4 chains / 16 joints
joints = ["cape_0", "cape_1", "cape_2"]   # each the child of the previous, at most 8
stiffness = 0.25           # 0..=1 pull towards the animated pose per 1/60 s step (default 0.2)
damping = 0.2              # 0..=1 velocity loss per step (default 0.15)
gravity = 9.81             # m/s², 0..=50 (default 9.81)

[[sockets]]                # optional attach points for weapons and VFX, at most 8
name = "spear_tip"         # unique
joint = "arm_r"
offset = [0.09, 1.3, 0.21] # metres in the joint's frame (-100..=100)
rotation = [0.0, 0.0, 0.0] # degrees about X, Y, Z (-360..=360)
```

TOML puts every key after a `[[table]]` header into that table, so keep `move_speed` and
`upper_body` above the first header.

- **Timing.** A trigger is stamped with the server tick it happened on, and the state starts
  `release` seconds before it, so the hit frame of an attack lands on the hit and a cast's
  release on the cast. Crossfades use `fade_in` of the new state; the previous one fades out
  underneath. An `upper` state plays on the whole body while the unit stands and only on
  `upper_body` while it moves, so a hero casts while running.
- **What triggers what.** Casts come from the server's cast events (by slot, or by ability
  name with `[[abilities]]`), deaths from death events or the dead flag, respawns from the dead
  flag clearing, hit reactions from the hero's health dropping (at most every 0.3 s). Basic
  attacks come from the server's attack events (protocol v41): the `attack` state's `release`
  lands on the tick the hit landed, and the next swing is started one attack interval ahead
  so its wind-up plays in full (it fades out if the hero stops attacking). An attack faster
  than the clip speeds the clip up to fit. The simulation has no recall, so `recall` is
  reserved.
- **Limits.** A set is at most 16 KiB. Unknown keys, wrong types, out-of-range or non-finite
  numbers and names that the model does not have are errors.

**Fallback.** Any problem is logged once per model and never fails a match:

| Problem | What the client plays |
|---|---|
| no `animations/<model>.toml` | the WP-4.1 pose: rest pose blended to the clip named `run` (or `walk`, else the first clip) by gait |
| an oversized or invalid set, or one naming a missing clip, joint or state | the same WP-4.1 pose |
| `MOBA_NO_ANIMATION_SETS=1` | the same, for every model |

`MOBA_ANIM_LOD` reduces how often far heroes animate: `<near>,<far>` metres from the camera
(default `80,140`: every frame within 80 m, every 2nd frame to 140 m, every 4th beyond or off
screen), or `off`.

### Materials and textures

A hero model can have a **material** (ADR-0010): a shader permutation, a parameter block and a
texture set. A model named `sentinel` uses `<root>/materials/sentinel.toml` if the file
exists; without one it keeps the vertex-colour look. Like map visuals and animation sets,
materials are client-local and cosmetic: they are not part of the content hash, the signature
or the wire, and two players with different files see different shading in the same match.
[`content/materials/sentinel.toml`](../content/materials/sentinel.toml) is the example:

```toml
shader = "textured"                  # "vertex-color" (the default look) or "textured"
base_color = "sentinel-base"         # <root>/textures/sentinel-base.ktx2
normal = "sentinel-normal"           # optional
metallic_roughness = "sentinel-mr"   # optional (read by R2's PBR)
mask = "sentinel-mask"               # optional: where the unit's team colour tints
base_color_factor = [1.0, 1.0, 1.0, 1.0]   # optional, linear RGBA, 0..=1 (default 1)
metallic = 0.0                       # optional, 0..=1 (default 0)
roughness = 1.0                      # optional, 0..=1 (default 1)
normal_scale = 1.0                   # optional, 0..=2 (default 1)
team_tint = 1.0                      # optional, 0..=1 (default 1)
emissive = [0.0, 0.0, 0.0]           # optional, linear RGB, 0..=16 (default 0; R2)
```

The textured permutation draws `base colour × base_color_factor`, multiplied by the unit's
team colour where the mask is set (scaled by `team_tint`), with the normal map applied. Keep
team-coloured areas large and readable: the mask is how a textured hero still reads as blue
or red at gameplay zoom. A `vertex-color` material takes no textures. A textured one needs a
`base_color`, and the model needs texture coordinates (`TEXCOORD_0` in the `.glb`).

**Textures** are KTX 2.0 files at `<root>/textures/<name>.ktx2`; names follow the model-name
rules (1..=32 of `[A-Za-z0-9_-]`). Each slot takes only its own formats, so sRGB and linear
data cannot be mixed up:

| Slot | Bake with | Format (8-bit fallback) | Encoding |
|---|---|---|---|
| `base_color` | `--kind color` | `bc7-srgb` (`rgba8-srgb`) | sRGB |
| `normal` | `--kind normal` | `bc5` (`rg8`) | linear tangent-space X/Y, +Y up |
| `metallic_roughness` | `--kind data` | `bc5` (`rg8`) | linear, R metallic, G roughness |
| `mask` | `--kind mask` | `bc4` (`r8`) | linear, R |

Bake them from PNG with mips:

```bash
cargo run --release -p moba-tools --bin texture-bake -- \
  bake art/knight-base.png content/textures/knight-base.ktx2 --kind color
cargo run --release -p moba-tools --bin texture-bake -- \
  bake art/knight-normal.png content/textures/knight-normal.ktx2 --kind normal
cargo run --release -p moba-tools --bin texture-bake -- info content/textures/knight-base.ktx2
```

`--format` picks the 8-bit fallback instead (`--format rg8`), and `--no-mips` writes level 0
only. The baker filters colour mips in linear light and renormalises normal mips, reports the
level-0 PSNR of BC output, and reloads every file through the runtime loader before writing
it. Its BC7 encoder is a fast single-mode one, fine for engine development; a production
encoder is an ADR-0010 follow-up. `scripts/bake-sample-textures.sh` regenerates the shipped,
self-generated sample set.

**Limits.** A descriptor is at most 4 KiB; unknown keys, wrong types and out-of-range or
non-finite numbers are errors. A texture is at most 4096×4096, 13 mip levels and 64 MiB,
one 2D image, no supercompression. A renderer holds 64 materials and 256 textures.

**Checks.** `content-check` loads the material next to every `[[assets.units]]` model
(`materials/<stem>.toml`, where `models/<stem>.glb` is the unit path) and every texture it
names, enforces the slot formats, reports each problem as an error, and prints the counts (one
descriptor set per textured material):

```text
materials  1 (1 textured, 1 descriptor sets)
textures   4 (4 bindings, 299 KiB)
```

The JSON output adds `materials`, `textured_materials`, `textures` and `texture_bytes`.

**Fallback.** Any problem logs one `[assets] material for model '<name>': ...` line and keeps
that model on the vertex-colour look: a missing, oversized or invalid descriptor, a missing or
malformed texture, a texture in the wrong slot, a model without UVs, or a renderer limit. A
match never fails because of a material. On GPUs without BC support (or with
`MOBA_COMPRESSED_TEXTURES=0`) BC textures are decoded on the CPU at load and look the same.

## Hot reload

```bash
cargo run -p moba-server -- \
  --listen 127.0.0.1:7777 --seconds 600 \
  --items content/example-pack.toml --reload-content
```

The server checks the pack about once per second. It is loaded, validated and trust-checked
**once per process**, and every match (`--matches N`) adopts the same pack in its next
Lobby/Results phase. Connected clients receive a
`ContentUpdate` packet with the full validated catalogue and replace catalogue-dependent state
atomically; a live `Playing` match is never hot-swapped.

Replace the pack atomically, so the server never reads a half-written file:

```bash
cp new-pack.toml content/.example-pack.toml.tmp   # same directory/filesystem
mv content/.example-pack.toml.tmp content/example-pack.toml   # rename(2) is atomic
```

A pack that fails to load (for example a torn in-place write) is logged and ignored; the
running pack stays in use until the file changes again.

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
