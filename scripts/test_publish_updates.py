"""Tests of scripts/publish-updates.py that need no network, no key and no lunepack (plan step 6).

    python3 -m unittest scripts/test_publish_updates.py
"""

import base64
import contextlib
import importlib.util
import io
import json
import os
import pathlib
import shutil
import tarfile
import tempfile
import unittest
import zipfile

HERE = pathlib.Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("publish_updates", HERE / "publish-updates.py")
pu = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(pu)

TARGETS3 = ("linux-x86_64", "macos-aarch64", "windows-x86_64")


def envelope(kind, payload):
    raw = base64.b64encode(json.dumps(payload).encode()).decode()
    return json.dumps(
        {"payloadType": "application/vnd.lunepack.%s.v1+json" % kind, "payload": raw, "signatures": []}
    ).encode()


def manifest(version, target, deltas=()):
    m = {
        "format": 1,
        "kind": "manifest",
        "critical": [],
        "app_id": "my.ijat.luneterm",
        "version": version,
        "target": target,
        "issued": 1,
        "pack": {"path": "%s/%s.pack" % (version, target), "size": 1, "sha256": "0" * 64},
        "entries": [],
    }
    if deltas:
        m["deltas"] = [
            {"from": d, "path": "%s/%s.from-%s.delta" % (version, target, d), "size": 1, "sha256": "0" * 64}
            for d in deltas
        ]
    return m


def versions(n, minor=1):
    return ["0.%d.%d" % (minor, i) for i in range(n)]


class Versions(unittest.TestCase):
    def test_precedence(self):
        order = ["0.9.0-rc.1", "0.9.0-rc.2", "0.9.0-rc.10", "0.9.0", "0.9.1", "0.10.0", "1.0.0-alpha",
                 "1.0.0-alpha.1", "1.0.0-beta", "1.0.0"]
        self.assertEqual(sorted(reversed(order), key=pu.version_key), order)
        self.assertLess(pu.version_key("0.9.0-rc.2"), pu.version_key("0.9.0"))

    def test_bad_versions_abort(self):
        for bad in ("v1.0.0", "1.0", "01.0.0", "1.0.0-", "1.0.0/x", "", None, "1.0.0-a..b"):
            with self.assertRaises(pu.Abort, msg=repr(bad)):
                pu.version_key(bad)

    def test_channels(self):
        self.assertEqual(pu.channels_for("0.9.0"), ("stable", "beta"))
        self.assertEqual(pu.channels_for("0.9.0-rc.2"), ("beta",))


class Retention(unittest.TestCase):
    def check_counts(self, n):
        vs = versions(n)
        drops, delete = pu.plan_retention({"stable": vs}, keep=10)
        newest = pu.newest_first(vs)
        self.assertEqual(drops["stable"], newest[10:])
        self.assertEqual(delete, newest[10:])
        return drops, delete

    def test_9_10_11_25(self):
        self.assertEqual(self.check_counts(9), ({"stable": []}, []))
        self.assertEqual(self.check_counts(10), ({"stable": []}, []))
        self.assertEqual(self.check_counts(11), ({"stable": ["0.1.0"]}, ["0.1.0"]))
        drops, delete = self.check_counts(25)
        self.assertEqual(len(drops["stable"]), 15)
        self.assertEqual(delete[0], "0.1.14")
        self.assertEqual(delete[-1], "0.1.0")

    def test_order_is_by_precedence_not_text(self):
        vs = ["0.1.%d" % i for i in (1, 2, 3, 10, 11, 9, 8, 7, 6, 5, 4)]
        drops, delete = pu.plan_retention({"stable": vs}, keep=10)
        self.assertEqual(delete, ["0.1.1"])

    def test_a_version_kept_by_beta_survives_stables_prune(self):
        stable = versions(11)  # 0.1.0 .. 0.1.10
        # beta: 0.1.0 plus newer release candidates, 10 in all: 0.1.0 is still in beta's newest 10.
        beta = ["0.1.0"] + ["0.0.%d" % i for i in range(9)]
        drops, delete = pu.plan_retention({"stable": stable, "beta": beta}, keep=10)
        self.assertEqual(drops["stable"], ["0.1.0"])
        self.assertEqual(drops["beta"], [])
        self.assertEqual(delete, [])

    def test_overlap_both_drop_then_delete(self):
        stable = versions(11)
        beta = stable + ["0.2.0-rc.1"]
        drops, delete = pu.plan_retention({"stable": stable, "beta": beta}, keep=10)
        self.assertEqual(drops["beta"], ["0.1.1", "0.1.0"])
        self.assertEqual(drops["stable"], ["0.1.0"])
        self.assertEqual(delete, ["0.1.0"])  # 0.1.1 is still kept by stable

    def test_rc_ordering_in_beta(self):
        beta = ["0.9.0", "0.9.0-rc.2", "0.9.0-rc.1"] + versions(8, minor=8)
        drops, delete = pu.plan_retention({"beta": beta}, keep=10)
        self.assertEqual(drops["beta"], ["0.8.0"])
        drops, _ = pu.plan_retention({"beta": ["0.9.0", "0.9.0-rc.2", "0.9.0-rc.1"]}, keep=2)
        self.assertEqual(drops["beta"], ["0.9.0-rc.1"])

    def test_republishing_a_version_is_idempotent(self):
        feeds = {"stable": versions(10), "beta": versions(10) + ["0.2.0-rc.1"]}
        once = pu.with_version(feeds, "0.2.0")
        twice = pu.with_version(once, "0.2.0")
        self.assertEqual({k: sorted(v) for k, v in once.items()}, {k: sorted(v) for k, v in twice.items()})
        self.assertEqual(pu.plan_retention(once), pu.plan_retention(twice))
        self.assertEqual(sorted(once["stable"]), sorted(versions(10) + ["0.2.0"]))
        self.assertEqual(pu.with_version({}, "0.2.0-rc.1"), {"beta": ["0.2.0-rc.1"]})


