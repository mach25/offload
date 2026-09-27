//! QUIC between real nodes, with the node key as the TLS credential.
//!
//! ADR-0015 fixes what is trusted here and it is a short list: **the peer's key, and nothing
//! else**. No certificate authority, no hostname, no address, no name a peer asserts about
//! itself. TLS 1.3 with RFC 7250 raw public keys makes that literal — what a peer presents in
//! the handshake *is* an ed25519 public key, which *is* its [`NodeId`] (ADR-0012), and the
//! handshake signature proves it holds the private half.
//!
//! That last clause is the whole security property. A membership certificate is public and
//! copyable; possession is what makes it non-transferable. So the key TLS authenticated is
//! passed to the membership handshake as its own argument, separate from anything the peer
//! claimed — see [`crate::greet`].
//!
//! ## Why raw public keys rather than a self-signed certificate
//!
//! An X.509 wrapper around a key we already trust adds a parser, a validity window nobody
//! consults, and a name field to get confused about. RFC 7250 removes all three: the
//! credential is a 44-byte `SubjectPublicKeyInfo`, and turning one into a `NodeId` is a prefix
//! check and a copy rather than a certificate parse. rustls implements both sides of it.

use crate::sends::{SendRefusal, SendRefusals, WatchedSocket};
use crate::{greet, respond, Connection, Membership, Session, Stream, Transport, TransportError};
use async_trait::async_trait;
use ed25519_dalek::pkcs8::EncodePrivateKey;
use ed25519_dalek::SigningKey;
use offload_core::NodeId;
use offload_proto::handshake::Peer;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{
    CertificateDer, PrivateKeyDer, ServerName, SubjectPublicKeyInfoDer, UnixTime,
};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

/// Application-layer protocol name. Two fleets running different major protocol versions on
/// one LAN fail at the TLS layer rather than three messages into a handshake.
const ALPN: &[u8] = b"offload/1";

/// How long a refused peer has to read its refusal before the connection is dropped.
const REFUSAL_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// How long one inbound handshake may take before its task gives up.
///
/// quinn's own default `max_idle_timeout` is 30 seconds, and this matches it deliberately: a
/// connection quinn would still consider live is never cut short here, so this bounds resources
/// without deciding anything. See [`QuicTransport::spawn_handshake`].
const HANDSHAKE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// Inbound handshakes that have finished and are waiting for somebody to call `accept`.
///
/// Deep enough that a burst of arrivals is absorbed, shallow enough that a serve loop nobody is
/// running does not hold connections open indefinitely: a full queue backs pressure up into the
/// handshake tasks, which is where it belongs.
const INBOUND_QUEUE: usize = 64;

/// SNI is mandatory in the QUIC handshake and meaningless here: the credential is a key, not a
/// name. A constant keeps it from looking like it is doing something.
const SERVER_NAME: &str = "offload.invalid";

/// DER prefix of a `SubjectPublicKeyInfo` for ed25519 — `SEQUENCE { SEQUENCE { OID 1.3.101.112 },
/// BIT STRING }`. Fixed for the algorithm, so an SPKI is this followed by the 32-byte key.
const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// The credential this node presents: its public key, in the one encoding TLS wants.
#[must_use]
pub fn spki_of(node: NodeId) -> Vec<u8> {
    let mut der = Vec::with_capacity(SPKI_PREFIX.len() + 32);
    der.extend_from_slice(&SPKI_PREFIX);
    der.extend_from_slice(node.as_bytes());
    der
}

/// Read a peer's key back out of what it presented.
///
/// Strict about the prefix: anything else is not an ed25519 raw public key, and guessing at a
/// key from bytes we do not recognise is how the wrong node gets treated as the right one.
#[must_use]
pub fn node_of_spki(der: &[u8]) -> Option<NodeId> {
    if der.len() != SPKI_PREFIX.len() + 32 || !der.starts_with(&SPKI_PREFIX) {
        return None;
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&der[SPKI_PREFIX.len()..]);
    Some(NodeId::from_bytes(key))
}

/// Where a node key might be reachable.
///
/// The only place in the system that knows about addresses (ADR-0015). mDNS and a seed list
/// both land here; everything above deals in [`NodeId`].
pub trait Resolver: Send + Sync + 'static {
    fn addresses(&self, node: NodeId) -> Vec<SocketAddr>;
    /// Record where a node was actually reached, so the next dial does not have to guess.
    fn learn(&self, node: NodeId, address: SocketAddr);
}

/// A resolver you fill in by hand: the seed list, and whatever inbound connections teach it.
#[derive(Debug, Default)]
pub struct StaticPeers {
    known: Mutex<HashMap<NodeId, Vec<SocketAddr>>>,
}

impl StaticPeers {
    #[must_use]
    pub fn new() -> StaticPeers {
        StaticPeers::default()
    }

    pub fn add(&self, node: NodeId, address: SocketAddr) {
        self.learn(node, address);
    }
}

impl Resolver for StaticPeers {
    fn addresses(&self, node: NodeId) -> Vec<SocketAddr> {
        self.known
            .lock()
            .map(|known| known.get(&node).cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    fn learn(&self, node: NodeId, address: SocketAddr) {
        if let Ok(mut known) = self.known.lock() {
            let entry = known.entry(node).or_default();
            // Most recently seen first: a laptop that moved networks is reachable at where it
            // just came from, not at where it was last week.
            entry.retain(|a| *a != address);
            entry.insert(0, address);
            entry.truncate(4);
        }
    }
}

pub struct QuicTransport {
    node: NodeId,
    name: String,
    membership: Arc<dyn Membership>,
    resolver: Arc<dyn Resolver>,
    endpoint: quinn::Endpoint,
    /// Datagrams this machine's kernel would not send, counted inside the socket (`sends`).
    refusals: Arc<SendRefusals>,
    signing: Arc<dyn rustls::sign::SigningKey>,
    provider: Arc<rustls::crypto::CryptoProvider>,
    /// Held by the transport itself, so [`Self::accept`]'s receiver never sees the channel end.
    handshakes: tokio::sync::mpsc::Sender<Result<Session, TransportError>>,
    /// Behind a lock because `accept` takes `&self`, and a receiver has one consumer.
    inbound: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Result<Session, TransportError>>>,
}

impl std::fmt::Debug for QuicTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicTransport")
            .field("node", &self.node.short())
            .field("name", &self.name)
            .field("bound", &self.endpoint.local_addr().ok())
            .finish()
    }
}

