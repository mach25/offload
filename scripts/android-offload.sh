#!/usr/bin/env bash
# Run the Android app's own `offload` against the app's daemon (ADR-0066 §4):
#
#   scripts/android-offload.sh [adb flags --] <offload args...>
#   scripts/android-offload.sh status
#   scripts/android-offload.sh -s emulator-5554 -- nodes
#
# Through `run-as`, which a debug build allows, so the CLI runs as the app — with the app's files,
# the app's socket and the app's SELinux domain, which is the point: a shell would see more.
set -euo pipefail
# The harness by default; OFFLOAD_APP_PACKAGE=se.mach25.offload.app drives the product app (ADR-0071).
PKG=${OFFLOAD_APP_PACKAGE:-se.mach25.offload}

adb_flags=()
if [[ " $* " == *" -- "* ]]; then
  while [[ $1 != -- ]]; do adb_flags+=("$1"); shift; done
  shift
fi

# nativeLibraryDir: where Android extracted liboffload.so, e.g. /data/app/~~…/se.mach25.offload-…/lib/arm64
libdir=$(adb "${adb_flags[@]}" shell pm dump "$PKG" | tr -d '\r' | sed -n 's/^ *legacyNativeLibraryDir=//p' | head -1)
abi=$(adb "${adb_flags[@]}" shell pm dump "$PKG" | tr -d '\r' | sed -n 's/^ *primaryCpuAbi=//p' | head -1)
case "$abi" in arm64-v8a) sub=arm64 ;; x86_64) sub=x86_64 ;; *) sub=$abi ;; esac
cli="$libdir/$sub/liboffload.so"

# The config and the state dir are relative to the app's data directory, which is run-as's cwd.
quoted=$(printf '%q ' "$@")
# Commands that read the state directory rather than the socket take --state-dir; the rest take --socket.
case "${1:-}" in
  id|init|join|fleet|grant|verify|invite|reapprove|revoke|rekey) extra="--state-dir files/s" ;;
  probe|policy|match) extra="--config files/node.toml" ;;
  *) extra="--socket files/s/offloadd.sock" ;;
esac
adb "${adb_flags[@]}" shell "run-as $PKG sh -c 'HOME=files TMPDIR=cache $cli $1 $extra ${quoted#* }'"
