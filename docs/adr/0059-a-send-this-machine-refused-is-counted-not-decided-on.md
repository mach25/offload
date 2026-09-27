# ADR-0059: A send this machine refused is counted, not decided on

**Status:** accepted · 2026-09-10 · built and measured in the session that decided it · closes
`docs/ROADMAP.md` phase 6's second item · does not amend ADR-0015

## Context

`quinn_udp::UdpSocketState::send` returns `Ok(())` for every send error but `WouldBlock`. Its own
doc comment gives the reason, and the reason is sound:

> UDP transmission errors are considered non-fatal because higher-level protocols must employ
> retransmits and timeouts anyway in order to deal with UDP's unreliable nature.

Nothing here disputes that. A transport that tore a connection down because one `sendmsg`
returned `EPERM` would be worse than the silence — a laptop switching networks produces
`EHOSTUNREACH` for a second and then works.

What is wrong is not the swallowing. It is that **the silence is attributed to the peer**. A node
whose kernel is refusing every datagram believes it sent them all, so the failure detector reports

```
no answer within 500ms
```

— *the peer did not reply* — for what is really *this machine would not let me speak*. Every layer
above the socket sees the same thing, because there is nothing else to see. `offload status` says
`accepting yes`, `offload nodes` says the fleet is down, and the one machine that knows says
nothing.

That is not hypothetical, and it is not cheap. Session sixty-five found that a Linux daemon and a
macOS 26 daemon would not mesh; session sixty-six resolved it as macOS refusing an ad-hoc-signed
binary Local Network access, three sessions after the symptom appeared. The only evidence that
ever named the real cause was one `EHOSTUNREACH` seen by hand under `strace`, and
`examples/udp_probe.rs` exists because there was no other way to ask the question. Every report the
product had pointed at the Mac's peer.

`UdpSocketState::try_send` is the call that hands the error back. The question this settles is what
to do with it.

## Decision

### 1. The transport owns its socket, so the error exists to be looked at

`QuicTransport::bind` no longer calls `quinn::Endpoint::server`, which builds quinn's own
`AsyncUdpSocket` — the one that calls `send`. It binds the socket, wraps it in
`offload_transport::sends::WatchedSocket`, and hands that to
`Endpoint::new_with_abstract_socket`.

`WatchedSocket` is a near-copy of quinn's private tokio socket. Everything delegates to the same
`UdpSocketState`, so GSO, GRO, ECN and MTU behaviour are quinn's and not ours. The single
difference is `try_send`.

### 2. Behaviour is unchanged by construction

`WatchedSocket::try_send` classifies what `UdpSocketState::try_send` returned and then answers
exactly what `send` would have answered:

| The kernel said | Answer to quinn | Counted |
| --- | --- | --- |
| `WouldBlock` | handed back, so quinn waits for writability | no |
| `EMSGSIZE` | `Ok(())` | no — this is quinn discovering the path MTU |
| anything else | `Ok(())` | **yes** |

The `EMSGSIZE` row is quinn-udp's own carve-out, kept for its own reason: an MTU probe that
exceeded the interface MTU is the transport working, not a muzzle. Counting it would make the
number noise on a VPN.

Nothing in this ADR can change whether a datagram is sent, whether a connection survives, or how
long anything waits. That is the point: the argument against building this was that a decision here
would be wrong, and it is not a decision.

### 3. It is a count and a breakdown, per destination, since this daemon started

`SendRefusals` keeps a total and a `HashMap<SocketAddr, (count, last)>` capped at sixteen
destinations. Both numbers are reported, because past the cap they disagree and the total is the
honest one.

The destination is an **address**, not a `NodeId`. A dial that fails this way may be a seed nobody
has identified yet — the bootstrap dial does not know whose address it is (ADR-0015) — and the
address is what the next command an operator types takes.

The kernel's own words are stored and printed **unreworded**. `Operation not permitted` on macOS
is Local Network access; `No route to host` on a multi-homed laptop is the wrong interface. The
errno is the thing somebody searches for, and a sentence of ours in its place would be a second
copy of a diagnosis we do not have.

### 4. One line in `offload status`, and only when there is a number

```
sends       7 refused by this machine's kernel — that is not the peer's silence.
            └─ 192.0.2.240:9000  ×7  ·  Operation not permitted (os error 1)
```

Printed only when the total is above zero, for the reason `asks` is: a line about refused sends on
a healthy node is noise on the command run most. A node with no mesh reports an empty list, which
is a fact there rather than a shrug — it has sent nothing.

It sits immediately above the `fleet` line on purpose. That line says how many members this node
has *met*, and a muzzled node's answer to it is the thing this one explains.

### 5. Refused: making it a decision, and making it a metric

Two shapes were considered and neither is built.

**Tearing the connection down, or marking the peer unreachable from this side.** This is what the
roadmap warns against and it is right to. A send error is transient far more often than it is
permanent, and a transport that acted on one would turn a two-second network change into a
membership event. Worse, it would be a *second* liveness mechanism beside the failure detector,
with its own opinion — and the standing rule here is that a decision computed from a second copy of
the rule is confidently wrong and silent.

**A Prometheus counter.** The phase-6 item above this one is still deliberately open and its
argument stands: nobody scrapes their phone. The audience for this fact is the person typing
`offload status` on the machine that is not working, and they get a sentence.

## Consequences

- A muzzled node can be identified from the muzzled node, in one command, which was previously
  possible only under `strace`.
- The count is per daemon start and is not persisted, gossiped or merged. It is an observation
  about *this process on this machine* — the one class of fact a peer must never be believed
  about — so there is no owner to write down and nothing to arbitrate.
- `offload-transport` gains `libc` on unix, for one constant. quinn-udp classifies `EMSGSIZE` the
  same way for the same reason.
- The socket swap is the largest risk in the change. It is covered by the tests that already
  drive real sockets — `two_nodes_connect_over_real_sockets_and_exchange_a_message`,
  `a_stalled_dialer_does_not_block_the_next_peer_from_getting_in`, and the 64 KiB reply that
  outlives its stream — and by two daemons on loopback: mesh, gossip, a graceful departure, a
  `kill -9` detected, a bid round **placing a run across the wire**, eight checkpoint blobs
  replicated to the node that ran nothing, and `offload logs` forwarded back in full.
- The muzzle is reproducible in a unit test, with no firewall, no root and no second machine: a UDP
  socket with `SO_BROADCAST` unset — which quinn never sets — is refused `EACCES` for every
  datagram addressed to `255.255.255.255`. Nothing leaves the host. Measured both ways on the same
  test: with `try_send` the count is the number of Initial packets quinn sent; with `send` restored
  in its place the count is **zero** and the dial fails with the peer looking silent, which is the
  behaviour this ADR is about.

## Residuals

- **A node that cannot send still does not tell anybody else**, and should not: the fact is about
  this machine, and a machine that cannot send cannot report it either. What it can do is be asked,
  which is now possible.
- **Nothing watches the number.** There is no threshold, no log line at the hundredth refusal and
  no notification. A count with no reader is where this started, so it is worth saying why: the
  situation this describes is one where the operator is *already* looking, because the fleet
  appears down. If somebody meets it without looking, the lever is `notify`, and it needs an ADR
  of its own — a node that cannot send also cannot deliver a notification off itself.
- **Receive has the same shape and is not covered.** A kernel dropping inbound datagrams produces
  the identical symptom from the other end, and there is no error to count: nothing is returned for
  a packet that never arrived. That asymmetry is real and this ADR does not close it.
