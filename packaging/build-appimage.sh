#!/usr/bin/env bash
# Builds an AppImage: a single self-contained executable that runs on any
# reasonably recent glibc distro. Unlike deb/rpm this bundles the
# non-system shared libraries the app needs, libmpv (loaded at run time)
# and everything libmpv links, so it can't rely on distro packages.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

need ldd

VERSION="$(version)"
ARCH="$(uname -m)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# libmpv from the build machine (CI: Ubuntu 24.04's libmpv2).
LIBMPV="${LIBMPV:-$(ldconfig -p | awk '/libmpv\.so\.2 / {print $NF; exit}')}"
if [ -z "$LIBMPV" ] || [ ! -f "$LIBMPV" ]; then
    echo "error: libmpv.so.2 not found (sudo apt install libmpv2, or set LIBMPV)" >&2
    exit 1
fi

# appimagetool isn't packaged by most distros; fetch it on demand and cache
# it next to the dist output.
TOOL="$DIST_DIR/appimagetool-$ARCH.AppImage"
if [ ! -x "$TOOL" ]; then
    echo "==> Downloading appimagetool"
    mkdir -p "$DIST_DIR"
    curl -fL -o "$TOOL" \
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
    chmod +x "$TOOL"
fi

build_release

APPDIR="$WORK/AppDir"
install -Dm0755 "$BIN_PATH" "$APPDIR/usr/bin/$APP_NAME"
install -Dm0644 "$REPO_DIR/res/$APP_ID.desktop" "$APPDIR/usr/share/applications/$APP_ID.desktop"
install -Dm0644 "$REPO_DIR/res/$APP_ID.metainfo.xml" \
    "$APPDIR/usr/share/metainfo/$APP_ID.metainfo.xml"
install_icons "$APPDIR/usr/share/icons/hicolor"

# AppImage expects the desktop file and icon at the AppDir root too.
cp "$APPDIR/usr/share/applications/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
cp "$APPDIR/usr/share/icons/hicolor/256x256/apps/$APP_ID.png" "$APPDIR/$APP_ID.png"
ln -sf "$APP_ID.png" "$APPDIR/.DirIcon"

# Bundles a library unless it has to come from the host: glibc, the
# graphics and video-acceleration drivers' libraries, the audio servers'
# client libraries (their plugins come from the host and must match), and
# fonts (so subtitles use the host's font setup).
mkdir -p "$APPDIR/usr/lib"
bundle() {
    local lib="$1"
    case "$(basename "$lib")" in
        libc.so.*|libm.so.*|libdl.so.*|libpthread.so.*|librt.so.*|ld-linux*|libresolv.so.*|libutil.so.*) return ;;
        libgcc_s.so.*|libstdc++.so.*) return ;;
        libGL*|libEGL*|libOpenGL*|libgbm*|libdrm*|libvulkan*) return ;;
        libX11*|libXext*|libXrandr*|libXss*|libXinerama*|libXpresent*|libXfixes*|libXrender*|libXau*|libXdmcp*|libXv*|libxcb*|libwayland*|libxkbcommon*) return ;;
        libva*|libvdpau*) return ;;
        libasound*|libpulse*|libpipewire*|libjack*) return ;;
        libfontconfig*|libfreetype*|libexpat*|libz.so.*|libdbus*|libsystemd*|libudev*) return ;;
    esac
    if [ -f "$lib" ]; then
        cp -Ln "$lib" "$APPDIR/usr/lib/"
    fi
}
while read -r lib; do bundle "$lib"; done < <(ldd "$APPDIR/usr/bin/$APP_NAME" | awk '/=> \// {print $3}')
cp -L "$LIBMPV" "$APPDIR/usr/lib/libmpv.so.2"
while read -r lib; do bundle "$lib"; done < <(ldd "$LIBMPV" | awk '/=> \// {print $3}')
echo "==> Bundled $(find "$APPDIR/usr/lib" -name '*.so*' | wc -l) libraries"

cat > "$APPDIR/AppRun" <<'RUN'
#!/usr/bin/env bash
HERE="$(dirname "$(readlink -f "$0")")"
export LD_LIBRARY_PATH="$HERE/usr/lib:${LD_LIBRARY_PATH:-}"
export XDG_DATA_DIRS="$HERE/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
exec "$HERE/usr/bin/nebula-player" "$@"
RUN
chmod +x "$APPDIR/AppRun"

# Checks the AppDir before packing: libmpv loads with only the bundled and
# host libraries (no missing link), and nothing the app links is missing.
if LD_LIBRARY_PATH="$APPDIR/usr/lib" ldd "$APPDIR/usr/lib/libmpv.so.2" "$APPDIR/usr/bin/$APP_NAME" | grep 'not found'; then
    echo "error: libraries missing from the AppImage" >&2
    exit 1
fi

if [ -n "${APPDIR_ONLY:-}" ]; then
    rm -rf "$APPDIR_ONLY"
    cp -a "$APPDIR" "$APPDIR_ONLY"
    echo "==> $APPDIR_ONLY (AppDir only, not packed)"
    exit 0
fi

mkdir -p "$DIST_DIR"
OUT="$DIST_DIR/$PRODUCT-$VERSION-$ARCH.AppImage"
# APPIMAGE_EXTRACT_AND_RUN lets appimagetool run without FUSE (CI, containers).
APPIMAGE_EXTRACT_AND_RUN=1 ARCH="$ARCH" "$TOOL" --no-appstream "$APPDIR" "$OUT"
echo "==> $OUT"
