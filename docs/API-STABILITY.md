# API stability policy

The workspace is **pre-1.0 preview**. There is no semver guarantee yet, but the following
tiers make breakage predictable and reviews honest.

| Tier | Crates | Promise |
|---|---|---|
| Foundation | `moba-core`, `moba-ecs`, `moba-jobs` | API is settling; breaking changes documented and deprecation preferred within a minor line |
| Engine | `moba-game`, `moba-sim`, `moba-net`, `moba-nav`, `moba-render`, `moba-platform`, `moba-assets` | Preview: breaking changes allowed between minor versions; migration notes in `CHANGELOG.md` |
| Applications | `moba-client`, `moba-server`, `moba-tools`, `moba-netsim`, `moba-shaders` | Binaries/internal crates; no API promise |
| Embedding | `moba-engine` | Umbrella facade for embedders; follows the Engine tier. Prefer feature-gated re-exports over new facade-only types. |

The intended 1.0 surface of every crate above, and which types are `#[non_exhaustive]`, exhaustive
by design or public but unstable, is listed in [api-1.0.md](api-1.0.md).

## Rules for changes

1. Prefer additive changes: new types, new fields and new enum variants.
   - Error, event, outcome, phase, command and key enums, read-only stats/report/result
     structs, and configs are `#[non_exhaustive]`, so a new variant or field is not a breaking
     change. Configs pair that with `Default` and one `with_*` builder per field; add a builder
     with every new config field.
   - A new public enum or struct in those categories must be `#[non_exhaustive]` from the start.
   - The content schema (`GameContent`, `*Spec`, content enums), the wire message structs and
     closed value types are exhaustive by design ([api-1.0.md](api-1.0.md#conventions)). A new
     field or variant there is a breaking change; it also bumps the content hash or
     `PROTOCOL_VERSION`.
2. Renaming/removing public items requires a `CHANGELOG.md` entry and a deprecation where
   feasible.
3. Wire protocol changes bump `PROTOCOL_VERSION`; never reinterpret an existing version.
4. Determinism-affecting changes must update golden/hash tests and be called out in the
   changelog.
5. Content table changes affect replays; note the content version.

## Verification

- `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS="-D warnings"` must pass.
- CI runs the test suite on Linux, Windows and macOS.
- `cargo-semver-checks` runs in CI as the informational `semver (informational)` job
  ([ci.yml](../.github/workflows/ci.yml), [scripts/semver-check.sh](../scripts/semver-check.sh)).
  - It compares the Foundation, Engine and Embedding crates against the latest release tag
    (`--baseline-rev`, currently `v0.4.0`). The baseline is a git tag, not crates.io, so it
    works with `publish = false` (#21).
  - It is not a required check, because pre-1.0 minors may break the API. A breaking change
    shows up as a warning and a job summary.
  - A "requires new major version" verdict means the next release must bump the minor
    version (`0.X+1.0`) and list the break in `CHANGELOG.md`.
  - Run it locally with `scripts/semver-check.sh [TAG]`.
- The release workflow verifies package file lists and artifacts; registry publication
  order/policy is documented in `RELEASE.md`.
- `cargo-public-api` diffs are reviewed when preparing release notes.

## Publishing

`publish = false` currently prevents accidental uploads while crate names and ownership are
settled. To publish a release, set the workspace publish policy, add repository/readme paths
per crate, run `cargo package` for every library crate, and tag the workspace. Binaries are
not published to crates.io.

Rust API guidelines and Cargo semver conventions are the reference for naming and breakage:
<https://rust-lang.github.io/api-guidelines/> and
<https://doc.rust-lang.org/cargo/reference/semver.html>.
