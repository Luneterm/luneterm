#!/usr/bin/env bash
# Regenerates crates/lune-vt-conformance/goldens from the reference xterm.js (tools/xterm-oracle),
# then self-checks the 27 Lunegit vectors: the oracle's grid must equal Lunegit's tmux grid for the
# 21 non-divergent vectors (fail otherwise); the 6 divergent ones are reported, not enforced.
# Needs node and pnpm. Deterministic: a second run leaves no git diff under goldens/.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ORACLE="$ROOT/tools/xterm-oracle"

for tool in node pnpm; do
  if ! command -v "$tool" > /dev/null 2>&1; then
    echo "regen-vt-goldens: '$tool' not found on PATH" >&2
    exit 2
  fi
done

cd "$ORACLE"
pnpm install --frozen-lockfile
node gen-goldens.mjs
node gen-goldens.mjs --check-lunegit
