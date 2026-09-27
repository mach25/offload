//! Who belongs to this fleet, and what they are allowed to be.
//!
//! ADR-0012: a fleet is a signing key derived from a passphrase, and membership is a
//! certificate that key signed. The property that decides the whole shape is **offline
//! verification at first contact**: mDNS means two nodes meet before either has gossiped, so
//! a peer must be able to accept a stranger using only the fleet public key it already
//! holds. A gossiped roster cannot do that; a signature can.
//!
//! Everything here is pure. Signing and verifying are functions of their inputs, entropy is
//! supplied by the caller, and expiry is checked against an injected `now` — so this stays
//! inside `offload-core`'s no-I/O, no-clock rule (ADR-0001). Owning both halves of the format
//! in one module is deliberate: sign and verify must agree byte for byte, and the cheapest
//! way to guarantee that is to give them one definition to disagree with.
//!
//! Two rules from ADR-0012 are load-bearing and easy to lose:
//!
//! * **Grants live on the certificate**, not in a node's own config, because `WorkPolicy` is
//!   self-asserted and a phone that should never host runs must not be able to promote itself
//!   by editing TOML. Its peers refuse it.
//! * **Revocation is never refutable by its subject.** The `incarnation` counter lets a node
//!   argue with peers about its liveness; pointed at membership, the same mechanism would let
//!   a revoked device talk its way back in.

pub mod passphrase;

use crate::id::NodeId;
use crate::time::Millis;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

pub use passphrase::{FleetKey, Passphrase, PassphraseError};

/// How long after issue a joining node's `HostRuns` grant stays dormant (ADR-0012).
///
/// The window is not about trusting the certificate — it verifies fine — but about the
/// notification that follows it. An enrolment nobody performed is visible within seconds;
/// probation is what keeps revoking ahead of the attacker rather than behind, by making the
/// grant that matters take effect after the alarm rather than before it.
pub const PROBATION: Millis = Millis(15 * 60 * 1_000);

/// How much of a probation window is left, phrased for somebody deciding whether to wait.
///
/// Every caller had this as `as_secs() / 60`, which renders the last fifty-nine seconds of every
/// probation as **`0 minutes`** — a number that reads as *no wait at all* in the one sentence
/// whose whole job is to say there is one. Five sites said it, including `offload explain`'s
/// per-node line, which is where somebody asks why a run has not been placed. Found in session
/// seventy-eight with `PROBATION` shortened for a walk, where it is the only thing the line ever
/// says; at fifteen minutes it is the last minute of every one.
#[must_use]
pub fn dormant_for(remaining: Millis) -> String {
    match remaining.as_secs() / 60 {
        0 => "less than another minute".to_string(),
        1 => "another minute".to_string(),
        minutes => format!("another {minutes} minutes"),
    }
}

/// A fleet's public identity: the verifying half of the key derived from its passphrase.
///
/// The private half is not represented here at all, and that is the point — it exists for the
/// seconds it takes to sign something and is never stored on a node (ADR-0012).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FleetId(NodeId);

impl FleetId {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        FleetId(NodeId::from_bytes(bytes))
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// Short form for humans comparing fingerprints. Never use for equality.
    #[must_use]
    pub fn short(&self) -> String {
        self.0.short()
    }

    fn verifying_key(&self) -> Result<VerifyingKey, MembershipError> {
        verifying_key(self.as_bytes())
    }
}

impl fmt::Display for FleetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// What a member is permitted to be, decided by whoever admitted it.
///
/// On the certificate rather than in node config: a node's own policy is self-asserted, and
/// the fleet needs a say a device cannot overrule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grant {
    /// May submit runs to the fleet.
    Submit,
    /// May carry notifications to a human (ADR-0010).
    Deliver,
    /// May host agent runs — the grant that turns membership into "executes code on your
    /// repositories with your credentials". Never issued by joining (ADR-0012).
    HostRuns,
    /// May approve enrolments and issue membership certificates under a delegation.
    Approve,
    /// May renew another member's **live** membership, restating it with fresh dates and nothing
    /// else (ADR-0069). Issued at the door: its power is bounded by the proof a renewal carries
    /// and by [`APPROVAL_LIFETIME`], not by who holds it — which is what lets a fleet renew itself
    /// with no particular device up.
    Renew,
}

impl fmt::Display for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Grant::Submit => "submit",
            Grant::Deliver => "deliver",
            Grant::HostRuns => "host-runs",
            Grant::Approve => "approve",
            Grant::Renew => "renew",
        })
    }
}

/// What a device receives by joining: everything except the ability to execute.
#[must_use]
pub fn default_grants() -> BTreeSet<Grant> {
    [Grant::Submit, Grant::Deliver, Grant::Renew]
        .into_iter()
        .collect()
}

/// How long a person's approval of a membership lasts, whatever its renewals say (ADR-0069 §3).
///
/// A protocol constant, because every node has to refuse the same certificates: a renewal
/// restates an approval and cannot make it younger, so after a year a membership needs a person
/// again (`offload reapprove`). The rebels' stolen shuttle code, refused.
pub const APPROVAL_LIFETIME: Millis = Millis(365 * 24 * 60 * 60 * 1_000);

/// How close to its year an approval has to be before it is listed and asked about, and how long
/// a person's decision to re-approve a device stands (ADR-0069 §3).
///
/// One month for both: long enough that a device opened once a fortnight is re-approved in time,
/// and short enough that a decision somebody made does not sit waiting for a device that turns up
/// a year later — by then it is a device nobody has looked at, which is what the year is for.
pub const REAPPROVAL_WINDOW: Millis = Millis(30 * 24 * 60 * 60 * 1_000);

/// Who signed a credential.
///
/// The fleet key is the root; an approver signs under a [`Delegation`] the fleet key issued.
/// An approver's identity is its `NodeId`, which *is* an ed25519 public key, so a chain needs
/// no key material beyond what membership already carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "issuer")]
pub enum Issuer {
    Fleet,
    Approver {
        node: NodeId,
    },
    /// A member renewing another's live membership under `Grant::Renew` (ADR-0069). Only ever a
    /// renewal: it restates the approval its [`RenewalProof`] carries and cannot say anything new.
    Renewer {
        node: NodeId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MembershipError {
    #[error("not a valid ed25519 key")]
    MalformedKey,
    #[error("signature does not verify against {issuer}")]
    BadSignature { issuer: String },
    #[error("that is not a DER-encoded ECDSA P-256 signature")]
    MalformedSignature,
    #[error("not a valid P-256 public key")]
    MalformedP256Key,
    /// Says how long ago, not when. `Millis`'s `Display` is a **duration** formatter — it
    /// exists so a deadline reads as `2h15m` — and this interpolated two *absolute* timestamps
    /// through it, so a lapsed certificate reported itself as
    /// `expired at 496980h44m, now 496980h44m`: two identical-looking numbers, neither of them
    /// a time anybody recognises, and the gap between them invisible at hour granularity.
    /// Measured in session seventy-eight on a node whose certificate was allowed to lapse,
    /// which is the ADR-0012 backstop working and therefore the one moment this message exists
    /// for. The elapsed span is a real duration, which is what the formatter is for.
    #[error("expired {} ago", now.saturating_sub(*expired_at))]
    Expired { expired_at: Millis, now: Millis },
    #[error("issued for fleet {found}, this is fleet {expected}")]
    WrongFleet { expected: String, found: String },
    #[error("issued by approver {node}, but no delegation for it was supplied")]
    NoDelegation { node: String },
    #[error("delegation is for approver {found}, certificate was issued by {expected}")]
    DelegationMismatch { expected: String, found: String },
    #[error("member {node} has been revoked")]
    Revoked { node: String },
    #[error(
        "issued by approver {approver} granting {grants}, which an approver may not issue — \
         that needs the fleet passphrase, or a renewal of a certificate the fleet key signed"
    )]
    NotIssuableByApprover { approver: String, grants: String },
    /// ADR-0069 §3: the membership's last approval by a person is over a year old.
    #[error(
        "approved {} ago, and an approval lasts a year — it needs a person again (`offload reapprove`)",
        now.saturating_sub(*approved_at)
    )]
    ApprovalTooOld { approved_at: Millis, now: Millis },
    #[error("renewed by {renewer}, but the renewal carries no proof of what it restates")]
    MissingProof { renewer: String },
    #[error("{node} renewed its own membership, which no node may do")]
    SelfRenewal { node: String },
    #[error("a renewal's proof must be an approval, and this one is itself a renewal")]
    NotAnApproval,
    #[error("renewal changes the membership's {what}, and a renewal may only restate it")]
    RenewalChanges { what: &'static str },
    #[error("renewed by {renewer}, whose own approval does not grant renew")]
    NotARenewer { renewer: String },
    #[error("delegation for approver {node} was not in force when it issued this")]
    DelegationNotInForce { node: String },
}

/// Everything a node needs to prove it belongs, verifiable with the fleet public key alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipCert {
    pub fleet: FleetId,
    pub member: NodeId,
    /// Cosmetic, for `offload nodes`. Signed anyway, so a peer cannot relabel a device.
    pub name: String,
    pub grants: BTreeSet<Grant>,
    pub issued_at: Millis,
    /// Short-lived on purpose: a node that never hears a revocation falls out of the fleet
    /// when this lapses. The backstop for eventually-consistent revocation.
    pub expires_at: Millis,
    pub serial: u64,
    pub issuer: Issuer,
    /// Whether `HostRuns` waits out [`PROBATION`] before it means anything.
    ///
    /// **Follows the grant, not the path** (`docs/pitfalls/membership-and-credentials.md`): set
    /// on any certificate that puts `HostRuns` in play when it was not already in force — which
    /// includes `offload invite --grant host-runs`, measured in session eighty-nine — and clear
    /// for the founding device, where the passphrase holder is at the keyboard and the alarm
    /// probation protects would be ringing in their own hand. This said an invite cleared it,
    /// which is what an invite *without* `host-runs` looks like: there is nothing to suppress.
    pub probation: bool,
    /// The fleet-signed certificate whose grants bound this one, for a renewal that carries
    /// more than an approver may issue on its own.
    ///
    /// This is ADR-0012's `may_issue` in the only form that also lets the backstop work.
    /// Mitigation 1 keeps `HostRuns` behind the passphrase, and the delegation carries no bound
    /// at all — so an approver, holding nothing but a delegation and its own key, could mint a
    /// certificate granting `HostRuns` to a device it controlled and every peer accepted it.
    ///
    /// The reason a plain bound is not enough is renewal. **Certificates are renewed on contact
    /// by an approver**, which is what makes an unheard revocation eventually bite, and a
    /// renewal copies the grants it is restating — so an approver forbidden from *ever* signing
    /// `HostRuns` cannot renew a host node either, and every host in the fleet needs the
    /// passphrase every thirty days. That is the same failure as having no renewal at all.
    ///
    /// So the distinction the verifier needs is between **minting** and **restating**, and this
    /// is what makes restating checkable offline: the renewal carries the fleet-signed
    /// certificate it descends from, and may not grant more than that certificate did. It is
    /// the *root* of the chain rather than the immediate predecessor, so a certificate renewed
    /// monthly for years stays one link long. Verified with its expiry ignored — a grant made
    /// two years ago is exactly what a renewal is restating — but with everything else checked,
    /// including that the fleet key signed it and that it names the same member.
    #[serde(default)]
    pub authority: Option<Box<MembershipCert>>,
    /// When a person last approved this membership (ADR-0069 §3): an enrolment, a new grant, or
    /// `offload reapprove`. **Copied by every renewal and never reset by one**, which is the
    /// whole point: a membership renewed monthly for three years was approved three years ago.
    ///
    /// `None` on a certificate issued before this existed, which reads it as its `issued_at`
    /// ([`Self::approved_at`]) — so nothing lapses on upgrade — and keeps the v1 signing bytes,
    /// so every certificate already in the fleet still verifies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<Millis>,
    /// What a [`Issuer::Renewer`] renewal restates, and who was entitled to restate it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof: Option<Box<RenewalProof>>,
    #[serde(with = "sig_hex")]
    signature: [u8; 64],
}

