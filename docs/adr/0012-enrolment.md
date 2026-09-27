# ADR-0012: The fleet is a passphrase, not a device — membership is a certificate anyone holding it can issue

**Status:** accepted · 2026-07-26 · settles the trust model that the transport ADR depends on

## Context

Everything in this design is careful about authority — repo-supplied allowlists are capped
to scoped commands, `Ask` is refused rather than silently denied, credentials never travel
in an assignment, and ADR-0011 insists a run sees only the capabilities it was granted. The
front door has had no design at all. `docs/ROADMAP.md` says:

> `offload invite` on an admitted device, `offload join <token>` on the new one

That is the whole of it, and it is the highest-stakes decision in the project. **Joining the
fleet means being handed the right to host runs on the owner's repositories, using the
owner's agent credentials, on the owner's machines.** An attacker who can join does not need
to break anything else.

The constraints are unusual enough that the obvious answers are wrong:

- **No centre.** ADR-0002 rules out a coordinator, so there is no server to ask "is this node
  a member?". Verification has to work on a LAN with no internet, during a partition, on a
  phone that has not gossiped in a week.
- **Nodes meet before they gossip.** mDNS means two nodes can discover each other before
  either has received any view containing the other. A membership check that requires prior
  gossip fails exactly at first contact, which is the moment it is needed.
- **No device may be privileged.** The fleet is fluid by design: devices join, leave, are
  replaced, are lost. A design where some machine is the one that admits others makes that
  machine load-bearing, and a fluid fleet with a load-bearing member is not fluid — it is a
  fleet with a single point of failure that only shows up on the day you sell the laptop.
- **The humans are one person with several devices.** Ceremony that is reasonable at
  organisational scale — a CA, an approval workflow, per-pair fingerprint comparison — is not
  reasonable for adding a laptop on a Sunday.

The tension to resolve: verification wants an anchor, and fluidity forbids anchoring it to a
device.

## Decision

**The anchor is the passphrase, and the fleet key is derived from it.** No device is
special, and no device holds long-term signing authority at rest.

```
fleet_secret = argon2id(passphrase, salt = "offload-fleet-v1", heavy params)
fleet_key    = ed25519 keypair seeded from fleet_secret
fleet_id     = fingerprint of the fleet public key
```

Every member stores the fleet **public** key. The private half exists only for the seconds
it takes to sign something, derived from a passphrase the human types and zeroized after.
Nothing in the fleet holds it between operations.

That single change is what makes the rest fall out:

- **Any device can admit.** Type the passphrase, sign a certificate. There is no admitting
  device, so there is nothing to lose, nothing to promote, and nothing to keep online.
- **A stolen device yields nothing.** There is no signing key at rest to extract. Possession
  of an unlocked phone is not authority to enrol anything.
- **Verification stays offline and works at first contact.** A peer checks a certificate
  against the fleet public key it already holds — no gossip, no roster propagation, no
  ordering problem. This is the property that gossiped rosters cannot provide and the reason
  membership is still a certificate.
- **Losing every device but one loses nothing.** The passphrase reconstitutes the fleet's
  authority anywhere. This is the failure mode a device-held fleet key could not survive.

**A node's identity is its own ed25519 public key.** `NodeId` becomes that key rather than
random bytes — already 32 bytes, so it is a change of the identity file's contents, not its
location. QUIC presents a self-signed certificate for it, so *who are you* is settled by the
handshake; membership answers *should I care*.

**Membership is a certificate.**

```
MembershipCert {
    fleet:      PublicKey,        // which fleet
    member:     NodeId,           // whose key is admitted
    name:       String,           // cosmetic, for `offload nodes`
    grants:     BTreeSet<Grant>,  // HostRuns | Deliver | Submit
    issued_at:  Millis,
    expires_at: Millis,           // ~30 days, renewed on contact
    serial:     u64,
}                                 // signed by the derived fleet key
```

`grants` lives on the certificate rather than in the node's own config because `WorkPolicy`
is self-asserted: a phone that should never host runs must not be able to promote itself by
editing its own TOML. Its peers refuse it.

**Joining has two paths, and the ordinary one does not involve the passphrase.**

```
offload join                 # asks an approver; you confirm on your phone
offload join --passphrase    # bootstrap and recovery: derive, self-issue, wipe
```

Ordinarily the joining device asks an enrolled approver, the owner confirms on a device they
are holding, and the approver issues the certificate under its delegation. The passphrase
path exists for founding the fleet, for enrolling the first approver, and for the day every
approver is gone — the new device derives the fleet key, issues **its own** certificate, and
wipes the key, needing nothing else to be awake.

`offload invite` remains for devices where interactive confirmation is awkward — a headless
box over SSH. An approver issues a one-time, minutes-long token carrying a certificate for a
key the joiner sends back. Convenience, not a third trust path.

