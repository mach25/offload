# Membership, certificates and credentials — full entries

The working rules are in `../membership-and-credentials.md`, in this same order. These are the entries they
were compressed from: the mechanism, the measurement, and how each one was found. Read one
when the rule alone is not enough to act on.

- **A node may refute `Suspect`; it must never refute `Revoked`.** `incarnation` exists so a
  node can argue with peers who guessed wrong about its liveness — correct there, and fatal
  if the same mechanism reaches membership, because a revoked device would talk its way back
  in. Same gossip path, same-looking fact, opposite rule (ADR-0012).

- **A membership command runs with no daemon, so the daemon has to look again.** Every one of
  them writes `fleet.json` from a process that is not `offloadd` — that is ADR-0012's central
  claim in operational form — and for two phases nothing re-read it. `offload grant host-runs`
  re-issued a certificate the daemon kept not presenting, and `offload revoke` wrote a fact it
  never read, which made "revocation is immediate and local" mean "immediate, once you remember
  to restart" on every machine, because the daemon is always up. The whole file is parsed each
  tick rather than compared by timestamp: two writes inside one filesystem timestamp — `join`
  then `grant`, which is how a device is set up — would leave the second invisible.

- **Membership is checked at the handshake, and a live session never handshakes again.** So a
  node that files a revocation and keeps the socket open has recorded a fact and changed nothing.
  Anything that changes who may talk to whom has to *close connections* in the same breath:
  revocation drops the one peer, a rekey drops all of them.

- **A certificate has a month on it, and a backstop nobody renews is a deadline.** ADR-0012
  leans on expiry twice — it is what makes an unheard revocation eventually bite and what
  removes a device that has been away too long — and nothing renewed anything, so the fleet's
  real behaviour at thirty days was every device dropping out at once. Renewal is *asked for*,
  never offered: the node whose certificate is lapsing is the one that knows, and an approver
  volunteering would renew nothing for the laptop that has been shut for three weeks. And
  `RenewMe` carries nothing — what is re-issued is the certificate the handshake authenticated,
  which is `admit`'s claimed-versus-proved split applied to the other place a certificate is
  minted.

- **Probation follows the grant, not the path.** It only ever suppresses `HostRuns`, so setting
  the flag on a certificate without that grant delays nothing — which means "an invite skips
  probation" is a statement about the *door*, where the grant is not in play. ADR-0012's
  mitigation 2 is about the grant and has no exemption for the passphrase, because somebody
  holding the passphrase is the threat it was written for. Read it the other way and
  `offload invite --grant host-runs` becomes the documented way around it.

