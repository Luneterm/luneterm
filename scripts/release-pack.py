#!/usr/bin/env python3
"""Luneterm's own part of a release: pack the build inputs, check the root, attach the setup files.

Run by .github/workflows/publish-updates.yml. Standard library only. Everything that publishes to
the update store is `lunepack publish`; this file holds only what is Luneterm's own.

    release-pack.py check-root --config <toml>
    release-pack.py pack --tag <tag> --lunepack <cli> --config <toml> --work <dir>
        [--inputs-dir <dir> | (the GitHub release of <tag>)] [--output-file <path>]
    release-pack.py attach --tag <tag> --dir <setup dir> --expect <target>=<sha256> ...

`check-root` exits 1 while `[root] keys` of the config holds the throwaway test root (its private
half was discarded): no release may ship with it.

`pack` runs on the pack job (environment release-pack). It checks the root, takes one build input
per target from the GitHub release of the tag (scripts/make-build-input.py; or --inputs-dir), checks
each against its `.sha256`, extracts it without trusting it, and checks its BUILD-INFO.txt (target,
version, channel = beta for a release candidate, the lunepack rev of Cargo.toml, one
`source_date_epoch` for all of them, equal to the time of the tag's commit). Then, with the pinned
`lunepack` CLI: `publish fetch` into <work>/release, per target `pack` (`--channel beta` for a
release candidate) and `setup`. The setup files go to <work>/setup with their `.sha256`, and one
`setup_sha256_<target>=<hex>` line per target is printed (and appended to --output-file, the job's
$GITHUB_OUTPUT, only after the digest is checked to be 64 lowercase hex characters).

`attach` runs on the attach job (GITHUB_TOKEN only): it checks every setup file against the digest
the pack job reported, uploads what the release lacks (an asset of the same name and another size is
exit 1), then deletes the release's `-build.` inputs.

Secrets come from the environment and are removed from it at start: LUNEPACK_RELEASE_KEY reaches
only the lunepack children that sign (`pack`), GITHUB_TOKEN only the GitHub API calls. No value of
one is ever printed. This is hygiene, not a boundary: processes of one user can read each other's
environment, and removing a variable from Python's `os.environ` does not erase it there. The
boundary is the job split and the environments of publish-updates.yml, and that these jobs run no
third-party code: the pinned lunepack CLI and the Python standard library only.
"""

import argparse
import base64
import hashlib
import importlib.util
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tomllib
import urllib.error
import urllib.request
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)

# The throwaway test root of packaging/lunepack.toml (its private half was discarded).
TEST_ROOT_KEYS = frozenset({"7c4f44f54daf49ca873f3aac937f0f19e668364a01178aa339f64690f998e996"})

SECRET_NAMES = ("LUNEPACK_RELEASE_KEY", "GITHUB_TOKEN")
# Never handed to a child: store and cloud credentials no child here has any use for (the push and
# refresh jobs hold the store tokens; these jobs never do), by name prefix.
DROPPED_PREFIXES = ("R2_", "CLOUDFLARE_", "CF_", "AWS_")


def _dropped(name):
    return name in SECRET_NAMES or name.startswith(DROPPED_PREFIXES)


CONFIG = "packaging/lunepack.toml"

