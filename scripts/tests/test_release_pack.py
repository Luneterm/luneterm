"""Tests of scripts/release-pack.py that need no network, no key and no real lunepack.

    python3 -B -m unittest discover -s scripts/tests -v
"""

import argparse
import base64
import contextlib
import hashlib
import importlib.util
import io
import json
import os
import pathlib
import re
import shutil
import stat
import sys
import tarfile
import tempfile
import tomllib
import unittest
import zipfile
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
REPO = SCRIPTS.parent

_spec = importlib.util.spec_from_file_location("release_pack", SCRIPTS / "release-pack.py")
rp = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(rp)

REAL_HTTP_GET = rp.http_get
REAL_ROOT = "ab" * 32
KEY = "-----BEGIN PRIVATE KEY-----KEYVALUE"
TOKEN = "ghs_TOKENVALUE"
EPOCH = 1700000000


def sha(data):
    return hashlib.sha256(data).hexdigest()


def run_quiet(fn, *args, **kwargs):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        try:
            result = fn(*args, **kwargs)
        except BaseException:
            sys.stdout.write("")
            raise
    return result, out.getvalue() + err.getvalue()


# ---------------------------------------------------------------------------- the root and the config


def repo_config_text(key):
    """packaging/lunepack.toml with its single [root] key replaced by `key`."""
    text = (REPO / "packaging" / "lunepack.toml").read_text()
    out, n = re.subn(r'(\[root\][^\[]*?keys = \[\s*)"[0-9a-f]{64}"', lambda m: m.group(1) + '"%s"' % key, text, count=1)
    assert n == 1
    return out


def read_config(path=None):
    with open(path or REPO / "packaging" / "lunepack.toml", "rb") as f:
        return tomllib.load(f)


class CheckRoot(unittest.TestCase):
    def config_with(self, tmp, key):
        out = pathlib.Path(tmp, "lunepack.toml")
        out.write_text(repo_config_text(key))
        return str(out)

    def test_a_config_with_the_test_root_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(rp.Abort) as ctx:
                rp.check_root(self.config_with(tmp, next(iter(rp.TEST_ROOT_KEYS))))
        self.assertIn("names the test root key; refusing to publish", str(ctx.exception))

    def test_the_repo_config_holds_a_real_root(self):
        rp.check_root(str(REPO / "packaging" / "lunepack.toml"))

    def test_a_real_root_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            rp.check_root(self.config_with(tmp, REAL_ROOT))

    def test_a_test_key_among_real_ones_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self.config_with(tmp, REAL_ROOT)
            text = pathlib.Path(path).read_text().replace(
                '"%s",' % REAL_ROOT, '"%s",\n    "%s",' % (REAL_ROOT, next(iter(rp.TEST_ROOT_KEYS)))
            )
            pathlib.Path(path).write_text(text)
            with self.assertRaises(rp.Abort) as ctx:
                rp.check_root(path)
            self.assertIn("test root", str(ctx.exception))

    def test_an_unreadable_or_rootless_config_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(rp.Abort) as ctx:
                rp.check_root(os.path.join(tmp, "absent.toml"))
            self.assertIn("cannot read [root] keys", str(ctx.exception))
            bare = pathlib.Path(tmp, "bare.toml")
            bare.write_text('app_id = "x"\n')
            with self.assertRaises(rp.Abort):
                rp.check_root(str(bare))
            empty = pathlib.Path(tmp, "empty.toml")
            empty.write_text("[root]\nkeys = []\n")
            with self.assertRaises(rp.Abort) as ctx:
                rp.check_root(str(empty))
            self.assertIn("not a list of keys", str(ctx.exception))

    def test_the_command_exits_1_with_the_reason_and_0_for_a_real_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            good = self.config_with(tmp, REAL_ROOT)
            code, out = run_quiet(rp.main, ["check-root", "--config", good])
            self.assertEqual(code, 0, out)
            code, out = run_quiet(rp.main, ["check-root", "--config", self.config_with(tmp, next(iter(rp.TEST_ROOT_KEYS)))])
            self.assertEqual(code, 1)
            self.assertIn("test root", out)


class PackagingConfig(unittest.TestCase):
    """The one packaging/lunepack.toml agrees with what the release jobs build (the guard root.rs
    had until the compiled root went)."""

    def test_the_effective_entry_of_each_os_is_the_one_build_info_names(self):
        c = read_config()
        for target, spec in rp.TARGETS.items():
            table = c.get("install", {})
            effective = table.get(spec["os"], {}).get("entry", table.get("entry"))
            self.assertEqual(effective, spec["info"]["entry"], target)
            self.assertEqual(spec["info"]["config"], "packaging/lunepack.toml", target)

    def test_the_icons_exist_where_the_config_says(self):
        c = read_config()
        for key in ("windows", "macos"):
            icon = REPO / "packaging" / c[key]["icon"]  # relative to the config
            self.assertTrue(icon.is_file(), "[%s] icon %s" % (key, icon))
        # The Linux icon is a file of the tree: release.yml stages it from assets/icon.
        linux_icon = c["install"]["linux"]["icon"]
        self.assertNotIn("icon", {k: v for k, v in c["install"].items() if not isinstance(v, dict)})
        staged = re.findall(
            r'^\s*cp assets/icon/(\S+) "\$stage/tree/(\S+)"$', (REPO / ".github" / "workflows" / "release.yml").read_text(), re.M
        )
        self.assertIn((linux_icon, linux_icon), staged)
        self.assertTrue((REPO / "assets" / "icon" / linux_icon).is_file())

    def test_the_config_holds_no_key_and_one_environment_signer(self):
        text = (REPO / "packaging" / "lunepack.toml").read_text()
        self.assertNotIn("PRIVATE KEY", text)  # a PEM header; the R2_PRIVATE_* names are variable names
        self.assertNotIn("BEGIN", text)
        c = read_config()
        self.assertEqual(c["signers"], {"release": {"env": "LUNEPACK_RELEASE_KEY"}})
        self.assertEqual(c["app_id"], "my.ijat.luneterm")
        self.assertEqual(c["update"]["feed"], "https://updates.izzat.dev/luneterm/")
        self.assertEqual(c["update"]["channel"], "stable")

    def test_publish_names_the_targets_this_script_packs(self):
        p = read_config()["publish"]
        self.assertEqual(sorted(p["require_targets"]), sorted(rp.TARGETS))
        self.assertIn(read_config()["update"]["channel"], p["channels"])
        self.assertEqual(p["prerelease_channels"], ["beta"])
        # Names of environment variables, never values.
        for key in ("account_id_env", "access_key_env", "secret_key_env", "private_access_key_env", "private_secret_key_env"):
            self.assertRegex(p[key], r"^[A-Z][A-Z0-9_]*$", key)


