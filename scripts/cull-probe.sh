#!/usr/bin/env bash
# Culling exactness check (R8, ADR-0018 §1): frustum culling must never change what a player
# sees. Each scene is captured with culling on (the default) and with MOBA_GPU_CULL=off, and
# the two captures must be byte-identical. The frame log of the culled run must show that
# something was culled (units or terrain chunks), so the check is not vacuous.
#
#   scripts/cull-probe.sh [client binary] [output dir]
#
# Captures are pinned (fixed tick per frame), so they are reproducible on one driver. CI
# runs it under lavapipe with validation (render-golden.yml); set RUN="xvfb-run -a" when
# there is no display.
set -euo pipefail
client=${1:-./target/release/moba-client}
out=${2:-cull-probe}
mkdir -p "$out"
capture() {
  local name=$1
  shift
  # shellcheck disable=SC2086 # RUN is a command prefix.
  env MOBA_CAPTURE="$out/$name.ppm" MOBA_FRAME_LOG="$out/$name.frames" "$@" \
    timeout 300 ${RUN:-} "$client" > "$out/$name.log" 2>&1 || { cat "$out/$name.log"; exit 1; }
  test -s "$out/$name.ppm"
}
# Fields of the frame-log line: `field <file> <name>` prints the value of `name=`.
field() {
  tr ' ' '\n' < "$1" | sed -n "s/^$2=//p" | head -1
}
status=0
# name | environment (space-separated KEY=VALUE pairs, no spaces inside values)
scenes=(
  "preview|MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90"
  "horizon|MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90 MOBA_PREVIEW_CAMERA=-0.7,0.12,170"
  "ultra-vat|MOBA_QUALITY=ultra MOBA_STRESS_MINIONS=64 MOBA_LOCAL_CAMERA_DISTANCE=40 MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90"
  "low-hull|MOBA_QUALITY=low MOBA_RENDER_SCALE=0.75 MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90"
  "network|MOBA_SERVER=local MOBA_HERO_MODELS=sentinel-skinned MOBA_CAPTURE_TICK=120 MOBA_SMOKE_FRAMES=100000"
)
for scene in "${scenes[@]}"; do
  name=${scene%%|*}
  read -r -a vars <<< "${scene#*|}"
  capture "$name-culled" "${vars[@]}"
  capture "$name-off" "${vars[@]}" MOBA_GPU_CULL=off
  candidates=$(field "$out/$name-culled.frames" cull_candidates)
  drawn=$(field "$out/$name-culled.frames" cull_drawn)
  chunks=$(field "$out/$name-culled.frames" terrain_chunks)
  off_drawn=$(field "$out/$name-off.frames" cull_drawn)
  culled=no
  if [ "${drawn:-0}" -lt "${candidates:-0}" ] || [ "${chunks%%/*}" != "${chunks##*/}" ]; then
    culled=yes
  fi
  if ! cmp -s "$out/$name-culled.ppm" "$out/$name-off.ppm"; then
    echo "FAIL: culling changed the $name frame (units $drawn/$candidates, terrain $chunks)"
    status=1
  elif [ "$off_drawn" != "$candidates" ]; then
    echo "FAIL: MOBA_GPU_CULL=off still culled in $name ($off_drawn of $candidates units)"
    status=1
  else
    echo "ok: $name identical with culling on and off (units $drawn/$candidates, terrain chunks $chunks, culled: $culled)"
  fi
  if [ "$culled" = no ] && [ "$name" = horizon ]; then
    echo "FAIL: the horizon view culled nothing; the probe would be vacuous"
    status=1
  fi
done
exit "$status"
