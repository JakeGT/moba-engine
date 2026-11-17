#!/usr/bin/env bash
# The deferred–forward parity gate (ADR-0023 gate 2, N2b): the deferred frame against N1's
# tiered forward frame (MOBA_DEFERRED=off, the same binary), on the default scene, the R3b
# river close-up and the R3b teamfight replay, with the map stylisation off on both paths
# (MOBA_STYLISATION=off: no wrap, rim, ramp or painted specular) and only STANDARD surfaces
# (MOBA_FOLIAGE=off: the foliage is the only other model the G-buffer holds), plus the default
# scene without the world's sky (MOBA_SKY=off: the deferred sky's draw writes the clear colour
# into `hdr`, ADR-0023 amendment 2 item 6). Each pair must
# agree within 2/255 on at least 99.5 % of the pixels and within 6/255 everywhere
# (`image-check --parity`). Any validation message fails a capture.
#
#   scripts/deferred-parity.sh [client binary] [image-check binary] [output dir]
#
# QUALITY picks the tier (default high: the Ultra configuration on Tiered, with full water)
# and UPSCALER the back end (default taau). PARITY_FOLIAGE=1 also captures the three scenes
# with the foliage (FOLIAGE, the extended tiles) and reports their parity without failing.
# PARITY_SCENES (space-separated names, default all four) limits the scenes. With
# MOBA_PAINT_DECAL_PASS=1 in the environment the deferred frame draws the `Paint` decals as
# the G-buffer decal pass (ADR-0023 amendment 3 item 31): each deferred capture must log
# that the pass was built, and the teamfight's (the scene with `Paint` decals) that its
# captured frame had a live `Paint` decal and drew through the pass.
# CI runs it under lavapipe with validation and sync validation (render-golden.yml); set
# RUN="xvfb-run -a" when there is no display.
set -euo pipefail
client=${1:-./target/release/moba-client}
check=${2:-./target/release/image-check}
out=${3:-deferred-parity}
mkdir -p "$out"
capture() {
  local file=$1 lighting=$2
  shift 2
  # shellcheck disable=SC2086 # RUN is a command prefix.
  env MOBA_RENDER_PATH=tiered MOBA_QUALITY="${QUALITY:-high}" MOBA_UPSCALER="${UPSCALER:-taau}" \
    MOBA_STYLISATION=off MOBA_CAPTURE="$file" "$@" timeout 300 ${RUN:-} "$client" \
    > "$file.log" 2>&1 || { cat "$file.log"; echo "FAIL: capture failed ($file)"; exit 1; }
  test -s "$file" || { tail -20 "$file.log"; echo "FAIL: empty capture ($file)"; exit 1; }
  if grep -E '^\[vulkan' "$file.log"; then
    echo "FAIL: validation messages ($file)"
    exit 1
  fi
  grep -q "^\[render\] lighting: $lighting" "$file.log" ||
    { grep "^\[render\]" "$file.log"; echo "FAIL: $file did not light $lighting"; exit 1; }
  if [ "$lighting" = deferred ] && [ "${MOBA_PAINT_DECAL_PASS:-}" = 1 ]; then
    grep -q "^\[render\] paint decals: gbuffer pass" "$file.log" ||
      { grep "^\[render\]" "$file.log"; echo "FAIL: $file did not build the decal pass"; exit 1; }
    if [ "$paint_scene" = 1 ]; then
      grep -Eq "^\[render\] capture frame: paint_decals=[1-9][0-9]* plan=gbuffer_decals" \
        "$file.log" || {
        grep "^\[render\] capture frame" "$file.log"
        echo "FAIL: $file's captured frame did not draw a Paint decal through the pass"
        exit 1
      }
    fi
  fi
}
scenes=(
  "default MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90"
  "default-skyoff MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90 MOBA_SKY=off"
  "river MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE_FRAME=90 MOBA_PREVIEW_CAMERA=0.9,0.3,14,4,-62"
  "teamfight MOBA_REPLAY=tests/replays/teamfight-5v5.mbr MOBA_WARMUP_TICKS=330
   MOBA_CAMERA_DISTANCE=30 MOBA_CAPTURE_FRAME=30 MOBA_SMOKE_FRAMES=40"
)
status=0
run_scene() {
  local name=$1 foliage=$2
  shift 2
  capture "$out/$name-forward.ppm" forward MOBA_DEFERRED=off $foliage "$@"
  capture "$out/$name-deferred.ppm" deferred $foliage "$@"
  "$check" "$out/$name-deferred.ppm" --against "$out/$name-forward.ppm" --parity \
    --diff "$out/$name-diff.ppm" | tee "$out/$name-parity.log"
}
for scene in "${scenes[@]}"; do
  # shellcheck disable=SC2086 # The scene's settings split into words.
  set -- $scene
  name=$1
  shift
  paint_scene=0
  [ "$name" = teamfight ] && paint_scene=1
  case " ${PARITY_SCENES:-$name} " in
    *" $name "*) ;;
    *) continue ;;
  esac
  if run_scene "$name" MOBA_FOLIAGE=off "$@"; then
    echo "ok: deferred parity $name"
  else
    echo "FAIL: deferred parity $name"
    status=1
  fi
  if [ "${PARITY_FOLIAGE:-0}" = 1 ]; then
    run_scene "$name-foliage" "" "$@" || echo "note: $name with foliage is outside gate 2"
  fi
done
exit $status
