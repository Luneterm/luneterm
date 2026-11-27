#!/usr/bin/env bash
# Build the macOS build input for a release tag and attach it to the tag's draft release. There is
# no macOS runner, so this runs on a Mac after .github/workflows/release.yml made the draft.
#
#   scripts/package-macos.sh <tag> [--no-upload]
#
# The build input is not a user download: it is Luneterm.app assembled for lunepack (its runtime,
# update config and update window's helper app placed in the bundle) and ad-hoc signed inner code
# first, in a tar.gz with a BUILD-INFO.txt. The publish job (auto-update plan step 6) packs it into
# the signed update feed and the macOS setup zip. No Developer ID and no notarization (arm64 macOS
# will not run code with no signature at all).
#
# Output: target/package-macos/ (the input is luneterm-<version>-macos-arm64-build.tar.gz). Builds
# the lunepack runtime, UI and CLI from the pinned source (scripts/build-lunepack.sh). Needs gh,
# logged in, unless --no-upload.
set -euo pipefail

die() { echo "package-macos: $*" >&2; exit 1; }

no_upload=0
tag=""
for arg in "$@"; do
  case "$arg" in
    --no-upload) no_upload=1 ;;
    -*) die "unknown option: $arg" ;;
    *) [ -z "$tag" ] || die "more than one tag given"; tag="$arg" ;;
  esac
done
[ -n "$tag" ] || die "usage: scripts/package-macos.sh <tag> [--no-upload]"
[ "$(uname -s)" = "Darwin" ] || die "this packages the macOS app; run it on macOS"
[ "$(uname -m)" = "arm64" ] || die "only arm64 is packaged; this host is $(uname -m)"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# The same rule as the verify job of release.yml.
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]] \
  || die "tag $tag is not v<major>.<minor>.<patch> or v<major>.<minor>.<patch>-rc.<N>"
asset_version="${tag#v}"
version="${asset_version%%-rc.*}"
cargo_version="$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' Cargo.toml)"
[ "$version" = "$cargo_version" ] || die "tag $tag does not match the workspace version '$cargo_version'"
[ -z "$(git status --porcelain)" ] || die "the working tree is not clean"
tag_commit="$(git rev-parse --verify --quiet "refs/tags/$tag^{commit}")" || die "no local tag $tag"
[ "$(git rev-parse HEAD)" = "$tag_commit" ] || die "HEAD is not the commit of $tag"

if [ "$no_upload" = 0 ]; then
  is_draft="$(gh release view "$tag" --json isDraft --jq .isDraft)" || die "no release found for $tag"
  [ "$is_draft" = "true" ] || die "the release for $tag is not a draft; not changing it"
fi

cargo build --release --locked -p luneterm
bin=target/release/luneterm
minos="$(vtool -show-build "$bin" | awk '$1 == "minos" { print $2; exit }')"
[ -n "$minos" ] || die "could not read the deployment target of $bin"

out=target/package-macos
rm -rf "$out"
app="$out/input/Luneterm.app"
contents="$app/Contents"
mkdir -p "$contents/MacOS" "$contents/Resources"
cp "$bin" "$contents/MacOS/luneterm"
cp -R licenses "$contents/Resources/licenses"
cp assets/icon/Luneterm.icns "$contents/Resources/Luneterm.icns"
cat > "$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Luneterm</string>
  <key>CFBundleDisplayName</key><string>Luneterm</string>
  <key>CFBundleExecutable</key><string>luneterm</string>
  <key>CFBundleIconFile</key><string>Luneterm</string>
  <key>CFBundleIdentifier</key><string>my.ijat.luneterm</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key><string>$minos</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>A program running in Luneterm wants to use the microphone.</string>
</dict>
</plist>
PLIST
plutil -lint "$contents/Info.plist"

# lunepack's runtime, update window UI and CLI, built from the pinned source.
scripts/build-lunepack.sh macos-aarch64 "$out/lunepack"
lunepack_rev="$(cat "$out/lunepack/lunepack-rev.txt")"

# The same time stamp for every pack of this tag: a rerun gives the same bytes (lunepack's
# determinism rule), and the publish job reads it from BUILD-INFO.txt.
SOURCE_DATE_EPOCH="$(git log -1 --format=%ct HEAD)"
[[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]] || die "cannot read the commit time of HEAD"
export SOURCE_DATE_EPOCH

# The channel is baked into the bundle's update config, and a signed bundle must be packed with
# exactly the config that assembled it: a release candidate (beta feed only) says beta.
case "$asset_version" in *-rc.*) channel=beta ;; *) channel=stable ;; esac

# Assemble (lunepack places its runtime, update config and helper app in the bundle), then sign
# inner code first: a signature seals what is inside it, so the outer bundle goes last. The pack
# of this signed tree (done by the publish job) needs no codesign tool.
stage="$out/stage"
tree="$stage/tree"
mkdir -p "$stage"
"$out/lunepack/lunepack" pack --input "$app" --version "$asset_version" --target macos-aarch64 \
  --runtime "$out/lunepack/lunepack-runtime" --ui "$out/lunepack/lunepack-ui" \
  --assemble "$tree" --config packaging/lunepack.toml --channel "$channel"
helper="$tree/Contents/Helpers/lunepack-update.app"
codesign --force --sign - "$tree/Contents/MacOS/lunepack-runtime"
codesign --force --sign - "$helper/Contents/MacOS/lunepack-ui"
codesign --force --sign - "$helper"
codesign --force --sign - "$tree/Contents/MacOS/luneterm"
codesign --force --sign - "$tree"
codesign --verify --deep --strict "$tree"

cat > "$stage/BUILD-INFO.txt" <<INFO
format=1
version=$asset_version
target=macos-aarch64
config=packaging/lunepack.toml
channel=$channel
entry=Contents/MacOS/luneterm
tree=tree
runtime=tree/Contents/MacOS/lunepack-runtime
ui=tree/Contents/Helpers/lunepack-update.app/Contents/MacOS/lunepack-ui
lunepack_rev=$lunepack_rev
source_date_epoch=$SOURCE_DATE_EPOCH
INFO

zip_name="luneterm-$asset_version-macos-arm64-build.tar.gz"
python3 -I scripts/make-build-input.py "$stage" "$out/$zip_name"
(cd "$out" && shasum -a 256 "$zip_name" > "$zip_name.sha256")
echo "package-macos: $out/$zip_name"

if [ "$no_upload" = 0 ]; then
  gh release upload "$tag" "$out/$zip_name" "$out/$zip_name.sha256"
  echo "package-macos: attached to the draft of $tag"
fi
