#!/usr/bin/env python3
"""Publish a Luneterm release to the signed update feed on R2, and keep the feed to its newest 10.

Run by .github/workflows/publish-updates.yml (auto-update plan step 6). Standard library only.

    publish-updates.py publish --tag <tag> --lunepack <cli> --work <dir> [--apply]
        [--inputs-dir <dir> | (GitHub release of <tag>)] [--store-dir <dir> | (R2)] [--keep N]
        [--targets <t,...>  (trial runs with --store-dir only)]
    publish-updates.py refresh --lunepack <cli> --work <dir> [--apply] [--if-expiring-within-days N]
        [--skip-if-unpublished] [--store-dir <dir> | (R2)]
    publish-updates.py plan --release-dir <dir> [--add <version>] [--keep N]

`publish` takes the release's build inputs (scripts/make-build-input.py; from the GitHub release of
the tag, or --inputs-dir), rebuilds the release directory from the bucket (keys.json, root-<k>.json,
the feeds, the manifests and packs of every listed version; the publisher state file from the private
bucket), packs each target (`lunepack pack --deltas 3`), drops what falls out of the newest 10 of a
channel (`feed refresh --drop`), adds the version (`feed add`, 365-day expiry; a stable version to
stable and beta, a release candidate to beta), builds the setup files, then writes in this order:
the new `<ver>/` objects (packs and deltas before manifests), the feeds (after checking that no live
feed changed since it was read), the state file, and last the deletes: a version's objects are
deleted only when no channel lists it any more, and are named from its own manifests, never by
listing the bucket. With a GitHub release it then attaches the setup files and deletes the
`*-build.*` inputs. `refresh` re-signs the feeds (all, or those expiring within N days) with a new
365-day expiry. `plan` prints what retention would drop and delete for a local release directory.

Nothing is written (bucket, release, local store) without --apply: the default is a dry run that
does all local work and prints the plan. Every key written or deleted is under `luneterm/`.

Secrets come from the environment and are removed from it at start: LUNEPACK_RELEASE_KEY reaches
only the lunepack child, R2_ACCESS_KEY_ID and R2_SECRET_ACCESS_KEY only this process's own signed R2
requests (the S3 API of R2, SigV4 on the standard library), GITHUB_TOKEN only the GitHub API calls.
No value of one is ever printed. This is hygiene, not a boundary: processes of one user can read each
other's environment (`/proc/<pid>/environ`), and removing a variable from Python's `os.environ` does
not erase it there. What keeps the release key safe is that this job runs no third-party code: the
pinned lunepack CLI and the Python standard library only (no npm tool, no wrangler).
The run refuses to start while the packaging configs or root.rs name the throwaway test root (D9).
"""

import argparse
import base64
import hashlib
import hmac
import http.client
import importlib.util
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)

PREFIX = "luneterm/"
PUBLIC_BUCKET = "izzat-updates"
PRIVATE_BUCKET = "izzat-updates-publisher"
PUBLIC_URL = "https://updates.izzat.dev/"
STATE_KEY = PREFIX + "lunepack-state.json"
# Deletes that were decided but not (all) done: kept in the private bucket so a rerun finishes them.
PENDING_KEY = PREFIX + "pending-deletes.json"

KEEP = 10
DELTAS = 3
EXPIRES_DAYS = 365
CHANNELS = ("stable", "beta")

# The throwaway test root of crates/luneterm/src/update/root.rs (its private half was discarded).
TEST_ROOT_KEYS = frozenset({"7c4f44f54daf49ca873f3aac937f0f19e668364a01178aa339f64690f998e996"})

SECRET_NAMES = ("LUNEPACK_RELEASE_KEY", "R2_ACCESS_KEY_ID", "R2_SECRET_ACCESS_KEY", "GITHUB_TOKEN")
# Removed from the environment every child inherits: stray cloud credentials a child has no use for.
DROPPED_NAMES = (
    "CLOUDFLARE_API_TOKEN", "CF_API_TOKEN", "CLOUDFLARE_R2_API_TOKEN", "CLOUDFLARE_ACCOUNT_ID",
    "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN",
)

# Per target: the packaging config's OS, the asset names, and what BUILD-INFO.txt must say.
TARGETS = {
    "linux-x86_64": {
        "os": "linux",
        "input": "luneterm-{v}-linux-x64-build.tar.gz",
        "setup": "luneterm-{v}-linux-x64-setup",
        "notices": "lunepack-NOTICES.txt",
        "info": {
            "target": "linux-x86_64",
            "config": "packaging/lunepack-linux.toml",
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
            "config": "packaging/lunepack-windows.toml",
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
            "config": "packaging/lunepack-macos.toml",
            "entry": "Contents/MacOS/luneterm",
            "tree": "tree",
            "runtime": "tree/Contents/MacOS/lunepack-runtime",
            "ui": "tree/Contents/Helpers/lunepack-update.app/Contents/MacOS/lunepack-ui",
        },
        "optional": (),
    },
}
TARGET_IDS = frozenset(
    {"windows-x86_64", "windows-aarch64", "macos-x86_64", "macos-aarch64", "macos-universal", "linux-x86_64", "linux-aarch64"}
)

# Safe extraction caps (P1).
MAX_INPUT_BYTES = 400 * 1024 * 1024
MAX_ENTRIES = 20000
MAX_FILE_BYTES = 512 * 1024 * 1024
MAX_TOTAL_BYTES = 1024 * 1024 * 1024
MAX_DOC_BYTES = 64 * 1024 * 1024

TAG_RE = re.compile(r"v([0-9]+\.[0-9]+\.[0-9]+(?:-rc\.[0-9]+)?)")
_IDENT = r"(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
SEMVER_RE = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-(%s(?:\.%s)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?" % (_IDENT, _IDENT)
)
CHANNEL_RE = re.compile(r"[a-z0-9][a-z0-9-]{0,31}")
COMPONENT_RE = re.compile(r"[A-Za-z0-9._+-]+")


