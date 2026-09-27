# ADR-0020: A trigger is a program the owner nominated; what it starts is a rule they wrote

**Status:** accepted · 2026-08-25 · builds the half ADR-0019 names as "the one to build if only
one gets built" · supersedes nothing

## Context

`Role::Trigger` — "something arrives and creates or wakes work" — has been in the vocabulary
since ADR-0011 and used by nothing. ADR-0019 considered it as an alternative to non-agent work,
rejected the framing, and kept the mechanism:

> **Triggers only, and no non-agent work at all.** … Genuinely attractive, and it is *half* of
> the right answer rather than a rejected one. … **If only one of the two gets built, build this
> one** — it is smaller, it changes no existing type, and it serves the case where the fleet does
> have somewhere to do real work.

That sentence is a decision about *which*, not about *what*. Four questions are open, and each
has a wrong answer that this project has already paid for once somewhere else.

1. **How does an event arrive?** A cadence in config means a scheduler inside the orchestrator.
2. **Who owns the binding between a trigger and the work it starts?** ADR-0005 wants an owner for
   anything gossiped, and ADR-0019 §3 spent its longest section on exactly this for schedules.
3. **What stops a runaway watcher?** A program printing a line a second is a fleet full of runs
   and a bill.
4. **What does the event text become?** The sink rule refuses templating in as many words.

## Decision

### 1. The trigger is a long-lived program the owner nominated, and its stdout is the event stream

`[[triggers]]` in node config: an id, a `Service`, a command, args, optional `env`, a
description. It advertises as `trigger:<id>` with `Role::Trigger`, and — as with a sink and a
resource — **the command never leaves the node**. A peer learns this device has an `email`
trigger, never how it watches.

The daemon spawns it once and reads its stdout: **one line is one event**. Nothing else is a
protocol. stderr is this node's own `tracing`, and an exit is a restart with backoff.

**The cadence is the program's, not ours.** This is the third application of the rule sinks and
resources already follow, and here it buys the thing ADR-0019 spent a section refusing: there is
no interval field, no cron parser, no HTTP client and no change-detection rule anywhere in the
orchestrator, because a program that wants a clock writes its own loop. `Service::Schedule` —
"fires on a clock rather than on an event" — is then something an owner *nominates* (`sh -c
'while sleep 300; do echo tick; done'`) rather than something the daemon implements.

**Refused: an `interval` in `[[triggers]]` with the daemon running the program per tick.** It is
smaller to write and it is a scheduler, which is the "never reimplement" rule broken from the
same end ADR-0019 rejected `Work::HttpPoll` from. It also makes a missed tick the daemon's
problem to have an opinion about, and §3 below is only simple because the daemon never has one.

**A program that keeps dying is retried for ever and reported, never given up on.** Backoff
climbs to a ceiling and stays there, and `offload triggers` shows the state and the last error.
Giving up would mean a watcher silently stopping — the delivery plane's lesson (a route that
stopped being retried is not a route) in the inbound direction.

### 2. The binding is a **rule**, it lives on the node holding the trigger, and it is not gossiped

A **rule** is: *when this service's trigger fires here, submit this run.* It holds a whole
`SubmitRequest` — every flag `offload run` has — plus the service that fires it.

**Owned by the node the trigger is on, and node-local.** Three reasons, and the first is the
entire argument for this being the smaller half:

* **A trigger fires on exactly one node**, because the program is that node's owner's. So the
  two-nodes-fire-one-tick problem that forced ADR-0019 §3 into a derived `RunId` does not arise,
  and none of that machinery is needed.
* **It is the sink rule again.** The command stays on the node, so the thing that binds the
  command to a run stays with it. A rule gossiped away from its trigger would be a fact whose
  subject the receiver cannot see.
* **A gossiped fact needs an owner and an observation does not** (ADR-0005, and the fleet log's
  reasoning). Two nodes holding rules for `email` are not disagreeing; they each have a mailbox.

