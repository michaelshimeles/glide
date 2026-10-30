#!/usr/bin/env bash
# Build a universal (Apple Silicon + Intel) DaisyCap.app, sign it with a Developer ID certificate and
# the hardened runtime, notarize and staple it, then package it as DaisyCap.dmg, itself signed,
# notarized and stapled, with the app icon as its volume icon. That file name is the release asset
# the download page links to. Next to it go DaisyCap-<version>.zip, the app alone, and appcast.xml,
# the signed update feed that points installed copies at that zip. Everything is written to
# $CARGO_TARGET_DIR/universal-apple-darwin/release once every step has passed; a failed run leaves
# the previous build's files there. Upload all three to a release tagged
# v<version> (the script prints the command). Bump the version in Cargo.toml and support/Info.plist
# first, since installed copies only offer a version higher than their own.
#
# Notarization credentials come from a notarytool keychain profile. Create it once with an
# app-specific password from account.apple.com and the team ID shown in the certificate name:
#
#   xcrun notarytool store-credentials daisycap-notary --apple-id you@example.com --team-id TEAMID
#
# Updates are signed with the EdDSA key in the login keychain under the account daisycap, whose
# public half is SUPublicEDKey in support/Info.plist. Keep an exported copy somewhere safe:
# installed copies accept no other key, so losing it ends their updates.
#
#   "$(scripts/fetch-sparkle.sh)/bin/generate_keys" --account daisycap -x daisycap-sparkle.key
#   "$(scripts/fetch-sparkle.sh)/bin/generate_keys" --account daisycap -f daisycap-sparkle.key  # on a new Mac
#
#   SIGN_IDENTITY    signing identity (default: the first "Developer ID Application" in the keychain)
#   NOTARY_PROFILE   notarytool keychain profile (default: daisycap-notary)
#   NOTARY_KEYCHAIN  keychain holding that profile (default: the login keychain)
#   SPARKLE_ACCOUNT  login keychain account of the update signing key (default: daisycap)
#   NOTARIZE=0       sign and package without notarizing or writing the update feed
set -euo pipefail
cd "$(dirname "$0")/.."
. scripts/env.sh

identity=${SIGN_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: .*\)"$/\1/p' | head -1)}
[ -n "$identity" ] || { echo "no Developer ID Application certificate in the keychain; set SIGN_IDENTITY" >&2; exit 1; }
notarize=${NOTARIZE:-1}
notary=(--keychain-profile "${NOTARY_PROFILE:-daisycap-notary}")
[ -z "${NOTARY_KEYCHAIN:-}" ] || notary+=(--keychain "$NOTARY_KEYCHAIN")
if [ "$notarize" = 1 ] && ! xcrun notarytool history "${notary[@]}" >/dev/null; then
  echo "no notarytool credentials; create the profile described at the top of $0, set NOTARY_PROFILE, or set NOTARIZE=0" >&2
  exit 1
fi

plist=support/Info.plist
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
for key in CFBundleShortVersionString CFBundleVersion; do
  if [ "$(plutil -extract "$key" raw "$plist")" != "$version" ]; then
    echo "$key in $plist isn't $version, the version in Cargo.toml" >&2
    exit 1
  fi
done
min_macos=$(plutil -extract LSMinimumSystemVersion raw "$plist")
repo=https://github.com/michaelshimeles/daisycap
sparkle=$(scripts/fetch-sparkle.sh)
sparkle_account=${SPARKLE_ACCOUNT:-daisycap}
sparkle_key=$(plutil -extract SUPublicEDKey raw "$plist")
if [ "$notarize" = 1 ]; then
  key=$("$sparkle/bin/generate_keys" --account "$sparkle_account" -p 2>/dev/null || true)
  if [ "$key" != "$sparkle_key" ]; then
    echo "the login keychain's update signing key (account $sparkle_account) is '$key', but the app trusts $sparkle_key." >&2
    echo "Import the exported key with $sparkle/bin/generate_keys --account $sparkle_account -f <file>; never generate a new one." >&2
    exit 1
  fi
fi

tmp=$(mktemp -d)
# A failure while a disk image is mounted would otherwise leave it attached, and rm -rf would
# delete into it.
cleanup() {
  local mnt
  for mnt in "$tmp/mnt" "$tmp/check"; do
    if [ -d "$mnt" ] && [ "$(stat -f %d "$mnt")" != "$(stat -f %d "$tmp")" ]; then
      hdiutil detach -force "$mnt" >/dev/null || { echo "left an image mounted at $mnt" >&2; return; }
    fi
  done
  rm -rf "$tmp"
}
trap cleanup EXIT

# Submits $1, waits for Apple's verdict, then staples the ticket to $2.
notarize() {
  local out id
  out=$(xcrun notarytool submit "$1" "${notary[@]}" --wait --output-format json) || true
  if [ "$(plutil -extract status raw -o - - <<<"$out" 2>/dev/null)" != Accepted ]; then
    echo "notarizing $1 failed: $out" >&2
    if id=$(plutil -extract id raw -o - - <<<"$out" 2>/dev/null); then
      xcrun notarytool log "$id" "${notary[@]}" >&2
    fi
    exit 1
  fi
  xcrun stapler staple "$2"
}

# whisper.cpp's build script doesn't notice a change of MACOSX_DEPLOYMENT_TARGET (set by env.sh),
# so it is rebuilt every time to be sure it targets $min_macos.
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo clean --release --target "$target" -p whisper-rs-sys
  cargo build --release --locked --target "$target"