/// The proof a renewer's renewal carries (ADR-0069 §2): two **approvals**, never renewals, so a
/// renewal is one link long however many times the membership has been renewed.
///
/// Each approval comes with the delegation it was issued under, if an approver issued it, and
/// is checked against that delegation *as it stood when the approval was issued* — a delegation
/// that has since expired does not un-approve a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenewalProof {
    /// The approval being restated: same fleet, member, name and grants as the renewal.
    pub approval: MembershipCert,
    pub approval_delegation: Option<Delegation>,
    /// The renewer's own approval, which must grant `Renew` and be under a year old.
    pub renewer: MembershipCert,
    pub renewer_delegation: Option<Delegation>,
}

/// What a certificate is being issued *for*, separate from who signs it.
///
/// A struct rather than eight positional arguments, because the two `Millis` and the two
/// identities are exactly the kind of thing that gets silently transposed at a call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terms {
    pub fleet: FleetId,
    pub member: NodeId,
    pub name: String,
    pub grants: BTreeSet<Grant>,
    pub issued_at: Millis,
    /// How long from `issued_at` the certificate is good for. ADR-0012 says ~30 days, short
    /// enough that a revocation which never arrives still takes effect eventually.
    pub lifetime: Millis,
    pub serial: u64,
    /// See [`MembershipCert::probation`]. Defaults to on via [`Terms::joining`], because the
    /// path that should skip it is the rarer and more deliberate one.
    pub probation: bool,
    /// See [`MembershipCert::authority`]. `None` for anything the fleet key signs, and for an
    /// approver issuing within what an approver may issue — which is every enrolment.
    pub authority: Option<Box<MembershipCert>>,
    /// See [`MembershipCert::approved_at`]. `Some(issued_at)` from an enrolment; copied by a
    /// renewal; `None` only for the legacy form.
    pub approved_at: Option<Millis>,
    /// See [`MembershipCert::proof`]. Only a renewer's renewal has one.
    pub proof: Option<Box<RenewalProof>>,
}

/// The key an approver approves with (ADR-0069 §4).
///
/// `Node` is the approver's own ed25519 node key, which is every delegation before §4 and still
/// the default. `P256` is a second key held in secure hardware — StrongBox, a TEE, the Secure
/// Enclave, a TPM — which signs ECDSA P-256 and never leaves the chip. A second key rather than a
/// replacement because the node key cannot move into hardware: the daemon uses it for every
/// connection. The fleet key names it, so a peer checks an approval against it offline, exactly as
/// it checks a node-key approval. Whether the key really is in hardware is the holder's report
/// alone, and no peer acts on it: a node must not believe a peer about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IssuerKey {
    #[default]
    Node,
    P256 {
        /// The public key, SEC1-compressed.
        #[serde(with = "sec1_hex")]
        sec1: [u8; 33],
    },
}

impl IssuerKey {
    #[must_use]
    pub fn is_node(&self) -> bool {
        matches!(self, IssuerKey::Node)
    }

    /// A P-256 key from its SEC1 encoding, compressed or not, checked to be a point on the curve.
    pub fn p256_from_sec1(bytes: &[u8]) -> Result<Self, MembershipError> {
        let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(bytes)
            .map_err(|_| MembershipError::MalformedP256Key)?;
        let point = key.to_encoded_point(true);
        let sec1: [u8; 33] = point
            .as_bytes()
            .try_into()
            .map_err(|_| MembershipError::MalformedP256Key)?;
        Ok(IssuerKey::P256 { sec1 })
    }
}

/// A DER-encoded ECDSA P-256 signature — what Android's Keystore, the Secure Enclave and most
/// TPM stacks produce — as the fixed 64 bytes (`r ‖ s`) a certificate carries.
pub fn p256_signature_from_der(der: &[u8]) -> Result<[u8; 64], MembershipError> {
    let signature =
        p256::ecdsa::Signature::from_der(der).map_err(|_| MembershipError::MalformedSignature)?;
    signature
        .to_bytes()
        .as_slice()
        .try_into()
        .map_err(|_| MembershipError::MalformedSignature)
}

/// A fleet-signed statement that an approver may issue membership certificates.
///
/// This is what keeps the passphrase off the daily path: it is used to delegate once, and
/// then the approver does the enrolling (ADR-0012).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delegation {
    pub fleet: FleetId,
    pub approver: NodeId,
    pub issued_at: Millis,
    pub expires_at: Millis,
    pub serial: u64,
    /// The key this approver approves with (ADR-0069 §4). Absent on the wire for `Node`, so a
    /// delegation issued before §4 serialises and signs exactly as it always did.
    #[serde(default, skip_serializing_if = "IssuerKey::is_node")]
    pub issuer_key: IssuerKey,
    #[serde(with = "sig_hex")]
    signature: [u8; 64],
}

/// A fleet-signed statement that a member is out.
///
/// Gossiped like any other fact, owned by the fleet key, arbitrated by signature — and,
/// unlike liveness, **never refutable by its subject**.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revocation {
    pub fleet: FleetId,
    pub member: NodeId,
    pub revoked_at: Millis,
    pub serial: u64,
    #[serde(with = "sig_hex")]
    signature: [u8; 64],
}

/// The old fleet key naming the fleet that replaces it (ADR-0012's rekey).
///
/// The one thing that makes `offload rekey` usable rather than a pile of manual re-enrolments,
/// and the reason it needs a signature at all is a question the joining device would otherwise
/// have no way to answer: *is this my fleet moving, or somebody else's fleet taking me?* An
/// invitation to a fleet a device has never heard of is exactly what an attacker would send.
///
/// So the succession is signed by the key being replaced, which the device already holds. Only
/// somebody with the old passphrase can move a device to a new fleet — which is the same
/// authority that could already do anything else to it.
///
/// It is public material. Knowing that fleet A was succeeded by fleet B grants nothing: joining
/// B still needs a certificate B's key signed, and the evicted device is simply not issued one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Succession {
    /// The fleet being replaced.
    pub fleet: FleetId,
    /// The one replacing it.
    pub successor: FleetId,
    pub at: Millis,
    pub serial: u64,
    #[serde(with = "sig_hex")]
    signature: [u8; 64],
}

impl Succession {
    #[must_use]
    pub fn issue(
        fleet_key: &SigningKey,
        fleet: FleetId,
        successor: FleetId,
        at: Millis,
        serial: u64,
    ) -> Self {
        let message = succession_bytes(fleet, successor, at, serial);
        Succession {
            fleet,
            successor,
            at,
            serial,
            signature: fleet_key.sign(&message).to_bytes(),
        }
    }

    /// Does this say that `fleet` — the one the device belongs to now — was replaced by
    /// `successor`?
    ///
    /// Both ends are checked, because half of the statement is the dangerous half: a valid
    /// succession out of some *other* fleet says nothing about this device's, and one that
    /// names a successor other than the certificate's issuer would move the device somewhere
    /// the signer never authorised.
    pub fn verify(&self, fleet: FleetId, successor: FleetId) -> Result<(), MembershipError> {
        if self.fleet != fleet {
            return Err(MembershipError::WrongFleet {
                expected: fleet.short(),
                found: self.fleet.short(),
            });
        }
        if self.successor != successor {
            return Err(MembershipError::WrongFleet {
                expected: successor.short(),
                found: self.successor.short(),
            });
        }
        let message = succession_bytes(self.fleet, self.successor, self.at, self.serial);
        check(
            &fleet.verifying_key()?,
            &message,
            &self.signature,
            "fleet (succession)",
        )
    }
}

// -- signing -----------------------------------------------------------------------------

/// Domain separation, so a signature over one credential can never be replayed as another.
const MEMBERSHIP_CONTEXT: &[u8] = b"offload-membership-v1";
/// ADR-0069's certificates: `approved_at`, renewers and their proof. A different context rather
/// than extra fields under the old one, so a v1 certificate's bytes are exactly what they were.
const MEMBERSHIP_CONTEXT_V2: &[u8] = b"offload-membership-v2";
const DELEGATION_CONTEXT: &[u8] = b"offload-delegation-v1";
/// A delegation naming a key other than the approver's node key (ADR-0069 §4).
const DELEGATION_CONTEXT_V2: &[u8] = b"offload-delegation-v2";
const REVOCATION_CONTEXT: &[u8] = b"offload-revocation-v1";
const SUCCESSION_CONTEXT: &[u8] = b"offload-succession-v1";

