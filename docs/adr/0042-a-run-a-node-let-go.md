# ADR-0042: A run a node let go is not a run a person parked

**Status:** accepted · 2026-08-30 · extends ADR-0041 · supersedes ADR-0041 §1's mechanism

## Context

`Pending` with a checkpoint beside it is two facts with one spelling.

* A person typed `offload checkpoint`. They mean to come back for the run, and picking it up on
  their behalf is the "we decided for you" this project refuses (ADR-0014).
* A node **let it go** on its way out — a drain, a shutdown, a commitment it could no longer
  keep. Nobody is coming back for it, and carrying on with it somewhere else is the promise the
  product is named for.

`offload_core::supervise` could see only the first, and said so in its own comment. Everything
downstream inherited that reading: the run sits `Pending`, no node offers it, `offload explain`
says *"it is parked with its checkpoint, and waits for `offload resume` rather than for a bid
round"* — about a run nobody parked — and the fleet that would continue it is one round away and
is never asked.

ADR-0041 fixed the case it could reach with the machinery that existed: a node-local set of runs
the drain still owed, re-offered on the reassignment backoff. That was the right size for the
evidence at the time and it deliberately left three cases open, all of them named in its own
"what this deliberately leaves":

1. **A drained node restarted before the fleet has room** strands the run exactly as before. The
   debt lives beside the departure flag, and a daemon that restarts has departed for real.
2. **`Lifespan::Exits` never records one at all** — a `SIGTERM`, which is what closing a laptop
   looks like. `main` stops the agents as soon as the drain returns, so a departing process has
   no later to promise and the run was left `Pending` and parked.
3. **ADR-0041's own residual**, which is the same gap said a third way.

Three justifications rather than one, which is the bar the last two handoffs set for building
the thing they each declined to build: a fact on the run itself, which under ADR-0005 means a
gossiped field with an owner and an arbiter.

### Measured

One node, a fake agent, `drain_deadline_secs = 10`. The run reaches its boundary *inside* the
deadline, is released, and is refused because there is nobody else — after which the daemon is
restarted, which is the whole of the case.

On the build this replaces, through the `SIGTERM` door:

```
07:07:14  kill -TERM        →  draining runs=1
07:07:15                       nobody would take it right now   refusals=1
07:07:17  the daemon exits
07:07:17  restarted — idle, accepting, holding the checkpoint at turn 4
07:08:32  pending. `offload explain`:
            state    pending — waiting for a node since 1m17s ago
                     nobody holds it: it is parked with its checkpoint, and waits
                     for `offload resume` rather than for a bid round
```

for as long as anybody watched, about a run nobody had parked.

## Decision

**The release states who gave the run up, and the fleet reads it.**

### 1. The fact lives inside `RunState::Pending`

`Pending { since, let_go_by: Option<NodeId> }`. Inside the state rather than beside it on the
record, and that is the whole of its merge story: the state is the holder's to write and rides
on whichever record `merge_run` settles on, so there is no new owner to name and no new
arbitration rule to get wrong (ADR-0005). Two properties fall out rather than being arranged:

* **It cannot go stale.** Every way out of `Pending` replaces the state, and every way in states
  it afresh — `assign` clears it by construction, `reopen` writes `None` because nobody let a
  failed run go, and a run released twice says who released it the second time.
* **`None` is the conservative reading**, and it is today's behaviour: a fresh submission, a
  reopened failure, a run a person parked, and a record from a build that predates the field all
  read the same and are all left for a person. The safe way to be wrong about who is coming back
  for a run is to wait for them.

`GivenUp::{Parked, LetGo}` is how it gets there. The same capture, at the same boundary, writing
the same blobs, means two different things depending on who asked — and the only moment that
knows is the request, several minutes and one turn earlier. So it travels with the request:
`Supervisor::request_checkpoint` takes it, `LiveRun::checkpoint_requested` holds it instead of a
`bool`, and `Run::checkpointed` writes it. `Run::release` — a commitment handed back — needs no
parameter, because there is no operator command that does that.

**A departure is not downgraded by a person asking afterwards.** A drain waits minutes and
somebody can type `offload checkpoint` inside that window; the node is leaving either way, so
`LetGo` is the fact that survives. This needed `Run::request_checkpoint` to become idempotent
from `Checkpointing` — it used to refuse a second request, which was harmless while a request
was only "somebody wants a checkpoint" and stopped being so the moment it carried *who*: a drain
reaching a run a person had already parked was turned away, and the run then released as parked
on a machine whose owner had said in as many words that it was leaving.

### 2. `supervise` offers it, and says why

The `Unheld` branch asks `run.let_go_by()` first, because it is the more specific fact.
`Supervision::Place` now carries `Offering::{Queued, LetGo { by }}` — the reason travels with the
decision rather than being worked out a second time by whoever reports it, which is the rule
`docs/pitfalls/reports-and-cli.md` is largely about and the mistake this ADR's own predecessor
made one report further on.

