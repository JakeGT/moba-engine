#!/usr/bin/env bash
# Frame-pacing and perf-budget run (R8, ADR-0018 §5-§6): records the long teamfight replay
# (`teamfight --ticks N`, 18,000 ticks = 10 minutes at 30 Hz), plays it on the release client
# once per tier with the frame log on, and checks every log with `perf-gate` against the
# roadmap's §7 GPU budgets, the pacing rule (p99 <= 1.5x p50), the no-mid-match-upload
# contract and the committed baseline. The nightly perf workflow runs it on the self-hosted
# GPU runner; run it the same way on a developer machine with a real GPU.
#
#   scripts/perf-replay.sh [output dir]
#
# Environment: TICKS (default 18000), TIERS (default "low medium high ultra"), BASELINE
# (default tests/perf/baseline.toml; empty skips the regression check), WINDOW (default
# 1920x1080), RUN (a command prefix such as "xvfb-run -a"). The replay itself is evidence
# only and never committed. Needs `cargo build --release -p moba-client -p moba-tools`.
set -euo pipefail
out=${1:-perf-replay}
ticks=${TICKS:-18000}
tiers=${TIERS:-low medium high ultra}
baseline=${BASELINE-tests/perf/baseline.toml}
mkdir -p "$out"
replay="$out/teamfight-$ticks.mbr"
if [ ! -s "$replay" ]; then
  ./target/release/teamfight --ticks "$ticks" --out "$replay" > "$out/teamfight.log" 2>&1 \
    || { cat "$out/teamfight.log"; exit 1; }
fi
logs=()
for tier in $tiers; do
  log="$out/frames-$tier.log"
  # A short run first, so the measured run starts from a warm pipeline cache (perf-gate
  # --warm-cache): the tier's every pipeline is then a cache hit.
  # shellcheck disable=SC2086 # RUN is a command prefix.
  env MOBA_QUALITY="$tier" MOBA_VULKAN_VALIDATION=off MOBA_SMOKE_FRAMES=30 \
    MOBA_WINDOW_SIZE="${WINDOW:-1920x1080}" \
    timeout 300 ${RUN:-} ./target/release/moba-client > "$out/warm-$tier.log" 2>&1 \
    || { tail -20 "$out/warm-$tier.log"; exit 1; }
  # shellcheck disable=SC2086 # RUN is a command prefix.
  env MOBA_QUALITY="$tier" MOBA_REPLAY="$replay" MOBA_REPLAY_EXIT=1 MOBA_WARMUP_TICKS=0 \
    MOBA_VULKAN_VALIDATION="${MOBA_VULKAN_VALIDATION:-off}" MOBA_VSYNC="${MOBA_VSYNC:-0}" \
    MOBA_WINDOW_SIZE="${WINDOW:-1920x1080}" MOBA_SMOKE_FRAMES=100000000 \
    MOBA_FRAME_LOG="$log" MOBA_FRAME_LOG_FRAMES=1000000 \
    timeout $((ticks / 30 + 300)) ${RUN:-} ./target/release/moba-client > "$log.stderr" 2>&1 \
    || { tail -20 "$log.stderr"; exit 1; }
  test -s "$log"
  logs+=("$log")
done
if [ -n "$baseline" ]; then
  ./target/release/perf-gate --warm-cache --baseline "$baseline" "${logs[@]}"
else
  ./target/release/perf-gate --warm-cache "${logs[@]}"
fi
