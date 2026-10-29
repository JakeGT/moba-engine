#!/usr/bin/env bash
# Regenerates the self-generated sample texture set shipped in content/textures/ (ADR-0010).
#
#   scripts/bake-sample-textures.sh
#
# The sources are procedural (crates/tools/src/bin/texture-bake/patterns.rs), so no external
# art is involved; the PNGs are written to a temporary directory and not committed.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q --release --locked -p moba-tools --bin texture-bake
bake=./target/release/texture-bake
src=$(mktemp -d)
trap 'rm -rf "$src"' EXIT
"$bake" generate sentinel "$src"
mkdir -p content/textures
"$bake" bake "$src/sentinel-base.png" content/textures/sentinel-base.ktx2 --kind color
"$bake" bake "$src/sentinel-normal.png" content/textures/sentinel-normal.ktx2 --kind normal
"$bake" bake "$src/sentinel-mr.png" content/textures/sentinel-mr.ktx2 --kind data
"$bake" bake "$src/sentinel-mask.png" content/textures/sentinel-mask.ktx2 --kind mask
