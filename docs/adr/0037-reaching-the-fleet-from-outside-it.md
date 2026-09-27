# ADR-0037: Reaching the fleet from outside it — one reachable end, and what that costs

**Status:** accepted · 2026-08-27 · builds on ADR-0015 §5, which deferred this · does **not**
decide iroh · opens two security questions for phase 5

## Context

The requirement, from the owner, and it is the first statement of it that is not a hypothesis:

> Everything needed to reach servers and code is on the laptop. Sometimes, away from it, a support
> email arrives asking about the status of a site — sweep the shops, check the logs, clear a cache,
> get a change to test or prod. I would like to do that from my phone.

Phase 5's demo has always been "a phone joining from outside the LAN". This ADR is what to build
for it, in what order, and it deliberately stops short of the thing ADR-0015 left open.

### What is already true, and does more than it looks

**Enrolment needs no internet.** Both devices are on one network when the fleet is formed. mDNS
finds them, and a membership certificate verifies against the fleet key offline at first contact
(ADR-0012) — no roster, no server. So the security-critical moment happens at home, once, and
nothing about it has to work over the internet ever again.

**Dial-by-key.** ADR-0015 §1 keeps every address inside `offload-transport`: none reaches
`ClusterView`, a bid, an assignment, or the store. Everything below is therefore transport work,
and nothing above it changes whichever way this goes.

**The asymmetry is real and mostly harmless.** The owner is right that the laptop cannot dial the
phone. But of the three things this use case needs, two are phone-initiated and the third does not
involve the phone's network at all:

| what | who opens the connection | needs the phone reachable? |
|---|---|---|
| submit a run | phone | no |
| approve or deny a question (ADR-0017) | phone | no |
| hear the result | the laptop's own **sink**, outbound | **no** |

A sink is a program the laptop runs (ADR-0010) — mail, ntfy, Pushover, a webhook. News reaches the
phone the way every other app's news does: outbound, through somebody else's push infrastructure,
with no listener on the phone and no wake lock. Only `offload logs -f` wants the link up, and the
phone opens that too.

**So one reachable end is enough, and it is the laptop.** Which is what Napster shipped in 1999,
and what BitTorrent tolerated NAT with for years: you need one connectable side and a way for the
other to dial out. The rest of the P2P canon — reverse connections (`PUSH`, "Low ID", BEP 55),
elected supernodes with open ports, UPnP/NAT-PMP, UDP hole punching, and relaying through a
peer — exists for the cases where *neither* side is connectable, formalised as ICE. None of that is
needed by a home router the owner administers.

### Two gaps found by reading, and they are exactly what the owner's question turns on

The owner asked: *what if I lose the lease on my IP and get a new one?* The answer today is that
the phone never reconnects, for two independent reasons.

1. **A seed must be a literal address.** `Mesh::bootstrap` does `seed.parse::<SocketAddr>()` and
   logs `seed is not a host:port address` for anything else. Dynamic DNS is the standard answer to
   a changing home address, and a DDNS name cannot be configured.
2. **Seeds are dialled exactly once, at startup.** `Bootstrap::run` is called from `main` and
   nothing calls it again. Compare the LAN path: `Mesh::discover`'s own doc comment says it "runs
   until the process ends", so a peer that changes address on the LAN is re-found for ever. **The
   self-healing exists for multicast and not for the internet** — so a phone that loses the link to
   a cell handover, to Doze, or to a new home address is stuck until `offloadd` restarts.

Underneath both: no `keep_alive_interval` is configured on the QUIC endpoint, so a connection
survives on SWIM probe traffic. When the phone sleeps, the probes stop, the NAT mapping lapses and
the link has to be re-made by the phone — which is (2) again, and is the ordinary case rather than
an edge one.

## Decision

### 0. Measure IPv6 first, before building any of this

Mobile networks are largely IPv6-native, and two globally addressed endpoints need no traversal at
all — the entire problem is a NAT artefact. What to measure: the address phone gets on mobile
data, and whether the home connection has routable IPv6. quinn is indifferent. If both are v6, the
rest of this ADR is a fallback rather than a plan.

### 1. Enrolment stays on the LAN, and off-LAN joining is not a goal

Nothing to build. A device joins at home, where mDNS works and where the passphrase or an
invitation is being handled by somebody standing there.

### 2. One reachable end — the laptop — and two small things to build

Forward one UDP port to the laptop; the phone always dials out. Then:

