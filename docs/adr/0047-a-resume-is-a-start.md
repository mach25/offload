# ADR-0047: A resume is a start, and the door that starts one asks what the other door asks

**Status:** accepted · 2026-08-30 · extends ADR-0046 to the second door and closes a hole in
ADR-0044's stand-down · no wire change, no schema change

## Context

ADR-0046 gave the no-cluster submission arm the clause it was missing and stated the invariant it
was repairing:

> **`permits` is asked on every path that can start a run; the queue clause is each path's own.**

It was written about one door. There are two. `offload resume` starts an agent — the same binary,
in the same worktree, on the same machine — and its handler asked **none** of the three questions
the submission door asks. It resolved the run id, built a `Room`, and called
`Supervisor::resume`, so the only gate on the whole path was the queue.

Measured on one daemon, three times, all three against the build that shipped ADR-0046 the same
day:

```
$ offload status
accepting   no — node is not accepting work            # [policy] accept = "never"
$ offload resume 01a0533de6f6
resumed 01a0533de6f6
$ offload ps --all
01a0533de6f6   running             6  …  count to six
```

```
$ offload drain
this node has stopped accepting work — restart offloadd to take work again
$ offload status
accepting   no — drained; restart offloadd to take work again
$ offload resume 01a053402a1e
resumed 01a053402a1e                                    # running, turn 5
```

and the one that matters most — a node **revoked** from its fleet, whose run ADR-0044 had halted
seconds earlier:

```
$ offload ps --all
01a05342ed25   failed              5  …  count to twelve
           └─ this node was revoked from its fleet, so it stopped running it
$ offload run --repo … "and again?"
Error: this node has been revoked from its fleet — it can host nothing and submit nothing
$ offload resume 01a05342ed25
resumed 01a05342ed25
$ offload ps --all
01a05342ed25   running             7  …  count to twelve
```

The submission door refuses. The resume door, one screen further down the same terminal, restarts
the agent on a device the fleet has evicted — ADR-0044's stand-down undone by one command, with
the run's own state carrying the eviction as the reason it stopped.

**And the report points at the hole.** A revoked node restarted while a run was active marks it
`failed — resumable from turn 16 with 'offload resume 01a05342ed25'`. The sentence naming the way
forward was naming the way around.

### Why it hid

The same shape as ADR-0046's, and this is now the third instance: **a gate assembled clause by
clause, on one door at a time.** The drain clause was added to the submission arm when it was
measured missing; the policy clause was added to the same arm two ADRs later, when it was measured
missing; the recovery tick got `departing` and `revoked` through `Circumstances` when *its*
version was measured missing, with a comment saying exactly why — *"the path `stop_accepting`
never reached. A drain refuses bids and refuses grants; this asks neither."*

Every one of those fixes was correct, tested, and written down. None of them asked the next
question: **what else starts an agent?** The answer was one match arm away, and the handler is
old enough that the drain and the revocation were both built around it without anybody opening it.

## Decision

### 1. One gate, named, in one place

```rust
fn hosting_refusal(ctx: &Ctx) -> Option<String>
```

The three standing questions in `Host::evaluate`'s order: this node leaving, the grant that says
whether it may host (where a revoked device is turned away, ADR-0044), and the owner's own
standing answer (`WorkPolicy::permits`, ADR-0046). The no-cluster submission arm and
`offload resume` both call it. Not two lists that agree today — the failure mode here is a
divergence nobody can see, since both copies are right on the day they are written.

`permits` and not `admits`, for ADR-0046 §3's reasons unchanged: the queue half is each path's
own, and on a fleet of one, being full is a refusal rather than ADR-0006's commitment.

### 2. The resume door asks it after `resolve`, and before everything else

After `resolve`, because the run id is the *request* and the refusal is about the *node*: a typo
should still earn `no such run` rather than a lecture about the drain.

Before `Supervisor::resume`, because everything that function knows is about this particular run
— no checkpoint, turn limit spent, agent still shutting down — and a node that will not host
anything should say so before litigating which run it will not host. It is also where the refusal
cannot spend anything: `stop_recovering` still runs only on the way *out*, so a refused resume
does not hand the run a fresh retry budget, which is the rule that path's own history is about.

### 3. `offload resume` is not an override

The tempting reading is that a person at the keyboard naming one specific run is exactly the case
policy should yield to. It loses on all three clauses:

