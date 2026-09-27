//! The one membership fact that has to travel: who has been thrown out (ADR-0012).
//!
//! Everything else about membership is verifiable offline from the fleet public key, which is
//! the property the whole enrolment design is built around — a peer admits a stranger at first
//! contact with nothing gossiped. Revocation is the exception, and it is the exception in the
//! precise sense that it is an *absence* of a signature rather than a presence: no certificate
//! can say "and this one is no longer valid", so the fact has to reach a peer some other way.
//!
//! Hence a registration of its own beside [`crate::Host`] and [`crate::Deliverer`], for the same
//! reason those are separate: this crate owns the loop and none of the state. A node with no
//! fleet — phases 1 and 2, and most of how this runs — registers nothing and gossips nothing.
//!
//! **The subject never gets to argue.** `incarnation` exists so a node can refute a peer's guess
//! about its liveness; pointed at membership, the same mechanism would let a revoked device talk
//! its way back in. Same gossip path, same-looking fact, opposite rule — which is why this is a
//! signed artifact rather than a field on a view.

use offload_core::{MembershipCert, Millis, Revocation};
use offload_proto::handshake::Credentials;

/// What this node knows about who no longer belongs.
///
/// Deliberately two methods and no more. The certificate a peer presents travels in the
/// handshake and is checked against a key we already hold, so nothing about *admission* belongs
/// here; what belongs here is the fact that outranks a perfectly valid signature.
pub trait Members: Send + Sync + 'static {
    /// Every revocation this node holds, to ride along with a probe.
    ///
    /// The whole list, not a delta. A revoked device is a rare event in a personal fleet, and
    /// the alternative — a digest and a fetch — is ADR-0005's unbuilt optimisation for the view
    /// itself, which is orders of magnitude larger. If a fleet ever accumulates enough of these
    /// for the size to matter, the answer ADR-0012 already names is `offload rekey`: a fleet the
    /// evicted devices were never in needs no revocations at all.
    fn revocations(&self) -> Vec<Revocation>;

    /// File one heard from a peer. `true` if it was new, so the caller can act on it once.
    ///
    /// Verified by the implementation against the fleet key, not by the caller: a revocation
    /// arriving over the wire is exactly as trustworthy as its signature, and a peer that could
    /// assert one would be a peer that can evict any device it likes.
    fn revoked(&self, revocation: Revocation) -> bool;

    /// Re-issue `cert` with a fresh clock, if this node is an approver (ADR-0012).
    ///
    /// The certificate handed in is the one the **handshake authenticated**, not one the peer
    /// asked for in a message — which is the same separation `handshake::admit` makes between
    /// what was claimed and what was proved, and for the same reason: a renewal built from an
    /// assertion would let any member have a certificate minted for any other.
    ///
    /// `Err` is the ordinary answer. Most members are not approvers, and saying so plainly is
    /// what lets the asking node try somebody else rather than wait.
    ///
    /// `delegation` is the one the certificate arrived with at the handshake, which a renewer
    /// needs to carry in its proof when an approver issued the approval (ADR-0069 §2).
    fn renew(
        &self,
        cert: &MembershipCert,
        delegation: Option<&offload_core::Delegation>,
        now: Millis,
    ) -> Result<Credentials, String>;

    /// Would `cert` be admitted from its member right now — genuine, live, in this fleet, and
    /// neither it nor its renewer revoked?
    ///
    /// Asked of a certificate a peer *carried* for itself (ADR-0069 §2), before this node keeps it
    /// as that peer's current papers. The handshake's copy is as old as the connection, so a
    /// connection that outlives a certificate leaves every report built on it describing a
    /// membership that lapsed weeks ago; a carried one is only kept if it passes what the
    /// handshake would have checked. `false` by default: a node with no fleet admits nothing.
    fn admits(
        &self,
        _cert: &MembershipCert,
        _delegation: Option<&offload_core::Delegation>,
        _now: Millis,
    ) -> bool {
        false
    }
}

/// A node that answers to no fleet.
///
/// The default, and the ordinary state for a single-machine daemon: it knows of no revocations,
/// and it has nowhere to put one it is told about. Refusing to remember is right rather than
/// convenient — accepting into thin air would make a peer believe the fact had landed.
#[derive(Debug)]
pub struct NoMembers;

impl Members for NoMembers {
    fn revocations(&self) -> Vec<Revocation> {
        Vec::new()
    }

    fn revoked(&self, _revocation: Revocation) -> bool {
        false
    }

    fn renew(
        &self,
        _cert: &MembershipCert,
        _delegation: Option<&offload_core::Delegation>,
        _now: Millis,
    ) -> Result<Credentials, String> {
        Err("this node belongs to no fleet, so it can renew nothing".into())
    }
}
