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
