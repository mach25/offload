# Roadmap

Each phase ends at something demonstrable. The demo is the acceptance criterion — if you
can't run it, the phase isn't done.

Phases 1 and 2 are deliberately ordered so that **something useful exists before any
distributed systems work pays off**: phase 2's single-node agent runner is worth using on its
own. If the mesh half turns out to be harder than expected, that is the fallback.

**`docs/phases.md` holds what each completed phase shipped**, item by item with its ADR, the demo
that closed it, and what was learned building it. This file is the live view: what is *not* built,
and the questions still to answer.

## Where each phase stands

| Phase | | |
| --- | --- | --- |
| 0 — Foundations | **done**, 11/11 | domain model, workspace, no networking, no agents |
| 1 — Run one agent locally | **done**, 10/10 | `offload-agent`, `offload-workspace`, one machine |
| 2 — Checkpoint and resume | **done**, 8/8 | migration mechanics before the network |
| 3 — Two nodes see each other | **done**, 12/12 | `offload-transport`, `offload-cluster` |
| 4 — Runs move between machines | **done**, 29/29 | built in `offload-cluster::place`, not `offload-sched` |
| 5 — Off the LAN | 12/14, **demo run** | the demo walked on a Samsung phone in session ninety-two, off the LAN over mobile data; the items below still open are relays and hole punching, and the mobile host process (half done: Android built, the iOS app not started) |
| 6 — Hardening | 40/41 | only metrics left, deliberately |
| 7 — Nice to have | not scheduled | recorded so they don't get invented mid-phase |
| 8 — Work that is not an agent run | **built**, 7/7 | the `Work` split; the cheap tier the fleet exists to schedule. Demo run on two daemons (session sixty-eight); in ninety-two the **whole sentence over a real network**: the Samsung phone on mobile data, off the LAN, as the node with no agent — schedule, trigger, a person's failing task, its escalation placed on the laptop, the phone's own sink. **Done on real machines** |
| 9 — A workspace that is not a repository | **6.5/7** | ADR-0061, accepted session seventy-eight. built but for §6's fourth refusal, which is deliberately unbuilt: a directory that is not a repository now runs, migrates, and says what travelled |
| 10 — Work you dispatch and come back to | **done**, 14/14 | ADR-0063 and ADR-0064, built session eighty-nine (wire v30; v31 in ninety); §2's weights walk run in ninety-two, on the laptop, the Mac and an emulated phone |

**7 is a parking lot, not a position in time.** It is where things go that nobody has scheduled,
and taking an item out of it is the mechanism working rather than a renumbering. Phase 8 is
numbered 8 only because six accepted ADRs already point at "phase 7" and bookkeeping is not worth
editing settled decisions for; **phase 9 is the next scheduled work**, and it is listed after 8
below for the same reason — the file is ordered by when a phase was written down, not by number.

A phase ends at its demo. Those for 0–4 have been run end to end and phase 6's is `storm.rs`;
**phase 5's has not** — its delivery-plane half is verified, and the outside-the-LAN half is the
target restated below. `docs/HANDOFF.md` carries the list that gets re-verified, `docs/DEMO.md`
runs the daemons.

## Not built

### Phase 5 — Off the LAN

- [ ] Relay + hole punching for NAT'd devices. ADR-0015 defers the `iroh`-versus-build-it
      choice to here and writes down what decides it: whether the fleet needs to leave the
      LAN at all, whose relay it would use (no third-party relay by default), whether iroh's
      pre-release `ed25519-dalek` has settled, and how much of it we would otherwise write.
- [~] Android/iOS host process for `offloadd` — background execution, doze constraints. **Android v1
      built** (ADR-0066, session ninety-two): an app hosting the same daemon under a foreground
      service, installed with adb and driven with `run-as`. Doze is measured as latency, not
      deaths. **iOS v1 built** (ADR-0070, session ninety-two): the daemon linked into an app as a
      static library (`offload-ios`, `ios/`), walked in the Simulator only. It meshes, submits and
      observes, and on a device it hosts nothing, because the sandbox forbids spawning. A real
      iPhone (signing), release signing and anything past "enough to walk" are not started.
