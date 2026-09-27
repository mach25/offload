# ADR-0034: A drain is a departure, so it happens whether or not there is anywhere to hand work to

**Status:** accepted · 2026-08-27 · corrects ADR-0007's drain as built · supersedes nothing

## Context

`offload drain` promises three things, in order: **stop accepting**, checkpoint at the next turn
boundary, hand off. On a node with `[cluster] enabled = false` it did none of the first.

Measured, one node, no mesh:

```
$ offload drain
nothing to hand over

$ offload status | grep accepting
accepting   yes

$ offload run --repo … -- "work after the drain"
run 01a043c86ade          ← accepted, and started
```

Two independent faults, both live, so fixing either alone still gives a broken drain.

### 1. The flag was set inside the pass that a fleet of one skips

`Request::Drain` branches on whether there is a cluster *and* a mesh. Only the first arm calls
`Mesh::drain`, and `supervisor.stop_accepting()` was the first line **of that function**. So on a
node with no mesh the flag was never set at all.

The comment on that line records the previous session finding the identical failure one level
lower and fixing it there:

> First, and before the early return below. A node with nothing to hand over is still a node that
> has been asked to stop taking work, and the version that set no flag at all meant `offload drain`
> on an idle laptop did *nothing whatsoever*.

That reasoning is right and it was applied to the early return inside the function rather than to
the branch outside it. The same sentence describes the behaviour that remained.

This is the shape CLAUDE.md already states, about a different function:

> **A node's own runs need tending whether or not there is a fleet.** … anything about *this
> node's* runs put into the gossip tick silently does not run on a fleet of one, which is how
> phases 1 and 2 run.

### 2. …and the refusal it turns on was not on the path a fleet of one takes

Fixing (1) alone produced a node that reported `accepting no — drained` and started the next run
anyway. `is_draining` is checked in `Host::evaluate` (the bid) and `Host::accept` (the grant), and
a fleet of one has neither: `submit_run`'s no-cluster arm goes straight to the supervisor. Its own
comment says what it did instead:

> Without, "here" is the only answer there has ever been — so the grant that says whether this node
> may host is checked right here instead.

It moved the *grant* check down to that path and not the *drain* check — while `evaluate`, ten
lines away, checks drain **first of all** and says why: *"this node is leaving, and a bid is a
promise to still be here. Not a capability question and not a policy one, which is why it is
neither of the two checks below it."*

## Decision

### 1. The departure is recorded by whoever was asked to leave

`ctx.supervisor.stop_accepting()` moves to `Request::Drain`, before the branch. "This node is
leaving" is a fact about the node, not about the handover pass, and putting it outside the branch
is what makes it unconditional. It is removed from `Mesh::drain` rather than duplicated — one
owner, for the reason four call-site checks are four things to remember.

### 2. The no-cluster submit path asks both of a bid's questions, in a bid's order

Draining first, then the grant, mirroring `Host::evaluate` — because on a fleet of one that path
*is* the bid.

**Not hoisted above the branch.** A draining node may still **submit**: the grant at the door is
`{Submit, Deliver}` and hosting is separate, so a laptop being closed can still ask the desktop to
do the work. That is the thing this product is named for, and refusing at the top of `submit_run`
would take it away. On a fleet of one the two questions coincide, which is why the check belongs in
that arm and nowhere higher.

### 3. `offload drain` says the thing it always does, first

The report led with `nothing to hand over`, which on a fleet of one is the whole output and says
nothing about the only action a drain there can take. It now prints the stop-accepting fact
unconditionally, with the handover outcome indented under it.

### 4. `Response::Drained` gains `no_fleet`, because two silences are two facts

The CLI got `(moved, left)` and printed *"either mid-turn at the deadline or refused by every other
node — `offload ps` and `offload nodes` say which"* — on a machine whose fleet is itself, advising a
command that lists nobody. The daemon's own arm knows which case it is (its comment says "No fleet,
so nothing was offered anywhere") and the distinction was dropped between two `u32`s. Same rule as
`Removal::{Removed, NothingHere}` and `Met::{Handshakes, NotAsked}`.

## Consequences

- `offload drain` on a fleet of one now does the one thing it can: measured after the change, the
  next submission is refused with `this node is draining; restart offloadd to take work again`,
  `offload status` says `accepting no`, and the report leads with what happened.
- A **rule** on a drained node therefore stops producing work: `fire`'s refusal arm counts the
  firing as a drop and keeps the reason, so `offload rules` shows `dropped` climbing with
  `this node is draining` beside it. That needed no change — it is the pre-existing handling of a
  refused submission, and it is the reason this walk did not also find a pile of unrunnable
  occurrences.
- No wire change: the control protocol is unversioned and `no_fleet` is `#[serde(default)]`.
- The `no_fleet` sentence was first written as one continued literal and came out as `every other
  node          —`, which `offload-node/tests/messages.rs` caught. Third session that check has
  fired on new strings and the second time on strings written by the session that widened it, which
  is the case for it being a test rather than a habit: the failure is invisible in review and the
  author is the person least able to see it. It is a slice of lines now.
- One-way and in memory still, per ADR-0007 as built: the way back is starting the daemon again.
  Nothing about this makes the flag durable, and it should not — a drain lasts as long as somebody's
  walk to the door.

## What this deliberately leaves

**A drain waits behind a question nobody is going to answer.** `DEFAULT_ASK_PATIENCE` and
`drain_deadline_secs` are both **300 seconds**, and a run blocked mid-turn on `--ask` reaches no
turn boundary until its patience expires — so `offload drain` blocks silently for up to five
minutes, while the one person who could release it instantly is the person who just typed `offload
drain`. Nothing says so: the request is blocking and prints nothing until the pass returns.

Left as a finding rather than a fix because the fix is a design choice and this ADR is about a
missing action rather than a missing sentence. The candidates are worth writing down: drain could
**expire the pending questions** of runs it is draining (they are unanswerable by construction once
the node is leaving, and `Unanswered` is already a pass-through, so this costs nothing but is a
policy decision about somebody else's tool call); or `offload drain` could report the blocked
question and let the operator answer it. The second is better and needs the request to stream
rather than block, which is a shape change to the one command that currently cannot report progress.

## Amendment, 2026-08-27: one owner meant one *function*, not one call site

§1 above is right about the fact and wrong about where to put it. Moving `stop_accepting()` from
`Mesh::drain` into `Request::Drain` fixed the operator's path and broke the other one: `Mesh::drain`
had two callers, and the second is the `SIGTERM` shutdown — which is how a laptop actually leaves.
Measured, from the daemon's own log: `SIGTERM` at 15:55:41, `draining runs=1` a millisecond later,
`spawning claude code` at 15:55:44 for a run submitted three seconds into the shutdown, with
`offload status` saying `accepting yes` throughout.

The sentence in §1 — "It is removed from `Mesh::drain` rather than duplicated — one owner, for the
reason four call-site checks are four things to remember" — is the argument *against* what it did.
The owner is neither the pass nor either caller: it is the departure, which is now
`mesh::depart`, with `Mesh::drain` private so nothing can take the other route. See **ADR-0035**,
which also builds the residual this ADR left and fixes a third fault in the same report.