# ------------------------------------------------------------------------------------- extraction


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
        rp.extract_input(path, dest)
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
            with self.assertRaises(rp.Abort, msg=str(i)):
                rp.extract_input(path, os.path.join(self.tmp, "out%d" % i))
            os.remove(path)

    def test_zip_good_and_bad(self):
        path = os.path.join(self.tmp, "in.zip")
        with zipfile.ZipFile(path, "w") as z:
            z.writestr("BUILD-INFO.txt", "format=1\n")
            z.writestr("tree/luneterm.exe", "MZ")
        rp.extract_input(path, os.path.join(self.tmp, "ok"))
        self.assertTrue(os.path.isfile(os.path.join(self.tmp, "ok", "tree", "luneterm.exe")))
        for i, name in enumerate(["tree\\luneterm.exe", "../x", "C:/x", "/x"]):
            bad = os.path.join(self.tmp, "bad%d.zip" % i)
            with zipfile.ZipFile(bad, "w") as z:
                z.writestr(name, "x")
            with self.assertRaises(rp.Abort, msg=name):
                rp.extract_input(bad, os.path.join(self.tmp, "b%d" % i))
        bad = os.path.join(self.tmp, "link.zip")
        with zipfile.ZipFile(bad, "w") as z:
            info = zipfile.ZipInfo("tree/l")
            info.external_attr = (0o120777) << 16
            z.writestr(info, "/etc/passwd")
        with self.assertRaises(rp.Abort):
            rp.extract_input(bad, os.path.join(self.tmp, "bl"))

    def test_an_unknown_format_is_refused(self):
        path = os.path.join(self.tmp, "in.rar")
        pathlib.Path(path).write_bytes(b"x")
        with self.assertRaises(rp.Abort) as ctx:
            rp.extract_input(path, os.path.join(self.tmp, "out"))
        self.assertIn("unknown build input format", str(ctx.exception))


