# ADR-0038: A node that does no work — introduction as a member's job, not a service's

**Status:** accepted · 2026-08-27 · builds ADR-0037 §2/§4 · extends ADR-0015 §1's resolver ·
needs a wire bump when built · unbuilt

## Context

ADR-0037 settled the order in which to make a phone reach a laptop from outside the LAN, and left
two shapes for the case where a static home address is not available. The owner chose, and the
choice is a design constraint worth taking literally:

> I like the directory idea… What is important is that we design it such that it keeps the
> information safe. Also it should be optional to use not required.
>
> It should act like any other node, just that it doesn't do any work itself.

That last sentence rules out the thing this ADR was going to be. The alternative on the table was a
non-member record store — signed, sealed, hostable on a bucket or a cloud function, holding
rotating labels it could not link to keys. It has a better privacy story and it is a **second
protocol, a second trust model, a second thing to enrol, an AEAD dependency this tree does not have,
and a component `offload nodes` cannot show you.** One component model is worth a lot, and the owner
is hosting the box either way.

### The fact that decides the mechanism

Being a member is *not* enough to make a peer dialable, and this surprises people:

ADR-0015 §1 keeps every address inside `offload-transport` — none reaches `ClusterView`, a bid, an
assignment or the store. So a phone that gossips happily with a VPS learns that the laptop **exists**,
its capabilities, its status and its runs, and still cannot dial it: `Transport::connect` asks a
`Resolver` for addresses, and the only two sources are mDNS (LAN only) and the configured seed list.
A member with a public address therefore solves the *rendezvous* problem only if something teaches
addresses across it.

The good news is that the extension point already exists and is already the right shape:

```rust
/// The only place in the system that knows about addresses (ADR-0015). mDNS and a seed list
/// are the two implementations today.
fn addresses(&self, node: NodeId) -> Vec<SocketAddr>;
```

An introducer is a third source for that trait. Nothing above the transport changes, which is the
whole return ADR-0015 §1 was bought for.

### Why this is cheap, and it is not the reason people expect

**An address is routing, not authority.** A wrong, stale or malicious introduction cannot
impersonate anybody: the handshake still demands a certificate the fleet key signed *and* proof of
possession of the key it names (ADR-0015 §4), and `connect` already refuses a peer that answers with
the wrong key — "belt and braces", in the code's own words. So the worst a hostile introducer
achieves is a wasted dial or a withheld one. **Denial of service, never hijack.** That is what makes
it safe to accept routing hints from a machine in somebody else's datacentre, and it is why this
needs no new cryptography at all.

## Decision

### 1. It is an ordinary member that does no work

No new binary, no second enrolment, no second protocol. `offloadd` on a VPS with
`accept = "never"` and no `host-runs`, enrolled the way any device is (`offload invite`, on the LAN,
once). It appears in `offload nodes`, its certificate expires and renews, revocation reaches it, and
`offload status` on it says what it is. That configuration is already supported and already walked —
it is exactly what the second daemon in session twenty-nine's resource walk was.

### 2. What it adds is **introduction**, and addresses stay in the transport

Two new `ClusterMessage`s, exchanged between connected peers and never gossiped into `ClusterView`:

- **"Where is key B?"** — answered from what the answering node has observed: the address a peer
  connected *from*, which every server learns for free from the connection itself.
- **"Tell B to dial me at these candidates"** — the 1999 reverse-connection trick, for the case the
  asker cannot be dialled. Napster's push, eDonkey's callback, BitTorrent's holepunch extension.

The answer feeds a third `Resolver` source. A node consults, in order: **mDNS, the configured seeds,
then whatever a connected peer can introduce.** Any member may answer; a node with a public address
is merely the one that is always reachable, so no election, no supernode protocol, and no
configuration naming which peer is "the introducer" — `Stability` and reachability already decide it
in practice.

### 3. Optional, and last

A fleet with no such member behaves exactly as it does today. A fleet whose introducer is down keeps
working on the LAN and on configured seeds. **Nothing may become unreachable because a box in a
datacentre is down** — the moment that is false, a €4 VPS has become the centre ADR-0002 refuses. It
is a hint source, in the position where a hint that fails costs one dial attempt.

### 4. It must not be given the run plane, and today it would be

This is the cost of "act like any other node", stated plainly rather than discovered later. A member
takes part in run gossip, so a public box would hold **every run record in the fleet, prompt
included** — and the door grants `{Submit, Deliver}` (ADR-0012), so a compromised introducer could
also submit runs to the machine holding every credential. That is a large grant for a machine chosen
because it was cheap and exposed because it must be reachable.

The fix is not a new mechanism but an existing rule applied one place further: **a grant is enforced
where it is used, against the certificate as it stands right now.** So:

- `offload invite <node> --introducer` mints a certificate with **no run-plane grants** — not
  `Submit`, not `HostRuns`. ADR-0012's door currently grants `{Submit, Deliver}` unconditionally,
  and this is the first case that wants less than the door gives.
- The paths that hand over run content — the gossip tick's run records, `Request::Logs` proxying,
  `fetch_blob` — withhold from a peer whose certificate carries no run-plane grant. Membership and
  liveness gossip continue, because that is the part an introducer needs and the only part.