**Every admission is announced.** A new member is a `Notification` on the delivery plane
(ADR-0010), delivered to whichever device the owner is holding: *"laptop-2 joined the fleet,
fingerprint ab12…"*. With no per-join approval step, this is the compensating control — an
enrolment the owner did not perform is visible within seconds rather than discovered later
in `offload nodes`.

**Revocation is a fleet-signed, monotonic, non-refutable fact.**

```
Revocation { fleet, member, revoked_at, serial }   // signed by the derived fleet key
```

`offload revoke <node>` prompts for the passphrase, same as admission. The record gossips
like any other fact and extends ADR-0005's ownership table with a row that behaves unlike
every existing one:

| Fact                    | Owned by            | Arbitrated by | Refutable by the subject? |
| ----------------------- | ------------------- | ------------- | ------------------------- |
| a node's liveness       | its peers           | `incarnation` | **yes** — that is SWIM    |
| a node's membership     | the **fleet key**   | signature     | **never**                 |

This is the trap the design would otherwise walk into. The `incarnation` counter exists so a
node can refute a `Suspect` assertion about itself — correct for liveness, because the node
knows and its peers are guessing. Applied to membership it would let a revoked device argue
its way back in. Same gossip path, same-looking fact, opposite rule.

Two backstops, because gossip is eventually consistent and revocation must not be:

- **Certificates expire** in ~30 days and are renewed on contact, so a node that never
  receives a revocation falls out of the fleet when its certificate lapses.
- **Revocation is immediate and local.** The revoking node drops connections to the member
  and refuses new ones without waiting for the fleet to converge.

**Leaving voluntarily needs no passphrase.** A node announces `Departed`, which peers believe
immediately — a node knows when it is leaving (ADR-0005). Revocation is for the devices that
will not say so themselves.

**The passphrase is generated, never chosen, and never stored on a node.** `offload init`
prints a diceware phrase of at least six words, once. Because the fleet public key is on
every device and travels in every certificate, an attacker holding either can grind candidate
passphrases offline; the only defences are entropy and a slow KDF, and a human-chosen
password defeats both. Argon2id parameters are tuned so one guess costs about a second on a
phone. Nodes keep the public half and nothing else, so there is no verifier on any device to
attack and nothing a stolen device can give up.

### The passphrase is a recovery secret, not a working credential

Deriving the fleet key from a typed passphrase makes *every* enrolment a moment where the
fleet's root secret is on a keyboard. That is the wrong daily posture, and it argues against
itself: a secret strong enough to resist offline grinding is one nobody will retype
willingly, and one that gets pasted from somewhere convenient the third time it is needed.

So the passphrase moves off the daily path entirely. **The shape is an offline root with
online issuers** — the same arrangement as an offline CA, a recovery key, or a hardware
backup key, and it is well-trodden rather than novel:

- **The passphrase is the offline root.** Written down, stored somewhere safe — a password
  manager, a paper copy in a drawer — and used for exactly three things: founding the fleet,
  delegating to an approver, and recovering when the approvers are gone.
- **Approvers are the online issuers.** At `init`, the passphrase-derived key signs a
  **delegation** naming an approver's key as permitted to issue membership certificates.
  Afterwards, enrolling a device is the approver's job and the passphrase stays in the safe.

```
Delegation { fleet, approver: NodeId, may_issue, expires_at, serial }   // fleet-signed
```

`may_issue` turned out not to be expressible as a field on the delegation, because renewal has
to restate grants an approver may not mint. See the 2026-08-21 amendment: what bounds an
approver is a fleet-signed certificate the renewal carries.

A peer verifying a new member checks a two-link chain — fleet key → delegation → membership
certificate — against the fleet public key it already holds. Still offline, still valid at
first contact, still no privileged device the fleet cannot lose: a delegation is re-mintable
by whoever holds the passphrase, which is why the approver is an issuer rather than a root.

**`offload init` pushes you to enrol an approver immediately**, while the passphrase is still
on screen and the fleet is one device. The prompt is the design: a fleet whose only
enrolment path is a secret in a drawer is one where the drawer gets opened routinely, which
is the failure this arrangement exists to prevent.

**Using the passphrase is an alarm.** Once an approver exists, any use of the passphrase —
enrolment, delegation, rekey — is announced on the delivery plane and recorded durably:
*"the fleet passphrase was used on desktop at 03:14"*. This is the compensating control that
break-glass credentials make possible and daily ones cannot: when the secret is never used,
every use is signal rather than noise.

**A recovery secret nobody has tested is not a recovery secret.** `offload verify` prompts
for the passphrase, derives the key, compares it to the fleet public key, and reports whether
what you have written down is what the fleet expects — granting nothing and changing nothing.
`offload status` says when it was last verified.