**Not conditioned on `--queue`**, and it must not be. `--queue` answers "may a *submission* be
filed away rather than accepted to my face" (ADR-0014), asked of a run that has never run. This
run *was* accepted; a node has since stopped intending to run it.

### 3. …which supersedes ADR-0041's in-memory debt

`Mesh::owed`, `Owed`, `Unplaced`, `finish_owed_handovers` and the `owed` channel into `offload
explain` are gone. The debt existed because the fact had nowhere to live; it has one now, and
keeping both would be worse than either.

**Because two offerers is the thing the epoch cannot fix.** The debt made a *departing* node
offer its own run while the run's arbiter watched — safe only because the arbiter could not see
the fact and therefore did nothing. Teaching the arbiter to see it while leaving the debt in
place would have put two nodes in a bid round for one run, which `place`'s own comment calls the
thing the epoch exists to make impossible: two arbiters both bump `next()` from the same number,
so the grants cannot be ordered and `merge_run`'s equal-epoch tiebreak is left holding a decision
it is only meant to back up.

Nothing is lost with it. A drained node's own `evaluate` answers `draining`, which is exactly
what `finish_owed_handovers` passed by hand, and the offer is rate-limited by the same
`may_retry` backoff — so on the node that is usually both drainer and arbiter the behaviour is
the one ADR-0041 measured, one tick later at most. Where the two differ, the record is right: the
run's arbiter is the single node entitled to hold the round, and it can be somewhere else, and it
can be a daemon that has not started yet.

### 4. `Drained::later` was two outcomes again, so it is two numbers again

`later` keeps the mid-turn runs; `pooled` is new and means released into the pool, nobody had
room. This is ADR-0041 §3's finding one bucket further on, and it is the third time this struct
has needed the split — but the question it now answers is the one somebody typing `offload drain`
is actually asking. A mid-turn run needs this process to reach the boundary it will be handed
over at. A pooled one does not need this daemon at all.

### 5. Wire v24

Strictly the "optional field an older node ignores" case, and a bump anyway — for the reason
`Run::origin` was *not* one. That field is immutable and identical in every copy, so a build that
drops it can contradict nobody. This one is mutable within a state peers also gossip: a v23 node
decoding the release, relaying it, and winning an equal-epoch tiebreak re-states the run as
`Pending` with no `let_go_by`, and what that erases is the only thing that makes the run
placeable. Failing towards keeping is not available here — the safe-sounding default *is* the
stranded case.

No schema change: the record is JSON in `run_json`, the `state` column is `state.name()`, and an
existing row decodes to `None`, which is what it has always meant.

## Consequences

Same harness, same sequence, on the fix:

```
07:05:11  kill -TERM        →  draining runs=1
07:05:11                       nobody would take it right now; its arbiter keeps offering it
07:05:13  the daemon exits
07:05:18  restarted
07:05:19  an unheld run was placed  offering=LetGo { by: node-a }  starting=starting now
          …resumed at turn 3, from the checkpoint left at turn 2
```

and through the `offload drain` door, with the decision reverted and restored on the same
daemon — a one-line A/B rather than an argument:

| | released | daemon restarted | outcome |
| --- | --- | --- | --- |
| reverted | 07:03:36 | 07:03:45 | still `pending` at 07:04:35, "waits for `offload resume`" |
| fixed | 07:02:36 | 07:02:52 | placed 07:02:53, running at turn 5 |

- `offload explain` says it from the verdict rather than from a side channel: *"nobody holds it:
  node-a let it go rather than a person parking it, so this node offers it to the fleet until
  somebody takes it"*, with the state line naming the node — because "which machine dropped this"
  is the next question and the record is the only thing that can answer it.
- A run **still mid-turn** when a process exits is unchanged and is not this ADR's case: the agent
  is stopped, no release happens, and the run is the orphan path's (ADR-0007).
- The drain's own pass keeps nothing. Its request is a standing instruction until honoured, the
  release it produces states the fact, and whoever arbitrates picks it up.

## What this deliberately leaves

**A commitment handed back is now stamped too**, which fixes a hole one branch over that nobody
had walked: `review_commitment`'s `GiveBack` releases a run that was never started, and a
non-queued run whose re-placement was then refused had exactly the same nothing-offers-it-again
future. The take-back path made it rare rather than impossible. It is not measured here, and it
is now the same fact by construction rather than a second mechanism.

**Who picks up a run that *failed* on a departing node** is still unanswered. `supervise` treats
failed as terminal and nobody bids on it, and this field is what such a decision would need —
but it is a different decision, about a state nothing here reopens on its own.
