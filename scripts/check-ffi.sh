#!/usr/bin/env bash
# Export check for the luneterm C ABI (M8 D8). Derives both sides from tools, never from a regex
# over source text:
#   exported = the defined global symbols `nm` lists in the built cdylib
#   declared = the FunctionDecl names `clang -Xclang -ast-dump=json` finds in include/luneterm.h
# and requires them to be equal, for the shipping build and for the panic-probe build, in both
# directions. It fails closed: a missing tool, an empty export list or an empty declaration list
# is an error (a wrong path or pattern would otherwise read as "nothing exported, nothing
# declared, equal"). Then it compiles tests/c/smoke.c against each build and runs it.
#
# Builds use `cargo build -p luneterm-ffi --profile ffi-release` (D4); the library paths come from
# cargo's own `--message-format=json` artifact list. The probe build runs first, so the
# shipping library is what is left in target/ffi-release.
#
# LUNETERM_FFI_HEADER=<path> checks another header (used to prove the script on a known-bad case).
# Prints names and sizes only. Exit 0 = clean.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
HEADER="${LUNETERM_FFI_HEADER:-crates/luneterm-ffi/include/luneterm.h}"
SMOKE="crates/luneterm-ffi/tests/c/smoke.c"
INCLUDE_DIR="$(dirname "$HEADER")"

missing=0
for tool in clang nm python3 cargo; do
  if ! command -v "$tool" > /dev/null 2>&1; then
    echo "check-ffi: required tool not found on PATH: $tool" >&2
    missing=1
  fi
done
[ "$missing" -eq 0 ] || exit 2
[ -f "$HEADER" ] || { echo "check-ffi: header not found: $HEADER" >&2; exit 2; }
[ -f "$SMOKE" ] || { echo "check-ffi: smoke test not found: $SMOKE" >&2; exit 2; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
fail=0
report() { echo "check-ffi: FAIL: $*" >&2; fail=1; }

# declared <probe: 0|1>: the function names declared by the header, one per line, sorted.
declared() {
  local defs=()
  if [ "$1" = 1 ]; then defs=(-DLUNETERM_PANIC_PROBE); fi
  printf '#include "%s"\n' "$(basename "$HEADER")" > "$WORK/tu.c"
  clang -std=c11 -Xclang -ast-dump=json -fsyntax-only ${defs[@]+"${defs[@]}"} \
    -I "$INCLUDE_DIR" "$WORK/tu.c" > "$WORK/ast.json"
  python3 -I - "$WORK/ast.json" << 'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as f:
    tu = json.load(f)
if tu.get("kind") != "TranslationUnitDecl":
    sys.exit("not a translation unit")
names, foreign = set(), set()
for node in tu.get("inner", []):
    if node.get("kind") == "FunctionDecl" and not node.get("isImplicit", False):
        name = node.get("name", "")
        # The TU includes nothing but the header (and its standard headers): any function whose
        # name is not luneterm_* is a stray declaration, reported instead of ignored.
        (names if name.startswith("luneterm_") else foreign).add(name)
if foreign:
    sys.exit("declarations outside the luneterm_ namespace: " + " ".join(sorted(foreign)))
print("\n".join(sorted(names)))
PY
}

# exported <library>: the defined global symbols of the library, one per line, sorted. Anything
# that does not start with luneterm_ is reported by the caller.
exported() {
  local raw
  case "$(uname -s)" in
    Darwin) raw="$(nm -gU "$1")" ;;
    *) raw="$(nm -D --defined-only "$1")" ;;
  esac
  printf '%s\n' "$raw" | python3 -I -c '
import sys
darwin = sys.argv[1] == "Darwin"
names = set()
for line in sys.stdin:
    parts = line.split()
    if len(parts) < 3 or parts[-2].lower() == "u":
        continue
    name = parts[-1].split("@")[0]
    if darwin and name.startswith("_"):
        name = name[1:]
    names.add(name)
print("\n".join(sorted(names)))
' "$(uname -s)"
}