**Losing the passphrase means re-founding the fleet, and that is the whole recovery story.**
No escrow, no split-key scheme, no social recovery, no "any three devices may re-mint" — each
of those is either a privileged device wearing a disguise or a pile of machinery guarding a
personal fleet. This is an explicit non-goal, recorded so it does not get invented later.

What makes that acceptable is that re-founding is a chore rather than a catastrophe, and two
rules keep it that way:

- **The fleet key authenticates; it never encrypts.** Nothing at rest is readable only under
  it — not blobs, not transcripts, not the store. Losing it must cost membership and nothing
  else. Any future design that encrypts data under the fleet key converts a lost passphrase
  from an afternoon into permanent data loss, and is forbidden by this ADR.
- **Re-founding keeps everything a device owns.** `offload init --refound` mints a new
  passphrase on a node that already has runs, mirrors, worktrees, blobs and — crucially — its
  own node identity, all of which survive untouched. Every other device runs `offload join`
  once. Old certificates are not revoked, they are simply signed by a fleet that no longer
  exists.

So the honest description of losing the passphrase is: walk to each device once. Given that,
the effort belongs in making sure it is *only* that, rather than in machinery to avoid it.

**An approver may delegate to another approver**, which keeps the drawer shut in the case
that actually happens — a phone dies, and its replacement is enrolled by the laptop. The
trade is that a compromised approver can breed more of them, so recovering from a compromised
approver means re-founding rather than revoking one certificate. That is the same answer as
above, which is what makes it tolerable; a fleet unwilling to accept it should grant `Approve`
from the passphrase only.

**Membership is not authorisation.** Being admitted puts a node in the view: it can be seen,
can bid within its grants, can submit. It does not grant use of any capability (ADR-0011),
and it does not grant repository access beyond what placing a run on it implies.

### One fleet per node identity — and a device may hold several

A *node* belongs to exactly one fleet. A *device* may run several nodes, each with its own
identity, state directory and fleet — which is how a laptop belongs to both a personal fleet
and a work one:

```
OFFLOAD_STATE_DIR=~/.offload        offloadd     # personal
OFFLOAD_STATE_DIR=~/.offload-work   offloadd     # work
```

This already works; it is how two nodes are run on one machine for testing. Making it the
answer for multi-fleet membership is a decision, not an accident, and the reason is that
every hazard of multi-tenancy is a **shared store** hazard:

- **Warmth leaks work.** `LocalFacts::workspace_warm` says which repositories this device
  already holds. Gossiping that to a second fleet discloses the existence of the first
  fleet's repositories — a client name is often the whole secret.
- **Blobs are a confirmation oracle.** Content addressing means a peer can ask "do you have
  this hash?". One blob store serving two fleets answers questions the second fleet should
  not be able to ask, and one careless fetch path serves a checkpoint across the boundary.
- **Grants would have to be per-fleet.** A mailbox capability granted in the personal fleet
  must be unreachable from a run placed by the work fleet (ADR-0011), which means every grant
  check gains a fleet dimension that is easy to forget in exactly one code path.
- **Run registries would need partitioning**, and `offload ps` would need to ask which fleet
  it means.

Separate processes give all of that by construction rather than by discipline, and separate
identities are a feature in their own right: the two fleets cannot correlate the device, and
neither learns that the other exists.

Two consequences that need saying plainly:

- **Whose account pays?** Agent auth is a node capability, so a run placed by the work fleet
  on this device runs against whatever agent credentials the device holds — potentially a
  personal account, with personal rate limits and a personal bill. Multi-fleet devices must
  configure per-instance agent credentials deliberately, and the probe must not report an
  account the operator did not intend that fleet to use.
- **Nothing may bridge.** A device in two fleets must never forward gossip, blobs, or runs
  between them. Separate processes make that the default; it stops being the default the
  moment anything is shared "for efficiency".

The genuine cost of this shape is that **the two nodes over-commit the device**: each has its
own `max_concurrent_runs` and neither knows about the other, so a laptop configured for two
runs per fleet can end up hosting four. Battery floors and thermal limits have the same
problem. That is a real gap, and the fix is a small device-local resource broker both
instances consult — a local file or socket carrying "what is this machine already committed
to" — rather than merging the daemons and re-creating every hazard above. Not designed here;
recorded as roadmap open question #8.

### Approvers: a second factor that is a grant, not a root

A fleet may require enrolments to be **approved** by a device that already belongs to it.
`Grant::Approve` is issued the same way as any other grant, and an approving device signs a
statement that a specific node key may join.

This is not the privileged device this ADR rejects, and the difference is worth being precise
about, because it is the whole reason the posture is admissible:

- **A root can admit alone.** Losing it removes the fleet's ability to enrol, so the fleet
  structurally cannot lose that member.