Stated rather than discovered, because the opposite is the reasonable guess: **a rule on a device
that is off does not fire, and nothing else picks it up.** That is correct and not a gap. The
trigger *is* that device's — migrating the rule would mean migrating the program that watches,
which the node never gossiped and the fleet cannot run. What migrates is the **run** the rule
fires, which is the ordinary path and the whole point.

`offload rules` therefore **canvasses** rather than reading gossip, exactly as `offload asks`
does, and for the same reason: the answer is the union of what each node holds and nobody owns
the union.

### 3. One occurrence in flight per rule; anything that arrives meanwhile is **dropped and counted**

A rule with a non-terminal run does not fire a second one. The event is discarded, the count is
kept, and `offload rules` shows it.

This is ADR-0019's "a missed tick is not made up", arrived at from the other direction and for
the same reason: a watcher's value is the current state, and a backlog of occurrences is the
notification storm ADR-0010's audience rules exist to prevent. It also needs no clock, no queue
and no minimum-interval knob, which is three mechanisms not built.

**The count is not decoration.** A rule reporting "fired 3, dropped 412" is a rule whose run
takes longer than its trigger's cadence, and that is the only symptom that failure has. A plane
that cannot explain a silence has failed at its only job — and a rule that silently swallows
events would be that failure with the numbers available and unprinted.

### 4. The event is appended under a fixed heading, capped, and it is untrusted

No templating and no substitution — the sink rule verbatim, and for its reason: *a little
template language would be a quoting bug with a syntax*. The line is appended to the rule's
prompt under one fixed heading that says where it came from, and it is **capped** (4 KiB, longer
truncated with a note), because a runaway watcher must not put a megabyte into a prompt.

**And it is text from outside that reaches an agent's prompt.** Say it plainly rather than imply
a boundary that is not there: a webhook body or a mail subject can try to talk to the model. What
bounds it is what already bounds every run — the allowlist, the permission mode and the resources
**the rule's author** chose, never anything the event says — which is the `.offload.toml` posture
(a repo may say "run my tests", not "give me `sh`") applied to a third source of content. The
heading exists so the model is told which half is instruction and which is data. Nothing here
pretends that is a guarantee.

### 5. `Grant::Submit` is checked when the rule fires, not only when it is written

Both, actually — writing a rule this node may not act on is worth refusing at the keyboard — but
the load-bearing one is at the fire, against the certificate as it stands right now. That is the
rule grants already follow (`Peer` carries the certificate, not a snapshot of its effect), and a
trigger is the one place in the system where a *run is submitted by no operator*, so a revoked
node whose watcher keeps firing is exactly the case the check exists for.

### 6. A rule reclaims its last occurrence's checkout — and only if there is nothing in it

Found by running it rather than by reading it, which is the reason it is a numbered decision
rather than a footnote. A watcher ticking every three seconds left **eight worktrees in
forty-five seconds**, all of finished runs, and nothing in the product would ever have removed
one.

`Supervisor::cleanup` is *deliberately* never automatic, and its reasoning is right: when an
agent finishes, its worktree holds the work, and reclaiming the disk the moment the process exits
would throw it away before anybody read it. What ADR-0020 breaks is the premise underneath —
**there is somebody who will read it**. A triggered run has no such person by construction; that
is the whole meaning of unattended. So the same rule, unchanged, means one checkout per firing,
for ever, on a machine nobody logs into.

So a rule reclaims the previous occurrence's checkout when it fires the next one, bounded three
ways:

* **Only if the checkout holds nothing uncommitted.** Asked of git, not of the worktree summary
  beside the run — that string is written at a turn boundary and is one boundary stale by
  construction, and this is the question whose wrong answer deletes somebody's work. Committed
  work is on the run branch in the mirror and survives teardown, which `remove` already says, so
  the fragile part is exactly the part `git status` reports. A rule whose occurrences leave
  something behind keeps every one of them — unbounded, and only ever when there is something
  worth keeping, which is ADR-0003's rule honoured rather than traded away.
