# Ability composition (#38 phase 2b)

Status: complete (protocol v15+; current protocol v38 / sim logic v7).

## Model

- `Targeting`: `SelfCast`, `Point { range }`, `Direction { range }`.
- `EffectList` / `AreaEffectList`: fixed-capacity arrays (3 effects, 3 area effects).
- `Effect`: `Damage`, `Heal`, `ApplyModifier`, `Displace`, `Projectile { on_hit }`,
  `Area { effects }`, `Zone { period, effects }`.
- `AreaEffect`: `Damage` or `ApplyModifier`.
- `ApplyModifier` carries `modifier` (content def id), duration, factor, amount and damage type.
- Ability defaults become data; legacy TOML `kind` strings are converted to the same data.
- Projectiles carry `on_hit` effects; zones are bounded `Vec<Zone>` state.

## Migration checklist

1. [x] Game data model + default ability table + parser + validation + hash.
2. [x] Simulation executor: targeting, effect walk, projectile on-hit, zones, modifiers.
3. [x] Protocol v15 wire catalogue + client reconstruction/range summary.
4. [x] Replay `SIM_LOGIC_VERSION` bump, golden hash re-record.
5. [x] Behavior tests for all 10 legacy kinds vs expected outcomes.
   - [ ] Old-vs-new ObservedUnit stream diff (schema/archetype change makes identical replay impossible).
6. [x] Data-only root-on-hit projectile and burn-zone content example.
7. [x] Remove `AbilityKind`.
8. [ ] Generic ModifierDef stat/flags/on-tick triggers beyond the six built-in kinds (#45).
