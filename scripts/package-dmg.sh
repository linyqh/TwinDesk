#!/usr/bin/env bash
# Package an already-notarized app without changing its signature or ticket.
# Usage: scripts/package-dmg.sh [app-path] [output-dmg]
# One-time setup: uv venv build/dmg-tools && uv pip install \
#   --python build/dmg-tools/bin/python ds-store==1.3.3 mac-alias==2.2.3
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="${1:-$ROOT/build/dist/notarized/同桌 · TwinDesk.app}"
PYTHON="$ROOT/build/dmg-tools/bin/python"
IDENTITY="${OSM_SIGN_IDENTITY:-}"

[[ -d "$APP" && -x "$PYTHON" ]] || {
  echo "A notarized app and build/dmg-tools Python environment are required." >&2
  exit 1
}
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
OUT="${2:-$ROOT/build/dist/TwinDesk-$VERSION-arm64-installer.dmg}"
[[ ! -e "$OUT" ]] || { echo "Output already exists: $OUT" >&2; exit 1; }
if [[ -z "$IDENTITY" ]]; then
  IDENTITY="$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -1)"
fi
[[ -n "$IDENTITY" ]] || { echo "Developer ID certificate required." >&2; exit 1; }
codesign --verify --deep --strict "$APP"
xcrun stapler validate "$APP"
mkdir -p "$ROOT/build/dist" "$(dirname "$OUT")"
WORK="$(mktemp -d "$ROOT/build/dist/dmg-work.XXXXXX")"
MOUNT="$WORK/volume"
MOUNTED=0
cleanup() {
  if [[ "$MOUNTED" == 1 ]]; then hdiutil detach "$MOUNT" >/dev/null || true; fi
  rm -rf "$WORK"
}
trap cleanup EXIT

mkdir -p "$WORK/staging/.background" "$MOUNT"
ditto "$APP" "$WORK/staging/$(basename "$APP")"
ln -s /Applications "$WORK/staging/Applications"
cp "$ROOT/assets/dmg/background.tiff" "$WORK/staging/.background/"
hdiutil create -size 32m -volname '同桌 · TwinDesk' -srcfolder "$WORK/staging" \
  -format UDRW -fs HFS+ "$WORK/installer.dmg"
hdiutil attach -readwrite -nobrowse -mountpoint "$MOUNT" "$WORK/installer.dmg"
MOUNTED=1
"$PYTHON" "$ROOT/scripts/layout-dmg.py" "$MOUNT" "$(basename "$APP")"
sync
hdiutil detach "$MOUNT"
MOUNTED=0
hdiutil convert "$WORK/installer.dmg" -format UDZO -imagekey zlib-level=9 -o "$OUT"
codesign --force --timestamp --sign "$IDENTITY" "$OUT"
codesign --verify --strict --verbose=2 "$OUT"
hdiutil verify "$OUT"
shasum -a 256 "$OUT" > "$OUT.sha256"
echo "Built $OUT"