/// Bind the endpoint's socket — **both families** when the address is `[::]`.
///
/// A socket bound to `0.0.0.0` can neither be dialled on IPv6 nor dial it: quinn answers
/// `invalid remote address` to an AAAA seed before a datagram leaves (measured, session
/// ninety-two). Mobile data is largely IPv6-native, which is why ADR-0037 §0 says to measure it
/// first, and a v4-only listener makes a routable home address unreachable from the phone it is
/// for. `[::]` covers both, but only where `IPV6_V6ONLY` is off — the Linux default, not a
/// universal one — so it is set rather than inherited. A host that cannot do either (IPv6
/// disabled, or no v4-mapped addresses) binds `0.0.0.0` on the same port and says so: IPv4 is
/// the family most likely to work, and losing it to gain v6 would be the worse trade.
/// Any other address is bound exactly as given.
fn bind_socket(bind_to: SocketAddr) -> std::io::Result<std::net::UdpSocket> {
    if bind_to != SocketAddr::new(std::net::Ipv6Addr::UNSPECIFIED.into(), bind_to.port()) {
        return std::net::UdpSocket::bind(bind_to);
    }
    let dual = || -> std::io::Result<std::net::UdpSocket> {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        socket.set_only_v6(false)?;
        socket.bind(&bind_to.into())?;
        Ok(socket.into())
    };
    dual().or_else(|e| {
        let v4 = SocketAddr::new(std::net::Ipv4Addr::UNSPECIFIED.into(), bind_to.port());
        tracing::warn!(
            error = %e,
            fallback = %v4,
            "cannot listen on IPv6 and IPv4 together; listening on IPv4 only, so no IPv6 peer \
             can reach this node and no IPv6 seed can be dialled"
        );
        std::net::UdpSocket::bind(v4)
    })
}

impl QuicTransport {
    /// Bind an endpoint and start listening.
    ///
    /// `key` is this node's identity: the same key its `NodeId` is derived from and the same
    /// one its membership certificate names. Handing a different key here would produce a node
    /// that authenticates as one identity and claims another — which every peer refuses, at
    /// the one place worth refusing it.
    pub fn bind(
        bind_to: SocketAddr,
        key: &SigningKey,
        name: impl Into<String>,
        membership: Arc<dyn Membership>,
        resolver: Arc<dyn Resolver>,
    ) -> Result<QuicTransport, TransportError> {
        let node = NodeId::from_bytes(key.verifying_key().to_bytes());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let signing = signing_key(key, &provider)?;

        let mut server = server_config(node, &signing, &provider)?;
        server.transport_config(transport_config());

        // The socket is ours rather than quinn's, for one reason: quinn's own tokio socket
        // calls `UdpSocketState::send`, which answers `Ok(())` to every error but `WouldBlock`
        // — so a machine that will not send believes it did (`sends`). `WatchedSocket` counts
        // those and then answers exactly what quinn's would have.
        let refusals = Arc::new(SendRefusals::default());
        let socket =
            bind_socket(bind_to).map_err(|e| TransportError::io("binding a QUIC endpoint", &e))?;
        let socket = WatchedSocket::new(socket, refusals.clone())
            .map_err(|e| TransportError::io("preparing the QUIC socket", &e))?;
        let runtime = quinn::default_runtime().ok_or_else(|| TransportError::Io {
            context: "binding a QUIC endpoint".into(),
            reason: "no async runtime is running".into(),
        })?;
        let endpoint = quinn::Endpoint::new_with_abstract_socket(
            quinn::EndpointConfig::default(),
            Some(server),
            Arc::new(socket),
            runtime,
        )
        .map_err(|e| TransportError::io("binding a QUIC endpoint", &e))?;

        let (handshakes, inbound) = tokio::sync::mpsc::channel(INBOUND_QUEUE);
        Ok(QuicTransport {
            node,
            name: name.into(),
            membership,
            resolver,
            endpoint,
            refusals,
            signing,
            provider,
            handshakes,
            inbound: tokio::sync::Mutex::new(inbound),
        })
    }

    /// Datagrams this machine's kernel refused to send, since this daemon started.
    ///
    /// Not a decision and deliberately not one: a UDP send error is non-fatal and the
    /// connection stays up. It is here because the *symptom* of a muzzled machine is a peer
    /// that looks silent, and nothing else in the system can tell those apart — see
    /// [`crate::sends`].
    #[must_use]
    pub fn send_refusals(&self) -> Vec<SendRefusal> {
        self.refusals.by_destination()
    }

    /// …and the count including destinations past the breakdown's cap.
    #[must_use]
    pub fn sends_refused(&self) -> u64 {
        self.refusals.total()
    }

    /// The address this node is listening on, for a seed list or an mDNS advertisement.
    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        self.endpoint
            .local_addr()
            .map_err(|e| TransportError::io("reading the local address", &e))
    }

    /// Stop listening and close every connection.
    pub fn shutdown(&self) {
        self.endpoint.close(0u32.into(), b"shutting down");
    }

    /// Dial an address without knowing whose it is.
    ///
    /// The bootstrap case, and the one dial that cannot pin: a seed list is written by a human
    /// who knows where a node lives, not what its key is. Nothing is weakened by it —
    /// whoever answers still has to present a membership certificate for this fleet, and TLS
    /// still proves they hold the key that certificate names. What is given up is only the
    /// guarantee that the node answering is the one you *meant*, which for a seed is not a
    /// thing you knew in the first place.
    pub async fn discover(&self, address: SocketAddr) -> Result<Session, TransportError> {
        let connection =
            self.dial(None, address)
                .await
                .map_err(|reason| TransportError::Unreachable {
                    peer: address.to_string(),
                    reason,
                })?;
        let authenticated = authenticated_peer(&connection)?;
        self.resolver.learn(authenticated, address);

        let mut stream = open_stream(&connection).await?;
        let admitted = greet(
            &mut stream,
            self.membership.as_ref(),
            self.node,
            &self.name,
            authenticated,
        )
        .await?;
        Ok(Session {
            peer: admitted.clone(),
            connection: Box::new(QuicConnection {
                peer: authenticated,
                grants: admitted,
                connection,
            }),
        })
    }

    async fn dial(
        &self,
        peer: Option<NodeId>,
        address: SocketAddr,
    ) -> Result<quinn::Connection, String> {
        // A fresh client config per dial, because the expected key is *in* the verifier: the
        // pinning is what makes reaching the wrong node at a stale address a failure rather
        // than a surprise.
        let config = client_config(peer, self.node, &self.signing, &self.provider)
            .map_err(|e| e.to_string())?;
        // quinn's own word for this is `invalid remote address`, which blames the address. The
        // address is fine; the socket is one family, and the fix is on this node.
        if let Ok(local) = self.endpoint.local_addr() {
            if address.is_ipv6() && local.is_ipv4() {
                return Err(format!(
                    "this node listens on IPv4 only ({local}), so it cannot dial an IPv6 address \
                     — `listen = \"[::]:<port>\"` covers both"
                ));
            }
        }
        self.endpoint
            .connect_with(config, address, SERVER_NAME)
            .map_err(|e| e.to_string())?
            .await
            .map_err(|e| e.to_string())
    }
}

