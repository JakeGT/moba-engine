# Content authoring

> **Preview:** content is compile-time Rust data. Runtime content packs and asset hot reload
> are planned for M8. This guide documents what is safe to change today and the intended
> extension path.

## Add or tune an item

Items live in `crates/game/src/item.rs`:

```rust
pub const ITEMS: &[ItemDef] = &[
    ItemDef { name: "Ironblade", cost: 350, attack_damage: 12, .. },
    // ...
];
```

1. Append or edit an `ItemDef`. Fields are flat bonuses (`health`, `mana`, `attack_damage`,
   `armor`, `magic_resist`, `move_speed_per_sec`).
2. The shop UI is generated from `ITEMS`; number keys 1–8 map to the first eight entries.
   More entries need a scrollable shop/extra binding.
3. Purchases apply stats once in `Simulation::buy_item`; if you add a new stat, extend
   `ItemDef`, the purchase application and the snapshot fields if clients must display it.
4. Add a determinism test if the item affects combat (`moba-sim` tests compare state hashes).

Balance constants (starting gold, passive gold rate, shop radius, level growth, XP curve) are
in the same module, so gameplay tuning stays out of the simulation loop.

## Add a hero or ability

- `crates/game/src/hero.rs` defines `HeroDef` and `HEROES`; `ability.rs` defines
  `AbilityDef`/`AbilityKind`.
- Hero picks are indices into `HEROES` (`SimConfig::hero_picks`). Keep the slot count at 10.
- Ability behaviour is interpreted by `moba-sim` (`cast_ability`); brand-new
  `AbilityKind` variants require a simulation branch, tests and likely snapshot/UI fields.
- Bot logic uses the same abilities by slot; verify bots do not panic on unusual ranges.

## Map and units

- `crates/game/src/map.rs` holds lanes, structures and spawn points; `units.rs` holds minion
  and structure stats.
- Structure count/positions are data; the simulation spawns whatever the table contains.
- Vision uses the per-unit `VisionRange`; the HUD fog derives circles from the same values.

## Determinism checklist

- No floats, wall clocks, environment reads or unordered iteration in gameplay paths.
- Break ties by entity id; use fixed-point math for positions/damage.
- Add/extend tests in `moba-sim/tests` and update any golden hash expectations deliberately.
- Run `cargo test --workspace` and a short soak after content changes.

## Planned content pipeline (M8)

The intended direction is a versioned content pack (heroes, abilities, items, map, UI metadata)
loaded before match creation, with compile-time tables as the default pack and hot reload in
development. The simulation will accept a content snapshot rather than reading global statics;
replays will record the content hash. Until then, the static tables are the single source of
truth and are deliberately small and readable.


## Runtime packs

`moba-server --items FILE` loads a TOML catalogue. The pack is validated (1–16 items,
unique 1–16 byte ASCII names, positive costs) and published in the `Welcome` handshake, so
clients render the same names and costs without recompiling. The simulation reads purchase
costs/bonuses from the same catalogue.

```toml
[[items]]
name = "Test Blade"
cost = 10
attack_damage = 5
move_speed = 1.2  # optional, world units/second
```

Fields: `name`, `cost`, `health`, `mana`, `attack_damage`, `armor`, `magic_resist`,
`move_speed` (all bonuses optional except name/cost). The catalogue is fixed-capacity on the
wire; a pack cannot exceed 16 items. Packs may also define up to 8 heroes:

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
abilities = [0, 1, 2]  # ids into the engine ability library
```

Packs can define abilities instead of using the engine's default library:

```toml
[[abilities]]
name = "Nova"
cooldown_ticks = 90
mana_cost = 20
cast_range = 5.0
kind = "burst"          # skillshot | dash | heal | burst
radius = 4.0
damage = 90
damage_type = "magic"   # physical | magic
```

Kinds take: `skillshot` (`speed`, `range`, `radius`, `damage`, `damage_type`), `dash`
(`range`), `heal` (`amount`, `radius`) or `burst` (`radius`, `damage`, `damage_type`).
Hero names must be 1–16 ASCII bytes and unique; ability ids must exist. Clients receive the
hero names/vision/ability bindings for HUD/vision, while simulation stats come from the same
pack. Replays record a content hash and inputs; keep the pack alongside a replay and verify with
`moba-tools replay --items pack.toml match.mbr`. A mismatched/missing pack is rejected. Ability behaviour effects are data-driven via these kinds; new behaviour kinds and map/lane
definitions remain compile-time until the rest of M8 lands.
