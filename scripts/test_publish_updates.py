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
    def __init__(self, feeds=None, fail_put=None):
        self.calls = []
        self.feeds = dict(feeds or {})
        self.fail_put = fail_put

    def get_doc(self, key):
        return self.feeds.get(key)

    def put(self, key, src, content_type, cache_control):
        if key == self.fail_put:
            raise pu.Abort("upload of %s failed" % key)
        self.calls.append(("put", key, content_type, cache_control))

    def put_state(self, src):
        self.calls.append(("state",))

    def delete(self, key):
        self.calls.append(("delete", key))


class Commit(unittest.TestCase):
    uploads = ["0.9.0/linux-x86_64.pack", "0.9.0/linux-x86_64.manifest.json"]
    deletes = ["luneterm/0.1.0/linux-x86_64.pack", "luneterm/0.1.0/linux-x86_64.manifest.json"]

    def run_commit(self, store, apply=True, feeds=None):
        feeds = {"stable": b"old-stable", "beta": None} if feeds is None else feeds
        out = []
        pu.commit(store, "/rd", self.uploads, feeds, "/state", self.deletes, apply, log=out.append)
        return out

    def test_order_uploads_feeds_state_deletes(self):
        store = FakeStore({"luneterm/feed-stable.json": b"old-stable"})
        self.run_commit(store)
        kinds = [c[0] if c[0] != "put" else c[1] for c in store.calls]
        self.assertEqual(
            kinds,
            ["luneterm/0.9.0/linux-x86_64.pack", "luneterm/0.9.0/linux-x86_64.manifest.json",
             "luneterm/feed-beta.json", "luneterm/feed-stable.json", "state", "delete", "delete"],
        )
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
        self.assertFalse([c for c in store.calls if "feed" in c[1] or c[0] != "put"])

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


class Secrets(unittest.TestCase):
    def test_secrets_leave_the_environment_and_are_redacted(self):
        env = {"LUNEPACK_RELEASE_KEY": "-----BEGIN PRIVATE KEY-----abc", "R2_UPDATES_TOKEN": "tok123",
               "GITHUB_TOKEN": "ghs_x", "CLOUDFLARE_ACCOUNT_ID": "acc", "CLOUDFLARE_API_TOKEN": "other", "PATH": "/bin"}
        s = pu.Secrets(env)
        self.assertEqual(env, {"PATH": "/bin"})
        self.assertEqual(s.account, "acc")
        self.assertEqual(s.redact("a tok123 b ghs_x"), "a *** b ***")
        with self.assertRaises(pu.Abort):
            pu.Secrets({}).get("R2_UPDATES_TOKEN")

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