# Per target: the asset names, and what BUILD-INFO.txt must say.
TARGETS = {
    "linux-x86_64": {
        "os": "linux",
        "input": "luneterm-{v}-linux-x64-build.tar.gz",
        "setup": "luneterm-{v}-linux-x64-setup",
        "notices": "lunepack-NOTICES.txt",
        "info": {
            "target": "linux-x86_64",
            "config": CONFIG,
            "entry": "luneterm",
            "tree": "tree",
            "runtime": "lunepack/lunepack-runtime",
            "ui": "lunepack/lunepack-ui",
        },
        "optional": ("glibc_min",),
    },
    "windows-x86_64": {
        "os": "windows",
        "input": "luneterm-{v}-windows-x64-build.zip",
        "setup": "luneterm-{v}-windows-x64-setup.exe",
        "notices": "lunepack-NOTICES.txt",
        "info": {
            "target": "windows-x86_64",
            "config": CONFIG,
            "entry": "luneterm.exe",
            "tree": "tree",
            "runtime": "lunepack/lunepack-runtime.exe",
            "stub": "lunepack/lunepack-stub.exe",
            "ui": "lunepack/lunepack-ui.exe",
        },
        "optional": (),
    },
    "macos-aarch64": {
        "os": "macos",
        "input": "luneterm-{v}-macos-arm64-build.tar.gz",
        "setup": "luneterm-{v}-macos-arm64-setup.zip",
        "notices": "Contents/Resources/lunepack-NOTICES.txt",
        "info": {
            "target": "macos-aarch64",
            "config": CONFIG,
            "entry": "Contents/MacOS/luneterm",
            "tree": "tree",
            "runtime": "tree/Contents/MacOS/lunepack-runtime",
            "ui": "tree/Contents/Helpers/lunepack-update.app/Contents/MacOS/lunepack-ui",
        },
        "optional": (),
    },
}

# Safe extraction caps (P1).
MAX_INPUT_BYTES = 400 * 1024 * 1024
MAX_ENTRIES = 20000
MAX_FILE_BYTES = 512 * 1024 * 1024
MAX_TOTAL_BYTES = 1024 * 1024 * 1024

TAG_RE = re.compile(r"v([0-9]+\.[0-9]+\.[0-9]+(?:-rc\.[0-9]+)?)")
SHA256_RE = re.compile(r"[0-9a-f]{64}")


class Abort(Exception):
    """A reason to stop. Printed without any secret."""


def channel_of(version):
    """A release candidate is listed in the beta feed only and packs as beta; any other release is stable."""
    return "beta" if "-rc." in version else "stable"


# ------------------------------------------------------------------------------ the root of trust


def check_root(config_path):
    """No release may ship with the throwaway test root."""
    try:
        with open(config_path, "rb") as f:
            keys = tomllib.load(f)["root"]["keys"]
    except (OSError, KeyError, tomllib.TOMLDecodeError) as err:
        raise Abort("cannot read [root] keys of %s: %s" % (config_path, type(err).__name__)) from err
    if not isinstance(keys, list) or not keys or not all(isinstance(k, str) for k in keys):
        raise Abort("%s: [root] keys is not a list of keys" % config_path)
    if set(keys) & TEST_ROOT_KEYS:
        raise Abort("%s names the test root key; refusing to publish" % config_path)


# ------------------------------------------------------------------------------- secrets, children


class Secrets:
    """Secrets taken out of the environment, so no child inherits one by accident."""

    def __init__(self, environ):
        self.values = {}
        for name in SECRET_NAMES:
            value = environ.pop(name, None)
            if value:
                self.values[name] = value
        for name in [n for n in environ if _dropped(n)]:
            environ.pop(name, None)

    def get(self, name):
        value = self.values.get(name)
        if not value:
            raise Abort("%s is not set" % name)
        return value

    def redact(self, text):
        for value in self.values.values():
            if value:
                text = text.replace(value, "***")
        return text


def child_env(extra=None):
    env = {k: v for k, v in os.environ.items() if not _dropped(k)}
    env.update(extra or {})
    return env


class Lunepack:
    def __init__(self, cli, secrets, config):
        self.cli = cli
        self.secrets = secrets
        self.config = config

    def run(self, args, epoch=None, sign=False):
        """One lunepack child. Only a signing one gets the release key; only pack and setup get
        SOURCE_DATE_EPOCH (it fixes the time documents are issued at)."""
        extra = {}
        if sign:
            extra["LUNEPACK_RELEASE_KEY"] = self.secrets.get("LUNEPACK_RELEASE_KEY")
        if epoch is not None:
            extra["SOURCE_DATE_EPOCH"] = str(epoch)
        print("+ lunepack " + " ".join(args), flush=True)
        run = subprocess.run([self.cli] + args, env=child_env(extra), capture_output=True, text=True)
        out = self.secrets.redact(run.stdout + run.stderr)
        sys.stdout.write(out)
        sys.stdout.flush()
        if run.returncode != 0:
            raise Abort("lunepack %s failed (exit %d)" % (" ".join(args[:2] if args[0] == "publish" else args[:1]), run.returncode))
        return out


