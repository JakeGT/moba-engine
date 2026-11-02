#!/usr/bin/env bash
# Downloads the pinned Slang compiler for this host (ADR-0020 §4c).
#
# Usage:
#   scripts/fetch-slang.sh            # fetch, verify and unpack (no-op when already present)
#   scripts/fetch-slang.sh --print    # print the slangc path it would use, fetch nothing
#
# The version and the SHA-256 of each host archive come from crates/shaders/slang-version.txt.
# The archive is downloaded only from the official release
# (https://github.com/shader-slang/slang/releases), checked against that SHA-256, and unpacked
# into ${CARGO_TARGET_DIR:-target}/tools/slang-<version>/, where crates/shaders/build.rs
# looks for it. Nothing is installed system-wide. Only the CARGO_TARGET_DIR variable is read:
# with a `build.target-dir` in a Cargo config, set CARGO_TARGET_DIR to the same directory
# for this script (or point SLANGC at the compiler).
#
# Idempotent and offline-friendly: an unpacked toolchain whose recorded checksum matches the
# pin is reused without network access, and a previously downloaded archive in
# tools/downloads/ is reused once its checksum is verified again.
#
# Windows: use scripts/fetch-slang.ps1 (this script also works from Git Bash).
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
PIN="$ROOT/crates/shaders/slang-version.txt"
RELEASES="https://github.com/shader-slang/slang/releases/download"

PRINT=0
case "${1:-}" in
  "") ;;
  --print) PRINT=1 ;;
  *) echo "usage: $0 [--print]" >&2; exit 2 ;;
esac

die() { echo "fetch-slang: error: $*" >&2; exit 1; }

version=$(sed -n 's/^version=\([^[:space:]]*\).*/\1/p' "$PIN")
[ -n "$version" ] || die "no version= line in $PIN"

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  MINGW* | MSYS* | CYGWIN*) os=windows ;;
  *) die "unsupported host OS $(uname -s)" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) die "unsupported host architecture $(uname -m)" ;;
esac
host="$os-$arch"

read -r archive expected < <(awk -v host="$host" '$1 == host { print $2, $3 }' "$PIN") || true
[ -n "${archive:-}" ] && [ -n "${expected:-}" ] \
  || die "no pinned archive for host $host in $PIN (pinned hosts: $(awk '/^[a-z]+-/ { printf "%s ", $1 }' "$PIN"))"

target_dir="${CARGO_TARGET_DIR:-$ROOT/target}"
tools="$target_dir/tools"
dest="$tools/slang-$version"
exe=slangc
[ "$os" = windows ] && exe=slangc.exe

if [ "$PRINT" = 1 ]; then
  echo "$dest/bin/$exe"
  exit 0
fi

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{ print $1 }'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{ print $1 }'
  else
    die "need sha256sum or shasum to verify the archive"
  fi
}

mkdir -p "$target_dir"
# Mark the target directory as Cargo does (Cargo only writes the tag when it creates the
# directory itself, and this script may run first). build.rs stops its search for slangc at
# the first directory with this tag.
if [ ! -f "$target_dir/CACHEDIR.TAG" ]; then
  printf '%s\n' 'Signature: 8a477f597d28d172789f06886806bc55' \
    '# This file is a cache directory tag created by scripts/fetch-slang.sh.' \
    '# For information about cache directory tags see https://bford.info/cachedir/' \
    > "$target_dir/CACHEDIR.TAG"
fi

# Already unpacked from an archive with the pinned checksum: nothing to do, no network.
if [ -x "$dest/bin/$exe" ] && [ "$(cat "$dest/.archive-sha256" 2>/dev/null || true)" = "$expected" ]; then
  echo "fetch-slang: slangc $version already present at $dest/bin/$exe"
  exit 0
fi

mkdir -p "$tools/downloads"
download="$tools/downloads/$archive"
if [ -f "$download" ] && [ "$(sha256 "$download")" = "$expected" ]; then
  echo "fetch-slang: reusing verified $download"
else
  url="$RELEASES/v$version/$archive"
  echo "fetch-slang: downloading $url"
  command -v curl >/dev/null 2>&1 || die "need curl to download $url"
  partial="$download.partial"
  rm -f "$partial"
  curl --fail --location --proto '=https' --tlsv1.2 --retry 3 --silent --show-error \
    --output "$partial" "$url" || die "download failed: $url"
  actual=$(sha256 "$partial")
  if [ "$actual" != "$expected" ]; then
    rm -f "$partial"
    die "checksum mismatch for $archive: expected $expected, got $actual"
  fi
  mv -f "$partial" "$download"
fi

# Unpack into a staging directory next to the destination, then move it into place, so an
# interrupted run never leaves a half-unpacked toolchain that looks complete.
staging="$tools/.slang-$version.staging"
rm -rf "$staging"
mkdir -p "$staging"
case "$archive" in
  *.tar.gz) tar -xzf "$download" -C "$staging" ;;
  # bsdtar (macOS, Windows' tar.exe) reads zip archives; fall back to unzip.
  *.zip) tar -xf "$download" -C "$staging" 2>/dev/null || unzip -q "$download" -d "$staging" ;;
  *) die "unknown archive type: $archive" ;;
esac
# Releases unpack to bin/, lib/, include/ at the top level; tolerate one wrapping directory.
root="$staging"
if [ ! -d "$root/bin" ]; then
  inner=$(find "$staging" -mindepth 1 -maxdepth 1 -type d)
  [ "$(printf '%s\n' "$inner" | wc -l)" -eq 1 ] && [ -d "$inner/bin" ] && root="$inner"
fi
[ -f "$root/bin/$exe" ] || die "$archive has no bin/$exe"
chmod +x "$root/bin/$exe"
# Drop what slangc's SPIR-V path never loads, so the install (and CI's cache of it) is about
# 70 MB instead of 250: the LLVM back end (CPU/CUDA targets), gfx, the runtime library,
# headers and docs. The licence files stay.
rm -rf "$root"/lib/*slang-llvm* "$root"/bin/*slang-llvm* "$root"/lib/*gfx* "$root"/bin/gfx.* \
  "$root"/lib/*slang-rt* "$root"/bin/slang-rt* "$root/lib/cmake" "$root/lib/pkgconfig" \
  "$root/include" "$root/share/doc"
printf '%s\n' "$expected" > "$root/.archive-sha256"
rm -rf "$dest"
mv "$root" "$dest"
rm -rf "$staging"

echo "fetch-slang: slangc $version installed at $dest/bin/$exe"
"$dest/bin/$exe" -version 2>&1 | head -1 || true
