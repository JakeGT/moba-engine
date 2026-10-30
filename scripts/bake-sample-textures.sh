#!/usr/bin/env bash
# Regenerates the self-generated textures shipped in content/textures/ (ADR-0010): the
# `sentinel` material sample set and the `terrain-*` layer set (albedo + AO, normal; ADR-0014),
# the particle atlas content/vfx/atlas.ktx2 (ADR-0013) and the colour-grading LUTs in
# content/grading/ (R6).
#
#   scripts/bake-sample-textures.sh
#
# The sources are procedural (crates/tools/src/bin/texture-bake/patterns.rs and
# moba_assets::procedural_atlas), so no external
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
"$bake" generate terrain "$src"
for layer in grass dirt rock moss pebbles; do
  "$bake" bake "$src/terrain-$layer.png" "content/textures/terrain-$layer.ktx2" --kind color
  "$bake" bake "$src/terrain-$layer-normal.png" "content/textures/terrain-$layer-normal.ktx2" --kind normal
done
"$bake" generate vfx "$src"
mkdir -p content/vfx
"$bake" bake "$src/vfx-atlas.png" content/vfx/atlas.ktx2 --kind data
# Colour-grading LUTs for the sample maps' [post] tables (R6): display-encoded in and out,
# 17³ points (~130 KB each). The grades are analytic (crates/tools/src/bin/grade-bake/grade.rs).
cargo build -q --release --locked -p moba-tools --bin grade-bake
mkdir -p content/grading
for preset in ember frost meadow; do
  ./target/release/grade-bake bake --preset "$preset" --size 17 "content/grading/$preset.cube"
done