# ----------------------------------------------------------------------------------- build inputs


def safe_member_name(name):
    if not name or name.startswith("/") or "\\" in name or ":" in name or "\x00" in name:
        raise Abort("unsafe path in a build input: %r" % name)
    parts = name.rstrip("/").split("/")
    if any(p in ("", ".", "..") for p in parts):
        raise Abort("unsafe path in a build input: %r" % name)
    return parts


def _copy_capped(src, dest_path, size, mode, budget):
    if size > MAX_FILE_BYTES or size > budget[0]:
        raise Abort("a build input entry is too large")
    os.makedirs(os.path.dirname(dest_path), exist_ok=True)
    written = 0
    with open(dest_path, "xb") as out:
        while True:
            chunk = src.read(1 << 20)
            if not chunk:
                break
            written += len(chunk)
            if written > size:
                raise Abort("a build input entry is longer than it says")
            out.write(chunk)
    if written != size:
        raise Abort("a build input entry is shorter than it says")
    budget[0] -= written
    os.chmod(dest_path, mode)


def extract_input(archive, dest):
    """P1: extract a build input without trusting it: only regular files and directories, no
    absolute path, `..`, backslash, link or device, at most MAX_ENTRIES entries and
    MAX_TOTAL_BYTES; the exec bit is the only mode bit kept. `dest` must not exist."""
    os.makedirs(dest)
    root = os.path.realpath(dest)
    budget = [MAX_TOTAL_BYTES]
    seen = set()

    def target_of(name):
        parts = safe_member_name(name)
        key = "/".join(parts)
        if key in seen:
            raise Abort("a build input names %r twice" % name)
        seen.add(key)
        path = os.path.join(root, *parts)
        if not os.path.realpath(path).startswith(root + os.sep):
            raise Abort("unsafe path in a build input: %r" % name)
        return path

    if archive.endswith(".zip"):
        with zipfile.ZipFile(archive) as z:
            infos = z.infolist()
            if len(infos) > MAX_ENTRIES:
                raise Abort("a build input has too many entries")
            for info in infos:
                kind = stat.S_IFMT(info.external_attr >> 16)
                if kind not in (0, stat.S_IFREG, stat.S_IFDIR):
                    raise Abort("a build input holds a link or special file: %r" % info.filename)
                path = target_of(info.filename)
                if info.is_dir():
                    os.makedirs(path, exist_ok=True)
                    continue
                mode = 0o755 if (info.external_attr >> 16) & 0o111 else 0o644
                with z.open(info) as src:
                    _copy_capped(src, path, info.file_size, mode, budget)
    elif archive.endswith(".tar.gz"):
        with tarfile.open(archive, "r:gz") as tar:
            count = 0
            for member in tar:
                count += 1
                if count > MAX_ENTRIES:
                    raise Abort("a build input has too many entries")
                path = target_of(member.name)
                if member.isdir():
                    os.makedirs(path, exist_ok=True)
                elif member.isreg():
                    src = tar.extractfile(member)
                    mode = 0o755 if member.mode & 0o111 else 0o644
                    _copy_capped(src, path, member.size, mode, budget)
                else:
                    raise Abort("a build input holds a link or special file: %r" % member.name)
    else:
        raise Abort("unknown build input format: %s" % os.path.basename(archive))


