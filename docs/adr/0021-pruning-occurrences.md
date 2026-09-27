# ADR-0021: A rule's spent occurrences are pruned; "spent" is what the delivery plane has finished with

**Status:** accepted · 2026-08-26 · builds the half ADR-0020 §6 named and left · supersedes nothing

## Context

ADR-0020 §6 reclaims a rule's last occurrence *checkout* and says plainly what it does not do:

> **Left unbuilt, and named rather than discovered.** The run *records* still accumulate — a row
> and its events per firing. … `Store::delete_run` cascades to a run's **outbox rows**, and an
> outbox row is the whole of what the delivery plane's at-least-once promise is made of. Anything
> that starts pruning records has to decide what happens to news still owed about a run it is
> deleting, which is its own ADR.

This is that ADR. `Store::delete_run` has had no production caller since it was written and its
doc comment has carried the warning the whole time; what follows is the answer, not a discovery.

Four questions are open, and three of them have a wrong answer that is silent.

1. **What may be deleted?** A record is the only copy of what the agent said.
2. **What is still owed about it?** Two different things: an outbox row that exists and has not
   been sent, and an event no route has *scanned* yet, which has no row to point at.
3. **When is it safe to delete?** A finished run is gossiped for a while after it ends, and a
   deletion the fleet undoes is not a deletion.
4. **How many chances does a prune get?** Every bound above is a *temporary* no.

## Decision

### 1. An occurrence carries the rule that fired it, in a node-local column

`runs.rule`, nullable, schema **v9**, set by the firing right after `note_rule_fired`. Not on
`RunSpec` and not gossiped: a rule is node-local (ADR-0020 §2), so which rule fired a run is a
fact about one machine's own bookkeeping, and a gossiped field would need an owner and a merge
rule for something no peer can state.

This is the cost ADR-0020's last alternative declined to pay — "it also needs no record of which
runs a rule fired". §4 below is why it has to be paid now: without it, a prune has exactly one
chance per record.

A column and not a second table, because `write_run`'s `ON CONFLICT DO UPDATE` names the columns
it overwrites and `rule` is not one of them — so a gossip merge writing the row back leaves the
tag alone. **That list is load-bearing**: adding `rule` to it would silently untag every
occurrence the fleet is still talking about.

### 2. Only a **completed** occurrence is a candidate

A run that failed or was cancelled is the record somebody comes back for: the notification said
*it failed*, and the log is the only thing that says why. So the outcome decides, the same way
uncommitted work decides for the checkout — prune the ones holding nothing, keep every one with
something in it (ADR-0003's rule honoured rather than traded away).

The consequence is stated rather than capped: **a rule that fails every firing keeps every
failure.** That is unbounded, it is exactly what somebody wants in front of them, and `offload
rules` prints the count. Capping it would mean choosing which failure to throw away, and the
first one is usually the one that explains the rest.

### 3. "Spent" is what the delivery plane has finished with — and that is two questions

An unattended run's record exists so that news can be derived from it (ADR-0010: the outbox
stores the *identity* of a notification and the content is re-derived from the log). Once every
route has been told, it has done its job. Two things have to be true, and asking only the first
is the trap:

* **Nothing owed.** No outbox row about this run is still pending. Delivered and *abandoned* both
  count as resolved — giving up on a route is a decision that was already taken, and a row that
  will never be retried is not news in flight.
* **Nothing left to notice.** Every route that currently exists has a run-log cursor past this
  run's highest event. This is the half with no row to point at: the outbox is populated by a
  *scan*, so deleting a run's events before a route's cursor reaches them destroys the news
  before the promise to deliver it is ever made. Asked of the routes that **exist right now**,
  which is the set the delivery pass itself uses — a cursor left behind by a peer that has gone
  is a watermark that never advances again, and reading it would pin every record for ever.

  A route with no cursor row does not block: `sink_cursor` initialises a new one to the *end* of
  the log, so a route that has never scanned will never want these events. And a route that
  exists nowhere means nothing is owed at all.

### 4. Every spent occurrence, at every firing — not the previous one, once

The bounds in §2 and §3 are all *temporary* noes: a phone asleep for one tick, a delivery pass
that has not run yet, a record the fleet is still gossiping. A prune that looks at one record
once and never comes back turns every one of them into a permanent no — and for the case that
motivated ADR-0020 §6, a watcher ticking every three seconds, it turns *all* of them into one,
because §5's quiet period has not elapsed at the next firing and never will be re-asked.

So a firing prunes every spent occurrence of its rule, excluding the one it has just recorded.
This is what §1's column buys, and it is why a single `last_run` pointer was not enough.

