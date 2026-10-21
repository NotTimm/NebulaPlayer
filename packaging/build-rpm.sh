#!/usr/bin/env bash
# Builds an .rpm. rpmbuild's automatic dependency generator reads the
# binary's ELF links and emits Requires on the providing system packages
# (libxkbcommon.so.0, libssl.so.3, ...). libmpv is loaded at run time
# (src/mpv.rs), so its Requires is written out below.
#
# The binary is built with cargo outside rpmbuild and staged into a tree;
# the spec's %install just copies that tree into %{buildroot}. (Passing a
# pre-filled --buildroot is fragile: rpm >= 4.20 manages the buildroot
# itself and may wipe or relocate it.)
#
# This is the GitHub release's .rpm. The Fedora/COPR package is built
# from packaging/fedora/nebula-player.spec instead.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

need rpmbuild "sudo dnf install rpm-build"

VERSION="$(version_native)"
# Release builds use 1; pass RPM_RELEASE (e.g. a timestamp) for local
# rebuilds so each sorts newer and dnf treats it as an upgrade.
RELEASE="${RPM_RELEASE:-1}"
TOP="$(mktemp -d)"
trap 'rm -rf "$TOP"' EXIT

build_release

STAGE="$TOP/stage"
stage_tree "$STAGE"

mkdir -p "$TOP/SPECS"
cat > "$TOP/SPECS/$APP_NAME.spec" <<SPEC
# Binaries are built outside rpmbuild and staged, so turn off the steps
# that expect sources/debuginfo here.
%global debug_package %{nil}
%define _build_id_links none

Name:           $APP_NAME
Version:        $VERSION
Release:        $RELEASE%{?dist}
Summary:        Native Jellyfin and local video player
License:        GPL-3.0-only
URL:            https://github.com/NotTimm/NebulaPlayer
# Loaded at run time, so the automatic requires miss it.
Requires:       mpv-libs
# dlopen()ed by winit on X11 sessions, so the automatic requires miss it.
Recommends:     libxkbcommon-x11

%description
A fast, native video player for Jellyfin servers and local files, built
on libcosmic and mpv: Jellyfin's home screen, libraries, shows and movies
rebuilt as native pages, with playback, resume, intro skipping and
progress sync through mpv.

%install
cp -a "$STAGE/." "%{buildroot}/"

%files
%{_bindir}/$APP_NAME
%{_datadir}/applications/$APP_ID.desktop
%{_datadir}/icons/hicolor/*/apps/$APP_ID.*
%{_datadir}/metainfo/$APP_ID.metainfo.xml
%doc %{_docdir}/$APP_NAME/README.md

%changelog
* $(LC_ALL=C date '+%a %b %d %Y') Tim Singel <gamer.tdogtim@gmail.com> - $VERSION-$RELEASE
- Built from git $(cd "$REPO_DIR" && git rev-parse --short HEAD 2>/dev/null || echo unknown).
SPEC

rpmbuild \
    --define "_topdir $TOP" \
    -bb "$TOP/SPECS/$APP_NAME.spec"

mkdir -p "$DIST_DIR"
built=()
while IFS= read -r -d '' rpm; do
    cp "$rpm" "$DIST_DIR/"
    built+=("$DIST_DIR/$(basename "$rpm")")
done < <(find "$TOP/RPMS" -name '*.rpm' -print0)
[ "${#built[@]}" -gt 0 ] || { echo "error: rpmbuild produced no package" >&2; exit 1; }
for f in "${built[@]}"; do echo "==> $f"; done
