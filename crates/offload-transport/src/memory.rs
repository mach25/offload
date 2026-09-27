//! A transport that never touches a socket.
//!
//! This exists so that partition and churn can be *arranged* rather than provoked. ADR-0005's
//! merge rules and ADR-0007's drop-off policy are the parts of this system most likely to be
//! subtly wrong, and the only way to test them honestly is to cut the network at an exact
//! moment and heal it at another. Against real sockets that is a sleep and a hope.
//!
//! ```ignore
//! let net = MemoryNetwork::new();
//! let laptop = net.join(laptop_id, "laptop", laptop_membership);
//! let phone  = net.join(phone_id,  "phone",  phone_membership);
//! net.partition(laptop_id, phone_id);      // they can no longer reach each other
//! net.heal(laptop_id, phone_id);
//! ```
//!
//! Delivery is a `tokio::io::duplex` pipe per stream, so ordering within a stream is the
//! same guarantee QUIC gives and nothing else is promised.

use crate::{greet, respond, Connection, Membership, Session, Stream, Transport, TransportError};
use async_trait::async_trait;
use offload_core::NodeId;
use offload_proto::handshake::Peer;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::Notify;

/// How much a stream buffers before writes wait for the reader. Small on purpose: a test that
/// only passes because everything fits in a buffer is not testing much.
const PIPE_BYTES: usize = 16 * 1024;

/// The wire everybody is plugged into.
#[derive(Clone, Default)]
pub struct MemoryNetwork {
    inner: Arc<Mutex<Wire>>,
}

#[derive(Default)]
struct Wire {
    listeners: HashMap<NodeId, mpsc::UnboundedSender<Incoming>>,
    /// Unordered pairs that cannot reach each other. A partition is symmetric because a
    /// one-way network failure is a different scenario, and pretending they are the same is
    /// how half-open connections go untested.
    cut: HashSet<(NodeId, NodeId)>,
    /// Nodes nobody can dial, which can still dial out — and the connections they open work in
    /// both directions. A phone on a carrier, behind a firewall that admits nothing inbound
    /// (measured on a Samsung phone on mobile data: reachable only over the connection it opened).
    firewalled: HashSet<NodeId>,
}

struct Incoming {
    peer: NodeId,
    /// Streams the dialling side opens.
    streams: mpsc::UnboundedReceiver<Stream>,
    /// Where to put streams this side opens.
    opener: mpsc::UnboundedSender<Stream>,
    /// **Shared with the dialling end**, because a connection is one thing and closing it is one
    /// event. Two independent flags model a hang-up that only the hanging-up side can observe,
    /// which is not what any real transport does and is the precise shape of the bug session
    /// forty-two found one layer up: the half that mattered was the half nothing reached.
    closed: Arc<AtomicBool>,
    hung_up: Arc<Notify>,
}

impl std::fmt::Debug for MemoryNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock();
        match inner {
            Ok(wire) => f
                .debug_struct("MemoryNetwork")
                .field("nodes", &wire.listeners.len())
                .field("cuts", &wire.cut.len())
                .finish(),
            Err(_) => f.write_str("MemoryNetwork(poisoned)"),
        }
    }
}

fn pair(a: NodeId, b: NodeId) -> (NodeId, NodeId) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

impl MemoryNetwork {
    #[must_use]
    pub fn new() -> MemoryNetwork {
        MemoryNetwork::default()
    }

    /// Plug a node in. The returned transport is what that node holds.
    pub fn join(
        &self,
        node: NodeId,
        name: impl Into<String>,
        membership: Arc<dyn Membership>,
    ) -> MemoryTransport {
        let (tx, rx) = mpsc::unbounded_channel();
        if let Ok(mut wire) = self.inner.lock() {
            wire.listeners.insert(node, tx);
        }
        MemoryTransport {
            node,
            name: name.into(),
            membership,
            network: self.clone(),
            inbox: AsyncMutex::new(rx),
        }
    }

    /// Cut two nodes off from each other.
    ///
    /// Dialling fails, and so does opening a stream on a connection that already existed — a
    /// route disappearing does not politely wait for everyone to hang up first. Streams
    /// already open simply go quiet, which is what the failure detector's timeout is for.
    pub fn partition(&self, a: NodeId, b: NodeId) {
        if let Ok(mut wire) = self.inner.lock() {
            wire.cut.insert(pair(a, b));
        }
    }

