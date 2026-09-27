# ADR-0005: Every node keeps a full cluster view, merged by fact ownership

**Status:** accepted · 2026-07-25 · ownership table extended by ADR-0012 (membership: owned
by the fleet key, arbitrated by signature, and — unlike liveness — never refutable by its
subject)

## Context

Something has to know who exists, what they can do, and who is running what. The obvious
shape is a central registry, but that reintroduces the single point of failure the whole
project is trying to avoid — and the registry would be on a laptop that closes.

The fleet is small (tens of nodes, tens of concurrent runs). Full state is kilobytes. There
is no scale reason to keep nodes ignorant of each other.

The hard part isn't storage, it's **merge**: when two nodes disagree, who wins? Get this
wrong and you get flapping state, resurrection of dead facts, or a node's own capabilities
being overwritten by a stale peer.

## Decision

Every node keeps a full [`ClusterView`]: all nodes, their capabilities, their policies, their
workloads, and all runs. Gossip merges into it.

Merge is decided by **fact ownership** — every field has exactly one authoritative writer, so
there is never a judgement call:

| Fact                                      | Owned by                | Arbitrated by |
| ----------------------------------------- | ----------------------- | ------------- |
| a node's capabilities, policy, workload   | that node               | `incarnation` |
| a node's liveness                         | its peers, refutable    | `incarnation` |
| a run's assignment and state              | that run's arbiter      | `epoch`       |

- **`incarnation`** is a counter the node bumps for itself. Higher wins. It is also how a
  node refutes a `Suspect` assertion — standard SWIM.
- **`epoch`** already exists as the fencing token. Reusing it as the merge order costs
  nothing and cannot disagree with fencing, because it *is* fencing.
- Within one epoch, a run only moves forward, so `(epoch, progress_rank)` is a total order.
  Releasing a run bumps the epoch precisely so this stays true.

Two deliberate exceptions to "the remote's version replaces mine":

- **Locally observed absence history stays local.** How long a node has historically been
  away is an observation *we* made; a refuting node must not be able to erase its own record
  of flakiness by bumping its incarnation.
- **A holder's own claim beats a peer's suspicion.** `Running` outranks `Orphaned` at equal
  epoch, because the holder knows and the peer is guessing.

## Consequences

Good: no central registry, so nothing to fail over and nothing to rebuild from a log. Any
node can answer any question locally — `offload ps` on a phone works without asking anyone.
A node joining late converges by observation. Merge needs no vector clocks and no
application-specific conflict resolution, because ownership makes conflicts impossible by
construction rather than resolvable after the fact.

Bad: every node pays memory and gossip bandwidth for the whole fleet, which is fine at tens
of nodes and would not be at thousands — this is a design that must be revisited if the scale
assumption changes. Views are eventually consistent, so two nodes can briefly disagree; the
epoch is what stops that disagreement from becoming two running agents. And the ownership
table is a rule that lives partly in reviewers' heads: a new field whose owner nobody decided
is a merge bug waiting to happen.

## Alternatives

**Central registry on an elected coordinator.** Simple, strongly consistent while the
coordinator lives. Rejected: failover means rebuilding state, and the coordinator is a
laptop.

**Per-node partial views** (know only your neighbours). Scales further, less bandwidth.
Rejected: bidding needs a node to reason about the whole fleet — account pressure and "is
there anywhere else this could go" are global questions.

**Off-the-shelf CRDT library.** More general merge semantics for free. Rejected as
unnecessary: ownership plus a monotonic counter is a CRDT, just a trivial one, and writing
the table above is clearer than encoding it in someone else's type system.

## Amendment, 2026-08-21: the one signed fact on the wire, and the one that is not gossiped

**Revocation joined the table and does not behave like anything else in it.** ADR-0012 wrote the
row and phase 3 never carried the fact; it does now, in `Gossip::revocations`. Everything else
here is arbitrated by a counter the owner controls; this one is arbitrated by a signature the
owner cannot produce, and its subject may never refute it. The ownership table's discipline still
applies — one authoritative writer, and it is the fleet key — but the mechanism is not
`incarnation`, and pointing `incarnation` at it would let a revoked device argue its way back in.

