#!/usr/bin/env bash
# Tiered goldens (ADR-0021 §7, verification gate 4): Low (Quality), High, native and the
# overlay over moving units (a two-tick capture: the history warms up at tick n - 1 and tick n
# is captured with the camera panned), on lavapipe at 640x360 with the HUD at half scale.
#
#   scripts/tiered-goldens.sh [client binary] [image-check binary] [output dir]
#
# UPSCALERS picks the back ends (default "taau fsr3"); CI runs "taau" in the required
# renderer-gate and "fsr3" in tiered-temporal. Per back end and scene:
#   - two captures, which must be byte-identical;
#   - an exact SHA-256 check against tests/golden/render-tiered-hashes.txt, but only on the
#     device that file was generated on (CI's lavapipe: its `device` line, the device name,
#     Vulkan version and driver info (Mesa and LLVM versions) from `vulkan-probe --json`).
#     llvmpipe's code generation differs between Mesa and LLVM versions, so another lavapipe
#     renders other bytes for every back end. On another device the check is skipped with a
#     log line, except on CI (GITHUB_ACTIONS) for FSR 3.1, whose only regression check it
#     is: there a mismatch fails the run (TAAU, in the required renderer-gate, warns and keeps
#     its fingerprints). WRITE_HASHES=1 rewrites the file for the current device (from CI's
#     artifact for CI's lavapipe); TIERED_HASHES names another file;
#   - the 64x36 fingerprint (tests/golden/render-tiered-[fsr3-]<name>-fingerprint.rgb, WRITE=1
#     rewrites them). It catches a broken frame, not an upscaler regression: it cannot tell
#     FSR 3.1 from TAAU (perf/0033), which is why the exact check exists.
#
# Captures fail on any validation message, and so does a 40-frame run per back end that
# resets the temporal history every 5th frame with frames in flight. CI runs it under
# lavapipe with validation and sync validation (render-golden.yml); set RUN="xvfb-run -a"
# when there is no display.
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
upscalers=${UPSCALERS:-taau fsr3}
hashes=${TIERED_HASHES:-tests/golden/render-tiered-hashes.txt}
probe="$(dirname "$client")/vulkan-probe"
# shellcheck disable=SC2086 # RUN is a command prefix.
device=$(${RUN:-} "$probe" --json 2>/dev/null | python3 -c '
import json, sys
d = json.load(sys.stdin)[0]
print("%s api=%s driver=%s" % (d["name"], d["api_version"], d["driver_info"]))' 2>/dev/null || true)
expected_device=$(sed -n 's/^device //p' "$hashes" 2>/dev/null || true)
exact=0
if [ "${WRITE_HASHES:-0}" = 1 ]; then
  test -n "$device" || { echo "FAIL: vulkan-probe named no device"; exit 1; }
  printf '%s\n%s\n' "# SHA-256 of the tiered captures (scripts/tiered-goldens.sh) on this device only." \
    "device $device" > "$hashes.new"
elif [ -n "$device" ] && [ "$device" = "$expected_device" ]; then
  exact=1
  echo "exact hashes: on $device"
fi
status=0
if [ "${WRITE_HASHES:-0}" != 1 ] && [ "$exact" = 0 ]; then
  echo "exact hashes: skipped, this device is '${device:-unknown}' and $hashes is for" \
    "'${expected_device:-none}'; the fingerprints alone are checked"
  # On CI the runner's lavapipe changed (or vulkan-probe failed). FSR 3.1 has no other
  # regression check, so its job fails (after the captures, which the artifact keeps);
  # TAAU's, in the required renderer-gate, warns.
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    regenerate="regenerate $hashes with WRITE_HASHES=1 from this run's artifact (the captures" \
    regenerate="$regenerate are in it) or on this lavapipe"
    if [[ " $upscalers " == *" fsr3 "* ]]; then
      echo "::error::tiered-goldens: no exact hashes for FSR 3.1 on '${device:-unknown}'; $regenerate"
      echo "FAIL: exact hashes do not cover this device; $regenerate"
      status=1
    else
      echo "::warning::tiered-goldens: exact hashes skipped on '${device:-unknown}'; $regenerate"
    fi
  fi
fi
for upscaler in $upscalers; do
  prefix=; [ "$upscaler" = taau ] || prefix="$upscaler-"
  for entry in "${cases[@]}"; do
    read -r name settings <<< "$entry"
    capture_name="$prefix$name"
    # shellcheck disable=SC2086 # settings are separate assignments.
    capture "$out/$capture_name-1.ppm" MOBA_UPSCALER="$upscaler" $settings
    # shellcheck disable=SC2086
    capture "$out/$capture_name-2.ppm" MOBA_UPSCALER="$upscaler" $settings
    grep -E "anti-aliasing" "$out/$capture_name-1.ppm.log" || true
    hash=$(sha256sum < "$out/$capture_name-1.ppm" | cut -d' ' -f1)
    # A back end that fell back would check TAAU's output under another name.
    if ! grep -q "upscaler $upscaler " "$out/$capture_name-1.ppm.log"; then
      echo "FAIL: tiered $capture_name: did not run $upscaler"
      status=1
      continue
    elif cmp -s "$out/$capture_name-1.ppm" "$out/$capture_name-2.ppm"; then
      echo "ok: tiered $capture_name: two runs byte-identical (${hash:0:16})"
    else
      echo "FAIL: tiered $capture_name: the two runs differ"
      status=1
    fi
    if [ "${WRITE_HASHES:-0}" = 1 ]; then
      echo "$capture_name $hash" >> "$hashes.new"
    elif [ "$exact" = 1 ]; then
      want=$(sed -n "s/^$capture_name //p" "$hashes")
      if [ "$hash" = "$want" ]; then
        echo "ok: tiered $capture_name: exact hash"
      else
        echo "FAIL: tiered $capture_name: hash ${hash:0:16}, expected ${want:0:16}"
        status=1
      fi
    fi
    fingerprint="tests/golden/render-tiered-$capture_name-fingerprint.rgb"
    if [ "${WRITE:-0}" = 1 ]; then
      "$check" "$out/$capture_name-1.ppm" --expected "$fingerprint" --write
    elif ! "$check" "$out/$capture_name-1.ppm" --expected "$fingerprint" \
      --diff "$out/$capture_name-diff.ppm"; then
      echo "FAIL: tiered $capture_name: fingerprint"
      status=1
    fi
  done
done
if [ "${WRITE_HASHES:-0}" = 1 ]; then
  # Keep the other back ends' lines when only some were run.
  if [ -f "$hashes" ] && [ "$(sed -n 's/^device //p' "$hashes")" = "$device" ]; then
    grep -v -e '^#' -e '^device ' "$hashes" | while read -r name hash; do
      grep -q "^$name " "$hashes.new" || echo "$name $hash" >> "$hashes.new"
    done
  fi
  { head -2 "$hashes.new"; tail -n +3 "$hashes.new" | sort; } > "$hashes"
  rm "$hashes.new"
  echo "wrote $hashes for $device"
fi
# A history reset mid-stream (ADR-0021 amendment 2 item 28): every 5th frame resets the
# temporal history with the previous frame still in flight, under the same validation. This
# is a validation smoke of the reset path: synchronization validation does not flag the
# reset barrier with a NONE source (on lavapipe, NVIDIA or RADV), so unit tests pin it.
for upscaler in $upscalers; do
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
