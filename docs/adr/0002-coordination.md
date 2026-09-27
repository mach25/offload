# ADR-0002: Gossip membership + single-writer placement, with epoch fencing

**Status:** accepted · 2026-07-25 · revisited at phase 6 and upheld ([ADR-0018](0018-no-quorum.md))
**Amended:** the elected-coordinator part is superseded by [ADR-0006](0006-negotiated-placement.md);
placement is now negotiated between nodes and granted by a per-run arbiter. Mechanisms (1)
and (3) below stand unchanged, and (3) is still the part that provides safety — but see the
2026-08-21 amendment, which is what made (3) actually true: an epoch orders the grants of one
arbiter and said nothing about the grants of two.

## Context

Something has to decide "run R happens on node N". That decision wants a single writer.
But the fleet is phones and laptops — there is no node we can count on being available, and
the whole premise is that devices come and go.

Two pressures pull apart:

- **Availability.** If placement stops when the desktop sleeps, the system is useless.
- **Safety.** If two things assign the same run, a non-idempotent job runs twice.

Classic answer is Raft: quorum, one leader, no split brain. But a Raft group whose members
are laptops that close and phones that doze will spend meaningful time without quorum, and
quorum loss means no placement at all.

## Decision

Three mechanisms, layered:

1. **Membership by gossip (SWIM).** Eventually consistent, AP, survives arbitrary churn.
   Every node knows roughly who is alive and what they can do. No quorum needed.
2. **A single writer per placement decision.** *(Superseded by ADR-0006.)* Originally a
   fleet-wide elected coordinator restricted to `Stable` nodes. Now a **per-run arbiter** —
   the run's home node while available, else the lowest-id available node — granting to
   whichever node bid highest. The structural point survives the change: exactly one party
   issues an epoch for a given run, and every node computes who that party is from its own
   view without a vote round-trip.
3. **Epoch fencing as the actual safety mechanism.** Every assignment carries a monotonically
   increasing epoch per run. Nodes present their epoch on lease renewal and on any
   side-effecting operation; stale epochs are rejected at the point of effect.

The important part is (3). We accept that (2) can produce two arbiters during a
partition. Fencing means a split-brain assignment produces a *rejected* second writer, not a
double execution.

The arbiter's run registry is deliberately reconstructible: nodes gossip what they are
actually running, so a newly appointed arbiter rebuilds state by observing the fleet for one
gossip round rather than by recovering a log.

## Consequences

Good: no quorum requirement, so placement keeps working with one stable node or none of the
usual ones. Arbiter selection is instant and free. Losing an arbiter costs a gossip round, not
a leader election protocol. Layers are independently testable.

Bad: during a partition, both sides may believe they can place runs, and reconciliation is
last-writer-wins on epoch. A run can be started on side A and fenced out moments later — so
runs must tolerate being killed shortly after start. The registry is only as durable as the
gossip view, meaning a total simultaneous fleet shutdown loses pending-but-unstarted work.

Fencing must be enforced *everywhere* effects happen. A single unfenced write path silently
voids the whole guarantee. This is the main thing to watch in review.

## Alternatives

**Raft from the start (`openraft`).** Correct, well-understood, removes split-brain assignment
entirely. Rejected for now: quorum among unreliable devices is the wrong availability trade
this early, and epoch fencing is needed regardless — a Raft leader can still be partitioned
from a worker that keeps running. Revisit at phase 6, when voting membership can be
restricted to `Stable` nodes with everything else as a learner. **Revisited and rejected
again**, this time with the simulation rather than the judgement: [ADR-0018](0018-no-quorum.md).
The sentence above about a partitioned worker is the whole of the argument, and it was already
here.

**Fully leaderless / CRDT registry.** Every node proposes placements, conflicts merge. Maximum
availability. Rejected: convergence for "exactly one node runs this" is not a natural CRDT,
and the merge rule ends up reinventing leases and epochs with less clarity.

**Fixed coordinator by config.** Simplest. Rejected outright — it contradicts the federated
premise, and the fixed node is a laptop that closes.

---