class Objects(unittest.TestCase):
    def test_objects_of_lists_data_first_then_manifests(self):
        ms = {t: manifest("0.9.0", t, deltas=("0.8.0", "0.9.0-rc.1") if t == "linux-x86_64" else ()) for t in TARGETS3}
        keys = pu.objects_of("0.9.0", ms)
        self.assertEqual(
            keys,
            [
                "0.9.0/linux-x86_64.pack",
                "0.9.0/linux-x86_64.from-0.8.0.delta",
                "0.9.0/linux-x86_64.from-0.9.0-rc.1.delta",
                "0.9.0/macos-aarch64.pack",
                "0.9.0/windows-x86_64.pack",
                "0.9.0/linux-x86_64.manifest.json",
                "0.9.0/macos-aarch64.manifest.json",
                "0.9.0/windows-x86_64.manifest.json",
            ],
        )

    def test_a_missing_or_wrong_manifest_aborts(self):
        with self.assertRaises(pu.Abort):
            pu.objects_of("0.9.0", {})
        with self.assertRaises(pu.Abort):
            pu.objects_of("0.9.0", {"linux-x86_64": None})
        with self.assertRaises(pu.Abort):
            pu.objects_of("0.9.0", {"linux-x86_64": manifest("0.8.0", "linux-x86_64")})
        bad = manifest("0.9.0", "linux-x86_64")
        bad["pack"]["path"] = "../whipspot/x.pack"
        with self.assertRaises(pu.Abort):
            pu.objects_of("0.9.0", {"linux-x86_64": bad})
        bad = manifest("0.9.0", "linux-x86_64", deltas=("0.8.0",))
        bad["deltas"][0]["path"] = "0.8.0/linux-x86_64.pack"
        with self.assertRaises(pu.Abort):
            pu.objects_of("0.9.0", {"linux-x86_64": bad})

    def test_delete_keys_are_all_under_luneterm(self):
        ms = {"0.1.0": {t: manifest("0.1.0", t) for t in TARGETS3}}
        keys = pu.delete_keys(["0.1.0"], ms)
        self.assertEqual(len(keys), 6)
        for key in keys:
            self.assertTrue(key.startswith("luneterm/0.1.0/"), key)

    def test_a_missing_manifest_names_no_delete(self):
        ms = {"0.1.0": {t: manifest("0.1.0", t) for t in TARGETS3}}
        with self.assertRaises(pu.Abort):
            pu.delete_keys(["0.1.0", "0.0.9"], ms)
        ms["0.0.9"] = {"linux-x86_64": None}
        with self.assertRaises(pu.Abort):
            pu.delete_keys(["0.1.0", "0.0.9"], ms)

    def test_only_version_objects_are_deletable(self):
        for key in ("luneterm/0.1.0/linux-x86_64.pack", "luneterm/0.9.0-rc.1/macos-aarch64.manifest.json"):
            pu.assert_deletable(key)
        for key in ("luneterm/feed-stable.json", "luneterm/keys.json", "luneterm/lunepack-state.json",
                    "whipspot/0.1.0/x.pack", "luneterm/../whipspot/x", "luneterm/0.1.0/../../x",
                    "luneterm/notaversion/x.pack", "luneterm/0.1.0/a/b", "/luneterm/0.1.0/x"):
            with self.assertRaises(pu.Abort, msg=key):
                pu.assert_deletable(key)

    def test_feed_releases_checks_the_manifest_paths(self):
        feed = {"format": 1, "kind": "feed", "channel": "beta", "releases": [
            {"version": "0.9.0", "targets": {"linux-x86_64": {"path": "0.9.0/linux-x86_64.manifest.json"}}}]}
        self.assertEqual(list(pu.feed_releases(feed, "beta")), ["0.9.0"])
        with self.assertRaises(pu.Abort):
            pu.feed_releases(feed, "stable")
        feed["releases"][0]["targets"]["linux-x86_64"]["path"] = "0.8.0/linux-x86_64.manifest.json"
        with self.assertRaises(pu.Abort):
            pu.feed_releases(feed, "beta")

    def test_envelope_kind_is_checked(self):
        m = manifest("0.9.0", "linux-x86_64")
        self.assertEqual(pu.open_envelope(envelope("manifest", m), "manifest"), m)
        with self.assertRaises(pu.Abort):
            pu.open_envelope(envelope("manifest", m), "feed")
        with self.assertRaises(pu.Abort):
            pu.open_envelope(b"{}", "manifest")


