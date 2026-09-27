# ADR-0066: An Android host is a thin shell around the same daemon — enough to walk a phone, no more

**Status:** accepted · 2026-09-25 · session ninety-two · builds the first step of phase 5's "Android
host process for `offloadd`" · amends no ADR

## Context

A Samsung phone (Android 16) under Termux ran the aarch64 build this session and meshed with the
laptop, mDNS included. Termux was the right harness for finding out what a phone does to this
daemon:

- `/sys/class/power_supply` is denied to apps, so power is unknown;
- quinn falls back from GSO on the first send;
- a backgrounded app gets the little cores (8 → 3);
- wifi power save adds up to ~370 ms latency;
- Claude Code has no Android build.

The owner then said the obvious thing: *nobody is going to use Termux*. Termux is an app that
installs a Linux userland, and a fleet member should be an app you `adb install`. They also set
the scope: *just enough to run our tests*.

## Decision

### 1. The app runs the same `offloadd`, from the APK's native-library directory

No second implementation and no JNI. The aarch64 build of `offloadd` and `offload` ships as
`lib/arm64-v8a/liboffloadd.so` and `liboffload.so`, packaged uncompressed so Android extracts them
to `nativeLibraryDir`. That is the one app-owned place Android still lets an app execute a file
(W^X forbids exec from the data directory since API 29). The daemon is unchanged, its tests are
unchanged, and "never reimplement" holds for our own code as much as for the agent's.

### 2. A foreground service supervises it, and holds what Android takes away by default

- **A foreground service** (`specialUse`), whose persistent notification is the price of not being
  killed.
- **A `WifiManager.MulticastLock`**, because without one Android drops inbound multicast and mDNS
  fails silently. That is exactly the failure this project refuses to leave unexplained.
- **The daemon restarted when it exits**, with a backoff. Its output goes to a log file in the
  app's own directory.

### 3. Platform facts arrive as a file the probe reads first

The service writes `host-facts.json` into the state directory every 15 s: battery percent and
charging from `BatteryManager`, and metered from `ConnectivityManager.isActiveNetworkMetered`. That
is the platform call ADR-0045 said a phone's host process would make. `deliver::capabilities`
overrides `power` and `metered` with it **only while it is fresh** (written within the last two
minutes). A stale file means the host has stopped telling, and **unknown is not good news**: the
probed values stand. Those probed values are `Unknown` for power on a phone, which a default phone
refuses on.

The fact lives in a file rather than on the control socket so that the daemon learns it through the
one writer it already has (`reprobe`), on the cadence it already uses (ADR-0048).

### 4. Everything else goes through adb

Configuration, enrolment (`offload join --token`) and every report run as
`adb shell run-as <package> …/liboffload.so --socket …`. A debug build is debuggable, which is
what `run-as` needs. **No enrolment screen, no notification route and no approver key in this
step.** Each is a real phone job (the roadmap names approving and being told as the phone's best
work), and each gets built when a walk needs it, not before.

## Consequences

- A walk on a phone is `adb install`, a config pushed with `run-as`, and the same commands as
  anywhere else. The Termux notes in `docs/DEMO.md` become the debugging path, not the product.
- The battery term now has a real source on a real phone, the probe's Termux:API fallback stays for
  Termux, and `ChargingUnknown` stays for a host that says nothing.
- The app hosts tasks and submits and watches runs. **It does not host agent runs** until Claude
  Code ships an Android build, and the probe says so rather than claiming one.

## What this deliberately leaves

- **Doze.** A foreground service is exempt from most of it and not from all of it. The walk
  measures what is left before anything is designed around it.
- **Background core confinement.** The probe reports what the process is allowed, which is the
  truth about what work there would get. Whether a phone should bid with its foreground numbers is
  a scoring question.
- **Release signing, an update path, a UI worth the name.** A debug APK installed over adb is the
  whole distribution story for now.

## Amendment, 2026-09-26: built, installed on the phone, and phase 5's demo walked through it

`android/` builds with `scripts/build-android-app.sh` (aarch64 and x86_64 in one APK, JBR 17 for
Gradle 8.14) and installs with `adb install -r`. `scripts/android-offload.sh` runs the app's own
`offload` through `run-as`. On the Samsung phone (Android 16):

- **Power and metered came from the platform:** `on mains, battery 100%` from `BatteryManager`, and
  `unmetered` from `ConnectivityManager`. That is the first time a phone in this fleet has *known*
  its metered state. No Termux:API was involved.
- **mDNS with no seeds** found the laptop, so the multicast lock does what §2 said.
- **Phase 5's demo, every clause but the off-LAN one.** Plugged in on wifi, it won a bid and ran a
  task on its default policy. On battery it declined with `node accepts work only while charging`,
  and an agent run submitted *on the phone* was placed on the laptop and followed to its end from
  the phone. Off the LAN, see ADR-0037's amendment of the same date: the carrier gives IPv6, and
  the router is the block.
- **What building it found in the daemon.** An app is denied `/proc/loadavg`, and sysinfo answered
  0.0, so the phone claimed `cpu 0%` for ever. It is `cpu not reported` now (`load.rs`).
  `offload status` also printed `probation for another 0 minutes` from its own second copy of
  the formatter.
- **The screen-off cost is latency, not deaths.** Ping to the sleeping phone peaked at 562 ms,
  over the 500 ms probe timeout. So the laptop logs a suspicion that clears in the same second
  every few seconds (300 in twenty minutes, none fatal). Whether a phone's probe timeout should be
  longer is a decision, recorded in the handoff.
- `java.lang.Process` on Android exposes no pid and `destroy()` is a SIGKILL. The service finds its
  child in `/proc` and sends SIGTERM, so the daemon drains.

**And work over mobile data, both ways.** Plugged in, on mobile data, reachable only through the
router rule of ADR-0037's amendment, the phone refused a normal task with `network is metered and
policy disallows it`. That is ADR-0045's refusal firing for a real reason for the first time, since
no phone could tell before `ConnectivityManager` reached the probe. With the owner's `[policy.light]
allow_metered = true` it then took a `--demand light` task and ran it, its output streaming back
across the internet. In the other direction, an agent run submitted on the phone over mobile data
was placed on the laptop and followed from the phone to its end. The owner's two answers for one
device (ADR-0019 §4) worked on the case they were written for.