class Abort(Exception):
    """A reason to stop. Printed without any secret."""


# ------------------------------------------------------------------------------------- versions


def version_key(version):
    """SemVer 2.0.0 precedence as a sortable key (build metadata ignored; FORMAT.md 10.3)."""
    m = SEMVER_RE.fullmatch(version) if isinstance(version, str) else None
    if m is None or len(version) > 64:
        raise Abort("not a version: %r" % (version,))
    core = (int(m.group(1)), int(m.group(2)), int(m.group(3)))
    if m.group(4) is None:
        return (core, 1, ())
    ids = tuple((0, int(p), "") if p.isdigit() else (1, 0, p) for p in m.group(4).split("."))
    return (core, 0, ids)


def newest_first(versions):
    return sorted(set(versions), key=lambda v: (version_key(v), v), reverse=True)


def channels_for(version):
    """D3: a stable version goes to stable and beta, a release candidate to beta only."""
    return ("beta",) if version_key(version)[1] == 0 else ("stable", "beta")


# ------------------------------------------------------------------------------------ retention


def plan_retention(feeds, keep=KEEP):
    """D4. `feeds`: {channel: [version, ...]} as they will be listed. Returns
    (drops: {channel: [versions to drop, newest first]}, delete_versions: sorted list): each channel
    keeps its `keep` newest by precedence; a version is deleted only when no channel keeps it."""
    if keep < 1:
        raise Abort("keep must be at least 1")
    drops = {}
    kept = set()
    listed = set()
    for channel in sorted(feeds):
        ordered = newest_first(feeds[channel])
        listed.update(ordered)
        kept.update(ordered[:keep])
        drops[channel] = ordered[keep:]
    delete = newest_first(listed - kept)
    return drops, delete


def with_version(feeds, version):
    """The channel lists after `version` is added to its channels (D3); adding twice is a no-op."""
    out = {channel: list(feeds.get(channel, [])) for channel in set(feeds) | set(channels_for(version))}
    for channel in channels_for(version):
        if version not in out[channel]:
            out[channel].append(version)
    return out


# ------------------------------------------------------------------------- documents and objects


def open_envelope(data, kind):
    """The payload of a lunepack DSSE envelope of `kind` (FORMAT.md 2). Signatures are not checked
    here: lunepack checks them before it writes anything built on the document, and nothing is
    deleted before it has."""
    try:
        env = json.loads(data)
        if not isinstance(env, dict) or set(env) != {"payloadType", "payload", "signatures"}:
            raise ValueError("not an envelope")
        if env["payloadType"] != "application/vnd.lunepack.%s.v1+json" % kind:
            raise ValueError("payload type is not %s" % kind)
        payload = json.loads(base64.b64decode(env["payload"], validate=True))
    except (ValueError, TypeError, KeyError) as err:
        raise Abort("a %s document does not parse: %s" % (kind, err)) from err
    if not isinstance(payload, dict) or payload.get("kind") != kind or payload.get("format") != 1:
        raise Abort("a %s document has the wrong kind or format" % kind)
    return payload


def feed_releases(payload, channel):
    """{version: {target: {path, size, sha256}}} of a feed payload, with the names checked."""
    if payload.get("channel") != channel:
        raise Abort("feed-%s.json names channel %r" % (channel, payload.get("channel")))
    out = {}
    for release in payload.get("releases") or []:
        version = release.get("version")
        version_key(version)
        targets = release.get("targets")
        if not isinstance(targets, dict) or not targets:
            raise Abort("feed-%s.json: release %s has no targets" % (channel, version))
        for target, ref in targets.items():
            if target not in TARGET_IDS:
                raise Abort("feed-%s.json: release %s has an unknown target" % (channel, version))
            if not isinstance(ref, dict) or ref.get("path") != "%s/%s.manifest.json" % (version, target):
                raise Abort("feed-%s.json: %s/%s: unexpected manifest path" % (channel, version, target))
        if version in out:
            raise Abort("feed-%s.json lists %s twice" % (channel, version))
        out[version] = targets
    return out


def objects_of(version, manifests):
    """The release-directory paths of one version: per target its pack and delta files (first), then
    its manifest (last: the upload order of CLI.md). `manifests`: {target: manifest payload}; a
    missing or unparsable one, or any path that is not exactly the name the format gives, aborts:
    a delete set is never guessed."""
    version_key(version)
    if not manifests:
        raise Abort("%s: no manifest to name its objects from" % version)
    data, docs = [], []
    for target in sorted(manifests):
        m = manifests[target]
        if target not in TARGET_IDS:
            raise Abort("%s: unknown target %r" % (version, target))
        if not isinstance(m, dict):
            raise Abort("%s/%s: the manifest is missing" % (version, target))
        if m.get("version") != version or m.get("target") != target:
            raise Abort("%s/%s: the manifest names another release" % (version, target))
        pack = (m.get("pack") or {}).get("path")
        if pack != "%s/%s.pack" % (version, target):
            raise Abort("%s/%s: unexpected pack path %r" % (version, target, pack))
        data.append(pack)
        for delta in m.get("deltas") or []:
            base = delta.get("from")
            version_key(base)
            if delta.get("path") != "%s/%s.from-%s.delta" % (version, target, base):
                raise Abort("%s/%s: unexpected delta path %r" % (version, target, delta.get("path")))
            data.append(delta["path"])
        docs.append("%s/%s.manifest.json" % (version, target))
    return data + docs


def bucket_key(path):
    """The bucket key of a release-directory path: always under `luneterm/`."""
    parts = path.split("/")
    if not parts or any(p in ("", ".", "..") or not COMPONENT_RE.fullmatch(p) for p in parts):
        raise Abort("refusing an object path %r" % path)
    key = PREFIX + path
    assert key.startswith(PREFIX)
    return key