* **Revoked** has no counter-argument at all. The fleet evicted this device; ADR-0044 exists to
  make it stop.
* **Drained** is the state whose whole purpose is that this node stops hosting so it can be shut,
  and `offload drain` is one-way by design — the documented way back is restarting `offloadd`.
* **The owner's policy** is the one with a real counter-case: a checkpoint marked `here only`, on
  a laptop under its battery floor, is work that can continue nowhere else. It still loses, and
  the reason is ADR-0046's: `accept = "never"` says *never host them*, a battery floor is about
  running out, and the lever is the owner's own — change the line, or plug the thing in. A run
  that must not start here is not more startable for being asked about by name.

This is the same question ADR-0046 declined to settle by adding `--here`, and it is settled the
same way, for the same reason: the flag is the answer only if the friction turns out to be real,
and what to bring to it is a count of how often somebody hit the refusal and meant it.

## Consequences

- A node that reports `accepting no` refuses a resume at its own socket, with the sentence
  `offload status` prints. Measured after the fix on all three stagings: the run stays `pending`
  (or `failed`, for the revoked one) and no agent spawns.
- ADR-0044's stand-down is now actually standing: the revoked node's runs cannot be restarted from
  its own keyboard.
- An ordinary node is unchanged — measured: `resumed`, turn 5, same conversation, same worktree.
- `dispatch`'s Resume arm is now four lines and a `resume_run` beside `submit_run`, which is what
  made the two tests below possible at all. The arm had grown to eighty lines of handler with the
  decision inside it.
- Three tests, and they are the enforcement rather than a description of it: a drained node
  refuses a resume *and* a typo still hears about itself; `accept = "never"` produces the **same
  string** at both doors, asserted by equality so a second copy of the list cannot drift; and
  ADR-0046's ordering is pinned in `offload-core` (see below).

## What this deliberately leaves

**The recovery tick still does not ask the owner's policy.** It asks `departing` and `revoked`,
through `Circumstances`, and a node under its battery floor or on a metered link will still pick
its own failed run back up unattended. That is a real gap and it is *not* fixed by putting
`permits` inside `Supervisor::resume`, which is the obvious-looking edit: the tick logs a refusal
as `could not resume it; this attempt still counts`, so a battery dip would spend the run's retry
budget on a condition that is nothing to do with the run. The right shape is one more standing
fact in `Circumstances` — beside `departing`, which it resembles exactly — probably answering
`LetGo`, so a node that will not host hands the failed run to the fleet instead of holding it.
That changes `decide_recovery`, which sessions forty-one and forty-three swept exhaustively, so it
is an ADR with a sweep to re-run and not a clause to append here.

**The stored failure reason still names `offload resume` on a node that will refuse it.** It is
composed once, at recovery, and stored on the run — so making it conditional would bake a live
fact about the node into a permanent record of why the run stopped, which is the one-field-two-
facts trap this project keeps paying for. The honest fix is at the *reading* end, where `ps`
renders the footnote, and it needs the node's standing answer to travel with the listing. Left,
because the cost is now one command and a clear sentence rather than a resumed run.

**`AgentNotAllowed` is still unreachable, and ADR-0046 §2's residual cannot be walked.** That ADR
asked for a walk of the commit-a-forbidden-agent chain. It cannot be staged: `AllowedAgents`
refuses every name but `claude-code` at deserialize time and refuses the empty list, and
`Supervisor::build` writes `ClaudeCode` into every `RunSpec` — so the only set a config can hold
is one that always contains the agent every run names. Measured: `["claude-code"]` starts,
`[]` and `["codex"]` are both refused at startup. The clause is not dead, it is waiting for the
second adapter (phase 7) — and the ordering has to already be right when that lands, because that
is the day it becomes both reachable and unnoticeable. So the ordering is pinned by a unit test
instead, `a_forbidden_agent_is_refused_before_a_full_node_is`, which reproduces ADR-0046's own
measurement (`Err(AtCapacity { running: 1, max: 1 })`) and fails on the old order. A rule nothing
enforces is a hope, and for this one there is nothing else that can.

**Adoption at startup was checked and needs nothing.** A daemon restarting over an interrupted
run marks it `failed` and does not spawn anything — measured on the revoked node, which came back
up, declined to join the mesh, and left the run alone. It is the resume door that lets it back in,
and that door is now shut.
