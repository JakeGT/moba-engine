# Ability composition (#38 phase 2b)

Status: in progress. Branch `feat/ability-composition`.

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

1. [ ] Game data model + default ability table + parser + validation + hash.
2. [ ] Simulation executor: targeting, effect walk, projectile on-hit, zones, modifiers.
3. [ ] Protocol v15 wire catalogue + client reconstruction/range summary.
4. [ ] Replay `SIM_LOGIC_VERSION` bump, golden hash re-record.
5. [ ] Behavior tests for all 10 legacy kinds vs expected outcomes.
6. [ ] Data-only root-on-hit projectile and burn-zone content example.
7. [ ] Remove/deprecate `AbilityKind` (phase 3).