#[async_trait]
impl Transport for QuicTransport {
    fn node(&self) -> NodeId {
        self.node
    }

    async fn connect(&self, peer: NodeId) -> Result<Session, TransportError> {
        let addresses = self.resolver.addresses(peer);
        if addresses.is_empty() {
            return Err(TransportError::Unreachable {
                peer: peer.short(),
                reason: "no known address — not discovered yet".into(),
            });
        }

        let mut last = String::new();
        for address in addresses {
            match self.dial(Some(peer), address).await {
                Ok(connection) => {
                    let authenticated = authenticated_peer(&connection)?;
                    // Belt and braces: the verifier already pinned this, and a mismatch here
                    // would mean the pinning silently stopped working.
                    if authenticated != peer {
                        return Err(TransportError::Unreachable {
                            peer: peer.short(),
                            reason: format!("{} answered instead", authenticated.short()),
                        });
                    }
                    self.resolver.learn(peer, address);

                    let mut stream = open_stream(&connection).await?;
                    let admitted = greet(
                        &mut stream,
                        self.membership.as_ref(),
                        self.node,
                        &self.name,
                        authenticated,
                    )
                    .await?;
                    return Ok(Session {
                        peer: admitted.clone(),
                        connection: Box::new(QuicConnection {
                            peer: authenticated,
                            grants: admitted,
                            connection,
                        }),
                    });
                }
                Err(reason) => last = format!("{address}: {reason}"),
            }
        }
        Err(TransportError::Unreachable {
            peer: peer.short(),
            reason: last,
        })
    }

    /// Take inbound connections, and do **no** per-connection work here.
    ///
    /// Everything after `Endpoint::accept` used to run inline in this loop, which made one
    /// peer's handshake the whole node's inbound path. The step that hurts is
    /// [`accept_stream`] — an unbounded `accept_bi()` on a connection whose peer may open no
    /// stream at all — because quinn does not drive a handshake until the application takes the
    /// `Incoming`. So a single dialer that completed TLS and then went quiet parked this loop,
    /// and every *later* dialer got not a refusal, not a close, but **silence**: an Initial sent,
    /// retransmitted on PTO, nothing ever coming back, and an idle timeout. The kernel counts no
    /// drops, raw UDP on the same 4-tuple is 100%, and this node logs nothing inbound — the
    /// wedge sits above the syscall layer and below anything either end reports, which is why
    /// every instrument reads clean at once.
    ///
    /// It is self-sustaining, which is the part that made it look like a network fault: two
    /// nodes redialling a peer they cannot reach keep supplying the stalled connections that
    /// keep them from reaching it. `a_stalled_dialer_does_not_block_the_next_peer_from_getting_in`
    /// is the reproduction, and it needs one machine.
    async fn accept(&self) -> Result<Session, TransportError> {
        let mut inbound = self.inbound.lock().await;
        loop {
            tokio::select! {
                incoming = self.endpoint.accept() => {
                    let Some(incoming) = incoming else {
                        return Err(TransportError::Closed {
                            peer: self.node.short(),
                            reason: "endpoint closed".into(),
                        });
                    };
                    self.spawn_handshake(incoming);
                }
                // Never `None`: `self` holds a sender for as long as the transport exists.
                Some(finished) = inbound.recv() => return finished,
            }
        }
    }
}

impl QuicTransport {
    /// Run one inbound handshake on its own task and post the outcome to [`Self::accept`].
    fn spawn_handshake(&self, incoming: quinn::Incoming) {
        let remote = incoming.remote_address();
        // **Logged because its absence was indistinguishable from the loop never running.**
        // A node that had accepted nothing for ten minutes looked exactly like a node nobody
        // had dialled. That cost a cross-platform walk its diagnosis.
        tracing::debug!(%remote, "inbound connection arriving");

        let membership = self.membership.clone();
        let resolver = self.resolver.clone();
        let finished = self.handshakes.clone();
        let node = self.node;
        let name = self.name.clone();
        tokio::spawn(async move {
            // A bound rather than a fix: the fix is that this runs off the accept loop at all.
            // It is set at quinn's own default idle timeout so it cannot fire before quinn
            // would have broken the connection anyway — it changes no outcome, it only stops a
            // peer that holds a connection open and opens nothing on it from accumulating tasks.
            let outcome = tokio::time::timeout(
                HANDSHAKE_DEADLINE,
                inbound_handshake(incoming, remote, &*membership, &*resolver, node, &name),
            )
            .await;
            match outcome {
                // One peer failing to connect is not this node's problem, and it is not worth
                // waking the accept loop for: keep listening rather than reporting somebody
                // else's TLS as an event.
                Ok(None) => {}
                Ok(Some(result)) => {
                    let _ = finished.send(result).await;
                }
                Err(_) => {
                    tracing::debug!(%remote, "inbound handshake exceeded its deadline");
                }
            }
        });
    }
}

