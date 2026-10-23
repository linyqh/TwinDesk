#!/usr/bin/env bash
# Notarizes and staples a signed .app or .dmg in place.
#
#   scripts/notarize.sh <path.app|path.dmg>
#
# Credentials, in order of preference:
#   1. $OSM_NOTARY_PROFILE: a `xcrun notarytool store-credentials` profile
#   2. $APPLE_ID, $APPLE_TEAM_ID and $APPLE_APP_PASSWORD (app-specific password)
set -euo pipefail

TARGET="${1:?usage: $0 <path.app|path.dmg>}"
[[ -e "$TARGET" ]] || { echo "Not found: $TARGET" >&2; exit 1; }

if [[ -n "${OSM_NOTARY_PROFILE:-}" ]]; then
  AUTH=(--keychain-profile "$OSM_NOTARY_PROFILE")
elif [[ -n "${APPLE_ID:-}" && -n "${APPLE_TEAM_ID:-}" && -n "${APPLE_APP_PASSWORD:-}" ]]; then
  AUTH=(--apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD")
else
  echo "Set OSM_NOTARY_PROFILE, or APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD." >&2
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

case "$TARGET" in
  *.app)
    # notarytool accepts zip, pkg or dmg; the ticket is stapled to the app itself.
    UPLOAD="$WORK/$(basename "$TARGET" .app).zip"
    ditto -c -k --keepParent "$TARGET" "$UPLOAD"
    ;;
  *.dmg) UPLOAD="$TARGET" ;;
  *) echo "Expected a .app or .dmg: $TARGET" >&2; exit 64 ;;
esac

xcrun notarytool submit "$UPLOAD" "${AUTH[@]}" --wait --timeout 1h \
  --output-format json > "$WORK/result.json"
STATUS="$(plutil -extract status raw -o - "$WORK/result.json")"
if [[ "$STATUS" != "Accepted" ]]; then
  ID="$(plutil -extract id raw -o - "$WORK/result.json")"
  echo "Notarization $STATUS for $TARGET" >&2
  xcrun notarytool log "$ID" "${AUTH[@]}" >&2 || true
  exit 1
fi
xcrun stapler staple "$TARGET"
xcrun stapler validate "$TARGET"
echo "notarized $TARGET"