/// Canonical bytes for signing.
///
/// Hand-rolled rather than serialised, because a signature must cover exactly what a verifier
/// reconstructs: a serde representation that gains a field, reorders a map, or changes how it
/// encodes an enum would silently invalidate every credential in the fleet. Each field is
/// length-prefixed so no two different field sets can produce the same bytes.
fn field(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

impl MembershipCert {
    /// The bytes a signature covers.
    ///
    /// Computed from the certificate itself so issuing and verifying cannot drift apart:
    /// there is one definition, and both halves call it.
    fn message(&self) -> Vec<u8> {
        // v1 exactly as it was for every certificate that has none of ADR-0069's fields, so the
        // certificates already in a fleet go on verifying (their digests are pinned below).
        let v2 = self.approved_at.is_some()
            || self.proof.is_some()
            || matches!(self.issuer, Issuer::Renewer { .. });
        let mut out = Vec::with_capacity(256);
        field(
            &mut out,
            if v2 {
                MEMBERSHIP_CONTEXT_V2
            } else {
                MEMBERSHIP_CONTEXT
            },
        );
        field(&mut out, self.fleet.as_bytes());
        field(&mut out, self.member.as_bytes());
        field(&mut out, self.name.as_bytes());
        // BTreeSet iterates in a defined order, which is why grants are a set and not a Vec.
        field(
            &mut out,
            &self.grants.iter().map(grant_tag).collect::<Vec<u8>>(),
        );
        field(&mut out, &self.issued_at.0.to_le_bytes());
        field(&mut out, &self.expires_at.0.to_le_bytes());
        field(&mut out, &self.serial.to_le_bytes());
        field(&mut out, &[u8::from(self.probation)]);
        // Bound by its signature rather than by its whole body: the authority is itself
        // fleet-signed, so 64 bytes name it unambiguously and keep this message a fixed size
        // however long the chain of renewals behind it is. Covering it at all is integrity
        // rather than authority — what makes an authority *mean* anything is `verify` checking
        // that the fleet key signed it for this same member, which an approver cannot forge.
        field(
            &mut out,
            self.authority
                .as_ref()
                .map_or(&[][..], |a| a.signature.as_slice()),
        );
        if v2 {
            field(&mut out, &self.approved_at().0.to_le_bytes());
            // The proof by its two approvals' signatures, for the authority's reason above.
            match &self.proof {
                Some(proof) => {
                    field(&mut out, proof.approval.signature.as_slice());
                    field(&mut out, proof.renewer.signature.as_slice());
                }
                None => {
                    field(&mut out, &[]);
                    field(&mut out, &[]);
                }
            }
        }
        match self.issuer {
            Issuer::Fleet => field(&mut out, b"fleet"),
            Issuer::Approver { node } => {
                field(&mut out, b"approver");
                field(&mut out, node.as_bytes());
            }
            Issuer::Renewer { node } => {
                field(&mut out, b"renewer");
                field(&mut out, node.as_bytes());
            }
        }
        out
    }

    /// When a person last approved this membership: [`Self::approved_at`], or the certificate's
    /// own `issued_at` for one issued before the field existed (ADR-0069 §5).
    #[must_use]
    pub fn approved_at(&self) -> Millis {
        self.approved_at.unwrap_or(self.issued_at)
    }
}

const fn grant_tag(g: &Grant) -> u8 {
    match g {
        Grant::Submit => 1,
        Grant::Deliver => 2,
        Grant::HostRuns => 3,
        Grant::Approve => 4,
        Grant::Renew => 5,
    }
}

fn delegation_bytes(
    fleet: FleetId,
    approver: NodeId,
    issued_at: Millis,
    expires_at: Millis,
    serial: u64,
    issuer_key: &IssuerKey,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(160);
    // Versioned as the certificate's bytes are: a `Node` delegation signs the v1 bytes unchanged,
    // so every delegation issued before ADR-0069 §4 still verifies, and a `P256` one is signed
    // under a context no v1 verifier could mistake for its own.
    field(
        &mut out,
        if issuer_key.is_node() {
            DELEGATION_CONTEXT
        } else {
            DELEGATION_CONTEXT_V2
        },
    );
    field(&mut out, fleet.as_bytes());
    field(&mut out, approver.as_bytes());
    field(&mut out, &issued_at.0.to_le_bytes());
    field(&mut out, &expires_at.0.to_le_bytes());
    field(&mut out, &serial.to_le_bytes());
    if let IssuerKey::P256 { sec1 } = issuer_key {
        field(&mut out, b"p256");
        field(&mut out, sec1);
    }
    out
}

fn revocation_bytes(fleet: FleetId, member: NodeId, revoked_at: Millis, serial: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    field(&mut out, REVOCATION_CONTEXT);
    field(&mut out, fleet.as_bytes());
    field(&mut out, member.as_bytes());
    field(&mut out, &revoked_at.0.to_le_bytes());
    field(&mut out, &serial.to_le_bytes());
    out
}

fn succession_bytes(fleet: FleetId, successor: FleetId, at: Millis, serial: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    field(&mut out, SUCCESSION_CONTEXT);
    field(&mut out, fleet.as_bytes());
    field(&mut out, successor.as_bytes());
    field(&mut out, &at.0.to_le_bytes());
    field(&mut out, &serial.to_le_bytes());
    out
}

fn verifying_key(bytes: &[u8; 32]) -> Result<VerifyingKey, MembershipError> {
    VerifyingKey::from_bytes(bytes).map_err(|_| MembershipError::MalformedKey)
}

fn check(
    key: &VerifyingKey,
    message: &[u8],
    signature: &[u8; 64],
    issuer: &str,
) -> Result<(), MembershipError> {
    key.verify_strict(message, &Signature::from_bytes(signature))
        .map_err(|_| MembershipError::BadSignature {
            issuer: issuer.to_string(),
        })
}

fn check_p256(
    sec1: &[u8; 33],
    message: &[u8],
    signature: &[u8; 64],
    issuer: &str,
) -> Result<(), MembershipError> {
    use p256::ecdsa::signature::Verifier;
    let bad = || MembershipError::BadSignature {
        issuer: issuer.to_string(),
    };
    let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(sec1).map_err(|_| bad())?;
    let signature = p256::ecdsa::Signature::from_slice(signature).map_err(|_| bad())?;
    key.verify(message, &signature).map_err(|_| bad())
}

impl MembershipCert {
    /// Issue a certificate. `key` is the fleet key, or an approver's node key under a
    /// delegation — `issuer` must say which, because it is covered by the signature.
    pub fn issue(key: &SigningKey, issuer: Issuer, terms: Terms) -> Self {
        let cert = Self::unsigned(issuer, terms);
        let signature = key.sign(&cert.message()).to_bytes();
        cert.signed(signature)
    }

    /// A certificate waiting for a signature made somewhere else — in secure hardware, under a
    /// delegation naming a P-256 key (ADR-0069 §4). It verifies as nothing until
    /// [`Self::signed`] is given a signature over [`Self::signing_bytes`].
    #[must_use]
    pub fn unsigned(issuer: Issuer, terms: Terms) -> Self {
        MembershipCert {
            fleet: terms.fleet,
            member: terms.member,
            name: terms.name,
            grants: terms.grants,
            issued_at: terms.issued_at,
            expires_at: terms.issued_at + terms.lifetime,
            serial: terms.serial,
            issuer,
            probation: terms.probation,
            authority: terms.authority,
            approved_at: terms.approved_at,
            proof: terms.proof,
            signature: [0u8; 64],
        }
    }

    /// What a signer signs: the one definition [`Self::verify`] checks against. Public for the
    /// host that holds a hardware key, which computes the bytes with this binary rather than a
    /// second implementation of them (ADR-0069 §4).
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        self.message()
    }

    /// This certificate with `signature` in place.
    #[must_use]
    pub fn signed(mut self, signature: [u8; 64]) -> Self {
        self.signature = signature;
        self
    }

    /// Is this credential valid for `fleet`, right now, given whatever delegation came with
    /// it?
    ///
    /// Everything a peer needs is here: no roster, no gossip, no network. That is the
    /// property mDNS discovery depends on.
    pub fn verify(
        &self,
        fleet: FleetId,
        delegation: Option<&Delegation>,
        now: Millis,
    ) -> Result<(), MembershipError> {
        if self.fleet != fleet {
            return Err(MembershipError::WrongFleet {
                expected: fleet.short(),
                found: self.fleet.short(),
            });
        }
        if now >= self.expires_at {
            return Err(MembershipError::Expired {
                expired_at: self.expires_at,
                now,
            });
        }

        // ADR-0069 §3, for every kind of certificate: an approval lasts a year, and no renewal
        // can make it younger.
        let approved_at = self.approved_at();
        if now.saturating_sub(approved_at) > APPROVAL_LIFETIME {
            return Err(MembershipError::ApprovalTooOld { approved_at, now });
        }

        let message = self.message();

        match self.issuer {
            Issuer::Fleet => check(&fleet.verifying_key()?, &message, &self.signature, "fleet"),
            Issuer::Renewer { node } => self.verify_renewal(fleet, node, &message, now),
            Issuer::Approver { node } => {
                // The chain: the fleet key said this approver may issue, and the approver
                // signed this certificate. Both links are checked, and both against the one
                // key the verifier already holds.
                let delegation = delegation
                    .ok_or_else(|| MembershipError::NoDelegation { node: node.short() })?;
                if delegation.approver != node {
                    return Err(MembershipError::DelegationMismatch {
                        expected: node.short(),
                        found: delegation.approver.short(),
                    });
                }
                // Against the delegation **as it stood when this was issued** (ADR-0069's step-3
                // amendment, the owner's decision): a delegation lasts a year from `init`, and
                // checking it as of `now` meant every certificate an approver had issued failed
                // at once on that day — invitations, renewals, re-approvals — while each was
                // still inside its own month and its approval's year, which are what bound it.
                // The approver's power to issue *new* ones still ends with the delegation:
                // `FleetState` checks it as of `now` before signing anything.
                delegation.verify_signature(fleet)?;
                if !delegation.was_in_force(self.issued_at) {
                    return Err(MembershipError::DelegationNotInForce { node: node.short() });
                }
                delegation.check_approver_signature(
                    &message,
                    &self.signature,
                    &format!("approver {}", node.short()),
                )?;
                self.within_what_an_approver_may_issue(fleet, node)
            }
        }
    }

    /// A renewer's renewal (ADR-0069 §2): it restates a live membership's approval, with fresh
    /// dates and nothing else, and the renewer was entitled to. Everything here is checked offline
    /// against the fleet key. Revocations are the caller's (`FleetState`), which must ask about
    /// [`Self::renewer`] as well as the member.
    fn verify_renewal(
        &self,
        fleet: FleetId,
        renewer: NodeId,
        message: &[u8],
        now: Millis,
    ) -> Result<(), MembershipError> {
        let proof = self
            .proof
            .as_ref()
            .ok_or_else(|| MembershipError::MissingProof {
                renewer: renewer.short(),
            })?;
        // The backstop ADR-0012 leans on twice: a node that could re-sign itself would never
        // fall out of a fleet it had been removed from.
        if renewer == self.member {
            return Err(MembershipError::SelfRenewal {
                node: renewer.short(),
            });
        }

        // What is restated: an approval, for this membership, exactly.
        let approval = &proof.approval;
        approval.verify_approval(fleet, proof.approval_delegation.as_ref())?;
        if approval.member != self.member {
            return Err(MembershipError::RenewalChanges { what: "member" });
        }
        if approval.name != self.name {
            return Err(MembershipError::RenewalChanges { what: "name" });
        }
        if approval.grants != self.grants {
            return Err(MembershipError::RenewalChanges { what: "grants" });
        }
        if approval.approved_at() != self.approved_at() {
            return Err(MembershipError::RenewalChanges {
                what: "approval date",
            });
        }
        // Probation may not be shed early: while the approval's probation is still running at
        // the renewal's issue, the renewal carries it.
        if approval.probation && self.issued_at < approval.issued_at + PROBATION && !self.probation
        {
            return Err(MembershipError::RenewalChanges { what: "probation" });
        }

        // Who restated it: a member a person approved within the year, with `Renew`.
        let entitled = &proof.renewer;
        entitled.verify_approval(fleet, proof.renewer_delegation.as_ref())?;
        if entitled.member != renewer || !entitled.grants.contains(&Grant::Renew) {
            return Err(MembershipError::NotARenewer {
                renewer: renewer.short(),
            });
        }
        let renewer_approved = entitled.approved_at();
        if now.saturating_sub(renewer_approved) > APPROVAL_LIFETIME {
            return Err(MembershipError::ApprovalTooOld {
                approved_at: renewer_approved,
                now,
            });
        }

        check(
            &verifying_key(renewer.as_bytes())?,
            message,
            &self.signature,
            &format!("renewer {}", renewer.short()),
        )
    }

    /// Is this an **approval** — a certificate a person issued, directly with the fleet key or
    /// through an approver — that verifies? Its expiry is not checked: a renewal restating it is
    /// what keeps it current (the same reasoning as [`Self::authority`]). An approver's delegation
    /// is checked as it stood when the approval was issued.
    fn verify_approval(
        &self,
        fleet: FleetId,
        delegation: Option<&Delegation>,
    ) -> Result<(), MembershipError> {
        if self.fleet != fleet {
            return Err(MembershipError::WrongFleet {
                expected: fleet.short(),
                found: self.fleet.short(),
            });
        }
        let message = self.message();
        match self.issuer {
            Issuer::Renewer { .. } => Err(MembershipError::NotAnApproval),
            Issuer::Fleet => check(
                &fleet.verifying_key()?,
                &message,
                &self.signature,
                "fleet (approval)",
            ),
            Issuer::Approver { node } => {
                let delegation = delegation
                    .ok_or_else(|| MembershipError::NoDelegation { node: node.short() })?;
                if delegation.approver != node {
                    return Err(MembershipError::DelegationMismatch {
                        expected: node.short(),
                        found: delegation.approver.short(),
                    });
                }
                if !delegation.was_in_force(self.issued_at) {
                    return Err(MembershipError::DelegationNotInForce { node: node.short() });
                }
                delegation.verify_signature(fleet)?;
                delegation.check_approver_signature(
                    &message,
                    &self.signature,
                    &format!("approver {} (approval)", node.short()),
                )?;
                self.within_what_an_approver_may_issue(fleet, node)
            }
        }
    }

    /// The approval behind this certificate, with the delegation it was issued under: the
    /// certificate itself unless a renewer renewed it, in which case the approval its proof
    /// carries. `own_delegation` is the one that came with this certificate. What a renewer puts
    /// in a [`RenewalProof`] (ADR-0069 §2).
    #[must_use]
    pub fn approval_of(
        &self,
        own_delegation: Option<&Delegation>,
    ) -> (MembershipCert, Option<Delegation>) {
        match (&self.issuer, &self.proof) {
            (Issuer::Renewer { .. }, Some(proof)) => {
                (proof.approval.clone(), proof.approval_delegation.clone())
            }
            _ => (self.clone(), own_delegation.cloned()),
        }
    }

    /// The member whose `Renew` a renewal leans on, for the revocation check a caller owes it.
    #[must_use]
    pub fn renewer(&self) -> Option<NodeId> {
        match self.issuer {
            Issuer::Renewer { node } => Some(node),
            _ => None,
        }
    }

    /// What an approver is allowed to have put in this certificate.
    ///
    /// [`default_grants`] freely — that is enrolment, and it is the whole reason approvers
    /// exist — plus whatever a certificate **the fleet key signed for this same member** already
    /// granted, which is renewal. ADR-0012 mitigation 1 is the rule being kept: `HostRuns` turns
    /// membership into "runs agents on your repositories with your credentials", and it stays
    /// behind the passphrase. Without this the delegation bounded *who* may issue and nothing
    /// about *what*, so one compromised approver was one certificate away from a device that
    /// receives and executes the fleet's work.
    ///
    /// `Approve` is treated the same way, which is the stricter of the two positions ADR-0012
    /// names — it accepts that "a compromised approver can breed more of them" and adds that "a
    /// fleet unwilling to accept it should grant `Approve` from the passphrase only". The strict
    /// reading is free here: nothing in this tree issues an approver-to-approver *delegation*,
    /// and a certificate granting `Approve` without one cannot issue anything, so the loose
    /// reading would buy a capability nobody can use at the cost of the escalation being real.
    fn within_what_an_approver_may_issue(
        &self,
        fleet: FleetId,
        approver: NodeId,
    ) -> Result<(), MembershipError> {
        let mut allowed = default_grants();
        if let Some(authority) = &self.authority {
            // An authority is only an authority if the fleet key signed it, for this member, in
            // this fleet. Its *expiry* is deliberately not checked: the grant it carries was
            // made once and the renewal in hand is what keeps it current, so requiring the
            // original to be live would mean it could only be renewed while it did not need to
            // be. Everything else is checked, and an approver-issued authority is refused
            // outright — otherwise a chain could launder a grant nobody with the passphrase ever
            // made.
            if authority.fleet != fleet {
                return Err(MembershipError::WrongFleet {
                    expected: fleet.short(),
                    found: authority.fleet.short(),
                });
            }
            if authority.member == self.member && authority.issuer == Issuer::Fleet {
                check(
                    &fleet.verifying_key()?,
                    &authority.message(),
                    &authority.signature,
                    "fleet (authority)",
                )?;
                allowed.extend(authority.grants.iter().copied());
            }
        }
        let excess: Vec<String> = self
            .grants
            .difference(&allowed)
            .map(ToString::to_string)
            .collect();
        if excess.is_empty() {
            Ok(())
        } else {
            Err(MembershipError::NotIssuableByApprover {
                approver: approver.short(),
                grants: excess.join(", "),
            })
        }
    }

    /// What this certificate means *right now*, which is not always what it says.
    ///
    /// A joining node's `HostRuns` is dormant until the certificate is [`PROBATION`] old
    /// (ADR-0012). Nothing else is time-dependent, and nothing else should be: a grant that
    /// comes and goes for reasons a peer cannot reconstruct from the certificate is a grant
    /// two nodes will disagree about.
    #[must_use]
    pub fn effective_grants(&self, now: Millis) -> BTreeSet<Grant> {
        let mut grants = self.grants.clone();
        if self.probation && now < self.issued_at + PROBATION {
            grants.remove(&Grant::HostRuns);
        }
        grants
    }

    /// May this member do `grant`, now? The check every caller wants — `self.grants` is the
    /// certificate's text, this is its effect.
    #[must_use]
    pub fn granted(&self, grant: Grant, now: Millis) -> bool {
        self.effective_grants(now).contains(&grant)
    }

    /// When probation lifts, or `None` if nothing is waiting on it.
    #[must_use]
    pub fn probation_until(&self, now: Millis) -> Option<Millis> {
        let until = self.issued_at + PROBATION;
        let waiting = self.probation && self.grants.contains(&Grant::HostRuns) && now < until;
        waiting.then_some(until)
    }

    #[must_use]
    pub fn is_expired(&self, now: Millis) -> bool {
        now >= self.expires_at
    }

    /// Is this certificate close enough to lapsing to be worth asking about?
    ///
    /// A quarter of its life, which on ADR-0012's thirty days is a week: long enough that a
    /// laptop opened once a fortnight still renews in time, short enough that renewal is not
    /// something the fleet is doing constantly. A node that is *already* expired is past
    /// asking — its peers will not admit it, so there is nobody left to ask, and the way back
    /// is the passphrase.
    #[must_use]
    pub fn is_due_for_renewal(&self, now: Millis) -> bool {
        let life = self.expires_at.0.saturating_sub(self.issued_at.0);
        !self.is_expired(now) && self.expires_at.0.saturating_sub(now.0) <= life / 4
    }

    /// The same certificate again, with a fresh clock. Everything else is copied.
    ///
    /// **Renewal changes nothing but time**, which is what makes it safe to do without asking
    /// a human: the member, the name and the grants are the ones already signed, so an
    /// approver renewing a peer is restating a decision somebody already made rather than
    /// making one. A renewal that could widen a grant would be an enrolment wearing a
    /// disguise, and it would happen automatically, in the background, on a schedule.
    ///
    /// Probation is the one field that is *not* copied verbatim, because it is measured from
    /// `issued_at` and renewal moves that. A certificate still serving its probation keeps it
    /// — and so restarts it, which is the direction to be wrong in — and one that has served
    /// it out drops the flag, because re-imposing fifteen minutes of dormancy on a device that
    /// has been hosting runs for a month would be a monthly outage nobody could explain.
    #[must_use]
    pub fn renewal(&self, now: Millis, lifetime: Millis) -> Terms {
        Terms {
            fleet: self.fleet,
            member: self.member,
            name: self.name.clone(),
            grants: self.grants.clone(),
            issued_at: now,
            lifetime,
            serial: now.0,
            probation: self.probation && now < self.issued_at + PROBATION,
            // The root of the chain, not the link before it: a certificate renewed every month
            // for two years stays one link long, and the thing a renewal has to prove is what
            // the *fleet key* once granted rather than what last month's renewal said.
            authority: match (&self.issuer, &self.proof) {
                (Issuer::Fleet, _) => Some(Box::new(self.clone())),
                // A renewer's renewal has no authority of its own; its approval is where the
                // grants were made, so an approver renewing it next restates what that approval
                // (or the fleet key behind it) granted — or it could not renew a host.
                (Issuer::Renewer { .. }, Some(proof)) if proof.approval.issuer == Issuer::Fleet => {
                    Some(Box::new(proof.approval.clone()))
                }
                (Issuer::Renewer { .. }, Some(proof)) => proof.approval.authority.clone(),
                _ => self.authority.clone(),
            },
            // Copied, never reset (ADR-0069 §3): renewing is not approving.
            approved_at: Some(self.approved_at()),
            proof: None,
        }
    }

    /// A renewer's renewal of this membership (ADR-0069 §2): the same terms with fresh dates,
    /// carrying the approval it restates and the renewer's own. The caller has checked that this
    /// certificate is live and not revoked — a renewer renews nothing else — and signs the result
    /// with its node key as [`Issuer::Renewer`].
    #[must_use]
    pub fn renewal_by_renewer(&self, proof: RenewalProof, now: Millis, lifetime: Millis) -> Terms {
        Terms {
            authority: None,
            proof: Some(Box::new(proof)),
            ..self.renewal(now, lifetime)
        }
    }

    /// When the approval behind this membership runs out, whatever its renewals say.
    #[must_use]
    pub fn approval_expires_at(&self) -> Millis {
        self.approved_at() + APPROVAL_LIFETIME
    }

    /// Is the approval behind this live membership within [`REAPPROVAL_WINDOW`] of its year?
    ///
    /// The approval's clock, not the certificate's: a certificate renewed yesterday still dies
    /// with its approval, and a member whose certificate is not due has to start asking anyway,
    /// or its first renewal attempt comes too late to be a re-approval.
    #[must_use]
    pub fn is_due_for_reapproval(&self, now: Millis) -> bool {
        !self.is_expired(now)
            && self.approval_expires_at().0.saturating_sub(now.0) <= REAPPROVAL_WINDOW.0
    }

    /// A person's fresh approval of this membership (ADR-0069 §3): [`Self::renewal`] with
    /// `approved_at` set to `now`, and nothing else changed.
    ///
    /// Signed by an approver or the fleet key, never by a renewer — a renewer restates, and this
    /// is the one act a renewal is defined as not being. The grants are the ones already held and
    /// the authority behind them is carried as a renewal carries it, so re-approving a host
    /// restates the `host-runs` the fleet key once granted rather than minting it.
    #[must_use]
    pub fn reapproval(&self, now: Millis, lifetime: Millis) -> Terms {
        Terms {
            approved_at: Some(now),
            ..self.renewal(now, lifetime)
        }
    }
}

