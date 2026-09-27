# ADR-0041: A drain's deadline bounds the wait, not the intent

**Status:** accepted · 2026-08-29 · extends ADR-0035 · supersedes nothing

## Context

ADR-0034 and ADR-0035 between them settled what `offload drain` *does* and what it *says*. Neither
asked what happens to a run after the pass gives up on it, because the answer looked obvious: the
run is "left to its lease", and the lease is the safety net — if the node goes away, the ordinary
orphan path (ADR-0007) moves the run from its last checkpoint.

That is true of the shutdown path and false of the operator's. The two halves that make it false
were built one session apart and each was walked alone:

* A drain asks every run it holds to checkpoint at its next turn boundary — the same
  `Supervisor::request_checkpoint` that `offload checkpoint` calls, arming the same flag.
* That flag is **read without clearing** (session thirty-one): a request is a standing instruction
  until it is honoured, so a capture that fails does not consume it.

So the drain's request outlives the drain's deadline. A run still mid-turn when the deadline
expires goes on to finish its turn, capture, and **release itself** into the pool — minutes after
the drain reported it as still here.

Nothing then offers it. `offload_core::supervise` returns `Bystanding::Unheld` for a checkpointed
`Pending` run, on a premise written in its own comment:

> a checkpointed run sitting `Pending` was parked by a human with `offload checkpoint`, and
> picking it up on their behalf is exactly the "we decided for you" this project refuses to do.

Correct, and true of every way a run reaches that state except this one.

### Measured

Two daemons on one machine, a fake agent with a thirty-second first turn, `drain_deadline_secs =
10`:

```
10:40:49  offload drain
10:40:59    nothing could be handed over; 1 run(s) still here
            still mid-turn at the drain deadline; leaving it to the lease
10:41:19  the turn ends: captured, released → pending
…and nothing else happens, for as long as anybody watches.
```

with, four minutes later:

```
state       pending — waiting for a node since 1m19s ago
arbiter     node-a (this node)
            nobody holds it, so it is waiting for a bid round rather than for a hold-down
checkpoint  turn 1, copied to node-b

what each node says about taking it, asked just now:
  → node-b       bid 57, starting now
```

The checkpoint was already replicated to the peer, the peer was bidding to take it, and the run
says it is waiting for a bid round. The round is never held. A drained laptop is left holding a
stopped run that every other machine in the fleet was willing to continue.

The operator was told the run was still here mid-turn, which was true for twenty seconds and then
quietly stopped being true.

## Decision

**A drain's deadline bounds how long somebody waits, not what the node intends.** A run still
mid-turn when it expires is one this node still means to hand over, so it hands it over when the
run gets there.

### 1. The drain keeps what it could not finish

`Mesh::owed` — node-local, in memory, beside the departure flag and for the same reason: a drain is
a departure (ADR-0034), and a daemon that restarts has departed for real, at which point its runs
are the orphan path's and nothing here needs remembering.

`Mesh::supervise` settles the debt each tick: a run released into the pool with nobody holding it
gets the same bid round the drain itself runs, from the same position — this node does not bid for
a run it is draining — on the same backoff a reassignment uses. Called from inside `supervise`
rather than beside it in the tick loop, so a node that supervises cannot fail to finish what it
started.

The decision is `fn owed(run, me) -> Owed::{NotYet, Offer, Done}`, a pure function of the record,
because the pass that acts on it needs a `Mesh`, a `Cluster` and peers to talk to, and the decision
needs neither. `me` is a parameter because **this node is a holder like any other**: asking only
whether a run *has* a holder answers "still ours, mid-turn" and "somebody else took it" identically,
and they are opposite instructions. Caught by the guard, not by review.

### 2. `Lifespan`, because only one of the two callers can keep the promise

`depart` takes `Lifespan::{StaysUp, Exits}`. An operator's `offload drain` stops the node accepting
and leaves it running, so the boundary arrives with somebody still there to act on it. A signal is
the process going away — `main` stops every agent the drain could not move as soon as the pass
returns — so there is no later boundary and nothing to promise.

A parameter rather than a read of `Report::silent()`, which is the other thing that differs between
the two callers. "Nobody is listening" and "this process is about to stop existing" are two facts,
and inferring one from the other is how the departure flag ended up in the wrong place twice
(ADR-0035 §3).

### 3. `left` was two outcomes, so it is now two numbers

`Drained::later` beside `moved`, `left` and `finished`. `left` keeps its meaning — final: nobody
would take it — and `later` means not yet. The CLI's advice for `left` (*"`offload ps` and `offload
nodes` say which"*) cannot answer a run that is merely mid-turn, because what it waits on is a turn
boundary that has not happened.

This is ADR-0035 §3's finding one bucket further on, and the same rule: **when a count means more
than one thing, a single sentence about it is right for at most one of them.**

## Consequences

- Measured on the same harness, three passes out of three: the drain now says

  ```
    1 run(s) still mid-turn — this node hands each over at its next
    turn boundary, without being drained again.
  ```

  and the run moves — `handed over at the boundary the drain could not wait for name=node-b` —
  nineteen seconds after the drain returned, continuing to completion on the peer.
- The `SIGTERM` path is unchanged and was watched: `still mid-turn at the drain deadline; leaving
  it to the lease`, `moved=0 left=1 finished=0`, agents stopped, and node-b orphaning and
  reassigning the run a second later exactly as before.
- No wire or schema change. `Response::Drained::later` is `#[serde(default)]`, which matters only
  for a CLI and a daemon of different builds sharing a socket — the same shape ADR-0035 used for
  `finished`.
- A drained node keeps offering an owed run until somebody takes it, on the reassignment backoff.
  It is a node on its way out; the fleet having no room now is not an answer about later.

## What this deliberately leaves

**The debt does not survive a restart**, and should not: it lives where the departure flag lives,
and a daemon that restarts has departed for real. What that costs is a narrow window — `offload
drain`, then the daemon stopped before the run reaches its boundary — in which the run is left
`Pending` and parked exactly as it was before this ADR. The alternative is a fact on the run itself,
which means a gossiped field, an owner, and an arbiter for it (ADR-0005); that is worth doing when
something else needs it, and inventing it for this window alone would be the heavier half of a
trade nobody has had to make yet.

**ADR-0035's residual is untouched.** The drain's deadline still does not bound the question's
patience, so a run stopped on an `--ask` raised after the drain began still outlives it. It now
outlives it *into a handover* rather than into a parked run, which makes the residual smaller and
does not settle it.
