# ADR-0076: A node gossips where it can be dialled

**Status:** accepted · 2026-09-27 · session ninety-two · wire v35

## Context

A node learned a peer's address three ways: a seed, an mDNS announcement, or a connection the
peer opened. None of them travel. Overnight, the Mac mini joined the phone's fleet and could reach
the laptop by its seed, but the laptop never received the Mac's mDNS announcements. It knew the
Mac only from gossip, with no address, and could talk to it only over a connection the Mac had
opened. Every laptop restart threw that away, and the pair stayed apart until the Mac dialled again.
The emulator, behind its NAT, could never reach the Mac at all. It kept declaring it dead, and the
Mac kept refuting.

Two more defects were found on the way, and both are fixed with this: discovery skipped an mDNS
announcement from any node already *known*, even one known only as dead from a peer's gossip; and
the failure detector's own regressions from the night before (a dead node probed rarely, and
redialled only after three timeouts; the all-dead rotation stuck on one node).

## Decision

1. **`NodeView::addresses`**: where a node can be dialled, as `ip:port`, by the rule mDNS
   advertises by. A specific bind is exactly the bound address. A wildcard bind is every interface's
   address with the bound port, leaving out loopback and IPv6 link-local
   (`discovery::dialable_addresses`).
2. **Owner: the node itself.** It rides in the node's own record and is replaced only by a newer
   incarnation, like its capabilities. A changed list bumps the incarnation
   (`Cluster::set_addresses`, re-stated every 30 s). A peer holding a previous life's addresses is
   refuted like one holding its old capabilities (`stale_facts`).
3. **Use**: every 10 s, a node dials peers it sees as `Dead` or `Suspect` at the addresses they
   state, once a minute at most per peer (`Mesh::gossip_addresses`), whether or not mDNS is on. A
   successful handshake with the right key introduces the peer, exactly as an mDNS find does. An
   address now answered by a different key is ignored.
4. **Wire v35.** A defaulted field would be ignored by older nodes, but a v34 node relaying a record
   drops it, and a record at the same incarnation never replaces the one held. An address erased in
   transit would stay erased, which is the case `gossip-and-merge` says earns a bump.

## What it does not do

It does not cross NAT. A phone on mobile data states addresses nobody can dial, and that costs a
failed dial a minute per peer that sees it dead. A malicious member could state false addresses for
another node, and the handshake refuses the impersonation. The cost is dials, which is acceptable for
a fleet that is one owner's devices.
