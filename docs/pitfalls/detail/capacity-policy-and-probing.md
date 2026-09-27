# Capacity, work policy, capability and probing — full entries

The working rules are in `../capacity-policy-and-probing.md`, in this same order. These are the entries they
were compressed from: the mechanism, the measurement, and how each one was found. Read one
when the rule alone is not enough to act on.

- **A refusal has to name a clause wherever that clause sits in the list.** `failures()`
  descends only through unsatisfied branches, so a branch whose children all held — a `Not` —
  reports itself instead; the check for that asked whether *anything* had been collected rather
  than anything of its own, so a sibling that failed earlier silenced it. `All([Not(Always),
  MinCores(99)])` named both and the reverse named one. Nobody chooses that order: the list is
  assembled from a repo config, a `--constraint` string, or `agent_ready`.

- **A policy field nothing can set is worse than one that does not exist.** `WorkPolicy`'s
  `allowed_agents` was enforced by `admits` from the beginning, gossiped in the struct, and
  settable by no config field and no flag — so `Refusal::AgentNotAllowed` was a sentence no fleet
  could produce, while the field read as a control being applied. It is the one owner decision a
  device *class* cannot express, which is why it belongs to the same split ADR-0013 rests on:
  "this machine is for light work, not an expensive agent session" is a preference, not a missing
  capability. A name this build cannot spawn is refused at **deserialize** time — the same rule
  `Service::from_str` follows in the capability direction — because `work_policy` is called from
  the gossip tick and must not be fallible, and a fallible version's only safe fallback would be
  "the owner said nothing", which is *less* restrictive than what they wrote. An empty list is
  refused too: it reads as a restriction and means "refuse everything", which is `accept = "never"`
  spelled in a way nothing else understands.

- **A grant constrains placement only while it cannot be honoured remotely.** `Constraint::CanUse`
  was right for exactly as long as its own justification held; once a call could be proxied,
  keeping it pinned a run to the one device that cannot host it — the phone with the mailbox.
  What survives is the question the operator can still act on, asked at submission: does
  *anybody* have one. And a proxied server carries no `env`, which is the whole feature: it is
  named by service rather than by the holder's own id, because the caller does not know that id
  and has no business learning it.

- **Don't over-claim in the probe.** An unverifiable auth is `false`. A node that claims
  capability it lacks wins bids and then fails every run it takes — much worse than idling.

- **…and the probe has to be asked about the program that will actually run.** It asked `claude`
  on `PATH` while the supervisor spawns `agent.binary`, so the setting whose entire job is "where
  the agent is" decided what ran and nothing about what the device *claimed* — and the two
  disagree exactly when somebody uses it, which is why the default hid it. Both directions were
  live and both were measured on two daemons: an owner whose agent is at a path (a version pin, a
  wrapper, a phone) advertised a different program's version and authentication and would fail
  every run at spawn, and with nothing named `claude` on `PATH` a node whose configured agent works
  perfectly advertised none, never bid, and refused every submission — `offload status` saying
  `none installed` one directory away from an authenticated install. The binary is an argument now
  (`probe_with_agent`), and `deliver::capabilities` is the single place the daemon joins probed
  facts to nominated ones, because the config is a fact only `offload-node` holds. The reports
  followed: `offload probe` and `offload match` take `--config` as `offload policy` does, each says
  which program it asked about, and `offload status` names the binary where it used to say "none".

- **…and which *account* it runs as was a fact about whatever shell started the daemon.** The same
  sentence one field over, and the sharpest of the three, because the wrong answer here spends the
  wrong account's money. Claude Code's account is selected by its state directory, read from the
  environment in two independent copies, with no config field able to say otherwise — so `offloadd`
  from a terminal ran on one login and from a service manager on another, and every report said
  `agent claude-code 9.9.9, authenticated` either way. `[agent] config_dir` nominates it (ADR-0028)
  and wins outright rather than only when the variable is unset, because the answer must not depend
  on the environment of a service manager nobody reads. It has to reach **three** places, which ask
  different questions about one directory: the probe (authenticated, and as whom), the capture
  (where the transcript is), and the spawn (where the agent will look). Two of three is the failure
  that hides — a node authenticated as the right account, checkpointing nothing all night. Which is
  why `spawn` now **tells** the child: `transcript::config_dir` argued the opposite in as many words
  ("the agent is spawned as a child and inherits it, so the two cannot disagree"), which was true
  only while nothing else could set the path. Its test drives a real spawn, and watched red it
  reported the config directory of the shell running the suite. `[agent] account` is the guard half,
  reported as **not authenticated** rather than as a missing agent — the program is right there.

- **…and a nominated directory answers for its own account or not at all.** `account_fingerprint`
  looks for the agent's settings in two places — inside the config directory, and `~/.claude.json` —
  which is a sound hedge for a path this crate *guessed* at and a wrong answer for one somebody
  wrote down. Measured: a daemon pointed at a fresh `config_dir` reported the personal login, from
  a file two levels up, and would have gossiped and rate-limit-accounted itself as the account its
  owner deliberately did not choose. The fallback is skipped exactly where the directory was
  nominated.

- **The account's own usage limit was a signal nothing remembered.**
  `AgentEvent::RateLimit`'s doc comment says it "feeds per-account bid scoring; this is the real
  signal the probe's account fingerprint cannot provide" — and it fed nothing: `judge_rate_limit`
  used it to decide whether *that run* would miss its deadline and dropped it, so the next run was
  started into the same wall, holding a session slot on a node idle in every report. That is the
  "dead code that asserts a mechanism" entry with the polarity reversed. A node remembers a
  *blocking* status with a *stated* reset (ADR-0029), latest wins per kind because the agent
  restates the same limit every turn, and a block with no reset is not remembered at all — a fact
  with no expiry is the one that gets stuck. It gates **starting**, which "load gates accepting,
  never starting" appears to forbid and does not: that rule is about a number nobody controls,
  and this one lifts at an instant the agent stated. Being full, with a clock instead of a count.

- **…and `From<Capacity>` is how the gate came not to exist on the path most testing uses.** The
  rate limit was first passed into `Room` by each caller, and the fleet-of-one submit path hands a
  bare `Capacity` in — the `From` impl fills the field with `None`, silently, for a field the
  caller ought to decide. Measured: a node rate-limited for another 76 seconds started the next run
  immediately. `Supervisor::room` is the funnel now, and it is the right owner anyway: the limit is
  this node's own observation, so a caller supplying it was a caller repeating us. The general
  shape — **a `From` impl that defaults a decision is a forgotten call site with no compiler
  error.**

- **A capability the owner is the only one who can state still has to be settable.**
  `AgentDetails::max_concurrent` is a probe guess of 2, and it is a **cap**: `admits` refuses
  past it and the bid treats it as full. On a desktop, server or VM `WorkPolicy::max_concurrent_runs`
  defaults to **4**, so the third run was refused with "agent claude-code is at its per-node
  concurrency limit" — a per-node limit no config could express — while `offload status` printed
  `runs 2/4` and raising the owner's number did nothing at all. What bounds concurrent sessions is
  a plan and a rate limit, and nothing on the machine says which one this login has: so the owner
  nominates it (`[agent] max_concurrent`), which is the sink and resource rule applied to what an
  agent *is*, and it stays a capability rather than becoming policy. Three ceilings, and the
  lowest happens: what the install sustains, what the owner gives the machine
  (`max_concurrent_runs`), and what the account may run fleet-wide (`max_concurrent_account`). The
  status line says which one binds, because both numbers are real and printing only one is a node
  refusing work at 2 under a line that says 4.

