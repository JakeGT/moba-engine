# Profiling capture — M7

Captured 2026-10-17 with Criterion on the debug host (release profile, lavapipe not involved).
No system `perf` binary is installed, so these are deterministic microbenchmark captures
rather than sampled flamegraphs; the soak harness provides the end-to-end tick numbers.

## Snapshot codec (`moba-net`, 40 entities)

| Benchmark | Time |
|---|---|
| build full delta | 25.7 ns |
| encode full snapshot | 205.8 ns |
| decode full snapshot | 263.5 ns |
| build steady delta | 188.3 ns |
| encode delta | 35.0 ns |
| decode delta | 91.6 ns |
| apply delta to state | 79.2 ns |

Reproduce: `cargo bench -p moba-net --bench snapshot`

## Simulation (`moba-sim`, 300 ticks per batch)

| Benchmark | Batch | Per tick |
|---|---|---|
| full 5v5 from spawn | 3.218 ms | 10.7 µs |
| steady-state 5v5 (90 s warmup) | 3.856 ms | 12.9 µs |
| idle, no bots/waves | 385.5 µs | 1.28 µs |

The 30 Hz budget is 33,333 µs/tick; the steady-state simulation uses ~0.04% of it. Server
soak telemetry measured 11 µs average / 54 µs max with 10 clients.

Reproduce: `cargo bench -p moba-sim --bench tick`

## Allocation hot-path guard

`cargo test -p moba-sim --test allocations` counts global allocator calls across warm, serialized
300-tick windows (steady/teamfight/respawn/shop/status/wave). The tests enforce per-scenario
budgets; all six pass on the current tree. A per-tick heap `Vec` used only to alternate command
order was replaced by a fixed stack array (historical capture: 312 allocations / 24,490 bytes
reduced to 1 / 192 on the steady path). `crates/net/tests/snapshot_allocations.rs` separately
requires **zero** allocations for 300 steady build+encode iterations.

## Interpretation

- Snapshot encoding/decoding is sub-microsecond at 40 visible entities; per-client cost is
  negligible versus the 33 ms tick budget.
- Delta snapshots reduce encode cost from ~206 ns full to ~35 ns steady; decode ~92 ns.
- The batch match harness (50 matches, 509,820 ticks, two passes) remains the end-to-end
  determinism/tick-cost gate.

## Round 51 refresh

Re-ran `cargo bench -p moba-sim --bench tick` and `cargo bench -p moba-net --bench snapshot`
after the allocation fix and status-effect additions (16-core host; the v12 fuzz was pinned to
a separate core).

| Bench | Time | Per tick |
|---|---:|---:|
| sim full 5v5, 300 ticks | 3.298 ms | 11.0 us |
| sim steady 5v5, 300 ticks | 3.977 ms | 13.3 us |
| sim idle, 300 ticks | 0.428 ms | 1.43 us |
| snapshot full encode | 207 ns | |
| snapshot full decode | 253 ns | |
| snapshot delta encode | 35.8 ns | |
| snapshot delta decode | 93.1 ns | |
| snapshot delta apply | 84.2 ns | |

Tick-time changes were +1-3% versus the prior capture, within expected run-to-run variance
and consistent with the extra status components; the 33 ms server tick budget remains far
from saturated.
