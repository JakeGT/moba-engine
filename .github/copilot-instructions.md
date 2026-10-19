# GitHub Copilot instructions

The authoritative repository guide is [`AGENTS.md`](../AGENTS.md). Read it before editing.
Use [`docs/agents.md`](../docs/agents.md) for the agent workflow, current contract versions,
known blockers and verification commands.

Minimum gate before proposing a change:

```bash
scripts/agent-verify.sh          # or `just ci-core` if just is installed
```

Additional requirements by area:

- wire/protocol/content/replay changes: bump the relevant version and update
  `docs/tasks/0005-networking.md`, `CHANGELOG.md`, parity tests and docs;
- renderer changes: run `MOBA_VULKAN_VALIDATION=required` capture and the
  `render-golden` workflow when the golden image changes;
- performance changes: record evidence in `docs/perf/`, do not invent numbers;
- security changes: update `SECURITY.md` and add negative tests.

Do not push to `main`. Branch, open a PR with `.github/PULL_REQUEST_TEMPLATE.md`, and wait for
the required checks (`gates` on 3 OSes, `tools + deterministic soak`, `deny`, `miri`).
