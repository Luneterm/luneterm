"""Tests of the release scripts that need no network (auto-update plan step 5).

    python3 -B -m unittest discover -s scripts/tests -v
"""

import importlib.util
import io
import os
import pathlib
import subprocess
import sys
import tarfile
import tempfile
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def make_input(staging, out, epoch="1700000000"):
    env = dict(os.environ, SOURCE_DATE_EPOCH=epoch)
    return subprocess.run(
        [sys.executable, "-I", "-B", str(SCRIPTS / "make-build-input.py"), str(staging), str(out)],
        env=env,
        capture_output=True,
        text=True,
    )


class MakeBuildInput(unittest.TestCase):
    def stage(self, root):
        (root / "tree").mkdir()
        (root / "tree" / "luneterm").write_bytes(b"exe")
        (root / "tree" / "luneterm").chmod(0o755)
        (root / "tree" / "data").write_bytes(b"d")
        (root / "BUILD-INFO.txt").write_text("format=1\n")

    def test_the_archive_is_reproducible_and_keeps_the_exec_bit(self):
        with tempfile.TemporaryDirectory() as tmp:
            a, b = pathlib.Path(tmp, "a"), pathlib.Path(tmp, "b")
            a.mkdir()
            b.mkdir()
            self.stage(a)
            self.stage(b)
            os.utime(b / "tree" / "luneterm", (5, 5))  # another time stamp, same content
            out_a, out_b = pathlib.Path(tmp, "a.tgz"), pathlib.Path(tmp, "b.tgz")
            self.assertEqual(make_input(a, out_a).returncode, 0)
            self.assertEqual(make_input(b, out_b).returncode, 0)
            self.assertEqual(out_a.read_bytes(), out_b.read_bytes())
            with tarfile.open(out_a) as tar:
                names = [m.name for m in tar.getmembers()]
                self.assertEqual(names, sorted(names))
                self.assertEqual(tar.getmember("tree/luneterm").mode, 0o755)
                self.assertEqual(tar.getmember("tree/data").mode, 0o644)
                self.assertTrue(all(m.uid == 0 and m.mtime == 1700000000 for m in tar.getmembers()))

    def test_it_refuses_what_a_build_input_must_not_hold(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp, "s")
            root.mkdir()
            self.stage(root)
            # No time stamp: refused.
            out = pathlib.Path(tmp, "o.tgz")
            env = {k: v for k, v in os.environ.items() if k != "SOURCE_DATE_EPOCH"}
            r = subprocess.run(
                [sys.executable, "-I", "-B", str(SCRIPTS / "make-build-input.py"), str(root), str(out)],
                env=env,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(r.returncode, 0)
            self.assertFalse(out.exists())
            # A hard link: refused.
            os.link(root / "tree" / "data", root / "tree" / "again")
            r = make_input(root, out)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("hard link", r.stderr)
            os.remove(root / "tree" / "again")
            # No BUILD-INFO.txt: refused.
            os.remove(root / "BUILD-INFO.txt")
            self.assertNotEqual(make_input(root, out).returncode, 0)


class PackagingConfig(unittest.TestCase):
    def run_script(self, *args):
        return subprocess.run(
            [sys.executable, "-I", "-B", str(SCRIPTS / "packaging-config.py"), *args],
            capture_output=True,
            text=True,
        )

    def test_only_the_channel_and_the_icon_paths_change(self):
        repo = SCRIPTS.parent
        with tempfile.TemporaryDirectory() as tmp:
            for target_os in ("linux", "windows", "macos"):
                out = pathlib.Path(tmp, target_os + ".toml")
                r = self.run_script(target_os, "beta", str(out))
                self.assertEqual(r.returncode, 0, r.stderr)
                base = (repo / "packaging" / ("lunepack-%s.toml" % target_os)).read_text().splitlines()
                got = out.read_text().splitlines()
                self.assertEqual(len(base), len(got))
                changed = [(a, b) for a, b in zip(base, got) if a != b]
                self.assertIn(('channel = "stable"', 'channel = "beta"'), changed)
                for a, b in changed:
                    if a != 'channel = "stable"':
                        # an icon path: relative to packaging/ before, absolute and existing now
                        self.assertTrue(a.startswith('icon = "../'), a)
                        self.assertTrue(pathlib.Path(b.split('"')[1]).is_file(), b)
                stable = pathlib.Path(tmp, target_os + "-stable.toml")
                self.assertEqual(self.run_script(target_os, "stable", str(stable)).returncode, 0)
                self.assertIn('channel = "stable"', stable.read_text())

    def test_it_refuses_bad_arguments_and_an_existing_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = pathlib.Path(tmp, "o.toml")
            self.assertNotEqual(self.run_script("plan9", "stable", str(out)).returncode, 0)
            self.assertNotEqual(self.run_script("linux", "nightly", str(out)).returncode, 0)
            self.assertEqual(self.run_script("linux", "stable", str(out)).returncode, 0)
            self.assertNotEqual(self.run_script("linux", "stable", str(out)).returncode, 0)


class PublishRelease(unittest.TestCase):
    def test_it_wants_exactly_the_two_build_inputs_of_the_version(self):
        pr = load("publish_release", "publish-release.py")
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "linux").mkdir()
            (root / "windows").mkdir()
            (root / "linux" / "luneterm-0.8.0-linux-x64-build.tar.gz").write_bytes(b"a")
            (root / "linux" / "glibc-min.txt").write_text("2.36\n")
            (root / "windows" / "luneterm-0.8.0-windows-x64-build.zip").write_bytes(b"b")
            (root / "notes.md").write_text("v{version} glibc {glibc}")
            version, notes, assets = pr.prepare("v0.8.0", str(root), str(root / "notes.md"))
            self.assertEqual(version, "0.8.0")
            self.assertEqual(notes, "v0.8.0 glibc 2.36")
            self.assertEqual(
                [a[0] for a in assets],
                [
                    "luneterm-0.8.0-linux-x64-build.tar.gz",
                    "luneterm-0.8.0-linux-x64-build.tar.gz.sha256",
                    "luneterm-0.8.0-windows-x64-build.zip",
                    "luneterm-0.8.0-windows-x64-build.zip.sha256",
                ],
            )
            # The old user-facing archive names are not accepted any more.
            (root / "windows" / "luneterm-0.8.0-windows-x64-build.zip").rename(
                root / "windows" / "luneterm-0.8.0-windows-x64.zip"
            )
            with self.assertRaises(pr.Failure):
                pr.prepare("v0.8.0", str(root), str(root / "notes.md"))


if __name__ == "__main__":
    unittest.main()
