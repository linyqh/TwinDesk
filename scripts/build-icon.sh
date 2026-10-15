#!/usr/bin/env bash
# Convert the shared brand artwork into all standard macOS icon representations.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE="$ROOT/apps/macos/Sources/OpenShareMouse/Resources/TwinDesk.png"
OUTPUT="${1:-$ROOT/build/branding/TwinDesk.icns}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/twindesk-icon.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
ICONSET="$WORK/TwinDesk.iconset"
mkdir -p "$ICONSET" "$(dirname "$OUTPUT")"

for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$SOURCE" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  retina=$((size * 2))
  sips -z "$retina" "$retina" "$SOURCE" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$OUTPUT"
echo "built $OUTPUT"
