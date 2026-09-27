# ADR-0044: A revoked node stops itself

**Status:** accepted · 2026-08-30 · extends ADR-0012 · wire v25

## Context

ADR-0012 says revocation is immediate and local, and session forty-two made that true of the
*fleet*: `Cluster::disconnect` now closes the session the peer opened as well as the one this node
dialled, so a revoked device is off the mesh in both directions and the run it was holding is
orphaned 1.7 seconds later and running elsewhere at the next epoch.

What that left, and stated plainly as its own residual:

> **A revoked node does not stop its own agents.** The fleet correctly evicts the node and moves
> the run; the node, which can hear nobody, runs its old leg to the end — two agents, measured,
> turn 31 against turn 15.

Two agents on one repository, both committing, is the failure `CLAUDE.md` puts first. Some of it
is unavoidable and ADR-0002 already accepts it: a partitioned node cannot be told anything, and
evicting the device is a better trade than leaving it in the mesh. **This device is not that
device.** It is the one machine in the fleet that could stop itself, and it had every fact it
needed to.

### Why the handler never fired

`change.ourselves` existed, on `FleetChange`, with a handler in `watch_membership` that logged and
did nothing more. It was on the wrong path twice over.

* **The path that cannot happen.** `ourselves` is set when `fleet.json` grows a revocation naming
  this node, and the only way one arrives on a revoked device is by gossip — from a peer that has
  just hung up on it in both directions and refuses every handshake it attempts from then on. The
  revocation is exactly the thing that ends the channel that would have carried it.
* **The path that happens had no handler.** `Refusal::Revoked` reaches the node on **every dial it
  makes**, once a second, for as long as it runs. `TransportError::Refused` was matched nowhere; on
  the reverted build it surfaces as `could not reach seed … refused by 32388d1a: member 64d1ca2e
  has been revoked`, at `WARN`, beside every other unreachable peer.
* **And the announcement was an edge, so it was eaten.** Even filed, `ourselves` could not have
  reached the tick: `NodeMembership::file` reloads to refresh its own copy and drops the
  `FleetChange` it gets back, so the daemon's own membership poll, a second later, was told nothing
  had changed. Whoever observed the edge first consumed it, and the observer is the writer.

### The objection to answer

**A refusal is a peer's claim about this node, and a node must not believe one of those about
itself.** That rule is not a technicality here — it is the whole reason `incarnation` exists, and
pointed at membership it is the rule that stops a device talking itself back in. A node that acted
on a bare `Refusal::Revoked` would be a node any peer could stop by saying so.

What is *not* a claim is the signed artifact. A `Revocation` verifies against the fleet key the
subject already holds, names the fleet it already belongs to, and names the subject — three checks
it can make alone, offline, with nobody to ask. That is the same property every other credential
here is built on, and it is what the refusal was missing.

### Measured, before

Two daemons on one machine, one fleet. node-a is the founder and hosts; node-b joined with
`submit, deliver` and is the machine the revocation is typed at. A fake agent, sixty turns, two
seconds apart.

```
13:23:28.955  offload revoke <node-a>          typed on node-b
13:24:14      node-a  ps      → running, turn 38
              node-a  status  → runs 1/2 · accepting yes
              node-a  log     → could not reach seed … refused: member 64d1ca2e has been revoked
                                (once every ten seconds, matched by nothing)
13:25:09      node-a  ps      → completed, turn 60
              node-b  ps      → orphaned, turn 12 · node-a dead
```

Forty-eight turns of agent after the fleet threw the device out, and a status line advertising the
machine as available throughout. The run node-b holds is `orphaned`, which is the state any capable
peer bids on — so on a fleet where somebody could host, those forty-eight turns are the second
agent.

**A restart hides all of it**, which is why five sessions of walks never met it: a daemon that
comes back reads `fleet.json`, finds its own certificate unusable, and does not join the mesh at
all. The symptom exists only on a live process.

## Decision

**A revoked node stops itself, on evidence it verified.**

### 1. The refusal carries the revocation (wire v25)

`Refusal::Revoked { node, proof: Box<Revocation> }`. `handshake::admit` already had to find a
covering, verifying revocation to refuse at all — `find` instead of `any`, and what travels is the
artifact this node checked rather than a sentence it composed.

A **required** field on a message a v24 node still sends without it, so a v24 refusal does not
decode here. That is the shape it has to have: the point of the field is that the refused node acts
on it, and a refusal that decodes with the proof missing is one it would have to act on without —
the failure the field exists to prevent, arrived at politely. It is also the one message a node
receives *after* being thrown out, so there is no later exchange to carry the proof instead.

### 2. It is filed through the gate a gossiped one passes

`Cluster::session` is the single choke point every dial goes through. A refusal carrying a
revocation whose `member` is **this node** goes to `Members::revoked`, which verifies against the
fleet key before writing anything down — the same call, the same check, the same file as a
revocation heard in gossip. A forged one is a warning naming who tried and nothing else. A refusal
naming somebody *else* files nothing: it arrived on a dial, so the subject should be us, and a
message addressed to us is not authority to evict a third party.

**The refusal is not what is believed. The signature is.**

### 3. Being revoked is a standing condition, not an announcement

`FleetChange::ourselves` is gone; `NodeMembership::revoked_here()` replaces it. Revocation is
monotonic — nothing un-revokes a device, and `offload rekey` founds a *different* fleet rather than
undoing one — so the honest shape is a question anybody may ask at any time and get the same answer
to. An edge belongs to whoever reads it first, and here the first reader is the code that writes
the fact down.

