//! The first thing two nodes say to each other, and the rule that decides whether there is a
//! second thing.
//!
//! ADR-0012 made membership verifiable offline: a peer checks a certificate against the fleet
//! public key it already holds, with no roster and no gossip, which is what makes first
//! contact over mDNS work at all. ADR-0015 puts that check in the handshake, so an unenrolled
//! node is refused before it can send anything else.
//!
//! **The check has two halves, and they are easy to mistake for one.** The certificate must
//! verify against the fleet key, *and* the connection must have proved possession of the key
//! that certificate names. A certificate is public: it sits in `fleet.json`, it travels in
//! gossip, and it is meant to be shown to strangers. Possession of the private key is the
//! only thing that makes it non-transferable — without that binding it is a bearer token, and
//! anyone who reads a backup wears it.
//!
//! The exchange is symmetric, because trust here is: the dialling node sends [`Hello`], the
//! listening node replies [`Welcome`] with its own credentials and the negotiated version, or
//! [`Refused`](Handshake::Refused) with a reason. Both sides run [`admit`] on what they
//! received. A node that only checked its peer while the peer checked nothing would be
//! offering its runs to whoever asked.

use crate::VersionRange;
use offload_core::fleet::Grant;
use offload_core::{Delegation, FleetId, MembershipCert, Millis, NodeId, Revocation};
use serde::{Deserialize, Serialize};

/// Everything a node presents to prove it belongs.
///
/// The `delegation` is present exactly when an approver issued the certificate rather than
/// the fleet key itself, and it is what lets the receiver check the chain without asking
/// anybody — including without the approver being awake (ADR-0012).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub membership: MembershipCert,
    pub delegation: Option<Delegation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "message", rename_all = "snake_case")]
pub enum Handshake {
    Hello(Hello),
    Welcome(Welcome),
    Refused(Refusal),
}

/// The dialling node's opener.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// What this build can speak, as a range — a fleet upgrades one device at a time.
    pub versions: VersionRange,
    /// Who this claims to be. Checked against the key the transport authenticated, not
    /// believed.
    pub node: NodeId,
    /// Cosmetic. The signed name on the certificate is the one to trust.
    pub name: String,
    pub credentials: Credentials,
}

/// The listening node's answer: its own credentials, and the version both will use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub version: crate::Version,
    pub node: NodeId,
    pub name: String,
    pub credentials: Credentials,
}

/// Why a handshake stopped.
///
/// Structured rather than a string, because "why can my phone not join" is asked from the
/// other end of the connection, where there are no logs to read (ADR-0006's rule that
/// decisions carry reasons, applied to the one decision made before anything is running).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "refused", rename_all = "snake_case")]
pub enum Refusal {
    /// Worded from neither end, because the same sentence is printed at both: `refusing bravo:
    /// …` on the node that refused, and `refused by alpha: …` on the one it refused. It said
    /// *this node speaks {ours}*, which is the refuser's first person — so the refused node's
    /// log read `refused by 29be31be: … this node speaks v30, the peer speaks v31` about a node
    /// that spoke v31, and the one debugging it read the versions backwards (session ninety).
    /// `ours` is always the refuser's: both constructors below are the refusing side's.
    #[error("no common protocol version: the refuser speaks {ours}, and was offered {theirs}")]
    IncompatibleVersion {
        ours: VersionRange,
        theirs: VersionRange,
    },
    #[error("that certificate is for fleet {found}, this is fleet {expected}")]
    WrongFleet { expected: String, found: String },
    #[error("membership could not be verified: {reason}")]
    NotAMember { reason: String },
    /// Carries the signed revocation, not just the name, and the difference is the whole of
    /// what a refused node may do about it (ADR-0044).
    ///
    /// A refusal is a *peer's claim about this node*, and a node must not believe one of those:
    /// pointed at membership, believing one would let any device evict any other by saying so.
    /// The signature is what makes it not a claim. The subject verifies it against the fleet
    /// key it already holds, against the fleet it already belongs to, and naming itself — three
    /// checks it can make alone and offline, which is the same property every other credential
    /// here is built for.
    ///
    /// `Box` because this is the one large variant and `Refusal` is returned by value from
    /// every handshake.
    #[error("member {node} has been revoked")]
    Revoked {
        node: String,
        proof: Box<Revocation>,
    },
    #[error("the certificate names {certificate}, but the connection proved {connection}")]
    IdentityMismatch {
        certificate: String,
        connection: String,
    },
    // There was a `Draining` here — "this node is draining and is not accepting connections" —
    // constructed by nothing, and it must stay that way: **a draining node needs its connections.**
    // Handing its runs over is `place()` dialling peers, and the fleet only learns it is leaving
    // because `NodeStatus::Draining` gossips out of it. Refusing at the door would break both, and
    // the layer that is actually right refuses at the **bid** and at the **grant** — a grant can
    // arrive after the bid that earned it, which is why it is two checks and not one.
    //
    // Deleted rather than left, for `bid_delay`'s reason: a refusal reason naming a state the
    // product is in is exactly the kind of thing somebody wires up, and wiring this one breaks
    // the feature it is named after.
}

