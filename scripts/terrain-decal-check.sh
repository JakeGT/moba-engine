#!/usr/bin/env bash
# Terrain decal-cost guard (R3b, ADR-0017 §1; ADR-0023 amendment 3 item 31). The terrain's
# fragment shaders carry the decal code, and the terrain's cost has a cliff: during R3b a
# local copy of the decal record doubled the terrain's cost on the Ultra overview, decals in
# view or not. Run this after any change to `terrain.frag`, `terrain_gbuffer.frag` or
# `include/decals.glsl`, on a real GPU. Each stage that inlines the decals is checked against
# its permutation with the decal code compiled out, which MOBA_DECALS=off selects:
#
# 1. Compat (MOBA_RENDER_PATH=compat): the forward `terrain.frag` against
#    `terrain_nodecals.frag`, timed by the `scene` lane.
# 2. Tiered (MOBA_RENDER_PATH=tiered, the deferred frame): the G-buffer stage
#    `terrain_gbuffer*_hdr.frag` against `terrain_gbuffer_nodecals*_hdr.frag`, timed by the
#    `prepass` lane (the G-buffer pass, N2b).
#
# Timing: the networked whole-map overview on Ultra, with decals on and with MOBA_DECALS=off.
# Each lane's GPU p50 with decals on must stay within RATIO (default 1.15) of the one
# without: at R3b the shipped shader was within 2 % on the RTX 5070 Ti and the record copy
# 1.6x. Two interleaved rounds, the faster of each kept (the GPU's power state moves the small
# numbers). A missing or zero p50 fails: the lane did not run.
#
# Registers: MOBA_PIPELINE_STATS=1 enables VK_KHR_pipeline_executable_properties and the
# client logs the pipelines' statistics. Compat's `terrain` fragment shader must need at most
# REGISTERS (default 64, the RTX 5070 Ti's count at R3b) and Tiered's `terrain_gbuffer`
# fragment shader at most GBUFFER_REGISTERS (default 48, its count at N2c). A driver without
# the extension or a `Register Count` statistic (lavapipe; RADV names its counters
# VGPRs/SGPRs) skips these; a logged line without a count fails. They catch register growth
# only: the record copy stayed at 64 registers.
#
#   scripts/terrain-decal-check.sh [client binary]
#
# OUT=<dir> keeps the logs there (default: a temporary directory, removed at exit). Set
# RUN="xvfb-run -a" when there is no display. Not for lavapipe: the timing needs a GPU.
set -euo pipefail
client=${1:-./target/release/moba-client}
ratio=${RATIO:-1.15}
registers_max=${REGISTERS:-64}
gbuffer_registers_max=${GBUFFER_REGISTERS:-48}
if [ -n "${OUT:-}" ]; then
  work=$OUT
  mkdir -p "$work"
else
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
fi
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

# The last p50 of a lane in a frame log, or nothing.
lane_p50() {
  sed -n "s/.*gpu_$2_p50_ms=\\([0-9.]*\\).*/\\1/p" "$1" | tail -1
}

paths=(compat tiered)
declare -A lane=([compat]=scene [tiered]=prepass)
for round in 1 2; do
  for path in "${paths[@]}"; do
    for decals in on off; do
      extra=()
      [ "$decals" = off ] && extra=(MOBA_DECALS=off)
      run "$path-$decals-$round" MOBA_RENDER_PATH="$path" MOBA_SERVER=local \
        MOBA_CAMERA=overview MOBA_SMOKE_FRAMES=1500 \
        MOBA_FRAME_LOG="$work/$path-$decals-$round.txt" "${extra[@]}"
    done
  done
done

# The faster of the two rounds; nothing if either round has no positive p50.
best() {
  local a b
  a=$(lane_p50 "$work/$1-1.txt" "$2")
  b=$(lane_p50 "$work/$1-2.txt" "$2")
  awk -v a="$a" -v b="$b" 'BEGIN {
    if (a == "" || b == "" || a + 0 <= 0 || b + 0 <= 0) exit
    print (a < b ? a : b)
  }'
}
for path in "${paths[@]}"; do
  name=${lane[$path]}
  on=$(best "$path-on" "$name")
  off=$(best "$path-off" "$name")
  if [ -z "$on" ] || [ -z "$off" ]; then
    echo "terrain-decal-check: FAIL $path: no positive gpu_${name}_p50_ms in the frame logs" \
      "(on '${on}', off '${off}')"
    status=1
  elif awk -v on="$on" -v off="$off" -v r="$ratio" 'BEGIN { exit !(on <= off * r) }'; then
    echo "terrain-decal-check: ok $path $name p50 ${on} ms with decals, ${off} ms without" \
      "(<= x${ratio})"
  else
    echo "terrain-decal-check: FAIL $path $name p50 ${on} ms with decals, ${off} ms without" \
      "(> x${ratio})"
    status=1
  fi
done

check_registers() {
  local log=$1 pattern=$2 max=$3 label=$4 lines line samples count
  lines=$(grep "$pattern" "$log" || true)
  if [ -z "$lines" ] || ! grep -q 'Register_Count=' <<< "$lines"; then
    echo "terrain-decal-check: no register count for $label; register check skipped"
    return
  fi
  while read -r line; do
    samples=$(sed -n 's/.*samples=\([0-9]*\).*/\1/p' <<< "$line")
    count=$(sed -n 's/.*Register_Count=\([0-9]*\).*/\1/p' <<< "$line")
    if [ -z "$count" ]; then
      echo "terrain-decal-check: FAIL $label samples=${samples:-1}: no Register_Count in: $line"
      status=1
    elif [ "$count" -gt "$max" ]; then
      echo "terrain-decal-check: FAIL $label samples=${samples:-1} registers=$count > $max"
      status=1
    else
      echo "terrain-decal-check: ok $label samples=${samples:-1} registers=$count <= $max"
    fi
  done <<< "$lines"
}
run stats-compat MOBA_RENDER_PATH=compat MOBA_PIPELINE_STATS=1 MOBA_SMOKE_FRAMES=5
run stats-tiered MOBA_RENDER_PATH=tiered MOBA_PIPELINE_STATS=1 MOBA_SMOKE_FRAMES=5
check_registers "$work/stats-compat.log" '\[render\] pipeline-stats terrain .*FRAGMENT' \
  "$registers_max" terrain
check_registers "$work/stats-tiered.log" '\[render\] pipeline-stats terrain_gbuffer .*FRAGMENT' \
  "$gbuffer_registers_max" terrain_gbuffer
exit $status