- **Not every battery on a machine powers the machine.** A mouse, a keyboard, a touchscreen and
  a controller each register under `/sys/class/power_supply` with `type=Battery` and a
  `capacity`, and `power::detect` took whichever one `read_dir` yielded last — so the answer
  depended on filesystem order and could differ across boots on one machine. Measured here:
  `BAT0` at 98%, an ELAN touchscreen's phantom battery at 0%, and `offload probe` printing
  **0%**. Unplug that laptop and every battery floor in the fleet refuses it at 98% charge; the
  reverse (system battery low, mouse full) accepts work and dies mid-run, which is the
  over-claim this crate exists to prevent. The kernel has the discriminator — `scope=Device` on
  a peripheral, absent or `System` on the real one, plus `present=0` for an empty bay — and
  nothing asked it. `settle` is a pure function of the parsed directory for that reason: the bug
  was reachable only through `read_dir` order, which no test against a real `/sys` can arrange.
  Several system batteries report the **lowest**, which is the wrong arithmetic and the right
  direction. And a system battery whose capacity will not parse is `Unknown`, never `Ac` — `Ac`
  claims there is nothing to run out of.

- **Capacity gates how many agents run, not how many runs are held.** Two questions, two
  counts: a commitment fills a slot for the purpose of accepting *more* work (ADR-0006), and
  occupies nothing for the purpose of deciding what to start next. Answering the second with
  the first deadlocks a node that took accept-without-starting at its word — three runs granted
  to a one-slot machine leaves two commitments, each reading the other as the reason it cannot
  begin, leases renewing for ever and `ps` saying `assigned`.

- **A cap that gates accepting but not starting is decorative, and worse than absent.** Every
  node accepts work up to the ceiling and then starts straight through it, each one individually
  behaving. The per-account cap therefore gates both — unlike observed *pressure*, which gates
  accepting only, because what a run waits behind under pressure is the owner's build (undrainable
  from here, never promised) while what it waits behind at a ceiling is other runs in this fleet,
  which finish. The asymmetry is deliberate; collapsing it either way is a bug.

- **The run being started must never be counted against its own ceiling.** A held run is
  `Assigned` and therefore visible in the view, so counting it made every commitment the reason it
  could not begin: a real daemon ran one run, completed it, and left the second `assigned` for
  ever. The start side counts *started* agents excluding this one — the same held-versus-started
  split the machine's own capacity already insists on, which is what makes forgetting it here so
  easy.

- **A node must not ask the view about its own runs.** Gossip is a tick stale, so two submissions
  a second apart both started on a one-run ceiling; the node's own store knew exactly and was not
  asked. `AccountUse::elsewhere` is peers-only for this reason and the field name says so — the
  local number arrives from the caller, which is also the only way the two different local
  questions (held, versus started-excluding-this-one) can each be answered correctly.

- **The account fingerprint is a wire format, like the fleet key.** Same rule, quieter failure:
  it is only ever compared *across* nodes, so two nodes computing it differently do not disagree
  loudly — they simply never match, and per-account accounting silently stops. Versioned salt,
  pinned known-answer test. It hashes the account **uuid** and never the email beside it: a digest
  of a guessable string is reversible in practice however opaque it looks, and `AccountId` promises
  it is not. The API-key path hashes the key's *value* — the label it replaced was the name of the
  environment variable, so every key-authenticated node in the world reported one account.

- **Capacity belongs to the device, not to the fleet.** Two `offloadd` instances, one per fleet
  (ADR-0012), cannot see each other's runs — so a laptop configured for one run per fleet hosts
  two unless something counts for the machine. That something is the broker's ledger, and it has
  to reach the **bid** and not only admission: consulted only when accepting, a node offers to
  start now, wins, and holds the run on arrival — the right outcome by a route that broke a
  promise between two lines of output. An unreadable ledger is a loud warning and the old
  over-committing behaviour, never a daemon that refuses to start.

- **A budget limits what runs *beside* something, never what runs at all.** A `Heavy` run wants
  more shares than a phone's entire budget, so a lone run always fits — otherwise a number the
  owner picked for concurrency has become an eligibility rule, and the run is unplaceable
  across a fleet of small devices rather than merely slow on one. The same sentence from the
  other end: hardware a run genuinely needs is a `Constraint`, which fails loudly and names
  itself.

- **Load gates accepting, never starting.** A committed run has nowhere else to be and nothing
  frees the machine on its behalf, so refusing to *start* it until the load average improves
  holds it hostage to a number nobody controls. The way out of a commitment is
  `review_commitment` giving it back — offering it first, so the run is never left `Pending` in
  nobody's hands — and only for a deadline somebody *stated*, because with an unspecified one
  every held run is overdue within a second and the rule would bounce every commitment in the
  fleet from queue to queue.

- **Being full and being under pressure are two different sentences.** Full is this node's own
  queue: it accepted that work, knows the depth, and it empties by itself, so a full node
  *commits* (`Availability::WhenFree`). Under pressure is the owner's build on the same
  machine — never promised, undrainable from here — so it defers (`NoBid::Busy`) and only when
  the run's slack can afford the wait. Collapsing the two either strands runs behind work
  nobody is managing, or has a node promise about a machine it does not control.

- **An unreported load average is not an idle machine.** `LocalFacts::cpu_load_percent` is an
  `Option` for the probe's usual reason: a plausible zero wins bids this device should lose, and
  the rules that read it skip rather than assume.

- **Capability match is necessary, not sufficient.** Policy, capacity, battery, and rate
  limits all gate separately, and each refusal must stay distinguishable in the output.

- **A battery floor is about running out, so it never applies on mains.** `WorkPolicy` says
  so and `admits` honours it; a second copy of the rule that forgot had a plugged-in laptop
  reporting 0% refuse every checkpoint replica, so nothing was durable anywhere and the only
  symptom was a `WARN`. Degrading quietly is worse here than refusing loudly.

- **Scoring resources linearly drowns out every other signal.** Cores and memory are scored
  per *doubling*; agent runs are model-latency bound. A warm workspace beats a bigger box.

- **Agents cost money and rate limit.** Concurrency caps are per-node *and* per-account.
  Nodes sharing an account share one limit.

## `ready_elsewhere` reads ability, and hosting needs permission

`review_commitment` will not give a commitment back unless somewhere else could start it — that is
`KeepReason::NobodyElseCouldStartIt`, and it is what stops a fleet with nowhere to put the work
bouncing a run from queue to queue at a bid round each. The count comes from `mesh::ready_elsewhere`:

```rust
view.alive()
    .filter(|n| n.id != me)
    .filter(|n| run.spec.constraint.matches(&n.capabilities))
    .filter(|n| Capacity::of(&n.policy).room_for(view.occupancy(&n.id), run.spec.demand).is_ok())
    .count()
```

Three gates, and they are every gate `NodeView` carries: liveness, **capabilities**, **policy**.
Which is precisely the vocabulary's own split — capabilities are *ability, not permission*, and a
work policy is what the device's **owner** permits. The fleet's `host-runs` grant is a third thing,
it is what actually decides whether a node may run an agent at all, and the view has no field for
it.