class FakeStore:
    def __init__(self, feeds=None, fail_put=None, pending=None, fail_delete=()):
        self.calls = []
        self.feeds = dict(feeds or {})
        self.fail_put = fail_put
        self.pending = pending  # bytes of the pending-deletes document, or None
        self.fail_delete = set(fail_delete)

    def get_doc(self, key):
        return self.feeds.get(key)

    def get_pending(self):
        return self.pending

    def put(self, key, src, content_type, cache_control):
        if key == self.fail_put:
            raise pu.Abort("upload of %s failed" % key)
        self.calls.append(("put", key, content_type, cache_control))

    def put_state(self, src):
        self.calls.append(("state",))

    def put_pending(self, src):
        with open(src, "rb") as f:
            self.pending = f.read()
        self.calls.append(("pending", tuple(pu.parse_pending(self.pending))))

    def delete(self, key):
        if key in self.fail_delete:
            raise pu.Abort("delete of %s failed: HTTP 500" % key)
        self.calls.append(("delete", key))


class Commit(unittest.TestCase):
    uploads = ["0.9.0/linux-x86_64.pack", "0.9.0/linux-x86_64.manifest.json"]
    deletes = ["luneterm/0.1.0/linux-x86_64.pack", "luneterm/0.1.0/linux-x86_64.manifest.json"]

    def run_commit(self, store, apply=True, feeds=None, deletes=None, live=()):
        feeds = {"stable": b"old-stable", "beta": None} if feeds is None else feeds
        out = []
        pu.commit(store, "/rd", self.uploads, feeds, "/state", self.deletes if deletes is None else deletes, apply,
                  log=out.append, live_versions=live)
        return out

    def test_order_uploads_pending_feeds_state_deletes_clear(self):
        store = FakeStore({"luneterm/feed-stable.json": b"old-stable"})
        self.run_commit(store)
        kinds = [c[0] if c[0] != "put" else c[1] for c in store.calls]
        self.assertEqual(
            kinds,
            ["luneterm/0.9.0/linux-x86_64.pack", "luneterm/0.9.0/linux-x86_64.manifest.json", "pending",
             "luneterm/feed-beta.json", "luneterm/feed-stable.json", "state", "delete", "delete", "pending"],
        )
        self.assertEqual(store.calls[2][1], tuple(sorted(self.deletes)))  # recorded before any feed
        self.assertEqual(store.calls[-1][1], ())  # cleared once every delete was done
        puts = {c[1]: c[2:] for c in store.calls if c[0] == "put"}
        self.assertEqual(puts["luneterm/feed-stable.json"], ("application/json", "no-cache"))
        self.assertEqual(puts["luneterm/0.9.0/linux-x86_64.pack"][1], pu.IMMUTABLE)

    def test_a_failed_upload_writes_no_feed_and_deletes_nothing(self):
        store = FakeStore({"luneterm/feed-stable.json": b"old-stable"}, fail_put="luneterm/0.9.0/linux-x86_64.manifest.json")
        with self.assertRaises(pu.Abort):
            self.run_commit(store)
        self.assertEqual([c[1] for c in store.calls], ["luneterm/0.9.0/linux-x86_64.pack"])

    def test_a_failed_feed_upload_deletes_nothing(self):
        store = FakeStore({"luneterm/feed-stable.json": b"old-stable"}, fail_put="luneterm/feed-stable.json")
        with self.assertRaises(pu.Abort):
            self.run_commit(store)
        self.assertFalse([c for c in store.calls if c[0] in ("delete", "state")])

    def test_a_feed_changed_meanwhile_stops_before_any_feed(self):
        store = FakeStore({"luneterm/feed-stable.json": b"someone-else"})
        with self.assertRaises(pu.Abort):
            self.run_commit(store)
        self.assertFalse([c for c in store.calls if c[0] != "put" or "feed" in c[1]])

    def test_dry_run_writes_nothing(self):
        store = FakeStore({"luneterm/feed-stable.json": b"old-stable"})
        out = self.run_commit(store, apply=False)
        self.assertEqual(store.calls, [])
        self.assertTrue(all(line.startswith("[dry run] would ") for line in out))
        self.assertIn("[dry run] would delete luneterm/0.1.0/linux-x86_64.pack", out)

    def test_a_foreign_delete_key_stops_before_anything(self):
        store = FakeStore({"luneterm/feed-stable.json": b"old-stable"})
        with self.assertRaises(pu.Abort):
            pu.commit(store, "/rd", self.uploads, {}, "/s", ["whipspot/0.1.0/x.pack"], True, log=lambda _: None)
        self.assertEqual(store.calls, [])

    # F2: a delete that fails is recorded and finished by the next run.
    def test_a_failed_delete_stays_recorded_and_the_others_still_run(self):
        store = FakeStore({"luneterm/feed-stable.json": b"old-stable"}, fail_delete={self.deletes[0]})
        with self.assertRaises(pu.Abort) as ctx:
            self.run_commit(store)
        self.assertIn(self.deletes[0], str(ctx.exception))
        self.assertIn(("delete", self.deletes[1]), store.calls)
        self.assertEqual(pu.parse_pending(store.pending), [self.deletes[0]])

    def test_a_rerun_removes_the_orphans_an_earlier_run_left(self):
        store = FakeStore({"luneterm/feed-stable.json": b"old-stable"}, fail_delete={self.deletes[0]})
        with self.assertRaises(pu.Abort):
            self.run_commit(store)
        store.fail_delete.clear()
        store.calls.clear()
        # The rerun finds nothing new to drop (the version is in no feed any more): deletes=[].
        self.run_commit(store, deletes=[])
        self.assertEqual([c for c in store.calls if c[0] == "delete"], [("delete", self.deletes[0])])
        self.assertEqual(pu.parse_pending(store.pending), [])

    def test_a_refresh_without_feeds_also_finishes_recorded_deletes(self):
        store = FakeStore(pending=pu.pending_document(self.deletes))
        pu.commit(store, "/rd", [], {}, "/s", [], True, log=lambda _: None)
        self.assertEqual([c[1] for c in store.calls if c[0] == "delete"], sorted(self.deletes))
        self.assertEqual(pu.parse_pending(store.pending), [])

    def test_a_recorded_key_of_a_version_listed_again_is_never_deleted(self):
        store = FakeStore(pending=pu.pending_document(self.deletes + ["luneterm/0.5.0/linux-x86_64.pack"]))
        out = []
        pu.commit(store, "/rd", [], {}, "/s", [], True, log=out.append, live_versions={"0.5.0"})
        self.assertEqual([c[1] for c in store.calls if c[0] == "delete"], sorted(self.deletes))
        self.assertIn("not deleting luneterm/0.5.0/linux-x86_64.pack: its version is listed again", out)
        self.assertEqual(pu.parse_pending(store.pending), [])

    def test_nothing_recorded_and_nothing_to_delete_writes_nothing(self):
        store = FakeStore()
        pu.commit(store, "/rd", [], {}, "/s", [], True, log=lambda _: None)
        self.assertEqual(store.calls, [])

    def test_a_bad_pending_document_aborts_before_any_write(self):
        for bad in (b"not json", b'{"format": 2, "delete": []}', b'{"format": 1, "delete": ["luneterm/keys.json"]}',
                    b'{"format": 1, "delete": [1]}', b'{"format": 1}', b"[]"):
            store = FakeStore(pending=bad)
            with self.assertRaises(pu.Abort, msg=bad):
                pu.commit(store, "/rd", self.uploads, {}, "/s", [], True, log=lambda _: None)
            self.assertEqual(store.calls, [])

    def test_dry_run_records_nothing(self):
        store = FakeStore(pending=pu.pending_document(self.deletes[:1]))
        out = []
        pu.commit(store, "/rd", [], {}, "/s", self.deletes, False, log=out.append)
        self.assertEqual(store.calls, [])
        self.assertTrue(any("record 2 delete(s)" in line for line in out))


