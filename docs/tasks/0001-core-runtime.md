# 0001: Core runtime (M1)

**Milestone:** M1   **Crates touched:** core, ecs, jobs   **Reviewers:** perf + /code-review

## Goal
Build the deterministic foundation every later milestone stands on: a fixed-point math
library for reproducible simulation, a seeded RNG, arena/bitset/time primitives, a custom
SoA archetype ECS with per-component change ticks, and a scoped work-stealing job system.
No rendering, platform or networking code lands in this task.

## Scope
- `core`: Q32.32 fixed-point (`Fix`, `FixVec2`) with saturating arithmetic, deterministic
  `sqrt`/`sin_cos`; `Rng` (xoshiro256++); generation-safe `Arena`; `BitSet64`/`BitSet`;
  `Tick` + integer `FixedStep`; `glam` re-exports and a small f32 helper/AABB layer.
- `ecs`: generational entities, type-erased SoA archetype columns, insert/remove/despawn
  that move values between archetypes without cloning, typed queries (up to 8 components)
  with `With`/`Without` filters, and wrapping-safe change ticks per component row.
- `jobs`: persistent worker pool with per-worker deques and a global injector, scoped
  spawns that borrow caller data, nested scopes, panic propagation, `parallel_for`.
- Criterion benches for ECS iteration against a naive AoS baseline.

## Non-goals
- Parallel system scheduling over `World` (that is M4 work; the job API is the enabler).
- Serialization of components and snapshot diffing (M5).
- Fixed-capacity pre-allocation policy for every archetype (M4 tunes capacities per match).

## Acceptance tests
- [x] `core::fix`: arithmetic saturates deterministically; `sqrt` matches `f64` within
      2^-24; `sin_cos` satisfies the Pythagorean identity within 2^-20; golden bit patterns
      lock the algorithm into the test suite.
- [x] `core::rng`: same seed produces the same 10k-value stream; jump/split produce
      independent streams; `gen_range` stays in bounds; shuffle is a permutation.
- [x] `core::arena`: stale generations are rejected; removed slots are reused; iteration
      visits exactly the live entries; values are dropped exactly once.
- [x] `ecs::world`: every insert/remove/despawn transition preserves the other components;
      entity locations stay valid after swap-removes (randomised model test).
- [x] `ecs::query`: mutable query marks exactly the rows it visits; `With`/`Without`
      filter archetypes; duplicate mutable components are rejected.
- [x] `jobs`: scoped borrows compile and mutate disjoint data; nested scopes complete;
      panics propagate to the caller; 10k spawns all run exactly once.
- [x] `benches/ecs_iter.rs`: 2-component chunk iteration beats the AoS baseline at 100k
      entities (11.95 µs vs 21.66 µs); item iteration (86 µs) is documented as the
      sparse-logic path. See `docs/perf/0001-core-runtime.md`.

## Budgets touched
Server tick and client frame are not wired yet. The M1 gate is throughput: ECS iteration
must beat AoS, and `spawn` in a job scope allocates only one small box per task (coarse
tasks, not per entity); `parallel_for` allocates once per chunk. Bench names:
`ecs/iter_aos`, `ecs/iter_ecs_2c`, `ecs/iter_ecs_1c`.

## Review log
| Reviewer | Finding | Verdict | Resolution |
|---|---|---|---|
