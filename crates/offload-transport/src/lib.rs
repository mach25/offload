//! Connections between nodes, addressed by node key.
//!
//! ADR-0015: the transport's address is a [`NodeId`] and nothing above this crate learns
//! otherwise. Resolving a key to somewhere to send packets — mDNS today, a seed list, a relay
//! in phase 5 — happens in here and changes when the network does. That is what makes the
//! phase-5 question ("iroh, or build it?") a swap rather than a migration.
//!
//! The trait stays as small as QUIC itself: connect to a node, open a bidirectional stream,
//! accept one. No broadcast — fan-out is `offload-cluster`'s decision. No request/response —
//! a message gains its meaning in `offload-proto`.
//!
//! ## Two implementations, and the reason for the first
//!
//! [`memory`] is not a toy. ADR-0005's merge rules and ADR-0007's drop-off policy are the two
//! things in this system most in need of testing under partition and churn, and a test that
//! opens sockets is testing the kernel's timing rather than the merge. The memory transport
//! has an explicit [`memory::MemoryNetwork::partition`] and delivers deterministically, which
//! is the same argument that keeps `offload-core` pure, one layer out.
//!
//! [`quic`] is what real nodes use.
//!
//! ## Nobody gets a connection without being a member
//!
//! Both `connect` and `accept` complete the membership handshake before handing anything
//! back. A caller cannot skip it, forget it, or do it later, because there is no way to
//! obtain a [`Session`] except through it — which is the difference between a rule and a
//! convention. What comes back names an admitted [`Peer`](offload_proto::Peer), with the
//! grants the fleet gave it and probation already applied.

// Tests are allowed to panic loudly; the lint is about production paths.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod discovery;
pub mod memory;
pub mod quic;
pub mod sends;
pub mod stream;

pub use stream::Stream;

use async_trait::async_trait;
use offload_core::{FleetId, Millis, NodeId, Revocation};
use offload_proto::handshake::{self, Credentials, Handshake, Peer, Refusal};

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("no route to {peer}: {reason}")]
    Unreachable { peer: String, reason: String },
    #[error("refused by {peer}: {refusal}")]
    Refused { peer: String, refusal: Refusal },
    #[error("refusing {peer}: {refusal}")]
    Refusing { peer: String, refusal: Refusal },
    #[error("connection to {peer} closed: {reason}")]
    Closed { peer: String, reason: String },
    #[error("protocol error: {0}")]
    Frame(#[from] offload_proto::FrameError),
    #[error("{context}: {reason}")]
    Io { context: String, reason: String },
    // There was a `NotAMember` here — "this node has not joined a fleet, so it has nobody to talk
    // to" — constructed by nothing, and it describes a state this layer cannot be in: a transport
    // is handed its credentials by the daemon, so a node with no fleet never builds one. The layer
    // that knows says it (`offload_node::fleet::FleetError::NotAMember`), and the *peer's*
    // membership is refused at the handshake (`proto::handshake::Refusal::NotAMember`). Two real
    // answers at two real layers; this was a third at a layer with no way to ask the question.
}

impl TransportError {
    /// An I/O failure with the operation that caused it named. Public because callers above
    /// this crate — the cluster's blob transfer, for one — fail in the middle of a transport
    /// exchange and should not have to invent a second error type to say so.
    pub fn io(context: &str, e: &impl std::fmt::Display) -> TransportError {
        TransportError::Io {
            context: context.to_string(),
            reason: e.to_string(),
        }
    }
}

/// What this node presents about itself, and what it checks a peer against.
///
/// A trait rather than a struct because all of it moves: certificates are renewed on contact,
/// revocations arrive by gossip, and the clock advances. A snapshot taken when the transport
/// started would keep talking to a device revoked an hour ago.
///
/// `now` lives here for the same reason it is a parameter everywhere else — the memory
/// transport's tests drive it, and a handshake that reads the wall clock cannot be tested
/// against an expiring certificate without waiting a month.
pub trait Membership: Send + Sync + 'static {
    fn fleet(&self) -> FleetId;
    /// This node's certificate, and the delegation behind it if an approver issued it.
    fn credentials(&self) -> Credentials;
    /// Revocations this node has heard. Checked on every handshake, so a revoked peer stops
    /// getting in as soon as *we* know, without waiting for the fleet to converge (ADR-0012).
    fn revocations(&self) -> Vec<Revocation>;
    fn now(&self) -> Millis;
}

/// An open connection to an admitted peer.
#[async_trait]
pub trait Connection: Send + Sync {
    /// Who is on the other end — the key the transport authenticated, never a claim.
    fn peer(&self) -> NodeId;

    /// What the fleet permits that peer to be.
    fn grants(&self) -> &Peer;

    /// Open a bidirectional stream. One stream per exchange: a repo bundle must not be able
    /// to delay a failure-detector probe, which is the whole reason this is QUIC.
    async fn open(&self) -> Result<Stream, TransportError>;

