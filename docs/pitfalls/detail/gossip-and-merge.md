# Gossip, merge rules and who owns a fact — full entries

The working rules are in `../gossip-and-merge.md`, in this same order. These are the entries they
were compressed from: the mechanism, the measurement, and how each one was found. Read one
when the rule alone is not enough to act on.

- **A node must not believe a peer about its own absence.** `merge_node` has said so since it
  was written — "our own entry is ours… never by believing it about ourselves" — and `merge_run`
  did not, at the one place it decides whether work counts. Two missed probes, the arbiter
  gossips `Orphaned`, the holder accepts it about itself, and now it has no lease (`Orphaned`
  returns none by design): it cannot renew, and when its agent *finishes* `complete` is fenced
  out and the work is recorded nowhere. Narrow on purpose — a node still believes its arbiter
  about a run it is not holding. The refutation needs nothing new: keeping `Running` and
  gossiping it *is* the reclaim path, from the other end.

- **An observation cannot be relayed, so a decision and an observation settle differently.**
  Only the arbiter may declare a run orphaned, which is what stops a third node's stale copy
  undoing a decision — and it means an orphan half-delivered when its author crashed stays
  half-delivered: some nodes say `orphaned`, the rest `running`, about one holder at one epoch,
  and no gossip moves either. Benign, and worth recognising rather than fixing: an orphan grants
  no authority, and any *action* bumps the epoch, which wins outright everywhere. So a fleet at
  rest agrees about **who holds a run and under what epoch** — those are decisions — and may
  differ about whether that holder is still answering.

- **A holder's own copy beats a peer's — about the fields it owns.** `absorb` skipped a peer's
  whole record for any run this node holds, on the true premise that our store is newer about the
  state, the epoch, the lease and the checkpoint. The deadline and the priority are not ours: the
  home node owns them, `offload deadline` is forwarded there for exactly that reason, and gossip
  is the only way the edit comes back. So a run submitted on the laptop and running on the
  desktop kept the deadline it was submitted with for ever — which is not cosmetic on the holder,
  because that is the number ordering the runs waiting for a slot and deciding whether a late
  commitment is handed back. Two halves now: `merge_run` for the record, `merge_spec_edit` for
  the edit, with the settling rule written once (`settle_spec`). And what `absorb` hands the
  store is the record as *settled*, not as it arrived — the two differ whenever a winning record
  carried an older spec revision, and the loser never wins a later merge, so the store would stay
  behind for good. **The same split has to reach the store**, which it did not: the held-run
  branch handed its whole record down to be written, and the view's copy of a run this node is
  *running* is whatever the last gossip tick published. So an edit arriving a second after a
  checkpoint wrote the run back as it stood before that checkpoint — nineteen turns of somebody's
  night, undone by a deadline being moved, with the blobs then unreferenced and collectable. What
  crosses now is the edit (`Host::record_spec_edit`), applied to the row rather than carried in
  on a copy of it.

- **A spec has exactly two editable fields, and they share one counter.** `SpecEdit` is the
  whole of what an operator may change after submitting — deadline and priority — owned by the
  run's home node (its arbiter successor once that node is gone) and arbitrated by `spec_rev`,
  settled in `merge_run` *before* the record is. One counter because they have one owner: a
  record at revision N carries the spec as it stood after N edits, whichever field the last one
  touched. A third editable field needs an ADR, not a follow-my-leader — and an edit applied
  where it was typed rather than forwarded is two nodes writing revision 1 and undoing each
  other for ever.

- **Unreachable is not dead, and dead is not a decision.** Failure detection produces
  `Suspect` before `Dead`, and neither one on its own moves a run — the hold-down policy
  does. Collapsing these states re-creates the migration thrash the design exists to avoid.

- **A restarted node's own facts were ignored by every peer that knew its last life.** An
  incarnation lives in memory, so a node begins again at **0**, and `merge_node` copies a peer's
  facts only when the incoming incarnation is *strictly greater* — so if the peer's remembered copy
  is also at 0, **nothing** is copied: not capabilities, not policy, not the running set. Measured
  on two daemons through today's own feature: a mailbox nominated on a node that then restarted was
  invisible to the fleet for **37 seconds**, and cleared only because that laptop's cpu load kept
  changing and each change bumps the counter by one. On a device whose capabilities do not churn — a
  phone, the thing most likely to restart — it never clears, and `Reachable::in_view` is what
  ADR-0036's warning and `submit_run`'s refusal both read. The refutation that existed answers a
  different question: `absorb` refuted when a peer reported us as *not Alive*, and a peer holding
  stale facts says `Alive` while being wrong. Two lessons beyond the fix. **A `+1` cannot escape a
  number somebody remembers** — `refute_above` takes a floor, because the point is to outrank rather
  than to increment. And the rule now lives in `merge_node` alone, where `absorb` used to `continue`
  past its own entry and refute *beside* the loop: one question answered in two places, with the
  second case missing from both. Adding to a shipped rule is not the same as replacing it — the
  first version of this dropped "any suspicion, at any incarnation" and
  `a_node_refutes_a_suspicion_the_moment_it_hears_of_it` caught it.