/// One inbound handshake, start to finish, off the accept loop.
///
/// `None` is the disposition that used to be `continue`: a peer whose TLS failed, which is
/// nobody's business but theirs. `Some(Err(..))` is a refusal or a broken exchange, which the
/// caller of `accept` is entitled to see — and which is deliberately **never**
/// [`TransportError::Closed`], because the serve loop above reads that one variant as *the
/// endpoint is gone* and stops listening for good.
async fn inbound_handshake(
    incoming: quinn::Incoming,
    remote: SocketAddr,
    membership: &dyn Membership,
    resolver: &dyn Resolver,
    node: NodeId,
    name: &str,
) -> Option<Result<Session, TransportError>> {
    let connection = match incoming.await {
        Ok(connection) => connection,
        Err(e) => {
            tracing::debug!(%remote, error = %e, "inbound connection failed");
            return None;
        }
    };

    let authenticated = match authenticated_peer(&connection) {
        Ok(authenticated) => authenticated,
        Err(e) => {
            // Re-shaped on purpose. `authenticated_peer` reports `Closed`, meaning "this
            // credential is not one" — and the serve loop reads `Closed` from `accept` as the
            // endpoint having gone away and returns, so one peer arriving with a credential
            // that is not an ed25519 key would have taken this node's inbound path down
            // permanently. Read rather than measured: reaching it needs a TLS client this
            // crate cannot build. Fixed anyway, because the cost of being wrong is the whole
            // failure mode above.
            tracing::debug!(%remote, error = %e, "inbound peer presented no usable credential");
            return Some(Err(TransportError::io(
                "authenticating an inbound peer",
                &e,
            )));
        }
    };
    resolver.learn(authenticated, remote);
    tracing::debug!(%remote, peer = %authenticated.short(), "inbound TLS done; awaiting the handshake stream");

    let mut stream = match accept_stream(&connection).await {
        Ok(stream) => stream,
        Err(e) => return Some(Err(e)),
    };
    tracing::debug!(%remote, peer = %authenticated.short(), "inbound handshake stream open");
    let admitted = match respond(&mut stream, membership, node, name, authenticated).await {
        Ok(admitted) => admitted,
        Err(e) => {
            tracing::debug!(%remote, peer = %authenticated.short(), error = %e, "inbound handshake refused");
            // Let the refusal land before the connection goes away. quinn discards anything
            // still unsent when a connection is dropped, and a peer that is refused without
            // being told why retries for ever.
            let _ = tokio::time::timeout(REFUSAL_GRACE, connection.closed()).await;
            return Some(Err(e));
        }
    };

    tracing::debug!(%remote, peer = %authenticated.short(), name = %admitted.name, "inbound peer admitted");
    Some(Ok(Session {
        peer: admitted.clone(),
        connection: Box::new(QuicConnection {
            peer: authenticated,
            grants: admitted,
            connection,
        }),
    }))
}

pub struct QuicConnection {
    peer: NodeId,
    grants: Peer,
    connection: quinn::Connection,
}

impl std::fmt::Debug for QuicConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicConnection")
            .field("peer", &self.peer.short())
            .field("remote", &self.connection.remote_address())
            .finish()
    }
}

#[async_trait]
impl Connection for QuicConnection {
    fn peer(&self) -> NodeId {
        self.peer
    }

    fn grants(&self) -> &Peer {
        &self.grants
    }

    async fn open(&self) -> Result<Stream, TransportError> {
        open_stream(&self.connection).await
    }

    async fn accept(&self) -> Result<Stream, TransportError> {
        accept_stream(&self.connection).await
    }

    fn close(&self, reason: &str) {
        self.connection.close(0u32.into(), reason.as_bytes());
    }

    fn is_closed(&self) -> bool {
        self.connection.close_reason().is_some()
    }
}

async fn open_stream(connection: &quinn::Connection) -> Result<Stream, TransportError> {
    let (send, recv) = connection
        .open_bi()
        .await
        .map_err(|e| TransportError::io("opening a stream", &e))?;
    Ok(Stream::new(Box::new(send), Box::new(recv)))
}

async fn accept_stream(connection: &quinn::Connection) -> Result<Stream, TransportError> {
    let (send, recv) = connection
        .accept_bi()
        .await
        .map_err(|e| TransportError::io("accepting a stream", &e))?;
    Ok(Stream::new(Box::new(send), Box::new(recv)))
}

/// The key TLS proved possession of.
fn authenticated_peer(connection: &quinn::Connection) -> Result<NodeId, TransportError> {
    let identity = connection
        .peer_identity()
        .ok_or_else(|| unauthenticated("peer presented no credential"))?;
    let credentials = identity
        .downcast::<Vec<CertificateDer>>()
        .map_err(|_| unauthenticated("peer credential was not a TLS credential"))?;
    let first = credentials
        .first()
        .ok_or_else(|| unauthenticated("peer presented an empty credential"))?;
    node_of_spki(first).ok_or_else(|| unauthenticated("peer credential is not an ed25519 key"))
}

fn unauthenticated(reason: &str) -> TransportError {
    TransportError::Closed {
        peer: "unknown".into(),
        reason: reason.to_string(),
    }
}

fn signing_key(
    key: &SigningKey,
    provider: &rustls::crypto::CryptoProvider,
) -> Result<Arc<dyn rustls::sign::SigningKey>, TransportError> {
    let pkcs8 = key
        .to_pkcs8_der()
        .map_err(|e| TransportError::io("encoding the node key", &e))?;
    let der = PrivateKeyDer::try_from(pkcs8.as_bytes().to_vec())
        .map_err(|e| TransportError::io("reading the node key", &e))?;
    provider
        .key_provider
        .load_private_key(der)
        .map_err(|e| TransportError::io("loading the node key", &e))
}

fn certified_key(
    node: NodeId,
    signing: &Arc<dyn rustls::sign::SigningKey>,
) -> Arc<rustls::sign::CertifiedKey> {
    // With raw public keys the "certificate" is the SPKI itself (RFC 7250).
    Arc::new(rustls::sign::CertifiedKey::new(
        vec![CertificateDer::from(spki_of(node))],
        signing.clone(),
    ))
}

fn server_config(
    node: NodeId,
    signing: &Arc<dyn rustls::sign::SigningKey>,
    provider: &Arc<rustls::crypto::CryptoProvider>,
) -> Result<quinn::ServerConfig, TransportError> {
    let mut tls = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| TransportError::io("configuring TLS", &e))?
        .with_client_cert_verifier(Arc::new(AnyKeyVerifier {
            provider: provider.clone(),
        }))
        .with_cert_resolver(Arc::new(OnlyKey(certified_key(node, signing))));
    tls.alpn_protocols = vec![ALPN.to_vec()];

    let tls = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
        .map_err(|e| TransportError::io("configuring QUIC", &e))?;
    Ok(quinn::ServerConfig::with_crypto(Arc::new(tls)))
}