    /// The next stream the peer opened.
    async fn accept(&self) -> Result<Stream, TransportError>;

    /// Stop the connection. Idempotent; a peer that is already gone is not an error.
    fn close(&self, reason: &str);

    /// Has this connection been closed, by either end? A session a caller would reuse rather
    /// than dial again has to be asked this: a closed one fails every stream opened on it, and
    /// the task that would forget it runs a moment after the close, not at it.
    fn is_closed(&self) -> bool;
}

/// A connection plus who it turned out to be.
pub struct Session {
    pub peer: Peer,
    pub connection: Box<dyn Connection>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("peer", &self.peer.node.short())
            .field("name", &self.peer.name)
            .finish()
    }
}

#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// This node's own id.
    fn node(&self) -> NodeId;

    /// Dial a peer by key and complete the membership handshake.
    async fn connect(&self, peer: NodeId) -> Result<Session, TransportError>;

    /// Accept the next inbound connection, membership handshake included. A peer that fails
    /// it never reaches the caller.
    async fn accept(&self) -> Result<Session, TransportError>;
}

/// Run the dialling half of the membership handshake on a freshly opened stream.
///
/// `authenticated` is the key the transport proved possession of — the TLS credential, or the
/// registered identity in the memory implementation. It is deliberately a separate argument
/// from anything the peer says about itself: a certificate is public and copyable, so the
/// proof of possession is the only thing that makes it non-transferable.
pub async fn greet(
    stream: &mut Stream,
    membership: &dyn Membership,
    me: NodeId,
    name: &str,
    authenticated: NodeId,
) -> Result<Peer, TransportError> {
    let hello = handshake::hello(me, name, membership.credentials());
    stream.send(&Handshake::Hello(hello)).await?;

    match stream.recv::<Handshake>().await? {
        Handshake::Welcome(welcome) => {
            handshake::accept_version(welcome.version).map_err(|refusal| {
                TransportError::Refusing {
                    peer: authenticated.short(),
                    refusal,
                }
            })?;
            let admitted = admit(
                membership,
                &welcome.credentials,
                welcome.node,
                authenticated,
            );
            if let Err(TransportError::Refusing { refusal, .. }) = &admitted {
                // Symmetry: they told us why they would refuse us, so tell them why we are
                // refusing them. A peer that is merely hung up on retries for ever.
                let _ = stream.send(&Handshake::Refused(refusal.clone())).await;
                let _ = stream.finish().await;
            }
            admitted
        }
        Handshake::Refused(refusal) => Err(TransportError::Refused {
            peer: authenticated.short(),
            refusal,
        }),
        Handshake::Hello(_) => Err(TransportError::Closed {
            peer: authenticated.short(),
            reason: "peer answered a hello with a hello".into(),
        }),
    }
}

/// Run the listening half. A refusal is sent to the peer before the error is returned, so the
/// other end learns why rather than seeing a connection drop.
pub async fn respond(
    stream: &mut Stream,
    membership: &dyn Membership,
    me: NodeId,
    name: &str,
    authenticated: NodeId,
) -> Result<Peer, TransportError> {
    let hello = match stream.recv::<Handshake>().await? {
        Handshake::Hello(hello) => hello,
        _ => {
            return Err(TransportError::Closed {
                peer: authenticated.short(),
                reason: "peer opened with something other than a hello".into(),
            })
        }
    };

    let peer = match admit(membership, &hello.credentials, hello.node, authenticated) {
        Ok(peer) => peer,
        Err(e) => {
            if let TransportError::Refusing { refusal, .. } = &e {
                // Best effort: the connection may already be gone, and the local refusal is
                // what matters. But a peer told "revoked" can act on it; one told nothing
                // retries for ever. Finish the stream so the bytes are on their way before
                // the caller tears the connection down.
                let _ = stream.send(&Handshake::Refused(refusal.clone())).await;
                let _ = stream.finish().await;
            }
            return Err(e);
        }
    };

    match handshake::welcome(&hello, me, name, membership.credentials()) {
        Ok(welcome) => {
            stream.send(&Handshake::Welcome(welcome)).await?;
            Ok(peer)
        }
        Err(refusal) => {
            let _ = stream.send(&Handshake::Refused(refusal.clone())).await;
            let _ = stream.finish().await;
            Err(TransportError::Refusing {
                peer: authenticated.short(),
                refusal,
            })
        }
    }
}

fn admit(
    membership: &dyn Membership,
    credentials: &Credentials,
    claimed: NodeId,
    authenticated: NodeId,
) -> Result<Peer, TransportError> {
    handshake::admit(
        credentials,
        claimed,
        authenticated,
        membership.fleet(),
        &membership.revocations(),
        membership.now(),
    )
    .map_err(|refusal| TransportError::Refusing {
        peer: authenticated.short(),
        refusal,
    })
}
