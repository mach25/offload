# The fleet in your pocket: a phone that is a member, and a terminal into the rest

Status: **exploration.** Nothing here is decided and nothing is built. Two of the findings
contradict a line in `docs/ROADMAP.md`, and that contradiction is the main output.

The prompting case is the owner's, in two parts. First: *"we can build an android app using rust —
I'm not sure exactly what the support is though. But I was thinking we could use the android
emulator at first to test."* Then, when Termux came up as the way to run the daemon today:
**"we can't expect people to use Termux"** — which settles the distribution question and, it turns
out, most of the design.

Second, and it is a different workload: *"what I'd also like to see with the mobile app is that it
is a sort of terminal into the other nodes to view status and perform actions."*

And the comparison that sharpens it: *"a little bit like the Claude Code remote control feature —
but with that you have to set up a running Claude instance with RC enabled first. With Offload you
just have the node up and running and you can dispatch tasks."*

They are not the same feature. The first is *what a phone is in the fleet*, and the platform
answers it more narrowly than the roadmap assumes. The second is *what a phone shows you about
everything else*, and it is largely built already — but the half that is missing is missing on
purpose, which is worth knowing before someone treats it as an oversight.

## The link is AOSP, and this is not that

`source.android.com/docs/setup/build/rust/` is the **platform** build: Rust modules compiled into
the OS image through Soong and `Android.bp`, the way Keystore2 and the DNS resolver are written. It
needs an AOSP checkout and a flashed image. It has nothing to say about shipping an app.

What applies is the NDK. `aarch64-linux-android` and `x86_64-linux-android` are tier-2 Rust targets
with std, and `cargo-ndk` handles the toolchain plumbing. Four shapes exist; only one of them is a
product:

| Shape | What it is | Verdict |
| --- | --- | --- |
| `adb push` to `/data/local/tmp` | shell UID, no app installed | **dev rig.** Cheapest measurement, no Android code |
| Termux | daemon as an ordinary Linux binary | **dev rig only** — ruled out as distribution |
| APK | foreground service hosting the daemon | **the product** |
| AOSP module | the linked doc | not applicable |

The two rigs are how you measure before writing Kotlin. They are not a fallback if the app is hard.

## The constraint that decides what a phone is

**W^X, since API 29.** An app UID cannot `exec()` anything from a directory it can write. The only
executable path is `nativeLibraryDir` — files shipped inside the APK, named `lib*.so`, with legacy
packaging. Anything downloaded, extracted or installed at runtime lands somewhere non-executable
and stays that way.

Hosting an agent run means spawning the agent. `offload-agent/src/claude.rs:85` is
`tokio::process::Command::new(&self.binary)`, and the binary defaults to `claude`
(`claude.rs:30`) — a Node CLI. For a phone to host a run, both `claude` and a Node runtime would
have to be packaged into the APK as pseudo-shared-objects, and `npm install` at runtime is dead on
arrival.

So: **a phone cannot host an agent run from inside an app.** Not a battery policy. Not a bid
score. Not something a charger fixes. The platform.

### Why Termux appears to disprove this, and does not

`docs/HANDOFF.md` stages the phase-5 walk with one genuine unknown left: *"whether Claude Code runs
under Termux on aarch64"*. That question is worth answering, and its answer — whatever it is —
**does not transfer to the app.**

Termux can exec what it installs because it targets an old Android API level; the W^X rule binds
apps by their `targetSdkVersion`, and staying below it is why Termux's `$PREFIX/bin` works at all.
That exemption is not available to anything shipped today: app stores require a recent target, and
raising the target is precisely what would break Termux's own package manager. So Termux is not an
early version of the app. It is a permanently different deal, on a legacy exemption, and the two
cannot converge.