/// An admitted peer: who it is, and the certificate that says what the fleet permits it to be.
///
/// The certificate travels rather than a snapshot of its effect, because a connection outlives
/// the facts a snapshot would bake in: probation ends while a session stays up, and a grant
/// checked against handshake-time grants would need a redial to notice. So [`Peer::may`] takes
/// `now` and asks the certificate — which is the point of probation being enforced by peers
/// rather than by the joining node's own good manners (ADR-0012).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub node: NodeId,
    pub name: String,
    pub membership: MembershipCert,
    /// The delegation the certificate came with, if an approver issued it. Kept because a
    /// renewer renewing this peer has to carry the approval *and* what makes it verify
    /// (ADR-0069 §2), and the handshake is the only place both arrive.
    pub delegation: Option<offload_core::Delegation>,
}

impl Peer {
    /// May this peer do `grant`, at this moment? Time-dependent on purpose — see the type doc.
    #[must_use]
    pub fn may(&self, grant: Grant, now: Millis) -> bool {
        self.membership.granted(grant, now)
    }
}

/// Decide whether to talk to whoever just connected.
///
/// `authenticated` is the node key the *transport* proved possession of — the TLS credential,
/// not anything the peer asserted in a message. Passing the claimed id here instead would
/// make every check below decorative, which is the one way to get this catastrophically
/// wrong while every test still passes.
pub fn admit(
    credentials: &Credentials,
    claimed: NodeId,
    authenticated: NodeId,
    fleet: FleetId,
    revocations: &[Revocation],
    now: Millis,
) -> Result<Peer, Refusal> {
    let cert = &credentials.membership;

    // Identity first: it produces the most specific message, and a signature failure would
    // otherwise mask the far more likely case of a peer presenting somebody else's papers.
    if claimed != authenticated {
        return Err(Refusal::IdentityMismatch {
            certificate: claimed.short(),
            connection: authenticated.short(),
        });
    }
    if cert.member != authenticated {
        return Err(Refusal::IdentityMismatch {
            certificate: cert.member.short(),
            connection: authenticated.short(),
        });
    }

    // Revocation before signature: a revoked certificate still verifies perfectly, and the
    // subject may never argue with the fact that it was revoked (ADR-0012).
    //
    // `find`, not `any`: the refusal carries the revocation it was refused by (ADR-0044). It is
    // the one already verified here, so what travels is a fact this node checked rather than a
    // sentence it composed — which is what lets the subject check it too.
    if let Some(proof) = revocations
        .iter()
        .find(|r| r.covers(authenticated) && r.verify(fleet).is_ok())
    {
        return Err(Refusal::Revoked {
            node: authenticated.short(),
            proof: Box::new(proof.clone()),
        });
    }

    // A renewal leans on its renewer's `Renew` (ADR-0069 §2), and a revoked renewer renews
    // nothing. `NotAMember` rather than `Revoked`: it is the *renewer* that was revoked, and
    // `Refusal::Revoked` is the subject's own eviction, which it acts on by standing down.
    if let Some(renewer) = cert.renewer() {
        if revocations
            .iter()
            .any(|r| r.covers(renewer) && r.verify(fleet).is_ok())
        {
            return Err(Refusal::NotAMember {
                reason: format!(
                    "renewed by {}, which has been revoked — a membership it renewed needs \
                     renewing again by a member that has not",
                    renewer.short()
                ),
            });
        }
    }

    cert.verify(fleet, credentials.delegation.as_ref(), now)
        .map_err(|e| match e {
            offload_core::MembershipError::WrongFleet { expected, found } => {
                Refusal::WrongFleet { expected, found }
            }
            other => Refusal::NotAMember {
                reason: other.to_string(),
            },
        })?;

    Ok(Peer {
        node: authenticated,
        name: cert.name.clone(),
        membership: cert.clone(),
        delegation: credentials.delegation.clone(),
    })
}

