#!/usr/bin/env bash
# Builds dist/nebula-player-<version>-vendor.tar.xz: every crate the app
# uses, from crates.io and git (the libcosmic fork and what it pulls in),
# for the Fedora spec's offline build. Fedora's %cargo_prep only points
# crates.io at vendor/, so the git sources' entries ride along in
# cargo-vendor-git.toml for %prep to append to .cargo/config.toml.
set -euo pipefail
# shellcheck source=packaging/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"
need cargo

VERSION="$(version)"
TOP="$(mktemp -d)"
trap 'rm -rf "$TOP"' EXIT

echo "==> Vendoring crates"
(cd "$REPO_DIR" && cargo vendor --locked --versioned-dirs "$TOP/vendor") > "$TOP/config.toml"

# Keep only the [source."git+..."] tables; the crates-io and directory ones
# come from %cargo_prep.
awk '
    /^\[/ { keep = ($0 ~ /^\[source\."git\+/) }
    keep
' "$TOP/config.toml" > "$TOP/cargo-vendor-git.toml"
grep -q '^\[source\."git+' "$TOP/cargo-vendor-git.toml" || {
    echo "error: no git sources in cargo vendor's output" >&2
    exit 1
}

mkdir -p "$DIST_DIR"
out="$DIST_DIR/$APP_NAME-$VERSION-vendor.tar.xz"
XZ_OPT="-T0 -6" tar -C "$TOP" --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner \
    -cJf "$out" vendor cargo-vendor-git.toml
echo "==> $out"
