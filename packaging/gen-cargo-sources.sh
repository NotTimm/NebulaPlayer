#!/usr/bin/env bash
# Generates packaging/cargo-sources.json from Cargo.lock, so the Flatpak
# can build offline. Rerun whenever Cargo.lock changes.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

# Pinned so a change upstream can't silently change what gets vendored.
GEN_REV="74697c75b630d7330e77250fc13cb5ea688d9479"
GEN_URL="https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/$GEN_REV/cargo/flatpak-cargo-generator.py"

need python3 "sudo dnf install python3  # or: sudo apt install python3"
need curl

python3 -c 'import aiohttp, tomlkit, yaml' 2>/dev/null || {
    echo "error: flatpak-cargo-generator needs aiohttp, tomlkit and PyYAML." >&2
    echo "  install with: sudo dnf install python3-aiohttp python3-tomlkit python3-pyyaml" >&2
    echo "            or: pip install --user aiohttp tomlkit pyyaml" >&2
    exit 1
}

GEN="$(mktemp)"
trap 'rm -f "$GEN"' EXIT
curl -fsSL -o "$GEN" "$GEN_URL"

OUT="$REPO_DIR/packaging/cargo-sources.json"
python3 "$GEN" "$REPO_DIR/Cargo.lock" -o "$OUT"
echo "==> $OUT"