/// Build this node's opener.
#[must_use]
pub fn hello(node: NodeId, name: impl Into<String>, credentials: Credentials) -> Hello {
    Hello {
        versions: VersionRange::ours(),
        node,
        name: name.into(),
        credentials,
    }
}

/// Answer an opener: negotiate a version, or refuse with both ranges in the message.
pub fn welcome(
    hello: &Hello,
    node: NodeId,
    name: impl Into<String>,
    credentials: Credentials,
) -> Result<Welcome, Refusal> {
    let ours = VersionRange::ours();
    let version = ours
        .negotiate(hello.versions)
        .ok_or(Refusal::IncompatibleVersion {
            ours,
            theirs: hello.versions,
        })?;
    Ok(Welcome {
        version,
        node,
        name: name.into(),
        credentials,
    })
}

/// Check that the version a peer chose is one we actually offered.
///
/// The dialling side must not simply believe the number in a [`Welcome`]: a peer that names a
/// version we do not implement gets a refusal here rather than a parse failure three messages
/// later.
pub fn accept_version(chosen: crate::Version) -> Result<crate::Version, Refusal> {
    let ours = VersionRange::ours();
    if ours.supports(chosen) {
        Ok(chosen)
    } else {
        Err(Refusal::IncompatibleVersion {
            ours,
            theirs: VersionRange::exactly(chosen),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VERSION;
    use ed25519_dalek::SigningKey;
    use offload_core::fleet::{FleetKey, PROBATION};
    use offload_core::{Issuer, Terms};

    const NOW: Millis = Millis(1_700_000_000_000);
    const DAY: Millis = Millis(86_400_000);

    fn fleet_key() -> FleetKey {
        FleetKey::derive("abacus zoom yo-yo").expect("derive")
    }

    fn node_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn node_of(key: &SigningKey) -> NodeId {
        NodeId::from_bytes(key.verifying_key().to_bytes())
    }

    fn credentials_for(key: &FleetKey, terms: Terms) -> Credentials {
        Credentials {
            membership: MembershipCert::issue(key.signing_key(), Issuer::Fleet, terms),
            delegation: None,
        }
    }

    fn joined(key: &FleetKey, member: NodeId) -> Credentials {
        credentials_for(key, Terms::joining(key.id(), member, "phone", NOW))
    }

    fn founded(key: &FleetKey, member: NodeId) -> Credentials {
        credentials_for(key, Terms::founding(key.id(), member, "desktop", NOW))
    }

    #[test]
    fn a_member_is_admitted_with_the_grants_its_certificate_carries() {
        let fleet = fleet_key();
        let member = node_of(&node_key(2));
        let peer = admit(
            &founded(&fleet, member),
            member,
            member,
            fleet.id(),
            &[],
            NOW,
        )
        .expect("admitted");

        assert_eq!(peer.node, member);
        assert_eq!(peer.name, "desktop");
        assert!(peer.may(Grant::HostRuns, NOW));
    }

    #[test]
    fn a_certificate_is_not_a_bearer_token() {
        // The half of the check that is easy to skip: fleet.json is public and copyable, so
        // what makes a certificate non-transferable is proving possession of the key it
        // names. Without this, reading somebody's backup is joining their fleet.
        let fleet = fleet_key();
        let victim = node_of(&node_key(2));
        let thief = node_of(&node_key(9));
        let stolen = founded(&fleet, victim);

        assert!(matches!(
            admit(&stolen, victim, thief, fleet.id(), &[], NOW),
            Err(Refusal::IdentityMismatch { .. })
        ));
    }

    #[test]
    fn claiming_to_be_someone_else_is_caught_before_anything_is_verified() {
        let fleet = fleet_key();
        let member = node_of(&node_key(2));
        let credentials = founded(&fleet, member);

        assert!(matches!(
            admit(
                &credentials,
                node_of(&node_key(7)),
                member,
                fleet.id(),
                &[],
                NOW
            ),
            Err(Refusal::IdentityMismatch { .. })
        ));
    }

    #[test]
    fn a_stranger_from_another_fleet_is_refused_by_name() {
        // Two fleets on one LAN is the normal case for a work laptop, so this refusal has to
        // be legible rather than a generic signature failure.
        let ours = fleet_key();
        let theirs = FleetKey::derive("zebra puppy abacus").expect("derive");
        let member = node_of(&node_key(3));

        assert!(matches!(
            admit(
                &founded(&theirs, member),
                member,
                member,
                ours.id(),
                &[],
                NOW
            ),
            Err(Refusal::WrongFleet { .. })
        ));
    }

    #[test]
    fn a_revoked_member_is_refused_even_though_its_certificate_verifies() {
        // Revocation is checked before the signature precisely because the signature is
        // still good. The subject never gets to argue (ADR-0012).
        let fleet = fleet_key();
        let member = node_of(&node_key(4));
        let revocation = Revocation::issue(fleet.signing_key(), fleet.id(), member, NOW, 1);

        let refused = admit(
            &founded(&fleet, member),
            member,
            member,
            fleet.id(),
            std::slice::from_ref(&revocation),
            NOW,
        )
        .expect_err("a revoked member is refused");
        let Refusal::Revoked { node, proof } = refused else {
            panic!("refused for the wrong reason: {refused}");
        };
        assert_eq!(node, member.short());
        // The signed artifact itself, not a sentence about it: what the refused node can act
        // on is a revocation it verifies for itself (ADR-0044), and a refusal carrying only a
        // name is a peer's claim about a node — which is the one thing a node may not believe
        // about itself.
        assert_eq!(*proof, revocation, "the refusal carries what refused it");
        assert!(proof.verify(fleet.id()).is_ok());
        assert!(proof.covers(member));
    }

    /// ADR-0069: a membership renewed by a renewer that has since been revoked is refused, and
    /// the same renewal is admitted while the renewer stands.
    #[test]
    fn a_renewal_by_a_revoked_renewer_is_refused_and_by_a_member_in_good_standing_admitted() {
        let fleet = fleet_key();
        let (member_key, renewer_key) = (node_key(5), node_key(6));
        let (member, renewer) = (node_of(&member_key), node_of(&renewer_key));
        let approval = MembershipCert::issue(
            fleet.signing_key(),
            Issuer::Fleet,
            Terms::joining(fleet.id(), member, "vps-17", NOW),
        );
        let renewers_own = MembershipCert::issue(
            fleet.signing_key(),
            Issuer::Fleet,
            Terms::joining(fleet.id(), renewer, "vps-3", NOW),
        );
        let renewed = MembershipCert::issue(
            &renewer_key,
            Issuer::Renewer { node: renewer },
            approval.renewal_by_renewer(
                offload_core::RenewalProof {
                    approval: approval.clone(),
                    approval_delegation: None,
                    renewer: renewers_own,
                    renewer_delegation: None,
                },
                NOW + DAY,
                offload_core::fleet::CERT_LIFETIME,
            ),
        );
        let credentials = Credentials {
            membership: renewed,
            delegation: None,
        };

        let later = NOW + DAY + Millis(1);
        assert!(
            admit(&credentials, member, member, fleet.id(), &[], later).is_ok(),
            "the control: the renewer stands, so the renewal is admitted"
        );

        let revoked = Revocation::issue(fleet.signing_key(), fleet.id(), renewer, later, 1);
        let refused = admit(
            &credentials,
            member,
            member,
            fleet.id(),
            std::slice::from_ref(&revoked),
            later,
        )
        .expect_err("a revoked renewer renews nothing");
        let Refusal::NotAMember { reason } = refused else {
            panic!("refused for the wrong reason: {refused}");
        };
        assert!(reason.contains("has been revoked"), "{reason}");
    }

    #[test]
    fn a_forged_revocation_does_not_keep_a_member_out() {
        // Otherwise any node could evict any other by asserting one, and membership would be
        // refutable in the direction it must never be.
        let fleet = fleet_key();
        let impostor = FleetKey::derive("zebra puppy abacus").expect("derive");
        let member = node_of(&node_key(4));
        let forged = Revocation::issue(impostor.signing_key(), fleet.id(), member, NOW, 1);

        assert!(admit(
            &founded(&fleet, member),
            member,
            member,
            fleet.id(),
            &[forged],
            NOW
        )
        .is_ok());
    }

    #[test]
    fn an_expired_certificate_is_refused_with_its_reason() {
        let fleet = fleet_key();
        let member = node_of(&node_key(5));
        let credentials = founded(&fleet, member);
        let later = NOW + Millis(31 * DAY.0);

        let refusal = admit(&credentials, member, member, fleet.id(), &[], later)
            .expect_err("expired by then");
        assert!(matches!(refusal, Refusal::NotAMember { .. }));
        assert!(refusal.to_string().contains("expired"));
    }

    #[test]
    fn probation_is_enforced_by_the_peer_not_by_the_joiner() {
        // A node serving probation is admitted — it is a member — but without the grant that
        // matters. Leaving this to the joining node's own good manners would make it advice.
        let fleet = fleet_key();
        let member = node_of(&node_key(6));
        let mut terms = Terms::joining(fleet.id(), member, "phone", NOW);
        terms.grants.insert(Grant::HostRuns);
        let credentials = credentials_for(&fleet, terms);

        let fresh = admit(&credentials, member, member, fleet.id(), &[], NOW).expect("member");
        assert!(fresh.may(Grant::Submit, NOW));
        assert!(!fresh.may(Grant::HostRuns, NOW));

        // The same admitted peer, later: the certificate travels with the session, so
        // probation lifts on a live connection without anybody redialling.
        assert!(fresh.may(Grant::HostRuns, NOW + PROBATION));
    }

    #[test]
    fn the_listener_picks_a_version_both_sides_speak() {
        let fleet = fleet_key();
        let dialler = node_of(&node_key(1));
        let listener = node_of(&node_key(2));

        let opener = hello(dialler, "laptop", founded(&fleet, dialler));
        let reply = welcome(&opener, listener, "desktop", founded(&fleet, listener))
            .expect("compatible with itself");
        assert_eq!(reply.version, VERSION);
        assert_eq!(accept_version(reply.version), Ok(VERSION));
    }

    #[test]
    fn a_version_gap_refuses_with_both_ranges_in_the_message() {
        // "Incompatible" without the numbers is the least useful thing a distributed system
        // can say to somebody holding two devices.
        let fleet = fleet_key();
        let dialler = node_of(&node_key(1));
        let listener = node_of(&node_key(2));

        let mut opener = hello(dialler, "laptop", founded(&fleet, dialler));
        opener.versions = VersionRange {
            min: crate::Version(900),
            max: crate::Version(901),
        };

        let refusal = welcome(&opener, listener, "desktop", founded(&fleet, listener))
            .expect_err("no overlap");
        assert!(matches!(refusal, Refusal::IncompatibleVersion { .. }));
        assert!(refusal.to_string().contains("900"));
        // Whose range is whose, in words that read right at *both* ends — the refused node
        // prints this under `refused by desktop`, where "this node" would mean itself.
        let said = refusal.to_string();
        assert!(
            said.contains(&format!("the refuser speaks {}", VersionRange::ours())),
            "{said}"
        );
        assert!(
            said.contains(&format!("was offered {}", opener.versions)),
            "{said}"
        );
        assert!(!said.contains("this node"), "{said}");

        // And the dialling side does not take a made-up version on trust either.
        assert!(accept_version(crate::Version(900)).is_err());
    }

    #[test]
    fn every_handshake_message_round_trips_in_both_directions() {
        // The `Response::Runs(Vec<_>)` lesson from phase 1: an encoding that compiles and
        // fails at runtime is found by decoding what you encoded, not by encoding alone.
        let fleet = fleet_key();
        let node = node_of(&node_key(1));
        let messages = vec![
            Handshake::Hello(hello(node, "laptop", joined(&fleet, node))),
            Handshake::Welcome(Welcome {
                version: VERSION,
                node,
                name: "desktop".into(),
                credentials: founded(&fleet, node),
            }),
            Handshake::Refused(Refusal::IncompatibleVersion {
                ours: VersionRange::ours(),
                theirs: VersionRange::exactly(crate::Version(9)),
            }),
            Handshake::Refused(Refusal::Revoked {
                node: node.short(),
                proof: Box::new(Revocation::issue(
                    fleet.signing_key(),
                    fleet.id(),
                    node,
                    NOW,
                    1,
                )),
            }),
            // A unit variant belongs in this list too — an internally-tagged enum encodes one
            // differently from a struct variant, and this workspace has shipped an enum that
            // compiled and then failed at runtime with a bare encoding error. `NotAMember` is the
            // unit-adjacent one left after `Draining` went.
            Handshake::Refused(Refusal::NotAMember {
                reason: "no certificate".into(),
            }),
        ];

        for message in messages {
            let frame = crate::frame::encode(&message).expect("encode");
            let mut reader = crate::FrameReader::new();
            reader.feed(&frame);
            let body = reader.next_frame().expect("read").expect("complete");
            assert_eq!(
                crate::frame::decode::<Handshake>(&body).expect("decode"),
                message
            );
        }
    }
}
