#!/usr/bin/env bash
# Build the iOS host app (ADR-0070) for the Simulator, on a Mac with Xcode: offload-ios as a static
# library for aarch64-apple-ios-sim, main.swift linked against it, an ad-hoc-signed .app. Install
# and launch with:
#   xcrun simctl boot "iPhone 15"            # once
#   xcrun simctl install booted dist/ios-sim/Offload.app
#   xcrun simctl launch booted se.mach25.offload
#
# No Xcode project on purpose: two source files and a plist, built with swiftc, so there is no
# .pbxproj to keep in step. A real device needs a signing identity, which this does not do.
set -euo pipefail
cd "$(dirname "$0")/.."

target=aarch64-apple-ios-sim
# One deployment target for Rust, the C the crates build (ring, sqlite) and swiftc, or the linker
# warns that every C object was built for the SDK's newest iOS rather than the app's.
export IPHONEOS_DEPLOYMENT_TARGET=17.0
rustup target add "$target" >/dev/null
cargo build --release --target "$target" -p offload-ios

app=dist/ios-sim/Offload.app
rm -rf "$app"
mkdir -p "$app"
sdk=$(xcrun --sdk iphonesimulator --show-sdk-path)
xcrun --sdk iphonesimulator swiftc \
  -target "arm64-apple-ios$IPHONEOS_DEPLOYMENT_TARGET-simulator" -sdk "$sdk" -O -parse-as-library \
  -import-objc-header ios/Offload/offload.h \
  ios/Offload/main.swift \
  -L "target/$target/release" -loffload_ios \
  -framework Security -framework SystemConfiguration -lresolv \
  -o "$app/Offload"
cp ios/Info.plist "$app/Info.plist"
# The app icon: an asset catalog compiled by Xcode's actool into Assets.car, from one opaque
# 1024x1024 PNG (iOS applies its own mask). actool also writes the Info.plist keys that name the
# icon, which are merged in rather than hand-copied, so the two cannot disagree.
partial=$(mktemp -t offload-icon).plist
xcrun actool ios/Assets.xcassets --compile "$app" --platform iphonesimulator \
  --minimum-deployment-target "$IPHONEOS_DEPLOYMENT_TARGET" \
  --app-icon AppIcon --target-device iphone --target-device ipad \
  --output-partial-info-plist "$partial" >/dev/null
/usr/libexec/PlistBuddy -c "Merge $partial" "$app/Info.plist"
rm -f "$partial"
codesign --force --sign - "$app"

echo "built $app"