    /// Put a node behind a firewall that admits nothing inbound: dialling it fails, and it can
    /// still dial out, over connections that carry streams both ways.
    pub fn firewall(&self, node: NodeId) {
        if let Ok(mut wire) = self.inner.lock() {
            wire.firewalled.insert(node);
        }
    }

    pub fn heal(&self, a: NodeId, b: NodeId) {
        if let Ok(mut wire) = self.inner.lock() {
            wire.cut.remove(&pair(a, b));
        }
    }

    /// Unplug a node entirely: nobody can dial it any more.
    pub fn leave(&self, node: NodeId) {
        if let Ok(mut wire) = self.inner.lock() {
            wire.listeners.remove(&node);
        }
    }

    fn reachable(&self, from: NodeId, to: NodeId) -> bool {
        self.inner
            .lock()
            .map(|wire| !wire.cut.contains(&pair(from, to)))
            .unwrap_or(false)
    }

    fn dial(&self, from: NodeId, to: NodeId) -> Result<MemoryConnection, TransportError> {
        let unreachable = |reason: &str| TransportError::Unreachable {
            peer: to.short(),
            reason: reason.to_string(),
        };
        let mut wire = self.inner.lock().map_err(|_| unreachable("network gone"))?;

        if wire.cut.contains(&pair(from, to)) {
            return Err(unreachable("partitioned"));
        }
        if wire.firewalled.contains(&to) {
            return Err(unreachable("behind a firewall that admits nothing inbound"));
        }
        let listener = wire
            .listeners
            .get_mut(&to)
            .ok_or_else(|| unreachable("no such node"))?;

        // Two stream channels, one per direction of *opening*. Each side sends the far half
        // of a fresh pipe to the other's queue.
        let (open_here, accept_there) = mpsc::unbounded_channel();
        let (open_there, accept_here) = mpsc::unbounded_channel();
        let closed = Arc::new(AtomicBool::new(false));
        let hung_up = Arc::new(Notify::new());

        listener
            .send(Incoming {
                peer: from,
                streams: accept_there,
                opener: open_there,
                closed: closed.clone(),
                hung_up: hung_up.clone(),
            })
            .map_err(|_| unreachable("node is not listening"))?;

        Ok(MemoryConnection {
            local: from,
            peer: to,
            grants: None,
            network: self.clone(),
            opener: open_here,
            incoming: AsyncMutex::new(accept_here),
            closed,
            hung_up,
        })
    }
}

pub struct MemoryTransport {
    node: NodeId,
    name: String,
    membership: Arc<dyn Membership>,
    network: MemoryNetwork,
    inbox: AsyncMutex<mpsc::UnboundedReceiver<Incoming>>,
}

impl std::fmt::Debug for MemoryTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryTransport")
            .field("node", &self.node.short())
            .field("name", &self.name)
            .finish()
    }
}

#[async_trait]
impl Transport for MemoryTransport {
    fn node(&self) -> NodeId {
        self.node
    }

    async fn connect(&self, peer: NodeId) -> Result<Session, TransportError> {
        let mut connection = self.network.dial(self.node, peer)?;
        let mut stream = connection.open().await?;

        // In memory there is no TLS, so "authenticated" is the identity the network has this
        // connection registered under. That is the same guarantee QUIC gives — you reached
        // the node you dialled — and keeping the argument explicit is what stops the real
        // implementation from quietly passing a claim instead.
        let admitted = greet(
            &mut stream,
            self.membership.as_ref(),
            self.node,
            &self.name,
            peer,
        )
        .await?;
        connection.grants = Some(admitted.clone());
        Ok(Session {
            peer: admitted,
            connection: Box::new(connection),
        })
    }

    async fn accept(&self) -> Result<Session, TransportError> {
        let incoming = {
            let mut inbox = self.inbox.lock().await;
            inbox.recv().await.ok_or(TransportError::Closed {
                peer: self.node.short(),
                reason: "this node has left the network".into(),
            })?
        };

        let mut connection = MemoryConnection {
            local: self.node,
            peer: incoming.peer,
            grants: None,
            network: self.network.clone(),
            opener: incoming.opener,
            incoming: AsyncMutex::new(incoming.streams),
            closed: incoming.closed,
            hung_up: incoming.hung_up,
        };
        let mut stream = connection.accept().await?;
        let admitted = respond(
            &mut stream,
            self.membership.as_ref(),
            self.node,
            &self.name,
            incoming.peer,
        )
        .await?;
        connection.grants = Some(admitted.clone());
        Ok(Session {
            peer: admitted,
            connection: Box::new(connection),
        })
    }
}

