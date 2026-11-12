#!/usr/bin/env python3
"""Third-party licence bundle (plan M10 step 10, D11).

Writes licenses/THIRD_PARTY_CRATES.txt: every third-party package the shipped binaries are built
from, with its licence text. `--check` regenerates in memory and fails on anything that is not
exactly what is committed.

    python3 scripts/gen-licenses.py            write the bundle
    python3 scripts/gen-licenses.py --check    fail on a stale or incomplete bundle (CI runs this)

The package set comes only from `cargo metadata --locked --filter-platform <target>`, for four
targets (the union): the normal dependencies, transitively, of the roots `luneterm` and
`luneterm-ffi`. Workspace packages are the project's own (LICENSE-APACHE) and are not
listed. The list is not read from Cargo.lock or from a grep.

Each package's text is, in order:
  1. the licence files in its own directory (LICENSE*, LICENCE*, COPYING*, UNLICENSE*, COPYRIGHT*,
     NOTICE*, its `license-file`, and the files of a LICENSES directory);
  2. for a git package with none, the licence files of the nearest directory above it in the
     checkout (luneui-rs keeps them at its root);
  3. else the vendored SPDX text of each id in the package's own `license` expression
     (licenses/spdx/<id>.txt).
Failures (exit 1, one line each): a package with no text; an SPDX id in any expression with no
vendored text (an unknown id); an expression that does not parse; a licence file that is not UTF-8;
a bundle that differs from what is generated; a static copy (project, luneui-rs, fonts) that differs
from its source. luneui-rs's own `licenses/` directory is copied file by
file as licenses/luneui-rs-<name>, and a `licenses/<file>` its notices file names that the directory
lacks fails (the glyph renderer it vendors has its text there). An `OR` alternative that is only copyleft (GPL, LGPL, AGPL) is not elected when a
permissive alternative is offered, so its text need not be vendored; an `AND` copyleft id is.
MPL-2.0 packages carry a source pointer. Output is sorted and identical texts appear once.

Exit 0 clean, 1 violations, 2 cannot run (no cargo, metadata error).
Test hooks (the negative checks of the plan): `--file PATH` (the bundle to write or compare),
`--spdx-dir DIR` (the vendored texts), `--metadata-file F` (repeatable: a saved `cargo metadata`
output instead of running cargo).
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

TARGETS = [
    "x86_64-pc-windows-msvc",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
]
ROOTS = ("luneterm", "luneterm-ffi")

BUNDLE = os.path.join(ROOT, "licenses", "THIRD_PARTY_CRATES.txt")
SPDX_DIR = os.path.join(ROOT, "licenses", "spdx")

LICENSE_NAME = re.compile(r"(?i)^(licen[cs]e|copying|unlicense|copyright|notice)([-_. ].*)?$")
LICENSE_DIRS = ("LICENSES", "licenses", "LICENCES")
COPYLEFT = re.compile(r"^(A|L)?GPL-")


class Violations(Exception):
    pass


# ----------------------------------------------------------------------------- SPDX expressions


def parse_expression(text):
    """The tree of an SPDX licence expression: ('id', s), ('with', id, exc), ('and', [..]),
    ('or', [..]). A legacy `A/B` means `A OR B`. Raises ValueError on anything else."""
    tokens = re.findall(r"\(|\)|[^\s()]+", text.replace("/", " OR "))
    pos = 0

    def peek():
        return tokens[pos] if pos < len(tokens) else None

    def take():
        nonlocal pos
        tok = peek()
        pos += 1
        return tok

    def ident():
        tok = take()
        if tok is None or tok in ("AND", "OR", "WITH", "(", ")"):
            raise ValueError("expected a licence id")
        return tok

    def atom():
        if peek() == "(":
            take()
            node = expr()
            if take() != ")":
                raise ValueError("missing )")
            return node
        name = ident()
        if peek() == "WITH":
            take()
            return ("with", name, ident())
        return ("id", name)

    def conj():
        nodes = [atom()]
        while peek() == "AND":
            take()
            nodes.append(atom())
        return nodes[0] if len(nodes) == 1 else ("and", nodes)

    def expr():
        nodes = [conj()]
        while peek() == "OR":
            take()
            nodes.append(conj())
        return nodes[0] if len(nodes) == 1 else ("or", nodes)

    if not tokens:
        raise ValueError("empty expression")
    node = expr()
    if pos != len(tokens):
        raise ValueError("trailing text")
    return node


def required_ids(node):
    """The ids whose text must exist for `node`: all of them, except that an `OR` alternative made
    only of copyleft ids is dropped while a permissive one remains."""
    kind = node[0]
    if kind == "id":
        return {node[1]}
    if kind == "with":
        return {node[1], node[2]}
    sets = [required_ids(n) for n in node[1]]
    if kind == "or":
        kept = [s for s in sets if not all(COPYLEFT.match(i) for i in s)]
        sets = kept or sets
    out = set()
    for s in sets:
        out |= s
    return out


# --------------------------------------------------------------------------------- the metadata


def load_metadata(files):
    docs = []
    if files:
        for f in files:
            with open(f, encoding="utf-8") as fh:
                docs.append(json.load(fh))
        return docs
    for target in TARGETS:
        cmd = ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target]
        try:
            run = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, check=False)
        except FileNotFoundError:
            print("gen-licenses: cargo not found", file=sys.stderr)
            sys.exit(2)
        if run.returncode != 0:
            tail = (run.stderr.strip().splitlines() or ["?"])[-1]
            print("gen-licenses: cargo metadata failed for %s: %s" % (target, tail), file=sys.stderr)
            sys.exit(2)
        docs.append(json.loads(run.stdout))
    return docs


def third_party(docs):
    """{(name, version, source): package} of the third-party packages in the union of `docs`."""
    out = {}
    for meta in docs:
        by_id = {p["id"]: p for p in meta["packages"]}
        members = set(meta["workspace_members"])
        nodes = {n["id"]: n for n in (meta.get("resolve") or {}).get("nodes", [])}
        if not nodes:
            print("gen-licenses: the metadata has no resolve graph", file=sys.stderr)
            sys.exit(2)
        start = [i for i in members if by_id[i]["name"] in ROOTS]
        if len(start) != len(ROOTS):
            print("gen-licenses: the roots %s are not all workspace members" % (ROOTS,), file=sys.stderr)
            sys.exit(2)
        seen = set(start)
        todo = list(start)
        while todo:
            for dep in nodes[todo.pop()]["deps"]:
                # Normal dependencies only (kind null): dev and build dependencies are not shipped.
                if any(k["kind"] is None for k in dep["dep_kinds"]) and dep["pkg"] not in seen:
                    seen.add(dep["pkg"])
                    todo.append(dep["pkg"])
        for pid in seen:
            if pid in members:
                continue
            p = by_id[pid]
            out[(p["name"], p["version"], p.get("source") or "")] = p
    return out


# ---------------------------------------------------------------------------------- licence texts


def normalise(text):
    lines = [line.rstrip() for line in text.replace("\r\n", "\n").replace("\r", "\n").split("\n")]
    return "\n".join(lines).strip("\n") + "\n"


def read_text(path, bad):
    with open(path, "rb") as f:
        raw = f.read()
    try:
        return normalise(raw.decode("utf-8"))
    except UnicodeDecodeError:
        bad.append("licence file is not UTF-8: %s" % path)
        return None


def licence_files(directory):
    found = []
    try:
        entries = sorted(os.listdir(directory))
    except OSError:
        return found
    for name in entries:
        full = os.path.join(directory, name)
        if os.path.isfile(full) and LICENSE_NAME.match(name):
            found.append((name, full))
        elif os.path.isdir(full) and name in LICENSE_DIRS:
            for sub in sorted(os.listdir(full)):
                inner = os.path.join(full, sub)
                if os.path.isfile(inner):
                    found.append((name + "/" + sub, inner))
    return found


def checkout_root_of(directory):
    """For a package in a cargo git checkout: the directories from it up to the checkout's root
    (`.../checkouts/<name>/<rev>`)."""
    chain = []
    cur = directory
    while True:
        chain.append(cur)
        parent = os.path.dirname(cur)
        if parent == cur or os.path.basename(os.path.dirname(parent)) == "checkouts":
            if os.path.basename(os.path.dirname(parent)) == "checkouts":
                chain.append(parent)
            return chain
        cur = parent


def package_texts(p, spdx, bad):
    """([(label, text)], vendored: bool) for one package, appending violations to `bad`."""
    who = "%s %s" % (p["name"], p["version"])
    directory = os.path.dirname(p["manifest_path"])
    expr = p.get("license")

    ids = set()
    if expr:
        try:
            ids = required_ids(parse_expression(expr))
        except ValueError as e:
            bad.append("%s: cannot parse the licence expression %r: %s" % (who, expr, e))
    for i in sorted(ids):
        if not os.path.isfile(os.path.join(spdx, i + ".txt")):
            bad.append("%s: SPDX id %s has no vendored text in licenses/spdx" % (who, i))

    files = licence_files(directory)
    lf = p.get("license_file")
    if lf:
        full = os.path.normpath(os.path.join(directory, lf))
        if os.path.isfile(full) and full not in [f for _, f in files]:
            files.append((os.path.relpath(full, directory), full))
    if not files and (p.get("source") or "").startswith("git+"):
        for above in checkout_root_of(directory)[1:]:
            files = licence_files(above)
            if files:
                break
    texts = []
    for label, full in files:
        t = read_text(full, bad)
        if t is not None:
            texts.append((label, t))
    if texts:
        return texts, False
    if ids:
        vendored = []
        for i in sorted(ids):
            path = os.path.join(spdx, i + ".txt")
            if os.path.isfile(path):
                t = read_text(path, bad)
                if t is not None:
                    vendored.append((i + " (SPDX)", t))
        if len(vendored) == len(ids):
            return vendored, True
        return [], True
    bad.append("%s: no licence text (no licence file, no usable licence expression)" % who)
    return [], False


def source_label(p):
    src = p.get("source") or ""
    if src.startswith("git+"):
        base, _, frag = src[4:].partition("#")
        return "git %s @ %s" % (base.split("?")[0], frag[:8])
    if src.startswith(("registry+", "sparse+")):
        return "crates.io"
    return src or "path"


# ------------------------------------------------------------------------------------ the bundle


def build(docs, spdx):
    bad = []
    packages = third_party(docs)
    rows = []
    texts = {}  # digest -> (text, [package labels])
    for key in sorted(packages):
        p = packages[key]
        got, vendored = package_texts(p, spdx, bad)
        who = "%s %s" % (p["name"], p["version"])
        digests = []
        for label, text in got:
            d = hashlib.sha256(text.encode("utf-8")).hexdigest()[:12]
            entry = texts.setdefault(d, (text, []))
            if who not in entry[1]:
                entry[1].append(who)
            if d not in digests:
                digests.append(d)
        extra = ""
        if p.get("license") and "MPL-2.0" in p["license"]:
            repo = p.get("repository") or "(none declared)"
            extra = "\n    MPL-2.0 source: https://crates.io/crates/%s/%s  repository: %s" % (
                p["name"],
                p["version"],
                repo,
            )
        rows.append(
            "%s | %s | %s | %s: %s%s"
            % (
                who,
                p.get("license") or "(licence file)",
                source_label(p),
                "SPDX text" if vendored else "texts",
                " ".join(digests) or "none",
                extra,
            )
        )
    out = []
    out.append("THIRD-PARTY CRATE LICENCES")
    out.append("")
    out.append("Generated by scripts/gen-licenses.py from `cargo metadata --locked`; do not edit.")
    out.append("Check with: python3 scripts/gen-licenses.py --check")
    out.append("Roots: %s (normal dependencies, transitively)" % ", ".join(ROOTS))
    out.append("Targets: %s" % ", ".join(TARGETS))
    out.append("Third-party packages: %d" % len(packages))
    out.append("The project's own packages are not listed: see LICENSE-APACHE.")
    out.append("")
    out.append("PACKAGES")
    out.append("name version | licence expression | source | licence texts (ids below)")
    out.append("")
    out.extend(rows)
    out.append("")
    out.append("LICENCE TEXTS")
    for d in sorted(texts):
        text, users = texts[d]
        out.append("")
        out.append("=" * 78)
        out.append("[%s] used by %d package(s): %s" % (d, len(users), ", ".join(sorted(users))))
        out.append("=" * 78)
        out.append(text.rstrip("\n"))
    return "\n".join(out) + "\n", bad, packages


def static_copies(packages):
    """[(name in licenses/, source path or None)]: the project's own licences, luneui-rs's, the
    bundled fonts'."""
    luneui = None
    for (name, _v, src), p in packages.items():
        if name == "lune-ui" and src.startswith("git+"):
            for above in checkout_root_of(os.path.dirname(p["manifest_path"])):
                if os.path.isfile(os.path.join(above, "LICENSE-MIT")):
                    luneui = above
                    break
    assets = os.path.join(ROOT, "crates", "luneterm", "assets")
    rows = [
        ("luneterm-LICENSE-APACHE", os.path.join(ROOT, "LICENSE-APACHE")),
        ("RobotoMonoNerdFont.LICENSE.txt", os.path.join(assets, "RobotoMonoNerdFont.LICENSE.txt")),
        ("RznSans.OFL.txt", os.path.join(assets, "RznSans.OFL.txt")),
        ("RznMono.OFL.txt", os.path.join(assets, "RznMono.OFL.txt")),
    ]
    for name in ("LICENSE-MIT", "LICENSE-APACHE", "THIRD_PARTY_NOTICES.md"):
        rows.append(("luneui-rs-" + name, os.path.join(luneui, name) if luneui else None))
    # Every text luneui-rs bundles itself (its notices file points into this directory: the text of
    # the glyph renderer it vendors, the fonts' and so on) travels with it, whatever a later pin adds.
    if luneui:
        sub = os.path.join(luneui, "licenses")
        for name in sorted(os.listdir(sub)) if os.path.isdir(sub) else []:
            if os.path.isfile(os.path.join(sub, name)):
                rows.append(("luneui-rs-" + name, os.path.join(sub, name)))
    return rows


