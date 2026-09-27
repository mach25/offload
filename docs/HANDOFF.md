# Where we are

Last updated: 2026-09-27 (session ninety-four).

`CLAUDE.md` is the standing instruction set — the rules that do not change. This is the part that
does: what is true right now, and what to do next. **Read this and then the pitfall file for
whatever you are about to touch.** The reasoning behind how things got this way is in
`docs/sessions.md` (by session) and `docs/phases.md` (by phase); read those when you need them, not
at the start of every session.

*(Session ninety-one cut this file from ~98 KB to about half — the rest is "What works today", untouched. What was removed was a
per-session history of the state line — every session's test count, why it did or did not bump the
wire, and a paragraph per walk — all of which is in `docs/sessions.md`. Nothing open was dropped:
every pick-up item, residual and "do not re-walk" subject below was carried over. Keep it short:
**replace** the pick-up list each session, do not append to it.)*

## State of the tree

Everything is on `main`. There are no other branches, and nothing has been pushed anywhere.

- **1138 tests, clippy clean, wire v37, schema v13** at the end of session ninety-four. v37 is
  ADR-0080 (`AgentDetails::models` as the agent's own named list, `Gossip::models_asked`), v36
  ADR-0078's `NodeView::quiet`, v35 ADR-0076's gossiped addresses. v31 is `ContinueBase` (session
  ninety); v29 `Gossip::schedules`; schema v13 is ADR-0067's `legs_json`. A number here is a stamp on a moment —
  `cargo test --workspace` says what is true today.
- **Phases — `docs/ROADMAP.md` is the authority**, and this file does not restate its lists. 0–4
  done; 5 at 10/14; 6 at 40/41 (metrics, deliberately); 8 built, demo run on two daemons; 9 at
  6.5/7 (§6's fourth refusal deliberately unbuilt); **10 done** (ADR-0063 §2's weights walk, run in
  ninety-two on the laptop, the Mac and an emulated phone). The roadmap's own table has been wrong four times —
  re-measure an inherited `[x]` before reasoning from it.
- **ADRs 0001–0080 are all accepted and all built except**: ADR-0061 (phase 9, above), ADR-0037
  (§0's phone half, §4's relay and §5's punching; §2 has been built since session twenty-nine, and
  this line said otherwise until ninety-two), and ADR-0038 (unbuilt, phase 7). **Read the amendments, not only the decision** — the ones that bite are
  ADR-0019's (`HasService` asks whether a program was *nominated*; `ServiceAuthenticated` whether
  it can run), ADR-0061 §4/§6 (the archive fits the 512 MiB cap: what travels is a subset the agent
  chose, never a snapshot), ADR-0063 and ADR-0064.
- **Session ninety-two**: amended ADR-0037 (§0 measured, and the Mac meshes) and ADR-0063 (§2
  walked); no wire or schema change. **The first Linux↔macOS fleet**: meshed over v6 and v4, ran agent
  runs on archive workspaces in both directions and a task from Linux to macOS, and walked the
  weights table. Code changed on the way:
  - `[cluster] listen` defaulted to `0.0.0.0:7433`, which could not be dialled on IPv6 **or dial
    it**. Now `[::]:7433`, dual-stack with `IPV6_V6ONLY` forced off and a loud v4 fall-back
    (`quic::bind_socket`); a v4-only node's refusal of a v6 seed names the socket.
  - …which then printed v4 peers as `[::ffff:…]` in the send-refusal report. Canonicalised.
  - `disk_free_mb`, probed to the MB, made every probe a "change": an incarnation and a full
    re-gossip every 30s, for ever. Whole GiB, rounded down. And `what this device is has changed`
    names the fields now (`Capabilities::differences`), which is how that was found.
  - …and it was measured under the process's working directory, not the state dir: a state dir on
    a 210 MB partition advertised the root's 266 GB. `free_disk_under(state_dir)` now.
  - **Linux↔Android on the SDK emulator**, with `ANDROID_ARCH=x86_64 scripts/build-android.sh`. It met
    the laptop, hosted tasks, and the battery row was measured with the console setting the charge.
    Then phase 8's whole sentence across the two, and one report fixed on the way: quinn's GSO
    fall-back on a virtual NIC (`EIO` on a segmented send) was counted as the kernel refusing to
    speak, for the life of the daemon. It is `Probing` now, like `EMSGSIZE`.
  - **A real phone**, a Samsung phone on Android 16. First under Termux, which found that `/sys` power is
    denied to apps and that Claude Code has no Android build. Then through **an Android app**
    (ADR-0066, `android/`), installed with adb, which walked phase 5's demo in every clause but
    off-LAN. That one is blocked by the home router, and the carrier *does* give IPv6.
- **The method, which twenty-odd sessions converged on.** The ordinary path is where the defects
  are. Read the *whole* screen, not the line under test — sixteen walks running found the screen
  around the thing wrong. Run the **control arm**: a reverted build, a second node, or a second
  command beside the first (`ps` beside `audit`, `fleet` beside `join --token`). Walk the
  **default** configuration; a walk with `cluster.enabled = false` once proved nothing for a whole
  session. And a record can be wrong in the direction of confidence — a doc comment, a roadmap tick
  or a line in this file is a claim with a date on it.

