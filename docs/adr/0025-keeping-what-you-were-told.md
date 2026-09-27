# ADR-0025: A node keeps the record of a run it was only told about, because that record is how the device in your hand reaches the log

**Status:** accepted · 2026-08-26 · answers the question ADR-0021 deferred · supersedes nothing

## Context

ADR-0021's consequences name this and decline it, in the strongest terms it uses anywhere:

> **The prune is node-local, so a peer that learned the occurrence keeps it — for ever.** Measured
> on two daemons: alpha's rule settles at 101 records while **beta grew from 81 to 181 in five
> minutes**… Deliberately not fixed here: the fix is a retention policy for a finished run a node
> neither hosted nor submitted, which is a decision about **every** run rather than about
> occurrences, and it would change what `offload ps --all` and `offload logs` can answer from a
> machine that was not the one running the work — the exact thing session twenty-three had to fix.

Two things have happened since. ADR-0024 made `machine_started` travel, so a peer prunes the
*completed* occurrences it learned — measured flat where they used to grow linearly. And ADR-0022
§5 took the **blobs**: a bystander holding replicas of three runs it never ran went from 4.9 MB to
zero, with nothing decided about records at all.

So the question is no longer "a peer is filling up". It is the narrow one that is left: what is a
bystander's *record* for, and what does deleting it cost?

## The measurement that decides it

Two daemons. Alpha founds the fleet, holds a trigger firing every two seconds and a rule bound to
it, and hosts every occurrence. Beta is enrolled by invitation **without `host-runs`**, so it
gossips and never bids — a pure bystander. The agent fails every time, which is the case ADR-0021
§2 keeps for ever on purpose.

```
                    alpha                     beta
records              127                      127
run_events           889                        0
blobs                  0                        0
outbox rows            0                        0
state.db          557 KB                   213 KB
progress_by set      127                      126
rule tagged          127                        0     (node-local, by design)
machine_started      127                      127     (travels, by design)
run_json          ~1022 bytes a record   ~1022 bytes a record
```

Beta's copy is a **stub**: one kilobyte of row, no events, no blobs, nothing owed. And then the
thing that settles the ADR — `offload logs <run>` **on beta**, for a run beta never touched:

```
$ offload logs 01a03f9b347c        # asked on the node that never ran it
one step
── turn 1 ──
!! checkpoint at turn 1 failed: agent: no transcript found for session f-353210
   the run continues; the next turn boundary tries again

failed: the agent stopped without reporting a result
```

Byte-identical to alpha's answer, and `offload explain` works too. The stub is not residue: it is
the **index**. `logs` resolves a run id in the local store and forwards to whichever leg last
wrote the run's position (`RunProgress::by` — session twenty-three's fix), and with no local row
there is nothing to resolve and nothing to forward to. The record that looks like a leftover is
what makes "read it from whatever device you are holding" true.

That reverses the premise ADR-0021 was reasoning from. It called the peer's copy an accumulation
and guessed that deleting it "would change what `offload ps --all` and `offload logs` can answer".
It would not *change* it; it would end it, for that run, on that device.

## Decision

### 1. A node does not prune a finished run it learned from the fleet

No retention window, no cap, no count. The copy is a routing table entry, and a routing table
that forgets the destination somebody is about to ask for is worse than a slightly larger one.

This is also the only answer available on the standing ADR-0021 §7 established. A node refuses a
peer's copy of *its own* run because a record naming us as `home` was made here and deleted here,
so a peer's copy is a memory rather than news. A node pruning **somebody else's** record has no
such standing: the gossip it deletes on Monday is news on Tuesday, taught back by any peer, and
nothing in the merge path is entitled to refuse it. A prune with no way to make itself stick is a
tick that deletes and re-learns for ever.

### 2. What bounds it, stated rather than assumed

Three supplies, and only one is unbounded:

* **Operator runs** — a person submitting work. One kilobyte a record at human pace: fifty runs a
  day is 18 MB a year, on the largest personal fleet anybody is going to run.
* **Completed occurrences** — pruned on every node by ADR-0024, measured flat.
* **Failed occurrences** — **unbounded, and not by anything this ADR introduces.** ADR-0021 §2
  keeps every failure because "the notification said it failed and only the log says why", and
  the measurement above is that clause holding *exactly* on both machines: 127 on alpha and 127
  on beta, because beta is where somebody reads it from. `offload unwatch` does not help, and
  should not: `spent_occurrences` requires `completed`, so a failing rule's records survive the
  rule itself.

  The answer is the one ADR-0021 already gives: a rule that fails every firing is a rule to fix
  or to stop. What was missing is not a sweep — it is that **nothing said so on the machine where
  it piles up**.

### 3. The number is printed where nothing else explains it

`offload status` gains one line and, when it applies, a note:

```
records     332  ·  331 for work this node neither ran nor submitted
            └─ 331 of those failed. Nothing prunes a failed run's record — it is
               how any device reads why. A large number is usually a rule
               failing every firing: `offload rules` on the node that owns it.
```

On alpha the same command prints `records 332` and nothing else, because alpha submitted and ran
every one of them and its explanation is a `offload rules` away.

This is the third statement of one habit and the first place it had nowhere to go. `offload rules`
prints what a rule is *keeping*, because a number that only grows is the one symptom every
residual ADR-0021 accepts has. `offload sinks` prints the waiting count that explains a silence.
A peer that hosts nothing has neither a rule nor a route — and it is the machine these arrive on
fastest, and the one nobody logs into.

*Learned* is defined as: this node neither submitted the run (`home_node`) nor ran the leg that
last wrote its position (`progress_by`). **Finished only** — a live foreign run is the fleet
working, and putting it in a line about residue would make every idle moment look like a leak.
The middle case is the one to get right: a run submitted elsewhere and **run here** is this
node's own work, and counting it as somebody else's would make every busy host look like it was
hoarding. Unstamped `progress_by` is *unknown*, which here reads as nobody ran it rather than
somebody else did (ADR-0023's rule, in the direction that under-reports).

### 4. What is *not* built

**No `offload rm --learned`, and no sweep behind a flag.** The failing-rule case has a cause and
the cause is on another machine; a command that tidied the symptom here would make it easy to stop
seeing. If a fleet ever does need the bytes back, `offload rm` already removes a run by name.

**Nothing gossiped about deletion.** No tombstones, for ADR-0021 §7's reason: a tombstone is the
row this would exist to remove, wearing a smaller hat.

## Consequences

Good: the item that sat at the top of two sessions' pick-up lists is answered, and answered by
measuring what the thing is for rather than by choosing a number. "Read it from any device" —
which is most of what this project is — is now written down as something a record is load-bearing
for, so the next person tempted to sweep these finds out what breaks before they try it.

Bad:

* **A fleet with a rule that fails every firing grows a kilobyte a firing on every node.** ~43 MB
  a day at two seconds a firing. Bounded by fixing the rule, which is a person's job, and now
  visible on every machine it happens to.
* **`offload status` has a line most people will never need.** The cost of the alternative is a
  number with no explanation anywhere in the product.
* **A bystander's copy is a snapshot.** It carries the record as of the last gossip that reached
  it — which is enough to resolve and forward, and is not a substitute for asking the node that
  ran the work. Nothing here makes it more current than it was.
* **`records_kept` scans the run table on every `offload status`.** Fine at this scale and worth
  knowing beside ADR-0022's note that a collector pass reads every row too.