So a member that joined and was never granted `host-runs` — the default, and deliberately so:
"joining a fleet is not permission to run agents on its repositories" — counts as ready for every
late commitment in the fleet. Measured, staging `GiveBack` with exactly that peer:

```
09:26:21.323  due and still not started here; offering it to the fleet  overdue_by=11.1s ready=1
09:26:21.324  nobody else would take it; keeping the commitment here    refusals=2
```

`ready=1` about a node that could never take it, and a round that could only ever be refused.

**Inside the stated tolerance, and not the failure the tolerance was written for.**
`review_commitment`'s docs say the count is an estimate read from state a tick old and that being
wrong costs a bid round. That anticipates a *stale* answer, which self-corrects on the next tick.
This one was permanent and knowable, so it repeated on the `may_retry` backoff for as long as the
run was late: a release, a refused round and a take-back — two epoch bumps — every thirty seconds,
all night, for a run that was never going anywhere.

## …and "the view has no field for it" was a claim about the view

The first write-up of the entry above ended here, concluding that it could not be fixed from the
reading end: the node that knows is the node itself, so the shape would be a self-reported "may
host" beside `policy`, owned by the node and arbitrated by its incarnation like every other
self-report (ADR-0005) — a new gossiped field, and therefore a decision rather than a patch.

That was wrong, and the thing that says so is one function further on in the file that already
enforces the rule. `Cluster::hosting_objection` is ADR-0012's other half — the handshake admits a
member, and this refuses the member's *bid* when the fleet never said it may host — and it works
by reading the certificate **the peer presented on the connection this node still holds**:

```rust
let membership = self.connections.lock().await
    .get(&peer).map(|session| session.peer.membership.clone());
```

Nothing is gossiped for that and nothing needs to be. The estimate was looking at the view because
the view is where its other three gates live, and the fourth gate was already local, in the same
file, twenty lines from the enforcement of it.

So the fix is `Cluster::peer_hosts_runs` beside `hosting_objection`, and **one** `permits_hosting`
predicate under both — an estimate and an enforcement that cannot disagree, which is the rule
`reports-and-cli` is largely about, applied before the two had a chance to drift. The reader is
side-effect-free on purpose: `hosting_objection` drops the connection when it objects, and an
estimate must not.

**Unknown is not `false` here.** `peer_hosts_runs` answers `None` where there is no live
connection, and `Mesh::peers_barred_from_hosting` counts such a peer as ready anyway — the
opposite of this codebase's usual "unknown is not good news". It is right for this one question
because `review_commitment` states which way to be wrong: "being wrong there costs a bid round;
being unwilling to estimate at all costs the run its night". That makes the change strictly
subtractive — it can only turn a doomed round into no round, never a possible handover into a run
left on a busy machine — which is also what made it safe to ship without an ADR.

Measured, same fleet and same run, on the fix:

```
08:01:02  keeping the commitment  reason=no other node could start it either
08:01:05  keeping the commitment  reason=no other node could start it either
   …      the run untouched at epoch 1, `assigned`, where before it flapped every 30s
```

and the regression that mattered more, with the peer granted `host-runs` and past probation:

```
08:03:18.825  due and still not started here; offering it to the fleet  overdue_by=2.5s ready=1
08:03:18.831  a late commitment was handed over  name=bravo  starting=starting now
08:03:18.858  bravo: spawning claude code
```

Six milliseconds, unchanged. A subtractive fix has exactly one way to be wrong, and it is to
subtract too much; that is the pass to walk, not the one that demonstrates the bug.

**Probation falls out of it for free**, which is the part nobody would have thought to build. A
node granted `host-runs` and still inside `PROBATION` has a certificate that says *granted but
dormant*, so `permits_hosting` is false and the estimate skips it — where before, every late
commitment in the fleet bought a doomed round for the whole fifteen minutes.


- **…and it has a sibling, which `offload policy` printed four lines away.**

`allowed_agents` was a policy field nothing could set. `metered_network` was a *capability* field
nothing could set, which is the same disease one axis over and was still live two ADRs later.

```rust
// Assumed until proven otherwise; a phone's daemon should override this from the
// platform's connectivity API, which is the only reliable source.
caps.metered_network = false;
```

