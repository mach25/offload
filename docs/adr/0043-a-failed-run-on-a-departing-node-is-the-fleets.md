# ADR-0043: A failed run on a departing node is the fleet's

**Status:** accepted · 2026-08-30 · extends ADR-0042 · supersedes ADR-0034 §1's escalation

## Context

ADR-0042 ends on the question this answers, and three handoffs before it said the same thing:

> **Who picks up a run that *failed* on a departing node** is still unanswered. `supervise`
> treats failed as terminal and nobody bids on it, and this field is what such a decision would
> need — but it is a different decision, about a state nothing here reopens on its own.

The last clause is the part that was wrong, and noticing it is most of this ADR. A failed run is
reopened without a person **every day**: that is what ADR-0013's auto-resume is, and
`Supervisor::resume` calls `Run::reopen` on the recovery tick's behalf. What has never happened
is reopening one for somebody *else* to run.

So the shape of the gap. ADR-0034 §1 found the fourth path `stop_accepting` had never reached —
auto-resume spawning agents on a drained laptop — and closed it by making `departing` the first
question `decide_recovery` asks and `Escalation::NodeIsDeparting` its whole answer. That was
right about the agent and it ended the question one step early:

* **"This node will not start an agent" and "nobody should" are two facts.** A drain is the one
  moment they come apart. The run is resumable, unattended, inside its retry budget, past its
  backoff — every other answer in that function says *resume* — and the only objection is a
  machine that is going away.
* **So the run was left `Failed`, which is where it stayed.** Not "until somebody looked":
  `offload_core::supervise` returns `Bystander(Terminal)` for a failed run, so no arbiter offers
  it and no node bids on it, and the recovery entry that held the reason is in memory and dies
  with the daemon. Its checkpoint, meanwhile, is *replicated* — on the peer that would have
  taken it.

ADR-0034's own doc comment stated the limit and the reason: "handing a *failed* run to the fleet
is a decision nothing here has made, and the safe direction for a departing node is to start
nothing and say so." Two phases on, the machinery that decision needed exists (ADR-0042), and
the safe direction is still to start nothing — which is not the same as to say nothing.

### Measured

Two daemons on one machine, one fleet, a fake agent that fails after two turns. The run is
submitted to node-a without `--follow`, so it is unattended when it breaks; its checkpoint
replicates to node-b, which is idle and accepting.

On the build this replaces, through the `offload drain` door:

```
08:49:52  offload drain    →  "nothing to hand over"
08:50:23  offload explain  →  state    failed — failed 31.1s ago
                              recovery left for a person: this node is draining, so it
                                       will not start an agent for it
                              checkpoint turn 2, copied to fedora
08:51:24  node-b: 01a0516e4b1f  failed  2 turns  replicated
08:51:50  both daemons restarted →  still failed, and now with no reason on screen at all,
                                    the recovery entry having gone with the process
```

Three things in that block are worth reading together: the drain says it did nothing about a run
it had just decided about, the run's own explanation names the drain as the cause of something
the drain cannot fix, and a peer holding a copy of the conversation is sitting idle throughout.

## Decision

**A departing node hands back the run it would have picked up itself.**

### 1. `departing` wraps the run's verdict rather than replacing it

`decide_recovery` computes what the *run* says first — would a retry work, and is it ours to make
— and applies this node's departure around it:

* `Resume` or `Wait` → **`Recovery::LetGo`**. `Wait` included: a backoff is a promise to try
  again in thirty seconds, and a departing node has no thirty seconds to promise. A refusal now
  is not an answer about later (ADR-0041), and neither is a wait.
* anything else → **the run's own reason**, unchanged.

**The guarantee ADR-0034 bought is preserved structurally, not by ordering.** `Resume` and `Wait`
are the only two answers that spawn an agent and neither leaves that match. There is exactly one
function that can say `Resume` and exactly one place that decides a departing node may not hear
it, and a property test sweeps every shape of failed run against every `Circumstances` a
departing node can present to assert that nothing falls through — which is a better check than
the old comment about being "above everything", because the old one was a claim about line order.

