# Release checklist

The workspace is pre-1.0. Releases are Git tags; crates.io publishing is opt-in until crate
names/ownership are settled.

1. **Scope and notes** — update `CHANGELOG.md`, `docs/status.md` and any migration notes.
2. **API** — run `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS="-D warnings"`;
   review public API changes against [docs/API-STABILITY.md](docs/API-STABILITY.md). When
   `cargo-semver-checks` is available, compare against the previous tag for each library crate.
3. **Quality** — `cargo fmt --all --check`, clippy with `-D warnings`, full tests, layering,
   and a short soak/fuzz for affected areas.
4. **Metadata** — set `repository`, `homepage`/`documentation` and per-crate `readme` when the
   public URL is known; confirm `license`, `keywords`, `categories` and `rust-version`.
5. **Package dry run** — `cargo package -p <crate> --allow-dirty` for every library crate;
   inspect the file list and license inclusion. Binaries are not published.
6. **Version** — bump `[workspace.package].version`; tag `vX.Y.Z`. Protocol and content changes
   must be called out in the tag notes.
7. **Artifacts** — attach Linux/Windows/macOS client/server builds and the validation-clean
   screenshots where practical.

## Stability tiers

See [docs/API-STABILITY.md](docs/API-STABILITY.md). Crate names may change before 1.0; pin a
commit or tag if you build on the engine today.