- **…and a flag carried forward is not the rule it stands for.** `add_grant` inherited the
  certificate's `probation` flag, which is right for a node still serving one and wrong for the
  path people use: an *invited* certificate carries `false` for the door's reason, so `offload
  grant host-runs` on an invited laptop took effect the same second — measured, `grants submit,
  deliver, host-runs` immediately, with the CLI's own "dormant for another 15 minutes" line
  unreachable by that route. The flag is derived now: a certificate probates exactly when it puts
  `HostRuns` in play that is not in force already, so an unrelated re-issue never suspends
  hosting a node already had, and a grant still being served out restarts the window rather than
  clearing it.

- **A rekey must not re-invite a revoked device.** `known_members` reads the enrolment log, which
  is what this machine *witnessed* and has no opinion about what happened next — so `offload
  revoke <node>` followed by `offload rekey` printed that node a fresh invitation, into a new
  fleet whose revocation list starts empty. The strongest eviction undoing the weaker one, with
  the token there to paste. The rule is `renew_for`'s and its test already said why: "the day it
  becomes reachable is the day a background loop nobody is watching re-papers an evicted device."
  What is left out is named in the output, because a device silently missing from a rekey's list
  is somebody wondering which command lost it.

- **An invitation is not a bearer token, and that is what makes it simple.** It is a certificate
  naming the joining device's key: useless to anybody else, so it needs no expiry of its own and
  can be pasted into a chat window. The rule it must not break is the other one — the passphrase
  is never on a command line — and it does not.

- **A device cannot tell its own fleet moving from somebody else's fleet taking it.** An
  invitation to an unfamiliar fleet is exactly what an attacker would send, so `offload rekey`
  carries a `Succession` signed by the key being replaced, which the device already holds. Both
  ends are checked: a valid succession out of some *other* fleet says nothing about this one, and
  one naming a successor other than the certificate's issuer would move the device somewhere the
  signer never authorised.

- **…and the same question about `NodeView::absences` had never been asked** (ADR-0031). A peer's
  absence history is what *this* node has seen of that peer, and it was a plain field on the
  gossiped struct — so a peer learned **indirectly** inherited the relayer's history of it wholesale
  (`merge_node`'s insert path takes `incoming` entire) while one learned **directly** started at
  zero, because a node's report about itself always carries zeros. Two answers, decided by the order
  a node met the fleet in. It does not travel now; it is durable instead, which is where the value
  was needed — a restart is precisely when a peer has just been absent. And the seeding is
  idempotent (never overwriting anything learned in this incarnation), which is what makes "a live
  observation beats a remembered one" one line rather than a rule to remember.

- **A gossiped fact needs an owner; an observation does not have one.** ADR-0012 says an
  enrolment is "written durably to every node's store", which taken literally is a gossiped fact
  — and ADR-0005's table is the reason it is not built that way. Two nodes recording the same
  enrolment are not disagreeing, they are describing what each of them saw, and merging them
  would mean deciding which observation is authoritative about an event neither owns. So the
  fleet log is per-node and `offload nodes --history` says so. Sometimes the answer to "who owns
  this field" is that it should not be a gossiped field.

- **An alarm nobody can act on is not a control.** ADR-0012 has no per-join approval and says the
  announcement is the compensating control, which makes it load-bearing rather than a nicety:
  probation's fifteen minutes are bought so that revoking stays ahead of an attacker, and they
  buy nothing without it. It has two producers on purpose — the CLI (these commands run with no
  daemon, so an alarm needing one is missing on the machine somebody just walked up to) and every
  *other* node when it meets an unfamiliar member (the inviting machine may hold no route to a
  person, and may be the machine that was used).

- **A grant is enforced where it is used, against the certificate as it stands right now.**
  The handshake admits a member; the *bid exchange* is where a peer without `HostRuns` is
  refused, because a node without it is still a full member. `Peer` carries the certificate
  rather than a snapshot of its effect — probation ends while a connection stays up — and an
  objection also drops the connection, because `offload grant` mints a new certificate the
  live session has never seen. Check a snapshot instead and one direction refuses a legitimate
  host until a redial nobody triggers; skip the drop and a fresh grant works only when the
  link happens to break.

- **A delegation bounds who may issue, and had to be made to bound what.** ADR-0012's own
  sketch has `may_issue` on the `Delegation` and the code never grew it, so an approver — which
  is a delegation plus a node key — could mint `HostRuns` for any device, probation cleared, and
  every peer admitted it. Mitigation 1's whole point is that this grant stays behind the
  passphrase. A flat prohibition is the wrong fix: **renewal restates grants**, so an approver
  that may never sign `HostRuns` cannot renew a host either and every host needs the passphrase
  monthly. What a verifier needs is *minting* versus *restating*, so a renewal carries the
  fleet-signed certificate it descends from (`MembershipCert::authority`) and may grant no more
  than that did — the chain's **root**, so a host renewed for years carries one link, with its
  expiry unchecked, because the grant was made once and the renewal is what keeps it current.

- **…and "the connections" means the ones the *peer* opened as well.** `Cluster::disconnect`
  removed the peer's entry from `connections` and closed it. That map is keyed by node and holds
  the one session *this node dialled*; a session the peer dialled is handed to `serve_session`,
  which owns it inside a spawned task and put it in no map at all. So the hang-up reached exactly
  the half that did not matter. The doc comment on `Mesh::refresh_membership` states the rule
  correctly — "refusing the next connection to a device that is not going to make one is not a
  revocation" — and the revoked device is the one that *does* keep making them.

  Measured on two daemons, `offload revoke` typed on the arbiter while the other node was
  eighteen turns into a run it was hosting. The revoked node kept its inbound QUIC connection and
  went on gossiping over it: run records, lease renewals and checkpoints from turn 8 to turn 40,
  and it finished the run and reported it home. `offload nodes` on the arbiter said `bravo alive
  ~48` — forty-eight absences in ninety seconds, and still `alive` — because the arbiter's own
  probe was correctly refused (`no answer: suspecting`) while the peer's inbound traffic came
  straight back through `serve_stream`'s "anything a peer sends is contact" (`answered: no longer
  suspect`), once a second, for ever. The run was therefore never orphaned and never moved. A
  restart of the arbiter cleaned it up completely, which is what made it invisible: there is no
  inbound session to survive, so every walk that restarted a daemon saw a working revocation.

  The unit test above it could not see it either, and for a reason worth carrying: **the test
  asserted about the node doing the dialling.** In `a_revoked_peer_stops_being_talked_to_rather_
  than_merely_being_marked` the desktop probes the laptop, so the only session is outbound and
  removing it from the map is enough. And the in-memory transport's `close` was a *no-op* — its
  comment said "dropping the sender is what the peer observes" — so what that test actually
  proved was that the session had been **forgotten**, not that anybody had hung up. A test double
  that ignores a trait method cannot distinguish the two, and those two are the halves of a
  revocation. `MemoryConnection::close` now really closes, including waking an `accept` already
  parked, which is where the serve loop a hang-up has to end is waiting.

  `Cluster::inbound` is the registry, keyed by a token rather than by node because one peer can
  hold more than one inbound session and a map keyed by node would leave the older one open.
  `disconnect` closes both directions; `drop_connection` — a *failed probe* — still drops only
  what this node dialled, deliberately, because a probe failing says as much about the path as
  about the peer and an inbound session that is still delivering is the better of the two answers.

  **The residual was that the fix produces two agents** — closed by ADR-0044, in the entry below.
  With the fleet no longer believing a revoked node, the run is orphaned 1.7s after the revocation
  and resumed on the arbiter at epoch 2, while the revoked node, which cannot hear anybody, ran its
  old leg to the end. The partition half of that is ADR-0002's trade and stays; the half that was
  not a partition is that this device could hear its fleet perfectly well, on every dial it made.

- **The handler was on the path that cannot happen, and the path that happens had no handler.**
  `FleetChange::ourselves` — this node is the one that was revoked — was set when `fleet.json`
  grew a revocation naming this node, and the only way one arrives on a revoked device is by
  gossip: from a peer that has just hung up in both directions and refuses every handshake it
  attempts from then on. The revocation is exactly the thing that ends the channel that would have
  carried it. Meanwhile `Refusal::Revoked` arrives on **every dial**, once a second, for as long as
  the daemon runs, and `TransportError::Refused` was matched nowhere: on the reverted build it
  surfaces as `could not reach seed … refused by 32388d1a: member 64d1ca2e has been revoked`, at
  `WARN`, beside every other unreachable peer.

  Measured, two daemons, node-b holding only `submit, deliver` so it can revoke without waiting out
  probation. Before: revoked at 13:23:28.955, still `running` at turn 38 forty-five seconds later
  with `runs 1/2 · accepting yes`, and **completed at turn 60** — forty-eight turns of agent after
  the fleet threw the device out, while node-b held the run `orphaned` and node-a `dead`. After:
  revoked at 13:26:33.548, the proof verified and filed at **+1.70s**, the agent stopped at
  **+2.43s**, the run `failed` at turn 15 saying *"this node was revoked from its fleet, so it
  stopped running it"*, and one `spawning claude code` in the whole log.

  **A restart hides all of it.** A daemon that comes back reads `fleet.json`, finds its own
  certificate unusable and does not join the mesh at all — so the symptom exists only on a live
  process, which is the same reason session forty-two's half went five sessions unnoticed.

  And it is the fifth path `stop_accepting` has been missing from: auto-resume asks neither the bid
  nor the grant, so shutting those two doors leaves the one that spawns agents without being asked.
  `Circumstances::revoked` is checked above everything including `departing`, and unlike a drain it
  does **not** become `Recovery::LetGo` — letting go is an offer to the fleet, and a revoked node's
  writes reach nobody. Three more rules fell out of hooking it up, below.

- **A refusal is a peer's claim about this node; the signature inside it is not.** The rule that a
  node must never believe a peer about itself is not a technicality here — pointed at membership it
  is what stops a device talking itself back in, and a node that acted on a bare `Refusal::Revoked`
  could be stopped by any peer that said so. What is not a claim is the signed artifact: a
  `Revocation` verifies against the fleet key the subject already holds, names the fleet it belongs
  to, and names the subject — three checks it makes alone and offline.

  So `Refusal::Revoked` carries `proof: Box<Revocation>` (wire v25) and `handshake::admit` uses
  `find` rather than `any`, so what travels is the artifact the refusing node checked instead of a
  sentence it composed. `Cluster::session` hands it to `Members::revoked`, which is the same call,
  the same verify and the same file a *gossiped* revocation goes through; a forged one is a warning
  naming who tried, and one naming a third party files nothing — it arrived on a dial, so the
  subject should be us, and a message addressed to us is not authority to evict somebody else.

  The field is **required**, which is what earns the bump. A refusal that decoded with the proof
  missing would be one the subject had to act on without it — the failure the field exists to
  prevent, arrived at politely — and this is the one message a node receives *after* being thrown
  out, so there is no later exchange to carry it instead.

- **A fact that latches must not be reported as an edge.** Even filed, `FleetChange::ourselves`
  could not have reached the daemon's membership tick: `NodeMembership::file` reloads to refresh its
  own copy and **drops** the change it gets back, and `file` is precisely the path a revocation
  heard from anywhere else arrives on. So the one announcement was eaten by the code that wrote the
  fact down, and the poll a second later was told nothing had happened.

  Whoever observes an edge first consumes it, and here the first observer is the writer.
  `NodeMembership::revoked_here()` is a standing question instead, asked every tick: revocation is
  monotonic, nothing un-revokes a device, and `offload rekey` founds a *different* fleet rather than
  undoing one. `Supervisor::stand_down` is then idempotent by construction rather than by a flag —
  `halt_agent` takes the cancel sender, so a second pass finds nothing to signal — and the latch
  decides only whether there is anything to say.

- **A certificate that still verifies is not permission.** `FleetState::grants` read the
  certificate alone, and a revoked device's certificate lists its grants and verifies perfectly:
  that is the whole reason revocation is the exception ADR-0012 makes it. So every door that asked
  stayed open — `offload status` said `accepting yes`, the bid and the grant passed their
  `host-runs` check, and `offload run` submitted. It now returns nothing once the node is revoked,
  in that one place, because "may this node do X" is one question with one answer.

  And the sentences above it had to change with it, both of them advice that cannot work:
  `grant_refusal` said *run `offload grant`* to a device the fleet had evicted, and `offload status`
  said *drained; restart offloadd to take work again* because standing down sets that latch too.
  Revocation is asked above the drain for exactly that reason.

- **A credential's signed message is a wire format, and only a pinned digest says so.** Same
  rule as the fleet key derivation and the same silence: add a field to `MembershipCert::message`
  and every certificate in every fleet stops verifying, with no error and one symptom — every
  device drops out at once. The derivation had a known-answer test from the start; the signing
  bytes did not, and a change to them broke every credential with nothing going red.
  `the_signing_bytes_are_pinned_to_a_known_answer` covers all four credential types now. A
  deliberate change updates the digests **and** says in its commit message that every device
  re-joins, because there is no migration for a signature.

- **A membership certificate is not a bearer token.** It is public, copyable, and meant to be
  shown to strangers; what makes it non-transferable is proving possession of the key it
  names. `proto::handshake::admit` therefore takes what the peer *claimed* and what the
  transport *authenticated* as separate arguments. Pass the claim for both and every check
  below it becomes decorative while every test still passes.

- **The fleet key derivation is a wire format.** Salt, argon2id parameters, the wordlist and
  the passphrase normalisation all feed the same 32 bytes, so editing any of them silently
  re-founds every fleet: certificates stop verifying and every device has to re-join. There
  is a pinned known-answer test whose only job is to fail when someone tunes a number. New
  values need a new version tag in the salt, not an edit.

- **The passphrase is never an argument and never an env var.** It is read from the terminal
  without echo — or, when stdin is a pipe, as one line, which is how tests drive it — then
  derived and dropped. A flag would put the fleet's root secret in a shell history on the one
  machine where that matters most, so `--passphrase` is a *mode*, not a value.

- **A rekey's refusal reaches the evicted device and nothing reads it.**

  **How it was found.** `offload verify` came off `docs/DEMO.md`'s mention-count loop, third
  session running. It is **sound** — six cases: the right phrase, a wrong one, on a founder, on a
  device that was *invited* and never saw a phrase, and on both sides of a rekey. The wrong-phrase
  error is better than it needed to be:

  ```
  Error: that is not this fleet's passphrase — it derives 7ec07847, this node belongs to 7289c74e
  ```

  Naming both ids is what separates "you typed it wrong" from "you are holding the previous
  fleet's secret", and after a rekey the second is the likely one. Nothing to fix. The finding is
  one command over, and it came from staging the rekey **with the daemons running** — the record
  had rekey walked only as a CLI operation.

  **The measurement.** Two meshed daemons, `offload rekey` typed on alpha:

  ```
  06:03:19  alpha  WARN  this fleet has been re-founded under a new key — … this node is alone
  06:03:19  bravo  INFO  no answer: suspecting node=cf869d98
  06:03:24  bravo  INFO  no answer from anybody: marking dead node=cf869d98
  06:03:25  bravo  WARN  could not reach seed … error=refused by cf869d98:
                         that certificate is for fleet bf36a64a, this is fleet 433de62b
  ```

  The mechanism is right — three seconds from the rekey to a fleet that no longer talks, in both
  directions, with no message needing to arrive. Every report bravo had was wrong:

  ```
  $ offload nodes    # bravo
   cf869d9847d6  alpha   dead

  $ offload status   # bravo
  fleet       bf36a64a  ·  2 member(s) met, 1 approver(s)  ·  …
  note: only one approver (alpha) — losing it means the next enrolment costs a rekey.
  ```

  Alpha is not dead; it answered, with a diagnosis. Bravo's fleet does not have two members. And
  the one note warns about losing an approver that has already gone — the event that just
  happened, in the future tense.

  **Why it cannot be believed.** ADR-0044 closed the sibling path: `Refusal::Revoked` carries the
  signed `Revocation`, the subject verifies it against the fleet key it already holds, and acts.
  `Refusal::WrongFleet` has nothing to carry. A re-founded fleet **revokes nobody** — a new fleet
  was founded and this device was not put in it — so no credential exists that could make the
  claim checkable. A node that stood down on it is a node any peer could switch off by saying so.

  **What is reportable** is the half that is this node's own: it dialled, it reached something, and
  it was turned away, which is a different fact from `no answer`. `Turnaways` mirrors
  `sends::SendRefusals` — total, capped breakdown, the peer's words unreworded, no decision — and
  ADR-0060 records the two shapes that were refused.

  **The general rule.** When a mechanism is designed so that *nothing has to arrive*, the thing
  that does arrive anyway is the only evidence there will ever be, and it is worth asking what
  reads it. Here the answer was a `WARN` line, reprinted every twenty seconds, on the one machine
  whose owner most needed the sentence.

- **…and both ends of a rekey are turned away by the other, in identical words.**

  **How it was found.** By running the control arm, which is the only reason it was found at all.
  The evicted device's report came out right on the first try; the same command on the machine
  that had *done* the rekey printed:

  ```
  turned away 61 handshake(s) refused by a peer — this node reached it and was not let in.
              └─ a2c845f2  ×61  ·  that certificate is for fleet 1baca1e5, this is fleet 66b19bab
  note: a peer refused this node's certificate, saying the fleet has moved on. If you ran
        `offload rekey`, this device needs the invitation that printed — `offload join --token …`.
  ```

  The count is factually right: bravo really is refusing alpha, because alpha's certificate is for
  a fleet bravo has never heard of. The advice is exactly backwards — alpha *issued* that
  invitation.

  **Why the refusal cannot settle it.** `WrongFleet { expected, found }` renders as *"that
  certificate is for fleet {found}, this is fleet {expected}"*, where `expected` is the refuser's
  fleet and `found` is ours. Both sides see their own fleet in `found` and the other's in
  `expected`. The message is symmetric because the situation is.

  **The discriminator** is `FleetState::last_rekeyed()` — a `PassphraseUse::Rekey` in this
  device's own record. It is this device's own action rather than anything a peer said, which is
  the only kind of evidence admissible about oneself here, and it splits the note in two:

  ```
  # evicted
  note: a peer refused this node's certificate, saying the fleet has moved on. If someone ran
        `offload rekey`, this device needs the invitation that printed — `offload join --token …`.
        This node cannot check that claim and has not acted on it.
  # evicting
  note: a device is still dialling this node with a certificate for the fleet this one replaced.
        That is `offload rekey` working — it is left out until it takes up the invitation that
        printed. `offload invite <node>` prints another.
  ```

  Only the first carries the disclaimer, and that is the point: the second is this node describing
  its own deliberate act, where there is no claim to disclaim.

  **The general rule.** A situation with two ends produces one message, and the message is
  symmetric precisely when the advice is not. The control arm is the whole of how you find that —
  the defective half looked correct in isolation and was measured as correct on the other machine
  ten minutes earlier.

- **One fact, two dedup rules, and the stricter one guarded the path nobody types.**

  Four `offload revoke` calls naming one device, on two meshed daemons, then each node's screen:

  ```
  alpha$ offload status | grep revoked
  revoked     5 device(s)
  bravo$ offload status | grep revoked
  revoked     2 device(s)
  ```

  Two devices had been revoked. Bravo is right; alpha — the machine the commands were typed on —
  is counting records. `offload fleet` on alpha printed the same id four times in a row.

  `FleetState::revoke` mints `Revocation::issue(..., now, now.0)`, putting the clock in the
  serial, and `FleetState::file` deduplicated on `(member, serial)`. So the guard could fire for
  a *relayed* copy (same object, same serial — which is what it was written for, and what its
  test `filing_the_same_revocation_twice_records_it_once` exercises) and could never fire for a
  second local call. One layer up, `NodeMembership::file` — the path a revocation heard from a
  peer arrives on — has always early-returned on `is_revoked(revocation.member)`. That is why
  bravo stayed at 2 and why the duplicates never spread: the daemon's copy of the rule was the
  correct one, and the CLI's was not.

  `Revocation::covers` is `self.member == node` and ignores the serial, so nothing anywhere
  decided differently — the whole cost was a record that gossiped every round for ever and a
  count of *records* wearing the word `device(s)`. Deduped by member now, which is the question
  `covers` answers and therefore the only one anything downstream asks. The record kept is the
  **first**, for the reason a tombstone keeps the earlier instant (ADR-0056): `revoked_at` is
  when the device stopped being a member, and the earlier answer is the one every node that
  already heard it holds.

  The test that passed for the wrong reason is worth naming, because its name reads as though it
  covered both cases: it files one `Revocation` **object** twice. There is a second test now for
  the other call, with a later clock, which is the field the old dedup compared.

- **…and a revocation may be idempotent; it may not claim the act twice.**

  ```
  $ offload revoke fa657b1831ce          # the second time
  Revoked fa657b1831ce8d5e0285338fc182b7811e2c854b71ab7f8bfb408018d2497941.

    Recorded here and refused immediately: a daemon running on this device picks
    it up within a second and hangs up on that node. Other machines learn of it by
    gossip, or when that device's certificate lapses.
  ```

  Nothing was recorded and nothing was sent. `Revoked::{Now, Already}` — session seventy-four's
  `Removal::{Done, Already, Missing}` at the membership tier, with no `Missing` because a device
  this fleet has never met is a legal thing to revoke. Still exit 0: the device is out, which is
  what was typed, and failing a retry of an idempotent command is its own wrong answer. The
  `Already` arm names how long ago the revocation in force was filed, and says plainly that
  nothing was written. The passphrase-use record is not spent either — `offload nodes --history`
  promises every *use* of the phrase, and a call that wrote nothing used it only to find out.

- **The command that evicts a device asked for an id nothing prints.**

  `offload revoke` and `offload rekey --evict` both parsed 64 hex characters. The two commands
  that list peers print twelve:

  ```
  NODE           NAME         STATUS
  *39d8387d6caa  alpha        alive
   fa657b1831ce  bravo        alive
  ```

  Measured across `nodes`, `nodes --history`, `status` and `fleet` on the revoking node: bravo's
  id did not appear in full anywhere until after it had been revoked, when it turned up in the
  `revoked` list. The two sources that did have it were `offload id` **on bravo** and bravo's
  `fleet.json`. So revoking a device meant going to it — and the device you cannot go to is the
  one the command exists for.

  `resolve_member` now accepts a prefix, matched against what this machine *witnessed*: the
  enrolment log `offload nodes --history` reads, plus this node itself and anybody already
  revoked. A full 64 characters bypasses the lookup entirely, so a device this machine has never
  met is still nameable from an id read off its own screen. No match and more than one match are
  both refused, and the ambiguous refusal names the candidates in full — which is the rule it is
  an instance of. Walked: twelve characters straight off `offload nodes` resolve and are echoed
  in full **before** the passphrase prompt, so the operator sees which device while there is
  still nothing to undo; `75` against two devices starting `75` refuses with both ids and both
  names; `abcdef` against neither refuses and points at `offload nodes --history`; `bravo`, ``,
  `75%`, `75_` and seventy zeroes are all refused before anything is consulted; uppercase
  resolves. The store-read is the only part outside the tested function, on purpose.

  The no-match refusal has two spellings, and the distinction is the one this file keeps making:
  *nothing matched* and *there was nothing to match against* are different things to tell
  somebody, and a resolver that conflated them would send them hunting for a typo in an id that
  was right.

- **…and revoking a device this fleet has never met is legal, so it gets a sentence.**

  It was accepted in silence, which is what a mistyped id also looks like — and nothing here can
  tell the two apart, which is exactly the thing worth saying out loud. Same predicate `offload
  when` has warned on since ADR-0032, and that session seventy-three found `offload every`
  accepting without a word. Found by walking the arm: the first version of the check asked
  `name_of` *after* filing the revocation, and `witnessed` reads the revocation list — so a
  stranger revoked a moment earlier had a name and the sentence never printed.

- **A revoked device was told its fleet was fine, one line under the only honest sentence.**

  ```
  accepting   no — this node has been revoked from its fleet — it can host nothing …
  records     0
  fleet       1a5835d0  ·  2 member(s) met, 1 approver(s)  ·  this node's certificate lasts 30 more days
  revoked     1 device(s)

  note: only one approver (alpha) — losing it means the next enrolment costs a rekey.
        Grant `approve` to a second device.
  ```

  Three things wrong under one right one. The certificate does not last thirty days — `offload
  fleet` on the same device says `cert UNUSABLE: … has been revoked`, one command away. The
  `revoked 1 device(s)` is *this* device and does not say so. And the note is advice a revoked
  device may not take, naming as the approver-to-protect the very machine that evicted it.

  This is ADR-0060's finding at the sibling command: that ADR gave the report the **rekey** half
  of the question, and the revoke half was never walked. The difference is the evidence, and it
  is why this is a `bool` where ADR-0060's is a count: a rekey revokes nobody and its refusal
  carries no signature, so a device may only *report* it; a revocation is signed by the fleet key
  and verified here, so it is one of the few claims about itself a node may act on — and
  `Supervisor` already did, which is precisely what made the `accepting` line honest while
  everything below it was not. `FleetHealth::revoked_here`, control socket and `#[serde(default)]`
  like the five fields before it, so a daemon too old to send it renders the old sentence rather
  than a wrong one.

  `revoked_here(state)` is a named function rather than four spellings of
  `state.is_revoked(state.membership.member)`, for the reason `steward_note` was extracted: the
  fourth spelling is where one of them quietly answers a slightly different question.

