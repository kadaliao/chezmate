#!/bin/sh
# Builds the release binaries and packs them for a GitHub release:
#   dist/Chezmate-<version>-macos-<arch>.zip      the desktop app
#   dist/chezmate-<version>-macos-<arch>.tar.gz   the command line (and MCP server)
#   dist/SHA256SUMS
#
# Signing: the first "Developer ID Application" identity in the keychain, or
# CHEZMATE_SIGN_IDENTITY; ad hoc when there is none.
# Notarization: set CHEZMATE_NOTARY_PROFILE to a notarytool keychain profile,
# made once with `xcrun notarytool store-credentials <profile>`. Without it
# the build is signed but not notarized, and macOS asks before the first
# launch of a downloaded copy.
set -eu

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
arch=$(uname -m)
dist=dist
app="$dist/Chezmate.app"
cli="$dist/cli/chezmate"
work="$dist/work"

identity=${CHEZMATE_SIGN_IDENTITY:-$(security find-identity -v -p codesigning |
    sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -n 1)}
profile=${CHEZMATE_NOTARY_PROFILE:-}
if [ -n "$profile" ] && [ -z "$identity" ]; then
    echo "Notarization needs a Developer ID Application identity." >&2
    exit 1
fi

sign() {
    if [ -n "$identity" ]; then
        codesign --force --options runtime --timestamp --sign "$identity" "$1"
    else
        codesign --force --sign - "$1"
    fi
    codesign --verify --strict "$1"
}

# Sends a zip to Apple and waits; fails unless it is accepted.
notarize() {
    result=$(xcrun notarytool submit "$1" --keychain-profile "$profile" \
        --wait --output-format json)
    id=$(printf '%s' "$result" | plutil -extract id raw -o - -)
    status=$(printf '%s' "$result" | plutil -extract status raw -o - -)
    echo "notarization $id: $status"
    if [ "$status" != "Accepted" ]; then
        xcrun notarytool log "$id" --keychain-profile "$profile" >&2
        exit 1
    fi
}

cargo build --release -p chezmate-cli -p chezmate-app

rm -rf "$dist"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$(dirname "$cli")" "$work"
cp target/release/chezmate-app "$app/Contents/MacOS/chezmate-app"
cp target/release/chezmate "$cli"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Chezmate</string>
  <key>CFBundleDisplayName</key><string>Chezmate</string>
  <key>CFBundleIdentifier</key><string>io.github.kadaliao.chezmate</string>
  <key>CFBundleExecutable</key><string>chezmate-app</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
EOF
plutil -lint "$app/Contents/Info.plist" >/dev/null
# Extended attributes would end up in the zip as ._ files inside the bundle.
xattr -cr "$app" "$cli"
sign "$app"
sign "$cli"

if [ -n "$profile" ]; then
    ditto -c -k --norsrc --noextattr --keepParent "$app" "$work/app.zip"
    notarize "$work/app.zip"
    xcrun stapler staple "$app"
    # A bare executable cannot hold a ticket; Gatekeeper looks it up online.
    ditto -c -k --norsrc --noextattr "$cli" "$work/cli.zip"
    notarize "$work/cli.zip"
fi

ditto -c -k --norsrc --noextattr --keepParent "$app" "$dist/Chezmate-$version-macos-$arch.zip"
COPYFILE_DISABLE=1 tar -czf "$dist/chezmate-$version-macos-$arch.tar.gz" -C "$(dirname "$cli")" chezmate
rm -rf "$work"
(cd "$dist" && shasum -a 256 ./*.zip ./*.tar.gz | sed 's| \./| |' > SHA256SUMS)

echo "signed by: ${identity:-ad hoc}; notarized with: ${profile:-nothing}"
spctl --assess --type execute --verbose=2 "$app" 2>&1 || true
ls -lh "$dist"