## Pick up here

**No known defect stands.** Session ninety-three added ADR-0078 (a set-down phone is quiet, wire
v36) and ADR-0079 (the phone runs its daemon only while the app is open or a run is held). A push
knock was built under ADR-0079 and withdrawn at the owner's request: **no third-party app on the
phone**, and a stopped phone hears its news when it is opened. `docs/sessions.md` is the story.

**Carried from session ninety-three:**

1. **A host's facts go stale *because* the phone is asleep** (`capacity-policy-and-probing`). Ask
   in which state a writer stops before reading "stale" as "unknown".
2. **Ask about the owner's constraint at the step that costs them, before building.** The knock
   was designed, built and deployed, and the owner declined it at "install ntfy on your phone".
   Name every install a design needs, on every device, when the choice is offered.

**The fleets and devices, as left** (the details are in `docs/DEMO.md`):

- **There is one fleet, `f1ee7001`** (consolidated in session ninety-four; everything before was
  testing). The laptop's `/tmp/mw` node founded it and is its only approver. The owner has the
  passphrase, and it is needed for every `host-runs` grant: an approver may never mint one. Members:
  - `laptop`, `/tmp/mw`, a **real** agent on `~/.claude-alt`, `acct:0a0a0a0a…`, which owns `~/offload`;
  - `macmini`, the Mac mini (macOS 27.0 since 2026-09-27): config `~/.config/offload/node.toml`, state `~/.offload`, checkouts
    `~/offload`, a real agent on the same account, `host-runs` since session ninety-four. It runs as the launchd
    LaunchAgent `se.mach25.offloadd` (`~/bin/offloadd`), built from `~/mach25-offload-host`. It
    offers one program, `demo` (a `[[tasks]]` entry that says hello, echoes its arguments, and prints
    uptime and free disk), so the app's "Run a program" has something to show;
  - `phone-app`, the phone's product app;
  - `tablet`, the tablet's product app, a fresh node since session ninety-four (`pm clear`, then
    joined by link). The old tablet fleet `f1ee7002`, its TEE approval key and its pending email
    run are gone;
  - `emu-fresh`, the emulator's product app.
  **Agent runs in this fleet cost real usage.** The test fleet `f1ee7003` and the `/tmp/hw`,
  `/tmp/hw2` daemons were stopped. Their directories are left in `/tmp`, harmless.
- **Every node is on wire v37** (session ninety-four): the laptop, the Mac and all three apps. The
  tablet's app is one build behind (no "Resume is refused" removal): it was off USB. The Mac's stray walk
  daemon on `~/.offload-w6` was stopped at the owner's request.

**Next, in order:**