This is the reconciliation between this document and the staged walk's stage 4 (the runbook that
held it was deleted in 45c6ef2; `docs/HANDOFF.md` still carries the staging), and it should be
checked against Termux's own documentation before anyone leans on it — **it is the single claim
here that everything else rests on.** If it holds, a Termux `yes` to *"does Claude Code run on
aarch64"* proves something true about a phone and false about the product, which is the most
expensive shape a measurement can have.

### The tree already tells the truth about this, without being asked

This is the part that should be reassuring. `offload-probe`'s agent detection takes the binary as
an argument and asks whether it is really there (`offload-probe/src/agents.rs:44`), and its own doc
comment already names the case:

> an owner whose agent is at a path (a version pin, a wrapper, **a phone**) advertised the version
> and the authentication of a different program

No `claude` on the device means no `Capability::Agent`, which means every agent run's `Constraint`
excludes the phone with a legible reason and no special case anywhere. The "don't over-claim" rule
is doing its job on a platform nobody has ever compiled for.

The probe goes further than that. `detect_os` (`offload-probe/src/lib.rs:131`) already recognises
Android two ways — `target_os = "android"`, and `"linux"` plus `/system/build.prop` or
`$ANDROID_ROOT`, which is the Termux case — and `detect_device_class` (`lib.rs:159`) maps
`Os::Android` straight to `DeviceClass::Phone` before it ever looks at DMI — including the awkward
case, where Termux calls itself `linux` and the property store gives it away.

None of that is speculative. `scripts/build-android.sh` cross-builds `offloadd` and `offload` for
`aarch64-linux-android` from this machine, and it has already paid the NDK tax the hard way: the
`cc` crate needs telling which compiler to use for `rusqlite`'s bundled SQLite, `ring` and
`blake3`, and the script exports all four variables. Its header records a measurement — the result
links `libc`, `libm` and `libdl` and nothing else, *"verified with `readelf -d`"*, so there is no
`libc++_shared.so` to ship. **The build works. What has never been measured is the daemon running
on a phone.**

## What the phone is instead, and it is already designed

Strip out hosting and what is left is not a consolation prize. It is precisely what two accepted
documents already say a phone should be:

- **ADR-0038 §1**, quoting the owner: *"It should act like any other node, just that it doesn't do
  any work itself."* That ADR was written about a VPS. It describes a phone exactly.
- **`docs/ROADMAP.md`, phase 5, approvers in hardware**: *"This is the phone's best job in the fleet
  and the reason to enrol one before it can host anything."* The key in secure hardware,
  biometric-gated, issuing invitations and renewing certificates without the passphrase leaving the
  drawer. Approval is a signed artifact peers verify offline, never a live call to the phone — so
  it works with the phone in a pocket on a train.
- **A sink** (ADR-0010): a route to a human, advertised as a capability. An APK can post a real
  Android notification; this is the one job where the app beats every alternative outright.
- **The answer surface for questions** (ADR-0017): a run blocked mid-tool-call, answerable from any
  device. The device you are holding is the one that should answer.

Four jobs, none of which needs to exec anything.

## The APK's shape is not "ship the binary"

The obvious build — package `offloadd` as `libofloadd.so`, exec it from the service — is the wrong
one, and for a reason that has nothing to do with W^X.

A binary exec'd by an app is an app child process, and Android 12 introduced a cap on those
(32) plus killing for CPU use. It is what mauls background work in Termux. A daemon whose entire
job is holding a QUIC socket open across a doze cycle is the worst possible candidate for a process
class Android reserves the right to reap.

The survivable shape is **in-process**: the daemon compiled as a `cdylib`, loaded over JNI, running
its tokio runtime on threads inside a foreground service the ActivityManager knows about and keeps.
The Android artifact is a library, not `offloadd`.

### What that costs in this tree — three things, checked