`Supervisor::stand_down` is idempotent by construction rather than by a flag: `halt_agent` takes the
cancel sender, so the second pass finds nothing to signal. The latch decides only whether there is
anything to *say*.

### 4. What stopping means

* **Every agent here is halted**, with `Halt::Revoked`. Deliberately not `Superseded`, though the
  agent stops the same way: `Superseded` writes nothing because somebody else's record has already
  arrived and is the truth, and a revoked node has heard from nobody and will hear from nobody.
  Leaving the row `Running` behind a process that is gone is the lie `fail_if_unfinished` exists to
  stop. Not `Cancelled` either, for `Stop::LimitReached`'s reason: `by` names the node an operator
  typed at, and there is no such node here.
* **The leg is written `Failed`, with the reason on it** — *"this node was revoked from its fleet,
  so it stopped running it"* — through `fail`, which asks `holds` first exactly as the turn-limit
  arm does.
* **Nothing is thrown away.** The worktree, the branch and the transcript stay where they are. The
  earlier handler's argument for doing nothing was that the runs here are still this node's problem
  and dropping them would lose the work; that argument was about a *lapsed certificate* and does
  not survive contact with a revocation. The fleet has already taken the run back under a higher
  epoch, and nothing this node produces from here can reach anybody.

### 5. It takes no more work, and every door says why

* `FleetState::grants` returns **nothing** once this node is revoked. The certificate still lists
  its grants and still verifies — that is what makes revocation the exception it is — so every door
  that asked the certificate stayed open: `offload status` said `accepting yes`, the bid and the
  grant passed their `host-runs` check, and `offload run` submitted. One question, one answer, one
  place.
* `grant_refusal` says the true sentence above every other: *"this node has been revoked from its
  fleet — it can host nothing and submit nothing until it is enrolled again"*. Telling an evicted
  device to run `offload grant` is advice that cannot work.
* `offload status` asks revocation **above** the drain, because standing down sets both latches and
  *"drained; restart offloadd to take work again"* is the other piece of advice that cannot work
  here.
* `Circumstances::revoked` and `Escalation::NodeIsRevoked`, checked **above everything including
  `departing`**. This is the fifth path `stop_accepting` never reached: auto-resume asks neither the
  bid nor the grant, so shutting those two doors leaves the one that spawns agents without being
  asked. The reason it outranks `departing` is one step stronger than the reason `departing`
  outranks the rest — every other input asks whether a retry would *work*, `departing` asks whether
  this is the machine to try it on, and this asks whether this machine is allowed to be a machine.
* It does **not** become `Recovery::LetGo` the way a drain's does. Letting go is an offer to the
  fleet, and a revoked node has no fleet: nothing it writes gossips, no checkpoint of its
  replicates, and the run is already the fleet's under a higher epoch. There is nobody to tell.

Swept by a property test over every shape of failed run against every `Circumstances` a revoked node
can present, `departing` in both positions — the same shape ADR-0043's sweep has, because the
guarantee is the same kind of guarantee and a claim about line order is not one.

### 6. The memory transport's `close` now reaches both ends

Not the decision, but found by trying to test it and worth the same paragraph. Session forty-two
made `MemoryConnection::close` do something; each end still held its **own** flag, so closing one
left the other opening streams and gossiping happily. A test could measure the side that hung up
and learn nothing about the side that had to notice — which is that session's own lesson, one layer
down. The flag and its `Notify` are shared between the two ends now, because a hang-up is one event.

## Consequences

Same fleet, same fake agent, same run, on the fix.

```
13:26:33.548  offload revoke <node-a>          typed on node-b
13:26:35.243  node-a  ERROR  this node has been revoked from its own fleet, and a peer
                             proved it with the fleet's own signature            +1.70s
13:26:35.974  node-a  ERROR  … it will take no more work, and the agents it was
                             running have been stopped                    runs=1  +2.43s
              node-a  WARN   stopped because this node was revoked; its worktree and
                             branch are untouched
13:27:25      node-a  ps       → failed, turn 15
                                 "this node was revoked from its fleet, so it stopped running it"
              node-a  status   → runs 0/2 · accepting no — this node has been revoked …
              node-a  explain  → recovery: left for a person: this node has been revoked
                                 from its fleet, so it will not start an agent for it
              node-a  run …    → Error: this node has been revoked from its fleet …
              node-a  log      → one `spawning claude code` in the whole file
```

| | before | after |
| --- | --- | --- |
| agent stopped | never — completed at turn 60 | **2.43s**, at turn 15 |
| turns after eviction | 48 | 1 |
| `offload status` | `runs 1/2 · accepting yes` | `accepting no — … revoked …` |
| the run's record here | `completed` | `failed`, with the reason |
| auto-resume afterwards | would resume | `NodeIsRevoked`, left for a person |

## What this deliberately leaves

- **The partition case is unchanged and stays unchanged.** A device that is revoked while it cannot
  hear anybody keeps running, because it has been told nothing. That is ADR-0002's trade and this
  decision does not touch it: what is fixed is the case where the fleet *is* reachable and is
  saying so on every dial.
- **The stopped run is not offered to the fleet from here.** See §5. If it is ever argued for, the
  objection is that a revoked node's writes reach nobody, so the offer would be to itself.
- **A node re-enrolled later still holds the `Failed` row and the worktree.** Deliberate — it is
  the honest local record of a leg that happened — and the epoch fence is what keeps it from
  contradicting the fleet's copy when the device comes back.
- **`offload status` does not say which device was revoked when it was somebody else.** The
  `revoked N device(s)` line is unchanged; naming them is `offload fleet`'s job.
