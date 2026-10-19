# 0004: Headless simulation (M4)

**Milestone:** M4   **Crates touched:** game, nav, sim, server   **Reviewers:** netcode + perf + /code-review

## Goal
A deterministic, server-authoritative, fixed-tick MOBA simulation with 5v5 heroes, three
lane minion waves, towers/nexuses, attacks, abilities, respawns, bots and a state hash —
running far inside the 2 ms/tick budget.

## Scope (landed)
- **game**: data tables for 5 heroes, 15 abilities, minion/tower/nexus stats, a symmetric
  three-lane default 256×256 map (runtime packs may change extent/theme/rules), and integer
  damage mitigation.
- **nav**: deterministic fixed-cell spatial grid (returns exact-radius candidates) used for
  target acquisition; lane waypoints live with the map.
- **sim**: archetype ECS state; integer/fixed-point maths; commands (`MoveTo`, `Attack`,
  `CastAbility`, `Stop`); skillshots, dashes, heals and bursts; tower/minion aggro;
  respawns; hero death handling; deterministic bot AI; stable state hash.
- **server**: headless 30 Hz match runner with `--ticks`/`--seed`, tick timing and state hash.
- Layering fixed: `game` no longer depended on `sim`; `sim` depends on `game` + `nav`.

## Determinism
- No wall clocks, no `HashMap`/`HashSet` (crate-level clippy), no floats in gameplay.
- Positions, speeds, ranges and timers are fixed-point or integers.
- Iteration order is ECS archetype order; spatial-grid queries are row-major + insertion
  order; target ties break on `Entity`.
- `Simulation::state_hash` hashes every dynamic component plus projectiles and match state.

## Non-goals
- Fog of war/vision filtering and snapshot delta compression (M5).
- Levels, items, runes and shop (M6 content).
- Navigation beyond lane waypoints (heroes steer directly; a navmesh/flow-field upgrade can
  land with jungle camps without changing the input contract).

## Acceptance tests
- [x] `idle_ticks_are_deterministic_for_ten_thousand_ticks` (10k ticks, hash every tick).
- [x] `full_bot_match_is_deterministic` (10k ticks, two sims, hash every 100 ticks).
- [x] `minion_waves_spawn_on_schedule_and_follow_lanes` (24 minions per wave, 8 per lane).
- [x] `basic_attacks_apply_mitigated_damage`, `skillshots_travel_and_hit`.
- [x] `dead_heroes_respawn_at_their_base`, `towers_can_be_destroyed`,
      `destroying_a_nexus_ends_the_match`.
- [x] `bots_fight_and_produce_events`, `heroes_respawn_after_death`.
- [x] Steady-state 5v5 + waves: **12.9 µs/tick** (see `docs/perf/0004-simulation.md`).
- [x] Client renders the live match (heroes, minions, towers, nexuses) with zero
      validation messages; see `docs/images/m4-simulation-slice.png`.

## Budgets touched
Server tick target ≤ 2 ms. Measured: 12.9 µs/tick steady state, 209.6 µs worst observed
tick (wave-spawn growth), 106 peak units. Headroom is ~155×.

## Review log
| Reviewer | Finding | Verdict | Resolution |
|---|---|---|---|
