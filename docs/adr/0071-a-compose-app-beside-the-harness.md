# ADR-0071: A Compose app beside the harness — the product UI, over a Rust client

**Status:** accepted · 2026-09-26 · session ninety-two · builds on ADR-0066 (the Android host) ·
supersedes nothing: ADR-0066's harness stays as it is

## Context

ADR-0066 built the Android host to "just enough to run our tests": the daemon under a foreground
service, `host-facts.json`, and a screen that prints `offload status` and the log tail. Everything
else goes through `adb shell run-as`. That harness has walked phase 5, phase 8 and the hardware key
on two devices, and it earns its keep because it is small and quick to change.

The owner now wants the real app, with Jetpack Compose. It should cover status and fleet, runs,
questions and approvals, and enrolment and settings. They also asked for it to be **built
separately**, so the harness stays the quick path for testing.

Two things make this more than a UI job. The first is `docs/pitfalls/reports-and-cli.md`'s first
rule: a report has to come from where the decision reads. A screen that recomputes "is this node
accepting?" from fields is a second copy of the rule, and it will be confidently wrong. The second
is that the control protocol has about seventy request and response types. Hand-written Kotlin
copies of them would drift from the Rust ones with no error to notice, and that is the failure
this workspace pins migrations and signing bytes to prevent.

## Decision

### 1. A second Android project, not a second screen in the first

`android-app/`, application id `se.mach25.offload.app`, beside `android/` (`se.mach25.offload`).
They share nothing at build time. The harness is not a library for the app, and the app is not
allowed to grow into the harness. Both can be installed on one device. Each hosts its own daemon,
and **only one should run**: both default to port 7433 and would contend for it. That is a testing
arrangement, not a product one.

### 2. The app talks to the daemon through Rust, with the daemon's own types

A new crate, **`offload-mobile`** (a `cdylib`), is exposed to Kotlin with **UniFFI**'s proc macros.
It uses the one socket client the CLI uses, moved out of `offload-cli` into
`offload_node::client`. So framing, `Done`, and the sentences for "no daemon at …" and "nothing is
listening" have one implementation.

It turns the daemon's responses into **screen models**: UniFFI records built from
`offload_node::api` types in Rust. The conversion is checked by the compiler. A field the daemon
removes fails the build, and one it adds is absent from the screen until somebody adds it, which is
a loud gap rather than a quiet disagreement. The daemon's reasons (refusals, `accepting no — …`, a
node's status) reach the screen as the daemon worded them. The app does not paraphrase a decision.

The generated Kotlin comes from the built library (UniFFI "library mode"), so the Kotlin can never
describe a different version of the Rust than the one shipped in the APK.

### 3. The app hosts the daemon as the harness does

The same `liboffloadd.so` and the same foreground service, `host-facts.json` and approval key,
reimplemented in the app's own package, so that changing the app never touches the harness. When the
app is stopped, it stops its daemon.

### 4. Compose, Material 3, one activity

Bottom navigation with four destinations, in this order: **Status**, **Runs**, **Questions**,
**Settings**. The approval prompt is the system `BiometricPrompt`, raised from the app, as in
ADR-0069 §4.

## Consequences

- The first slice is the plumbing and one screen: `offload_node::client`, `offload-mobile` with its
  UniFFI bindings, the Compose shell, and Status and fleet. Runs, Questions and approvals, and
  Enrolment and settings follow, each walked on a device.
- `scripts/build-android-product.sh` builds the daemon, the CLI and `liboffload_mobile.so` for both
  ABIs, generates the Kotlin bindings, and assembles the app. `scripts/build-android-app.sh` still
  builds the harness and nothing else.

## What this deliberately leaves

- **Release signing and a store listing.** Debug builds, installed with adb, as the harness is.
- **iOS.** UniFFI generates Swift from the same crate, which is the reason to choose it over JNI by
  hand. The owner has decided the iOS product app will use **SwiftUI**, built separately from the
  ADR-0070 host harness in `ios/`, as this app is from `android/`. It is not being built yet.

## Amendment, 2026-09-26: slice 2 (Runs) and the brand, walked on the emulator

