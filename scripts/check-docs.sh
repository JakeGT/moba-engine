#!/usr/bin/env bash
# Documentation consistency gate: relative links, current contract versions and test counts.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

python3 - <<'PY'
import pathlib, re, subprocess, sys, urllib.parse

root = pathlib.Path('.')
errors = []

# 1) Relative Markdown links must resolve.
for f in [p for p in root.rglob('*.md') if 'target' not in p.parts]:
    for m in re.finditer(r'\[[^\]]*\]\(([^)]+)\)', f.read_text(errors='ignore')):
        target = m.group(1).strip()
        if target.startswith(('http://', 'https://', 'mailto:', '#')):
            continue
        target = target.split('#', 1)[0]
        if target and not (f.parent / urllib.parse.unquote(target)).resolve().exists():
            errors.append(f'{f}: broken link {target}')

def read(path):
    return (root / path).read_text(errors='ignore')

protocol = re.search(r'PROTOCOL_VERSION: u8 = (\d+)', read('crates/net/src/protocol.rs')).group(1)
replay = re.search(r'(?:pub )?const VERSION: u8 = (\d+)', read('crates/sim/src/replay.rs')).group(1)
logic = re.search(r'SIM_LOGIC_VERSION: u8 = (\d+)', read('crates/sim/src/replay.rs')).group(1)

checks = {
    'protocol': (f'**v{protocol}**', ['AGENTS.md', 'docs/status.md']),
    'replay': (f'**v{replay}**', ['docs/status.md', 'AGENTS.md']),
    'simulation logic': (f'**v{logic}**', ['AGENTS.md', 'docs/status.md']),
}
for name, (needle, files) in checks.items():
    for path in files:
        if needle not in read(path):
            errors.append(f'{path}: missing current {name} token {needle!r}')

# Test counts in the front-door docs must agree.
counts = {}
for path in ['README.md', 'docs/status.md', 'AGENTS.md']:
    text = read(path)
    match = re.search(r'(\d+)\s+(?:workspace\s+)?(?:tests|passed)', text)
    if match:
        counts[path] = match.group(1)
if len(set(counts.values())) > 1:
    errors.append(f'test-count mismatch: {counts}')

# Build output must never be tracked.
tracked = subprocess.check_output(['git', 'ls-files', 'fuzz/target'], text=True).strip()
if tracked:
    errors.append('fuzz/target is tracked; keep build output out of git')

if errors:
    print('docs check failed:', file=sys.stderr)
    for error in errors:
        print(' -', error, file=sys.stderr)
    sys.exit(1)
print(f'docs check ok: protocol v{protocol}, replay v{replay}, logic v{logic}, tests {set(counts.values()) or "unknown"}')
PY