def assert_deletable(key):
    """Every delete is a `<ver>/<file>` object under `luneterm/`: never a feed, keys, root or state."""
    if not key.startswith(PREFIX):
        raise Abort("refusing to delete %r: not under %s" % (key, PREFIX))
    rest = key[len(PREFIX) :].split("/")
    if len(rest) != 2 or not COMPONENT_RE.fullmatch(rest[1]) or rest[1] in (".", ".."):
        raise Abort("refusing to delete %r" % key)
    version_key(rest[0])


def delete_keys(delete_versions, manifests_by_version):
    """All keys to delete, or Abort before anything is deleted (a missing manifest names no keys)."""
    keys = []
    for version in delete_versions:
        manifests = manifests_by_version.get(version)
        if not manifests:
            raise Abort("%s: no manifests to name its objects from; deleting nothing" % version)
        for path in objects_of(version, manifests):
            key = bucket_key(path)
            assert_deletable(key)
            keys.append(key)
    return keys


# ------------------------------------------------------------------------------- secrets, children


class Secrets:
    """Secrets taken out of the environment, so no child inherits one by accident."""

    def __init__(self, environ):
        self.values = {}
        for name in SECRET_NAMES:
            value = environ.pop(name, None)
            if value:
                self.values[name] = value
        self.account = environ.pop("CLOUDFLARE_ACCOUNT_ID", "") or ""
        for name in DROPPED_NAMES:
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
    env = {k: v for k, v in os.environ.items() if k not in SECRET_NAMES and k not in DROPPED_NAMES}
    env.update(extra or {})
    return env


class Lunepack:
    def __init__(self, cli, secrets, config_dir):
        self.cli = cli
        self.secrets = secrets
        self.config_dir = config_dir

    def config(self, target_os, channel):
        return os.path.join(self.config_dir, "lunepack-%s-%s.toml" % (target_os, channel))

    def run(self, args, epoch=None, sign=True):
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
            raise Abort("lunepack %s failed (exit %d)" % (" ".join(args[:2] if args[0] == "feed" else args[:1]), run.returncode))
        return out


# ------------------------------------------------------------------------------- pending deletes


def key_version(key):
    """The version of a `luneterm/<version>/<file>` key."""
    return key[len(PREFIX) :].split("/")[0]


def parse_pending(data):
    """The delete keys recorded by an earlier run (`None`: no file). Fail closed: a file that is not
    exactly `{"format": 1, "delete": [<deletable key>, ...]}` aborts."""
    if data is None:
        return []
    try:
        doc = json.loads(data)
    except ValueError as err:
        raise Abort("%s does not parse: %s" % (PENDING_KEY, err)) from err
    if not isinstance(doc, dict) or set(doc) != {"format", "delete"} or doc["format"] != 1:
        raise Abort("%s is not a pending-deletes document" % PENDING_KEY)
    keys = doc["delete"]
    if not isinstance(keys, list) or not all(isinstance(k, str) for k in keys):
        raise Abort("%s: `delete` is not a list of keys" % PENDING_KEY)
    for key in keys:
        assert_deletable(key)
    return sorted(set(keys))


def pending_document(keys):
    return (json.dumps({"format": 1, "delete": sorted(set(keys))}, indent=1) + "\n").encode("ascii")


# ----------------------------------------------------------------------------------------- SigV4

EMPTY_SHA256 = hashlib.sha256(b"").hexdigest()


def _hmac(key, text):
    return hmac.new(key, text.encode("utf-8"), hashlib.sha256).digest()


def uri_encode(text, keep_slash=False):
    """The SigV4 URI encoding: everything but `A-Za-z0-9-_.~` (and `/` in a path) as %XX."""
    return urllib.parse.quote(text, safe="/" if keep_slash else "")


def sigv4_authorization(method, path, query, headers, payload_hash, key_id, secret, region, service, amz_date):
    """The `Authorization` header value of AWS Signature Version 4 (one signed request, the S3 way:
    the path is encoded once). `headers` are the headers to sign, any case, and must hold `host` and
    `x-amz-date` (S3 also `x-amz-content-sha256`); `query` is {name: value}; `amz_date` is
    `YYYYMMDDTHHMMSSZ`. Standard library only."""
    lowered = {name.lower(): " ".join(str(value).split()) for name, value in headers.items()}
    names = sorted(lowered)
    canonical_headers = "".join("%s:%s\n" % (name, lowered[name]) for name in names)
    signed = ";".join(names)
    canonical_query = "&".join(
        "%s=%s" % pair for pair in sorted((uri_encode(k), uri_encode(v)) for k, v in query.items())
    )
    canonical = "\n".join([method, uri_encode(path, True), canonical_query, canonical_headers, signed, payload_hash])
    date = amz_date[:8]
    scope = "%s/%s/%s/aws4_request" % (date, region, service)
    to_sign = "\n".join(["AWS4-HMAC-SHA256", amz_date, scope, hashlib.sha256(canonical.encode("utf-8")).hexdigest()])
    signing_key = _hmac(_hmac(_hmac(_hmac(("AWS4" + secret).encode("utf-8"), date), region), service), "aws4_request")
    signature = hmac.new(signing_key, to_sign.encode("utf-8"), hashlib.sha256).hexdigest()
    return "AWS4-HMAC-SHA256 Credential=%s/%s, SignedHeaders=%s, Signature=%s" % (key_id, scope, signed, signature)


def file_sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            chunk = f.read(1 << 20)
            if not chunk:
                return digest.hexdigest()
            digest.update(chunk)


