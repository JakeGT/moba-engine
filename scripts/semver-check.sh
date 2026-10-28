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

# Foundation, Engine and Embedding tiers from docs/API-STABILITY.md.
CRATES=(
  moba-core moba-ecs moba-jobs
  moba-game moba-sim moba-net moba-nav moba-render moba-platform
  moba-engine
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
RUSTFLAGS="" cargo semver-checks --baseline-rev "$baseline" "${args[@]}"
