#!/usr/bin/env bash
# Regenerates the sample lighting environments shipped in content/environments/ (R2a
# image-based lighting): a prefiltered RGBA16F specular cube map (<name>.ktx2) and the SH-L2
# diffuse descriptor (<name>.toml) per preset.
#
#   scripts/bake-sample-environments.sh
#
# The skies are analytic (crates/tools/src/bin/env-bake/sky.rs), so no external HDRI is
# involved and the files are the repository's own.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q --release --locked -p moba-tools --bin env-bake
bake=./target/release/env-bake
mkdir -p content/environments
for preset in meadow ember frost; do
  "$bake" bake --preset "$preset" --size 64 --samples 256 "content/environments/$preset"
done
