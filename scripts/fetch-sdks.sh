#!/usr/bin/env bash
# Downloads and verifies the pinned vendor SDKs behind moba-render's optional features
# (ADR-0020 §6b). Nothing is installed system-wide and nothing from an archive is executed.
#
# Usage:
#   scripts/fetch-sdks.sh                     # every verified entry in vendor-sdks.toml
#   scripts/fetch-sdks.sh --only dlss,nrd     # just these features or entry names
#   scripts/fetch-sdks.sh --only vendor-sdks  # the aggregates nvidia, vendor-sdks, dlss-fg work too
#   scripts/fetch-sdks.sh --present-features  # print the features whose SDKs are all present
#
# Each archive comes from the URL pinned in vendor-sdks.toml (an official GitHub release asset
# or tag/commit archive), is checked against its SHA-256 and unpacked into
# ${MOBA_SDK_DIR:-target/vendor-sdks}/<dir>/. Git submodules and CMake downloads are entries
# of their own, unpacked inside their parent at the path its build expects. The licence file's
# SHA-256 is checked after unpacking: a changed licence text fails the fetch, so a version bump
# forces the re-read roadmap §7 asks for.
#
# Idempotent and offline-friendly: an entry whose stamp matches the pin is left alone, and a
# previously downloaded archive in downloads/ is reused once its checksum verifies again.
#
# Windows: use scripts/fetch-sdks.ps1 (this script also works from Git Bash).
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
MANIFEST="$ROOT/vendor-sdks.toml"
SDK_DIR="${MOBA_SDK_DIR:-$ROOT/target/vendor-sdks}"
# A relative MOBA_SDK_DIR means the same directory as in moba-render's build script: relative
# to the workspace root, not to wherever this script is run from.
case "$SDK_DIR" in /*) ;; *) SDK_DIR="$ROOT/$SDK_DIR" ;; esac
STAMP=.moba-sdk-sha256

die() { echo "fetch-sdks: error: $*" >&2; exit 1; }
say() { echo "fetch-sdks: $*"; }

ONLY=""
MODE=fetch
while [ $# -gt 0 ]; do
  case "$1" in
    --only) [ $# -ge 2 ] || die "--only needs a comma-separated list"; ONLY="$2"; shift 2 ;;
    --only=*) ONLY="${1#--only=}"; shift ;;
    --present-features) MODE=present; shift ;;
    -h | --help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "usage: $0 [--only a,b,...] [--present-features]" >&2; exit 2 ;;
  esac
done

[ -f "$MANIFEST" ] || die "missing $MANIFEST"

# One line per [[sdk]] entry, in file order, with the fields separated by the unit separator
# (0x1f, not whitespace, so empty fields survive `read`). Only the flat subset the manifest
# uses is understood: `key = "string"` and `key = ["a", "b"]` on one line.
FIELDS=(name parent features url sha256 github_digest strip dir licence licence_sha256 status platforms)
entries() {
  awk -v fields="${FIELDS[*]}" '
    BEGIN { n = split(fields, order, " ") }
    function flush() {
      if (!have) return
      line = ""
      for (i = 1; i <= n; i++) line = line (i > 1 ? "\037" : "") (order[i] in v ? v[order[i]] : "")
      print line
      delete v
      have = 0
    }
    /^\[\[sdk\]\]/ { flush(); have = 1; next }
    /^\[/ { flush(); next }
    have && /^[a-z_0-9]+[ \t]*=/ {
      key = $0; sub(/[ \t]*=.*/, "", key)
      val = $0; sub(/^[^=]*=[ \t]*/, "", val)
      if (val ~ /^\[/) { gsub(/[\[\]" \t]/, "", val) } else { sub(/^"/, "", val); sub(/".*$/, "", val) }
      v[key] = val
    }
    END { flush() }
  ' "$MANIFEST"
}

# --only accepts entry names, feature names and the aggregate features of moba-render.
expand() {
  local item out=""
  for item in ${1//,/ }; do
    case "$item" in
      nvidia) out+=" dlss nrd rtxdi sharc" ;;
      vendor-sdks) out+=" dlss nrd rtxdi sharc fsr4 xess" ;;
      dlss-fg) out+=" dlss streamline" ;;
      *) out+=" $item" ;;
    esac
  done
  echo "$out "
}
WANTED=$(expand "$ONLY")

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{ print $1 }'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{ print $1 }'
  else
    die "need sha256sum or shasum to verify archives"
  fi
}