def notices_problems(statics):
    """Fail closed: luneui-rs's notices file names `licenses/<file>` texts; each must be one of the
    files copied from its `licenses/` directory."""
    bad = []
    notices = None
    for name, src in statics:
        if name == "luneui-rs-THIRD_PARTY_NOTICES.md" and src and os.path.isfile(src):
            notices = src
    if notices is None:
        return bad
    with open(notices, encoding="utf-8") as f:
        named = set(re.findall(r"`licenses/([A-Za-z0-9._-]+)`", f.read()))
    copied = {n[len("luneui-rs-") :] for n, _ in statics}
    for name in sorted(named - copied):
        bad.append("luneui-rs's notices name licenses/%s but its licenses/ directory has no such file" % name)
    return bad


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--file", default=BUNDLE)
    ap.add_argument("--spdx-dir", default=SPDX_DIR)
    ap.add_argument("--metadata-file", action="append", default=[])
    args = ap.parse_args()

    docs = load_metadata(args.metadata_file)
    text, bad, packages = build(docs, args.spdx_dir)
    licdir = os.path.join(ROOT, "licenses")
    statics = static_copies(packages)
    bad.extend(notices_problems(statics))
    for name, src in statics:
        if src is None or not os.path.isfile(src):
            bad.append("static licence source missing for licenses/%s" % name)

    if args.check:
        try:
            with open(args.file, encoding="utf-8") as f:
                have = f.read()
        except OSError:
            have = None
            bad.append("%s is missing" % os.path.relpath(args.file, ROOT))
        if have is not None and have != text:
            old, new = have.split("\n"), text.split("\n")
            line = next((i + 1 for i, (a, b) in enumerate(zip(old, new)) if a != b), min(len(old), len(new)) + 1)
            bad.append(
                "%s is stale (first difference at line %d): run scripts/gen-licenses.py"
                % (os.path.relpath(args.file, ROOT), line)
            )
        for name, src in statics:
            dst = os.path.join(licdir, name)
            if src is None or not os.path.isfile(src):
                continue
            if not os.path.isfile(dst):
                bad.append("licenses/%s is missing" % name)
            else:
                with open(src, "rb") as a, open(dst, "rb") as b:
                    if a.read() != b.read():
                        bad.append("licenses/%s differs from its source: run scripts/gen-licenses.py" % name)
    if bad:
        for line in sorted(set(bad)):
            print("gen-licenses: " + line, file=sys.stderr)
        sys.exit(1)
    if args.check:
        print("gen-licenses: clean (%d third-party packages)" % len(packages))
        return
    with open(args.file, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    for name, src in statics:
        with open(src, "rb") as a, open(os.path.join(licdir, name), "wb") as b:
            b.write(a.read())
    print("gen-licenses: wrote %s (%d third-party packages)" % (os.path.relpath(args.file, ROOT), len(packages)))


if __name__ == "__main__":
    main()