class BuildInfo(unittest.TestCase):
    def info(self, **over):
        base = dict(rp.TARGETS["linux-x86_64"]["info"], format="1", version="0.9.0", channel="stable",
                    lunepack_rev="a" * 40, source_date_epoch="1700000000", glibc_min="2.39")
        base.update(over)
        return {k: v for k, v in base.items() if v is not None}

    def test_exact_values_pass(self):
        self.assertEqual(rp.check_build_info(self.info(), "linux-x86_64", "0.9.0", "stable", "a" * 40), 1700000000)

    def test_anything_else_fails(self):
        for over in ({"channel": "beta"}, {"version": "0.9.1"}, {"lunepack_rev": "b" * 40}, {"extra": "1"},
                     {"runtime": "../x"}, {"source_date_epoch": "x"}, {"target": "windows-x86_64"}, {"tree": None},
                     {"config": "packaging/lunepack-linux.toml"}):
            with self.assertRaises(rp.Abort, msg=str(over)):
                rp.check_build_info(self.info(**over), "linux-x86_64", "0.9.0", "stable", "a" * 40)

    def test_sidecar(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = os.path.join(tmp, "f")
            pathlib.Path(p).write_bytes(b"data")
            good = sha(b"data") + "  f\n"
            rp.check_sidecar(p, good.encode(), "f")
            with self.assertRaises(rp.Abort) as ctx:
                rp.check_sidecar(p, good.replace("  f", "  g").encode(), "f")
            self.assertIn("does not match its .sha256", str(ctx.exception))


class Secrets(unittest.TestCase):
    def test_secrets_leave_the_environment_and_are_redacted(self):
        env = {"LUNEPACK_RELEASE_KEY": KEY, "GITHUB_TOKEN": TOKEN, "R2_SECRET_ACCESS_KEY": "tok123", "R2_TOKEN": "t",
               "CLOUDFLARE_API_TOKEN": "other", "AWS_SECRET_ACCESS_KEY": "aws", "PATH": "/bin"}
        s = rp.Secrets(env)
        self.assertEqual(env, {"PATH": "/bin"})
        self.assertEqual(s.redact("a %s b %s" % (KEY, TOKEN)), "a *** b ***")
        with self.assertRaises(rp.Abort) as ctx:
            rp.Secrets({}).get("LUNEPACK_RELEASE_KEY")
        self.assertEqual(str(ctx.exception), "LUNEPACK_RELEASE_KEY is not set")

    def test_child_env_carries_no_secret(self):
        with mock.patch.dict(os.environ, {"GITHUB_TOKEN": "x", "CF_API_TOKEN": "y", "R2_ACCESS_KEY_ID": "z",
                                          "LUNEPACK_RELEASE_KEY": "k"}):
            env = rp.child_env({"SOURCE_DATE_EPOCH": "1"})
        for name in ("GITHUB_TOKEN", "CF_API_TOKEN", "R2_ACCESS_KEY_ID", "LUNEPACK_RELEASE_KEY"):
            self.assertNotIn(name, env)
        self.assertEqual(env["SOURCE_DATE_EPOCH"], "1")


# ------------------------------------------------------------------------------------------ pack


FAKE_LUNEPACK = r"""#!%(python)s
import base64, json, os, sys
argv = sys.argv[1:]
log = os.environ["FAKE_LOG"]
sub = argv[1] if argv[0] == "publish" else argv[0]
rec = {"argv": argv, "sub": sub, "epoch": os.environ.get("SOURCE_DATE_EPOCH"),
       "has_key": "LUNEPACK_RELEASE_KEY" in os.environ, "has_token": "GITHUB_TOKEN" in os.environ,
       "has_r2": any(k.startswith("R2_") for k in os.environ)}
with open(log, "a") as f:
    f.write(json.dumps(rec) + "\n")
def opt(name):
    return argv[argv.index(name) + 1]
if os.environ.get("FAKE_FAIL") == sub:
    print("boom " + os.environ.get("LUNEPACK_RELEASE_KEY", ""))
    sys.exit(3)
if "LUNEPACK_RELEASE_KEY" in os.environ:
    print("signing with " + os.environ["LUNEPACK_RELEASE_KEY"])  # a leak the script must redact
if sub == "fetch":
    os.makedirs(opt("--release-dir"))
elif sub == "pack":
    d = os.path.join(opt("--release-dir"), opt("--version"))
    os.makedirs(d, exist_ok=True)
    paths = [] if os.environ.get("FAKE_NO_NOTICES") else ["lunepack-NOTICES.txt", "Contents/Resources/lunepack-NOTICES.txt"]
    payload = {"kind": "manifest", "format": 1, "entries": [{"path": p} for p in paths]}
    env = {"payloadType": "application/vnd.lunepack.manifest.v1+json",
           "payload": base64.b64encode(json.dumps(payload).encode()).decode(), "signatures": []}
    with open(os.path.join(d, opt("--target") + ".manifest.json"), "w") as f:
        json.dump(env, f)
elif sub == "setup":
    with open(opt("--out"), "w") as f:
        f.write("setup-" + opt("--target"))
"""


def build_inputs(directory, version, epoch=EPOCH, rev=None, channel=None, override=None):
    """The three build inputs and their .sha256 as the release jobs write them."""
    rev = rev or rp.read_pin()
    channel = channel or rp.channel_of(version)
    override = override or {}
    os.makedirs(directory, exist_ok=True)
    for target, spec in rp.TARGETS.items():
        info = dict(spec["info"], format="1", version=version, channel=channel, lunepack_rev=rev,
                    source_date_epoch=str(epoch))
        info.update(override.get(target, {}))
        files = {"BUILD-INFO.txt": "\n".join("%s=%s" % kv for kv in info.items()) + "\n"}
        files["tree/" + info["entry"]] = "app"
        for key in ("runtime", "ui", "stub"):
            if key in info:
                files[info[key]] = key
        name = spec["input"].format(v=version)
        path = os.path.join(directory, name)
        if name.endswith(".zip"):
            with zipfile.ZipFile(path, "w") as z:
                for n, data in files.items():
                    z.writestr(n, data)
        else:
            with tarfile.open(path, "w:gz") as tar:
                for n, data in files.items():
                    raw = data.encode()
                    ti = tarfile.TarInfo(n)
                    ti.size = len(raw)
                    ti.mode = 0o755
                    tar.addfile(ti, io.BytesIO(raw))
        with open(path, "rb") as f:
            digest = sha(f.read())
        with open(path + ".sha256", "w") as f:
            f.write("%s  %s\n" % (digest, name))


class Pack(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)
        self.log = os.path.join(self.tmp, "calls.jsonl")
        self.cli = os.path.join(self.tmp, "lunepack")
        pathlib.Path(self.cli).write_text(FAKE_LUNEPACK % {"python": sys.executable})
        os.chmod(self.cli, 0o755)
        self.config = os.path.join(self.tmp, "lunepack.toml")
        pathlib.Path(self.config).write_text(repo_config_text(REAL_ROOT))
        self.inputs = os.path.join(self.tmp, "inputs")
        self.env = mock.patch.dict(os.environ, {"FAKE_LOG": self.log, "R2_SECRET_ACCESS_KEY": "r2value",
                                                "GITHUB_TOKEN": "inherited"})
        self.env.start()
        self.addCleanup(self.env.stop)
        clock = mock.patch.object(rp, "tag_commit_time", lambda: EPOCH)
        clock.start()
        self.addCleanup(clock.stop)

    def args(self, tag="v0.9.0", work="work", **over):
        base = dict(tag=tag, lunepack=self.cli, config=self.config, work=os.path.join(self.tmp, work),
                    inputs_dir=self.inputs, output_file=None)
        base.update(over)
        return argparse.Namespace(**base)

    def secrets(self):
        return rp.Secrets({"LUNEPACK_RELEASE_KEY": KEY, "GITHUB_TOKEN": TOKEN})

    def pack(self, **over):
        return run_quiet(rp.cmd_pack, self.args(**over), self.secrets())

    def calls(self):
        if not os.path.exists(self.log):
            return []
        with open(self.log) as f:
            return [json.loads(line) for line in f]

    def refused(self, message, **over):
        with self.assertRaises(rp.Abort) as ctx:
            self.pack(**over)
        self.assertIn(message, str(ctx.exception))

    def test_it_runs_fetch_then_pack_then_setup_and_reports_each_setup_digest(self):
        build_inputs(self.inputs, "0.9.0")
        _, out = self.pack(output_file=os.path.join(self.tmp, "gh_output"))
        calls = self.calls()
        self.assertEqual([c["sub"] for c in calls], ["fetch"] + ["pack"] * 3 + ["setup"] * 3)
        fetch = calls[0]["argv"]
        self.assertEqual(fetch[:2], ["publish", "fetch"])
        self.assertEqual(fetch[fetch.index("--version") + 1], "0.9.0")
        self.assertEqual(fetch[fetch.index("--config") + 1], self.config)
        packed = [c["argv"][c["argv"].index("--target") + 1] for c in calls[1:4]]
        self.assertEqual(sorted(packed), sorted(rp.TARGETS))
        for c in calls[1:4]:
            a = c["argv"]
            self.assertEqual(a[a.index("--sign") + 1], "release")
            self.assertEqual(a[a.index("--config") + 1], self.config)
            self.assertEqual(a[a.index("--release-dir") + 1], os.path.join(self.tmp, "work", "release"))
            self.assertEqual(("--stub" in a), a[a.index("--target") + 1] == "windows-x86_64")
            self.assertTrue(a[a.index("--runtime") + 1].startswith(os.path.join(self.tmp, "work", "inputs")))
            self.assertNotIn("--channel", a)  # a stable release packs as the config's channel
        for c in calls[4:]:
            a = c["argv"]
            self.assertEqual(("--zip" in a), a[a.index("--target") + 1] == "macos-aarch64")
            self.assertNotIn("--sign", a)
        # The files and the job-output lines.
        lines = [l for l in out.splitlines() if l.startswith("setup_sha256_")]
        self.assertEqual(len(lines), 3)
        for target, spec in rp.TARGETS.items():
            name = spec["setup"].format(v="0.9.0")
            path = os.path.join(self.tmp, "work", "setup", name)
            data = pathlib.Path(path).read_bytes()
            self.assertEqual(data, b"setup-" + target.encode())
            self.assertEqual(pathlib.Path(path + ".sha256").read_text(), "%s  %s\n" % (sha(data), name))
            self.assertIn("setup_sha256_%s=%s" % (target.replace("-", "_"), sha(data)), lines)
        self.assertEqual(
            pathlib.Path(self.tmp, "gh_output").read_text().splitlines(), lines)
        for line in lines:
            self.assertRegex(line, r"^setup_sha256_[a-z0-9_]+=[0-9a-f]{64}$")

    def test_only_the_release_candidate_packs_as_beta(self):
        build_inputs(self.inputs, "0.9.0-rc.1")
        self.pack(tag="v0.9.0-rc.1")
        for c in self.calls():
            a = c["argv"]
            if c["sub"] == "pack":
                self.assertEqual(a[a.index("--channel") + 1], "beta")
            else:
                self.assertNotIn("--channel", a)

    def test_the_epoch_and_the_key_reach_only_the_children_that_need_them(self):
        build_inputs(self.inputs, "0.9.0")
        self.pack()
        for c in self.calls():
            self.assertEqual(c["epoch"], str(EPOCH) if c["sub"] in ("pack", "setup") else None, c["sub"])
            self.assertEqual(c["has_key"], c["sub"] == "pack", c["sub"])
            self.assertFalse(c["has_token"], c["sub"])
            self.assertFalse(c["has_r2"], c["sub"])

    def test_no_secret_reaches_any_output(self):
        build_inputs(self.inputs, "0.9.0")
        _, out = self.pack(output_file=os.path.join(self.tmp, "gh_output"))
        self.assertIn("signing with ***", out)  # the fake leaked it; it was redacted
        self.assertNotIn("KEYVALUE", out)
        self.assertNotIn("TOKENVALUE", out)
        self.assertNotIn("KEYVALUE", pathlib.Path(self.tmp, "gh_output").read_text())
        # A failing child: the message names the command and the exit code, no value.
        shutil.rmtree(os.path.join(self.tmp, "work"))
        os.remove(self.log)
        with mock.patch.dict(os.environ, {"FAKE_FAIL": "pack"}):
            with self.assertRaises(rp.Abort) as ctx:
                self.pack()
        self.assertEqual(str(ctx.exception), "lunepack pack failed (exit 3)")

    def test_the_test_root_refuses_before_anything_runs(self):
        build_inputs(self.inputs, "0.9.0")
        test_config = os.path.join(self.tmp, "test-root.toml")
        pathlib.Path(test_config).write_text(repo_config_text(next(iter(rp.TEST_ROOT_KEYS))))
        with self.assertRaises(rp.Abort) as ctx:
            self.pack(config=test_config)
        self.assertIn("names the test root key; refusing to publish", str(ctx.exception))
        self.assertEqual(self.calls(), [])
        self.assertFalse(os.path.exists(os.path.join(self.tmp, "work")))

    def test_refusals(self):
        build_inputs(self.inputs, "0.9.0")
        self.refused("bad tag", tag="0.9.0")
        self.refused("bad tag", tag="v0.9")
        self.refused("bad tag", tag="v0.9.0-beta.1")
        os.makedirs(os.path.join(self.tmp, "taken"))
        self.refused("exists; give a new work directory", work="taken")
        self.refused("has no luneterm-0.9.1-linux-x64-build.tar.gz (or its .sha256)", tag="v0.9.1")
        self.assertEqual(self.calls(), [])

    def test_a_bad_input_is_refused_before_lunepack_runs(self):
        cases = {
            "wrong channel": ({"linux-x86_64": {"channel": "beta"}}, "BUILD-INFO.txt channel is 'beta', expected 'stable'"),
            "wrong rev": ({"windows-x86_64": {"lunepack_rev": "b" * 40}}, "BUILD-INFO.txt lunepack_rev"),
            "old config": ({"macos-aarch64": {"config": "packaging/lunepack-macos.toml"}}, "BUILD-INFO.txt config"),
            "unexpected key": ({"linux-x86_64": {"extra": "1"}}, "unexpected keys"),
        }
        for label, (override, message) in cases.items():
            shutil.rmtree(self.inputs, ignore_errors=True)
            build_inputs(self.inputs, "0.9.0", override=override)
            with self.subTest(label):
                self.refused(message, work="w-" + label.replace(" ", "-"))
        self.assertEqual(self.calls(), [])

    def test_a_modified_input_fails_its_sha256(self):
        build_inputs(self.inputs, "0.9.0")
        path = os.path.join(self.inputs, rp.TARGETS["linux-x86_64"]["input"].format(v="0.9.0"))
        with open(path, "ab") as f:
            f.write(b"x")
        self.refused("does not match its .sha256")
        self.assertEqual(self.calls(), [])

    def test_the_inputs_must_come_from_the_tag_commit_and_from_one_commit(self):
        build_inputs(self.inputs, "0.9.0", epoch=EPOCH + 1)
        self.refused("is not the time of the tag's commit")
        shutil.rmtree(self.inputs)
        build_inputs(self.inputs, "0.9.0", override={"linux-x86_64": {"source_date_epoch": str(EPOCH + 5)}})
        self.refused("different commits", work="w2")
        self.assertEqual(self.calls(), [])

    def test_a_manifest_without_the_notices_is_refused(self):
        build_inputs(self.inputs, "0.9.0")
        with mock.patch.dict(os.environ, {"FAKE_NO_NOTICES": "1"}):
            self.refused("has no lunepack-NOTICES.txt (the lunepack binaries' licences)")

    def test_a_failing_lunepack_stops_the_run(self):
        build_inputs(self.inputs, "0.9.0")
        with mock.patch.dict(os.environ, {"FAKE_FAIL": "fetch"}):
            self.refused("lunepack publish fetch failed (exit 3)")
        self.assertEqual([c["sub"] for c in self.calls()], ["fetch"])

    def test_no_release_key_stops_before_the_first_pack(self):
        build_inputs(self.inputs, "0.9.0")
        with self.assertRaises(rp.Abort) as ctx:
            run_quiet(rp.cmd_pack, self.args(), rp.Secrets({"GITHUB_TOKEN": TOKEN}))
        self.assertEqual(str(ctx.exception), "LUNEPACK_RELEASE_KEY is not set")
        self.assertEqual([c["sub"] for c in self.calls()], ["fetch"])

    def test_output_lines_are_hex_checked(self):
        self.assertEqual(rp.output_line("linux-x86_64", "a" * 64), "setup_sha256_linux_x86_64=" + "a" * 64)
        for target, digest in (("linux-x86_64", "A" * 64), ("linux-x86_64", "a" * 63), ("linux-x86_64", "a" * 64 + "\nX=1"),
                               ("linux-x86_64", ""), ("evil\nX", "a" * 64)):
            with self.assertRaises(rp.Abort, msg=repr((target, digest))):
                rp.output_line(target, digest)

    def test_the_main_command_line(self):
        build_inputs(self.inputs, "0.9.0")
        with mock.patch.dict(os.environ, {"LUNEPACK_RELEASE_KEY": KEY, "GITHUB_TOKEN": TOKEN}):
            code, out = run_quiet(rp.main, ["pack", "--tag", "v0.9.0", "--lunepack", self.cli, "--config", self.config,
                                            "--work", os.path.join(self.tmp, "m"), "--inputs-dir", self.inputs])
            self.assertEqual(code, 0, out)
            # The secrets left the real environment at start.
            self.assertNotIn("LUNEPACK_RELEASE_KEY", os.environ)
            self.assertNotIn("GITHUB_TOKEN", os.environ)
        code, out = run_quiet(rp.main, ["pack", "--tag", "nope", "--lunepack", self.cli, "--config", self.config,
                                        "--work", os.path.join(self.tmp, "m2")])
        self.assertEqual(code, 1)
        self.assertIn("release-pack: bad tag", out)


# ---------------------------------------------------------------------------------------- attach


class FakeRelease:
    """Stands in for the GitHub release of the tag."""

    existing = {}
    instance = None

    def __init__(self, secrets, tag):
        self.tag = tag
        self.uploaded = []
        self.deleted = []
        self._assets = {n: {"id": i, "name": n, "size": s} for i, (n, s) in enumerate(FakeRelease.existing.items(), 1)}
        FakeRelease.instance = self

    def assets(self):
        return dict(self._assets)

    def upload(self, name, path):
        self.uploaded.append(name)

    def delete_asset(self, asset):
        self.deleted.append(asset["name"])


class Attach(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)
        self.dir = os.path.join(self.tmp, "setup")
        os.makedirs(self.dir)
        self.digests = {}
        for target, spec in rp.TARGETS.items():
            name = spec["setup"].format(v="0.9.0")
            data = ("setup-" + target).encode()
            pathlib.Path(self.dir, name).write_bytes(data)
            rp.sidecar(os.path.join(self.dir, name), name, sha(data))
            self.digests[target] = sha(data)
        FakeRelease.existing = {}
        patch = mock.patch.object(rp, "Release", FakeRelease)
        patch.start()
        self.addCleanup(patch.stop)

    def expect(self, **over):
        d = dict(self.digests, **over)
        return ["%s=%s" % (t, h) for t, h in d.items()]

    def attach(self, expect=None, tag="v0.9.0", directory=None):
        args = argparse.Namespace(tag=tag, dir=directory or self.dir, expect=expect or self.expect())
        return run_quiet(rp.cmd_attach, args, rp.Secrets({"GITHUB_TOKEN": TOKEN}))

    def test_it_uploads_what_is_missing_and_deletes_the_build_inputs(self):
        FakeRelease.existing = {"luneterm-0.9.0-linux-x64-build.tar.gz": 5, "luneterm-0.9.0-linux-x64-build.tar.gz.sha256": 1,
                                "luneterm-0.9.0-windows-x64-build.zip": 5, "notes.txt": 1}
        self.attach()
        r = FakeRelease.instance
        self.assertEqual(sorted(r.uploaded), sorted(os.listdir(self.dir)))
        self.assertEqual(sorted(r.deleted), ["luneterm-0.9.0-linux-x64-build.tar.gz",
                                             "luneterm-0.9.0-linux-x64-build.tar.gz.sha256",
                                             "luneterm-0.9.0-windows-x64-build.zip"])

    def test_an_asset_that_is_already_there_is_kept_and_a_different_one_is_refused(self):
        name = rp.TARGETS["linux-x86_64"]["setup"].format(v="0.9.0")
        size = os.path.getsize(os.path.join(self.dir, name))
        FakeRelease.existing = {name: size}
        self.attach()
        self.assertNotIn(name, FakeRelease.instance.uploaded)
        self.assertEqual(len(FakeRelease.instance.uploaded), 5)
        FakeRelease.existing = {name: size + 1}
        with self.assertRaises(rp.Abort) as ctx:
            self.attach()
        self.assertIn("the release already has a different %s; delete it by hand" % name, str(ctx.exception))

    def check_refusal_uploads_nothing(self, message, **kw):
        FakeRelease.instance = None
        with self.assertRaises(rp.Abort) as ctx:
            self.attach(**kw)
        self.assertIn(message, str(ctx.exception))
        self.assertIsNone(FakeRelease.instance, "the release was contacted before the check")

    def test_a_file_that_is_not_the_one_the_pack_job_built_is_refused(self):
        name = rp.TARGETS["windows-x86_64"]["setup"].format(v="0.9.0")
        pathlib.Path(self.dir, name).write_bytes(b"swapped")
        self.check_refusal_uploads_nothing("is not the file the pack job built (its SHA-256 differs)")

    def test_a_wrong_sidecar_a_missing_file_and_an_extra_file_are_refused(self):
        name = rp.TARGETS["linux-x86_64"]["setup"].format(v="0.9.0")
        pathlib.Path(self.dir, name + ".sha256").write_text("%s  %s\n" % ("0" * 64, name))
        self.check_refusal_uploads_nothing("does not match its .sha256")
        rp.sidecar(os.path.join(self.dir, name), name, self.digests["linux-x86_64"])
        pathlib.Path(self.dir, "extra.bin").write_bytes(b"x")
        self.check_refusal_uploads_nothing("holds files that are not setup files")
        os.remove(os.path.join(self.dir, "extra.bin"))
        os.remove(os.path.join(self.dir, name))
        self.check_refusal_uploads_nothing("has no %s" % name)

    def test_bad_expectations_are_refused(self):
        for expect, message in (
            (["linux-x86_64=" + "a" * 64], "--expect must name every target"),
            (self.expect() + ["linux-x86_64=" + "a" * 64], "bad --expect"),
            (self.expect(**{"linux-x86_64": "A" * 64}), "bad --expect"),
            (["linux-x86_64=zz"] + self.expect()[1:], "bad --expect"),
            (["plan9-x86_64=" + "a" * 64] + self.expect(), "bad --expect"),
            (["linux-x86_64"] + self.expect()[1:], "bad --expect"),
        ):
            with self.subTest(expect=expect[0]):
                self.check_refusal_uploads_nothing(message, expect=expect)
        self.check_refusal_uploads_nothing("bad tag", tag="0.9.0")


# ------------------------------------------------------------------------------------------ dmg

BASE = "https://updates.izzat.dev/luneterm/"


def envelope(payload):
    return json.dumps({"payloadType": "application/vnd.lunepack.test+json",
                       "payload": base64.b64encode(json.dumps(payload).encode()).decode(), "signatures": []}).encode()


def published(version="0.9.0", pack=b"PACKBYTES", roots=2, app_id="my.ijat.luneterm"):
    """A fake update store: {url: (status, bytes)} for one release, the way the real one lays it out."""
    channel = rp.channel_of(version)
    pack_path = "%s/macos-aarch64.pack" % version
    manifest = envelope({"kind": "manifest", "app_id": app_id, "version": version, "target": "macos-aarch64",
                         "pack": {"path": pack_path, "size": len(pack), "sha256": sha(pack)}})
    m_path = "%s/macos-aarch64.manifest.json" % version
    feed = envelope({"kind": "feed", "app_id": app_id, "channel": channel, "releases": [
        {"version": version, "targets": {"macos-aarch64": {"path": m_path, "size": len(manifest),
                                                            "sha256": sha(manifest)}}}]})
    store = {BASE + "keys.json": (200, envelope({"kind": "keys"})),
             BASE + "feed-%s.json" % channel: (200, feed),
             BASE + m_path: (200, manifest), BASE + pack_path: (200, pack)}
    for k in range(2, 2 + roots):
        store[BASE + "root-%d.json" % k] = (200, envelope({"kind": "root", "version": k}))
    store[BASE + "root-%d.json" % (2 + roots)] = (404, b"")
    return store


class Dmg(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)
        self.log = os.path.join(self.tmp, "calls.jsonl")
        self.cli = os.path.join(self.tmp, "lunepack")
        pathlib.Path(self.cli).write_text(FAKE_LUNEPACK % {"python": sys.executable})
        os.chmod(self.cli, 0o755)
        self.config = os.path.join(self.tmp, "lunepack.toml")
        pathlib.Path(self.config).write_text(repo_config_text(REAL_ROOT))
        env = mock.patch.dict(os.environ, {"FAKE_LOG": self.log})
        env.start()
        self.addCleanup(env.stop)
        self.fetched = []
        self.store = {}
        self.caps = {}
        for patch in (mock.patch.object(rp, "http_get", self.get),
                      mock.patch.object(rp.platform, "system", lambda: "Darwin")):
            patch.start()
            self.addCleanup(patch.stop)

    def get(self, url, cap):
        self.fetched.append(url)
        self.caps[url] = cap
        return self.store.get(url, (404, b""))

    def args(self, tag="v0.9.0", work="work", **over):
        base = dict(tag=tag, lunepack=self.cli, config=self.config, work=os.path.join(self.tmp, work))
        base.update(over)
        return argparse.Namespace(**base)

    def dmg(self, **over):
        return run_quiet(rp.cmd_dmg, self.args(**over), rp.Secrets({}))

    def refused(self, message, **over):
        with self.assertRaises(rp.Abort) as ctx:
            self.dmg(**over)
        self.assertIn(message, str(ctx.exception))
        return str(ctx.exception)

    def calls(self):
        if not os.path.exists(self.log):
            return []
        with open(self.log) as f:
            return [json.loads(line) for line in f]

    def test_happy_path_fetches_in_order_runs_setup_without_zip_and_writes_the_sidecar(self):
        self.store = published("0.9.0", roots=2)
        _, out = self.dmg()
        self.assertEqual([u[len(BASE):] for u in self.fetched],
                         ["keys.json", "root-2.json", "root-3.json", "root-4.json", "feed-stable.json",
                          "0.9.0/macos-aarch64.manifest.json", "0.9.0/macos-aarch64.pack"])
        calls = self.calls()
        self.assertEqual([c["sub"] for c in calls], ["setup"])
        a = calls[0]["argv"]
        self.assertNotIn("--zip", a)
        self.assertIsNone(calls[0]["epoch"])
        self.assertFalse(calls[0]["has_key"])
        self.assertEqual(a[a.index("--target") + 1], "macos-aarch64")
        self.assertEqual(a[a.index("--version") + 1], "0.9.0")
        self.assertEqual(a[a.index("--config") + 1], self.config)
        self.assertEqual(a[a.index("--out") + 1], os.path.join(self.tmp, "work", "setup", "luneterm-0.9.0-macos-arm64-setup.dmg"))
        release = a[1]
        self.assertEqual(release, os.path.join(self.tmp, "work", "release"))
        for name in ("keys.json", "root-2.json", "root-3.json", "feed-stable.json", "0.9.0/macos-aarch64.pack"):
            self.assertTrue(os.path.isfile(os.path.join(release, *name.split("/"))), name)
        self.assertFalse(os.path.exists(os.path.join(release, "root-4.json")))
        path = a[a.index("--out") + 1]
        digest = sha(pathlib.Path(path).read_bytes())
        self.assertEqual(pathlib.Path(path + ".sha256").read_text(), "%s  luneterm-0.9.0-macos-arm64-setup.dmg\n" % digest)
        self.assertIn("dmg: " + path, out)
        self.assertIn("dmg_sha256=" + digest, out)

    def test_caps(self):
        self.store = published("0.9.0")
        self.dmg()
        self.assertEqual(self.caps[BASE + "keys.json"], 1024 * 1024)
        self.assertEqual(self.caps[BASE + "root-2.json"], 1024 * 1024)
        self.assertEqual(self.caps[BASE + "feed-stable.json"], 1024 * 1024)
        self.assertEqual(self.caps[BASE + "0.9.0/macos-aarch64.manifest.json"], 64 * 1024 * 1024)
        self.assertEqual(self.caps[BASE + "0.9.0/macos-aarch64.pack"], len(b"PACKBYTES"))

    def test_a_release_candidate_reads_the_beta_feed(self):
        self.store = published("0.9.0-rc.1")
        self.dmg(tag="v0.9.0-rc.1")
        self.assertIn(BASE + "feed-beta.json", self.fetched)
        self.assertNotIn(BASE + "feed-stable.json", self.fetched)
        self.assertEqual(self.calls()[0]["argv"][self.calls()[0]["argv"].index("--version") + 1], "0.9.0-rc.1")

    def test_stable_reads_the_stable_feed(self):
        self.store = published("0.9.0")
        self.dmg()
        self.assertIn(BASE + "feed-stable.json", self.fetched)
        self.assertNotIn(BASE + "feed-beta.json", self.fetched)

    def test_the_root_walk_stops_on_404_410_and_403(self):
        for status in (404, 410, 403):
            with self.subTest(status=status):
                self.fetched.clear()
                self.store = published("0.9.0", roots=1)
                self.store[BASE + "root-3.json"] = (status, b"")
                self.dmg(work="work%d" % status)
                self.assertIn(BASE + "root-3.json", self.fetched)
                self.assertNotIn(BASE + "root-4.json", self.fetched)

    def test_other_statuses_fail(self):
        for status in (500, 301, 302, 401):
            with self.subTest(status=status):
                self.store = published("0.9.0", roots=1)
                self.store[BASE + "root-3.json"] = (status, b"")
                self.refused("root-3.json: HTTP %d" % status, work="w%d" % status)
        self.store = published("0.9.0")
        self.store[BASE + "keys.json"] = (302, b"")
        self.refused("keys.json: HTTP 302", work="wk")
        self.assertEqual(self.calls(), [])

    def test_a_missing_required_object_fails(self):
        for name in ("keys.json", "feed-stable.json", "0.9.0/macos-aarch64.manifest.json", "0.9.0/macos-aarch64.pack"):
            with self.subTest(name=name):
                self.store = published("0.9.0")
                self.store[BASE + name] = (404, b"")
                self.refused("%s: HTTP 404" % name, work="w" + name.replace("/", "_"))
        self.assertEqual(self.calls(), [])

    def test_a_runaway_root_walk_fails(self):
        self.store = published("0.9.0", roots=0)
        with mock.patch.object(rp, "MAX_ROOT_FILES", 3):
            for k in range(2, 20):
                self.store[BASE + "root-%d.json" % k] = (200, b"{}")
            self.refused("more than 3 root files")

    def test_a_version_not_in_the_feed_fails_and_setup_never_runs(self):
        self.store = published("0.9.0")
        self.store.update({k: v for k, v in published("0.8.0").items() if "feed" in k})
        self.refused("0.9.0 is not listed in feed-stable.json")
        self.assertEqual(self.calls(), [])
        self.assertNotIn(BASE + "0.9.0/macos-aarch64.manifest.json", self.fetched)

    def test_the_feed_must_be_the_feed_of_this_app_and_channel(self):
        for bad in (dict(kind="manifest"), dict(app_id="other.app"), dict(channel="beta")):
            with self.subTest(bad=bad):
                self.store = published("0.9.0")
                feed = json.loads(base64.b64decode(json.loads(self.store[BASE + "feed-stable.json"][1])["payload"]))
                feed.update(bad)
                self.store[BASE + "feed-stable.json"] = (200, envelope(feed))
                self.refused("feed-stable.json", work="w%s" % "".join(bad))
        self.assertEqual(self.calls(), [])

    def test_a_feed_with_two_entries_or_a_wrong_manifest_path_or_bad_digest_fails(self):
        def edit(fn):
            store = published("0.9.0")
            feed = json.loads(base64.b64decode(json.loads(store[BASE + "feed-stable.json"][1])["payload"]))
            fn(feed)
            store[BASE + "feed-stable.json"] = (200, envelope(feed))
            return store

        cases = {
            "twice": lambda f: f["releases"].append(f["releases"][0]),
            "path": lambda f: f["releases"][0]["targets"]["macos-aarch64"].update(path="0.9.0/../x.json"),
            "notarget": lambda f: f["releases"][0]["targets"].pop("macos-aarch64"),
            "size": lambda f: f["releases"][0]["targets"]["macos-aarch64"].update(size="9"),
            "boolsize": lambda f: f["releases"][0]["targets"]["macos-aarch64"].update(size=True),
            "negsize": lambda f: f["releases"][0]["targets"]["macos-aarch64"].update(size=-1),
            "upper": lambda f: f["releases"][0]["targets"]["macos-aarch64"].update(
                sha256=f["releases"][0]["targets"]["macos-aarch64"]["sha256"].upper()),
            "releases": lambda f: f.update(releases={}),
        }
        for name, fn in cases.items():
            with self.subTest(name):
                self.store = edit(fn)
                self.refused("feed-stable.json", work="w" + name)
        self.assertEqual(self.calls(), [])

    def test_a_manifest_that_differs_from_the_feed_fails(self):
        self.store = published("0.9.0")
        key = BASE + "0.9.0/macos-aarch64.manifest.json"
        status, data = self.store[key]
        self.store[key] = (status, data + b" ")  # same JSON, other size and digest
        self.refused("is not the manifest the feed lists")
        self.assertNotIn(BASE + "0.9.0/macos-aarch64.pack", self.fetched)
        self.assertEqual(self.calls(), [])

    def test_a_manifest_whose_digest_alone_differs_fails(self):
        self.store = published("0.9.0")
        key = BASE + "0.9.0/macos-aarch64.manifest.json"
        status, data = self.store[key]
        same_length = data.replace(b"vnd.lunepack.test", b"vnd.lunepack.best")
        self.assertEqual(len(same_length), len(data))
        self.assertNotEqual(same_length, data)
        self.store[key] = (status, same_length)
        self.refused("is not the manifest the feed lists")
        self.assertEqual(self.calls(), [])

    def test_a_pack_that_differs_from_the_manifest_fails(self):
        self.store = published("0.9.0")
        key = BASE + "0.9.0/macos-aarch64.pack"
        self.store[key] = (200, b"PACKBYTEZ")  # same size, other digest
        self.refused("is not the pack the manifest lists")
        self.assertEqual(self.calls(), [])
        self.store[key] = (200, b"PACKBYTE")
        self.refused("is not the pack the manifest lists", work="w2")

    def test_a_manifest_that_names_another_version_target_or_pack_path_fails(self):
        def with_manifest(**change):
            store = published("0.9.0")
            m_key = BASE + "0.9.0/macos-aarch64.manifest.json"
            manifest = json.loads(base64.b64decode(json.loads(store[m_key][1])["payload"]))
            for k, v in change.items():
                if isinstance(v, dict):
                    manifest[k].update(v)
                else:
                    manifest[k] = v
            data = envelope(manifest)
            store[m_key] = (200, data)
            feed_key = BASE + "feed-stable.json"
            feed = json.loads(base64.b64decode(json.loads(store[feed_key][1])["payload"]))
            ref = feed["releases"][0]["targets"]["macos-aarch64"]
            ref.update(size=len(data), sha256=sha(data))
            store[feed_key] = (200, envelope(feed))
            return store

        for name, change in {"version": dict(version="0.9.1"), "target": dict(target="linux-x86_64"),
                             "path": dict(pack={"path": "0.9.0/other.pack"}), "kind": dict(kind="feed"),
                             "size": dict(pack={"size": "x"}), "sha": dict(pack={"sha256": "AB"})}.items():
            with self.subTest(name):
                self.store = with_manifest(**change)
                self.refused("macos-aarch64.manifest.json", work="w" + name)
        self.assertEqual(self.calls(), [])

    def test_an_oversized_document_fails(self):
        self.store = published("0.9.0")
        with mock.patch.object(rp, "http_get", lambda url, cap: (200, b"x" * (cap + 1))):
            self.refused("larger than")
        self.assertEqual(self.calls(), [])

    def test_a_pack_larger_than_a_pack_may_be_fails(self):
        self.store = published("0.9.0")
        with mock.patch.object(rp, "MAX_INPUT_BYTES", 4):
            self.refused("larger than a pack may be")
        self.assertNotIn(BASE + "0.9.0/macos-aarch64.pack", self.fetched)

    def test_unparsable_documents_fail_with_the_object_name_and_no_traceback(self):
        for name, data in {"feed-stable.json": b"not json", "0.9.0/macos-aarch64.manifest.json": b"[1]"}.items():
            for label, bad in {"json": data, "b64": json.dumps({"payload": "!!!"}).encode(),
                               "payload-type": json.dumps({"payload": 5}).encode(),
                               "payload-json": json.dumps({"payload": base64.b64encode(b"nope").decode()}).encode(),
                               "payload-list": json.dumps({"payload": base64.b64encode(b"[]").decode()}).encode(),
                               "deep": b"[" * 100000}.items():
                with self.subTest(name=name, label=label):
                    self.store = published("0.9.0")
                    if name.startswith("0.9.0"):
                        # keep the feed honest about the replaced manifest so the manifest check is reached
                        feed_key = BASE + "feed-stable.json"
                        feed = json.loads(base64.b64decode(json.loads(self.store[feed_key][1])["payload"]))
                        feed["releases"][0]["targets"]["macos-aarch64"].update(size=len(bad), sha256=sha(bad))
                        self.store[feed_key] = (200, envelope(feed))
                    self.store[BASE + name] = (200, bad)
                    self.refused(name, work="w-%s-%s" % (name.replace("/", "_"), label))
        self.assertEqual(self.calls(), [])

    def test_a_non_macos_host_fails_before_any_fetch(self):
        self.store = published("0.9.0")
        with mock.patch.object(rp.platform, "system", lambda: "Linux"):
            self.refused("hdiutil")
        self.assertEqual(self.fetched, [])

    def test_the_test_root_fails_before_any_fetch(self):
        self.store = published("0.9.0")
        pathlib.Path(self.config).write_text(repo_config_text(next(iter(rp.TEST_ROOT_KEYS))))
        self.refused("test root")
        self.assertEqual(self.fetched, [])

    def test_an_existing_work_dir_fails_before_any_fetch(self):
        self.store = published("0.9.0")
        os.makedirs(os.path.join(self.tmp, "work"))
        self.refused("exists")
        self.assertEqual(self.fetched, [])

    def test_a_bad_tag_fails(self):
        self.refused("bad tag", tag="0.9.0")

    def test_the_feed_base_must_be_an_https_url_ending_in_a_slash(self):
        self.store = published("0.9.0")
        original = pathlib.Path(self.config).read_text()
        for bad in ("http://updates.izzat.dev/luneterm/", "https://updates.izzat.dev/luneterm"):
            with self.subTest(bad=bad):
                pathlib.Path(self.config).write_text(original.replace("https://updates.izzat.dev/luneterm/", bad))
                self.refused("must be an https URL", work="w" + str(len(bad)))
        self.assertEqual(self.fetched, [])

    def test_lunepack_failing_or_writing_nothing_fails(self):
        self.store = published("0.9.0")
        with mock.patch.dict(os.environ, {"FAKE_FAIL": "setup"}):
            self.refused("lunepack setup failed")
        self.store = published("0.9.0")
        cli = os.path.join(self.tmp, "noop")
        pathlib.Path(cli).write_text("#!%s\nimport sys\n" % sys.executable)
        os.chmod(cli, 0o755)
        self.refused("wrote no luneterm-0.9.0-macos-arm64-setup.dmg", work="w2", lunepack=cli)

    def test_setup_argv_default_still_adds_zip_for_macos_only(self):
        self.assertIn("--zip", rp.setup_argv("c", "r", "1.0.0", "macos-aarch64", "o"))
        self.assertNotIn("--zip", rp.setup_argv("c", "r", "1.0.0", "macos-aarch64", "o", zip=False))
        self.assertNotIn("--zip", rp.setup_argv("c", "r", "1.0.0", "linux-x86_64", "o"))
        self.assertEqual(rp.TARGETS["macos-aarch64"]["dmg"].format(v="0.9.0"), "luneterm-0.9.0-macos-arm64-setup.dmg")

    def test_the_real_getter_is_https_only_has_no_credentials_and_follows_no_redirect(self):
        seen = {}

        class Opener:
            def open(self, request, timeout=None):
                seen["request"], seen["timeout"] = request, timeout
                raise rp.urllib.error.HTTPError(request.full_url, 302, "Found", {}, None)

        def build_opener(*handlers):
            seen["handlers"] = handlers
            return Opener()

        with self.assertRaises(rp.Abort):
            REAL_HTTP_GET("http://updates.izzat.dev/luneterm/keys.json", 10)
        with mock.patch.object(rp.urllib.request, "build_opener", build_opener):
            status, data = REAL_HTTP_GET(BASE + "keys.json", 10)
        self.assertEqual((status, data), (302, b""))
        request = seen["request"]
        self.assertEqual(request.get_method(), "GET")
        self.assertEqual(request.full_url, BASE + "keys.json")
        self.assertEqual(dict(request.header_items()), {"User-agent": "luneterm-release-pack"})
        self.assertLessEqual(seen["timeout"], 60)
        (handler,) = seen["handlers"]
        self.assertIsNone(handler().redirect_request(request, None, 302, "Found", {}, "https://elsewhere/x"))

    def test_the_real_getter_fails_above_the_cap(self):
        class Response:
            status = 200

            def __enter__(self):
                return self

            def __exit__(self, *exc):
                return False

            def read(self, n):
                return b"x" * n

        class Opener:
            def open(self, request, timeout=None):
                return Response()

        with mock.patch.object(rp.urllib.request, "build_opener", lambda *h: Opener()):
            with self.assertRaises(rp.Abort):
                REAL_HTTP_GET(BASE + "keys.json", 10)

    def test_main_runs_dmg_from_the_command_line(self):
        self.store = published("0.9.0")
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = rp.main(["dmg", "--tag", "v0.9.0", "--lunepack", self.cli, "--config", self.config,
                            "--work", os.path.join(self.tmp, "viamain")])
        self.assertEqual(code, 0)
        self.assertIn("dmg_sha256=", out.getvalue())


class Scope(unittest.TestCase):
    def test_the_glue_holds_no_store_feed_retention_or_state_code(self):
        text = (SCRIPTS / "release-pack.py").read_text().lower()
        for needle in ("sigv4", "r2store", "s3.", "retention", "pending-deletes", "state.json", "feed add", "feed refresh",
                       "--trust-release-dir", "wrangler", "npm "):
            self.assertNotIn(needle, text, needle)

    def test_the_old_publisher_and_config_renderer_are_gone(self):
        for name in ("publish-updates.py", "test_publish_updates.py", "packaging-config.py"):
            self.assertFalse((SCRIPTS / name).exists(), name)


if __name__ == "__main__":
    unittest.main()
