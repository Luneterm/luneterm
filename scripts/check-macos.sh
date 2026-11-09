#!/usr/bin/env bash
# The macOS gate, run locally (there is no macOS CI runner since 2026-11-09, when hosted runners
# were blocked by billing; CI covers Linux and Windows on self-hosted runners). It is what the
# macOS job of .github/workflows/ci.yml ran: fmt, clippy, the workspace tests, the C ABI header
# check (needs clang and nm), the lune-pty soak and the lune-termview probe, every cargo call
# with --locked. Stops at the first failure; exit 0 only when every step passed.
set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
  echo "check-macos: this is the macOS gate; run it on macOS" >&2
  exit 2
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

run() { echo "+ $*"; "$@" || { echo "FAILED: $*" >&2; exit 1; }; }

run cargo fmt --all -- --check
run cargo clippy --workspace --all-targets --locked -- -D warnings
run cargo test --workspace --locked
run scripts/check-ffi.sh
# M3 D10: lune-pty flake detector, 5 consecutive runs.
for i in 1 2 3 4 5; do
  echo "lune-pty soak run $i"
  run cargo test -p lune-pty --locked
done
run cargo test -p lune-termview --locked --test probe -- --nocapture
echo "check-macos: all passed"
