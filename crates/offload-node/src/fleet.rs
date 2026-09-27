//! What this node knows about the fleet it belongs to, on disk.
//!
//! Everything here is **public material**: the fleet's public key, the certificate that
//! admitted this node, the delegation behind it, and any revocations heard so far. The fleet
//! secret is not in this file and must never be — ADR-0012's central claim is that a stolen
//! device gives an attacker a member's identity and no ability to enrol anything, and that
//! claim is worth exactly as much as this rule.
//!
//! One file, rewritten whole, because it is small, changes rarely, and is read by a human
//! when something is wrong. Phase 3's gossip will add revocations here from peers; the shape
//! already anticipates that, which is why `revocations` is a list rather than a flag.

use offload_core::fleet::{FleetKey, IssuerKey, CERT_LIFETIME, REAPPROVAL_WINDOW};
use offload_core::{
    Delegation, FleetId, Grant, Issuer, MembershipCert, MembershipError, Millis, NodeId,
    Revocation, Succession, Terms,
};
use offload_proto::handshake::Credentials;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Public fleet material. Sits beside `node-key`, which is the half that must stay secret.
pub const FLEET_FILE: &str = "fleet.json";

/// How long a delegation runs before an approver has to be re-authorised.
///
/// Longer than a certificate on purpose: re-minting it needs the passphrase, and a delegation
/// that expires monthly is a drawer that gets opened monthly — the failure ADR-0012's whole
/// offline-root arrangement exists to prevent.
pub const DELEGATION_LIFETIME: Millis = Millis(365 * 24 * 60 * 60 * 1_000);

#[derive(Debug, thiserror::Error)]
pub enum FleetError {
    #[error("this node has not joined a fleet — run `offload init` or `offload join`")]
    NotAMember,
    #[error("that passphrase derives fleet {derived}, but this node belongs to {expected}")]
    WrongPassphrase { expected: String, derived: String },
    #[error("this node's certificate is not usable: {0}")]
    Invalid(#[from] MembershipError),
    #[error("certificate names {member}, but this node is {node}")]
    NotOurs { member: String, node: String },
    #[error("could not read {path}: {reason}")]
    Read { path: String, reason: String },
    #[error("could not write {path}: {reason}")]
    Write { path: String, reason: String },
    #[error("{path} is not valid fleet state: {reason}")]
    Corrupt { path: String, reason: String },
}

/// Why the fleet passphrase came out of the drawer.
///
/// ADR-0012: once an approver exists, using the passphrase is an alarm rather than routine,
/// which only works if every use is recorded. The delivery-plane notification is phase 5;
/// this is the durable half, and it is the half that survives a missed notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassphraseUse {
    /// The fleet was founded here.
    Init,
    /// This node enrolled itself without asking an approver.
    Join,
    /// A grant was added to this node's certificate.
    Grant,
    /// A member was revoked.
    Revoke,
    /// The passphrase was checked against the fleet, granting nothing.
    Verify,
    /// The fleet was re-founded under a new passphrase (ADR-0012 mitigation 5).
    Rekey,
}

impl std::fmt::Display for PassphraseUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PassphraseUse::Init => "init",
            PassphraseUse::Join => "join",
            PassphraseUse::Grant => "grant",
            PassphraseUse::Revoke => "revoke",
            PassphraseUse::Verify => "verify",
            PassphraseUse::Rekey => "rekey",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UseRecord {
    pub at: Millis,
    pub what: PassphraseUse,
}

/// What `FleetState::revoke` did.
///
/// A revocation is idempotent and monotonic — nothing here un-revokes a device — so revoking
/// twice is not an error and must still exit 0. What it may not do is claim the act a second
/// time: `offload revoke` printed the whole paragraph about what had just been recorded and
/// what the daemon was about to hang up on, for a call that wrote nothing. Same shape as
/// `Removal::{Done, Already, Missing}` one tier over, with no `Missing`: a device this fleet
/// has never met is a legal thing to revoke, and that is `Now`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Revoked {
    /// Filed here now. It gossips, and a daemon on this device hangs up within a second.
    Now(Revocation),
    /// Already revoked, and this is the revocation that did it — the **first**, which is the one
    /// the rest of the fleet holds.
    Already(Revocation),
}

impl Revoked {
    /// The revocation in force for that member, however it got there.
    #[must_use]
    pub fn revocation(&self) -> &Revocation {
        match self {
            Revoked::Now(r) | Revoked::Already(r) => r,
        }
    }
}

/// What [`FleetState::adopt`] did with a certificate somebody offered this node, and why.
///
/// A `bool` here answered "did anything change" and threw away the measurement that decided it,
/// which is how a certificate granting *less* came to be called an improvement: the test was
/// `expires_at`, and nothing downstream had anything left to word a sentence with. Measured in
/// session seventy-eight — `offload invite <a node that already holds host-runs>` needs no
/// passphrase, and the device taking that token up was told `Took up a new certificate` with a
/// `grants` line identical to the one it had printed nine seconds earlier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Adopted {
    /// Taken up: it is ours, it verifies, it takes nothing away, and it expires later.
    Taken,
    /// Ours and verified, and it buys nothing — it expires no later than what is held. Adopting
    /// it anyway would mean a peer could replace this node's certificate whenever it liked.
    NoBetter,
    /// Ours and verified, and it would take grants away. Refused, because nothing here asks for
    /// that: a renewal restates the grants it descends from, and `offload invite` exists to widen
    /// a certificate. Narrowing one is `offload revoke`, which says what it is doing.
    Narrower {
        /// What the offered certificate leaves out, in the order `Grant` declares them.
        lost: Vec<Grant>,
    },
}

impl Adopted {
    /// Whether the certificate held by this node changed.
    #[must_use]
    pub fn took_it(&self) -> bool {
        matches!(self, Adopted::Taken)
    }