- **An approver can only consent.** It adds a factor to an act the passphrase already
  authorises. Losing every approver removes a *safeguard*, never the fleet's ability to
  operate — and the safeguard is re-issuable to any device.

Three conditions keep it on the right side of that line:

1. **Approval is an offline artifact, not an online check.** The approver signs
   `Approval { fleet, member, expires_at }`; the joiner carries it; peers verify it against a
   key they already hold. Peers never need a live path to the approver — a rule that matters
   because "the phone was asleep" must not mean "the laptop cannot be enrolled", and because
   requiring reachability would give up the offline first-contact property the rest of this
   ADR is built around.
2. **`Approve` is re-issuable with the passphrase**, like every other grant. A drowned phone
   is replaced by granting `Approve` to another device, not by a recovery procedure.
3. **`offload rekey` is always available and needs only the passphrase.** It re-founds the
   fleet from whatever devices are still reachable. This is the escape hatch that makes the
   whole posture safe to adopt: however the approver set is lost, corrupted, or turned
   against the owner, there is a way back that depends on no device at all.

**Hardware backing is the point.** Where the platform offers it — Secure Enclave, StrongBox,
a TPM — the approver key is generated in it, non-extractable, and gated on biometric or PIN.
That converts "something you have" from a file that can be copied into a key that cannot,
and it puts the confirmation prompt somewhere the owner already trusts. The probe reports
`hardware_backed` per the existing rule: unverifiable means `false`, because a node claiming
protection it does not have is worse than one claiming none.

**The policy is monotonic in the safe direction.** Once a fleet has recorded that enrolment
requires approval, peers reject certificates without one. Turning the requirement *off*
requires an approval — raising the bar is easy, lowering it needs the thing being lowered.
Otherwise an attacker holding the passphrase would simply issue themselves a certificate
asserting that approval was never needed, and the factor would be decorative.

**Register at least two approvers.** One is a single point of inconvenience: lose it and the
next enrolment costs a rekey. `offload status` warns when a fleet has fewer than two, which
is the kind of drift nobody notices until the day it matters.

The residual risk is the one already accepted: a stolen, unlocked approver has both factors.
Hardware backing narrows even that — a thief needs the device *and* the biometric — which is
the same bargain every 2FA device makes, and it is a better bargain than the alternative of
having no second factor at all.

## Mitigations

The passphrase being the fleet is an accepted risk, not an ignored one. None of what follows
introduces a privileged device; all of it either narrows what a silent enrolment achieves,
shortens how long it survives, or makes it visible.

**1. Least privilege at the door.** A joining node receives `{Submit, Deliver}`. `HostRuns`
— the grant that turns membership into "runs agents on your repositories with your
credentials" — is never issued by joining. It is granted explicitly:

```
offload grant <node> host-runs        # prompts for the passphrase
```

This is the highest-value mitigation in the list, because it decouples *being in the fleet*
from *the thing an attacker wants from the fleet*. A silently enrolled node can observe and
submit; it cannot execute.

**2. Probation.** Even once granted, `HostRuns` does not take effect until the certificate is
`probation` old — 15 minutes by default. The window exists so that a notification about an
enrolment nobody performed arrives while revoking is still ahead of the attacker rather than
behind. A certificate issued through `offload invite` skips probation, because an existing
member deliberately issued it: that is the invite path's real purpose, and the reason to keep
it after joining stopped needing it.

**3. Once an approver exists, it is required.** The fleet records that enrolments need an
approval, and the record is monotonic in the safe direction: turning the requirement off
needs an approval. That makes the approver a real second factor — a passphrase leaked into a
screenshot, a backup, or a keylogger on a non-member machine does not enrol anything by
itself.

An earlier revision proposed time-boxed enrolment windows attested by any member
(`offload open 15m`) to achieve the same thing without approvers. Approvers do it better —
hardware-backed, explicitly consented to, verifiable offline — so windows are dropped rather
than kept alongside. Two mechanisms for one property is machinery, not defence.

What the requirement cannot cover is its own escape hatch: `offload join --passphrase` has to
work when every approver is gone, or the fleet has no recovery. That path is deliberately
unguarded and deliberately loud — announced, recorded, and subject to probation and least
privilege like any other enrolment.

**4. Every enrolment is announced and recorded.** A join emits a `Notification` on the
delivery plane (ADR-0010) — *"laptop-2 joined the fleet, fingerprint ab12…, granted submit,
deliver"* — and is written durably to every node's store, so the audit trail survives a
missed notification and a lost device. `offload nodes --history` reads it back.

**5. `offload rekey` is the convergent revocation.** Ordinary revocation is eventually
consistent and, in the worst case, outrunnable. Rekeying is not:

```
offload rekey                    # new passphrase, re-issue to every known member
offload rekey --evict <node>     # ... except that one
```

