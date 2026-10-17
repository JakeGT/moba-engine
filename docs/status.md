# Engine status

Last updated: 2026-10-17 (round 1 of the build).

## Milestones

| # | Milestone | State | Notes |
|---|---|---|---|
| M0 | Bootstrap | ✅ | Workspace, lints, CI, justfile, agent definitions. |
| M1 | Core runtime | ✅ (miri/loom pending) | 62 tests green, clippy `-D warnings` green, ECS benches beat AoS. |
| M2 | Platform | ⬜ | winit window + input + fixed-step loop. |
| M3 | Renderer | ⬜ | ash device/swapchain/render graph, shaders, golden images. |
| M4 | Simulation | ⬜ | Heroes, minions, towers, abilities, nav, fog, determinism. |
| M5 | Networking | ⬜ | UDP transport, snapshots, prediction/reconciliation, lag comp. |
| M6 | Vertical slice | ⬜ | Playable 5v5 on one map. |
| M7 | Hardening | ⬜ | 50 matches/box, fuzz, profiling capture. |
| M8 | Content pipeline | ⬜ | Data-driven abilities. |

## What exists now

- **`moba-core`** — Q32.32 fixed-point (`Fix`, `FixVec2`) with deterministic integer
  `sqrt` and CORDIC `sin_cos`; xoshiro256++ RNG with streams/jump; generation-checked
  arena; 64-bit and growable bit sets; wrap-safe ticks and an integer `FixedStep`; FNV-1a
  stable hashing; `glam` re-exports plus AABB helpers.
- **`moba-ecs`** — generational entities; type-erased SoA archetype columns; insert,
  remove and despawn move components without cloning; typed queries up to 8 components
  with `With`/`Without` filters; per-row change ticks; zero-allocation item and chunk
  iteration.
- **`moba-jobs`** — scoped work-stealing pool with a global injector, per-worker deques,
  condvar idle sleep, caller participation, nested scopes, panic capture and
  `parallel_for` over disjoint chunks.

## Verification actually run

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked` — 62 tests
- `cargo bench -p moba-ecs --bench ecs_iter` — see `docs/perf/0001-core-runtime.md`
- `just layering` equivalent via `cargo tree` — server/sim/game/nav/net are clean of
  render/platform/winit/ash.

`just`, `cargo-nextest` and `cargo-deny` are not installed in this environment; their
commands were reproduced manually. Miri and loom are still to run (needs a nightly
toolchain).

## Known gaps / next round

1. Run miri over `core`, `ecs`, `jobs`; add a loom model of the scope counter protocol.
2. Add `World`-level parallel iteration (chunk queries split across the job pool) with a
   bench, then move on to M2 (winit platform) and M3 (ash renderer).
3. The reference machine has no usable Vulkan device (loader finds no GPU), so renderer
   work must compile clean and be validated on a machine with a GPU or lavapipe.