An introducer then knows: which keys are in the fleet, their addresses, and when each is online. That
is a real disclosure and it is the honest floor for anything that can introduce peers at all — it is
strictly what a rendezvous *is*. What it no longer knows is what any of them are being asked to do.

### 5. It holds addresses in memory, keeps no history, and logs none

Observed addresses live as long as the connection and the resolver cache; nothing is written to the
store, nothing is gossiped, and the reference deployment must not log peer addresses. A directory
that keeps a presence log is the harm; one that answers from live connections cannot produce one.

### 6. Not forwarding, and not punching

Introduction only. Relaying ciphertext (ADR-0037 §4) and hole punching (§5, iroh) stay where they
are. This keeps the component non-amplifying — it answers a question, it does not copy bytes — and
it means the failure mode when neither end is reachable is an honest "no route" rather than a slow
path nobody chose.

### 7. It exists first, and it is not the founder

The owner's observation, and it is the right setup order: *"if you want to start a cluster that is
going to be reachable from the internet, spin one of those up first and then join all the other
devices to it."* Every device then names one static address in `seeds` that never changes, and a
device that has never been on the fleet's LAN can still reach it. Worth writing down because it is
invisible from the code and expensive to rediscover.

**Founding on it, however, is exactly backwards**, for two reasons that are both in this repository
already:

- `Terms::founding` grants `{Submit, Deliver, HostRuns, Approve}` with `probation: false`. That
  hands the exposed box the two grants §4 exists to withhold, *plus* the authority to enrol devices
  and renew certificates — so a compromised introducer could admit an attacker's device with
  `Submit`, and §4 is what that means on the machine holding the credentials.
- `offload init` derives the fleet key from the passphrase **on the machine that runs it**. That is
  the secret ADR-0012 keeps in a drawer, and CLAUDE.md's rule about it — never an argument, never an
  env var, read from a terminal without echo — is about exactly this: not putting the fleet's root
  where it does not need to be. An SSH session into a rented box is where it does not need to be.

**And giving it up costs nothing, because enrolment needs no network.** An invitation is a
certificate naming the joining device's key, and `offload join --token` is offline — ADR-0012's
central claim, and the reason "join the devices to it" was never a network operation in the first
place. What the introducer's existence actually buys is a `seeds` entry, which is free whoever
founded.

So the order is:

1. `offload init` on the **laptop**, at home, passphrase back in the drawer.
2. `offload invite <vps> --introducer` — the certificate from §4, carrying less than the door
   grants.
3. That address in every other device's `seeds`, permanently.
4. `offload grant approve` on the **phone**, once, at home. The issuing authority you can reach
   while away should be the device in your pocket rather than the public box — and it answers the
   single-approver warning `offload status` already prints.

What a seized introducer discloses is bounded by this: `FleetState` persists **no private fleet
material** — the fleet *public* key, this node's certificate, its chain, its approver delegation if
it has one, and revocations. The signing key is derived from the passphrase and never written down.
So the disk yields that box's own credential, which is why step 2 makes that credential as small as
possible and step 4 keeps it from being an approver.

## Consequences

- The owner's IP-lease problem goes away without dynamic DNS: `seeds` on the phone names the VPS,
  whose address is static, and the laptop's *current* address is learned by introduction. Both ends
  may move.
- ADR-0037 §2's DDNS half becomes optional. Its **re-dial tick is still required** and is still the
  first thing to build: a phone that lost its link must dial again, whatever taught it the address.
- A wire bump when built, and no schema change: introduction is transport traffic, and by
  construction none of it is durable.
- `--introducer` is the first certificate that wants **less** than the door grants, which is a small
  amendment to ADR-0012 rather than a new grant type — worth noticing because it is also the answer
  to ADR-0037 §6's second open question, from the other end: if a certificate can carry less than
  `Submit`, a stolen phone can be issued one too.
- Nothing above `offload-transport` changes. Fifth time ADR-0015 §1 has paid for itself.
- The setup order in §7 is the fleet's *documented* one for an internet-reachable fleet, and it
  inverts the obvious reading: the reachable node comes first in time and last in authority.

## What this deliberately leaves

**The sealed-record design, and it is worth keeping in the file.** A signed, sealed record under a
rotating label — hostable on a bucket, a cloud function, or any static host, holding data its host
cannot read — discloses strictly less than a member can, because it is not a member. It was rejected
for one-component simplicity and not on the merits, so if the introducer's disclosure ever turns out
to matter more than the operational cost, this is the shape to come back to. What it needs that the
tree does not have: an AEAD, a versioned label derivation with a pinned known-answer test (the fleet
key derivation's rule, which applies to anything compared across nodes), and a rotation window
chosen against clock skew.

**Whether a revoked device stops being able to ask.** Revocation reaches an introducer like any peer,
so an evicted phone is refused at the handshake — but until it is revoked *or* its certificate
lapses, it can ask where the fleet's devices are. That is ADR-0012's existing best-effort-until-expiry
posture, and `offload rekey` is still the strong eviction.

**No measurement.** Every claim here about what a phone's network does is a prediction until
`offloadd` is running in Termux on mobile data. ADR-0037 §0 comes first: with routable IPv6 on both
ends, an introducer is a convenience about *changing* addresses rather than a way through a NAT.
