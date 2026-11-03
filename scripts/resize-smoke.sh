#!/usr/bin/env bash
# Resize smoke (ADR-0021 amendment 5): the client resizes its own window mid-run
# (MOBA_SMOKE_RESIZE) on the tiered path with AO on and on Compat, and every run must
# recreate its swapchain at each size with no validation message. It guards the transient
# re-binding after a resize (lighting's shadow, contact and AO views are written together:
# writing them one at a time wrote a destroyed AO view, `Invalid VkImageView`), which is the
# same code under every anti-aliasing mode, so one tiered case (TAAU) and Compat cover it.
#
#   scripts/resize-smoke.sh [client binary] [output dir]
#
# Needs an X11 display (CI: RUN="xvfb-run -a"); under Wayland the compositor may ignore the
# size requests. UPSCALERS (default "taau") picks the tiered back ends; each one other than
# TAAU (fsr3 in CI's tiered-temporal job; dlss, fsr4 or xess in a build with the feature on a
# GPU that runs it) also gets a resize to 8x8, which the back end may refuse: it must then
# fall back to TAAU, never exit.
set -euo pipefail
client=${1:-./target/release/moba-client}
out=${2:-resize-smoke}
mkdir -p "$out"
sizes="25:480x270,50:320x180,75:640x360"
status=0
run() {
  local name=$1 resizes=$2
  shift 2
  # shellcheck disable=SC2086 # RUN is a command prefix.
  if ! env -u WAYLAND_DISPLAY MOBA_SETTINGS=/dev/null MOBA_WINDOW_SIZE=640x360 MOBA_AO=on \
    MOBA_SMOKE_FRAMES=100 MOBA_SMOKE_RESIZE="$resizes" "$@" \
    timeout 300 ${RUN:-} "$client" > "$out/$name.log" 2>&1; then
    tail -20 "$out/$name.log"
    echo "FAIL: $name exited with an error"
    status=1
    return
  fi
  if grep -E '^\[vulkan' "$out/$name.log"; then
    echo "FAIL: validation messages ($name)"
    status=1
    return
  fi
  # Every requested size must have been reached (a window manager may enlarge a tiny one, so
  # sizes under 64 pixels are only required to have recreated the swapchain once more).
  local entry size missing=""
  for entry in ${resizes//,/ }; do
    size=${entry#*:}
    if [ "${size%x*}" -ge 64 ] && [ "${size#*x}" -ge 64 ]; then
      grep -q "swapchain recreated at $size " "$out/$name.log" || missing="$missing $size"
    fi
  done
  local recreated
  recreated=$(grep -c 'swapchain recreated' "$out/$name.log" || true)
  if [ -n "$missing" ] || [ "$recreated" -lt "$(tr ',' '\n' <<< "$resizes" | wc -l)" ]; then
    echo "FAIL: $name did not recreate its swapchain at every size (missing:${missing:- none})"
    status=1
    return
  fi
  echo "ok: $name ($recreated swapchain recreations, no validation messages)"
}
upscalers=" ${UPSCALERS:-taau} "
for upscaler in $upscalers; do
  run "tiered-$upscaler" "$sizes" MOBA_RENDER_PATH=tiered MOBA_UPSCALER="$upscaler"
  # A back end that fell back at start-up would test TAAU under another name.
  if ! grep -q "upscaler $upscaler " "$out/tiered-$upscaler.log"; then
    echo "FAIL: tiered-$upscaler did not run $upscaler"
    status=1
  fi
  [ "$upscaler" = taau ] && continue
  run "tiered-$upscaler-tiny" "25:8x8,50:640x360" MOBA_RENDER_PATH=tiered \
    MOBA_UPSCALER="$upscaler" MOBA_UPSCALE_MODE=performance
done
# Compat with TAAU's set (it has no temporal back end).
if [[ $upscalers == *" taau "* ]]; then
  run compat "$sizes" MOBA_RENDER_PATH=compat
fi
exit $status
