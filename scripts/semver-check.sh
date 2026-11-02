#!/usr/bin/env bash
# Public-API breakage report for the library crates against a release tag.
#
# Usage:
#   scripts/semver-check.sh            # baseline = latest v* tag reachable from HEAD
#   scripts/semver-check.sh v0.4.0     # explicit baseline tag/rev
#
# Needs `cargo install cargo-semver-checks --locked` and the baseline tag locally
# (`git fetch --tags`). Exit codes are cargo-semver-checks': 0 = no semver update required,
# non-zero = a breaking change needs a bigger version bump (or the tool failed). The workspace
# is pre-1.0, so a "major" verdict means the next release must bump the minor version
# (0.X.0); see docs/API-STABILITY.md.
#
# Every crate has `publish = false` (#21). cargo-semver-checks skips unpublishable crates under
# `--workspace`, so the crates with an API promise are listed explicitly here; explicit `-p`
# selections are checked regardless of `publish`. Nothing is published or uploaded.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

# Foundation, Engine and Embedding tiers from docs/API-STABILITY.md. `moba-assets` is in the
# Engine tier because `moba-game` and `moba-render` re-export its types.
#
# cargo-semver-checks turns on every feature that does not look unstable by default. That
# would include moba-render's vendor SDK features (ADR-0020 §6), whose build script fails
# without the fetched proprietary SDKs, so the two crates that have them are checked with
# their default features plus the open build's: moba-render's default is `fsr3`, and
# moba-engine adds `render`. The other crates keep the default heuristic.
CRATES=(
  moba-core moba-ecs moba-jobs
  moba-game moba-sim moba-net moba-nav moba-platform moba-assets
)

baseline=${1:-}
if [[ -z $baseline ]]; then
  baseline=$(git describe --tags --abbrev=0 --match 'v[0-9]*' HEAD)
fi

if ! cargo semver-checks --version >/dev/null 2>&1; then
  echo "error: cargo-semver-checks is not installed (cargo install cargo-semver-checks --locked)" >&2
  exit 2
fi

echo "semver baseline: $baseline"
args=()
for krate in "${CRATES[@]}"; do
  args+=(-p "$krate")
done
# Warnings must not fail the baseline build on a newer toolchain; this is an API diff, not a lint.
# Every run happens, and the script fails if any of them did.
status=0
RUSTFLAGS="" cargo semver-checks --baseline-rev "$baseline" "${args[@]}" || status=1
RUSTFLAGS="" cargo semver-checks --baseline-rev "$baseline" -p moba-render \
  --default-features || status=1
RUSTFLAGS="" cargo semver-checks --baseline-rev "$baseline" -p moba-engine \
  --default-features --features render || status=1
exit "$status"