## Amendment, 2026-08-21: an epoch orders one arbiter's grants, and nothing else

Mechanism (3) is the one this ADR says provides safety, and the sentence it rests on —
"fencing means a split-brain assignment produces a *rejected* second writer, not a double
execution" — was not true of the code. Found by the property tests, which is the point of
having them: the claim is a sentence about *every* pair of grants, so nothing that arranges a
case was ever going to check it.

**An epoch is monotonic per arbiter, which is a weaker thing than a total order.** Both
arbiters compute `epoch.next()` from the same record, so both issue the *same number* to
different nodes. `Run::fence` refuses a caller whose epoch is *behind*; neither of these is.
So both writers were admitted, both agents ran, and the merge — which this ADR's own
consequences section describes as "last-writer-wins on epoch" — had nothing to break the tie
with: at equal epoch each record is spoken for by its own holder, so `merge_run` believed
whichever arrived most recently and flipped again on the next gossip tick. Nothing ever told
either agent it had lost.

Two agents on one repository, both committing. The failure this whole design exists to prevent,
reachable through the mechanism written down as the thing that prevents it.

**And it did not need a partition.** The ADR frames two arbiters as a partition-time
compromise, which made this look like the price of choosing AP over quorum. It is not: a node
that concludes the home node dead grants the run and *then* hears the refutation, so a healthy
LAN produces the same collision through a few missed probes. Worse, a single arbiter produced
it too. `Cluster::place` re-cloned the original record for each grant attempt in a round, so
every node in one round was offered the run at one epoch — and ADR-0006's "silence is a
decline" means the ordinary case is a node that took the grant, started the agent, and whose
answer was lost. It is running under exactly the token the next node is then handed.

Three changes, and the first is the actual bug:

1. **A grant spends a token whether or not it is confirmed.** `hand_over` threads one `Run`
   through the round and gives the token back with `Run::release` when an attempt fails, which
   bumps the epoch — the same sentence `release` already carried for a commitment handed back:
   it "ends the departing holder's right to act on the run at the moment it stops intending
   to". An unconfirmed grant is that moment seen from the arbiter's end.
2. **Two live grants at one epoch are decided by the token, not by who spoke last.** The
   tiebreak is the lowest holder id — the same one the bid round already uses, computable by
   every node from the record alone, offline, with no message. *Which* grant wins is arbitrary;
   that every node picks the same one is the whole point, because it is what makes the loser's
   own record name somebody else and its own `fence` refuse it. This is where the ADR's promise
   is actually kept: fencing rejects the second writer because the merge decided which writer
   is second.
3. **A superseded holder learns it lost.** `record_run`'s fencing-becomes-an-action check read
   `live.epoch < run.epoch` — the ordinary reassignment, and the one case that is not this one.
   `<=` now, with the holder check above it excluding our own leg.

What remains true, and is worth stating precisely because the old sentence was not: under a
partition that *persists*, two agents can run, and no amount of fencing changes that without
quorum. What fencing buys is that the moment the two records meet, exactly one leg survives and
the other's side effects are refused. That is the honest form of this ADR's claim, and it is
now the form the code implements.

A better tiebreak exists and is deliberately not built: the granting arbiter on the lease,
which would let the run's *home* node's grant win over a successor's, since a successor only
acted on a belief that turned out to be wrong. That costs a field on `Lease`, a wire version,
and a schema default, to choose between two grants either of which is safe. It is worth doing
the day the choice of *which* leg survives starts mattering; it is not worth doing to make the
choice prettier.

**And the revisit this ADR scheduled has happened: [ADR-0018](0018-no-quorum.md).** The answer is
to keep negotiated arbitration, and the argument is one this amendment set up without noticing —
if two arbiters are only *one* of the paths to two live legs, quorum removes only that one. The
other is a grant whose acknowledgement was lost, which needs no partition and which no consensus
protocol reaches, because the decision is replicated and the side effect is on one machine. What
the two ADRs together say about the tiebreak is unchanged: arbitrary, agreed, and now the run's
*numbers* follow it too (ADR-0005's second amendment), which is a different thing from the choice
having started to matter.