- [x] Mobile capability probing: battery, thermal state, metered network, from platform APIs —
      **done for Android** (ADR-0066 and ADR-0068, session ninety-two): all three from the app's
      platform calls, walked on the phone. The iOS app writes the same file (`UIDevice`,
      `NWPathMonitor`, `thermalState`), walked in the Simulator, which has no battery to read.
      *Originally:*
      **The metered half is half done** (ADR-0045, session forty-six): the capability is
      three-valued rather than a `bool` the probe hardcoded to `false`, an owner nominates it with
      `metered = "yes"` and NetworkManager answers where it can — which fixed a control that read
      as applied and could not fire. The platform half is **done for Android** (session ninety-two): the app writes
      `BatteryManager` and `ConnectivityManager` answers to `host-facts.json`, and the phone read
      `on mains` and `unmetered` from them. Thermal state is untouched.
- [x] **Approvers in hardware** (ADR-0012; **ADR-0069, built and walked on the tablet**, session ninety-two; this line said *unbuilt* until ninety-four, while the ADR's own amendments recorded §4 and hardware re-approval as walked).
      Reshaped by the owner: renewal is a widely held, renew-only authority (every member, by
      default), approval is a person's act and lasts a year, and a hardware-backed approval key
      (P-256, StrongBox or TEE via the Android app) is an *option of any approver*, not the
      phone's role: `Grant::Approve` and its delegation exist and do
      real work — they are what issues an invitation and what renews a certificate without the
      passphrase. What is left is the platform half: the approver key in
      secure hardware where there is any — non-extractable, biometric-gated, and probed
      honestly (`hardware_backed` is `false` unless verified). Approval is a signed artifact
      peers check offline, never a live call to the phone. This is the phone's best job in
      the fleet and the reason to enrol one before it can host anything.

**Demo** — walked on a Samsung phone through the Android app (session ninety-two), every clause but
the first: on battery it declines with a legible reason and still submits and observes runs; plugged
in on wifi it wins a bid and runs the work (a task, since no Claude Code build exists for Android).
**And it joined from outside the LAN on mobile data** once the owner added one inbound IPv6 rule on
the router: phone and laptop both have global IPv6, the phone dials out, and no relay or NAT
traversal was involved (ADR-0037's 2026-09-26 amendment). **The demo has run**, and what the phase
still lists is iOS, relays for fleets without IPv6, thermal state and approvers in hardware. The
original sentence: phone on mobile data joins from outside the LAN. On battery it declines
work with a legible reason but still submits and observes runs. Plugged in on Wi-Fi it wins a bid
for a review run and executes it. (The delivery-plane half of phase 5 is done and verified — see
`docs/phases.md`.)

**The defect below the phase is fixed, and the setting on the Mac is no longer in the way** —
session ninety-two meshed the two over v4 and v6 and ran agent, archive and task work across them
(ADR-0037's second amendment). What is left between them is an unreliable LAN, not code. The history: Session
sixty-five found that a Linux daemon and a macOS daemon do not mesh — the platform axis having been
untested by construction, since every multi-node test in this project's history was daemons on one
Linux box. Session sixty-six resolved it as two faults: `QuicTransport::accept` did the whole
inbound handshake inline in its own loop, so one stalled dialer shut the door on every later peer
(fixed); and **macOS 26 will not let an ad-hoc-signed binary send to the LAN**, which needs Local
Network access granted at that machine's console — System Settings → Privacy & Security → Local
Network — and no code. A **stable** code signature is what makes such a grant survive a rebuild,
which is the one new thing this demo has to arrange. The record is in `docs/DEMO.md`.
**Linux↔Android has run on an emulator** (session ninety-two, ADR-0037's last amendment): it met,
stayed up and ran tasks. It has **not** run on a phone, so assume nothing about Termux, mobile data or Doze.

### Phase 6 — Hardening

- [ ] Metrics and a Prometheus endpoint. **Deliberately still open, and worth arguing before
      building**: nobody scrapes their phone, and a counter saying three epochs were rejected today
      answers none of the questions this project's users ask. The audit log above was the half of
      this item with a stated purpose; the metrics half needs one.

### Phase 9 — A workspace that is not a repository

**ADR-0061, accepted; §1 and §2 built in session seventy-eight.** What this phase closes was, until
then, the only thing in this file reachable in one command and refused by every node: `offload run
--repo <any directory that is not a git repository>`. It is now submittable as
`offload run --archive <tar>`. The motivating case is the ordinary one — `~/client-project`, 6.5 GB, root not a
repo, one nested repo whose `.git` is 4.5 MB against a 3.6 GB tree — so the fleet cannot be pointed
at the work its owner actually has.

Read the ADR before any of this. Its §4 and §6 were amended in session seventy-eight after the blob
plane was **measured**: `MAX_BLOB_BYTES` is 512 MiB, a transfer resumes from nothing, and a blob
costs its whole size in RAM at both ends. The owner resolved the fork that opened — **the archive
fits the cap** — so what travels is a *subset the agent chose*, never a snapshot of the directory.

In dependency order:

- [x] **Say the cap before the expensive step** (§4, session seventy-eight). Both halves. An
      over-cap archive is refused at `StoreArchive` — before the run exists, before any bytes are
      stored, naming both numbers — **and** `offload status` prints the limit, so an agent
      budgeting a 6.5 GB directory reads it before packing anything. The two are the same constant,
      walked with the cap lowered to check they agree. The number was briefly hardcoded in
      `--archive`'s help and is not any more: nothing ever re-measures a `--help`.
- [x] **A workspace source that is *these bytes*** (§1) — content-addressed, carried on the
      existing blob plane, hash-verified. No new transport and no new trust decision.
      `RepoSource::Archive`, spelled `archive:<digest>` in the run spec's `repo` string so every
      existing reader keeps working; `offload run --archive <tar>` stores it and submits against
      the digest the daemon computed.
- [x] **`git init` on the receiver when the archive is not already a repository** (§2). The
      load-bearing clause, and it held: `ensure_archive_mirror` leaves an **ordinary bare mirror**,
      so nothing downstream learns about archives at all. Walked end to end — a directory refused
      one command earlier ran, committed on its run branch, and left the submitter's directory
      without a `.git`.
- [x] **Acquire once per node per run, not per turn** (§3, session seventy-eight). The cadence half
      was true by construction — a checkpoint is transcript, bundle and patch — so the work was the
      walk, and two daemons found two defects one could not. **`replicated` was a false promise**: a
      peer held every checkpoint blob and not the archive, so a run whose SAFE column said it would
      survive failed at turn 12 when its holder was killed. And once that was fixed, **the
      synthetic history was not content-addressed** — `git init` embeds the moment it ran, so two
      nodes built different base commits from identical bytes and the receiver answered `fatal:
      invalid reference`. Both fixed; walked end to end, killed on alpha at turn 5 and running on
      bravo at turn 13 on one unbroken branch.
- [x] **`max_archive_bytes: Option<u64>` on `WorkPolicy`** (§4, session seventy-eight) — a
      *tightening* knob only. `None` means the structural cap and no opinion beyond it; a larger
      value is a **config error** naming both numbers rather than a silent clamp. `offload status`
      reports the *effective* limit (the fleet's, tightened by the owner's), because one number is
      what an agent packs to. Carries `WorkspaceSpec::archive_bytes` so a node can bid against the
      size instead of discovering it by fetching (§5) — no wire bump: the `archive:` spelling is
      its own gate, since a node too old to know the form reads it as a missing path and declines.
- [~] **Four refusals that were one sentence** (§6): not here, here and not a repository, larger
      than **this node** will take, larger than **any** node will take. **Three of four exist** —
      the two repo ones since session seventy-five, and `NoBid::ArchiveTooLarge` with the knob
      above, which ends *"another node may still take it"* so it cannot be read as the fleet's
      answer. What is left is the structural one as a *`NoBid`*: today it is refused earlier, at
      `StoreArchive`, so a run carrying an over-cap archive never reaches a bid round — which is
      better, and leaves the variant unbuilt rather than missing.
- [x] **An acquisition report naming what travelled** (session seventy-eight). Promoted from a
      nicety by the amendment: a subset's failure mode is a run dying for want of a file that
      exists on the operator's disk, and a cloned repository needs no such line because its ref
      says what is in it. `LogKind::WorkspaceAcquired` in the **run's** log — a count and the
      top-level entries, not every path — and written only when bytes actually moved, so a warm
      node cannot claim an acquisition that did not happen. Walked against an agent that stops for
      a file the archive deliberately left out: two lines apart, the log says the selection was
      wrong rather than the machinery.

Open, and not blocking the above: **whether results ever return to the submitter's directory**. The
ADR is one-way deliberately, and the amendment sharpens the question rather than answering it — an
operator who sent 12 MB of a 6.5 GB directory is *more* likely to want the result in place.

### Phase 8 — Work that is not an agent run

**Not optional, and the reason is in `docs/ARCHITECTURE.md`'s graduated-cost section.** A fleet that
can only run agents costs money while it sleeps. The trigger tier is built and the agent tier is
built; this phase is the cheap middle. Before it, no run of any kind could avoid naming a
repository, so every poller had to arrive dressed as an agent run against a repo it never opens.
**All seven items are done and the tier executes end to end**: measured on one daemon, a node
with no agent installed won a bid for a task, ran it, and refused an agent run in the same
minute with a legible reason.

ADR-0019 is accepted and holds the design. Its smaller half — `Role::Trigger` — is **built**
(ADR-0020, session twenty-four).

- [x] **The `RunSpec` split** (session sixty-six): shared fields plus a `Work` enum,
      `Work::Agent(AgentWork)` beside `Work::Task(TaskWork)` (§2). Each variant holds a named
      struct rather than inline fields — the ADR's decision with ergonomics it did not specify,
      since the agent half is eight fields that travel together everywhere below `offload-core`.
      `RunSpec::check` refuses `Restartability::Resumable` for a task at submission. Wire v27,
      and `RunSpec` gained a hand-written `Deserialize` that accepts the pre-split flat shape —
      **not politeness**: a whole `Run` is one JSON blob in `runs.run_json`, so without it a
      node that upgrades loses every run on disk. Measured, not just asserted: a pre-split
      binary wrote a row, the new one read it, and both shapes now decode from one database.
      `Work::Task` was **a type and nothing more** when this shipped — nothing constructed one
      outside tests, and every path that would execute one refused with a reason. Those refusals
      are now doors that open: `NoBid::UnsupportedWork` is **deleted**, nothing constructing it
      any more, and `SubmitError::UnsupportedWork` still guards the three agent-only paths below
      `start_run` that a task reaches none of: `prepare_start`, `resume` and `drive`.
- [x] **An agent-agnostic supervisor** (§2, session sixty-six). `start_run` has **one** dispatch
      point: everything above it is the same for both tiers, because it is about a *run* rather
      than about what the run does. `drive_task` is `drive` with what a task does not have taken
      out — no workspace hold, no `Start`, no turns, no checkpoint — and with the **fencing
      unchanged**, because a nominated program can send mail or restart a service and two of
      those at once is the harm double execution always was. `TaskHandle` is deliberately *not*
      behind a trait shared with the agent's: an agent handle carries a session, a transcript
      and a turn counter, and a trait wide enough for both would be a type saying half its
      methods might not mean anything. What the two share is how the supervisor treats them.
- [x] **`[[tasks]]` in node config, addressed by service** (§1, session sixty-six) — the same
      shape sinks and resources have, advertised as `Role::Execute`, with the command never
      leaving the node. `offload run --task <service>` submits one; `Request::SubmitTask` is its
      own request rather than optional fields on the agent one.
- [x] **The bid round evaluates a task** (session sixty-seven) — **not on this list before, and
      it was the door the whole tier was still behind.** `bid::evaluate` refused every
      `Work::Task` at its first line, a refusal written when nothing could construct one, so the
      cheap tier ran only where `cluster.enabled = false` and was refused at the keyboard on
      every default node. Measured before and after on one daemon. Three checks are asked of
      the agent half alone — the repository, the account's rate limit, the checkpoint's agent
      version — and everything else applies to both tiers unchanged. `WorkPolicy::permits` and
      `admits` take a `Tier`, which is what stops `allowed_agents`, an agent's ceiling and an
      account's from being asked about a shell script.
- [x] **`offload ps` has a *kind* column** (session sixty-seven), plus the reports beside it that
      were making claims about work they had never seen: `TURNS` is a dash rather than `0` for a
      task, `offload explain` says `kind` and `work` where it said `prompt`, a task's closing log
      line and its **notification** say `finished` rather than `finished after 0 turn(s),
      $0.0000`, and `offload policy` no longer tells an agent-less device that nominates a task
      that it would accept no work at all — which was ADR-0019's own scenario answered wrongly.
- [x] **Schedules that fire idempotent occurrences** (§3, session sixty-seven, **ADR-0056**).
      `offload every 15m --task webhook` writes a standing instruction on a clock; it is
      **gossiped**, so it outlives the device that created it, which is the whole difference
      between this and `cron`. The occurrence's `RunId` is derived from `(schedule, tick)` —
      bytes 0..6 are the tick's unix milliseconds, so the twelve displayed characters are still
      the clock, and two nodes firing one tick converge on one record. Walked on two daemons:
      the schedule gossiped in seconds, exactly one node fired it, killing the home node made
      the survivor the steward, and a learned schedule survived that node's own restart.
      ADR-0056 settles the four things §3 left open — an interval with a UTC offset rather than
      cron, what says a tick has been served, a whole `RunSpec` so either tier can be
      scheduled, and a tombstone for removal. Wire v29, schema v12.
- [x] **The owner's gates gain the demand axis** (§4, session sixty-seven). `[policy.light]`
      gives `accept`, `min_battery_percent` and `allow_metered` a light form, every field an
      override whose `None` inherits — so **a config that says nothing behaves exactly as it
      did**, which is the `budget_shares` precedent and matters more here because these are
      gates people have already configured. `permits` takes the demand beside the tier, and the
      three resolvers (`accept_for`, `battery_floor_for`, `allows_metered_for`) are what both
      the gate and `offload policy` read, so the report cannot work it out for itself. A stated
      floor of `0` is how an owner says *any charge will do*, since `None` is spoken for.
      **Wire v28**: a `WorkPolicy` is gossiped inside a `NodeView` and a `NodeView` is relayed,
      and the light form can tighten a door as well as loosen one — so a peer that decoded it
      away could report a node as willing to do work its owner refused. Walked on a daemon
      configured `accept = "never"` with `[policy.light] accept = "always"`: it ran
      `--task webhook --demand light` and refused the same task at ordinary demand one command
      later. The walk also found `offload status` saying `accepting no` on that device, one
      command after it had run a task — a line that describes a machine has to describe what
      the machine does, so the owner's second answer now travels beside the first
      (`NodeStatus::light_refusal`, said only when it differs).

**Demo, the one thing left in this phase:** a node with **no agent installed** hosts a task. A
trigger fires it, it runs, it reports through a sink, and it spends no tokens. When the task exits
non-zero, a rule submits an agent run that the fleet places on a machine that *does* have an
agent. Three tiers and the escalation between them, end to end, on two machines.

**Most of it is walked, on one machine.** A node with `agent.binary` pointing at nothing wins a
bid for a task, runs it, and refuses an agent run in the same minute; a **schedule** fires the
cheap tier on a clock across two daemons; and a **trigger fires a task** — `offload when
<service> --task <task>`, measured on a daemon with a nominated ticker and a nominated program,
three firings and three occurrences. A task rule for a service nobody nominates gets ADR-0036's
*note* rather than a refusal, and the note's promise was checked: `offload rules` counted the
refused firings under DROPPED with the round's own reason.

**And the escalation is built** (ADR-0057): `offload when failed --on-notice …` binds a rule to a
notice instead of to a trigger's service, and the *delivery plane* fires it — so the cursor, the
`(sink, topic, seq)` dedup, the ordering and the retries are the ones this project already
measured into shape, and there is no second scan over the event log. A notice about a run a
**machine** started fires nothing, which is what stops an escalation escalating itself. Walked on
one daemon: a task exits 2, the rule fires exactly once, the escalated occurrence exits 2 as well,
and nothing fires again.

**The whole sentence has now been run at once** (session sixty-eight), on two daemons on one
machine: alpha with `agent.binary` pointing at nothing and both `[[tasks]]` entries plus a
`[[triggers]]` ticker and a `[[sinks]]` push route, bravo with a fake agent and no tasks. A task
submitted **from bravo** was accepted by alpha; the schedule fired the cheap tier on the
boundary; the ticker fired a task; a person's failing task fired the escalation and each
escalated agent run was placed on **bravo**, the machine with an agent (`trigger fired a run
rule=… trigger=failed node="bravo"`); and the sink on alpha reported each failure. The staging is
in `docs/DEMO.md`.

It found two defects, both reports and both invisible from the node that did the work — which is
the argument for submitting from the other one. `offload logs` printed **nothing at all** for a
task on the node it was submitted from, and `offload explain` promised a retry the daemon could
not make (**ADR-0058**). Both fixed and re-walked.

What is left of the demo is **two machines** rather than two daemons, and that is the Mac leg —
a console click rather than code (phase 5 above). What it would add is the network, which
sessions sixty-five and sixty-six already measured; what the placement does is proven here.

### Phase 10 — Work you dispatch and come back to

**ADR-0063 and ADR-0064, both accepted in session eighty-eight at the owner's request.** Two halves
of one want, stated by the owner against Claude Code's Remote Control: dispatch a session from
anywhere, have it land where it should — here, the Mac, the machine they will be at later — and
come back to it with a fresh context rather than a full one. Both change `RunSpec`, so this phase is
**one wire bump**, not two: build the fields together. Read `storage-and-encoding` first (a
`RunSpec` change is a change to what a node reads back from its own disk).

ADR-0063 — a preference is scored, never filtered:

- [x] `RunSpec.prefer: Constraint`, scored by the bidder about itself, never an eligibility pass
      (§1); `BidWeights::preferred` = 60 as a constant, not configuration (§2).
- [x] `Constraint::Node(NodeId)`, spelled `here` and `node=<name>`, resolved at submission —
      unknown and ambiguous names refused, an unmeetable preference said out loud (§3).
- [x] `--hold <duration>` (a duration, like `--deadline` — the CLI has no timezone): the preference is a requirement until a *stated* time; implies `--queue`;
      refused past a stated deadline (§4).
- [x] `BidWeights::resource_local` = 20 per granted resource the bidder holds (§5).
- [x] `--require` on `offload run` and `offload when`; `offload match` parses `here` and `node=`
      (§6).
- [x] `explain`, `offload run` and `offload ps` report from the `Score` and the hold function the
      decision read (§7).
- [x] **Walk §2's table** — load and warmth on the laptop against the Mac, battery on an emulated
      phone (session ninety-two). *Originally:* load and warmth walked, battery not (laptop against
      the Mac mini). Six arms, every one on the arithmetic: a preference beats one reason and yields
      to two. Two numbers amended in the ADR. The load term tops out at −22 for a normal run,
      because a busier node declines rather than bidding low, and a 15.5 GiB Linux machine scores a
      memory doubling short of "16 GiB". The battery row, measured on the SDK emulator with its battery set from the console: −20 at 50 %,
      as the table says. On a default phone the term is always zero, because a phone accepts only
      while charging, and its ceiling is the floor (−24 phone, −32 laptop).

ADR-0064 — a finished run is continued by a new run:

- [x] `result: Option<String>` on `LogKind::Finished`, capped, `#[serde(default)]` — the closing
      message is dropped today (amendment, check 2).
- [x] `RunSpec.parent` and `offload continue <run>`: `Completed` and `Cancelled` only; `Failed`
      refused naming `offload resume`; tasks refused (§1).
- [x] The base built read-only on the parent's node — typed on **any** node since session ninety (wire v31, ADR-0064's third amendment) — bundle and patch of branch *and* worktree —
      and started from as a checkpoint is (§2).
- [x] `handoff` by default: the prompt heading, and the parent's transcript as a blob named by the
      spec, materialised at `.offload/parent/transcript.jsonl`, excluded through the mirror's
      `info/exclude`, kept by `referenced_blobs` (§3).
- [x] `--session`: fork the parent's conversation; refused, never downgraded, when no transcript
      can be found (§3).
- [x] Inheritance: spec yes; origin, deadline, hold and budgets no (§4). `LogKind::Continued` with
      what was handed over, loudly when the closing message was empty (§6).
- [x] **Demo** — walked session eighty-nine with Haiku, $0.13: dispatch a run from one node, continue it on another with `handoff`, and read in
      the continuation's own log that it opened the parent's transcript only when it needed to.
- [x] *(Residual, closed session ninety.)* The agent's auto-memory is keyed per repository on a
      node, measured as a leak between runs; off for every spawn (ADR-0065).

### Phase 7 — Nice to have

Not scheduled. Recorded so they don't get invented mid-phase. Ten more phase-7 items have been
built; they are listed in `docs/phases.md`.

- Run DAGs: fan out a refactor across repos, gate on review
- **Reaching the fleet from outside the LAN** — decided (ADR-0037), mostly unbuilt. In order:
  measure IPv6 (**home half measured**, session ninety-two: routable, no NAT66 — and the default
  `listen` was v4-only and could not use it, now `[::]` dual-stack; the phone's half is one
  `ip -6 addr` on mobile data), then one reachable end (the laptop, forwarded, with the phone always dialling out),
  then a relay the owner hosts, then iroh for punching. **§2's two build items are done** (session
  twenty-nine): a seed may be a **name**, resolved on every attempt, and the list is **re-dialled**
  while there is no live peer — where before a name was refused outright and the list was dialled
  once at startup, so the self-healing existed for multicast and not for the internet. Measured
  before: one attempt, given up after 30s, and two daemons that never met. The walk then found what
  the re-dial exposed — **an impolite death healed and a polite departure was permanent** — now
  fixed. Everything the laptop needs to *say* already leaves through a sink, so nothing needs the
  phone to be reachable. `offload-relay` is a separate non-member binary that forwards ciphertext
  and verifies registrations by node key alone: no certificate, no fleet key, and therefore none of
  the trust a VPS joined as a member would hold. Not a cloud function — it needs a long-lived UDP
  socket.
- **A node that does no work** (ADR-0038, unbuilt): the rendezvous half, as an ordinary member
  rather than a service — one binary, one enrolment, one trust model, and it shows up in `offload
  nodes`. Being a member is *not* enough to be dialable, because ADR-0015 §1 keeps addresses inside
  the transport: what it adds is **introduction**, a third `Resolver` source behind mDNS and the
  seeds. Cheap because an address is routing and not authority — a hostile introduction wastes a
  dial and can never impersonate, since the handshake still demands a fleet-signed certificate and
  proof of the key. Its cost is the run plane: a member reads every prompt the fleet gossips, so
  `offload invite --introducer` mints the first certificate that carries **less** than the door
  grants, and the paths serving run content withhold from it.
- Additional agent adapters
- **Work that is not an agent run** — **moved to phase 8**, and it is no longer optional. It was
  filed here as a nice-to-have from the day ADR-0019 was accepted; it is the tier the fleet's
  economics rest on. See phase 8 above.
- ~~Cost and token accounting per run, per account~~ — **built** (ADR-0040, session thirty-one).
  Tokens are read from the transcript, deduplicated by `message.id`, and shown in `offload ps`
  beside the cost; dollars stay the agent's alone and stay absent when it did not say, which is
  every run that ends without a `result` — checkpointed, drained, cancelled, or stopped by its
  turn limit. What is **not** done, and was never this item: per-*account* accounting, and any
  notion of a budget that acts on a number rather than reporting it.
- Sandboxing (containers/namespaces per run)
- Web dashboard
- Human-in-the-loop approvals routed to whichever device you're holding

## Open questions

Answer these before the phase that needs them, not now. The five that have been answered, with
their reasoning, are in `docs/phases.md`.

5. **Does the same repo tolerate two concurrent runs?** Separate worktrees make it mechanically
   possible; whether it's a good idea is a policy question, and the answer affects whether
   `workspace` is a schedulable resource with exclusion. (Blocks phase 4.)

6. **What work is actually worth doing on a phone?** Review, triage, planning and doc runs
   need no build toolchain and are model-latency bound, so they fit. The open part is whether
   that is a run *profile* users pick, or something the constraint system already expresses
   well enough. (Blocks phase 5.)

7. **Are bid weights cluster config or node preference?** Two nodes running different
   `BidWeights` bid in different currencies, so scores stop being comparable. Leaning:
   cluster-wide config, gossiped, with a version. **Does not block anything yet**, and session
   twenty checked why: `BidWeights::default()` is the only constructor in the workspace, so
   nothing can configure them and two nodes cannot disagree. The answer is a gossiped struct with
   a version, and building one for a knob nobody can turn is machinery ahead of its use — so the
   day somebody adds a config field is the day to answer this, and the field should not be added
   without doing so. What the check found next door was the opposite problem: `bid_delay` and the
   two weights that fed it were the remains of ADR-0006 step 3, replaced by that ADR's own
   amendment before it ever shipped, and describing the superseded protocol in the present tense.

9. **Should a healthy run move to a better node that just freed up?** Placement happens once,
   at bid time, and a run then stays put until it finishes, is checkpointed, or its holder
   goes away. So a run that landed on the laptop because nothing better was free stays on the
   laptop even after the desktop finishes its build. Note this is *not* the "can the desktop
   take a queued job" case — pending runs live in the fleet-wide view and any node may bid on
   them the moment it frees capacity (ADR-0006 step 1, ADR-0013's re-bid trigger); that
   already works by design. This question is only about runs already in flight. Moving one
   costs a checkpoint, a transfer and at most a turn, so it is worth it only for a large
   improvement over a long remaining runtime — which nobody can measure, since remaining
   runtime is exactly what an agent run cannot estimate. If it is ever built it needs: turn
   boundaries only (ADR-0004), a minimum residency and a large score delta to prevent the
   thrash ADR-0007 exists to avoid, deadline slack to pay for the lost turn, and the holder's
   consent — ADR-0006 step 6 pointed the other way. Leaning: not worth it; the fleet gets
   most of the benefit from bidding well the first time. (Blocks nothing.)
