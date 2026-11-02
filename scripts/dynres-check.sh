#!/usr/bin/env bash
# Dynamic resolution holds a forced budget (ADR-0020 §5c, N0 gate 4). Plays the teamfight
# replay twice on the release client with the frame log on:
#   1. dynamic resolution off, at the preset's scale ceiling: the natural GPU p95;
#   2. dynamic resolution forced to FACTOR x that p95 (MOBA_DYNRES=<ms>).
# The second run passes when the scale stepped down, settled (at most MAX_LATE_CHANGES scale
# changes over the second half of the frames) and GPU p95 is at or under the forced target.
# Needs a real GPU: on a software device the GPU time is CPU time and does not follow the
# pixel count.
#
#   scripts/dynres-check.sh [output dir]
#
# Environment: TIER (default ultra), RENDER_PATH (default tiered), FACTOR (default 0.75),
# TARGET_MS (a fixed target instead of FACTOR x the natural p95), MAX_LATE_CHANGES (default
# 2), FRAMES (default 3000), WINDOW (default 1920x1080), REPLAY (default
# tests/replays/teamfight-5v5.mbr), RUN (a command prefix such as "xvfb-run -a"). Needs
# `cargo build --release -p moba-client`.
set -euo pipefail
out=${1:-dynres-check}
tier=${TIER:-ultra}
path=${RENDER_PATH:-tiered}
factor=${FACTOR:-0.75}
max_late=${MAX_LATE_CHANGES:-2}
replay=${REPLAY:-tests/replays/teamfight-5v5.mbr}
mkdir -p "$out"

play() { # play NAME MOBA_DYNRES
  local log="$out/$1.log"
  # shellcheck disable=SC2086 # RUN is a command prefix.
  env MOBA_QUALITY="$tier" MOBA_RENDER_PATH="$path" MOBA_DYNRES="$2" \
    MOBA_REPLAY="$replay" MOBA_REPLAY_EXIT=1 MOBA_WARMUP_TICKS="${WARMUP_TICKS:-330}" \
    MOBA_VULKAN_VALIDATION="${MOBA_VULKAN_VALIDATION:-off}" MOBA_VSYNC=0 \
    MOBA_WINDOW_SIZE="${WINDOW:-1920x1080}" MOBA_SMOKE_FRAMES="${FRAMES:-3000}" \
    MOBA_FRAME_LOG="$log" MOBA_FRAME_LOG_FRAMES=1000000 \
    timeout 900 ${RUN:-} ./target/release/moba-client > "$log.stderr" 2>&1 \
    || { tail -20 "$log.stderr"; exit 1; }
  test -s "$log"
}
field() { # field LOG KEY
  grep -o "[[:space:]]$2=[^ ]*" "$1" | head -1 | cut -d= -f2
}

play natural off
natural=$(field "$out/natural.log" gpu_total_p95_ms)
[ -n "$natural" ] || { echo "dynres-check: no gpu_total_p95_ms (no GPU timestamps?)"; exit 1; }
target=${TARGET_MS:-$(awk -v p="$natural" -v f="$factor" 'BEGIN { printf "%.3f", p * f }')}
play forced "$target"
log="$out/forced.log"
p95=$(field "$log" gpu_total_p95_ms)
first=$(field "$log" scale_first)
final=$(field "$log" scale_final)
low=$(field "$log" scale_min)
late=$(field "$log" scale_changes_late)
ran=$(field "$log" path)
echo "dynres-check: path=$ran tier=$tier natural_p95=$natural ms target=$target ms" \
  "forced_p95=$p95 ms scale $first -> $final (min $low) late_changes=$late"
fail=0
[ "$ran" = "$path" ] || { echo "ran on path=$ran, not $path"; fail=1; }
# scale_first is logged after the frame log's warm-up, when the controller may already have
# stepped, so the check is against full scale.
awk -v a="$final" 'BEGIN { exit !(a < 1.0) }' \
  || { echo "the scale did not step down ($first -> $final)"; fail=1; }
[ "$late" -le "$max_late" ] \
  || { echo "the scale changed $late times in the second half (limit $max_late)"; fail=1; }
awk -v a="$p95" -v b="$target" 'BEGIN { exit !(a <= b) }' \
  || { echo "GPU p95 $p95 ms is over the forced target of $target ms"; fail=1; }
if [ "$fail" -ne 0 ]; then
  echo "dynres-check: FAILED"
  exit 1
fi
echo "dynres-check: ok"
