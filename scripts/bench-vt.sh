#!/usr/bin/env bash
# Throughput of lune-vt (Terminal::write, release) vs @xterm/headless 6.0.0 over the same ~16 MiB
# generated stream (tools/xterm-oracle/bench.mjs), 64 KiB writes, 80x24, Unicode 11, median of 5.
# Prints MiB/s side by side with the Node version, machine and command. No pass/fail threshold.
# The stream goes to a scratch dir this script creates and removes; nothing large is committed.
# Needs node and pnpm (for the oracle's node_modules) and a Rust toolchain.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ORACLE="$ROOT/tools/xterm-oracle"

for tool in node pnpm cargo; do
  if ! command -v "$tool" > /dev/null 2>&1; then
    echo "bench-vt: '$tool' not found on PATH" >&2
    exit 2
  fi
done

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/bench-vt.XXXXXX")"
trap 'rm -rf "$SCRATCH"' EXIT
STREAM="$SCRATCH/stream.bin"

(cd "$ORACLE" && pnpm install --frozen-lockfile > /dev/null)
node "$ORACLE/bench.mjs" --generate "$STREAM"

(cd "$ROOT" && cargo build --release -q -p lune-vt --example throughput)
RUST_OUT="$(cd "$ROOT" && cargo run --release -q -p lune-vt --example throughput -- "$STREAM")"
JS_OUT="$(cd "$ORACLE" && node bench.mjs --run "$STREAM")"

field() { # <output> <key> -> value after "key=" on its line
  printf '%s\n' "$1" | sed -n "s/^$2=//p" | head -n 1
}
median_of() { # <line> -> value after "median="
  printf '%s\n' "$1" | sed -n 's/.*median=//p'
}

js_api="$(median_of "$(printf '%s\n' "$JS_OUT" | grep '^write_api_mib_s=')")"
js_sync="$(median_of "$(printf '%s\n' "$JS_OUT" | grep '^core_write_sync_mib_s=')")"
rs="$(median_of "$(printf '%s\n' "$RUST_OUT" | grep '^write_mib_s=')")"

echo
echo "machine:  $(uname -srm); $(sysctl -n machdep.cpu.brand_string 2> /dev/null || grep -m1 'model name' /proc/cpuinfo 2> /dev/null | cut -d: -f2- | sed 's/^ //' || echo unknown)"
echo "node:     $(node --version)   rustc: $(rustc --version)"
echo "command:  scripts/bench-vt.sh"
echo "input:    $(field "$RUST_OUT" bytes)   (js: $(field "$JS_OUT" bytes))"
echo
echo "--- raw: lune-vt"
printf '%s\n' "$RUST_OUT"
echo "--- raw: @xterm/headless"
printf '%s\n' "$JS_OUT"
echo
printf '%-52s %10s\n' "engine (MiB/s, median of 5)" "MiB/s"
printf '%-52s %10s\n' "lune-vt Terminal::write (release)" "$rs"
printf '%-52s %10s\n' "@xterm/headless Terminal.write (public API)" "$js_api"
printf '%-52s %10s\n' "@xterm/headless core.writeSync (no time slicing)" "$js_sync"
echo
awk -v rs="$rs" -v api="$js_api" -v sy="$js_sync" 'BEGIN {
  printf "lune-vt / headless write API:  %.2fx\nlune-vt / headless writeSync:  %.2fx\n", rs / api, rs / sy
  if (rs < api / 2) print "NOTE: lune-vt is below half of @xterm/headless (public write API); record in notes.md and Risks"
}'