pub struct MemoryConnection {
    local: NodeId,
    peer: NodeId,
    grants: Option<Peer>,
    network: MemoryNetwork,
    opener: mpsc::UnboundedSender<Stream>,
    incoming: AsyncMutex<mpsc::UnboundedReceiver<Stream>>,
    /// Whether [`Connection::close`] has been called. Modelled rather than skipped, because a
    /// test transport whose `close` does nothing cannot tell a caller that *hung up* from one
    /// that merely *forgot* — and those are the two halves of a revocation. Dropping the
    /// `Session` used to be the only way a connection here ever ended, which made every test
    /// of a hang-up a test of the map it was removed from.
    ///
    /// **Shared with the far end** (`Arc`), because that is what a hang-up is: QUIC's `close`
    /// ends the connection, not one side's opinion of it. Modelled per-end, this flag let a peer
    /// that had been hung up on go on opening streams and gossiping — so a test could measure
    /// the closing side and learn nothing about the one that had to notice.
    closed: Arc<AtomicBool>,
    /// Wakes an `accept` already waiting when the close arrives. Without it a closed connection
    /// is only observed by the *next* caller, and the serve loop that is parked in `accept` —
    /// the one a revocation has to end — waits for ever.
    hung_up: Arc<Notify>,
}

impl std::fmt::Debug for MemoryConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryConnection")
            .field("peer", &self.peer.short())
            .finish()
    }
}

#[async_trait]
impl Connection for MemoryConnection {
    fn peer(&self) -> NodeId {
        self.peer
    }

    fn grants(&self) -> &Peer {
        // Only reachable through a `Session`, and a `Session` only exists after the
        // handshake filled this in.
        self.grants
            .as_ref()
            .unwrap_or_else(|| unreachable!("a connection without an admitted peer"))
    }