class SigV4(unittest.TestCase):
    """The signer against the worked examples of AWS's Signature Version 4 documentation (S3)."""

    KEY = "AKIAIOSFODNN7EXAMPLE"
    SECRET = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
    DATE = "20130524T000000Z"
    HOST = "examplebucket.s3.amazonaws.com"

    def sign(self, method, path, query, headers, payload_hash):
        return pu.sigv4_authorization(method, path, query, headers, payload_hash, self.KEY, self.SECRET,
                                      "us-east-1", "s3", self.DATE)

    def expect(self, signed_headers, signature):
        return ("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, "
                "SignedHeaders=%s, Signature=%s" % (signed_headers, signature))

    def test_get_object_with_a_range(self):
        got = self.sign("GET", "/test.txt", {}, {"Host": self.HOST, "Range": "bytes=0-9",
                        "x-amz-content-sha256": pu.EMPTY_SHA256, "x-amz-date": self.DATE}, pu.EMPTY_SHA256)
        self.assertEqual(got, self.expect("host;range;x-amz-content-sha256;x-amz-date",
                         "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"))

    def test_put_object_encodes_the_path_once(self):
        payload = pu.hashlib.sha256(b"Welcome to Amazon S3.").hexdigest()
        self.assertEqual(payload, "44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072")
        got = self.sign("PUT", "/test$file.text", {}, {"Host": self.HOST, "Date": "Fri, 24 May 2013 00:00:00 GMT",
                        "x-amz-storage-class": "REDUCED_REDUNDANCY", "x-amz-content-sha256": payload,
                        "x-amz-date": self.DATE}, payload)
        self.assertEqual(got, self.expect("date;host;x-amz-content-sha256;x-amz-date;x-amz-storage-class",
                         "98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"))

    def test_bucket_lifecycle_has_an_empty_query_value(self):
        got = self.sign("GET", "/", {"lifecycle": ""}, {"Host": self.HOST, "x-amz-content-sha256": pu.EMPTY_SHA256,
                        "x-amz-date": self.DATE}, pu.EMPTY_SHA256)
        self.assertEqual(got, self.expect("host;x-amz-content-sha256;x-amz-date",
                         "fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"))

    def test_list_objects_sorts_the_query(self):
        got = self.sign("GET", "/", {"prefix": "J", "max-keys": "2"}, {"Host": self.HOST,
                        "x-amz-content-sha256": pu.EMPTY_SHA256, "x-amz-date": self.DATE}, pu.EMPTY_SHA256)
        self.assertEqual(got, self.expect("host;x-amz-content-sha256;x-amz-date",
                         "34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"))

    def test_the_signature_depends_on_the_secret_and_the_body(self):
        base = {"Host": self.HOST, "x-amz-content-sha256": pu.EMPTY_SHA256, "x-amz-date": self.DATE}
        one = self.sign("GET", "/a", {}, base, pu.EMPTY_SHA256)
        self.assertNotEqual(one, self.sign("GET", "/b", {}, base, pu.EMPTY_SHA256))
        self.assertNotEqual(one, self.sign("DELETE", "/a", {}, base, pu.EMPTY_SHA256))
        other = pu.sigv4_authorization("GET", "/a", {}, base, pu.EMPTY_SHA256, self.KEY, self.SECRET + "x",
                                       "us-east-1", "s3", self.DATE)
        self.assertNotEqual(one, other)

    def test_uri_encode(self):
        self.assertEqual(pu.uri_encode("a b+c/d~e_f-g.h"), "a%20b%2Bc%2Fd~e_f-g.h")
        self.assertEqual(pu.uri_encode("luneterm/0.9.0-rc.1/x+y", True), "luneterm/0.9.0-rc.1/x%2By")


