# ADR-0069: Renewal is routine and widely held, approval is a person's act, and an approval lasts a year

**Status:** accepted · 2026-09-26 · session ninety-two · amends ADR-0012 (renewal, delegation,
issuer) · builds phase 5's "approvers in hardware" as an option of any approver · **needs a wire
bump** · **built and walked** (the amendments below; this header said *unbuilt* until session ninety-four)

## Context

Phase 5 asked for the approver key in secure hardware (ADR-0012: *"hardware backing is the point"*).
The first draft of this ADR made the owner's phone the fleet's approver, with a StrongBox key and a
fingerprint for every signature, renewals included. The owner took it apart in three questions, and
each answer is now a rule:

1. *"I'm not going to sit and actively renew all devices all the time."* A certificate lasts 30 days
   and is renewed in its last quarter, so a device renews about every three weeks. A prompt per
   renewal is a chore that grows with the fleet.
2. *"Let's say I have setup 20 VPS … a lot of work."* And, pointed out, *"you are giving a lot of
   weight to the phone."* The draft had made the phone a device the fleet could not do without: 20
   servers that cannot reach it cannot renew, and a phone off for a week lapses the fleet. That is
   the privileged device ADR-0012 exists to refuse.
3. *"The fleet could be suspicious of really old certificates."* The rebels in the stolen shuttle,
   with a code that was valid once. A renewal that carries its original approval can be renewed for
   ever by anything holding renewal authority, so nobody would ever look at the membership again.

What is left of the first draft is the part that was right: **approving a device is a person's act,
and where a device has secure hardware, the key that approves can live in it.**

## Decision

### 1. Two authorities: approving and renewing

- **Approve**, as ADR-0012 has it: enrol a device, grant it something, or re-approve it (§3). A
  person's act, done by an approver or with the passphrase.
- **Renew**, new: restate a **live** membership with fresh dates. It cannot add a member, cannot
  add or change a grant, and cannot revive an expired certificate. It is `Grant::Renew`, **issued at
  the door with `Submit` and `Deliver`**, so every member can renew every other by default. Its power
  is bounded by §2 and §3, not by who holds it, which is what lets 20 VPSes renew each other with the
  phone off. An owner who wants fewer renewers withholds it.

### 2. A renewal proves what it restates, and only a live certificate is renewed

A renewal is a certificate issued by `Issuer::Renewer { node }` that carries its proof:

```
MembershipCert {
    …the same fields…,
    approved_at: Millis,              // when a person last approved this membership (§3)
    issuer: Issuer::Renewer { node },
    proof: RenewalProof {
        approved: MembershipCert,     // the approval this restates: fleet- or approver-signed
        renewer: MembershipCert,      // the renewer's own certificate, carrying Renew
    },
}
```

Every node checks it offline, against the fleet key it already holds:

- `approved` verifies through the ordinary chain (fleet key, or fleet key → delegation → approver),
  and names the **same fleet, member, name and grants** as the renewal. A renewal restates; it does
  not say anything new.
- `renewal.approved_at == approved.approved_at`. Renewals copy the approval date and never reset it.
- `renewer` verifies, grants `Renew`, was valid at the renewal's `issued_at`, is not revoked, and
  its key signed the renewal.
- Neither the member nor the renewer is on the revocation list this node holds.

A **renewer** also refuses what the rules above cannot see: it renews only a certificate that is
unexpired *now* and not revoked by anything it has heard. So honest renewers never bridge a gap. A
device that fell out stays out, and coming back is §3's re-approval.

Renewal is **asked, and can be relayed**. A node whose certificate is due sends its current
certificate (it is public) to any peer, and a peer that is not a renewer passes it on toward one.
Since a renewal can only restate what it is shown, it does not matter who carried the request,
and ADR-0012's "renew only what this connection authenticated" becomes "renew only what the rules
admit". Any renewer that is up and reachable by some path renews. No device is required.

### 3. An approval lasts a year

`APPROVAL_LIFETIME` is 365 days, a protocol constant. Every node refuses any certificate whose
`approved_at` is more than a year old, whatever its renewals say. After a year the membership needs a
person again: `offload reapprove`, run on an approver (or with the passphrase), re-issues the
certificates it names with `approved_at = now`, **all due devices in one action**. `offload status`
and `offload fleet` list every membership within 30 days of its limit, so the chore is one command,
or one confirmation on a hardware-backed approver, once a year, for 20 devices or 2.

