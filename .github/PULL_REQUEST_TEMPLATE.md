## Summary

<!-- What changed and why. Link an issue/spec. -->

## Verification

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] `cargo doc --workspace --no-deps`
- [ ] Layering check (`just layering` or documented equivalent)
- [ ] Bench/soak/fuzz run for hot-path or netcode changes (attach numbers)

## Impact

- [ ] Determinism/state-hash changes (explain and update tests)
- [ ] Protocol version bump (version: __)
- [ ] Public API change (note in CHANGELOG)
- [ ] Content/balance change (note replay impact)
- [ ] Visual change (screenshot attached)

## Notes for reviewers

<!-- Risks, alternatives considered, follow-ups. -->
