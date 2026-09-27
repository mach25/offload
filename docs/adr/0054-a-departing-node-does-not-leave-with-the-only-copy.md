# ADR-0054: A departing node pushes the last copy of a checkpoint before it decides what to strand

**Status:** accepted · 2026-09-05 · amends ADR-0043 (closes its third residual) · builds on ADR-0016

## Context

ADR-0016 makes replication **spawned and unwaited** on purpose: the agent's next turn must not
wait for a slow peer. That is right everywhere a next turn exists.

ADR-0043 left a residual saying replication is never forced on the way out, and asked for a
measurement first. Two sessions supplied it, and the answer moved the question rather than
closing it:

* **With a peer, durability is never a race.** 1.3–47.3 ms for blobs up to 11.5 MB, **30 of 30**
  replicated, the peer's blob store byte-identical to the holder's. About 244 MB/s on loopback.
* **`Supervisor::replicate` has exactly one caller** — `checkpoint` — with no retry and no
  background pass. A capture taken while no peer was reachable stays `here only` **for ever**: four
  such checkpoints were still `here only` after a peer came up and both nodes called each other
  `alive`, and only the *next* capture replicated.

So the residual's "only when there was never a peer" is really **"whenever no peer was reachable
at the last capture"** — and two kinds of run have no next capture to fix it. A **failed** run has
no next turn boundary. A **`Pending`** one released by `offload checkpoint` has no agent at all.

Meanwhile `decide_recovery` asks `fleet_could_start`, which asks `Checkpoint::is_durable`, to
choose between handing a failed run to the fleet and stranding it with `NoCopyElsewhere` — *"this
node is leaving and its conversation exists nowhere else, so nobody could continue it"*. That
sentence is honest, and until now it was decided by a fact nothing on the way out had ever tried
to improve.

### Measured

Two daemons, the same staging on both arms: alpha alone, a run that fails at turn 4 with its only
checkpoint here, **then** bravo starts, both nodes `alive`, then `offload drain` on alpha.

| | before | after |
| --- | --- | --- |
| `offload drain` says | `nothing to hand over` | `copied 1 checkpoint(s) nobody else had to a peer` · `1 failed run(s) given back to the fleet` |
| the run ends up | `failed`, `here only`, on the machine that is leaving | **`running` on bravo at turn 5**, `replicated` |
| bravo's blob store | 0 | 3 |

The work went from dying with the laptop to continuing on the desktop, which is the sentence on
the front page of this repository.

## Decision

1. **`Supervisor::push_last_copies` runs inside `depart`, before `hand_back_failed_runs`.** It is
   the one place the ADR-0016 trade-off inverts: there is no next turn to protect, and what is at
   stake is the only copy of a conversation. Before the failed-run pass, because that pass reads
   `is_durable` and this is what can still change the answer.
2. **Every run with a `resumable_checkpoint` that is not durable**, not only the failed ones. A
   `Pending` run released by `offload checkpoint` strands the same way, one door over: a peer wins
   it later and fetches from a node that has gone. `resumable_checkpoint` is the filter — its arms
   are written out, so a `Completed` or `Cancelled` capture is skipped and a new terminal state has
   to decide.
3. **Bounded as a whole, not per run.** `LAST_COPY_BUDGET` is 30 seconds for the pass. A drain has
   a deadline and this runs before it; the honest failure is *some* runs unattempted with a line
   saying so, never a departure that hangs.
4. **Four numbers, not one.** `LastCopies { pushed, refused, unattempted, already }`. `refused` is
   a fleet that would not take a copy and `unattempted` is this node running out of time — they
   need opposite fixes, and a drain's whole job is to say what it could not do. `already` exists so
   "nothing was pushed" can be told from "nothing needed pushing".
5. **Every failure is soft.** A run whose copy could not be pushed is exactly as stranded as it was
   before this existed, and says so through the same `NoCopyElsewhere` sentence. This pass can
   improve the outcome and can never make it worse.
6. **`DrainStep::PushedLastCopies`, said only when something was at risk.** A drain that reports
   every pass it makes is a drain nobody reads; `stranded` is the number that decides whether
   closing the lid loses work, and the line points at `offload ps --all`'s SAFE column.

### Why not the alternatives

* **A background retry pass, so `here only` heals on its own.** The better fix in the abstract, and
  it is a tick, a schedule and a backoff policy for a condition that only *matters* at one moment.
  It also spends bandwidth on a laptop that is not going anywhere. Worth revisiting if `here only`
  is ever observed lasting for reasons other than "there was no peer"; the departure is where the
  loss is.
* **Replicate on every `Failed` transition.** Closer to the event, and wrong twice: a run that
  fails while the fleet is unreachable is exactly the case this is about, and a run that fails and
  is resumed here five seconds later has paid for a copy nobody needed.
* **Make `checkpoint`'s replication awaited.** It is ADR-0016, and it would put the network in
  front of every turn boundary of every run to fix a problem that appears once per departure.
* **Extend `drain_deadline_secs` to cover this.** Two timeouts that sound alike is how ADR-0035's
  residual happened. That one bounds the wait for *turn boundaries*; this bounds bytes. Separate
  numbers, and this one deliberately not configurable.

## Consequences

* A failed or pending run whose only copy was on a departing node now leaves with the fleet
  instead of with the machine. `offload drain` says which, and says what it could not save.
* A departure can take up to 30 seconds longer than before, and only when something is genuinely
  at risk — `already` is the ordinary case and costs one `is_durable` per run.
* ADR-0043's third residual is closed. Its *first* two are not, and are unaffected: a node that
  **dies** rather than departs still strands its failed runs, because it says nothing and nobody
  can see its attendance.
* **Residual: this is the only forced replication anywhere.** A checkpoint still becomes `here
  only` the moment it is taken with no peer reachable, and stays that way until the node departs.
  A machine that is simply switched off — the case ADR-0043's first residual is about — never
  reaches this pass. What that means in practice is that `here only` in `offload ps` is a warning
  with a *deadline*, not a permanent state, and only for a graceful exit.
* **Residual: the budget is a constant chosen against loopback numbers.** 244 MB/s is not a wifi
  figure, and nobody has drained a node holding several large checkpoints over a real link. The
  failure mode if it is too small is visible and honest — `stranded` counts it — which is why a
  constant was preferred to a knob nobody would know how to set.