The shuttle, checked against this: an old code gets in only if a person approved it within the year.
The limit is enforced by every node, so no renewer can waive it. A stolen renewal credential can
keep existing memberships alive until their year is up, and cannot revive, enrol or grant anything.
Revocation stays immediate and explicit (ADR-0012), and gossips.

Approvers age too. Two approvers re-approve each other, and a sole approver's re-approval is a use of
the passphrase once a year, announced like every use (ADR-0012's alarm). That is the passphrase's
proper frequency, not a daily one.

### 4. A hardware-backed approval key is an option of any approver

`Delegation` names the key an approver approves with:

```
Delegation { fleet, approver: NodeId, issuer_key: IssuerKey, issued_at, expires_at, serial }
IssuerKey = Node                    // the approver's ed25519 node key, today's behaviour
          | P256 { sec1: [u8; 33] }  // a key held in secure hardware
```

Secure hardware signs with **ECDSA P-256** (StrongBox, most TEEs, the Secure Enclave, TPMs), not
ed25519, and the node key cannot move into hardware anyway, because the daemon uses it for every
connection. So a hardware-backed approver has a second key, which only approves. Verification is
`p256` in `offload-core`, with no I/O. **`Node` stays the default**, and a laptop approves exactly as
it does today.

Where the host can hold a key, it does. **The Android host (ADR-0066) is the first:** a P-256 key in
StrongBox, or in the TEE on a device without one, non-exportable, with user authentication required
per use. A request arrives as `sign-requests/<id>.json` in the app's state directory, the channel
`host-facts.json` already uses. The app parses the certificate it is asked to sign and words a
system `BiometricPrompt` from it: *"Approve `vps-17` into your fleet, with submit, deliver and
renew?"*. It writes the signature on a confirmed touch. The prompt is built from the certificate, not
passed in, so a caller cannot show one thing and have another signed. The Mac's Secure Enclave and a
TPM fit `IssuerKey::P256` unchanged, when each has a host to hold the key.

`hardware_backed` is reported only as the host verified it (`KeyInfo.getSecurityLevel()`:
`STRONGBOX`, `TRUSTED_ENVIRONMENT` or `SOFTWARE`), in that device's own `offload status`, and **no
peer acts on it**: a node must not believe a peer about itself. Verifying it remotely is Android key
attestation against Google's root, recorded here and not built.

### 5. A wire bump, and existing certificates

`approved_at`, `Issuer::Renewer` and `IssuerKey` are new on the wire, and an older node can read
none of them. A peer that meets one says *"issued under rules this build does not know — upgrade it"*
rather than *"bad signature"*. Certificates already issued have no `approved_at`, and they read it
as their `issued_at`, so the fleet's first year starts at each device's last issue. Nothing lapses on
upgrade.

## Consequences

- **Nobody renews anything by hand.** Every member renews every other, silently, over any path, on
  a fleet of any size, with no device required. A person approves each device once a year, all due
  devices in one action.
- **No device is the approver.** Any approver approves, the passphrase is the last resort, and a
  hardware-backed key is a property an approver may have, not a role a device must play.
- A stolen credential of any kind, apart from the passphrase, is worth at most a year, and less once
  it is revoked.
- ADR-0012's "renew only what this connection authenticated" and its approver-only renewal are
  replaced by §1 and §2. Its 30-day lifetime and quarter-life renewal stand.

## What this deliberately leaves

- **Remote attestation** of a hardware key (§4).
- **iOS, the Secure Enclave and TPMs**: `IssuerKey::P256` fits them, and each needs a host that holds
  the key.
- **A configurable approval lifetime.** It is a constant because every node must enforce the same
  one, for the reason `BidWeights` is not configurable (ADR-0063 §2). A year was the owner's choice.

## Amendment, 2026-09-26: step 1 built — the certificates and their offline checks

Core and handshake only, no daemon wiring yet. What building it settled:

- **Proofs carry approvals, never renewals.** A renewal carrying the renewer's *current*
  certificate would nest a generation deeper every time it was renewed. `RenewalProof` holds two
  approvals, and `verify_approval` refuses a renewer-issued one (`NotAnApproval`), so a renewal
  stays one link long, as `authority` already did.
- **An approver's approval is checked against its delegation as it stood then**
  (`Delegation::was_in_force(issued_at)`): a delegation that has since expired does not un-approve a
  device, and an approval issued outside its window is refused (`DelegationNotInForce`).
- **The signing bytes are versioned.** A certificate with none of this ADR's fields signs exactly the
  v1 bytes, under the v1 context, and the pinned v1 digests are unchanged. The pinned fixture had to
  name its grants instead of calling `default_grants()`, which grew `Renew`. Everything new signs v2
  (`offload-membership-v2`), pinned too. So **no device re-joins**, but every device needs the v32
  build before the first enrolment or renewal after it.
- **Every renewal copies `approved_at`, whichever path signs it.** Today's automatic approver
  renewal re-stamps `issued_at` every month, so if the approval date were only `issued_at`, the year
  could never bite. Only an enrolment (`Terms::joining`, and `founding` through it) stamps a fresh
  one.
- **A renewal may not shed probation early**, and the handshake refuses one whose **renewer** is
  revoked (`NotAMember`, not `Revoked`: it is not the subject that was evicted).

Tests: nine renewal cases in `offload-core` (valid; self-renewal; widening; no `Renew`; each side's
year; an approver's renewal keeping the date; a renewal as proof; the delegation window; a legacy
certificate), plus the revoked-renewer case with its control arm in the handshake. Wire v32.
**Next, step 2:** a daemon renews any due peer it can reach, a request relays toward a renewer, and
`FleetState` checks the renewer against its revocations as the handshake now does.

## Amendment, 2026-09-26: step 2 walked, and the renewal bug under it

**A member with `Renew` renews a due peer**, as `Issuer::Renewer`, and an approver still renews as
before (`FleetState::renew_for` tries the approver path, then the renewer path). `Peer` carries the
delegation it was admitted with, so a renewer can prove an approver-issued approval. `renewal()`
takes a renewer-renewed certificate's authority from the approval in its proof, or an approver
could not renew a host a renewer had renewed; the test fails without it (control run). `adopt`
refuses a renewal whose renewer this node knows is revoked.

**Walked on two daemons with four-minute certificates** (`CERT_LIFETIME` and the renewal retry
shortened for the walk, then restored). The founder, an approver, was renewed by the joiner, which
holds only `Renew` (`membership certificate renewed by=… as_renewer=true`). The stored certificate
said `issuer: renewer`, with the original `approved_at`, the proof attached, and `host-runs` and
`approve` restated.

**And every renewal after the first failed**, on both nodes: `that certificate has already lapsed`.
The renewer was renewing the certificate *the connection authenticated* (ADR-0012), and a live
session never handshakes again, so at the second cycle it was looking at the certificate the
connection opened with, which had lapsed a minute after the first renewal replaced it. **This
predates ADR-0069.** The approver path had it too: any two nodes holding one connection for longer
than a certificate's life would renew once and then lapse, 30 days in. Fixed by §2's own reasoning:
`RenewMe` carries the asker's current credentials, a renewer takes them only for the asker itself,
and `renew_for` verifies whatever it is given before restating it (new, and needed now that the
certificate is carried rather than proved at the handshake). Re-walked: three renewals each, three
minutes apart, over one connection, with no failures.

## Amendment, 2026-09-26: step 3 built — `offload reapprove`, and a cliff it found

**`offload reapprove` records a decision, not a certificate.** It runs on an approver:
- with no arguments, it lists the memberships the daemon has met whose approval runs out within
  `REAPPROVAL_WINDOW` (30 days);
- `offload reapprove <id>…` or `--due` writes the decision to `fleet.json`, as `reapprovals`,
  which is never gossiped.

A member whose approval is in its last month keeps asking over the existing `RenewMe`, every 15
minutes, whether or not its certificate is due. An approver with a standing decision answers with
`MembershipCert::reapproval`: the member's own terms, restated with `approved_at = now`. A decision
applies while it is newer than the member's approval and for 30 days, so the re-approval it
produces is what makes it moot, and nothing has to consume it. Nothing is done on the devices, so
twenty machines take one command. A host keeps `host-runs`: the fleet-signed authority travels as
it does in a renewal, and verification checks only the certificate's own `approved_at`. `adopt`
takes a newer approval as an improvement whatever its expiry. No node re-approves itself, and a
node that is not an approver refuses to record a decision. `offload status` lists the rows,
`offload fleet` prints the approval's age, and `health` notes the node's own approval. No wire bump.

**Walked on two daemons**, with the certificate lifetime at 4 minutes, the approval at 12 and the
window at 6:
- the approver listed the joiner, `--due` decided, and 18 s later both logs said
  `reapproved=true`;
- the founder, whom nobody re-approved, went `UNUSABLE: approved 12m48s ago, and an approval lasts
  a year`.

The walk found two defects:
- **The approver's view of its peers was their handshake certificates**, which a long connection
  outlives. The joiner was missing from the list because its handshake certificate had lapsed two
  renewals earlier. The `expiring` list had the same flaw. A certificate a peer carries in
  `RenewMe` is now kept once `Members::admits` passes it, and so is what the approver issues. Each
  replaces the kept one only if it is newer.
- **A lapsed approver's warning blamed its certificate** and sent it to find a second approver
  every 20 s. It now says the approval needs a person.

**The cliff, left open for the owner.** A `Delegation` is issued only at `init`, for 365 days, and
nothing re-mints it. An `Issuer::Approver` certificate is verified against its delegation *now*,
which a test states on purpose: `an_expired_delegation_invalidates_what_it_signed`. So one year
after founding, every approver-issued certificate in the fleet stops verifying at once, and the
founder can issue nothing more. That covers invitations, approver renewals and these re-approvals.
Two ways out:
- verify an approver's certificate against its delegation **as it stood at issue**, the rule this
  ADR's first amendment already adopted for approvals inside a proof, since the certificate's own
  month and the approval's year still bound it;
- keep today's rule and re-mint delegations with the passphrase before they lapse. That needs a
  ceremony at least a month early, or certificates carrying the old delegation lapse with it.

It is a security decision with a test stating the current rule, so it was not changed here.

## Amendment, 2026-09-26: the owner's decision on the delegation cliff

**Checked at signing.** An `Issuer::Approver` certificate verifies against its delegation as it
stood at the certificate's `issued_at` (`was_in_force`, plus the fleet key's signature on the
delegation). A delegation that has since expired no longer un-issues what it signed. The test that
stated the old rule is now `a_delegation_is_checked_as_it_stood_when_the_certificate_was_issued`,
and it also checks that a certificate issued after the window is refused. Nothing lives longer than
before: a certificate is bounded by its own month, and an approval by its year.