    /// What was left out, phrased for somebody reading a refusal.
    #[must_use]
    pub fn lost(&self) -> String {
        match self {
            Adopted::Narrower { lost } => lost
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetState {
    /// The fleet's public key: the one thing every verification starts from.
    pub fleet: FleetId,
    /// This node's own certificate.
    pub membership: MembershipCert,
    /// The delegation authorising whoever issued `membership`. `None` when the fleet key
    /// signed it directly, which is every path that does not go through an approver.
    pub chain: Option<Delegation>,
    /// The delegation naming *this* node as an approver, if it is one. Presented alongside
    /// certificates this node issues, so a peer can check the chain without asking anybody.
    pub approver: Option<Delegation>,
    /// Revocations heard so far. Not "the revoked set": a revocation is a signed fact that
    /// travels, and keeping the signature is what lets this node pass it on.
    #[serde(default)]
    pub revocations: Vec<Revocation>,
    /// Every time the passphrase came out of the drawer.
    #[serde(default)]
    pub passphrase_uses: Vec<UseRecord>,
    /// Members a person, at this approver, decided to re-approve (ADR-0069 §3), waiting for each
    /// to ask. Only ever acted on here, where the decision was made, and never gossiped: it is a
    /// person's standing instruction to this device, not a fact about the fleet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reapprovals: Vec<Reapproval>,
}

/// A person's decision to re-approve one member, made with `offload reapprove` on an approver.
///
/// A decision rather than a certificate, because the certificate to restate is the one the member
/// carries when it next asks, and that may be a renewal newer than anything this node has seen.
/// It stands for [`REAPPROVAL_WINDOW`] and applies while it is newer than the approval the member
/// holds, so it needs no bookkeeping: the re-approval it produces is what makes it moot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reapproval {
    pub member: NodeId,
    pub name: String,
    pub decided_at: Millis,
}

impl FleetState {
    /// Found a fleet on this node: the founding certificate, plus the delegation that keeps
    /// the passphrase out of the daily path afterwards (ADR-0012).
    #[must_use]
    pub fn found(key: &FleetKey, node: NodeId, name: &str, now: Millis) -> FleetState {
        let fleet = key.id();
        FleetState {
            fleet,
            membership: MembershipCert::issue(
                key.signing_key(),
                Issuer::Fleet,
                Terms::founding(fleet, node, name, now),
            ),
            chain: None,
            approver: Some(Delegation::issue(
                key.signing_key(),
                fleet,
                node,
                now,
                DELEGATION_LIFETIME,
                now.0,
            )),
            revocations: Vec::new(),
            passphrase_uses: vec![UseRecord {
                at: now,
                what: PassphraseUse::Init,
            }],
            reapprovals: Vec::new(),
        }
    }

    /// [`Self::found`], with the founding delegation naming a key the host holds in secure
    /// hardware (ADR-0069 §4): a fleet founded on a phone approves with that key from its first
    /// day, and the passphrase — in hand only now — never has to come out again to say so.
    #[must_use]
    pub fn found_with_key(
        key: &FleetKey,
        node: NodeId,
        name: &str,
        now: Millis,
        issuer_key: IssuerKey,
    ) -> FleetState {
        let mut state = Self::found(key, node, name, now);
        state.approver = Some(Delegation::issue_with_key(
            key.signing_key(),
            state.fleet,
            node,
            issuer_key,
            now,
            DELEGATION_LIFETIME,
            now.0,
        ));
        state
    }

    /// Enrol this node into an existing fleet using the passphrase directly.
    ///
    /// The recovery path, not the ordinary one: it needs nothing else to be awake, which is
    /// why it exists and why what it grants is deliberately thin — `Submit` and `Deliver`,
    /// with `HostRuns` withheld and probation running.
    #[must_use]
    pub fn join(key: &FleetKey, node: NodeId, name: &str, now: Millis) -> FleetState {
        let fleet = key.id();
        FleetState {
            fleet,
            membership: MembershipCert::issue(
                key.signing_key(),
                Issuer::Fleet,
                Terms::joining(fleet, node, name, now),
            ),
            chain: None,
            approver: None,
            revocations: Vec::new(),
            passphrase_uses: vec![UseRecord {
                at: now,
                what: PassphraseUse::Join,
            }],
            reapprovals: Vec::new(),
        }
    }

    /// Re-issue this node's certificate with `grant` added.
    ///
    /// ADR-0012 measures probation from the certificate carrying the grant, not from when the
    /// device first appeared — **probation follows the grant, not the path**, which is that ADR's
    /// own amendment to mitigation 2 and has no exemption for the passphrase, because somebody
    /// holding the passphrase is the threat it was written for.
    ///
    /// So the flag is not simply inherited. Inheriting was right for a node still serving one and
    /// wrong for the ordinary case: an *invited* device carries `probation = false` for the
    /// door's reason — an existing member deliberately enrolled it — and `offload grant
    /// host-runs` there therefore took effect the same second, on the enrolment path this whole
    /// ADR exists to make the normal one. Mitigation 4's alarm buys fifteen minutes to revoke in,
    /// and there were none. A grant already in force is never re-probated: a founder granted
    /// `approve` must not have its hosting suspended for a quarter of an hour by an unrelated
    /// act.
    pub fn add_grant(
        &mut self,
        key: &FleetKey,
        grant: Grant,
        now: Millis,
        issuer_key: Option<IssuerKey>,
    ) -> Result<(), FleetError> {
        self.expect_key(key)?;
        let mut grants: BTreeSet<Grant> = self.membership.grants.clone();
        grants.insert(grant);
        // Said as the rule rather than as a flag to carry forward: the new certificate probates
        // exactly when it puts `HostRuns` in play that is not in force already. A grant this node
        // is still serving out counts as not in force, so the window restarts — the same answer
        // renewal gives, and the safe direction.
        let probation =
            grants.contains(&Grant::HostRuns) && !self.membership.granted(Grant::HostRuns, now);
        self.membership = MembershipCert::issue(
            key.signing_key(),
            Issuer::Fleet,
            Terms {
                grants,
                issued_at: now,
                lifetime: CERT_LIFETIME,
                serial: now.0,
                probation,
                fleet: self.fleet,
                member: self.membership.member,
                name: self.membership.name.clone(),
                // The fleet key is signing, so this certificate *is* the authority. Anything
                // renewed from it carries it (`MembershipCert::renewal`), which is what lets an
                // approver keep a host node alive without being able to create one.
                authority: None,
                approved_at: None,
                proof: None,
            },
        );
        // …and the delegation that makes `approve` usable, whenever the certificate grants it.
        // Nothing but `init` issued one, so `offload grant approve` on a second device produced
        // "granted `approve` with no delegation to prove it" — the second approver every note
        // recommends could not exist — and a founder's delegation ran out a year after `init`
        // with no way to renew it. The passphrase is in hand here, which is what a delegation
        // needs, so re-running `offload grant approve` is the sole approver's yearly step: a
        // fresh approval of this node and a fresh year to issue in (ADR-0069's step-3
        // amendment). Only for this node, and only when it is granted `approve` — a grant of
        // anything else leaves the delegation as it was.
        if self.membership.grants.contains(&Grant::Approve) {
            // The key named, else the one the delegation already names: re-running this every
            // year must not quietly move a hardware-backed approver back onto its node key.
            let issuer_key = issuer_key
                .or_else(|| self.approver.as_ref().map(|d| d.issuer_key.clone()))
                .unwrap_or_default();
            self.approver = Some(Delegation::issue_with_key(
                key.signing_key(),
                self.fleet,
                self.membership.member,
                issuer_key,
                now,
                DELEGATION_LIFETIME,
                now.0,
            ));
        }
        self.record(PassphraseUse::Grant, now);
        Ok(())
    }

    /// Re-issue a peer's certificate under this node's delegation (ADR-0012).
    ///
    /// The backstop this ADR relies on twice over — "certificates expire in ~30 days and are
    /// renewed on contact" is what makes a revocation that never arrives eventually take
    /// effect, and what makes a lost device fall out of the fleet by itself. Nothing renewed
    /// anything for two phases, which meant the fleet was not converging on safety after a
    /// month; it was converging on *nobody being a member*, all at once, with the passphrase as
    /// the only way back on every device.
    ///
    /// `cert` is the one the handshake authenticated. Renewal changes nothing but time, so this
    /// restates a decision somebody already made rather than making one — which is exactly what
    /// justifies its happening automatically, in the background, with nobody at a keyboard.
    ///
    /// Every refusal is a sentence rather than a `None`, because the asking node tries peers in
    /// turn and "most members are not approvers" has to be distinguishable from "your papers
    /// are wrong".
    pub fn renew_for(
        &self,
        signing: &ed25519_dalek::SigningKey,
        me: NodeId,
        cert: &MembershipCert,
        delegation: Option<&Delegation>,
        now: Millis,
    ) -> Result<Credentials, String> {
        if cert.fleet != self.fleet {
            return Err(format!(
                "that certificate is for fleet {}, this is {}",
                cert.fleet.short(),
                self.fleet.short()
            ));
        }
        if self.is_revoked(cert.member) {
            // Unreachable through an admitted connection, and checked anyway: the day this
            // becomes reachable is the day a revoked device is handed fresh papers by a
            // background loop nobody is watching.
            return Err(format!("{} has been revoked", cert.member.short()));
        }
        // What is restated has to be genuine, whoever carried it (ADR-0069 §2): since `RenewMe`
        // carries the asker's current certificate rather than leaning on the handshake's, a
        // restatement of unchecked papers would be minting. Expiry is left to the sentence below.
        match cert.verify(self.fleet, delegation, now) {
            Ok(()) | Err(offload_core::MembershipError::Expired { .. }) => {}
            Err(e) => return Err(format!("that certificate does not verify: {e}")),
        }
        if cert
            .renewer()
            .is_some_and(|renewer| self.is_revoked(renewer))
        {
            return Err("that certificate was renewed by a revoked member".into());
        }
        if cert.is_expired(now) {
            return Err("that certificate has already lapsed — re-join with the passphrase".into());
        }
        // A person's decision here outranks the renewal schedule: a member whose approval is in
        // its last month asks before its certificate is due, and this is who it is asking.
        let reapprove = self.stands_for(cert, now);
        if !cert.is_due_for_renewal(now) && !reapprove {
            // Bounds the work rather than protecting anything: a renewer that re-signed on
            // request would sign for every peer on every probe.
            return Err(if cert.is_due_for_reapproval(now) {
                "that membership's approval runs out within a month, and nobody here has decided \
                 to re-approve it — `offload reapprove` on an approver"
                    .to_string()
            } else {
                "that certificate is not due for renewal yet".to_string()
            });
        }
        if cert.member == me {
            // A node that could re-sign itself would never fall out of a fleet it was removed
            // from (ADR-0012); peers refuse the result anyway, and this says so first.
            return Err("no node renews its own membership".into());
        }
        // An approver renews as it always has. Anyone else holding `Renew` renews as a renewer
        // (ADR-0069 §1–2), which is what lets a fleet renew itself with no approver up.
        let approver = self
            .approver
            .clone()
            .filter(|d| self.membership.granted(Grant::Approve, now) && d.approver == me)
            .filter(|d| d.verify(self.fleet, now).is_ok());
        let Some(delegation_for_me) = approver else {
            // A decision left over from when this node was an approver re-approves nothing, and
            // must not let a renewer renew early either.
            if !cert.is_due_for_renewal(now) {
                return Err("that certificate is not due for renewal yet".into());
            }
            return self.renew_as_renewer(signing, me, cert, delegation, now);
        };
        let members_delegation = delegation;
        let delegation = delegation_for_me;
        if !delegation.issuer_key.is_node() {
            // A hardware approval key needs a person at the device for every signature, and this
            // runs in the background whenever a peer asks. So routine renewals go the renewer's
            // way — the node key under `Renew`, which is what renewal is for (ADR-0069 §1) — and
            // the hardware key is kept for the acts a person makes.
            if reapprove {
                // The daemon files the request for a person and serves the answer when it comes
                // (`hardware_reapproval_for`, `NodeMembership::renew`); this path cannot sign.
                return Err(
                    "the re-approval waits for a person to confirm it on this device".into(),
                );
            }
            if !cert.is_due_for_renewal(now) {
                return Err("that certificate is not due for renewal yet".into());
            }
            return self.renew_as_renewer(signing, me, cert, members_delegation, now);
        }
        let terms = if reapprove {
            cert.reapproval(now, CERT_LIFETIME)
        } else {
            cert.renewal(now, CERT_LIFETIME)
        };
        Ok(Credentials {
            membership: MembershipCert::issue(signing, Issuer::Approver { node: me }, terms),
            delegation: Some(delegation),
        })
    }

    /// The re-approval a person has to sign in hardware (ADR-0069 §4), when one is due: a
    /// decision made here stands for `cert`, this node approves with a key in secure hardware,
    /// and everything `renew_for` checks before signing holds. Unsigned; the host signs it after
    /// a person confirms, and [`crate::approval_key::answered`] hands it back.
    #[must_use]
    pub fn hardware_reapproval_for(
        &self,
        me: NodeId,
        cert: &MembershipCert,
        delegation: Option<&Delegation>,
        now: Millis,
    ) -> Option<(MembershipCert, Delegation)> {
        let mine = self
            .approver
            .clone()
            .filter(|d| self.membership.granted(Grant::Approve, now) && d.approver == me)
            .filter(|d| d.verify(self.fleet, now).is_ok())
            .filter(|d| !d.issuer_key.is_node())?;
        let genuine = matches!(
            cert.verify(self.fleet, delegation, now),
            Ok(()) | Err(offload_core::MembershipError::Expired { .. })
        );
        let ok = cert.fleet == self.fleet
            && genuine
            && !cert.is_expired(now)
            && cert.member != me
            && !self.is_revoked(cert.member)
            && !cert.renewer().is_some_and(|r| self.is_revoked(r))
            && self.stands_for(cert, now);
        ok.then(|| {
            (
                MembershipCert::unsigned(
                    Issuer::Approver { node: me },
                    cert.reapproval(now, CERT_LIFETIME),
                ),
                mine,
            )
        })
    }

    /// Does a decision made here stand for re-approving the holder of `cert` now?
    fn stands_for(&self, cert: &MembershipCert, now: Millis) -> bool {
        self.reapprovals.iter().any(|r| {
            r.member == cert.member
                && r.decided_at > cert.approved_at()
                && now.saturating_sub(r.decided_at) <= REAPPROVAL_WINDOW
        })
    }

    /// Record a person's decision to re-approve `member` (ADR-0069 §3), to be acted on when it
    /// next asks. Refused, in words, for what this node could never act on: itself, a revoked
    /// member, or anything at all when this node is not an approver.
    pub fn decide_reapproval(
        &mut self,
        me: NodeId,
        member: NodeId,
        name: &str,
        now: Millis,
    ) -> Result<(), String> {
        if member == me {
            return Err(
                "no node re-approves itself: run `offload reapprove` for this device \
                        on another approver, or use the passphrase"
                    .into(),
            );
        }
        if self.is_revoked(member) {
            return Err(format!("{name} ({}) has been revoked", member.short()));
        }
        let approver = self
            .approver
            .as_ref()
            .filter(|d| self.membership.granted(Grant::Approve, now) && d.approver == me)
            .is_some_and(|d| d.verify(self.fleet, now).is_ok());
        if !approver {
            return Err(
                "this node is not an approver, so a decision here would never be acted on \
                        — run it on a device holding `approve`"
                    .into(),
            );
        }
        self.reapprovals.retain(|r| {
            r.member != member && now.saturating_sub(r.decided_at) <= REAPPROVAL_WINDOW
        });
        self.reapprovals.push(Reapproval {
            member,
            name: name.to_string(),
            decided_at: now,
        });
        Ok(())
    }

    /// A renewer's renewal (ADR-0069 §2): the member's approval and this node's own, both carried,
    /// signed with this node's key. Refused, in words, where the rules would refuse it anyway —
    /// a node without `Renew`, or one whose own approval is over a year old.
    fn renew_as_renewer(
        &self,
        signing: &ed25519_dalek::SigningKey,
        me: NodeId,
        cert: &MembershipCert,
        delegation: Option<&Delegation>,
        now: Millis,
    ) -> Result<Credentials, String> {
        let (mine, my_delegation) = self.membership.approval_of(self.chain.as_ref());
        if !mine.grants.contains(&Grant::Renew) {
            return Err(
                "this node may not renew: it is not an approver and was not granted renew".into(),
            );
        }
        if now.saturating_sub(mine.approved_at()) > offload_core::fleet::APPROVAL_LIFETIME {
            return Err(
                "this node's own approval is over a year old, so it may renew nobody until a \
                 person re-approves it"
                    .into(),
            );
        }
        let (approval, approval_delegation) = cert.approval_of(delegation);
        let proof = offload_core::RenewalProof {
            approval,
            approval_delegation,
            renewer: mine,
            renewer_delegation: my_delegation,
        };
        let renewed = MembershipCert::issue(
            signing,
            Issuer::Renewer { node: me },
            cert.renewal_by_renewer(proof, now, CERT_LIFETIME),
        );
        // Checked here rather than left to the asking node, which would only learn "no" with no
        // reason: a renewal this node would itself refuse is not worth sending.
        renewed
            .verify(self.fleet, None, now)
            .map_err(|e| format!("this node's renewal would not verify: {e}"))?;
        Ok(Credentials {
            membership: renewed,
            delegation: None,
        })
    }

    /// Re-found this fleet under a new key, keeping this device and everything it owns.
    ///
    /// ADR-0012 mitigation 5, the convergent revocation. Ordinary revocation is eventually
    /// consistent and, in the worst case, outrunnable: it has to *reach* every node, and a
    /// device that stays out of contact keeps its certificate until it lapses. Rekeying is not
    /// a message at all. The evicted device is not revoked, it is simply not in the new fleet,
    /// and no gossip has to arrive anywhere for that to be true.
    ///
    /// What survives is everything this device *owns* — its node identity, its runs, mirrors,
    /// worktrees and blobs — because ADR-0012 forbids the fleet key from encrypting anything
    /// for exactly this reason: losing or replacing it must cost membership and nothing else.
    ///
    /// What does not survive is the revocation list, and that is not an oversight. A revocation
    /// is a statement about a certificate in the *old* fleet, and no certificate in the old
    /// fleet means anything here. Carrying them forward would be keeping a list of devices that
    /// are already excluded by not being members.
    #[must_use]
    pub fn refound(&self, key: &FleetKey, now: Millis) -> FleetState {
        let mut state = FleetState::found(key, self.membership.member, &self.membership.name, now);
        // The record of the passphrase coming out of the drawer follows the device rather than
        // the fleet: it is this machine's own history, and the whole value of it is being able
        // to say "the last time this was used was in March".
        state.passphrase_uses = self.passphrase_uses.clone();
        state.record(PassphraseUse::Rekey, now);
        state
    }

    /// Issue a certificate for *another* device, to be carried to it (ADR-0012's `invite`).
    ///
    /// Under this node's delegation, so it needs no passphrase — which is the whole point of
    /// having approvers: enrolling a device is a thing that happens on a Sunday, and a fleet
    /// whose only enrolment path is a secret in a drawer is a fleet where the drawer gets
    /// opened routinely.
    ///
    /// What it may hand out is [`offload_core::default_grants`] and nothing more. `HostRuns` is
    /// the grant that turns membership into "runs agents on your repositories with your
    /// credentials", and ADR-0012 mitigation 1 keeps it behind the passphrase — see
    /// [`FleetState::issue_for`], which is the same act with the root doing the signing.
    ///
    /// **No probation**, because an existing member deliberately did this: probation exists so
    /// that an enrolment nobody performed is noticed before it can execute anything, and there
    /// is nothing to notice about one somebody typed. That is the invite path's real purpose
    /// and the reason to keep it now that joining no longer needs it.
    pub fn invite(
        &self,
        signing: &ed25519_dalek::SigningKey,
        hardware: Option<&dyn crate::approval_key::HardwareSigner>,
        member: NodeId,
        name: &str,
        now: Millis,
    ) -> Result<Credentials, String> {
        let me = self.membership.member;
        if !self.membership.granted(Grant::Approve, now) {
            return Err("this node is not an approver — `offload grant approve` first".into());
        }
        let Some(delegation) = self.approver.clone() else {
            return Err("this node is an approver with no delegation to prove it".into());
        };
        delegation
            .verify(self.fleet, now)
            .map_err(|e| format!("this node's delegation is not usable: {e}"))?;
        if self.is_revoked(member) {
            return Err(format!("{} has been revoked", member.short()));
        }
        let terms = Terms {
            probation: false,
            ..Terms::joining(self.fleet, member, name, now)
        };
        let issuer = Issuer::Approver { node: me };
        let membership = match &delegation.issuer_key {
            IssuerKey::Node => MembershipCert::issue(signing, issuer, terms),
            // ADR-0069 §4: this approver approves with a key in secure hardware, which the host
            // holds and a person unlocks. The node key signing instead would be refused by every
            // peer — the delegation names the other key — so it is not tried.
            IssuerKey::P256 { .. } => {
                let Some(hardware) = hardware else {
                    return Err(
                        "this approver's key is held in secure hardware, and nothing here \
                                can reach it — run this on the device whose app holds it"
                            .into(),
                    );
                };
                let unsigned = MembershipCert::unsigned(issuer, terms);
                let signature = hardware.sign(&unsigned)?;
                unsigned.signed(signature)
            }
        };
        // Checked before it is handed out: a signature from the wrong key — a key the host made
        // again, a request answered by something else — is caught here, in words, rather than as
        // a refused handshake on a device somebody carried the token to.
        membership
            .verify(self.fleet, Some(&delegation), now)
            .map_err(|e| format!("the signed invitation does not verify: {e}"))?;
        Ok(Credentials {
            membership,
            delegation: Some(delegation),
        })
    }

    /// The same, signed by the fleet key, and therefore able to grant anything.
    ///
    /// The passphrase path exists here for one grant: `HostRuns`, which ADR-0012 keeps behind
    /// the root deliberately. Every other grant an approver can hand out, and this is also how
    /// a device that is already a member is handed a wider certificate without somebody having
    /// to walk to it.
    ///
    /// **Probation follows `HostRuns` rather than the path**, which is the invite exemption read
    /// carefully. Probation only ever suppresses `HostRuns` — a certificate without it that sets
    /// the flag delays nothing — so "an invite skips probation" is a statement about the door,
    /// where the grant is not in play. Mitigation 2 is about the grant, and it has no exemption
    /// for the passphrase, because an attacker holding the passphrase is the threat it was
    /// written for. Skipping it here would have made this command the way around it.
    pub fn issue_for(
        &self,
        key: &FleetKey,
        member: NodeId,
        name: &str,
        grants: BTreeSet<Grant>,
        now: Millis,
    ) -> Result<Credentials, FleetError> {
        self.expect_key(key)?;
        if self.is_revoked(member) {
            return Err(FleetError::Invalid(MembershipError::Revoked {
                node: member.short(),
            }));
        }
        Ok(Credentials {
            membership: MembershipCert::issue(
                key.signing_key(),
                Issuer::Fleet,
                Terms {
                    probation: grants.contains(&Grant::HostRuns),
                    grants,
                    ..Terms::joining(self.fleet, member, name, now)
                },
            ),
            delegation: None,
        })
    }

    /// Take up a certificate an approver re-issued for this node.
    ///
    /// Verified against the fleet key like anything else, because who handed it over decides
    /// nothing: an approver's signature is only good with the fleet-signed delegation beside
    /// it, and a peer that could skip that check would be a peer any member could re-paper.
    ///
    /// Refuses anything that is not an improvement, and an improvement is measured on **both**
    /// axes a certificate has. A renewal is a fresh clock and nothing else, so one that expires
    /// no later than what is already held has bought nothing — and adopting it would mean a
    /// certificate this node holds could be *replaced* by a peer's choosing, which is a much
    /// larger thing than renewal.
    ///
    /// The second axis is the grants, and it used to be unchecked. `expires_at` alone called a
    /// later-expiring certificate an improvement however little it granted, so the two paths
    /// that reach here could both take a grant away silently:
    ///
    /// - `offload invite <node>` under an approver's delegation needs **no passphrase** and
    ///   issues [`offload_core::default_grants`]. Offered to a device already holding
    ///   `HostRuns`, it expires later and grants less, and the device took it up.
    /// - the renewal loop in `mesh.rs` calls this with whatever an approver peer answered
    ///   `RenewMe` with, so the same narrowing arrives over the network with nobody pasting
    ///   anything.
    ///
    /// Which makes it the mirror of the bound ADR-0012 mitigation 1 already puts on an approver:
    /// it may not *mint* `HostRuns`, and until now it could strip it. Neither path ever wants to
    /// narrow — `renew_for` restates the grants it descends from, and `invite` exists to widen —
    /// so a narrower certificate is a mistake or a downgrade, and is refused by name.
    pub fn adopt(&mut self, credentials: Credentials, now: Millis) -> Result<Adopted, FleetError> {
        let cert = credentials.membership;
        if cert.member != self.membership.member {
            return Err(FleetError::NotOurs {
                member: cert.member.short(),
                node: self.membership.member.short(),
            });
        }
        cert.verify(self.fleet, credentials.delegation.as_ref(), now)?;
        // A renewal leans on its renewer's standing (ADR-0069 §2): one from a member this node
        // knows is revoked is refused here, as the handshake refuses it from a peer.
        if let Some(renewer) = cert.renewer() {
            if self.is_revoked(renewer) {
                return Err(FleetError::Invalid(
                    offload_core::MembershipError::NotARenewer {
                        renewer: renewer.short(),
                    },
                ));
            }
        }
        // The certificates' own grants, not `grants(now)`: probation suppresses `HostRuns`
        // without removing it, so comparing what is *in force* would read a certificate offered
        // during a probation window as taking away the very grant it is there to carry.
        let lost: Vec<Grant> = self
            .membership
            .grants
            .difference(&cert.grants)
            .copied()
            .collect();
        if !lost.is_empty() {
            return Ok(Adopted::Narrower { lost });
        }
        // A *wider* certificate is an improvement whatever its clock says, and this arm is here
        // because the first draft did not have one: it argued that every issuing path stamps
        // `now`, so anything carrying a new grant was minted later and expires later. That is
        // true of one machine and false of two. The issuer is a different device — that is the
        // whole point of `offload invite` — and a few seconds of clock skew behind this node is
        // enough to make a deliberately widened certificate expire *earlier* than what is held,
        // which the expiry test alone would refuse as `NoBetter`. Refusing a grant somebody
        // typed the passphrase to issue, with a sentence saying it buys nothing, is the same
        // class of wrong as the narrowing this function now catches. Caught by the test below
        // rather than by reading, which is the only reason it is written down.
        if cert.grants.is_superset(&self.membership.grants)
            && cert.grants.len() > self.membership.grants.len()
        {
            self.membership = cert;
            self.chain = credentials.delegation;
            return Ok(Adopted::Taken);
        }
        // A newer approval is an improvement whatever its expiry (ADR-0069 §3), for the widening
        // arm's reason above: a re-approval issued on a machine a few seconds behind this one can
        // expire a moment earlier than the renewal it replaces, and refusing it as `NoBetter`
        // would throw away the one thing that keeps this membership past its year.
        if cert.approved_at() > self.membership.approved_at() {
            self.membership = cert;
            self.chain = credentials.delegation;
            return Ok(Adopted::Taken);
        }
        if cert.expires_at <= self.membership.expires_at {
            return Ok(Adopted::NoBetter);
        }
        self.membership = cert;
        self.chain = credentials.delegation;
        Ok(Adopted::Taken)
    }

    /// Sign a revocation for `member` and file it here.
    ///
    /// Answers what it *did*, not what it was asked to do: a device already revoked is left with
    /// the revocation it already has, and the caller is told so rather than being handed a fresh
    /// one to announce. Revoking twice is a thing people do — out of caution, or because the
    /// first attempt's output scrolled past — and the second time must not read like the first.
    pub fn revoke(
        &mut self,
        key: &FleetKey,
        member: NodeId,
        now: Millis,
    ) -> Result<Revoked, FleetError> {
        self.expect_key(key)?;
        // Asked before anything is signed, because a revocation minted and then dropped is a
        // signature over a fact this fleet has already stated differently.
        if let Some(existing) = self.revocations.iter().find(|r| r.covers(member)) {
            return Ok(Revoked::Already(existing.clone()));
        }
        let revocation = Revocation::issue(key.signing_key(), self.fleet, member, now, now.0);
        self.file(revocation.clone())?;
        self.record(PassphraseUse::Revoke, now);
        Ok(Revoked::Now(revocation))
    }

    /// Accept a revocation from anywhere — signed by the fleet key, so where it came from
    /// does not matter. Unlike liveness, its subject never gets to argue (ADR-0012).
    ///
    /// Deduplicated by **member**, which is the question `Revocation::covers` answers and
    /// therefore the only one anything downstream decides on. It used to be `(member, serial)`,
    /// which is right for the case it was written for — a peer relaying a copy we already hold —
    /// and could never fire for the other one, because `revoke` puts the clock in the serial. So
    /// a second `offload revoke` of one device appended a second record that changed nothing,
    /// gossiped for ever and made `revoked N device(s)` a count of records. Measured on two
    /// meshed daemons: the node the command was typed on said **5**, the node that merely heard
    /// it said **2**, and the one that heard it was right — `NodeMembership::file` has always
    /// asked `is_revoked(member)` one layer up. Two dedup rules for one fact, and the stricter
    /// one guarded the path nobody types.
    ///
    /// The record kept is the **first**, for the reason a tombstone keeps the earlier instant
    /// (ADR-0056): `revoked_at` is when the device stopped being a member, and the earlier answer
    /// is the one every node that already heard it holds.
    pub fn file(&mut self, revocation: Revocation) -> Result<(), FleetError> {
        revocation.verify(self.fleet)?;
        if !self.is_revoked(revocation.member) {
            self.revocations.push(revocation);
        }
        Ok(())
    }

    #[must_use]
    pub fn is_revoked(&self, node: NodeId) -> bool {
        self.revocations.iter().any(|r| r.covers(node))
    }

    pub fn record(&mut self, what: PassphraseUse, at: Millis) {
        self.passphrase_uses.push(UseRecord { at, what });
    }

    /// When the passphrase was last checked against the fleet, if ever.
    ///
    /// ADR-0012 wants this visible: a recovery secret nobody has tested is not a recovery
    /// secret, and the failure is silent until the day it is not.
    #[must_use]
    pub fn last_verified(&self) -> Option<Millis> {
        self.passphrase_uses
            .iter()
            .filter(|u| u.what == PassphraseUse::Verify)
            .map(|u| u.at)
            .max()
    }

    /// Did this device re-found its own fleet, and when?
    ///
    /// The local discriminator for a refused handshake (ADR-0060). Both ends of a rekey are
    /// turned away by the other — the evicted device's certificate is for the old fleet, and
    /// the rekeying device's is for a fleet the evicted one has never heard of — so the
    /// refusal itself cannot say which end you are on. This can, and it does it from **this
    /// device's own action** rather than from anything a peer said, which is the only kind of
    /// evidence admissible about oneself here.
    #[must_use]
    pub fn last_rekeyed(&self) -> Option<Millis> {
        self.passphrase_uses
            .iter()
            .filter(|u| u.what == PassphraseUse::Rekey)
            .map(|u| u.at)
            .max()
    }

    /// Is this node's own membership good right now?
    ///
    /// The same check a peer would make, run against ourselves — which is the point: a node
    /// whose certificate has lapsed should say so locally rather than discover it in a
    /// handshake refusal.
    pub fn check(&self, node: NodeId, now: Millis) -> Result<(), FleetError> {
        if self.membership.member != node {
            return Err(FleetError::NotOurs {
                member: self.membership.member.short(),
                node: node.short(),
            });
        }
        if self.is_revoked(node) {
            return Err(FleetError::Invalid(MembershipError::Revoked {
                node: node.short(),
            }));
        }
        self.membership
            .verify(self.fleet, self.chain.as_ref(), now)?;
        Ok(())
    }

    /// What this node may actually do right now, probation included.
    ///
    /// Nothing at all once this node has been revoked (ADR-0044). The certificate still lists
    /// its grants and still verifies — that is what makes revocation the exception it is — so a
    /// reader that asked only the certificate got the pre-revocation answer, and every door that
    /// asks this one stayed open: `offload status` said `accepting yes`, the bid and the grant
    /// passed their `host-runs` check, and `offload run` submitted. Read here rather than at
    /// each of them, because "may this node do X" is one question and it has one answer.
    #[must_use]
    pub fn grants(&self, now: Millis) -> BTreeSet<Grant> {
        if self.is_revoked(self.membership.member) {
            return BTreeSet::new();
        }
        self.membership.effective_grants(now)
    }

    fn expect_key(&self, key: &FleetKey) -> Result<(), FleetError> {
        if key.is(self.fleet) {
            Ok(())
        } else {
            Err(FleetError::WrongPassphrase {
                expected: self.fleet.short(),
                derived: key.id().short(),
            })
        }
    }
}

/// How this fleet is doing, for `offload status` (ADR-0012 mitigation 6).
///
/// Reported without being asked for, because the two states that turn a bad day into a bad week
/// are both silent: a fleet with no approver is one where the passphrase is the working
/// credential, and a fleet with one approver is one enrolment away from a recovery. Neither
/// produces an error until the day it matters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetHealth {
    pub fleet: String,
    /// Members this node has actually handshaken with, itself included.
    ///
    /// **Met, not known.** Grants live on a certificate precisely because a node's claims about
    /// itself are not to be believed, so these counts come from what handshakes proved rather
    /// than from gossip — which means a device that has been shut all week is not in them. The
    /// caveat is stated in the output rather than smoothed over: a nag that quietly stopped
    /// counting an approver would send somebody to enrol one they already have.
    pub members_met: u32,
    pub approvers_met: u32,
    /// Days until this node's own certificate lapses.
    ///
    /// Meaningless once `revoked_here` is set, and the reader must say so rather than print it:
    /// a revoked certificate does not lapse in thirty days, it is unusable now.
    pub cert_days: u64,
    /// Has **this** node been revoked from the fleet it is reporting on (ADR-0044)?
    ///
    /// ADR-0060 gave this report the rekey half of the same question and deliberately made it a
    /// *count* of refused handshakes, because a rekey revokes nobody and its refusal carries no
    /// proof. This half is the opposite and is why it is a `bool`: a revocation is signed by the
    /// fleet key and filed here after verifying, so it is one of the few claims about itself a
    /// node here may act on — and `Supervisor` already does, which is what makes the `accepting`
    /// line honest while every line under it said the fleet was fine.
    ///
    /// Control socket, defaulted: a daemon too old to send it renders the old sentence rather
    /// than a wrong one.
    #[serde(default)]
    pub revoked_here: bool,
    /// Peers whose certificates are running out, as "name in N days".
    pub expiring: Vec<String>,
    /// Memberships, this node's included, whose approval runs out within
    /// [`REAPPROVAL_WINDOW`] (ADR-0069 §3): the list `offload reapprove` works from.
    ///
    /// Structured rather than sentences, because `offload reapprove --due` acts on it — a list a
    /// command acts on has to be the list the person was shown, not a second reading of it.
    #[serde(default)]
    pub reapproval_due: Vec<ApprovalDue>,
    /// How long since the passphrase was last checked against the fleet, if ever.
    pub passphrase_days: Option<u64>,
    pub revoked: u32,
    /// The nags. Sentences rather than flags, because each one has a different thing to do
    /// about it and a boolean would need the sentence written somewhere else anyway.
    pub notes: Vec<String>,
}

/// One membership whose approval is in its last month, as [`health`] saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalDue {
    pub node: NodeId,
    pub name: String,
    /// Whole days until the approval runs out, rounded down: `0` means within the day.
    pub days: u64,
    /// A person already decided, on this node, to re-approve it; it has not asked since.
    pub decided: bool,
}