class FakeTransport:
    """Stands in for the network: records each request, answers from `answers`."""

    def __init__(self, answers=None):
        self.answers = answers or {}
        self.requests = []

    def __call__(self, method, host, path, headers, body_path, limit):
        body = None
        if body_path:
            with open(body_path, "rb") as f:
                body = f.read()
        self.requests.append((method, host, path, dict(headers), body, limit))
        return self.answers.get((method, path), (200, b""))


class R2StoreTests(unittest.TestCase):
    ACCOUNT = "0123456789abcdef0123456789abcdef"

    def store(self, transport, **env):
        values = {"R2_ACCESS_KEY_ID": "AKIDEXAMPLE", "R2_SECRET_ACCESS_KEY": "s3cr3t-value", "GITHUB_TOKEN": "ghs_x",
                  "LUNEPACK_RELEASE_KEY": "relkey", "CLOUDFLARE_ACCOUNT_ID": self.ACCOUNT}
        values.update(env)
        secrets = pu.Secrets(values)
        return pu.R2Store(secrets, tempfile.gettempdir(), transport=transport, clock=lambda: (2026, 11, 26, 5, 23, 0, 0, 0, 0))

    def test_a_put_is_signed_streams_the_file_and_names_the_host_and_bucket(self):
        transport = FakeTransport()
        with tempfile.TemporaryDirectory() as tmp:
            src = os.path.join(tmp, "f.pack")
            pathlib.Path(src).write_bytes(b"pack bytes")
            self.store(transport).put("luneterm/0.9.0/linux-x86_64.pack", src, "application/octet-stream", pu.IMMUTABLE)
        method, host, path, headers, body, _ = transport.requests[0]
        self.assertEqual((method, host), ("PUT", self.ACCOUNT + ".r2.cloudflarestorage.com"))
        self.assertEqual(path, "/izzat-updates/luneterm/0.9.0/linux-x86_64.pack")
        self.assertEqual(body, b"pack bytes")
        self.assertEqual(headers["x-amz-content-sha256"], pu.hashlib.sha256(b"pack bytes").hexdigest())
        self.assertEqual(headers["x-amz-date"], "20261126T052300Z")
        self.assertEqual(headers["Content-Length"], "10")
        self.assertEqual(headers["cache-control"], pu.IMMUTABLE)
        self.assertIn("Credential=AKIDEXAMPLE/20261126/auto/s3/aws4_request", headers["Authorization"])
        self.assertIn("SignedHeaders=cache-control;content-type;host;x-amz-content-sha256;x-amz-date", headers["Authorization"])
        self.assertNotIn("s3cr3t-value", json.dumps(headers))

    def test_the_signature_is_the_one_the_signer_gives(self):
        transport = FakeTransport()
        self.store(transport).get_doc("luneterm/keys.json")
        _, host, path, headers, _, _ = transport.requests[0]
        signed = {"host": host, "x-amz-content-sha256": pu.EMPTY_SHA256, "x-amz-date": "20261126T052300Z"}
        self.assertEqual(headers["Authorization"], pu.sigv4_authorization(
            "GET", path, {}, signed, pu.EMPTY_SHA256, "AKIDEXAMPLE", "s3cr3t-value", "auto", "s3", "20261126T052300Z"))

    def test_get_answers(self):
        key = "/izzat-updates/luneterm/keys.json"
        transport = FakeTransport({("GET", key): (200, b"doc")})
        self.assertEqual(self.store(transport).get_doc("luneterm/keys.json"), b"doc")
        transport = FakeTransport({("GET", key): (404, b"<Error><Code>NoSuchKey</Code></Error>")})
        self.assertIsNone(self.store(transport).get_doc("luneterm/keys.json"))
        transport = FakeTransport({("GET", key): (403, b"<Error><Code>AccessDenied</Code><Message>AKIDEXAMPLE</Message></Error>")})
        with self.assertRaises(pu.Abort) as ctx:
            self.store(transport).get_doc("luneterm/keys.json")
        self.assertIn("HTTP 403 AccessDenied", str(ctx.exception))
        self.assertNotIn("AKIDEXAMPLE", str(ctx.exception))
        transport = FakeTransport({("GET", key): (200, b"x" * (pu.MAX_DOC_BYTES + 1))})
        with self.assertRaises(pu.Abort):
            self.store(transport).get_doc("luneterm/keys.json")

    def test_state_and_pending_live_in_the_private_bucket(self):
        transport = FakeTransport({("GET", "/izzat-updates-publisher/luneterm/lunepack-state.json"): (200, b"{}")})
        store = self.store(transport)
        self.assertEqual(store.get_state(), b"{}")
        store.get_pending()
        self.assertEqual(transport.requests[-1][2], "/izzat-updates-publisher/luneterm/pending-deletes.json")
        with tempfile.TemporaryDirectory() as tmp:
            src = os.path.join(tmp, "s")
            pathlib.Path(src).write_bytes(b"{}")
            store.put_state(src)
            store.put_pending(src)
        self.assertEqual([r[2] for r in transport.requests[-2:]],
                         ["/izzat-updates-publisher/luneterm/lunepack-state.json",
                          "/izzat-updates-publisher/luneterm/pending-deletes.json"])
        self.assertEqual(transport.requests[-1][3]["cache-control"], "no-store")

    def test_delete_accepts_204_and_refuses_anything_that_is_not_a_version_object(self):
        key = "luneterm/0.1.0/linux-x86_64.pack"
        transport = FakeTransport({("DELETE", "/izzat-updates/" + key): (204, b"")})
        store = self.store(transport)
        store.delete(key)
        with self.assertRaises(pu.Abort):
            store.delete("luneterm/feed-stable.json")
        self.assertEqual(len(transport.requests), 1)
        transport = FakeTransport({("DELETE", "/izzat-updates/" + key): (500, b"<Code>InternalError</Code>")})
        with self.assertRaises(pu.Abort) as ctx:
            self.store(transport).delete(key)
        self.assertIn("HTTP 500 InternalError", str(ctx.exception))

    def test_a_put_that_is_refused_aborts(self):
        transport = FakeTransport({("PUT", "/izzat-updates/luneterm/0.9.0/a.pack"): (403, b"<Code>SignatureDoesNotMatch</Code>")})
        with tempfile.TemporaryDirectory() as tmp:
            src = os.path.join(tmp, "f")
            pathlib.Path(src).write_bytes(b"x")
            with self.assertRaises(pu.Abort) as ctx:
                self.store(transport).put("luneterm/0.9.0/a.pack", src, "application/octet-stream", pu.IMMUTABLE)
        self.assertIn("SignatureDoesNotMatch", str(ctx.exception))

    def test_a_missing_credential_or_a_bad_account_aborts(self):
        for env in ({"R2_ACCESS_KEY_ID": ""}, {"R2_SECRET_ACCESS_KEY": ""}, {"CLOUDFLARE_ACCOUNT_ID": ""},
                    {"CLOUDFLARE_ACCOUNT_ID": "evil.example/x"}):
            with self.assertRaises(pu.Abort, msg=str(env)):
                self.store(FakeTransport(), **env)

    def test_the_publisher_runs_no_npm_tool(self):
        text = (HERE / "publish-updates.py").read_text()
        workflow = (HERE.parent / ".github" / "workflows" / "publish-updates.yml").read_text()
        for needle in ("wrangler", "pnpm", "npm ", "npx", "setup-node"):
            # The docstring and the comments may say what is NOT there; code and steps may not run it.
            code = [line for line in (text + workflow).splitlines() if needle in line and not line.lstrip().startswith("#")]
            self.assertEqual([line for line in code if "no npm tool" not in line and "no wrangler" not in line], [], needle)


