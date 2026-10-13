#!/usr/bin/env bash
# Builds the Rust core as a static xcframework and generates its Swift
# bindings for the macOS app.
#
#   scripts/build-rust.sh [debug|release]   (default: release)
#
# Outputs (both git-ignored):
#   apps/macos/Frameworks/OSMCoreFFI.xcframework
#   apps/macos/Sources/OSMCore/Generated/OSMCore.swift
#
# Set OSM_UNIVERSAL=1 to also build x86_64 and lipo a universal library.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="${1:-release}"
export PATH="$HOME/.cargo/bin:$PATH"
# Xcode build phases inherit a minimal environment; keep macOS deployment
# target aligned with the app (report §1.1: macOS 14+).
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"

# A flag string, not an array: macOS ships bash 3.2, where expanding an
# empty array under `set -u` is an error.
case "$PROFILE" in
  release) PROFILE_FLAG="--release" ;;
  debug) PROFILE_FLAG="" ;;
  *) echo "usage: $0 [debug|release]" >&2; exit 64 ;;
esac

TARGETS=(aarch64-apple-darwin)
if [[ "${OSM_UNIVERSAL:-0}" == "1" ]]; then
  TARGETS+=(x86_64-apple-darwin)
fi

BUILD="$ROOT/build/rust"
FRAMEWORKS="$ROOT/apps/macos/Frameworks"
GENERATED="$ROOT/apps/macos/Sources/OSMCore/Generated"
LIB_NAME="libosm_ffi.a"

cd "$ROOT"

LIBS=()
for target in "${TARGETS[@]}"; do
  cargo build -p osm-ffi --target "$target" $PROFILE_FLAG
  LIBS+=("$ROOT/target/$target/$PROFILE/$LIB_NAME")
done

rm -rf "$BUILD"
mkdir -p "$BUILD/lib" "$BUILD/include" "$GENERATED"

if [[ ${#LIBS[@]} -gt 1 ]]; then
  lipo -create "${LIBS[@]}" -output "$BUILD/lib/$LIB_NAME"
else
  cp "${LIBS[0]}" "$BUILD/lib/$LIB_NAME"
fi

# Bindings come from the metadata embedded in the library, so they always
# match the code that is actually linked.
BINDGEN=(cargo run -q -p uniffi-bindgen-swift --)
"${BINDGEN[@]}" "${LIBS[0]}" "$BUILD/swift" --swift-sources
"${BINDGEN[@]}" "${LIBS[0]}" "$BUILD/include" --headers
# Plain `module` (not `framework module`): the xcframework wraps a static
# library plus a Headers directory, not a .framework bundle.
"${BINDGEN[@]}" "${LIBS[0]}" "$BUILD/include" --modulemap \
  --module-name OSMCoreFFI --modulemap-filename module.modulemap

rm -rf "$FRAMEWORKS/OSMCoreFFI.xcframework"
mkdir -p "$FRAMEWORKS"
xcodebuild -create-xcframework \
  -library "$BUILD/lib/$LIB_NAME" -headers "$BUILD/include" \
  -output "$FRAMEWORKS/OSMCoreFFI.xcframework" >/dev/null

rm -f "$GENERATED"/*.swift
cp "$BUILD/swift/"*.swift "$GENERATED/"

echo "xcframework: $FRAMEWORKS/OSMCoreFFI.xcframework"
echo "bindings:    $(ls "$GENERATED"/*.swift | tr '\n' ' ')"