/// The transport settings both ends share, and the one thing in them that is not a default.
///
/// **MTU discovery is off.** quinn enables DPLPMTUD by default: it opens at a safe 1200-byte
/// datagram and then probes upward, 1452 being the first step. Measured between a Fedora laptop
/// and a Mac mini on one switch — the handshake completes at 1200 and the 1452-byte probe comes
/// back from macOS's `sendmsg` as `EHOSTUNREACH` rather than being dropped, which is not a
/// failure mode a path-MTU probe is designed to survive: every subsequent liveness probe reports
/// `no answer within 500ms` and each node marks the other dead. Linux to Linux is unaffected,
/// which is why no test caught it — every multi-node test in this project's history ran two or
/// three daemons on one Linux machine.
///
/// The cost of turning it off is the difference between a 1200-byte datagram and a ~1450-byte one
/// on a path that would carry the larger, which is a few percent of throughput on checkpoint
/// replication and nothing at all on gossip. The cost of leaving it on is a fleet that cannot
/// contain a Mac. 1200 is what QUIC guarantees every path carries (RFC 9000 §14), so this is the
/// conservative floor rather than a guess.
fn transport_config() -> Arc<quinn::TransportConfig> {
    let mut config = quinn::TransportConfig::default();
    config.mtu_discovery_config(None);
    Arc::new(config)
}

fn client_config(
    expected: Option<NodeId>,
    me: NodeId,
    signing: &Arc<dyn rustls::sign::SigningKey>,
    provider: &Arc<rustls::crypto::CryptoProvider>,
) -> Result<quinn::ClientConfig, TransportError> {
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| TransportError::io("configuring TLS", &e))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedKeyVerifier {
            expected,
            provider: provider.clone(),
        }))
        .with_client_cert_resolver(Arc::new(OnlyKey(certified_key(me, signing))));
    tls.alpn_protocols = vec![ALPN.to_vec()];

    let tls = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|e| TransportError::io("configuring QUIC", &e))?;
    let mut config = quinn::ClientConfig::new(Arc::new(tls));
    config.transport_config(transport_config());
    Ok(config)
}

/// This node has exactly one credential and presents it to everybody.
#[derive(Debug)]
struct OnlyKey(Arc<rustls::sign::CertifiedKey>);

impl rustls::server::ResolvesServerCert for OnlyKey {
    fn resolve(
        &self,
        _hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(self.0.clone())
    }

    fn only_raw_public_keys(&self) -> bool {
        true
    }
}

impl rustls::client::ResolvesClientCert for OnlyKey {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _schemes: &[SignatureScheme],
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(self.0.clone())
    }

    fn has_certs(&self) -> bool {
        true
    }

    fn only_raw_public_keys(&self) -> bool {
        true
    }
}

