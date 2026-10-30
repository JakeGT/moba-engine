# MOBA Engine — Claude Code entry point

The authoritative agent guide is **[AGENTS.md](AGENTS.md)** (invariants, contract versions,
commands, workflow, blockers). The operational runbook is **[docs/agents.md](docs/agents.md)**
(work selection, PR lifecycle, verification matrix, handoff template). Machine-specific setup
is in `agents.local.md` (gitignored; template at `agents.local.example.md`).

Current contract: workspace **0.4.0**, wire protocol **v41**, replay **v5**, simulation logic
**v8**, **780 tests**; Rust MSRV 1.88, Vulkan 1.3 minimum. Use `scripts/agent-verify.sh` (add `--full` for release/features/Loom/
soak/fuzz) as the default gate; `just` may not be installed.

Claude-specific hooks and reviewer definitions live in `.claude/`:
`perf-reviewer`, `gpu-safety-reviewer`, `netcode-reviewer` and the rustfmt/cargo-check hook.
Project rules in `AGENTS.md` take precedence if they disagree. Known external blockers:
#9/#127 (real Windows/macOS GPU results) and #21 (crates.io credentials/ownership).