* **Only where the checkout is.** An occurrence may have been placed on a peer (ADR-0006), and a
  node reclaiming a worktree it never had would report a removal it did not perform — into
  numbers that gossip and win a tie on the author's clock. That is session seventeen's `offload
  rm` bug, and it matters *more* here: there was a person typing `rm`, and this fires by itself.
  "Not here" is therefore a third answer rather than a kind of "no".
* **Only for a finished occurrence**, which the caller has already established: a rule fires its
  next occurrence only once the last one is done.

**Left unbuilt, and named rather than discovered.** The run *records* still accumulate — a row
and its events per firing. That is much smaller than a checkout and it is a decision rather than
an oversight: `Store::delete_run` cascades to a run's **outbox rows**, and an outbox row is the
whole of what the delivery plane's at-least-once promise is made of. Anything that starts pruning
records has to decide what happens to news still owed about a run it is deleting, which is its
own ADR. A rule firing hourly is twenty-four rows a day; a rule firing every three seconds is a
rule nobody should write, and `offload rules` shows the numbers that say so.

## Consequences

Good: `Role::Trigger` stops being a variant that describes nothing. A phone that hosts no agent
gains a second job beside reaching a person — noticing that something happened — and the work it
starts is an ordinary run, so bidding, leases, epochs, migration, `explain`, the audit log and
the delivery plane all apply with no change at all. Nothing in `offload-core` moves, and
`RunSpec` is untouched: this is the half of ADR-0019 that changes no existing type.

Bad:

* **Run records accumulate and nothing prunes them**, per §6's last paragraph. Bounded in
  practice by how often a sensible watcher fires, and visible in `offload rules` either way.
* **A rule is durable state with a lifetime nobody manages.** It outlives the runs it fires, and
  a rule for a trigger the owner has since deleted from their config is a rule that will never
  fire. It is listed as such rather than removed, because removing somebody's standing
  instruction on the strength of a config edit is the wrong direction to be confident in.
* **Two ways a run is born.** "Every run was submitted by an operator at a keyboard" stops being
  true, which matters for reading a log at 07:00 and for any future simulation — the same shape
  as ADR-0019's warning that a derived `RunId` costs an invariant, and cheaper, because a
  triggered run's id is still minted by the daemon that fired it.
* **The dropped-event count is the only backpressure.** A rule whose runs are slower than its
  trigger is correct, silent in the fleet, and visible only to somebody who looks. There is no
  alarm, deliberately: a notification per dropped event is the storm, and one per rule would need
  a threshold nobody can pick.
* **A trigger's program is a child of the daemon**, so it dies with it — and unlike an agent it
  is *supposed* to, because it holds no work. The leftover sweep does not apply and must not be
  extended to cover it: killing a watcher on somebody's machine at startup is a different act
  from stopping a stray agent that is spending money.

## Alternatives

**An `interval` field and the daemon polls.** Rejected in §1: it is a scheduler in the
orchestrator, and it hands the daemon an opinion about missed ticks that it otherwise never needs.

**The rule gossips, and any node holding a matching trigger may fire it.** Rejected: it
re-introduces ADR-0019 §3's whole problem — two nodes, one event, two runs — for no gain, since
the two nodes' triggers are two different programs watching two different things, so "one event"
was never true in the first place.

**The trigger program submits the run itself by invoking `offload run`.** This is cron plus the
CLI, and ADR-0019 already rejected it as the fleet's answer. Worth noting *what* it loses, since
half of it survives: the **run** would still get placement, migration and delivery, so the loss
is narrower than it looks — it is the *trigger* that becomes invisible. No `offload triggers`, no
`offload rules`, nothing that says why it did not fire, and no drop count. Those are the same
four things cron lacks, one layer up.

**A minimum interval per rule instead of one-in-flight.** Rejected: it is a knob whose right
value is "however long the run takes", which is what one-in-flight measures for free.

**`--keep N` occurrences instead of §6's reclaim-the-last-one.** Rejected as a knob nobody can
pick: N checkouts of what, exactly — the last N firings, or the last N that left something? The
version built needs no number because it answers the question the project already has an answer
to, which is whether there is uncommitted work. It also needs no record of which runs a rule
fired, and therefore no second table.
