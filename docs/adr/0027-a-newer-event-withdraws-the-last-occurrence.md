# ADR-0027: A newer event withdraws the previous occurrence's resume

**Status:** accepted · 2026-08-27 · amends ADR-0020 §3 and ADR-0013's autonomy axis · supersedes
nothing

## Context

ADR-0020 §3 says a rule fires **one occurrence at a time**: an event arriving while the last run
is still going is dropped and counted, never queued, because a watcher's value is the current
state. `trigger::fire` implements it, and its own comment says what it is preventing:

> running it beside the first is two agents on one repository by a new route.

Auto-resume is a new route. ADR-0013's third axis picks a failed run back up when **nobody was
watching** when it broke, which is true of every occurrence by construction — that is what
unattended *means* for a triggered run. `Supervisor::recover_failed_runs` walks a per-node
in-memory watch list that knows nothing about rules, and `Store::rule_run_in_flight` reads
`rules.last_run`, which by the time the backoff has elapsed names the **newer** occurrence. So
the two mechanisms cannot see each other, and neither is doing anything it was not told to.

Measured, one daemon, a rule firing every three seconds, occurrences of three turns that
checkpoint and then fail:

```
27 firings  ·  39 events dropped as "still running"  ·  39 resumes
peak: 2 agents, from a rule that reports one occurrence at a time
```

```
04:41:41  nobody was watching; resuming it   run=01a0418573f2 resume=1   ← failed at 04:41:06
04:41:42  an event was dropped: 01a0418600e8 was still running
```

Three turns of an agent, on an event thirty-five seconds old, beside the occurrence that is
working on the current one. The arithmetic is three extra launches per failing occurrence — the
default policy is `max_resumes = 3`, and the backoff grows 30 s, 60 s, 90 s from each successive
failure — so a rule whose agent
fails spends **four times** what one firing costs. And it is visible on the phone as well as on
the bill: with the sink from ADR-0026 attached, 13 firings produced **19 notifications**, because
each resumed leg fails again and each failure is news.

**Two agents in one repository is the mild reading.** The sharper one is that a resumed
occurrence is an agent working on a **stale event**, which is worse than the queued event ADR-0020
refuses to keep. The whole argument for dropping rather than queueing is that a watcher's value is
the current state; resuming is queueing, with the queue hidden in another subsystem and the
staleness unbounded.

And autonomy-in-failure's own justification does not survive the trip. It is written as: a run due
in ten minutes with nobody watching should resume itself, *because waiting for an absent human is
how it misses the deadline*. For a watcher there is no human, ever, and the recovery is the next
firing — three seconds later, on fresh state. This is the mechanism-whose-justification-names-a-
person problem for the fourth time (`Supervisor::cleanup`, `Store::delete_run`,
`blobs::collect_garbage`), and ADR-0020's own write-up predicted it: "every mechanism whose
justification quietly assumes an operator has to be re-read on that path — `Supervisor::cleanup`
was the first and will not be the last."

## Decision

**A firing withdraws the previous occurrence's recovery watch.** The rule plane already has a
moment that means *the previous occurrence is finished with* — the `Ok(None)` branch of the
in-flight test, where `reclaim_occurrence` deletes its checkout — and the withdrawal goes there,
one line from the reclaim.

```rust
if let Some(previous) = rule.last_run {
    ctx.supervisor.stop_recovering(previous);   // ADR-0027
    ctx.supervisor.reclaim_occurrence(previous).await;
}
```

### Why this rather than the two obvious alternatives

* **Not "never auto-resume an occurrence."** It is one predicate and it is wrong for the rule
  people actually write: a nightly watcher whose occurrence dies at 02:00 gets no second chance
  for twenty-four hours, and that is exactly the case ADR-0013's axis was written for. The value
  of a resume is inversely proportional to how soon the next event arrives, and nothing but the
  next event knows that.

* **Not "a resumed occurrence blocks new firings."** That restores "one at a time" by inverting
  the priority: a stale occurrence in backoff would hold off the current state for up to ninety-
  five seconds. §3's premise is that the newer event wins.

The withdrawal is the reading that makes both mechanisms mean what they say. It needs no new
field, nothing gossiped, and no `Origin` check — which matters, because ADR-0024 keeps `origin`
out of bidding, placement, capacity and the delivery plane, and a fix keyed on it would have been
one more caller to argue about. What decides here is not what kind of thing started the run; it is
that **a newer event about the same thing exists**, which only the rule knows.

### What is preserved, exactly

* A rule with no new event resumes its occurrence normally. Nothing withdraws the watch, the
  backoff elapses, and the occurrence is picked up — and it is then `rules.last_run`, so it is
  *in flight* and the next event is dropped as §3 says. The nightly case is untouched.
* An occurrence that is resumed **before** the next event arrives is the rule's current occurrence
  and blocks new firings, which is the existing behaviour and is right.
* Every other run keeps auto-resume exactly as it was. This is a fact about a rule superseding its
  own occurrence, not about triggered runs being second-class.

### And it makes the checkout reclaim honest

`reclaim_occurrence` has two guards — nothing uncommitted, and `cleanup`'s terminal check — and
neither asks whether this node still intends to resume the run. Measured on the same walk:
`workspace removed run_id=01a0418600e8` at 04:41:45, and that occurrence resumed at 04:42:21, into
a worktree rebuilt from the mirror. It was harmless because the uncommitted check is the one that
matters and it held. But the reclaim's premise — *nothing is coming back for this checkout* — was
false, and the resume was the thing coming back. The withdrawal makes the premise true rather than
merely usually harmless, and it happens first, on the line above.

Walked with the fix in, same rule and same failing agent: **13 firings, 0 resumes, 12
withdrawals, 12 notifications** — one per failing occurrence. Before it, on the same daemon with
ADR-0026's filter corrected so the failures were visible at all: **49 firings, 105 resumes, 150
notifications.**

## Consequences

* A failing rule costs one agent per firing again, not four, and its phone gets one notification
  per failing occurrence rather than up to four.
* A resumed occurrence can no longer report a turn count from a stale event beside the occurrence
  working on the current one.
* `stop_recovering` returns whether it withdrew anything, and says so at `info` when it did,
  because "why did my occurrence stop at turn 3 and never come back" is a question with a
  one-line answer and no other place to read it.
* ~~The residual, stated: an occurrence placed on a **peer** and failed there is watched by that
  peer, and the rule is on another machine.~~ **Closed by ADR-0030, and this paragraph's bound was
  wrong** — which is why it was measured rather than left as a note. "What is left is a resume of a
  stale event, three times, and not two legs of a rule racing" understated it in three ways: the
  withdrawal above is a *no-op* on the peer, so nothing bounded it at all; the rule's node reports
  `0 events dropped`, so the spend is invisible from the machine somebody logs into; and the
  resumes arrive in **bursts of five a second**. Measured at 29 firings and 50 agent launches. The
  fix needed no gossip about a rule after all: `origin` already travels and `runs.rule` already
  does not, which between them answer "is this one of *mine*".
* Not a `RecoveryPolicy` knob. The policy answers *how eagerly*, and this answers *whether the
  question still applies* — a resume nobody has any use for is not a tuning value.