The evicted device is not revoked; it is simply not in the new fleet, and no gossip has to
reach anybody for that to be true. This is the "I am serious" button, and it is one command
precisely because the alternative — a documented multi-step procedure — is a thing nobody
does at 2am.

**6. Fleet health is visible without being asked for.** `offload status` reports member
count, certificates nearing expiry, the age of the passphrase, and any member that has not
been seen since before the last rekey. It also nags about the two states that turn a bad day
into a bad week: **no approver enrolled** (so the passphrase is still the working credential)
and **only one approver** (so losing it costs a recovery). Silent drift is how a fleet ends
up with a member nobody remembers adding, and how a recovery secret nobody has read in a year
turns out to be wrong.

Worth stating alongside these: **a rogue member still cannot obtain the owner's agent
credentials.** Auth is a node capability and never travels in an assignment (ADR-0002), a run
sees only the capabilities it was granted (ADR-0011), and tool access is allowlisted. The
blast radius of an enrolled attacker is what a member can do, and the rest of this design has
been keeping that small for other reasons.

## Consequences

Good:

- **Nothing is load-bearing.** Any device can be lost, sold or wiped without weakening the
  fleet's ability to admit, revoke, or verify. That is what "fluid" has to mean to be worth
  the word.
- **Verification is local and offline.** Two members that have never met, on a LAN with no
  internet, mid-partition, accept each other immediately — no bootstrap ordering, no waiting
  for a roster to propagate at exactly the moment membership matters.
- **Joining is one command and one secret**, which is the difference between doing it
  properly and working around it.
- **No secret sits at rest anywhere.** The fleet key exists for the duration of a signature.
  A stolen device gives an attacker a member's identity — bounded by that member's grants —
  and no ability to enrol anything.
- Revocation composes with the existing gossip model: one more fact, with an owner and an
  arbitration rule, exactly the shape ADR-0005 demands.

Bad, and worth being clear-eyed about:

- **The passphrase is the fleet.** Anything that learns it can admit, revoke, and act with
  the owner's authority. Keeping it off the daily path and requiring an approver raise this
  from "knows the passphrase" to "knows the passphrase *and* holds an unlocked approver", and
  least privilege plus probation narrow what a successful enrolment achieves — but a stolen,
  unlocked approver satisfies both, and no device may refuse it, because refusing would
  require a privileged device. This is the price of no root, paid knowingly.
- **The recovery secret is the thing most likely to be lost**, because it is deliberately
  never used — and losing it means re-founding, by decision rather than by oversight. The
  defences are `offload verify`, the `status` nag, and approver-to-approver delegation so the
  drawer stays shut in the ordinary case. What is left is a chore that scales with device
  count, and a fleet of eight is a genuinely tedious afternoon.
- **"Never encrypt under the fleet key" is a constraint on work not yet written**, which
  makes it the easiest rule in this document to break by accident. It belongs in review of
  anything that touches blobs or the store.
- **Rekeying is the unpleasant path.** Changing the passphrase changes the fleet key, which
  invalidates every certificate and requires every device to re-join. Necessary after a
  suspected leak, and it should be one command rather than a folklore procedure.
- **Revocation is eventually consistent**, worst case bounded by certificate lifetime. A
  partitioned member honours a revoked certificate until it hears or the certificate lapses.
  30 days is a guess and wants revisiting against real usage.
- **Revocation does not un-give what a device already holds.** A revoked phone still has
  every repository, blob and token it had. Revocation stops future participation; the honest
  remedy for a lost device is rotating what it held, and the docs must say so.
- **Silent enrolment is possible by design**, and the mitigations are mostly detective or
  limiting rather than preventive. The compensating controls also have a dependency problem:
  the enrolment notification is only as good as the delivery plane it rides on, and that does
  not exist yet. Until it does, probation protects a window nobody is watching.
- **The mitigations add real machinery** — windows, probation timers, grant issuance,
  approvals, rekeying — to what was one signature check. Least privilege and probation are
  cheap and should ship with enrolment; windows, approvers and rekey are worth their cost but
  are not the minimum viable version.
- **Approvers make the good path depend on a device being to hand.** Enrolling a laptop at a
  desk while the phone is upstairs is a small, real annoyance, and the mitigation for it —
  registering several approvers — is exactly the thing that widens the set of devices whose
  theft matters. There is no arrangement of this that is both convenient and tight.
- **Hardware-backed approver keys cannot be migrated**, by design. Replacing a phone means
  granting `Approve` to the new one, and the old key is simply abandoned. This is correct and
  it will still surprise someone restoring a device from a backup.
- **A device that sleeps past its certificate expiry wakes up outside the fleet.** Plausible
  for a phone, so renewal has to be automatic on contact rather than something the owner
  remembers.

## Alternatives

