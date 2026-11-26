#!/usr/bin/env bash
# Regenerate the app icon set in assets/icon/ from the master assets/icon/luneterm.svg. macOS only
# (needs rsvg-convert, python3 and iconutil); the outputs are checked in, CI has none of these
# tools. Deterministic: a rerun leaves no git diff.
#
#   scripts/gen-icons.sh
#
# Outputs: luneterm-{16,32,48,64,128,256,512,1024}.png, luneterm.ico (16, 24, 32, 48, 64, 128,
# 256), Luneterm.icns.
set -euo pipefail

die() { echo "gen-icons: $*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "iconutil exists on macOS only; run this on a Mac"
for tool in rsvg-convert python3 iconutil; do
  command -v "$tool" > /dev/null 2>&1 || die "required tool not found: $tool"
done

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dir="$root/assets/icon"
svg="$dir/luneterm.svg"
[ -f "$svg" ] || die "missing master $svg"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

render() { # <size> <out.png>
  rsvg-convert --width "$1" --height "$1" --format png --output "$2" "$svg"
}

# The checked-in PNG set (the tree's `[install] icon` is the 256 one).
for s in 16 32 48 64 128 256 512 1024; do
  render "$s" "$dir/luneterm-$s.png"
done

# The .ico: seven sizes, each rendered from the vector (not scaled from the 256) and stored as a
# PNG entry (Vista and later; ~15 KB against ~370 KB for BMP entries, which matters in the exe).
# The container is written here, not by magick, so the bytes are fixed.
ico_sizes=(16 24 32 48 64 128 256)
for s in "${ico_sizes[@]}"; do
  render "$s" "$work/ico-$s.png"
done
python3 -I - "$dir/luneterm.ico" "$work" "${ico_sizes[@]}" << 'PY'
import struct
import sys

out, work, *sizes = sys.argv[1:]
pngs = [open(f"{work}/ico-{s}.png", "rb").read() for s in sizes]
head = struct.pack("<HHH", 0, 1, len(pngs))
offset = 6 + 16 * len(pngs)
entries = b""
for s, png in zip(sizes, pngs):
    n = int(s)
    entries += struct.pack("<BBBBHHII", 0 if n == 256 else n, 0 if n == 256 else n, 0, 0, 1, 32, len(png), offset)
    offset += len(png)
open(out, "wb").write(head + entries + b"".join(pngs))
PY

# The .icns through an iconset.
set="$work/Luneterm.iconset"
mkdir "$set"
render 16 "$set/icon_16x16.png"
render 32 "$set/icon_16x16@2x.png"
render 32 "$set/icon_32x32.png"
render 64 "$set/icon_32x32@2x.png"
render 128 "$set/icon_128x128.png"
render 256 "$set/icon_128x128@2x.png"
render 256 "$set/icon_256x256.png"
render 512 "$set/icon_256x256@2x.png"
render 512 "$set/icon_512x512.png"
render 1024 "$set/icon_512x512@2x.png"
iconutil -c icns -o "$dir/Luneterm.icns" "$set"

echo "gen-icons: wrote $dir"
