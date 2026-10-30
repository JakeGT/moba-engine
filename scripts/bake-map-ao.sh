#!/usr/bin/env bash
# Regenerates the baked ambient occlusion shipped in content/maps/ao/ (R2b, ADR-0016): an
# R8 occlusion map (<name>.ktx2) and its sidecar (<name>.toml) per example map with static
# occluders. Re-run after changing a map's walls or its visuals' props or wall height: the
# client ignores a bake whose fingerprint no longer matches (one log line).
#
#   scripts/bake-map-ao.sh
#
# The default grass map has no walls or props, so it has no bake.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q --release --locked -p moba-tools --bin ao-bake
bake=./target/release/ao-bake
"$bake" bake --pack content/ember-map.toml --root content --visuals ember
"$bake" bake --pack content/frost-map.toml --root content --visuals frost
