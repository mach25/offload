# ADR-0014: A submission is accepted by a node, or refused to your face

**Status:** accepted · 2026-07-27 · settles roadmap open question #3, including its
starvation half once ADR-0013 reads an unspecified deadline as the moment of submission

## Context

The point of the project is to stop babysitting work. You fire off a run, walk away from the
keyboard, and it happens. Everything about migration and checkpointing serves that, and it is
all undone by an ambiguity at the very first step: what does it mean when `offload run`
returns successfully?

Under the phase 4 design as it stood, it means *the run exists and is `Pending`*. Some node
may bid on it. Or none may, in which case it sits — correctly, durably, and invisibly —
until somebody thinks to look. That is a system that still needs watching, and it fails in the
worst possible way: silently, at the moment the human stops paying attention, which is
precisely the moment they were promised they could.

Two things make this fixable now that were not available before:

- **Nodes can commit without starting** (ADR-0006). A busy-but-capable node can take a run
  and hold it, so "accepted" no longer has to mean "running".
- **Refusals are structured and complete** (ADR-0006's `NoBid`, ADR-0013's `Busy`). A fleet
  that will not take a run can say why, per node, in one round.

There is also a precedent already in the code, and it turns out to be right. On a single node
`submit()` refuses at capacity — `SubmitError::AtCapacity` — rather than queueing. Phase 4
was about to loosen that into accept-and-pend. The opposite is correct: keep the strictness
and let *commitment* be the thing that relaxes it.

## Decision

**`offload run` returns one of two things: the node that has taken the run, or the reason
nobody would.** There is no third outcome, and in particular there is no success that means
"filed away, good luck".

Submission runs one bounded bid round synchronously. It is short — the score-proportional
announce delay already bounds it to well under a second in the ordinary case — and it has to
happen anyway, because a submission is not durable until it has been replicated off the
submitting node (ADR-0006).

```
$ offload run --repo … --by 08:00 "refactor the parser"
run 019f9fb2 — accepted by desktop, starting in ~20m (building)

$ offload run --repo … "review this diff"
refused: no node will take this run
  desktop   busy, free in ~40m — past your deadline
  laptop    battery 22%, policy floor is 40%
  phone     no agent installed
  (use --queue to leave it pending anyway)
```

**Acceptance means a node holds it under a lease**, whether or not the agent has started. That
is a commitment with a fencing token behind it, not a hope.

**`--queue` is the opt-in for the old behaviour.** "I know nobody can take it now; leave it
pending and let somebody pick it up when things change." Legitimate — the desktop is off and
will be on in the morning — but it is the case where you *have* chosen to check back, so it
is the case you have to ask for.

**Acceptance creates an obligation to report.** This is the half that makes walking away safe.
If the committing node later releases the run — it got busier, it can no longer make the
deadline — or dies and nothing else will take it, the run does **not** quietly revert to
`Pending`. It re-enters bidding immediately, and if nothing commits, that is a notification on
the delivery plane (ADR-0010): *"nobody can take run 019f9fb2 any more: desktop released it,
laptop is on battery"*. A promise that was made and cannot be kept is news; a run that was
never accepted at all is a refusal you already saw.

**What the fleet may not do is decide on its own to stop trying.** No timeout cancels an
accepted run, because a timeout is just a babysitting interval with extra steps — it converts
"check on it yourself" into "we checked on your behalf and threw it away". A run that cannot
be placed is reported, not discarded.

## Consequences

Good:

- **Walking away is safe**, which is the entire product claim. Either a node has your work or
  you found out while you were still standing there.
- The failure that used to be silent — nothing will take this — now happens in the one place
  where a human is guaranteed to be present.
- `--queue` makes the deliberate case explicit rather than making it the default that everyone
  gets by accident.
- It generalises the single-node behaviour rather than replacing it. `AtCapacity` at submit was
  the right instinct; the fleet version simply has more nodes to ask first.

Bad, and worth being clear-eyed about:

- **Submission now blocks on a bid round.** Sub-second in the ordinary case, but it is no
  longer a local operation, and a script submitting fifty runs pays it fifty times.
- **A slow or partitioned fleet can refuse work it could have done.** A node that was mid-GC
  or on a flaky link misses the round and its capability with it. The wait is bounded rather
  than generous, so this is a real false-negative rate, and `--queue` is the escape hatch
  rather than a fix.
- **"Accepted" invites more confidence than it earns.** A commitment is a lease, not a
  guarantee: the node can still die, and the run can still end up unplaceable at 03:00. The
  obligation to report is what keeps that honest, which means this decision leans on the
  delivery plane existing — and it does not yet.
- **A committing node's start estimate is a guess**, so "starting in ~20m" will sometimes be
  wrong by a lot. Printing it is still better than printing nothing, but it should read as an
  estimate and not a schedule.

## Alternatives

**Accept and leave it pending — the phase 4 design as written.** Simplest, and the run is
never lost. Rejected because it moves the work of noticing onto the human at exactly the point
the project promises to take it off them, and because "pending" and "nobody will ever take
this" look identical in `offload ps`.

**Accept, then fail after a timeout with `Unschedulable`.** Bounds the ambiguity. Rejected:
picking the timeout is picking how long the user should have waited before checking, which is
the babysitting interval wearing a disguise — and it destroys work rather than reporting a
problem. If the fleet cannot place a run, saying so is strictly better than deleting it.

**Ask interactively at submit** — "nobody can take this now, queue it? [y/N]". Rejected: it
puts a prompt in the path of every script, and `--queue` expresses the same choice without
requiring anyone to be at the terminal.

**Always queue, and rely on notifications to tell you nothing happened.** Coherent, and
depends entirely on the delivery plane, which does not exist yet. Worth revisiting once it
does — but the refusal-at-submit path costs nothing then either, and being told immediately
beats being told eventually.
