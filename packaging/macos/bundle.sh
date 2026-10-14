#!/usr/bin/env bash
# Builds "Axi Launcher.app" and a .dmg for Apple Silicon. Run on a Mac from the repository root:
#   packaging/macos/bundle.sh
# Output: target/aarch64-apple-darwin/release/bundle/ (.dmg for users, .app.tar.gz for updates)
set -euo pipefail

TARGET=aarch64-apple-darwin
VERSION=$(cargo pkgid -p axi-app | sed 's/.*[#@]//')
OUT="target/$TARGET/release/bundle"
APP="$OUT/Axi Launcher.app"

cargo build --release --locked -p axi-app --target "$TARGET"

rm -rf "$OUT"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/$TARGET/release/axi-launcher" "$APP/Contents/MacOS/"
sed "s/@VERSION@/$VERSION/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"

# .icns from the 256px PNG icon.
ICONSET="$OUT/axi.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256; do
    sips -z $size $size crates/app/assets/axi-icon.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    [ $double -le 256 ] && sips -z $double $double crates/app/assets/axi-icon.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/axi.icns"
rm -rf "$ICONSET"

# Ad-hoc signature: arm64 binaries must be signed to run at all. Without a Developer ID,
# Gatekeeper still asks users to confirm the first launch (right-click > Open).
codesign --force --deep --sign - "$APP"

# The in-app updater downloads this archive and swaps the whole bundle.
# COPYFILE_DISABLE keeps macOS tar from adding ._ metadata files.
COPYFILE_DISABLE=1 tar -czf "$OUT/Axi-Launcher-$VERSION-macos-arm64.app.tar.gz" -C "$OUT" "Axi Launcher.app"

DMG="$OUT/Axi-Launcher-$VERSION-macos-arm64.dmg"
STAGE="$OUT/dmg"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "Axi Launcher" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
echo "Built $DMG"
