# ADR-0031: A peer's absence history is an observation, so it is per-node and durable

**Status:** accepted · 2026-08-27 · answers ADR-0005's table for `NodeView::absences` ·
wires what ADR-0007 needed · supersedes nothing

## Context

ADR-0007's hold-down waits longer for a node whose absences are historically short, and
`NodeObservation::typical_absence`'s own doc comment calls it "the single most useful input here: a
laptop that always comes back in 90 seconds should be waited for; one that vanishes for hours
should not."

`offload-store`'s `observations` module was built to make that survive a restart, and says so:

> In phase 1 that history lived in memory, so it reset on every daemon restart — which is precisely
> when a node has just been absent. Every restart threw away the evidence of the event that caused
> it. Small table, disproportionate value.

**Nothing called it.** The table, `save_observation`, `all_observations` and `record_return` had no
reference in production code at all, so every sentence of that paragraph was still true in the
present tense. Found by sweeping for public functions with no non-test caller — the same pass that
found a second start gate (`WorkPolicy::admits_start`) and, inside this very module, a second copy
of the averaging.

And looking at it turned up the half nobody had asked: **the same history is a field on the
gossiped `NodeView`**, with no `serde(skip)` and no entry in ADR-0005's ownership table. What that
produced was two different answers to one question:

* a peer learned **indirectly** inherited the relayer's history of it wholesale, because
  `merge_node`'s insert path takes `incoming` entire;
* a peer learned **directly** started at zero, because a node's report about *itself* always carries
  zeros — `set_status` refuses to believe a peer about its own liveness, so nobody ever observes
  their own absences.

So the only values that ever crossed the wire were third-party hearsay, and which one a node got
depended on the order it met the fleet in.

## Decision

### 1. It does not travel

`#[serde(skip)]` on `absences`, `typical_absence` and `absent_since`. This is the rule CLAUDE.md
already states for orphan status ("an observation cannot be relayed"), for attendance ("gossiping
it hands the arbiter a value from before the silence and calls it current"), and for the fleet log
("two nodes recording the same enrolment are not disagreeing, they are describing what each of them
saw"). Absence history is the same kind of fact: it is what **this** node has seen of that peer's
comings and goings, and merging two of them would need an owner for an event neither owns.

Sometimes the answer to "who owns this gossiped field" is that it should not be a gossiped field.

### 2. It is durable instead

Which is where the value was needed all along, and the reason the store module exists. A restart is
precisely when a peer has just been absent.

* **Read once, at startup**, and used only to *seed* a peer nothing has been learned about yet — so
  a row that goes stale while the daemon runs is a row a live observation has already superseded.
* **Written on the reprobe cadence** (thirty seconds), not every tick: a row per peer per second is
  a lot of small writes to buy a second of freshness in a value measured in minutes. What that
  costs is a transition in the last thirty seconds before a crash, which is stated rather than
  hidden.
* **`absent_since` is not persisted.** Whether a peer is away *right now* is this incarnation's
  observation and a restarting daemon has no business believing it; what survives is how long its
  absences have historically lasted.
* **An unreadable table is a warning and the old behaviour** — every peer starting from the policy
  default — never a daemon that refuses to come up. The same rule the device capacity ledger
  follows.

### 3. Seeding is idempotent, so nothing has to remember having done it

`NodeView::seed_absence_history` refuses to overwrite anything learned in this incarnation, and
refuses to treat an empty row as a value. That makes it safe to call on every tick — which is how a
peer met a moment ago gets its history without a separate "have I seeded this one" set — and it
makes the precedence right in one line: **a live observation always beats a remembered one.**

`Cluster::exchange_absence_history` does both directions in one pass over one lock, because they are
the same fact travelling opposite ways. The local node is excluded from both halves.

### 4. One learner, not two

`Store::record_return` folded an absence into the average itself, duplicating
`NodeView::set_status`'s arithmetic — and its doc comment named the hazard: *"Both places must
agree, or a restart would visibly change how patient the policy is."* Two copies of a rule with a
comment asking somebody to keep them in step is worse than either, and neither had a caller.

Deleted. The view computes the average; the table persists what it produced. Its best test — that a
thirty-minute outlier decays toward the normal case over about a dozen samples, asserted as a
monotonic property rather than a guessed threshold — moved to `offload-core`, where the one
implementation now lives.

## Walked, on two daemons

Both up, `beta` killed with `SIGKILL`, brought back, then **`alpha` killed and brought back** — which
is the case the store module was written for and the one that never happened:

```
beta gone      beta   suspect
beta back      beta   alive ~1        ← learned in memory (offload nodes' own report)
alpha's store  ('7D9D253B8D4F', 1, 1505)   ← written for the first time
alpha restart  beta   alive ~1        ← seeded from the store
```

The `~1` after alpha's restart can *only* have come from the store: with the fields skipped, gossip
carries zero, and beta's report about itself always did.

And the symmetry is the ownership half, measured rather than argued:

```
alpha's store  beta  → typical_absence 1505 ms
beta's store   alpha → typical_absence 2722 ms
```

One row each, about the *other* node, with different numbers — because they observed different
absences. That is what "an observation is the observer's" means, and before this those two numbers
were on the wire and could overwrite one another.

## Consequences

* The hold-down adapts per device across restarts, which is what ADR-0007 assumed and what nothing
  delivered. On a fleet of one it changes nothing, which is worth knowing: that is the mode most of
  this project's testing happens in, and it is why this survived so long.
* A node no longer inherits another node's opinion of a third node's reliability. Nobody asked for
  that behaviour and nobody would have chosen it.
* **Wire:** none. Removing a field from a gossiped struct whose body is JSON is read by an older
  build as an absent field, and the field's `Default` is exactly what a node should assume about a
  peer it has learned nothing about. This is ADR-0005's amendment rule applied in the subtractive
  direction.
* The residual, stated: a transition in the thirty seconds before a crash is lost, and a node's
  first-ever contact with a peer still uses the policy default because there is nothing to
  remember yet. Both are the shape of the thing rather than gaps in it.