**A device-held fleet key** (the first draft of this ADR). Stronger against passphrase
compromise, since a keylogger yields nothing without the key file. Rejected because it makes
some device privileged: the fleet's ability to admit and revoke then depends on that machine
still existing, and a fluid fleet cannot have a member it structurally cannot lose.

**Multi-root — several devices hold signing authority.** Softens the single-device
dependency and keeps the passphrase out of the threat model. Rejected for the same reason at
one remove: roots are still privileged devices, the root set needs its own promotion and
demotion story, and revoking a root while it can revoke back is a genuinely hard corner. The
fluidity requirement rules out the whole family.

Note the distinction from the approvers described above, which look similar and are not the
same thing. A root *can admit by itself*, so the fleet depends on it. An approver can only
*consent* to an act the passphrase already authorises, its grant is re-issuable to any
device, and `offload rekey` recovers from losing all of them. One is a dependency; the other
is a safeguard that fails open into a well-defined recovery.

**A gossiped roster any member may add to**, with no signing at all. The most fluid option
and the original suggestion this ADR grew from. Rejected on two counts: a peer that has not
yet received the roster cannot verify a member at first contact, which is precisely when
mDNS needs it; and concurrent ban/admit has no owner and no total order, so it converges to
either "one compromised device can ban everyone permanently" or "a banned device gets back in
via any peer that has not heard yet". Hardening it against both reconstructs signed
certificates — which is what this ADR is, with the signing authority moved off devices
entirely.

**Trust on first use with per-pair approval**, SSH style. Rejected on ergonomics: enrolling
the fifth device costs four confirmations, and the human ends up approving fingerprints they
do not check.

**A shared password as the membership credential itself**, wifi style — every device holds
the same secret and knowing it *is* being a member. Rejected: no per-device identity, so no
per-device revocation, no grants, and no attribution of which device did what. The passphrase
here authorises *issuing* credentials; it is not itself the credential.

## Amendment, 2026-08-20: where peers actually refuse a self-promoted host

The decision said "its peers refuse it" and left the where implicit. Building it settled
three things.

**The refusal lives at the bid exchange, not the handshake.** A node without `HostRuns` is a
full member — it submits, it delivers, it carries a phone's whole usefulness — so the
handshake must keep admitting it. What peers refuse is the one thing the grant gates: its
*bid*. An honest node never bids without the grant (its own bid path checks its own
certificate), so an arbiter-side objection fires only for a node that is misconfigured, out
of date, or lying — and it answers with a reason like any other refusal, so `offload run` and
`explain` print "bid, but its certificate does not grant host-runs" rather than placing the
run or shrugging.

**The certificate travels with the session and is asked at decision time.** The handshake
used to reduce the certificate to an effective grant set, which bakes in a moment: probation
ends while a connection stays up, and a check against admission-time grants would refuse a
now-legitimate host until something happened to redial. `Peer` now carries the certificate
itself and `Peer::may(grant, now)` asks it fresh, so probation lifts on a live connection
with nothing re-issued, nothing redialled and nothing gossiped — every node computes it, which
is the property grants were put on the certificate to get.

**An objection drops the connection, because staleness runs the other way too.** `offload
grant host-runs` mints a *new* certificate, and a live connection still carries the old one —
enforcement against the session's papers would turn "a grant needs no restart" into "a grant
needs the link to happen to break". Refusing the bid therefore also closes the connection:
the next round re-handshakes, presents the renewed certificate, and the run lands within the
arbiter's own retry. The same close handles an expired certificate — the re-handshake is
refused at admission, and the node fades to silence rather than bidding for ever on papers
that lapsed mid-connection.

Still open, and named so it is not mistaken for done: the *sender's* authority is not checked
at the bid exchange — any admitted member can act as an arbiter and send a grant, which is
what arbiter failover requires, so tightening it needs an answer to "who may arbitrate this
run" rather than a grant lookup. And revocation mid-connection still relies on the probe path
noticing; the bid check refuses a revoked node's bids only once the connection re-handshakes.

## Amendment, 2026-08-21: what building the rest of it settled

Six things were built against this ADR in one session, and four of them were claims the code
did not honour. Recorded here so the ADR describes the tree rather than an intention.

**A revocation now travels, and a grant now takes effect.** Both were written down here and
neither happened. Every membership command works with no daemon — that is this ADR's central
claim in operational form — so `fleet.json` is written by a process that is not the daemon, and
the daemon never looked again. `offload grant host-runs` re-issued a certificate the daemon kept
not presenting; `offload revoke` wrote a fact the daemon never read, which made "revocation is
immediate and local" mean "immediate, once you remember to restart the daemon", on every machine,
because the daemon is always up. The daemon re-reads the file now, and the revocation gossips.