`Escalation::NodeIsDeparting` is retired with it. It answered every departing case with a fact
about the *node*, which was right while there was one answer and became a report naming the wrong
cause the moment there were three: a run that had spent its turn limit was told the drain was
what stopped it, which sends somebody to restart a daemon. A report that names a cause has to be
told when it stops being the cause.

### 2. The effect is a transition, not a start — which is where the fact lives

`Run::let_go(node, now)`: `Failed` → `Pending { since, let_go_by: Some(node) }`, epoch bumped.
The other half of `Run::reopen`, and the difference is who is expected to run it next — `reopen`
is a run picked back up *here*, so nobody let it go and `let_go_by` is `None`.

**This is the answer to the question ADR-0042 left open, and it is that the question dissolves.**
That ADR expected a failed run to need its own home for the fact, `let_go_by` living inside
`Pending` and a failed run not being pending. It does not, because the run stops being failed:
one transition puts it in the state the fact already belongs to, and everything downstream —
`supervise`'s `Unheld` branch, `Offering::LetGo`, the arbiter's bid round, `explain`'s sentence —
is ADR-0042's, unchanged and untouched. The whole of this ADR's placement machinery is a call to
a function that already existed.

Unfenced, like `reopen`: a `Failed` run holds no lease, the terminal transition took it. The
epoch bump is what ends the last leg's right to act, and it is why this runs inside
`Store::update_run` and not on a caller's copy.

### 3. Only where the fleet could actually start it

`Escalation::NoCopyElsewhere`. Offering a run whose transcript exists on the departing machine
alone is offering work the winner cannot begin: it would take the run, fetch nothing, and leave
it `Pending` behind a checkpoint that may never be reachable again — **strictly worse than the
`Failed` it came from**, which at least carries its own reason. The test is
`Checkpoint::is_durable`, whose holder is `None` here because a failed run has none, so it asks
the plain question: does anybody else have it. `Restartability::Idempotent` never reaches it —
that work is re-runnable from its spec, so there is nothing to fetch.

This is what is left of `NodeIsDeparting`: the one case where a departing node keeps a run it
would otherwise have handed over, said in the words that are true of it.

### 4. The drain does its own pass, and the fact leaves with the goodbye

Both halves were found by walking it, and neither is optional.

**`depart` sweeps, rather than leaving it to the recovery tick.** A failed run is terminal, so
`Mesh::drain` filters it out of `held` and has never seen one — and a node whose only run has
failed holds nothing at all, which makes that pass return on its first line. The tick would get
there a second later on the `offload drain` door; on the signal door there is no later, because
`main` stops everything the moment `depart` returns. That is the `SIGTERM` door, which is what
closing a laptop looks like, and it is the door ADR-0041 was measured on and left open. The pass
goes in `depart` for the reason `stop_accepting` does: it is the one way in, and a fact hoisted
into "the caller" has as many owners as that function has callers.

**And the record is published into the view before the process dies.** Measured, on a build with
the sweep and without this: node-a wrote `Pending` to its own store, exited, and every peer went
on holding the run as `Failed` until that daemon came back — the stranding this decision is
about, moved from the record to the network. `announce_departure` carries `Gossip::runs` and is
already waited for, one step after `depart` returns, so the fix is to put the run in the view the
goodbye is built from.

`Capacity::runs(0)` is passed to the sweep deliberately. Capacity is read in exactly one place —
the resume — which a departing node cannot reach, so nought rather than the node's real figure
makes that a second and independent guard: if the first ever breaks, the resume is refused for
want of room instead of starting an agent on a machine that is leaving.

### 5. `offload drain` says it, and it is a number of its own

`Drained::handed_back`. Not a share of any field beside it, because it is not counted from the
same list: every other number there describes a run this node *held*. Its own line in the CLI,
and its own arm in the "nothing to hand over" guard — a node whose only run had failed holds
nothing, so every existing number is nought and the drain said *nothing to hand over* about a
pass that had just given a run to the fleet. That would have been the eighth thing this command
was silent or wrong about; this time the fix and the sentence arrived together.