`offload unwatch` prunes too, on the way out, and it prunes **past §5's quiet period** — which is
the one thing here that was found by running it rather than by writing it down. The quiet period
does not make a deletion safe; it buys the ability to *re-ask*, because the copy a peer teaches
back is untagged and no future firing could prune it again. Where there is no future firing the
clause protects nothing and costs everything: measured on a rule firing every three seconds,
`offload unwatch` left **101 records** behind, every one of them inside the five-minute window and
none of them ever looked at again. Deleting them instead trades a certain leak for a possible one
of at most the same size, on a node that may have no peer at all.

`Prune::{AtAFiring, Finally}` is that distinction, and it is the answer to "how many chances does
a prune get" read from the other end: at a firing the answer is *another one*, and at an unwatch
it is *none*.

### 5. Only once the fleet has stopped talking about it

`gossipable_runs` publishes a finished run for five minutes after it ends, so deleting a record
inside that window means a peer teaches it straight back — and the row that comes back is written
by `absorb`, which knows nothing about rules, so the resurrected copy would be *untagged* and
therefore unprunable for ever. A deletion the fleet undoes is worse than no deletion.

The test is `updated_at_ms`, not the terminal timestamp: *nothing has written this row* for as
long as a finished run is gossiped. That is the question asked directly rather than inferred, and
it is self-correcting — a late gossip pushes the row's clock forward and the next firing asks
again. `Supervisor::GOSSIP_TAIL` is one constant used by both sides, because two numbers that
agree only because somebody remembered are two things to remember.

### 6. The record goes whole, or not at all

Never the events alone. A row whose events have been deleted is a run that reports it produced no
output, which is the shape of lie this project has fixed twice already — session sixteen's
adopted-but-empty worktree and session twenty-three's finished run that was invisible from the
laptop it was submitted on. A pruned run is `no such run`, which is true.

What goes with it, deliberately:

* Its **events**, by the existing cascade.
* Its **outbox rows**, also by cascade — and safely, because §3 is exactly the condition that
  there are none left that matter.
* Its **checkpoint blobs**, which become unreferenced and collectable. Safe for the same reason
  §2 is: a completed run is never resumed from.

What stays: the **audit log**, which has no foreign key on purpose and holds what this machine
decided about the run; and the rule's own `fired` and `dropped` counters, which are the history
that survives the records.

### 7. A node does not learn of its own run from somebody else

**Amendment, same day, found by asking what a deletion is exposed to rather than by running
it.** Every clause in §3 and §5 is about a peer that is *present*. None of them is about one
that is **away**, and that is the case a prune is exposed to, because a deletion the fleet can
undo is not a deletion (§5) and the quiet period cannot see a node that is not talking.

The sequence needs no partition, only a lid. A laptop learns occurrence X while it is
`Running` and closes. The rule's node finishes X, waits out five quiet minutes — quiet partly
*because* the laptop is away — and prunes the record. The laptop opens an hour later and
gossips the copy it still holds. `merge_run` has never seen X, so it inserts it: a run the home
node has no memory of, **held by itself**, at an epoch nothing can contradict, with no agent
behind it. `heartbeat` renews that lease for ever, `offload ps` reports last night's finished
work as running, and a restart offers it to somebody as resumable. That is the "a run left
`Running` with no agent behind it, its lease renewed by the very loop that erased it" shape,
reached from outside the machine.

So: **a record naming this node as `home`, for a run this node has no record of, is refused.**
A run's home is the node that created it, so such a record was made here; if it is not here
now, it was deleted here, and a peer's copy of it is a memory rather than news. This is the
sibling of a rule the same file has held since it was written — a node must not believe a peer
about its own absence — applied to its own runs *existing* rather than to its own liveness.

It is safe because the ordering is not a race: `hand_over` records a grant locally before it
publishes one, `take_run` saves a run kept here before the arbiter confirms it, and the gossip
tick republishes this node's store straight into its own view. By the time any peer can echo
one of our runs back, our copy is already there. Six view fixtures and one mesh fixture had to
change, and every one of them was introducing the home node's own run *by gossip* — a step the
daemon has no code path for, which is `storm.rs`'s lesson one layer down.

**The residual, stated.** The stale peer keeps its copy and goes on gossiping it, because
nothing tells it the run is over — there are no tombstones here, deliberately, since a
tombstone is the row this ADR exists to remove wearing a smaller hat. Nodes that never saw the
run may learn it from that peer. What none of them can do is act on it: the home node refuses
it, and the home node is its arbiter, so it is never re-placed and never re-run. It ages out of
`gossipable_runs` as newer runs push it past the sixty-four that travel. A phantom in some
views is a strictly smaller thing than a phantom on the one machine entitled to act.

