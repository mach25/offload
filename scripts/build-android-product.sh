#!/usr/bin/env bash
# Build the product app (ADR-0071): the daemon, the CLI and liboffload_mobile.so for both ABIs, the
# Kotlin UniFFI generates from that library, and a debug APK. Install with:
#   adb install -r android-app/app/build/outputs/apk/debug/app-debug.apk
#
# The harness (android/, scripts/build-android-app.sh) is separate and untouched by this.
set -euo pipefail
cd "$(dirname "$0")/.."

for pair in aarch64:arm64-v8a x86_64:x86_64; do
  arch=${pair%%:*} abi=${pair##*:}
  ANDROID_ARCH=$arch ANDROID_WITH_MOBILE=1 scripts/build-android.sh >/dev/null
  libs=android-app/app/src/main/jniLibs/$abi
  mkdir -p "$libs"
  cp "dist/android-$arch/offloadd" "$libs/liboffloadd.so"
  cp "dist/android-$arch/offload" "$libs/liboffload.so"
  cp "dist/android-$arch/liboffload_mobile.so" "$libs/liboffload_mobile.so"
done

# Library mode: the bindings describe exactly the library that ships, read from the library itself —
# the **unstripped** one in target/, because UniFFI's metadata lives in symbols `llvm-strip` removes.
# Generating from the stripped copy produced no Kotlin at all and said nothing, so this checks.
generated=android-app/app/src/generated/uniffi
rm -rf "$generated"
cargo run -q -p offload-mobile --bin uniffi-bindgen -- generate --no-format \
  --library target/x86_64-linux-android/release/liboffload_mobile.so --language kotlin \
  --out-dir "$generated"
if ! find "$generated" -name '*.kt' | grep -q .; then
  echo "uniffi-bindgen generated no Kotlin from liboffload_mobile.so" >&2
  exit 1
fi

export JAVA_HOME="${JAVA_HOME_FOR_GRADLE:-$(ls -d "$HOME"/.jdks/jbr-17* | sort -V | tail -1)}"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
(cd android-app && ./gradlew --quiet assembleDebug)

echo "built android-app/app/build/outputs/apk/debug/app-debug.apk"
