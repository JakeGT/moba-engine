#!/usr/bin/env bash
# Terrain decal-cost guard (R3b, ADR-0017 §1). The terrain fragment shader carries the decal
# code, and the terrain's cost has a cliff: during R3b a local copy of the decal record
# doubled the terrain's cost on the Ultra overview, decals in view or not. Run this after any
# change to `terrain.frag` or `include/decals.glsl`, on a real GPU. Two checks:
#
# 1. The decal code's fixed cost. The networked whole-map overview (no decal in view) on
#    Ultra, with decals on and with MOBA_DECALS=off, which builds the terrain from
#    `terrain_nodecals.frag` (the decal code compiled out). The scene pass's GPU p50 with
#    decals on must stay within RATIO (default 1.15) of the one without: the shipped shader is
#    within 2 % on the RTX 5070 Ti, the record copy was 1.6x. Two interleaved rounds, the
#    faster of each kept (the GPU's power state moves the small numbers).
# 2. Registers. MOBA_PIPELINE_STATS=1 enables VK_KHR_pipeline_executable_properties and the
#    client logs the terrain pipeline's statistics; its fragment shader must need at most
#    REGISTERS (default 64, the RTX 5070 Ti's count at R3b). A driver without the extension
#    or a `Register Count` statistic (lavapipe; RADV names its counters VGPRs/SGPRs) skips
#    this check. It catches register growth only: the record copy stayed at 64 registers.
#
#   scripts/terrain-decal-check.sh [client binary]
#
# Set RUN="xvfb-run -a" when there is no display. Not for lavapipe: the timing needs a GPU.
set -euo pipefail
client=${1:-./target/release/moba-client}
ratio=${RATIO:-1.15}
registers_max=${REGISTERS:-64}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
status=0

run() {
  local name=$1
  shift
  # shellcheck disable=SC2086 # RUN is a command prefix.
  env MOBA_QUALITY=ultra MOBA_VSYNC=0 MOBA_PIPELINE_CACHE=off MOBA_VULKAN_VALIDATION=off \
    "$@" timeout 300 ${RUN:-} "$client" > "$work/$name.log" 2>&1 || {
    cat "$work/$name.log"
    exit 1
  }
}

scene_p50() {
  sed -n 's/.*gpu_scene_p50_ms=\([0-9.]*\).*/\1/p' "$1" | tail -1
}

for round in 1 2; do
  for decals in on off; do
    extra=()
    [ "$decals" = off ] && extra=(MOBA_DECALS=off)
    run "overview-$decals-$round" MOBA_SERVER=local MOBA_CAMERA=overview MOBA_SMOKE_FRAMES=1500 \
      MOBA_FRAME_LOG="$work/overview-$decals-$round.txt" "${extra[@]}"
  done
done
best() {
  local a b
  a=$(scene_p50 "$work/overview-$1-1.txt")
  b=$(scene_p50 "$work/overview-$1-2.txt")
  awk -v a="$a" -v b="$b" 'BEGIN { print (a < b ? a : b) }'
}
on=$(best on)
off=$(best off)
if [ -z "$on" ] || [ -z "$off" ]; then
  echo "terrain-decal-check: no GPU timings in the frame log"
  exit 1
fi
if awk -v on="$on" -v off="$off" -v r="$ratio" 'BEGIN { exit !(on <= off * r) }'; then
  echo "terrain-decal-check: ok scene p50 ${on} ms with decals, ${off} ms without (<= x${ratio})"
else
  echo "terrain-decal-check: FAIL scene p50 ${on} ms with decals, ${off} ms without (> x${ratio})"
  status=1
fi

run stats MOBA_PIPELINE_STATS=1 MOBA_SMOKE_FRAMES=5
lines=$(grep '\[render\] pipeline-stats terrain .*FRAGMENT' "$work/stats.log" || true)
if [ -z "$lines" ] || ! grep -q 'Register_Count=' <<< "$lines"; then
  echo "terrain-decal-check: the driver reports no register count; register check skipped"
else
  while read -r line; do
    samples=$(sed -n 's/.*samples=\([0-9]*\).*/\1/p' <<< "$line")
    count=$(sed -n 's/.*Register_Count=\([0-9]*\).*/\1/p' <<< "$line")
    if [ "$count" -gt "$registers_max" ]; then
      echo "terrain-decal-check: FAIL samples=$samples registers=$count > $registers_max"
      status=1
    else
      echo "terrain-decal-check: ok samples=$samples registers=$count <= $registers_max"
    fi
  done <<< "$lines"
fi
exit $status
