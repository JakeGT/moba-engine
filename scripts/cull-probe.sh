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
#
# CULL_PROBE_REUSE=<dir>: the four local scenes reuse the golden workflow's culled captures
# of the same scenes (`<dir>/render[-x]-actual.ppm` with its frame log `<dir>/render[-x].frames`)
# instead of capturing them again, so only their culling-off captures run. The networked scene
# always captures both itself: its HUD (a hero's health-bar ticks) can differ by a few pixels
# between two separate runs on a slow runner, unrelated to culling, so its pair runs together
# under the same conditions.
# CULL_PROBE_JOBS=<n> (default 1): captures run n at a time (the network pair first).
# Any validation message in a capture fails it.
set -euo pipefail
client=${1:-./target/release/moba-client}
out=${2:-cull-probe}
mkdir -p "$out"
reuse=${CULL_PROBE_REUSE:-}
jobs=${CULL_PROBE_JOBS:-1}
capture() {
  local name=$1
  shift
  # shellcheck disable=SC2086 # RUN is a command prefix.
  env MOBA_CAPTURE="$out/$name.ppm" MOBA_FRAME_LOG="$out/$name.frames" "$@" \
    timeout 300 ${RUN:-} "$client" > "$out/$name.log" 2>&1 || {
    cat "$out/$name.log"
    echo "FAIL: capture $name failed"
    return 1
  }
  test -s "$out/$name.ppm" || { echo "FAIL: no image for $name"; return 1; }
  if grep -q '^\[vulkan' "$out/$name.log"; then
    grep '^\[vulkan' "$out/$name.log" | head -20
    echo "FAIL: validation messages in $name"
    return 1
  fi
}
# Fields of the frame-log line: `field <file> <name>` prints the value of `name=`.
field() {
  tr ' ' '\n' < "$1" | sed -n "s/^$2=//p" | head -1
}
# name | golden capture of the same scene (render-golden.yml), or - to always capture both |
# environment (space-separated KEY=VALUE pairs, no spaces inside values; the golden step's
# environment exactly)
scenes=(
  "network|-|MOBA_SERVER=local MOBA_HERO_MODELS=sentinel-skinned MOBA_CAPTURE_TICK=120 MOBA_SMOKE_FRAMES=100000"
  "preview|render|MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90"
  "horizon|render-horizon|MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90 MOBA_PREVIEW_CAMERA=-0.7,0.12,170"
  "ultra-vat|render-ultra|MOBA_QUALITY=ultra MOBA_STRESS_MINIONS=64 MOBA_LOCAL_CAMERA_DISTANCE=40 MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90"
  "low-hull|render-low|MOBA_QUALITY=low MOBA_RENDER_SCALE=0.75 MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90"
)

# Every capture to run, as "<capture name>|<environment>".
tasks=()
names=()
for scene in "${scenes[@]}"; do
  name=${scene%%|*}
  rest=${scene#*|}
  golden=${rest%%|*}
  env_vars=${rest#*|}
  names+=("$name")
  if [ -n "$reuse" ] && [ "$golden" != - ]; then
    if [ ! -s "$reuse/$golden-actual.ppm" ] || [ ! -s "$reuse/$golden.frames" ]; then
      echo "FAIL: $reuse/$golden-actual.ppm or $reuse/$golden.frames is missing"
      exit 1
    fi
    cp "$reuse/$golden-actual.ppm" "$out/$name-culled.ppm"
    cp "$reuse/$golden.frames" "$out/$name-culled.frames"
  else
    tasks+=("$name-culled|$env_vars")
  fi
  tasks+=("$name-off|$env_vars MOBA_GPU_CULL=off")
done

status=0
running=0
for task in "${tasks[@]}"; do
  if [ "$running" -ge "$jobs" ]; then
    wait -n || status=1
    running=$((running - 1))
  fi
  read -r -a vars <<< "${task#*|}"
  capture "${task%%|*}" "${vars[@]}" &
  running=$((running + 1))
done
while [ "$running" -gt 0 ]; do
  wait -n || status=1
  running=$((running - 1))
done
[ "$status" = 0 ] || exit 1

for name in "${names[@]}"; do
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
