# Membership, certificates and credentials

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/membership-and-credentials.md`, same order.

- **A node may refute `Suspect`; it must never refute `Revoked`.** Same gossip path, same-looking
  fact, opposite rule — `incarnation` reaching membership talks a revoked device back in.
- **A membership command runs with no daemon, so the daemon has to look again.** Every one writes
  `fleet.json` from a process that is not `offloadd`. The whole file is re-parsed each tick, not
  compared by timestamp: two writes inside one filesystem timestamp is how a device is set up.
- **Membership is checked at the handshake, and a live session never handshakes again.** Anything
  changing who may talk to whom closes connections in the same breath.
- **…and "the connections" means the ones the *peer* opened as well.** A hang-up that walks the
  map of sessions this node dialled reaches the peer that is going to redial anyway, and misses
  the revoked one — which is by definition the device still trying. Measured: a revoked node
  went on gossiping, renewing its lease and pushing checkpoints for the rest of its run, and the
  arbiter flapped `suspecting` / `no longer suspect` once a second for ever, so the run was never
  moved. An inbound session lives in the task serving it; if it is in no map, `disconnect` is a
  log line.
- **The handler was on the path that cannot happen, and the path that happens had no handler.**
  A revoked node's own eviction cannot reach it by gossip — the revocation is what ends the
  channel — and it *does* reach it as `Refusal::Revoked` on every dial, which was matched nowhere.
  Measured: it ran 48 turns after being thrown out and said `accepting yes` throughout; now it
  stops 2.4s after (ADR-0044). Three rules fell out.
- **…a refusal is a peer's claim about this node; the signature inside it is not.** So
  `Refusal::Revoked` carries the `Revocation` (wire v25, required) and it is filed through
  `Members::revoked` — the same verify a gossiped one passes. Never act on the sentence.
- **…a fact that latches must not be reported as an edge.** `FleetChange::ourselves` was consumed
  by `NodeMembership::file`'s own reload, so the one announcement was eaten by the writer.
  `revoked_here()` is a standing question. Revocation is monotonic; ask, do not be told.
- **…and a certificate that still verifies is not permission.** `FleetState::grants` read the
  certificate alone, so `status`, the bid, the grant and `run` all stayed open for a revoked
  device. Nothing granted once revoked, in one place.
- **A certificate has a month on it, and a backstop nobody renews is a deadline.** Renewal is
  *asked for*, never offered — the lapsing node is the one that knows. `RenewMe` carries nothing;
  what is re-issued is the certificate the handshake authenticated.
- **Probation follows the grant, not the path.** It only suppresses `HostRuns`, so "an invite skips
  probation" is a statement about the *door*, where the grant is not in play.
- **…and the flag is derived, not carried forward.** A certificate probates exactly when it puts
  `HostRuns` in play that is not already in force — so an unrelated re-issue never suspends hosting
  a node already had, and a grant still being served restarts the window rather than clearing it.
- **A rekey must not re-invite a revoked device.** `known_members` reads what this machine
  *witnessed* and has no opinion about what happened next. What is left out is named in the output.
- **An invitation is not a bearer token.** It is a certificate naming the joining device's key:
  useless to anybody else, so it needs no expiry and can be pasted into a chat window.
- **A device cannot tell its own fleet moving from somebody else's fleet taking it.** `offload
  rekey` carries a `Succession` signed by the key being replaced. Check both ends: a valid
  succession out of some *other* fleet says nothing about this one.
- **`NodeView::absences` is an observation, so it does not travel** (ADR-0031). Gossiped, a peer
  learned indirectly inherited the relayer's history wholesale while one learned directly started
  at zero. Durable instead, and seeded idempotently so a live observation beats a remembered one.
- **A gossiped fact needs an owner; an observation does not have one.** Two nodes recording one
  enrolment are not disagreeing. The fleet log is per-node, and `offload nodes --history` says so.
  Sometimes the answer to "who owns this field" is that it should not be a gossiped field.
- **An alarm nobody can act on is not a control.** Probation's fifteen minutes buy nothing without
  the announcement, which has two producers on purpose: the CLI (these commands need no daemon) and
  every *other* node meeting an unfamiliar member.
- **A grant is enforced where it is used, against the certificate as it stands right now.** The
  handshake admits a member; the *bid exchange* refuses one without `HostRuns`. `Peer` carries the
  certificate rather than a snapshot, and an objection drops the connection.
- **A delegation bounds who may issue, and had to be made to bound what.** An approver may not mint
  `HostRuns`, but **renewal restates grants** — so a renewal carries the fleet-signed certificate
  it descends from (`MembershipCert::authority`, the chain's *root*) and may grant no more.
- **…and the operational consequence is that a lost passphrase is a fleet that can never host
  again.** `offload invite <node>` needs no phrase, which reads as "an approver can enrol anybody"
  — but `--grant host-runs` (and `--grant approve`) go to the root, so a fleet whose phrase nobody
  wrote down can add *observers* for ever and never another host. `offload fleet` says
  `verified never` on exactly those fleets, and that line is the warning. Met while driving a
  three-device walk: the entry above was already in this file, indexed from CLAUDE.md against the
  very file that implements it, and a runbook still went out saying `invite --grant host-runs`
  works without the phrase. **Reading the pitfall file is not optional because you think you
  remember the rule** — the third time that has been recorded here.
- **A credential's signed message is a wire format, and only a pinned digest says so.** Add a field
  to `MembershipCert::message` and every certificate stops verifying, with one symptom: every
  device drops out at once. A deliberate change updates the digests **and** says in its commit
  message that every device re-joins.
- **A membership certificate is not a bearer token.** What makes it non-transferable is proving
  possession of the key it names, so `handshake::admit` takes *claimed* and *authenticated*
  separately. Pass the claim for both and every check below becomes decorative.
- **The fleet key derivation is a wire format.** Salt, argon2id parameters, wordlist and
  normalisation all feed the same 32 bytes. New values need a new version tag in the salt.
- **The passphrase is never an argument and never an env var.** Read from the terminal without
  echo, or one line from a pipe. `--passphrase` is a *mode*, not a value.
- **A rekey's refusal reaches the evicted device and nothing reads it.** `offload rekey` evicts by
  re-founding the fleet without a device, so nothing has to arrive — and nothing does, except
  `Refusal::WrongFleet` on that device's own next dial, which names both fleets and is the entire
  diagnosis. It went to a `WARN` line every twenty seconds. Measured on two meshed daemons: three
  seconds after the rekey each called the other **`dead`**, and the evicted node's `offload status`
  said `2 member(s) met` with its only note warning about *losing* the approver that had already
  gone. Counted now (ADR-0060) and reported beside the `fleet` line — never acted on, because
  unlike `Refusal::Revoked` there is no signature: a re-founded fleet revokes nobody, so nothing
  could make the claim checkable, and a device that stood down on an uncheckable claim is one any
  peer could switch off.
- **…and both ends of a rekey are turned away by the other, in identical words.** The evicted
  device holds a certificate for the old fleet; the rekeying device holds one the evicted device
  has never heard of. So the refusal cannot say which end you are on, and the first cut of the note
  told the machine that had *issued* the invitation to go and take one up. The discriminator is
  `FleetState::last_rekeyed` — **this device's own recorded action**, which is the only evidence
  about itself a node here may reason from. Caught by running the control arm on the other daemon,
  not by reading.
- **`offload verify` is sound, and the walk is worth not repeating.** Session seventy-one put six
  cases through it: right phrase, wrong phrase, on an invited device that never saw one, and on
  both sides of a rekey. The wrong-phrase error names *both* fleet ids
  (`it derives 7ec07847, this node belongs to 7289c74e`), which is what tells somebody they are
  holding the previous fleet's secret rather than a typo. `offload fleet`'s three-way
  `verified never | n/a | N days ago` and `offload status`'s nag agree, and each has a test.
- **A wrong passphrase is recorded nowhere, and that is right.** `offload nodes --history` promises
  "every use of the passphrase" and a failed attempt is not in it — deliberately, and worth writing
  down so nobody builds it. There is no remote oracle here: the key is derived locally, so a failed
  attempt is somebody with a shell on the machine, who already has everything the log would warn
  about and can delete it. What a log of typos would buy is noise in the one place ADR-0012 needs
  read.
- **One fact, two dedup rules, and the stricter one guarded the path nobody types.**
  `FleetState::file` deduplicated on `(member, serial)` — right for the case it was written for, a
  peer relaying a copy we already hold — and `revoke` mints the serial from the clock, so a second
  `offload revoke` of one device could never hit it. `NodeMembership::file`, one layer up on the
  *gossip* path, has always asked `is_revoked(member)`. Measured on two meshed daemons: the node
  the command was typed on said **`revoked 5 device(s)`** about two devices while the node that
  merely heard it said **2**, and the one that heard it was right. `covers()` ignores the serial,
  so nothing decided differently — what the duplicates did was gossip every round and make a
  *count of records* wear the word `device(s)`. Deduped by member now, keeping the **first**
  record for a tombstone's reason (ADR-0056): `revoked_at` is when the device stopped being a
  member, and the earlier answer is the one the fleet already holds.
- **…and a revocation may be idempotent; it may not claim the act twice.** The second
  `offload revoke` reprinted the whole paragraph about what had just been recorded and what the
  daemon was about to hang up on, having written and sent nothing. `Revoked::{Now, Already}` —
  seventy-four's `Removal` rule at the membership tier, with no `Missing`, because a device this
  fleet has never met is a legal thing to revoke.
- **The command that evicts a device asked for an id nothing prints.** `offload revoke <node>` and
  `offload rekey --evict` took 64 hex characters; `offload nodes` and `offload nodes --history` —
  the two commands that list peers — print **twelve**. Measured: no command on the revoking node
  showed the other's id in full until it had already been revoked, so the only sources were
  `offload id` *on that device* and `fleet.json`. Revoking a device meant going to it, and the
  device you cannot go to is the one revocation exists for. A prefix now resolves against what
  this device **witnessed** (the enrolment log, plus itself and anyone already revoked), refusing
  no-match and ambiguity and naming the candidates in full; 64 characters still bypass the lookup
  entirely, so a device never met is still nameable. The rule is
  `reports-and-cli`'s — *an error that tells somebody to try harder must be satisfiable from the
  screen* — met from the other side: here it was the **argument** the screen could not supply.
- **…and revoking a device this fleet has never met is legal, so it gets a sentence rather than
  silence.** Nothing here can check such an id, which is exactly what a mistyped one looks like.
  Same predicate `offload when` has warned on since ADR-0032, and that `offload every` was found
  accepting in silence.
- **A revoked device was told its fleet was fine, one line under the only honest sentence on the
  screen.** ADR-0060 fixed the *rekey* half of this and the revoke half was never walked:
  `offload status` on a device just revoked read `2 member(s) met, 1 approver(s) · this node's
  certificate lasts 30 more days`, `revoked 1 device(s)` (the device being *this* one), and a note
  telling it to grant `approve` to a second device — advice it may not take, naming the machine
  that had just evicted it. `offload fleet` one command away said `cert UNUSABLE: … has been
  revoked`. Unlike ADR-0060's case there is nothing to disclaim: a revocation is signed by the
  fleet key and verified here, so it is one of the few claims about itself a node may act on —
  and `Supervisor` already did, which is what made the `accepting` line honest while everything
  under it was not. `FleetHealth::revoked_here` (control socket, `#[serde(default)]`).
