# ADR-0056: A schedule is an interval, and its occurrence's id *is* the tick

**Status:** accepted · 2026-09-10 · settles what ADR-0019 §3 left open · design and build in one
session, so read it as description rather than as intent · **two sentences amended by measurement
in session sixty-nine** — see *Amendments* at the foot

## Context

ADR-0019 §3 decided the hard half of recurring work and said so: not a long-lived run that sleeps
between polls, but short `Idempotent` occurrences; owned by the node the schedule was created on
with arbitration's own successor rule; an occurrence id derived from `(schedule, tick)` so that two
nodes firing one tick converge on one record; and **no catch-up**, because a phone asleep for six
hours must not wake to six runs.

What it did not say is enough to build from. Four things were open, and each of them can be got
wrong in a way that looks like it works:

1. **How a schedule is spelled.** "Something that fires on a clock" is not a syntax. ADR-0020
   refused an *interval* for a trigger, which is a different question with a different answer — a
   trigger is a program that notices something, and giving it an interval would have made Offload
   the poller. Here the interval is the whole content.
2. **How wide the window is.** "No catch-up" says which ticks are *not* fired. It does not say
   what stops one tick from being fired twice, or what a node that has been asleep does when it
   wakes inside a tick that has already been served.
3. **What a schedule fires** — an agent run, a task, or either. §3 says "idempotent runs" and its
   context is the watcher, which is the cheap tier; the sentence and the context disagree.
4. **How one is removed.** A gossiped *set* with removals is the problem tombstones exist for, and
   `docs/pitfalls/rules-and-retention.md` already carries the version of it this project has met:
   *a peer learns every occurrence and nothing there will ever remove one.*

## Decision

### 1. The spelling is an interval, aligned to the unix epoch, with an optional offset

```
Schedule { every: Millis, offset: Millis, .. }     // offset < every
tick(now) = ((now - offset) / every) * every + offset
```

Integer arithmetic on UTC milliseconds, and nothing else. Every node computes the same tick from
the same schedule with no message, no clock agreement beyond a millisecond, and no dependency —
which is the property the derived occurrence id is *built on*, and the same reason the equal-epoch
tiebreak and `winner()` are chosen the way they are.

The offset is what makes `every 24h` useful rather than a fixed appointment at midnight UTC: with
`every = 24h, offset = 3h` a schedule fires at 03:00 UTC. It costs one subtraction and keeps the
whole calculation pure.

**Refused: cron, local time, calendars and days of the week.** Each of them needs a timezone, a
timezone needs a database, and a database is a dependency in a crate whose whole discipline is not
having any (`offload-core`: no I/O, no tokio, **no clock**). Worse than the dependency is the
divergence: the nodes of one fleet are in one person's life but not necessarily in one timezone,
and two nodes that disagree about what "03:00" means compute two different ticks and fire two
different occurrences — which is precisely the failure the derived id exists to prevent, arriving
through the front door. A schedule is a fleet-wide fact, so it is expressed in the one clock every
device already agrees on.

What that costs is stated plainly rather than hidden: **"every weekday at 09:00 local" is not
sayable**, and the way to say it today is a `[[triggers]]` program that prints a line and an
`offload when` rule bound to it — node-local, dying with the device, which is the trade. If
somebody needs it in the fleet, that is an ADR with a timezone decision in it.

### 2. Only the current tick is a candidate, and two different things say it has been served

A node fires tick `T` only when **`T` is the current tick**, **`T` is later than the last tick
this node fired**, and **no run with `T`'s derived id exists**. Three conditions, and the first
is the one that makes the other two small:

* **No catch-up**, because only the current tick is ever a candidate. A device asleep for six
  hours wakes, computes one tick, and fires at most that.
* **A node-local high-water mark** (`schedules.last_tick_ms`) is what stops *this* node from
  firing a tick twice.
* **The occurrence's own record** is what stops a *second* node from firing a tick this one has
  already served — a transient dual steward, or a successor taking over. Its id is derived, so
  any node can look for it without being told.

**The first draft of this ADR had only the last of those**, on the grounds that a cursor would be
a mutable gossiped field needing an owner and a merge rule (ADR-0005) when the occurrence is
already a gossiped fact with both. That argument is still right about a *gossiped* cursor and it
is wrong about the mechanism, and the build found out why before the walk did: **a scheduled
occurrence is machine-started, so `prune_spent_records` reclaims its record about an hour after it
finishes** (ADR-0021, ADR-0024). For a daily schedule that is twenty-three hours *before* its tick
ends — so with the record as the only evidence, a nightly schedule fires roughly hourly.

The mark is therefore **node-local and never gossiped**, which is `runs.rule`'s arrangement
exactly: it is this machine's memory of what it did, no peer can state it, and it is outside the
row's `ON CONFLICT DO UPDATE` list so a gossip merge cannot erase it. It is a high-water mark
rather than an assignment, because the pass that reads it is a poll and a clock that steps
backwards must not re-open a served tick.

