#!/usr/bin/env bash
# Builds the lunepack binaries a release needs, from the source at the exact revision Luneterm
# pins (the `lunepack` dependency in Cargo.toml): the runtime and the update window's UI for the
# target, and on macOS also the `lunepack` CLI (package-macos.sh assembles and packs with it).
# lunepack publishes no prebuilt binaries, and its own pinned toolchain and lock build them.
#
#   scripts/build-lunepack.sh <linux-x86_64|macos-aarch64> <outdir>
#
# <outdir> must not exist or be empty. It gets lunepack-runtime and lunepack-ui, and on macOS
# lunepack too, and lunepack-rev.txt (the revision built). Windows has scripts/build-lunepack.ps1.
#
# The checkout goes to target/lunepack-src-<rev12> (reused when it is clean at the pinned
# revision, else the script stops: nothing is deleted), the build to target/lunepack-target. Before
# anything is built, `cargo metadata --locked` of the checkout must show every package from
# crates.io or the checkout itself: this code ships inside every user's install, and the
# supply-chain gate (scripts/check-deps.sh) covers only Luneterm's own graph.
#
# Linux needs musl-tools (`musl-gcc`) and readelf; the static runtime is checked by lunepack's own
# build script. macOS needs the Xcode command line tools. rustup, when present, installs the
# checkout's pinned toolchain and the musl target.
set -euo pipefail

die() { echo "build-lunepack: $*" >&2; exit 1; }

[ "$#" -eq 2 ] || die "usage: scripts/build-lunepack.sh <linux-x86_64|macos-aarch64> <outdir>"
target="$1"
case "$target" in
  linux-x86_64) [ "$(uname -s)" = "Linux" ] && [ "$(uname -m)" = "x86_64" ] \
    || die "linux-x86_64 is built on an x86_64 Linux host" ;;
  macos-aarch64) [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ] \
    || die "macos-aarch64 is built on an arm64 Mac" ;;
  *) die "unknown target: $target" ;;
esac
for tool in git python3 cargo; do
  command -v "$tool" > /dev/null 2>&1 || die "required tool not found: $tool"
done

case "$2" in /*) outdir="$2" ;; *) outdir="$PWD/$2" ;; esac
if [ -e "$outdir" ]; then
  [ -d "$outdir" ] && [ -z "$(ls -A "$outdir")" ] || die "$outdir exists and is not an empty directory"
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The pin: the one `lunepack = { git = ..., rev = ... }` line of the root manifest.
read -r repo rev < <(python3 -I - "$root/Cargo.toml" << 'PY'
import re
import sys
import tomllib

with open(sys.argv[1], "rb") as f:
    dep = tomllib.load(f)["workspace"]["dependencies"]["lunepack"]
git, rev = dep.get("git", ""), dep.get("rev", "")
if not re.fullmatch(r"https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", git):
    sys.exit("the lunepack dependency is not a github.com git dependency")
if not re.fullmatch(r"[0-9a-f]{40}", rev):
    sys.exit("the lunepack dependency has no 40-hex rev")
print(git, rev)
PY
) || die "cannot read the lunepack pin from Cargo.toml"

src="$root/target/lunepack-src-${rev:0:12}"
mkdir -p "$root/target"
if [ -d "$src" ]; then
  [ "$(git -C "$src" rev-parse HEAD 2> /dev/null)" = "$rev" ] || die "$src is not at $rev; remove it by hand"
  [ -z "$(git -C "$src" status --porcelain)" ] || die "$src has local changes; remove it by hand"
else
  tmp="$(mktemp -d "$root/target/lunepack-src.XXXXXX")"
  git -C "$tmp" init -q
  git -C "$tmp" remote add origin "$repo"
  if ! git -C "$tmp" fetch -q --depth 1 origin "$rev"; then
    git -C "$tmp" fetch -q origin || die "cannot fetch $repo"
  fi
  git -C "$tmp" checkout -q --detach "$rev" || die "$rev is not in $repo"
  [ "$(git -C "$tmp" rev-parse HEAD)" = "$rev" ] || die "checked out the wrong commit"
  mv "$tmp" "$src"
fi

# lunepack's own rustflags and build id rule: nothing from the caller's environment.
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_RUSTFLAGS LUNEPACK_BUILD_ID
export CARGO_TARGET_DIR="$root/target/lunepack-target"
if command -v rustup > /dev/null 2>&1; then
  (cd "$src" && rustup toolchain install > /dev/null 2>&1) || die "cannot install the toolchain $src pins"
  if [ "$target" = "linux-x86_64" ]; then
    (cd "$src" && rustup target add x86_64-unknown-linux-musl > /dev/null 2>&1) \
      || die "cannot add the musl target"
  fi
fi

# Every package of the checkout's graph: crates.io, or a path inside the checkout.
(cd "$src" && cargo metadata --locked --format-version 1) | python3 -I -c '
import json
import os
import sys

src = os.path.realpath(sys.argv[1])
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
meta = json.load(sys.stdin)
bad = []
for p in meta["packages"]:
    source = p.get("source")
    if source is None:
        path = os.path.realpath(os.path.dirname(p["manifest_path"]))
        if path != src and not path.startswith(src + os.sep):
            bad.append("%s %s: path outside the checkout" % (p["name"], p["version"]))
    elif source != CRATES_IO:
        bad.append("%s %s: source %s" % (p["name"], p["version"], source.split("#")[0].split("?")[0]))
if bad:
    sys.exit("lunepack graph check failed:\n  " + "\n  ".join(bad))
print("lunepack graph: %d packages, all crates.io or in the checkout" % len(meta["packages"]))
' "$src" || die "the lunepack checkout's dependencies are not all from crates.io"

mkdir -p "$outdir"
build_runtime="$src/scripts/build-runtime.sh"
cd "$src"
case "$target" in
  linux-x86_64)
    "$build_runtime" linux-x86_64 --package lunepack-runtime --out "$outdir/lunepack-runtime"
    # The UI links the host's C library: it runs on this glibc and newer (the build job records
    # the floor in glibc-min.txt).
    cargo build --locked --release -p lunepack-ui --target x86_64-unknown-linux-gnu \
      --config profile.release.strip=true
    ui="$CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release/lunepack-ui"
    [ -f "$ui" ] || die "the build left no $ui"
    readelf -hW "$ui" | grep -q 'Advanced Micro Devices X86-64' || die "$ui is not an x86-64 ELF"
    cp "$ui" "$outdir/lunepack-ui"
    ;;
  macos-aarch64)
    "$build_runtime" macos-aarch64 --package lunepack-runtime --out "$outdir/lunepack-runtime"
    "$build_runtime" macos-aarch64 --package lunepack-ui --out "$outdir/lunepack-ui"
    cargo build --locked --release -p lunepack-cli --config profile.release.strip=true
    [ -f "$CARGO_TARGET_DIR/release/lunepack" ] || die "the build left no lunepack CLI"
    cp "$CARGO_TARGET_DIR/release/lunepack" "$outdir/lunepack"
    ;;
esac
printf '%s\n' "$rev" > "$outdir/lunepack-rev.txt"
echo "build-lunepack: $target built from $rev into $outdir"
