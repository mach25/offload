# ADR-0030: Only the machine holding a rule may retry its occurrences

**Status:** accepted · 2026-08-27 · closes ADR-0027's stated residual · supersedes nothing

## Context

ADR-0027 stopped a rule resuming its own failed occurrence beside the occurrence that superseded
it, and named what it did not reach:

> an occurrence placed on a **peer** and failed there is watched by that peer, and the rule is on
> another machine. Nothing withdraws it… What bounds it is that the peer will not fire a competing
> occurrence either — it has no rule — so what is left is a resume of a stale event, three times,
> and not two legs of a rule racing.

That paragraph is half right and its conclusion is wrong, which is what measuring it was for. Two
daemons, a rule on `alpha` firing every four seconds, `alpha` configured `accept = "never"` so
every occurrence is placed on `beta` and fails there after checkpointing:

```
alpha  29 firings   ·  0 events dropped  ·  0 withdrawals
beta   50 agent launches  (29 fresh + 23 resumes)  ·  0 escalations
       bursts of 5 resumes in a single second
```

**Worse than the case ADR-0027 fixed, in three ways.**

* **Nothing bounds it at all.** ADR-0027's withdrawal is a *no-op* here — the recovery watch is on
  beta and the rule is on alpha, so `stop_recovering` removes nothing, 29 times.
* **Alpha reports perfect health.** `0 events dropped`, because each occurrence finishes before the
  next tick, so the rule's own bookkeeping is correct and says nothing is wrong. The 1.7× spend is
  entirely on the machine nobody logs into.
* **It arrives in bursts.** Failures accumulate while backoffs elapse, so resumes land five at a
  time rather than spread out — the shape that looks like a fault in the peer.

And there is a contradiction underneath it, on one machine, needing no rule and no gossip. Beta
already treats a terminal occurrence as **disposable**: `reclaim_departed_checkouts`'
`nobody_waiting` deletes its checkout precisely because `origin == Rule` and the state is terminal
(ADR-0023, ADR-0024), and `prune_spent_records` will delete its record. Meanwhile
`recover_failed_runs` intends to start an agent *in that checkout*. Both cannot be right about the
same run on the same node.

## Decision

**Recovery of a machine-started run belongs to the machine that started it.**

`decide_recovery` takes a `Standing`, and a node whose answer is `MachineStartedElsewhere`
escalates with `Escalation::NotOursToRetry` rather than resuming.

```rust
pub enum Standing { Ours, MachineStartedElsewhere }
```

### The discriminator needs nothing new

Two facts already have exactly the right shapes, and the pair is what makes this answerable
without gossiping anything about a rule:

* **`Run::origin` travels** (ADR-0024), so a node that has never heard of a rule can tell an
  occurrence from somebody's work. That is the one thing origin is for — *what may be thrown
  away* — and this is that question: a run nobody will come back for is not a run to retry.
* **`runs.rule` does not.** `write_run`'s `ON CONFLICT DO UPDATE` names the columns a merge
  overwrites and that is deliberately not one of them, so a peer learns the record *untagged*.
  Which makes "tagged here" mean "a rule on this machine fired it".

So `origin == Rule && !is_local_occurrence(id)` is somebody else's rule's occurrence, and no
`RuleId` has to leave the machine that owns it.

### It is checked **before** attendance

An occurrence is unattended *by construction* — that is what a rule firing into the night means —
so a standing check placed after the attendance branch would be waved through every single time.
The test asserts the order rather than assuming it: neither `Attended` nor "cannot tell" gets to
answer first.

### A store error reads as *ours*

The pre-ADR-0030 behaviour. The wrong direction here costs one stale retry; the other wrong
direction would silently stop retrying the runs this whole axis exists for, on any node whose
store hiccuped once.

### Why not the alternatives

* **Not "gossip the rule".** A `RuleId` is one machine's name for one of its own things, useless to
  a peer for the same reason an `Audience` names services and never route ids. ADR-0021 and
  ADR-0024 have both already declined this trade, and it would buy a retry.
* **Not "the rule's node reaches out".** A message from alpha authorising or withdrawing beta's
  retry is real machinery — a new wire message, a new failure mode when alpha is asleep — for a
  case whose entire value is that a nightly occurrence gets a second chance.
* **Not "never auto-resume an occurrence".** ADR-0027 rejected that and the reason still holds: a
  nightly rule whose occurrence dies at 02:00 gets no other chance for twenty-four hours. What this
  ADR narrows is *which node* answers, not whether anybody does.

## Consequences

* A rule's occurrences cost one agent launch per firing again, wherever they are placed. Walked:
  **30 firings → 30 launches, 0 resumes, 28 escalations**, each naming the reason in beta's log.
* **The nightly retry survives where it can happen at all.** It was always conditional on
  placement — ADR-0027 preserved it for an occurrence hosted on the rule's own node, and that is
  exactly the case `Standing::Ours` still covers. What is lost is a retry of an occurrence placed
  elsewhere, which was never reliable and was never bounded.
* **`nobody_waiting` becomes true rather than nearly true.** A peer that may throw away an
  occurrence's checkout and record is now also a peer that will not resume it, so the two
  mechanisms agree about the same run. That removes the need to revisit `is_terminal` there — which
  is the wrong question for `Failed` everywhere else (ADR-0022 §5) and is the right one here only
  because of this decision.
* The residual, and it is small: an occurrence placed on a peer that fails for a *transient*
  reason is not retried anywhere. It is reported — ADR-0026's `Problems` is a rule's default and a
  failure is a problem — and the next firing is the recovery, which is ADR-0027's argument arriving
  one machine over.
