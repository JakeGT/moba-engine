#!/usr/bin/env bash
# Re-bakes the index-only mesh LODs of the sample models (R8, ADR-0018 §3):
# content/models/<model>.lod next to each content/models/<model>.glb, with mesh-bake's
# defaults: permissive (the samples are flat-shaded, so every edge is a hard edge), normal and
# UV attribute weights, and the error measured against LOD0 (posed, for skinned models).
# The output is deterministic, and the same on every GCC or Clang build: `just ci-tools`
# re-bakes and diffs the committed files. Needs the optional meshoptimizer build
# (`--features mesh-bake`).
#
# meshoptimizer is C++, and its attribute-weighted simplification is sensitive to fused
# multiply-adds: a compiler allowed to contract `a * b + c` into an FMA (GCC and Clang do by
# default whenever the target has FMA, e.g. `-march=native`) bakes
# different low bits, sometimes different collapses. `-ffp-contract=off` keeps every C/C++
# float operation separately rounded, so GCC 16, Clang and `-march=native` builds all bake
# the committed bytes (ADR-0018 §3). Rust never contracts.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

export CFLAGS="${CFLAGS:-} -ffp-contract=off"
export CXXFLAGS="${CXXFLAGS:-} -ffp-contract=off"
cargo build --release --locked -q -p moba-tools --features mesh-bake --bin mesh-bake
for model in content/models/*.glb; do
  echo "== ${model}"
  ./target/release/mesh-bake lod "${model}" || echo "   (no levels for ${model})"
done
# The .lod fuzz target's seeds are the committed files (bound to their models by hash).
cp content/models/*.lod fuzz/corpus/mesh_lod/