- **An impolite death healed; a polite departure was permanent.** Two correct mechanisms with no
  path between them. `probeable` excludes `Draining | Departed` — "minus those who told us they were
  going", right while they are going — and a `Dead` node is re-probed for ever, "because that is how
  a node comes back". So the *good* behaviour was the unrecoverable one. And the one thing that could
  have revised it could not: `Cluster::introduce` built a `NodeView` at **incarnation 0**, so
  `merge_node` copied nothing and left `last_heard` untouched, while a restarted node's own gossip is
  rejected for being *behind* the incarnation its previous life reached. The consequence chains — a
  peer stuck at `Draining` is never probed and never gossiped to, so the node that answered the
  handshake receives nothing to learn from. Measured on two daemons: a peer re-met **40 times over
  four minutes**, still `draining`, last heard `7m`, with the other side's `offload nodes` containing
  only itself. `introduce` records first-hand contact through `alive` now, which is the *observation*
  path `serve_stream` already uses ("anything a peer sends is contact: it is alive, whatever we
  believed a moment ago") — an authenticated handshake is an observation, not a peer's claim about
  itself, so it outranks a remembered declaration. Note what made this newly urgent: ADR-0034 and
  ADR-0035 made `SIGTERM` announce departure properly, so every polite restart was poisoning every
  peer's view until that peer restarted too.

- **A seed was an address, dialled once — so the self-healing existed for multicast and not for the
  internet.** `Mesh::bootstrap` parsed `seed.parse::<SocketAddr>()`, refusing a name outright
  (`seed is not a host:port address`), which makes dynamic DNS unconfigurable — and it was called
  from `main` exactly one time, while `Mesh::discover`'s own doc comment says it "runs until the
  process ends". Measured with the old binary: **one attempt, given up after 30 seconds, and forty
  seconds after the peer came up the two daemons still knew nothing about each other.** Both fixed
  (ADR-0037 §2): `lookup_host` takes either form and is asked **on every attempt** — the name is the
  stable thing and what it resolves to is what changed — and the list is re-dialled while there is no
  live peer, with a backoff. Two things the walk added: a one-shot dial's grace period was QUIC's
  handshake timeout, which is why an earlier attempt at this walk accidentally *succeeded* (the peer
  came up four seconds in, inside the 30); and awaiting the dial in `main` delayed the whole delivery
  plane by seven seconds, so it is spawned now.

- **Advertise what you bound, not what the machine has.** mDNS will happily fill in every
  interface address, so a node bound to loopback announced `192.168.x.x` and peers dialled a port
  nothing was listening on — a working machine reporting `no route` about itself. A wildcard bind
  is the only case where "every interface" is the truth; a loopback bind is not advertisable at
  all and says so rather than announcing something misleading.

- **The default listen address was IPv4 only, and nothing had ever dialled a v6 address.**
  ADR-0037 §0 said to measure IPv6 before building any of the off-LAN plan, and for sixty sessions
  nobody did. Session ninety-two did: this laptop has a global `2001:…/64` and leaves on it
  unchanged (no NAT66), so a phone on v6-native mobile data could reach it directly — except that
  `listen` defaulted to `0.0.0.0:7433`. A throwaway test over the whole bind × dial matrix gave
  the rule: a `0.0.0.0` socket fails both ways on v6 (`invalid remote address` for `[::1]` and for
  the global address alike), while `[::]` connected on `::1`, `127.0.0.1` and the global address,
  to and from either kind of peer. The same held on two real daemons — seeded at `[::1]`, at the
  global address and at `127.0.0.1` — and with mDNS on. The control arm, bravo back on `0.0.0.0`,
  logged the old refusal for both v6 seeds. The trap under the fix: `[::]` is dual-stack only
  because Linux defaults `IPV6_V6ONLY` off, and that default differs by platform (and by
  `net.ipv6.bindv6only`). Inheriting it would have traded IPv4 for IPv6 on exactly those machines.
  With the option forced on, the test's `127.0.0.1` rows time out, so the test is what checks that it is set.
  quinn's error is also rewritten, because it blames the address and the fix is the socket.
- **The fix above printed the peer as `[::ffff:192.0.2.5]:7601`.** Read on the Mac's
  `offload status`, the first time a node listened on both families: the send-refusal breakdown
  keys on the destination quinn passes the socket, and for a dual-stack socket that is the
  v4-mapped form. Nobody wrote that address, so nobody recognises it. `SendRefusals::record`
  canonicalises now, and one peer spelled both ways counts as one row. The fix that introduced it
  was a session old, which is the ordinary distance for a report defect here. A change to what the
  transport binds is a change to every address it reports.

- **A node gossiped every run it had heard of as one it was running.** `active_runs` fills
  `NodeView::running` — "what this node says it is running", the ground truth a registry could be
  rebuilt from — and it was `list_runs(false)` with no holder filter, while the store deliberately
  keeps runs this node placed elsewhere *and*, since ADR-0025, work it neither ran nor submitted.
  Measured on two daemons: one run held by alpha, and `offload nodes` printed `RUNS 1` for **both**
  rows on **both** machines, about a node whose own `offload status` said `runs 0/2` and which had
  not been granted `host-runs` at all. On a fleet of five, one run reads as five. The predicate was
  already written two functions below, in `held`'s doc comment — "**Held, not merely known**" —
  which is the same bug, fixed for capacity in an earlier session and left live on the *gossiped*
  copy of the number. It survived because nothing *decides* on it: two reports read it and neither
  placement nor capacity does, so the cost was a wrong column and a false claim in a doc comment.
  The general shape: when a bug is fixed for the value a decision reads, look for the copy a
  *report* reads, and for the one that travels.

- **A leg that lost a run set the run's position for the rest of its life.** `RunProgress` was
  merged forward-only — larger wins, ties on the author's clock — on the stated grounds that its
  numbers have "a single author by construction: only the node running the turns produces them".
  A second grant is exactly what breaks that construction, and it needs no partition: ADR-0006's
  silence-is-a-decline makes "took the grant, answer lost" the ordinary case, so two legs of one
  run publish two positions. The lost leg started first, so it gets further, so *its* number is
  the larger one — and the surviving leg can never correct it, because for the next sixteen turns
  everything it says is smaller and refused. A run on turn 3 reporting turn 19, beside a worktree
  summary describing a checkout on a machine that is not running it, in the column somebody reads
  at 07:00 to find out whether there is uncommitted work. No tick fixes it. **Two kinds of number,
  two rules:** position follows the leg the record settled on (`RunProgress::absorb`, the same
  epoch-then-lowest-id arithmetic as `merge_run`, and it has to stay identical to it), spend stays
  cumulative from every leg. The `holds` check the last session declined on this path was never
  the fix — during a fork *both* legs pass it, and each is right; what was missing was the stamp
  that lets one position be **ranked** against another rather than merely compared for size.

- **…and that comparison has to be total, not merely forward.** The half of the same finding that
  needs no fork at all: two legs writing different worktree summaries in one millisecond tie on
  every field that was compared, so the merge kept whichever gossip arrived *first* — per node.
  A fleet at rest, fully connected, permanently disagreeing about a string, with `offload ps`
  giving two answers depending on which machine you ask. The tiebreaks run out somewhere, and
  wherever that is has to be a value every node can order the same way; the summary itself is
  where it runs out here.
- **Turn numbers are made cumulative *on the way in*, so don't reason about the per-leg counter.**
  Three places touch a run's turn count and only two of them are where you would look.
  `offload-agent`'s `Turns` counts boundaries in the stream it is parsing, so every resumed leg
  starts at one; `Supervisor::record_event` writes `stats.turns = turn`, an assignment, not an
  add; and `RunProgress::absorb` takes the winning leg's `turns` outright — deliberately, because
  position belongs to a leg (ADR-0005) and a `max` would let a stale leg speak for a newer one.
  Read those three and the conclusion is that `--max-turns` hands a run its whole budget again on
  every migration, which would be a serious defect and is not one: `drive` computes a
  `turn_offset` from the checkpoint on a `Start::Resume` and `accumulate_turns` shifts every
  `TurnBoundary` and `Outcome` by it *before* the pump hands the event to anything. So the number
  the writer assigns is already cumulative, and the merge is comparing cumulative counts.
  Verified rather than trusted, because the doc comment was not evidence: a run submitted
  `--max-turns 3`, drained mid-turn-2 so it migrated at the boundary, and node-b logged
  `spawning claude code` at 10:06:55.903 and `the run has taken the turns it was given; stopping
  it here turn=3 limit=3` at 10:06:55.925 — twenty-two milliseconds, on its first boundary. The
  capture at that limit succeeded on the migrated leg too (`checkpoint turn 3, copied to node-a`),
  `ps` showed `3/3` with the reason under it, and `offload resume` refused from **both** nodes.
  Worth writing down mainly as a warning in the other direction: somebody who finds this by
  reading rather than walking will reach for a `max` in `absorb`, and that is the one change here
  that would break the leg-ownership rule two entries above.

- **`writing_leg`'s `None` covered two situations that mean opposite things.** Every local write to
  a run's numbers goes through `Supervisor::update_stats`, which stamps the row with the writing
  leg so that `RunProgress::absorb` can *rank* this position against another leg's rather than
  merely find it larger. `writing_leg` returns `None` when this node has neither a `live` entry for
  the run nor `describes` it, and `None` was documented as "stamps nothing and leaves whatever the
  row already carried" — deliberate, for the case its comment names: a note about a run that ended
  here and whose `live` entry is long gone, where the stamp already on the row is this leg's own.

  After a restart `live` is empty for runs that ended **elsewhere** too. The stamp sitting on those
  rows is not ours — it is the winning leg's, absorbed by gossip — so the same `None` was granting
  permission to write a sentence in somebody else's name. And the write was not inert: `update_stats`
  set `stats.at = now()` unconditionally, `at` is the second component of `RunProgress::position`,
  and `superseded_by` at equal epoch and equal author falls through to `incoming.position() >
  self.position()`. Touching the row was therefore enough to beat the peer's own copy of it.

  Reached by `offload rm`. Session seventeen already fixed the loud half of this — a node with *no*
  checkout saying "removed" about another machine's disk — by noting only where `Workspaces::remove`
  actually returned `Removed`. That fix does not reach a node that ran an *earlier* leg, because
  nothing removes a worktree when a run leaves (ADR-0023's whole premise), so the checkout is really
  there, the removal really happens, and the note really is this node's to make. What it is not is
  the winning leg's to make. Measured in a fixture that hands the resulting row to the peer the way
  gossip does: the peer's `3 modified` became `removed`, about a worktree still sitting on its disk.

  The fix is one question asked before the write — is this row stamped by somebody else, and do we
  have a leg of our own to sign a correction with — and it lives in `update_stats` rather than at
  the one call site, so all seven writers get it. The local fact it drops (this node removed its own
  leftover checkout) has no field to live in, and inventing one is a gossiped field with an owner
  and an arbiter under ADR-0005 rather than a line in a bug fix. An **unstamped** row is nobody's
  and stays writable, which is what keeps a freshly submitted run's `waiting for a slot` and
  `preparing` working.

  Two things kept it from being over-strict, and the guard test asserts the second. Every writer of
  *spend* — cost, denials, asks, tokens, turns — runs while an agent is live here, so `writing_leg`
  answers `Some` for all of them and none is affected; and a run that ended here still owns its own
  row, so `offload rm` on it still says `removed`, restart or no restart. That control is in the
  test beside the failing case, because a rule of this shape is one line away from silently
  discarding every note the system makes.

  Found by widening the previous session's search by one word: not what a person and a machine both
  *call*, but what they both **write**.

## A new gossiped fact does not always need a new owner — put it where one already is

ADR-0005's rule is that every gossiped field needs a stated owner and a stated arbiter, and two
successive handoffs used it as the reason *not* to record which node had let a run go: "that is a
gossiped field with an owner and an arbiter, so it is an ADR", parked behind a bar of three
justifications. The bar was right. What was wrong was the shape everybody had in their head — a
new field on `Run`, beside `origin` and `spec_rev`, with a new clause in `merge_run`.

It went inside `RunState::Pending` instead. The state is already the holder's to write and
already rides on whichever record `merge_run` settles on, so the owner and the arbiter are the
ones the run has always had and there is no second rule to keep in step with the first. Two
properties that would otherwise have been *arranged* fall out of the placement:

- **It cannot go stale.** `assign`, `complete`, `fail`, `cancel` and `orphan` all replace the
  whole state, so the fact is gone the moment the run stops being pending — without anybody
  remembering to clear it. A field on the record would have needed clearing at five transitions
  and would have been forgotten at the sixth, which is the class of bug this repository has
  already paid for twice under different names.
- **Every way in has to state it.** `Run::new` writes `None`, `reopen` writes `None`, `release`
  writes the node, `checkpointed` writes what the request carried. The compiler asks, because the
  variant has the field.

The general question is the useful one: **what is the fact a property of?** *Which node let this
run go* is a property of the run being pending, not of the run. Get that wrong and the machinery
that follows is all correct and all unnecessary.

## …and a field a peer can erase by relaying earns a version bump, even when it is `#[serde(default)]`

`offload-proto`'s rule is "bump when a message changes shape in a way an older node would
misread", and it has three recorded non-bumps: `Gossip::progress`, `RunProgress::{by, epoch,
tokens}` and `Run::origin`. Read quickly, `let_go_by` is a fourth — a defaulted field on a JSON
body, whose default is exactly today's behaviour.

The difference is what an old build does when it *relays*. `origin` is immutable and identical in
every copy, so a v-old node that drops it re-emits a record nobody disagrees with. `let_go_by`
lives inside a state that peers gossip and that changes: an older node decoding the release,
dropping the field, and winning `merge_run`'s equal-epoch tiebreak re-states the run as `Pending`
with nothing on it — and what that erases is the only thing that makes the run placeable. The run
goes back to being stopped in front of a fleet that would continue it, reached by a node that
meant nothing by it and logged nothing about it.

So the question to ask of a defaulted field is not "would an older node ignore this" but **"could
an older node relay something that erases it, and is the default the safe answer if it does"**.
For `tokens` the answer was that a relay has the same position and cannot supersede. Here the
answer is that it can, and that the safe-sounding default *is* the failure. Wire v24, and
`MIN_VERSION` with it, which costs nothing while no older node exists outside this repository's
history and is the whole point of moving them together.

- **A signed fact and an unsigned one about the same field is two owners, and the merge let the
  unsigned one win.** Three places in this tree say the certificate's name is the one the fleet
  displays. `main::report_membership`, at startup, with a `WARN` when the two differ:

  > *Two names for one machine, and peers only ever see one of them. The certificate's is signed,
  > so it is what the fleet displays (`record_name`); the config's is what every local command
  > shows.*

  `Cluster::serve_session`, calling `record_name` with `session.peer.name` — which `handshake::
  admit` builds from `cert.name`, so it is the signed one and not a claim. And ADR-0012, at the
  point it defines the certificate: `name: String, // cosmetic, for offload nodes`.

  What the code does is copy the gossiped name over it. `ClusterView::merge_node` takes a peer's
  facts wholesale when `incoming.incarnation > existing.incarnation` — capabilities, policy,
  running set **and name** — and `Cluster::set_capabilities` bumps the incarnation whenever the
  probe finds anything changed, which on a laptop is the cpu load, every thirty seconds.

  Measured on two daemons on one machine, neither with `name` in its `node.toml`, so both took the
  hostname. Alpha restarted, then `offload nodes` every five seconds:

  ```
  07:02:59  peer name = bravo      ← the handshake
  …
  07:03:19  peer name = bravo
  07:03:24  peer name = fedora     ← the first merge above incarnation 0
  07:03:29  peer name = fedora     … and for ever
  ```

  Twenty-five seconds. And the consequence is not one column: `offload explain` reads the same
  field for `holder`, `arbiter` and every canvass row, so one screen showed `holder fedora`,
  `arbiter fedora (this node)` and the two nodes' bids both labelled `fedora` — two machines under
  one name, in the report whose whole job is saying which machine has the run.

  **The fix corrects `incoming` on the way in** rather than repairing the entry afterwards.
  `absorb` takes the certificate names once, before it locks the view (the certificates have their
  own lock and an ordering nobody should have to reason about is not worth the tidiness), and
  overwrites the name on each incoming `NodeView` that this node holds a certificate for. Three
  properties fall out: `merge_node` stays in `offload-core`, which has no certificates to consult;
  no reader ever sees the wrong name for an instant, because nothing is written and then fixed;
  and a peer learned by **relay** — gossiped about by a third node, never handshaken — keeps its
  gossiped name, which is the only one there is and is the honest answer rather than a gap.

  Why no test caught it: every helper in `offload-cluster/tests/mesh.rs` builds a node with
  `TestNode::new(key, seed, name)` *and* `node_view(node).named(name)` — one name, used twice. A
  fixture that cannot represent the disagreement cannot fail on it. `join_named` is the helper
  that can, and `a_peer_is_shown_by_the_name_on_its_certificate_and_gossip_does_not_take_it_back`
  goes red with `left: Some("fedora")` against the merge with the correction forced off.

- **…and the node's own entry is the same fact from the inside.** `merge_node` returns early for
  the local id — a node is not told about itself — so the local `NodeView` keeps whatever it was
  constructed with, and `mesh::start` constructed it `.named(&config.name)`. That entry is what
  the node **gossips about itself** and what `Cluster::name_of(self.node())` answers, which is the
  `by` on a forwarded cancel (`ClusterMessage::Cancel`) and on a forwarded answer. So a run's own
  log, read on the machine that ran it and on every machine that ever learns of the run, said
  `cancelled from fedora`.

  `fleet::display_name(state_dir, configured)` is the one answer to *what is this device called to
  the fleet* — the certificate's name if it has one, else the configured one — and it is read from
  `fleet.json` rather than cached at startup, for `server::revoked_refusal`'s reason: `offload
  rekey` rewrites that file under a running daemon. Three callers: the cluster's own view entry,
  the synthesised local view on the no-cluster path, and `server::cancel`'s `by`. `offload status`
  keeps `config.name`, which is the local half of the split and the thing the startup warning
  exists to explain.

*Backfilled in session ninety-one from `docs/sessions.md` and the commit that added each rule —
sourced, not reconstructed from the rule text.*

- **Nothing in this project had ever run on two operating systems, and it does not.**

  The rule itself carries the mechanism, because it was rewritten across sessions sixty-four to
  sixty-six as the hunt went on (commits `bc8ba78` through `91a3581`); the narrative is
  `docs/sessions.md`'s *Session sixty-six — the cross-platform failure was two faults, and the
  instrument was the third* and *Session sixty-five*, and `docs/DEMO.md`'s Linux ↔ macOS section.
  In short: `QuicTransport::accept` ran each inbound handshake inline, so one stalled dialer shut
  the door on every later peer (fixed), and macOS 26 refuses LAN sends from an ad-hoc-signed binary
  (Local Network permission, a setting). The instrument lessons from that hunt are in
  `testing-and-sweeps` — `quinn-udp`'s `send` returning `Ok(())` for every error but
  `WouldBlock` measured 0 of 40 once switched to `try_send`, where it had "measured" 10 of 10.

- **A fork's second leg was spent and not counted.** Found on the laptop and the Mac in session
  ninety-two. WARMUP finished on the Mac at epoch 1; the laptop, which had the Mac `dead`, ran it
  again at epoch 2; both then agreed on 110 tokens when 220 were spent. Nothing was wrong with the
  position rule (`absorb` takes the winning leg's tokens, because they are cumulative along a
  chain) or with cost's `max` (each leg adds its own cost to a row that, along a chain, already
  holds its predecessors'). Both are correct for a chain, and a fork is not one. The owner chose to
  count it. The fix leaves both rules alone and adds `RunProgress::legs`, one entry per `(by,
  epoch)`, holding the base the leg started from, the tokens it reached and the cost it added.
  `spent()` sums (tokens − base), which along a chain equals the position exactly, since each
  base is where the previous leg ended. The base is counted at the leg's start from the restored
  checkpoint transcript, not from whatever the row held, because gossip may or may not have
  delivered the predecessor's numbers by the first capture. And the union needs a total order per
  leg, since two copies of one entry must settle the same way everywhere. The order-independence
  property now generates leg entries too.

- **A per-leg base taken from the row instead of the checkpoint, and every drain would have said a leg
  was lost.** Walked with a three-turn fake agent drained on bravo at turn 2 and resumed on the laptop.
  `ps` on both nodes: `330` and `+110 tokens spent by a leg that lost`, about a run that had never
  forked. The stored entries said the resumed leg's base was 110 when the checkpoint it restored held
  220. Before any turn, the new leg's `update_stats` calls ("preparing", "workspace ready") reached the
  wrapper, which created the entry with base = the tokens then in the row. That was the old leg's
  turn-1 capture, since the turn-2 numbers had not gossiped over yet. The start's own read of the
  restored transcript then met an existing entry and, by design ("begun once"), left it. The unit
  tests passed throughout because they build entries directly. Fixed by making the start the only
  creator. The general form: when a value is only knowable at one moment, let nothing else create the
  record that stores it, because a creator elsewhere has to guess, and a guess here is a false alarm.

- **"It was Doze" was the first reading, and the logs said a peer.** Session ninety-two left the phone
  app meshed overnight on mobile data. By morning the phone had refuted its death 3 790 times, the
  laptop had logged `answered: back after being marked dead` 713 times, and `marking dead` for it
  **zero** times. So the laptop never concluded anything; it adopted a peer's claim. The only peer
  was bravo, still running because its pid file held `setsid`'s pid (DEMO.md's trap), and bravo's log
  had `no answer from anybody: marking dead` 1 772 times. The direct probe had to fail, since nobody
  dials a phone on a carrier. The indirect one should not have: the helper answers a `PingReq` with
  its own `probe`, which goes through `session()`, and `session()` looked only in `connections`, the
  sessions this node dialled. The phone's connection was in `inbound`, kept only so the node could
  hang up on it. Three changes and one regression. `session()` falls back to a non-closed inbound
  session. A node serves streams on what it dialled. A certificate objection hangs up both ways, or
  the peer-dialled session keeps presenting the old certificate, as the new test
  `…_when_the_peer_dialled` showed. And `is_closed` exists because `disconnect` leaves the entry for
  its serving task to remove, so a moment after a hang-up the closed session was still there to be
  picked. The rule the whole thing adds: **when a role is split across two maps, check every caller
  that looks in only one.**
- **…and every question about "the connection to that peer" has to use the same lookup.** The
  entry above ends with its own rule, *check every caller that looks in only one map*, and the
  session that wrote it did not. `peer_certificate` in `place.rs` still read `connections` alone.
  Found in the iOS Simulator walk: the iOS node was arbiter, the Mac peer had just restarted and
  dialled it, and the first `offload run --task` came back `mac-peer  bid, but its connection
  closed before its certificate could be checked`. The retry passed, because the refusal hangs up
  both ways and the next round dials out, which works on a LAN. Behind a carrier firewall there is
  no dialling out, so a phone could never have hosted. The test that should have caught it,
  `…_when_the_peer_dialled`, only asserted that its first round placed nothing, and the round was
  being refused for the wrong reason (see testing-and-sweeps). The fix is one function,
  `Cluster::existing_session`, used by both `session()` and `peer_certificate`. Fixing the old test
  to block the other path found a second hole in the same lookup: a *dialled* session that the far
  end had closed was still returned, so the first exchange after any hang-up failed. It is skipped
  and forgotten now. Walked after the fix: three peer restarts, each followed 15 s later by a task
  placed on the restarted peer.
- **A peer's handshake certificate goes stale on a long connection.** Walked on the ADR-0069 §3
  re-approval walk, with 4-minute certificates. At minute 6 the approver's `offload status` and
  `offload reapprove` listed only itself, while the joiner's approval was just as due. The joiner
  had renewed at minutes 3 and 6 over the connection it opened at minute 0, so the approver still
  held the minute-0 certificate, expired at minute 4, which `is_due_for_reapproval` filters out.
  Deciding by id still worked, because `RenewMe` carries the current papers. Fixed by keeping the
  carried certificate once `Members::admits` verifies it (fleet key, liveness, revocations), and by
  keeping what the approver issues. Re-walked: the joiner was listed, `--due` decided, and it was
  re-approved 18 s later. Control-run in `a_certificate_running_out_is_re_issued_by_an_approver`:
  without the change, the approver holds the old expiry.

- **A "replace my own contribution" rule never drops a record nobody holds.**

  Session ninety-two, found by the owner asking how much battery Offload uses. Android put the phone's
  app at the top of the phone's list, 59 mAh in its first 31 minutes off the charger. With the walk
  fixtures removed, its daemon still used 24.9% of a core idle, against the tablet's 2.9%. The
  the phone's profiler is off on Samsung's user build, so the laptop's node in the same fleet stood in
  (29.1% idle against 1.7% for a node in the tablet's fleet, same binary). Sixty `eu-stack`
  snapshots put every busy thread in `probe_round → probe → ask → frame::encode`, or in
  `serve_stream` decoding, serializing `ProgressReport` lists. `Cluster::gossip` sends
  `view.runs` and `view.progress` whole. `publish_runs` retained runs with `holder() != Some(me)`,
  which keeps every finished run. The view grew with every run the fleet had ever seen, and a
  restart did not shrink it, because peers taught it back. Fixed by `forget_finished_before` at the
  tick, with the same `GOSSIP_TAIL` `gossipable_runs` publishes by. After: the phone 6.1%, laptop 5.9%.
  No wire change. A node on an old build keeps sending the history until it is updated.

- **One probe timeout for the fleet is wrong for a phone** (with the two rules after it).

  Session ninety-two, after the gossip fix brought the phone's idle CPU down. It still flapped 22
  times in 10 minutes, and `emu-fresh` 17. The first change, re-dialling only after three timeouts
  in a row, took the emulator to 3 and left the phone at 22. Tracing the probe round trips on the
  laptop (`--log info,offload_cluster=trace`) gave the answer: 104 answers to the phone in three
  minutes, median 82 ms, p90 414, max 503, bunched against the 500 ms cut, plus 17 timeouts (14%).
  With a per-peer RFC 6298 timeout: 406 answers, median 79, p90 563, p99 705, max 1 060, mean
  timeout 983 ms, 5 timeouts, 2 suspicions refuted, and **0 flaps** for either node in 10 minutes.
  The dead-node change broke `a_healed_fleet_agrees_everybody_is_alive`: mutual `Dead` outlasted
  a five-round quiet window. The property holds at 200 cases once the window includes `DEAD_EVERY`.

- **Patience for a slow peer must not become patience for a dead one** (with the three rules after it).

  Session ninety-two, the night after the phone's flapping fix, with the owner asleep. After the laptop
  node restarted, the Mac mini (just joined as `macmini`) and the laptop stayed apart for minutes.
  In order, the causes found:
  - the previous evening's changes: a dead node probed one round in 30, and three timeouts needed
    before a redial, while the connection to a restarted peer answers nothing. Also the all-dead
    rotation indexed by `round / DEAD_EVERY`, which put one node (with no address) in thirty rounds
    running. Fixed;
  - `Mesh::discover` skipped `cluster.knows(node)`, so the laptop, which knew the Mac from the
    the phone's gossip as dead, discarded the Mac's mDNS answer (it did resolve one at 22:56:27). Fixed
    to skip only an alive node;
  - addresses did not travel, so the laptop had no address for the Mac at all ("no route …
    not discovered yet") and depended on the Mac dialling in. ADR-0076 gossips them. Walked: the
    laptop reached the Mac at a gossiped IPv6 address, and the Mac reached the phone at a gossiped one;
  - the environment: the Mac's `offload status` listed 38 sends refused by its own kernel with
    `ENETDOWN`, to the laptop and the phone alike. Its link drops, as `docs/DEMO.md` already said.
  Theories that were measured and dropped on the way: ARP flux on the dual-homed laptop (ARP
  correct), asymmetric routing (raw UDP 20/20 on both laptop addresses), macOS Local Network privacy
  (a fresh C binary sent fine), and a macOS dual-stack socket not delivering IPv4 (python listeners
  on `[::]` and `0.0.0.0` both received).
- **A fleet-wide request is passed on before the node asks whether it has anything to do.**
  ADR-0080, session ninety-four. `read_models` in the daemon raised `Cluster::models_asked` inside
  the branch that ran after `let Some(agent) = … else { continue }`. The tablet's app node has no
  agent, so a Refresh pressed there continued before raising anything. Walked: Refresh at 11:43:45Z
  on the tablet, and the laptop's log showed no read. With the raise moved above the agent check,
  Refresh at 11:48:43Z was answered by `read the agent's model list … reason="asked by the fleet"`
  on the laptop at 11:48:44.9Z.
- **…and "the first count a node hears is old news" means the first *exchange*, not the first
  value.** The first cut kept `answered: Option<u64>` in the reader and treated the first value it
  learned as covered by the startup read. A `watch` does not fire for its initial value, so a node
  that restarted into a fleet whose count was 0 learned nothing, and the next Refresh (count 1) was
  the first value it learned, adopted without a read. Found by walking the rule on paper before the
  button was pressed. The cluster now adopts a count without waking anybody only in its first
  absorbed gossip. The mesh test's control run, with `&& !first` removed, fails on "without waking
  its reader".
- **"When did I last hear from it" is an observation, so gossip must not move it.** Session
  ninety-four. Seen first on 2026-09-27 at 12:44: the laptop's `offload nodes` printed `macmini dead
  ~599 … 3s` while the tablet said `alive`. `summarize` printed `now - last_heard`, and `merge_node`
  wrote `existing.last_heard = now` whenever an incoming copy carried a higher incarnation, whoever
  relayed it. The insert path took the relayer's `NodeView` whole, `last_heard` included, so a node
  this one had never spoken to showed the relayer's contact time: the stopped phone read `alive … 1s`
  on the laptop. `heard_here` is written only by `Cluster::alive` (a handshake, a probe or
  indirect-probe answer, any message a peer sends) and on this node's own entry. Walked after the
  fix: the three stopped phone apps read `never`, and the Mac `now`. Opening the tablet's app turned
  its row to `alive … now` within seconds. The test `a_node_nobody_has_spoken_to_is_shown_by_its_id`
  had asserted "last heard 0 ms ago" for exactly that node.
- **A finished run is news until somebody has heard it, not for five minutes after it ends.**
  Session ninety-four. The run `01a0df9ff628` ("fix the typo in the title") was cancelled on its home,
  `emu-fresh`, at about 14:36Z, right after the emulator's app was opened and before it had met any
  peer. Every copy elsewhere stayed `pending`, and `explain` showed epoch 0 on both sides, so the
  merge would have taken the cancel. It was never sent: `gossipable_runs` filters finished runs to
  the last `GOSSIP_TAIL` (5 min). With `runs_to_publish` on the emulator, opening its app at
  16:45:40Z put the laptop and the Mac at `cancelled` by 16:45:45Z.
  `a_finished_run_is_published_until_it_has_been_told_not_only_for_the_tail` and the mesh test
  `a_peer_with_a_stale_copy_of_a_finished_run_is_noticed_and_put_right` pin it.
- **…and the view forgetting a finished run is not the store forgetting it.** Walking the retell,
  the tablet was opened with its stale `pending` copy while the laptop's view had already aged the
  cancelled record out. The laptop's `ps --all` went back to `pending`: `absorb` inserted the copy
  as new, and `record_run` did `save_run` over the stored `cancelled`. Found only because the walk
  checked the laptop as well as the tablet. After the guard, the tablet was corrected at 17:05:21Z,
  eight seconds after opening, and the laptop stayed `cancelled`. The control run of
  `a_stale_live_copy_cannot_reopen_a_finished_run_in_the_store` fails with the guard disabled.
