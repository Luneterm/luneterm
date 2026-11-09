#!/usr/bin/env python3
"""Create the draft GitHub release for a tag and attach the Windows and Linux archives.

Run by the publish job of .github/workflows/release.yml:

    publish-release.py <tag> <artifacts-dir> <notes-file>

Needs GITHUB_TOKEN (contents: write) and GITHUB_REPOSITORY in the environment. Standard library
only: the runner image has python3 and no gh.

Everything local is checked before the first API call. An existing release for the tag, draft or
not, is never changed: the script stops, and a failed draft of an earlier run is deleted by hand.
"""

import hashlib
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

# Constants on purpose: the token goes to these two hosts and nowhere else.
API = "https://api.github.com"
UPLOADS = "https://uploads.github.com"

TAG_RE = re.compile(r"v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?")
REPO_RE = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")
GLIBC_RE = re.compile(r"[0-9]+\.[0-9]+(\.[0-9]+)?")
PER_PAGE = 100


class Failure(Exception):
    """A reason to stop, printed without the token."""


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    # urllib would resend the Authorization header to wherever a redirect points.
    def redirect_request(self, *args, **kwargs):
        return None


_OPENER = urllib.request.build_opener(_NoRedirect)


def http_send(method, url, headers, data):
    """Send one request and return (status, body). A redirect comes back as its 3xx status."""
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with _OPENER.open(request, timeout=300) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as err:
        return err.code, err.read()
    except OSError as err:
        raise Failure("%s %s: %s" % (method, url, err)) from err


class GitHub:
    def __init__(self, repo, token, send):
        self.repo = repo
        self.token = token
        self.send = send

    def call(self, method, base, path, data=None, content_type=None):
        url = "%s/repos/%s/%s" % (base, self.repo, path)
        if not url.startswith((API + "/", UPLOADS + "/")):
            raise Failure("refusing to send the token to %s" % url)
        headers = {
            "Authorization": "Bearer " + self.token,
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "luneterm-publish-release",
        }
        if content_type:
            headers["Content-Type"] = content_type
        status, body = self.send(method, url, headers, data)
        if not 200 <= status < 300:
            text = body[:500].decode("utf-8", "replace")
            raise Failure("%s %s: HTTP %d: %s" % (method, url, status, text))
        return json.loads(body) if body else None

    def release_exists(self, tag):
        page = 1
        while True:
            # The list endpoint is the one that also returns drafts.
            found = self.call("GET", API, "releases?per_page=%d&page=%d" % (PER_PAGE, page))
            if any(release.get("tag_name") == tag for release in found):
                return True
            if len(found) < PER_PAGE:
                return False
            page += 1

    def create_draft(self, tag, version, notes):
        payload = {
            "tag_name": tag,
            "name": "Luneterm " + version,
            "body": notes,
            "draft": True,
            "prerelease": "-rc." in tag,
        }
        data = json.dumps(payload).encode("utf-8")
        return int(self.call("POST", API, "releases", data, "application/json")["id"])

    def upload(self, release_id, name, data, content_type):
        path = "releases/%d/assets?name=%s" % (release_id, urllib.parse.quote(name, safe=""))
        print("uploading %s (%d bytes)" % (name, len(data)))
        self.call("POST", UPLOADS, path, data, content_type)

    def delete_release(self, release_id):
        self.call("DELETE", API, "releases/%d" % release_id)


def only_file(directory, suffix, expected):
    matches = sorted(n for n in os.listdir(directory) if n.endswith(suffix))
    if matches != [expected]:
        raise Failure("%s: expected only %s among *%s, found %s" % (directory, expected, suffix, matches))
    return os.path.join(directory, expected)


def read_bytes(path):
    with open(path, "rb") as handle:
        return handle.read()


def prepare(tag, directory, notes_path):
    """Check everything local; return (version, notes text, [(name, bytes, content type)])."""
    if not TAG_RE.fullmatch(tag):
        raise Failure("bad tag: %r" % tag)
    version = tag[1:]
    linux = only_file(directory, "-linux-x64.tar.gz", "luneterm-%s-linux-x64.tar.gz" % version)
    windows = only_file(directory, "-windows-x64.zip", "luneterm-%s-windows-x64.zip" % version)
    glibc = read_bytes(os.path.join(directory, "glibc-min.txt")).decode("ascii").strip()
    if not GLIBC_RE.fullmatch(glibc):
        raise Failure("glibc-min.txt does not hold a version: %r" % glibc)
    with open(notes_path, encoding="utf-8") as handle:
        notes = handle.read().replace("{version}", version).replace("{glibc}", glibc)
    assets = []
    for path, content_type in ((linux, "application/gzip"), (windows, "application/zip")):
        name = os.path.basename(path)
        data = read_bytes(path)
        sidecar = "%s  %s\n" % (hashlib.sha256(data).hexdigest(), name)
        assets.append((name, data, content_type))
        assets.append((name + ".sha256", sidecar.encode("ascii"), "text/plain"))
    return version, notes, assets


def publish(argv, env, send):
    if len(argv) != 3:
        raise Failure("usage: publish-release.py <tag> <artifacts-dir> <notes-file>")
    tag, directory, notes_path = argv
    token = env.get("GITHUB_TOKEN", "")
    repo = env.get("GITHUB_REPOSITORY", "")
    if not token:
        raise Failure("GITHUB_TOKEN is not set")
    if not REPO_RE.fullmatch(repo):
        raise Failure("bad GITHUB_REPOSITORY: %r" % repo)
    try:
        version, notes, assets = prepare(tag, directory, notes_path)
    except (OSError, ValueError) as err:
        raise Failure(str(err)) from err

    github = GitHub(repo, token, send)
    if github.release_exists(tag):
        raise Failure("a release for %s already exists; not touching it" % tag)
    release_id = github.create_draft(tag, version, notes)
    try:
        for name, data, content_type in assets:
            github.upload(release_id, name, data, content_type)
    except Failure as failure:
        try:
            github.delete_release(release_id)
        except Failure as cleanup:
            print("could not delete draft %d: %s" % (release_id, cleanup), file=sys.stderr)
        raise failure
    print("draft release created for %s (id %d)" % (tag, release_id))


def main():
    try:
        publish(sys.argv[1:], os.environ, http_send)
    except Failure as failure:
        print("publish-release: %s" % failure, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