def https_transport(method, host, path, headers, body_path, limit):
    """One HTTPS request to `host`; the body is streamed from `body_path`. Returns (status, bytes of
    the answer, at most `limit` + 1 of them)."""
    conn = http.client.HTTPSConnection(host, timeout=600)
    try:
        if body_path is None:
            conn.request(method, path, headers=headers)
            response = conn.getresponse()
        else:
            with open(body_path, "rb") as body:
                conn.request(method, path, body=body, headers=headers)
                response = conn.getresponse()
        return response.status, response.read(limit + 1)
    except (OSError, http.client.HTTPException) as err:
        raise Abort("%s https://%s%s: %s" % (method, host, path, type(err).__name__)) from err
    finally:
        conn.close()


# ---------------------------------------------------------------------------------------- stores


class LocalStore:
    """A directory standing in for the two buckets (<dir>/<bucket>/<key>): tests and trial runs."""

    def __init__(self, root):
        self.root = root

    def _path(self, bucket, key):
        return os.path.join(self.root, bucket, *key.split("/"))

    def _read(self, bucket, key):
        path = self._path(bucket, key)
        if not os.path.isfile(path):
            return None
        with open(path, "rb") as f:
            return f.read()

    def _write(self, bucket, key, src):
        dest = self._path(bucket, key)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        shutil.copyfile(src, dest)

    def get_doc(self, key):
        return self._read(PUBLIC_BUCKET, key)

    def get_object(self, key, dest):
        path = self._path(PUBLIC_BUCKET, key)
        if not os.path.isfile(path):
            return False
        shutil.copyfile(path, dest)
        return True

    def get_state(self):
        return self._read(PRIVATE_BUCKET, STATE_KEY)

    def get_pending(self):
        return self._read(PRIVATE_BUCKET, PENDING_KEY)

    def put(self, key, src, content_type, cache_control):
        assert key.startswith(PREFIX)
        self._write(PUBLIC_BUCKET, key, src)

    def put_state(self, src):
        self._write(PRIVATE_BUCKET, STATE_KEY, src)

    def put_pending(self, src):
        self._write(PRIVATE_BUCKET, PENDING_KEY, src)

    def delete(self, key):
        assert_deletable(key)
        path = self._path(PUBLIC_BUCKET, key)
        if os.path.isfile(path):
            os.remove(path)


class R2Store:
    """The buckets through R2's S3 API, every request signed with SigV4 by this process (no other
    program holds the access key), and the public URL (the immutable `<ver>/` objects, checked
    against their manifests). The documents are read from the bucket itself, so no cache."""

    REGION = "auto"

    def __init__(self, secrets, scratch, transport=https_transport, clock=time.gmtime):
        self.secrets = secrets
        self.scratch = scratch
        if not re.fullmatch(r"[0-9a-f]{32}", secrets.account or ""):
            raise Abort("CLOUDFLARE_ACCOUNT_ID is not set (or is not a 32-hex account id)")
        self.host = "%s.r2.cloudflarestorage.com" % secrets.account
        self.key_id = secrets.get("R2_ACCESS_KEY_ID")
        self.secret = secrets.get("R2_SECRET_ACCESS_KEY")
        self.transport = transport
        self.clock = clock

    def _request(self, method, bucket, key, body_path=None, extra=None, limit=1 << 16):
        """One signed request. Returns (status, body bytes, at most `limit` + 1 of them)."""
        path = "/%s/%s" % (bucket, key)
        payload_hash = file_sha256(body_path) if body_path else EMPTY_SHA256
        signed = {
            "host": self.host,
            "x-amz-content-sha256": payload_hash,
            "x-amz-date": time.strftime("%Y%m%dT%H%M%SZ", self.clock()),
        }
        signed.update(extra or {})
        headers = dict(signed)
        headers["Authorization"] = sigv4_authorization(
            method, path, {}, signed, payload_hash, self.key_id, self.secret, self.REGION, "s3", signed["x-amz-date"]
        )
        if body_path:
            headers["Content-Length"] = str(os.path.getsize(body_path))
        return self.transport(method, self.host, path, headers, body_path, limit)

    def _why(self, status, body):
        """`HTTP <status> <S3 error code>`: no header, no body text beyond the code."""
        code = re.search(rb"<Code>([A-Za-z0-9]{1,64})</Code>", body[:4096])
        return "HTTP %d%s" % (status, " " + code.group(1).decode("ascii") if code else "")

    def _get(self, bucket, key):
        status, body = self._request("GET", bucket, key, limit=MAX_DOC_BYTES)
        if status == 404:
            return None
        if status != 200:
            raise Abort("get %s/%s failed: %s" % (bucket, key, self._why(status, body)))
        if len(body) > MAX_DOC_BYTES:
            raise Abort("%s/%s is larger than a document may be" % (bucket, key))
        return body

    def get_doc(self, key):
        return self._get(PUBLIC_BUCKET, key)

    def get_state(self):
        return self._get(PRIVATE_BUCKET, STATE_KEY)

    def get_pending(self):
        return self._get(PRIVATE_BUCKET, PENDING_KEY)

    def get_object(self, key, dest):
        request = urllib.request.Request(PUBLIC_URL + key, headers={"User-Agent": "luneterm-publish-updates"})
        try:
            with urllib.request.urlopen(request, timeout=300) as response, open(dest, "wb") as out:
                total = 0
                while True:
                    chunk = response.read(1 << 20)
                    if not chunk:
                        break
                    total += len(chunk)
                    if total > MAX_TOTAL_BYTES:
                        raise Abort("%s is too large" % key)
                    out.write(chunk)
            return True
        except urllib.error.HTTPError as err:
            if err.code == 404:
                return False
            raise Abort("GET %s: HTTP %d" % (PUBLIC_URL + key, err.code)) from err
        except OSError as err:
            raise Abort("GET %s: %r" % (PUBLIC_URL + key, err)) from err

    def _put(self, bucket, key, src, content_type, cache_control):
        status, body = self._request(
            "PUT", bucket, key, body_path=src, extra={"content-type": content_type, "cache-control": cache_control}
        )
        if status != 200:
            raise Abort("upload of %s failed: %s" % (key, self._why(status, body)))

    def put(self, key, src, content_type, cache_control):
        assert key.startswith(PREFIX)
        self._put(PUBLIC_BUCKET, key, src, content_type, cache_control)

    def put_state(self, src):
        self._put(PRIVATE_BUCKET, STATE_KEY, src, "application/json", "no-store")

    def put_pending(self, src):
        self._put(PRIVATE_BUCKET, PENDING_KEY, src, "application/json", "no-store")

    def delete(self, key):
        assert_deletable(key)
        status, body = self._request("DELETE", PUBLIC_BUCKET, key)
        if status not in (200, 204):  # S3 answers 204 for a key that is already gone
            raise Abort("delete of %s failed: %s" % (key, self._why(status, body)))


