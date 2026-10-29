#!/usr/bin/env bash
# Fetch the pinned xterm.js source into reference/xterm.js (gitignored) for porting.
# The pin must match lune_vt::XTERM_JS_TAG / XTERM_JS_COMMIT.
set -euo pipefail

TAG=6.0.0
COMMIT=f447274f430fd22513f6adbf9862d19524471c04
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/reference/xterm.js"

if [[ ! -d "$DEST/.git" ]]; then
  git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$TAG" https://github.com/xtermjs/xterm.js.git "$DEST"
fi

actual="$(git -C "$DEST" rev-parse HEAD)"
if [[ "$actual" != "$COMMIT" ]]; then
  echo "reference/xterm.js is at $actual, expected $COMMIT (tag $TAG)" >&2
  exit 1
fi
echo "reference/xterm.js @ $TAG ($COMMIT)"
