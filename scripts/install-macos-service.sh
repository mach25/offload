#!/usr/bin/env bash
# Install `offloadd` as a launchd service on a Mac, and restart it onto the current build.
# **Run this on the Mac**, after `cargo build --release -p offload-node -p offload-cli`
# (or `scripts/build-macos.sh`). Running it again is how the service is updated.
#
# A LaunchAgent in the login session, not a LaunchDaemon, deliberately: Claude Code keeps its
# login in the user's Keychain, which a boot-time daemon has no session to unlock, and a host
# with no authenticated agent wins bids and fails every run it takes. The cost is that after a
# reboot the node starts at login, not at boot, so a Mac without auto-login waits for somebody
# to log in.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "this script installs a launchd service; run it on the Mac" >&2
  exit 1
fi

label="se.mach25.offloadd"
domain="gui/$(id -u)"
plist="$HOME/Library/LaunchAgents/$label.plist"
config="${XDG_CONFIG_HOME:-$HOME/.config}/offload/node.toml"
bin="$HOME/bin"
src="${1:-target/release}"

if ! launchctl print "$domain" >/dev/null 2>&1; then
  echo "no login session for $(id -un) — log in at the Mac once, then run this again" >&2
  exit 1
fi
for b in offloadd offload; do
  if [[ ! -x "$src/$b" ]]; then
    echo "no $src/$b — build first: cargo build --release -p offload-node -p offload-cli" >&2
    exit 1
  fi
done
if [[ ! -f "$config" ]]; then
  echo "no config at $config — the daemon reads that path (ADR-0074)" >&2
  exit 1
fi

# Installed by rename, never by copying over the file: macOS kills a process whose executable
# pages change under it, so `cp` onto a running `offloadd` crashes the daemon it is updating.
mkdir -p "$bin"
for b in offloadd offload; do
  cp "$src/$b" "$bin/.$b.new"
  mv -f "$bin/.$b.new" "$bin/$b"
done

# launchd sends SIGTERM and then SIGKILLs after `ExitTimeOut` (20 s unless set). A SIGTERM'd
# daemon drains: it checkpoints its runs and hands them off, for up to `drain_deadline_secs`.
# Killing it inside that window is a checkpoint cut in half, so wait out the deadline and a
# margin. The margin is a guess, not a measurement.
deadline=$(sed -n 's/^[[:space:]]*drain_deadline_secs[[:space:]]*=[[:space:]]*\([0-9]*\).*/\1/p' "$config" | head -1)
exit_timeout=$(( ${deadline:-300} + 30 ))

log_dir="$HOME/.offload"
mkdir -p "$log_dir" "$(dirname "$plist")"
cat > "$plist.new" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$label</string>
  <key>ProgramArguments</key>
  <array>
    <string>$bin/offloadd</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>$bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <!-- Restarted after a crash, left down after a clean exit: a daemon somebody stopped stays stopped. -->
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ExitTimeOut</key>
  <integer>$exit_timeout</integer>
  <key>StandardOutPath</key>
  <string>$log_dir/daemon.log</string>
  <key>StandardErrorPath</key>
  <string>$log_dir/daemon.log</string>
</dict>
</plist>
EOF
plutil -lint "$plist.new" >/dev/null
mv -f "$plist.new" "$plist"

# A daemon started by hand holds the state directory's lock, and the service's daemon would die
# on it. It can take longer than the drain deadline to let go, and while it is letting go
# `pgrep` still shows it, which looks exactly like a restart that silently failed. So wait for
# the process to be gone rather than for the signal to be sent.
pidfile="$log_dir/daemon.pid"
if [[ -f "$pidfile" ]] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then
  pid=$(cat "$pidfile")
  if ! launchctl print "$domain/$label" 2>/dev/null | grep -q "pid = $pid\$"; then
    echo "stopping the daemon started by hand (pid $pid); it drains first, up to ${deadline:-300}s"
    kill -TERM "$pid"
    while kill -0 "$pid" 2>/dev/null; do sleep 1; done
  fi
fi
rm -f "$pidfile"

if launchctl print "$domain/$label" >/dev/null 2>&1; then
  # Already loaded: pick up an edited plist, then restart onto the new binary.
  launchctl bootout "$domain/$label" 2>/dev/null || true
  while launchctl print "$domain/$label" >/dev/null 2>&1; do sleep 1; done
fi
launchctl bootstrap "$domain" "$plist"

sleep 2
pid=$(launchctl print "$domain/$label" | sed -n 's/^[[:space:]]*pid = \([0-9]*\)$/\1/p')
if [[ -z "$pid" ]]; then
  echo "the service is loaded but not running — read $log_dir/daemon.log" >&2
  tail -20 "$log_dir/daemon.log" >&2 || true
  exit 1
fi
echo "offloadd is running under launchd as $label (pid $pid), log $log_dir/daemon.log"
echo "  restart:  launchctl kickstart -k $domain/$label"
echo "  stop:     launchctl bootout $domain/$label   (it comes back at the next login)"