/// ADR-0012's ~30 days: long enough not to be a chore, short enough that a node which never
/// hears a revocation falls out of the fleet by itself. Explicitly a guess awaiting real
/// usage — see the ADR's consequences.
pub const CERT_LIFETIME: Millis = Millis(30 * 24 * 60 * 60 * 1_000);

impl Terms {
    /// A device enrolling itself: least privilege at the door, and probation behind it.
    ///
    /// `HostRuns` — the grant that turns membership into "runs agents on your repositories
    /// with your credentials" — is never issued by joining. It is granted afterwards, on
    /// purpose, so that a silent enrolment does not hand over the thing an attacker wants.
    #[must_use]
    pub fn joining(fleet: FleetId, member: NodeId, name: impl Into<String>, now: Millis) -> Terms {
        Terms {
            fleet,
            member,
            name: name.into(),
            grants: default_grants(),
            issued_at: now,
            lifetime: CERT_LIFETIME,
            serial: now.0,
            probation: true,
            authority: None,
            // An enrolment is a person's approval (ADR-0069 §3).
            approved_at: Some(now),
            proof: None,
        }
    }

    /// The device founding the fleet: everything, immediately.
    ///
    /// It gets `HostRuns` because the passphrase holder is at its keyboard, and `Approve`
    /// because ADR-0012 wants the delegation minted while the passphrase is still on screen
    /// — a fleet whose only enrolment path is a secret in a drawer is one where the drawer
    /// gets opened routinely. Probation would only delay the owner from their own device.
    #[must_use]
    pub fn founding(fleet: FleetId, member: NodeId, name: impl Into<String>, now: Millis) -> Terms {
        let mut grants = default_grants();
        grants.insert(Grant::HostRuns);
        grants.insert(Grant::Approve);
        Terms {
            grants,
            probation: false,
            ..Terms::joining(fleet, member, name, now)
        }
    }
}

impl Delegation {
    pub fn issue(
        fleet_key: &SigningKey,
        fleet: FleetId,
        approver: NodeId,
        issued_at: Millis,
        lifetime: Millis,
        serial: u64,
    ) -> Self {
        Self::issue_with_key(
            fleet_key,
            fleet,
            approver,
            IssuerKey::Node,
            issued_at,
            lifetime,
            serial,
        )
    }

    /// [`Self::issue`], naming the key the approver approves with (ADR-0069 §4).
    pub fn issue_with_key(
        fleet_key: &SigningKey,
        fleet: FleetId,
        approver: NodeId,
        issuer_key: IssuerKey,
        issued_at: Millis,
        lifetime: Millis,
        serial: u64,
    ) -> Self {
        let expires_at = issued_at + lifetime;
        let message = delegation_bytes(fleet, approver, issued_at, expires_at, serial, &issuer_key);
        Delegation {
            fleet,
            approver,
            issued_at,
            expires_at,
            serial,
            issuer_key,
            signature: fleet_key.sign(&message).to_bytes(),
        }
    }

    /// Check `signature` over `message` as this delegation's approver signing — with its node
    /// key, or with the hardware key the fleet key named for it (ADR-0069 §4). The one place an
    /// approver's signature is checked, so the two kinds of key cannot be told apart anywhere
    /// else.
    fn check_approver_signature(
        &self,
        message: &[u8],
        signature: &[u8; 64],
        issuer: &str,
    ) -> Result<(), MembershipError> {
        match &self.issuer_key {
            IssuerKey::Node => check(
                &verifying_key(self.approver.as_bytes())?,
                message,
                signature,
                issuer,
            ),
            IssuerKey::P256 { sec1 } => check_p256(sec1, message, signature, issuer),
        }
    }

    pub fn verify(&self, fleet: FleetId, now: Millis) -> Result<(), MembershipError> {
        if now >= self.expires_at {
            return Err(MembershipError::Expired {
                expired_at: self.expires_at,
                now,
            });
        }
        self.verify_signature(fleet)
    }

