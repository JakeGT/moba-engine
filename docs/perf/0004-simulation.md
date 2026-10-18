# Performance log — M4 simulation

Measured 2026-10-17, release profile, 16-logical-core Linux, criterion 0.7. Each criterion
sample simulates **300 ticks**; per-tick numbers divide by 300.

| Bench | 300 ticks | Per tick |
|---|---|---|
| `sim/tick_idle_300_ticks` (10 heroes, no waves, no bots) | 379.5 µs | **1.27 µs** |
| `sim/tick_full_5v5_300_ticks` (bots, wave 1) | 3.096 ms | **10.32 µs** |
| `sim/tick_steady_5v5_300_ticks` (warmed to 90 s, waves 1–4 active) | 3.862 ms | **12.87 µs** |

Server run (`target/release/moba-server --ticks 20000`, wall-clock timed per tick):

```
tick 19800 wave 22 units 79 avg 12.6us
done: ticks=20000 wall=0.25s avg=12.6us max=209.6us peak_units=106
      towers=11 nexuses=2 minions=75 outcome=None
```

- **Budget:** 2 ms/tick for one 5v5 match → 12.9 µs is ~155× inside budget.
- **Worst tick:** 209.6 µs, from structural ECS growth when a wave adds entities. Still
  ~10× inside budget; pre-sizing the minion archetype would remove it.
- **Peak units:** 106 (26 heroes/structures + up to 80 minions). The hard cap is 512.
- **Determinism cost:** the state hash is only invoked by tests/tools, not by the tick.
- **Allocations:** the grid, event queue, projectile list and per-system scratch vectors are
  pre-sized. Structural spawning grows ECS columns; no per-tick heap allocation has been
  observed in steady state. A dedicated allocation-counting test is future work.

## How the tick is spent

For each tick: timers/regen → inputs/abilities → spatial rebuild → minion/structure target
selection → movement → spatial rebuild → attacks → projectiles → deaths → wave spawn.
Two grid rebuilds cost ~1 µs combined per 100 units; target acquisition is neighbourhood
limited rather than O(n²).

## Next optimisation

- Split movement/targeting across the job pool per archetype once unit counts exceed ~500.
- Pre-reserve the minion archetype for the worst-case live count to remove the 209 µs spike.

## Allocation guard matrix

`cargo test -p moba-sim --test allocations` serialises its cases and measures on the calling
thread. Latest 300-tick windows:

| Case | Allocations | Bytes |
|---|---:|---:|
| steady bots | 0 | 0 |
| 10-hero teamfight | 0 | 0 |
| wave spawn window | 2 | 192 |
| status modifier pack (all kinds) | 0 | 0 |
| respawn window (900 ticks) | 0 | 0 |
| shop purchases | 0 | 0 |

Budgets are ≤8 allocations / ≤1 KiB per window; steady, teamfight, respawn, shop and status
paths are expected to be zero (wave spawn may allocate to grow pools). The earlier
30-allocation budget was masking cross-test allocator contention as well as real wave growth.

The spatial grid is now flat: items/cell indices append into reusable vectors, and a stable
counting sort rebuilds row-major cell ranges only after inserts. This removed the `Vec<Vec<T>>`
per-cell allocations and the per-query scratch clone; steady ticks are fully allocation-free.

## Observe/fog pass

`sim/observe_3_views` (two team views + spectator, warmed past three waves) improved from
**19.2 µs to 13.2 µs** (~31%) with the new implementation:

- source units are gathered through one tuple query instead of per-entity `TypeId` scans;
- all visible-candidate required fields are copied from a single tuple query into a reused
  scratch vector, then the fog/wall pass runs without touching archetypes;
- optional hero/progression/modifier fields are fetched only for visible units.

Deterministic output is unchanged: the cross-OS golden state hash remains
`0x1923e3d7f3392604`.
