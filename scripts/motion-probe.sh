#!/usr/bin/env bash
# Motion-vector probe (ADR-0021 verification gate 2): the client renders two scripted frames
# (a panned camera, a translated static instance, a skinned and a VAT instance, foliage in a
# steady wind, a `motion_reset` and two unkeyed instances) on the tiered path with temporal
# AA, reads back frame B's motion and depth, and compares every classified pixel with a CPU
# twin; each class must match within 1/64 render pixel.
#
#   scripts/motion-probe.sh [client binary] [output dir]
#
# CI runs it under lavapipe with validation (render-golden.yml); set RUN="xvfb-run -a" when
# there is no display.
set -euo pipefail
client=${1:-./target/release/moba-client}
out=${2:-motion-probe}
mkdir -p "$out"
status=0
for quality in low high; do
  log="$out/motion-$quality.log"
  # shellcheck disable=SC2086 # RUN is a command prefix.
  if env MOBA_MOTION_PROBE=1 MOBA_RENDER_PATH=tiered MOBA_AA=temporal \
    MOBA_QUALITY="$quality" timeout 180 ${RUN:-} "$client" > "$log" 2>&1; then
    echo "ok: motion probe ($quality)"
  else
    echo "FAIL: motion probe ($quality)"
    status=1
  fi
  grep -E "motion probe|^  (ok|FAIL)" "$log" || cat "$log"
  if grep -E '^\[vulkan' "$log"; then
    echo "FAIL: validation messages from the motion probe ($quality)"
    status=1
  fi
done
exit "$status"
