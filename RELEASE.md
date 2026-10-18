# Release checklist

The workspace is pre-1.0. Releases are Git tags with prebuilt artifacts; crates.io publishing
is opt-in until crate names/ownership are settled.

## Publish policy (0.2.0)

- **Intended for crates.io later:** `moba-core`, `moba-ecs`, `moba-jobs`, `moba-nav`,
  `moba-game`, `moba-sim`, `moba-net`, `moba-engine`.
- **Git-only for now:** `moba-platform`, `moba-shaders`, `moba-render`, `moba-assets`,
  `moba-client`, `moba-server`, `moba-tools`, `moba-netsim` (binary/internal or shader-build
  dependent).
- `publish = false` remains authoritative until the owner list, names and dependency
  publication order are approved. `cargo package` for a crate depending on unpublished
  workspace crates requires publishing from the bottom up; the release workflow verifies the
  leaf package, inspects every package file list, and builds artifacts from the tag.

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
7. **Artifacts** — push a `vX.Y.Z` tag. `.github/workflows/release.yml` runs release tests,
   inspects package file lists, builds Linux/Windows/macOS server/client/tools archives and
   attaches them to the GitHub release.
8. **crates.io later** — publish in dependency order after the policy above is approved:
   foundation crates, then game/sim/net, then the umbrella crate. Run `cargo package` and
   `cargo semver-checks` against the previous tag first.

## Tagged release evidence

The 0.1.0 preview tag is `v0.1.0`; the 0.2.0 preview tag is `v0.2.0`; release notes are generated from commits and the three
platform archives are attached by the release workflow.

## Stability tiers

See [docs/API-STABILITY.md](docs/API-STABILITY.md). Crate names may change before 1.0; pin a
commit or tag if you build on the engine today.