- **…and a probation countdown under an unusable certificate promises a wait that has no end.**
  `offload fleet` printed `host-runs dormant for another 14 minutes` directly beneath
  `cert UNUSABLE: … has been revoked`. The `if` was on the line after the one that decided, which
  is the removed schedule's `home` line again — a line computed from one field has to be told what
  the line above it concluded.
- **An improvement was measured on one of the certificate's two axes.** `FleetState::adopt` refused
  anything that did not extend `expires_at` and never looked at the grants, so a later-expiring
  certificate that granted **less** was taken up — by `offload join --token` *and* by the renewal
  loop, which takes whatever an approver peer answers `RenewMe` with. Measured: `offload invite
  <a node already holding host-runs>` needs no passphrase, and the device that took that token up
  printed `grants submit, deliver`, which is **the same line it had printed while holding
  host-runs** — `grant_list` renders what is in force and probation renders host-runs separately,
  so the only difference on screen was a countdown disappearing, which reads as probation elapsing.
  The founder form takes the fleet's only approver to `submit, deliver` in two commands. Grants are
  monotonic under adoption now (ADR-0062), refused by name, and compared on the **certificates'
  own** grants rather than `grants(now)` — probation suppresses host-runs without removing it, so
  the in-force comparison would refuse every invitation that carries it.
