#!/bin/sh
# Builds the release binaries and packs them for a GitHub release:
#   dist/Chezmate-<version>-macos-<arch>.zip      the desktop app
#   dist/chezmate-<version>-macos-<arch>.tar.gz   the command line (and MCP server)
#   dist/SHA256SUMS
# The app is signed ad hoc, not notarized: macOS asks before the first launch.
set -eu

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
arch=$(uname -m)
dist=dist
app="$dist/Chezmate.app"

cargo build --release -p chezmate-cli -p chezmate-app

rm -rf "$dist"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/chezmate-app "$app/Contents/MacOS/chezmate-app"
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
xattr -cr "$app"
codesign --force --sign - "$app"
codesign --verify --strict "$app"

ditto -c -k --norsrc --noextattr --keepParent "$app" "$dist/Chezmate-$version-macos-$arch.zip"
tar -czf "$dist/chezmate-$version-macos-$arch.tar.gz" -C target/release chezmate
(cd "$dist" && shasum -a 256 ./*.zip ./*.tar.gz | sed 's| \./| |' > SHA256SUMS)

ls -lh "$dist"