## Consequences

Good: the accumulation ADR-0020 named is bounded for the case it was named about, and bounded by
conditions rather than by a knob. A rule that fires all night and succeeds keeps one record.
`Store::delete_run`'s warning is answered rather than inherited.

**Corrected the same day:** this section first said "the disk stops being a function of how long
the daemon has been up", which was wrong by two orders of magnitude. A record is ~1.5 KB; the
*blobs* a run leaves are ~600 KB per six turns, because `record_checkpoint` replaces the
checkpoint and a transcript is the whole conversation so far. Rows were the smaller half. ADR-0022
is the other one.

Bad:

* **A route that is away for ever pins records.** `delivery_deferred` already accepts this cost
  for outbox rows ("the answer for a device that is gone for good is to revoke it"); this extends
  it to the records those rows point at, which is a larger number. Visible: `offload rules` prints
  what a rule is keeping, and `offload sinks` prints the waiting count that explains it.
* **A rule that stops firing keeps its last two occurrences.** The current one by design, and the
  one before it because §5's quiet period had not elapsed at the last firing and nothing will ask
  again. Two records is the intended floor plus one. `offload unwatch` is the case where that
  arithmetic does not hold and §4 handles it separately.
* **A fast rule's steady state is a window's worth of records, not one.** At three seconds a
  firing, ~100 completed occurrences sit inside §5's quiet period at any moment — measured: 193
  firings, 101 kept, flat. Bounded, which is the whole point, and at any cadence somebody would
  actually write it is one or two. A rule firing every three seconds is a rule nobody should
  write, and this is one more number that says so.
* **The prune is node-local, so a peer that learned the occurrence keeps it — for ever.** Measured
  on two daemons: alpha's rule settles at 101 records while **beta grew from 81 to 181 in five
  minutes**, linearly, on a machine hosting nothing. Gossip teaches a peer the record and nothing
  on that peer will ever remove it, because the tag is node-local and the rule is somebody else's.
  Not a regression this ADR introduces — *nothing* has ever deleted a run record on a peer, for
  triggered runs or submitted ones — but ADR-0020 is what made the supply of them automatic and
  unbounded, so this is where it starts to matter. Deliberately not fixed here: the fix is a
  retention policy for a finished run a node neither hosted nor submitted, which is a decision
  about **every** run rather than about occurrences, and it would change what `offload ps --all`
  and `offload logs` can answer from a machine that was not the one running the work — the exact
  thing session twenty-three had to fix. That is its own ADR, and the numbers above are the
  argument for writing it.
* **A deletion is exposed to nodes that are away, and no local clause can see one.** §7 is the
  answer for the case that matters — this node's own runs — and it is deliberately not a general
  one. A node that pruned *somebody else's* record would have no such standing to refuse, which
  is one more reason the peer accumulation above is left alone rather than solved with a sweep.
* **Deleting a run record is now something the daemon does by itself.** It is narrow — a completed
  occurrence of a rule on this node — but "nothing ever removes a run row" stops being true, and
  anything that later assumes a record is permanent has to check.
* **`runs.rule` is a column nothing outside this path reads**, and its survival depends on a list
  of column names in `write_run`. Written down in §1 and in the migration, which is the most that
  can be done for it: there is no type that says "do not add this to that `ON CONFLICT` clause".

## Alternatives

**Delete the run and let the outbox rows cascade.** This is what the code would have done if
nobody had asked, and it silently breaks the plane's only promise: at-least-once becomes
sometimes, for the runs nobody was watching, which are the runs the plane exists to report on.

**Keep the outbox rows and delete only the events.** Worse. The content is re-derived from the
log by design, so a surviving row would name a notification that can no longer be built — a
promise kept as a row and broken at delivery, with the failure appearing as an unexplained
silence rather than as a missing row.

**A retention sweep on a timer: delete finished runs older than N days.** Rejected twice over. It
is a policy about *every* run, including the ones a person submitted and has not read yet, which
is `cleanup`'s "never automatic" rule broken from the other end. And N is a knob whose right value
is "after the news went out", which §3 measures for free.

**Keep the last N occurrences per rule.** ADR-0020 rejected the same shape for checkouts, and the
objection survives translation: N of what — the last N firings, or the last N worth reading? The
version built needs no number, because "spent" is a question the system can already answer about
itself.

**Prune on a maintenance tick rather than at the firing.** It would also cover a rule that has
stopped firing. Not taken: it is a new loop for a case whose floor is two records, and the firing
is already the moment the rule's bookkeeping changes. If a rule that stopped ever matters, the
tick is the fix and this decision does not stand in its way.