# -------------------------------------------------------------------------------- the commit order

IMMUTABLE = "public, max-age=31536000, immutable"
NO_CACHE = "no-cache"


def content_type_of(path):
    return "application/json" if path.endswith(".json") else "application/octet-stream"


def commit(store, release_dir, uploads, feeds, state_path, deletes, apply, log=print, live_versions=(), scratch=None):
    """Write in the only safe order: new `<ver>/` objects; a check that no live feed changed since
    it was read (P3); the record of the deletes still to do; the feeds; the state file; the deletes.
    Any failure raises before the next stage, so a failed upload leaves the live feeds as they were
    (P9). `feeds`: {channel: bytes of the live feed read at the start, or None}. Without `apply`
    only prints.

    The deletes are recorded in the private bucket before any feed is written (a feed that no longer
    lists a version is what makes its objects orphans), joined to what an earlier run left there, and
    each run does all of them, so a delete that failed (or a run that died after the feeds) is done
    by the next run, a publish or a refresh. A recorded key whose version is in `live_versions` (a
    version listed again) is dropped from the record, never deleted. A delete that fails does not
    stop the others; the run then fails with the keys left, which stay recorded."""
    for key in deletes:
        assert_deletable(key)
    verb = "" if apply else "[dry run] would "
    recorded = parse_pending(store.get_pending())
    live = set(live_versions)
    todo = sorted(k for k in set(recorded) | set(deletes) if key_version(k) not in live)
    for key in sorted(set(recorded) - set(todo)):
        log("not deleting %s: its version is listed again" % key)

    recorded_now = [recorded]  # what the bucket's record holds now

    def record(keys):
        if not apply or sorted(keys) == recorded_now[0]:
            return
        fd, path = tempfile.mkstemp(dir=scratch)
        try:
            with os.fdopen(fd, "wb") as f:
                f.write(pending_document(keys))
            store.put_pending(path)
        finally:
            os.remove(path)
        recorded_now[0] = sorted(keys)

    for path in uploads:
        key = bucket_key(path)
        log("%supload %s" % (verb, key))
        if apply:
            store.put(key, os.path.join(release_dir, *path.split("/")), content_type_of(path), IMMUTABLE)
    for channel, original in sorted(feeds.items()):
        live_doc = store.get_doc(bucket_key("feed-%s.json" % channel))
        if live_doc != original:
            raise Abort("feed-%s.json changed since this run read it (another publish?); writing no feed" % channel)
    if todo != recorded:
        log("%srecord %d delete(s) in %s/%s" % (verb, len(todo), PRIVATE_BUCKET, PENDING_KEY))
        record(todo)
    for channel in sorted(feeds):
        key = bucket_key("feed-%s.json" % channel)
        log("%supload %s" % (verb, key))
        if apply:
            store.put(key, os.path.join(release_dir, "feed-%s.json" % channel), "application/json", NO_CACHE)
    if feeds:
        log("%supload the state file %s/%s" % (verb, PRIVATE_BUCKET, STATE_KEY))
        if apply:
            store.put_state(state_path)
    failed = []
    for key in todo:
        log("%sdelete %s" % (verb, key))
        if apply:
            try:
                store.delete(key)
            except Abort as err:
                log("delete failed: %s" % err)
                failed.append(key)
    if apply and todo:
        record(failed)
    if failed:
        raise Abort(
            "%d delete(s) failed and stay recorded in %s/%s; rerun a publish or a refresh to retry them: %s"
            % (len(failed), PRIVATE_BUCKET, PENDING_KEY, ", ".join(failed[:5]) + (" ..." if len(failed) > 5 else ""))
        )


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


# ----------------------------------------------------------------------------------- repo checks


def read_pin():
    with open(os.path.join(REPO, "Cargo.toml"), "rb") as f:
        dep = tomllib.load(f)["workspace"]["dependencies"]["lunepack"]
    rev = dep.get("rev", "")
    if not re.fullmatch(r"[0-9a-f]{40}", rev):
        raise Abort("the lunepack dependency has no 40-hex rev")
    return rev


def refuse_test_root():
    """D9: no release may ship with the throwaway test root, in the configs or in the app."""
    for target_os in ("linux", "windows", "macos"):
        with open(os.path.join(REPO, "packaging", "lunepack-%s.toml" % target_os), "rb") as f:
            keys = set(tomllib.load(f)["root"]["keys"])
        if keys & TEST_ROOT_KEYS:
            raise Abort("packaging/lunepack-%s.toml names the test root key; refusing to publish" % target_os)
    with open(os.path.join(REPO, "crates", "luneterm", "src", "update", "root.rs"), encoding="utf-8") as f:
        root_rs = f.read()
    if re.search(r"pub const IS_TEST_ROOT: bool = true;", root_rs) or "TEST_ROOT_KEY]" in root_rs:
        raise Abort("crates/luneterm/src/update/root.rs is the test root; refusing to publish")


