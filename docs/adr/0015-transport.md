# ADR-0015: Dial a public key, not an address — QUIC we own now, iroh as a phase-5 decision

**Status:** accepted · 2026-07-27 · settles phase 3's transport, and writes down what will
settle phase 5's

## Context

Phase 3 is two nodes seeing each other, and it needs a transport. The roadmap has said
"`Transport` trait; QUIC via `quinn`" since phase 0 and has also carried, since ADR-0012, a
note that `iroh` deserves evaluating and an ADR before choosing. This is that ADR.

What the transport has to do falls out of decisions already made:

- **Dial by identity.** ADR-0012 made `NodeId` an ed25519 public key, so "connect to this
  node" is naturally "connect to this key". An address is a routing detail, and a routing
  detail that reaches `ClusterView` or an assignment is one that has to be kept correct
  everywhere a node changes networks — which, for a laptop that moves between a desk and a
  café, is constantly.
- **Refuse strangers in the handshake.** A peer presents its membership certificate and is
  verified against the fleet public key we already hold. That has to happen before it can say
  anything else, or an unenrolled node gets to make us parse its messages first.
- **Many small messages and a few enormous ones, on the same link.** Failure-detector probes
  and gossip are tens of bytes and timing-sensitive; a repo bundle is hundreds of megabytes.
  On one TCP connection the bundle blocks the probes and the failure detector starts lying
  about liveness — the exact input ADR-0007's drop-off policy depends on. Independent streams
  are the requirement; QUIC is how you get them without opening a connection per message.
- **A LAN with no internet today, a phone on mobile data in phase 5.** Phase 3's demo is three
  daemons on one network. Phase 5's is a phone joining from outside it, which is a different
  problem entirely: NAT traversal, relays, and discovery that does not depend on multicast.
- **It runs on a phone.** Dependency weight, battery and background execution all matter more
  here than they would in a server daemon.

The two candidates are not the pair most people picture:

- **`quinn` 0.11** — QUIC over rustls, fifteen dependencies, nothing above the connection. We
  write the endpoint setup, the identity pinning, discovery, and — in phase 5 — anything that
  gets through a NAT.
- **`iroh` 1.0** (July 2026) — dial-by-public-key, hole punching, relays, and several
  discovery mechanisms. Its `NodeId` *is* an ed25519 public key: the identity model this
  project arrived at independently, already built. Worth knowing before reasoning from an
  older impression of it: as of 1.0 it no longer sits on quinn but on `noq`, n0's own QUIC
  stack; it requires `ed25519-dalek` 3.0.0-rc while this workspace is on 2.x; and it brings
  roughly 45 crates, including an HTTP client and a DNS resolver.

The tension is one of timing rather than merit. **Phase 3 needs only what the two have in
common. Phase 5 needs precisely where they differ.** Choosing now means choosing on
requirements nobody has met yet; refusing to choose means writing an abstraction before its
second implementation exists, which is the standard way to get the wrong one.

## Decision

**1. The transport's address is a `NodeId`, and nothing above `offload-transport` knows
otherwise.** No IP, no port, no relay URL in `ClusterView`, in a bid, in an assignment, or in
the store. Resolving a key to somewhere to send packets — mDNS today, a seed list, DNS or a
relay later — is the transport's business and changes when the network does. This is the
single property that makes the phase-5 question answerable later instead of now: whatever
answers it, the code above it does not change.

**2. `Transport` is a trait, and its first implementation is in memory.** Not for
portability — for tests. ADR-0005's merge rules and ADR-0007's drop-off policy are the two
things in this system that most need testing under partition, reordering and churn, and a test
that opens sockets is testing the kernel's timing more than it is testing the merge. An
in-memory transport with a controlled clock makes those tests deterministic, which is the same
argument that keeps `offload-core` pure (ADR-0001), one layer out.

The trait stays as small as QUIC itself: connect to a `NodeId`, open a bidirectional stream,
accept one, and nothing else. No broadcast — fan-out is `offload-cluster`'s decision, not a
transport primitive. No request/response convenience — that belongs to `offload-proto`, which
is where a message gains a meaning.

**3. Phase 3 ships quinn.** The credential is the node key: TLS 1.3 in which the peer's key
*is* its identity — RFC 7250 raw public keys if rustls supports them cleanly, otherwise a
self-signed certificate whose subject public key is the node key, verified by pinning. Which
of those two is an implementation detail. What this ADR fixes is that nothing else is
trusted: no certificate authority, no hostname, no address, no name a peer asserts about
itself.

**4. Membership is checked in the handshake, and the check has two halves that are easy to
mistake for one.**

- The peer presents its `MembershipCert`, plus the `Delegation` behind it if an approver
  issued it, and we verify the chain against the fleet public key we already hold — offline,
  at first contact, no roster (ADR-0012).
- **The TLS credential must be the key that certificate names.** A certificate is public and
  copyable: it sits in `fleet.json`, it will travel in gossip, and it is meant to be shown to
  strangers. What makes it non-transferable is possession of the private key it names, proven
  by the handshake. Without that binding it is a bearer token, and anyone who reads a backup
  wears it.

