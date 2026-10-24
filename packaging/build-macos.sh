#!/usr/bin/env bash
# Builds "Nebula Player.app" and a drag-to-Applications .dmg for the Mac
# this runs on (Apple Silicon on CI). Needs Homebrew's mpv and
# dylibbundler: `brew install mpv dylibbundler`.
#
# On macOS the app plays through the mpv program (libmpv can't open its
# own window there; see src/mpv.rs), so Homebrew's mpv is copied into the
# bundle (Contents/MacOS/mpv, found next to the app's binary) with every
# library it links (Contents/Frameworks), and runs without Homebrew. The
# app is signed ad hoc, not with a Developer ID, so Gatekeeper asks once
# before the first launch (see README).
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

need iconutil "it ships with Xcode's command line tools"
need hdiutil
need codesign
need dylibbundler "brew install dylibbundler"
need brew "https://brew.sh"

VERSION="$(version)"
ARCH="$(uname -m)" # arm64 or x86_64
# CFBundleShortVersionString must be plain numbers: 0.1.0-alpha.1 -> 0.1.0.
SHORT_VERSION="${VERSION%%-*}"

MPV_BIN="${MPV_BIN:-$(brew --prefix mpv)/bin/mpv}"
[ -x "$MPV_BIN" ] || { echo "error: mpv not found at $MPV_BIN (brew install mpv)" >&2; exit 1; }

build_release

STAGE="$DIST_DIR/macos"
APP="$STAGE/Nebula Player.app"
rm -rf "$STAGE"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"

echo "==> Assembling $(basename "$APP")"
install -m0755 "$BIN_PATH" "$APP/Contents/MacOS/$APP_NAME"
install -m0755 "$MPV_BIN" "$APP/Contents/MacOS/mpv"

ICONSET="$STAGE/NebulaPlayer.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
    cp "$REPO_DIR/res/icons/hicolor/${size}x${size}/apps/$APP_ID.png" "$ICONSET/icon_${size}x${size}.png"
    double=$((size * 2))
    if [ -f "$REPO_DIR/res/icons/hicolor/${double}x${double}/apps/$APP_ID.png" ]; then
        cp "$REPO_DIR/res/icons/hicolor/${double}x${double}/apps/$APP_ID.png" "$ICONSET/icon_${size}x${size}@2x.png"
    fi
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/NebulaPlayer.icns"
rm -rf "$ICONSET"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Nebula Player</string>
    <key>CFBundleDisplayName</key>
    <string>Nebula Player</string>
    <key>CFBundleIdentifier</key>
    <string>$APP_ID</string>
    <key>CFBundleExecutable</key>
    <string>$APP_NAME</string>
    <key>CFBundleIconFile</key>
    <string>NebulaPlayer</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$SHORT_VERSION</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.video</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>© Tim Singel, GPL-3.0</string>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist"

echo "==> Bundling shared libraries"
# --search-path resolves @rpath links in Homebrew's libraries, so
# dylibbundler never stops to ask.
dylibbundler --overwrite-dir --bundle-deps --create-dir \
    --fix-file "$APP/Contents/MacOS/$APP_NAME" \
    --fix-file "$APP/Contents/MacOS/mpv" \
    --dest-dir "$APP/Contents/Frameworks" \
    --install-path "@executable_path/../Frameworks/" \
    --search-path "$(brew --prefix)/lib" </dev/null
# Anything still pointing outside the system would break without Homebrew.
shopt -s nullglob
libs=("$APP"/Contents/Frameworks/*.dylib)
leftover="$(otool -L "$APP/Contents/MacOS/$APP_NAME" "$APP/Contents/MacOS/mpv" "${libs[@]}" |
    grep -E '^\s+/(opt|usr/local)/' || true)"
if [ -n "$leftover" ]; then
    echo "error: libraries still linked from outside the bundle:" >&2
    echo "$leftover" >&2
    exit 1
fi
echo "==> Bundled ${#libs[@]} libraries"

echo "==> Signing (ad hoc)"
# Rewriting install names invalidated the libraries' signatures; Apple
# Silicon refuses to load unsigned code, so sign inside-out.
for lib in "${libs[@]}"; do
    codesign --force --sign - "$lib"
done
codesign --force --sign - "$APP/Contents/MacOS/mpv"
codesign --force --sign - "$APP"
codesign --verify --strict --verbose=2 "$APP"

echo "==> Checking the bundled mpv runs"
"$APP/Contents/MacOS/$APP_NAME" --check-mpv

echo "==> Packing the in-app update"
# What the app downloads to update itself (src/updater.rs): the signed .app
# as is. COPYFILE_DISABLE keeps macOS's ._ metadata files out of it.
UPDATE="$DIST_DIR/$PRODUCT-$VERSION-macos-$ARCH.app.tar.gz"
rm -f "$UPDATE"
COPYFILE_DISABLE=1 tar -czf "$UPDATE" -C "$STAGE" "$(basename "$APP")"
echo "==> $UPDATE"

echo "==> Building the disk image"
DMG_ROOT="$STAGE/dmg"
mkdir -p "$DMG_ROOT"
cp -R "$APP" "$DMG_ROOT/"
ln -s /Applications "$DMG_ROOT/Applications"
OUT="$DIST_DIR/$PRODUCT-$VERSION-macos-$ARCH.dmg"
rm -f "$OUT"
# hdiutil sometimes fails with "Resource busy" on CI Macs (something still
# holds the freshly written files); it goes through on a retry.
for attempt in 1 2 3 4 5; do
    if hdiutil create -volname "Nebula Player" -srcfolder "$DMG_ROOT" -fs HFS+ -format UDZO -ov "$OUT"; then
        break
    fi
    [ "$attempt" = 5 ] && { echo "hdiutil create kept failing" >&2; exit 1; }
    echo "hdiutil create failed (attempt $attempt), retrying in 5 s"
    sleep 5
done
rm -rf "$DMG_ROOT"

echo "==> $OUT"