- **…and a bound stated in one direction is not a bound.** ADR-0012 mitigation 1 says an approver
  may not *mint* `host-runs`; until ADR-0062 it could *strip* it, which is the same door from the
  other side. When an ADR bounds what a delegation may do, ask what it may **undo**.
- **…and the widening test must not be the clock.** The first cut argued a wider certificate always
  expires later, because every issuing path stamps `now`. True of one machine, false of two: the
  issuer is a different device, and clock skew behind this node makes a deliberately widened
  certificate expire *earlier* and be refused as buying nothing. A superset is taken whatever its
  clock says. Found by a test, not by reading — the staging that exposed it issued both
  certificates at the same instant.
- **The `approver` line read the delegation; everything that enforces it reads the grant.**
  `offload fleet` asked `state.approver.is_some()`, while `FleetState::invite`'s door, `health`'s
  note and `offload status` all ask `granted(Approve)`. They agreed only because nothing had ever
  desynchronised them — and a de-granted certificate does, which put `approver this node may enrol
  others` four lines above `note: this device is not an approver` on one screen, with the command
  refusing. Three-valued now, in the order the door asks: grant, then delegation.
- **The sole approver cannot renew itself, and said so by naming the wrong cause.** `renew_if_due`
  asks *peers* and never itself — deliberately, since a node that could re-sign its own membership
  would never fall out of a fleet it had been removed from, which is the backstop ADR-0012 leans on
  twice. So the only approver in a fleet is exactly the device that cannot be renewed, and the
  warning it printed was `a fleet with no approver needs `offload grant approve` on a device that
  has one` — said by the approver, with its own `offload fleet` one command away answering
  `approver this node may enrol others`. **The mechanism is right and only the sentence was wrong.**
  Split on `granted(Approve)` now, in the `mesh` warning and in `offload status`'s note.
