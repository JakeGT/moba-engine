# MOBA Engine

A Rust + Vulkan MOBA engine with a server-authoritative netcode model. Targets: Linux, Windows, macOS (MoltenVK).
The full workflow plan is in `docs/adr/` and in the milestone table below.

## Hard rules
- `sim`, `game`, `nav` and `net` never depend on `render`, `platform`, `ash` or `winit`. `just layering` enforces this, and the server must build without Vulkan.
- The client sends **intent** (inputs), never state. The server validates everything.
- Hot paths (anything that runs per frame, tick or entity) do **zero heap allocations**. Use arenas, pools or pre-sized buffers.
- `sim`/`game` must be reproducible from (seed, inputs): no wall clocks, no `std` `HashMap`/`HashSet` (enforced by the crate-level `clippy.toml`).
- `unsafe` lives only in `core`, `ecs`, `jobs`, `platform` and `render`. Every block needs a `// SAFETY:` comment, with one unsafe op per block. All other crates `#![forbid(unsafe_code)]`.
- Vulkan: target 1.2 + `dynamic_rendering` + `synchronization2` + descriptor indexing. Don't use features MoltenVK lacks without a fallback.
- Add dependencies via `[workspace.dependencies]` in the root `Cargo.toml`. Every new crate needs a license `deny.toml` allows.

## Budgets
| Budget | Target |
|---|---|
| Client frame (1080p, min-spec) | ≤ 6.9 ms; never > 16.6 ms |
| Client CPU main thread | ≤ 4 ms |
| Server tick, one 5v5 match (30 Hz) | ≤ 2 ms |
| Downstream per client | ≤ 20 KB/s typical, ≤ 40 KB/s teamfight |
| Upstream per client | ≤ 4 KB/s |
| Playable network | 150 ms RTT, 5% loss, 30 ms jitter |

## Commands
- `just ci`: fmt, clippy (`-D warnings`), nextest, layering, cargo-deny. **Must be green before requesting review.**
- `just miri`: UB check for `core`, `ecs`, `jobs` (nightly).
- `just bench-compare`: compare against the `main` baseline. A regression over 3% blocks merge unless the PR justifies it.
- Use `cargo build --profile profiling` for Tracy/RenderDoc captures.

## Per-task workflow
1. Write the spec in `docs/tasks/NNNN-name.md` (copy `TEMPLATE.md`). For a non-trivial design, run a Plan agent first. Architectural choices get an ADR in `docs/adr/`.
2. Branch `task/NNNN-name`. Write the failing test or bench first, then implement. The post-edit hook runs rustfmt + `cargo check` on the crate.
3. Run `just ci` until it's green.
4. Run the specialist reviewers **in parallel** (one message, several Agent calls), selected by the paths touched:
   - `render/`, `shaders/`, any `unsafe` → `gpu-safety-reviewer` + `perf-reviewer`
   - `net/`, `netsim/`, `server/`, client prediction → `netcode-reviewer` + `perf-reviewer`
   - `sim/`, `ecs/`, `jobs/`, `nav/` → `perf-reviewer` (+ `netcode-reviewer` if the snapshot format changes)
   - always → `/code-review`
5. Fix CONFIRMED findings and re-run the affected reviewer on the fixed diff. Note any PLAUSIBLE findings you decline, with the reason.
6. Run `just bench-compare` if hot paths changed, then merge to `main` with conventional commits.
7. Milestone exit: profiling capture review, ADR summary, a `/code-review ultra` (the user triggers it), and `just bench-baseline`.

## Milestones
M0 bootstrap → M1 core runtime (ECS, jobs, arenas, Tracy) → M2 platform → M3 renderer → M4 simulation → M5 networking → M6 vertical slice → M7 hardening → M8 content pipeline.
Each milestone's exit criteria are in `docs/adr/0000-workflow.md`.
