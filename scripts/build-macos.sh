#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export MACOSX_DEPLOYMENT_TARGET=14.0
cargo build --release
app="dist/fesReview.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/fesreview "$app/Contents/MacOS/fesreview"
xcrun swiftc -swift-version 5 -O native/main.swift -o "$app/Contents/MacOS/fesreview-native" \
  -framework AppKit -framework AVFoundation -framework CoreImage -framework Speech
cp native/Info.plist "$app/Contents/Info.plist"
codesign --force --sign - "$app/Contents/MacOS/fesreview-native"
codesign --force --sign - "$app"
echo "Built $app"
