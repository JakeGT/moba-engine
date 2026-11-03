#!/usr/bin/env bash
# Ghost energy (ADR-0021 verification gate 3): a hero dashes at 10 m per 0.2 s at 120 fps and
# a projectile crosses the screen; over 20 frames, the share of the wake behind them that
# differs from a no-history render by more than 4/255 must stay under a fixed bound, with
# TAAU on the tiered path (`Renderer::ghost_probe`).
#
#   scripts/ghost-probe.sh [client binary] [output dir]
#
# Modes: Quality, Performance and native. Performance is the strongest check of the hero's
# motion keys (its interior smears 7.30 % without them against a 1.75 % bound, Quality
# 2.59 %); native does not detect missing hero motion keys (0.14 %) and is the no-upscale
# baseline (perf/0031).
#
# CI runs it under lavapipe with validation (render-golden.yml); set RUN="xvfb-run -a" when
# there is no display.
set -euo pipefail
client=${1:-./target/release/moba-client}
out=${2:-ghost-probe}
mkdir -p "$out"
status=0
for mode in quality performance native; do
  log="$out/ghost-$mode.log"
  # shellcheck disable=SC2086 # RUN is a command prefix.
  if env MOBA_GHOST_PROBE=1 MOBA_RENDER_PATH=tiered MOBA_AA=temporal MOBA_QUALITY=high \
    MOBA_UPSCALE_MODE="$mode" MOBA_EXPOSURE=fixed MOBA_DYNRES=off timeout 300 ${RUN:-} \
    "$client" > "$log" 2>&1; then
    echo "ok: ghost probe (taau $mode)"
  else
    echo "FAIL: ghost probe (taau $mode)"
    status=1
  fi
  grep -E "ghost probe|^  frame" "$log" || cat "$log"
  if grep -E '^\[vulkan' "$log"; then
    echo "FAIL: validation messages from the ghost probe ($mode)"
    status=1
  fi
done
exit "$status"