That was the only write in the workspace outside a unit test. Four readers acted on it:
`WorkPolicy::admits` (so `Refusal::MeteredNetwork` was a sentence no fleet could produce),
`mesh::accepts_replica` (so the metered half of ADR-0016's durability check never fired),
`bid::evaluate`'s `metered_penalty`, and `Constraint::UnmeteredNetwork` — which meant
`offload match "unmetered"`, a documented command whose whole job is that question, said yes on
**every device there has ever been**.

Scope, stated precisely because the first draft of this entry did not: `offload run` has no
`--require` flag. Every submission is built with `Constraint::agent_ready(ClaudeCode, None)`, and
`constraint_expr::parse` has one caller, `offload match`. So the constraint reader was giving a
wrong answer to a *person*, not misplacing runs; the three that were load-bearing are the policy
refusal, the replica check and the bid penalty. It still mattered, because the tree is what
`bid::evaluate` reads and any future `--require` would have inherited an answer that was always
yes.

Measured, on the build before the fix:

```
$ offload match "unmetered"
match: this device satisfies the constraint
$ offload policy | grep metered
metered network   refused
$ offload probe | grep -i metered      # nothing at all
```

and after:

```
$ offload match "unmetered"
no match:
  - unmetered network (metered = unknown, and this run needs it known to be free)
$ offload run …                        # with `metered = "yes"` nominated
Error: no node will take this run
  fedora       network is metered and policy disallows it
```

The second is the first time the fleet has produced that refusal.

**Found while looking for something else**, which is how this class keeps being found: the probe's
own comment said "assumed", `offload policy` printed the guard as applied, and the two had been
sitting four lines apart in the same terminal output for months. The rule above was written about
`allowed_agents` and would have caught this one if anybody had run it against the field beside it.

**The report half.** `offload probe` printed nothing when the bool was false, so silence meant
both "this link is free" and "nobody asked" — and it was always the second. It has its own line
now and never a blank one. The first draft of that line said *"no answer from NetworkManager"*,
which was wrong on the machine it was written on: NM answered, with a guess. A report that names a
cause it did not establish is the same mistake the field itself had just stopped making, so it
says what is true of both cases — nothing here can tell a tether from plain wifi.

- **…and a node that says `accepting no` has to mean it at every door that starts an agent.**

Found while walking ADR-0045, decided in ADR-0046, and then found again one door over in
ADR-0047. The measurement that opened it: a daemon reporting

```
accepting   no — network is metered and policy disallows it
```

accepted a submission at its own socket and ran it to completion. `server::place`'s no-cluster arm
went straight to `Supervisor::submit`, which checks `Room::for_one_more` and never
`WorkPolicy` — so capacity bound a local submission and nothing else did: metered, the battery
floor, `NotCharging`, `accept = "never"`. Whether the owner's policy bound a submission typed at
this machine depended on whether the node happened to be in a fleet, which nobody decided.

**ADR-0046 settled it in the direction `AcceptWork::Never`'s own doc comment always stated** —
*control plane only… never host them* — and split `WorkPolicy::permits` out of `admits`: the four
clauses that do not empty by themselves (accept, battery floor, metered, `allowed_agents`), asked
before any question about the queue. That hoist is load-bearing, not tidy: a full node **commits**
to a run and says when (ADR-0006), so reporting a standing "never" as a "not yet" is what makes a
node promise work its owner forbade.

**ADR-0047 then measured the same thing on `offload resume`, which asked none of the three.**
Three stagings on one daemon:

```
accepting   no — node is not accepting work                       → resumed, turn 6
accepting   no — drained; restart offloadd to take work again     → resumed, turn 5
accepting   no — this node has been revoked from its fleet …      → resumed, turn 7
```

The third is the one that matters: ADR-0044 had halted that run seconds earlier and written *this
node was revoked from its fleet, so it stopped running it* onto it, `offload run` was refused with
the fleet's sentence, and `offload resume` restarted the agent on an evicted device. The failed
run's own footnote — `resumable from turn 16 with 'offload resume …'` — was pointing at the hole.

One `hosting_refusal(ctx)` now, called by both doors: draining, then the grant (where a revocation
turns a node away), then `permits`. `permits` rather than `admits` because the queue half is each
path's own and is answered better where it already is — `Room::for_one_more` sees the device ledger
and the account's rate limit too.

**The rule underneath, which is the reason this entry is long:** *a gate assembled one clause at a
time, on one door at a time, is the shape that hides.* The drain clause was added to the
submission arm when it was measured missing. The policy clause was added to the same arm two ADRs
later, when it was measured missing. The recovery tick got `departing` and `revoked` when *its*
version was measured missing, with a comment saying exactly why. Every one of those fixes was
correct, tested and written down, and not one of them asked **what else starts an agent** — which
was one match arm away in the same file. When a clause is found missing, the next question is not
"is it fixed" but "how many doors are there".

Still open, deliberately (ADR-0047): the recovery tick does not ask the owner's policy, and it
must not be fixed by putting `permits` inside `Supervisor::resume` — the tick counts a refusal as
a spent retry, so a battery dip would burn the run's budget. It wants a standing fact in
`Circumstances` beside `departing`, and `decide_recovery` has an exhaustive sweep to re-run.

- **…and a probed fact is only as good as the last probe.**

The entry above is about a gate that was not asked. This one is about the *fact* it asks for.

`main` built `Arc<Capabilities>` once, at startup, under a comment that had been true and had
stopped being so:

> Probe once at startup. Capabilities do change — battery drains, agents get upgraded — and phase
> 3 will re-probe on a schedule to keep gossip honest. **For a single node with no one to tell,
> once is enough.**

Phase 3 built precisely that and no more: a re-probe that called `Cluster::set_capabilities` and
nothing else, inside the gossip loop — the loop a fleet of one never enters. So the fleet's copy
moved and the daemon's own copy did not, and "no one to tell" stopped being true the day ADR-0046
and ADR-0047 gave the local doors something to decide with.

**How it was measured**, and the trick is reusable: `Command::new("nmcli")` is a `PATH` lookup, so
a two-line script that prints the contents of a file makes the link metered or free under a
running daemon, with nothing about the machine's real network touched. Thirteen seconds after the
flip the daemon logged `capabilities changed; gossiping them`. Then:

```
$ offload status
accepting   yes
$ offload run --repo … "new work?"
Error: no node will take this run
  solo         network is metered and policy disallows it
$ offload resume 01a053607c0a
resumed 01a053607c0a          → running, turn 4
```

One fact, three readers, two of them stale and staying that way. And on a fleet of one with **no**
cluster there was no re-probe at all — measured on the old build, sixty seconds after the flip:
`offload probe` said `metered — bytes cost money here`, `offload status` said `accepting yes`, and
the submission ran.

Two rules fall out.

**A snapshot is correct until somebody decides with it.** Printing a stale battery percentage is a
small lie; refusing or accepting work on one is not. The change that makes a read-only fact into a
gate is the change that has to ask how fresh the fact is — and neither of the two ADRs that did it
here, one after the other in one session, asked.

**A node's own state needs tending whether or not there is a fleet.** Third instance of the same
mistake: leases and recovery (`tend_own_runs`), the drain's flag (`Mesh::drain`), and now the
probe. The gossip loop is the wrong home for anything a lone laptop still needs.

The fix is one live handle, `deliver::Current`, with `now()` and `refresh()`; one writer, the
`reprobe` task, spawned with or without a cluster; and the fleet's copy still set from the same
probe in the same statement, so the two cannot disagree. **Callers take one `now()` per decision**
— a site that read the device class and the capabilities separately would answer with half of one
probe and half of the next, and the class is what selects the policy the capabilities are judged
against.

Left, deliberately: the cadence is thirty seconds and nothing probes on demand, because a probe
shells out to the agent binary and `nmcli` and that is why this was a snapshot in the first place.
So `offload probe` (fresh) can disagree with `offload status` (the daemon's last probe) for up to
a cadence, which is honest rather than a bug.

## A fallback for the platforms with no DMI was written with a `/sys` read

**The mechanism.** `detect_device_class` reads `/sys/class/dmi/id/chassis_type` first, which is
the reliable signal on PC hardware. Under it sat a fallback whose comment read *"No DMI
(containers, macOS, BSD): a battery means portable"* — and it called `power::has_battery()`, whose
body is `read_supplies().iter().any(...)` over `/sys/class/power_supply`. That directory does not
exist on macOS. So the fallback for macOS asked a Linux-only question and got `false` every time,
and **every Mac classified as `DeviceClass::Unknown`**.

**Why half of it was invisible.** `Unknown` and `Laptop` both map to `Stability::Transient`
(`capability.rs:614`), so a MacBook got the answer a laptop should get, by accident. Only the
mains-only machines show the fault: a Mac mini, Studio, Pro or iMac is a desktop and was scored
`Transient`, which is a *lower* bid than the hardware deserves. Stability affects scoring and
never eligibility, so nothing failed — it just bid wrong, quietly, for as long as macOS has been
in the `Os` enum.

**The fix, and why it needed no new probe.** `power::detect()` already answers on both platforms:
`detect_linux` reads `/sys` and `detect_macos` shells out to `pmset -g batt`. And `settle`'s own
arms define the vocabulary — `None if mains_online => Some(PowerSource::Ac)`, under the comment
*"Mains present with no battery: a desktop or server"*. So the class is decided from
`PowerSource`, which meant hoisting the power probe above the class in `probe_with` (the class
depends on it, so computing it first also expresses the dependency and avoids shelling out to
`pmset` twice). `has_battery` was then called by nothing and is gone; the peripheral-filter test
that named it now names the arm that replaced it.

**The asymmetry is deliberate and is the interesting part.** `class_from_power` claims `Desktop`
only on macOS. `Ac` there is a positive answer from `pmset` about a lineup where "no battery"
means one of four desktops. On Linux, reaching this function at all means the DMI read failed,
which is a container, a single-board machine or WSL — and `Desktop` carries `Stability::Stable`,
which is the one claim this crate exists not to make on a guess. A node that over-claims wins
bids and then fails every run it takes. So Linux with `Ac` and no DMI stays `Unknown`, and
`PowerSource::Unknown` — a battery whose level will not read — stays `Unknown` on both, because
knowing there *is* a battery is not enough to say the machine is portable.

**How it was found.** Not by a walk and not by a test: by working out what a Mac mini would report
before plugging one in, for a runbook. Reading the fallback's comment against the body of the
function it called was the whole of it. The lesson generalises past this instance — **a
cross-platform fallback has to be reached by a cross-platform question**, and the comment naming
three platforms was the tell that nobody had checked which of them the call could answer for.

The tests are the pure `class_from_power`, split out for that purpose: the DMI read is a file a
test cannot be given, and every interesting case here is a platform the machine running the suite
is not.

- **A refusal written for "nothing can construct this yet" outlives the yet.** `bid::evaluate`
  opened with `let Work::Agent(work) = &run.spec.work else { return Err(NoBid::UnsupportedWork
  {..}) }`, and the comment above it was honest about why: the `Work` split (ADR-0019 §2) landed
  a session before anything that could execute a task, and a task that could be placed but not
  run would be worse than one that could not be submitted. Every word of that was true when it
  was written.

  What happened next is the part worth carrying. The same session that split `RunSpec` also
  built `[[tasks]]`, `Request::SubmitTask`, `--task` on the CLI and `drive_task` in the
  supervisor — and *none* of those touches `evaluate`, so nothing failed to compile and no test
  went red. The tier had a submission path, a config, a capability, an execution path, and one
  door left shut. `server::submit_task_run`'s own comment pointed straight at it — *"With a
  cluster this is the bid round's job and `Constraint::HasService` already does it"* — which was
  a statement about the design and not about the code.

  **The measurement.** One daemon, the default `[cluster]` (`enabled = true`), a `[[tasks]]`
  entry for `webhook`, and `offload run --task webhook --arg hello --arg world`:

  ```
  Error: no node will take this run
    walker       this node cannot host task work
    (use --queue to leave it pending anyway)
  ```

  The same config with `enabled = false` ran the program and completed the run. That is the
  whole diagnosis: the fleet-of-one arm has no round, so it never reached the refusal, and the
  cluster arm reached nothing else.

  **How it was found.** By walking the item the roadmap called small — `offload ps` wants a kind
  column — which needs a task on screen to look at. The first submission of the session produced
  the refusal above. Nothing about the code was read first; the previous session's own walk had
  been real, and its config had the cluster off, which is the mode `ClusterConfig`'s doc comment
  calls "the mode most of this project's testing happens in".

  The variant is **deleted** rather than kept: nothing constructs it, and `NoBid` never crosses
  the wire — only its `to_string()` does — so a refusal this binary cannot give is not a refusal
  a peer can receive either. `SubmitError::UnsupportedWork` stays, because three paths below
  `start_run` genuinely are agent-only and a task reaches none of them.

- **A gate takes a whole `Tier`, not the agent it used to assume.** `WorkPolicy::permits` and
  `admits` took `&AgentKind`, and every caller with no agent in hand had to invent one:
  `owner_policy_refusal`, `status` and the recovery tick all passed `AgentKind::ClaudeCode` as a
  stand-in, which was harmless while `allowed_agents` was the only clause that read it and
  `ClaudeCode` was the only agent anybody had.

  `AgentKind::Other(String)` is what makes it not harmless. A config may name `codex`, and then
  `permits` refuses `ClaudeCode` — so the fleet-of-one **task** door, which calls the same
  function, would refuse a shell script with `agent claude-code is not allowed here`. Reachable,
  not theoretical, and invisible on any machine whose config does not set the field.

  `Tier::{Agent(&AgentKind), Task}` lives in `policy.rs` and is constructed by `Work::tier`, so
  the mapping from work to gate is made in one place. **An enum rather than
  `Option<&AgentKind>`** for a reason this file has already paid for once: `allowed_agents` was
  an owner's control defeated by an ordering (the entry above, `admits` asking it after
  capacity), and a `None` that silently skips three of the owner's clauses is one an agent-run
  caller can reach from a `RunSpec::agent()` that happened to be empty. `Tier::Task` has to be
  named, so the compiler asks for the claim.

  The half that is easy to get wrong in the other direction: `accept`, `min_battery_percent`,
  `allow_metered` and the demand budget are facts about the *device*, and they apply to both
  tiers unchanged. A phone told to work only while charging does not run a shell script on
  battery. A tier that skipped those would be a way around the owner's own answer — which is
  exactly what ADR-0019 §4 refuses about letting a submitter's `--demand light` bypass a gate.
  Tested both ways: `phone.permits(Tier::Task)` is `Err(NotCharging)`, and
  `permits(Tier::Task)` under `allowed_agents = ["something-else"]` is `Ok`.

- **`max_archive_bytes`: a ceiling that may only tighten, and what that costs to get right.**

  ADR-0061 §4's knob, built in session seventy-eight. Four things about it are worth keeping,
  because none is specific to archives.

  **It is refused when it loosens, never clamped.** The structural limit is `MAX_BLOB_BYTES` and no
  config can raise it, so `policy.max_archive_bytes = 999999999` is a startup error:

  ```
  invalid config: policy.max_archive_bytes is 999999999, which is above the 536870912 bytes this
  fleet can move in one exchange — it tightens that limit and cannot raise it, so there is no
  setting here that would make a larger archive travel
  ```

  Clamping is the tempting alternative and is worse: an owner who wrote a bigger number believes
  their fleet moves that much, and a silent clamp leaves them believing it until a run fails
  somewhere else entirely. The last clause matters as much as the numbers — *there is no setting
  here that would* stops somebody hunting for the one that would.

  **`None` means the existing limit, not the absence of one.** The ADR said the opposite until the
  blob plane was measured, and the argument it gave — a limit invented for a case nobody has hit is
  the knob-nobody-understands this project refuses to ship — was sound and pointed at the wrong
  ceiling. The reusable question before adding any `Option` ceiling: **what is already enforcing
  one?**

  **Two limits, one reported number.** `offload status` prints the *effective* one — the fleet's,
  tightened by the owner's — because one number is what an agent packs to. Reporting both and
  leaving the reader to work out which binds is the shape this tree keeps deleting.

  **The refusals stay distinct.** `NoBid::ArchiveTooLarge` ends *"another node may still take it"*,
  because it is one owner's answer; the structural cap is refused before a run exists and says the
  archive has to be smaller. Measured on a daemon with the ceiling at 100 KB:

  ```
  alpha   its workspace archive is 307200 bytes and this node's owner takes at most 102400
          — another node may still take it
  ```

  Collapsing the two would be `NoBid::RepoUnavailable`'s own defect repeated, which session
  seventy-five had to unpick.

  And a fifth, on the spec side: `WorkspaceSpec::archive_bytes` is `Option`, absent for anything
  that did not know to state it — **a `None` size is not a zero size**, and refusing on absence
  would turn a missing field into a policy decision. The node finds out at acquisition, which is
  slower and correct. The size is measured by the submitting node from the blob already in its own
  store rather than taken from the caller, which is the same posture as hashing the bytes instead
  of believing the digest.

- **A clause that asks whether somebody *nominated* a program is not the one that asks whether they
  have it.** `Supervisor::build_task` wrote `Constraint::HasService { service, role: Some(Execute) }`
  and said so in its own doc comment, which was quoting ADR-0019 §1: *"`Constraint::HasService`
  already asks the question that matters at the keyboard: does **anybody** have this task."* Written
  as design ahead of code, and wrong by one word — `HasService` asks whether anybody *nominated*
  one, and a `[[tasks]]` block is three fields of which one is a path.

  Found by staging the thing rather than reading it. One daemon, default config, `cluster.enabled =
  true` on loopback, a `[[tasks]]` entry pointing at `/tmp/ow81/nightly-report.sh` which does not
  exist:

  ```
  $ offload run --task nightly
  run 01a0942db7e8
  follow with: offload logs -f 01a0942db7e8

  $ offload ps --all
  01a0942db7e8   failed   task   -  -  -   nightly
             └─ agent: the program for task `nightly` was not found: /tmp/ow81/nightly-report.sh
  ```

  Accepted, placed here, failed in a millisecond — then picked up again by the recovery tick
  (`recovery  picking it up again in 21.4s (try 1)`, epoch climbing) until the budget went. The
  control, `--task neverheardof`, was refused correctly at the keyboard. So the door was not
  missing; it was asking the weaker of two questions it had both of.

  The bit was already there and already right: `task_capabilities` sets `Capability::authenticated`
  from `which(&cfg.command)` on **every probe**, and its own doc comment explains why an unusable
  task is advertised rather than dropped. `Constraint::ServiceAuthenticated` — a variant that
  existed in the enum and that nothing in the tree constructed — is the clause that reads it. The
  tell that this was an oversight rather than a decision is one ADR back: `Constraint::CanUse`, the
  **resource** tier's clause, is `is_resource() && authenticated`. Two applications of one rule and
  the newer one dropped the bit.

  No wire bump: `ServiceAuthenticated` is an existing variant of an existing enum, so a peer that
  never heard of the change deserialises the spec and evaluates it correctly. What does change for
  an existing **schedule** is nothing — its `RunSpec` was stored when it was created and keeps the
  old clause, which is the honest behaviour and worth knowing before reading a stale row as a bug.

- **…and `authenticated` is one bit with a different meaning per role.** For a sink it is that the
  credential works; under `Role::Execute` it is that the nominated program is on the device. Both
  halves of `explain` used the credential word:

  ```
  fedora   ineligible: has nightly as execute, authenticated (task:nightly (execute, unauthenticated))
  ```

  Two sentences on one line sending somebody to look for a login for a shell script. The label is
  role-aware now (`has nightly as execute, with its program there`) and so is the observation
  (`task:nightly (execute, no program there)`).

  The **first** cut of that label was `runs nightly, program present`, which reads as a finding
  rather than as a requirement — and it is printed directly after the word `ineligible:`, so the
  line stated the opposite of what had been measured. Caught by re-running the walk after the fix,
  which is session eighty's lesson 5 doing its job: a fix creates new sentences and they can be
  false too.

- **The probe checked a file was there and not that it could be run.** `resource::which` ended in
  `.find(|candidate| candidate.is_file())`. Staged with one `chmod`: a `[[tasks]]` entry naming a
  file with mode `644`.

  ```
  $ offload status | grep noexec
  task        noexec (noexec, /tmp/ow81/noexec.sh)  ·  a program that is there and cannot be run

  $ offload run --task noexec
  run 01a0944296d1
  $ offload ps --all
  01a0944296d1   failed   task   -  -  -   noexec
             └─ task: starting task `noexec`: Permission denied (os error 13)
  ```

  The same over-claim as a missing program, one permission bit in, and with nothing on any screen
  to tell the two apart — `offload status` called it fine on the line above. `runnable` asks
  `mode() & 0o111` under `#[cfg(unix)]`, with the other arm keeping the old answer: the bit is a
  unix concept and this daemon's control socket is a unix socket, so the `cfg` is honest rather
  than defensive, and it makes the omission visible on the day that changes.

- **…and there were two resolvers, which is why only one of them knew anything.** `deliver::resolve`
  and `resource::which` were the same function written twice, and each had learned something the
  other had not: `resolve` treated any command containing `/` as a path (right — looking for
  `bin/watch` inside every `PATH` entry finds nothing and then says "not on this device" about a
  file that is right there), and neither asked about the execute bit. `resolve` delegates now.
  Nobody would have found this by reading either one; it showed up because the exec-bit fix had to
  be written twice.

- **One field carried two different things depending on who set it.** `Capability::description` is
  set in five places. Four of them — `task_capabilities`, `trigger_capabilities`,
  `resource_capabilities`, `sink_capabilities` — put the **owner's one-line label** in it, with a
  comment beside each saying the command must never travel. The fifth,
  `hold_the_agent_to_one_account`, puts a **reason** in it (*"logged in as X, and this node is
  configured for Y"*). And `offload-probe`'s report prints it as a `└─` clause under a `NOT
  authenticated` line, with its own comment calling that clause *"…and **why**, for anything this
  device says it cannot use"*.

  So, measured with all four nominated kinds broken on one daemon:

  ```
  service     resource:helper — NOT authenticated
              └─ a helper a run may be granted
  service     sink:phone — NOT authenticated
              └─ the owner's phone
  service     task:nightly — NOT authenticated
              └─ posts the nightly report to slack
  service     trigger:ticker — NOT authenticated
              └─ notices when something happens
  ```

  Four clauses that read as explanations and are not. The reason existed at each site — every one
  of the four `tracing::warn!`s beside them names it — and went to a log that `offload probe
  --config` cannot reach, since that command runs with no daemon at all. `Capability::unusable`
  keeps both facts, in one order, in one place: label, then reason. The reason gossips with the
  rest of the capability, so it says what the device cannot do and never how — a peer still learns
  that this machine has a push route and never what runs it.

  And the consequence for `offload status`: its `task` and `resource` lines take `description`
  from the **capability** rather than from the config, so the sentence that states the cause is
  the one the measurement wrote. The `bool` beside it says `— NOT usable` and nothing more. It had
  been asserting `its program was not found` from that `bool`, which is right for one of two
  causes and is the entry above.

- **A report has to print the spelling its own config accepts.** `offload policy` renders the
  owner's standing answer with `{:?}`:

  ```
  accept work       WhenCharging
  ```

  and a `node.toml` saying `accept = "WhenCharging"` does not start:

  ```
  Error: invalid config: TOML parse error at line 10, column 10
  unknown variant `WhenCharging`, expected one of `never`, `when_charging`, `always`
  ```

  The refusal is a good one — it names all three — so the cost is a detour rather than a dead end,
  and that is why this is a small entry rather than a large one. It is still the command whose
  whole job is to say what the policy in force *is*, printing it in a form its own input rejects,
  and an operator cannot grep their config for what they were shown.

  `AcceptWork` has a `Display` giving the serde spelling, and the test asserts it against
  `serde_json::to_string` rather than against three literals, so renaming a variant or changing
  `rename_all` cannot leave the two disagreeing again.

  The sweep behind it is worth recording, because it came out mostly clean: every other `{:?}` in
  operator-facing output round-trips. `offload probe` prints `Laptop / Linux / X86_64
  (Transient)`, and `offload match`'s parser is case-insensitive, so `class=Laptop`, `arch=X86_64`
  and `stability>=Transient` all parse — typed, not assumed. `offload nodes` lowercases its status
  column at the print site. Device class is probed and not configurable, so it has no round trip
  to fail.

- **…and "it parses" was the wrong bar for the rest.** The entry above checked that the other `{:?}`
  renderings round-trip through `offload match`'s case-insensitive parser and left them. Session
  ninety read them on a refusal: `--require "node=bravo, os=macos, class=phone"` printed `os ==
  MacOs (have Linux); device class == Phone (have Laptop)` — parseable, and not what anybody typed —
  and `Constraint::OnMains` rendered its observation as `have {:?}` of `PowerSource`, which is `have
  Battery { percent: 57, charging: false }`: a struct literal, the one clause a phone fails most, and
  nothing parses it. `Os`, `Arch`, `DeviceClass`, `Stability` and `PowerSource` have a `Display` in
  the grammar's spelling now (`battery at 57%, not charging`), used by every clause's label and
  `have`; `offload policy`'s `device class` line and `offload status`'s `(laptop)` use it too, and
  `capability::tests::power_reads_as_words` asserts `DeviceClass` and `Stability` against serde's
  own rendering, which is what a config spells.
- **A heading that claims the owner said something must be true of every line under it.**
  `LightWork` is three independent overrides and `None` inherits, which ADR-0019 §4 states in as
  many words: *"a fleet that says nothing behaves as it always has."* The report then printed all
  three under one heading:

  ```
  light work        the owner has said what this device does with it:
    accept work     Always
    battery floor   40%
    metered network refused
  ```

  Measured against a `[policy.light]` block containing `accept = "always"` and nothing else. The
  `40%` is the **main** policy's floor; it changes when that one changes, without the light block
  being touched, and somebody reading this has been told they set a light-work floor they did not
  set.

  What is right about the existing code is where the values come from, and the fix does not touch
  it: the comment beside it says *"printed through the same `*_for` resolvers the gate calls, never
  by re-reading `policy.light`"*, which is this file's own rule. The heading was a different claim
  — about **authorship**, not about value — and answering it needs `policy.light`'s `Option`s,
  which is a fact about whether the owner stated something rather than about what the answer is.
  So the values still come from the resolvers and the marker comes from the options:

  ```
  light work        what this device does with it:
    accept work     always
    battery floor   40%            (inherited)
    metered network refused        (inherited)
  ```

  The marker is in a column, because three values of different widths put it in three places
  otherwise — `when_charging` is the widest there is.

- **The agent's own ceiling bound the bid and stopped nothing.** Two functions ask "is there room
  for one more", and only one of them knows about the agent install's own limit.

  `WorkPolicy::admits` has four clauses — the owner's standing doors, the machine's capacity, the
  **agent's** ceiling, and the account's:

  ```rust
  if let Some(agent) = tier.agent() {
      match caps.agent_details(agent) {
          Some(a) if load.held_for_agent >= a.max_concurrent => {
              return Err(Refusal::AgentAtCapacity(agent.clone()));
  ```

  `Room::for_one_more` has three, and the agent's is not among them: `capacity.room_for`, the
  account's cap (only when `Room::account` is `Some`), and the rate limit. And `admits` has
  exactly one caller — `bid::evaluate`.

  So the ceiling shaped what a node **bid** (`Availability::WhenFree { behind }`, which is where
  *"starting when one of the 2 runs ahead of it finishes"* comes from) and bound nothing at the
  moment an agent was spawned. `Refusal::AgentAtCapacity` could be computed, was rendered by
  `summarise_refusal` as `agent is full`, and could never stop a run.

  Measured on a fleet of one with `cluster.enabled = false` — the configuration `ClusterConfig`'s
  own doc calls "the mode most of this project's testing happens in", and the one where `admits`
  is never reached at all:

  ```
  $ offload status
  runs        0/3  ·  claude-code sustains 2, which is what binds
  $ offload run … ; offload run … ; offload run …
  run 01a0951f7f46
  run 01a0951f833c
  run 01a0951f872e
  $ offload status
  runs        3/3  ·  claude-code sustains 2, which is what binds
  $ pgrep -c -x agent.sh
  3
  ```

  Three agent processes for an install the node itself says sustains two, with every submission
  accepted without a word. `deliver::set_agent_concurrency`'s own doc comment calls that number
  *"a cap"*, and `offload status` calls it *what binds*.

  On the fix, the same staging:

  ```
  submit 3: Error: node cannot take it: agent claude-code is at its per-node concurrency limit
  runs        2/3  ·  claude-code sustains 2, which is what binds
  $ pgrep -c -x agent.sh
  2
  ```

  and on two daemons, where a grant holds rather than refuses: two running, the rest `assigned —
  waiting for a slot`, draining one at a time as each finishes.

  **Four doors, which is the count that keeps being the lesson.** `Supervisor::agent_full` is
  asked by `submit_built` (the tail of every submission, and the whole of placement with no mesh),
  by `start_refusal` (the tick and the grant — `take_run` had a *second copy* of the capacity
  arithmetic and now calls `start_refusal`, which is how a clause could be added to one start path
  and missed by the other), and by `resume`, which a person types at and the recovery tick uses
  unasked. `restart_task` is the fourth caller of that shape and needs nothing: a task names no
  agent, so `agent_full` answers `None` by construction.

  The **count** each door passes is its own, deliberately: a submission asks what this node has
  *committed* (`held_for_agent`), a start asks what is *going* (`started_excluding_on_agent`).
  That is the same split `Room::for_one_more`'s callers already draw for the machine's own
  capacity, and passing the count in rather than computing it inside is what keeps the ceiling
  from becoming a third opinion about which runs to count.

  And the supervisor had to be **told what the device is**: the ceiling is a capability and it
  held none. `Arc<deliver::Current>` is wired at startup, the same shape as `Supervisor::room`
  overlaying the rate limit it alone observes, and for the same stated reason — a caller supplying
  it would be a caller repeating the node. `None` is tests and nothing else, written down at the
  call site because it is the line that goes quiet if another construction path appears.

*The six entries below were backfilled in session ninety-one from `docs/sessions.md` and the
commits that added each rule (`b4b1a37`, `32a93ef`, `0fcc290`, `e0b4798`) — sourced, not
reconstructed from the rule text.*

- **`Unknown` is not resolved once, at the type.**

  Session forty-six (commit `b4b1a37`, ADR-0045). With `Metered { Yes, No, Unknown }` in place, the
  four readers disagree on purpose. A policy refusal claims the bytes cost money; a constraint
  claims they do not; `Unknown` proves neither, so it supports neither — the burden of proof lies
  with whoever makes the claim. Refusing work on `Unknown` would stop every machine without
  NetworkManager from hosting runs or holding replicas, a certain cost paid to avoid a possible one.
  *Unknown is not good news* means unknown must not be quietly converted into whichever answer the
  caller found convenient — not that it is always the pessimistic one.

- **A guess the platform labels as a guess is not an answer.**

  Same session. Where the owner said nothing, NetworkManager is asked, and its `(guessed)` suffix
  is kept rather than cast away: `no (guessed)` becomes `Unknown`, because NM guesses *no* for every
  wifi and ethernet link, including a tethered one — the first case the field's own doc comment
  names. Promoting the guess would have reimplemented the bug with a nicer type. Before the fix,
  `offload match "unmetered"` said the device satisfied it, `offload probe` printed no network line,
  and `offload policy` said `metered network refused`.

- **…and a field that has no meaning for this kind of work must not be read as though it did.**

  Session sixty-seven (commit `32a93ef`), the bid round evaluating a task. Three of `evaluate`'s
  checks are about the agent half — the repository, the account's rate limit, the checkpoint's
  agent version — and two were actively wrong if hoisted: `LocalFacts::repo_available` is computed
  for a task too (`can_obtain("")`) and comes out `false`, so one `if` in the wrong place refuses
  every task on every node for a repository nobody named; and `account_limited_until` is an agent
  account's rate limit, so a node blocked until 09:00 would have reported `NotBefore` for a shell
  script it could run that second.

- **A gate that can answer two ways makes every report of it a claim about one of them.**

  Session sixty-seven, ADR-0019 §4 (commit `0fcc290`). A daemon with `accept = "never"` and
  `[policy.light] accept = "always"` ran `--task webhook --demand light`, refused the same task at
  ordinary demand one command later — right — and then `offload status` said `accepting no — node
  is not accepting work` about a machine that had just done work. Both sentences were true and the
  pair was the answer, so `NodeStatus::light_refusal` travels beside the first.

- **…and the demand has to reach every door, which means finding out what each door is about.**

  Same commit. `permits` gained a `Demand` beside its `Tier`; the thinking was in the callers. Where
  a run is in hand its own demand is the question — `submit`, `submit_task`, `resume`, `explain` —
  and where there is none the honest question is about an ordinary run, now a named constant. Two
  had to be looked up rather than guessed: the recovery tick passed **one** `Hosting` for a whole
  pass over a list that can hold a light watcher beside an expensive session, so it takes a closure
  and asks per run; and `ready_elsewhere` was left alone, because its own docs say which way to be
  wrong and a peer's gossiped battery is a second old.

- **…and the first fix returned an `Option`, which threw the measurement away.**

  Session eighty-one (commit `e0b4798`). `which` asked `is_file()`, so a program present and **not
  executable** was advertised, won its bid, and failed the spawn with `Permission denied (os error
  13)`. The first fix returned an `Option`, so every report then said *its program was not found*
  about a file sitting right there — six sentences introduced by the fix, caught by re-running the
  walk. `Program::{Runnable, NotFound, NotExecutable}` is the third two-valued answer to a
  three-valued question this tree has unpicked, after `can_obtain` and `steward_of(..) == Some(me)`.
  The tell: the fix is written at the *decision*, and the decision only needs to know whether.

- **`disk_free_mb` changed on every probe, and every change is an incarnation.** Found on the first
  two-machine walk, from a line that said `what this device is has changed` every thirty seconds and
  not what. The log line now names the fields (`Capabilities::differences`, destructured so that a
  new field will not compile until it is named), and it said `changed=disk_free_mb`: 55 times in
  forty minutes on an idle laptop. Each one bumped the incarnation in `set_capabilities` and
  re-gossiped the whole capability set, which is why the laptop's refutation later read
  `incarnation=44` about a node nobody had suspected 44 times. `offload probe` run three times in
  thirty seconds showed the same number each time, which is what made it look like something else:
  the disk moves in bursts, and a quiet half-minute is not evidence. Rounded down to a whole GiB,
  since `MinDiskFreeMb` is the only reader and a node that claims space it lacks takes a run and
  fails it; under-claiming by less than a GiB costs at most a bid. Zero changes in the next 24
  minutes, with CPU burners and a full `cargo test` writing to the same disk. (The disk it measures
  is the one under the process's *working directory*, while the doc comment says worktrees live
  under the state dir. Not changed here, and noted in the handoff.)
- **The load row of ADR-0063 §2's table could not happen.** It priced a busy node at −30. The walk
  on the laptop and the Mac found the first attempt void, because a background property run held
  the laptop at 100 % and the canvass said `cpu at 100%: … the machine is not`. That is a refusal,
  not a low bid. `admits` asks `idle < demand.needs_idle_percent()` before any score is computed,
  so for a normal run a bidder is at most 75 % loaded and its load term at most −22 (Light −27,
  Heavy −15). Measured at 64–66 %: the term was −19, and `offload run`'s note said `load +15` of a
  difference of 5 between the two machines. The rule generalises to every weight table. What a term
  can contribute is bounded by whichever gate runs first, and a number on paper that ignores the
  gate states a range nobody will see.
- **The probe advertised the root filesystem's space for a state dir on another disk.**
  `free_disk_mb` picked the mount holding `std::env::current_dir()`, under a comment that said
  worktrees live under the state dir and that this was therefore the number that constrains a
  run. The comment named the right directory and the code asked a different one, so a daemon
  started from `/` by a service manager, or from a home directory, reported whatever that
  filesystem had. Measured with `offload probe --config` against a state dir on the 210 MB `/boot`:
  HEAD said `266240 MB free disk`, the root's, and the fix says `0 MB` (210 rounded down to whole
  GiB). `/tmp` and `/dev/shm` were the first two tries and proved nothing: `/tmp` is the same btrfs
  as `/home` here, and sysinfo does not list tmpfs at all, so `/dev/shm` falls through to `/`. That
  is a blind spot the doc comment now states. It is the agent-binary rule met again — ask about the
  thing that will actually be used — and was found by reading the function while fixing its
  rounding, not by any report.

- **The emulator said the battery was readable, and the phone said otherwise.** Walked the same
  evening: on the SDK emulator `adb shell` read `/sys/class/power_supply/battery/capacity`, and the
  battery row was measured through it. On a Samsung phone under Termux the same path was
  `Permission denied`. An app is not `shell`, and Android 16's SELinux policy gives apps no
  `sysfs` power nodes. The probe's `detect_linux` saw the directory exist, could not list it,
  settled on nothing, and the phone reported `power unknown — treated as not on mains`. With the
  phone's default policy that is a refusal of all work, plugged in or not, and it was worded
  `node accepts work only while charging`, which sent its owner to the cable. Two changes. The
  refusal is now `ChargingUnknown` ("…and cannot tell whether it is"), read before and after on
  the phone itself. And the probe asks Android through Termux:API (`termux-battery-status`), with
  a 5 s kill because the command waits for ever when the Termux:API *app* is missing. With it
  installed the phone read `on mains, battery 87%`, took a task on its default policy and ran it.
  The general point cost an evening's walk to see: the emulator's shell has rights an app does
  not, so an emulator answers "what can this platform tell a privileged shell", which is not the
  question.

- **The Android app reported `cpu 0%` on a phone that was not idle.** The first logcat from the app on
  the SDK emulator carried `tokio-rt-worker: avc: denied { read } for name="loadavg"`, and the app's
  `offload status` said `cpu 0%`. `load.rs`'s own doc comment names the hazard ("a node reporting 0 %
  because nobody asked the kernel is claiming to be idle") and guarded only `Os::Windows`, where
  sysinfo is known to answer 0.0. On Linux the same answer arrives when the read is *denied*, and
  under an Android app's SELinux domain it always is. Now the file is read first, and a denial is
  `None`, printed `cpu not reported`, on both the emulator and the phone. The unit tests cannot deny
  `/proc` on the laptop, so the walk is the test. The detail worth keeping is that
  the emulator's `adb shell` could read `/proc/loadavg`, like `/sys/class/power_supply`, and only the
  app's domain showed the problem. **Measure a platform fact from the sandbox the product runs in.**

- **A host's facts go stale because the phone is asleep.** Found while wiring ADR-0078, before any
  walk. The freshness rule (`HOST_FACTS_FRESH`, 120 s) exists so that a host that stopped writing
  stops being believed about its battery. The quiet decision asks a different question: "is this
  phone set down?" And the one state in which the writer reliably stops is the phone being
  suspended, which is the state being asked about. With the fresh-only reader, a phone asleep for
  more than two minutes would read as "no facts, not quiet". It would bump its incarnation back to
  the ordinary rate on the first packet that woke it, then back to quiet 15 s later when the writer
  ran: the flapping the detector work was meant to end. Fixed by reading the last facts at any age
  for this question only, and by the app writing the moment the screen changes, so "screen off"
  never outlives the screen being off. The app dying takes the daemon with it (the service stops
  its child), so a file left saying "quiet" has nobody to mislead.