A peer that is unenrolled, revoked, or expired is refused there, before it can send anything.
Revocation is immediate and local (ADR-0012): a revoked peer's connection is dropped and new
ones refused without waiting for the fleet to converge.

**5. iroh is deferred to phase 5, and these are the questions that will decide it** — written
down now so that it is re-decided on evidence rather than on whichever impression is handy:

- **Does the fleet need to work off the LAN at all?** A phone on mobile data is phase 5's
  demo. If the honest answer is "the devices are all on one network", iroh's whole advantage
  is unrealised weight.
- **Is a relay acceptable, and whose?** Recorded here because it is a decision, not a default:
  **no third-party relay.** ADR-0002's "no centre" is about authority, and a relay is not a
  coordinator — but it is a metadata centre that learns which of the owner's devices talk to
  which, and when. If iroh arrives, its relays are self-hosted or explicitly opted into.
- **Has the crypto dependency converged?** Two `ed25519-dalek` versions in one binary is
  tolerable — keys cross as bytes — but this crate is the centre of the trust model, and
  taking a release candidate for it is not the same as taking one for a JSON parser.
- **How much of it would we otherwise build?** The honest answer today is that hole punching
  and a relay protocol are weeks of work that end in a subtle NAT bug. That is the strongest
  argument for iroh and it should be weighed at full strength when the requirement is real.

The trade, in one line: adopting iroh later throws away the endpoint setup and the pinning
code — a few hundred lines, with the framing, handshake, and everything above them untouched.
Adopting it now takes 45 crates, a pre-release crypto dependency and somebody else's relay
defaults into a phase that needs none of them.

## Consequences

Good:

- **Phase 3's dependency surface stays small enough to reason about**, which matters most on
  the device where this is hardest to run at all.
- **The partition tests exist from the first commit.** A memory transport that arrives with
  the trait is one that gets used; one retrofitted after the QUIC implementation is one that
  never quite matches it.
- **The phase-5 choice is a swap, not a migration**, because dial-by-public-key is the same
  model iroh has. It also leaves room for both — LAN over quinn, off-LAN over iroh — if that
  ever turns out to be worth the complexity, without designing for it now.
- **None of the identity work is wasted either way.** Binding the TLS credential to the node
  key is required whatever QUIC stack is underneath.

Bad, and worth being clear-eyed about:

- **This is an abstraction written before its second implementation.** That is how wrong
  abstractions get locked in. The mitigation is to keep the trait to what QUIC already
  offers and refuse to add anything convenient to it; if it grows a method that iroh cannot
  satisfy trivially, the abstraction has failed and should be collapsed rather than patched.
- **Some quinn work is thrown away** if iroh wins in phase 5. Bounded, and known in advance,
  which is the difference between a cost and a surprise.
- **Deferring does not make NAT traversal cheaper.** We will either write it or adopt iroh;
  the deferral buys information, not work avoided, and it will feel like a debt when phase 5
  arrives.
- **A hand-written certificate verifier is a silent, total failure when it is wrong.** One
  that returns `Ok` too eagerly accepts everybody and looks exactly like one that works. The
  test that matters is that a peer presenting the *wrong* key is refused — not that the right
  one is accepted.
- **iroh 1.0 is more stable than most opinions of it.** A deferral risks re-litigating this in
  phase 5 against a stale impression, in either direction. The criteria above exist to make
  that conversation short.

## Alternatives

**Adopt iroh now.** Genuinely tempting, and the closest call in this document: the identity
models match exactly, phase 5's hardest connectivity work disappears, and 1.0 means the API
is no longer a moving target. Rejected for phase 3 on timing rather than merit — a
pre-release crypto crate at the centre of the trust model, a QUIC stack that is now n0's own
rather than the widely-deployed one, and relay defaults that would need turning off on day
one, all in service of a capability this phase does not use. None of that is disqualifying,
which is exactly why it is written down as a phase-5 decision instead of a rejection.

**TCP with our own framing.** Fewer moving parts, and wrong for one specific reason: a single
connection carrying both a repo bundle and a failure detector's probes makes the probes'
timing meaningless, and the drop-off policy is built on that timing. Multiplexed streams are
the feature here, not the buzzword.

**libp2p.** Everything iroh offers plus a peer identity model, a DHT, and a stack of protocol
opinions this project has already decided differently about — ADR-0005's gossip is a merge
rule of our own, ADR-0006's placement is ours. Adopting it means adopting its opinions and
translating between two identity models forever.

**A WireGuard mesh underneath — Tailscale, Netbird, or hand-rolled — and plain sockets above.**
Works today, gives phase 5's property immediately, and is genuinely the right answer for some
owners. Rejected as a *design* dependency because it puts an account and a control plane the
fleet does not own underneath a system whose first decision was to have no centre. Nothing
here prevents an owner from doing it anyway, and the transport should stay indifferent to
whether the network under it is one.

**No abstraction — quinn types throughout.** Fewer lines today and honest about only having
one implementation. Rejected because it makes the memory transport impossible, and with it the
partition and churn tests that are the main reason this system's hard parts are testable at
all.