# Features of each entry, sub-entries inheriting their parent's.
declare -A FEATURES_OF=()
declare -A SELECTED=()
mapfile -t ROWS < <(entries)
[ "${#ROWS[@]}" -gt 0 ] || die "no [[sdk]] entries in $MANIFEST"

for row in "${ROWS[@]}"; do
  IFS=$'\x1f' read -r name parent features _ <<<"$row"
  [ -n "$parent" ] && features="${FEATURES_OF[$parent]:-}"
  FEATURES_OF[$name]="$features"
done

# Every --only name must be an entry, a feature or an aggregate: a typo is an error, not a
# silently smaller fetch.
for item in ${ONLY//,/ }; do
  case "$item" in nvidia | vendor-sdks | dlss-fg) continue ;; esac
  [ -n "${FEATURES_OF[$item]+set}" ] && continue
  known=0
  for features in "${FEATURES_OF[@]}"; do
    [[ ",$features," == *",$item,"* ]] && { known=1; break; }
  done
  [ "$known" = 1 ] || die "--only: unknown name '$item' (use entry or feature names from $MANIFEST, or nvidia, vendor-sdks, dlss-fg)"
done

selected() {
  local name="$1" parent="$2" feature
  [ -z "$ONLY" ] && return 0
  [[ "$WANTED" == *" $name "* ]] && return 0
  [ -n "$parent" ] && [ -n "${SELECTED[$parent]:-}" ] && return 0
  for feature in ${FEATURES_OF[$name]//,/ }; do
    [[ "$WANTED" == *" $feature "* ]] && return 0
  done
  return 1
}

# An entry is present when its stamp records the pinned archive and its licence still hashes
# to the pin.
present() {
  local dest="$1" archive_sha="$2" licence="$3" licence_sha="$4"
  [ -f "$dest/$STAMP" ] && [ "$(cat "$dest/$STAMP")" = "$archive_sha" ] \
    && [ -f "$dest/$licence" ] && [ "$(sha256 "$dest/$licence")" = "$licence_sha" ]
}

if [ "$MODE" = present ]; then
  declare -A MISSING=()
  declare -A KNOWN=()
  for row in "${ROWS[@]}"; do
    IFS=$'\x1f' read -r name parent _ url archive_sha _ _ dir licence licence_sha status _ <<<"$row"
    for feature in ${FEATURES_OF[$name]//,/ }; do
      KNOWN[$feature]=1
      if [ "$status" != verified ] || ! present "$SDK_DIR/$dir" "$archive_sha" "$licence" "$licence_sha"; then
        MISSING[$feature]=1
      fi
    done
  done
  out=""
  for feature in dlss streamline nrd rtxdi sharc fsr4 xess; do
    [ -n "${KNOWN[$feature]:-}" ] && [ -z "${MISSING[$feature]:-}" ] && out+="${out:+,}$feature"
  done
  echo "$out"
  exit 0
fi

unpack() {
  local archive="$1" into="$2"
  case "$archive" in
    *.zip)
      # GNU tar cannot read zip archives; bsdtar (libarchive) can.
      if command -v unzip >/dev/null 2>&1; then
        unzip -q "$archive" -d "$into"
      elif command -v bsdtar >/dev/null 2>&1; then
        bsdtar -xf "$archive" -C "$into"
      else
        die "need unzip or bsdtar to unpack $archive"
      fi
      ;;
    *.tar.gz | *.tgz) tar -xzf "$archive" -C "$into" ;;
    *) die "unknown archive type: $archive" ;;
  esac
}

