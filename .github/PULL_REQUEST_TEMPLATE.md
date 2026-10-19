## Summary

Describe the change and the problem it solves.

## Area

- [ ] core/ECS/jobs
- [ ] simulation/game/content
- [ ] networking/security
- [ ] rendering/platform/client
- [ ] tooling/docs/CI

## Contract impact

- [ ] no wire/replay/content/behavior change
- [ ] wire protocol bumped and documented (`PROTOCOL_VERSION`, task 0005, CHANGELOG)
- [ ] replay/simulation logic version bumped and golden fixtures updated
- [ ] content schema/hash changed; replay/content-check impact documented
- [ ] state hash or golden image rebaselined deliberately (commit/run linked)

## Verification

- [ ] `scripts/agent-verify.sh` (or the equivalent `just ci-core`/explicit gates)
- [ ] `cargo test --workspace --release --locked` for releases/contract changes
- [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`
- [ ] determinism/replay/soak/fuzz/render-validation evidence attached where relevant
- [ ] docs/status, task log, CHANGELOG and screenshots updated where relevant

## Evidence

Commands, workflow runs, captures and measured numbers (do not invent hardware results).

## Notes

Known limitations, follow-ups and external blockers (hardware, registry credentials, approvals).
See `AGENTS.md` and `docs/agents.md` for the required workflow.