- **`seeds` accepts a name, resolved at dial time and on every attempt.** Not once at startup: the
  point of a DDNS name is that what it resolves to changes. Resolution is transport business per
  ADR-0015 §1, so nothing above learns an address.
- **Seeds are re-dialled on a tick while no peer is live**, with a backoff, stopping as soon as one
  answers. This is the mDNS loop's property, granted to the path that has none. It is what makes a
  new home IP, a cell handover and a phone waking up all the same event: *dial again*.

Explicitly **not**: any inbound reachability on the phone. An `Ephemeral` node advertises nothing
off-LAN and listens for nothing.

### 3. News leaves through a sink, and nothing is held open to receive it

Written down because the obvious implementation is a connection kept alive to push notifications at
the phone, which Android's Doze will kill and a battery will notice. The delivery plane already
makes that unnecessary (ADR-0010), and this ADR refuses to build the other thing.

### 4. A relay you host — deferred, with its shape and its trust model recorded

**Superseded in part by ADR-0038**, which settles the *rendezvous* half as a member that does no
work rather than as a non-member service. What survives here is the forwarding half, for the case
where neither end can be dialled at all.

For the case step 2 cannot reach: CGNAT *at home*, so no port can be forwarded.

The first draft of this section said a VPS could join as an ordinary `Server`-class member and that
this "adds no new trust". **That is wrong, and it is worth recording why**, because it is the
mistake the owner's own suggestion corrects. A member holds a certificate and therefore takes part
in gossip — and gossip carries `Run` records, which carry the **prompt**. So a box in somebody
else's datacentre would read every prompt in the fleet, and holding `{Submit, Deliver}` from the
door (ADR-0012), it could submit runs to the laptop that holds every credential. That is a large
grant for a machine chosen because it was cheap.

**A relay that forwards bytes grants none of it.** The peers' QUIC session is end-to-end TLS 1.3
between two node keys; a relay that copies those datagrams cannot decrypt them, cannot inject into
them, and needs no fleet credential to do its job. What it learns is metadata: which key contacts
which, when, and how much. That is a real disclosure and a far smaller one, and it is bounded by
being *the owner's own machine* — which is what ADR-0015's "no third-party relay" was reaching for.

The load-bearing property, and the reason this is cheap: **`NodeId` is an ed25519 public key**, so a
peer can register with a relay by *signing its own registration*. The relay verifies that the
claimant holds the key it claims — with no fleet key, no roster, and no knowledge of this fleet at
all. A relay slot is bound to a key and only its holder can claim or refresh it. There is no secret
to configure on either side, which is what keeps a relay from becoming a second thing to enrol.

So the shape, when it is built:

- **A separate binary that is not a fleet member** and holds nothing: no certificate, no fleet key,
  no run state, no store.
- **Two services.** *Rendezvous*: give key A the candidate addresses key B registered, so the two
  can punch directly (Napster's push, eDonkey's callback, BitTorrent's holepunch extension, ICE's
  signalling — the same 1999 trick each time). *Forward*: when punching fails, copy ciphertext
  between two registered sockets, which is what makes the path always work.
- **Never a dependency.** A relay that is down must cost nothing: the LAN keeps working, a
  forwarded port keeps working, and a direct path is always preferred when one exists. The moment
  the fleet cannot operate without it, it has become the centre ADR-0002 refuses.
- **Never an open reflector.** Forward only between two keys that have both registered and both
  proved possession, with a bandwidth cap. An unauthenticated UDP forwarder is a DDoS amplifier,
  and this is the one part of a relay that is dangerous to get wrong.
- **Direct paths beat relayed ones for bulk.** A repo bundle is hundreds of megabytes and a cheap
  VPS's transfer allowance is not; relaying a migration should work, be slow, and be visible rather
  than silent.

**Not a cloud function.** Asked directly, so answered directly: a relay needs a long-lived UDP
socket and per-key state, and serverless request/response runtimes (Lambda, Cloud Functions,
Workers) offer neither raw UDP nor a socket that outlives a request. What works is anything with a
public address and an always-on process: the cheapest VPS, a small container host, or a machine the
owner already has somewhere else.

Deferred rather than built, because step 2 covers the network the owner actually has, and because
the thing most likely to make this unnecessary is step 0.

### 5. Hole punching and relays remain ADR-0015's iroh question