class ScheduledRefresh(unittest.TestCase):
    """F5: the weekly schedule is skipped, not red, while nothing can be published yet."""

    def run_main(self, tmp, extra, store_dir=None):
        argv = ["refresh", "--lunepack", "/nonexistent", "--work", os.path.join(tmp, "w")] + extra
        if store_dir:
            argv += ["--store-dir", store_dir]
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = pu.main(argv)
        return code, out.getvalue(), err.getvalue()

    def test_the_test_root_skips_the_schedule_but_not_a_dispatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            code, out, _ = self.run_main(tmp, ["--skip-if-unpublished"])
            self.assertEqual(code, 0)
            self.assertIn("::notice::scheduled refresh skipped", out)
            self.assertFalse(os.path.exists(os.path.join(tmp, "w")))
            code, _, err = self.run_main(tmp, [])
            self.assertEqual(code, 1)
            self.assertIn("test root", err)

    def test_an_empty_bucket_skips_the_schedule_but_not_a_dispatch(self):
        saved = pu.refuse_test_root
        pu.refuse_test_root = lambda: None
        try:
            with tempfile.TemporaryDirectory() as tmp:
                store = os.path.join(tmp, "store")
                os.makedirs(store)
                code, out, _ = self.run_main(tmp, ["--skip-if-unpublished"], store)
                self.assertEqual(code, 0)
                self.assertIn("has no luneterm/keys.json", out)
                os.rename(os.path.join(tmp, "w"), os.path.join(tmp, "w1"))
                code, _, err = self.run_main(tmp, [], store)
                self.assertEqual(code, 1)
                self.assertIn("keys.json", err)
        finally:
            pu.refuse_test_root = saved


