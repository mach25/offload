# ADR-0018: Keep negotiated arbitration; `openraft` is not the answer, and the simulation says why

**Status:** accepted · 2026-08-24 · answers the phase 6 roadmap item "reconsider negotiated
arbitration vs `openraft` over `Stable` nodes" and roadmap open question #6 · does not supersede
[ADR-0002](0002-coordination.md) or [ADR-0006](0006-negotiated-placement.md), which stand

## Context

ADR-0002 rejected Raft "for now" and wrote down when to look again: *"Revisit at phase 6, when
voting membership can be restricted to `Stable` nodes with everything else as a learner."* The
reason for deferring was an availability judgement made without evidence — quorum among laptops
and phones felt like the wrong trade this early.

Phase 6 built the evidence. `offload-cluster/tests/storm.rs` drives four real `Cluster`s over an
in-memory transport with the clock as an argument, from a generated script of ticks, partitions,
bid rounds, declines and lost acceptances. It has already found two shipped bugs. What makes it
the right place to argue this is that it can *produce* a fork rather than describe one, so the
question stops being "how bad is split-brain" and becomes "which forks does quorum actually
remove".

ADR-0002's amendment sharpened the question in the other direction: an epoch is monotonic **per
arbiter**, so two arbiters issue the same number, and what fencing buys is not that a fork cannot
happen but that exactly one leg survives the moment the records meet.

## Decision

**Keep negotiated arbitration. Do not adopt `openraft`.** The reason is not the availability
judgement ADR-0002 made; it is that the fork the system actually produces is not the one quorum
removes.

There are two paths to two live legs of one run, and the simulation reaches both.

**Path one: an unacknowledged grant.** ADR-0006 step 6 makes a grant an offer, and
silence-is-a-decline makes "took it, answer lost" the ordinary case rather than an exotic one. The
arbiter spends the token and grants the next bidder; the first node is already running under the
number it was handed. Shrunk by proptest to four steps and no partition at all:

```
[Swallow(3), Place(0), Turn(2), Turn(3)]
```

One arbiter. One fleet, fully connected. Two legs.

**Quorum does not remove this path.** A Raft leader commits "run R to node N at epoch E" to a
majority and then has exactly the same problem: it cannot distinguish a node that never got the
grant from a node that got it, started an agent, and whose acknowledgement was lost. The decision
is replicated; the *side effect* is on one machine, and no consensus protocol reaches across that
gap. Fencing is what covers it, which is why ADR-0002 already said fencing is needed regardless.

**Path two: two arbiters.** A node concludes the home node dead, grants, and then hears the
refutation — or a partition genuinely separates two arbiting sides. This one quorum *would*
remove, and it is the whole of what quorum would buy here.

So the ledger is: Raft removes one of the two paths to a fork, and the path it leaves is the one
that needs no partition to occur.

### What it would cost

Restricting voting membership to `Stable` nodes was the mitigation ADR-0002 imagined, and on a
personal device fleet it is where the plan comes apart. `Stability::Stable` describes a VM and
possibly a desktop; the phones and laptops that make up the rest are `Ephemeral` at best.

- **One stable node** is a one-node quorum, which is ADR-0002's "fixed coordinator by config"
  alternative arriving under a different name — the one rejected outright, because it contradicts
  the federated premise.
- **Two stable nodes** make a quorum of two, so either one being down stops placement fleet-wide.
  That is strictly worse than today, where arbitration follows the run.
- **Learners do not help.** A learner cannot vote, so a fleet whose voters are asleep cannot place
  work — and "the desktop is asleep, the phone is awake, and the run needs to go somewhere" is the
  scenario this project exists for.

And it would be *additional* machinery rather than replacement machinery: every fence, every
epoch check, every lease stays exactly as it is, because path one survives.

### What is done instead

The cost of not having quorum is that under a partition that persists, work on the losing leg is
discarded. That is unavoidable without quorum and this ADR accepts it. What is *not* acceptable is
the fleet being wrong about it, and it was: a run's numbers were merged forward-only, so the leg
that lost froze the run's reported position at whatever it had reached — turn 19 for a run on turn
3, beside a worktree summary describing a checkout on a machine that was not running it, with no
tick that ever corrected either. See [ADR-0005's amendment](0005-cluster-view.md); position now
follows the same decision the record follows, and spend stays cumulative because the money left
the account on both legs.

That is the shape of the answer to this whole question. Quorum is not how this system stops a
fork; it stops one of two causes at a price that closes the fleet down. What is worth paying for
is a fork that is settled identically everywhere and reported honestly, which needs no messages
and no votes.

### The sub-question, still not taken

Open question #6 asked whether the losing leg should be *chosen* rather than settled by node id —
the granting arbiter on the `Lease`, so the run's home node's grant would win over a successor's,
since a successor only acted on a belief that turned out to be wrong. Still not taken, and the
reason is unchanged: both grants are safe, the choice is arbitrary but *agreed*, and as of this
session the run's numbers agree with it too. ADR-0002's amendment said to do it "the day the
choice of which leg survives starts mattering"; making the numbers follow that choice did not make
the choice itself matter more.

## Consequences

Good: nothing changes, which is the point of asking the question with evidence rather than
adopting the well-understood answer. Placement keeps working with one node awake. The two-arbiter
case remains reachable and remains settled offline, by every node, from the record alone.

Bad, and stated plainly because this is the second time this ADR family has softened it:

- **A partition that persists runs two agents on one repository**, and both may commit. Fencing
  guarantees only that exactly one leg survives contact. This is the residual risk of the whole
  design and it is not closed.
- **Work on the losing leg is lost**, including uncommitted work, which is the valuable part. The
  fleet now says so correctly instead of reporting the loser's position; it cannot recover it.
- **The choice of which leg survives is arbitrary.** Lowest holder id, which is agreed rather
  than good.

### When to look again

Not "at phase 7". Two concrete triggers, either of which changes the arithmetic above:

1. **A genuinely always-on majority appears** — a hosted control node, or three stable machines
   that are not in the same building. Then voting membership is not a fiction and path two closes
   for a real price.
2. **A workload appears whose double execution fencing cannot make safe** — a side effect outside
   the run's own workspace, which today's agent runs do not have (they act on a worktree, and a
   fenced leg's worktree is discarded). A run that sends mail or moves money is a different
   question, and it should be asked as one rather than absorbed into this one.

## Alternatives

**Raft over `Stable` nodes with the rest as learners** (ADR-0002's own suggestion). Rejected
above: it removes one of two fork paths, and its quorum floor is either one node or a fleet that
stops when a laptop shuts.

**Raft for the epoch counter only** — a replicated sequencer, so two arbiters cannot mint one
number. Attractive because it is small, and it has the same availability floor as full Raft: no
quorum means no epoch means no placement. It also does not close path one, since the number is not
what goes missing there.

**A lease server on one nominated node.** Simplest thing that makes epochs totally ordered.
Rejected for ADR-0002's original reason, unchanged: the nominated node is a laptop that closes,
and this design's whole claim is that no such node is required.

**Do nothing at all, and leave the numbers as they were.** Rejected: the residual risk of a
no-quorum design is acceptable only if it is visible, and the reporting was actively wrong in the
direction that hides it — a fleet confidently reporting sixteen turns of progress that no
surviving leg had made.