The gossiping needed one sentence of justification the ADR had not made explicit, and it is worth
keeping: **membership is the one thing here that does not need to travel** — a certificate
verifies against the fleet public key at first contact, with nothing gossiped — and revocation is
the exception in a precise sense. It is the *absence* of a signature, and no certificate can say
"except this one". That is why it is the only membership fact on the wire.

And the half that is easy to leave out: membership is checked at the handshake, and a live QUIC
session never handshakes again. A node that files a revocation and keeps the socket open has
recorded a fact and changed nothing.

**Certificates are renewed on contact.** This ADR leans on expiry twice — it is what makes a
revocation that never arrives eventually take effect, and what makes a device out of contact for
a month fall out by itself — and nothing renewed anything. So the real behaviour at thirty days
was not decay towards safety; it was every device dropping out at the same moment, with the
passphrase as the only way back on each of them. Founding a fleet quietly set a date.

An approver re-issues under its delegation, asked rather than offered: the node whose certificate
is lapsing is the one that knows, and an approver volunteering renewals would renew nothing for
the laptop that has been shut for three weeks — which is the device that needs it. The request
carries *nothing*: what is re-issued is the certificate this connection's handshake authenticated,
which is `admit`'s separation of the claimed from the proved applied to the other place a
certificate is minted.

Renewal changes nothing but time, which is what justifies doing it automatically, with nobody at
a keyboard. **Probation is the one field it cannot copy verbatim**, because it is measured from
`issued_at` and renewal moves that: a certificate still in probation keeps it and so restarts it,
and one that has served it out drops the flag. Re-imposing fifteen minutes of dormancy on a
device that has been hosting runs for a month would be a monthly outage nobody could explain.

**`offload invite` exists, and the token is not a bearer credential.** This ADR imagined
something "one-time, minutes-long". What is actually carried is a membership certificate, which
names the joining device's key and is useless to anybody who does not hold it — so it needs no
expiry of its own, and it can be pasted anywhere. That is the same sentence as *a membership
certificate is not a bearer token*, arrived at from the other end. The rule it must not break is
the other one, and it does not: nothing here puts the passphrase on a command line.

Which produced a correction to mitigation 2. **Probation follows the grant, not the path.**
Probation only ever suppresses `HostRuns`, so a certificate without that grant which sets the flag
delays nothing: "an invite skips probation" is a statement about the door, where the grant is not
in play. Mitigation 2 is about the grant, and it has no exemption for the passphrase, because
somebody holding the passphrase is the threat it was written for. So `offload invite --grant
host-runs` — which is this ADR's `offload grant <node>`, by the route that does not need the two
devices to be on speaking terms — probates like the local command does. Without that it would
have been the documented way around mitigation 2.

**Mitigation 4 is built, and it is the one this posture actually rests on.** With no per-join
approval step, "an enrolment the owner did not perform is visible within seconds" is the whole
compensating control, and probation's fifteen minutes were being bought for an alarm that did not
exist. It needed the delivery plane to be able to address something other than a run, so a
notification now carries a subject and the store has a second log.

Two producers, because neither is enough alone: the CLI writes the event (these commands run with
no daemon, so an alarm that needed one would be missing on the machine somebody just walked up
to), and every *other* node writes one when it meets a member it has no record of — because the
machine that issued the invitation may hold no route to a person and may be the machine an
attacker used.

That makes it **a log of what a node witnessed, not the fleet's memory**, and this ADR's "written
durably to every node's store" is corrected accordingly. A fact written durably everywhere is a
gossiped fact, and a gossiped fact needs an owner and an arbitration rule (ADR-0005), which an
append-only log of independent observations does not have. Each node records what it saw; `offload
nodes --history` says so and tells you to ask another device.

**`offload rekey` exists, and needed a mechanism this ADR did not name.** The first version
printed an invitation per member and every one of them was refused, because a device quite rightly
will not let an unfamiliar fleet replace the one it belongs to — an invitation to a fleet it has
never heard of is exactly what an attacker would send, and the device cannot tell the two apart by
looking.

So a rekey carries a **succession**: the new fleet's identity signed by the key being replaced,
which every member already holds. Only somebody with the old passphrase can move a device, which
is the same authority that could already do anything to it. Both ends of the statement are
checked, because half of it is the dangerous half — a valid succession out of some other fleet
says nothing about this one, and one naming a successor other than the certificate's issuer would
move the device somewhere the signer never authorised. The succession is public material: knowing
that fleet A was succeeded by B grants nothing, because joining B still needs a certificate B's
key signed, and the evicted device is simply not issued one.

Members to re-issue to come from the fleet log, which is the only list a command with no daemon
and no network can have. It holds the devices this node witnessed, so one enrolled elsewhere and
never met has to be re-invited by hand — said in the output rather than quietly left out.

### And one thing deliberately not built: mitigation 3

