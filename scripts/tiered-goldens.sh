#!/usr/bin/env bash
# Tiered goldens (ADR-0021 §7, verification gate 4): TAAU at Low (Quality), at High and at
# native, and the overlay over moving units (a two-tick capture: the history warms up at
# tick n - 1 and tick n is captured with the camera panned), on lavapipe at 640x360 with the
# HUD at half scale. Each is captured twice and the two runs must be byte-identical; then its
# 64x36 fingerprint is compared with tests/golden/render-tiered-<name>-fingerprint.rgb
# (WRITE=1 rewrites them).
#
#   scripts/tiered-goldens.sh [client binary] [image-check binary] [output dir]
#
# FSR 3.1 (ADR-0021 amendment 4) captures the same four scenes twice each, which must be
# byte-identical, but has no committed fingerprint: the committed goldens are TAAU's.
#
# Captures fail on any validation message, and so does a 40-frame run per back end that
# resets the temporal history every 5th frame with frames in flight. CI runs it under lavapipe with validation and
# sync validation (render-golden.yml); set RUN="xvfb-run -a" when there is no display.
set -euo pipefail
client=${1:-./target/release/moba-client}
check=${2:-./target/release/image-check}
out=${3:-tiered-goldens}
mkdir -p "$out"
capture() {
  local file=$1
  shift
  # shellcheck disable=SC2086 # RUN is a command prefix.
  env MOBA_RENDER_PATH=tiered MOBA_WINDOW_SIZE=640x360 MOBA_UI_SCALE=0.5 \
    MOBA_CAPTURE="$file" "$@" timeout 300 ${RUN:-} "$client" > "$file.log" 2>&1 || { cat "$file.log"; exit 1; }
  test -s "$file"
  if grep -E '^\[vulkan' "$file.log"; then
    echo "FAIL: validation messages ($file)"
    exit 1
  fi
}
local_scene=(MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90)
network_scene=(MOBA_SERVER=local MOBA_HERO_MODELS=sentinel-skinned MOBA_CAPTURE_TICK=120
  MOBA_SMOKE_FRAMES=100000)
cases=(
  "low MOBA_QUALITY=low ${local_scene[*]}"
  "high MOBA_QUALITY=high ${local_scene[*]}"
  "native MOBA_QUALITY=high MOBA_UPSCALE_MODE=native ${local_scene[*]}"
  "overlay MOBA_QUALITY=high MOBA_CAPTURE_TWO_TICK=1 ${network_scene[*]}"
)
status=0
for entry in "${cases[@]}"; do
  read -r name settings <<< "$entry"
  # shellcheck disable=SC2086 # settings are separate assignments.
  capture "$out/$name-1.ppm" $settings
  # shellcheck disable=SC2086
  capture "$out/$name-2.ppm" $settings
  grep -E "anti-aliasing" "$out/$name-1.ppm.log" || true
  if cmp -s "$out/$name-1.ppm" "$out/$name-2.ppm"; then
    echo "ok: tiered $name: two runs byte-identical"
  else
    echo "FAIL: tiered $name: the two runs differ"
    status=1
  fi
  # FSR 3.1: the same scene, deterministic across two runs, no fingerprint.
  # shellcheck disable=SC2086
  capture "$out/fsr3-$name-1.ppm" MOBA_UPSCALER=fsr3 $settings
  # shellcheck disable=SC2086
  capture "$out/fsr3-$name-2.ppm" MOBA_UPSCALER=fsr3 $settings
  if ! grep -q "upscaler fsr3 " "$out/fsr3-$name-1.ppm.log"; then
    echo "FAIL: tiered fsr3 $name: did not run fsr3"
    status=1
  elif cmp -s "$out/fsr3-$name-1.ppm" "$out/fsr3-$name-2.ppm"; then
    echo "ok: tiered fsr3 $name: two runs byte-identical ($(sha256sum < "$out/fsr3-$name-1.ppm" | cut -c1-16))"
  else
    echo "FAIL: tiered fsr3 $name: the two runs differ"
    status=1
  fi
  fingerprint="tests/golden/render-tiered-$name-fingerprint.rgb"
  if [ "${WRITE:-0}" = 1 ]; then
    "$check" "$out/$name-1.ppm" --expected "$fingerprint" --write
  elif ! "$check" "$out/$name-1.ppm" --expected "$fingerprint" --diff "$out/$name-diff.ppm"; then
    echo "FAIL: tiered $name: fingerprint"
    status=1
  fi
done
# A history reset mid-stream (ADR-0021 amendment 2 item 28): every 5th frame resets the
# temporal history with the previous frame still in flight, under the same validation. This
# is a validation smoke of the reset path: synchronization validation does not flag the
# reset barrier with a NONE source (on lavapipe, NVIDIA or RADV), so unit tests pin it.
for upscaler in taau fsr3; do
  reset_log="$out/mid-stream-reset-$upscaler.log"
  # shellcheck disable=SC2086 # RUN is a command prefix.
  if env MOBA_RENDER_PATH=tiered MOBA_WINDOW_SIZE=640x360 MOBA_UI_SCALE=0.5 MOBA_QUALITY=high \
    MOBA_UPSCALER="$upscaler" MOBA_SMOKE_FRAMES=40 MOBA_TEMPORAL_RESET_EVERY=5 timeout 300 \
    ${RUN:-} "$client" > "$reset_log" 2>&1 && ! grep -E '^\[vulkan' "$reset_log" &&
    grep -q "upscaler $upscaler " "$reset_log"; then
    echo "ok: tiered mid-stream history reset ($upscaler): no validation messages"
  else
    tail -20 "$reset_log"
    echo "FAIL: tiered mid-stream history reset ($upscaler)"
    status=1
  fi
done
exit "$status"
