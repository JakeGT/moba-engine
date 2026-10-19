# API stability policy

The workspace is **pre-1.0 preview**. There is no semver guarantee yet, but the following
tiers make breakage predictable and reviews honest.

| Tier | Crates | Promise |
|---|---|---|
| Foundation | `moba-core`, `moba-ecs`, `moba-jobs` | API is settling; breaking changes documented and deprecation preferred within a minor line |
| Engine | `moba-game`, `moba-sim`, `moba-net`, `moba-nav`, `moba-render`, `moba-platform` | Preview: breaking changes allowed between minor versions; migration notes in `CHANGELOG.md` |
| Applications | `moba-client`, `moba-server`, `moba-tools`, `moba-netsim`, `moba-shaders` | Binaries/internal crates; no API promise |
| Embedding | `moba-engine` | Umbrella facade for embedders; follows the Engine tier. Prefer feature-gated re-exports over new facade-only types. |

## Rules for changes

1. Prefer additive changes: new types, new fields behind constructors, and new enum variants
   only after marking the enum `#[non_exhaustive]` (most public enums are not yet marked, so
   adding a variant is a breaking change today).
2. Renaming/removing public items requires a `CHANGELOG.md` entry and a deprecation where
   feasible.
3. Wire protocol changes bump `PROTOCOL_VERSION`; never reinterpret an existing version.
4. Determinism-affecting changes must update golden/hash tests and be called out in the
   changelog.
5. Content table changes affect replays; note the content version.

## Verification

- `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS="-D warnings"` must pass.
- CI runs the test suite on Linux, Windows and macOS.
- `cargo-semver-checks` is not wired into CI yet (#21); compare against `v0.2.0` before the
  next release. The release workflow verifies package file lists and artifacts; registry
  publication order/policy is documented in `RELEASE.md`.
- `cargo-public-api` diffs are reviewed when preparing release notes.

## Publishing

`publish = false` currently prevents accidental uploads while crate names and ownership are
settled. To publish a release, set the workspace publish policy, add repository/readme paths
per crate, run `cargo package` for every library crate, and tag the workspace. Binaries are
not published to crates.io.

Rust API guidelines and Cargo semver conventions are the reference for naming and breakage:
<https://rust-lang.github.io/api-guidelines/> and
<https://doc.rust-lang.org/cargo/reference/semver.html>.