/// What the caller could have met, which decides what its report may claim.
///
/// An empty slice used to mean both of these and they are not the same sentence: a daemon that
/// has met nobody *knows* there is no approver about, while `offload fleet` runs with no daemon
/// and no network on purpose (ADR-0012) and knows nothing of the kind. Told apart by the caller
/// rather than by the data, because the difference is in who is asking — the CLI's `&[]` had a
/// laptop that gossips with an approver every second being told there was none and sent to fetch
/// the passphrase, which is the one posture this whole arrangement exists to avoid.
#[derive(Debug, Clone, Copy)]
pub enum Met<'a> {
    /// Certificates peers proved at a handshake. Only a daemon has these, and an empty one here
    /// is a fact: this node has met nobody.
    Handshakes {
        certificates: &'a [MembershipCert],
        /// How many handshakes a peer has **refused** (ADR-0060), which is the other half of
        /// the same question and belongs in the same variant for the same reason the
        /// certificates do: a process with no mesh has dialled nobody, so zero there would be
        /// "never looked" wearing the face of "none seen". Inside the variant, that state
        /// cannot be spelled.
        turned_away: u64,
    },
    /// Nobody was asked. This node's own certificate is the whole of the evidence.
    NotAsked,
}

impl Met<'_> {
    fn certificates(&self) -> &[MembershipCert] {
        match self {
            Met::Handshakes { certificates, .. } => certificates,
            Met::NotAsked => &[],
        }
    }

    /// Refused handshakes, or `None` where nobody dialled anything to find out.
    fn turned_away(&self) -> Option<u64> {
        match self {
            Met::Handshakes { turned_away, .. } => Some(*turned_away),
            Met::NotAsked => None,
        }
    }
}

