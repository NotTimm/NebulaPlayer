# Fedora / COPR spec: builds from the release's source tarball plus its
# vendor tarball (packaging/fedora/make-vendor.sh, attached to every
# release), offline, with Fedora's Rust macros. The GitHub .rpm comes from
# packaging/build-rpm.sh instead. Keep Version in step with Cargo.toml, with
# '~' for Cargo's '-' (the release workflow checks).

# Tests use temporary folders only; no network.
%bcond check 1

Name:           nebula-player
Version:        0.1.0~alpha.3
Release:        1%{?dist}
Summary:        Native Jellyfin and local video player

# Cargo.toml's version: 0.1.0~alpha.1 -> 0.1.0-alpha.1 (the git tag and
# the release's file names).
%global upstream_version %(echo %{version} | tr '~' '-')

SourceLicense:  GPL-3.0-only
# The app (GPL-3.0-only) plus the crates built into it, from
# %%cargo_license_summary in the build log; refresh when dependencies change:
# (Apache-2.0 OR MIT) AND BSD-3-Clause
# (MIT OR Apache-2.0) AND Unicode-3.0
# 0BSD OR MIT OR Apache-2.0
# Apache-2.0
# Apache-2.0 AND ISC
# Apache-2.0 AND MIT
# Apache-2.0 OR BSL-1.0
# Apache-2.0 OR GPL-2.0-only
# Apache-2.0 OR ISC OR MIT
# Apache-2.0 OR MIT
# Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT
# BSD-2-Clause
# BSD-2-Clause OR Apache-2.0 OR MIT
# BSD-3-Clause
# BSD-3-Clause OR Apache-2.0
# CC0-1.0
# GPL-3.0-only
# ISC
# MIT
# MIT OR Apache-2.0
# MIT OR Apache-2.0 OR Zlib
# MIT OR LGPL-3.0-or-later
# MIT OR Zlib OR Apache-2.0
# MPL-2.0
# Unicode-3.0
# Unlicense OR MIT
# Zlib
# Zlib OR Apache-2.0 OR MIT
License:        (Apache-2.0 OR MIT) AND BSD-3-Clause AND (MIT OR Apache-2.0) AND Unicode-3.0 AND (0BSD OR MIT OR Apache-2.0) AND Apache-2.0 AND (Apache-2.0 AND ISC) AND (Apache-2.0 AND MIT) AND (Apache-2.0 OR BSL-1.0) AND (Apache-2.0 OR GPL-2.0-only) AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR MIT) AND (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT) AND BSD-2-Clause AND (BSD-2-Clause OR Apache-2.0 OR MIT) AND BSD-3-Clause AND (BSD-3-Clause OR Apache-2.0) AND CC0-1.0 AND GPL-3.0-only AND ISC AND MIT AND (MIT OR Apache-2.0) AND (MIT OR Apache-2.0 OR Zlib) AND (MIT OR LGPL-3.0-or-later) AND (MIT OR Zlib OR Apache-2.0) AND MPL-2.0 AND Unicode-3.0 AND (Unlicense OR MIT) AND Zlib AND (Zlib OR Apache-2.0 OR MIT)
URL:            https://github.com/NotTimm/NebulaPlayer
Source0:        %{url}/archive/v%{upstream_version}/NebulaPlayer-%{upstream_version}.tar.gz
Source1:        %{url}/releases/download/v%{upstream_version}/%{name}-%{upstream_version}-vendor.tar.xz

%global app_id io.github.NotTimm.NebulaPlayer

BuildRequires:  cargo-rpm-macros >= 26
BuildRequires:  gcc
BuildRequires:  gcc-c++
BuildRequires:  pkgconfig(xkbcommon)
BuildRequires:  pkgconfig(wayland-client)
BuildRequires:  pkgconfig(openssl)
BuildRequires:  desktop-file-utils
BuildRequires:  libappstream-glib

# libmpv is loaded at run time (src/mpv.rs), so the automatic requires
# miss it.
Requires:       mpv-libs
# dlopen()ed by winit on X11 sessions, so the automatic requires miss it.
Recommends:     libxkbcommon-x11

%description
Nebula Player is a fast, native video player for Jellyfin servers and
local files. Jellyfin's home screen (featured titles, Continue Watching,
Next Up, Recently Added), libraries, shows and movies are native pages,
and everything plays in mpv: resume, the server's audio and subtitle
choices, intro and credits skipping, the next episode, and progress kept
in sync with the server.

%prep
%autosetup -n NebulaPlayer-%{upstream_version} -p1 -a1
%cargo_prep -v vendor
# %%cargo_prep only redirects crates.io; libcosmic and its git
# dependencies are vendored too.
cat cargo-vendor-git.toml >> .cargo/config.toml

%build
%cargo_build
%{cargo_license_summary}
%{cargo_license} > LICENSE.dependencies
%{cargo_vendor_manifest}

%install
install -Dpm0755 target/rpm/%{name} %{buildroot}%{_bindir}/%{name}
install -Dpm0644 res/%{app_id}.desktop %{buildroot}%{_datadir}/applications/%{app_id}.desktop
install -Dpm0644 res/%{app_id}.metainfo.xml %{buildroot}%{_metainfodir}/%{app_id}.metainfo.xml
for icon in res/icons/hicolor/*/apps/%{app_id}.*; do
    install -Dpm0644 "$icon" "%{buildroot}%{_datadir}/icons/${icon#res/icons/}"
done

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{app_id}.desktop
appstream-util validate-relax --nonet %{buildroot}%{_metainfodir}/%{app_id}.metainfo.xml
%if %{with check}
%cargo_test
%endif

%files
%license LICENSE LICENSE.dependencies cargo-vendor.txt
%doc README.md
%{_bindir}/%{name}
%{_datadir}/applications/%{app_id}.desktop
%{_metainfodir}/%{app_id}.metainfo.xml
%{_datadir}/icons/hicolor/*/apps/%{app_id}.*

%changelog
* Thu Oct 22 2026 Tim Singel <gamer.tdogtim@gmail.com> - 0.1.0~alpha.3-1
- SyncPlay (watch together), one native title bar on Windows

* Wed Oct 21 2026 Tim Singel <gamer.tdogtim@gmail.com> - 0.1.0~alpha.2-1
- Video inside the app with Jellyfin-style controls, trickplay previews,
  library A-Z bar and search, smoother scrolling, desktop accent colour

* Wed Oct 21 2026 Tim Singel <gamer.tdogtim@gmail.com> - 0.1.0~alpha.1-1
- First Fedora spec
