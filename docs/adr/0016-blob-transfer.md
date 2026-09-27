# ADR-0016: Ask for blobs, don't advertise them — and a checkpoint isn't durable until a second node has it

**Status:** accepted · 2026-07-27, accepted 2026-08-07 after building and running it · settles
the first thing phase 4 needs, and corrects a line in `ARCHITECTURE.md`

## Context

Migration is "fetch these hashes, materialise the worktree, resume the agent" (ADR-0003). The
fetching part does not exist. Phase 3 gave nodes a transport and a view of each other; nothing
yet moves a byte of a checkpoint between them.

`ARCHITECTURE.md` has said since phase 0:

> Nodes gossip which hashes they hold; a node needing one fetches from any peer advertising it.

That is one design, written before there was a mesh to test it against, and phase 3 changed
two of the facts it rests on. It is worth re-deciding now rather than implementing from
memory.

What a checkpoint actually contains matters here, and is smaller than it sounds:

- **the transcript** — the agent's own conversation state, hundreds of kilobytes to a few
  megabytes;
- **a git bundle of `base..HEAD`** — only the commits the agent made, so kilobytes for a
  typical turn;
- **a patch of uncommitted work**, plus the untracked files policy allowed — kilobytes;
- **metadata** — bytes.

The repository itself is *not* in there. A receiving node clones it from origin or already
holds a mirror, which is what `Portability` is about. So a checkpoint is megabytes, not
hundreds of megabytes, and that changes what is affordable.

The second fact: **checkpoints are taken once per turn.** The set of blobs a node holds is
therefore the fastest-changing thing about it, and anything gossiped about that set is stale
almost immediately.

## Decision

**1. A blob is fetched by asking, not by consulting an advertisement.**

```
→ BlobRequest { hash }
← BlobFound { size } | BlobMissing
  <size bytes, raw, then the stream ends>
```

One stream per blob, raw bytes after a framed header — this is the traffic `offload-proto`'s
JSON framing was explicitly not designed for, and QUIC's streams are what stop a hundred
megabytes from delaying a failure-detector probe.

Availability is **not gossiped**. Four reasons, in order of weight:

- **The set churns every turn.** Gossiping it would make blob availability the largest and
  most frequently changing thing in the view, to answer a question that is asked rarely.
- **An advertisement is stale by construction.** A node that garbage-collected a blob a minute
  ago still appears to have it, so the fetch has to handle "actually, no" regardless — which
  means the ask exists either way and the gossip is a hint on top of it.
- **The holder is usually already known.** A checkpoint belongs to a run, the run's record
  names who held it, and that node is the first place to ask. Discovery only matters when that
  node is *gone* — and then the real question is whether anybody else has a copy, which is
  decided by (2) below rather than by gossip.
- **It is easy to add later and hard to remove.** A digest in the view, once nodes depend on
  it, is a protocol version to take back out.

Asking every member is O(fleet) small messages, which is nothing at tens of nodes and would
want revisiting at hundreds — the same scale assumption ADR-0005 already makes.

**2. Integrity comes from the hash, so a blob may be fetched from anyone.** The receiver
hashes what arrived and discards it if it does not match what was asked for. A member that
serves corrupt or malicious bytes achieves a retry, not a poisoned checkpoint. This is what
content addressing is *for*, and it means the fetch path needs no trust decision beyond the
membership check the transport already made.

**3. A checkpoint is not durable until a second node holds it.**

This is the substantive decision, and it is what makes phase 4's overnight demo possible at
all. A checkpoint that exists only on the node that died cannot be migrated from — no amount
of discovery finds a copy that was never made. So capture is followed by a **push** to at
least one peer, and the run records where its blobs are.

- **Who receives it:** a node that could plausibly take the run over — eligible by constraint,
  available, and preferring `Stable` over `Ephemeral`. The replica is then also the likely
  successor, so the common case is that the migration needs no fetch at all.
- **When:** immediately after capture, asynchronously. The agent's next turn does not wait for
  the network.
- **If nobody takes it:** the run is *marked* not-yet-durable rather than quietly treated as
  safe. `offload ps` shows it. A fleet of one has no second node by definition, and saying so
  is the honest form of a limitation that would otherwise appear as a lost run.

Megabytes per turn per run is the cost. That is affordable on a LAN and worth watching on a
metered phone, which is why the receiver's `WorkPolicy` gates it like any other work.

Measured, once it was built: a four-turn run on a one-file repository replicated a 10 KB
transcript at the first checkpoint and 20 KB plus a 183-byte patch at the last. Tens of
kilobytes, not megabytes — the estimate above is an upper bound for a large repository and a
long conversation, and the ordinary case is far cheaper than the decision assumed.

**4. Garbage collection stays local and conservative.** A node deletes what none of *its* runs
reference (already true). It does not consult peers, because a blob that matters to a run
somewhere else is a blob that node should be holding itself — that is what (3) is for.

## Consequences

Good:

- **Migration after an ungraceful loss becomes possible**, which is the case the whole phase
  exists for. The graceful path was never the hard one.
