# ADR-0006: Nodes bid for runs; a per-run arbiter grants

**Status:** accepted · 2026-07-25 · supersedes the coordinator-election part of ADR-0002 ·
amended 2026-07-26 with step 6, a granted node's right to decline · amended 2026-07-27 in
implementation: with a transport that addresses peers directly, **the arbiter asks and nodes
answer**, rather than nodes broadcasting bids after a score-proportional delay. The delay
existed to keep a broadcast from becoming a storm; asking costs the same messages, needs no
delay, and lets the asker bound the round — which ADR-0014 requires, since somebody is
standing at the keyboard. Steps 2, 4, 5 and 6 are unchanged, and `winner()` stays
deterministic so bids can travel again when arbiter failover needs them to.

## Context

ADR-0002 proposed an elected coordinator that decides placement for the whole fleet. That
works, but it has an information problem that no amount of gossip fixes.

The facts that best predict a good placement are the ones a central scheduler sees worst:

- Is the repo already cloned here? (Worth more than a machine twice the size.)
- What is this node's load *right now*, not thirty seconds ago?
- Is the battery draining, is it thermally throttled, is the network suddenly metered?
- Is the user actively typing on this laptop?

Gossiping all of that at the fidelity a scheduler would need means gossiping constantly. And
the node already knows it, for free, exactly when the decision is being made.

There's also an authority problem: a device's owner should be able to say "not right now"
without the fleet overriding them. A central scheduler assigning work to a phone that has
decided it is on battery is a scheduler that is wrong.

## Decision

**Nodes bid. Arbiters grant.**

1. A `Pending` run is visible in every node's cluster view (ADR-0005).
2. Each node evaluates it *locally*: owner policy first (`WorkPolicy::admits`), then
   eligibility (`Constraint::matches`), then a self-score using local facts gossip cannot
   carry — warm workspace, real load, battery, whether it already holds the checkpoint blobs.
3. A node that wants it broadcasts a `Bid` after a **score-proportional delay**: keener bids
   go out sooner. In the common case the obvious best node claims first and nobody else
   bothers, so negotiation costs one message. A real auction only happens when two nodes want
   it about equally — which is exactly when one is worth holding. *(Superseded by the
   implementation amendment above: the arbiter asks and nodes answer, and there is no delay. Left
   in place because the reasoning is why the replacement is cheap rather than a regression — it
   costs the same messages and buys a bounded round.)*
4. The run's **arbiter** grants it, bumping the epoch. The arbiter is the run's *home* node
   (whichever accepted the submission) while that node is available; otherwise the
   lowest-id available node. Every node computes the same successor from the same view, so
   failover needs no election round-trip.
5. `winner()` is deterministic — highest score, ties by lowest node id — so every node that
   saw the same bids agrees without another message.

6. **The granted node confirms or declines.** A grant is an offer, not an instruction. The
   node re-runs the same local checks it bid with and either starts the run or returns a
   structured `NoBid` — most usefully `Busy { retry_after }` once capacity is a budget
   (ADR-0013).

Arbitration is **per run**, not per fleet. There is no single coordinator; arbitration load
spreads naturally across whichever nodes accepted submissions.

### Why step 6 exists

A bid is a statement about a moment, and the grant arrives later. In between, the node may
have won two other bids, started a build, been unplugged, or dropped below its battery floor.

Epoch fencing does not cover this. It guards against stale **run** state — *this run has
moved on since you bid* — and says nothing about stale **node** state, which is the case that
actually bites when capacity is a budget rather than a count. Without a confirmation step the
arbiter's only recourse is to grant, watch nothing happen, and wait out the lease.

Three rules keep the extra round trip from creating new problems:

- **A declined grant must never strand the run.** The arbiter grants the next-best bid, or
  returns the run to `Pending` with the decline recorded alongside the other reasons. A run
  left `Assigned` to a node that said no is worse than no grant at all.
- **Silence is a decline**, after a short window. A node that went away between bidding and
  being granted must not cost the run a full lease expiry.
- **A node that declines is not reconsidered in the same round.** Otherwise arbiter and node
  can ping-pong: granted, declined, re-granted to the same node because it still has the
  highest score. It becomes eligible again on the next bid round, which is also when its
  score reflects whatever made it decline.

The epoch bumps on grant, so a decline burns one. That is the right trade and a useful
signal: a node that flaps between bidding and declining shows up as a run whose epoch climbs
without it going anywhere, which `offload explain` can say out loud.

### Accepting without starting, and commitments that outlive the submitter

The scenario this has to serve: submit a run from a laptop at bedtime, close the laptop, and
find the work done in the morning. It is the project's headline case, and it needs two things
that step 6 as stated does not give.

**A granted node may accept a run it cannot start yet.** Deferring with
`Busy { retry_after }` leaves nobody committed — the run stays `Pending` and hopes. Instead
the node may take the grant and remain in `Assigned`, holding the lease and renewing it by
heartbeat, and start the agent when its budget frees (ADR-0013). That is a real commitment
with a real fencing token, and it costs no new machinery: `Assigned` already exists as the
state between grant and start; this only allows it to last.

