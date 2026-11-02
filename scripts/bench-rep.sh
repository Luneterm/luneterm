#!/usr/bin/env bash
# Cost of REP (CSI Ps b) per input byte in lune-vt, release build (crates/lune-vt/examples/rep_cost.rs):
# plain, link, cluster, extended-attribute REP at 80x24 and 300x100 (scrollback 1,000 / 10,000), the
# DECALN control, and the no-wrap insert REP on lines longer than the window. Median of 5 fresh
# terminals each; prints one table with the machine's load average. No pass/fail threshold: the
# S7 residual steps (plan M7 11 to 13) compare their before and after tables from this script.
# Needs only a Rust toolchain. Takes about a minute; run it on an idle machine and read the load
# average printed with the table.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v cargo > /dev/null 2>&1; then
  echo "bench-rep: 'cargo' not found on PATH" >&2
  exit 2
fi

(cd "$ROOT" && cargo build --release -q -p lune-vt --example rep_cost)
echo "command:  scripts/bench-rep.sh   rustc: $(rustc --version)"
(cd "$ROOT" && cargo run --release -q -p lune-vt --example rep_cost)