mkdir -p "$SDK_DIR/downloads"
fetched=0
skipped=0
for row in "${ROWS[@]}"; do
  IFS=$'\x1f' read -r name parent _ url archive_sha digest strip dir licence licence_sha status platforms <<<"$row"
  selected "$name" "$parent" || continue
  SELECTED[$name]=1
  dest="$SDK_DIR/$dir"

  if [ "$status" != verified ]; then
    say "skipping $name: marked $status in vendor-sdks.toml (see docs/dev/vendor-sdks.md)"
    skipped=$((skipped + 1))
    continue
  fi
  [ -n "$url" ] && [ -n "$archive_sha" ] && [ -n "$dir" ] && [ -n "$licence" ] && [ -n "$licence_sha" ] \
    || die "$name: incomplete entry in $MANIFEST"
  case "$url" in https://github.com/*) ;; *) die "$name: $url is not a GitHub URL" ;; esac
  if [ -n "$digest" ] && [ "$digest" != "sha256:$archive_sha" ]; then
    die "$name: github_digest $digest does not match sha256 $archive_sha"
  fi

  if present "$dest" "$archive_sha" "$licence" "$licence_sha"; then
    say "$name: present at $dest"
    continue
  fi

  download="$SDK_DIR/downloads/$name-${url##*/}"
  if [ -f "$download" ] && [ "$(sha256 "$download")" = "$archive_sha" ]; then
    say "$name: reusing verified $download"
  else
    command -v curl >/dev/null 2>&1 || die "need curl to download $url"
    say "$name: downloading $url"
    partial="$download.partial"
    rm -f "$partial"
    curl --fail --location --proto '=https' --tlsv1.2 --retry 3 --silent --show-error \
      --output "$partial" "$url" || die "$name: download failed: $url"
    actual=$(sha256 "$partial")
    if [ "$actual" != "$archive_sha" ]; then
      rm -f "$partial"
      die "$name: checksum mismatch for $url: expected $archive_sha, got $actual"
    fi
    mv -f "$partial" "$download"
  fi

  # Unpack into a staging directory, check the licence, then move into place, so an
  # interrupted or rejected fetch never leaves a tree that looks complete.
  staging="$SDK_DIR/.staging-$name"
  rm -rf "$staging"
  mkdir -p "$staging"
  unpack "$download" "$staging"
  root="$staging"
  if [ -n "$strip" ]; then
    root="$staging/$strip"
    [ -d "$root" ] || die "$name: the archive has no top-level directory $strip"
  fi
  [ -f "$root/$licence" ] || die "$name: the archive has no licence file $licence"
  actual=$(sha256 "$root/$licence")
  if [ "$actual" != "$licence_sha" ]; then
    kept="$SDK_DIR/changed-licence-$name-${licence##*/}"
    cp "$root/$licence" "$kept"
    rm -rf "$staging"
    die "$name: the licence text changed ($licence: expected $licence_sha, got $actual). Re-read it (roadmap §7; copy at $kept), record the re-read in the PR and update licence_sha256 in vendor-sdks.toml"
  fi
  printf '%s\n' "$archive_sha" >"$root/$STAMP"
  rm -rf "$dest"
  mkdir -p "$(dirname "$dest")"
  mv "$root" "$dest"
  rm -rf "$staging"
  say "$name: verified archive and licence; unpacked to $dest (binaries for: ${platforms//,/, })"
  fetched=$((fetched + 1))
done

[ -n "$ONLY" ] && [ "${#SELECTED[@]}" -eq 0 ] && die "--only $ONLY matched no entry in $MANIFEST"
say "done: $fetched fetched, $skipped skipped, SDK dir $SDK_DIR"
