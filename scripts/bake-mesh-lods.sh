#!/usr/bin/env bash
# Re-bakes the index-only mesh LODs of the sample models (R8, ADR-0018 §3):
# content/models/<model>.lod next to each content/models/<model>.glb. The samples are
# flat-shaded low-poly models (every edge is a hard edge), so they bake --permissive.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

cargo build --release --locked -q -p moba-tools --bin mesh-bake
for model in content/models/*.glb; do
  echo "== ${model}"
  ./target/release/mesh-bake lod "${model}" --permissive || echo "   (no levels for ${model})"
done
