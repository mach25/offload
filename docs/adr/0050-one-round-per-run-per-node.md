# ADR-0050: One placement round per run, per node

**Status:** accepted · 2026-08-30 · extends ADR-0006 · closes the hole ADR-0042 left open

## Context

An epoch is the fencing token for **one grant**, and it is monotonic *per arbiter*. Three things
make that enough (`docs/pitfalls/fencing-and-epochs.md`): a grant spends a token whether or not it
is confirmed, two live grants at one epoch are ordered by lowest holder id, and a holder whose
record names somebody else at its own epoch stops its agent.

All three assume the two grants came from **different** arbiters. An arbiter is a node, and a node
that runs two rounds at once for one run reads `run.epoch` from two copies that both say *N* and
grants *N+1* twice. Nothing downstream can tell those legs apart: `fence`, `holds` and `describes`
all wave both through, because neither of them is stale.

ADR-0042 removed the drain's in-memory debt so that exactly one node — the run's arbiter — offers
an unheld run, and the loop that does it says so in its own comment: *this is the only offerer,
which is the property that matters*. That sentence is true of nodes and false of the node it is
written on. A departing node runs **two** passes:

* `Mesh::drain` releases the run and calls `Cluster::place` itself.
* `Mesh::supervise`'s tick snapshots the view, sees the same run `Pending { let_go_by }` with no
  holder, and calls `Cluster::place` too.

Same node, same arbiter, two rounds, one run.

This is a shape this repository has already paid for once, one layer down. *"Two legs assigned
from one loaded copy get the same epoch, so no fence can tell them apart"* was written about
`Supervisor::start_run`, and what fixed it was read-decide-write under one lock. The round is the
same transition one layer up — it reads an epoch, decides, and writes — and it had no lock at all.

### Measured

Found while walking something else (session fifty-one), on two daemons with a fake agent: submit
to alpha while it is the only node, start bravo, wait for the checkpoint to replicate, `offload
drain` alpha.

```
handed over                run_id=… name=bravo         (Mesh::drain's own round)
an unheld run was placed   run_id=… name=bravo         (Mesh::supervise's tick)   1.4ms later
```

Two `granted to <bravo> at epoch 3` rows in `offload audit`, two `cloning repo mirror` lines on
bravo, and two concurrent `git worktree add` for one run.

What stopped *that* pass being two agents was **git**, not a fence: the second `worktree add`
failed with `already used by worktree at …`. And the outcome was worse than a coin flip, because
the leg that collided wrote `Failed` first — so the healthy leg, workspace ready and transcript
restored, was fenced out by the loser's write, and a clean migration ended as a run `failed` with a
raw git error on it.

Reproduced again on the build this ADR changes, to have a control rather than a memory.
Forty-eight passes of that staging — sixteen with the supervise tick at its usual one second and
thirty-two at `probe_interval_ms = 200` — of which two aborted on machine load and **one hit**:

```
alpha  19:35:58.293886  handed over               run_id=… name=bravo
alpha  19:35:58.295668  an unheld run was placed  run_id=… name=bravo   +1.78ms
audit  granted to 3e453757 at epoch 3
       granted to 3e453757 at epoch 3
       granted to 3e3b8523 at epoch 1
```

Worth more than the count: **that hit was silent.** Bravo committed rather than starting (ADR-0006:
*starting when one of the 2 runs ahead of it finishes*), so there was one clone, no second
`worktree add`, no error in any log, and nothing but the audit rows to say a token had been spent
twice. The git collision that made this visible at all is incidental to it.

### Why not narrow it

A re-read of the run before `place` is this codebase's favourite idiom and it is the wrong tool
here. Two rounds that overlap exactly still both read the same copy; a re-read makes the window
smaller, not empty. The same file says it plainly about the sweep: *a fence after the effect is not
a fence, and when a guard and the thing it guards are separated by an `await`, the guard is on the
wrong side of it*. A whole bid round is an `await`.

## Decision

### 1. A placement round for a run is exclusive on the node running it