/// Has this node been revoked from its own fleet?
///
/// The same question `NodeMembership::revoked_here` asks of the live daemon state, asked here of
/// a `FleetState` so the *report* and the *door* cannot drift apart. Written once because it is
/// now read at four points in `health` and a fifth spelling of it is how one of them ends up
/// answering a slightly different question.
fn revoked_here(state: &FleetState) -> bool {
    state.is_revoked(state.membership.member)
}

/// Read this node's fleet state and what its peers' handshakes proved, and say how it is doing.
///
/// What [`Met`] carries is added to this node's own certificate, because it is a member of its
/// own fleet and forgetting that makes a fleet of one report zero approvers.
#[must_use]
pub fn health(state: &FleetState, met: Met<'_>, now: Millis) -> FleetHealth {
    let mut seen: BTreeMap<NodeId, &MembershipCert> = met
        .certificates()
        .iter()
        .filter(|cert| cert.fleet == state.fleet && !state.is_revoked(cert.member))
        .map(|cert| (cert.member, cert))
        .collect();
    seen.insert(state.membership.member, &state.membership);

    let approvers: Vec<&&MembershipCert> = seen
        .values()
        .filter(|cert| cert.grants.contains(&Grant::Approve))
        .collect();
    let expiring: Vec<String> = seen
        .values()
        .filter(|cert| cert.member != state.membership.member && cert.is_due_for_renewal(now))
        .map(|cert| {
            let left = days(cert.expires_at.saturating_sub(now));
            format!(
                "{} in {left} day{}",
                cert.name,
                if left == 1 { "" } else { "s" }
            )
        })
        .collect();

    let reapproval_due: Vec<ApprovalDue> = seen
        .values()
        .filter(|cert| cert.is_due_for_reapproval(now))
        .map(|cert| ApprovalDue {
            node: cert.member,
            name: cert.name.clone(),
            days: days(cert.approval_expires_at().saturating_sub(now)),
            decided: state.stands_for(cert, now),
        })
        .collect();

    let mut notes = Vec::new();
    // Before every other note, because on a revoked device every one of them is advice about a
    // fleet this node is no longer in — `grant approve to a second device`, said to a machine
    // that may grant nothing, naming the approver that evicted it as the one it should worry
    // about losing. The `accepting` line four rows up has been honest since ADR-0044; the whole
    // fleet block under it was not, which is ADR-0060's finding arriving at the sibling command.
    // The difference from ADR-0060's case is the evidence: a rekey's refusal is an uncheckable
    // claim by a peer, and this is a revocation signed by the fleet key and verified here.
    if revoked_here(state) {
        notes.push(
            "this device has been revoked from this fleet. Its certificate is not usable, \
             no peer will admit it, and nothing below is advice it can act on. The way \
             back is a fresh enrolment from a device that is still in: `offload invite \
             <this node>` there, `offload join --token …` here."
                .to_string(),
        );
    }
    match (met, approvers.len()) {
        // Nobody was asked, so neither of the sentences below can be said: what this node's own
        // certificate carries is the whole of what is known, and the count it produces is 1 or 0
        // whatever the fleet looks like. Pointing at the command that *can* answer is the useful
        // half; nagging on a number this could not have is how somebody is sent to the drawer.
        (Met::NotAsked, count) => notes.push(format!(
            "this device {} an approver. Whether any other is, this command cannot see — it runs \
             with no daemon on purpose, and a peer's grants arrive at a handshake. `offload \
             status` counts the approvers this node has met.",
            if count == 0 { "is not" } else { "is" }
        )),
        // Both of these tell somebody to arrange an approver, which a revoked device cannot do
        // and does not need: it is not in the fleet whose approvers are being counted.
        (Met::Handshakes { .. }, 0) if !revoked_here(state) => notes.push(
            "no approver among the devices this node has met — if there is none anywhere, \
             every enrolment needs the passphrase, which is the posture ADR-0012 exists to \
             avoid. `offload grant approve` on a device you keep."
                .to_string(),
        ),
        (Met::Handshakes { .. }, 1) if !revoked_here(state) => notes.push(format!(
            "only one approver ({}) — losing it means the next enrolment costs a rekey. Grant \
             `approve` to a second device.",
            approvers[0].name
        )),
        (Met::Handshakes { .. }, _) => {}
    }
    // The note a device rekeyed out of its fleet needs, and the one it could not get: every
    // report it has says the fleet is fine, because a refused handshake and a silent peer are
    // the same thing to the failure detector (ADR-0060).
    //
    // **Both ends of a rekey are turned away by the other**, so which sentence to print is not
    // in the refusal — the evicted device holds a certificate for the old fleet and the
    // rekeying one holds a certificate the evicted device has never heard of, and each refuses
    // the other in the same words. The walk that added this note caught it printing the
    // evicted device's advice on the machine that had done the evicting, which is the advice
    // exactly backwards. `last_rekeyed` is this device's own action and is the only evidence
    // about itself it may reason from.
    if met.turned_away().is_some_and(|n| n > 0) {
        notes.push(if state.last_rekeyed().is_some() {
            // Nothing for this operator to do to themselves, and no claim to disclaim: the
            // refusal is this node refusing to recognise a device it deliberately left out,
            // seen from the other side.
            "a device is still dialling this node with a certificate for the fleet this one \
             replaced. That is `offload rekey` working — it is left out until it takes up the \
             invitation that printed. `offload invite <node>` prints another."
                .to_string()
        } else {
            // Three sentences, and the third is not decoration: it is the difference between a
            // report and a verdict, and somebody reading the first two at three in the morning
            // is entitled to know which one they are holding.
            "a peer refused this node's certificate, saying the fleet has moved on. If someone \
             ran `offload rekey`, this device needs the invitation that printed — `offload join \
             --token …`. This node cannot check that claim and has not acted on it."
                .to_string()
        });
    }
    if state.membership.is_due_for_renewal(now) {
        // Split, because the sole approver is the device this fires on most and the undivided
        // sentence was false there. A node asks its *peers* to renew it and never itself —
        // deliberately: a device that could re-sign its own membership would never fall out of
        // a fleet it had been removed from, which is the backstop ADR-0012 leans on twice. So
        // the only approver in a fleet cannot renew itself, and telling it that "a fleet with no
        // approver reachable cannot renew" sends it looking for a missing device while
        // `offload fleet` two lines up says `approver this node may enrol others`.
        notes.push(if state.membership.granted(Grant::Approve, now) {
            "this node's certificate is running out and no peer has renewed it. This node is \
             itself an approver and cannot renew its own certificate — grant `approve` to a \
             second device, or re-issue this one with the passphrase. Once it lapses the way \
             back is the passphrase."
                .to_string()
        } else {
            "this node's certificate is running out and no peer has renewed it — a fleet with \
             no approver reachable cannot renew, and once it lapses the way back is the \
             passphrase."
                .to_string()
        });
    }
    if let Some(delegation) = state.approver.as_ref().filter(|d| {
        d.approver == state.membership.member
            && now < d.expires_at
            && d.expires_at.0.saturating_sub(now.0) <= REAPPROVAL_WINDOW.0
    }) {
        // What it signed stays valid (it is checked as of issue), but from that day this node
        // enrols and re-approves nobody — and on a fleet with one approver, nobody does.
        notes.push(format!(
            "this node's approver delegation runs out in {} day(s). What it has issued stays \
             valid, but after that it can enrol and re-approve nobody — `offload grant approve` \
             (the passphrase) renews it, and this node's own approval, for a year.",
            days(delegation.expires_at.saturating_sub(now))
        ));
    }
    if state.membership.is_due_for_reapproval(now) && !revoked_here(state) {
        // Its own row is in `reapproval_due` too; this is what to do about it, which differs by
        // who is asking, for the reason the renewal note above is split.
        let left = days(state.membership.approval_expires_at().saturating_sub(now));
        notes.push(if state.membership.granted(Grant::Approve, now) {
            format!(
                "this node's own approval runs out in {left} day(s), and no node re-approves \
                 itself — `offload reapprove {}` on another approver, or `offload grant \
                 approve` here with the passphrase. After that, no peer will admit it.",
                state.membership.name
            )
        } else {
            format!(
                "this node's approval runs out in {left} day(s) — `offload reapprove {}` on an \
                 approver re-approves it the next time it asks, which it is doing every fifteen \
                 minutes. After that, no peer will admit it.",
                state.membership.name
            )
        });
    }
    match state.last_verified() {
        // Only on a device the passphrase has actually been typed on. An invited laptop has no
        // record and never will, so nagging it would be telling somebody to go and use the
        // secret this arrangement exists to keep in a drawer — on the wrong machine.
        None if !state.passphrase_uses.is_empty() => notes.push(
            "the passphrase has never been checked against this fleet — a recovery secret \
             nobody has read back is not a recovery secret. `offload verify`."
                .to_string(),
        ),
        None => {}
        Some(when) if days(now.saturating_sub(when)) > 365 => notes.push(format!(
            "the passphrase was last checked {} days ago — `offload verify` reads it back and \
             grants nothing.",
            days(now.saturating_sub(when))
        )),
        Some(_) => {}
    }

    FleetHealth {
        fleet: state.fleet.short(),
        members_met: u32::try_from(seen.len()).unwrap_or(u32::MAX),
        approvers_met: u32::try_from(approvers.len()).unwrap_or(u32::MAX),
        cert_days: days(state.membership.expires_at.saturating_sub(now)),
        revoked_here: revoked_here(state),
        expiring,
        reapproval_due,
        passphrase_days: state
            .last_verified()
            .map(|when| days(now.saturating_sub(when))),
        revoked: u32::try_from(state.revocations.len()).unwrap_or(u32::MAX),
        notes,
    }
}

/// Rounded rather than truncated: a certificate with 29 days and 23 hours left has 30 days
/// left, and saying 29 makes a fresh one look a day old.
fn days(span: Millis) -> u64 {
    (span.as_secs() + 43_200) / 86_400
}

/// What an invitation looks like when a person has to move it between two machines.
///
/// Tagged and versioned, because it is pasted into terminals and will one day be pasted into
/// the wrong version of this program: a prefix turns that into a sentence rather than a
/// base64 decode error about a certificate.
pub const INVITE_PREFIX: &str = "offload-invite-1.";

/// What is actually pasted between two machines.
///
/// A certificate, the delegation behind it, and — only for a rekey — the old fleet key's
/// statement that this new fleet replaces it. The last one is what lets a device that is
/// *already* a member be moved: without it, an invitation to an unfamiliar fleet is
/// indistinguishable from somebody trying to take the machine, and the only safe answer is to
/// refuse (ADR-0012).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invitation {
    #[serde(flatten)]
    pub credentials: Credentials,
    /// Present when this invitation replaces a fleet the device already belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub succession: Option<Succession>,
}

impl Invitation {
    #[must_use]
    pub fn new(credentials: Credentials) -> Invitation {
        Invitation {
            credentials,
            succession: None,
        }
    }

    #[must_use]
    pub fn succeeding(credentials: Credentials, succession: Succession) -> Invitation {
        Invitation {
            credentials,
            succession: Some(succession),
        }
    }
}