The approver's power to issue *new* certificates still ends with its delegation. `FleetState`
checks the delegation as of now before it signs anything. So the step the owner accepted with this
choice is `offload grant approve`. It already asks for the passphrase, and it now issues this
device a fresh delegation whenever the certificate grants `approve`. Before this, only `init`
issued one, so the second approver that every note recommends could not exist: the grant alone
produced "granted `approve` with no delegation to prove it". Re-running it on a sole approver is
the yearly passphrase use §3 describes, and it re-approves the node too, because a fleet-signed
certificate is a fresh approval. `health` notes a delegation in its last month. The node-level
re-approval test now runs on the one-year delegation `init` issues, re-approving on day 337 and
verifying on day 366. Under the old rule it failed with the delegation expiring on day 365.
Unwalked: `grant` prompts for the passphrase, which a session does not type.

## Amendment, 2026-09-26: §4 built and walked on the Samsung tablet

**Core.** `Delegation::issuer_key` is `Node` or `P256 { sec1 }`. A `Node` delegation is absent on
the wire and signs the v1 bytes unchanged, so the existing pin still holds. A `P256` delegation
signs under `offload-delegation-v2`, which has its own pin. `Delegation::check_approver_signature`
is the one place an approver's signature is checked, with its node key or with the named P-256 key
(ECDSA-SHA256, `r ‖ s` in the certificate's existing 64 bytes, converted from the DER that hardware
produces). Once a hardware key is named, the node key no longer approves, so a stolen disk enrols
nobody. `p256` is verify-only in core, with no I/O. Wire v33, because a v32 node would call a
hardware approver's papers forged.

