# ADR-0070: An iOS host links the daemon in — ADR-0066's app, where the platform allows no second process

**Status:** accepted · 2026-09-26 · session ninety-two · builds phase 5's "Android/iOS host process"
for iOS, to ADR-0066's scope ("just enough to run our tests") · amends no ADR

## Context

The owner asked for the iOS equivalent of the Android app. There is no iPhone, so the target is the
iOS Simulator on the Mac mini (Xcode 27; the "iPhone 15" device runs the iOS 17.5 runtime). ADR-0066's shape carries over — the same
daemon, platform facts in `host-facts.json`, everything else driven from outside — with three
differences iOS imposes:

1. **An iOS app cannot launch a separate program.** There is no `posix_spawn` of a bundled binary
   under the iOS sandbox, so `nativeLibraryDir`'s trick has no equivalent. The daemon has to run
   *inside* the app's process.
2. **An app is suspended shortly after it leaves the foreground.** There is no foreground service.
   An iOS node is present while its app is open, which is `Stability::Ephemeral` taken literally.
3. **There is no `run-as`.** On the Simulator the app's container is a directory on the Mac, and
   the app is a Mac process, so the Mac's own `offload` binary can talk to the daemon's socket
   directly, if the socket path fits `SUN_LEN`.

## Decision

### 1. The daemon is a library function, and the app calls it

`offload_node::daemon::run(config, shutdown)` holds what `offloadd`'s `main` did, moved unchanged
except for how it learns to stop. The binary calls it with SIGTERM/SIGINT. A new crate,
**`offload-ios`** (a `staticlib`), exposes a two-function C ABI, `offload_start(config_path)` and
`offload_stop()`. It runs `run` on its own thread and tokio runtime, with logging to a file in the
state directory. The Swift app links it. It is one daemon, two hosts, and no second implementation.

### 2. Platform facts, as on Android

The app writes `host-facts.json` every 15 s, in the shape the daemon already reads (ADR-0066 §3,
ADR-0068):

- battery and charging from `UIDevice` (the Simulator reports no battery, so no level);
- metered from `NWPathMonitor` (`isExpensive || isConstrained`);
- thermal from `ProcessInfo.thermalState`, mapped onto Android's scale: nominal 0, fair 1, serious 3,
  critical 4;
- form from `userInterfaceIdiom`: `tablet` on an iPad, `phone` otherwise.

### 3. Present while open, and it leaves politely

The app starts the daemon when it launches. When it moves to the background it asks for background
time (`beginBackgroundTask`) and calls `offload_stop()`, so the daemon drains and announces its
departure the way SIGTERM makes `offloadd` do (ADR-0034). A suspended app is gone, and the fleet
says so rather than finding out by probe timeouts. Hosting long agent runs on iOS is not a goal:
there is no agent build for iOS, and an app cannot keep one alive in the background.

### 4. Driven from the Mac, not from the app

A test build's config sets `socket` to a short path, since the container path is far past `SUN_LEN`.
The Mac's `offload` binary is the CLI, and the app shows the tail of its own log and nothing else.
There is no enrolment screen, as on Android.

## Consequences

- `offloadd`'s behaviour is unchanged. Its own log lines now come from the target
  `offload_node::daemon`.
- An iOS node joins the fleet and reports its platform facts while its app is open. **On a device
  it hosts nothing**: an agent run needs an agent binary, and a task or a sink is a program the
  daemon spawns, which the iOS sandbox forbids. What it is for is the control plane — submitting,
  watching, answering a question — from the device in somebody's pocket. The default config
  nominates no task, so it never bids on one it could not start. On the Simulator a task *would*
  spawn, because the app is a Mac process there, and a walk there is no evidence either way.

## What this deliberately leaves

- **A real iPhone.** It needs signing with a developer identity, which is not set up here, and the
  Simulator answers what can be answered without one: the daemon runs, meshes and reads the
  platform.
- **Background execution** past the drain window, and push-driven wake-ups.
- **The Secure Enclave approver key** (ADR-0069 §4). It fits `IssuerKey::P256` when this host exists
  on a device.

## Amendment, 2026-09-26: walked in the Simulator

Built by `scripts/build-ios-sim.sh` on the Mac mini (a 27 MB app, the Rust side about 1.5 minutes
warm), installed with `simctl`, and driven by the Mac's `offload` over `/tmp/offload-ios.sock`:

- The daemon started, applied the schema and bound its socket. `offload status` said `phone`, no
  agent, `thermal none`, and `accepting no`, because power is unknown and a phone's default policy
  is `when_charging`. The Simulator has no battery, so both `battery_percent` and `charging` are
  `null`. `metered` is `null` in the first write and `false` once `NWPathMonitor` has answered.
- `cpu 100%` was honest: a Simulator app reads the *Mac's* load average, which was at 8–29 while
  the Simulator booted. Memory, likewise, is the Mac's.
- Invited by a v32 `offloadd` on the Mac and joined with `--token`, it meshed over mDNS after a
  restart, found by the defect below. Backgrounding the app drained and stopped the daemon in a
  second and removed the socket. Opening it again started a fresh daemon in the same process.
- Tasks submitted from the iOS node were placed on the Mac and completed. That is the role this
  host has on a device.

Three defects found on the way, all fixed:

- **A reinstall moved the container, and the config still named the old one.** iOS changes an
  app's container path on reinstall and on update. The config's absolute `state_dir` then sent the
  daemon to a directory that was no longer the app's. The Simulator let it create one there, where
  it **minted a new node identity**, so the device became a different node, outside its fleet. The
  app now re-points `state_dir` (and a device's `socket`) at the current container on every launch,
  and leaves the rest of the config as the owner wrote it. After that fix, two reinstalls kept node
  `56af…` and its membership.

- **A bid over a connection the bidder opened was refused.** The bid round read the bidder's
  certificate only from sessions this node had dialled, so it refused the bid as "connection closed
  before its certificate could be checked" and hung up. Seen after a peer restart. On a LAN the retry
  dials out and it looks like a flake. A phone nobody can dial could never host. This came from
  session ninety-two's own bidirectional sessions. The same lookup also returned a dialled session
  the far end had already closed, so the first exchange after any hang-up failed. Both are fixed.
  After the fix, three peer restarts in a row were each followed, 15 s later, by a task placed on
  the restarted peer. One earlier round, before per-instance logs were kept, took about eight
  minutes to answer and never saw the peer. It did not recur, and nobody knows why.
- **Joining while the daemon runs needs a restart, and nothing said so.** `offload nodes` then told
  the device to run `offload init`. `init` and `join` now say it, and so does `nodes`. Starting the
  mesh without a restart is left for a later ADR: an app host has no operator to restart it, but a
  restart with runs in flight is not a thing to do casually.
