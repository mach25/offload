# ADR-0060: A fleet a peer will not admit us to is reported, not believed

**Status:** accepted · 2026-09-11 · built and measured in the session that decided it · extends
ADR-0044 to the refusal that carries no proof · does not amend ADR-0012 or ADR-0002

## Context

`offload rekey` is the **convergent** revocation (ADR-0012). Ordinary revocation has to reach every
node; a rekey does not have to reach anybody, because the evicted device is simply not in the new
fleet. That is its whole value, and it is why a fleet can be recovered from a device that is off.

The consequence is that the evicted device is told **nothing**. It goes on running, its certificate
still verifies against the fleet it still believes in, and its dials start coming back refused.
Measured on two meshed daemons, with the rekey typed on alpha while both were live:

```
06:03:19  alpha  WARN  this fleet has been re-founded under a new key — every other device needs
                       the invitation `offload rekey` printed, and until then this node is alone
06:03:19  bravo  INFO  no answer: suspecting node=cf869d98
06:03:24  bravo  INFO  no answer from anybody: marking dead node=cf869d98
06:03:25  bravo  WARN  could not reach seed address=127.0.0.1:47831
                       error=refused by cf869d98: that certificate is for fleet bf36a64a,
                                                  this is fleet 433de62b
```

The mechanism is right: three seconds from the rekey to a fleet that no longer talks, in both
directions, with no message needing to arrive. What is wrong is every report bravo has.

```
$ offload nodes                    # on bravo
 cf869d9847d6  alpha   dead

$ offload status                   # on bravo
fleet       bf36a64a  ·  2 member(s) met, 1 approver(s)  ·  …
note: only one approver (alpha) — losing it means the next enrolment costs a rekey.
```

Alpha is not dead; it is running and answering, and it answered bravo's dial with a precise
diagnosis. Bravo's fleet does not have two members. And the one note it prints warns about losing
an approver that has *already gone*, which is the exact event that just happened, described in the
future tense.

The honest sentence exists. It is produced every twenty seconds, and it goes to a log.

This is the sibling of ADR-0044. That one closed the path a **revoked** node's eviction reaches it
by: `Refusal::Revoked` carries the signed `Revocation`, the subject verifies it against the fleet
key it already holds, and acts. `Refusal::WrongFleet` is the other refusal a dial can come back
with, and it is different in the way that matters: **there is nothing to sign.** A re-founded fleet
produces no revocation, because nobody was revoked — a new fleet was founded and this device was
not put in it. There is no credential that could make the claim checkable.

## Decision

### 1. It is reported and never acted on

A node must not believe a peer about itself. That rule is what makes membership safe here, and
pointed at a refusal it is load-bearing: if a device could be talked out of its fleet by a peer
saying "you are in the wrong fleet", any device could evict any other by saying so. ADR-0044 gets
past the rule with a signature. This has none, so it does not get past it.

Nothing about this node's membership, grants, status or behaviour changes. `offload verify` still
passes against the old phrase, the certificate still verifies, and the daemon goes on dialling.
That is correct: a device that cannot reach its fleet because of a partition and a device that has
been rekeyed out of one are indistinguishable from inside, and ADR-0002 chose to keep running in
the first case deliberately.

### 2. What *is* this node's own to state

Two things, and neither is a claim about membership:

- **A handshake was refused.** This node dialled, reached something, and was turned away. That is
  an observation of this node's own socket, and it is a different fact from `no answer` — which is
  what every report said instead.
- **What the peer said while refusing.** Stored and printed as the peer's words, attributed, never
  reworded and never resolved into a verdict. The same discipline ADR-0059 applies to an errno.

### 3. A count and a breakdown, per peer, since this daemon started

`Turnaways` mirrors `sends::SendRefusals` exactly: a total, plus a `HashMap` capped at sixteen
peers holding a count and the last reason. Both numbers are reported, because past the cap they
disagree and the total is the honest one.

It is keyed by **`NodeId`**, unlike the send counter's address: a handshake refusal comes back
from a peer that presented a certificate and identified itself, so the id is known and is what
`offload nodes` and every other membership command take. Where the dial was a seed and the
address is all there is, that address is carried in the reason, which is the peer's own sentence.

Not persisted, not gossiped, not merged. It is an observation about this process on this machine,
which is the one class of fact a peer must never be believed about — ADR-0059's argument, and the
reason neither of these counters is a gossiped field.

### 4. One note in `offload status`, and only when there is a number

```
turned away 3 handshake(s) refused by a peer — this node reached it and was not let in.
            └─ cf869d9847d6  ×3  ·  that certificate is for fleet bf36a64a, this is fleet 433de62b
```

Above the `fleet` line, for the reason ADR-0059 §4 puts the send count there: that line says how
many members this node has *met*, and a node nobody will admit needs its answer to that explained
rather than trusted.

And a note, because the actionable half is not the count:

> a peer refused this node's certificate saying the fleet has moved on. If you ran
> `offload rekey`, this device needs the invitation it printed — `offload join --token …`. This
> node cannot check that claim, and nothing here has acted on it.

The last sentence is not decoration. It is the difference between a report and a verdict, and
somebody reading the first two sentences at three in the morning is entitled to know which one
they are holding.

### 5. Refused: believing it, and asking about it

**Marking this node evicted, or standing down.** This is ADR-0044's behaviour applied to a claim
with no proof, and it is exactly the attack the succession design exists to stop. A device that
stood down when told to would be a device any peer could switch off.

**Dialling the peer to ask which fleet it is in now.** There is no answer it could give that this
node could check — the new fleet's key is by construction one this device does not hold — and an
unverifiable answer obtained on purpose is worse than one that arrived unasked.

**Marking the peer unreachable, or dropping it from the view.** A second liveness mechanism beside
the failure detector, computed from a different input. Session sixty-nine's rule, and the most
expensive one in this tree.

## Consequences

- A device rekeyed out of its fleet can be identified **from that device**, in one command, which
  was previously possible only by reading the daemon's log.
- `offload nodes` still says `dead` for such a peer, and deliberately: `Status` is the failure
  detector's and nothing else writes it. The explanation sits beside it rather than inside it.
- A partitioned device and a rekeyed-out device still look the same from inside, because they are
  the same. What changes is that a *refused* dial no longer looks like silence.
- The note fires on any `WrongFleet` refusal, which includes the ordinary case of a node dialling
  a seed before it has joined. That is not a false positive: "that machine will not admit your
  certificate" is what somebody mid-enrolment most needs to read.