# build <label> <cargo args...>: builds and prints the cdylib path taken from cargo's artifact list.
build() {
  local label="$1"
  shift
  echo "check-ffi: building ($label)" >&2
  cargo build -p luneterm-ffi --profile ffi-release --message-format=json "$@" > "$WORK/build.json"
  python3 -I - "$WORK/build.json" << 'PY'
import json, sys
found = []
with open(sys.argv[1], encoding="utf-8") as f:
    for line in f:
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("reason") != "compiler-artifact":
            continue
        target = msg.get("target", {})
        if target.get("name") not in ("luneterm_ffi", "luneterm-ffi") or "cdylib" not in target.get("crate_types", []):
            continue
        found += [p for p in msg.get("filenames", []) if p.endswith((".dylib", ".so", ".dll"))]
if len(set(found)) != 1:
    sys.exit("expected exactly one luneterm_ffi cdylib artifact, got: %s" % sorted(set(found)))
print(found[0])
PY
}

# check_build <label> <probe: 0|1> <library>: header == exports, and the probe symbol rule.
check_build() {
  local label="$1" probe="$2" lib="$3" decl exp extra
  decl="$(declared "$probe")"
  exp="$(exported "$lib")"
  if [ -z "$exp" ]; then report "($label) the library exports no symbols ($lib): wrong file or nm output"; return; fi
  if [ -z "$decl" ]; then report "($label) the header declares no functions"; return; fi
  extra="$(printf '%s\n' "$exp" | grep -v '^luneterm_' || true)"
  if [ -n "$extra" ]; then report "($label) exports outside the luneterm_ namespace: $(echo $extra)"; fi
  extra="$(comm -23 <(printf '%s\n' "$decl") <(printf '%s\n' "$exp"))"
  if [ -n "$extra" ]; then report "($label) declared in the header but not exported: $(echo $extra)"; fi
  extra="$(comm -13 <(printf '%s\n' "$decl") <(printf '%s\n' "$exp"))"
  if [ -n "$extra" ]; then report "($label) exported but not declared in the header: $(echo $extra)"; fi
  if printf '%s\n' "$exp" | grep -qx 'luneterm_test_panic'; then
    [ "$probe" = 1 ] || report "($label) the shipping library exports luneterm_test_panic"
  else
    [ "$probe" = 0 ] || report "($label) the probe library lacks luneterm_test_panic"
  fi
  echo "check-ffi: ($label) $(printf '%s\n' "$exp" | wc -l | tr -d ' ') exports, $(printf '%s\n' "$decl" | wc -l | tr -d ' ') declarations" >&2
}

# smoke <label> <define|-> <library>: compile smoke.c against the library and run it. (The dylib's
# install name is its build-directory path, so the loader finds this same file; the rpath covers
# the other platforms.)
smoke() {
  local label="$1" define="$2" lib="$3" dir="$WORK/smoke-$1" defs=()
  mkdir -p "$dir"
  if [ "$define" != "-" ]; then defs=("$define"); fi
  clang -std=c11 -Wall -Wextra -Werror -pedantic ${defs[@]+"${defs[@]}"} -I "$INCLUDE_DIR" "$SMOKE" \
    "$lib" "-Wl,-rpath,$(dirname "$lib")" -o "$dir/smoke" || { report "($label) smoke.c does not compile"; return; }
  "$dir/smoke" > "$dir/out.txt" 2>&1 || { report "($label) the C smoke test failed:"; cat "$dir/out.txt" >&2; return; }
  echo "check-ffi: ($label) C smoke test ok" >&2
}

# Probe build first, so the shipping library is the one left in place.
PROBE_LIB="$(build probe --features panic-probe)"
smoke probe -DLUNETERM_PANIC_PROBE "$PROBE_LIB"
check_build probe 1 "$PROBE_LIB"

SHIP_LIB="$(build shipping)"
smoke shipping - "$SHIP_LIB"
check_build shipping 0 "$SHIP_LIB"
echo "check-ffi: shipping library $(basename "$SHIP_LIB"): $(wc -c < "$SHIP_LIB" | tr -d ' ') bytes"

if [ "$fail" -ne 0 ]; then exit 1; fi
echo "check-ffi: ok"