/// Wrap a certificate up for carrying.
///
/// **This is not a bearer token, and that is what makes the design simple.** ADR-0012 imagined
/// a one-time, minutes-long credential; what is actually being carried is a membership
/// certificate, which names the joining device's key and is useless to anybody who does not
/// hold it. So it needs no expiry of its own beyond the certificate's, it can be pasted into a
/// chat window, and it is public material in the same sense the fleet's public key is. The rule
/// it must not break is the other one: the *passphrase* never goes near a command line, and
/// nothing here carries it.
#[must_use]
pub fn encode_invite(invitation: &Invitation) -> String {
    use base64::Engine as _;
    let json = serde_json::to_vec(invitation).unwrap_or_default();
    format!(
        "{INVITE_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    )
}

/// What taking up an invitation did (`offload join --token`, and the app's Join).
#[derive(Debug)]
pub enum TakenUp {
    /// A device with no fleet joined this one.
    Joined(FleetState),
    /// A member moved to the fleet that replaced its own, on a succession the old key signed.
    Moved { from: FleetId, state: FleetState },
    /// A member took up a wider or later certificate in the fleet it already belongs to.
    NewCertificate(FleetState),
}

/// Take up an invitation in `dir` — the decisions `offload join --token` makes, in one place, so
/// the CLI and the app (ADR-0071) join, move and refuse by the same rules. Refusals are the
/// sentences a person reads. The enrolment is written to the fleet log; if that write fails the
/// membership still stands and the warning is logged.
pub fn take_up_invitation(dir: &Path, token: &str, now: Millis) -> Result<TakenUp, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("creating state dir {}: {e}", dir.display()))?;
    let identity =
        crate::identity::load_or_create(dir).map_err(|e| format!("node identity: {e}"))?;
    let invitation = decode_invite(token).map_err(|e| e.to_string())?;
    let enrolled = |state: &FleetState| {
        let event = offload_core::FleetEvent::Enrolled {
            node: state.membership.member,
            name: state.membership.name.clone(),
            grants: state.membership.grants.clone(),
            how: offload_core::Enrolment::Invited,
        };
        if let Err(e) = offload_store::Store::open(dir).and_then(|store| {
            store.append_fleet_event(&offload_core::FleetLogEvent::new(now.0, event))
        }) {
            tracing::warn!(error = %e, "joined, but the enrolment could not be written to the fleet log");
        }
    };

    let Some(mut existing) = load(dir).map_err(|e| e.to_string())? else {
        let state = accept_invite(invitation, identity.id(), now).map_err(|e| e.to_string())?;
        save(dir, &state).map_err(|e| e.to_string())?;
        enrolled(&state);
        return Ok(TakenUp::Joined(state));
    };
    let offered = invitation.credentials.membership.fleet;
    if existing.fleet != offered {
        let Some(succession) = &invitation.succession else {
            return Err(format!(
                "this node belongs to fleet {} and that invitation is for {}, with nothing from {} \
                 saying it was replaced. A device joins a second fleet under its own \
                 OFFLOAD_STATE_DIR; it is moved to a new one by `offload rekey`.",
                existing.fleet.short(),
                offered.short(),
                existing.fleet.short()
            ));
        };
        succession.verify(existing.fleet, offered).map_err(|e| {
            format!(
                "that invitation claims fleet {} was replaced by {}, and {} did not sign it: {e}",
                existing.fleet.short(),
                offered.short(),
                existing.fleet.short()
            )
        })?;
        let mut state = accept_invite(invitation, identity.id(), now).map_err(|e| e.to_string())?;
        state.passphrase_uses = existing.passphrase_uses.clone();
        save(dir, &state).map_err(|e| e.to_string())?;
        enrolled(&state);
        return Ok(TakenUp::Moved {
            from: existing.fleet,
            state,
        });
    }
    match existing
        .adopt(invitation.credentials, now)
        .map_err(|e| e.to_string())?
    {
        Adopted::Taken => {}
        Adopted::NoBetter => {
            return Err(
                "that certificate is no better than the one this node already holds".into(),
            );
        }
        outcome @ Adopted::Narrower { .. } => {
            return Err(format!(
                "that certificate takes {} away from this node, so it was not taken up. A \
                 renewal restates what it descends from and `offload invite` widens a \
                 certificate; neither narrows one. If this device really should stop hosting, \
                 `offload revoke` on the device with the passphrase says so.",
                outcome.lost()
            ));
        }
    }
    save(dir, &existing).map_err(|e| e.to_string())?;
    Ok(TakenUp::NewCertificate(existing))
}

/// Unwrap one, or say what it is instead.
pub fn decode_invite(token: &str) -> Result<Invitation, FleetError> {
    use base64::Engine as _;
    let token = token.trim();
    let body = token
        .strip_prefix(INVITE_PREFIX)
        .ok_or(FleetError::Corrupt {
            path: "invitation".into(),
            reason: format!("does not start with `{INVITE_PREFIX}` — is this the whole token?"),
        })?;
    // The commonest way an invitation fails is being cut short between printed and pasted — a
    // terminal wrapping it, a chat app truncating it, `adb input text` dropping its tail — and the
    // decoder's own words for that ("EOF while parsing a list at line 1 column 93") send nobody to
    // the fix. Measured on the app's Join screen, session ninety-two.
    let cut_short = || FleetError::Corrupt {
        path: "invitation".into(),
        reason: format!(
            "is incomplete — it was cut short somewhere between being printed and pasted here. \
             Copy the whole line, from `{INVITE_PREFIX}` to its end."
        ),
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|e| match e {
            base64::DecodeError::InvalidLength(_) | base64::DecodeError::InvalidLastSymbol(..) => {
                cut_short()
            }
            other => FleetError::Corrupt {
                path: "invitation".into(),
                reason: other.to_string(),
            },
        })?;
    serde_json::from_slice(&bytes).map_err(|e| {
        if e.is_eof() {
            cut_short()
        } else {
            FleetError::Corrupt {
                path: "invitation".into(),
                reason: e.to_string(),
            }
        }
    })
}

/// The scheme the product app registers for joining by a tapped link (ADR-0071).
pub const JOIN_LINK_PREFIX: &str = "offload://join?token=";

/// An invitation as a link the app opens: the token is URL-safe base64, so it goes in as it is.
#[must_use]
pub fn join_link(token: &str) -> String {
    format!("{JOIN_LINK_PREFIX}{token}")
}

/// Enrol this node using an invitation an existing member issued.
///
/// The moment of trust, and it is worth being plain about where it sits: the joining device has
/// no fleet key yet, so it cannot check that this is *the* fleet — it can only check that the
/// invitation is internally consistent and is for this device. Whoever pasted the token is the
/// one vouching for it, exactly as they are when they read out a fingerprint. Everything after
/// this moment verifies against the key learned here.
pub fn accept_invite(
    invitation: Invitation,
    node: NodeId,
    now: Millis,
) -> Result<FleetState, FleetError> {
    let credentials = invitation.credentials;
    let cert = credentials.membership;
    if cert.member != node {
        return Err(FleetError::NotOurs {
            member: cert.member.short(),
            node: node.short(),
        });
    }
    // Against the fleet named *in the certificate*: there is nothing else to check it against
    // yet. What this catches is a malformed or tampered invitation, not a hostile fleet.
    cert.verify(cert.fleet, credentials.delegation.as_ref(), now)?;
    Ok(FleetState {
        fleet: cert.fleet,
        membership: cert,
        chain: credentials.delegation,
        // Being enrolled is not being made an approver: that is a separate delegation, and
        // conflating them would make every invited device able to invite.
        approver: None,
        revocations: Vec::new(),
        // Deliberately empty. The passphrase was not used here — that is the entire point of
        // the invite path — so recording a use would put noise in the one log whose value is
        // that every line in it is signal.
        passphrase_uses: Vec::new(),
        reapprovals: Vec::new(),
    })
}

#[must_use]
pub fn path(state_dir: &Path) -> PathBuf {
    state_dir.join(FLEET_FILE)
}

/// Read this node's fleet state, or `None` if it has not joined one.
///
/// Not joining is an ordinary state, not an error: `offloadd` runs perfectly well as a fleet
/// of one, and phase 1's single-node behaviour predates membership entirely.
pub fn load(state_dir: &Path) -> Result<Option<FleetState>, FleetError> {
    load_from(&path(state_dir))
}

/// What this device is called **to the fleet**: its certificate's name, or `configured` if it has
/// none.
///
/// A device has two names and which is right depends on who reads it, which `main::
/// report_membership` says at startup and warns about when they differ. The configured one is
/// local — `offload status`, the daemon's own log — and it defaults to the machine's *hostname*.
/// The certificate's is the name the fleet gave the device; it is signed, so it is the one
/// nobody else could have chosen, and ADR-0012 puts it on the certificate *"for `offload
/// nodes`"* in as many words.
///
/// So this is the name for anything a **peer** will read: the entry this node gossips, the `by`
/// on a forwarded cancel, the sentence in a run's log. Measured before it was: two daemons on
/// one laptop, neither with `name` in its config, both therefore `fedora` — and `offload nodes`
/// showed the certified `bravo` for **25 seconds** and the hostname for ever after.
///
/// Read from the file rather than cached, which is `server::revoked_refusal`'s posture: `offload
/// rekey` rewrites it under a running daemon. Falling back to `configured` rather than to
/// nothing, because an unreadable `fleet.json` must not make a device anonymous — the same
/// choice, for the same reason, that `revoked_refusal` makes one function over.
#[must_use]
pub fn display_name(state_dir: &Path, configured: &str) -> String {
    match load(state_dir) {
        Ok(Some(state)) if !state.membership.name.is_empty() => state.membership.name,
        _ => configured.to_string(),
    }
}

/// The same, given the file itself rather than the directory holding it.
///
/// Two entry points because two callers know different things: a command knows a state
/// directory, and the daemon's live mirror of this file knows the path it has been watching.
/// Resolving the directory again there would be a second answer to a question already settled.
pub fn load_from(path: &Path) -> Result<Option<FleetState>, FleetError> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).map_err(|e| FleetError::Read {
        path: path.display().to_string(),
        reason: e.to_string(),
    })?;
    let state = serde_json::from_str(&text).map_err(|e| FleetError::Corrupt {
        path: path.display().to_string(),
        reason: e.to_string(),
    })?;
    Ok(Some(state))
}

/// Read fleet state for a caller that cannot proceed without it.
pub fn require(state_dir: &Path) -> Result<FleetState, FleetError> {
    load(state_dir)?.ok_or(FleetError::NotAMember)
}

/// Write fleet state, replacing whatever was there.
///
/// Via a temporary file and a rename, so an interrupted write leaves the old state rather
/// than half of the new one. A node that loses its certificate mid-write has to re-join, and
/// re-joining needs the passphrase.
pub fn save(state_dir: &Path, state: &FleetState) -> Result<(), FleetError> {
    save_to(&path(state_dir), state)
}