Two rules keep a commitment from becoming hoarding:

- **A node must not hold a run it cannot start in time.** If its own estimate of when it will
  free slips past the run's deadline, it releases rather than sitting on it — otherwise a
  busy node's optimism is indistinguishable from the fleet having nowhere to put the work.
- **A held run is still a lease.** It expires if the holder stops heartbeating, which is what
  makes "the desktop died at 01:00" recoverable by the ordinary orphan path rather than a
  special case.

**A submission must be durable somewhere other than the node that accepted it before the CLI
reports success.** The home node writing the run to its own store is not enough when that node
is about to be shut: an acknowledgement that means "saved locally" is a lie told to somebody
whose next action is closing the lid. So `offload run` returns once the run is replicated to
at least one other node, or returns with a warning that it is not — never silently. Nodes
receiving a pending run in gossip must persist it rather than holding it in view memory only,
or the same gap reopens at their next restart.

The corollary is that **the submitting node stops being special the moment the run exists**.
Arbitration already survives its departure by the deterministic-successor rule above; the run
record survives by replication; the result comes back over the delivery plane (ADR-0010)
rather than to the terminal that submitted it; and the laptop reconciles on rejoin through the
ordinary epoch-ordered merge.

What this does *not* fix is a run whose repository only exists on the departing laptop. A
`Portability::NodeLocal` workspace cannot be cloned once its source is asleep, which turns the
headline scenario into a silent failure at exactly the moment nobody is watching. That is
roadmap open question #4, and this scenario is the argument for closing it before phase 4
rather than during it.

**This is not two-phase commit, and must not grow into it.** The arbiter does not collect
confirmations from multiple nodes or hold a prepare phase; exactly one node is granted at a
time, and epoch fencing remains the entire safety story. Step 6 buys *liveness* — do not hand
work to a node that will sit on it — and buys nothing for safety. If a future change starts
treating a confirmation as a correctness requirement, the design has drifted.

Epoch fencing (ADR-0002) is unchanged and remains the actual safety mechanism. Two arbiters
during a partition can each grant the same run; only one epoch survives contact.

## Consequences

Good: decisions are made where the information is. Owner policy is enforced by the owner
rather than requested of a scheduler, which is what makes a phone a safe worker. The fast
path is one message. Arbitration failover is deterministic and instant. And `NoBid` is
structured — "phone declined: not charging" and "phone ineligible: no rust toolchain" are
different sentences, which is most of what `offload explain` needs to be useful.

Bad:

- **Placement is no longer globally optimal.** Nodes optimise for themselves; nobody is
  packing the fleet. For tens of long-running agent sessions this is irrelevant, and if it
  ever isn't, the arbiter is the place to add global policy.
- **Bid storms are possible** if many nodes score identically. Mitigated by the deterministic
  winner, not eliminated. This used to say "by the delay and by the deterministic winner", which
  outlived the delay: the implementation amendment at the top replaced step 3 before it shipped,
  and the sentence went on naming a mitigation that is not in the code. The delay's remains —
  `bid_delay`, `max_bid_delay`, `score_ceiling` — were deleted in session twenty, called by
  nothing but their own test and carrying a doc comment that described the mechanism in the
  present tense. Under *asking*, a storm is bounded by the asker: the arbiter contacts the nodes
  it means to contact and closes the round on its own clock, so "many nodes score identically"
  costs one extra comparison rather than one extra message.
- **A silent fleet stalls a run.** If every node declines, the run sits `Pending` with a
  collection of reasons. That is the correct outcome but it must be *visible*, or it looks
  like a hang. Roadmap open question #3.
- **Scores are not comparable across weight versions.** Two nodes running different
  `BidWeights` bid in different currencies. Weights need to be part of cluster config, not
  per-node preference.
- **Step 6 adds a round trip to the common path**, where the node almost always confirms. It
  is paid on every placement to avoid a lease-length stall on the rare one, which is the
  right trade only because a lease is minutes and a round trip is milliseconds.
- **A node can now decline work it advertised**, so `offload explain` has to distinguish "did
  not bid" from "bid and then backed out". The second is a worse look and a more useful
  diagnostic — it usually means the node's capacity moved between the two moments.

## Alternatives

**Central scheduler assigns (original ADR-0002).** Globally optimal in principle, simpler to
reason about. Rejected for the information and authority problems above — and note it does
not actually remove split-brain risk, so it does not even buy simpler safety.

**Pure claim, no arbiter** — first node to broadcast a claim wins. One fewer round trip.
Rejected: concurrent claims resolve to "both started the agent", and unwinding that means
killing an agent that has already taken actions. The arbiter exists so exactly one epoch is
ever issued.

**Work stealing** — idle nodes pull from a queue. Elegant, and genuinely good for short
tasks. Rejected as the primary mechanism because it inverts the constraint story: a puller
has to scan for work it qualifies for, and "why did nothing pull my run" is much harder to
answer than "here are the nine nodes that declined and why". Worth revisiting for a future
short-task kind.