/// Dialling: the peer must be the exact key we meant to reach.
///
/// This is the pinning. Without it, reaching *a* node at a remembered address and treating it
/// as *the* node is a one-line mistake with no visible symptom.
#[derive(Debug)]
struct PinnedKeyVerifier {
    /// `None` only for a seed dial, where the caller knew an address and not a key.
    expected: Option<NodeId>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for PinnedKeyVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match (node_of_spki(end_entity), self.expected) {
            (Some(node), Some(expected)) if node == expected => Ok(ServerCertVerified::assertion()),
            (Some(_), None) => Ok(ServerCertVerified::assertion()),
            _ => Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            )),
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        // TLS 1.3 only; QUIC requires it and nothing here should ever see 1.2.
        Err(rustls::Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_with_key(&self.provider, message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

/// Listening: anybody may connect, and who they are is whatever key they proved.
///
/// Deliberately not "anybody is welcome": membership is checked immediately afterwards, in the
/// handshake, where a refusal can carry a reason. Refusing at the TLS layer would give the
/// peer a bare connection error and us nothing to log.
#[derive(Debug)]
struct AnyKeyVerifier {
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ClientCertVerifier for AnyKeyVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        if node_of_spki(end_entity).is_some() {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_with_key(&self.provider, message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        // Every peer authenticates. A connection whose other end is anonymous has nothing to
        // check a membership certificate against.
        true
    }

    fn offer_client_auth(&self) -> bool {
        true
    }
}

/// The proof of possession: the presented key signed this handshake.
fn verify_with_key(
    provider: &rustls::crypto::CryptoProvider,
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, rustls::Error> {
    rustls::crypto::verify_tls13_signature_with_raw_key(
        message,
        &SubjectPublicKeyInfoDer::from(cert.as_ref()),
        dss,
        &provider.signature_verification_algorithms,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use offload_core::fleet::{FleetKey, Grant};
    use offload_core::{FleetId, Issuer, MembershipCert, Millis, Revocation, Terms};
    use offload_proto::handshake::Credentials;
    use std::time::Duration;

    const NOW: Millis = Millis(1_700_000_000_000);

    struct Fixed {
        fleet: FleetId,
        credentials: Credentials,
    }

    impl Membership for Fixed {
        fn fleet(&self) -> FleetId {
            self.fleet
        }
        fn credentials(&self) -> Credentials {
            self.credentials.clone()
        }
        fn revocations(&self) -> Vec<Revocation> {
            Vec::new()
        }
        fn now(&self) -> Millis {
            NOW
        }
    }

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn membership(fleet: &FleetKey, key: &SigningKey, name: &str) -> Arc<dyn Membership> {
        let node = NodeId::from_bytes(key.verifying_key().to_bytes());
        Arc::new(Fixed {
            fleet: fleet.id(),
            credentials: Credentials {
                membership: MembershipCert::issue(
                    fleet.signing_key(),
                    Issuer::Fleet,
                    Terms::founding(fleet.id(), node, name, NOW),
                ),
                delegation: None,
            },
        })
    }

    fn local() -> SocketAddr {
        "127.0.0.1:0".parse().expect("address")
    }

    fn node_of(key: &SigningKey) -> NodeId {
        NodeId::from_bytes(key.verifying_key().to_bytes())
    }

    #[test]
    fn an_spki_round_trips_through_the_wire_encoding() {
        let node = node_of(&key(1));
        assert_eq!(node_of_spki(&spki_of(node)), Some(node));
        assert_eq!(spki_of(node).len(), 44);
    }

    #[test]
    fn anything_that_is_not_an_ed25519_key_is_refused_rather_than_guessed_at() {
        // Guessing a key out of bytes we do not recognise is how the wrong node ends up
        // treated as the right one.
        assert_eq!(node_of_spki(&[]), None);
        assert_eq!(node_of_spki(&[0u8; 44]), None);
        let mut wrong_length = spki_of(node_of(&key(1)));
        wrong_length.push(0);
        assert_eq!(node_of_spki(&wrong_length), None);
    }

    /// **A reply the responder has "sent" must survive the responder dropping the stream.**
    ///
    /// This is the production sequence, and nothing tested it. `Cluster::serve_session` spawns a
    /// task per stream; that task calls `serve_stream`, which writes the answer with
    /// `Stream::send` — `write_all` plus `flush`, neither of which waits for transmission — then
    /// `finish()`es, then **returns, dropping the `Stream`**. quinn resets a `SendStream` dropped
    /// without being finished and discards whatever is still buffered, which is the same hazard
    /// the sibling test below documents one level up ("dropping a quinn endpoint closes its
    /// connections at once, discarding anything still in flight") and works around with a
    /// oneshot — a workaround that also means no test here has ever exercised the drop.
    ///
    /// The reply is deliberately **large**: a small one fits in a single datagram that goes out
    /// before anything can be dropped, which is exactly why a mesh of two daemons on one machine
    /// has always worked.
    #[tokio::test]
    async fn a_reply_survives_the_responder_dropping_the_stream() {
        let fleet = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let (a_key, b_key) = (key(1), key(2));

        let peers_a = Arc::new(StaticPeers::new());
        let a = QuicTransport::bind(
            local(),
            &a_key,
            "laptop",
            membership(&fleet, &a_key, "laptop"),
            peers_a.clone(),
        )
        .expect("bind a");
        let b = QuicTransport::bind(
            local(),
            &b_key,
            "desktop",
            membership(&fleet, &b_key, "desktop"),
            Arc::new(StaticPeers::new()),
        )
        .expect("bind b");
        peers_a.add(node_of(&b_key), b.local_addr().expect("addr"));

        // 64 KiB of reply: many datagrams, so whether it arrives depends on the connection
        // still being driven after the stream is dropped rather than on luck.
        let reply = "y".repeat(64 * 1024);
        let expected = reply.clone();

        let (done, wait) = tokio::sync::oneshot::channel::<()>();
        let listening = tokio::spawn(async move {
            let session = b.accept().await.expect("accepted");
            // The endpoint stays alive for the whole exchange — this test is about the
            // *stream* being dropped, not the endpoint, and conflating the two proves nothing.
            {
                let mut stream = session.connection.accept().await.expect("stream");
                let _got: String = stream.recv().await.expect("recv");
                stream.send(&reply).await.expect("send");
                stream.finish().await.expect("finish");
                // …and dropped here, exactly as the spawned task in `serve_session` drops it.
            }
            let _ = wait.await;
        });

        let session = a.connect(node_of(&b_key)).await.expect("connected");
        let mut stream = session.connection.open().await.expect("stream");
        stream.send(&"ping".to_string()).await.expect("send");
        let got: String = stream.recv().await.expect("the reply must arrive in full");
        assert_eq!(
            got.len(),
            expected.len(),
            "the responder dropped the stream after finishing it and {} of {} bytes survived",
            got.len(),
            expected.len()
        );
        assert_eq!(got, expected);

        let _ = done.send(());
        listening.await.expect("listener");
    }

    /// A machine that *can* speak says nothing, which is what makes the line worth printing
    /// only when there is one. Asserted on the same exchange the test below makes, because a
    /// counter that is always zero passes a test for zero.
    #[tokio::test]
    async fn a_machine_that_can_send_counts_no_refusals() {
        let fleet = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let (a_key, b_key) = (key(1), key(2));

        let peers_a = Arc::new(StaticPeers::new());
        let a = QuicTransport::bind(
            local(),
            &a_key,
            "laptop",
            membership(&fleet, &a_key, "laptop"),
            peers_a.clone(),
        )
        .expect("bind a");
        let b = QuicTransport::bind(
            local(),
            &b_key,
            "desktop",
            membership(&fleet, &b_key, "desktop"),
            Arc::new(StaticPeers::new()),
        )
        .expect("bind b");
        peers_a.add(node_of(&b_key), b.local_addr().expect("addr"));

        let (done, wait) = tokio::sync::oneshot::channel::<()>();
        let listening = tokio::spawn(async move {
            let session = b.accept().await.expect("accepted");
            let mut stream = session.connection.accept().await.expect("stream");
            let _got: String = stream.recv().await.expect("recv");
            stream.send(&"pong".to_string()).await.expect("send");
            let _ = wait.await;
        });

        let session = a.connect(node_of(&b_key)).await.expect("connected");
        let mut stream = session.connection.open().await.expect("stream");
        stream.send(&"ping".to_string()).await.expect("send");
        let _reply: String = stream.recv().await.expect("recv");

        assert_eq!(a.sends_refused(), 0, "loopback refused a datagram");
        assert!(a.send_refusals().is_empty());

        let _ = done.send(());
        listening.await.expect("listener");
        a.shutdown();
    }

    /// The muzzle, without a firewall or a second machine: a UDP socket with `SO_BROADCAST`
    /// unset — which quinn never sets — is refused `EACCES` by the kernel for every datagram
    /// addressed to the broadcast address. Nothing leaves the host, and the failure is the
    /// shape session sixty-six spent three sessions on: quinn discards the error, the dial
    /// runs its whole timeout, and the only report is that the far end said nothing.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_kernel_that_refuses_every_datagram_is_counted_rather_than_blamed_on_the_peer() {
        let fleet = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let a_key = key(1);
        let a = QuicTransport::bind(
            local(),
            &a_key,
            "laptop",
            membership(&fleet, &a_key, "laptop"),
            Arc::new(StaticPeers::new()),
        )
        .expect("bind a");

        let muzzled = SocketAddr::from(([255, 255, 255, 255], 9));
        // `discover` rather than `connect`, so no peer has to be resolvable: the point is what
        // happens to the datagrams, and there is nobody at the other end either way.
        let dialled =
            tokio::time::timeout(std::time::Duration::from_secs(3), a.discover(muzzled)).await;
        // Whether the dial times out here or inside quinn is not the assertion — the dial
        // cannot succeed, and either way the Initial packets were handed to the kernel.
        assert!(
            dialled.map_or(true, |r| r.is_err()),
            "a dial to a broadcast address must not succeed"
        );

        assert!(
            a.sends_refused() > 0,
            "the kernel refused every datagram and nothing counted one"
        );
        let refusals = a.send_refusals();
        assert_eq!(
            refusals.len(),
            1,
            "one destination was dialled: {refusals:?}"
        );
        assert_eq!(refusals[0].destination, muzzled);
        assert_eq!(refusals[0].refused, a.sends_refused());
        assert!(
            !refusals[0].last.is_empty(),
            "the kernel's own words are what an operator acts on"
        );
        a.shutdown();
    }

    #[tokio::test]
    async fn two_nodes_connect_over_real_sockets_and_exchange_a_message() {
        let fleet = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let (a_key, b_key) = (key(1), key(2));

        let peers_a = Arc::new(StaticPeers::new());
        let a = QuicTransport::bind(
            local(),
            &a_key,
            "laptop",
            membership(&fleet, &a_key, "laptop"),
            peers_a.clone(),
        )
        .expect("bind a");
        let b = QuicTransport::bind(
            local(),
            &b_key,
            "desktop",
            membership(&fleet, &b_key, "desktop"),
            Arc::new(StaticPeers::new()),
        )
        .expect("bind b");
        peers_a.add(node_of(&b_key), b.local_addr().expect("addr"));

        // The listener holds its endpoint open until the dialler says it is done: dropping a
        // quinn endpoint closes its connections at once, discarding anything still in flight,
        // which in a test looks exactly like a protocol bug.
        let (done, wait) = tokio::sync::oneshot::channel::<()>();
        let listening = tokio::spawn(async move {
            let session = b.accept().await.expect("accepted");
            let mut stream = session.connection.accept().await.expect("stream");
            let got: String = stream.recv().await.expect("recv");
            stream
                .send(&format!("{got}, and hello back"))
                .await
                .expect("send");
            let peer = session.peer.node;
            let _ = wait.await;
            peer
        });

        let session = a.connect(node_of(&b_key)).await.expect("connected");
        assert_eq!(session.peer.node, node_of(&b_key));
        assert_eq!(session.peer.name, "desktop");
        assert!(session.peer.may(Grant::HostRuns, NOW));

        let mut stream = session.connection.open().await.expect("stream");
        stream.send(&"hello".to_string()).await.expect("send");
        let reply: String = stream.recv().await.expect("recv");
        assert_eq!(reply, "hello, and hello back");

        let _ = done.send(());
        assert_eq!(listening.await.expect("task"), node_of(&a_key));
        a.shutdown();
    }

    #[tokio::test]
    async fn dialling_the_right_key_at_the_wrong_address_fails() {
        // The pinning, tested from the outside: an address is a hint, and a node that answers
        // at a remembered address is not thereby the node that used to be there.
        let fleet = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let (a_key, b_key, c_key) = (key(1), key(2), key(3));

        let peers = Arc::new(StaticPeers::new());
        let a = QuicTransport::bind(
            local(),
            &a_key,
            "laptop",
            membership(&fleet, &a_key, "laptop"),
            peers.clone(),
        )
        .expect("bind a");
        let c = QuicTransport::bind(
            local(),
            &c_key,
            "someone-else",
            membership(&fleet, &c_key, "someone-else"),
            Arc::new(StaticPeers::new()),
        )
        .expect("bind c");

        // We think b lives where c actually is.
        peers.add(node_of(&b_key), c.local_addr().expect("addr"));

        let listening = tokio::spawn(async move { c.accept().await.map(|_| ()) });
        assert!(matches!(
            a.connect(node_of(&b_key)).await,
            Err(TransportError::Unreachable { .. })
        ));
        listening.abort();
        a.shutdown();
    }

    #[tokio::test]
    async fn a_node_from_another_fleet_gets_a_connection_and_no_further() {
        // TLS says who you are; membership says whether we care. The refusal happens in the
        // handshake so that it can carry a reason.
        let ours = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let theirs = FleetKey::derive("zebra puppy abacus").expect("derive");
        let (a_key, b_key) = (key(1), key(2));

        let peers = Arc::new(StaticPeers::new());
        let a = QuicTransport::bind(
            local(),
            &a_key,
            "intruder",
            membership(&theirs, &a_key, "intruder"),
            peers.clone(),
        )
        .expect("bind a");
        let b = QuicTransport::bind(
            local(),
            &b_key,
            "desktop",
            membership(&ours, &b_key, "desktop"),
            Arc::new(StaticPeers::new()),
        )
        .expect("bind b");
        peers.add(node_of(&b_key), b.local_addr().expect("addr"));

        let (done, wait) = tokio::sync::oneshot::channel::<()>();
        let listening = tokio::spawn(async move {
            let refused = b.accept().await.map(|_| ());
            let _ = wait.await;
            refused
        });
        assert!(matches!(
            a.connect(node_of(&b_key)).await,
            Err(TransportError::Refused { .. })
        ));
        let _ = done.send(());
        assert!(matches!(
            listening.await.expect("task"),
            Err(TransportError::Refusing { .. })
        ));
        a.shutdown();
    }

    #[tokio::test]
    async fn a_seed_dial_learns_who_answered_and_still_checks_membership() {
        // A seed list is an address written down by a human, who knows where a node lives and
        // not what its key is. Bootstrapping cannot pin — but it still cannot let a stranger
        // in.
        let ours = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let theirs = FleetKey::derive("zebra puppy abacus").expect("derive");
        let (a_key, b_key, c_key) = (key(1), key(2), key(3));

        let a = QuicTransport::bind(
            local(),
            &a_key,
            "laptop",
            membership(&ours, &a_key, "laptop"),
            Arc::new(StaticPeers::new()),
        )
        .expect("bind a");
        let b = QuicTransport::bind(
            local(),
            &b_key,
            "desktop",
            membership(&ours, &b_key, "desktop"),
            Arc::new(StaticPeers::new()),
        )
        .expect("bind b");
        let stranger = QuicTransport::bind(
            local(),
            &c_key,
            "stranger",
            membership(&theirs, &c_key, "stranger"),
            Arc::new(StaticPeers::new()),
        )
        .expect("bind c");

        let (done, wait) = tokio::sync::oneshot::channel::<()>();
        let b_addr = b.local_addr().expect("addr");
        let listening = tokio::spawn(async move {
            let accepted = b.accept().await;
            let _ = wait.await;
            accepted
        });

        let session = a.discover(b_addr).await.expect("discovered");
        assert_eq!(
            session.peer.node,
            node_of(&b_key),
            "learned from the handshake"
        );
        let _ = done.send(());
        assert!(listening.await.expect("task").is_ok());

        // And the same dial against a node from another fleet gets nowhere.
        let (done, wait) = tokio::sync::oneshot::channel::<()>();
        let stranger_addr = stranger.local_addr().expect("addr");
        let refusing = tokio::spawn(async move {
            let refused = stranger.accept().await.map(|_| ());
            let _ = wait.await;
            refused
        });
        assert!(matches!(
            a.discover(stranger_addr).await,
            Err(TransportError::Refused { .. })
        ));
        let _ = done.send(());
        assert!(refusing.await.expect("task").is_err());
        a.shutdown();
    }

    #[tokio::test]
    async fn a_peer_with_no_known_address_is_unreachable_rather_than_a_hang() {
        let fleet = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let a_key = key(1);
        let a = QuicTransport::bind(
            local(),
            &a_key,
            "laptop",
            membership(&fleet, &a_key, "laptop"),
            Arc::new(StaticPeers::new()),
        )
        .expect("bind");

        assert!(matches!(
            a.connect(node_of(&key(2))).await,
            Err(TransportError::Unreachable { .. })
        ));
    }

    /// **One dialer that stalls must not shut the door on everybody else.**
    ///
    /// `accept` runs every step of an inbound handshake *inline* in its own loop, and one of
    /// those steps is [`accept_stream`] — an unbounded `accept_bi()` on a connection whose peer
    /// may never open a stream. A dialer that completes TLS and then goes quiet therefore parks
    /// the acceptor there, and because quinn only drives a handshake once the application has
    /// taken the `Incoming`, every *later* dialer gets no reply at all: not a refusal, not a
    /// close, silence, until it gives up on its own idle timeout.
    ///
    /// That is the shape a cross-platform walk kept reading as a network fault — the dialer's
    /// `quinn_proto` trace shows an Initial sent, retransmitted on PTO, and nothing ever coming
    /// back, while the kernel underneath counts no drops and raw UDP on the same 4-tuple is
    /// 100%. The wedge is above the syscall layer and below anything the acceptor logs, which is
    /// why every instrument read clean at once.
    ///
    /// No second machine, and no lossy link: a stalled dialer is a thing anybody can write.
    #[tokio::test]
    async fn a_stalled_dialer_does_not_block_the_next_peer_from_getting_in() {
        let fleet = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let (a_key, b_key, stall_key) = (key(1), key(2), key(3));

        let peers_a = Arc::new(StaticPeers::new());
        let a = QuicTransport::bind(
            local(),
            &a_key,
            "laptop",
            membership(&fleet, &a_key, "laptop"),
            peers_a.clone(),
        )
        .expect("bind a");
        let b = QuicTransport::bind(
            local(),
            &b_key,
            "desktop",
            membership(&fleet, &b_key, "desktop"),
            Arc::new(StaticPeers::new()),
        )
        .expect("bind b");
        let stalled = QuicTransport::bind(
            local(),
            &stall_key,
            "phone",
            membership(&fleet, &stall_key, "phone"),
            Arc::new(StaticPeers::new()),
        )
        .expect("bind stalled");

        let b_addr = b.local_addr().expect("addr");
        peers_a.add(node_of(&b_key), b_addr);

        // `Cluster::serve`, in miniature: accept in a loop and hand each session off. The loop
        // is the subject, so it has to be the real one.
        let accepted = Arc::new(tokio::sync::Notify::new());
        let signal = accepted.clone();
        let serving = tokio::spawn(async move {
            loop {
                match b.accept().await {
                    Ok(session) => {
                        signal.notify_one();
                        // Held, exactly as a spawned `serve_session` holds it.
                        tokio::spawn(async move {
                            let _session = session;
                            std::future::pending::<()>().await;
                        });
                    }
                    Err(TransportError::Closed { .. }) => return,
                    Err(_) => continue,
                }
            }
        });

        // The stalled peer: TLS completes, and then it opens nothing, ever.
        let _parked = stalled
            .dial(None, b_addr)
            .await
            .expect("TLS completes for a peer that then says nothing");

        // Long enough for the acceptor to have taken it and parked in `accept_bi`.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // And now an ordinary peer, which has done nothing wrong.
        let got = tokio::time::timeout(Duration::from_secs(5), a.connect(node_of(&b_key))).await;
        serving.abort();
        assert!(
            matches!(got, Ok(Ok(_))),
            "a peer that stalled after TLS parked the accept loop and the next dialer never got \
             in: {got:?}"
        );
        accepted.notify_waiters();
    }

    /// **`[::]` is both families, and `0.0.0.0` is one.** The default listen address was
    /// `0.0.0.0:7433`, which made this laptop's routable IPv6 address unreachable and every AAAA
    /// seed undiallable — quinn refuses the destination before a datagram leaves. Measured in
    /// session ninety-two against the whole bind × dial matrix; these are its rows that decide
    /// the default: a dual-stack node reaches and is reached on both, and a v4-only one cannot
    /// dial v6 at all, which is the control that says the dual rows prove something.
    #[tokio::test]
    async fn a_wildcard_v6_bind_speaks_both_families() {
        let fleet = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let cases = [
            ("[::]:0", "[::]:0", "::1", true),
            ("[::]:0", "[::]:0", "127.0.0.1", true),
            ("[::]:0", "0.0.0.0:0", "127.0.0.1", true),
            ("0.0.0.0:0", "[::]:0", "127.0.0.1", true),
            ("0.0.0.0:0", "0.0.0.0:0", "::1", false),
        ];
        for (n, (server, client, dial, reaches)) in cases.into_iter().enumerate() {
            let seed = u8::try_from(n).expect("few cases") * 2;
            let (a_key, b_key) = (key(40 + seed), key(41 + seed));
            let peers = Arc::new(StaticPeers::new());
            let a = QuicTransport::bind(
                client.parse().expect("address"),
                &a_key,
                "a",
                membership(&fleet, &a_key, "a"),
                peers.clone(),
            )
            .expect("bind a");
            let b = QuicTransport::bind(
                server.parse().expect("address"),
                &b_key,
                "b",
                membership(&fleet, &b_key, "b"),
                Arc::new(StaticPeers::new()),
            )
            .expect("bind b");
            let port = b.local_addr().expect("addr").port();
            let ip: std::net::IpAddr = dial.parse().expect("ip");
            peers.add(node_of(&b_key), SocketAddr::new(ip, port));

            let listening = tokio::spawn(async move {
                let accepted = b.accept().await.map(|_| ());
                (accepted, b)
            });
            let dialled =
                tokio::time::timeout(Duration::from_secs(5), a.connect(node_of(&b_key))).await;
            let got = matches!(dialled, Ok(Ok(_)));
            assert_eq!(
                got, reaches,
                "server {server}, client {client}, dialling {dial}: {dialled:?}"
            );
            if let Ok(Err(e)) = &dialled {
                // …and the refusal names the socket, not the address, since that is what to fix.
                assert!(e.to_string().contains("listens on IPv4 only"), "{e}");
            }
            listening.abort();
        }
    }
}
