# Release checklist

The workspace is pre-1.0. Releases are Git tags with prebuilt artifacts; crates.io publishing
is opt-in until crate names/ownership are settled.

## Publish policy (0.4.0)

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
   review public API changes against [docs/API-STABILITY.md](docs/API-STABILITY.md). Run
   `scripts/semver-check.sh` (or `just semver`; needs
   `cargo install cargo-semver-checks --locked`), which diffs the library crates against the
   previous release tag (`--baseline-rev`, latest `v*` tag by default). CI's informational
   `semver (informational)` job runs the same script on every PR and push to `main`; check
   its summary on the last `main` run before tagging.
   - **No semver update required:** a patch release (`0.X.Y+1`) is allowed.
   - **Requires new major version:** the next release must bump the minor version
     (`0.X+1.0`), and the CHANGELOG must list the breaking changes.
   - The check never publishes anything and works with `publish = false`: the crates are
     passed explicitly with `-p`, because `--workspace` skips unpublishable crates.
3. **Quality** — `cargo fmt --all --check`, clippy with `-D warnings`, full tests, layering,
   and a short soak/fuzz for affected areas.
4. **Metadata** — set `repository`, `homepage`/`documentation` and per-crate `readme` when the
   public URL is known; confirm `license`, `keywords`, `categories` and `rust-version`.
5. **Package dry run** — `cargo package -p <crate> --allow-dirty` for every library crate
   (bottom-up; dependent crates cannot package until their internal dependencies are on the
   registry). Inspect the file list and license inclusion. Binaries are not published.
6. **Version** — bump `[workspace.package].version`; tag `vX.Y.Z`. Protocol and content changes
   must be called out in the tag notes.
7. **Artifacts** — push a `vX.Y.Z` tag. `.github/workflows/release.yml` runs release tests,
   inspects package file lists, builds Linux/Windows/macOS server/client/tools archives and
   attaches them to the GitHub release.
8. **crates.io later** — publish in dependency order after the policy above is approved:
   foundation crates, then game/sim/net, then the umbrella crate. Run `cargo package` and
   `scripts/semver-check.sh <previous tag>` first.

## Tagged release evidence

- `v0.1.0`: first preview tag, workflow-built Linux/Windows/macOS archives.
- `v0.2.0` (2026-10-18): release workflow run `39156546938` passed; attached
  `moba-linux-x86_64.tar.gz`, `moba-macos-aarch64.tar.gz`, `moba-windows-x86_64.zip` and
  `package-list.txt`. Package-list inspection and release tests passed.
- `v0.3.0` (2026-10-20): current preview after the audit remediation. Workspace **0.3.0**,
  protocol **v36**, replay **v5**, simulation logic **v7**, **304 tests**; full
  `scripts/agent-verify.sh --full` passed locally, golden state hash
  `0x8db35a0f5aab5190`. Release workflow run `39476849105` passed release tests,
  package-list inspection and Linux/Windows/macOS builds; artifacts are attached to the
  GitHub release. The run was dispatched from `main` because the tag is immutable; the only
  difference from the tag is the release-workflow dependency fix.
- `v0.4.0` (2026-10-21): security release for GHSA-jw7c-q9x9-cgg3 (published, patched 0.4.0). Workspace **0.4.0**, protocol **v37**, replay **v5**, simulation logic **v7**, 316 tests. `scripts/agent-verify.sh --full` passed locally (635 test runs). Release workflow run `39599870292` passed release tests, package metadata, Linux/Windows/macOS builds and asset attachment (`moba-linux-x86_64.tar.gz`, `moba-macos-aarch64.tar.gz`, `moba-windows-x86_64.zip`, `package-list.txt`). The 0.3.0 advisories GHSA-23q3, GHSA-vpcq, GHSA-9j54 and GHSA-v6g8 were published the same day with patched version 0.3.0.
- Remaining release blocker: **#21** registry publication (ownership + `CARGO_REGISTRY_TOKEN`);
  `publish = false` is intentional until that is approved. Real Windows/macOS GPU validation
  is tracked separately in **#9/#127**, and post-1.0 asset/skinning work in **#276**.

## Stability tiers

See [docs/API-STABILITY.md](docs/API-STABILITY.md). Crate names may change before 1.0; pin a
commit or tag if you build on the engine today.