class SetupConfig(unittest.TestCase):
    def test_each_setup_uses_its_own_oss_config_and_the_versions_channel(self):
        lp = pu.Lunepack("lunepack", None, "/cfg")
        win = pu.setup_argv(lp, "/rd", "0.9.0", "windows-x86_64", "/out/s.exe", "stable")
        self.assertEqual(win[win.index("--config") + 1], "/cfg/lunepack-windows-stable.toml")
        self.assertNotIn("--zip", win)
        mac = pu.setup_argv(lp, "/rd", "0.9.0-rc.1", "macos-aarch64", "/out/s.zip", "beta")
        self.assertEqual(mac[mac.index("--config") + 1], "/cfg/lunepack-macos-beta.toml")
        self.assertIn("--zip", mac)
        lin = pu.setup_argv(lp, "/rd", "0.9.0", "linux-x86_64", "/out/s", "stable")
        self.assertEqual(lin[lin.index("--config") + 1], "/cfg/lunepack-linux-stable.toml")


class Secrets(unittest.TestCase):
    def test_secrets_leave_the_environment_and_are_redacted(self):
        env = {"LUNEPACK_RELEASE_KEY": "-----BEGIN PRIVATE KEY-----abc", "R2_ACCESS_KEY_ID": "akid",
               "R2_SECRET_ACCESS_KEY": "tok123", "GITHUB_TOKEN": "ghs_x", "CLOUDFLARE_ACCOUNT_ID": "acc",
               "CLOUDFLARE_API_TOKEN": "other", "AWS_SECRET_ACCESS_KEY": "aws", "PATH": "/bin"}
        s = pu.Secrets(env)
        self.assertEqual(env, {"PATH": "/bin"})
        self.assertEqual(s.account, "acc")
        self.assertEqual(s.redact("a tok123 b ghs_x"), "a *** b ***")
        with self.assertRaises(pu.Abort):
            pu.Secrets({}).get("R2_SECRET_ACCESS_KEY")

    def test_child_env_carries_no_secret(self):
        saved = dict(os.environ)
        try:
            os.environ["GITHUB_TOKEN"] = "x"
            os.environ["CF_API_TOKEN"] = "y"
            env = pu.child_env({"SOURCE_DATE_EPOCH": "1"})
            self.assertNotIn("GITHUB_TOKEN", env)
            self.assertNotIn("CF_API_TOKEN", env)
            self.assertEqual(env["SOURCE_DATE_EPOCH"], "1")
        finally:
            os.environ.clear()
            os.environ.update(saved)


class TestRoot(unittest.TestCase):
    def test_the_repo_today_refuses_to_publish(self):
        # The committed configs and root.rs are the test root (D9): publishing must refuse.
        with self.assertRaises(pu.Abort) as ctx:
            pu.refuse_test_root()
        self.assertIn("test root", str(ctx.exception))

    def test_a_real_root_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = pathlib.Path(tmp)
            (repo / "packaging").mkdir()
            for os_name in ("linux", "windows", "macos"):
                text = (HERE.parent / "packaging" / ("lunepack-%s.toml" % os_name)).read_text()
                (repo / "packaging" / ("lunepack-%s.toml" % os_name)).write_text(
                    text.replace(next(iter(pu.TEST_ROOT_KEYS)), "ab" * 32))
            rs = repo / "crates" / "luneterm" / "src" / "update"
            rs.mkdir(parents=True)
            (rs / "root.rs").write_text("pub const IS_TEST_ROOT: bool = false;\npub const ROOT_KEYS: &[[u8; 32]] = &[REAL];\n")
            saved = pu.REPO
            pu.REPO = str(repo)
            try:
                pu.refuse_test_root()
                (rs / "root.rs").write_text("pub const IS_TEST_ROOT: bool = true;\n")
                with self.assertRaises(pu.Abort):
                    pu.refuse_test_root()
            finally:
                pu.REPO = saved


