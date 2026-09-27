#!/usr/bin/env bash
# Build `offloadd` and `offload` on a Mac. **Run this on the Mac, not on the laptop.**
#
# There is no cross-build for this and that is not an oversight: linking for Apple silicon
# needs Apple's SDK, which ships inside Xcode and whose licence covers use on Apple hardware.
# osxcross can be made to work from Linux; it is an afternoon, it needs the SDK extracted from
# an Xcode download anyway, and `rusqlite`'s bundled SQLite plus `ring` are exactly the crates
# that make it fiddly. Building here is ten minutes and is the supported path.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "this script builds on macOS; run it on the Mac" >&2
  exit 1
fi

# `rusqlite` is `features = ["bundled"]`, so SQLite is compiled from C and a C compiler has to
# exist. On a fresh Mac it does not until this is run, and the failure is a wall of missing
# headers rather than anything that names the cause.
if ! xcode-select -p >/dev/null 2>&1; then
  echo "Xcode command line tools are missing. Run: xcode-select --install" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "no cargo. Install with: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
  exit 1
fi

cargo build --release -p offload-node -p offload-cli

out="dist/macos-aarch64"
mkdir -p "$out"
cp target/release/offloadd target/release/offload "$out/"

# `~/bin` rather than `/usr/local/bin`, deliberately: it needs no `sudo`, which means this whole
# path works over a non-interactive ssh session with nothing to type a password into.
mkdir -p "$HOME/bin"
cp "$out/offloadd" "$out/offload" "$HOME/bin/"

echo
echo "built into $out, and installed into ~/bin"
case ":$PATH:" in
  *":$HOME/bin:"*) ;;
  *) echo "  ~/bin is not on PATH. Add it:  echo 'export PATH=\$HOME/bin:\$PATH' >> ~/.zshrc" ;;
esac
echo
echo "then check the classification this build fixed:"
echo "  offload probe        # a Mac mini should say desktop / stable, a MacBook laptop / transient"
