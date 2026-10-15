#!/usr/bin/env bash
# Compile the native macOS icon and its legacy ICNS fallback (Xcode 26+).
# Assets.car must stay next to the output ICNS inside Contents/Resources.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE="$ROOT/apps/macos/Sources/OpenShareMouse/Resources/TwinDesk.png"
OUTPUT="${1:-$ROOT/build/branding/TwinDesk.icns}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/twindesk-icon.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
ICON="$WORK/TwinDesk.icon"
COMPILED="$WORK/compiled"
mkdir -p "$ICON/Assets" "$COMPILED" "$(dirname "$OUTPUT")"

# Keep the shared PNG as the single artwork source. The composition expands
# its transparent margins beyond the system mask, avoiding a tile inside a tile.
cp "$ROOT/apps/macos/Support/TwinDesk.icon/icon.json" "$ICON/icon.json"
sips -z 1024 1024 "$SOURCE" --out "$ICON/Assets/TwinDesk.png" >/dev/null
xcrun actool "$ICON" \
  --compile "$COMPILED" \
  --output-format human-readable-text \
  --app-icon TwinDesk \
  --include-all-app-icons \
  --target-device mac \
  --platform macosx \
  --minimum-deployment-target 14.0 \
  --output-partial-info-plist "$COMPILED/PartialInfo.plist"

cp "$COMPILED/TwinDesk.icns" "$OUTPUT"
cp "$COMPILED/Assets.car" "$(dirname "$OUTPUT")/Assets.car"
echo "built $OUTPUT"
