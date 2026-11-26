#!/usr/bin/env python3
"""Packs a staging directory into a release build input, byte for byte reproducible.

    make-build-input.py <staging-dir> <out.tar.gz>

The publish job (scripts/publish-updates.py, plan step 6) takes one build input per target from
the draft release and packs it with lunepack. The staging directory holds BUILD-INFO.txt and the
directories it names (`tree/`, and for Linux and Windows `lunepack/`). The archive has the same
entries in sorted order, owner 0:0, every time stamp SOURCE_DATE_EPOCH (set in the environment) and
the mode bits as they are (the executable bit is what matters; a symlink stays one). Only regular
files, directories and symlinks are allowed: anything else is an error, so no device or hard link
can ride into the publish job. Standard library only.
"""

import gzip
import io
import os
import stat
import sys
import tarfile


def die(message):
    print("make-build-input: " + message, file=sys.stderr)
    sys.exit(1)


def main(argv):
    if len(argv) != 2:
        die("usage: make-build-input.py <staging-dir> <out.tar.gz>")
    staging, out = argv
    epoch = os.environ.get("SOURCE_DATE_EPOCH", "")
    if not epoch.isdigit():
        die("SOURCE_DATE_EPOCH must be set to a number")
    mtime = int(epoch)
    if not os.path.isdir(staging) or os.path.islink(staging):
        die("%s is not a directory" % staging)
    if os.path.exists(out):
        die("%s exists" % out)

    entries = []
    for base, dirs, files in os.walk(staging):
        dirs.sort()
        for name in sorted(dirs + files):
            path = os.path.join(base, name)
            entries.append((os.path.relpath(path, staging).replace(os.sep, "/"), path))
    entries.sort()
    if not any(rel == "BUILD-INFO.txt" for rel, _ in entries):
        die("%s has no BUILD-INFO.txt" % staging)

    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as tar:
        for rel, path in entries:
            st = os.lstat(path)
            info = tarfile.TarInfo(rel)
            info.mtime = mtime
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.mode = stat.S_IMODE(st.st_mode)
            if stat.S_ISDIR(st.st_mode):
                info.type = tarfile.DIRTYPE
                tar.addfile(info)
            elif stat.S_ISLNK(st.st_mode):
                info.type = tarfile.SYMTYPE
                info.linkname = os.readlink(path)
                tar.addfile(info)
            elif stat.S_ISREG(st.st_mode):
                if st.st_nlink > 1:
                    die("%s is a hard link" % rel)
                info.type = tarfile.REGTYPE
                info.size = st.st_size
                with open(path, "rb") as handle:
                    tar.addfile(info, handle)
            else:
                die("%s is not a regular file, directory or symlink" % rel)
    with open(out, "xb") as handle:
        with gzip.GzipFile(filename="", fileobj=handle, mode="wb", mtime=0, compresslevel=9) as gz:
            gz.write(raw.getvalue())


if __name__ == "__main__":
    main(sys.argv[1:])
