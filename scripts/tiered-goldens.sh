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
#   - an exact SHA-256 check against tests/golden/render-tiered-hashes.txt, which has one
#     section per device and CPU class (ADR-0021 amendment 6): a `device <device> cpu=<class>`
#     line, then `<capture> <sha256>` lines. The device is CI's lavapipe (the device name,
#     Vulkan version and driver info (Mesa and LLVM versions) from `vulkan-probe --json`):
#     llvmpipe's code generation differs between Mesa and LLVM versions, so another lavapipe
#     renders other bytes for every back end. The class is the CPU vendor (`amd`, `intel`,
#     from /proc/cpuinfo): llvmpipe's JIT uses RSQRTPS, an approximation whose bits are the
#     vendor's own, so the same lavapipe renders other bytes on Intel than on AMD (AVX-512 or
#     not makes no difference). On a device the file names, a CPU class it has no section for
#     fails the run for every back end, with the section to add. On another device the check
#     is skipped with a log line, except on CI (GITHUB_ACTIONS) for FSR 3.1, whose only
#     regression check it is: there it fails the run (TAAU, in the required renderer-gate,
#     warns and keeps its fingerprints). Every run writes its section to
#     <output dir>/hashes-<back ends>.txt (with the CPU it ran on), which CI's artifact keeps:
#     fill the file from CI runs only, one per class. WRITE_HASHES=1 rewrites the current
#     device and class's section in place (on CI's lavapipe only); TIERED_HASHES names
#     another file;
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
# The runner's CPU and its class, the vendor (above; ADR-0021 amendment 6).
cpuinfo() { sed -n "/^$1[[:space:]]*: /{s///p;q;}" /proc/cpuinfo 2>/dev/null || true; }
cpu_flags=" $(cpuinfo flags) "
cpu_has=
for flag in avx2 fma f16c avx512f avx512vl avx512bw avx512dq avx512_bf16 avx512_fp16; do
  if [[ "$cpu_flags" == *" $flag "* ]]; then cpu_has="$cpu_has $flag"; fi
done
cpu_line="vendor=$(cpuinfo vendor_id) family=$(cpuinfo 'cpu family') model=$(cpuinfo model)"
cpu_line="$cpu_line stepping=$(cpuinfo stepping) nproc=$(nproc) name='$(cpuinfo 'model name')'"
cpu_line="$cpu_line flags:${cpu_has:- none}"
case "$(cpuinfo vendor_id)" in
  GenuineIntel) cpu_class=intel ;;
  AuthenticAMD) cpu_class=amd ;;
  "") cpu_class=unknown ;;
  *) cpu_class=$(cpuinfo vendor_id | tr -c 'A-Za-z0-9\n' _) ;;
esac
echo "cpu: $cpu_line (class $cpu_class)"
key="${device:-unknown} cpu=$cpu_class"
# The hash file has one section per key: a `device <device> cpu=<class>` line, then
# `<capture> <sha256>` lines.
section() { awk -v want="device $1" '/^device /{on = ($0 == want); next} on && NF == 2 && !/^#/' "$hashes"; }
keys=$(sed -n 's/^device //p' "$hashes" 2>/dev/null || true)
classes=$(awk -v d="device $device cpu=" 'index($0, d) == 1 { print substr($0, length(d) + 1) }' \
  "$hashes" 2>/dev/null | tr '\n' ' ' || true)
# This run's hashes as a section of the file, kept in the artifact (CI fills the file from it).
manifest="$out/hashes-${upscalers// /-}.txt"
printf '%s\n%s\n' "# cpu $cpu_line" "device $key" > "$manifest"
exact=0
status=0
regenerate="add a 'device $key' section to $hashes from a CI run on this class (its artifact"
regenerate="$regenerate has it in $manifest); never from a local run"
if [ "${WRITE_HASHES:-0}" = 1 ]; then
  test -n "$device" || { echo "FAIL: vulkan-probe named no device"; exit 1; }
  : > "$hashes.new"
elif [ -n "$device" ] && printf '%s\n' "$keys" | grep -qxF "$key"; then
  exact=1
  echo "exact hashes: on $key"
elif [ -n "$device" ] && [ -n "$classes" ]; then
  # The device is known and this CPU class is not: fail loudly, for every back end.
  echo "::error::tiered-goldens: no exact hashes for CPU class '$cpu_class' on '$device'" \
    "(recorded: ${classes% }); $regenerate"
  echo "FAIL: exact hashes: no section for '$key' (recorded classes: ${classes% }); $regenerate"
  status=1
else
  echo "exact hashes: skipped, this device is '${device:-unknown}' and $hashes has" \
    "'$(printf '%s' "$keys" | tr '\n' ';')'; the fingerprints alone are checked"
  # On CI the runner's lavapipe changed (or vulkan-probe failed). FSR 3.1 has no other
  # regression check, so its job fails (after the captures, which the artifact keeps);
  # TAAU's, in the required renderer-gate, warns.
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
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
    echo "$capture_name $hash" >> "$manifest"
    echo "sha256: $capture_name $hash"
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
      want=$(section "$key" | sed -n "s/^$capture_name //p")
      if [ "$hash" = "$want" ]; then
        echo "ok: tiered $capture_name: exact hash"
      else
        echo "FAIL: tiered $capture_name: hash ${hash:0:16}, expected ${want:0:16}"
        echo "  actual   $hash"
        echo "  expected ${want:-none}"
        echo "  on       $key ($cpu_line)"
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
  # Keep this section's other back ends when only some were run, and every other section.
  if [ -f "$hashes" ]; then
    section "$key" | while read -r name hash; do
      grep -q "^$name " "$hashes.new" || echo "$name $hash" >> "$hashes.new"
    done
  fi
  {
    if [ -f "$hashes" ] && grep -q '^#' "$hashes"; then
      sed -n '/^#/p;/^device /q' "$hashes"
    else
      echo "# SHA-256 of the tiered captures (scripts/tiered-goldens.sh), per device and CPU class."
    fi
    { printf '%s\n' "$keys"; echo "$key"; } | grep -v '^$' | sort -u | while IFS= read -r k; do
      echo "device $k"
      if [ "$k" = "$key" ]; then sort "$hashes.new"; else section "$k" | sort; fi
    done
  } > "$hashes.tmp"
  mv "$hashes.tmp" "$hashes"
  rm "$hashes.new"
  echo "wrote $hashes for $key"
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
