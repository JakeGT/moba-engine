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
#    `prepass` lane (the G-buffer pass, N2b). Every Tiered run must log
#    `[render] lighting: deferred`, or the half fails. A device the gate does not admit to
#    Tiered (macOS, or one missing a requirement: the probe run logs `path=compat`) skips
#    this half, with the gate's reason.
#
# Timing: the networked whole-map overview on Ultra in a WINDOW-sized window (default
# 1920x1080, logical: a scaled Wayland output makes the swapchain larger), with decals on and
# with MOBA_DECALS=off, ROUNDS interleaved rounds (default 5). Each lane's median GPU p50 with
# decals on must stay within RATIO (default 1.15) of the median without: at R3b the shipped
# shader was within 2 % on the RTX 5070 Ti and the record copy 1.6x. A missing or zero p50
# fails: the lane did not run.
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
# The whole run holds the GPU lock GPU_LOCK (default `handoff/gpu.lock` beside the main
# checkout, when that file exists; GPU_LOCK= runs without one). OUT=<dir> keeps the logs and
# the command line there (default: a temporary directory, removed at exit). Set
# RUN="xvfb-run -a" when there is no display. Not for lavapipe: the timing needs a GPU.
set -euo pipefail
if [ -z "${TERRAIN_DECAL_CHECK_LOCKED:-}" ]; then
  if [ -z "${GPU_LOCK+set}" ]; then
    common=$(git -C "$(dirname "$0")" rev-parse --path-format=absolute --git-common-dir \
      2> /dev/null || true)
    GPU_LOCK=""
    if [ -n "$common" ] && [ -e "$(dirname "$(dirname "$common")")/handoff/gpu.lock" ]; then
      GPU_LOCK="$(dirname "$(dirname "$common")")/handoff/gpu.lock"
    fi
  fi
  if [ -n "$GPU_LOCK" ]; then
    echo "terrain-decal-check: waiting for the GPU lock $GPU_LOCK"
    export TERRAIN_DECAL_CHECK_LOCKED="$GPU_LOCK"
    exec flock "$GPU_LOCK" bash "$0" "$@"
  fi
  export TERRAIN_DECAL_CHECK_LOCKED="none"
fi
client=${1:-./target/release/moba-client}
ratio=${RATIO:-1.15}
rounds=${ROUNDS:-5}
window=${WINDOW:-1920x1080}
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
{
  echo "command: $0 $*"
  echo "client: $client"
  echo "checkout: $(git -C "$(dirname "$0")" rev-parse --short HEAD 2> /dev/null || echo ?)"
  echo "lock: $TERRAIN_DECAL_CHECK_LOCKED"
  echo "settings: RATIO=$ratio ROUNDS=$rounds WINDOW=$window REGISTERS=$registers_max" \
    "GBUFFER_REGISTERS=$gbuffer_registers_max RUN=${RUN:-}"
} | tee "$work/command.txt"

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

lane_of() {
  case $1 in
    compat) echo scene ;;
    tiered) echo prepass ;;
  esac
}

# The registers runs double as the Tiered probe: is the device admitted, and does it light
# deferred?
run stats-compat MOBA_RENDER_PATH=compat MOBA_PIPELINE_STATS=1 MOBA_SMOKE_FRAMES=5
run stats-tiered MOBA_RENDER_PATH=tiered MOBA_PIPELINE_STATS=1 MOBA_SMOKE_FRAMES=5
paths=(compat)
if grep -q 'path=compat' "$work/stats-tiered.log"; then
  reason=$(grep -o 'admission=[a-z-]*' "$work/stats-tiered.log" | head -1)
  missing=$(grep -o 'missing=[^ ]*' "$work/stats-tiered.log" | head -1)
  echo "terrain-decal-check: SKIP tiered: the device is not admitted to Tiered" \
    "(${reason:-admission unknown} ${missing:-})"
elif ! grep -q '^\[render\] lighting: deferred' "$work/stats-tiered.log"; then
  grep '^\[render\]' "$work/stats-tiered.log" || true
  echo "terrain-decal-check: FAIL tiered: admitted to Tiered but did not light deferred"
  status=1
else
  paths+=(tiered)
fi

for round in $(seq 1 "$rounds"); do
  for path in "${paths[@]}"; do
    for decals in on off; do
      extra=()
      [ "$decals" = off ] && extra=(MOBA_DECALS=off)
      run "$path-$decals-$round" MOBA_RENDER_PATH="$path" MOBA_SERVER=local \
        MOBA_CAMERA=overview MOBA_SMOKE_FRAMES=1500 MOBA_WINDOW_SIZE="$window" \
        MOBA_FRAME_LOG="$work/$path-$decals-$round.txt" "${extra[@]}"
      if [ "$path" = tiered ] &&
        ! grep -q '^\[render\] lighting: deferred' "$work/$path-$decals-$round.log"; then
        echo "terrain-decal-check: FAIL tiered: $path-$decals-$round did not light deferred"
        status=1
      fi
    done
  done
done

# The median of the rounds' p50s, and the rounds themselves; nothing if any round has no
# positive p50.
median() {
  local name=$1 lane=$2 round values=""
  for round in $(seq 1 "$rounds"); do
    values+="$(sed -n "s/.*gpu_${lane}_p50_ms=\\([0-9.]*\\).*/\\1/p" \
      "$work/$name-$round.txt" | tail -1) "
  done
  tr ' ' '\n' <<< "$values" | awk -v n="$rounds" '
    NF { v[++c] = $1 + 0; all = all sep $1; sep = "," }
    END {
      if (c != n) exit
      for (i = 1; i <= c; i++) if (v[i] <= 0) exit
      for (i = 2; i <= c; i++) for (j = i; j > 1 && v[j - 1] > v[j]; j--) {
        t = v[j]; v[j] = v[j - 1]; v[j - 1] = t
      }
      m = (c % 2) ? v[(c + 1) / 2] : (v[c / 2] + v[c / 2 + 1]) / 2
      printf "%.3f ms (rounds %s)\n", m, all
    }'
}
for path in "${paths[@]}"; do
  lane=$(lane_of "$path")
  on=$(median "$path-on" "$lane")
  off=$(median "$path-off" "$lane")
  if [ -z "$on" ] || [ -z "$off" ]; then
    echo "terrain-decal-check: FAIL $path: a round has no positive gpu_${lane}_p50_ms" \
      "(on '${on}', off '${off}')"
    status=1
  elif awk -v on="${on%% *}" -v off="${off%% *}" -v r="$ratio" \
    'BEGIN { exit !(on <= off * r) }'; then
    echo "terrain-decal-check: ok $path $lane median p50 ${on} with decals, ${off}" \
      "without (<= x${ratio})"
  else
    echo "terrain-decal-check: FAIL $path $lane median p50 ${on} with decals, ${off}" \
      "without (> x${ratio})"
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
check_registers "$work/stats-compat.log" '\[render\] pipeline-stats terrain .*FRAGMENT' \
  "$registers_max" terrain
if [[ " ${paths[*]} " == *" tiered "* ]]; then
  check_registers "$work/stats-tiered.log" \
    '\[render\] pipeline-stats terrain_gbuffer .*FRAGMENT' "$gbuffer_registers_max" \
    terrain_gbuffer
fi
exit $status
