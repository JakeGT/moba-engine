#!/usr/bin/env bash
# Overlay invariance (ADR-0021 §5, verification gate 3): the post-upscale gameplay overlay
# reads only the unjittered output-resolution overlay prepass, so its ID target must be
# byte-identical whichever anti-aliasing and upscaler (TAAU, FSR 3.1) run and at every render
# scale. Each capture shows
# the overlay IDs (MOBA_DEBUG_VIEW=overlay-ids) of the VFX preview arena with an outlined
# enemy hero in the open (MOBA_OUTLINE_PROBE=open), on the tiered path, and every one must
# equal the first.
#
#   scripts/overlay-invariance.sh [client binary] [output dir]
#
# Captures are pinned (fixed tick per frame). CI runs it under lavapipe with validation
# (render-golden.yml); set RUN="xvfb-run -a" when there is no display.
set -euo pipefail
client=${1:-./target/release/moba-client}
out=${2:-overlay-invariance}
mkdir -p "$out"
capture() {
  local name=$1
  shift
  # shellcheck disable=SC2086 # RUN is a command prefix.
  env MOBA_RENDER_PATH=tiered MOBA_QUALITY=high MOBA_VFX_PREVIEW=cast-flash \
    MOBA_OUTLINE_PROBE=open MOBA_DEBUG_VIEW=overlay-ids MOBA_SMOKE_FRAMES=30 \
    MOBA_CAPTURE_FRAME=20 MOBA_CAPTURE="$out/$name.ppm" "$@" timeout 180 ${RUN:-} "$client" \
    > "$out/$name.log" 2>&1 || { cat "$out/$name.log"; exit 1; }
  test -s "$out/$name.ppm"
  if grep -E '^\[vulkan' "$out/$name.log"; then
    echo "FAIL: validation messages ($name)"
    exit 1
  fi
}
cases=(
  "taau-quality MOBA_AA=temporal MOBA_UPSCALE_MODE=quality"
  "taau-native MOBA_AA=temporal MOBA_UPSCALE_MODE=native"
  "taau-performance MOBA_AA=temporal MOBA_UPSCALE_MODE=performance"
  "fsr3-quality MOBA_AA=temporal MOBA_UPSCALER=fsr3 MOBA_UPSCALE_MODE=quality"
  "fsr3-native MOBA_AA=temporal MOBA_UPSCALER=fsr3 MOBA_UPSCALE_MODE=native"
  "fsr3-performance MOBA_AA=temporal MOBA_UPSCALER=fsr3 MOBA_UPSCALE_MODE=performance"
  "fxaa MOBA_AA=fxaa"
  "fxaa-scaled MOBA_AA=fxaa MOBA_RENDER_SCALE=0.6"
  "off MOBA_AA=off"
)
status=0
reference=
for entry in "${cases[@]}"; do
  read -r name settings <<< "$entry"
  # shellcheck disable=SC2086 # settings are separate assignments.
  capture "$name" $settings
  # A back end that fell back would compare TAAU with itself.
  case $name in
    fsr3-*)
      if ! grep -q "upscaler fsr3 " "$out/$name.log"; then
        echo "FAIL: $name did not run fsr3"
        grep "upscaler" "$out/$name.log" || true
        status=1
        continue
      fi
      ;;
  esac
  if [ -z "$reference" ]; then
    reference=$name
    echo "reference: $name"
  elif cmp -s "$out/$reference.ppm" "$out/$name.ppm"; then
    echo "ok: $name overlay IDs identical to $reference"
  else
    echo "FAIL: $name overlay IDs differ from $reference"
    status=1
  fi
done
exit "$status"