    async fn open(&self) -> Result<Stream, TransportError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(TransportError::Closed {
                peer: self.peer.short(),
                reason: "this connection was closed".into(),
            });
        }
        if !self.network.reachable(self.local, self.peer) {
            return Err(TransportError::Unreachable {
                peer: self.peer.short(),
                reason: "partitioned".into(),
            });
        }
        let (mine, theirs) = tokio::io::duplex(PIPE_BYTES);
        self.opener
            .send(wrap(theirs))
            .map_err(|_| TransportError::Closed {
                peer: self.peer.short(),
                reason: "peer closed the connection".into(),
            })?;
        Ok(wrap(mine))
    }

    async fn accept(&self) -> Result<Stream, TransportError> {
        let closed = TransportError::Closed {
            peer: self.peer.short(),
            reason: "peer closed the connection".into(),
        };
        if self.closed.load(Ordering::SeqCst) {
            return Err(closed);
        }
        let mut incoming = self.incoming.lock().await;
        tokio::select! {
            stream = incoming.recv() => stream.ok_or(closed),
            () = self.hung_up.notified() => Err(closed),
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn close(&self, reason: &str) {
        tracing::debug!(peer = %self.peer.short(), reason, "closing memory connection");
        self.closed.store(true, Ordering::SeqCst);
        // Whoever is parked in `accept` right now as well as whoever calls it next: the serve
        // loop a hang-up has to end is waiting inside `accept`, not about to enter it.
        self.hung_up.notify_waiters();
    }
}

fn wrap(duplex: tokio::io::DuplexStream) -> Stream {
    let (recv, send) = tokio::io::split(duplex);
    Stream::new(Box::new(send), Box::new(recv))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use offload_core::fleet::{FleetKey, Grant, PROBATION};
    use offload_core::{FleetId, Issuer, MembershipCert, Millis, Revocation, Terms};
    use offload_proto::handshake::Credentials;
    use std::sync::atomic::{AtomicU64, Ordering};

    const NOW: Millis = Millis(1_700_000_000_000);

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn id(seed: u8) -> NodeId {
        NodeId::from_bytes(key(seed).verifying_key().to_bytes())
    }

    fn fleet_key() -> FleetKey {
        FleetKey::derive("abacus zoom yo-yo").expect("derive")
    }

    /// A node's view of the fleet, with a clock and a revocation list a test can move.
    struct TestMembership {
        fleet: FleetId,
        credentials: Credentials,
        revocations: Mutex<Vec<Revocation>>,
        now: AtomicU64,
    }

    impl TestMembership {
        fn member(key: &FleetKey, node: NodeId, name: &str) -> Arc<TestMembership> {
            Self::with_terms(key, Terms::founding(key.id(), node, name, NOW))
        }

        fn joiner(key: &FleetKey, node: NodeId, name: &str) -> Arc<TestMembership> {
            let mut terms = Terms::joining(key.id(), node, name, NOW);
            terms.grants.insert(Grant::HostRuns);
            Self::with_terms(key, terms)
        }

        fn with_terms(key: &FleetKey, terms: Terms) -> Arc<TestMembership> {
            Arc::new(TestMembership {
                fleet: key.id(),
                credentials: Credentials {
                    membership: MembershipCert::issue(key.signing_key(), Issuer::Fleet, terms),
                    delegation: None,
                },
                revocations: Mutex::new(Vec::new()),
                now: AtomicU64::new(NOW.0),
            })
        }

        fn advance(&self, by: Millis) {
            self.now.fetch_add(by.0, Ordering::SeqCst);
        }

        fn revoke(&self, revocation: Revocation) {
            self.revocations.lock().expect("lock").push(revocation);
        }
    }

    impl Membership for TestMembership {
        fn fleet(&self) -> FleetId {
            self.fleet
        }
        fn credentials(&self) -> Credentials {
            self.credentials.clone()
        }
        fn revocations(&self) -> Vec<Revocation> {
            self.revocations.lock().expect("lock").clone()
        }
        fn now(&self) -> Millis {
            Millis(self.now.load(Ordering::SeqCst))
        }
    }

    #[tokio::test]
    async fn two_members_connect_and_learn_who_the_other_is() {
        let fleet = fleet_key();
        let net = MemoryNetwork::new();
        let a = net.join(
            id(1),
            "laptop",
            TestMembership::member(&fleet, id(1), "laptop"),
        );
        let b = net.join(
            id(2),
            "desktop",
            TestMembership::member(&fleet, id(2), "desktop"),
        );

        let listening = tokio::spawn(async move { b.accept().await });
        let dialled = a.connect(id(2)).await.expect("connected");
        let accepted = listening.await.expect("task").expect("accepted");

        assert_eq!(dialled.peer.node, id(2));
        assert_eq!(dialled.peer.name, "desktop");
        assert_eq!(accepted.peer.node, id(1));
        assert!(dialled.peer.may(Grant::HostRuns, NOW));
    }

    #[tokio::test]
    async fn messages_go_both_ways_on_a_stream() {
        let fleet = fleet_key();
        let net = MemoryNetwork::new();
        let a = net.join(
            id(1),
            "laptop",
            TestMembership::member(&fleet, id(1), "laptop"),
        );
        let b = net.join(
            id(2),
            "desktop",
            TestMembership::member(&fleet, id(2), "desktop"),
        );

        let listening = tokio::spawn(async move {
            let session = b.accept().await.expect("accepted");
            let mut stream = session.connection.accept().await.expect("stream");
            let got: String = stream.recv().await.expect("recv");
            stream
                .send(&format!("{got} to you too"))
                .await
                .expect("send");
        });

        let session = a.connect(id(2)).await.expect("connected");
        let mut stream = session.connection.open().await.expect("stream");
        stream.send(&"hello".to_string()).await.expect("send");
        let reply: String = stream.recv().await.expect("recv");

        assert_eq!(reply, "hello to you too");
        listening.await.expect("task");
    }

    #[tokio::test]
    async fn a_stranger_from_another_fleet_never_becomes_a_session() {
        // The property ADR-0015 asks for: refused in the handshake, not filtered later. The
        // caller has no way to obtain a connection to a non-member.
        let ours = fleet_key();
        let theirs = FleetKey::derive("zebra puppy abacus").expect("derive");
        let net = MemoryNetwork::new();
        let member = net.join(
            id(1),
            "laptop",
            TestMembership::member(&ours, id(1), "laptop"),
        );
        let stranger = net.join(
            id(2),
            "intruder",
            TestMembership::member(&theirs, id(2), "intruder"),
        );

        let listening = tokio::spawn(async move { member.accept().await.map(|_| ()) });
        let dialled = stranger.connect(id(1)).await;

        assert!(matches!(dialled, Err(TransportError::Refused { .. })));
        assert!(matches!(
            listening.await.expect("task"),
            Err(TransportError::Refusing { .. })
        ));
    }

    #[tokio::test]
    async fn a_revoked_peer_is_refused_as_soon_as_we_know() {
        // Revocation is immediate and local: no waiting for the fleet to converge.
        let fleet = fleet_key();
        let net = MemoryNetwork::new();
        let keeper = TestMembership::member(&fleet, id(1), "laptop");
        let a = net.join(id(1), "laptop", keeper.clone());
        let b = net.join(
            id(2),
            "desktop",
            TestMembership::member(&fleet, id(2), "desktop"),
        );

        keeper.revoke(Revocation::issue(
            fleet.signing_key(),
            fleet.id(),
            id(2),
            NOW,
            1,
        ));

        let listening = tokio::spawn(async move { a.accept().await.map(|_| ()) });
        assert!(matches!(
            b.connect(id(1)).await,
            Err(TransportError::Refused { .. })
        ));
        assert!(matches!(
            listening.await.expect("task"),
            Err(TransportError::Refusing { .. })
        ));
    }

    #[tokio::test]
    async fn probation_is_visible_to_the_peer_and_lifts_on_its_own() {
        let fleet = fleet_key();
        let net = MemoryNetwork::new();
        let host = TestMembership::member(&fleet, id(1), "desktop");
        let a = net.join(id(1), "desktop", host.clone());
        let joiner = TestMembership::joiner(&fleet, id(2), "phone");
        let b = net.join(id(2), "phone", joiner.clone());

        let a = Arc::new(a);
        let listening = tokio::spawn({
            let a = a.clone();
            async move { a.accept().await }
        });
        b.connect(id(1)).await.expect("connected");
        let session = listening.await.expect("task").expect("accepted");
        assert!(
            !session.peer.may(Grant::HostRuns, NOW),
            "still serving probation"
        );

        // Time passes on the *observing* node. Nothing is re-issued and nothing is told to
        // anybody: the certificate already said this, and every node computes it.
        host.advance(PROBATION);
        joiner.advance(PROBATION);
        let listening = tokio::spawn({
            let a = a.clone();
            async move { a.accept().await }
        });
        b.connect(id(1)).await.expect("connected");
        let session = listening.await.expect("task").expect("accepted");
        assert!(
            session.peer.may(Grant::HostRuns, NOW + PROBATION),
            "probation has lapsed"
        );
    }

    #[tokio::test]
    async fn a_partition_breaks_the_connection_that_was_already_open() {
        // A route disappearing does not wait for everyone to hang up first, and a failure
        // detector that keeps getting answers over a cut link is not being tested at all.
        let fleet = fleet_key();
        let net = MemoryNetwork::new();
        let a = net.join(
            id(1),
            "laptop",
            TestMembership::member(&fleet, id(1), "laptop"),
        );
        let b = net.join(
            id(2),
            "desktop",
            TestMembership::member(&fleet, id(2), "desktop"),
        );

        let listening = tokio::spawn(async move { b.accept().await });
        let session = a.connect(id(2)).await.expect("connected");
        // Held, not dropped: the far end of a connection going away is a different failure
        // from the route between them going away, and this test is about the second.
        let _far_end = listening.await.expect("task").expect("accepted");
        assert!(session.connection.open().await.is_ok());

        net.partition(id(1), id(2));
        assert!(matches!(
            session.connection.open().await,
            Err(TransportError::Unreachable { .. })
        ));
    }

    #[tokio::test]
    async fn a_partition_makes_a_peer_unreachable_and_healing_restores_it() {
        let fleet = fleet_key();
        let net = MemoryNetwork::new();
        let a = net.join(
            id(1),
            "laptop",
            TestMembership::member(&fleet, id(1), "laptop"),
        );
        let b = net.join(
            id(2),
            "desktop",
            TestMembership::member(&fleet, id(2), "desktop"),
        );

        net.partition(id(1), id(2));
        assert!(matches!(
            a.connect(id(2)).await,
            Err(TransportError::Unreachable { .. })
        ));
        // Symmetric, because a one-way failure is a different scenario.
        assert!(matches!(
            b.connect(id(1)).await,
            Err(TransportError::Unreachable { .. })
        ));

        net.heal(id(1), id(2));
        let listening = tokio::spawn(async move { b.accept().await });
        assert!(a.connect(id(2)).await.is_ok());
        assert!(listening.await.expect("task").is_ok());
    }

    #[tokio::test]
    async fn dialling_a_node_that_left_says_so_rather_than_hanging() {
        let fleet = fleet_key();
        let net = MemoryNetwork::new();
        let a = net.join(
            id(1),
            "laptop",
            TestMembership::member(&fleet, id(1), "laptop"),
        );
        net.join(
            id(2),
            "desktop",
            TestMembership::member(&fleet, id(2), "desktop"),
        );
        net.leave(id(2));

        assert!(matches!(
            a.connect(id(2)).await,
            Err(TransportError::Unreachable { .. })
        ));
    }
}
