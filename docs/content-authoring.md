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
recall_ticks = 240            # recall channel, ticks (0..=900; 0 disables recall)

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
`nexus_win`, `score_limit`) and the recall channel (`recall_ticks`, since **v42**) reach a
joining client, so its rules match the server's instead of falling back to defaults.
`recall_ticks` is the channel a recall (**B**) runs before the hero teleports to its spawn;
it is cancelled by moving, attacking, stopping, casting, taking damage, a stun or death. At
most 900 ticks (30 s), the longest the client's recall effect plays. It is hashed and signed only when it differs from 240, so packs written before recall keep their
content hash and attestation. `content/economy-override.toml` is the regression pack;
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

### Mesh LODs and per-tier budgets

**LOD files** (R8, [ADR-0018](adr/0018-culling-lods-memory.md)). A model may have up to three
coarser levels in `models/<model>.lod` beside its `.glb`. Bake them with meshoptimizer:

```text
$ cargo run --release -p moba-tools --features mesh-bake --bin mesh-bake -- lod content/models/knight.glb
$ scripts/bake-mesh-lods.sh        # every content/models/*.glb
$ cargo run -p moba-tools --features mesh-bake --bin mesh-bake -- info content/models/knight.glb
```

The default ratios are 50 %, 25 % and 12.5 % of the triangles (`--ratios 0.5,0.25,0.125`); a
level is kept only if it removes at least 20 % of the previous one. The baker (behind the
`mesh-bake` feature, so plain workspace builds never compile meshoptimizer) may collapse
across attribute seams, which flat-shaded meshes need (every edge is a seam), but weights each
collapse by how far it moves the normals and, on a textured mesh, the UVs, so it avoids
smearing the texture. `--protect-uv-seams` locks every UV-seam vertex instead (exact seams,
but a mesh whose faces are all UV islands gets no levels); `--keep-seams` turns seam collapses
off for smooth-shaded meshes; `--lock-border` keeps open borders in place. Each level's error
is measured against the full mesh (and, for a skinned mesh, over poses sampled from its
clips), so a level that folds at a joint is not picked up close. A level is an index list over the model's own
vertices, so one file serves the static mesh, the GPU-skinned mesh (and every bone-LOD
reduced skin) and a VAT minion. The file records the mesh's hash: **re-bake after changing the
`.glb`**. A stale or malformed file is logged (`[lod] ... drawing LOD0 only`) and ignored; a
missing one means no levels. The renderer draws, per unit and view, the coarsest level whose
error projected to the screen is within the tier's tolerance (Low 2 px, Medium 1, High 0.75,
Ultra 0.5); `MOBA_MESH_LOD=off` draws LOD0 everywhere, `MOBA_MESH_LOD=<px>` overrides the
tolerance.

**Budgets.** `content-check` reports every `[[assets.units]]` model against each quality tier
(`moba_assets::TIER_BUDGETS`):

| Tier | Triangles per model | Joints | Texture bytes per model | Texture VRAM | Total VRAM |
|---|---|---|---|---|---|
| Low | 8 k | 32 | 4 MiB | 384 MiB | 1 GiB |
| Medium | 16 k | 48 | 16 MiB | 1 GiB | 2 GiB |
| High | 32 k | 64 | 32 MiB | 2.5 GiB | 4 GiB |
| Ultra | 64 k | 64 | 64 MiB | 4 GiB | 6 GiB |

```text
budgets    model: triangles (LOD0/levels), joints, texture KiB; low medium high ultra
  sentinel: triangles=462/230/114/56 joints=0 texture_kib=299; fits fits fits fits
```