"Once an approver exists, it is required" is not implemented, and the reason is in this ADR's own
text. The requirement is supposed to stop a leaked passphrase enrolling anything by itself — and
two paragraphs later the ADR grants the exemption that removes the protection: *"`offload join
--passphrase` has to work when every approver is gone, or the fleet has no recovery. That path is
deliberately unguarded."* An attacker holding the passphrase takes the unguarded path.

What is left of the mitigation after that exemption is a bar against a *careless* use of the
passphrase rather than a hostile one, and the cost is a fleet-signed, gossiped, monotonic policy
record — a new arbitration rule for the sake of a control its own escape hatch defeats. The alarm
(mitigation 4) is what actually covers this threat, and it is built. If per-join approval is ever
wanted for real, the thing to design is the online `offload join` this ADR describes, where an
approver is *asked* and a person confirms — not a flag saying certificates must carry a
countersignature.

---

## Amendment, 2026-08-21: `may_issue`, and the thing it could not be

Mitigation 1 of this ADR is that `HostRuns` stays behind the passphrase, and the delegation
sketch above has the field that would have bounded it:

```
Delegation { fleet, approver: NodeId, may_issue, expires_at, serial }   // fleet-signed
```

`may_issue` never reached the code. So the delegation bounded **who** may issue and nothing at
all about **what**, and an approver — holding a delegation and its own node key, which is what
an approver *is* — could sign a certificate granting `HostRuns` to any device, with probation
cleared, and every peer in the fleet verified it and admitted it. One compromised approver was
one certificate away from a device that receives and executes the fleet owner's runs. Found by
asking the property tests the obvious question about a credential chain: does one that verifies
ever grant more than it was issued.

**A flat bound would have been the wrong fix, and this is the part the sketch had not reckoned
with.** Certificates are renewed on contact by an approver — that is what makes an unheard
revocation eventually bite and what removes a device that has been away a month — and a renewal
*restates the grants it is renewing*. An approver forbidden from ever signing `HostRuns` cannot
renew a host node either, so every host in the fleet would need the passphrase every thirty
days. That is the same failure as having no renewal at all, which this ADR already had once.

So what a verifier needs is not a bound on grants but the difference between **minting** and
**restating**, and it has to be checkable with the fleet public key alone, offline, at first
contact — this ADR's central property. A renewal therefore carries the fleet-signed certificate
it descends from, and may grant no more than that certificate did:

```
MembershipCert { …, authority: Option<Box<MembershipCert>> }
```

- An approver may issue `default_grants()` — `{submit, deliver}` — freely. That is enrolment,
  and it is the entire reason approvers exist.
- Anything beyond that verifies only against an `authority`: a certificate **the fleet key
  signed, for this same member, in this fleet**. An approver-issued authority is refused, or a
  chain could launder a grant nobody with the passphrase ever made.
- The authority is the **root** of the chain rather than the previous link, so a host renewed
  monthly for two years still carries one certificate rather than twenty-four.
- Its **expiry is deliberately not checked**. The grant was made once, long ago; the renewal in
  hand is what keeps it current. Requiring the original to be live would mean a certificate
  could only be renewed while it did not need to be.

**`Approve` is bounded the same way, which is the stricter of the two positions this ADR
names.** It accepts that "a compromised approver can breed more of them" and adds that "a fleet
unwilling to accept it should grant `Approve` from the passphrase only". The strict reading is
free here: nothing in this tree issues an approver-to-approver *delegation*, and a certificate
granting `Approve` with no delegation beside it cannot issue anything — so the loose reading
would buy a capability nobody can exercise at the price of the escalation being real. If
approver-to-approver delegation is ever built, that is the moment to revisit this, and the
mechanism to revisit it *with* is `may_issue` on the delegation, which is where the ADR put it.

**`may_issue` is therefore implemented as a constant rather than a field.** No caller would set
it to anything but `default_grants()`, and a fleet-signed, gossiped, per-approver policy record
that every fleet ships identically is machinery guarding a personal fleet — the same argument
this ADR made against mitigation 3. Where it goes when it is needed is written down; it is not
built.

### And the cost, which is not small: every device re-joins

Adding a field to a certificate changes the bytes its signature covers, so **every credential
issued before this change stops verifying**. There is no migration for a signature. A fleet
upgrading past this runs `offload join --passphrase` on each device once, or `offload invite`
from one that has already re-joined.

That is stated loudly because nothing in the tree made it loud. `fleet::passphrase` has had a
pinned known-answer test since it was written, for a reason that applies to the signed message
word for word: the value is compared across machines, so changing it does not fail, it silently
re-founds every fleet. The certificate's own signing bytes had no such test, and this amendment's
change to them broke every existing credential with no test anywhere going red. There is one
now, over all four credential types — see `the_signing_bytes_are_pinned_to_a_known_answer`. A
deliberate change to the format is one that updates those digests *and* says in its commit
message that every device has to re-join.