/// The same, given the file itself. See [`load_from`].
pub fn save_to(path: &Path, state: &FleetState) -> Result<(), FleetError> {
    let temp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(state).map_err(|e| FleetError::Write {
        path: path.display().to_string(),
        reason: e.to_string(),
    })?;

    let write = |target: &Path| -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(target)?;
            file.write_all(text.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()
        }
        #[cfg(not(unix))]
        std::fs::write(target, format!("{text}\n"))
    };

    write(&temp)
        .and_then(|()| std::fs::rename(&temp, path))
        .map_err(|e| FleetError::Write {
            path: path.display().to_string(),
            reason: e.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use offload_core::fleet::PROBATION;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "offload-fleet-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&path).expect("create scratch");
            Scratch(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// Cheap stand-in for a derived key: `FleetKey` only ever comes from a passphrase, and
    /// deriving one per test would cost a third of a second each for no extra coverage.
    fn fleet_key(phrase: &str) -> FleetKey {
        FleetKey::derive(phrase).expect("derive")
    }

    fn node(seed: u8) -> NodeId {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        NodeId::from_bytes(signing.verifying_key().to_bytes())
    }

    const NOW: Millis = Millis(1_700_000_000_000);

    #[test]
    fn founding_produces_a_member_who_can_host_and_approve_immediately() {
        let key = fleet_key("abacus zoom yo-yo");
        let me = node(1);
        let state = FleetState::found(&key, me, "desktop", NOW);

        assert_eq!(state.check(me, NOW).map_err(|e| e.to_string()), Ok(()));
        let grants = state.grants(NOW);
        assert!(
            grants.contains(&Grant::HostRuns),
            "no probation for a founder"
        );
        assert!(grants.contains(&Grant::Approve));
        assert!(state.approver.is_some(), "the drawer closes at init");
    }

    #[test]
    fn joining_with_the_passphrase_grants_less_and_waits() {
        // The recovery path is deliberately thin: it needs nothing else to be awake, so it
        // is the path an attacker with the passphrase alone would take.
        let key = fleet_key("abacus zoom yo-yo");
        let me = node(2);
        let state = FleetState::join(&key, me, "laptop", NOW);

        assert_eq!(state.check(me, NOW).map_err(|e| e.to_string()), Ok(()));
        assert!(!state.grants(NOW).contains(&Grant::HostRuns));
        assert!(state.grants(NOW).contains(&Grant::Submit));
        assert!(
            state.approver.is_none(),
            "joining does not make an approver"
        );
    }

    #[test]
    fn a_granted_host_runs_still_serves_its_probation() {
        // ADR-0012 mitigation 2: the window is measured from the certificate carrying the
        // grant, so granting is not a way around it.
        let key = fleet_key("abacus zoom yo-yo");
        let me = node(2);
        let mut state = FleetState::join(&key, me, "laptop", NOW);

        let later = NOW + Millis(60 * 60 * 1_000);
        state
            .add_grant(&key, Grant::HostRuns, later, None)
            .expect("grant");

        assert!(!state.grants(later).contains(&Grant::HostRuns));
        assert!(state.grants(later + PROBATION).contains(&Grant::HostRuns));
        assert_eq!(state.check(me, later).map_err(|e| e.to_string()), Ok(()));
    }

    #[test]
    fn granting_host_runs_to_an_invited_device_probates_it() {
        // The path the previous test does not cover, and the one people actually use. An invited
        // certificate carries `probation = false` for the door's reason — a member deliberately
        // enrolled it — and `add_grant` inherited that flag, so `offload grant host-runs` on a
        // laptop enrolled by invitation took effect the same second. Measured on two daemons:
        // `grants submit, deliver, host-runs` in `offload fleet`, immediately. ADR-0012's
        // amendment is that probation follows the *grant*, with no exemption for the passphrase,
        // and mitigation 4's alarm is bought fifteen minutes to be acted on in.
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let signing = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let mut invited = accept_invite(
            Invitation::new(
                founder
                    .invite(&signing, None, node(2), "laptop", NOW)
                    .expect("issue"),
            ),
            node(2),
            NOW,
        )
        .expect("accept");
        assert!(!invited.membership.probation, "the door does not probate");

        let later = NOW + Millis(60 * 60 * 1_000);
        invited
            .add_grant(&key, Grant::HostRuns, later, None)
            .expect("grant");
        assert!(!invited.grants(later).contains(&Grant::HostRuns));
        assert!(invited.grants(later + PROBATION).contains(&Grant::HostRuns));

        // And an unrelated grant does not suspend hosting that is already in force — the window
        // is for a grant being made, not for every re-issue of a certificate.
        invited
            .add_grant(&key, Grant::Approve, later + PROBATION, None)
            .expect("grant");
        assert!(invited.grants(later + PROBATION).contains(&Grant::HostRuns));
    }

    #[test]
    fn a_founder_granted_something_new_does_not_acquire_probation() {
        let key = fleet_key("abacus zoom yo-yo");
        let me = node(1);
        let mut state = FleetState::found(&key, me, "desktop", NOW);
        let later = NOW + Millis(1_000);
        state
            .add_grant(&key, Grant::Approve, later, None)
            .expect("grant");
        assert!(state.grants(later).contains(&Grant::HostRuns));
    }

    #[test]
    fn the_wrong_passphrase_signs_nothing() {
        // Without this check a mistyped passphrase would mint a perfectly valid certificate
        // for a fleet that does not exist, and the node would simply stop being recognised.
        let key = fleet_key("abacus zoom yo-yo");
        let other = fleet_key("zebra puppy abacus");
        let mut state = FleetState::found(&key, node(1), "desktop", NOW);

        assert!(matches!(
            state.add_grant(&other, Grant::HostRuns, NOW, None),
            Err(FleetError::WrongPassphrase { .. })
        ));
        assert!(matches!(
            state.revoke(&other, node(3), NOW),
            Err(FleetError::WrongPassphrase { .. })
        ));
    }

    #[test]
    fn a_revoked_member_fails_its_own_check() {
        // Revocation is immediate and local (ADR-0012): the node does not wait for the fleet
        // to converge before it stops considering itself a member.
        let key = fleet_key("abacus zoom yo-yo");
        let me = node(2);
        let mut state = FleetState::join(&key, me, "laptop", NOW);
        state.revoke(&key, me, NOW).expect("revoke");

        assert!(state.is_revoked(me));
        assert!(state.check(me, NOW).is_err());
    }

    #[test]
    fn a_forged_revocation_is_not_filed() {
        // The subject may never refute a revocation — and nobody but the fleet key may
        // assert one, or that rule would cut the other way.
        let key = fleet_key("abacus zoom yo-yo");
        let impostor = fleet_key("zebra puppy abacus");
        let mut state = FleetState::found(&key, node(1), "desktop", NOW);

        let forged = Revocation::issue(impostor.signing_key(), state.fleet, node(5), NOW, 1);
        assert!(state.file(forged).is_err());
        assert!(!state.is_revoked(node(5)));
    }

    #[test]
    fn filing_the_same_revocation_twice_records_it_once() {
        let key = fleet_key("abacus zoom yo-yo");
        let mut state = FleetState::found(&key, node(1), "desktop", NOW);
        let filed = state.revoke(&key, node(4), NOW).expect("revoke");
        state.file(filed.revocation().clone()).expect("file again");
        assert_eq!(state.revocations.len(), 1);
    }

    #[test]
    fn revoking_the_same_member_twice_records_it_once_and_says_so() {
        // The test above files the *same object* twice, which is the gossip relay's case and is
        // the one the old `(member, serial)` dedup was written for. This is the other one, and
        // it could never fire: `revoke` puts the clock in the serial, so a second call minted a
        // record that differed in the one field the dedup compared and in no field anything
        // decides on. Measured on two meshed daemons before the fix — the node the command was
        // typed on said `revoked 5 device(s)` about two devices, the node that heard it by
        // gossip said 2.
        let key = fleet_key("abacus zoom yo-yo");
        let mut state = FleetState::found(&key, node(1), "desktop", NOW);

        let first = state.revoke(&key, node(4), NOW).expect("revoke");
        assert!(matches!(first, Revoked::Now(_)));

        // A later clock, which is what made the old dedup miss: same member, new serial.
        let again = state
            .revoke(&key, node(4), Millis(NOW.0 + 60_000))
            .expect("revoke again");
        assert_eq!(
            again,
            Revoked::Already(first.revocation().clone()),
            "the record in force is the first one, which is what the fleet already holds"
        );
        assert_eq!(state.revocations.len(), 1);
        assert!(state.is_revoked(node(4)));

        // And a no-op does not spend the passphrase record either: `offload nodes --history`
        // promises every *use* of it, and a call that wrote nothing used it to find that out.
        assert_eq!(
            state
                .passphrase_uses
                .iter()
                .filter(|u| u.what == PassphraseUse::Revoke)
                .count(),
            1
        );
    }

    #[test]
    fn a_certificate_for_another_node_is_refused() {
        // Copying somebody else's fleet.json is not a way into the fleet: the certificate
        // names a key this node does not hold.
        let key = fleet_key("abacus zoom yo-yo");
        let state = FleetState::found(&key, node(1), "desktop", NOW);
        assert!(matches!(
            state.check(node(2), NOW),
            Err(FleetError::NotOurs { .. })
        ));
    }

    #[test]
    fn state_round_trips_through_the_file_and_stays_verifiable() {
        let scratch = Scratch::new("roundtrip");
        let key = fleet_key("abacus zoom yo-yo");
        let me = node(1);
        let mut state = FleetState::found(&key, me, "desktop", NOW);
        state.revoke(&key, node(7), NOW).expect("revoke");

        assert!(
            load(&scratch.0).expect("load").is_none(),
            "not a member yet"
        );
        save(&scratch.0, &state).expect("save");
        let back = load(&scratch.0).expect("load").expect("present");

        assert_eq!(back, state);
        assert_eq!(back.check(me, NOW).map_err(|e| e.to_string()), Ok(()));
        assert!(back.is_revoked(node(7)));
    }

    #[test]
    fn saving_twice_replaces_rather_than_appends() {
        let scratch = Scratch::new("replace");
        let key = fleet_key("abacus zoom yo-yo");
        let state = FleetState::found(&key, node(1), "desktop", NOW);
        save(&scratch.0, &state).expect("first");
        save(&scratch.0, &state).expect("second");
        assert!(load(&scratch.0).expect("load").is_some());
        assert!(!path(&scratch.0).with_extension("json.tmp").exists());
    }

    #[test]
    fn every_use_of_the_passphrase_leaves_a_record() {
        // The compensating control for a break-glass secret: when it is never used, every
        // use is signal.
        let key = fleet_key("abacus zoom yo-yo");
        let mut state = FleetState::found(&key, node(1), "desktop", NOW);
        assert_eq!(state.passphrase_uses.len(), 1);

        state.record(PassphraseUse::Verify, NOW + Millis(5));
        state
            .add_grant(&key, Grant::HostRuns, NOW + Millis(10), None)
            .expect("grant");

        assert_eq!(state.last_verified(), Some(NOW + Millis(5)));
        assert_eq!(
            state
                .passphrase_uses
                .iter()
                .map(|u| u.what)
                .collect::<Vec<_>>(),
            vec![
                PassphraseUse::Init,
                PassphraseUse::Verify,
                PassphraseUse::Grant
            ]
        );
    }

    /// ADR-0069: an approver re-issues as it always has; a plain member holding `Renew` (every
    /// member, from the door) renews as a renewer, and the renewal verifies; one without `Renew`
    /// refuses, in words, so the asking node tries somebody else.
    #[test]
    fn an_approver_or_any_member_with_renew_re_issues_a_certificate() {
        let key = fleet_key("abacus zoom yo-yo");
        let approver_id = node(1);
        let founder = FleetState::found(&key, approver_id, "desktop", NOW);
        let plain = FleetState::join(&key, node(2), "laptop", NOW);
        let due = NOW + Millis(29 * 24 * 60 * 60 * 1_000);

        founder
            .renew_for(
                &ed25519_dalek::SigningKey::from_bytes(&[1; 32]),
                approver_id,
                &plain.membership,
                None,
                due,
            )
            .expect("the founder holds both the grant and the delegation");

        let by_a_renewer = plain
            .renew_for(
                &ed25519_dalek::SigningKey::from_bytes(&[2; 32]),
                node(2),
                &founder.membership,
                None,
                due,
            )
            .expect("a member with renew renews");
        assert_eq!(by_a_renewer.membership.renewer(), Some(node(2)));
        assert_eq!(by_a_renewer.membership.grants, founder.membership.grants);
        assert!(by_a_renewer.membership.verify(key.id(), None, due).is_ok());

        let mut observer = FleetState::join(&key, node(3), "observer", NOW);
        let mut terms = Terms::joining(key.id(), node(3), "observer", NOW);
        terms.grants.remove(&Grant::Renew);
        observer.membership = MembershipCert::issue(key.signing_key(), Issuer::Fleet, terms);
        let refusal = observer
            .renew_for(
                &ed25519_dalek::SigningKey::from_bytes(&[3; 32]),
                node(3),
                &founder.membership,
                None,
                due,
            )
            .expect_err("no renew, not an approver");
        assert!(refusal.contains("not granted renew"), "{refusal}");
    }

    /// A token cut short in copying says so, rather than quoting the JSON decoder.
    #[test]
    fn an_invitation_cut_short_says_so() {
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let signing = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let credentials = founder
            .invite(&signing, None, node(2), "laptop", NOW)
            .expect("invite");
        let token = encode_invite(&Invitation::new(credentials));
        assert!(decode_invite(&token).is_ok());
        for cut in [token.len() / 2, token.len() - 7, 40] {
            let err = decode_invite(&token[..cut])
                .expect_err("cut short")
                .to_string();
            assert!(err.contains("cut short"), "at {cut}: {err}");
        }
    }

    /// ADR-0069 §3 end to end on the two fleet files: a member in its approval's last month asks
    /// before its certificate is due, is told nobody here has decided, and — once a person runs
    /// `offload reapprove` on the approver — is handed a re-approval that starts a new year and
    /// keeps `host-runs`, and takes it up.
    #[test]
    fn a_decision_on_an_approver_re_approves_a_member_when_it_next_asks() {
        const DAY: u64 = 24 * 60 * 60 * 1_000;
        let key = fleet_key("abacus zoom yo-yo");
        let signing = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let me = node(1);
        let mut founder = FleetState::found(&key, me, "desktop", NOW);
        // The delegation `init` issues, which lasts exactly a year: the re-approval below is
        // issued inside it and checked past it, which is the cliff the owner decided away.
        let mut host = FleetState::join(&key, node(2), "vps-17", NOW);
        let mut terms = Terms::joining(key.id(), node(2), "vps-17", NOW);
        terms.grants.insert(Grant::HostRuns);
        terms.probation = false;
        let approved = MembershipCert::issue(key.signing_key(), Issuer::Fleet, terms);

        // Both renewed on day 330, as the ordinary loop would have: live, approved on day 0.
        let renewed_on = NOW + Millis(330 * DAY);
        host.membership = MembershipCert::issue(
            key.signing_key(),
            Issuer::Fleet,
            approved.renewal(renewed_on, CERT_LIFETIME),
        );
        founder.membership = MembershipCert::issue(
            key.signing_key(),
            Issuer::Fleet,
            founder.membership.renewal(renewed_on, CERT_LIFETIME),
        );

        let asked = NOW + Millis(336 * DAY);
        assert!(host.membership.is_due_for_reapproval(asked));
        let refusal = founder
            .renew_for(&signing, me, &host.membership, None, asked)
            .expect_err("nobody has decided");
        assert!(refusal.contains("nobody here has decided"), "{refusal}");

        assert!(founder
            .decide_reapproval(me, me, "desktop", asked)
            .is_err_and(|e| e.contains("no node re-approves itself")));
        assert!(host
            .decide_reapproval(node(2), me, "desktop", asked)
            .is_err_and(|e| e.contains("not an approver")));
        founder
            .decide_reapproval(me, node(2), "vps-17", asked)
            .expect("an approver records a decision");

        let answered = asked + Millis(DAY);
        let credentials = founder
            .renew_for(&signing, me, &host.membership, None, answered)
            .expect("the decision stands");
        assert_eq!(credentials.membership.approved_at(), answered);
        assert!(credentials.membership.grants.contains(&Grant::HostRuns));
        assert!(matches!(
            host.adopt(credentials, answered),
            Ok(Adopted::Taken)
        ));

        // A new year: it verifies past the first one, and the decision is now moot.
        // Day 366: past the first approval's year, and the re-approval (day 337) still live.
        let past_first_year = NOW + Millis(366 * DAY);
        assert_eq!(
            host.membership
                .verify(key.id(), host.chain.as_ref(), past_first_year),
            Ok(())
        );
        assert!(founder
            .renew_for(
                &signing,
                me,
                &host.membership,
                host.chain.as_ref(),
                answered + Millis(DAY)
            )
            .is_err_and(|e| e.contains("not due")));
    }

    /// `offload grant approve` issues the delegation that makes the grant usable, and re-running
    /// it on the founder renews both its approval and its year to issue in.
    #[test]
    fn granting_approve_issues_a_delegation_and_renews_the_founders_year() {
        const DAY: u64 = 24 * 60 * 60 * 1_000;
        let key = fleet_key("abacus zoom yo-yo");
        let mut second = FleetState::join(&key, node(2), "laptop", NOW);
        assert!(second.approver.is_none());
        second
            .add_grant(&key, Grant::Approve, NOW + Millis(DAY), None)
            .expect("grant");
        let delegation = second
            .approver
            .clone()
            .expect("a delegation to prove the grant");
        assert_eq!(delegation.approver, node(2));
        assert!(delegation.verify(key.id(), NOW + Millis(2 * DAY)).is_ok());

        let mut founder = FleetState::found(&key, node(1), "desktop", NOW);
        let later = NOW + Millis(340 * DAY);
        assert!(health(&founder, Met::NotAsked, later)
            .notes
            .iter()
            .any(|n| n.contains("approver delegation runs out")));
        founder
            .add_grant(&key, Grant::Approve, later, None)
            .expect("grant");
        assert_eq!(founder.membership.approved_at(), later);
        let renewed = founder.approver.clone().expect("delegation");
        assert!(renewed.verify(key.id(), NOW + Millis(400 * DAY)).is_ok());
        assert!(!health(&founder, Met::NotAsked, later)
            .notes
            .iter()
            .any(|n| n.contains("approver delegation runs out")));
    }

    /// ADR-0069: a host renewed by a renewer takes the renewal up, and an approver can still renew
    /// it after that — the approval behind a renewer's renewal is where `host-runs` was granted,
    /// so it is the approver's authority too. Without that, the first renewer renewal of a host
    /// would have left it renewable only by renewers.
    #[test]
    fn a_host_renewed_by_a_renewer_adopts_it_and_an_approver_can_renew_it_again() {
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let renewer = FleetState::join(&key, node(3), "vps-3", NOW);
        let mut host = FleetState::join(&key, node(2), "vps-17", NOW);
        let mut terms = Terms::joining(key.id(), node(2), "vps-17", NOW);
        terms.grants.insert(Grant::HostRuns);
        terms.probation = false;
        host.membership = MembershipCert::issue(key.signing_key(), Issuer::Fleet, terms);

        let due = NOW + Millis(29 * 24 * 60 * 60 * 1_000);
        let renewed = renewer
            .renew_for(
                &ed25519_dalek::SigningKey::from_bytes(&[3; 32]),
                node(3),
                &host.membership,
                None,
                due,
            )
            .expect("a renewer renews a host");
        assert_eq!(host.adopt(renewed, due).expect("adopt"), Adopted::Taken);
        assert_eq!(host.membership.renewer(), Some(node(3)));
        assert!(host.membership.grants.contains(&Grant::HostRuns));

        let again = due + Millis(29 * 24 * 60 * 60 * 1_000);
        let by_approver = founder
            .renew_for(
                &ed25519_dalek::SigningKey::from_bytes(&[1; 32]),
                node(1),
                &host.membership,
                None,
                again,
            )
            .expect("an approver renews what a renewer renewed");
        assert!(
            by_approver
                .membership
                .verify(key.id(), by_approver.delegation.as_ref(), again)
                .is_ok(),
            "host-runs survives, bounded by the approval behind the renewal"
        );
        assert_eq!(
            by_approver.membership.approved_at(),
            NOW,
            "and the year is not reset"
        );
    }

    /// A renewal request now carries the asker's certificate (ADR-0069 §2), so what is restated
    /// is verified first: papers the fleet key never signed are refused rather than re-signed.
    #[test]
    fn a_certificate_that_does_not_verify_is_not_restated() {
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let impostor = fleet_key("some other passphrase");
        let mut forged_terms = Terms::joining(key.id(), node(2), "laptop", NOW);
        forged_terms.grants.insert(Grant::HostRuns);
        let forged = MembershipCert::issue(impostor.signing_key(), Issuer::Fleet, forged_terms);
        let due = NOW + Millis(29 * 24 * 60 * 60 * 1_000);
        let refusal = founder
            .renew_for(
                &ed25519_dalek::SigningKey::from_bytes(&[1; 32]),
                node(1),
                &forged,
                None,
                due,
            )
            .expect_err("forged papers are not renewed");
        assert!(refusal.contains("does not verify"), "{refusal}");
    }

    #[test]
    fn a_certificate_is_not_re_issued_before_it_needs_to_be() {
        // Bounds the work rather than protecting anything: an approver that re-signed on
        // request would sign for every peer on every probe.
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let plain = FleetState::join(&key, node(2), "laptop", NOW);
        let signing = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);

        let refusal = founder
            .renew_for(&signing, node(1), &plain.membership, None, NOW)
            .expect_err("a month of life left");
        assert!(refusal.contains("not due"), "{refusal}");
    }

    #[test]
    fn a_revoked_member_is_not_handed_fresh_papers() {
        // Unreachable through an admitted connection, and checked anyway: the day it becomes
        // reachable is the day a background loop nobody is watching re-papers an evicted device.
        let key = fleet_key("abacus zoom yo-yo");
        let mut founder = FleetState::found(&key, node(1), "desktop", NOW);
        let plain = FleetState::join(&key, node(2), "laptop", NOW);
        founder.revoke(&key, node(2), NOW).expect("revoke");

        let signing = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let due = NOW + Millis(29 * 24 * 60 * 60 * 1_000);
        let refusal = founder
            .renew_for(&signing, node(1), &plain.membership, None, due)
            .expect_err("revoked");
        assert!(refusal.contains("revoked"), "{refusal}");
    }

    #[test]
    fn a_renewal_is_taken_up_only_if_it_is_ours_and_an_improvement() {
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let mut plain = FleetState::join(&key, node(2), "laptop", NOW);
        let signing = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let due = NOW + Millis(29 * 24 * 60 * 60 * 1_000);

        let renewed = founder
            .renew_for(&signing, node(1), &plain.membership, None, due)
            .expect("issue");
        let was = plain.membership.expires_at;
        assert_eq!(
            plain.adopt(renewed.clone(), due).expect("adopt"),
            Adopted::Taken
        );
        assert!(plain.membership.expires_at > was);
        assert!(plain.chain.is_some(), "the delegation comes with it");
        assert_eq!(plain.check(node(2), due).map_err(|e| e.to_string()), Ok(()));

        // The same one again buys nothing, and adopting it would mean a peer could replace a
        // certificate this node holds whenever it liked.
        assert_eq!(plain.adopt(renewed, due).expect("adopt"), Adopted::NoBetter);

        // And one issued for somebody else is refused by name rather than quietly ignored.
        let other = FleetState::join(&key, node(3), "other", NOW);
        let theirs = founder
            .renew_for(&signing, node(1), &other.membership, None, due)
            .expect("issue");
        assert!(plain.adopt(theirs, due).is_err());
    }

    #[test]
    fn a_certificate_that_grants_less_is_refused_however_long_it_lasts() {
        // The axis the test above never varied. Its name has said "an improvement" since it was
        // written, and it only ever moved `expires_at` — so a later-expiring certificate that
        // granted less was an improvement by the only measure anything took, and both paths into
        // `adopt` could strip a grant. Measured in session seventy-eight on two state
        // directories: `offload invite <node>` under an approver's delegation needs no
        // passphrase, and the device holding `host-runs` that took that token up printed
        // `grants submit, deliver` — the same words it had printed before losing it.
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let mut wide = FleetState::join(&key, node(2), "laptop", NOW);

        // Give it `host-runs`, which only the root may mint (ADR-0012 mitigation 1).
        let mut grants = offload_core::default_grants();
        grants.insert(Grant::HostRuns);
        let widened = founder
            .issue_for(&key, node(2), "laptop", grants, NOW)
            .expect("issue");
        assert_eq!(wide.adopt(widened, NOW).expect("adopt"), Adopted::Taken);
        assert!(wide.membership.grants.contains(&Grant::HostRuns));
        let held = wide.membership.clone();

        // Now the approver offers what an approver may issue — later-expiring, and narrower.
        // This is `offload invite <node>` with no `--grant`, which costs no passphrase at all.
        let signing = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let later = NOW + Millis(60 * 1_000);
        let narrower = founder
            .invite(&signing, None, node(2), "laptop", later)
            .expect("invite");
        assert!(
            narrower.membership.expires_at > held.expires_at,
            "the staging is only meaningful if the narrower one lasts longer"
        );

        let outcome = wide.adopt(narrower, later).expect("adopt");
        assert_eq!(
            outcome,
            Adopted::Narrower {
                lost: vec![Grant::HostRuns]
            }
        );
        assert_eq!(outcome.lost(), "host-runs");
        assert_eq!(
            wide.membership, held,
            "a refused certificate leaves the held one exactly as it was"
        );
    }

    #[test]
    fn probation_is_not_read_as_a_grant_being_taken_away() {
        // `grants(now)` suppresses `HostRuns` for PROBATION without removing it, so comparing
        // what is *in force* would read the certificate that carries the grant as dropping it.
        // The comparison is between the certificates' own grants, and this is what says so.
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let mut plain = FleetState::join(&key, node(2), "laptop", NOW);
        let mut grants = offload_core::default_grants();
        grants.insert(Grant::HostRuns);

        let probating = founder
            .issue_for(&key, node(2), "laptop", grants.clone(), NOW)
            .expect("issue");
        assert!(probating.membership.probation);
        assert_eq!(plain.adopt(probating, NOW).expect("adopt"), Adopted::Taken);
        assert!(
            !plain.grants(NOW).contains(&Grant::HostRuns),
            "dormant under probation, and still in the certificate"
        );

        // A second one, later and identical in what it grants. Nothing is lost.
        let later = NOW + Millis(60 * 1_000);
        let again = founder
            .issue_for(&key, node(2), "laptop", grants, later)
            .expect("issue");
        assert_eq!(plain.adopt(again, later).expect("adopt"), Adopted::Taken);
    }

    #[test]
    fn a_renewal_signed_by_a_node_with_no_delegation_is_not_believed() {
        // An approver's signature is only good with the fleet-signed delegation beside it.
        // Without that check, any member could re-paper any other.
        let key = fleet_key("abacus zoom yo-yo");
        let mut plain = FleetState::join(&key, node(2), "laptop", NOW);
        let impostor = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let due = NOW + Millis(29 * 24 * 60 * 60 * 1_000);

        let forged = Credentials {
            membership: MembershipCert::issue(
                &impostor,
                Issuer::Approver {
                    node: NodeId::from_bytes(impostor.verifying_key().to_bytes()),
                },
                plain.membership.renewal(due, CERT_LIFETIME),
            ),
            delegation: None,
        };
        assert!(plain.adopt(forged, due).is_err());
    }

    #[test]
    fn an_invitation_travels_as_text_and_enrols_the_device_it_names() {
        // ADR-0012's headless path: the joining machine says who it is, an approver issues, and
        // what crosses is a certificate rather than anything secret.
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let signing = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);

        let credentials = founder
            .invite(&signing, None, node(2), "laptop", NOW)
            .expect("an approver may issue what a device gets at the door");
        let token = encode_invite(&Invitation::new(credentials));
        assert!(token.starts_with(INVITE_PREFIX));

        let joined =
            accept_invite(decode_invite(&token).expect("decode"), node(2), NOW).expect("accept");
        assert_eq!(joined.fleet, founder.fleet);
        assert_eq!(
            joined.check(node(2), NOW).map_err(|e| e.to_string()),
            Ok(())
        );
        assert_eq!(joined.grants(NOW), offload_core::default_grants());
        assert!(
            joined.approver.is_none(),
            "being enrolled is not being made an approver"
        );
        assert!(
            joined.passphrase_uses.is_empty(),
            "the whole point of this path is that the drawer stayed shut"
        );
    }

    #[test]
    fn an_invitation_is_useless_to_the_device_it_does_not_name() {
        // Which is why it needs no expiry of its own and can be pasted anywhere: it is a
        // certificate, not a bearer token, and it names a key its holder does not have.
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let signing = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let credentials = founder
            .invite(&signing, None, node(2), "laptop", NOW)
            .expect("issue");

        assert!(accept_invite(Invitation::new(credentials), node(3), NOW).is_err());
    }

    #[test]
    fn a_member_who_is_not_an_approver_cannot_invite() {
        let key = fleet_key("abacus zoom yo-yo");
        let plain = FleetState::join(&key, node(2), "laptop", NOW);
        let signing = ed25519_dalek::SigningKey::from_bytes(&[2; 32]);

        let refusal = plain
            .invite(&signing, None, node(3), "phone", NOW)
            .expect_err("no delegation");
        assert!(refusal.contains("not an approver"), "{refusal}");
    }

    #[test]
    fn host_runs_issued_to_a_named_device_still_serves_its_probation() {
        // ADR-0012 mitigation 2 read carefully. Probation only ever suppresses `host-runs`, so
        // "an invite skips probation" is a statement about the door — where the grant is not in
        // play. The mitigation is about the grant, and it has no exemption for the passphrase,
        // because somebody holding the passphrase is the threat it was written for. Without
        // this, issuing to a named device would have been the way around it.
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let mut grants = offload_core::default_grants();
        grants.insert(Grant::HostRuns);

        let credentials = founder
            .issue_for(&key, node(2), "laptop", grants, NOW)
            .expect("the root may grant anything");
        let joined = accept_invite(Invitation::new(credentials), node(2), NOW).expect("accept");

        assert!(joined.membership.grants.contains(&Grant::HostRuns));
        assert!(!joined.grants(NOW).contains(&Grant::HostRuns));
        assert!(joined.grants(NOW + PROBATION).contains(&Grant::HostRuns));
    }

    #[test]
    fn a_tampered_invitation_is_refused_rather_than_half_read() {
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let signing = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let mut credentials = founder
            .invite(&signing, None, node(2), "laptop", NOW)
            .expect("issue");

        // The name is signed, so relabelling a device is not something a carrier can do.
        credentials.membership.name = "desktop".into();
        assert!(accept_invite(Invitation::new(credentials), node(2), NOW).is_err());

        assert!(decode_invite("not-a-token").is_err());
        assert!(decode_invite(&format!("{INVITE_PREFIX}!!!")).is_err());
    }

    #[test]
    fn an_invitation_signed_by_an_approver_with_no_delegation_is_not_believed() {
        // The joining device has no fleet key yet, so this is the only check it can make: the
        // chain has to hang together on its own. Without it, anybody could mint an invitation
        // to a fleet of their own naming and the device would join it — which is a different
        // failure from the one below, and a worse one.
        let impostor = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let forged = Credentials {
            membership: MembershipCert::issue(
                &impostor,
                Issuer::Approver {
                    node: NodeId::from_bytes(impostor.verifying_key().to_bytes()),
                },
                Terms::joining(
                    FleetId::from_bytes(*node(9).as_bytes()),
                    node(2),
                    "laptop",
                    NOW,
                ),
            ),
            delegation: None,
        };
        assert!(accept_invite(Invitation::new(forged), node(2), NOW).is_err());
    }

    #[test]
    fn a_revoked_device_is_not_told_its_fleet_is_fine() {
        // ADR-0060 gave this report the *rekey* half of this question and the revoke half was
        // never walked. Measured on two meshed daemons: `offload revoke <bravo>` from alpha, and
        // bravo's `offload status` then read `2 member(s) met, 1 approver(s) · this node's
        // certificate lasts 30 more days`, with a note telling it to grant `approve` to a second
        // device — one line under the only honest sentence on the screen, and contradicted by
        // `offload fleet` on the same device saying `cert UNUSABLE: … has been revoked`.
        //
        // Unlike ADR-0060's case there is nothing to disclaim: a revocation is signed by the
        // fleet key and verified here, so this is one of the few claims about itself a node may
        // act on — and the supervisor already does.
        let key = fleet_key("abacus zoom yo-yo");
        let approver = FleetState::found(&key, node(1), "desktop", NOW);
        let mut me = FleetState::join(&key, node(2), "laptop", NOW);

        let met = || Met::Handshakes {
            certificates: std::slice::from_ref(&approver.membership),
            turned_away: 0,
        };

        // The control: before the revocation this device is an ordinary member, and the
        // approver nag it gets is right.
        let before = health(&me, met(), NOW);
        assert!(!before.revoked_here);
        assert!(before.notes.iter().any(|n| n.contains("only one approver")));

        me.file(
            approver
                .clone()
                .revoke(&key, node(2), NOW)
                .expect("revoke")
                .revocation()
                .clone(),
        )
        .expect("file");

        let after = health(&me, met(), NOW);
        assert!(after.revoked_here, "this device knows it is out");
        assert!(
            after.notes.iter().any(|n| n.contains("has been revoked")),
            "{:?}",
            after.notes
        );
        // Advice about arranging approvers is advice for members. This device may grant nothing
        // and is not in the fleet whose approvers are being counted.
        assert!(
            !after.notes.iter().any(|n| n.contains("approver (")),
            "{:?}",
            after.notes
        );
        assert!(
            !after.notes.iter().any(|n| n.contains("no approver among")),
            "{:?}",
            after.notes
        );
    }

    #[test]
    fn fleet_health_counts_approvers_from_certificates_rather_than_claims() {
        // Grants live on a certificate precisely because a node's claims about itself are not
        // to be believed (ADR-0012), so counting from gossip would count whatever a device said
        // it was. What is counted here is what a handshake proved.
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let plain = FleetState::join(&key, node(2), "laptop", NOW);

        let alone = health(
            &founder,
            Met::Handshakes {
                certificates: &[],
                turned_away: 0,
            },
            NOW,
        );
        assert_eq!(alone.members_met, 1, "it is a member of its own fleet");
        assert_eq!(alone.approvers_met, 1);
        assert!(alone.notes.iter().any(|n| n.contains("only one approver")));

        let with_peer = health(
            &founder,
            Met::Handshakes {
                certificates: std::slice::from_ref(&plain.membership),
                turned_away: 0,
            },
            NOW,
        );
        assert_eq!(with_peer.members_met, 2);
        assert_eq!(with_peer.approvers_met, 1, "the laptop is not one");

        // And a fleet with no approver reachable says the thing worth acting on.
        let from_laptop = health(
            &plain,
            Met::Handshakes {
                certificates: &[],
                turned_away: 0,
            },
            NOW,
        );
        assert_eq!(from_laptop.approvers_met, 0);
        assert!(from_laptop.notes.iter().any(|n| n.contains("no approver")));
    }

    #[test]
    fn a_command_that_met_nobody_does_not_report_on_who_is_out_there() {
        // `offload fleet` runs with no daemon (ADR-0012) and used to pass an empty slice, which
        // `health` read as "met nobody and therefore knows there is no approver". Measured on two
        // daemons: beta — gossiping with an approver every second — was told "no approver among
        // the devices this node has met" and pointed at `offload grant approve`, which needs the
        // passphrase. The caveat that would have softened it was printed only where
        // `state.approver.is_some()`, so the node being misled was the one node never shown it.
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let plain = FleetState::join(&key, node(2), "laptop", NOW);

        for state in [&founder, &plain] {
            let notes = health(state, Met::NotAsked, NOW).notes;
            assert!(
                notes
                    .iter()
                    .any(|n| n.contains("cannot see") && n.contains("offload status")),
                "a command that asked nobody says so, approver or not: {notes:?}"
            );
            assert!(
                !notes
                    .iter()
                    .any(|n| n.contains("no approver among") || n.contains("only one approver")),
                "and claims no count it could not have: {notes:?}"
            );
        }

        // The daemon's empty slice keeps meaning what it always did: it has met nobody, which is
        // a fact worth acting on rather than an absence of evidence.
        assert!(health(
            &plain,
            Met::Handshakes {
                certificates: &[],
                turned_away: 0
            },
            NOW
        )
        .notes
        .iter()
        .any(|n| n.contains("no approver among")));
    }

    #[test]
    fn a_revoked_peer_is_not_counted_as_a_member() {
        let key = fleet_key("abacus zoom yo-yo");
        let mut founder = FleetState::found(&key, node(1), "desktop", NOW);
        let plain = FleetState::join(&key, node(2), "laptop", NOW);
        founder.revoke(&key, node(2), NOW).expect("revoke");

        let after = health(
            &founder,
            Met::Handshakes {
                certificates: std::slice::from_ref(&plain.membership),
                turned_away: 0,
            },
            NOW,
        );
        assert_eq!(after.members_met, 1);
        assert_eq!(after.revoked, 1);
    }

    /// A device rekeyed out of its fleet must be told, and not told what to conclude.
    ///
    /// Measured on two meshed daemons with `offload rekey` typed on one of them: three seconds
    /// later each called the other `dead`, and the evicted node's `offload status` went on
    /// saying `2 member(s) met` with its only note warning about *losing* the approver that had
    /// already gone. The honest sentence was in its log, every twenty seconds (ADR-0060).
    #[test]
    fn a_node_a_peer_will_not_admit_is_told_so_and_told_it_cannot_check_it() {
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);

        let turned_away = health(
            &founder,
            Met::Handshakes {
                certificates: &[],
                turned_away: 3,
            },
            NOW,
        );
        let note = turned_away
            .notes
            .iter()
            .find(|n| n.contains("the fleet has moved on"))
            .expect("a refused handshake is reported");
        // The way out, named, because the count alone is not actionable.
        assert!(note.contains("offload rekey"), "{note}");
        assert!(note.contains("offload join"), "{note}");
        // …and the sentence that keeps it a report rather than a verdict. A node must not
        // believe a peer about itself, and somebody reading this at three in the morning is
        // entitled to know which of the two they are holding.
        assert!(note.contains("cannot check that claim"), "{note}");
        assert!(note.contains("has not acted on it"), "{note}");

        // **The other end of the same rekey**, and the reason this is two sentences. Both
        // devices are turned away by the other in identical words, so the refusal cannot say
        // which end you are on; the walk caught this printing "you need the invitation" on the
        // machine that had *issued* it. `last_rekeyed` is this device's own action.
        let mut rekeyed = founder.clone();
        rekeyed.record(PassphraseUse::Rekey, NOW);
        let evicting = health(
            &rekeyed,
            Met::Handshakes {
                certificates: &[],
                turned_away: 61,
            },
            NOW,
        );
        let note = evicting
            .notes
            .iter()
            .find(|n| n.contains("still dialling this node"))
            .expect("the evicting end is told what it is seeing");
        assert!(note.contains("offload invite"), "{note}");
        // The advice for the *other* end must not appear here, in either half.
        assert!(!note.contains("offload join"), "{note}");
        assert!(!note.contains("this device needs the invitation"), "{note}");
        assert!(
            !evicting
                .notes
                .iter()
                .any(|n| n.contains("the fleet has moved on")),
            "the evicted device's sentence must not reach the machine that evicted it"
        );

        // The control: the same node, having been refused by nobody, says none of it.
        assert!(!health(
            &founder,
            Met::Handshakes {
                certificates: &[],
                turned_away: 0
            },
            NOW
        )
        .notes
        .iter()
        .any(|n| n.contains("the fleet has moved on")));

        // …and so does a process that never dialled anything. `offload fleet` runs with no
        // daemon, so it has no business answering this question either way — which is why the
        // count lives inside `Met::Handshakes` and cannot be spelled for `NotAsked`.
        assert!(!health(&founder, Met::NotAsked, NOW)
            .notes
            .iter()
            .any(|n| n.contains("the fleet has moved on")));
    }

    #[test]
    fn a_device_that_never_saw_the_passphrase_is_not_told_to_go_and_find_it() {
        // The invite path's whole point is that the drawer stayed shut. Nagging an invited
        // laptop to `offload verify` would send somebody to use the secret on the wrong machine.
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let signing = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let invited = accept_invite(
            Invitation::new(
                founder
                    .invite(&signing, None, node(2), "laptop", NOW)
                    .expect("issue"),
            ),
            node(2),
            NOW,
        )
        .expect("accept");

        assert!(!health(
            &invited,
            Met::Handshakes {
                certificates: &[],
                turned_away: 0
            },
            NOW
        )
        .notes
        .iter()
        .any(|n| n.contains("passphrase has never been checked")));
        // The device the fleet was founded on is a different matter: it has the phrase and has
        // never read it back.
        assert!(health(
            &founder,
            Met::Handshakes {
                certificates: &[],
                turned_away: 0
            },
            NOW
        )
        .notes
        .iter()
        .any(|n| n.contains("passphrase has never been checked")));
    }

    #[test]
    fn a_peer_whose_certificate_is_running_out_is_named() {
        let key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&key, node(1), "desktop", NOW);
        let plain = FleetState::join(&key, node(2), "laptop", NOW);
        let late = NOW + Millis(29 * 24 * 60 * 60 * 1_000);

        let health = health(
            &founder,
            Met::Handshakes {
                certificates: std::slice::from_ref(&plain.membership),
                turned_away: 0,
            },
            late,
        );
        assert_eq!(health.expiring, vec!["laptop in 1 day".to_string()]);
    }

    #[test]
    fn re_founding_keeps_the_device_and_drops_the_fleet() {
        // ADR-0012: losing or replacing the fleet key must cost membership and nothing else,
        // which is why the key authenticates and never encrypts. What survives here is what
        // this device *owns* — its identity, its name, and the record of when the passphrase
        // was used on it.
        let old_key = fleet_key("abacus zoom yo-yo");
        let mut before = FleetState::found(&old_key, node(1), "desktop", NOW);
        before.revoke(&old_key, node(3), NOW).expect("revoke");

        let new_key = fleet_key("zebra puppy abacus");
        let later = NOW + Millis(1_000);
        let after = before.refound(&new_key, later);

        assert_ne!(after.fleet, before.fleet);
        assert_eq!(after.membership.member, before.membership.member);
        assert_eq!(after.membership.name, before.membership.name);
        assert_eq!(
            after.check(node(1), later).map_err(|e| e.to_string()),
            Ok(())
        );
        assert!(
            after.grants(later).contains(&Grant::Approve),
            "still the approver"
        );

        // The revocation is gone, and that is the point: it was a statement about a
        // certificate in a fleet that no longer exists.
        assert!(after.revocations.is_empty());
        assert!(!after.is_revoked(node(3)));

        // And the passphrase history follows the device.
        assert!(after.passphrase_uses.len() > before.passphrase_uses.len());
        assert_eq!(
            after.passphrase_uses.last().map(|u| u.what),
            Some(PassphraseUse::Rekey)
        );
    }

    #[test]
    fn a_device_left_out_of_a_rekey_needs_nothing_to_reach_it() {
        // The difference between this and `offload revoke`, and the reason it is called
        // convergent: an evicted device is not carrying a revoked certificate that some node
        // might not have heard about. It is holding a perfectly valid certificate for a fleet
        // that has no other members.
        let old_key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&old_key, node(1), "desktop", NOW);
        let evicted = FleetState::join(&old_key, node(2), "laptop", NOW);

        let new_key = fleet_key("zebra puppy abacus");
        let after = founder.refound(&new_key, NOW);

        // Its certificate still verifies — against a fleet key nobody else uses any more.
        assert_eq!(
            evicted.check(node(2), NOW).map_err(|e| e.to_string()),
            Ok(())
        );
        assert!(
            evicted.membership.verify(after.fleet, None, NOW).is_err(),
            "and means nothing in the new one"
        );
    }

    #[test]
    fn a_rekey_invitation_moves_a_device_only_if_its_own_fleet_signed_the_move() {
        // The question a joining device would otherwise have no way to answer: *is this my
        // fleet moving, or somebody else's fleet taking me?* An invitation to an unfamiliar
        // fleet is exactly what an attacker would send, so what settles it is a statement
        // signed by the key being replaced — which this device already holds.
        let old_key = fleet_key("abacus zoom yo-yo");
        let member = FleetState::join(&old_key, node(2), "laptop", NOW);
        let founder = FleetState::found(&old_key, node(1), "desktop", NOW);

        let new_key = fleet_key("zebra puppy abacus");
        let refounded = founder.refound(&new_key, NOW);
        let credentials = refounded
            .issue_for(
                &new_key,
                node(2),
                "laptop",
                offload_core::default_grants(),
                NOW,
            )
            .expect("issue");

        // Signed by the right key, naming both ends.
        let good = Succession::issue(old_key.signing_key(), member.fleet, refounded.fleet, NOW, 1);
        assert_eq!(good.verify(member.fleet, refounded.fleet), Ok(()));

        // Signed by somebody else: the whole point is that only the old passphrase can move a
        // device, because that is the authority that could already do anything to it.
        let stranger = fleet_key("puppy zebra abacus");
        let forged = Succession::issue(
            stranger.signing_key(),
            member.fleet,
            refounded.fleet,
            NOW,
            1,
        );
        assert!(forged.verify(member.fleet, refounded.fleet).is_err());

        // And naming a successor other than the one issuing the certificate: half a valid
        // statement is the dangerous half, so both ends are checked.
        assert!(good.verify(member.fleet, member.fleet).is_err());
        assert!(good.verify(refounded.fleet, refounded.fleet).is_err());

        // With the right one, the device moves and keeps everything it owns.
        let moved =
            accept_invite(Invitation::succeeding(credentials, good), node(2), NOW).expect("accept");
        assert_eq!(moved.fleet, refounded.fleet);
        assert_eq!(moved.membership.member, node(2));
    }

    #[test]
    fn an_invitation_token_carries_the_succession_intact() {
        let old_key = fleet_key("abacus zoom yo-yo");
        let founder = FleetState::found(&old_key, node(1), "desktop", NOW);
        let new_key = fleet_key("zebra puppy abacus");
        let refounded = founder.refound(&new_key, NOW);
        let credentials = refounded
            .issue_for(
                &new_key,
                node(2),
                "laptop",
                offload_core::default_grants(),
                NOW,
            )
            .expect("issue");
        let invitation = Invitation::succeeding(
            credentials,
            Succession::issue(
                old_key.signing_key(),
                founder.fleet,
                refounded.fleet,
                NOW,
                1,
            ),
        );

        let token = encode_invite(&invitation);
        assert_eq!(decode_invite(&token).expect("decode"), invitation);
    }
}
