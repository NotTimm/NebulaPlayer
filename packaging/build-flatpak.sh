#!/usr/bin/env bash
# Builds a single-file .flatpak bundle from the manifest in this directory.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

need flatpak "sudo dnf install flatpak  # or: sudo apt install flatpak"
need flatpak-builder "sudo dnf install flatpak-builder"

VERSION="$(version)"
MANIFEST="$(dirname "${BASH_SOURCE[0]}")/$APP_ID.yml"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# The build is offline; crate sources come from cargo-sources.json.
SOURCES="$(dirname "$MANIFEST")/cargo-sources.json"
if [ ! -f "$SOURCES" ] || [ "$REPO_DIR/Cargo.lock" -nt "$SOURCES" ]; then
    echo "==> Generating cargo-sources.json"
    bash "$(dirname "${BASH_SOURCE[0]}")/gen-cargo-sources.sh"
fi

echo "==> Building (runtimes come from Flathub)"
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak-builder --force-clean --user --install-deps-from=flathub \
    --repo="$WORK/repo" \
    "$WORK/build" "$MANIFEST"

mkdir -p "$DIST_DIR"
OUT="$DIST_DIR/$PRODUCT-$VERSION-x86_64.flatpak"
flatpak build-bundle "$WORK/repo" "$OUT" "$APP_ID"
echo "==> $OUT"
echo "    install with: flatpak install --user $OUT"
