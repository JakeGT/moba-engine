#!/usr/bin/env bash
# PostToolUse hook: format the edited Rust file and type-check its crate.
# Exit 2 feeds compiler errors back to Claude; anything else is non-blocking.
set -uo pipefail

file=$(jq -r '.tool_input.file_path // .tool_response.filePath // empty')
[[ "$file" == *.rs && -f "$file" ]] || exit 0
command -v cargo >/dev/null 2>&1 || exit 0

rustfmt --edition 2024 "$file" 2>/dev/null || true

# Find the owning crate's manifest.
dir=$(dirname "$file")
while [[ "$dir" != "/" && ! -f "$dir/Cargo.toml" ]]; do dir=$(dirname "$dir"); done
[[ -f "$dir/Cargo.toml" ]] || exit 0
pkg=$(sed -n 's/^name = "\(.*\)"/\1/p' "$dir/Cargo.toml" | head -1)
[[ -n "$pkg" ]] || exit 0

if ! out=$(cd "$dir" && cargo check -p "$pkg" --all-targets --message-format short 2>&1); then
  echo "cargo check -p $pkg failed:" >&2
  echo "$out" | grep -E '^(error|warning)|-->' | head -40 >&2
  exit 2
fi
exit 0
