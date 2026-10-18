## Summary

Describe the change and the problem it solves.

## Area

- [ ] core/ECS/jobs
- [ ] simulation/game/content
- [ ] networking/security
- [ ] rendering/platform/client
- [ ] tooling/docs/CI

## Verification

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`
- [ ] determinism/replay/relevant soak evidence attached

## Notes

Protocol/content changes must state the version/hash impact and include migration notes.
