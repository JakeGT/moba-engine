# Engine status

Last updated: 2026-10-17 (round 1 of the build).

## Milestones

| # | Milestone | State | Notes |
|---|---|---|---|
| M0 | Bootstrap | ✅ | Workspace, lints, CI, justfile, agent definitions. |
| M1 | Core runtime | ✅ | 67 workspace tests green, Miri clean on core/ecs (jobs: tree borrows), benches beat AoS. Loom model still pending. |
| M2 | Platform | ✅ (Linux run; Win/macOS CI pending) | winit window, input, 30 Hz fixed step, 60-frame smoke run exits 0. |
| M3 | Renderer | 🟡 | Device/swapchain/dynamic rendering + 500 instanced units and terrain validated on lavapipe (zero validation messages); skinning, perf and golden CI pending. |
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
- **`moba-platform`** — the only crate that links `winit`: window config/error type,
  physical-key input snapshot with focus-loss safety, 30 Hz `FixedStep`, frame pacing,
  and the `App` loop. `moba-client` is a smoke-test app that exits after
  `MOBA_SMOKE_FRAMES` frames.
- **`moba-shaders`** — GLSL compiled to SPIR-V at build time and validated with
  `spirv-val`.
- **`moba-render`** — ash-based Vulkan renderer: validation layers, device scoring,
  Vulkan 1.2/1.3 feature negotiation, swapchain, sync2 barriers, dynamic rendering,
  instanced units via a storage buffer, procedural terrain, and CPU readback capture.
  `moba-client` renders a 500-unit demo scene.

## Verification actually run

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked` — 67 tests
- `cargo bench -p moba-ecs --bench ecs_iter` — see `docs/perf/0001-core-runtime.md`
- `just layering` equivalent via `cargo tree` — server/sim/game/nav/net are clean of
  render/platform/winit/ash.
- Miri: `moba-core` + `moba-ecs` pass under Stacked Borrows with strict provenance
  (85 s); `moba-jobs` passes zero-worker and threaded tests under Tree Borrows with
  `-Zmiri-ignore-leaks` (crossbeam-epoch's deferred bags are the leak source).
- Client smoke: `MOBA_SMOKE_FRAMES=60 cargo run -p moba-client` exits 0 on Linux/X11.
- Renderer: 500 units + terrain at 1920×1080 under lavapipe with
  `MOBA_VULKAN_VALIDATION=required` — zero validation messages; capture stored at
  `docs/images/m3-500-units.png` (see `docs/dev/software-vulkan.md`).

`just`, `cargo-nextest` and `cargo-deny` are not installed in this environment; their
commands were reproduced manually. Miri and loom are still to run (needs a nightly
toolchain).

## Known gaps / next round

1. Loom model of the scope counter protocol; add `World`-level parallel iteration over
   chunk queries and bench it.
2. M3 remainder: skeletal skinning, frame-time measurement on min-spec hardware, and a
   golden-image test under lavapipe in CI.
3. M4 simulation: fixed-tick MOBA rules on the ECS — heroes, abilities, minions, towers,
   navmesh flow fields, vision and a determinism hash over 10k ticks.
4. The reference machine has no usable hardware Vulkan device; lavapipe is installed in
   `.tools/` for validation (see `docs/dev/software-vulkan.md`).