- **…and a probation countdown under an unusable certificate promises a wait that has no end.**

  ```
  cert     UNUSABLE: this node's certificate is not usable: member 752e6baa has been revoked
           host-runs dormant for another 14 minutes (probation of 15 minutes)
  ```

  Host-runs is not coming back in fourteen minutes. `offload fleet` matched on `state.check(...)`
  to print the `cert` line and then asked `probation_until` on the next statement, unconditionally
  — an `if` that was never told what the line above it had concluded. Same shape as the removed
  schedule whose `home` line still said `fired from here right now`, and the same fix: the
  countdown is printed only under a certificate that is going to start working. Walked both ways
  in one pass — bravo before the revocation showed the countdown under a valid certificate, and
  after it showed neither.

- **An improvement was measured on one of the certificate's two axes.**

  `FleetState::adopt` is the one funnel two paths reach: `offload join --token` when the device is
  already a member, and the renewal loop in `mesh.rs`, which calls it with whatever an approver peer
  answered `RenewMe` with. It decided like this, and answered a `bool`:

  ```rust
  if cert.expires_at <= self.membership.expires_at {
      return Ok(false);
  }
  ```

  The doc comment above it said "Refuses anything that is not an improvement", and the test beside
  it is named `a_renewal_is_taken_up_only_if_it_is_ours_and_an_improvement`. Both were true of the
  axis the test varied, and the test only ever varied expiry — a renewal restates grants, so the
  grants axis never moved in any test in the file. A certificate that expired later and granted
  **less** was an improvement by the only measure anything took.

  Measured on two state directories, `bravo` holding `HostRuns`. The invitation costs no
  passphrase, because `default_grants()` is exactly what an approver's delegation may issue:

  ```
  $ offload invite 580993bf… --name bravo         # on alpha — no prompt, exit 0
  $ offload join --token offload-invite-1.…       # on bravo
  Took up a new certificate in fleet 21e2074c….
    grants      submit, deliver
  ```

  Nine seconds earlier, taking up the certificate that *carried* `HostRuns`, the same command had
  printed:

  ```
  Took up a new certificate in fleet 21e2074c….
    grants      submit, deliver
    host-runs   dormant for another 14 minutes (probation)
  ```

  The `grants` line is identical in both. `grant_list` renders `state.grants(at)` — what is in
  force — and probation suppresses `HostRuns`, so the grant never appeared on that line in either
  state; it had its own line, and the only difference on screen is that line's absence, which reads
  as probation having elapsed. `offload fleet` a moment later was the control arm, and it is what
  made the loss visible: `host-runs dormant for another 14 minutes` before, gone after.

  The founder form is the one to remember, because it needs one device and two commands:

  ```
  $ offload invite <its own id>                    # accepted, no prompt
  $ offload join --token …
  Took up a new certificate in fleet 21e2074c….
    grants      submit, deliver
  ```

  Alpha went from `submit, deliver, host-runs, approve` to `submit, deliver`. It was the fleet's
  only approver, so the fleet then had none, and making another needs the passphrase. It also lost
  its **name** — `alpha` became `448dd027` — because `invite` defaults `--name` to a short form of
  the id, and `adopt` takes the whole certificate.

  The fix is ADR-0062: grants are monotonic under adoption, compared before the clock is consulted,
  on the **certificates' own** `grants` rather than `grants(now)`. That last detail is not
  cosmetic — comparing what is in force would see a certificate carrying `HostRuns` under probation
  as dropping `HostRuns`, and refuse every invitation that grants it, which is the exact case the
  decision exists to protect. `Adopted::{Taken, NoBetter, Narrower { lost }}` replaces the `bool`,
  which is session seventy-five's `RepoReach` fix in a second file and for the same reason: the
  refusal had to word a sentence out of a measurement the `bool` had already thrown away.

