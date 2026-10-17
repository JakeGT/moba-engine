# ADR-0000: Agentic development workflow

**Status:** accepted (2026-10-14)

## Decision
Every change goes through: task spec → (design) → TDD implementation → `just ci` → parallel specialist review → fix and re-review → bench compare → merge.
The orchestration level is "Balanced": one implementer per task, plus 2–3 specialist reviewers (`.claude/agents/`) and `/code-review`. Independent milestone tracks run in parallel git worktrees once M1 lands.

## Milestone exit criteria
| # | Milestone | Exit criteria |
|---|---|---|
| M0 | Bootstrap | `just ci` green on Linux, Windows and macOS with empty crates; the hook and reviewers proven on a dummy change |
| M1 | Core runtime | ECS iteration benches beat a naive AoS baseline; miri + loom clean |
| M2 | Platform | window + input + fixed-step loop on all 3 OSes |
| M3 | Renderer | 500 skinned units + map ≤ 6.9 ms; zero validation/sync-validation messages; golden images pass |
| M4 | Simulation | 5v5 + minion waves headless ≤ 2 ms/tick; determinism hash stable for 10k ticks across OSes |
| M5 | Networking | 5-min soak at 150 ms/5%/30 ms with 10 bots passes; bandwidth within budget; fuzzers run 1 h clean |
| M6 | Vertical slice | 10-client playtest on a real network, no visible rubber-banding at 100 ms RTT |
| M7 | Hardening | 50 concurrent matches per server box within budget; `/code-review ultra` clean |
| M8 | Content pipeline | a designer adds an ability with no code change |

## Gates added as milestones land
- M1: miri, loom, criterion baselines
- M3: lavapipe + validation layers + sync validation in Linux CI; golden-image tests
- M4: cross-platform determinism hash test
- M5: netsim soak test (`just soak`), cargo-fuzz targets (short in CI, long nightly)
