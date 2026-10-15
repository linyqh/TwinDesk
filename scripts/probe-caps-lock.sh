#!/usr/bin/env bash
# Runs the target-side Caps Lock operations in a signed bundle.
# Temporarily changes this Mac's input source and lock state, then restores them.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
IDENTITY="${OSM_SIGN_IDENTITY:-}"
if [[ -z "$IDENTITY" ]]; then
  IDENTITY="$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -1)"
fi
if [[ -z "$IDENTITY" ]]; then
  echo "Developer ID unavailable; using an ad-hoc signature for this local probe." >&2
  IDENTITY="-"
fi
cargo build -p osm-platform-macos --example caps_lock_probe
PROBE_APP="$ROOT/build/probes/CapsLockProbe.app"
mkdir -p "$PROBE_APP/Contents/MacOS"
cp target/debug/examples/caps_lock_probe "$PROBE_APP/Contents/MacOS/CapsLockProbe"
cat > "$PROBE_APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>io.github.opensharemouse.CapsLockProbe</string>
<key>CFBundleName</key><string>CapsLockProbe</string>
<key>CFBundleExecutable</key><string>CapsLockProbe</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSUIElement</key><true/>
</dict></plist>
PLIST
codesign --force --options runtime --timestamp=none --sign "$IDENTITY" "$PROBE_APP"
codesign --verify --strict "$PROBE_APP"
"$PROBE_APP/Contents/MacOS/CapsLockProbe"