class Extract(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def tar(self, members):
        path = os.path.join(self.tmp, "in.tar.gz")
        with tarfile.open(path, "w:gz") as tar:
            for info, data in members:
                tar.addfile(info, io.BytesIO(data) if data is not None else None)
        return path

    def reg(self, name, data=b"x", mode=0o644):
        info = tarfile.TarInfo(name)
        info.size = len(data)
        info.mode = mode
        return info, data

    def test_a_good_tar_keeps_the_exec_bit(self):
        d = tarfile.TarInfo("tree")
        d.type = tarfile.DIRTYPE
        path = self.tar([(d, None), self.reg("tree/luneterm", b"exe", 0o755), self.reg("BUILD-INFO.txt", b"format=1\n")])
        dest = os.path.join(self.tmp, "out")
        pu.extract_input(path, dest)
        self.assertTrue(os.access(os.path.join(dest, "tree", "luneterm"), os.X_OK))
        self.assertFalse(os.access(os.path.join(dest, "BUILD-INFO.txt"), os.X_OK))

    def test_unsafe_tars_are_refused(self):
        link = tarfile.TarInfo("tree/l")
        link.type = tarfile.SYMTYPE
        link.linkname = "/etc/passwd"
        hard = tarfile.TarInfo("tree/h")
        hard.type = tarfile.LNKTYPE
        hard.linkname = "tree/a"
        dev = tarfile.TarInfo("tree/d")
        dev.type = tarfile.CHRTYPE
        cases = [[(link, None)], [(hard, None)], [(dev, None)], [self.reg("../x")], [self.reg("/abs")],
                 [self.reg("tree/../../x")], [self.reg("a\\b")], [self.reg("t/x"), self.reg("t/x")]]
        for i, members in enumerate(cases):
            path = self.tar(members)
            with self.assertRaises(pu.Abort, msg=str(i)):
                pu.extract_input(path, os.path.join(self.tmp, "out%d" % i))
            os.remove(path)

    def test_zip_good_and_bad(self):
        path = os.path.join(self.tmp, "in.zip")
        with zipfile.ZipFile(path, "w") as z:
            z.writestr("BUILD-INFO.txt", "format=1\n")
            z.writestr("tree/luneterm.exe", "MZ")
        pu.extract_input(path, os.path.join(self.tmp, "ok"))
        self.assertTrue(os.path.isfile(os.path.join(self.tmp, "ok", "tree", "luneterm.exe")))
        for i, name in enumerate(["tree\\luneterm.exe", "../x", "C:/x", "/x"]):
            bad = os.path.join(self.tmp, "bad%d.zip" % i)
            with zipfile.ZipFile(bad, "w") as z:
                z.writestr(name, "x")
            with self.assertRaises(pu.Abort, msg=name):
                pu.extract_input(bad, os.path.join(self.tmp, "b%d" % i))
        bad = os.path.join(self.tmp, "link.zip")
        with zipfile.ZipFile(bad, "w") as z:
            info = zipfile.ZipInfo("tree/l")
            info.external_attr = (0o120777) << 16
            z.writestr(info, "/etc/passwd")
        with self.assertRaises(pu.Abort):
            pu.extract_input(bad, os.path.join(self.tmp, "bl"))


class BuildInfo(unittest.TestCase):
    def info(self, **over):
        base = dict(pu.TARGETS["linux-x86_64"]["info"], format="1", version="0.9.0", channel="stable",
                    lunepack_rev="a" * 40, source_date_epoch="1700000000", glibc_min="2.39")
        base.update(over)
        return {k: v for k, v in base.items() if v is not None}

    def test_exact_values_pass(self):
        self.assertEqual(pu.check_build_info(self.info(), "linux-x86_64", "0.9.0", "stable", "a" * 40), 1700000000)

    def test_anything_else_fails(self):
        for over in ({"channel": "beta"}, {"version": "0.9.1"}, {"lunepack_rev": "b" * 40}, {"extra": "1"},
                     {"runtime": "../x"}, {"source_date_epoch": "x"}, {"target": "windows-x86_64"}, {"tree": None}):
            with self.assertRaises(pu.Abort, msg=str(over)):
                pu.check_build_info(self.info(**over), "linux-x86_64", "0.9.0", "stable", "a" * 40)

    def test_sidecar(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = os.path.join(tmp, "f")
            pathlib.Path(p).write_bytes(b"data")
            good = pu.hashlib.sha256(b"data").hexdigest() + "  f\n"
            pu.check_sidecar(p, good.encode(), "f")
            with self.assertRaises(pu.Abort):
                pu.check_sidecar(p, good.replace("  f", "  g").encode(), "f")


class PlanCommand(unittest.TestCase):
    def test_plan_on_a_local_release_dir(self):
        with tempfile.TemporaryDirectory() as tmp:
            rd = pathlib.Path(tmp)
            stable = versions(11)
            beta = stable + ["0.2.0-rc.1"]
            for channel, vs in (("stable", stable), ("beta", beta)):
                releases = [{"version": v, "targets": {"linux-x86_64": {"path": "%s/linux-x86_64.manifest.json" % v}}} for v in vs]
                (rd / ("feed-%s.json" % channel)).write_bytes(
                    envelope("feed", {"format": 1, "kind": "feed", "channel": channel, "releases": releases}))
            for v in beta:
                (rd / v).mkdir()
                (rd / v / "linux-x86_64.manifest.json").write_bytes(envelope("manifest", manifest(v, "linux-x86_64")))
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                self.assertEqual(pu.main(["plan", "--release-dir", str(rd)]), 0)
            text = out.getvalue()
            self.assertIn("stable: drop ['0.1.0']", text)
            self.assertIn("delete luneterm/0.1.0/linux-x86_64.pack", text)
            self.assertIn("delete luneterm/0.1.0/linux-x86_64.manifest.json", text)
            self.assertNotIn("delete luneterm/0.1.1", text)
            # A manifest missing from the directory: nothing is planned for deletion.
            (rd / "0.1.0" / "linux-x86_64.manifest.json").unlink()
            err = io.StringIO()
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
                self.assertEqual(pu.main(["plan", "--release-dir", str(rd)]), 1)
            self.assertIn("the manifest is missing", err.getvalue())


if __name__ == "__main__":
    unittest.main()