    /// Was this delegation in force at `at` — for an approval checked long after it was issued
    /// (ADR-0069 §2), where what matters is whether the approver could issue *then*.
    #[must_use]
    pub fn was_in_force(&self, at: Millis) -> bool {
        self.issued_at <= at && at < self.expires_at
    }

    /// The fleet key signed this delegation, for this fleet; nothing about time.
    pub fn verify_signature(&self, fleet: FleetId) -> Result<(), MembershipError> {
        if self.fleet != fleet {
            return Err(MembershipError::WrongFleet {
                expected: fleet.short(),
                found: self.fleet.short(),
            });
        }
        let message = delegation_bytes(
            self.fleet,
            self.approver,
            self.issued_at,
            self.expires_at,
            self.serial,
            &self.issuer_key,
        );
        check(
            &fleet.verifying_key()?,
            &message,
            &self.signature,
            "fleet (delegation)",
        )
    }
}

impl Revocation {
    pub fn issue(
        fleet_key: &SigningKey,
        fleet: FleetId,
        member: NodeId,
        revoked_at: Millis,
        serial: u64,
    ) -> Self {
        let message = revocation_bytes(fleet, member, revoked_at, serial);
        Revocation {
            fleet,
            member,
            revoked_at,
            serial,
            signature: fleet_key.sign(&message).to_bytes(),
        }
    }

    pub fn verify(&self, fleet: FleetId) -> Result<(), MembershipError> {
        if self.fleet != fleet {
            return Err(MembershipError::WrongFleet {
                expected: fleet.short(),
                found: self.fleet.short(),
            });
        }
        let message = revocation_bytes(self.fleet, self.member, self.revoked_at, self.serial);
        check(
            &fleet.verifying_key()?,
            &message,
            &self.signature,
            "fleet (revocation)",
        )
    }

    /// Deliberately **not** `refute`. There is no such operation, and the absence is the
    /// point: a node may argue with peers about whether it is alive, and may never argue
    /// about whether it is a member.
    #[must_use]
    pub fn covers(&self, node: NodeId) -> bool {
        self.member == node
    }
}

/// Hex, so a certificate is legible in a config file and in `sqlite3` output — the same
/// argument ADR-0009 made for the state store.
mod sig_hex {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(sig: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(sig))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        let text = String::deserialize(d)?;
        let bytes = hex::decode(&text).map_err(serde::de::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("signature must be 64 bytes"))
    }
}