`fits` is within every budget; `lod<N>` means LOD0 is over the triangle budget and the tier
draws baked level N as its finest; `over` is a triangle count no level fixes, or too many
joints or texture bytes. Over a tier is a warning (the model still loads; a texture that does
not fit the tier's texture VRAM uploads without its largest mips, logged as `[vram]`); over
Ultra is an error. `--json` adds a `budgets` array (`model`, `triangles`, `joints`,
`texture_bytes`, `tiers`).

**Load, never mid-match.** Every model, texture and effect a match uses is loaded when the
match (or the local view) starts. The client counts any upload or pipeline build after the
first frame of play (`locked_uploads`, `locked_pipelines` in the frame log and the `[vram]`
line); both must stay 0.

### Map visuals

A map's visuals name (its explicit `visuals`, else the theme's `map-<theme>`) resolves on each
client to a small TOML **descriptor** at `<root>/maps/<name>.toml` (#287). Lay map entries out
as `maps/<name>.toml` so the pack directory doubles as an asset root.
[`content/maps/ember.toml`](../content/maps/ember.toml) and
[`content/maps/frost.toml`](../content/maps/frost.toml) are the examples:

```toml
terrain = [0.24, 0.09, 0.05]   # required: terrain tint, linear RGB, each 0..=1 (or `tint`
                               #   in a [terrain] table, see "Terrain" below)
wall = [0.55, 0.20, 0.08]      # required: wall colour
wall_height = 6.0              # optional: metres, 0.5..=32 (default 4)
wall_mesh = "rampart"          # optional: <root>/models/rampart.glb along every wall

[[props]]                      # optional decoration, at most 32
position = [96, 72]            # required: world units, the pack's map coordinates
color = [1.0, 0.42, 0.10]      # required: linear RGB, each 0..=1
height = 11.0                  # optional: metres, 0.1..=32 (default 3)
yaw = 45.0                     # optional: degrees about the vertical axis, -360..=360
mesh = "crystal"               # optional: <root>/models/crystal.glb, else a built-in block

[[effects]]                    # optional looping ambient effects, at most 16 (ADR-0019)
effect = "heat-vent"           # required: <root>/vfx/heat-vent.toml
position = [96, 72]            # required: world units, the pack's map coordinates
radius = 2.0                   # optional: the effect's radius, 0.1..=20 (default 1)
```

- **Walls** are the map's `[[map.walls]]` segments, the same data the server sends, so the
  local preview and a networked client draw the same walls. Each is the built-in block
  (2.5 m thick, `wall_height` tall) or, with `wall_mesh`, that mesh fitted to the same box: a
  wall mesh is authored along +X and stretched to the segment's length.
- **Ambient effects** play their effect at `position` over and over (each play starts when
  the last one's emission window ends), as a neutral effect: culled by distance, view and the
  tier's budgets like any other. An entry holds at most two live plays (a start waits for the
  older one to end) and never takes one of the last 64 free effect slots, so looping effects
  can never crowd out gameplay events such as enemy telegraphs. Give an ambient effect a
  `duration` of at least half its lifespan (its longest particle or decal) or it pulses;
  `content-check` warns. An ambient effect may not contain a telegraph emitter or decal
  (`content-check` fails it, the client skips it): a neutral loop would draw at full alpha as
  a fake telegraph. The Ember map's heat vents are haze only, so they show on High and Ultra
  and nowhere else. The authoring preview plays none.
- **Props** are cosmetic only. They never block movement or vision; walls do the gameplay. A
  prop mesh is scaled uniformly to `height`, standing on the ground and centred on `position`.
  Meshes are static (a skin is ignored).
- **Mesh names** are 1..=32 of `[A-Za-z0-9_-]`, and a descriptor may reference at most 4
  distinct meshes (wall, props and the `[terrain]` edge mesh together). A client uploads at most 16 distinct map meshes
  per session (renderer mesh slots are shared with hero models and never freed); later ones
  draw the built-in block.
- **Limits.** A descriptor is at most 32 KiB (raised from 16 KiB in R3b for `[[water]]` and
  `[foliage]`). Unknown keys, wrong types and out-of-range or
  non-finite numbers are errors. The client reads it with a bounded read and never panics on
  it.

**Fallback.** A client logs each problem once per name and never fails a match:

| Problem | What the client draws |
|---|---|
| no descriptor for a theme's built-in name (`map-grass`, ...) | the built-in theme, silently (the default case) |
| a missing, oversized or invalid descriptor | the built-in theme of the map's `theme`, exactly as before #287 |
| a missing, malformed or unuploadable wall or prop mesh | the built-in block for that wall or prop, in the descriptor's colours |

A shipped descriptor for a built-in name restyles every pack that uses that theme without a
`visuals` name. The repository ships one, [`content/maps/map-grass.toml`](../content/maps/map-grass.toml):
the built-in grass colours plus a `[lighting]` table with the `meadow` environment, so the
default map (and both golden images) are lit with IBL (R2a; see [Lighting](#lighting)), and
meadow `[terrain]`, `[atmosphere]` and `[fog_of_war]` tables (R3a; see [Terrain](#terrain)).

**Switching maps.** Descriptors and meshes are read only when the map changes: at startup, on
a `ContentUpdate` (hot reload) and on map rotation. Each name is loaded once per session, so
rotating back to a map touches neither the disk nor the GPU. A live `Playing` match is never
restyled. To see two maps back to back in one client session, rotate packs on the server
(`--items content/ember-map.toml --items content/frost-map.toml`) or hot-reload one with
`--reload-content`. `MOBA_CAPTURE_MAPS=<prefix>` (network mode) writes `<prefix>-<n>.ppm` after
`MOBA_CAPTURE_FRAME` frames on each map and keeps playing; see
[screenshots](screenshots.md#per-map-visuals).

Further optional tables restyle the ground and the air: [`[lighting]`](#lighting),
[`[terrain]`](#terrain), [`[atmosphere]`](#atmosphere) and [`[fog_of_war]`](#fog-of-war),
and `content-check` checks the descriptor against the pack's map
([Map consistency checks](#map-consistency-checks)).

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
model = "crystal"          # optional (R4b): a static models/<name>.glb drawn at the socket
scale = 0.1                # optional, 0.01..=10 (default 1); needs `model`
```

A **2D locomotion** space (R4b) gives each `[[locomotion]]` point `x` (strafe) and `y`
(forward), m/s in the unit's facing frame, instead of `speed`; the points must be distinct and
all of one kind (1D or 2D, never mixed). The weights are gradient-band interpolation, exact at
each point. Heroes face their movement, so a 2D space mostly matters for models that strafe;
the shipped sets stay 1D.

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
  than the clip speeds the clip up to fit. `recall` plays from the server's recall events
  (protocol v42): it starts on the tick the channel started (make it a `loop` with
  `interrupt_on_move = true`) and fades out (`fade_out`) on the tick it was cancelled or
  completed; the recall flag in snapshots stands in for a lost start or end event.
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

**Attachments.** A socket with a `model` draws that static model (loaded from
`<root>/models/<model>.glb` like a hero model, once per name) at the socket every frame, in
a brightened tint of the hero's colour, scaled by `scale`. A missing or invalid model is
logged once and the socket draws nothing. On `Low` attachments are skipped for heroes on bone
LOD2 (beyond the far distance or off screen); `MOBA_ATTACHMENTS=off` hides them all.

**Bone LOD** (R4b). Each skinned model also gets two reduced skins at load: LOD1 (at most 2
influences per vertex and 75 % of the joints, spring-chain joints dropped first) and LOD2 (1
influence, at most 4 joints). A dropped joint's vertices follow its nearest kept ancestor.
Heroes beyond the tier's near distance draw LOD1, beyond the far distance or off screen LOD2
(`Low` 40/80 m, `Medium` 60/110 m, `High` 80/140 m, `Ultra` 110/180 m). Pinned captures
always draw LOD0. `MOBA_BONE_LOD=off` keeps the full skin; `MOBA_BONE_LOD=<near>,<far>`
overrides the distances. Rigid parts lose nothing; soft parts (a cape) stiffen to their
parent at distance, so author spring chains as leaves of the hierarchy.

### Vertex animation textures

Minions are drawn from a **vertex-animation texture** (VAT, R4b) when the client has one:
the clips of a skinned model, skinned on the CPU at bake time and stored per vertex and frame,
played in the vertex shader with no bones. A VAT is client-local and cosmetic like everything
in this section. `content/vat/minion.toml` + `minion.ktx2` is the example, baked from
`sentinel-skinned`:

```bash
cargo run -p moba-tools --bin vat-bake -- content/models/sentinel-skinned.glb content/vat/minion \
  --clips idle:loop,run:loop,attack,death --fps 20
```

`--clips` lists the model's clips (`:loop` marks looping ones; at most 16, 4,096 frames in
all); `--fps` (default 20, the shader interpolates between frames), `--model` (the model whose
static mesh is drawn, default the file's stem), `--run-speed` (m/s at which the run clip plays
at its authored rate, default 4), `--attack-release` (seconds into `attack` that land on the
hit, default 0.35) and `--crossfade` (seconds, default 0.15) go into the descriptor.

The baker writes `<out>.ktx2` (one `RGBA16F` level: positions, then normals, rows of at most
4,096 texels) and `<out>.toml`:

```toml
model = "sentinel-skinned"     # models/<model>.glb is the mesh drawn
vertex_count = 974
frame_count = 88
width = 4096
height = 42
fps = 20.0
mesh_hash = "0x87565fec404bbb79"   # FNV-1a of the mesh's positions and indices

[[clips]]                      # at most 16
name = "idle"                  # the client uses idle, run (or walk), attack and death
first = 0
frames = 32
loop = true

[playback]                     # optional; defaults shown
run_speed = 4.0
attack_release = 0.35
crossfade = 0.15
```

Melee minions use `vat/minion-melee`, else `vat/minion`; ranged minions `vat/minion-ranged`,
else `vat/minion`. A descriptor over 8 KiB, with unknown keys, out-of-range values, a texture
of the wrong size or format, or a `mesh_hash` that no longer matches the model (re-export
without a re-bake) is logged once and that kind is drawn as a cube. **Re-bake whenever the
model or its clips change.** Idle and run follow the interpolated speed (the run clip's rate
scales with speed / `run_speed`), the attack's release lands on the server's attack hit and
death plays on a corpse left where the minion died. Tiers: `Low` nearest frame, `Medium`
interpolated frames, `High`/`Ultra` interpolated and crossfaded; all cast shadows.
`MOBA_VAT=off` draws every minion as a cube.

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
metallic_roughness = "sentinel-mr"   # optional: R metallic, G roughness (times the factors)
mask = "sentinel-mask"               # optional: where the unit's team colour tints
base_color_factor = [1.0, 1.0, 1.0, 1.0]   # optional, linear RGBA, 0..=1 (default 1)
metallic = 0.0                       # optional, 0..=1 (default 0)
roughness = 1.0                      # optional, 0..=1 (default 1)
normal_scale = 1.0                   # optional, 0..=2 (default 1)
team_tint = 1.0                      # optional, 0..=1 (default 1)
emissive = [0.0, 0.0, 0.0]           # optional, linear RGB, 0..=16 (default 0), added unlit
# Stylisation (ADR-0011), all optional; they refine the map's [lighting] values:
wrap = 0.3                           # 0..=1 wrap lighting (default: the map's)
rim = 1.0                            # 0..=4, multiplies the map's rim strength (default 1)
ramp = 1.0                           # 0..=1, multiplies the map's ramp strength (default 1)
specular = 1.0                       # 0..=2 specular scale (default 1)
specular_tint = [1.0, 1.0, 1.0]      # linear RGB 0..=1, painted-specular tint (default 1)
```

The textured permutation draws `base colour × base_color_factor`, multiplied by the unit's
team colour where the mask is set (scaled by `team_tint`), lit by the stylised PBR model of
[ADR-0011](adr/0011-art-style-lighting.md): metallic and roughness are the
`metallic_roughness` texture's R and G times `metallic` and `roughness`, and the normal map is
applied in the mesh's tangent frame (`TANGENT` from the `.glb`, else generated from the UVs
at load; `sample-mesh` writes them into the shipped samples). Keep
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

### Lighting

Lighting is art direction, set per map in the visuals descriptor's optional `[lighting]` table
([ADR-0011](adr/0011-art-style-lighting.md)); every key is optional and unset keys keep the
engine defaults. Like the rest of the descriptor it is client-local and never hashed or sent.
[`content/maps/ember.toml`](../content/maps/ember.toml) (a low, hot sun with orange rims) and
[`content/maps/frost.toml`](../content/maps/frost.toml) (a high, cold sun over snow) are the
examples:

```toml
[lighting]
environment = "ember"          # <root>/environments/ember.{toml,ktx2} (IBL); default: none
sun_direction = [-0.6, 0.35, 0.45]  # towards the sun, normalised on load; y >= 0.2
sun_color = [1.0, 0.84, 0.70]  # linear RGB 0..=4 (default [1.0, 0.97, 0.92])
sun_intensity = 1.4            # 0..=8 (default 1.15)
ambient = [0.32, 0.35, 0.42]   # hemispheric sky ambient without an environment, 0..=4
ambient_ground = [0.2, 0.2, 0.3]  # hemispheric ground ambient (default: ambient x 0.55-0.7)
ibl_intensity = 0.3            # environment multiplier, 0..=8 (default 1)
shadow_strength = 0.8          # 0..=0.9: how much shadow removes the sun (default 0.78)
shadow_softness = 0.04         # 0..=0.2: sun angular radius in radians, soft (PCSS) tiers
wrap = 0.3                     # 0..=1 wrap lighting (default 0.2)
rim = 0.45                     # above 0, at most 2: rim strength (default 0.25)
rim_color = [1.0, 0.55, 0.25]  # 0..=4 (default white)
ramp = "ember-ramp"            # optional <root>/textures/<name>.ktx2: an N.L colour ramp
ramp_strength = 0.5            # 0..=1 (default 0: no ramp)
specular = 1.0                 # 0..=2 specular scale
specular_tint = [1.0, 0.8, 0.6]  # 0..=1 painted-specular tint (default white)
unit_roughness = 0.55          # 0..=1, units without a material (default 0.55)
terrain_roughness = 0.9        # 0..=1 (default 0.9)
baked_ao = "ember-ao"          # optional <root>/maps/ao/<name>.{toml,ktx2} (below, ADR-0016)
ao_strength = 0.85             # 0..=1 baked AO strength (default 1)
```

The parser enforces the readability rules a descriptor alone can break, as errors: `rim` above
0, `terrain_roughness` above `unit_roughness` (with the defaults filling in an unset one),
`shadow_strength` at most 0.9, and some ambient light (an `ambient` of zero is rejected
without an environment to light the shadowed side).

**Readability rules** (ADR-0011 §6) are the review checklist for any lighting change: team
colours stay readable in light, shadow and fog; `shadow_strength` stays below 1 so units in
shadow keep their colour; units keep a rim and stay glossier than the terrain; tinted
specular never washes a unit to white; and every quality tier passes the same checks.

**Environments** come from `moba-tools env-bake`, which evaluates an analytic sky (a zenith,
horizon and ground gradient plus a broad sun glow; no sun disk, since the map's sun gives the
direct light and shadows) into a prefiltered specular cube and SH-L2 diffuse coefficients:

```bash
# writes content/environments/meadow.ktx2 and meadow.toml
cargo run --release -p moba-tools --bin env-bake -- bake --preset meadow content/environments/meadow
cargo run --release -p moba-tools --bin env-bake -- bake --preset ember \
  --sun-dir -0.6,0.35,0.45 --horizon 1.0,0.4,0.12 content/environments/my-ember
cargo run --release -p moba-tools --bin env-bake -- info content/environments/meadow.ktx2
```

Presets are `meadow`, `ember` and `frost`; `--zenith`, `--horizon`, `--ground`, `--sun-dir`,
`--sun-color` (linear, may exceed 1) and `--sun-glow <exponent>` override any value, `--size`
(default 64) and `--samples` (default 256) set the cube. The `.ktx2` is a 64² `rgba16f` cube
with a full chain (mip `m` of `M` holds GGX roughness `m / (M - 1)`, 256 KiB); the `.toml`
holds the nine SH coefficients (`sh`, cosine-convolved and divided by π), `intensity`
(0..=16) and `specular_mips` (1..=11), at most 4 KiB with unknown keys rejected, and its
header comments record every sky parameter so a bake is reproducible.
`scripts/bake-sample-environments.sh` regenerates the shipped three. Environment names follow
the model-name rules. No external HDRIs ship (an HDRI would need a CC0 licence and an entry
in `THIRD_PARTY.md`).

**Ramps** are optional 2D colour textures (`rgba8-srgb` or `bc7-srgb`, for example 256×1),
looked up with `u = N·L` remapped from −1..1 to 0..1 and blended over the wrapped diffuse by
`ramp_strength` (times the material's `ramp`): hard bands give a cel look, a warm-to-cool
gradient a painterly terminator.

**Quality tiers** (`quality` in the settings, `MOBA_QUALITY`) pick the shadow filter and size,
whether the specular cube is sampled and (R2b, [ADR-0016](adr/0016-lights-outlines-ao.md)) the
dynamic-light caps, the outline method and contact shadows; `MOBA_SHADOWS=off|hard|pcf|pcss`,
`MOBA_SHADOW_SIZE=<256..8192>`, `MOBA_IBL=0`, `MOBA_SPECULAR_IBL=0`, `MOBA_LIGHTS=off|<cap>`,
`MOBA_OUTLINES=off|hull|screen`, `MOBA_BAKED_AO=off` and `MOBA_CONTACT_SHADOWS=off|<steps>`
override them:

| Tier | Shadows | Shadow map | Specular IBL | Lights (all / per column) | Outlines | Contact shadows |
|---|---|---|---|---|---|---|
| low | hard (1 tap) | 1024² | off (SH ambient only; the cube is not uploaded) | 16 / 8 | hull | off |
| medium | 3×3 PCF | 2048² | on | 64 / 12 | hull | off |
| high | PCSS-lite (16 + 16 taps) | 2048² | on | 128 / 16 | screen | 8 steps |
| ultra | PCSS-lite (16 + 32 taps) | 4096² | on | 256 / 16 | screen | 16 steps |

Baked AO is applied on every tier. Team outlines (allied, enemy and your own hero; a hovered
unit; the unit you last ordered attacked) use the HUD's team palette, so the colour-blind
palette (`team_palette = "colorblind"`) recolours them too. They are drawn only on units the
client draws, and depth-tested: never through a wall, in fog or in brush.

**Fallback.** A missing or invalid environment logs `[assets] environment '<name>': ...` once
and lights the map with the hemispheric ambient; a missing ramp logs once and draws without
it. A match never fails because of lighting.

### Baked AO

Static occluders darken the ambient light around them through a baked top-down ambient
occlusion map ([ADR-0016](adr/0016-lights-outlines-ao.md)): the ground beside walls and props
and the feet of units walking past them. `moba-tools ao-bake` bakes it from a pack's walls
(as the 2.5 m-thick, `wall_height`-high boxes the client draws; a `wall_mesh` is fitted into
the same box, so the box stands in for it) and the visuals descriptor's props (their meshes'
triangles, or the built-in block). Structures are not baked, since they are destroyed in play.

```bash
# writes content/maps/ao/ember-ao.ktx2 and ember-ao.toml
cargo run --release -p moba-tools --bin ao-bake -- bake --pack content/ember-map.toml \
  --root content --visuals ember
# options: --texel 0.5 (metres per texel), --height 3 (falloff), an explicit <out-stem>
scripts/bake-map-ao.sh          # regenerates the shipped ember and frost bakes
```

The `.ktx2` is a square `r8` map of the map's `[0, extent]²` world square (0.5 m texels, at
most 2048², 256 KiB for a 256-unit map; 1 = open), computed as height-field horizon AO (16
directions, 12 m). The `.toml` sidecar records `extent`, `texels`, `height` (the falloff
above the ground, 0.1..=64 m, default 3) and a `fingerprint` of the inputs; it is at most
2 KiB with unknown keys rejected. Name it in the descriptor's `[lighting] baked_ao` (and
optionally `ao_strength`). It multiplies only the ambient line (diffuse and specular
ambient), faded out over `height` above the ground, so it never darkens direct sunlight or
a team colour.

**Re-bake after changing the occluders.** The client recomputes the fingerprint (the map
extent, every wall segment, the wall height and each prop's position, height, yaw, mesh name and
mesh shape, its bounds and vertex and index counts, so editing a prop's model invalidates the
bake too; not colours) for the map it actually binds, and ignores a bake that does not match with
`[assets] baked AO '<name>': stale: ...; no baked AO`. A visuals name shared by two packs with
different walls therefore never shows wrong occlusion; a test fails when the shipped bakes go
stale. A missing or invalid bake logs once and draws without it; `MOBA_BAKED_AO=off` turns it
off. The default grass map has no walls or props, so it ships no bake.

### VFX

Ability, projectile, zone, death and attack effects are GPU particles described by small
client-local TOML files under `<root>/vfx/` ([ADR-0013](adr/0013-vfx-runtime.md)). Like map
visuals, animation sets and materials they are cosmetic: never hashed, signed, sent or
replayed, so two players with different files see different effects in the same match. The
sample set for every shipped ability is in [`content/vfx/`](../content/vfx/).

**Bindings.** `<root>/vfx/bindings.toml` says which effect plays on which event. Abilities
are matched by their catalogue `name` (ASCII case-insensitive); `[defaults]` covers unit
events and any ability without its own binding:

```toml
[defaults]                        # every key optional
cast = "cast-flash"               # an ability is cast
projectile = "bolt"               # carried by the ability's projectile while it flies
hit = "impact"                    # where the projectile hits
zone = "zone"                     # for the zone's whole lifetime
hero_death = "hero-death"
minion_death = "minion-death"
structure_destroyed = "collapse"  # a tower or nexus
attack = "attack-hit"             # a basic attack lands (at the target)
recall = "recall"                 # a hero channels a recall, at the hero, until it ends (its recall flag clears, or it dies)

[[abilities]]                     # at most 128
name = "Arc Bolt"
cast = "cast-flash"
cast_socket = "spear_tip"         # the caster's animation-set socket for `source` emitters
projectile = "arc-bolt"
hit = "arc-hit"
zone = "cinder-zone"
tint = [1.0, 1.0, 1.0]            # colour multiplier for this binding's effects, 0..=4
```

Projectiles, hits and zones carry no ability on the wire; they take the ability of their
owner's most recent cast. A cast socket needs an animated hero (an animation set with that
`[[sockets]]` name); otherwise `source` emitters attach to the caster's mid-height.

**Effects.** `<root>/vfx/<name>.toml` (names are 1..=48 of `[A-Za-z0-9_-]`) holds up to eight
emitters and up to four ground decals (below), at least one of either:

```toml
duration = 0.6            # seconds the effect emits (0..=30)
importance = "high"       # low | normal | high: which effects budgets cull first
event_duration = false    # zones and recalls: emit for the zone's lifetime (the recall channel) instead of `duration`

[[emitters]]
primitive = "sprite"      # sprite | flipbook | trail | beam | ring | mesh
blend = "additive"        # additive | alpha | distortion (heat haze, below)
sprite = "soft"           # an atlas sprite (below)
anchor = "target"         # target | source | line (source to target)
burst = 24                # particles at `delay` (0..=512)
rate = 0                  # particles per second while emitting (0..=1024)
delay = 0.0               # seconds before this emitter starts
stop = 0.4                # optional: stop the rate early
lifetime = [0.3, 0.6]     # required, 0.01..=10 s, low <= high
speed = [2.0, 6.0]        # m/s
direction = "out"         # up | out | random | forward | none
spread = 30               # cone half-angle for up/forward, degrees
shape = "sphere"          # point | sphere | disc | circle
radius = 0.5              # shape radius, metres
scale_with_radius = false # multiply radius and size by the event's radius (zones)
offset = [0, 1, 0]        # metres from the anchor, +Y up
size = [0.4, 1.2]         # required, start size, metres
size_end = 0.2            # size multiplier at the end of life
color = [1.0, 0.6, 0.2, 1.0]      # required, linear RGBA; RGB may exceed 1 (HDR glow, 0..=32)
color_end = [0.8, 0.1, 0.0, 0.0]  # default: `color` with alpha 0
team_tint = 0.0           # pull towards the team colour, 0..=1
gravity = -2.0            # m/s² along +Y
drag = 1.0                # 1/s
spin = [-90, 90]          # degrees per second
width = 0.25              # trails and beams, metres
fps = 0                   # flipbooks: frames per second (0 = spread over the lifetime)
mesh = "crystal"          # mesh emitters: `cube` or <root>/models/<name>.glb
telegraph = false         # an area telegraph (readability rules below)

[light]                   # optional: one dynamic point light (ADR-0016)
color = [1.0, 0.5, 0.2]   # required, linear RGB 0..=4
intensity = 6.0           # 0..=64 multiplier (default 1)
radius = 5.0              # required, 0.25..=24 m: the light reaches zero here
anchor = "target"         # target | source (not line)
offset = [0, 1, 0]        # metres from the anchor (default one metre up)
delay = 0.0               # seconds before it switches on
duration = 0.2            # seconds it stays on (default: while the effect emits)
fade_in = 0.0             # seconds to ramp up
fade_out = 0.2            # seconds to ramp down after it ends (default 0.2)
flicker = 0.3             # 0..=1 random dimming
team_tint = 0.0           # pull towards the team colour, 0..=1
scale_with_radius = false # multiply radius by the event's radius (zones)
```

- **Primitives.** `sprite` and `flipbook` are camera-facing quads (a flipbook plays a
  multi-frame sprite). `trail` draws a ribbon behind a moving anchor (projectiles), one segment
  per frame; `beam` draws a segment from the source to the target every frame while the effect
  emits (dashes); `ring` lies flat on the ground (shockwaves, area telegraphs); `mesh` draws a
  registered mesh tumbling at its spin (at most four distinct meshes over all effects; more, a
  missing model or the Low tier draw a shard billboard instead).
- **Blending.** `additive` adds light and never darkens. `alpha` is weighted blended
  order-independent transparency on Medium and up (no sorting needed) and unsorted
  premultiplied blending on Low. `distortion` is heat haze (below).
- **Heat haze** ([ADR-0019](adr/0019-distortion.md)): a `blend = "distortion"` emitter
  bends the scene behind its sprites instead of adding colour, on High and Ultra only. It
  must be a `sprite` emitter (always an upright billboard) and takes no `color`,
  `color_end`, `team_tint`, `width`, `fps` or `telegraph`; instead:

  ```toml
  [[emitters]]
  blend = "distortion"
  rate = 6
  lifetime = [1.2, 1.6]
  size = [1.4, 1.8]
  distortion = 0.7          # required: strength at birth, 0.05..=1 of the tier's pixel cap
  distortion_end = 0.0      # strength at the end of life, 0..=1 (default 0)
  distortion_scale = 2.0    # noise cells across the sprite, 0.5..=8 (default 2)
  distortion_speed = 1.0    # noise rise, sprite heights per second, 0..=8 (default 1)
  fallback_color = [1.0, 0.4, 0.1, 0.15]  # optional: a faint additive sprite where there is
                                          #   no distortion (RGB 0..=4, alpha 0..=0.5)
  ```

  The offset is rising procedural noise, faded at the sprite's edge, over its first 15 % of
  life, where it meets the scene and with view distance (70–110 m), and never longer than
  the tier's cap: 6 px (High) or 8 px (Ultra) at a 1080-pixel render height, scaled with the
  render height. **Units and telegraph edges are never bent**: their pixels are kept and
  nothing is pulled out of or into a unit's silhouette (the prepass ID target masks them) or
  a telegraph decal's rim and halo bands (marked in the scene's alpha); geometry in front of
  the haze is never pulled into it; particles (telegraph rings included), outlines and the
  HUD draw after it. Allied haze is scaled by `ally_vfx_opacity`; haze is never exempt from
  budgets. On Low, Medium, with `MOBA_DISTORTION=off`, on `MoltenVK` or without screen-space
  outlines the emitter is skipped (no particle, no budget) unless it has a `fallback_color`.
  `MOBA_DISTORTION=nodraw` keeps every haze particle but skips drawing it, to compare a
  capture with and without the refraction alone.
- **Sprites** (the 8×8 atlas layout): `soft`, `dot`, `ring`, `spark`, `star`, `shard`, `rune`,
  `beam`, `noise`, `swirl`, `cross`, `halo`, `solid`, and the flipbooks `burst` (16 frames),
  `smoke` (16) and `lightning` (8). The built-in atlas is procedural; `<root>/vfx/atlas.ktx2`
  replaces it (512×512 or any 8×8 grid; R is coverage, G a hot core that whitens the colour;
  [`scripts/bake-sample-textures.sh`](../scripts/bake-sample-textures.sh) bakes the shipped one
  as BC5 through `texture-bake`).
- **Anchors.** `target` is the event's position: the cast point, the impact, the zone centre,
  the dead unit; for a projectile effect, the flying projectile. `source` follows the caster
  (or its socket). `line` spawns along the segment between them.

**Decals** ([ADR-0017](adr/0017-decals-foliage-water.md)) project straight down onto the
terrain, the water and the foliage (never onto units, walls or props, so a telegraph never
paints the enemy standing in it). An effect may have up to four:

```toml
[[decals]]
shape = "ring"            # disc | ring | cone | line | splat (default disc)
style = "glow"            # glow | paint (default glow; paint for splats)
anchor = "target"         # target | source | line (from the source to the target)
radius = 1.0              # metres, 0.1..=50 (default 2; a line's length unless anchored on a line)
width = 0.3               # ring and line band width, metres, 0.05..=20 (default 0.5)
angle = 90.0              # cone angle, degrees, 1..=360 (default 90)
delay = 0.0               # seconds after the effect starts, 0..=30
lifetime = 1.0            # seconds once shown, 0.05..=30 (default: the effect's emit window)
fade_in = 0.1             # seconds, 0..=10
fade_out = 0.25           # seconds, 0..=10
color = [2.0, 0.4, 0.1, 0.8]   # required, linear RGBA; RGB may exceed 1 (HDR, 0..=32)
team_tint = 0.0           # pull towards the team colour, 0..=1
scale_with_radius = true  # radius and width times the event's radius (zones)
telegraph = true          # an area telegraph (readability rules below)
progress = true           # an inner fill grows to the edge over the decal's life (cast timers)
stamp = false             # persist after the effect (scorch marks), for `lifetime`
```

- **Styles.** `glow` is composited after lighting, fog of war and height fog, so nothing dims
  it: telegraphs and zones. `paint` blends into the ground's colour before lighting, so it is
  shadowed and fogged like the ground: scorch, blood, goo.
- **Lifetime.** Without `lifetime` a decal lasts the effect's emit window (a zone's decal with
  `event_duration` lasts the zone). A **stamp** is copied at its `delay` into a ring of 96
  persistent decals and stays for its `lifetime` (required) after the effect ends; when the
  ring is full the oldest stamp goes. A stamp can be neither a telegraph nor a progress fill.
- **Telegraphs** must use `glow`. An enemy's telegraph decal is never culled, keeps at least
  35 % opacity until the last 15 % of its life, and gets a two-tone contrast rim (a bright
  band and a dark halo just inside it) so it reads on any ground, over water too; on a
  `ring` the band is the ring itself and `progress` fills its inside from the centre. An
  ally's keeps the rim. Decals exist only
  for effects the client plays, so they reveal nothing the server did not send.
- **Tiers.** At most 48 (Low), 96 (Medium) or 128 (High, Ultra) decals a frame, enemy
  telegraphs first and exempt. `MOBA_DECALS=off` draws none (particles still play).

**Readability rules** (AAA roadmap §2) are applied for every viewer:

- An **enemy** effect's `telegraph = true` emitters are never culled by distance, visibility or
  budgets, and keep at least 35 % opacity until the last 15 % of their life (they are drawn
  without the soft-particle fade).
- **Enemy projectiles** (and, for a spectator, every projectile) are exempt from culling and
  budgets the same way: a skillshot is gameplay information. A projectile whose effect does
  not resolve, or that spawns nothing for 0.1 s (an ally's over budget), is drawn as the
  pre-VFX cube, so a projectile never becomes invisible.
- **Allied** effects (a teammate's, not your own) are scaled by the `ally_vfx_opacity` setting
  (`MOBA_VFX_ALLY_OPACITY`, default `1`), and their `alpha` particles are capped at 75 %
  opacity so an ally's smoke never fully hides an enemy behind it.
- **Lights.** An effect's `[light]` lights the ground and units around it (diffuse and
  specular, no shadows; lights add light and never replace a team colour). The falloff is
  floored at 1 m and the lights' sum at a point is clamped to 1.5× the sun, so a unit inside
  a flash brightens without washing out to white. Each tier caps the lights in a frame (Low
  16 … Ultra 256) and per 32×32 grid column (8 … 16; a column is about 4.6 m at the default
  camera, so a 6 m light covers about 3 × 3 of them); enemy and neutral effects' lights are
  listed first, so a cap drops allied lights (scaled by `ally_vfx_opacity`; your own are not)
  before an enemy's. A light belongs to an effect the client already
  plays, so it never reveals anything the client was not sent. `MOBA_LIGHTS=off` turns
  them off.
- **Budgets.** Each quality tier caps live particles (Low 4,096 … Ultra 49,152) and an overdraw
  estimate (particle pixels per screen pixel); as either fills, new `low` effects are dropped
  first (at 50 %), then `normal` (80 %), then `high` (95 %). Effects farther than the tier's
  distance (90–240 m, from the camera's focus and scaled up when the camera is zoomed out past
  46 m) or off screen spawn nothing.

**Limits and fallback.** A descriptor is at most 16 KiB and a bindings file 32 KiB; unknown
keys, wrong types and out-of-range or non-finite numbers are errors (the `vfx` fuzz target
covers both parsers). `content-check` fails the pack when the bindings do not load, or when
an effect the bindings or a map's `[[effects]]` name does not exist or does not load (or, for
a map effect, has a telegraph emitter or decal); it also loads every other `vfx/*.toml` next
to the pack, but one that does not load is a warning (nothing references it). A missing or invalid file is logged once (`[vfx] effect ...: ...; no
effect`) and plays nothing; a match never fails because of an effect. A missing or invalid
`bindings.toml` turns effects off (`[vfx] ...; effects off`), as do `MOBA_VFX=off` and the
`vfx = false` setting; projectiles then draw as cubes.

**Preview.** `MOBA_VFX_PREVIEW=<name> cargo run -p moba-client` opens an empty arena with a
stand-in caster and target and replays `vfx/<name>.toml` as soon as each play ends: a cast at
the target point, a projectile from the caster to the target if the effect has a trail, or a
3 s, 4 m zone if it sets `event_duration`. Saving any file in `vfx/` reloads the effects
within half a second, without restarting.

### Terrain

A map can replace the flat ground with a **heightfield terrain**: gentle relief on the board,
hills or cliffs beyond its edge, and up to eight texture layers blended by a splat map
([ADR-0014](adr/0014-terrain-fog-of-war.md)). It lives in the descriptor's `[terrain]` table
and, like the rest of the descriptor, is client-local: never hashed, signed or sent, and
never seen by the simulation. The shipped examples are
[`content/maps/map-grass.toml`](../content/maps/map-grass.toml) (a meadow basin),
[`content/maps/ember.toml`](../content/maps/ember.toml) (terraced basalt cliffs) and
[`content/maps/frost.toml`](../content/maps/frost.toml) (snow drifts).

A TOML key cannot be both a value and a table, so with a `[terrain]` table the top-level
`terrain = [r, g, b]` tint moves into the table as `tint`:

```toml
wall = [0.55, 0.20, 0.08]        # top-level keys as before, without `terrain = [...]`

[terrain]
tint = [0.24, 0.09, 0.05]        # required: the flat plane's tint (the fallback look)
relief = 0.35                    # board relief amplitude, metres, 0..=0.5 (default 0.25)
edge = "cliffs"                  # "none" | "hills" (default) | "cliffs" outside the board
edge_height = 22.0               # metres, 0..=48 (default 18)
edge_mesh = "rampart"            # optional <root>/models/<name>.glb placed around the edge
edge_spacing = 24.0              # metres between edge props, 8..=128 (default 24)
edge_mesh_height = 10.0          # edge prop height, metres, 1..=48 (default 10)
lane_width = 12.0                # generated path width along lanes, metres, 2..=40 (default 12)
seed = 3                         # u32 noise seed (default 1)
splat = "ember-splat"            # optional <root>/textures/<name>.ktx2: weights of layers 1-4
splat2 = "ember-splat2"          # optional: layers 5-8 (only with splat and > 4 layers)

[[terrain.layers]]               # 1..=8 layers; the position is the role (below)
albedo = "terrain-grass"         # required <root>/textures/<name>.ktx2
normal = "terrain-grass-normal"  # optional <root>/textures/<name>.ktx2
tint = [0.55, 0.75, 0.45]        # linear RGB multiplier, 0..=2 (default 1)
tiling = 6.0                     # metres per texture repeat, 0.5..=64 (default 8)
```

**Layers and roles.** A layer's position in the list is its role:

| Layer | Role | Where the generated splat puts it |
|---|---|---|
| 1 | base ground | everywhere nothing else covers |
| 2 | path | lane corridors (`lane_width` wide along the lane polyline, with a noisy ±2 m edge), 7 m discs around structures and 12 m discs around spawns |
| 3 | rock | slopes (fully rock where the gradient passes ~0.5: the edge hills and cliffs) and the feet of walls (~0.7 within 3 m of a wall segment) |
| 4 | variation | noise patches on the base ground |
| 5-8 | extra variation | sparser noise patches, one seed each |

Rock covers path, and path covers the ground layers. A role without a layer (for example a
two-layer terrain has no rock) folds its weight into the base, so only the listed layers ever
get weight. Albedo textures are sRGB colour (`texture-bake --kind color`: `bc7-srgb`, or
`rgba8-srgb`) with ambient occlusion in alpha, which also drives the height blend between
layers; normal maps are `bc5` or `rg8` (`--kind normal`). All albedo layers must have the
same size and mip count, and so must all normal maps: the renderer assembles them into
texture arrays. A layer without a normal map is drawn flat.

**Splat.** Without `splat`, the client generates the weights at map bind (`moba_assets::
generate_terrain`, 512² by default, about 35 ms with a 384-cell heightfield in a release build) from
the simulation's map alone: lanes, structures, spawns and walls. A baked `splat` is a linear
RGBA texture (`texture-bake --kind data`; `rgba8` or `bc7`) whose channels are the weights of
layers 1-4 over the terrain square (the board plus its margin, texel (0, 0) at the corner
before `[0, 0]`, rows along +y); `splat2` holds layers 5-8 and must match `splat`'s size and
mip count. With `splat` but no `splat2`, layers 5-8 get no weight.

**Relief rules.** Units stand at height 0 and the simulation's board is flat, so the terrain
may never suggest a ramp, ridge, pit or cliff the simulation does not have:

- **Inside the board** the height stays within ±`relief`, at most 0.5 m, and tapers to 0 over
  the last 2 m to the edge.
- **Outside the board** the ground only rises: a flat apron (`none`), rolling hills over the
  first third of the margin (`hills`) or a steep terraced rise a few metres past the edge
  (`cliffs`), up to `edge_height`. The board reads as a basin; the foot of the slope is the
  edge of the walkable space. The margin is 0.6 × the map extent, clamped to 96-240 m. Every
  style starts its rise one Low-tier grid cell (`(extent + 2 × margin) / 128`, 2-4 m) past
  the edge, so the cells that straddle the edge stay flat on every tier.

The generator clamps to these rules and `content-check` verifies them on the generated
heightfield of every tier's grid (128, 192, 256 and 384 cells), including the interpolated
surface of the cells that straddle the edge (see
[Map consistency checks](#map-consistency-checks)).

**Edge props.** `edge_mesh` places a model around the board every `edge_spacing` metres (at
most 256), offset outward so its footprint stays off the board: the half diagonal of the
mesh's x/z bounds scaled to `edge_mesh_height` (never less than `edge_mesh_height × 0.25 +
2` m, at most half the margin), facing inward (a mesh authored along +X runs along the edge). It counts
toward the four distinct meshes a descriptor may name. Walls are still drawn along every
simulation wall segment (`wall_mesh` or the built-in block).

**Fallback.** No `[terrain]` table, a missing or mismatched layer texture, or
`MOBA_TERRAIN=flat`: the flat R2a plane in `tint`. A match never fails because of terrain.

### Atmosphere

The `[atmosphere]` table sets the height fog and the sky (ADR-0014 §4). Every key is
optional; unset keys keep the engine defaults.

```toml
[atmosphere]
fog_color = [0.62, 0.72, 0.85]   # linear RGB 0..=4; also the sky's horizon blend
fog_density = 0.005              # per metre at the ground, 0..=0.2
fog_height = 30.0                # height falloff scale, metres, 1..=400
fog_start = 70.0                 # metres from the camera before fog begins, 0..=2000
fog_max = 0.65                   # maximum opacity, 0..=0.85
sky_zenith = [0.18, 0.36, 0.75]  # gradient sky (maps without an environment), linear 0..=8
sky_horizon = [0.62, 0.72, 0.85]
```

The fog is exponential in height and integrated along the view ray, so it is thin near the
camera and thick towards the low horizon. The gameplay camera sits about 40 m above the
ground looking at points 18-80 m away; a `fog_start` around 60-70 m and a density of
0.005-0.008 keep the board clear there while the horizon and the spectator's low orbit fade
into the sky. `fog_max` is capped at 0.85 so fog never fully hides a unit (ADR-0011 rule 1).
Keep `sky_horizon` close to `fog_color` so distant terrain meets the sky without a seam.
`MOBA_FOG=off` and `MOBA_SKY=off` switch them off.

### Fog of war

The `[fog_of_war]` table styles unseen areas (ADR-0014 §3). Every key is optional.

```toml
[fog_of_war]
color = [0.03, 0.05, 0.08]       # linear RGB 0..=1, the colour unseen areas tend towards
darken = 0.65                    # 0.2..=0.9
desaturate = 0.45                # 0..=1
mist_height = 2.5                # metres, 0..=32: ground mist in unseen areas
mist = 0.3                       # mist strength, 0..=1
```

`darken` is limited to 0.2..=0.9: below it the vision boundary is hard to see, above it
unseen ground stops being readable. The mist thins with height, so unseen ground fogs while
the tops of tall walls stay legible. The fog texture is built only from the client's own
team's vision; the server still decides what each client is sent. It is a smoothed
presentation, so static ground up to about one fog texel past the server's vision can show,
and lost vision drops to half visible at once before it fades.

### Foliage

With a `[terrain]` table, the `[foliage]` table scatters grass, brush, reeds and flowers
([ADR-0017](adr/0017-decals-foliage-water.md) §2). Every key is optional; an empty
`[foliage]` table gives the default meadow.

```toml
[foliage]
density = 1.5                    # grass clumps per 10 m² of board, 0..=8
height = [0.3, 0.6]              # clump height range, metres, 0.1..=1.5, low <= high
base_color = [0.05, 0.16, 0.04]  # linear RGB 0..=1 at the root
tip_color = [0.35, 0.55, 0.18]   # at the tip
flower_color = [0.9, 0.75, 0.3]
wind = [0.8, 0.3]                # direction times strength (map x, y), length 0..=4
wind_speed = 1.2                 # gust speed, 0..=8
reeds = true                     # reeds along the water's banks
seed = 1                         # u32, combined with the terrain seed

[[foliage.patches]]              # optional, at most 32
center = [200, 60]               # map coordinates
radius = 8.0                     # metres, 1..=60
kind = "brush"                   # required: grass | brush | reeds | flowers
density = 4.0                    # clumps per 10 m², 0..=20
```

Placement is procedural and deterministic (`moba_assets::generate_foliage`): a jittered grid
over the board kept where a low-frequency mask allows (meadows with bare patches), with a few
flower clumps; reeds `width / 2 + 0.2` to `+ 2` m from each water body's centre line; and
the patches. Nothing is ever placed within `lane_width / 2 + 1` m of a lane, 6 m of a
structure, 10 m of a spawn, 2 m of a wall, in water or off the board. Grass and flowers take
the `height` range; brush is 0.9-1.4 m and reeds 0.9-1.5 m. Foliage is **decorative**: the
simulation has no brush areas and no one hides in a bush, and only units the client draws
bend it. Quality tiers thin it (Low 30 %, Medium 60 %) and cap it (8k / 24k / 48k / 64k
clumps); `MOBA_FOLIAGE=off` draws none.

### Water

With a `[terrain]` table, up to eight `[[water]]` bodies add shallow rivers and pools
([ADR-0017](adr/0017-decals-foliage-water.md) §3):

```toml
[[water]]
points = [[40, 210], [128, 200], [216, 214]]  # required, 1..=32; one point is a round pool
width = 12.0                     # metres across (a pool's diameter), 2..=60
level = 0.1                      # surface height, metres, 0..=0.3 (never below the ground)
depth = 0.45                     # bed depth below the surface, 0.05..=0.8
flow = 1.0                       # flow speed along the points, m/s, 0..=8 (pools do not flow)
color = [0.02, 0.12, 0.16]       # deep colour, linear RGB 0..=1
shallow_color = [0.10, 0.30, 0.30]
clarity = 0.35                   # metres of depth per e-fold towards the deep colour, 0.05..=4
foam = 0.6                       # shoreline foam, 0..=1
```

The bed is carved into the generated terrain (`moba_assets::generate_terrain_with_water`):
it falls smoothly to `level - depth` at the centre line, meets the surface exactly at
`width / 2`, and the banks rise a few centimetres above the surface before blending back
into the relief by `width / 2 + 3` m. The splat's path layer (layer 2) covers the bed. All of
it stays within the board's ±0.5 m relief rule, so `level - depth` must be at least -0.5:
units stand at height 0 and visibly wade (at most 0.3 m above their feet, 0.8 m above the
bed), never swim. The surface mesh shares the terrain grid, so the shoreline lands exactly
where the water meets the drawn ground. `MOBA_WATER=simple` drops the screen-space
refraction; `MOBA_WATER=off` draws no water (the carved bed still shows).

### Map consistency checks

`content-check` runs `moba_assets::check_map_consistency` on every `[[assets.maps]]`
descriptor whose name the pack's map uses (its `visuals`, or the theme's name), and on
`maps/<visuals>.toml` next to the pack whenever it exists, listed or not (the client loads
it by name; one that does not parse is an error), against the pack's map (ADR-0014 §2). Errors fail the check; warnings are reported and may be accepted
by art direction. Each message names the wall, prop or edge prop and its position:

```text
error: map visuals 'ember': wall 0 ([112, 60] to [112, 100]): wall_height 1.5 m is below 2 m, too low to read as a line-of-sight blocker next to a hero
warning: map visuals 'ember': prop 2 at [128, 128] (16 m tall, footprint radius 3.60 m) overlaps lane 0's corridor (0.0 m from the lane, half-width 6 m): looks like an obstacle units walk through
```

| Rule | Kind | Why |
|---|---|---|
| Board relief above 0.5 m, or edge ground below 0 (on the generated heightfield of every tier's grid, sampled across the cells straddling the edge) | error | units stand at height 0: more relief reads as a ramp or pit, and the board must read as the basin |
| A simulation wall without a visual wall covering both endpoints, or `wall_height` below 2 m | error | walls block line of sight; a missing or knee-high wall hides a gameplay rule |
| A simulation wall endpoint outside the board | error | its visual would stand on the edge slope |
| An edge prop whose footprint (from `models/<edge_mesh>.glb`'s bounds, as the client places it) reaches the board | error | edge props mark the end of the walkable space |
| A missing or mis-formatted terrain texture, or layers that disagree on size or mip count | error (file check) | the client would fall back to the flat plane |
| A prop at least 2 m tall whose footprint (radius `height × 0.225`) overlaps a lane corridor (`lane_width / 2`, 12 m wide without `[terrain]`) or comes within 4 m of a structure | warning | it looks like an obstacle units walk through (props elsewhere are decoration by design) |
| A water body whose `level` is outside 0..=0.3 m, or that (its points widened by `width / 2`) leaves the board | error | units stand at height 0 and must visibly wade (a lower surface leaves them standing over it); outside the board the ground only rises |
| A water mesh over 262,144 vertices (`MAX_WATER_MESH_VERTICES`) on any terrain grid (every tier's, bed carved) | error | the renderer rejects a larger mesh, so that tier would draw no water |
| A board sample under a water body more than 0.8 m below its surface (every tier's grid, bed carved), or the relief rule above failing with the beds carved | error | the simulation lets every unit walk everywhere, so no water may look too deep to cross |
| `[foliage]` or `[[water]]` without `[terrain]` (code-built descriptors; the parser rejects it) | error | both stand on the heightfield |
| Water within 4 m of a structure or covering a spawn; water crossing a wall; two bodies that overlap | warning | structures and spawns read as dry ground; a wall in water or merged beds look broken |
| A `brush` patch over a lane corridor (`lane_width / 2` plus its radius) or within 6 m of a structure | warning | it would read as cover the simulation does not have |

The JSON output adds `map_errors` and `map_warnings` counts; the errors are also in `errors`.

### Post-processing

Exposure, eye adaptation, bloom and colour grading are set per map in the visuals descriptor's
optional `[post]` table (R6); every key is optional and unset keys keep the engine defaults.
Like the rest of the descriptor it is client-local and never hashed or sent. The three shipped
descriptors each have one; [`content/maps/ember.toml`](../content/maps/ember.toml) is the
auto-exposure example:

```toml
[post]
exposure = 0.0                 # exposure compensation in EV stops, -4..=4 (default 0)
auto_exposure = true           # opt in to eye adaptation (default off)
exposure_min = -1.0            # auto-exposure clamp in EV stops, -8..=8 (default -2)
exposure_max = 1.0             # -8..=8, at least exposure_min (default 2)
adaptation = 1.5               # adaptation rate per second, 0.1..=10 (default 1.5)
bloom = 0.06                   # bloom strength, 0..=0.1 (default 0.04)
grading = "ember"              # <root>/grading/ember.cube: a colour-grading 3D LUT
grading_strength = 0.9         # 0..=1: blend from the ungraded to the graded colour
```

- **Exposure** is in EV stops: `+1` doubles the scene light reaching the tonemapper, `-1`
  halves it.
- **Auto exposure** is opt-in per map. When on, the exposure adapts towards the scene's
  brightness at `adaptation` per second, clamped to `exposure_min..=exposure_max` EV. An
  unset bound takes its default, so `exposure_min = 3` alone is an error (above the default
  `exposure_max` of 2). Captures and golden images pin auto exposure off so they stay
  deterministic.
- **Bloom** is kept low by default so bright surfaces glow; the range stops at 0.1 so a map
  cannot wash its units and team colours out.
- **Grading names** follow the model-name rules (1..=32 of `[A-Za-z0-9_-]`) and resolve to
  `<root>/grading/<name>.cube`.
- **Limits.** Unknown keys, wrong types and out-of-range or non-finite numbers are errors,
  like the rest of the descriptor.

**Grading LUTs** use the Adobe/Resolve `.cube` text format, 3D only. The LUT's input and
output are **display-encoded** RGB: the input is the tonemapper's AgX output (roughly a 2.2
gamma) and the output is written to the display as is, so a LUT from a grading tool must be
built for a display-referred (gamma 2.2 / sRGB-like) image, not for log or linear footage.
The loader accepts:

- `LUT_3D_SIZE` from 2 to 64, exactly once and before the data, then exactly size³ data lines
  of three numbers, red varying fastest, then green, then blue;
- `TITLE` (ignored), `#` comments and blank lines;
- `DOMAIN_MIN 0 0 0` and `DOMAIN_MAX 1 1 1` only: any other input domain is an error, since
  the input is always `0..1`;
- data values that are finite and within −0.5..=1.5, stored clamped to `0..=1`.

`LUT_1D_SIZE`, unknown keywords and keywords after the data are errors. A file is at most
8 MiB (a 64³ LUT with six decimals is about 7 MB). The shipped LUTs are 17³ (about 130 KB).

**Readability.** Grades are art direction, not a filter: they must keep team colours
distinguishable (ADR-0011 rule 1), and this is **checked**. For the standard red/blue unit
colours and the colour-blind palette's orange/sky blue, at the map's `grading_strength` and
at its exposure ± 1 EV, the grade must move no team hue by 20° or more, keep each team's
chroma (brightest minus darkest channel) at 0.2 or more, and keep the two teams at least 90°
apart in hue. `grade-bake bake` refuses a grade that fails and `grade-bake info` reports the
result for any `.cube`; the client checks a map's grade when it loads it and, if it fails,
logs the rule and draws the map ungraded. Keep blue-channel loss small in warm grades, and
look at a hand-made LUT with `grade-bake apply` on a capture.

**LUTs** come from `moba-tools grade-bake`, which evaluates a deterministic grade on every
lattice point: per-channel lift/gamma/gain (`out = (gain·(x + lift·(1 − x)))^(1/gamma)`),
contrast around 0.5, saturation around Rec.709 luma, then white balance as per-channel gains
(`r ·= 1 + 0.1·temperature`, `b ·= 1 − 0.1·temperature`, `g ·= 1 − 0.1·tint`), clamped to
`0..=1`, all in display space:

```bash
# writes content/grading/ember.cube (17³)
cargo run --release -p moba-tools --bin grade-bake -- bake --preset ember --size 17 content/grading/ember.cube
cargo run --release -p moba-tools --bin grade-bake -- bake --preset frost --saturation 0.85 \
  --lift 0,0.01,0.02 content/grading/my-frost.cube
cargo run --release -p moba-tools --bin grade-bake -- info content/grading/ember.cube
cargo run --release -p moba-tools --bin grade-bake -- apply content/grading/ember.cube shot.png graded.png
```

Presets are `identity` (the default), `ember` (warm, slightly lifted shadows, gentle
contrast), `frost` (cool, slightly desaturated, crisp) and `meadow` (neutral-warm, a touch
more saturation). `--saturation` (0..=2), `--contrast` (0.5..=2), `--temperature` and
`--tint` (−1..=1), `--lift r,g,b` (−0.5..=0.5), `--gamma r,g,b` (0.2..=5) and
`--gain r,g,b` (0..=2) override any value; `--size` is 2..=64 (default 33). The file's header
comments record the preset and every parameter, so a bake is reproducible. `info` prints the
size, each channel's output range and the mean absolute deviation from the identity; `apply`
grades an 8-bit PNG (its values taken as display-encoded) for side-by-side previews.
`scripts/bake-sample-textures.sh` regenerates the shipped three.

## Provenance

Every file in a content root's asset directories (`models/`, `textures/`, `vfx/`, `maps/ao/`
and so on) needs a provenance entry in the `provenance.toml` of its own directory (A0,
[ADR-0022](adr/0022-provenance-gate-mikktspace.md)). The root itself may hold only packs
(`*.toml`) and the generated `CREDITS.txt` and `AI-DISCLOSURE.md`; put any other file in an
asset directory. Dot-files and dot-directories (`.gitkeep`, `.cache/`, ...) and symlinks are
rejected everywhere, so nothing in a content root is hidden from the gate.

One entry can cover several files. List binary assets (textures, meshes, LUTs, audio) by
name, so each new one is a reviewed manifest change; keep `*` patterns for families of text
descriptors such as `*.toml`:

```toml
[[asset]]
files = ["rock-042-color.ktx2", "rock-042-normal.ktx2"]   # names (or `*` patterns) here, no `/`
title = "Rock 042"
source = "https://ambientcg.com/view?id=Rock042"   # or "original" (made for this project)
author = "ambientCG"
licence = "CC0-1.0"
made_with = "texture-bake bake … --kind color"     # optional
# attribution = "…"                    # required for CC-BY-*: the exact credit line
# notes = "…"                          # optional

# Only for AI-generated assets (roadmap §9.5):
# [asset.ai]
# model = "TRELLIS.2-4B"
# model_version = "2025-12"
# weights_licence = "MIT"
# seed = 1234
# human_modified = false               # true, with `notes` on the changes, for CC-BY
# prompt = "a bronze brazier, game asset"   # or empty when inputs drive it
# tools = ["ComfyUI 0.3.x", "mesh-bake"]
# [[asset.ai.inputs]]
# name = "brazier-concept.png"
# source = "original"                  # a third-party input must be CC0-1.0
# licence = "CC0-1.0"
```

**The gate.** `content-check --provenance` fails when a file has no entry or two, a pattern
matches nothing (or is not a plain ASCII name: letters, digits, `.`, `_`, `-`, `*`), a key is
unknown, the root holds something other than packs and notices, a dot entry or symlink
exists, or a licence is not allowed:

- allowed: `CC0-1.0`, `CC-BY-3.0`, `CC-BY-4.0`, `MIT`, `Apache-2.0`, `MIT OR Apache-2.0`,
  `OFL-1.1`;
- approved exceptions, each only where it was approved: `LicenseRef-CMU-Mocap` as an asset
  `licence` under `animations/`, and `LicenseRef-NVIDIA-Open-Model-License` (Kimodo) as
  `ai.weights_licence`. A new exception, or a wider scope, is a code change in
  `moba_assets::provenance`, approved by the maintainer;
- rejected: non-commercial (`NC`), no-derivatives (`ND`), share-alike, unknown and
  all-rights-reserved.

`CC-BY-*` needs `attribution`. An AI-generated asset is `CC0-1.0`, or `CC-BY-*` only with
`ai.human_modified = true` and `notes` saying what a person changed; it records its model,
version, weights licence (allowed or an exception), seed, tools and a prompt or inputs.
Manifest text may not contain control characters (line breaks included) or `<!--`/`-->`, so
it cannot break the generated files; Markdown characters are escaped in them.

**Generated notices.** The same command writes the content section of `THIRD_PARTY.md`
(between its `BEGIN`/`END content provenance` markers: exactly one of each, in order, on
their own lines, or the command fails without writing; with neither it appends the section),
`content/CREDITS.txt` (plain text the
game can show) and `content/AI-DISCLOSURE.md` (the Steam AI-disclosure list). Commit them with
the asset; `--check` writes nothing and fails when any is stale, and
`scripts/agent-verify.sh` runs it:

```text
$ cargo run -p moba-tools --bin content-check -- --provenance          # check and write
$ cargo run -p moba-tools --bin content-check -- --provenance --check  # CI: check only
$ cargo run -p moba-tools --bin content-check -- --provenance --content mods/x --third-party mods/x/NOTICES.md
```

`provenance.toml` is never an asset: the VFX loader skips `vfx/provenance.toml`, so do not
name an effect, map visuals, environment, material or animation set `provenance` (nothing
rejects the name; the file would be read as a manifest). Provenance is never hashed, signed or
sent, and the client never reads it.

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