- **…and a bound stated in one direction is not a bound.**

  ADR-0012 mitigation 1 is enforced and tested: `FleetState::invite` issues `default_grants()` and
  `HostRuns` needs `issue_for` and the root. What it bounds is *minting*. Stripping was never
  considered, and it is the same door: an approver that cannot grant the right to host could remove
  it, from any member, over the network, via the renewal loop that takes an approver's answer with
  nobody watching. The `warn` line there now names what was left out and takes nothing up.

  The reusable question is the one this took two readings to ask: when an ADR bounds what a
  delegation may **do**, ask separately what it may **undo**.

- **…and the widening test must not be the clock.**

  The first cut of ADR-0062 had no arm for a wider certificate, and a comment explaining why it
  needed none: every issuing path stamps `now`, so a certificate carrying a grant this node lacks
  was minted later than what it holds and therefore expires later, and the existing expiry test
  takes it. That is true on one machine. The issuer is a *different* device — the whole point of
  `offload invite` — and a few seconds of clock skew behind this node makes a deliberately widened
  certificate expire earlier than the held one, which the expiry test refuses as `NoBetter`:
  somebody types the passphrase to grant `host-runs` and is told the certificate buys nothing.

  Found by writing the test, which issued both certificates at `NOW` and so hit the equality case
  immediately. The comment had been written confidently enough that reading it again would not have
  caught it — a superset with more in it is `Taken` whatever its clock says, and the reasoning that
  said otherwise is kept in the ADR because it is the plausible kind of wrong.