**Runs.** `offload-mobile` gained `runs`, `log`, `submit_task`, `submit_agent` (a repository URL,
since a phone path means nothing to the node that takes the run) and `cancel`. Two rules the screen
needs moved out of the CLI so both clients use one copy:
- `offload_node::render::event_lines`, the log wording `offload logs` prints (it was
  `print_event`'s `println!`s), with `now` passed in rather than read from the wall clock;
- `render::is_finished`, which decides both `ps`'s filter and the app's Active/All.

The screen lists runs, opens a sheet with the log (polled while open) and Cancel, and submits
through a dialog. A refusal is shown in the fleet's words. Because it names `--queue`, the dialog
has the matching "leave it pending" box.

Walked on the emulator's product node, which joined the laptop's walk fleet:
- `tick` was refused with each node's reason (the phone was unplugged);
- queued, it listed as `pending`;
- cancelled, with the daemon's note;
- under All, a completed run held on the phone opened with its real log.

Found on the way: the keyboard autocorrected `tick` to `tickets`, so service, URL and model fields
now take text verbatim.

**The brand**, at the owner's request. The app is always the icon's navy (a gradient from
`#172554` to `#0B1024`) whatever the system theme, with the mark (`assets/offload-logo.svg`) and an
"Offload" wordmark in the loop's cyan-to-violet gradient. Cyan is primary, the arrow's orange marks
warnings, and cards and sheets are navy. The system bars are forced light-on-dark: in the
emulator's light mode the clock had been dark on navy.

**Following a live run**, walked once the phone was charging again. `tick` from the emulator was
accepted by the phone. Its finished notice fired the phase 8 walk's standing rule, which placed an
agent run on the laptop (the walk's stub agent). Opened while running, the sheet showed that run's
log live: the submission, the prompt with the trigger's report, the workspace and the branch. Two
fixes came out of it:
- the sheet kept the row it was opened with and went on saying `running`, with Cancel, after the
  run had finished. It now reads the run from each listing;
- the sheet opened half-expanded, so the newest lines, where the log scrolls to, were off screen.
  It opens fully expanded.

Right after a restart, a log held elsewhere is refused with the daemon's own "no known address —
not discovered yet". The sheet's retry fills it in once gossip brings the address.

## Amendment, 2026-09-26: slice 3 (Questions and approvals), walked on the phone

`offload-mobile` gained `asks` (asked live every time, never kept, per ADR-0017) and `answer`,
which is addressed to the agent's own `tool_use_id`. `submit_agent` gained `ask`, the CLI's
`--ask` with the `ask` permission mode. The Questions screen lists what is waiting anywhere in the
fleet, with Allow and Deny, and pending hardware-key approvals with Review, which raises the
system prompt as ADR-0069 §4 describes. The tab shows a badge with the daemon's count.

Walked on the phone, after it moved to this app as the same node:
- an agent run on the demo repository, submitted from the phone with "ask me", was accepted by the
  laptop;
- the walk's stub agent posed a real question through `offloadd ask-hook`, and the phone's tab
  showed a badge within seconds: "May it use Bash? rm -rf build/ && cargo build --release, run … on
  laptop · waiting 29.6s · decided by nobody answering in 4m30s";
- Allow reached the hook on the laptop as `allow`, "allowed by an operator on phone-app", and the
  run finished.

Found on the way: headings outside cards drew black on the navy, because the transparent scaffold
had no content colour; it now has one. SwiftKey turns the stub's trigger word into prose, so the
stub triggers on "rebuild". The Approvals half is built but not walked here: this app has no way
to make its own key until slice 4, and Android keeps the harness's key with the harness.

**The tablet moved too**, the same way as the phone: same node (`3502859e`), same fleet (`f1ee7003`),
harness state kept as `s.moved-to-app`. It was that fleet's hardware approver, and its TEE key stays
with the harness. Approving from this app needs its own key, named again with the passphrase
(slice 4).

## Amendment, 2026-09-26: slice 4 (Enrolment and settings) and joining by link

**Settings** shows this device's node id (with Copy), its fleet and grants, and the approval key
with a "Make approval key" button. It says whether this node's delegation names the key, names
another one, or names none. The work policy (accept never, while charging or always; a battery
floor; metered data) is written into `node.toml` with `toml_edit`, so the owner's comments and
other settings stay. The daemon's own loader must accept the result before it replaces the file,
an emptied `[policy]` table is removed, and the app restarts its daemon, since config is read at
start. `offload-mobile::Device` does these through the state directory, as `offload id`, `join`
and `fleet` do.

**Joining** goes through `offload_node::fleet::take_up_invitation`: the decisions `offload join
--token` made, moved out of the CLI so both clients join, move to a successor fleet and refuse by
the same rules. After a first join the app restarts its daemon itself, which the CLI can only
advise.

**By link**, at the owner's suggestion. `offload invite` also prints `offload://join?token=…`, and
the app registers the scheme. Opening the link lands on a confirmation read from the invitation:
the fleet, the name and grants this device would get, whether it names this device, and a trust
note. The tap never joins by itself, because a link can come from anybody. Custom schemes open
from a browser, a notes app or a QR scanner. Some chat apps linkify only `http(s)`, and a verified
`https` App Link needs a domain the owner controls.

Walked:
- **tablet:** the key was made in the TEE, and the card named the harness's key as the one the
  delegation names. Setting "Never" restarted the daemon into `accepting no — node is not accepting
  work`, and Default brought it back;
- **emulator**, reset to a fresh node: an invitation link opened the confirmation, Join took it up,
  and the daemon restarted as a member.

Found on the way:
- a token cut short produced the decoder's "EOF while parsing a list at line 1 column 93", now
  "is incomplete — it was cut short … Copy the whole line" (shared, so the CLI says it too);
- the refusal appeared under a "Done" heading, and now has its own;
- `adb input text` truncates a 1,700-character token, which is how the cut-short case was met.

## Amendment, 2026-09-26: notifications — the app is a sink

The Questions badge showed only while the app was open, so a question asked with the phone in a
pocket went unnoticed. The fleet already has the delivery plane for this (ADR-0010): routes,
advertised as capabilities, with an outbox that deduplicates and retries. So the app is a route. At
start the service adds `[[sinks]] id = "app", service = "push"` to the config if it is missing,
through `Device::ensure_app_sink` (`toml_edit`, the daemon's loader must accept the result, an
owner's own `app` sink is left alone). Its program is one line of `/system/bin/sh` that renames the
JSON the daemon writes on stdin into `s/notifications/`, whole. The service turns each file into an
Android notification and deletes it. `asked` goes on a high-importance "Questions from agents"
channel and opens the Questions tab; the rest is "Fleet news" and opens Runs. Runs notify everyone by
default, so nothing has to name the route.

Walked on the tablet:
- `offload sinks --test` answered `test delivered` and appeared as "run 000000000000 failed";
- a `hello` task submitted from the laptop's node in the same fleet ran on the tablet, and its
  finished notice arrived as "run 01a0de53637c finished", with the logo, in the shade.

The sink's script also runs under `sh` in `the_app_sink_files_one_notification_whole`. **Not walked:**
the question channel, which needs an agent to ask (the phone's fleet has the stub).

**The question channel, walked on the phone** (18:41, phone locked, app updated with the sink added
at start). An `--ask` run submitted on the laptop posed its question at 18:41:44. The laptop
delivered "run 01a0de97b1b6 needs a decision" to `phone-app/app` at 18:41:47, and it was posted on
the high-importance "Questions from agents" channel. The owner tapped it and allowed from the phone,
and the hook on the laptop received `allow`, "allowed by an operator on phone-app", at 18:42:39.

Tapping the notification opened the Questions tab, as designed. The owner could not tell whether it
buzzed: the phone was in mute mode, which silences every app. The first Questions channel had not
asked for vibration at all (`mVibrationEnabled=false` in `dumpsys notification`). It is replaced by
`questions-v2`, with vibration (a short double buzz) and the notification light. Channel settings
are fixed once a channel exists, so the old one is deleted rather than edited.

**Re-walked with vibration**, with the phone on vibrate: the question delivered at 18:47:23 buzzed
(the owner confirmed), and the Allow from the notification reached the laptop at 18:47:38.
Notifications for questions are done.

## Amendment, 2026-09-26: approving with the product app's key, walked on the tablet

The tablet's own walk fleet was moved aside (`fleet.json.f1ee7003`), and `init --hardware-key --name
tablet` founded fleet `f1ee7002` with the key the product app made in the TEE. Status then read
"approval P-256 in the TEE (hardware-backed) — this node approves with it". `offload invite` for a
fresh laptop node (`/tmp/hw2`, `laptop-hw2`) was confirmed on the tablet. `offload join --token`
on the laptop accepted the P-256-signed certificate, and the two meshed: each lists the other
`alive`.

Found on the way:
- **the first attempt expired unnoticed.** The request was filed and the "Approval requested"
  notification posted, but the tablet was locked and the channel had no vibration
  (`mVibrationEnabled=false`, as the first Questions channel had). It is `approvals-v2` now, with the
  Questions channel's buzz and light, and it opens the Questions tab, where Review is;
- **two daemons were running**, the harness's and the app's, on the same port. The harness had been
  running since 17:50 on an empty state (not a member, so it listened on nothing), started from a
  launcher entry identical to the app's. What started it was not traced. The harness is labelled
  "Offload harness" now, so the two can be told apart.
- `offload join --name` was silently ignored with `--token`, because the invitation carries the
  name. The two conflict now, and both fields have help.

**Not walked:** re-approval through the product app's key (`purpose = "reapprove"`, built and
tested in `mesh.rs`). It needs the approval window shortened to minutes on a walk build.

**Small, from the owner on the phone:** the work policy's selected segment drew a check mark, and in a
quarter of the row that broke "Charging" across two lines. The segments have no check mark now (the
fill marks the choice), and labels stay on one line. The submit dialog changed with ADR-0072: agent
run first, an optional repository, and remembered repositories.

## Amendment, 2026-09-26: the run composer is the app

The owner: creating a run "feels like a secondary function. The fields feel clunky … This is the
primary function of the app." They chose, from three shapes, a **composer home**:
- **Runs is the first tab**, with the fleet's status second and renamed Fleet ("I'm not really
  interested in that");
- **a composer is pinned to the bottom**, as in a messaging app: a growing prompt box and a send
  button. Above it are a "where it works" chip (Empty folder, the remembered repositories, or
  another URL, in a sheet), an "Ask me" toggle, and More (model, "leave it pending", and "Run a task
  instead…", which is where tasks went, by the owner's choice);
- **a refusal** appears above the composer in the fleet's words with the prompt kept, and "Leave it
  pending" is one tap;
- **the list** shows runs that are going, then Earlier. A finished task shows only if it failed,
  since the walk fleet's per-minute `tick` buried everything. A waiting question shows as a banner
  that opens Questions;
- **it cleans itself up**, at the owner's suggestion: a finished run leaves the list a day after it
  started, a swipe archives one now (with undo), and "Archived (N)" shows them again. Archiving is
  this device's view (`Archive.kt`). The run's record, log and branch stay in the fleet.

Two layout fixes found on the emulator:
- the composer floated a keyboard-height above the keys, because the window was also *panning*.
  `adjustResize` on the activity fixed it;
- the tab bar is hidden while typing.

Not in it: which device holds a run, since the listing does not say (`RunSummary` has no holder).
Not built yet: looking inside a run's workspace, and sending a new prompt to the same workspace
(the fleet already has `offload continue`, ADR-0064). The owner asked for both.

**Continue from the app** (the owner: "send another run to the same workspace with a new prompt"):
a finished agent run's sheet has "What next?" under its log. It is `offload continue` (ADR-0064)
through `Daemon::continue_run`: the new run starts on the parent's branch, handed the parent's
prompt and result, or with "Keep the conversation", the parent's session. Walked on the emulator
against the stub laptop: `continues 01a0df31bc6c (handoff)`, the parent transcript placed, and the
new run completed. Its row says `continues <parent>` (`RunRow::continues`, from the daemon's own
field).

**Reading the answer, and editing a pending run** (the owner, from the tablet: "being able to read
the output would help"). The agent's closing message was on the record since ADR-0064 and printed
nowhere, not even by `offload logs`. `render::event_lines` now ends a finished run's log with
`answer` and the message verbatim. The app shows it first in the run's sheet, in a selectable card.
A pending agent run has **Edit**: it is withdrawn through the ordinary cancel, which its arbiter
decides, and its prompt and repository go back into the composer to change and send. It is not
edited in place: a submitted spec gossips, and only its deadline and priority have an owner and
a counter. If a node takes it in the moment before, the run is cancelled and the text is still in
the composer. The run's actions have a row of their own, since "Cancel" beside the id and the state
was crushed into a column of letters.

**The choice in plain sight, and help** (the owner, after queueing an email summary as an agent run
while believing it was a task: "I didn't even tap the dotted symbol … make the options more visible",
and "how do I even use the What should it do? thing? … you get no help"):
- the composer opens with a switch, **Ask the agent | Run a program**. "Task" is Offload's word for a
  nominated program (ADR-0019), and in plain English it means any job, so the app says "program";
- a program is picked from chips listing what the fleet's devices offer. `NodeSummary::programs` is
  the capabilities with the execute role that are nominated and runnable, which is what a task's
  constraint asks, so the list is the fleet's own;
- one line says who can do it before anything is sent: "Devices with an agent: …", or in orange that
  none has one. It says what devices *have*, never "done by", since whether one takes the run is
  the fleet's decision (the tablet's laptop has an agent and no `host-runs`);
- every option is a visible chip: where it works, Ask me first, the model, Wait if nobody's free.
  The ⋮ sheet is gone;
- an empty list shows a guide: what to type, with examples, and where the answer appears;
- a pending run's sheet shows **Why it's waiting**: the `offload explain` verdict and each node's
  answer (`Daemon::why`), where it had said only "pending".

**Background cost** (the owner's battery question): the screens polled the daemon every few seconds
with nobody looking, and the service checked two directories every second. Each screen's loop now
runs under `repeatOnLifecycle(STARTED)`, cancelled in the background and started again on return.
The service waits on a `FileObserver` (`DirWatch`, with a 30 s fallback) for notifications and
signing requests. Measured on the phone in the background: the app process went from 2.5% to 0.12%
of a core, and the daemon was at 1.9% after the gossip and failure-detector fixes. A test
notification through the app's route still appeared within 3 s.

**Who can do it, with permission** (overnight): the composer's line listed the Mac mini as able to
take agent runs, but it has an agent and no `host-runs`. `NodeSummary::may_host` now reads it from
the certificate each node presented, which is what the bid round checks, or from this node's own
grant (`None` when no certificate has been seen, which is unknown, not no). The line says "Can take
agent runs: laptop · macmini has an agent but isn't allowed to host".