def read_build_info(directory):
    path = os.path.join(directory, "BUILD-INFO.txt")
    if not os.path.isfile(path) or os.path.getsize(path) > 64 * 1024:
        raise Abort("the build input has no BUILD-INFO.txt")
    info = {}
    with open(path, encoding="utf-8") as f:
        for line in f.read().splitlines():
            if not line:
                continue
            key, sep, value = line.partition("=")
            if not sep or key in info:
                raise Abort("BUILD-INFO.txt has a bad or repeated line")
            info[key] = value
    return info


def check_build_info(info, target, version, channel, rev):
    """Fail closed on anything but exactly what the release jobs write for `target`."""
    spec = TARGETS[target]
    want = dict(spec["info"], format="1", version=version, channel=channel, lunepack_rev=rev)
    allowed = set(want) | {"source_date_epoch"} | set(spec["optional"])
    extra = sorted(set(info) - allowed)
    if extra:
        raise Abort("%s: BUILD-INFO.txt has unexpected keys %s" % (target, extra))
    for key, value in want.items():
        if info.get(key) != value:
            raise Abort("%s: BUILD-INFO.txt %s is %r, expected %r" % (target, key, info.get(key), value))
    epoch = info.get("source_date_epoch", "")
    if not epoch.isdigit() or len(epoch) > 15:
        raise Abort("%s: BUILD-INFO.txt has no usable source_date_epoch" % target)
    return int(epoch)




def check_sidecar(path, sidecar_bytes, name):
    with open(path, "rb") as f:
        digest = hashlib.sha256(f.read()).hexdigest()
    want = "%s  %s\n" % (digest, name)
    if sidecar_bytes.decode("ascii", "replace") != want:
        raise Abort("%s does not match its .sha256" % name)


def file_sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            chunk = f.read(1 << 20)
            if not chunk:
                return h.hexdigest()
            h.update(chunk)


def sidecar(path, name, digest):
    """Writes `<path>.sha256` for a file whose digest is already known."""
    out = path + ".sha256"
    with open(out, "w", encoding="ascii", newline="\n") as f:
        f.write("%s  %s\n" % (digest, name))
    return out


# ----------------------------------------------------------------------------------- repo checks


def read_pin():
    with open(os.path.join(REPO, "Cargo.toml"), "rb") as f:
        dep = tomllib.load(f)["workspace"]["dependencies"]["lunepack"]
    rev = dep.get("rev", "")
    if not re.fullmatch(r"[0-9a-f]{40}", rev):
        raise Abort("the lunepack dependency has no 40-hex rev")
    return rev


def tag_commit_time():
    """The commit time of the checkout (the job checks out the tag): the SOURCE_DATE_EPOCH every
    build input was built with."""
    run = subprocess.run(
        ["git", "-C", REPO, "log", "-1", "--format=%ct", "HEAD"], capture_output=True, text=True, env=child_env()
    )
    text = run.stdout.strip()
    if run.returncode != 0 or not text.isdigit():
        raise Abort("cannot read the commit time of HEAD")
    return int(text)


# ------------------------------------------------------------------------------- the GitHub release