Two checks, two different failures, and neither covers the other. What remains — an operator who
deletes an occurrence record on the node that will fire the next one, or a successor firing a tick
whose record was pruned while the home was away — is bounded and acceptable for the reason the
whole tier is: an occurrence is `Idempotent` by construction, which is what "safe to run again
from the start" means.

**And a refusal does not consume the tick.** The first build marked the tick before submitting,
which is the ordering that cannot double-fire — and it meant an occurrence nobody would take spent
its tick, so a daily schedule whose fleet was busy for one second lost the day. Measured on two
daemons: the home node was killed, the successor fired correctly, and nobody could host the run
because the successor was still on probation. So the mark is written **after** the placement, and
what makes that safe is a bounded number of attempts per tick — a dozen, which at the pass's
cadence is the first minute. Without the retry a transient refusal costs a whole period; without
the bound a schedule naming a service nobody offers, which is the likeliest way to write one
wrong, runs a bid round every five seconds for ever.

The count is **attempts** and not elapsed time, and that distinction was itself a defect on the
way through: a window measured from the start of the tick refuses to serve a daemon that started
up in the middle of one, which is exactly the case where firing is right. It is held in memory
rather than in the row, because it describes this process's attempts and is worth nothing after a
restart — whatever was refusing may have been the thing that restarted.

### 3. A schedule carries a whole `RunSpec`, so it fires either tier

Not a `SubmitRequest`. A submission is turned into a spec by `Supervisor::build`, which applies
*that node's* configuration — the default model, the permission mode, the node's baseline
allowlist — and a schedule may be fired by a machine that has never seen the creator's config.
So the spec is complete when the schedule is created, and the creating node's defaults are the
ones that travel.

**This is not a new rule; it is the rule submissions already follow.** `offload run` on the laptop
merges the laptop's baseline allowlist into the spec and the desktop then runs it, adding the
repo's `.offload.toml` layer and any resource grants when it starts. A schedule is a submission
somebody wrote down, and it behaves like one.

It differs from a **rule** (ADR-0020), which stores a request and builds at the firing — and can,
because a rule is fired by the one machine that stored it. That difference is a consequence of
gossiping, and it is the reason a schedule is the larger mechanism.

Because the spec carries `Work`, both tiers come free: `--task webhook` and a prompt against a
repo are the same schedule with a different `Work` variant. The creating command stamps
`Restartability::Idempotent` on both, which is §3's own decision — an occurrence that has to move
starts again, and migration becomes a reschedule.

### 4. Who fires is the steward, and it is arbitration's rule with no second copy

The schedule's **home node while it is available, else the lowest-id available node**. That is
`ClusterView::arbiter_for`'s rule exactly, so it is factored rather than restated:
`ClusterView::steward_of(home)` is the one implementation and `arbiter_for` calls it. A second
copy of a successor rule is two answers to "who acts", and this project has paid for that shape
before.

The derived id is the **net** under this, not the mechanism. The successor rule can transiently
name two stewards — it is the few-missed-probes case that produces two arbiters (ADR-0002's
amendment) — and when it does, two firings of one tick are one record. What that must never be
read as is permission to place a run twice: a duplicate *record* merges and a duplicate *agent*
does not, and placement still goes through one bid round per run per node (ADR-0050).

### 5. Removal is a tombstone, owned by the home node

`removed_at: Option<Millis>`, set by the home node, monotonic, and any copy carrying it wins the
merge. A gossiped set with removals needs one — a peer that has learned a schedule will otherwise
keep firing it for ever, which is `docs/pitfalls/rules-and-retention.md`'s standing entry about
occurrences reached from the other end.

The precedent is `Revocation`, and so is the argument: *membership is a certificate, so it need
not travel; revocation must.* A schedule's definition is its own node's business until that node
is away, and its removal is the one fact about it that has to reach everybody.

Tombstones are kept for ever. There are a handful of schedules in a personal fleet and each is a
row, so no sweeper is worth the risk of one deleting a tombstone whose schedule then comes back
from a laptop that was in a bag.

## Consequences

Good: the cheap tier can now fire itself, which is the whole graduated-cost argument — before
this, the only thing that could submit a task was a person at a keyboard, since `offload when`
builds a `SubmitRequest` and a trigger therefore cannot start one. A schedule survives the device
that created it, which is the difference between this and `cron`. And the machinery it needs
already existed: one bid round, one lease, one arbiter, one delivery plane.

Bad, and none of it hidden:

* **No local time, no calendar, no cron.** Said above, and it is the limitation somebody will meet
  first.
* **Ids have a third provenance.** ADR-0019 already counted the second; a `RunId` may now be
  minted by `uuid::now_v7`, or derived from `(schedule, tick)`. Any future simulation has to know
  which, and the invariant "every run id is minted once, by the daemon that took the operator's
  command" is a sentence about phase 4.