Three of its four criteria are now answered: off-LAN is a real requirement (this ADR's context),
relays must be the owner's own (already decided), and hand-rolling traversal "ends in a subtle NAT
bug" (already conceded). The open one is whether `ed25519-dalek` 3.0 has left release-candidate.
Deliberately still not decided, because everything above is achievable with a router — and because
§4 changes the calculation: iroh's strongest offer is punching *plus relays*, and a relay the owner
hosts is the half this project has already decided it wants. What iroh would still save is the
punching itself, which is the part ADR-0015 rightly calls a subtle NAT bug waiting to happen.

### 6. Security: what an exposed port exposes

**Not** a risk, and specifically:

- There is no password and no shared secret on the wire, so there is nothing to phish and no
  authority to compromise. A peer presents a certificate the fleet key signed, and the TLS
  credential must be the key that certificate names (ADR-0015 §4) — a certificate is public and
  copyable, and possession of the private key is what makes it non-transferable.
- A stranger is refused **in the handshake**, before it can send a message this daemon parses.
- Revocation is checked at every handshake and drops live sessions; certificates last thirty days
  and are renewed on contact, so a device that stops being renewed stops being a member.

**Is** exposed, plainly:

- **A pre-authentication QUIC/TLS surface**, reachable by anyone who can send UDP to that port.
  Small and memory-safe — quinn and rustls — and still the real attack surface.
- **Connection-attempt flooding.** Every attempt costs a signature verification. QUIC address
  validation (Retry) and a per-source rate limit are the hardening, and **neither is configured
  today**.
- **The node's public key is disclosed** to anything that completes a handshake far enough to read
  our certificate, which also confirms that an offload node lives at that address. Not a secret,
  and worth stating because **WireGuard's silence to unauthenticated packets is a property this
  design does not have**. That is the honest argument for running a tunnel underneath even though
  ADR-0015 refuses to *depend* on one.
- **And the sharpest one, which is not about the network at all.** A member holding `Submit` can
  make the laptop run an agent, and an operator's `--allow` is deliberately *not* capped the way a
  repository's `.offload.toml` is — so a compromised phone can submit a run asking for
  `Bash(sh:*)` on the machine that holds every credential. That is the product working as designed.
  It means **the phone's lock screen is now part of the fleet's security boundary**, and it is a
  bigger change to the threat model than opening the port is.

Two questions this opens, named rather than answered, because each is a change to ADR-0012's grant
set rather than to the transport:

- **Should a submission arriving from a peer be capped like a repository's rather than trusted like
  a keyboard's?** `require_scoped` already exists and already refuses a shell; the question is
  whether a remote `Submit` is the same kind of authority as somebody at the machine.
- **Should `Submit` be splittable** — "may fire rules that already exist" versus "may submit any
  prompt with any grants"? The first is what a phone actually needs for *sweep the shops* and
  *check the logs*, and it is a much smaller thing to lose with a stolen phone.

## Consequences

- The owner's arrangement works with **two small changes and no new dependency**: a forwarded UDP
  port, a DDNS name in `seeds`, and the re-dial tick. A tunnel remains the shortcut for this week
  and stops being necessary after that.
- Everything the laptop needs to say goes out through a sink, so nothing depends on the phone being
  reachable, awake, or on the same network.
- What stays impossible on purpose: dialling the phone, joining a fleet from outside the LAN, and
  CGNAT at home — the last of which is §4, whose shape and trust model are now written down.
- **A relay is not a member, and that is the whole point of it.** Anything that would make the
  relay hold a certificate should be read as a design error: it would then read every prompt the
  fleet gossips, for the convenience of forwarding packets it cannot decrypt anyway.
- If §4 or §5 ever lands, nothing above `offload-transport` changes. That is the whole return on
  ADR-0015 §1.
- The two questions in §6 are the first phase-5 security work, and neither is about NAT. A phone
  that can spend an agent's grants on a machine full of credentials is the interesting boundary;
  the port is the boring one.

## What this deliberately leaves

**No measurement yet.** Steps 0 and 2 are a prediction until the phone is on mobile data with a
daemon in Termux. Both gaps in the context section were found by *reading*, which this project's
own history says is the weaker instrument — the seed parse is unambiguous, but "the phone
reconnects after a new lease" is a claim to walk rather than to believe.

**Android's power management is not costed.** SWIM probes once a second and a lease is renewed six
times a period; both are laptop numbers. An `Ephemeral` node wants a longer probe interval and
event-driven wakeups, and its *absence* is normal rather than a fault — which ADR-0007's states and
ADR-0031's absence history already tolerate, but nobody has run it on a phone.

## Amendment, 2026-08-27: §2 is built, and the re-dial exposed the bug under it

Both items measured before and after on two daemons. The old binary: a name refused outright, one
dial attempt, given up after **30 seconds**, and forty seconds after the peer came up the two
daemons still knew nothing about each other. After: passes at 40s, 50s and 70s intervals as the
backoff compounds on the dial timeout, and contact restored with no restart.

Two things the walk added that reading had not. A one-shot dial's grace period was QUIC's handshake
timeout, which is why the first attempt at this walk accidentally *succeeded* — the peer came up
four seconds in, inside the 30. And awaiting the dial in `main` delayed the whole delivery plane by
seven seconds, so it is spawned now.

And the finding the fix exposed, which matters more than either item: **the re-dial worked and the
fleet still did not recover.** `probeable` excludes `Draining | Departed` while a `Dead` node is
re-probed for ever, so an announced departure was never revised — and `introduce` could not revise
it either, building a placeholder at incarnation 0 that `merge_node` ignores. A peer re-met 40 times
over four minutes stayed `draining`, and the node it handshaked with never learned it existed.
First-hand contact goes through `alive` now. Newly urgent because ADR-0034 and ADR-0035 had just
made `SIGTERM` announce departure properly: every polite restart was poisoning every peer's view.

## Amendment, 2026-09-25: §0 measured, the home half — and the default could not use it

**The home connection has routable IPv6.** This laptop holds a global `2001:db8:…/64` on both
wifi and ethernet, with a default route from the router's RA, and an outbound request reports the
same address back: no NAT66. So a v6 phone needs no traversal to reach it. That covers the home
half of §0; **the phone's half is still unmeasured**, and it is one `ip -6 addr` in Termux on
mobile data.

**The measurement found the fleet could not have used it.** `[cluster] listen` defaulted to
`0.0.0.0:7433`, and a v4-only socket can neither be dialled on IPv6 nor dial it: quinn refuses the
destination (`invalid remote address`), so an AAAA seed failed on the dialling side too. The
default is now `[::]:7433`, bound with `IPV6_V6ONLY` off explicitly, because that option's default
varies by platform and inheriting it would trade v4 for v6 where it is on. A host that cannot do
dual-stack binds `0.0.0.0` on the same port and warns. Walked on two daemons over `[::1]`, the
global address and `127.0.0.1`, and with mDNS on; the control, one side back on `0.0.0.0`, logs
the refusal, now reworded to name the socket rather than the address. An owner who wrote
`0.0.0.0` explicitly keeps it.

What this does not change: inbound reachability through the router's IPv6 firewall is still §2's
"forward one port", except that it becomes one firewall rule and no NAT mapping. That was not measured here
(it needs a device outside the LAN).

**Same day, later: the Mac meshed, over both families.** Session sixty-six left Linux↔macOS
blocked on Local Network access for an ad-hoc-signed binary. A freshly compiled ad-hoc sender on
the Mac now gets 20 of 20 datagrams through to the laptop on v4 *and* v6. A walk fleet then met
over the laptop's global v6 address from the Mac and over v4 from the laptop, and ran agent runs on
archive workspaces in both directions and a task across the two. What still stands between them is the LAN: the Mac's
`en0` drops (`status: inactive`, and Apple's own `ping` says `Network is down`), and the laptop is
dual-homed on one subnet, so ssh needs `-b` pinned to one of its addresses. The laptop saw the Mac
drop 24 times in about seventy minutes, with dead spells of up to ten minutes. It healed every time,
and the daemon's `sends … refused by this machine's kernel` line named the cause throughout.

**Linux↔Android, on an emulator.** The x86_64 Android build (`ANDROID_ARCH=x86_64
scripts/build-android.sh`) ran in the SDK emulator. The probe said `Phone / Android / X86_64
(Ephemeral)`, the node joined by invitation, and it met the laptop across the emulator's NAT
(dialling `10.0.2.2`, with `adb emu redir add udp:7602:7602` for the way back). It stayed `alive`
from both ends and ran tasks placed from the laptop. This is not the phone. Mobile data, Termux, a
real battery and Doze are all still unmeasured, and so is this ADR's §0 phone half.

**And a real phone, under Termux.** A Samsung phone (Android 16, aarch64) ran the aarch64 build
and meshed with the laptop, **mDNS included, both ways**. The laptop found the phone at its global
IPv6 address, the only one the phone advertised, and could dial it only because `listen` is now
dual-stack. On home wifi the phone goes out on a global v6 address. The mobile-data half of §0 is
still unmeasured: the first attempt met a half-upgraded Termux whose `curl` would not link.

## Amendment, 2026-09-26: §0 measured, the phone half — and the router is what is left

**The phone has a public IPv6 address on mobile data.** The phone on its carrier, wifi off, went out
as `2a00:801:…` (read off the phone's screen, and the carrier's prefix confirms it). With the home
half above, **both ends are globally addressed IPv6 and no NAT is anywhere**, so the plan's
fallbacks (§3's relay and §5's punching) are not needed for this fleet.

**It still did not connect, and the block is the home router.** Walked with the Android app
(ADR-0066) seeded at the laptop's global address. The laptop marked the phone `dead` five seconds
after wifi went off, and in two minutes of the phone re-dialling it logged no inbound handshake at
all, not even a refused one. The laptop's firewall allows UDP 1025–65535 on both families, so the
packets died upstream, at the ASUS router's IPv6 firewall, which drops unsolicited inbound by
default. That is §2's "forward one port", which on IPv6 becomes **one inbound firewall rule and
no NAT mapping**. It has not been added. It is the owner's network and the owner's call, and
§6's argument (an exposed port admits nobody without a fleet-signed certificate) is what they
weigh it against.

**Back on wifi the phone rejoined by itself**, `alive` again within about a minute with no restart.
That is the re-dial from this ADR's first amendment, on a real phone, across a real network change.

**Then the rule, and it worked.** The owner added one inbound IPv6 rule on the RT-AX56U (Advanced
Settings → Firewall → IPv6 Firewall: UDP 7601 to the laptop's address, from the carrier's
`2a00:801::/32`) and left the firewall on. Wifi off again. The phone stopped answering ping on the
home network, the laptop marked it `dead` for 35 s while it changed networks, and at 22:54:24 UTC
it was back: `a peer thought we were gone; refuting`, then `alive`, steady, **with the phone
reachable only over mobile data**. So §2 is walked end to end. There is one reachable end, the
phone dials out, IPv6 at both ends, one firewall rule, and no relay and no NAT anywhere. §4's relay
and §5's punching stay deferred, and for a fleet like this one they are not needed.

## Amendment, 2026-09-26: "the phone dials out" needs the connection to work both ways

§2 has the phone always dial out, and it walked, with one home node. Overnight, with a **second**
home node (bravo) and the phone on mobile data, the fleet declared the phone dead **1 772 times**.
The phone refuted each one (incarnation 3 790 by morning), and every flap orphaned whatever it held.
The first reading, Doze, was wrong. The mechanism came out of the logs:

- Nobody can dial a phone behind a carrier firewall, so bravo's direct probes failed, as they must.
- Bravo's **indirect** probe asked the laptop, and the laptop was talking to the phone that second.
  But `Cluster::session` only reused sessions this node had *dialled*, so the laptop dialled the
  phone, failed, and answered `Nack`. Bravo concluded `dead`, the laptop adopted the claim, and the
  laptop only ever knew the phone was alive because the phone kept probing *it*.

So a node that can only dial out is reachable over its own connection or not at all. The fix makes
that connection carry streams both ways:

- `session(peer)` falls back to the peer's newest **inbound** session that is not closed, before
  dialling (the new `Connection::is_closed`).
- A node **serves** streams on the sessions it dialled, as it always did on the ones it accepted.
  They are handed from `session` to `serve` over a channel, and only inbound sessions are registered
  for hang-up, since a dialled one already lives in `connections`.
- A bid refused on its certificate now hangs up **both** directions (probation excepted, since that
  lifts by time), because the reused session may be one the peer opened, and dropping only what
  this node dialled left the old certificate answering every round. That regression was caught by
  a new test before any walk.

`MemoryNetwork::firewall(node)` models the phone: nobody can dial it, and it can dial out.
`a_node_nobody_can_dial_is_kept_alive_over_the_connection_it_opened` reproduced the night (`Dead`)
and passes now. **Walked:** the phone app on mobile data, the laptop, and bravo on the new build.
For five minutes bravo saw the phone `alive` throughout, with 0 deaths where the night had had about
ten a minute.

**Re-measured on the current build (wire v33), the phone locked and unplugged on the home wifi, 14:43
to 15:13.** The laptop declared it dead **0** times in 30 minutes. On the build before the
bidirectional-session fix, the same phone dozing was declared dead about 30 times an hour. It was
`suspect` in 18 of 179 ten-second samples, and its absence count rose by 463: about fifteen
suspicions an hour, each refuted within seconds. That is doze showing up as latency, which is what
the detector should see. The one death in the first attempt was a reinstall of the app mid-walk,
not doze.
