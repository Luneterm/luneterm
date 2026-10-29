#!/usr/bin/env bash
# Runs the CI check commands on Linux in Docker (rust:1.92), with the repo mounted read-only.
# Target dir, cargo home and rustup state live in named volumes prefixed luneterm-linux-;
# the script creates them if missing and removes nothing (only its own --rm container).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

for v in target cargo rustup; do
  docker volume inspect "luneterm-linux-$v" >/dev/null 2>&1 || docker volume create "luneterm-linux-$v" >/dev/null
done

exec docker run --rm \
  -v "$root:/work:ro" \
  -v luneterm-linux-target:/target \
  -v luneterm-linux-cargo:/usr/local/cargo \
  -v luneterm-linux-rustup:/usr/local/rustup \
  -e CARGO_TARGET_DIR=/target \
  -w /work \
  rust:1.92 \
  bash -c '
    set -euo pipefail
    # No apt packages needed: rust:1.92 builds the workspace as is (verified 2026-10-29).
    # If a dependency ever needs one, install it here with apt-get.
    run() { echo "+ $*"; "$@" || { echo "FAILED: $*" >&2; exit 1; }; }
    run cargo fmt --all -- --check
    run cargo clippy --workspace --all-targets --locked -- -D warnings
    run cargo test --workspace --locked
    echo "check-linux: all passed"
  '
