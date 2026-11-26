#!/usr/bin/env python3
"""Writes the lunepack config for one OS and one channel from packaging/lunepack-<os>.toml.

    packaging-config.py <linux|windows|macos> <stable|beta> <out.toml>

The channel is baked into a pack (`lunepack-update.json`), and a macOS bundle is code-signed
before it is packed, so the config that assembles it and the one that packs it must be
the same. Both come from here: a stable release says `stable`, a release candidate (listed in the
beta feed only) says `beta`. The icon paths in the file are relative to packaging/; they are made
absolute, so the output can be written anywhere. Standard library only.
"""

import os
import re
import sys

REPO = os.path.realpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))


def die(message):
    print("packaging-config: " + message, file=sys.stderr)
    sys.exit(1)


def main(argv):
    if len(argv) != 3:
        die("usage: packaging-config.py <linux|windows|macos> <stable|beta> <out.toml>")
    target_os, channel, out = argv
    if target_os not in ("linux", "windows", "macos"):
        die("unknown os: %r" % target_os)
    if channel not in ("stable", "beta"):
        die("unknown channel: %r" % channel)
    if os.path.exists(out):
        die("%s exists" % out)
    with open(os.path.join(REPO, "packaging", "lunepack-%s.toml" % target_os), encoding="utf-8") as f:
        text = f.read()
    text, n = re.subn(r'(?m)^channel = "stable"$', 'channel = "%s"' % channel, text)
    if n != 1:
        die('expected exactly one `channel = "stable"` line')
    text, _ = re.subn(
        r'(?m)^(icon = )"\.\./([^"]+)"',
        lambda m: '%s"%s"' % (m.group(1), os.path.join(REPO, m.group(2))),
        text,
    )
    with open(out, "x", encoding="utf-8") as f:
        f.write(text)


if __name__ == "__main__":
    main(sys.argv[1:])
