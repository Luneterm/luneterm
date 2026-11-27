#!/usr/bin/env bash
# Build the macOS installer (.dmg) of a published release and attach it to the release.
#
#   scripts/package-macos-dmg.sh <tag> [--no-upload] [--lunepack <cli>]
#
# Run it once the publish run's `attach` job has finished (the release then holds the macOS
# setup .zip): the .dmg is built from the release that is already published and signed, by
# `release-pack.py dmg` (it downloads the feed's documents and the macOS pack, checks them against
# each other, and lets `lunepack setup` do the cryptographic checks and write the .dmg; needs a
# macOS host for hdiutil). Run it at the commit of <tag> in a clean tree: the config of the tag is
# the one `lunepack setup` trusts. This is the manual path; the publish workflow's `dmg` job calls
# this script unchanged with GH_TOKEN set and --lunepack pointing at a CLI it checked.
#
# Output: target/package-macos-dmg/ (removed and recreated by this script; the .dmg and its
# .sha256 are in work/setup). Without --lunepack the pinned lunepack CLI is built from source
# (scripts/build-lunepack.sh). Uploads with `gh` (logged in, or GH_TOKEN) unless --no-upload; it
# never replaces an asset, and it exits 0 without building when the release already holds both.
set -euo pipefail

die() { echo "package-macos-dmg: $*" >&2; exit 1; }

no_upload=0
tag=""
cli=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --no-upload) no_upload=1 ;;
    --lunepack) [ "$#" -ge 2 ] || die "--lunepack needs a path"; cli="$2"; shift ;;
    -*) die "unknown option: $1" ;;
    *) [ -z "$tag" ] || die "more than one tag given"; tag="$1" ;;
  esac
  shift
done
[ -n "$tag" ] || die "usage: scripts/package-macos-dmg.sh <tag> [--no-upload] [--lunepack <cli>]"
[ "$(uname -s)" = "Darwin" ] || die "this builds a macOS installer with hdiutil; run it on macOS"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# The same rule as the verify job of release.yml.
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]] \
  || die "tag $tag is not v<major>.<minor>.<patch> or v<major>.<minor>.<patch>-rc.<N>"
asset_version="${tag#v}"
version="${asset_version%%-rc.*}"

# No release ships with the throwaway test root.
python3 -I scripts/release-pack.py check-root --config packaging/lunepack.toml || die "the config names the test root"

cargo_version="$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' Cargo.toml)"
[ "$version" = "$cargo_version" ] || die "tag $tag does not match the workspace version '$cargo_version'"
[ -z "$(git status --porcelain)" ] || die "the working tree is not clean"
tag_commit="$(git rev-parse --verify --quiet "refs/tags/$tag^{commit}")" || die "no local tag $tag"
[ "$(git rev-parse HEAD)" = "$tag_commit" ] || die "HEAD is not the commit of $tag"

dmg_name="luneterm-$asset_version-macos-arm64-setup.dmg"
zip_name="luneterm-$asset_version-macos-arm64-setup.zip"
if [ "$no_upload" = 0 ]; then
  is_draft="$(gh release view "$tag" --json isDraft --jq .isDraft)" || die "no release found for $tag"
  [ "$is_draft" = "false" ] || die "the release for $tag is a draft; publish it (and let the publish run attach) first"
  names="$(gh release view "$tag" --json assets --jq '.assets[].name')" || die "cannot list the assets of $tag"
  has() { printf '%s\n' "$names" | grep -Fxq -- "$1"; }
  has "$zip_name" || die "the release for $tag has no $zip_name; the publish run's attach job has not finished"
  if has "$dmg_name" && has "$dmg_name.sha256"; then
    echo "package-macos-dmg: $tag already has $dmg_name and its .sha256; nothing to do"
    exit 0
  fi
  if has "$dmg_name"; then die "$tag has $dmg_name but not its .sha256; delete it by hand and run again"; fi
  if has "$dmg_name.sha256"; then die "$tag has $dmg_name.sha256 but not $dmg_name; delete it by hand and run again"; fi
fi

out=target/package-macos-dmg
rm -rf "$out"
mkdir -p "$out"

if [ -n "$cli" ]; then
  [ -f "$cli" ] && [ -x "$cli" ] || die "--lunepack $cli is not an executable file"
  cli="$(cd "$(dirname "$cli")" && pwd)/$(basename "$cli")"
else
  scripts/build-lunepack.sh macos-aarch64 "$out/lunepack"
  cli="$root/$out/lunepack/lunepack"
  [ -x "$cli" ] || die "build-lunepack.sh produced no lunepack CLI"
  pin="$(sed -n 's/^lunepack = {.*rev = "\([0-9a-f]\{40\}\)".*$/\1/p' Cargo.toml)"
  [[ "$pin" =~ ^[0-9a-f]{40}$ ]] || die "the lunepack dependency has no 40-hex rev in Cargo.toml"
  built="$(cat "$out/lunepack/lunepack-rev.txt")"
  [ "$built" = "$pin" ] || die "the built lunepack is at $built, Cargo.toml pins $pin"
fi

python3 -I scripts/release-pack.py dmg --tag "$tag" --lunepack "$cli" --config packaging/lunepack.toml --work "$out/work"

dmg="$out/work/setup/$dmg_name"
[ -s "$dmg" ] && [ -s "$dmg.sha256" ] || die "release-pack.py wrote no $dmg_name"
(cd "$out/work/setup" && shasum -a 256 -c "$dmg_name.sha256")
echo "package-macos-dmg: $dmg"

if [ "$no_upload" = 0 ]; then
  gh release upload "$tag" "$dmg" "$dmg.sha256"
  echo "package-macos-dmg: attached $dmg_name to $tag"
fi
