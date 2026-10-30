#!/usr/bin/env bash
# Outline occlusion check (R2b, ADR-0016): a unit fully behind a wall must give zero outline
# pixels on both outline paths. The VFX preview arena stages an enemy-hero-outlined unit
# (MOBA_OUTLINE_PROBE): `hidden` puts it behind a wall, and that capture must be
# byte-identical to the same frame with MOBA_OUTLINES=off, on the hull path (Low) and the
# screen-space path (High); `open` is the control, where each path must change the frame.
#
#   scripts/outline-probe.sh [client binary] [output dir]
#
# Captures are pinned (fixed tick per frame), so they are reproducible on one driver. CI
# runs it under lavapipe with validation (render-golden.yml); set RUN="xvfb-run -a" when
# there is no display.
set -euo pipefail
client=${1:-./target/release/moba-client}
out=${2:-outline-probe}
mkdir -p "$out"
capture() {
  local name=$1
  shift
  # shellcheck disable=SC2086 # RUN is a command prefix.
  env MOBA_VFX_PREVIEW=cast-flash MOBA_SMOKE_FRAMES=30 MOBA_CAPTURE_FRAME=20 \
    MOBA_CAPTURE="$out/$name.ppm" "$@" timeout 180 ${RUN:-} "$client" \
    > "$out/$name.log" 2>&1 || { cat "$out/$name.log"; exit 1; }
  test -s "$out/$name.ppm"
}
status=0
for probe in hidden open; do
  for case in low:hull high:screen; do
    quality=${case%%:*}
    path=${case##*:}
    capture "$probe-$quality-off" MOBA_OUTLINE_PROBE="$probe" MOBA_QUALITY="$quality" \
      MOBA_OUTLINES=off
    capture "$probe-$quality-$path" MOBA_OUTLINE_PROBE="$probe" MOBA_QUALITY="$quality" \
      MOBA_OUTLINES="$path"
    if cmp -s "$out/$probe-$quality-off.ppm" "$out/$probe-$quality-$path.ppm"; then
      same=yes
    else
      same=no
    fi
    if [ "$probe" = hidden ] && [ "$same" = no ]; then
      echo "FAIL: $path outline of a unit behind a wall reached the screen ($quality)"
      status=1
    elif [ "$probe" = open ] && [ "$same" = yes ]; then
      echo "FAIL: $path outline of a unit in the open did not draw ($quality)"
      status=1
    else
      echo "ok: $probe unit, $path outlines ($quality): identical to outlines off: $same"
    fi
  done
done
exit "$status"