### 6. No wire bump, and no schema change

Worth stating because ADR-0042 needed one and this builds entirely on what that bought.
`Recovery` and `Escalation` are node-local — they reach an operator as sentences over the control
socket and are never gossiped. What crosses the wire is `RunState::Pending { let_go_by }`, which
is v24 and unchanged, carrying a value v24 nodes already know how to read and act on.

## Consequences

Same fleet, same fake agent, same failing run, on the fix.

Through the `offload drain` door:

```
08:44:26.070  node-a  this node is leaving; handing the failed run back to the fleet
08:44:26.070  node-a  gave failed runs back to the fleet  runs=1
              offload drain  →  "1 failed run(s) given back to the fleet to carry on with"
08:44:26.620  node-a  an unheld run was placed  offering=LetGo { by: node-a }  starting=starting now
08:44:26.675  node-b  spawning claude code  session=11111111-…  cwd=…/b/worktrees/…
08:44:33      node-b  completed, 4 turns — having failed here at turn 2
```

and through the `SIGTERM` door, which is the one the sweep and the announcement exist for:

```
08:47:26.265  node-a  handing the failed run back to the fleet
08:47:26.268  node-b  peer is leaving          ← the goodbye, carrying the record
08:47:26.510  node-b  an unheld run was placed  offering=LetGo { by: node-a }
08:47:26.530  node-b  spawning claude code, same session
```

**265 milliseconds** from the signal to the run being placed on another machine, against a build
where it stayed failed through a drain, a wait, and a restart of both daemons.

| | drain | 30s later | both daemons restarted |
| --- | --- | --- | --- |
| reverted | "nothing to hand over" | `failed` — "this node is draining" | `failed`, no reason at all |
| fixed | "1 failed run(s) given back" | completed at turn 4 on node-b | — |

- `offload explain` says it from the verdict, in the window before the tick takes it: *"this node
  is leaving; about to hand it back to the fleet to carry on with"*.
- **The departing node still starts nothing**, which is the whole of what ADR-0034 bought and is
  now a property rather than a line order.
- A run whose only copy is on the departing machine stays `Failed` and says why, which is the
  same outcome as before under a sentence that is true.

## What this deliberately leaves

**A node that *dies* rather than departs.** The same run on a machine that is killed, or that
loses power, stays `Failed` and is a person's. That is not an oversight and the asymmetry is the
point: a node that departs **says so**, and saying so is the entire fact this decision turns on.
An arbiter looking at a failed run whose last holder is gone can see the state and the checkpoint
and cannot see attendance — and *unknown is not good news*, so inferring an intention to hand
over from a machine's silence is precisely the guess this codebase refuses everywhere else.

**A run that failed under an earlier incarnation of the daemon.** It escalates
`AttendanceUnknown` and is not handed over, on both doors. Right for the same reason: a restarted
daemon cannot tell whether somebody was watching, and handing it to the fleet would restart an
agent behind that person exactly as resuming it here would.

**A run already escalated before the drain begins.** `recover_failed_runs` skips an entry whose
decision has been taken, so a run left for a person at 23:00 is still theirs when the laptop is
drained at 23:05. Correct — every remaining escalation is a fact about the run — but it does mean
the outcome depends on whether the tick got there first, for the one case where it should not
matter and does not: a `Wait` is not a decision, and `Wait` is what the interesting run is in.

**Replication is not forced on the way out.** A departing node with an unreplicated checkpoint
could copy it to a peer and then hand the run over, and does not: the drain has a deadline, the
blobs can be large, and `NoCopyElsewhere` is an honest answer where this would be a slow one. The
run is left `Failed` with the reason said. If this is argued for, the thing to measure first is
how often a checkpoint is *not* durable by the time a node leaves — background replication starts
at the turn it was taken, so the answer may well be "only when there was never a peer".