**Node and CLI.** The host and the daemon meet in the state directory:
- the host writes `approval-key.json`;
- a signer writes `sign-requests/<id>.json`, holding the **unsigned certificate** and nothing else;
- the host computes the bytes to sign with the bundled `offload signing-bytes` (hidden), words its
  prompt from the same certificate, and answers with `<id>.sig` (DER) or `<id>.refused`.

So what a person reads and what is signed cannot differ, and there is no second implementation of
the format. `invite` signs through it, and checks the result against the delegation before handing
it out. `offload init --hardware-key` founds a fleet that approves with the key from day one, and
`offload grant approve --hardware-key` names the key later. Re-running `grant approve` keeps the
named key rather than falling back to the node key. Routine renewals by a hardware approver go the
renewer's way, with the node key under `Renew`, so the background never needs a person.
**Re-approval through a hardware key is refused with a sentence, not built**: it needs a prompt
raised from the background and a signature served later. `offload status` prints `approval  P-256
in the TEE (hardware-backed) — this node approves with it`. That is the host's own report, acted on
by no peer.

**Android.** The key is created in the Keystore with `setUserAuthenticationParameters(0,
BIOMETRIC_STRONG | DEVICE_CREDENTIAL)`, so every use needs the person. StrongBox is tried first and
the TEE used otherwise. The tablet has no StrongBox and reported `TRUSTED_ENVIRONMENT`. A pending
request raises a notification, and the open app shows the system `BiometricPrompt` with the
signature as its `CryptoObject`.