* **The spec is frozen at creation.** A schedule written in March runs March's model until
  somebody edits it, and editing means removing and re-creating (there is no `spec_rev` for a
  schedule, deliberately — the two editable fields on a *run* are scheduling inputs, and a
  schedule's whole content is not).
* **A schedule is a fourth gossiped fact**, so the wire moves and there is a merge rule to keep in
  step. It is a small one — every field but the tombstone is immutable — which is why it was worth
  having rather than avoiding.
* **`Origin::Rule` is stamped on an occurrence**, and the variant's *name* is now narrower than
  its meaning: what it records is that a **machine** started this and nobody is waiting for the
  output, which is what the column is actually called (`runs.machine_started`) and what everything
  reading it wants. Renaming the variant would be a wire change for a word, so it keeps the name
  and this paragraph exists instead.

## Alternatives

**Nominated in node config, like a sink, a trigger, a task or a resource.** Genuinely attractive:
it is the shape everything else the owner declares already has, it needs no new gossiped type, no
tombstone, no wire bump and no schema change, and it puts the schedule next to the `[[tasks]]`
entry it fires. Rejected because it duplicates the *definition*: the same schedule written on two
machines is two definitions that can diverge, and two nodes computing `every = 15m` and
`every = 30m` derive different ids and fire two occurrences — which is the failure the derived id
was chosen to prevent, arriving through configuration drift instead of through a race. One
definition, gossiped, is what makes the arithmetic convergent.

**A cursor on the schedule** (`last_fired_tick`), gossiped and merged by taking the maximum.
Rejected in §2: it is a mutable gossiped field where the record it would describe is already a
gossiped fact, and a `max` merge over a field two nodes write is a new disagreement every tick.

**A long-lived run that sleeps between polls.** ADR-0019 §3 rejected it and the reasons stand: it
pays a lease and a slot all night for a second of work, and it makes "the phone is asleep"
indistinguishable from "the run is between polls".

**Catch-up with a bound** ("fire at most the last three missed ticks"). Rejected: a stale
occurrence's value is a question nobody has answered — a watcher's worth is the *current* state —
and a bound is a number that is wrong for some fleet. ADR-0019 refused it and named what it would
take to change: a decision about what a stale occurrence even means.

## Amendments

Both from walking this on two daemons in session sixty-nine, and neither changes a decision: the
code was already doing the better thing in one case and the ADR's prose was narrower than what was
built.

### §5 says the tombstone is "set by the home node". Any node may set it, and that is right.

`Request::Unschedule` calls `remove_schedule` with no check that this node is the schedule's home,
and the removal then gossips like any other. Measured: `offload unschedule` typed on **bravo** for
a schedule homed on **alpha** removed it, and the tombstone reached alpha within eight seconds.
Both nodes then reported it removed and it fired nothing again.

That is the behaviour to keep, and §5's own precedent is the argument for it. `Revocation` is a
fact any device may hold and relay — *membership is a certificate, so it need not travel;
revocation must* — and a tombstone whose only author is the home node cannot be written at all
once that device has been sold, which is exactly when somebody wants it. The merge rule needs no
owner here: earliest `removed_at` wins, which converges however many nodes set one and in whatever
order the gossip arrives.

So the ownership sentence is withdrawn. What §5 should say is that a removal is **authored by
whoever types it and owned by nobody**, and that this is safe because the field is monotonic and
its merge is a minimum.

### …and "a schedule survives the device that created it" holds only while that device is *remembered*

`ClusterView::steward_of` has three answers, not two: the home while the home is present and not
gone, the lowest-id live node when the home is present and gone, and **`None` when the home is not
in this node's view at all**. The third is the safe answer and stays — a daemon that has just
started and met no peers must not begin firing the whole fleet's schedules — but it means a node
that restarted while the home was away fires nothing until the home comes back.

Measured: bravo restarted while alpha was off, held a live copy of alpha's schedule, and fired
nothing across two whole tick periods; alpha returned, bravo re-met it on the seed backoff, and the
schedule resumed on its own with no intervention. So the window is real, self-healing, and bounded
by the home's absence — and for a device that is *gone for good* it does not close, which is what
`offload unschedule` from any node is for (above).

What was actually wrong was the report. `ScheduleReport::mine` was a `bool` over this three-valued
fact, so the third answer rendered as **"fired elsewhere"** — naming another machine for work no
machine was doing — with a countdown to the next tick underneath it. The honest half of the same
fact was one line above, where `home` had already fallen back to a bare node id because that node
is not in the view either. `api::Steward::{Here, Elsewhere, Nobody}` now carries all three, the
`Nobody` arm says what it means and what to do about it instead of printing arithmetic, and
`steward_of`'s three answers have three tests where they had none.