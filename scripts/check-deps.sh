#!/usr/bin/env bash
# Supply-chain gate (M9 D18). Every package in the dependency graph must come from one of:
#   - this repo (a path package under the repo root),
#   - crates.io,
#   - the one luneui-rs git rev pinned below.
# The graph is read from `cargo metadata --locked` (cargo's own resolution, all platforms), never
# from a grep over Cargo.lock. Fails closed: a missing tool, a metadata error or an unrecognised
# source is a failure, never a skip. Exit 0 clean, 1 violations (one line each), 2 cannot run.
#   scripts/check-deps.sh          check
#   scripts/check-deps.sh --list   print the non-workspace packages as `name version source`
# A luneui-rs pin bump edits ALLOWED_REV here and the rev in the root Cargo.toml in one reviewed
# change. Violations:
#   - a package from a source outside the allowlist, or from luneui-rs at another rev;
#   - a path package outside the repo root;
#   - a dependency declared from git with a branch, a tag, no rev, a rev that is not 40 hex
#     digits, another repo, or another rev; a dependency from another registry;
#   - a `[patch]` or `[replace]` table in the root or a member manifest;
#   - a repo `.cargo/config*` with `[patch]`, `[source]` or `paths` (source replacement and path
#     overrides are invisible in cargo metadata), or one that cannot be parsed.
# Out of scope (not visible from the repo): $CARGO_HOME/config and `.cargo` dirs above the root.
set -euo pipefail

ALLOWED_REPO="https://github.com/lune-rs/luneui-rs"
ALLOWED_REV="cebd7d7572ae7f0642cb7c7505fcfa467e0456fc"

mode="check"
case "${1:-}" in
  "") ;;
  --list) mode="list" ;;
  *) echo "usage: check-deps.sh [--list]" >&2; exit 2 ;;
esac

SELF="${BASH_SOURCE[0]}"
case "$SELF" in */*) ;; *) SELF="./$SELF" ;; esac
ROOT="$(cd "${SELF%/*}/.." && pwd)"
cd "$ROOT"

for tool in cargo python3; do
  if ! command -v "$tool" > /dev/null 2>&1; then
    echo "check-deps: required tool not found: $tool" >&2
    exit 2
  fi
done

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if ! cargo metadata --format-version 1 --locked > "$WORK/meta.json" 2> "$WORK/meta.err"; then
  echo "check-deps: cargo metadata --locked failed: $(tail -n 1 "$WORK/meta.err")" >&2
  exit 2
fi

python3 -I - "$mode" "$ROOT" "$ALLOWED_REPO" "$ALLOWED_REV" "$WORK/meta.json" << 'PY'
import json
import os
import re
import sys

mode, root, repo, rev, meta_path = sys.argv[1:6]
root = os.path.realpath(root)

try:
    import tomllib
except ImportError:
    print("check-deps: python3 has no tomllib (needs 3.11+)", file=sys.stderr)
    sys.exit(2)

with open(meta_path, encoding="utf-8") as f:
    meta = json.load(f)
packages = meta.get("packages") or []
members = set(meta.get("workspace_members") or [])
if not packages or not members:
    print("check-deps: cargo metadata listed no packages or no workspace members", file=sys.stderr)
    sys.exit(2)

CRATES_IO = {
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
}
GIT_BASE = "git+" + repo + "?rev=" + rev

if mode == "list":
    rows = sorted(
        (p["name"], p["version"], p.get("source") or "path")
        for p in packages
        if p["id"] not in members
    )
    for name, version, source in rows:
        print(name, version, source)
    sys.exit(0)

bad = []

def under_root(path):
    real = os.path.realpath(path)
    return real == root or real.startswith(root + os.sep)

# 1. Resolved packages.
for p in packages:
    who = "%s %s" % (p["name"], p["version"])
    src = p.get("source")
    if src is None:
        if not under_root(p["manifest_path"]):
            bad.append("%s: path package outside the repo: %s" % (who, p["manifest_path"]))
    elif src in CRATES_IO:
        pass
    elif src.startswith("git+"):
        base, _, frag = src.partition("#")
        if base == GIT_BASE and frag == rev:
            pass
        elif base.startswith("git+" + repo):
            bad.append("%s: luneui-rs resolves to another rev: %s" % (who, src))
        else:
            bad.append("%s: git source not allowed: %s" % (who, src))
    else:
        bad.append("%s: source not allowed: %s" % (who, src))

# 2. Declared dependencies (any package): git only as the exact pin, registry only crates.io.
seen = set()
for p in packages:
    for d in p.get("dependencies") or []:
        src = d.get("source")
        key = (p["name"], d["name"], src)
        if key in seen:
            continue
        seen.add(key)
        where = "%s declares %s" % (p["name"], d["name"])
        if d.get("registry"):
            bad.append("%s from registry %s" % (where, d["registry"]))
        if src is None or src in CRATES_IO:
            continue
        if src.startswith("git+"):
            # Deps between packages of one git checkout carry the resolved `#<sha>` too.
            if src == GIT_BASE or src == GIT_BASE + "#" + rev:
                continue
            m = re.search(r"[?&](branch|tag|rev)=([^&#]*)", src)
            if not src.startswith("git+" + repo):
                bad.append("%s from another git repo: %s" % (where, src))
            elif m is None or m.group(1) != "rev":
                bad.append("%s without a rev (branch, tag or default branch): %s" % (where, src))
            elif not re.fullmatch(r"[0-9a-f]{40}", m.group(2)):
                bad.append("%s with a rev that is not 40 hex digits: %s" % (where, src))
            else:
                bad.append("%s at another rev: %s" % (where, src))
        else:
            bad.append("%s from a source not allowed: %s" % (where, src))

# 3. [patch] / [replace] in the root and member manifests.
manifests = {os.path.join(root, "Cargo.toml")}
for p in packages:
    if p["id"] in members:
        manifests.add(p["manifest_path"])
for path in sorted(manifests):
    rel = os.path.relpath(path, root)
    try:
        with open(path, "rb") as f:
            doc = tomllib.load(f)
    except (OSError, tomllib.TOMLDecodeError) as e:
        bad.append("%s: cannot parse manifest: %s" % (rel, e))
        continue
    for key in ("patch", "replace"):
        if key in doc:
            bad.append("%s: has a [%s] table" % (rel, key))

# 4. Repo cargo config files: no source replacement, no patch, no path overrides.
SKIP = {"target", ".git", "node_modules", "reference"}
for dirpath, dirnames, filenames in os.walk(root):
    dirnames[:] = [d for d in dirnames if d not in SKIP]
    if os.path.basename(dirpath) != ".cargo":
        continue
    for name in filenames:
        if not name.startswith("config"):
            continue
        path = os.path.join(dirpath, name)
        rel = os.path.relpath(path, root)
        try:
            with open(path, "rb") as f:
                doc = tomllib.load(f)
        except (OSError, tomllib.TOMLDecodeError) as e:
            bad.append("%s: cannot parse cargo config: %s" % (rel, e))
            continue
        for key in ("patch", "source", "paths"):
            if key in doc:
                bad.append("%s: has %s" % (rel, "paths" if key == "paths" else "[%s]" % key))

if bad:
    for line in bad:
        print("check-deps: " + line)
    print("check-deps: FAILED (%d violation%s)" % (len(bad), "" if len(bad) == 1 else "s"), file=sys.stderr)
    sys.exit(1)
external = sum(1 for p in packages if p["id"] not in members)
print("check-deps: clean (%d external packages)" % external)
PY