def render_configs(config_dir):
    """packaging-config.py for every OS and channel, into one directory: one state file next to them."""
    os.makedirs(config_dir, exist_ok=True)
    for target_os in ("linux", "windows", "macos"):
        for channel in CHANNELS:
            out = os.path.join(config_dir, "lunepack-%s-%s.toml" % (target_os, channel))
            run = subprocess.run(
                [sys.executable, "-I", os.path.join(HERE, "packaging-config.py"), target_os, channel, out],
                env=child_env(),
                capture_output=True,
                text=True,
            )
            if run.returncode != 0:
                raise Abort("packaging-config.py %s %s failed: %s" % (target_os, channel, run.stderr.strip()))


# ------------------------------------------------------------------------------ the release dir


class ReleaseDir:
    """The release directory rebuilt from the bucket, and what was read (to check before writing)."""

    def __init__(self, store, path, state_path):
        self.store = store
        self.path = path
        self.state_path = state_path
        self.feeds_raw = {}  # channel -> bytes read at start, or None
        self.listed = {}  # channel -> {version: {target: ref}}
        self.manifests = {}  # version -> {target: payload}
        self.has_state = False

    def file(self, rel):
        return os.path.join(self.path, *rel.split("/"))

    def write(self, rel, data):
        path = self.file(rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            f.write(data)

    def load(self):
        os.makedirs(self.path)
        keys = self.store.get_doc(bucket_key("keys.json"))
        if keys is None:
            raise Abort("the bucket has no %skeys.json (key ceremony not done?)" % PREFIX)
        self.write("keys.json", keys)
        k = 2
        while True:
            root = self.store.get_doc(bucket_key("root-%d.json" % k))
            if root is None:
                break
            self.write("root-%d.json" % k, root)
            k += 1
        for channel in CHANNELS:
            raw = self.store.get_doc(bucket_key("feed-%s.json" % channel))
            self.feeds_raw[channel] = raw
            if raw is None:
                self.listed[channel] = {}
                continue
            self.write("feed-%s.json" % channel, raw)
            self.listed[channel] = feed_releases(open_envelope(raw, "feed"), channel)
        state = self.store.get_state()
        if state is not None:
            self.has_state = True
            with open(self.state_path, "wb") as f:
                f.write(state)
        elif any(raw is not None for raw in self.feeds_raw.values()):
            raise Abort("a feed is live but the publisher state file is missing; not starting over (CLI.md, publisher state)")
        for channel, releases in self.listed.items():
            for version, targets in releases.items():
                for target, ref in targets.items():
                    self.fetch_manifest(version, target, ref)

    def fetch_manifest(self, version, target, ref):
        have = self.manifests.setdefault(version, {})
        rel = ref["path"]
        data = self.store.get_doc(bucket_key(rel))
        if data is None:
            raise Abort("%s is listed but missing from the bucket" % rel)
        if len(data) != ref.get("size") or hashlib.sha256(data).hexdigest() != ref.get("sha256"):
            raise Abort("%s does not match the feed's reference" % rel)
        payload = open_envelope(data, "manifest")
        if target in have and have[target] != payload:
            raise Abort("%s differs between feeds" % rel)
        have[target] = payload
        self.write(rel, data)

    def fetch_pack(self, version, target):
        m = self.manifests[version][target]
        ref = m["pack"]
        rel = ref["path"]
        path = self.file(rel)
        if os.path.exists(path):
            return
        os.makedirs(os.path.dirname(path), exist_ok=True)
        if not self.store.get_object(bucket_key(rel), path):
            raise Abort("%s is listed but missing from the bucket" % rel)
        with open(path, "rb") as f:
            data = f.read()
        if len(data) != ref.get("size") or hashlib.sha256(data).hexdigest() != ref.get("sha256"):
            raise Abort("%s does not match its manifest" % rel)

    def versions(self, channel):
        return list(self.listed.get(channel, {}))


# ------------------------------------------------------------------------------------- GitHub


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
            "User-Agent": "luneterm-publish-updates",
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
            return urllib.request.Request(newurl, headers={"User-Agent": "luneterm-publish-updates"}, method="GET")

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


def sidecar(path, name):
    with open(path, "rb") as f:
        digest = hashlib.sha256(f.read()).hexdigest()
    out = path + ".sha256"
    with open(out, "w", encoding="ascii", newline="\n") as f:
        f.write("%s  %s\n" % (digest, name))
    return out


def gather_inputs(args, secrets, version, need, work, rev, release):
    """{target: (tree dir, info, epoch)} for each target in `need`."""
    channel = channels_for(version)[0]
    out = {}
    assets = release.assets() if release else None
    for target in need:
        name = TARGETS[target]["input"].format(v=version)
        archive = os.path.join(work, "downloads", name)
        os.makedirs(os.path.dirname(archive), exist_ok=True)
        if release:
            if name not in assets or name + ".sha256" not in assets:
                raise Abort("the release has no %s (or its .sha256)" % name)
            release.download(assets[name], archive)
            release.download(assets[name + ".sha256"], archive + ".sha256")
        else:
            src = os.path.join(args.inputs_dir, name)
            if not os.path.isfile(src) or not os.path.isfile(src + ".sha256"):
                raise Abort("%s has no %s (or its .sha256)" % (args.inputs_dir, name))
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


def setup_argv(lp, release_dir, version, target, out, channel):
    """`lunepack setup` of one target. Its config is the one the pack used (its OS and the version's
    channel): lunepack stamps the Windows setup exe with that config's `[windows] icon`, so the Linux
    config would leave it without one."""
    argv = ["setup", release_dir, "--version", version, "--target", target, "--out", out,
            "--config", lp.config(TARGETS[target]["os"], channel)]
    if target.startswith("macos-"):
        argv.append("--zip")
    return argv


def cmd_publish(args, secrets):
    m = TAG_RE.fullmatch(args.tag or "")
    if not m:
        raise Abort("bad tag: %r" % args.tag)
    version = m.group(1)
    targets = list(TARGETS)
    if args.targets:
        # Trial runs only (a local store): a real release always carries every target.
        if not args.store_dir:
            raise Abort("--targets needs --store-dir")
        targets = [t for t in TARGETS if t in args.targets.split(",")]
        if not targets or len(targets) != len(args.targets.split(",")):
            raise Abort("bad --targets: %r" % args.targets)
    refuse_test_root()
    rev = read_pin()
    work = os.path.abspath(args.work)
    if os.path.exists(work):
        raise Abort("%s exists; give a new work directory" % work)
    os.makedirs(work)
    config_dir = os.path.join(work, "config")
    render_configs(config_dir)
    lp = Lunepack(os.path.abspath(args.lunepack), secrets, config_dir)
    store = LocalStore(os.path.abspath(args.store_dir)) if args.store_dir else R2Store(secrets, work)
    release = None if args.inputs_dir else Release(secrets, args.tag)

    rd = ReleaseDir(store, os.path.join(work, "release"), os.path.join(config_dir, "lunepack-state.json"))
    rd.load()
    first = not rd.has_state
    current = {channel: rd.versions(channel) for channel in CHANNELS}
    after = with_version(current, version)
    drops, delete_versions = plan_retention(after, args.keep)
    if version in delete_versions or any(version in d for d in drops.values()):
        raise Abort("%s would fall out of the newest %d at once; not publishing it" % (version, args.keep))
    print("channels: %s; first publish: %s" % (", ".join(channels_for(version)), "yes" if first else "no"))

    # This version may be in the bucket already (a rerun): reuse those targets, never re-pack them.
    for target in targets:
        rel = "%s/%s.manifest.json" % (version, target)
        if version in rd.manifests and target in rd.manifests[version]:
            continue
        data = store.get_doc(bucket_key(rel))
        if data is not None:
            payload = open_envelope(data, "manifest")
            rd.manifests.setdefault(version, {})[target] = payload
            rd.write(rel, data)
            print("reusing %s from the bucket" % rel)
    existing = set(rd.manifests.get(version, {}))
    need = [t for t in targets if t not in existing]

    # Packs every write verifies: every version still listed after the drops (they include the
    # delta bases: the newest 3 of each feed), and this version's reused targets.
    kept = {v for ch in after for v in after[ch] if v not in drops.get(ch, [])}
    for v in sorted(kept):
        for target in rd.manifests.get(v, {}):
            rd.fetch_pack(v, target)

    inputs = gather_inputs(args, secrets, version, need, work, rev, release)
    channel = channels_for(version)[0]
    new_paths = []
    for target in need:
        dest, info, epoch = inputs[target]
        spec = TARGETS[target]
        argv = ["pack", "--input", os.path.join(dest, "tree"), "--version", version, "--target", target,
                "--release-dir", rd.path, "--sign", "release", "--deltas", str(DELTAS),
                "--runtime", os.path.join(dest, *info["runtime"].split("/")),
                "--ui", os.path.join(dest, *info["ui"].split("/")),
                "--config", lp.config(spec["os"], channel)]
        if "stub" in info:
            argv += ["--stub", os.path.join(dest, *info["stub"].split("/"))]
        lp.run(argv, epoch=epoch)
        rel = "%s/%s.manifest.json" % (version, target)
        with open(rd.file(rel), "rb") as f:
            payload = open_envelope(f.read(), "manifest")
        if not any(e.get("path") == spec["notices"] for e in payload.get("entries", [])):
            raise Abort("%s has no %s (the lunepack binaries' licences)" % (rel, spec["notices"]))
        rd.manifests.setdefault(version, {})[target] = payload
        new_paths += objects_of(version, {target: payload})

    any_config = lp.config("linux", "stable")
    changed = {}
    for ch in CHANNELS:
        if drops.get(ch):
            argv = ["feed", "refresh", "--release-dir", rd.path, "--channel", ch, "--sign", "release",
                    "--expires-after-days", str(EXPIRES_DAYS), "--config", any_config]
            for v in drops[ch]:
                argv += ["--drop", v]
            lp.run(argv)
            changed[ch] = rd.feeds_raw[ch]
    for ch in channels_for(version):
        listed = rd.listed[ch].get(version)
        if listed is not None and set(listed) == set(targets):
            print("feed-%s.json already lists %s with every target" % (ch, version))
            continue
        argv = ["feed", "add", "--release-dir", rd.path, "--channel", ch, "--version", version, "--sign", "release",
                "--expires-after-days", str(EXPIRES_DAYS), "--config", any_config]
        if first:
            print("no publisher state file and no feed: first publish, the bucket's keys.json is the baseline")
            argv.append("--trust-release-dir")
        lp.run(argv)
        changed[ch] = rd.feeds_raw[ch]

    deletes = delete_keys(delete_versions, rd.manifests)
    setups = []
    out_dir = os.path.join(work, "out")
    os.makedirs(out_dir)
    for target in targets:
        name = TARGETS[target]["setup"].format(v=version)
        path = os.path.join(out_dir, name)
        lp.run(setup_argv(lp, rd.path, version, target, path, channel), sign=False)
        setups.append((name, path))

    print("retention (keep %d): drop %s; delete versions %s" % (args.keep, json.dumps(drops, sort_keys=True), delete_versions))
    live_versions = {v for ch in after for v in after[ch] if v not in drops.get(ch, [])}
    commit(store, rd.path, new_paths, changed, rd.state_path, deletes, args.apply, live_versions=live_versions,
           scratch=work)

    verb = "" if args.apply else "[dry run] would "
    if release is None:
        for name, path in setups:
            print("%skeep %s (%s)" % (verb, name, path))
        return
    assets = release.assets()
    for name, path in setups:
        side = sidecar(path, name)
        for asset_name, asset_path in ((name, path), (name + ".sha256", side)):
            have = assets.get(asset_name)
            if have is not None:
                if have.get("size") != os.path.getsize(asset_path):
                    raise Abort("the release already has a different %s; delete it by hand" % asset_name)
                print("the release already has %s" % asset_name)
                continue
            print("%sattach %s" % (verb, asset_name))
            if args.apply:
                release.upload(asset_name, asset_path)
    for asset_name in sorted(assets):
        if "-build." in asset_name:
            print("%sdelete release asset %s" % (verb, asset_name))
            if args.apply:
                release.delete_asset(assets[asset_name])


def cmd_refresh(args, secrets):
    try:
        refuse_test_root()
    except Abort as err:
        if not args.skip_if_unpublished:
            raise
        # The weekly schedule must not turn red before the real root and the first feed exist.
        print("::notice::scheduled refresh skipped: %s" % err)
        return
    work = os.path.abspath(args.work)
    if os.path.exists(work):
        raise Abort("%s exists; give a new work directory" % work)
    os.makedirs(work)
    config_dir = os.path.join(work, "config")
    render_configs(config_dir)
    lp = Lunepack(os.path.abspath(args.lunepack), secrets, config_dir)
    store = LocalStore(os.path.abspath(args.store_dir)) if args.store_dir else R2Store(secrets, work)
    if args.skip_if_unpublished and store.get_doc(bucket_key("keys.json")) is None:
        print("::notice::scheduled refresh skipped: the bucket has no %skeys.json yet (nothing published)" % PREFIX)
        return
    rd = ReleaseDir(store, os.path.join(work, "release"), os.path.join(config_dir, "lunepack-state.json"))
    rd.load()
    now = int(time.time())
    changed = {}
    for ch in CHANNELS:
        raw = rd.feeds_raw[ch]
        if raw is None:
            continue
        expires = open_envelope(raw, "feed").get("expires")
        days_left = (expires - now) / 86400 if isinstance(expires, int) else None
        if args.if_expiring_within_days is not None and days_left is not None and days_left >= args.if_expiring_within_days:
            print("feed-%s.json: %.0f days left; not re-signed" % (ch, days_left))
            continue
        for v, targets in rd.listed[ch].items():
            for target in targets:
                rd.fetch_pack(v, target)
        lp.run(["feed", "refresh", "--release-dir", rd.path, "--channel", ch, "--sign", "release",
                "--expires-after-days", str(EXPIRES_DAYS), "--config", lp.config("linux", "stable")])
        changed[ch] = raw
    if not changed:
        print("nothing to re-sign")
    # Also finishes the deletes an earlier run left recorded (nothing is written when there are none).
    live_versions = {v for releases in rd.listed.values() for v in releases}
    commit(store, rd.path, [], changed, rd.state_path, [], args.apply, live_versions=live_versions, scratch=work)


def cmd_plan(args):
    """Retention of a local release directory (no lunepack, no network): what would be dropped and
    deleted, by key."""
    listed, manifests = {}, {}
    for channel in CHANNELS:
        path = os.path.join(args.release_dir, "feed-%s.json" % channel)
        if not os.path.isfile(path):
            continue
        with open(path, "rb") as f:
            listed[channel] = feed_releases(open_envelope(f.read(), "feed"), channel)
    for releases in listed.values():
        for version, targets in releases.items():
            for target, ref in targets.items():
                mpath = os.path.join(args.release_dir, *ref["path"].split("/"))
                if os.path.isfile(mpath):
                    with open(mpath, "rb") as f:
                        manifests.setdefault(version, {})[target] = open_envelope(f.read(), "manifest")
                else:
                    manifests.setdefault(version, {})[target] = None
    feeds = {ch: list(r) for ch, r in listed.items()}
    if args.add:
        feeds = with_version(feeds, args.add)
    drops, delete_versions = plan_retention(feeds, args.keep)
    for channel in sorted(feeds):
        print("%s: keep %s" % (channel, newest_first(v for v in feeds[channel] if v not in drops[channel])))
        print("%s: drop %s" % (channel, drops[channel]))
    for key in delete_keys(delete_versions, manifests):
        print("delete %s" % key)
    if not delete_versions:
        print("delete nothing")


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("publish")
    p.add_argument("--tag", required=True)
    p.add_argument("--lunepack", required=True)
    p.add_argument("--work", required=True)
    p.add_argument("--inputs-dir")
    p.add_argument("--store-dir")
    p.add_argument("--keep", type=int, default=KEEP)
    p.add_argument("--targets", help="trial runs with --store-dir: a comma list of targets")
    p.add_argument("--apply", action="store_true")
    r = sub.add_parser("refresh")
    r.add_argument("--lunepack", required=True)
    r.add_argument("--work", required=True)
    r.add_argument("--store-dir")
    r.add_argument("--if-expiring-within-days", type=int)
    r.add_argument("--skip-if-unpublished", action="store_true",
                   help="the weekly schedule: exit 0 with a notice while the test root is in or the bucket has no keys.json")
    r.add_argument("--apply", action="store_true")
    q = sub.add_parser("plan")
    q.add_argument("--release-dir", required=True)
    q.add_argument("--add")
    q.add_argument("--keep", type=int, default=KEEP)
    args = ap.parse_args(argv)
    secrets = Secrets(os.environ)
    try:
        if args.cmd == "publish":
            cmd_publish(args, secrets)
        elif args.cmd == "refresh":
            cmd_refresh(args, secrets)
        else:
            cmd_plan(args)
    except Abort as err:
        print("publish-updates: " + secrets.redact(str(err)), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