1. **`offload-node` is not library-shaped where it matters.** `src/main.rs` is 1230 lines and every
   bit of daemon wiring lives in `main`; `src/lib.rs` is a module list and two re-exports. An
   in-process host needs an entry point of roughly `run(Config, shutdown) -> Result<()>` with
   `main.rs` reduced to argument parsing over it. **This is worth doing whatever Android turns out
   to need** — it is also what would let a test drive a whole daemon in-process instead of spawning
   one.

2. **Two paths re-exec `current_exe`, and in-process there is no such thing.**
   `offload-node/src/supervisor.rs:4693` re-execs the daemon as the ADR-0017 permission hook;
   `offload-node/src/resource.rs:270` re-execs it as the ADR-0011 `use-resource` MCP server. Both
   are deliberate — *"`current_exe` is exact, needs no discovery, and cannot be pointed at a
   different version"* — and both are meaningless inside an app process, where `/proc/self/exe` is
   `app_process64`.

   **Both fire only when hosting a run.** A non-hosting phone never reaches either. The platform
   constraint and the role the phone is being given agree, exactly — and that agreement is
   load-bearing, because the day somebody makes a phone host anything, these two are what break
   first, silently, in a re-exec of the Android zygote. It is written down here and nowhere else.

3. **Paths are already injectable, and no new seam is needed.** `default_state_dir`
   (`offload-node/src/config.rs:657`) reads `$OFFLOAD_STATE_DIR` first, then `$HOME/.offload`;
   `broker::resolve_path` (`offload-node/src/broker.rs:111`) is already a pure function of
   `$XDG_RUNTIME_DIR` precisely so the rule is testable. Android sets neither variable, so the app
   passes its own `filesDir` in and both fall into place.

   One smaller sibling: `hostname()` (`config.rs:664`) reads `/etc/hostname` and then `$HOSTNAME`,
   neither of which Android has, so every phone comes out **`unnamed`** unless its config names it.
   Invisible until there are two of them in `offload nodes`.

## Discovery: mDNS is dead on a phone, and that is the expected shape

`offload-transport` uses `mdns-sd` (`Cargo.toml:14`, `src/discovery.rs`). Receiving multicast on
Android requires a `WifiManager.MulticastLock` — an app can take one, a Termux process cannot — and
on mobile data there is no multicast at all.

So a phone is **seeds-only**, permanently. Which is not a gap: it is ADR-0037 §2 (a seed may be a
name, resolved on every attempt, and re-dialled while there is no live peer — both built) and
ADR-0038 (introduction as a third `Resolver` source behind mDNS and the seeds). The phone is the
device those two ADRs were written for, and this is the first time the reason has been stated as a
platform fact rather than a network one.

## Probing: less broken than expected, and cheap to find out

- **Power may already work unmodified.** `power.rs:20` reads `/sys/class/power_supply`, and Android
  exposes `battery/capacity` and `battery/status` there. If SELinux permits the read, a phone
  reports its real charge with no platform code at all. **Measure it; do not assume either way** —
  and note the file's own warning about peripheral batteries applies doubly on a device that pairs
  earbuds and a watch.
- **Metered stays `Unknown`, and `Unknown` does not refuse.** `docs/HANDOFF.md` states the live
  consequence: a phone on mobile data currently **accepts** work, because a three-valued capability
  deliberately declines to guess. That is the right default for a laptop and the wrong outcome on a
  metered link, and it is the sharpest single argument for the app that is not about notifications.
  The mechanism: `network.rs:23` shells out to `nmcli`, which Android does not have.
  `ROADMAP.md:41` already names the fix — `ConnectivityManager.isActiveNetworkMetered` — and that
  call needs the app.
- **Thermal is untouched**, and Android is the platform where it matters most.
- `systemd-detect-virt` (`lib.rs:223`) and the DMI chassis read (`lib.rs:174`) both come up empty,
  but neither is reached: `Os::Android` short-circuits to `DeviceClass::Phone` above them.

## The terminal into the other nodes