Worth stating why revocation is on the wire at all when membership is not. A certificate verifies
against the fleet public key at first contact with nothing gossiped, which is the property the
whole enrolment design is built around. A revocation is the *absence* of a signature, and no
certificate can carry "except this one" — so it is the exception, and it is the only one.

**And a deliberate non-entry: the fleet event log.** ADR-0012 mitigation 4 says an enrolment is
"written durably to every node's store". Taken literally that is a gossiped fact, and this table
is the reason it was not built that way: a fact written everywhere needs an owner and an
arbitration rule, and an append-only log of *observations* has neither. Two nodes recording the
same enrolment are not disagreeing; they are describing what each of them saw, and merging them
would mean deciding which observation is authoritative about an event neither of them owns.

So each node keeps its own log of what it witnessed, and `offload nodes --history` says so. The
rule this follows is the one in the "Bad" section above, applied rather than broken: a field whose
owner nobody decided is a merge bug waiting to happen, and the answer is sometimes that it should
not be a gossiped field.

## Amendment, 2026-08-24: one field, two owners — a run's numbers

`RunProgress` — turns, cost, denials, questions, and the worktree summary — has never had a row
in the table above, and building it that way was the bug. It was merged **forward only**, on the
stated grounds that these numbers "have a single author by construction: only the node running
the turns produces them". That construction is what a second grant breaks, and it does not need a
partition to break it: ADR-0006's silence-is-a-decline makes "took the grant, answer lost" the
ordinary case, so two legs of one run publish two sets of numbers, each one legitimately the
holder in its own view.

Filling in the row means admitting there are two rows, because there are two kinds of number
here and they were sharing one rule:

| Fact                                      | Owned by                | Arbitrated by |
| ----------------------------------------- | ----------------------- | ------------- |
| a run's **position** (turn, worktree)     | the run's current leg   | `epoch`, then lowest author id |
| a run's **spend** (cost, denials, asks)   | nobody — a max-register | field-wise `max` |

**Position** says where the surviving branch of the run *is*, which is a fact about one leg:
one node, holding the run under one epoch. So it is settled by the arithmetic that settles the
record beside it — the same epoch, and at equal epoch the same lowest-holder-id tiebreak
(ADR-0002's amendment). It has to be the *identical* rule, or a run's numbers describe one leg
while its record names another.

**Spend** says what the run used up, and is owned by nobody, which is the honest answer rather
than a gap: money left the account and a person was interrupted on the losing leg too. A
field-wise max is a join, so it needs no owner — and it is a max rather than a sum because a
relay repeats what it heard, so adding would count the same dollar on every gossip tick. That
understates a forked run's bill, and is the tightest lower bound an idempotent merge can give.

Two consequences worth stating.

First, **a merged record can be a combination of two legs** — this leg's position beside the
fleet's high-water spend — which is a shape nothing else in this view has. Everything that writes
one down has to write the record *as settled* rather than as it arrived, or the store ends up
holding a record the view never agreed with, permanently, since the loser never wins a later
merge. That is the same mistake `absorb` made about run records and the same fix.

Second, **the ordering has to be total**, not merely forward. Two writes in one millisecond with
different worktree summaries used to be settled by whichever gossip arrived first, so a fleet at
rest disagreed for ever about a string — no partition, no fork, just two legs at one instant. The
tiebreaks run out at the summary itself, so the summary is in the comparison. Arbitrary, like the
lowest-id tiebreak, and agreed for the same reason.

What this cost before it was found: a run at turn 3 reporting turn 19, because the leg that lost
had got further and forward-only keeps the larger number — with the losing machine's worktree
summary beside it, in the column somebody reads at 07:00 to find out whether there is uncommitted
work. And no tick that ever corrected it, because for the next sixteen turns everything the
surviving leg said was smaller and refused.
