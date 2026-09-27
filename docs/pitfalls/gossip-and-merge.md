# Gossip, merge rules and who owns a fact

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/gossip-and-merge.md`, same order.

- **A node must not believe a peer about its own absence.** Accepting `Orphaned` about itself
  leaves the holder with no lease, so it cannot renew and its `complete` is fenced out. Keeping
  `Running` and gossiping it *is* the reclaim path. Narrow: a node still believes its arbiter about
  a run it is not holding.
- **An observation cannot be relayed, so a decision and an observation settle differently.** Only
  the arbiter may declare a run orphaned. A half-delivered orphan stays half-delivered, and that is
  benign — a fleet at rest agrees on holder and epoch, and may differ on whether that holder answers.
- **A holder's own copy beats a peer's — about the fields it owns.** The state, epoch, lease and
  checkpoint are the holder's; the deadline and priority belong to the home node and arrive by
  gossip. `merge_run` for the record, `merge_spec_edit` for the edit, `settle_spec` for the rule.
  Hand the store the record as **settled**, and apply an edit to the row rather than carrying it in
  on a copy of the row.
- **A spec has exactly two editable fields sharing one counter** (deadline, priority), owned by the
  home node, arbitrated by `spec_rev` *before* the record settles. A third needs an ADR.
- **Unreachable is not dead, and dead is not a decision.** `Suspect` then `Dead`; neither moves a
  run on its own — the hold-down policy does.
- **A restarted node begins at incarnation 0, so its own facts lose to a peer's memory of its last
  life.** `merge_node` copies only on *strictly greater*. `refute_above` takes a floor, because the
  point is to outrank rather than to increment; the rule lives in `merge_node` alone.
- **A polite departure must be able to heal.** `probeable` excludes `Draining | Departed` while a
  `Dead` node is re-probed for ever, so the good behaviour was the unrecoverable one. `introduce`
  records contact through `alive` — an authenticated handshake is an *observation*, so it outranks
  a remembered declaration.
- **A seed is re-resolved and re-dialled, not parsed once.** `lookup_host` on every attempt (the
  name is the stable thing), and re-dial while there is no live peer. Don't await the dial in
  `main` — it delayed the delivery plane by seven seconds.
- **Advertise what you bound, not what the machine has.** A wildcard bind is the only case where
  "every interface" is true; a loopback bind is not advertisable and must say so.
- **`0.0.0.0` is one family, and so was the default.** A v4-only socket cannot be dialled on IPv6
  *and cannot dial it* — quinn refuses an AAAA seed as `invalid remote address` before a datagram
  leaves. `[::]` is both only with `IPV6_V6ONLY` off, which is a platform default, not a guarantee:
  set it, and fall back to v4 loudly. Checked by `a_wildcard_v6_bind_speaks_both_families`.
- **…and a dual-stack socket spells every v4 peer as v6.** quinn hands a `[::]` socket's IPv4
  destinations over v4-mapped, so a report printed `[::ffff:192.0.2.5]:7601`. Any address that
  reaches an operator from the socket goes through `to_canonical()` first.
- **`active_runs` means held, not merely known.** The store keeps runs this node placed elsewhere,
  so an unfiltered list gossips every run the node has heard of as one it is running.
- **Position and spend merge by different rules.** Position follows the leg the record settled on
  (`RunProgress::absorb`, the same epoch-then-lowest-id arithmetic as `merge_run`, and it must stay
  identical); spend is cumulative from every leg, because a leg that lost still spent it.
- **A merge tiebreak has to be total, not merely forward.** Two legs writing different summaries in
  one millisecond tie on every compared field, and the fleet then disagrees permanently. Wherever
  the tiebreaks run out has to be a value every node orders the same way.
- **Turn numbers are made cumulative *on the way in*, so don't reason about the per-leg counter.**
  `offload-agent`'s parser counts a fresh process from one, and `accumulate_turns(event, offset)`
  in the pump shifts every turn number by the checkpoint's before anything reads it — the event
  log, the checkpoint, the cadence, `stats.turns = turn`, and the limit. Read only the writer
  (`stats.turns = turn`, an assignment) and the merge (`absorb` takes the winning leg's `turns`,
  deliberately not a `max`), and you will conclude that `--max-turns` resets on migration. It does
  not: walked with a run capped at 3 that migrated after turn 2, node-b's *first* boundary was
  recorded as `turn=3` and stopped the run 22ms after spawning. **Do not "fix" this by adding a
  `max` to `absorb`** — that would break the leg-ownership rule ADR-0005 sets and this file
  carries two entries above.
- **Not stamping a row is not the same as being allowed to write it.** `Supervisor::writing_leg`
  answers `None` for two opposite situations — a run that ended *here* whose `live` entry a restart
  took (the stamp on the row is ours) and a run that ended *elsewhere* (the stamp is the peer's,
  absorbed by gossip). Writing under the second one does not merely record a local fact: `at` is
  part of `RunProgress::position`, so the touch alone wins the tiebreak and carries our sentence
  back to the peer **in their name**. Measured: `offload rm` on a node holding a leftover checkout
  told the peer its own checkout was gone while it sat there untouched — session seventeen's bug,
  reached through the half of the door its fix does not cover, because `remove` really does return
  `Removed` there. `update_stats` now asks whose the stamp is; an unstamped row is nobody's and
  stays writable.
- **A new gossiped fact does not always need a new owner — put it where one already is.** ADR-0005
  says every gossiped field needs an owner and an arbiter, which reads as "a field on `Run` plus a
  merge rule", and two handoffs declined to build one for that reason. *Which node let this run
  go* went inside `RunState::Pending` instead (ADR-0042): the state is the holder's, arbitrated by
  `merge_run` already, so there is no second rule to keep in step — and because every transition
  replaces the whole state, the fact cannot outlive what it is about. **Ask what the fact is a
  property of.** This one is a property of *being pending*, so a field on the record would have
  needed clearing in five places and would have been wrong in the sixth.
- **…and a field a peer can erase by relaying earns a version bump, even when it is
  `#[serde(default)]`.** `Run::origin` did not, correctly: immutable, identical in every copy,
  nothing to contradict. `let_go_by` is mutable inside a state peers also gossip, so an older
  build that drops it and wins an equal-epoch tiebreak re-states the run without it — silently
  restoring the exact bug the field exists to end. The test is not "would an old node ignore it"
  but **"could an old node relay something that erases it, and is the default the safe answer"**.
  Here the safe-sounding default is the broken case, so: wire v24.