1. **The phone's battery question is answered; no overnight test is needed.** 4h20m on battery on
   2026-09-27 afternoon, daemon stopped: Offload held the multicast lock 3 min and drained 16.6 of
   758 mAh. The rest was the phone's own apps and **wireless debugging**, which holds
   `AdbMulticastLock` whenever it is on (ADR-0078's correction). The owner was told to turn wireless
   debugging off between walks. Ask them to turn it on before using the phone over adb.

2. **The Mac overnight, awake or not.** ADR-0077 now holds `PreventSystemSleep`, walked for five
   untouched minutes only. In the morning, and *before* any ssh (an ssh wakes it), check the laptop's
   `offload nodes` for `macmini`. Then run `pmset -g log | grep -E ' (Sleep|DarkWake|Wake) '` on the
   Mac and look for any `Entering Sleep` after 12:23 on 2026-09-27.

3. **ADR-0080 is walked on two hosts**: the Mac (macOS 27.0, after the owner allowed `offloadd` in
   Local Network and rebooted) and the laptop each read 11 models at startup, `offload models` lists
   every model as offered by both, and one `offload models --refresh` on the laptop at 14:34:22Z was
   read on both within a second ("asked on this node", "asked by the fleet"). The reboot also proved
   the LaunchAgent: the daemon came back at login with the Keychain login, held
   `PreventSystemSleep`, and logged no sleep. **Not yet walked:** the read after a Claude Code update
   (the version trigger). The next `claude` update on either host is the walk: its log should show
   `reason="the agent changed"`.
4. **A rebuild of the Mac's `offloadd` may need the Local Network click again.** It did after the
   first ADR-0080 build (macOS 26.6.2). After the owner allowed it, updated to macOS 27.0 and
   rebooted, the next rebuild (`85fe9b1`) was *not* blocked: one refused send at startup, then
   meshed. One observation, not a rule: warn the owner before a rebuild, and run the `python3`
   control (`docs/DEMO.md`) if sends are refused for more than a minute.

5. **The app's run rows and details (session ninety-four):** rows say who ran it and the model
   asked for (`RunSummary::host`, `::model`), and the detail sheet lists ran on, model used (from
   the log's `agent … , model …` line) against the model asked, agent, start, workspace, turns,
   tokens, cost and id. Walked on the tablet with the owner's email runs (laptop, haiku).
6. **Fixed in session ninety-four: a finished run was never told if its decider was alone, and a
   stale copy could reopen it.** See `gossip-and-merge`. Walked on the typo run: every node agrees
   it is cancelled now. On every node, the Mac included (`4932bbd`).
7. **The owner's email runs could not read mail**: Offload starts Claude Code with
   `--strict-mcp-config`, so the account's own Gmail and Microsoft 365 connectors are not handed to
   runs. Deliberate (the MCP rule in `agent-adapter`). Granting mail is ADR-0011's resources: a
   nominated MCP server granted per run with `--use`. Offer that to the owner rather than loosening
   the rule.

**Decided, do not re-ask:** a stopped phone does not host, not even on charge (the owner, session
ninety-four; ADR-0079's residual has the Android constraint if it is ever re-opened).

**Waiting on the owner:**

Nothing. (The Mac's `host-runs` was granted in session ninety-four, and the owner keeps the
passphrase in their password manager.)

**Open, not blocked:**

0. **Fixed in session ninety-four: `offload nodes` showed `dead` beside `seen 3s`.** `SEEN` is now
   first-hand only (`NodeView::heard_here`), and `never` for a node known only by gossip. On the
   laptop only: the Mac's daemon and the apps get it with their next build.
0. **Fixed in session ninety-four, on every node: news for an away device was given up on.** Two
   holes, both walked and both with control runs: a send with no answer counted as a failed
   attempt, and a restarted node took "not in my view yet" for "no longer in the fleet". The Mac got
   it at 15:20Z (`85fe9b1`).
1. **Keep-awake on Linux and Windows hosts** (ADR-0077): only if one is found asleep.
2. **Seen once, not re-walked: a second node declared the phone dead during a wifi-to-mobile
   handover.**
3. **iOS product app**: SwiftUI, the owner's choice, not started.
4. **`refused` is still unread on a screen.** (The app's "Resume is refused" note was removed in
   session ninety-four: the app has no Resume. The tablet was off USB and has not got that build.)
5. **`docs/pitfalls/detail/` is caught up**: 501 rules, 450 entries. Write the pair together.

**Built in session ninety-two, for finding the ADR:** hardware approval keys and re-approval
(ADR-0069 §4), the iOS host (0070), the Compose app with composer, Files, Continue, Edit and answers
(0071), empty workspaces (0072), good news to the asking device (0073), `~/.config/offload` and
`~/offload` (0074), the workspace viewer (0075), gossiped addresses (0076), keep-awake (0077). Also the
gossip ageing and the adaptive failure detector, with no ADR (pitfalls in `gossip-and-merge`).

**Decisions that want an ADR, and nobody has asked** — do not patch any of these.

- **A daemon that started outside a fleet joins the mesh only after a restart** (session
  ninety-two). `init`, `join` and `nodes` say so now. Starting the mesh in place would suit the app
  hosts, which have no operator to restart them. But a restart path with runs in flight needs the
  drain and adoption rules to be thought through, not patched in.

- **A fleet route's `last failure` carries the owning node's command path to a peer**
  (`alpha/flaky … /tmp/ow83/flaky.sh exited 7`, read on bravo), and ADR-0010 says a command never
  leaves its node. It arrives because delivery is proxied and it *is* the diagnosis. Matters for a
  shared fleet, not a personal one.
- **A device has two names**: the certificate's (every fleet-facing report) and the config's
  (`offload status`, the daemon's log — `main::report_membership`'s documented split, with a startup
  `WARN`). Changing it makes `[node] name` decorative on an enrolled device. The no-mesh path keeps
  node ids deliberately: there is no view to look a name up in.
- **A schedule freezes its `RunSpec` at creation** (ADR-0056 §3), so one written before ADR-0019's
  amendment keeps `HasService`. Repair is `offload unschedule` + `offload every`. Whether a stored
  spec is ever migrated is the question.
- **An escalation bound to a failing watcher fires once per failure** — up to four times for one
  submission since ADR-0058. Levers: the rule (once per subject until it recovers) or the
  notification plane (dedup the sentence).
- **The owner cannot say a nominated program is unsafe to re-run** (ADR-0058's residual). The lever
  would be a field on `TaskConfig`; unbuilt until somebody names such a program.
- **Hosting a task needs `host-runs`**, deliberately, though the cheap tier is exactly the work a
  device not trusted with an agent could do. "This phone may run tasks and never agents" would be
  `allowed_agents` with an empty-means-none form or a third grant.
- **`max_turns` cannot be raised** (ADR-0039's residual): a spent run's way forward is a new run —
  now `offload continue` (ADR-0064), which is most of the answer.
- **A drain cannot tell the person holding a question it is leaving** (ADR-0035) — that is the
  notification plane, not the drain.
- **A sole approver's certificate lapses after thirty days and nothing renews it** — deliberate (a
  self-renewing node never falls out of a fleet it was removed from) and reported correctly; levers
  are a second approver or the passphrase.

**Deliberate residuals, argued in their ADRs — know them before reading one as a bug.**

- A node that **dies** rather than departs still strands its failed runs (ADR-0043); a run failed
  under an earlier incarnation escalates `AttendanceUnknown` on either door. "Unknown is not good
  news" is the rule a change has to get past. ADR-0054 closed the graceful half; its 30-second
  budget is a constant measured on loopback, never over wifi with large checkpoints.
- A device revoked while partitioned keeps running (ADR-0044 / ADR-0002).
- ADR-0055: rescues holding uncommitted work or a named left-behind commit are kept for ever;
  `refs/offload/left-behind/` is written unconditionally.
- The probe cadence is 30s and nothing probes on demand (ADR-0048): `offload probe` and `offload
  status` can disagree for that long.
- A run escalated before the policy changed is not re-examined (ADR-0049) — deliberate.
- Which two callers `register` refused (ADR-0051) is still unknown; it logs at `warn` with **both
  epochs**, so the next occurrence names itself — equal epochs are two legs of one assignment.
- One unexplained failure in ~80 runs of the ADR-0052 race test; its assertion now says which side
  of the guard it landed on. **Re-measured session ninety-two: 0 in 800**, 600 of them ten at a time
  at load ~10. Not closed — just now below one in a few hundred.
- `offload-agent`'s `the_agent_is_told_where_its_state_is_rather_than_left_to_inherit_it` failed
  once at its `spawn` (`claude.rs`, the `expect("spawn")`) in a workspace run beside a 20 000-case
  property run, and then passed in 1 280 runs of its binary (up to 16 at a time, 40 threads each)
  and a full workspace run. Its message was lost to an output filter. `ETXTBSY` from writing a
  script and exec'ing it while another test forks fits, and is a guess; the `expect` prints the
  error, so the next failure names itself.

**Noticed, not defects.** A one-shot `offload logs` from a peer counts as attendance for
`WATCHER_GRACE` (10 s), the same as `logs -f`: in session ninety-two it made a restarted task's
second failure `attended`, so the task was left for a person. By ADR-0013's rule asking for the log
is the observation, so this is working as written. **Also noticed.** A peer's `WrongFleet` refusal is stored as its own first-person words
(ADR-0060). After a restart, a node that has not met the parent's node names it by id in `offload
continue`'s refusal (`node_name`'s fallback). *(The `--require node=` refusal that used to print two
ids is fixed — `bid.rs` names through the view since session ninety; this list said otherwise for
one session.)* A node still accepts more commitments than it can run (ADR-0006), so `runs 5/3` in
`offload status` is commitments.

**Already walked — do not re-walk.** From session thirty-four's await-audit: `replicate`, the
agent-start path, `collect_garbage`, the delivery pass, `delete_run`. Also: `--max-turns` under a
migration; `offload cancel` at an owed run; the trigger path to checkout removal; the spend writers;
the peer-hosted occurrence (ADR-0030); the withdrawal audit; `start_run`, `hold_here` and
`register` (ADR-0051); `Supervisor::live`'s readers; the drain's release paths; `decide_recovery`'s
escalations (swept three times); `GiveBack`; the revocation path, both directions, and a revoked
node's own `resume`; the two hosting doors and the fact behind them; adoption at startup on a
revoked node; both branches of ADR-0049's handover; the cost of an unplaceable run (one round per
30.2s, no epochs); the checkout sweep; the three refusals a resume meets; ADR-0052's three doors;
`resume` onto a node lacking the blobs, and with a bundle (ADR-0053); the three-leg ping-pong;
`BundleOutcome::AlreadyHere`; the trigger/notice name collision; ADR-0056's tombstone (every
direction but a contended occurrence, which is `storm.rs`'s job); the muzzled node and QUIC over
`WatchedSocket`; phase 8's demo as one sentence; `offload explain` state by state from two vantages
(session eighty-six) and from a third node and a node that never met the home (ninety-one); the
enrolment path end to end, including the 17ms pickup and ADR-0062's refusal over the network.

**Staging recipes are all in `docs/DEMO.md`** — the fake agent (`--version`, a transcript whose
assistant rows carry `message.id`, a `config_dir` per daemon, per-node file names), `pkill -x` and
never `-f`, state dirs short enough for `SUN_LEN` (so not the scratchpad), shortening `PROBATION`
and the sweep timers and putting them back, resetting a walk by deleting `state.db*` only, seeds on
loopback, and the one-daemon / two-daemon / three-daemon stagings each walk above used. Read the
section for what you are staging before you stage it.

## What works today, verified end to end

```bash
offloadd                                    # daemon; no config file needed
offload status                              # what this device is, and whether it'd take work —
                                            #   from the *last probe*, not from startup, so a
                                            #   link that becomes metered or a battery that
                                            #   drops changes the answer within a cadence
                                            #   (ADR-0048). Measured against a build where the
                                            #   fleet refused a submission for being metered
                                            #   while this line still said `accepting yes`
offload run --repo ~/dev/foo --follow "add tests for the parser"
                                            #   …and a node with no fleet refuses it when its
                                            #   owner's policy says so, which used to depend on
                                            #   whether it was in one (ADR-0046): `accept =
                                            #   "never"` measured refusing at its own socket,
                                            #   against a build that ran it to completion under
                                            #   `accepting no`
offload ps --all                            # survives a daemon restart — and where a row says a
                                            #   run is resumable, says whether *this* node would
                                            #   take the command it names. The stored reason is
                                            #   right about the run and silent about the machine,
                                            #   so the node's standing answer travels beside the
                                            #   listing and the two are paired on screen.
                                            #   Measured three times against a build that printed
                                            #   `resumable from turn 5 with offload resume <id>`
                                            #   one command away from `Error: node is not
                                            #   accepting work`
offload logs -f <run>                       # replays from SQLite, follows live
offload checkpoint <run>                    # capture at the next boundary, release the run
offload resume <run> [prompt] [--follow]    # continue from the last checkpoint — and *only*
                                            #   where this node may host at all: a drained node,
                                            #   one whose owner said `accept = "never"`, and a
                                            #   revoked one each refuse it now with the sentence
                                            #   `offload status` prints (ADR-0047). Measured
                                            #   three times against a build where all three
                                            #   resumed the run and went on taking turns — the
                                            #   revoked one having halted that same run seconds
                                            #   earlier.
                                            #   …and on a node the checkpoint was never
                                            #   replicated to it **fetches the conversation
                                            #   first**, which it never did until now: measured
                                            #   on two daemons, `fetching checkpoint blob
                                            #   blob=bf5035b4` and the run carrying on from turn
                                            #   5 to 12 with SAFE `replicated`, against a build
                                            #   where the same command on the same state
                                            #   directory answered `resumed <id>` and failed the
                                            #   run 12ms later with `state store: blob bf5035b4…
                                            #   is not stored here`. With nobody able to supply
                                            #   it, it refuses at the door — `no peer could
                                            #   supply the blob` — and leaves the run `pending`
                                            #   at the epoch it had.
                                            #   …and a run that migrated away, **committed**, and
                                            #   came back keeps what it did while it was away
                                            #   (ADR-0053). Measured over three legs — alpha,
                                            #   bravo, alpha — against a build where the third
                                            #   leg said `re-checked out, patch reapplied` and
                                            #   kept four of the eight files, the bundle having
                                            #   been replicated, named by the checkpoint and
                                            #   never opened. `offload logs` names which of the
                                            #   four ways the checkout came back, and that line
                                            #   is worth reading on a resume that *worked*
offload drain                               # …and it no longer leaves with the only copy
                                            #   (ADR-0054): every checkpoint nobody else has is
                                            #   pushed to a peer first, within 30s for the whole
                                            #   pass, and what could not be saved is counted out
                                            #   loud. Measured on two daemons against a build
                                            #   that said `nothing to hand over` about a run
                                            #   left `failed` and `here only` on the machine
                                            #   that was leaving — it is now `running` on the
                                            #   peer at turn 5.
                                            #   …and where it waits behind a question it names
                                            #   whichever deadline is nearer and what happens at
                                            #   it (ADR-0035, amended). Measured against a build
                                            #   that said `or 4m52s from now` two lines under its
                                            #   own `up to 15.0s`, then gave up and left the run
                                            #   mid-turn
offload cancel <run> / offload rm <run>
offload probe / policy / match "cores>=8"   # local questions, no daemon needed
offload match "unmetered"                   # …and it answers `no match` on a link nobody could
                                            #   ask about, where it used to say every device in
                                            #   the world satisfied it (ADR-0045). `metered =
                                            #   "yes"` in the config is the owner saying so, and
                                            #   the fleet then refuses the work out loud:
                                            #   "network is metered and policy disallows it",
                                            #   measured through a one-node bid round against a
                                            #   build where that sentence was unreachable

offload init                                # found a fleet; prints the passphrase once
offload join --passphrase                   # enrol this device; grants submit+deliver only
offload id / offload invite <node>          # the two-step path, for a device where confirming
offload join --token <token>                #   interactively is awkward. Walked case by case in
                                            #   session seventy-eight: ten refusals, each naming
                                            #   what it wants — a token pasted on the wrong device
                                            #   says *"certificate names 580993bf, but this node
                                            #   is b6efe4c9"*, which is the non-transferability
                                            #   rule doing its job. Re-inviting an existing member
                                            #   widens its certificate without walking to it; it
                                            #   can no longer *narrow* one (ADR-0062), measured
                                            #   against the build where two commands and no
                                            #   passphrase took `host-runs` off a device.
                                            #   On two daemons: a certificate taken up while
                                            #   the daemon runs changes the *arbiter's* answer
                                            #   in 17ms with no restart, and the renewal loop
                                            #   renews from an approver peer with the grants
                                            #   intact — and refuses, at `warn`, an approver
                                            #   that answers `RenewMe` with a narrower one
offload grant host-runs                     # deliberate, prompts, then 15 minutes' probation
offload verify                              # is what you wrote down still the fleet?
offload revoke <node> / offload fleet       # a revoked device is off the mesh in both
                                            #   directions, including the connection *it* opened
                                            #   — measured: a run it was hosting is orphaned
                                            #   1.7s after the revocation and running on the
                                            #   arbiter at epoch 2, against a build where it
                                            #   kept the run to turn 40 and finished it.
                                            #   And the device *itself* stops: the refusal on
                                            #   its next dial carries the signed revocation, it
                                            #   checks that against the fleet key it already
                                            #   holds, and stands down (ADR-0044) — agents
                                            #   halted, runs `failed` with the reason on them,
                                            #   `accepting no — this node has been revoked`,
                                            #   and nothing picks them back up — **including
                                            #   the person at its keyboard**: `offload resume`
                                            #   restarted the agent on an evicted device until
                                            #   ADR-0047, which is the door sessions forty-two
                                            #   and forty-three walked past. Measured 2.4s from
                                            #   the revocation, against a build that ran 48 more
                                            #   turns and completed — and it now refuses with the
                                            #   *revocation*, not with a drain. Standing down
                                            #   sets both latches and the door asked the drain
                                            #   first, so an evicted machine was told to `restart
                                            #   offloadd to take work again`, which cannot work.
                                            #   Measured one line under a run that had failed
                                            #   with `this node was revoked from its fleet`;
                                            #   `revoked_refusal` is one place now, shared with
                                            #   `offload status`, which had it right all along
offload nodes                               # who else is out there, and what they are holding
offload run …                               # → "accepted by node-b", or every reason nobody
                                            #   would — and "starting when its current run
                                            #   finishes" when node-b is full but committing,
                                            #   including for a submission a second after the
                                            #   one that filled it: the acceptance is in the
                                            #   view the next bid is answered from, and stays
                                            #   there. Measured on a node capped at one run,
                                            #   against a build where three submissions in one
                                            #   second were each told they could start now
offload run --queue …                       # …and when nobody will, the run is filed at the
                                            #   epoch the round *spent*, not the one it started
                                            #   from — so the token a swallowed grant issued is
                                            #   never handed out twice. Measured on three
                                            #   daemons with a grant that was never confirmed:
                                            #   recorded at epoch 2 and the retry granted at 3,
                                            #   against a build that recorded 0 and granted the
                                            #   same node epoch 1 a second time. The record
                                            #   names that node without claiming it meant
                                            #   anything: `last assigned to`, beside the
                                            #   submission's own `did not confirm`
offload drain                               # hand this node's runs to the fleet, mid-conversation
                                            #   — with a *real* agent, proven by a word that
                                            #   existed only in the conversation (session 23).
                                            #   A run still mid-turn at the deadline is handed
                                            #   over at its next boundary rather than left
                                            #   parked: the deadline bounds the wait, not the
                                            #   intent (ADR-0041), measured 3/3. And the run is
                                            #   the *fleet's* from the release onward, not this
                                            #   daemon's — a refusal now is not an answer about
                                            #   later, and the record says so, so a SIGTERM or a
                                            #   restart no longer strands it (ADR-0042):
                                            #   measured, placed one second after the restart
                                            #   where the previous build sat pending for good.
                                            #   And a run that *failed* here goes with it, if
                                            #   this node would have picked it up itself and
                                            #   somebody else holds a copy (ADR-0043) — 265ms
                                            #   from a SIGTERM to the same conversation
                                            #   spawning on the peer, against a build where it
                                            #   stayed failed through a drain, a wait and a
                                            #   restart of both daemons
offload explain <run>                       # who decides, what they decided, and what every
                                            #   node says about taking it right now — and,
                                            #   beside `recovery` rather than folded into it,
                                            #   whether `offload resume` typed here would be
                                            #   refused. What this node does unasked and what it
                                            #   does when asked are two answers, and a run left
                                            #   for a person is exactly where they differ
offload audit [<run>]                       # what *this machine* did, then: every grant with
                                            #   its epoch, every write it was fenced out of,
                                            #   every checkout it reclaimed or set aside.
                                            #   Per-node and never gossiped, so on a fleet ask
                                            #   each device. Walked in session eighty-seven for
                                            #   the four kinds one daemon can produce; the two
                                            #   that need a fence to fire have unit tests and
                                            #   have never been read on a screen. Timestamps are
                                            #   **UTC and say so**, and the unfiltered listing
                                            #   is the last hundred and says when there are more
offload ps                                  # turns and cost included for runs on other nodes
offload run --deadline 2h …                 # when it needs to be done; urgency rises by itself
offload deadline <run> 45m | none           # …and when that changes, at whichever node you
                                            #   are sitting at. On a run still *waiting for a
                                            #   slot* it reorders the queue then and there, and
                                            #   usually backwards: an unstated deadline means
                                            #   as soon as you can, which beats any time you
                                            #   can name. The note says where it now sits and
                                            #   the walk watched that come true (session 71)
offload rekey                               # the convergent revocation, walked with both
                                            #   daemons up: 3s from the command to a fleet that
                                            #   no longer talks, in both directions, with no
                                            #   message needing to arrive. Each end now says
                                            #   which end it is — the evicted device is told it
                                            #   needs the invitation, the other that a device it
                                            #   left out is still dialling (ADR-0060)
offload every 15m --task <service>          # …and it now says at the keyboard when nothing in
                                            #   the fleet nominates that service, which is a
                                            #   tick refused for ever and counted by nothing.
                                            #   The refusals around it are good: 30s, a bad
                                            #   unit, and an offset longer than the period
offload id                                  # sound (session 73): stable across calls, survives
                                            #   `init` on the same dir, agrees with `fleet` and
                                            #   `status`
offload verify                              # sound in six cases (session 72), and a wrong
                                            #   phrase names both fleet ids, which is what
                                            #   separates a typo from the previous fleet's secret
offload priority <run> 10 | -5              # who yields when two runs tie on urgency, and
                                            #   nothing else — so on two runs with no deadline
                                            #   submitted seconds apart it moves nothing, and
                                            #   says so. `offload explain` shows the value back
offload explain <run>                       # …and when a node will not pick a failed run back
                                            #   up, which of the three reasons it is: leaving,
                                            #   revoked, or its owner's policy — measured as
                                            #   `left for a person: this node's owner does not
                                            #   allow it to host runs, and its conversation
                                            #   exists nowhere else`, against a build where the
                                            #   same node resumed the run sixty seconds after it
                                            #   started refusing everything else (ADR-0049).
                                            #   And where somebody *does* hold a copy it is
                                            #   handed over instead: measured 412ms from the
                                            #   handover to the same conversation spawning on
                                            #   the peer, on a node whose link had gone metered
                                            #   under it
offload logs <run>                          # …including "this run will not make its deadline",
                                            #   said once, by whoever established it
offload sinks [--test]                      # routes to a human, this node's and the fleet's,
                                            #   including the ones that cannot be used
offload run --notify push …                 # …and which of them this run is for; says at the
                                            #   keyboard when the fleet has no such route
offload run --ask …                         # stop and ask a person rather than be denied
offload asks                                # …what is waiting, and how long it has left
offload approve|deny <run> [tool-use-id]    # …and the answer, from whichever device you are
                                            #   holding; it is forwarded to the agent's node.
                                            #   Both halves measured on two daemons in session
                                            #   seventy: the hook gets `permissionDecision`,
                                            #   the log records who said so, and a deny typed
                                            #   at bravo for a run on alpha is logged on both
                                            #   as `denied (an operator on bravo)`
offload explain <run>                       # …and a run *stopped* on one of those questions
                                            #   says so here too, with the clock and the
                                            #   command to answer it — the one way a run is
                                            #   `running` and making no progress by design.
                                            #   **From any node**, since session eighty-six:
                                            #   the holder reads its own registry, everybody
                                            #   else canvasses the fleet the way `offload
                                            #   asks` does. It used to be blank off the
                                            #   holder while `offload asks` on that same
                                            #   socket had the question, the clock and the
                                            #   `offload approve` line — which travels
offload run --max-turns 10 …                # stop it after ten turns however unfinished, counted
                                            #   across migrations; `ps` shows 4/10, and nothing
                                            #   — not the fleet, not `resume` — gives it an
                                            #   eleventh (ADR-0039). It keeps the conversation
                                            #   it was stopped in: the capture at the limit
                                            #   waits for the agent to write the turn, measured
                                            #   8/8 where it used to be 1/3 (session 32)
offload when <service> --max-turns 10 …     # …which matters more for a rule, firing unattended
                                            #   for months at an hour nobody is reading
offload ps                                  # …with a TOKENS column beside COST, read from the
                                            #   transcript — so it is there for the runs that
                                            #   never reached a result and have no cost at all
                                            #   (ADR-0040): 21.8k on a capped run, measured
offload run --task webhook --arg one        # the cheap tier: a program the owner nominated in
                                            #   `[[tasks]]`, asked for by *service* and never by
                                            #   command line, with no agent, no workspace and no
                                            #   tokens. Its stdout and stderr are the run's log.
                                            #   Walked on the **default** config — through the
                                            #   ordinary bid round, which is where it was
                                            #   refused outright until session sixty-seven —
                                            #   and on a daemon whose `agent.binary` points at
                                            #   nothing, which is phase 8's whole claim: it won
                                            #   the bid, ran, and refused an agent run a minute
                                            #   later with `ineligible: has agent claude-code
                                            #   (not installed)`
offload ps --all                            # …and a KIND column, because a table whose busiest
                                            #   column means a prompt on one row and a service
                                            #   on the next is the report shape this project
                                            #   keeps having to fix. `TURNS` is a dash for a
                                            #   task, not `0`; `offload explain` says `kind` and
                                            #   `work`; the closing log line and the
                                            #   notification say `finished` rather than
                                            #   `finished after 0 turn(s), $0.0000`
offload logs <task>                         # …and a task's output is served from the machine that
                                            #   ran it, asked from any node. `RunProgress::by`
                                            #   is what says which machine that is, and a task
                                            #   writes no numbers — so nothing stamped the leg
                                            #   and `offload logs` printed **nothing at all** on
                                            #   the node the task was submitted from: exit 0, no
                                            #   output, which reads as "it produced none".
                                            #   Measured on two daemons with the escalated
                                            #   *agent* run's log forwarding correctly beside it
                                            #   as the control
offload run --task check-api                # …and when it exits non-zero unattended, the fleet
                                            #   **runs it again** — up to `max_resumes`, at a
                                            #   fresh epoch, with the program's second run in
                                            #   the same log (ADR-0058). `Recovery::Resume` had
                                            #   one mechanism and it was `resume`, which refuses
                                            #   a task for want of a conversation, so the tick
                                            #   was refused in the same millisecond, counted the
                                            #   refusal as a spent attempt, and the program never
                                            #   ran twice. Measured on two daemons: `restarting
                                            #   it resume=1`, epoch 3, against a build whose log
                                            #   said a shell script had "no conversation to
                                            #   continue" while `offload explain` promised
                                            #   "picking it up again in 16.7s (try 1)".
                                            #   `offload resume` still refuses a task: that door
                                            #   is the one a person types at
offload every 15m --task webhook --arg one  # run something on a clock, fleet-wide (ADR-0019 §3,
                                            #   ADR-0056). Walked on two daemons: created on A,
                                            #   gossiped to B in seconds and shown there as
                                            #   `home nodeA · fired elsewhere`; exactly one node
                                            #   fired it; **A killed at 10:39:28, B was the
                                            #   steward at :34 and fired and ran the :40 tick**;
                                            #   and a *learned* schedule survived B's own
                                            #   restart, which is why it is written down rather
                                            #   than kept in gossip memory
offload schedules                           # …and who fires each one right now. On one daemon:
                                            #   the first tick lands on the boundary rather than
                                            #   at creation, two ticks make exactly two runs,
                                            #   the occurrence's twelve-character prefix *is*
                                            #   the tick (`01a08ad3ee20` = 1789035540000), and
                                            #   `offload unschedule` stops it across the next
                                            #   boundary while keeping the tombstone that
                                            #   travels
offload when failed --on-notice --task fix  # …and a rule may be fired by a **notice** instead
                                            #   of a trigger (ADR-0057), which is the demo's
                                            #   escalation: a run here fails, this fires. The
                                            #   delivery plane fires it, so it gets the outbox's
                                            #   cursor, dedup and retries for free. Walked on one
                                            #   daemon: a task exits 2, the rule fires **once**,
                                            #   the escalated occurrence exits 2 as well, and
                                            #   nothing fires again — a notice about
                                            #   machine-started work fires nothing
offload when <service> --task <task>        # …and a trigger can fire the cheap tier, which is
                                            #   phase 8's demo's first clause. Walked on one
                                            #   daemon with a nominated ticker and a nominated
                                            #   program: three firings, three occurrences, the
                                            #   `KIND` column in `offload rules`. **The event
                                            #   is not passed to a task** (ADR-0019 §1) — an
                                            #   agent rule appends it to the prompt under a
                                            #   heading saying it is data. A task rule for a
                                            #   service nobody nominates gets ADR-0036's note
                                            #   rather than a refusal, and the note's promise
                                            #   was checked: `offload rules` counts the refused
                                            #   firings under DROPPED with the round's reason
offload policy --config node.toml           # …and it answers for both tiers, so a device with
                                            #   no agent that nominates a task is no longer
                                            #   told it would accept no work at all — which was
                                            #   ADR-0019's own scenario, answered backwards
offload run --task webhook --demand light   # …and the owner's `[policy.light]` answer decides
                                            #   it (ADR-0019 §4). Walked on a daemon configured
                                            #   `accept = "never"` with `[policy.light] accept
                                            #   = "always"`: the light task ran, the same task
                                            #   at ordinary demand was refused one command
                                            #   later with `node is not accepting work`, and
                                            #   `offload status` says both answers
```

Two daemons on one machine, both with `[cluster]` in their config and no seeds, find each
other. `OFFLOAD_STATE_DIR` and a distinct `listen` port are all that separates them — which
is also how every three-node test in this document was run.

**The product claim is true of the code**: start a run on one machine, drain or kill that
machine, and the agent carries on somewhere else in the same conversation.

Phase 2 is done, both halves of the demo verified against a real agent:

- `offload checkpoint` mid-run → the agent finishes its turn, the run goes `Pending`,
  `offloadd` restarts without failing it, `offload resume` picks it up again.
- `kill -9` the daemon four turns in → `ps` says *resumable from turn 4*, and
  `offload resume --follow` continues **at turn 5 in the same conversation**, with the
  agent's four uncommitted files still in the worktree.

Checkpoints are taken at every turn boundary by default (`[checkpoint] every_turns`).

## Where the rest is

| | |
| --- | --- |
| Why something is the way it is, session by session | `docs/sessions.md` |
| What a completed phase actually shipped | `docs/phases.md` |
| Running two or three daemons on this machine | `docs/DEMO.md` |
| What is not built, and the open questions | `docs/ROADMAP.md` |
| The mistakes already made here, by subject | `docs/pitfalls/`, indexed from `CLAUDE.md` |
| A workload explored against the code, before designing for it | `docs/use-cases/` |

**Keeping this file useful.** When a session ends, put its paragraph at the *top* of
`docs/sessions.md` and replace the pick-up list above — do not append to this file. It is read at
the start of every session, so it earns its length in what it saves the next one; the previous
version reached 4,635 lines, and its own head had gone three ADRs stale because nobody could see
the top of it.
