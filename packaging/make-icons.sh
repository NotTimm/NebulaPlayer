#!/usr/bin/env bash
# Generates every icon the app's packages use from one master,
# res/icons/source/nebula-player.svg. Outputs are committed, so building
# never needs these tools:
#
#   res/icons/hicolor/<N>x<N>/apps/io.github.NotTimm.NebulaPlayer.png  (Linux, macOS)
#   res/icons/hicolor/scalable/apps/io.github.NotTimm.NebulaPlayer.svg (Linux)
#   res/icons/io.github.NotTimm.NebulaPlayer.ico                       (Windows exe, installer)
#
# Needs rsvg-convert and ImageMagick.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

need rsvg-convert "sudo dnf install librsvg2-tools  # or: sudo apt install librsvg2-bin"
need convert "sudo dnf install ImageMagick  # or: sudo apt install imagemagick"

ICONS="$REPO_DIR/res/icons"
MASTER="$ICONS/source/nebula-player.svg"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "==> Linux icon theme sizes"
rm -rf "$ICONS/hicolor"
for s in 16 24 32 48 64 128 256 512 1024; do
    dir="$ICONS/hicolor/${s}x${s}/apps"
    mkdir -p "$dir"
    rsvg-convert -w "$s" -h "$s" "$MASTER" -o "$dir/$APP_ID.png"
done
install -Dm0644 "$MASTER" "$ICONS/hicolor/scalable/apps/$APP_ID.svg"

echo "==> Windows .ico"
for s in 16 20 24 32 40 48 64 128 256; do
    rsvg-convert -w "$s" -h "$s" "$MASTER" -o "$WORK/$s.png"
done
convert "$WORK"/{16,20,24,32,40,48,64,128,256}.png "$ICONS/$APP_ID.ico"

echo "==> Done. Preview: res/icons/hicolor/*/apps/$APP_ID.png"