- **Nothing in this project had ever run on two operating systems, and it does not.** Linux↔Linux
  meshes; Linux↔macOS completes the QUIC handshake and then dies when quinn sends its first
  1452-byte datagram, which macOS's `sendmsg` answers with `EHOSTUNREACH` rather than dropping.
  Every multi-node test here has been daemons on one Linux box with distinct ports, so the
  platform axis was untested by construction. Measured with the network proven innocent —
  bidirectional fixed-port UDP crosses, 1400-byte DF pings cross, and a second Linux daemon on the
  same address meshes in seconds. **A test matrix with one row does not know which of its columns
  is load-bearing.** Half-answered: `mtu_discovery_config(None)` takes the `sendmsg` errors to zero
  and lets connections establish both ways, and the mesh still does not come up — a ~2370-byte
  request goes unanswered at 5s on a 0.8ms path, while the serving node's own trace says nothing
  about the connection at all. **An accept path with no log line cannot be told from one that is
  never entered** — so it has one now, and the answer was that the loop runs and both nodes
  **admit each other**. Requests arrive and decode on both sides; replies arrive on neither; and
  it is **intermittent**, which is the part no tidy theory survives. Leading suspect: quinn resets
  an unfinished `SendStream` on drop and `send` is only `write_all` + `flush`, so a reply `send`
  accepted may never reach the wire — `serve_stream`'s `finish()` error was discarded and is now
  logged. That theory died to a local test, as did GSO, ECN, datagram size and bursts.

  **Where it ended up: two faults, one ours.** Ours is that `QuicTransport::accept` ran every
  step of an inbound handshake **inline in its own loop**, including an unbounded `accept_bi()`.
  quinn does not drive a handshake until the application takes the `Incoming`, so one dialer that
  completed TLS and then went quiet parked the loop and every later dialer got *silence* — an
  Initial, PTO retransmits, nothing back — with the kernel counting no drops and this node
  logging nothing inbound. Self-sustaining, because redialling a peer you cannot reach supplies
  the stalled connections that keep you from reaching it. **A per-connection step in an accept
  loop is head-of-line blocking for the whole node**, and it needs no second machine to
  reproduce: `a_stalled_dialer_does_not_block_the_next_peer_from_getting_in`, five seconds. Fixed
  by spawning each handshake and posting its outcome to `accept` over a channel. Related and
  fixed with it: `authenticated_peer` reported `TransportError::Closed`, which `Cluster::serve`
  reads as *the endpoint is gone* and stops listening for good — so a per-connection failure is
  now never `Closed`.

  **The other fault is the Mac's, and every "raw UDP works" control that hid it was broken.**
  macOS 26 refuses to let an **ad-hoc-signed** binary send to the LAN and reports it as
  `EHOSTUNREACH`; Apple-signed `python3` sends fine from the same machine at the same instant.
  Measured interleaved in one loop: python 30 of 30, an ad-hoc C sender 0 of 30, ad-hoc Rust
  0 of 30. Socket options, syscall, ECN, socket lifetime and the message header are all
  eliminated — see `docs/DEMO.md` for the table. **The reason it took three sessions is an
  instrument that could not fail**: `udp_probe` used `UdpSocketState::send`, which returns
  `Ok(())` for every error but `WouldBlock`, so its "10 of 10" counted calls rather than
  datagrams — and the contradiction this entry was built around ("quinn fails where raw UDP
  never does") never existed. The remedy is a click at the Mac's console, not code.

  **What is still ours to decide.** `quinn-udp` swallowing the send error means Offload says
  `no answer within 500ms` — *the peer did not reply* — for what is really *this machine would
  not let me speak*. Nothing above the syscall can tell those apart today; `try_send` is the call
  that could, and that is a design question rather than a fix.

- **A signed fact and an unsigned one about the same field is two owners, and the merge let the
  unsigned one win.** A device has two names: its config's, which defaults to the machine's
  **hostname**, and its certificate's, which is what the fleet gave it. `main::report_membership`
  states the rule at startup and warns when they differ — *"peers show the certificate's name,
  because that one is signed"* — `Cluster::record_name` writes it at the handshake, and ADR-0012
  puts a name on the certificate *"for `offload nodes`"*. Then `merge_node` copies a peer's
  `NodeView` wholesale above its own incarnation, name included, and an incarnation rises whenever
  capabilities or policy change: every probe on a machine whose cpu load moves. Measured on two
  daemons neither of which named itself in `node.toml`, so both were `fedora` — `offload nodes`
  showed `bravo` for **25 seconds** and the hostname for ever after, and `offload explain` then
  named the holder, the arbiter and both canvass rows `fedora`. Corrected **on the way in**
  (`absorb` rewrites an incoming name from the certificate this node holds) rather than repaired
  after the merge: `merge_node` is `offload-core` and has no certificates to consult, and no reader
  ever sees the wrong name for an instant. A peer learned by *relay* keeps its gossiped name,
  because that is the only one there is.
- **…and the node's own entry is the same fact from the inside.** `Cluster::name_of(self.node())`
  is the `by` on a forwarded cancel or answer and every peer-facing sentence that names this
  machine, and it read the config name — so a run's log said `cancelled from fedora` on the
  machine that ran it. The local `NodeView` is named from `fleet::display_name` now, which is the
  one answer to *what is this device called to the fleet*; `offload status` still shows the
  configured name, which is the local half of the split the startup warning explains.
  **The tell: a field written from two places, one of which can prove what it says.**
- **Two legs from nothing are not a chain, and a chain's rules keep one of them.** Tokens follow
  the position and cost merges by `max`; both assume each leg resumed from the last, and a partition
  that runs a run twice from zero breaks that. The fix is not to touch either rule. It is per-leg
  entries (`RunProgress::legs`, ADR-0067), each with the base it started from, unioned and summed.
- **Only the place that knows a value may create the record that holds it.** ADR-0067's leg entry was
  created lazily by whichever stats write came first, taking the row's tokens as its base, and an
  ordinary drain then reported a lost leg. Creation moved to the leg's start, the one place the base
  is known; everyone else updates or does nothing, so the failure is an under-count, never an alarm.
- **A node that can only dial out is reachable over its own connection or not at all.** A phone
  behind a carrier firewall was declared dead 1 772 times in a night by a second home node, because
  the node it *had* dialled answered the indirect probe by dialling it back. A connection a peer
  opened is used for this node's own asks too (`Cluster::session`), and a node serves streams on
  what it dialled. Tested with `MemoryNetwork::firewall`.
- **…and every question about "the connection to that peer" has to use the same lookup.** The
  inbound fall-back went into `session()` alone. `peer_certificate`, which the bid round uses to
  check a bidder's host-runs, still read only the dialled sessions. A bid that arrived over the
  bidder's own connection had "no certificate", so it was refused and hung up on. On a LAN the next
  round dials out and it passes for a flake. A phone behind a carrier firewall could never host.
  Both go through `Cluster::existing_session` now. Found in the iOS Simulator walk just after a peer
  restarted. Tested by `a_host_nobody_can_dial_wins_a_round_over_its_own_connection`.
- **A peer's handshake certificate goes stale on a long connection, and anything reporting from it
  goes stale with it.** QUIC sessions last longer than a certificate does. `Cluster::certificates()`
  held each peer's handshake copy, so an approver's re-approval list left out a member whose
  certificate had been renewed twice over one connection. `expiring` used the same copies. A
  certificate carried in `RenewMe` is now kept once `Members::admits` passes it, as is the one this
  node issues, each only if newer (`record_newer_certificate`). A third node still sees the
  handshake copy until it reconnects.
- **A "replace my own contribution" rule never drops a record nobody holds.** `publish_runs` removed
  runs held by this node and reinserted the store's list, and a finished run has no holder. So the
  view kept every finished run it was ever sent, and every probe carried the fleet's whole history,
  while `gossipable_runs` and two doc comments said only live and recent runs travel. On the phone's
  fleet that was ~190 records and **25% of a core idle** (6% after). `ClusterView::forget_finished_before`,
  called each tick with `GOSSIP_TAIL`, is the ageing the docs assumed. Measure a payload's size
  before trusting a sentence about what is in it.
- **One probe timeout for the fleet is wrong for a phone.** 500 ms suited the LAN (16 ms median) and
  cut off the phone on LTE, whose radio sleeps between packets: p90 563 ms, p99 705, max 1 060 once
  measured past the cut. At 500 ms, 14% of its probes timed out and it flapped twice a minute. The
  timeout is now learned per peer (RFC 6298's estimator, floor at the configured value, cap
  `MAX_PROBE_TIMEOUT` under `suspect_timeout`, doubling after a miss). Result: 0 flaps in 10
  minutes. A comment had recorded "562 ms against a 500 ms timeout" and quietened the log instead.
- **…and a slow answer is not a broken connection.** Every failed probe hung up, and the phone's
  dialled connection was the laptop's only way back to it. Only `TIMEOUTS_BEFORE_REDIAL` timeouts
  in a row hang up now, and a transport error at once. The emulator behind NAT went from 17 flaps
  in 10 minutes to 3 on this alone.
- **A node concluded dead is visited one round in `DEAD_EVERY`, and never through helpers.**
  Helpers exist to stop a false death; for a node already dead they had every helper, a phone
  among them, dial its stale address each rotation. Two nodes that marked each other dead heal when
  either visits, so the storm's `settle` quiet window includes `DEAD_EVERY` rounds. A shorter window
  called the fleet at rest while both still said `Dead`.
- **Patience for a slow peer must not become patience for a dead one.** Re-dialling only after three
  timeouts, and probing a dead node one round in 30, together kept a restarted laptop and the Mac
  apart for three minutes: the old connection answers nothing after a peer restarts. A node already
  concluded dead is re-dialled at its first silence. And with nobody alive, the dead rotate round by
  round; counted in dead rounds, one node with no address was probed thirty times running.
- **"Known" is not "reachable".** Discovery skipped an mDNS announcement from any node already in the
  view, including one known only from a peer's gossip, as dead, with no address here. Its every
  announcement was thrown away. It skips only a node that is alive now.
- **An address that only arrives by mDNS or by a peer's own dial is one a restart loses.** Neither
  travelled, so a node that missed another's mDNS could talk to it only over a connection that node
  opened. `NodeView::addresses` gossips them (ADR-0076, owned by the node, v35 because a relaying
  older node would erase them). Leave out loopback, IPv6 link-local and container or VM bridges.
- **Read `offload status`'s kernel refusals before blaming the network or the code, and then ask
  whether the machine was awake.** A night of theories about the laptop's two addresses, ARP and
  macOS privacy ended at "sends 38 refused by this machine's kernel … Network is down (os error 50)"
  on the Mac, and that ended at `pmset -g`: `sleep 1`. The Mac mini slept after a minute idle and woke
  for packets, so every probe found it asleep or waking. Raw UDP tests passed because the ssh that
  ran them had just woken it. `pmset -g log` shows Sleep and DarkWake.
- **A fleet-wide request is passed on before the node asks whether it has anything to do.**
  ADR-0080's Refresh raised the gossiped count after the reader looked for an agent, so the tablet
  (no agent) returned early and its request went nowhere. The device a person presses Refresh on is
  usually the one with nothing to read. Found on the walk: the laptop logged no read.
- **…and "the first count a node hears is old news" means the first *exchange*, not the first
  value.** A restarted node learned the count 0 silently (a `watch` does not fire for its initial
  value), so the next real request, 1, was "the first value it learned" and skipped. The cluster
  adopts a count quietly only in its first absorbed gossip (`heard_any`), and a mesh test checks both
  halves: `a_request_to_read_models_again_travels_by_gossip_and_only_rises`.
- **"When did I last hear from it" is an observation, so gossip must not move it.** `merge_node`
  set `last_heard = now` on any newer incarnation, relayed or not, and inserted a relayed node with
  the relayer's time. Incarnations rise with every probed change, so `offload nodes` showed the Mac
  `dead` (this node's detector) and seen `3s` ago (the tablet's gossip) in one row, and stopped
  phones as recently seen. `NodeView::heard_here` is first-hand only, `#[serde(skip)]` like
  `absences`, and `None` prints `never`. `last_heard` still travels because v37 requires it, and
  nothing reports it.
- **A finished run is news until somebody has heard it, not for five minutes after it ends.**
  `gossipable_runs` stopped publishing a finished run `GOSSIP_TAIL` after it finished, so a
  decision made by a node out of contact was never told: the emulator cancelled a run before
  meeting anybody, and the laptop, the Mac and the tablet showed it `pending` for hours.
  `runs_to_publish` keeps it until `Cluster::exchanges` has moved since (within a day), and a peer
  that sends a live copy of a run known finished is told again (`Host::stale_copy`, `retell`).
- **…and the view forgetting a finished run is not the store forgetting it.** `record_run` wrote a
  fleet record whole on the view's word, and the view forgets finished runs after the tail: the
  tablet's stale `pending` copy arrived as a run never heard of and overwrote the laptop's stored
  `cancelled`. The store now refuses a live record at the same or a lower epoch against a finished
  one. A merge rule that lives only in a cache that expires is a rule with an expiry date.
