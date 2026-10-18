# MOBA Engine — Claude Code entry point

The authoritative agent guide is **[AGENTS.md](AGENTS.md)** (layering, determinism, commands,
workflow, issue tracker). Machine-specific setup is in `agents.local.md` (gitignored; template
at `agents.local.example.md`).

Claude-specific hooks and reviewer definitions live in `.claude/`:
`perf-reviewer`, `gpu-safety-reviewer`, `netcode-reviewer` and the rustfmt/cargo-check hook.
They remain available, but project rules in `AGENTS.md` take precedence if they disagree.

Quick gates: `cargo fmt --all --check`, clippy `-D warnings`, `cargo test --workspace`,
rustdoc `-D warnings`, plus the content/replay/soak checks relevant to the change.
