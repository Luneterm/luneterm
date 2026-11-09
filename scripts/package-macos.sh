#!/usr/bin/env bash
# Build Luneterm.app for a release tag, zip it and attach it to the tag's draft release. There is
# no macOS runner, so this runs on a Mac after .github/workflows/release.yml made the draft.
#
#   scripts/package-macos.sh <tag> [--no-upload]
#
# Output: target/package-macos/. The app has an ad-hoc signature only: no Developer ID and no
# notarization (arm64 macOS will not run code with no signature at all). Needs gh, logged in,
# unless --no-upload.
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
contents="$out/Luneterm.app/Contents"
mkdir -p "$contents/MacOS" "$contents/Resources"
cp "$bin" "$contents/MacOS/luneterm"
cp -R licenses "$contents/Resources/licenses"
cat > "$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Luneterm</string>
  <key>CFBundleDisplayName</key><string>Luneterm</string>
  <key>CFBundleExecutable</key><string>luneterm</string>
  <key>CFBundleIdentifier</key><string>my.ijat.luneterm</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key><string>$minos</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
plutil -lint "$contents/Info.plist"

codesign --force --sign - "$out/Luneterm.app"
codesign --verify --strict "$out/Luneterm.app"

zip_name="luneterm-$asset_version-macos-arm64.zip"
ditto -c -k --keepParent "$out/Luneterm.app" "$out/$zip_name"
(cd "$out" && shasum -a 256 "$zip_name" > "$zip_name.sha256")
echo "package-macos: $out/$zip_name"

if [ "$no_upload" = 0 ]; then
  gh release upload "$tag" "$out/$zip_name" "$out/$zip_name.sha256"
  echo "package-macos: attached to the draft of $tag"
fi