mod sec1_hex {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(key: &[u8; 33], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(key))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 33], D::Error> {
        let text = String::deserialize(d)?;
        let bytes = hex::decode(&text).map_err(serde::de::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("a compressed P-256 key is 33 bytes"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_probation_with_seconds_left_does_not_say_zero_minutes() {
        use super::{dormant_for, Millis};
        // The whole point of the line is that there is a wait; "another 0 minutes" says there
        // is not. Every caller divided seconds by sixty, so the last fifty-nine seconds of
        // every probation read as no wait at all.
        assert_eq!(dormant_for(Millis(59 * 1_000)), "less than another minute");
        assert_eq!(dormant_for(Millis(0)), "less than another minute");
        assert_eq!(dormant_for(Millis(60 * 1_000)), "another minute");
        assert_eq!(dormant_for(Millis(119 * 1_000)), "another minute");
        assert_eq!(dormant_for(Millis(14 * 60 * 1_000)), "another 14 minutes");
    }

    use super::*;

    const DAY: Millis = Millis(86_400_000);

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn node_of(key: &SigningKey) -> NodeId {
        NodeId::from_bytes(key.verifying_key().to_bytes())
    }

    fn fleet_of(key: &SigningKey) -> FleetId {
        FleetId::from_bytes(key.verifying_key().to_bytes())
    }

    fn terms(fleet: FleetId, member: NodeId, name: &str) -> Terms {
        Terms {
            fleet,
            member,
            name: name.to_string(),
            grants: default_grants(),
            issued_at: Millis(1_000),
            lifetime: DAY,
            serial: 1,
            probation: false,
            authority: None,
            approved_at: None,
            proof: None,
        }
    }

    fn cert_for(fleet_key: &SigningKey, member: NodeId, grants: BTreeSet<Grant>) -> MembershipCert {
        MembershipCert::issue(
            fleet_key,
            Issuer::Fleet,
            Terms {
                grants,
                ..terms(fleet_of(fleet_key), member, "laptop")
            },
        )
    }

    #[test]
    fn a_peer_verifies_a_stranger_with_only_the_fleet_public_key() {
        // The property the whole design rests on: mDNS puts two nodes in contact before
        // either has gossiped, so verification cannot need a roster.
        let fleet_key = key(1);
        let member = node_of(&key(2));
        let cert = cert_for(&fleet_key, member, default_grants());

        assert_eq!(
            cert.verify(fleet_of(&fleet_key), None, Millis(2_000)),
            Ok(())
        );
    }

    #[test]
    fn a_certificate_from_another_fleet_is_refused_by_name() {
        // Two fleets on one LAN is the normal case for a work laptop (ADR-0012).
        let ours = key(1);
        let theirs = key(9);
        let cert = cert_for(&theirs, node_of(&key(2)), default_grants());

        assert!(matches!(
            cert.verify(fleet_of(&ours), None, Millis(2_000)),
            Err(MembershipError::WrongFleet { .. })
        ));
    }

    #[test]
    fn expiry_is_the_backstop_for_revocation_that_never_arrives() {
        let fleet_key = key(1);
        let cert = cert_for(&fleet_key, node_of(&key(2)), default_grants());
        assert_eq!(
            cert.expires_at,
            Millis(1_000) + DAY,
            "issued at 1_000, runs a day"
        );

        assert!(!cert.is_expired(cert.expires_at - Millis(1)));
        assert!(cert.is_expired(cert.expires_at), "expiry is inclusive");
        assert!(matches!(
            cert.verify(fleet_of(&fleet_key), None, cert.expires_at),
            Err(MembershipError::Expired { .. })
        ));
    }

    #[test]
    fn tampering_with_a_grant_invalidates_the_certificate() {
        // The reason grants live on the certificate: a node must not be able to promote
        // itself to HostRuns by editing what it holds.
        let fleet_key = key(1);
        let mut cert = cert_for(&fleet_key, node_of(&key(2)), default_grants());
        assert!(!cert.granted(Grant::HostRuns, Millis(2_000)));

        cert.grants.insert(Grant::HostRuns);

        assert!(matches!(
            cert.verify(fleet_of(&fleet_key), None, Millis(2_000)),
            Err(MembershipError::BadSignature { .. })
        ));
    }

    #[test]
    fn tampering_with_the_name_invalidates_it_too() {
        // Cosmetic, and signed anyway — otherwise a peer could relabel a device in
        // `offload nodes` and make the wrong machine look like the right one.
        let fleet_key = key(1);
        let mut cert = cert_for(&fleet_key, node_of(&key(2)), default_grants());
        cert.name = "desktop".into();

        assert!(cert
            .verify(fleet_of(&fleet_key), None, Millis(2_000))
            .is_err());
    }

    #[test]
    fn joining_never_grants_the_ability_to_execute() {
        // ADR-0012's highest-value mitigation: being in the fleet is not the thing an
        // attacker wants from the fleet.
        let grants = default_grants();
        assert!(grants.contains(&Grant::Submit));
        assert!(grants.contains(&Grant::Deliver));
        assert!(!grants.contains(&Grant::HostRuns));
        assert!(!grants.contains(&Grant::Approve));
    }

    #[test]
    fn a_joining_device_holds_its_host_runs_grant_dormant() {
        // Probation buys the window ADR-0012's enrolment notification needs: an enrolment
        // nobody performed is revocable before the grant that matters wakes up.
        let fleet_key = key(1);
        let fleet = fleet_of(&fleet_key);
        let joined = Millis(10_000);
        let mut terms = Terms::joining(fleet, node_of(&key(2)), "phone", joined);
        terms.grants.insert(Grant::HostRuns);
        let cert = MembershipCert::issue(&fleet_key, Issuer::Fleet, terms);

        assert!(cert.granted(Grant::Submit, joined), "the rest is immediate");
        assert!(!cert.granted(Grant::HostRuns, joined));
        assert!(!cert.granted(Grant::HostRuns, joined + PROBATION - Millis(1)));
        assert!(cert.granted(Grant::HostRuns, joined + PROBATION));
        assert_eq!(cert.probation_until(joined), Some(joined + PROBATION));
        assert_eq!(cert.probation_until(joined + PROBATION), None);

        // Dormant, not invalid: the certificate itself verifies throughout.
        assert_eq!(cert.verify(fleet, None, joined), Ok(()));
    }

    #[test]
    fn joining_grants_less_than_founding_does() {
        // The two paths ADR-0012 distinguishes: enrolling yourself gets you the least the
        // fleet can give, while founding it happens with the passphrase in your hands.
        let fleet = FleetId::from_bytes([1; 32]);
        let member = NodeId::from_bytes([2; 32]);
        let joining = Terms::joining(fleet, member, "phone", Millis(0));
        let founding = Terms::founding(fleet, member, "desktop", Millis(0));

        assert!(!joining.grants.contains(&Grant::HostRuns));
        assert!(!joining.grants.contains(&Grant::Approve));
        assert!(joining.probation);

        assert!(founding.grants.contains(&Grant::HostRuns));
        assert!(founding.grants.contains(&Grant::Approve));
        assert!(
            !founding.probation,
            "the alarm probation protects would ring in the founder's own hand"
        );
    }

    #[test]
    fn probation_is_covered_by_the_signature() {
        // Otherwise a joining node clears the flag on the certificate it was handed and
        // hosts runs fifteen minutes early — which is exactly the window that matters.
        let fleet_key = key(1);
        let fleet = fleet_of(&fleet_key);
        let mut cert = MembershipCert::issue(
            &fleet_key,
            Issuer::Fleet,
            Terms::joining(fleet, node_of(&key(2)), "phone", Millis(0)),
        );
        cert.probation = false;

        assert!(matches!(
            cert.verify(fleet, None, Millis(1_000)),
            Err(MembershipError::BadSignature { .. })
        ));
    }

    #[test]
    fn an_approver_can_enrol_without_the_passphrase() {
        // The daily path: the fleet key delegated once, and now the phone does the work
        // while the passphrase stays in a drawer.
        let fleet_key = key(1);
        let fleet = fleet_of(&fleet_key);
        let approver_key = key(3);
        let approver = node_of(&approver_key);

        let delegation = Delegation::issue(&fleet_key, fleet, approver, Millis(0), DAY, 1);
        let cert = MembershipCert::issue(
            &approver_key,
            Issuer::Approver { node: approver },
            Terms {
                serial: 2,
                ..terms(fleet, node_of(&key(4)), "new-laptop")
            },
        );

        assert_eq!(cert.verify(fleet, Some(&delegation), Millis(2_000)), Ok(()));
    }

    /// ADR-0012 mitigation 1, which the delegation could not express.
    ///
    /// The delegation says *who* may issue and said nothing about *what*, so an approver holding
    /// nothing but a delegation and its own key could mint `HostRuns` — the grant that turns
    /// membership into "runs agents on your repositories with your credentials" — for a device
    /// it controlled, and every peer accepted it. `may_issue` is in this ADR's own type sketch
    /// and was never in the code.
    ///
    /// The rule is not a flat prohibition, because renewal has to keep working: an approver
    /// forbidden from ever signing `HostRuns` cannot renew a host node either, and then every
    /// host in the fleet needs the passphrase every thirty days. So what an approver may do is
    /// **restate**, and what it may not do is **mint** — told apart by the fleet-signed
    /// certificate a renewal carries.
    #[test]
    fn an_approver_may_renew_a_grant_it_could_never_have_issued() {
        let fleet_key = key(1);
        let fleet = fleet_of(&fleet_key);
        let approver_key = key(3);
        let approver = node_of(&approver_key);
        let host = node_of(&key(4));
        let delegation = Delegation::issue(&fleet_key, fleet, approver, Millis(0), DAY, 1);
        let issuer = Issuer::Approver { node: approver };

        let mut wide = default_grants();
        wide.insert(Grant::HostRuns);
        wide.insert(Grant::Approve);

        // Minting: refused, and the refusal names both the approver and what it reached for.
        let minted = MembershipCert::issue(
            &approver_key,
            issuer,
            Terms {
                grants: wide.clone(),
                ..terms(fleet, host, "attacker-controlled")
            },
        );
        let err = minted
            .verify(fleet, Some(&delegation), Millis(2_000))
            .expect_err("an approver minted host-runs");
        let MembershipError::NotIssuableByApprover { grants, .. } = &err else {
            panic!("wrong error: {err}");
        };
        assert!(grants.contains("host-runs"), "{err}");
        assert!(grants.contains("approve"), "{err}");

        // What it may still do freely is the door, which is the entire reason approvers exist.
        let invited = MembershipCert::issue(&approver_key, issuer, terms(fleet, host, "laptop"));
        assert_eq!(
            invited.verify(fleet, Some(&delegation), Millis(2_000)),
            Ok(())
        );

        // Restating: the fleet key granted `HostRuns` once, and the approver keeps that
        // certificate alive without being able to have made it.
        let granted = MembershipCert::issue(
            &fleet_key,
            Issuer::Fleet,
            Terms {
                grants: wide.clone(),
                ..terms(fleet, host, "desktop")
            },
        );
        let renewed =
            MembershipCert::issue(&approver_key, issuer, granted.renewal(Millis(2_000), DAY));
        assert_eq!(
            renewed.verify(fleet, Some(&delegation), Millis(3_000)),
            Ok(())
        );
        assert_eq!(
            renewed.grants, wide,
            "renewal is a fresh clock and nothing else"
        );

        // A month of renewals is still one link: the chain's root is what a renewal proves, not
        // the link before it, so this does not grow without bound on a long-lived host.
        let again =
            MembershipCert::issue(&approver_key, issuer, renewed.renewal(Millis(4_000), DAY));
        assert_eq!(
            again.verify(fleet, Some(&delegation), Millis(5_000)),
            Ok(())
        );
        assert_eq!(
            again.authority.as_deref(),
            Some(&granted),
            "a renewal of a renewal must still point at what the fleet key signed"
        );

        // And an authority is only an authority for the member it names: a stolen certificate
        // from the fleet's one real host does not widen somebody else's.
        let borrowed = MembershipCert::issue(
            &approver_key,
            issuer,
            Terms {
                grants: wide.clone(),
                authority: Some(Box::new(granted.clone())),
                ..terms(fleet, node_of(&key(5)), "somebody-else")
            },
        );
        assert!(matches!(
            borrowed.verify(fleet, Some(&delegation), Millis(3_000)),
            Err(MembershipError::NotIssuableByApprover { .. })
        ));

        // Nor may a chain launder a grant nobody with the passphrase ever made: an
        // approver-issued certificate is not an authority, however wide it claims to be.
        let self_signed = MembershipCert::issue(
            &approver_key,
            issuer,
            Terms {
                grants: wide.clone(),
                ..terms(fleet, host, "bootstrap")
            },
        );
        let laundered = MembershipCert::issue(
            &approver_key,
            issuer,
            Terms {
                grants: wide,
                authority: Some(Box::new(self_signed)),
                ..terms(fleet, host, "laundered")
            },
        );
        assert!(matches!(
            laundered.verify(fleet, Some(&delegation), Millis(3_000)),
            Err(MembershipError::NotIssuableByApprover { .. })
        ));
    }

    #[test]
    fn an_expired_authority_still_authorises_a_renewal() {
        // Deliberate, and the reason the authority's expiry is skipped: the grant was made once,
        // long ago, and the renewal in hand is what keeps it current. Checking the original's
        // expiry would mean a certificate could only be renewed while it did not need to be —
        // and the thirty-day backstop would come back as a monthly passphrase ritual on every
        // host in the fleet.
        let fleet_key = key(1);
        let fleet = fleet_of(&fleet_key);
        let approver_key = key(3);
        let approver = node_of(&approver_key);
        let host = node_of(&key(4));
        let delegation = Delegation::issue(
            &fleet_key,
            fleet,
            approver,
            Millis(0),
            Millis(DAY.0 * 400),
            1,
        );

        let mut wide = default_grants();
        wide.insert(Grant::HostRuns);
        let granted = MembershipCert::issue(
            &fleet_key,
            Issuer::Fleet,
            Terms {
                grants: wide,
                issued_at: Millis(0),
                lifetime: DAY,
                ..terms(fleet, host, "desktop")
            },
        );
        let long_after = Millis(DAY.0 * 300);
        assert!(granted.is_expired(long_after), "the test's premise");

        let renewed = MembershipCert::issue(
            &approver_key,
            Issuer::Approver { node: approver },
            granted.renewal(long_after, DAY),
        );
        assert_eq!(renewed.verify(fleet, Some(&delegation), long_after), Ok(()));
        assert!(renewed.granted(Grant::HostRuns, long_after + Millis(DAY.0 / 2)));
    }

    #[test]
    fn an_approver_without_a_delegation_is_nobody() {
        // Otherwise any member could enrol anyone, which is the transitive-trust escalation
        // ADR-0012 rejected the roster model over.
        let fleet_key = key(1);
        let fleet = fleet_of(&fleet_key);
        let pretender_key = key(7);
        let pretender = node_of(&pretender_key);

        let cert = MembershipCert::issue(
            &pretender_key,
            Issuer::Approver { node: pretender },
            terms(fleet, node_of(&key(8)), "attacker"),
        );

        assert!(matches!(
            cert.verify(fleet, None, Millis(2_000)),
            Err(MembershipError::NoDelegation { .. })
        ));

        // And a delegation belonging to somebody else does not help.
        let other = node_of(&key(3));
        let delegation = Delegation::issue(&fleet_key, fleet, other, Millis(0), DAY, 1);
        assert!(matches!(
            cert.verify(fleet, Some(&delegation), Millis(2_000)),
            Err(MembershipError::DelegationMismatch { .. })
        ));
    }

    #[test]
    fn a_forged_delegation_does_not_verify() {
        // The chain is only worth having if its first link is checked.
        let fleet_key = key(1);
        let fleet = fleet_of(&fleet_key);
        let approver_key = key(3);
        let approver = node_of(&approver_key);

        // Signed by the approver rather than by the fleet — i.e. self-appointed.
        let forged = Delegation::issue(&approver_key, fleet, approver, Millis(0), DAY, 1);
        let cert = MembershipCert::issue(
            &approver_key,
            Issuer::Approver { node: approver },
            Terms {
                serial: 2,
                ..terms(fleet, node_of(&key(4)), "new-laptop")
            },
        );

        assert!(matches!(
            cert.verify(fleet, Some(&forged), Millis(2_000)),
            Err(MembershipError::BadSignature { .. })
        ));
    }

    /// The owner's decision (ADR-0069's step-3 amendment): a delegation that has since expired
    /// does not un-issue what it signed while it was in force, and signs nothing after. This
    /// test used to assert the opposite, and that rule put a cliff a year after every `init`.
    #[test]
    fn a_delegation_is_checked_as_it_stood_when_the_certificate_was_issued() {
        let fleet_key = key(1);
        let fleet = fleet_of(&fleet_key);
        let approver_key = key(3);
        let approver = node_of(&approver_key);

        let delegation = Delegation::issue(&fleet_key, fleet, approver, Millis(0), Millis(500), 1);
        let issue_at = |at: Millis| {
            MembershipCert::issue(
                &approver_key,
                Issuer::Approver { node: approver },
                Terms {
                    issued_at: at,
                    serial: at.0,
                    ..terms(fleet, node_of(&key(4)), "new-laptop")
                },
            )
        };

        // Issued inside the delegation's window, checked after it closed: still valid, bounded
        // by its own lifetime.
        let inside = issue_at(Millis(100));
        assert!(!inside.is_expired(Millis(1_000)));
        assert_eq!(
            inside.verify(fleet, Some(&delegation), Millis(1_000)),
            Ok(())
        );

        // Issued after it closed: refused, however fresh.
        let after = issue_at(Millis(600));
        assert!(matches!(
            after.verify(fleet, Some(&delegation), Millis(700)),
            Err(MembershipError::DelegationNotInForce { .. })
        ));

        // And the fleet key's signature on the delegation is still checked.
        let forged = Delegation::issue(&approver_key, fleet, approver, Millis(0), Millis(500), 1);
        assert!(inside.verify(fleet, Some(&forged), Millis(200)).is_err());
    }

    #[test]
    fn revocation_verifies_against_the_fleet_and_names_its_subject() {
        let fleet_key = key(1);
        let fleet = fleet_of(&fleet_key);
        let member = node_of(&key(2));
        let revocation = Revocation::issue(&fleet_key, fleet, member, Millis(5_000), 1);

        assert_eq!(revocation.verify(fleet), Ok(()));
        assert!(revocation.covers(member));
        assert!(!revocation.covers(node_of(&key(3))));
    }

    #[test]
    fn a_revocation_forged_by_its_subject_does_not_verify() {
        // The trap the design walks around: `incarnation` lets a node refute a Suspect
        // claim about itself. Membership must not work that way, or a revoked device
        // argues its way back in.
        let fleet_key = key(1);
        let fleet = fleet_of(&fleet_key);
        let victim_key = key(2);

        let forged = Revocation::issue(&victim_key, fleet, node_of(&key(5)), Millis(5_000), 1);

        assert!(matches!(
            forged.verify(fleet),
            Err(MembershipError::BadSignature { .. })
        ));
    }

    #[test]
    fn signatures_are_domain_separated_between_credential_types() {
        // Without a context prefix, bytes signed as one credential could be replayed as
        // another with a compatible layout.
        let a = MembershipCert::issue(
            &key(1),
            Issuer::Fleet,
            Terms {
                grants: BTreeSet::new(),
                ..terms(
                    FleetId::from_bytes([1; 32]),
                    NodeId::from_bytes([2; 32]),
                    "n",
                )
            },
        )
        .message();
        let b = delegation_bytes(
            FleetId::from_bytes([1; 32]),
            NodeId::from_bytes([2; 32]),
            Millis(0),
            Millis(1),
            1,
            &IssuerKey::Node,
        );
        assert_ne!(a, b);
        assert!(a.starts_with(&(MEMBERSHIP_CONTEXT.len() as u64).to_le_bytes()));
    }

    /// The signing bytes are a wire format, and this is the only thing that says so.
    ///
    /// [`crate::fleet::passphrase`] has had a pinned known-answer test from the start, for a
    /// reason that applies here word for word: the derivation is compared across machines, so
    /// tuning it does not fail loudly, it silently re-founds every fleet. A credential's *signed
    /// message* is the same kind of thing one level up. Add a field, reorder two, change how a
    /// grant is tagged, and every certificate, delegation, revocation and succession in every
    /// fleet stops verifying — with no error anywhere and one symptom: every device drops out
    /// and the passphrase is the only way back onto each of them.
    ///
    /// This test exists because that happened. Adding [`MembershipCert::authority`] changed
    /// these bytes and nothing in the tree noticed, which is exactly the failure the passphrase
    /// test was written to prevent for its own constant. So: four digests, one per credential
    /// type, and a deliberate change is one that updates them **and** says in its commit message
    /// that every device has to re-join.
    /// ADR-0069: a renewer restates a live membership, exactly, and is refused for everything else.
    mod renewal_by_a_renewer {
        use super::*;

        const T0: Millis = Millis(1_000_000);
        const YEAR: Millis = APPROVAL_LIFETIME;

        struct Fleet {
            key: SigningKey,
            id: FleetId,
        }

        fn fleet() -> Fleet {
            let key = key(90);
            let id = fleet_of(&key);
            Fleet { key, id }
        }

        /// A person's approval of `member`, signed with the fleet key at `at`.
        fn approval(f: &Fleet, member: &SigningKey, name: &str, at: Millis) -> MembershipCert {
            MembershipCert::issue(
                &f.key,
                Issuer::Fleet,
                Terms {
                    probation: false,
                    ..Terms::joining(f.id, node_of(member), name, at)
                },
            )
        }

        fn proof(approved: &MembershipCert, renewer: &MembershipCert) -> RenewalProof {
            RenewalProof {
                approval: approved.clone(),
                approval_delegation: None,
                renewer: renewer.clone(),
                renewer_delegation: None,
            }
        }

        fn renew(
            renewer: &SigningKey,
            approved: &MembershipCert,
            renewers_own: &MembershipCert,
            now: Millis,
        ) -> MembershipCert {
            MembershipCert::issue(
                renewer,
                Issuer::Renewer {
                    node: node_of(renewer),
                },
                approved.renewal_by_renewer(proof(approved, renewers_own), now, CERT_LIFETIME),
            )
        }

        #[test]
        fn a_renewal_that_restates_the_approval_verifies_and_keeps_its_date() {
            let f = fleet();
            let (m, r) = (key(91), key(92));
            let approved = approval(&f, &m, "vps-17", T0);
            let renewers_own = approval(&f, &r, "vps-3", T0);
            let now = T0 + Millis(29 * DAY.0);
            let renewed = renew(&r, &approved, &renewers_own, now);

            assert_eq!(renewed.verify(f.id, None, now), Ok(()));
            assert_eq!(renewed.approved_at(), T0, "renewing is not approving");
            assert_eq!(renewed.renewer(), Some(node_of(&r)));
        }

        #[test]
        fn no_node_renews_itself() {
            let f = fleet();
            let m = key(91);
            let approved = approval(&f, &m, "vps-17", T0);
            let renewed = renew(&m, &approved, &approved, T0 + DAY);
            assert!(matches!(
                renewed.verify(f.id, None, T0 + DAY),
                Err(MembershipError::SelfRenewal { .. })
            ));
        }

        #[test]
        fn a_renewal_cannot_widen_a_grant() {
            let f = fleet();
            let (m, r) = (key(91), key(92));
            let approved = approval(&f, &m, "vps-17", T0);
            let renewers_own = approval(&f, &r, "vps-3", T0);
            let mut terms = approved.renewal_by_renewer(
                proof(&approved, &renewers_own),
                T0 + DAY,
                CERT_LIFETIME,
            );
            terms.grants.insert(Grant::HostRuns);
            let widened = MembershipCert::issue(&r, Issuer::Renewer { node: node_of(&r) }, terms);
            assert_eq!(
                widened.verify(f.id, None, T0 + DAY),
                Err(MembershipError::RenewalChanges { what: "grants" })
            );
        }

        #[test]
        fn a_member_without_renew_is_not_a_renewer() {
            let f = fleet();
            let (m, r) = (key(91), key(92));
            let approved = approval(&f, &m, "vps-17", T0);
            let mut no_renew = Terms::joining(f.id, node_of(&r), "observer", T0);
            no_renew.grants.remove(&Grant::Renew);
            let renewers_own = MembershipCert::issue(&f.key, Issuer::Fleet, no_renew);
            let renewed = renew(&r, &approved, &renewers_own, T0 + DAY);
            assert!(matches!(
                renewed.verify(f.id, None, T0 + DAY),
                Err(MembershipError::NotARenewer { .. })
            ));
        }

        /// The stolen shuttle: a year after a person last approved it, no renewal gets it in —
        /// and a renewer whose own approval is over a year old renews nothing.
        #[test]
        fn an_approval_over_a_year_old_is_refused_whatever_its_renewals_say() {
            let f = fleet();
            let (m, r) = (key(91), key(92));
            let approved = approval(&f, &m, "vps-17", T0);
            let late = T0 + YEAR + DAY;
            let fresh_renewer = approval(&f, &r, "vps-3", late - DAY);
            let renewed = renew(&r, &approved, &fresh_renewer, late - Millis(1));
            assert!(matches!(
                renewed.verify(f.id, None, late),
                Err(MembershipError::ApprovalTooOld { .. })
            ));

            let fresh_member = approval(&f, &m, "vps-17", late - DAY);
            let old_renewer = approval(&f, &r, "vps-3", T0);
            let renewed = renew(&r, &fresh_member, &old_renewer, late - Millis(1));
            assert!(matches!(
                renewed.verify(f.id, None, late),
                Err(MembershipError::ApprovalTooOld { .. })
            ));
        }

        /// ADR-0069 §3's other half: a person, through an approver, starts a new year — and a
        /// host keeps `host-runs`, restated from the fleet key's grant, which an approver may
        /// not mint.
        #[test]
        fn an_approvers_reapproval_starts_a_new_year_and_restates_host_runs() {
            let f = fleet();
            let (m, a) = (key(91), key(93));
            let mut host = Terms {
                probation: false,
                ..Terms::joining(f.id, node_of(&m), "vps-17", T0)
            };
            host.grants.insert(Grant::HostRuns);
            let approved = MembershipCert::issue(&f.key, Issuer::Fleet, host);
            let delegation = Delegation::issue(&f.key, f.id, node_of(&a), T0, YEAR + YEAR, 1);

            let at = T0 + Millis(350 * DAY.0);
            let again = MembershipCert::issue(
                &a,
                Issuer::Approver { node: node_of(&a) },
                approved.reapproval(at, CERT_LIFETIME),
            );
            assert_eq!(again.approved_at(), at);
            assert!(again.grants.contains(&Grant::HostRuns));
            // Day 366: past the first approval's year, well inside the new one's.
            assert_eq!(
                again.verify(f.id, Some(&delegation), T0 + YEAR + DAY),
                Ok(())
            );
            assert!(!again.is_due_for_reapproval(T0 + YEAR + DAY));
        }

        #[test]
        fn an_approval_is_due_for_reapproval_in_its_last_month_and_not_before() {
            let f = fleet();
            let m = key(91);
            let approved = approval(&f, &m, "vps-17", T0);
            let renewed_at = |day: u64| {
                MembershipCert::issue(
                    &f.key,
                    Issuer::Fleet,
                    approved.renewal(T0 + Millis(day * DAY.0), CERT_LIFETIME),
                )
            };
            // A certificate renewed on day 330 is not due for renewal on day 334, and its
            // approval is not yet in its last month.
            assert!(!renewed_at(330).is_due_for_reapproval(T0 + Millis(334 * DAY.0)));
            // Day 336 is within thirty days of day 365, though the certificate has weeks left.
            let cert = renewed_at(330);
            let day_336 = T0 + Millis(336 * DAY.0);
            assert!(cert.is_due_for_reapproval(day_336));
            assert!(!cert.is_due_for_renewal(day_336));
            assert_eq!(cert.approval_expires_at(), T0 + YEAR);
        }

        /// Renewing by an approver copies the date too, so the old automatic path cannot reset
        /// the year either.
        #[test]
        fn an_approvers_renewal_keeps_the_approval_date() {
            let f = fleet();
            let m = key(91);
            let approved = approval(&f, &m, "vps-17", T0);
            let renewed = MembershipCert::issue(
                &f.key,
                Issuer::Fleet,
                approved.renewal(T0 + Millis(355 * DAY.0), CERT_LIFETIME),
            );
            assert_eq!(renewed.approved_at(), T0);
            // Live until day 385, so only the approval's age can refuse it on day 366.
            assert!(matches!(
                renewed.verify(f.id, None, T0 + YEAR + DAY),
                Err(MembershipError::ApprovalTooOld { .. })
            ));
        }

        #[test]
        fn what_is_restated_must_be_an_approval_not_another_renewal() {
            let f = fleet();
            let (m, r, s) = (key(91), key(92), key(93));
            let approved = approval(&f, &m, "vps-17", T0);
            let r_own = approval(&f, &r, "vps-3", T0);
            let s_own = approval(&f, &s, "vps-4", T0);
            let once = renew(&r, &approved, &r_own, T0 + DAY);
            let twice = renew(&s, &once, &s_own, T0 + Millis(2 * DAY.0));
            assert_eq!(
                twice.verify(f.id, None, T0 + Millis(2 * DAY.0)),
                Err(MembershipError::NotAnApproval)
            );
        }

        /// An approver's approval stays an approval after its delegation expires, as long as the
        /// delegation was in force when it was issued — and not otherwise.
        #[test]
        fn an_approval_is_checked_against_its_delegation_as_it_stood_then() {
            let f = fleet();
            let (a, m, r) = (key(94), key(91), key(92));
            let delegation = Delegation::issue(&f.key, f.id, node_of(&a), T0, DAY, 1);
            let by_approver = MembershipCert::issue(
                &a,
                Issuer::Approver { node: node_of(&a) },
                Terms {
                    probation: false,
                    ..Terms::joining(f.id, node_of(&m), "vps-17", T0 + Millis(1))
                },
            );
            let r_own = approval(&f, &r, "vps-3", T0);
            let now = T0 + Millis(20 * DAY.0);
            let mut p = proof(&by_approver, &r_own);
            p.approval_delegation = Some(delegation.clone());
            let renewed = MembershipCert::issue(
                &r,
                Issuer::Renewer { node: node_of(&r) },
                by_approver.renewal_by_renewer(p, now, CERT_LIFETIME),
            );
            assert_eq!(
                renewed.verify(f.id, None, now),
                Ok(()),
                "delegation expired since, fine"
            );

            let outside = MembershipCert::issue(
                &a,
                Issuer::Approver { node: node_of(&a) },
                Terms {
                    probation: false,
                    ..Terms::joining(f.id, node_of(&m), "vps-17", T0 + Millis(2 * DAY.0))
                },
            );
            let mut p = proof(&outside, &r_own);
            p.approval_delegation = Some(delegation);
            let renewed = MembershipCert::issue(
                &r,
                Issuer::Renewer { node: node_of(&r) },
                outside.renewal_by_renewer(p, now, CERT_LIFETIME),
            );
            assert!(matches!(
                renewed.verify(f.id, None, now),
                Err(MembershipError::DelegationNotInForce { .. })
            ));
        }

        /// A certificate from before ADR-0069 has no approval date and keeps its v1 bytes: it
        /// still verifies, and reads its approval as its own issue.
        #[test]
        fn a_legacy_certificate_still_verifies_and_reads_its_issue_as_its_approval() {
            let f = fleet();
            let m = key(91);
            let legacy = MembershipCert::issue(
                &f.key,
                Issuer::Fleet,
                Terms {
                    approved_at: None,
                    probation: false,
                    ..Terms::joining(f.id, node_of(&m), "old", T0)
                },
            );
            assert_eq!(legacy.verify(f.id, None, T0 + DAY), Ok(()));
            assert_eq!(legacy.approved_at(), T0);
        }
    }

    /// ADR-0069 §4: an approver whose delegation names a P-256 key approves with that key, and
    /// only that key. A software key stands in for the hardware one; the verifier cannot tell and
    /// does not need to.
    mod hardware_approval {
        use super::*;
        use p256::ecdsa::signature::Signer;

        fn hardware_key(seed: u8) -> p256::ecdsa::SigningKey {
            let mut bytes = [0u8; 32];
            bytes[31] = seed;
            bytes[0] = 1;
            p256::ecdsa::SigningKey::from_bytes(&bytes.into()).expect("a valid scalar")
        }

        fn issuer_key(hw: &p256::ecdsa::SigningKey) -> IssuerKey {
            IssuerKey::p256_from_sec1(hw.verifying_key().to_encoded_point(true).as_bytes())
                .expect("a point on the curve")
        }

        /// Signed as Android's Keystore signs: SHA256withECDSA, DER out — then converted.
        fn hardware_sign(hw: &p256::ecdsa::SigningKey, cert: MembershipCert) -> MembershipCert {
            let signature: p256::ecdsa::Signature = hw.sign(&cert.signing_bytes());
            let der = signature.to_der();
            cert.signed(p256_signature_from_der(der.as_bytes()).expect("DER"))
        }

        struct Setup {
            fleet_key: SigningKey,
            fleet: FleetId,
            approver_node: SigningKey,
            hw: p256::ecdsa::SigningKey,
            delegation: Delegation,
        }

        fn setup() -> Setup {
            let fleet_key = key(1);
            let fleet = fleet_of(&fleet_key);
            let approver_node = key(3);
            let hw = hardware_key(7);
            let delegation = Delegation::issue_with_key(
                &fleet_key,
                fleet,
                node_of(&approver_node),
                issuer_key(&hw),
                Millis(0),
                DAY,
                1,
            );
            Setup {
                fleet_key,
                fleet,
                approver_node,
                hw,
                delegation,
            }
        }

        fn invitation(s: &Setup) -> MembershipCert {
            MembershipCert::unsigned(
                Issuer::Approver {
                    node: node_of(&s.approver_node),
                },
                Terms {
                    issued_at: Millis(100),
                    serial: 2,
                    ..terms(s.fleet, node_of(&key(4)), "vps-17")
                },
            )
        }

        #[test]
        fn an_approval_signed_by_the_named_hardware_key_verifies() {
            let s = setup();
            assert_eq!(s.delegation.verify(s.fleet, Millis(50)), Ok(()));
            let cert = hardware_sign(&s.hw, invitation(&s));
            assert_eq!(
                cert.verify(s.fleet, Some(&s.delegation), Millis(200)),
                Ok(())
            );
        }

        #[test]
        fn the_node_key_no_longer_approves_once_a_hardware_key_is_named() {
            // The point of a second key: a stolen laptop disk holds the node key, and that must
            // not be enough to enrol anybody.
            let s = setup();
            let unsigned = invitation(&s);
            let by_node = MembershipCert::issue(
                &s.approver_node,
                unsigned.issuer,
                Terms {
                    issued_at: Millis(100),
                    serial: 2,
                    ..terms(s.fleet, node_of(&key(4)), "vps-17")
                },
            );
            assert!(matches!(
                by_node.verify(s.fleet, Some(&s.delegation), Millis(200)),
                Err(MembershipError::BadSignature { .. })
            ));
        }

        #[test]
        fn another_hardware_key_does_not_approve() {
            let s = setup();
            let cert = hardware_sign(&hardware_key(8), invitation(&s));
            assert!(matches!(
                cert.verify(s.fleet, Some(&s.delegation), Millis(200)),
                Err(MembershipError::BadSignature { .. })
            ));
        }

        #[test]
        fn the_named_key_is_covered_by_the_fleet_signature() {
            // Swapping the key in a delegation is the attack a delegation has to stop: it would
            // let whoever holds any P-256 key approve under somebody else's delegation.
            let s = setup();
            let mut swapped = s.delegation.clone();
            swapped.issuer_key = issuer_key(&hardware_key(8));
            assert!(matches!(
                swapped.verify(s.fleet, Millis(50)),
                Err(MembershipError::BadSignature { .. })
            ));
            let cert = hardware_sign(&hardware_key(8), invitation(&s));
            assert!(cert.verify(s.fleet, Some(&swapped), Millis(200)).is_err());
            // …and a v1 delegation's signature is not a v2 one's: the bytes differ by context.
            let node_delegation = Delegation::issue(
                &s.fleet_key,
                s.fleet,
                node_of(&s.approver_node),
                Millis(0),
                DAY,
                1,
            );
            let mut upgraded = node_delegation.clone();
            upgraded.issuer_key = issuer_key(&s.hw);
            assert!(upgraded.verify(s.fleet, Millis(50)).is_err());
        }

        #[test]
        fn a_node_key_delegation_round_trips_without_the_new_field() {
            let s = setup();
            let old = Delegation::issue(
                &s.fleet_key,
                s.fleet,
                node_of(&s.approver_node),
                Millis(0),
                DAY,
                1,
            );
            let json = serde_json::to_string(&old).expect("json");
            assert!(!json.contains("issuer_key"), "{json}");
            let back: Delegation = serde_json::from_str(&json).expect("parse");
            assert_eq!(back, old);
            let json = serde_json::to_string(&s.delegation).expect("json");
            let back: Delegation = serde_json::from_str(&json).expect("parse");
            assert_eq!(back.verify(s.fleet, Millis(50)), Ok(()));
        }

        #[test]
        fn the_delegation_bytes_are_pinned_for_both_kinds_of_key() {
            let fleet = FleetId::from_bytes([9; 32]);
            let approver = NodeId::from_bytes([7; 32]);
            let digest = |key: &IssuerKey| {
                blake3::hash(&delegation_bytes(
                    fleet,
                    approver,
                    Millis(1),
                    Millis(2),
                    3,
                    key,
                ))
                .to_hex()
                .to_string()[..16]
                    .to_string()
            };
            assert_eq!(
                digest(&IssuerKey::Node),
                "fe3ee723f31e38e6",
                "v1 delegation bytes changed"
            );
            assert_eq!(
                digest(&IssuerKey::P256 { sec1: [2; 33] }),
                "9e2129206fb58e54",
                "v2 delegation bytes changed"
            );
        }
    }

    #[test]
    fn the_signing_bytes_are_pinned_to_a_known_answer() {
        let fleet = FleetId::from_bytes([9; 32]);
        let member = NodeId::from_bytes([8; 32]);
        let approver = NodeId::from_bytes([7; 32]);

        // Named rather than `default_grants()`, which grew `Renew` (ADR-0069): the pin fixes the
        // format for fixed content, and a fixture that follows the defaults would move with them.
        let grants: BTreeSet<Grant> = [Grant::Submit, Grant::Deliver, Grant::HostRuns]
            .into_iter()
            .collect();
        let mut cert = MembershipCert::issue(
            &key(1),
            Issuer::Approver { node: approver },
            Terms {
                fleet,
                member,
                name: "pinned".into(),
                grants,
                issued_at: Millis(1_000),
                lifetime: DAY,
                serial: 42,
                probation: true,
                authority: None,
                approved_at: None,
                proof: None,
            },
        );
        let digest = |bytes: &[u8]| blake3::hash(bytes).to_hex().to_string();

        assert_eq!(
            &digest(&cert.message())[..16],
            "620f1937f16a3587",
            "the membership certificate's signed bytes changed"
        );

        // …and again with an authority, since that is the field whose *presence* has to be
        // covered: a peer that ignored it could be handed a renewal with the chain stripped off.
        cert.authority = Some(Box::new(MembershipCert::issue(
            &key(1),
            Issuer::Fleet,
            Terms {
                grants: [Grant::Submit, Grant::Deliver].into_iter().collect(),
                ..terms(fleet, member, "root")
            },
        )));
        assert_eq!(
            &digest(&cert.message())[..16],
            "92afbc8e81c82634",
            "an authority no longer changes what the signature covers"
        );

        // ADR-0069's v2 bytes: a renewer's renewal, with `approved_at` and a proof. Pinned the
        // same way, for the same reason.
        let approval = MembershipCert::issue(
            &key(1),
            Issuer::Fleet,
            Terms {
                grants: [Grant::Submit, Grant::Deliver, Grant::Renew]
                    .into_iter()
                    .collect(),
                ..terms(fleet, member, "pinned")
            },
        );
        let renewal = MembershipCert::issue(
            &key(2),
            Issuer::Renewer { node: approver },
            Terms {
                approved_at: Some(Millis(1_000)),
                proof: Some(Box::new(RenewalProof {
                    approval: approval.clone(),
                    approval_delegation: None,
                    renewer: approval,
                    renewer_delegation: None,
                })),
                ..terms(fleet, member, "pinned")
            },
        );
        assert_eq!(
            &digest(&renewal.message())[..16],
            "8cd732c11a776170",
            "the v2 (ADR-0069) certificate's signed bytes changed"
        );

        assert_eq!(
            &digest(&delegation_bytes(
                fleet,
                approver,
                Millis(1),
                Millis(2),
                3,
                &IssuerKey::Node
            ))[..16],
            "fe3ee723f31e38e6",
            "the delegation's signed bytes changed"
        );
        assert_eq!(
            &digest(&revocation_bytes(fleet, member, Millis(4), 5))[..16],
            "9aff5c7fa3ff80f7",
            "the revocation's signed bytes changed"
        );
        assert_eq!(
            &digest(&succession_bytes(
                fleet,
                FleetId::from_bytes([6; 32]),
                Millis(7),
                8
            ))[..16],
            "cf0abf4c3c3e1bf7",
            "the succession's signed bytes changed"
        );
    }

    #[test]
    fn field_encoding_cannot_be_confused_by_moving_bytes_between_fields() {
        // Length prefixes exist so that ("ab", "c") and ("a", "bc") do not sign the same.
        let mut left = Vec::new();
        field(&mut left, b"ab");
        field(&mut left, b"c");
        let mut right = Vec::new();
        field(&mut right, b"a");
        field(&mut right, b"bc");
        assert_ne!(left, right);
    }

    #[test]
    fn credentials_round_trip_through_json() {
        // They travel in gossip and get written to disk, so both directions are tested —
        // the `Response::Runs` lesson from phase 1.
        let fleet_key = key(1);
        let cert = cert_for(&fleet_key, node_of(&key(2)), default_grants());
        let json = serde_json::to_string(&cert).expect("encode");
        let back: MembershipCert = serde_json::from_str(&json).expect("decode");
        assert_eq!(back, cert);
        assert_eq!(
            back.verify(fleet_of(&fleet_key), None, Millis(2_000)),
            Ok(())
        );

        let revocation =
            Revocation::issue(&fleet_key, fleet_of(&fleet_key), cert.member, Millis(9), 1);
        let json = serde_json::to_string(&revocation).expect("encode");
        assert_eq!(
            serde_json::from_str::<Revocation>(&json).expect("decode"),
            revocation
        );
    }

    #[test]
    fn renewal_is_the_same_certificate_with_a_fresh_clock() {
        // The property that makes renewal safe to do automatically, in the background, with
        // nobody at a keyboard: it restates a decision somebody already made. A renewal that
        // could widen a grant would be an enrolment in disguise.
        let fleet_key = key(1);
        let approver = key(2);
        let mut grants = default_grants();
        grants.insert(Grant::HostRuns);
        let original = cert_for(&fleet_key, node_of(&key(3)), grants.clone());

        let later = original.issued_at + Millis(29 * DAY.0 / 30);
        let renewed = MembershipCert::issue(
            &approver,
            Issuer::Approver {
                node: node_of(&approver),
            },
            original.renewal(later, DAY),
        );

        assert_eq!(renewed.member, original.member);
        assert_eq!(renewed.name, original.name);
        assert_eq!(renewed.grants, grants);
        assert_eq!(renewed.issued_at, later);
        assert_eq!(renewed.expires_at, later + DAY);
    }

    #[test]
    fn a_renewal_does_not_re_impose_a_probation_already_served() {
        // The one field that cannot be copied verbatim: probation is measured from `issued_at`,
        // and renewal moves it. Re-imposing fifteen minutes of dormancy on a device that has
        // been hosting runs for a month would be a monthly outage nobody could explain.
        let fleet_key = key(1);
        let mut grants = default_grants();
        grants.insert(Grant::HostRuns);
        let joined = MembershipCert::issue(
            &fleet_key,
            Issuer::Fleet,
            Terms {
                grants,
                probation: true,
                ..terms(fleet_of(&fleet_key), node_of(&key(3)), "laptop")
            },
        );

        let after = joined.issued_at + PROBATION + Millis(1);
        assert!(!joined.renewal(after, DAY).probation);

        // And the other direction: one still serving it keeps it, which restarts the clock.
        // Wrong in the safe direction, which is the direction to be wrong in here.
        let during = joined.issued_at + Millis(1);
        assert!(joined.renewal(during, DAY).probation);
    }

    #[test]
    fn a_certificate_is_due_for_renewal_in_its_last_quarter_and_never_after_it_lapses() {
        let fleet_key = key(1);
        let cert = cert_for(&fleet_key, node_of(&key(3)), default_grants());

        assert!(!cert.is_due_for_renewal(cert.issued_at));
        assert!(!cert.is_due_for_renewal(cert.issued_at + Millis(DAY.0 / 2)));
        assert!(cert.is_due_for_renewal(cert.expires_at - Millis(DAY.0 / 8)));
        // Past asking: its peers will not admit it, so there is nobody left to ask and the way
        // back is the passphrase.
        assert!(!cert.is_due_for_renewal(cert.expires_at));
        assert!(!cert.is_due_for_renewal(cert.expires_at + DAY));
    }
}
