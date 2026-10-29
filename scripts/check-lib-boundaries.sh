#!/usr/bin/env bash
# Library boundary rules for the lune-* crates (decisions D12 and D21). Grep only, so it runs
# the same on macOS (bash 3.2, BSD grep) and Linux (GNU grep). Prints each offending line with
# its rule number. Exit 1 on any offender, 0 when clean.
#   1  no crates/lune-*/Cargo.toml mentions luneterm (no dependency on an app crate)
#   2  no file under crates/lune-* mentions luneterm
#   3  no file under crates/lune-ui mentions app or terminal words; exceptions are the fixed
#      strings in scripts/lib-boundaries.allow
#   4  no crates/lune-ui/src or crates/lune-termview/src file reads the clock or the environment
#      (keeps frames deterministic)
#   5  every crates/*/Cargo.toml and spikes/*/Cargo.toml inherits the licence from the workspace
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
ALLOW="scripts/lib-boundaries.allow"
fail=0

# drop_allowed: reads grep hits on stdin, prints the lines that contain no fixed string from $ALLOW.
# Blank lines and lines starting with # in $ALLOW are ignored.
drop_allowed() {
  local line pat keep
  while IFS= read -r line; do
    keep=1
    while IFS= read -r pat || [ -n "$pat" ]; do
      case "$pat" in '' | '#'*) continue ;; esac
      case "$line" in *"$pat"*) keep=0; break ;; esac
    done < "$ALLOW"
    if [ "$keep" -eq 1 ]; then printf '%s\n' "$line"; fi
  done
}

# check <rule> <allow: yes|no> <grep args...>: every grep hit is an offender (after the allow list).
# grep exit 2 (an error, e.g. a missing path) stops the script, so a bad path can't pass silently.
check() {
  local rule="$1" allow="$2" out kept rc=0
  shift 2
  out="$(grep "$@" 2>&1)" || rc=$?
  if [ "$rc" -gt 1 ]; then
    echo "check-lib-boundaries: grep failed (exit $rc) in rule $rule: $out" >&2
    exit 2
  fi
  if [ "$rc" -eq 0 ]; then
    kept="$out"
    if [ "$allow" = yes ]; then kept="$(printf '%s\n' "$out" | drop_allowed)"; fi
    if [ -n "$kept" ]; then
      printf '%s\n' "$kept" | sed "s/^/rule $rule: /"
      fail=1
    fi
  fi
}

check 1 no -n -i luneterm crates/lune-*/Cargo.toml
check 2 no -rn -i luneterm crates/lune-*
check 3 yes -rn -i -E 'mock|terminal|xterm|scrollback|glass_ui|(^|[^[:alnum:]_])(pty|shell|ansi)($|[^[:alnum:]_])' crates/lune-ui
check 4 no -rn -F -e 'env::var' -e 'Instant::now' -e 'SystemTime::now' -e '.elapsed()' crates/lune-ui/src crates/lune-termview/src

for m in crates/*/Cargo.toml spikes/*/Cargo.toml; do
  if ! grep -q -F 'license.workspace = true' "$m"; then
    echo "rule 5: $m: no 'license.workspace = true'"
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  echo "check-lib-boundaries: FAILED" >&2
  exit 1
fi
echo "check-lib-boundaries: clean"