`Cluster::place` takes a per-run rendezvous for the whole of the round and holds it until the round
has spent whatever tokens it is going to spend. One round per run per node, at all times.

Keyed by **run**, because that is the scope of the invariant. Two rounds for two runs at once is
what a fleet is for, and a node-wide placement lock would serialise a fleet's worth of work behind
one slow peer's timeout.

### 2. The guard is inside `place`, not in its callers

There are six callers today — a submission, the drain's unstarted pass, the drain's main pass, the
supervise tick's `Place` and `Reassign`, and the commitment hand-back — and the two that raced were
not the two anybody would have guessed. Six call-site checks are six things to remember and the
seventh caller will not, which is the argument that already put `Supervisor::holds` in the writer
rather than at each call site. The round is where the token is spent, so the round is where the
exclusion goes; the storm harness and the submission path get it without anybody auditing them.

### 3. The second arrival waits — it does not skip

The drain's pass cannot yield. Under `Lifespan::Exits`, `main` stops every agent the moment
`depart` returns, so a drain that stood aside for "the tick, later" would be handing the run to a
tick that never runs again. That is precisely the stranding ADR-0042 exists to have ended.

So the loser of the race waits for the winner rather than returning early, and no caller has to
know which of the two it is.

### 4. …and then stands down, reporting what the round it waited for decided

Running the round *after* the wait would be the bug again, one lock later: a second round asking
the same fleet the same question about a copy of the run the first round has already moved past.

What the waiter returns is the first round's own `Placement`. That is the honest answer rather than
a convenient one — this node *did* place the run, a moment ago, and every caller wants exactly what
that round decided. The drain in particular counts a handover it did not itself perform, which is
right: `offload drain` reports what became of the run, not which of the daemon's tasks did it.
Telling it "refused" would make the command say nobody took a run that had just left, which is the
fourth thing that command has been caught saying untruthfully.

### 5. The rendezvous lives only as long as somebody holds it

Created on the way in and dropped by the last round out, counted under the same synchronous lock
that hands it out. It is a rendezvous, not a registry: it does not grow with the number of runs the
node has ever seen, and — the half that matters — a stored outcome can never be handed to a round
that arrives minutes later, which would be this node reporting a decision it did not take about a
fleet that has since changed.

## Consequences

* Two grants at one epoch from one node are now impossible by construction rather than by luck,
  which is what the epoch was always documented to guarantee.
* `Mesh::may_retry` stays exactly as it is. It answers a different question — *has this run waited
  long enough for another attempt* — which is a rate limit across time, not exclusion within an
  instant. Neither substitutes for the other, and the drain deliberately does not consult it.
* A round that hangs delays the next round **for that run only**, bounded by the same window every
  round is bounded by (ADR-0014: an operator is standing at the keyboard).
* No deadlock is reachable: `place` → `hand_over` → `offer` → `accept` or a peer request, and
  nothing on that path re-enters `place` for the same run.
* `offload-cluster/tests/storm.rs` and `server::place` are covered without either being changed.
* Walked on two daemons, thirty-two passes of the staging above: no double grant, and thirty clean
  migrations. The exclusion costs a handover nothing measurable, which is worth checking for a lock
  held across a network round.

## What this deliberately leaves

* **Exclusion is per node, because an arbiter is a node.** Two nodes both believing they arbitrate
  one run — the successor case, where "gone" is not agreed — can still issue one epoch twice, and a
  node-local lock cannot reach it. That is ADR-0007's `arbiter_for` and the pre-existing weakness
  `fencing-and-epochs` already names as *"an epoch is monotonic per arbiter, which is weaker than a
  total order"*. Unchanged here, and the three properties above are what carry it.
* **`git worktree remove --force` still spans an await with no exclusion** (the residual `cleanup`
  and `reclaim_departed_checkouts` share). It wants the same *kind* of primitive keyed by checkout
  rather than by round, and sharing this one would be naming two invariants with one lock.
  **Built in ADR-0052**, keyed by checkout and separate from this lock, on those terms.