- **…and the loop measured a reason per peer and threw them all away.** The same warning named one
  cause whatever had happened, while `renew_with` and `adopt` had just answered per peer. Measured
  on a joiner whose approver was reachable, had answered, and had been refused one line above
  (`lost=host-runs`): the summary still said the fleet had no approver. It carries `refused=<peer>:
  <reason>` now. *Decisions carry reasons* applies to the line that reports a decision failing, not
  only to the decision.
- **`Millis`'s `Display` is a duration, and two absolute timestamps went through it.**
  `MembershipError::Expired` read `expired at 496980h44m, now 496980h44m` — two identical-looking
  numbers, neither a time anybody recognises, the gap between them invisible at hour granularity.
  It is the one message that exists for the moment a certificate lapses, which is the backstop
  working. `expired 25.1s ago` now: the elapsed span is a real duration, which is what the
  formatter is for. Grep before adding a field of type `Millis` to a message — the type does not
  say whether it is an instant or a span, and the formatter assumes span.
- **`as_secs() / 60` renders the last minute of every probation as `0 minutes`**, which reads as
  *no wait at all* in the one sentence whose whole job is to say there is one. Five sites said it,
  including `offload explain`'s per-node line — where somebody is asking why a run has not been
  placed. One `offload_core::fleet::dormant_for`, five callers, and a test.
- **What a node may do changes without its file changing, and a file diff cannot report it.**
  `FleetChange::grants` compared `fleet.json` with the copy in memory, so a daemon joined under
  probation printed `grants=submit,deliver` at startup and **nothing** when `host-runs` came into
  force — the certificate is the same bytes, only the clock moved — and a certificate the daemon
  adopted itself was eaten by `adopt`'s own `reload`. The `ourselves` edge (ADR-0044) a second
  time. Compare against what was last *said*: `NodeMembership::grants_changed`, caller-owned.
  A fixture whose `reload` read the wall clock passed through the probation it was testing.
- **"Renew what the handshake authenticated" renews the certificate the connection opened with.**
  A live session never handshakes again, so the second renewal over a long-lived connection
  restated a certificate that had already been replaced and lapsed, and was refused as `already
  lapsed`. Latent since renewal was built, because a 30-day certificate outlives most sessions and
  a four-minute walk certificate does not. `RenewMe` carries the asker's current credentials,
  taken only for the asker, and `renew_for` verifies before it restates.
- **A running daemon picks up every membership change except its first fleet.** It decides at
  startup whether it has a mesh. One that started outside a fleet has nowhere to put a certificate
  that `init` or `join` writes later, while a grant, a renewal or a revocation arrives within a
  second. `nodes` went on telling the just-joined device to run `offload init`. `init`, `join` and
  `nodes` now say to restart. On an app host the restart is the app's stop and start (ADR-0070).
- **A delegation lasted a year from `init`, nothing re-minted it, and certificates were checked
  against it as of now.** A year after founding, every approver-issued certificate would have failed
  at once. The re-approval test found it by crossing day 365. The owner decided to check a
  delegation as it stood at issue, and `offload grant approve` now issues a fresh one (ADR-0069's
  last amendment). An approver still issues nothing once its own delegation has lapsed, so `health`
  notes that in the delegation's last month.
- **When the approval is the cause, the renewal warning must say so.** A lapsed approver was told
  every 20 s that its certificate was running out and that it needed a second approver. No renewal
  restates an approval a person has not renewed, so the approval sentence now comes first.