- **The `approver` line read the delegation; everything that enforces it reads the grant.**

  `FleetState` holds both halves: `approver: Option<ApproverDelegation>`, the fleet-signed
  delegation, and `membership.grants`, which may contain `Grant::Approve`. `FleetState::invite`
  asks for them in that order — grant first, then the delegation to prove it — and `health`'s note
  and `offload status` count the grant. `offload fleet` asked `state.approver.is_some()` alone.

  On a founder whose certificate had been narrowed, one screen said both:

  ```
  grants   submit, deliver
  approver this node may enrol others
  ...
  note: this device is not an approver. Whether any other is, this command cannot see …
  ```

  and `offload invite` refused with *"this node is not an approver — `offload grant approve`
  first"*. The note and the door were right; the line that reads like the answer was wrong. It is
  three-valued now and mirrors the door, including the state the door already distinguishes —
  granted with no delegation to prove it.

  Worth keeping even though ADR-0062 makes the desync unreachable: the rule is that a report comes
  from where the decision reads, and holding two fields in agreement by arranging for nothing to
  ever separate them is not the same as reading the one that decides.

- **The sole approver cannot renew itself, and said so by naming the wrong cause.**

  `Mesh::renew_if_due` builds its candidate list as every node in the view that is not `me` and not
  `Dead`. Excluding `me` is the right call and is worth stating, because it is not obvious: a node
  that could re-sign its own membership would never fall out of a fleet it had been removed from,
  and "certificates lapse unless somebody else renews them" is the backstop ADR-0012 relies on
  twice — for a revocation that never arrives, and for a device that is simply lost.

  The consequence is that the **only approver in a fleet is the one device nothing can renew**,
  which is the default two-device fleet: `offload init` makes the founder the only approver. After
  thirty days its certificate lapses, and the way back is the passphrase.

  Measured in session seventy-eight with `CERT_LIFETIME` shortened to forty seconds. On the
  founder:

  ```
  WARN this node's membership certificate is running out and no peer would renew it — a fleet
       with no approver needs `offload grant approve` on a device that has one
  ```

  and one command away, on the same device:

  ```
  approver this node may enrol others
  ```

  The reader who believes the warning goes looking for a fleet-wide problem that does not exist;
  the reader who believes `offload fleet` concludes the warning is broken. Both are reading a
  screen that disagrees with itself, and the *mechanism underneath is correct* — which is the
  pattern this subsystem keeps producing.

  Split on `state.membership.granted(Grant::Approve, now)` now, in `mesh`'s warning and in the
  `offload status` note beside it, which had the same sentence in a softer form (*"a fleet with no
  approver reachable cannot renew"*). The advice was always right — grant `approve` to a second
  device — and only the diagnosis was wrong, which is the more dangerous half: advice that arrives
  with a false explanation gets argued with.

- **…and the loop measured a reason per peer and threw them all away.**

  The same warning fired at the bottom of the peer loop with a fixed string, while every arm above
  it had a specific answer: `renew_with`'s `Err(reason)` (*"this node is not an approver"*),
  `adopt`'s `NoBetter`, `adopt`'s `Narrower { lost }`, and `adopt`'s verification errors. Three of
  those four logged at `debug`, so at the default level the only thing on screen was the guess.

  The sharpest measurement was on the *joiner*, with an approver deliberately patched to answer
  renewals with a narrower certificate:

  ```
  WARN an approver offered a certificate that takes grants away; not taken up
       node=802a2bcd lost=host-runs
  WARN this node's membership certificate is running out and no peer would renew it — a fleet
       with no approver needs `offload grant approve` on a device that has one
  ```

  Two lines, 49 microseconds apart, the second contradicting the first: the fleet had an approver,
  it was reachable, it had answered, and the answer had been refused by the line above. The summary
  now carries `refused=<peer>: <reason>` for every peer it tried.

  The rule is one this tree already has for decisions, applied one level out: *decisions carry
  reasons* is usually read as being about `Hold { until, reason }` and `NoBid::Refused(..)`. It
  applies just as much to the line that reports a decision **failing**, and that line is the one
  nobody writes a type for.

- **`Millis`'s `Display` is a duration, and two absolute timestamps went through it.**

  `time.rs` formats a `Millis` as `2h15m` / `1m30s` / `450ms`, and its own comment says why: "
  deadlines are the reason this branch exists". `MembershipError::Expired` carried two `Millis`
  fields that are *instants*, and interpolated both:

  ```
  cert UNUSABLE: this node's certificate is not usable: expired at 496980h44m, now 496980h44m
  ```

  496980 hours is the Unix epoch rendered as an elapsed span — about 56.7 years — so both numbers
  are the same to the minute and the difference that matters, how long ago it lapsed, is invisible.
  The message conveys nothing except that something is wrong, and it is the one message that exists
  for the moment a certificate lapses, which is the ADR-0012 backstop working as designed: the
  laptop that has been shut for a month.

  `#[error("expired {} ago", now.saturating_sub(*expired_at))]` now — `expired 25.1s ago`, measured
  on a node whose certificate was allowed to lapse. The elapsed span is a genuine duration, which
  is what the formatter is for, and `offload-core` has no clock so a wall-clock date was never
  available here anyway.

  The reusable form: **a type that holds milliseconds does not say whether it is an instant or a
  span, and the formatter assumes span.** Grep the `#[error]` and `format!` sites before adding a
  `Millis` field to a message. The sweep for this one found a single site, which is the good case.

- **`as_secs() / 60` renders the last minute of every probation as `0 minutes`.**

  Five sites computed the countdown as `until.saturating_sub(at).as_secs() / 60` — four in
  `offload-cli/src/fleet.rs` (`join --token` on a new member and on an existing one, `offload
  grant`, and `offload fleet`) and one in `mesh.rs`, which is the per-node line `offload explain`
  prints:

  ```
  bravo        host-runs granted but on probation for another 0 minutes
  ```

  That is the line somebody reads when they are asking why a run has not been placed, and "another
  0 minutes" says the wait is over. It is not: there can be fifty-nine seconds left. Found with
  `PROBATION` shortened for a walk, where it is the *only* thing the line ever says; at fifteen
  minutes it is the last minute of every probation.

  `offload_core::fleet::dormant_for` now, with one test: `less than another minute` / `another
  minute` / `another 14 minutes`. Five call sites for one rule is the shape session seventy-seven
  named — **grep for every site that renders a rule before fixing the one you saw** — met here
  before writing the fix rather than a session later.

- **What a node may do changes without its file changing, and a file diff cannot report it.**

  Found session ninety-one, from the handoff's note that *"the daemon says nothing when it adopts
  a certificate"*. The line `this node's grants changed` existed; it fired from
  `FleetChange::grants`, which `reload` computed as `fresh.grants(now) != state.grants(now)` — the
  file against the copy in memory, both through `effective_grants`. Two changes never reach it:

  1. **Probation lifting.** `effective_grants` drops `HostRuns` while `now < issued_at +
     PROBATION`. At probation's end the file is untouched, so `reload` returns early on
     `*state == fresh`, and the grant that went into force was never announced. At the moment of
     the grant itself both sides are still probating, so that write reports nothing either.
  2. **`NodeMembership::adopt`**, which saves and then calls `self.reload()` and drops what it
     returns — so the membership tick a second later sees an unchanged file.

  Measured with `PROBATION` at 20s, one daemon (bravo, joined by passphrase), `offload grant
  host-runs` three seconds after start. Fixed build: `member of fleet … grants=submit,deliver`
  at 15:45:24, `this node's grants changed grants=submit,deliver,host-runs` at 15:45:47, twenty
  seconds after the grant. Control (the two files checked out from HEAD, same staging): the
  startup line and nothing else, over 28 seconds.

  The unit test `a_grant_typed_at_the_cli_reaches_a_running_daemon` had asserted
  `change.grants` contained `HostRuns` at the fixture's `NOW` — and passed, because `reload`
  read `SystemClock` while the certificate was issued at the fixture's `NOW`, years earlier, so
  the re-probated grant had long lifted by the wall clock. **A fixture's clock and the code's
  clock disagreeing is a test of nothing.** `grants_changed` takes `now`, and the test now checks
  both sides of the lift.

*Backfilled in session ninety-one from `docs/sessions.md` and the commit that added each rule —
sourced, not reconstructed from the rule text.*

- **…and the operational consequence is that a lost passphrase is a fleet that can never host
  again.**

  Commit `45c6ef2`, whose body is the only record (no `docs/sessions.md` paragraph). A runbook for
  a three-device walk said `offload invite --grant host-runs` from an approver, which this file had
  already ruled out and `CLAUDE.md`'s index maps to `offload-cli/src/fleet.rs`; it was not opened —
  the third time this file recorded somebody re-deriving a mistake it already covered. So the rule
  now states the consequence that made it matter: a fleet whose phrase nobody wrote down can add
  observers for ever and never another host, and `offload fleet` saying `verified never` is the
  warning about precisely that. The runbook was deleted in the same commit.

- **A wrong passphrase is recorded nowhere, and that is right.**

  Session seventy-two (commit `b2527b4`, ADR-0060), written down so nobody builds it. The key is
  derived locally, so there is no remote oracle: a failed attempt is somebody with a shell on the
  machine, who already has everything the log would warn about and can delete it. What a log of
  typos would buy is noise in the one place ADR-0012 needs read.

- **The second renewal over one connection always failed, and it was older than anyone noticed.**
  ADR-0069's step 2 walk shortened `CERT_LIFETIME` to four minutes so renewal could be watched. The
  first cycle worked on both nodes, and every later one logged `no peer would renew it` with
  `refused=<peer>: that certificate has already lapsed`, about certificates with a minute left. The
  renewer answers `RenewMe` from `who.membership`, the certificate the handshake authenticated,
  deliberately, so the asker could assert nothing. But the connection had been up since before the
  first renewal, and a live QUIC session never handshakes again, so the renewer was restating the
  certificate the asker had *replaced*, which had since expired. The approver path had done exactly
  this since renewal was built. On a real fleet it needs a connection outliving a certificate,
  which an always-on desktop pair easily manages in 30 days: renewed once, then lapsed with nothing
  to warn of it but a `WARN` per retry. The fix carries the asker's current credentials in
  `RenewMe`, takes them only for the asker, and verifies them in `renew_for` before restating.
  That verify step is newly load-bearing, since restating unchecked papers from a message would be
  minting. The general form is in this file already ("a live session never handshakes again");
  it was met here from the renewal side, where it had not been looked for.
- **A running daemon picks up every membership change except its first fleet.** Walked in the iOS
  Simulator. The app's daemon started with no fleet. `offload join --state-dir … --token …`
  succeeded, and twenty seconds later both it and the Mac peer (whose `init` came after its start)
  answered `nodes` with "This node is not in a mesh. `offload init` founds a fleet here…". That
  sends somebody to repeat what they just did. `daemon::run` builds the mesh only when
  `fleet::load` finds a fleet at startup. `watch_membership`, which picks up grants, renewals and
  revocations within a second, is spawned only inside that branch. After a restart (for the app,
  background then foreground) both meshed within a second. `init`, `join` (both paths) and `nodes`
  now say to restart. Doing it in place is listed in HANDOFF as wanting an ADR.
- **Delegations last a year from `init`, and nothing re-mints them.** Found writing
  `a_decision_on_an_approver_re_approves_a_member_when_it_next_asks`. It checks the re-approved
  host on day 366, and got `Expired { expired_at: day 365 }`: the founder's delegation
  (`DELEGATION_LIFETIME`, issued by `FleetState::found`) had expired, not the certificate. The
  approver branch of `MembershipCert::verify` calls `delegation.verify(fleet, now)`, and
  `an_expired_delegation_invalidates_what_it_signed` states that rule on purpose. The only
  `Delegation::issue` outside tests is in `found`. The test now gives the founder a two-year
  delegation and says why. The decision is the owner's, and ADR-0069's step-3 amendment lays out
  the two options.
- **When the approval is the cause, the renewal warning must say so.** In the same walk, the
  founder's approval lapsed at minute 12 because nobody else could re-approve it. `renew_if_due`
  then warned every 20 s: "certificate is running out… a second device needs `offload grant
  approve`". Its peers were refusing for the approval, not the certificate. Fixed in `renew_if_due`.
