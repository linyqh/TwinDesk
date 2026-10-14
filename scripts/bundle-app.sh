#!/usr/bin/env bash
# Builds the Swift app and wraps it into a signed OpenShareMouse.app.
#
#   scripts/bundle-app.sh [debug|release]   (default: debug)
#
# Signing identity, in order of preference:
#   1. $OSM_SIGN_IDENTITY
#   2. the first "Developer ID Application" identity in the keychain
#   3. ad-hoc ("-"), with a warning
#
# Use a real certificate even for development: TCC records grants such as
# Accessibility against the code signature, so ad-hoc builds lose their
# grants on every rebuild.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG="${1:-debug}"
APP_DIR="$ROOT/apps/macos"
OUT="$ROOT/build/app"
APP="$OUT/OpenShareMouse.app"

BUNDLE_ID="${OSM_BUNDLE_ID:-io.github.opensharemouse.OpenShareMouse}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
BUILD_NUMBER="${OSM_BUILD_NUMBER:-$(git -C "$ROOT" rev-list --count HEAD 2>/dev/null || echo 1)}"

case "$CONFIG" in
  debug | release) ;;
  *) echo "usage: $0 [debug|release]" >&2; exit 64 ;;
esac

if [[ ! -d "$APP_DIR/Frameworks/OSMCoreFFI.xcframework" ]]; then
  "$ROOT/scripts/build-rust.sh" "$CONFIG"
fi

swift build --package-path "$APP_DIR" -c "$CONFIG" --product OpenShareMouse
BIN="$(swift build --package-path "$APP_DIR" -c "$CONFIG" --show-bin-path)/OpenShareMouse"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/OpenShareMouse"
sed -e "s/\$(BUNDLE_ID)/$BUNDLE_ID/" \
    -e "s/\$(VERSION)/$VERSION/" \
    -e "s/\$(BUILD_NUMBER)/$BUILD_NUMBER/" \
    "$APP_DIR/Support/Info.plist" > "$APP/Contents/Info.plist"
plutil -lint -s "$APP/Contents/Info.plist"

IDENTITY="${OSM_SIGN_IDENTITY:-}"
if [[ -z "$IDENTITY" ]]; then
  IDENTITY="$(security find-identity -v -p codesigning \
    | sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -1)"
fi
if [[ -z "$IDENTITY" ]]; then
  echo "warning: no Developer ID identity found; signing ad-hoc." >&2
  echo "         Permission grants will not survive rebuilds." >&2
  IDENTITY="-"
fi

# Notarization requires a secure timestamp; skip it for debug builds so
# signing works offline and stays fast.
if [[ "$CONFIG" == "release" && "$IDENTITY" != "-" ]]; then
  TIMESTAMP="--timestamp"
else
  TIMESTAMP="--timestamp=none"
fi
codesign --force --options runtime "$TIMESTAMP" \
  --entitlements "$APP_DIR/Support/OpenShareMouse.entitlements" \
  --sign "$IDENTITY" "$APP"
codesign --verify --strict --verbose=2 "$APP"

echo "built $APP"
echo "signed with: $IDENTITY"
