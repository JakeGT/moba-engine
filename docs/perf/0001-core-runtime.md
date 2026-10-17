# Performance log — M1 core runtime

Measured 2026-10-17 on a 16-logical-core Linux machine, `rustc 1.99.1`, criterion 0.7,
100,000 entities, median of 50 samples after 1 s warm-up.

| Bench | Time |
|---|---|
| `ecs/iter_aos` (baseline AoS `Vec<Unit>`) | 21.66 µs |
| `ecs/iter_ecs_2c` (item iterator, `&mut Position` + `&Velocity`) | 86.29 µs |
| `ecs/iter_ecs_2c_chunks` (chunk iterator, same components) | **11.95 µs** |
| `ecs/iter_ecs_1c` (item iterator, `&Position` sum) | 222.24 µs |
| `ecs/iter_ecs_1c_chunks` (chunk iterator, same) | 36.96 µs |

## What this tells us

- The **chunk API is the hot path**. It resolves columns once per archetype and hands the
  system plain slices, which LLVM can vectorise. At 11.95 µs it is 1.81× faster than the
  AoS baseline for the canonical position/velocity update.
- The **item API is not the hot path**. It is ergonomic for sparse logic (a handful of
  actors, commands, event handling), but each item materialises a reference through the
  iterator state machine, which blocks vectorisation. Systems that touch every entity must
  use `query_chunks`.
- The 1-component sum is not a meaningful AoS comparison (there is no AoS equivalent in
  this benchmark); the numbers are kept to catch regressions in slice traversal.

## Earlier measurement, kept for the record

The first implementation resolved component columns per row (binary search + trait object
call). `ecs/iter_ecs_2c` measured **384 µs** — 17× slower than AoS. That result is why the
query layer was redesigned around a per-archetype `Fetch` state. The item iterator improved
to 86 µs (4.5×) and the chunk iterator to 12 µs.

## Allocation notes

- Query construction: zero heap allocation (fixed `[u32; 64]` id buffer, 64-bit masks).
- Chunk iteration: zero heap allocation.
- Item iteration: zero heap allocation.
- Structural changes: allocation only when a column or archetype grows.
- Job pool: one box per spawned job; use coarse chunks (≥ 1k elements) in hot systems so
  scheduling and allocation are amortised.

## Next performance work

- M4 will add per-archetype parallelism (`World` split across job chunks) and measure a
  5v5 + minion-wave tick against the 2 ms budget.
- `parallel_for` chunk sizing should become adaptive (target 50–200 µs per job).
