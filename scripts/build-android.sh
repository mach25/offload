#!/usr/bin/env bash
# Cross-build `offloadd` and `offload` for an Android phone, from Linux.
#
# Termux runs on Android's own bionic, so an NDK build needs nothing shipped alongside it:
# the result links libc, libm and libdl and no more — verified with `readelf -d`. That is why
# there is no `libc++_shared.so` to copy, which is the usual Android cross-build tax.
#
# API 24 is Android 7, which is also Termux's own floor. Raising it buys nothing here.
set -euo pipefail

NDK="${ANDROID_NDK_HOME:-$HOME/Android/Sdk/ndk/27.0.12077973}"
API="${ANDROID_API:-24}"
# `aarch64` for a phone; `x86_64` for the SDK's emulator images, which run at native speed under KVM
# and are the only Android this project can reach without a phone in hand.
ARCH="${ANDROID_ARCH:-aarch64}"
TARGET="$ARCH-linux-android"
TARGET_ENV="${TARGET//-/_}"
TARGET_ENV_UPPER="${TARGET_ENV^^}"
BIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"

if [[ ! -x "$BIN/$TARGET$API-clang" ]]; then
  echo "no NDK clang at $BIN/$TARGET$API-clang" >&2
  echo "set ANDROID_NDK_HOME, or install the NDK from Android Studio's SDK manager" >&2
  exit 1
fi

rustup target add "$TARGET" >/dev/null

# `rusqlite` is built with `features = ["bundled"]`, so SQLite is compiled from C for the
# target and the `cc` crate needs to be told which compiler that is. `ring` and `blake3`
# have the same requirement.
export "CARGO_TARGET_${TARGET_ENV_UPPER}_LINKER=$BIN/$TARGET$API-clang"
export "CC_${TARGET_ENV}=$BIN/$TARGET$API-clang"
export "AR_${TARGET_ENV}=$BIN/llvm-ar"
export "CFLAGS_${TARGET_ENV}=-D__ANDROID_API__=$API"

# `ANDROID_WITH_MOBILE=1` also builds liboffload_mobile.so, the product app's client (ADR-0071).
packages=(-p offload-node -p offload-cli)
if [[ "${ANDROID_WITH_MOBILE:-0}" == 1 ]]; then packages+=(-p offload-mobile); fi
cargo build --release --target "$TARGET" "${packages[@]}"

out="dist/android-$ARCH"
mkdir -p "$out"
cp "target/$TARGET/release/offloadd" "target/$TARGET/release/offload" "$out/"
"$BIN/llvm-strip" "$out/offloadd" "$out/offload"
if [[ "${ANDROID_WITH_MOBILE:-0}" == 1 ]]; then
  cp "target/$TARGET/release/liboffload_mobile.so" "$out/"
  "$BIN/llvm-strip" "$out/liboffload_mobile.so"
fi

echo
echo "built into $out:"
ls -la "$out"
echo
echo "push with:  adb push $out/offloadd $out/offload /data/local/tmp/"
echo "then in Termux:  cp /data/local/tmp/offload{,d} \$PREFIX/bin/ && chmod +x \$PREFIX/bin/offload{,d}"