def load_publish_release():
    spec = importlib.util.spec_from_file_location("publish_release", os.path.join(HERE, "publish-release.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Release:
    """The GitHub release of the tag (publish-release.py's client: the token goes to the API hosts
    only; an asset download follows GitHub's redirect without the token)."""

    def __init__(self, secrets, tag):
        self.pr = load_publish_release()
        repo = os.environ.get("GITHUB_REPOSITORY", "")
        if not self.pr.REPO_RE.fullmatch(repo):
            raise Abort("bad GITHUB_REPOSITORY: %r" % repo)
        self.repo = repo
        self.token = secrets.get("GITHUB_TOKEN")
        self.gh = self.pr.GitHub(repo, self.token, self.pr.http_send)
        self.fetch = asset_fetcher()
        try:
            self.release = self.gh.call("GET", self.pr.API, "releases/tags/" + tag)
        except self.pr.Failure as err:
            raise Abort(str(err)) from err
        if self.release.get("draft"):
            raise Abort("the release of %s is still a draft" % tag)

    def assets(self):
        out = {}
        page = 1
        while True:
            try:
                found = self.gh.call("GET", self.pr.API, "releases/%d/assets?per_page=100&page=%d" % (self.release["id"], page))
            except self.pr.Failure as err:
                raise Abort(str(err)) from err
            for asset in found:
                out[asset["name"]] = asset
            if len(found) < 100:
                return out
            page += 1

    def download(self, asset, dest):
        if asset.get("size", 0) > MAX_INPUT_BYTES:
            raise Abort("%s is larger than a build input may be" % asset["name"])
        url = "%s/repos/%s/releases/assets/%d" % (self.pr.API, self.repo, asset["id"])
        headers = {
            "Authorization": "Bearer " + self.token,
            "Accept": "application/octet-stream",
            "User-Agent": "luneterm-release-pack",
            "X-GitHub-Api-Version": "2022-11-28",
        }
        status, data = self.fetch(url, headers)
        if status != 200:
            raise Abort("download of %s: HTTP %d" % (asset["name"], status))
        if len(data) != asset.get("size") or len(data) > MAX_INPUT_BYTES:
            raise Abort("download of %s: wrong size" % asset["name"])
        with open(dest, "xb") as f:
            f.write(data)

    def upload(self, name, path):
        with open(path, "rb") as f:
            data = f.read()
        try:
            self.gh.upload(self.release["id"], name, data, "application/octet-stream")
        except self.pr.Failure as err:
            raise Abort(str(err)) from err

    def delete_asset(self, asset):
        try:
            self.gh.call("DELETE", self.pr.API, "releases/assets/%d" % asset["id"])
        except self.pr.Failure as err:
            raise Abort(str(err)) from err


def asset_fetcher():
    """GitHub answers an asset download with a redirect to its storage host: follow it without the
    Authorization header (urllib's default handler would resend it), https only."""

    class Handler(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, req, fp, code, msg, headers, newurl):
            if not newurl.startswith("https://"):
                return None
            return urllib.request.Request(newurl, headers={"User-Agent": "luneterm-release-pack"}, method="GET")

    def fetch(url, headers):
        if not url.startswith("https://api.github.com/"):
            raise Abort("refusing to send the token to %s" % url)
        request = urllib.request.Request(url, headers=headers, method="GET")
        opener = urllib.request.build_opener(Handler)
        try:
            with opener.open(request, timeout=600) as response:
                return response.status, response.read(MAX_INPUT_BYTES + 1)
        except urllib.error.HTTPError as err:
            return err.code, b""
        except OSError as err:
            raise Abort("GET %s: %r" % (url, err)) from err

    return fetch


# ------------------------------------------------------------------------------------- commands


def gather_inputs(inputs_dir, version, work, rev, release):
    """{target: (tree dir, info, epoch)} for every target: the build input of each, from the GitHub
    release or `inputs_dir`, checked against its .sha256, extracted and checked."""
    channel = channel_of(version)
    out = {}
    assets = release.assets() if release else None
    for target in TARGETS:
        name = TARGETS[target]["input"].format(v=version)
        archive = os.path.join(work, "downloads", name)
        os.makedirs(os.path.dirname(archive), exist_ok=True)
        if release:
            if name not in assets or name + ".sha256" not in assets:
                raise Abort("the release has no %s (or its .sha256)" % name)
            release.download(assets[name], archive)
            release.download(assets[name + ".sha256"], archive + ".sha256")
        else:
            src = os.path.join(inputs_dir, name)
            if not os.path.isfile(src) or not os.path.isfile(src + ".sha256"):
                raise Abort("%s has no %s (or its .sha256)" % (inputs_dir, name))
            if os.path.getsize(src) > MAX_INPUT_BYTES:
                raise Abort("%s is larger than a build input may be" % name)
            shutil.copyfile(src, archive)
            shutil.copyfile(src + ".sha256", archive + ".sha256")
        with open(archive + ".sha256", "rb") as f:
            check_sidecar(archive, f.read(4096), name)
        dest = os.path.join(work, "inputs", target)
        extract_input(archive, dest)
        info = read_build_info(dest)
        epoch = check_build_info(info, target, version, channel, rev)
        for key in ("runtime", "ui", "stub"):
            if key in info and not os.path.isfile(os.path.join(dest, *info[key].split("/"))):
                raise Abort("%s: BUILD-INFO.txt names %s, which the input lacks" % (target, info[key]))
        entry = os.path.join(dest, "tree", *info["entry"].split("/"))
        if not os.path.isfile(entry):
            raise Abort("%s: the input has no %s" % (target, info["entry"]))
        out[target] = (dest, info, epoch)
    epochs = {epoch for _, _, epoch in out.values()}
    if len(epochs) > 1:
        raise Abort("the build inputs were built from different commits (source_date_epoch differs)")
    return out


def parse_tag(tag):
    m = TAG_RE.fullmatch(tag or "")
    if not m:
        raise Abort("bad tag: %r" % (tag,))
    return m.group(1)


def setup_argv(config, release_dir, version, target, out):
    """`lunepack setup` of one target. lunepack stamps the Windows setup exe with the config's
    `[windows] icon`, so the config is the same file the pack used."""
    argv = ["setup", release_dir, "--version", version, "--target", target, "--out", out, "--config", config]
    if target.startswith("macos-"):
        argv.append("--zip")
    return argv


def manifest_lists(path, wanted):
    """Whether the manifest envelope at `path` lists an entry named `wanted`. Signatures are not
    checked here: `lunepack publish push` verifies the whole release before it writes anything."""
    try:
        with open(path, "rb") as f:
            env = json.loads(f.read(64 * 1024 * 1024))
        payload = json.loads(base64.b64decode(env["payload"], validate=True))
        return any(e.get("path") == wanted for e in payload.get("entries", []))
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as err:
        raise Abort("%s does not parse as a manifest" % os.path.basename(path)) from err


def output_line(target, digest):
    """One job-output line. The digest is checked before the line exists, so nothing but 64 hex
    characters can reach $GITHUB_OUTPUT."""
    if target not in TARGETS or not SHA256_RE.fullmatch(digest):
        raise Abort("refusing an output line for %r" % (target,))
    return "setup_sha256_%s=%s" % (target.replace("-", "_"), digest)


def cmd_pack(args, secrets):
    version = parse_tag(args.tag)
    channel = channel_of(version)
    config = os.path.abspath(args.config)
    check_root(config)
    rev = read_pin()
    work = os.path.abspath(args.work)
    if os.path.exists(work):
        raise Abort("%s exists; give a new work directory" % work)
    os.makedirs(work)
    lp = Lunepack(os.path.abspath(args.lunepack), secrets, config)
    release = None if args.inputs_dir else Release(secrets, args.tag)

    inputs = gather_inputs(args.inputs_dir, version, work, rev, release)
    epoch = next(iter(inputs.values()))[2]
    if epoch != tag_commit_time():
        raise Abort("the build inputs' source_date_epoch is not the time of the tag's commit")

    release_dir = os.path.join(work, "release")
    lp.run(["publish", "fetch", "--release-dir", release_dir, "--version", version, "--config", config])
    for target, (dest, info, _) in inputs.items():
        spec = TARGETS[target]
        argv = ["pack", "--input", os.path.join(dest, "tree"), "--version", version, "--target", target,
                "--release-dir", release_dir, "--sign", "release", "--config", config,
                "--runtime", os.path.join(dest, *info["runtime"].split("/")),
                "--ui", os.path.join(dest, *info["ui"].split("/"))]
        if "stub" in info:
            argv += ["--stub", os.path.join(dest, *info["stub"].split("/"))]
        if channel == "beta":
            argv += ["--channel", "beta"]
        lp.run(argv, epoch=epoch, sign=True)
        manifest = os.path.join(release_dir, version, "%s.manifest.json" % target)
        if not manifest_lists(manifest, spec["notices"]):
            raise Abort("%s has no %s (the lunepack binaries' licences)" % (os.path.basename(manifest), spec["notices"]))

    setup_dir = os.path.join(work, "setup")
    os.makedirs(setup_dir)
    lines = []
    for target in TARGETS:
        name = TARGETS[target]["setup"].format(v=version)
        path = os.path.join(setup_dir, name)
        lp.run(setup_argv(config, release_dir, version, target, path), epoch=epoch)
        if not os.path.isfile(path):
            raise Abort("lunepack setup wrote no %s" % name)
        digest = file_sha256(path)
        sidecar(path, name, digest)
        lines.append(output_line(target, digest))
    for line in lines:
        print(line)
    if args.output_file:
        with open(args.output_file, "a", encoding="ascii", newline="\n") as f:
            f.write("".join(line + "\n" for line in lines))


def cmd_attach(args, secrets):
    version = parse_tag(args.tag)
    expect = {}
    for item in args.expect:
        target, sep, digest = item.partition("=")
        if not sep or target not in TARGETS or target in expect or not SHA256_RE.fullmatch(digest):
            raise Abort("bad --expect: %r" % (target,))
        expect[target] = digest
    if set(expect) != set(TARGETS):
        raise Abort("--expect must name every target: %s" % ", ".join(sorted(TARGETS)))
    files = []
    for target, digest in expect.items():
        name = TARGETS[target]["setup"].format(v=version)
        path = os.path.join(args.dir, name)
        if not os.path.isfile(path) or os.path.islink(path):
            raise Abort("%s has no %s" % (args.dir, name))
        if file_sha256(path) != digest:
            raise Abort("%s is not the file the pack job built (its SHA-256 differs)" % name)
        side = path + ".sha256"
        try:
            with open(side, "rb") as f:
                check_sidecar(path, f.read(4096), name)
        except OSError as err:
            raise Abort("%s has no usable .sha256" % name) from err
        files += [(name, path), (name + ".sha256", side)]
    names = {name for name, _ in files}
    extra = sorted(set(os.listdir(args.dir)) - names)
    if extra:
        raise Abort("%s holds files that are not setup files: %s" % (args.dir, extra))

    release = Release(secrets, args.tag)
    assets = release.assets()
    for name, path in files:
        have = assets.get(name)
        if have is not None:
            if have.get("size") != os.path.getsize(path):
                raise Abort("the release already has a different %s; delete it by hand" % name)
            print("the release already has %s" % name)
            continue
        print("attach %s" % name)
        release.upload(name, path)
    for name in sorted(assets):
        if "-build." in name:
            print("delete release asset %s" % name)
            release.delete_asset(assets[name])


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("check-root")
    c.add_argument("--config", required=True)
    p = sub.add_parser("pack")
    p.add_argument("--tag", required=True)
    p.add_argument("--lunepack", required=True)
    p.add_argument("--config", required=True)
    p.add_argument("--work", required=True)
    p.add_argument("--inputs-dir", help="trial runs: the build inputs and their .sha256 from a directory, not the release")
    p.add_argument("--output-file", help="append the setup_sha256_<target> lines here (the job's $GITHUB_OUTPUT)")
    a = sub.add_parser("attach")
    a.add_argument("--tag", required=True)
    a.add_argument("--dir", required=True)
    a.add_argument("--expect", required=True, nargs="+", metavar="TARGET=SHA256")
    args = ap.parse_args(argv)
    secrets = Secrets(os.environ)
    try:
        if args.cmd == "check-root":
            check_root(args.config)
        elif args.cmd == "pack":
            cmd_pack(args, secrets)
        else:
            cmd_attach(args, secrets)
    except Abort as err:
        print("release-pack: " + secrets.redact(str(err)), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
