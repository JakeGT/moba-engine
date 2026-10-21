# Agentic coding guide

This is the operational runbook for autonomous or semi-autonomous coding agents. The
normative rules and invariants live in [`AGENTS.md`](../AGENTS.md); this page explains how to
work without breaking the repository or overstating results. Claude-specific reviewer prompts
are in [`.claude/agents/`](../.claude/agents), and the short GitHub Copilot entry point is
[`.github/copilot-instructions.md`](../.github/copilot-instructions.md).

## Current contract (2026-10-20)

| Item | Value | Source of truth |
|---|---|---|
| Workspace version | `0.4.0` pre-1.0 | `Cargo.toml` |
| Wire protocol | **v38** | `crates/net/src/protocol.rs` |
| Replay format | **v5**, simulation logic **v7** | `crates/sim/src/replay.rs` |
| Content | runtime TOML, hashed; signed packs optional |
| Toolchain | Rust MSRV **1.88**, Vulkan **1.3** minimum | `Cargo.toml`, ADR-0002 | `crates/game/src/content.rs` |
| Tests | **324 passed**, 3 ignored | `cargo test --workspace --locked` |
| Golden state hash | `0x8db35a0f5aab5190` | `tests/golden-state-hash.txt` |
| Golden image | 64×36 fingerprint, lavapipe + validation | `.github/workflows/render-golden.yml` |

Always re-run the commands rather than copying these numbers into a PR if they are stale.

## Start-of-task checklist

1. `git status --short` and `git branch --show-current` — never assume a clean `main`.
2. `git switch main && git fetch origin && git reset --hard origin/main` (the local branch may
   be an old squash-merged branch).
3. Read the linked issue and acceptance criteria; search for existing work before coding.
4. Claim the issue (comment) and create a branch from `main`: `feat/…`, `fix/…`, `docs/…`,
   `perf/…`, `ci/…`, `security/…`.
5. If the task needs hardware, credentials or an upstream release, say so immediately and
   record the blocker instead of fabricating evidence.

## Choosing work

Priority order:

1. correctness/security/determinism regressions;
2. broken CI or docs that contradict code;
3. issues with complete acceptance criteria and an available environment;
4. roadmap items that do not depend on external services.

Check the open issues and the completed audit plan first:

```bash
gh issue list --repo JakeGT/moba-engine --state open
gh issue view <n> --repo JakeGT/moba-engine
```

Known external blockers (do not silently work around them):

- **#9/#127** — real Windows/macOS GPU frame results. Linux/NVIDIA evidence exists; hosted
  macOS records p95 16.945 ms and the Windows runner has no Vulkan device.
- **#21** — crates.io publishing needs registry ownership and a `CARGO_REGISTRY_TOKEN`.
  `publish = false` is intentional; Git tags/artifacts are the current distribution path.

## Branch and PR lifecycle

```bash
git switch main && git fetch origin && git reset --hard origin/main
git switch -c feat/<issue>-<slug>
# edit + verify
scripts/agent-verify.sh
git add -A && git commit -m "feat(area): summary"
git push -u origin HEAD
gh pr create --repo JakeGT/moba-engine --base main --head "$(git branch --show-current)" \
  --title "feat(area): summary" --body-file /tmp/pr-body.md
gh pr checks <n> --repo JakeGT/moba-engine --watch --interval 20
gh pr merge <n> --repo JakeGT/moba-engine --squash --delete-branch
git fetch origin && git reset --hard origin/main
```

Rules that have bitten previous agents:

- Never commit directly to `main`; branch protection requires PRs and linear history.
- After a squash merge, local `main` may still contain the pre-squash commit. Always
  `fetch && reset --hard origin/main` instead of pulling.
- If you start editing before branching, create the branch immediately; the uncommitted work
  moves with you.
- The PR body must state protocol/content/replay impact, commands run and evidence. Avoid
  shell-expanding backticks in unquoted `--body "..."`; use a body file.
- Required checks: `gates (ubuntu-latest|windows-latest|macos-latest)`,
  `tools + deterministic soak`, `deny`, `miri`; release/MSRV may also run.
- `render-golden` is manual: `gh workflow run render-golden.yml --ref main`. Rebaseline only
  from the CI-produced capture, not a differently sized local window.

## Verification matrix

Run the smallest sufficient gate, then the full fast gate before merge:

```bash
scripts/check-docs.sh            # links, protocol/replay/logic versions, front-door test counts
scripts/agent-verify.sh          # fmt/clippy/tests/doc/layering/hash/content/fuzz-check
scripts/agent-verify.sh --full   # + release tests/features/loom/bench compile/soak/fuzz
```

Also run:

| Change | Extra evidence |
|---|---|
| wire/protocol/replay/content schema | version bump, parity/round-trip tests, replay/content-check, update task docs |
| simulation/gameplay | state hash, allocation guard, match-soak, balance notes |
| renderer/UI | `MOBA_VULKAN_VALIDATION=required` capture; golden workflow if pixels change |
| network/server | real-UDP tests, soak/fuzz, bandwidth numbers; no secrets in logs |
| performance | before/after in `docs/perf/`; never invent hardware results |
| docs/agent setup | link checker, stale version/count scan, `scripts/agent-verify.sh` still passes |

## Evidence and honesty

- Commit only outputs you actually saw. Quote commands, dates, hardware/driver and exact
  numbers.
- External/hosted results must name the runner and limitation (e.g. macOS CI is virtualised;
  Windows hosted runners have no Vulkan GPU).
- Do not mark an issue closed unless every acceptance checkbox has linked evidence.
- If a task is blocked by credentials/hardware, leave the goal/issue open and describe the
  concrete unblock action.

## Area map for reviewers

- **Simulation/determinism:** `crates/sim`, `crates/game`, `crates/nav`, `crates/ecs`.
- **Netcode/security:** `crates/net`, `crates/netsim`, `crates/server`.
- **GPU/unsafe:** `crates/render`, `crates/shaders`, `crates/platform`.
- **Client/HUD/audio/settings:** `crates/client`, `crates/platform`.
- **Content:** `crates/game/src/content.rs`, `content/`, `docs/content-authoring.md`.
- **Tools/CI:** `crates/tools`, `.github/workflows`, `fuzz/`, `scripts/`.

Run the matching `.claude/agents/*-reviewer.md` prompt before merging risky changes.

## Handoff template

When stopping or blocking, report:

1. branch/PR and merged state;
2. what changed and verification output;
3. open issues/checkboxes and why they remain;
4. exact external requirement (hardware/credential/approval);
5. next safe command for the next agent.
