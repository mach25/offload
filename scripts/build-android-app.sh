#!/usr/bin/env bash
# Build the Android host app (ADR-0066): cross-build offloadd and offload for aarch64, put them in
# the APK as lib*.so, and assemble a debug APK. Install with:
#   adb install -r android/app/build/outputs/apk/debug/app-debug.apk
#
# A debug build on purpose: it is debuggable, so `adb shell run-as se.mach25.offload` reaches the
# app's files and can run its bundled `offload` — which is how a walk configures and drives it.
set -euo pipefail
cd "$(dirname "$0")/.."

# aarch64 for a phone, x86_64 for the SDK emulator — the one Android this project can reach without
# a phone in hand. Android installs whichever ABI the device has.
for pair in aarch64:arm64-v8a x86_64:x86_64; do
  arch=${pair%%:*} abi=${pair##*:}
  ANDROID_ARCH=$arch scripts/build-android.sh >/dev/null
  libs=android/app/src/main/jniLibs/$abi
  mkdir -p "$libs"
  cp "dist/android-$arch/offloadd" "$libs/liboffloadd.so"
  cp "dist/android-$arch/offload" "$libs/liboffload.so"
done

# Gradle 8.14 does not run on the newest JDKs (25+); a 17 is what AGP 8.10 is built against.
export JAVA_HOME="${JAVA_HOME_FOR_GRADLE:-$(ls -d "$HOME"/.jdks/jbr-17* | sort -V | tail -1)}"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
(cd android && ./gradlew --quiet assembleDebug)

echo "built android/app/build/outputs/apk/debug/app-debug.apk"