- **The fetch path has no trust decision in it.** The hash is the authority, so "which peer"
  is a performance question rather than a security one.
- **Nothing new is gossiped**, so the view stays what it is: who exists, what they can do,
  what they hold. The fastest-changing state in the system stays off the wire.
- **The common migration does no transfer at all**, because the replica was chosen for being a
  plausible successor.

Bad, and worth being clear-eyed about:

- **Every turn now costs a push.** One checkpoint per turn was chosen (ADR-0003) partly
  because content addressing made repeats nearly free *locally*; over a network they are not
  free, and a metered phone is exactly where this hurts. Policy gates the receiver, but the
  sender's cost is real and this is the decision most likely to want tuning.
- **Choosing the replica is a placement decision made twice.** The node picked here and the
  node that eventually wins the bid are chosen by similar-but-not-identical logic, and when
  they disagree the fetch happens anyway. Keeping the two rules from drifting is a maintenance
  cost, and merging them is a phase-5 question rather than a phase-4 one.
- **Asking every member does not scale**, and the fleet size at which that stops being true is
  not measured. It is the same assumption ADR-0005 makes about full views, so at least the two
  fail together and for the same reason.
- **A run can be durable and still unplaceable.** Holding the blobs is necessary and not
  sufficient: the successor also needs the repository (ADR-0003's portability) and a
  compatible agent version. "Durable" here means the checkpoint survived, not that the run
  will move.
- **It contradicts a line in `ARCHITECTURE.md`**, which is updated to point here.

## Accepted, after building it

Everything above is implemented and has been exercised on real daemons, including the case it
exists for: `kill -9` the holder, and the run resumes elsewhere from a copy the dead node had
pushed. The two costs flagged as worth a second opinion stand, and neither changes the
decision:

- **Every turn pays for a push.** Measured rather than estimated now: tens of kilobytes per
  turn on a small repository, against an upper bound of megabytes for a large one. The
  receiver's policy gates it, `[checkpoint] every_turns` is the dial for a metered link, and
  nothing has been observed that would justify checkpointing less often — which is the only
  other thing that could reduce it.
- **The replica is chosen by logic that overlaps with the bid.** Still true, still two rules to
  keep from drifting, and still a phase-5 question. What has changed is that the overlap is
  visible: `bid::replica_for` and `bid::evaluate` sit beside each other and read the same
  constraint, which is as close to one rule as they can get without the replica choice
  acquiring the bid round's cost.

Four things the code does that the decision did not say, recorded here rather than left for
somebody to find as a surprise:

1. **A node that could not host the run is still a better replica than nowhere.** The decision
   says the copy goes to a plausible successor; `replica_for` falls back to any available node
   when nobody is eligible. The run cannot move *there*, but the copy still survives the
   holder — and losing the only copy because the fleet happens to be a laptop and a phone is
   not an acceptable reading of a durability decision.
2. **"Accepted" is not "has it".** The push waits for `BlobStored`, sent after the bytes are on
   the receiver's disk, rather than for the agreement to receive them. A sender that stopped at
   accepted would report a checkpoint durable while it was still in flight, which is precisely
   the claim this ADR exists to make true.
3. **The receiver's gate is narrower than admission.** Holding a replica is not hosting a run:
   a busy node is a perfectly good place for a copy, so `accepts_replica` checks what
   replication actually costs — somebody else's bytes over a metered link, disk on a device
   that is nearly flat — and nothing else. It is a second copy of a policy rule, and it
   promptly drifted: a plugged-in laptop reporting 0% battery refused every push for a day.
   There is a test whose only job is that case.
4. **A checkpoint on the holder is not a copy.** `is_durable` excludes the holder from its own
   replica set, so a fleet of one reports `here only` rather than counting itself.

## Alternatives

**Gossip availability, as originally written.** Fetching then needs no discovery round, and a
node can prefer a nearby holder. Rejected on churn: with a checkpoint per turn, the advertised
set is the most volatile state in the fleet, and it is advertised to answer a question that
arises only when a node has died. If fetches ever become frequent enough to notice the ask,
this comes back as a digest — the fetch path does not change.

**Replicate to every node.** Simplest durability story and the fetch becomes unnecessary.
Rejected as an obvious multiplier on the cost that is already the worst part of this decision:
a fleet of six would move every turn's transcript six times, to five nodes that will never run
it.

**Replicate nothing; migrate only what the departing node hands over.** Correct for a drain,
and free. Rejected because it makes the ungraceful case unrecoverable, and the ungraceful case
is a closed laptop — the single most likely event in this system.

**Erasure-code checkpoints across peers.** Cheaper than full replicas at high replication
factors. Rejected as machinery for a problem this fleet does not have: at one replica of a
few megabytes, the coding overhead exceeds what it saves.

**Push the repository mirror too, so a cold node needs no origin.** Tempting for
`Portability::NodeLocal`, and rejected: mirrors are hundreds of megabytes, they are not
run-specific, and a node that wants one can clone it from the node that has it as a normal
repo operation. The eligibility rule already refuses to place a local-path run somewhere cold,
which is the honest answer rather than a hidden transfer.
