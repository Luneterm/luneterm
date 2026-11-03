#!/usr/bin/env bash
# Cost of REP (CSI Ps b) per input byte in lune-vt, release build (crates/lune-vt/examples/rep_cost.rs):
# plain, link, cluster, extended-attribute REP at 80x24 and 300x100 (scrollback 1,000 / 10,000), the
# DECALN control, the no-wrap insert REP on lines longer than the window, (M7 fix round 2) REP
# below a scroll region, below a region on a long row in Windows mode (no reflow), and of a
# joining-base unit, (M7 fix round 3) REP below a region at odd widths (a widened unit's
# joiner wraps its base there), and (M7 fix round 4) REP at odd widths over narrowed rows of
# combined strings (that wrap copies the rows' strings; M7 fix round 5 adds rows not narrowed,
# the even widths 80 and 22 as the same-unit reference, plain `a` and the keycap at the scroll
# bottom), and (M7 fix round 7) REP in insert mode below a region at odd widths over narrowed
# rows of strings at the security review's geometries (first REP and sustained rows too;
# `REP_COST_TABLE=odd_widths_insert` or `below_region_odd` runs that table alone). Median of 5 fresh terminals
# each; prints one table with the machine's load average. No pass/fail threshold: the S7 residual steps (plan M7 11 to 13) and the M7 fix rounds
# compare their before and after tables from this script.
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