This is the second half of the ask, and it splits three ways along a line the API's own doc
comments already draw. `offload-node/src/api.rs` is the whole surface, and the app calls it
in-process rather than over the control socket — no CLI on the phone, no new protocol.

**1. Fleet-wide already, answered locally from gossip.** `Status`, `List` (*"all runs this node
knows about"*), `Nodes` (*"every node this one knows about, and how it is doing"*). This is the
"view status" half of the ask and it is done. `docs/ARCHITECTURE.md:126` is the property that makes
it good on a phone: *"`offload ps` on a phone answers locally, without asking anyone"* — no round
trip, works on a dying signal, works with the laptop asleep.

**2. Already forwarded to whoever owns the thing.** `SetDeadline` is *"forwarded to the node that
owns the field rather than applied wherever it was typed"* (`api.rs:131`); `Logs` proxies to the
holder (`server.rs:1886`); `Answer` reaches a run's mailbox from wherever it lands
(`server.rs:212`); `Explain` runs a fresh bid round costing one message per peer. `Cancel`,
`Checkpoint`, `Resume`, `Remove` and `SetPriority` sit on the same machinery — local first, then
forwarded to the holder (`server.rs:515`). **This is the "perform actions" half, and it also
mostly exists.** A phone that can reach one member can act on any run in the fleet.

**3. Deliberately local, and this is the gap.** `Audit`, `FleetHistory`, `Sinks`, and the whole rule
set (`Watch`, `Rules`, `Unwatch`, `Triggers`) are local *by design*, each with a comment saying so.
A sink's credentials never leave the device holding them; a rule binds a program this node's owner
nominated; an audit log is a record of what *this machine* decided. `Audit`'s comment states the
consequence outright: *"Asked of each node in turn when the question is about the fleet."*

So a phone terminal showing *"why didn't my phone buzz"* for the desktop, or *"what did the laptop
refuse last night"*, has to reach that specific node and ask it. Two things are missing for that,
and only one of them is a design question:

- **No request in the protocol names a node.** Not one variant carries a `node` field. Fleet-wide
  answers come from gossip and per-run answers are routed by *run*, so the addressing mode a
  terminal wants — *ask that machine over there* — has never been needed.
- **`Drain` is the sharp one.** `Request::Drain` (`api.rs:92`) is *"hand every run here to somebody
  else and stop taking new ones"*, and it acts on whichever node receives it. **"Close the laptop
  from my phone" is not expressible**, and closing the laptop is this project's own tagline. Draining
  the machine you are walking away from, from the device in your hand, is the single most obviously
  correct thing a phone terminal would do.

That is the finding worth carrying out of this section: the terminal is roughly two-thirds built,
and the missing third is not "remote control" in general — it is **per-node addressing for the
handful of requests that are honestly local**, plus a decision about whether `Drain` may be aimed.

### It is not Claude Code's Remote Control, and the difference is the whole product

The owner's framing, and it is the sharpest statement of what this is for:

> I know the Claude app has remote control already, but with that you have to set up a running
> Claude instance with RC enabled first. With Offload you just have the node up and running and
> you can dispatch tasks.

That is a real difference in kind, not in polish. **Remote Control attaches a phone to a session
that already exists.** Something must already be running, on a machine somebody already chose,
prepared in advance, and still alive when the phone connects. The phone is a remote keyboard for
one instance.

**Offload dispatches work into a fleet.** `Request::Submit` from the phone, a bid round decides
where it lands (`offload-cluster::place`), and the phone never names a machine — it does not need
to know which of them is awake, plugged in, or has the repo. Four consequences fall out, and each
one is something an attach model structurally cannot do:

- **Nothing has to be running.** The daemon is up because the laptop is on, not because somebody
  started a session before leaving the house. Dispatch is the *first* interaction, not the second.
- **The run does not belong to the phone.** Close the app, lose signal, run out of battery — the
  run is held under a lease by whichever node took it, and the phone is an observer that left. An
  attached session ends when the attachment does.
- **The run can move while you watch it.** Lid closes, the holder drains, the transcript and the
  workspace migrate and the agent resumes mid-conversation on another machine. There is no session
  for a remote keyboard to be attached *to* — that is exactly the property that makes this project
  worth building, and it is invisible from the phone, which is the point.
- **The phone is a member, not a client.** Its own key, its own certificate, its own local view
  answered from gossip. `offload ps` works on a train with the laptop asleep, because it is not
  asking anybody.

**Where the attach model is genuinely better, and this is the honest part:** you can talk to the
agent mid-run. Nothing in `api.rs` injects a message into a running agent — the enum is dispatch,
observation, permission answers and lifecycle, and there is no `Say { run, text }`. The nearest
thing that exists is `Checkpoint` followed by `Resume { run, prompt }` (`api.rs:109`, `:113`):
capture at the next turn boundary, then restart with something new to say.

That is not a workaround so much as the same idea arriving from the other direction — ADR-0004 says
the turn boundary is the only safe place to capture a run, so a mid-turn interjection was never
going to be honoured mid-turn anyway. Whether the round trip is good enough, or whether a phone
wants a queued `Say` that the holder delivers at its next boundary, is an open question and is
listed as one below. It is the one place where "a terminal into the other nodes" asks for something
this protocol does not have.

## What this costs the phase-5 demo

`docs/ROADMAP.md:57` states the target:

> phone on mobile data joins from outside the LAN. On battery it declines work with a legible
> reason but still submits and observes runs. **Plugged in on Wi-Fi it wins a bid for a review run
> and executes it.**

The first two sentences are reachable, and `scripts/build-android.sh` plus a forwarded port is most
of what they need. The third is the problem, and it is subtler than "it will not work":

- **Under Termux it may well pass**, which is exactly what the walk's stage 4 was going to find
  out. Termux execs `claude` on its legacy exemption, the phone wins the bid, the demo is green.
- **In the app it can never pass**, by W^X, whatever the battery says.

So the demo as written can be *closed* by a configuration that is not the product — and closing it
that way would record phase 5 as done on the strength of the one property the shipped app does not
have. That is worse than leaving it open.

Nothing here amends the roadmap; a use case does not decide. But that sentence should not survive
its next reading unchallenged, and the choice is between narrowing it to what an app can do and
saying plainly that clause three is a Termux-only result.

## Which gap is worth an ADR

One, and it is not the app.

**A phone is a non-hosting member because of the platform, not because of its owner's policy.**
Today the only way to say "this device hosts nothing" is `accept = "never"` under `[policy]`
(`offload-node/src/config.rs:476`) — and that is a **work policy**: what the device's owner
*permits*. What is true on Android is a **capability** fact: what the device *is able to do*.
`docs/GLOSSARY.md` keeps those two apart deliberately, and this project's own vocabulary is
emphatic that *"a phone that says 'not right now' is exercising policy, not lacking capability, and
the two must not be conflated."*

Expressing a platform's hard limit as an owner's preference is that conflation, in the direction
that hurts: it reads as revocable, it reads as a choice, and it makes `offload explain` say the
owner declined when the truth is the device cannot. The saving grace is that the probe already gets
this right for the wrong reason — no `claude` binary, no agent capability — so the ADR may find the
mechanism is already correct and only the *reporting* needs to say which of the two it is. That is
worth an ADR either way, because the answer decides what phase 5's demo becomes.

Not worth an ADR yet, because they are engineering rather than decisions: the JNI shim, the
`run()` extraction, the emulator rig. The per-node addressing question above is close to one, and
should wait until somebody has actually wanted it twice.

## The emulator, and what it cannot answer

The owner's instinct is right, and better than it looks. The emulator sits behind qemu user-mode
NAT: outbound UDP works, inbound needs an explicit `redir`, the host is `10.0.2.2`, and there is no
multicast. **That is a faithful mobile-data shape, not a limitation** — dials out to a seed, never
dialable, no mDNS. It is the phase-5 rig.

It simulates more than expected: `dumpsys battery set ac 0` and `set level 15` drive the policy
path, `dumpsys deviceidle force-idle` drives doze, `cmd thermalservice override-status` drives
thermal, and `-netdelay`/`-netspeed` drive the transport. On an x86_64 host the
`x86_64-linux-android` target runs at native speed.

What it cannot answer, and what ADR-0038's closing line is actually asking for — *"every claim here
about what a phone's network does is a prediction until `offloadd` is running on mobile data"* —
is real radio sleep, carrier CGNAT rebinding timeouts, genuine doze wakeups, and phantom-process
behaviour under memory pressure. Those need the physical device.

One more: this would put `quinn-udp` on a third platform, while the open bug in this tree right now
is `quinn-udp` on macOS. Android's GSO/GRO story is its own measurement and should be budgeted as
one rather than discovered.

## What would be worth doing, in order

1. **Measure, with what is already built.** `scripts/build-android.sh` exists and the NDK tax is
   already paid; the emulator adds an `x86_64-linux-android` target to it and needs no device.
   `adb push` to `/data/local/tmp`, a seed pointing at the laptop, join. **No app code, no Kotlin,
   and no Termux** — the shell UID execs from there without the exemption, so this measures the
   daemon rather than Termux's deal. It answers: does the QUIC path survive a NAT and a doze cycle,
   does `/sys/class/power_supply` read under SELinux, does `DeviceClass::Phone` actually come out,
   and what does `quinn-udp` do on a third platform.
2. **Extract `run()` from `main.rs`.** Independently worth it, and the precondition for every
   in-process host including the test harness.
3. **The ADR above**, once (1) has said whether the capability half already reports correctly.
4. **The APK**: foreground service, JNI shim, `ConnectivityManager.isActiveNetworkMetered`,
   `BatteryManager`, notifications as a sink, Keystore-backed approver, and the terminal UI over
   `api.rs`.

## Open questions

1. **Does a person holding an unlocked phone count as attendance for a run on the desktop?**
   Attendance is observed, never declared, and **never gossiped** (ADR-0013). So today: no. But if
   the phone is the answer surface for ADR-0017 questions, then the device that is attended and the
   device deciding whether to act autonomously are different machines, and the fleet has no way to
   know the human is right there. This may be the most interesting thing the mobile app exposes.
2. **Ed25519 in Android Keystore, and StrongBox.** Keystore gained Curve25519 in Android 13;
   StrongBox implementations generally top out at P-256, RSA, AES and HMAC. The fleet is ed25519
   end to end (`ed25519-dalek`, workspace `Cargo.toml`), so a hardware-backed approver may force an
   algorithm-agility question onto ADR-0012 — or may have to settle for TEE rather than StrongBox.
   **Unverified against current KeyMint documentation, and it should be, before the approver item
   is scheduled.**
3. **Is in-process actually required, or is a packaged binary under a foreground service survivable
   in practice?** Measurable on a real device over a few days, and the answer changes how much of
   (2) in the previous section is needed.
4. **Should a phone be able to say something to a running agent?** There is no `Say { run, text }`
   in `api.rs`, and `Checkpoint` + `Resume { prompt }` is the round trip that stands in for one.
   ADR-0004 makes the turn boundary the only honest place to deliver such a thing anyway, so the
   question is not *whether* it waits but whether the waiting should be **queued by the holder**
   rather than performed by the operator as two commands. The moment somebody uses the phone
   terminal in anger is the moment to answer it.
5. **iOS.** Not explored. Strictly harder — no `exec` at all, no background daemon, and a much
   narrower socket lifetime. If the answer for Android is "a member that does no work", iOS is the
   same answer against a worse app model, which is at least a cheap thing to find out.
