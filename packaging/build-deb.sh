#!/usr/bin/env bash
# Builds a .deb. Runtime dependencies are discovered from the binary's
# actual ELF links via dpkg-shlibdeps, so the package depends on real
# system packages (libxkbcommon0, libssl3t64, ...) rather than a
# hand-maintained list that drifts. libmpv is loaded at run time
# (src/mpv.rs), so dpkg-shlibdeps can't see it: it's added by hand.
#
# Build it on the Debian/Ubuntu release you're targeting (CI uses Ubuntu
# 24.04): the binary picks up that system's glibc/openssl symbol versions,
# and dpkg-shlibdeps needs that system's dpkg database to map libraries to
# packages.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

need dpkg-deb "sudo apt install dpkg-dev  # or: sudo dnf install dpkg"

# Only used when building on a non-Debian host, where the local dpkg
# database can't map sonames to packages. Such a .deb is fine for local
# testing but shouldn't be published.
FALLBACK_DEPS="libc6, libgcc-s1, libxkbcommon0, libssl3"
# Loaded at run time; libmpv2 on Debian 12+ and Ubuntu 23.10+.
MPV_DEPS="libmpv2 | libmpv1"

VERSION="$(version_native)"
if command -v dpkg >/dev/null 2>&1; then
    ARCH="$(dpkg --print-architecture)"
else
    case "$(uname -m)" in
        x86_64)  ARCH=amd64 ;;
        aarch64) ARCH=arm64 ;;
        *)       ARCH="$(uname -m)" ;;
    esac
fi
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
# mktemp makes the dir 0700, and dpkg-deb records it as the mode of "/".
chmod 0755 "$STAGE"

build_release
stage_tree "$STAGE"

# Debian policy: ship stripped binaries (rpmbuild does this on its own).
if command -v strip >/dev/null 2>&1; then
    strip --strip-unneeded "$STAGE/usr/bin/$APP_NAME"
fi

DEPS=""
if [ -f /etc/debian_version ]; then
    need dpkg-shlibdeps "sudo apt install dpkg-dev"
    # dpkg-shlibdeps insists on running from a source-package root with a
    # parseable debian/control present.
    mkdir -p "$STAGE/debian"
    printf 'Source: %s\n\nPackage: %s\nArchitecture: any\n' \
        "$APP_NAME" "$APP_NAME" > "$STAGE/debian/control"
    # Errors are fatal here on purpose: silently shipping the fallback list
    # from a real Debian/Ubuntu build would hide a broken package.
    shlibs="$(cd "$STAGE" && dpkg-shlibdeps -O "usr/bin/$APP_NAME")"
    rm -rf "$STAGE/debian"
    DEPS="$(sed -nE 's/^shlibs:Depends=//p' <<<"$shlibs")"
    [ -n "$DEPS" ] || { echo "error: dpkg-shlibdeps found no dependencies" >&2; exit 1; }
else
    echo "==> Not a Debian-based host; using the curated dependency list"
    echo "    (fine for testing; build on Debian/Ubuntu for a release .deb)"
    DEPS="$FALLBACK_DEPS"
fi
DEPS="$DEPS, $MPV_DEPS"
echo "==> Depends: $DEPS"
# winit dlopen()s libxkbcommon-x11 for X11 sessions, so dpkg-shlibdeps
# can't see it; it's listed as Recommends in the control file below.

INSTALLED_SIZE="$(du -sk --apparent-size "$STAGE" | cut -f1)"

mkdir -p "$STAGE/DEBIAN"
cat > "$STAGE/DEBIAN/control" <<CONTROL
Package: $APP_NAME
Version: $VERSION
Section: video
Priority: optional
Architecture: $ARCH
Installed-Size: $INSTALLED_SIZE
Depends: $DEPS
Recommends: libxkbcommon-x11-0
Maintainer: Tim Singel <gamer.tdogtim@gmail.com>
Homepage: https://github.com/NotTimm/NebulaPlayer
Description: Native Jellyfin and local video player
 A fast, native video player for Jellyfin servers and local files, built
 on libcosmic and mpv: Jellyfin's home screen, libraries, shows and
 movies rebuilt as native pages, with playback, resume, intro skipping
 and progress sync through mpv.
CONTROL

mkdir -p "$DIST_DIR"
OUT="$DIST_DIR/${APP_NAME}_${VERSION}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "$STAGE" "$OUT"
echo "==> $OUT"