done
out=$CARGO_TARGET_DIR/universal-apple-darwin/release
# Everything is built in $work and moves to $out only after the last step passes, so a failed run
# leaves the previous build's files in place.
work=$tmp/out
finish() {
  mkdir -p "$out"
  rm -rf "$out/DaisyCap.app" "$out"/DaisyCap-*.zip "$out/DaisyCap.dmg" "$out/appcast.xml"
  local f
  for f in "$work"/*; do
    mv "$f" "$out/"
    echo "built $out/$(basename "$f")"
  done
}
app=$work/DaisyCap.app
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks"
cp "$plist" "$app/Contents/Info.plist"
lipo -create -output "$app/Contents/MacOS/DaisyCap" \
  "$CARGO_TARGET_DIR/aarch64-apple-darwin/release/daisycap" "$CARGO_TARGET_DIR/x86_64-apple-darwin/release/daisycap"
"$app/Contents/MacOS/DaisyCap" icon "$tmp/AppIcon.iconset" >/dev/null
iconutil -c icns -o "$app/Contents/Resources/AppIcon.icns" "$tmp/AppIcon.iconset"
# The app isn't sandboxed, so Sparkle's XPC services aren't needed; headers and modules are
# build-time only.
framework=$app/Contents/Frameworks/Sparkle.framework
ditto "$sparkle/Sparkle.framework" "$framework"
for part in XPCServices Headers PrivateHeaders Modules; do
  rm -rf "${framework:?}/$part" "${framework:?}/Versions/B/$part"
done

echo "signing as $identity"
# Inside out, Sparkle's helpers first. The hardened runtime loads Sparkle.framework only when it
# is signed by the app's own team.
for code in "$framework/Versions/B/Autoupdate" "$framework/Versions/B/Updater.app" "$framework"; do
  codesign --force --options runtime --timestamp --sign "$identity" "$code"
done
codesign --force --options runtime --timestamp --entitlements support/entitlements.plist --sign "$identity" "$app"
codesign --verify --deep --strict --verbose=2 "$app"
if [ "$notarize" = 1 ]; then
  ditto -c -k --keepParent "$app" "$tmp/DaisyCap.zip"
  notarize "$tmp/DaisyCap.zip" "$app"
fi

dmg=$work/DaisyCap.dmg
stage=$tmp/dmg
mkdir "$stage" "$tmp/mnt" "$tmp/check"
ditto "$app" "$stage/DaisyCap.app"
ln -s /Applications "$stage/Applications"
cp "$app/Contents/Resources/AppIcon.icns" "$stage/.VolumeIcon.icns"
# -srcfolder drops the volume's custom-icon flag, so it is set on a writable image that is then
# compressed.
hdiutil create -volname DaisyCap -srcfolder "$stage" -format UDRW "$tmp/rw.dmg"
hdiutil attach -nobrowse -noautoopen -mountpoint "$tmp/mnt" "$tmp/rw.dmg" >/dev/null
SetFile -a C "$tmp/mnt"
hdiutil detach "$tmp/mnt" >/dev/null
hdiutil convert "$tmp/rw.dmg" -format UDZO -ov -o "$dmg"
codesign --force --timestamp --sign "$identity" "$dmg"
if [ "$notarize" = 1 ]; then
  notarize "$dmg" "$dmg"
  spctl --assess --type execute --verbose=2 "$app"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
fi
# Check the app as a user gets it: dragged out of the finished DMG.
hdiutil attach -nobrowse -noautoopen -readonly -mountpoint "$tmp/check" "$dmg" >/dev/null
ditto "$tmp/check/DaisyCap.app" "$tmp/copied/DaisyCap.app"
hdiutil detach "$tmp/check" >/dev/null
codesign --verify --deep --strict "$tmp/copied/DaisyCap.app"
[ "$notarize" = 0 ] || spctl --assess --type execute "$tmp/copied/DaisyCap.app"

# ditto keeps the signature and the stapled ticket; plain zip does not.
zip=$work/DaisyCap-$version.zip
ditto -c -k --keepParent "$app" "$zip"
[ "$notarize" = 1 ] || { finish; exit 0; }

# Installed copies read releases/latest/download/appcast.xml (SUFeedURL). generate_keys created
# the key's keychain item, so it can export it without a prompt; the export lives in $tmp only
# while signing.
key_file=$tmp/sparkle.key
"$sparkle/bin/generate_keys" --account "$sparkle_account" -x "$key_file" >/dev/null
# sparkle:edSignature="…" length="…"
enclosure=$("$sparkle/bin/sign_update" --ed-key-file "$key_file" "$zip")
appcast=$work/appcast.xml
cat > "$appcast" <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>DaisyCap</title>
    <link>$repo</link>
    <item>
      <title>DaisyCap $version</title>
      <pubDate>$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')</pubDate>
      <sparkle:version>$version</sparkle:version>
      <sparkle:shortVersionString>$version</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>$min_macos</sparkle:minimumSystemVersion>
      <sparkle:fullReleaseNotesLink>$repo/releases/tag/v$version</sparkle:fullReleaseNotesLink>
      <enclosure url="$repo/releases/download/v$version/$(basename "$zip")" type="application/octet-stream" $enclosure/>
    </item>
  </channel>
</rss>
XML
# The app sets SURequireSignedFeed, so the feed carries its own signature as well.
"$sparkle/bin/sign_update" --ed-key-file "$key_file" "$appcast"
"$sparkle/bin/sign_update" --ed-key-file "$key_file" --verify "$appcast"
rm -f "$key_file"
finish
echo "publish with: gh release create v$version $out/DaisyCap.dmg $out/$(basename "$zip") $out/appcast.xml"