**Walked.** The tablet made the key and founded fleet `f1ee7003` with `init --hardware-key`. A
laptop node's `offload id` was invited from the tablet, the owner confirmed on the tablet's
prompt, and the laptop joined with the token. The two meshed, each accepting the other's handshake
on papers the tablet's TEE had signed. Found on the way: the app lacked `USE_BIOMETRIC`, and the
uncaught `SecurityException` crashed it (fixed, and a failure to prompt is now a refusal); and
Android 15's edge-to-edge put the button row under the status bar (fixed with insets).

**And on the phone** (StrongBox), the same build made its key in StrongBox. `approval-key.json` says
`"security_level":"STRONGBOX"`, and `offload status` says `approval  P-256 in StrongBox
(hardware-backed) — not named by this node's delegation`. That is right for a phone that is a member
of the laptop's walk fleet and not an approver. The fall-back to the TEE and the StrongBox path have
each now run on a real device.

## Amendment, 2026-09-26: re-approval through a hardware key, built (not walked)

The refusal §4's amendment left in place is gone. A hardware approver records a decision like any
approver. When the member asks, the daemon (`NodeMembership::renew`) takes
`FleetState::hardware_reapproval_for`, the unsigned re-approval checked as `renew_for` checks,
and files it as `sign-requests/reapprove-<member>.json` with `purpose: "reapprove"`
(`approval_key::file_request`). It answers "the re-approval of vps-17 waits for a person to confirm
it on tablet". The app raises "Re-approve a device for another year?" whenever the person looks.
The member keeps asking every 15 minutes, and once the signature is there
(`approval_key::answered`) it is served the signed certificate. It carries the new `approved_at`
and verifies past the old year. A refusal is kept and reported, so nobody is prompted again while
the decision stands. One request per member however often it asks.
`a_hardware_reapproval_is_filed_for_a_person_and_served_once_signed` walks the whole chain with a
software key standing in for the TEE, and fails without the glue (control run). **Not walked on a
device**: it needs the owner at the tablet, and a member whose approval is due, so minute-scale
constants as in step 3's walk.

## Amendment, 2026-09-26: re-approval through a hardware key, walked on the tablet

Walked with the product app (ADR-0071) on the tablet as the only approver of fleet `f1ee7002`,
with its P-256 key in the TEE, and the laptop node `laptop-hw2` as the member. It ran on a walk build
with `APPROVAL_LIFETIME` at 6 h, `REAPPROVAL_WINDOW` at 350 min and the member's retry at 20 s. That
build ran on both sides, because the approver checks the window too. The source was reverted as
soon as it was built.

- `offload reapprove` on the tablet listed `laptop-hw2 … in 0 day(s)`, and the tablet itself as "this
  node: another approver has to do it";
- `offload reapprove e0070273` recorded the decision. At the member's next ask the tablet logged "a
  re-approval is waiting for a person to confirm it here" (19:29:04 local), and the app prompted;
- the owner confirmed. At the member's next ask, 19 s later, it logged `membership certificate
  renewed by=3502859e as_renewer=false reapproved=true`. The new certificate is issued by the
  tablet as approver, with `approved_at` equal to its `issued_at`. The two stayed meshed on it, and
  after going back to the normal build both read "365 more days".

Found on the way:
- `scripts/android-offload.sh` sent `reapprove`, `revoke` and `rekey` the socket instead of the state
  directory, and they answered "this node has not joined a fleet" on a member. Fixed in the script;
- `reapprove`'s sentences restated "30 days" and "every fifteen minutes", which the walk build
  proved wrong beside a 350-minute window. They now read `REAPPROVAL_WINDOW` and
  `mesh::RENEW_RETRY`, the constants the daemon decides by.
