//! Who is out there, and who has gone quiet.
//!
//! This is the loop that turns a [`Transport`] into a fleet: it probes peers, spreads what it
//! believes, merges what it hears, and keeps a [`ClusterView`] that every other part of the
//! system reads. It owns no policy — the decisions live in `offload-core` (merge rules,
//! ADR-0005) and [`detector`] (when silence becomes suspicion) — and it makes no decision
//! about work at all.
//!
//! ## Nothing here is on a hidden timer
//!
//! [`Cluster::probe_round`] is one SWIM period, and it takes the time as an argument. Running
//! it on a schedule is [`Cluster::run`], a dozen lines that sleep and call it. That split is
//! the whole reason the partition tests are readable: they drive the periods themselves,
//! against the in-memory transport, with the clock in the test's hands.

// Tests are allowed to panic loudly; the lint is about production paths.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod blobs;
pub mod detector;
pub mod members;
pub mod notify;
pub mod place;
pub mod resources;
pub mod turnaway;

pub use blobs::{Blobs, NoBlobs};
pub use detector::{Detector, DetectorConfig, DEAD_EVERY};
pub use members::{Members, NoMembers};
pub use notify::{Deliverer, NoDelivery};
pub use place::{Host, NoHost, Opinion, Placement, Refusal, Verdict};
pub use resources::{NoResources, ResourceChannel, Resources};

use offload_core::{
    BlobHash, Capabilities, ClusterView, Millis, NodeId, NodeStatus, NodeView, RunProgress,
    WorkPolicy,
};
use offload_proto::cluster::{ClusterMessage, Gossip};
use offload_transport::{Session, Stream, Transport, TransportError};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Mutex as AsyncMutex;

/// Wall-clock access, injected so the detector's inputs can be driven in a test.
///
/// `offload-core` never reads a clock and this crate has to, so the seam is here — the same
/// place the I/O is.
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> Millis;
}

/// The real one.
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Millis {
        Millis(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        )
    }
}

/// What a peer did with a blob it was offered.
///
/// `bool` threw the sentence away, and the two declines it flattened together want opposite
/// responses: *already held* is the common, good one — the peer is a replica already — while
/// *over the limit* is permanent and is the one an operator has to act on. The caller was
/// re-asking the peer (`peer_has_blob`) to tell them apart, which is a second round trip to
/// recover something the first answer had already said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pushed {
    /// The peer confirmed it holds the bytes.
    Stored,
    /// Larger than [`offload_proto::cluster::MAX_BLOB_BYTES`], so no peer was asked at all.
    /// Every node enforces the same cap, which makes this a fact about the blob rather than
    /// about the peer — and therefore one worth deciding before dialling.
    TooLarge { size: u64, limit: u64 },
    /// The peer said no, in these words.
    Declined { reason: String },
}

impl Pushed {
    /// Whether waiting could change this answer.
    ///
    /// Only [`Pushed::TooLarge`] is permanent, and it is decided from the bytes in hand rather
    /// than by reading the peer's sentence: every node in the fleet enforces the same cap, so a
    /// checkpoint carrying an over-sized blob is never going to be durable anywhere and a retry
    /// against a different peer is a retry that cannot succeed.
    #[must_use]
    pub fn is_permanent(&self) -> bool {
        matches!(self, Pushed::TooLarge { .. })
    }

    /// The refusal in the words the caller should report.
    #[must_use]
    pub fn why(&self) -> String {
        match self {
            Pushed::Stored => String::new(),
            Pushed::TooLarge { size, limit } => format!(
                "the blob is {size} bytes and the protocol moves at most {limit} in one \
                 exchange, so no node in this fleet can take a copy"
            ),
            Pushed::Declined { reason } => reason.clone(),
        }
    }
}

/// Probe timeouts in a row before a peer's connection is hung up and re-dialled. One was the
/// old rule, and on a phone over LTE it hung up a healthy connection twice a minute.
pub const TIMEOUTS_BEFORE_REDIAL: u32 = 3;

/// Probe timeouts in a row, per peer, since each last answered.
#[derive(Debug, Default)]
struct TimeoutStreaks(HashMap<NodeId, u32>);

impl TimeoutStreaks {
    /// A probe to `peer` got no answer in time. `true` when that makes
    /// [`TIMEOUTS_BEFORE_REDIAL`] in a row, and the streak starts again.
    fn timed_out(&mut self, peer: NodeId) -> bool {
        let count = self.0.entry(peer).or_default();
        *count += 1;
        if *count >= TIMEOUTS_BEFORE_REDIAL {
            self.0.remove(&peer);
            true
        } else {
            false
        }
    }

    /// `peer` answered, so any streak is over.
    fn answered(&mut self, peer: NodeId) {
        self.0.remove(&peer);
    }
}

/// Why a peer did not carry a notification, which decides whether to try again (ADR-0010).
///
/// Two different facts. `Refused` is the peer's answer: it tried its route and the route failed,
/// which is a delivery attempt and counts towards giving up. `Unanswered` is no answer at all: no
/// address, a timeout, a closed connection. That is a device that is away, and news for a device
/// that is away waits for it (the rule in `delivery-and-notifications`). Folded into one string,
/// a phone whose daemon had stopped had its news abandoned in ten seconds by a node that had just
/// restarted and still held it as alive (session ninety-four).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliverError {
    Unanswered(String),
    Refused(String),
}

impl std::fmt::Display for DeliverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeliverError::Unanswered(why) | DeliverError::Refused(why) => f.write_str(why),
        }
    }
}

pub struct Cluster {
    transport: Arc<dyn Transport>,
    clock: Arc<dyn Clock>,
    view: Mutex<ClusterView>,
    detector: Mutex<Detector>,
    /// One connection per peer, reused. A probe is a stream, not a connection: the handshake
    /// is not free and re-running it every second would make the failure detector the most
    /// expensive thing in the fleet.
    connections: AsyncMutex<HashMap<NodeId, Arc<Session>>>,
    /// Probe timeouts in a row, per peer, since its last answer. A slow answer is not a broken
    /// connection: see [`TIMEOUTS_BEFORE_REDIAL`].
    timeouts: Mutex<TimeoutStreaks>,
    /// Sessions a **peer** dialled, which [`Self::connections`] cannot hold.
    ///
    /// That map is keyed by peer and holds the one connection *this* node opened; an inbound
    /// session lives in the task serving it and used to be in no map at all. So there was
    /// nothing for [`Self::disconnect`] to reach — and since a live session never handshakes
    /// again, a revoked device that went on dialling in kept a channel nothing closed. Measured
    /// on two daemons: `offload revoke` on a node holding a run, and the revoked node went on
    /// gossiping, renewing its lease and pushing checkpoints for the rest of the run, which it
    /// finished. Every message on that channel is contact, so the arbiter's own probe (correctly
    /// refused) and the peer's inbound traffic alternated once a second — `no answer: suspecting`
    /// then `answered: no longer suspect`, forty-eight times — and the run was never orphaned.
    ///
    /// Keyed by a token rather than by peer: one peer can hold more than one inbound session at
    /// a time, and a map keyed by node would drop the older one on the floor still open.
    inbound: AsyncMutex<HashMap<u64, (NodeId, Arc<Session>)>>,
    /// Sessions this node dialled, handed to [`Self::serve`] so the far end can open streams on
    /// them too. One QUIC connection carries streams both ways, and for a peer that can only dial
    /// out — a phone behind a carrier firewall — the connection it opened is the only way anybody
    /// reaches it; see [`Self::session`].
    dialled: tokio::sync::mpsc::UnboundedSender<Arc<Session>>,
    dialled_rx: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Arc<Session>>>>,
    seq: Mutex<u64>,
    /// The schedules this node will gossip: its own and every one it has learned
    /// (ADR-0019 §3, ADR-0056).
    ///
    /// A copy for the wire, deliberately not a second source of truth. The daemon's store is the
    /// durable one, and the same tick that republishes runs re-syncs this from it
    /// ([`Self::publish_schedules`]) — so anything wrong here is corrected within a tick, which
    /// is the arrangement `publish_runs` already documents.
    ///
    /// Not in [`ClusterView`], because nothing that reads the view has any business with a
    /// schedule: placement, bidding and arbitration are about runs, and an occurrence is an
    /// ordinary run by the time any of them sees it.
    schedules: Mutex<std::collections::BTreeMap<offload_core::ScheduleId, offload_core::Schedule>>,
    /// How many times the fleet has been asked to read its agents' model lists again (ADR-0080),
    /// merged by maximum. A `watch` so the node's model reader wakes when it rises, rather than
    /// at its next tick; the value lives in the sender whether or not anybody is subscribed.
    models_asked: tokio::sync::watch::Sender<u64>,
    /// Whether any gossip has been absorbed yet. The count in the first exchange is adopted
    /// without waking anybody: it describes requests made before this node started, which its
    /// startup read already answers (ADR-0080).
    heard_any: std::sync::atomic::AtomicBool,
    /// Exchanges in which this node's gossip reached a peer: a probe answered, or a ping this
    /// node answered with its own gossip. What tells the daemon a finished run has been told.
    exchanges: std::sync::atomic::AtomicU64,
    blobs: Arc<dyn Blobs>,
    /// What this node says when asked to take a run. Set after construction because the
    /// supervisor that answers needs the mesh that asks (`OnceLock`, not a lock: it is
    /// written once at startup and read on every round).
    host: std::sync::OnceLock<Arc<dyn Host>>,
    /// What this node does when a peer asks it to tell somebody something (ADR-0010).
    ///
    /// Separate from `host` because the two planes are separate: the whole point of the split is
    /// that a phone answers `NoHost` to every run and still carries every notification.
    deliverer: std::sync::OnceLock<Arc<dyn Deliverer>>,
    /// Who this node knows has been thrown out of the fleet (ADR-0012). Unset on a node with no
    /// fleet, which is not a degraded mode: there is nobody to be thrown out of.
    members: std::sync::OnceLock<Arc<dyn Members>>,
    /// What each peer's handshake proved about it. Kept because a node's grants are on its
    /// certificate and its gossip is only what it says about itself.
    certificates: Mutex<HashMap<NodeId, offload_core::MembershipCert>>,
    /// Peers that refused this node's certificate at a dial, and what they said (ADR-0060).
    ///
    /// Counted, never acted on. `offload rekey` evicts a device by re-founding the fleet
    /// without it, so the only message the evicted device ever gets is this refusal — and a
    /// refusal is a peer's claim about *this* node, which nothing here may believe. What is
    /// reportable is the half that is ours: we dialled, we reached something, and we were
    /// turned away, which is a different fact from `no answer` and was previously indis-
    /// tinguishable from it in every report.
    turnaways: turnaway::Turnaways,
    /// What this node does when a peer's run asks to use one of its resources (ADR-0011).
    resources: std::sync::OnceLock<Arc<dyn Resources>>,
    /// One placement round per run, on this node (ADR-0050).
    ///
    /// An epoch is monotonic *per arbiter*, and an arbiter is a node — so two rounds running at
    /// once **on one node** for one run both read the same number and both hand out the same
    /// number, which is the one thing the token exists to make impossible. Nothing above this
    /// was arranging them: `Mesh::drain` runs a round of its own while `Mesh::supervise`'s tick
    /// runs one for the same released run, and they are the same node.
    ///
    /// Keyed by run, because that is the scope of the invariant: two rounds for two runs are
    /// exactly what a fleet is for. An entry lives only as long as somebody holds it — created
    /// on the way in and dropped by the last round out — so this is a rendezvous, not a
    /// registry, and it does not grow with the number of runs the node has ever seen.
    rounds: Mutex<HashMap<offload_core::RunId, Arc<Round>>>,
}

/// A placement round for one run, and the answer it left for anybody who waited on it.
///
/// The answer matters as much as the exclusion. A round that waits and then runs anyway is a
/// second round asking the same fleet the same question about a run the first round has already
/// moved past — so the second one stands down and reports what the first one decided. See
/// [`Cluster::place`].
type Round = AsyncMutex<Option<place::Placement>>;

impl std::fmt::Debug for Cluster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cluster")
            .field("local", &self.transport.node().short())
            .finish()
    }
}

/// What a peer's gossip taught us that is worth writing down.
///
/// Two lists rather than one, because the record and the numbers travel independently: a run
/// can move without doing any more work, and it does more work every turn without moving.
#[derive(Debug, Default)]
struct Learned {
    runs: Vec<offload_core::Run>,
    /// Runs *this node holds*, carrying an operator's edit to the half of the spec it does not
    /// own. Separate from `runs` because what may be written down differs: a record for a run
    /// elsewhere is written whole, and for one held here only the edit may be taken — the rest
    /// of the record here is a gossip tick old, and the store's copy is not.
    edits: Vec<offload_core::Run>,
    progress: Vec<(offload_core::RunId, RunProgress)>,
    /// Schedules whose stored copy this gossip *changed* — a new one, or a tombstone for one
    /// already known. Not every schedule in the message: a node that wrote down what it already
    /// knew on every probe would rewrite a row a second for the life of the fleet.
    schedules: Vec<offload_core::Schedule>,
    /// Runs a peer sent as live that this node knows finished: the peer is behind.
    stale: Vec<offload_core::RunId>,
}

impl Cluster {
    /// `local` is this node's own entry: its capabilities and policy, which it owns and
    /// gossips (ADR-0005).
    ///
    /// Its incarnation starts at 1, never 0, because 0 is what a *placeholder* has — see
    /// [`Self::introduce`]. A node's own report has to outrank the guess somebody made about
    /// it before they had met, and one number is the cheapest way to say so.
    #[must_use]
    pub fn new(
        transport: Arc<dyn Transport>,
        local: NodeView,
        config: DetectorConfig,
        clock: Arc<dyn Clock>,
        blobs: Arc<dyn Blobs>,
    ) -> Arc<Cluster> {
        let mut local = local;
        local.incarnation = local.incarnation.max(1);
        let mut view = ClusterView::new(local.id);
        view.upsert_node(local);
        let (dialled, dialled_rx) = tokio::sync::mpsc::unbounded_channel();
        Arc::new(Cluster {
            transport,
            clock,
            view: Mutex::new(view),
            detector: Mutex::new(Detector::new(config)),
            connections: AsyncMutex::new(HashMap::new()),
            timeouts: Mutex::new(TimeoutStreaks::default()),
            inbound: AsyncMutex::new(HashMap::new()),
            dialled,
            dialled_rx: Mutex::new(Some(dialled_rx)),
            seq: Mutex::new(0),
            schedules: Mutex::new(std::collections::BTreeMap::new()),
            models_asked: tokio::sync::watch::channel(0).0,
            heard_any: std::sync::atomic::AtomicBool::new(false),
            exchanges: std::sync::atomic::AtomicU64::new(0),
            blobs,
            host: std::sync::OnceLock::new(),
            deliverer: std::sync::OnceLock::new(),
            members: std::sync::OnceLock::new(),
            certificates: Mutex::new(HashMap::new()),
            turnaways: turnaway::Turnaways::default(),
            resources: std::sync::OnceLock::new(),
            rounds: Mutex::new(HashMap::new()),
        })
    }

    #[must_use]
    pub fn node(&self) -> NodeId {
        self.transport.node()
    }

    /// Say who answers "would you take this run".
    ///
    /// Without one, this node refuses every ask — which is correct for a member that holds no
    /// agent at all, and is what a phone with only a delivery capability should say.
    pub fn hosts_runs(&self, host: Arc<dyn Host>) {
        let _ = self.host.set(host);
    }

    /// Say who carries a notification for a peer (ADR-0010).
    ///
    /// Without one this node refuses every ask, which is the honest answer for a member with no
    /// route configured — and the reason it is a separate registration from `hosts_runs` is that
    /// the interesting device does exactly one of the two.
    pub fn delivers(&self, deliverer: Arc<dyn Deliverer>) {
        let _ = self.deliverer.set(deliverer);
    }

    /// Say who answers when a peer's run asks to use one of this node's resources (ADR-0011).
    ///
    /// Without one this node offers nothing, which is the ordinary answer: most devices nominate
    /// no resources at all.
    pub fn offers_resources(&self, resources: Arc<dyn Resources>) {
        let _ = self.resources.set(resources);
    }

    #[must_use]
    fn resources(&self) -> Arc<dyn Resources> {
        self.resources
            .get()
            .cloned()
            .unwrap_or_else(|| Arc::new(NoResources))
    }

    /// Say who holds this node's membership facts (ADR-0012).
    ///
    /// Without one this node gossips no revocations and remembers none it is told about, which
    /// is the honest answer for a daemon that belongs to no fleet.
    pub fn members_via(&self, members: Arc<dyn Members>) {
        let _ = self.members.set(members);
    }

    #[must_use]
    fn members(&self) -> Arc<dyn Members> {
        self.members
            .get()
            .cloned()
            .unwrap_or_else(|| Arc::new(NoMembers))
    }

    /// What this node does when asked to tell somebody something.
    #[must_use]
    pub fn deliverer(&self) -> Arc<dyn Deliverer> {
        self.deliverer
            .get()
            .cloned()
            .unwrap_or_else(|| Arc::new(NoDelivery))
    }

    /// Ask a peer to carry a notification through one of its routes.
    ///
    /// The reply is a sentence either way, and it goes straight into the sender's outbox: the
    /// node that logged the event is the one that remembers whether anybody was told, because it
    /// is the only node that knows what it has already sent.
    pub async fn deliver_via(
        &self,
        node: NodeId,
        sink: &str,
        note: &offload_core::Notification,
        window: offload_core::Millis,
    ) -> Result<(), DeliverError> {
        let ask = ClusterMessage::Deliver {
            sink: sink.to_string(),
            note: Box::new(note.clone()),
        };
        match self.request(node, &ask, window).await {
            Ok(ClusterMessage::Delivered { .. }) => Ok(()),
            Ok(ClusterMessage::Undelivered { reason, .. }) => Err(DeliverError::Refused(reason)),
            Ok(_) => Err(DeliverError::Refused(
                "answered a notification with something else".into(),
            )),
            Err(e) => Err(DeliverError::Unanswered(format!(
                "{} did not answer ({e})",
                self.name_of(node)
            ))),
        }
    }

    /// Open a stream to a peer that holds a resource one of our runs was granted (ADR-0011).
    ///
    /// Returns the stream itself rather than an answer, because what follows is a conversation
    /// and not a request: the caller pipes the agent's own protocol over it until the agent
    /// stops. The one thing settled before it comes back is that the holder agreed.
    pub async fn open_resource(
        &self,
        node: NodeId,
        run: offload_core::RunId,
        service: &offload_core::Service,
    ) -> Result<Stream, String> {
        let session = self
            .session(node)
            .await
            .map_err(|e| format!("{} is not reachable ({e})", self.name_of(node)))?;
        let mut stream = session
            .connection
            .open()
            .await
            .map_err(|e| format!("{} would not open a stream ({e})", self.name_of(node)))?;
        stream
            .send(&ClusterMessage::UseResource {
                run,
                service: service.clone(),
            })
            .await
            .map_err(|e| format!("{e}"))?;
        match stream.recv::<ClusterMessage>().await {
            Ok(ClusterMessage::ResourceReady { .. }) => Ok(stream),
            Ok(ClusterMessage::ResourceRefused { reason }) => Err(reason),
            Ok(_) => Err("answered a resource request with something else".into()),
            Err(e) => Err(format!("{} did not answer ({e})", self.name_of(node))),
        }
    }

    /// Ask a peer to re-issue this node's membership certificate (ADR-0012).
    ///
    /// Nothing is sent about *which* certificate: the answer is built from the one this
    /// connection's handshake authenticated, which is the same rule `handshake::admit` follows
    /// and the reason the message is empty.
    ///
    /// A refusal is the ordinary answer — most members are not approvers — so it comes back as
    /// a sentence for the caller to log at the right volume and try somebody else.
    pub async fn renew_with(
        &self,
        node: NodeId,
        window: offload_core::Millis,
        current: offload_proto::handshake::Credentials,
    ) -> Result<offload_proto::handshake::Credentials, String> {
        let ask = ClusterMessage::RenewMe {
            credentials: Some(Box::new(current)),
        };
        match self.request(node, &ask, window).await {
            Ok(ClusterMessage::Renewed { credentials }) => Ok(*credentials),
            Ok(ClusterMessage::NotRenewed { reason }) => Err(reason),
            Ok(_) => Err("answered a renewal with something else".into()),
            Err(e) => Err(format!("{} did not answer ({e})", self.name_of(node))),
        }
    }

    /// What this node would answer if asked to take a run. Public so the daemon can ask
    /// *itself* before asking anybody else — its own bid is one of the bids.
    #[must_use]
    pub fn host(&self) -> Arc<dyn Host> {
        self.host
            .get()
            .cloned()
            .unwrap_or_else(|| Arc::new(place::NoHost) as Arc<dyn Host>)
    }

    pub(crate) fn clock_now(&self) -> Millis {
        self.clock.now()
    }

    /// Send one message and wait for one answer. The placement round's only transport need.
    pub(crate) async fn request(
        &self,
        peer: NodeId,
        message: &ClusterMessage,
        window: Millis,
    ) -> Result<ClusterMessage, TransportError> {
        self.ask(peer, message, window).await
    }

    /// A snapshot, for `offload nodes` and for anything that wants to reason about the fleet
    /// without holding a lock.
    #[must_use]
    pub fn view(&self) -> ClusterView {
        self.locked_view().clone()
    }

    /// Seed peers with what this node had learned about their absences before it restarted, and
    /// hand back what it knows now so the caller can write it down (ADR-0031).
    ///
    /// One call rather than two because it is one pass over the same lock, and because the two
    /// halves are the same fact travelling in opposite directions. The seeding is idempotent —
    /// `NodeView::seed_absence_history` refuses to overwrite anything learned in this incarnation
    /// — which is what lets this be called on a tick with no bookkeeping about which peers have
    /// already been seeded.
    ///
    /// The local node is excluded from both halves: nobody observes their own absences, and
    /// `set_status` refuses to believe a peer about them.
    pub fn exchange_absence_history(
        &self,
        remembered: &std::collections::BTreeMap<NodeId, offload_core::NodeObservation>,
    ) -> Vec<(NodeId, offload_core::NodeObservation)> {
        let mut view = self.locked_view();
        let local = view.local;
        let mut out = Vec::new();
        for (id, node) in view.nodes.iter_mut() {
            if *id == local {
                continue;
            }
            if let Some(seed) = remembered.get(id) {
                node.seed_absence_history(seed.observed_absences, seed.typical_absence);
            }
            out.push((*id, node.observation()));
        }
        out
    }

    /// Is this node already in the view? Discovery asks before it dials, because meeting
    /// somebody you already know is a handshake nobody needed.
    #[must_use]
    pub fn knows(&self, peer: NodeId) -> bool {
        self.locked_view().nodes.contains_key(&peer)
    }

    /// What this node is running, gossiped so the fleet knows who is doing what and not only
    /// who exists.
    ///
    /// Bumps the incarnation when the set changes, because that is the version number peers
    /// merge on (ADR-0005) — a workload that changed without one would be ignored as stale by
    /// everybody. Unchanged means untouched: bumping on every poll would make every node's
    /// entry look new once a second and defeat the ordering it exists to provide.
    pub fn set_running(&self, running: std::collections::BTreeSet<offload_core::RunId>) -> bool {
        let now = self.clock.now();
        let local = self.node();
        let mut view = self.locked_view();
        let Some(me) = view.nodes.get_mut(&local) else {
            return false;
        };
        if me.running == running {
            return false;
        }
        me.running = running;
        me.incarnation += 1;
        me.last_heard = now;
        me.heard_here = Some(now);
        true
    }

    /// Re-report this node's capabilities, for when they have changed: a battery drains, an
    /// agent is upgraded, a laptop is plugged in. Same incarnation rule as [`Self::set_running`].
    pub fn set_capabilities(&self, capabilities: Capabilities, policy: WorkPolicy) -> bool {
        let now = self.clock.now();
        let local = self.node();
        let mut view = self.locked_view();
        let Some(me) = view.nodes.get_mut(&local) else {
            return false;
        };
        if me.capabilities == capabilities && me.policy == policy {
            return false;
        }
        me.capabilities = capabilities;
        me.policy = policy;
        me.incarnation += 1;
        me.last_heard = now;
        me.heard_here = Some(now);
        true
    }

    /// Re-state where this node can be dialled (ADR-0076), for when an interface comes or goes.
    /// Same incarnation rule as [`Self::set_capabilities`]: a change is a new version of our record.
    pub fn set_addresses(&self, addresses: Vec<String>) -> bool {
        let now = self.clock.now();
        let local = self.node();
        let mut view = self.locked_view();
        let Some(me) = view.nodes.get_mut(&local) else {
            return false;
        };
        if me.addresses == addresses {
            return false;
        }
        me.addresses = addresses;
        me.incarnation += 1;
        me.last_heard = now;
        me.heard_here = Some(now);
        true
    }

    /// Say whether this node asks to be left mostly alone (ADR-0078). Same incarnation rule as
    /// [`Self::set_capabilities`]. Returns whether it changed.
    pub fn set_quiet(&self, quiet: bool) -> bool {
        let now = self.clock.now();
        let local = self.node();
        let mut view = self.locked_view();
        let Some(me) = view.nodes.get_mut(&local) else {
            return false;
        };
        if me.quiet == quiet {
            return false;
        }
        me.quiet = quiet;
        me.incarnation += 1;
        me.last_heard = now;
        me.heard_here = Some(now);
        true
    }

    /// Teach this node about a peer it has not met, so the rotation has somewhere to start.
    /// Discovery — a seed list today, mDNS next — lands here.
    ///
    /// What goes in is a *placeholder*: an id, and whatever the discoverer could guess about
    /// the rest. It carries incarnation 0 so that the peer's own first gossip replaces it
    /// wholesale — capabilities nobody verified must never survive contact with the node that
    /// has them.
    pub fn introduce(&self, peer: NodeId, capabilities: Capabilities, policy: WorkPolicy) {
        let now = self.clock.now();
        if self.knows(peer) {
            // **A completed handshake is first-hand contact**, and it has to outrank what we
            // remember — which is the whole of a bug measured on two daemons: an *impolite* death
            // heals and a *polite* departure was permanent. A node that announces it is going is
            // `Draining`, `probeable` excludes exactly `Draining | Departed` ("minus those who
            // told us they were going" — right while they are going), and a `Dead` node is
            // re-probed for ever because that is how one comes back. So the good behaviour was
            // the unrecoverable one: nothing ever revised a departure.
            //
            // And a placeholder cannot revise it either, which is why this is not a `merge_node`:
            // `NodeView::new` carries incarnation 0, so `merge_node` copies nothing and — worse —
            // leaves `last_heard` untouched, while a restarted node's own gossip is rejected for
            // being *behind* the incarnation its previous life had reached. Measured: a peer
            // re-met **40 times** over four minutes, still shown as `draining`, last heard `7m`
            // ago, and never probed or gossiped to again, so the node it had just handshaked with
            // never learned it existed at all.
            //
            // `alive` is the right channel because it is the *observation* path — the same one
            // `serve_stream` uses for "anything a peer sends is contact: it is alive, whatever we
            // believed a moment ago" — and an observation is not a peer's claim about itself.
            self.alive(peer, now);
            return;
        }
        let placeholder = NodeView::new(peer, capabilities, policy, now);
        self.locked_view().merge_node(placeholder, now);
    }

    /// One SWIM period: probe somebody, escalate if they do not answer, and turn old
    /// suspicions into conclusions.
    pub async fn probe_round(&self, now: Millis) {
        // We are demonstrably here. Without this our own entry ages like a peer's and
        // `offload nodes` reports the local node as last heard from minutes ago.
        self.alive(self.node(), now);

        for gone in self.expired_suspicions(now) {
            // Said once. A node that is already dead is probed again every period — which is
            // deliberate, and is how it comes back — so it is re-suspected and re-concluded for
            // as long as it stays away. Repeating the conclusion at probe rate costs nothing but
            // buries every other line in the log, which in a demo is where the real events are.
            if self.status_of(gone) != Some(NodeStatus::Dead) {
                tracing::info!(node = %gone.short(), "no answer from anybody: marking dead");
            }
            self.declare(gone, NodeStatus::Dead, now);
        }

        let Some(target) = self.pick_target(now) else {
            return;
        };

        if self.probe(target, now).await {
            return;
        }

        // A direct probe failing says as much about the path as about the peer. Ask somebody
        // whose path is different before concluding anything — unless it is concluded already.
        // Helpers exist to stop a false death; for a node that is dead they only made every
        // helper dial its stale address, a phone over LTE among them (session ninety-two).
        // Nor for a quiet one (ADR-0078): three helpers probing it are three more wakes of a phone
        // asleep in a pocket, and its longer suspicion is what covers a missed answer.
        let quiet = self.view().nodes.get(&target).is_some_and(|n| n.quiet);
        if quiet || self.status_of(target) == Some(NodeStatus::Dead) {
            if let Some(status) = self.suspect(target, now) {
                self.declare(target, status, now);
            }
            return;
        }
        let helpers = self.pick_helpers(target);
        if self.probe_indirect(target, &helpers, now).await {
            tracing::debug!(
                node = %target.short(),
                "unreachable from here, reachable from a peer"
            );
            return;
        }

        if let Some(status) = self.suspect(target, now) {
            // Debug, not info: a suspicion is a question, and on a phone asleep on wifi it is asked
            // and answered every few seconds for ever (ping peaks at 562 ms against a 500 ms probe
            // timeout, measured on a Samsung phone — 300 pairs of lines in twenty minutes, none
            // ending in a death). The conclusion (`marking dead`) and a return from it stay info.
            tracing::debug!(node = %target.short(), "no answer: suspecting");
            self.declare(target, status, now);
        }
    }

    /// Run [`Self::probe_round`] on the detector's interval until `shutdown` resolves.
    pub async fn run(self: Arc<Self>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        loop {
            // A quiet node (ADR-0078) probes about once a minute: each probe wakes it as surely as
            // being probed does.
            let quiet = self
                .view()
                .nodes
                .get(&self.node())
                .is_some_and(|me| me.quiet);
            let interval = if quiet {
                Duration::from_millis(detector::QUIET_PROBE_EVERY.0)
            } else {
                Duration::from_millis(self.config().probe_interval.0)
            };
            tokio::select! {
                () = tokio::time::sleep(interval) => {
                    let now = self.clock.now();
                    self.probe_round(now).await;
                }
                _ = shutdown.changed() => return,
            }
        }
    }

    /// Answer whatever peers send us, for as long as the transport accepts connections.
    pub async fn serve(self: Arc<Self>) {
        // What this node dialled is served too: the peer that dialled nobody but us can then be
        // reached over its own connection, which for a node behind a firewall is the only one.
        let taken = self
            .dialled_rx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(mut dialled) = taken {
            let cluster = self.clone();
            tokio::spawn(async move {
                while let Some(session) = dialled.recv().await {
                    let cluster = cluster.clone();
                    tokio::spawn(async move { cluster.serve_shared(session, false).await });
                }
            });
        }
        loop {
            match self.transport.accept().await {
                Ok(session) => {
                    let cluster = self.clone();
                    tokio::spawn(async move { cluster.serve_session(session).await });
                }
                Err(TransportError::Closed { .. }) => return,
                Err(e) => {
                    // One peer failing to get in is not a reason to stop listening — that is
                    // precisely what an attacker would want it to be.
                    tracing::debug!(error = %e, "inbound connection refused");
                }
            }
        }
    }

    /// Tell the fleet we are going, so peers mark us `Draining` instead of detecting us.
    ///
    /// A node knows when it is leaving (ADR-0005), and saying so turns a shutdown from a
    /// ten-second detection into an immediate, correct fact.
    pub async fn announce_departure(&self) {
        let peers: Vec<NodeId> = self
            .locked_view()
            .nodes
            .values()
            .filter(|n| n.id != self.node() && n.status.is_available())
            .map(|n| n.id)
            .collect();

        let timeout = self.config().probe_timeout;
        for peer in peers {
            let message = ClusterMessage::Leaving {
                seq: self.next_seq(),
                gossip: self.gossip(),
            };
            // Waited for, briefly: the value of announcing is that nobody spends a detection
            // timeout on us, and an announcement that arrives after the process exits buys
            // nothing. Not retried — a peer that misses it detects us the ordinary way.
            if let Err(e) = self.ask(peer, &message, timeout).await {
                tracing::debug!(node = %peer.short(), error = %e, "could not announce departure");
            }
        }
    }

    // -- the halves of a period, each small enough to read -------------------------------

    async fn probe(&self, target: NodeId, now: Millis) -> bool {
        let seq = self.next_seq();
        let message = ClusterMessage::Ping {
            seq,
            gossip: self.gossip(),
        };
        match self.ask_probe(target, &message, Millis(0)).await {
            Some(Ok(ClusterMessage::Ack { seq: acked, gossip })) if acked == seq => {
                // Our ping carried our gossip, and the answer proves it arrived.
                self.exchanges
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.learn(&gossip, target, now).await;
                self.alive(target, now);
                true
            }
            Some(Ok(_)) => false,
            Some(Err(e)) => {
                tracing::debug!(node = %target.short(), error = %e, "probe failed");
                self.drop_connection(target).await;
                false
            }
            None => false,
        }
    }

    /// A probe's exchange, with its timeout kept apart from a transport error (`None`).
    ///
    /// **A slow answer is not a broken connection.** Every failed probe used to hang up, and a
    /// phone on LTE, whose acks peak past the 500 ms timeout, hung up the one connection it had
    /// dialled to the laptop, which was also the laptop's only way back to it. It was suspected,
    /// re-dialled and refuted, twice a minute (session ninety-two). A connection that has died
    /// silently, after a network switch, still has to be re-dialled, so
    /// [`TIMEOUTS_BEFORE_REDIAL`] in a row do hang up; a real transport error hangs up at once.
    ///
    /// `on_behalf` is the time the answer also has to cover: for a `PingReq`, the helper's own
    /// probe of the target, which on a slow target outlasted the whole of the old timeout.
    async fn ask_probe(
        &self,
        peer: NodeId,
        message: &ClusterMessage,
        on_behalf: Millis,
    ) -> Option<Result<ClusterMessage, TransportError>> {
        let timeout = self.locked_detector().timeout_for(peer) + on_behalf;
        let exchange = async {
            let session = self.session(peer).await?;
            let mut stream = session.connection.open().await?;
            stream.send(message).await?;
            stream.recv().await
        };
        let started = std::time::Instant::now();
        let answer = tokio::time::timeout(Duration::from_millis(timeout.0), exchange).await;
        let mut timeouts = self
            .timeouts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match answer {
            Ok(result) => {
                if result.is_ok() {
                    timeouts.answered(peer);
                    let rtt = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                    tracing::trace!(node = %peer.short(), rtt_ms = rtt, timeout_ms = timeout.0, "probe answered");
                    drop(timeouts);
                    self.locked_detector().answered_in(peer, Millis(rtt));
                }
                Some(result)
            }
            Err(_) => {
                // Patience is for a live peer on a slow link. One already concluded dead that is
                // asked again gets a fresh connection at the first silence: after a peer restarts,
                // the old connection answers nothing, and waiting out three rare probes on it kept
                // the Mac and the restarted laptop apart for three minutes (session ninety-two).
                let concluded = self.status_of(peer) == Some(NodeStatus::Dead);
                let redial = timeouts.timed_out(peer) || concluded;
                self.locked_detector().no_answer_from(peer);
                tracing::debug!(node = %peer.short(), redial, timeout_ms = timeout.0, "no answer in time");
                if redial {
                    Some(Err(TransportError::Closed {
                        peer: peer.short(),
                        reason: format!(
                            "no answer within {}ms, {TIMEOUTS_BEFORE_REDIAL} times in a row",
                            timeout.0
                        ),
                    }))
                } else {
                    None
                }
            }
        }
    }

    async fn probe_indirect(&self, target: NodeId, helpers: &[NodeId], now: Millis) -> bool {
        for helper in helpers {
            let seq = self.next_seq();
            let message = ClusterMessage::PingReq {
                seq,
                target,
                gossip: self.gossip(),
            };
            let target_wait = self.locked_detector().timeout_for(target);
            match self.ask_probe(*helper, &message, target_wait).await {
                Some(Ok(ClusterMessage::Ack { seq: acked, gossip })) if acked == seq => {
                    self.learn(&gossip, target, now).await;
                    self.alive(target, now);
                    return true;
                }
                Some(Ok(_)) | None => {}
                Some(Err(e)) => {
                    tracing::debug!(node = %helper.short(), error = %e, "indirect probe failed");
                    self.drop_connection(*helper).await;
                }
            }
        }
        false
    }

    async fn serve_session(self: Arc<Self>, session: Session) {
        self.serve_shared(Arc::new(session), true).await;
    }

    /// Serve every stream a peer opens on one session, whichever end dialled it.
    ///
    /// Only an **inbound** session is registered in [`Self::inbound`]. A dialled one already lives
    /// in `connections`, where `drop_connection` and `disconnect` find it, and registering it
    /// twice let [`Self::session`] hand back a session `drop_connection` had just closed —
    /// measured by the two tests that refuse a bid, drop the connection and expect the next round
    /// to re-handshake and read the new certificate.
    async fn serve_shared(self: Arc<Self>, session: Arc<Session>, inbound: bool) {
        let peer = session.peer.node;
        // The name on the certificate, which is signed — unlike the one in gossip, which is
        // whatever the node says it is called (ADR-0012).
        self.record_name(peer, &session.peer.name);
        self.record_certificate(&session.peer.membership);
        // What the handshake *proved* about this peer, kept for the whole session so that a
        // message answered from it is answered against a certificate rather than a claim.
        let who = Arc::new(session.peer.clone());
        // Registered *before* the first stream is served, so there is no window in which this
        // node is answering a peer it cannot hang up on.
        let token = self.next_seq();
        if inbound {
            self.inbound
                .lock()
                .await
                .insert(token, (peer, session.clone()));
        }
        tracing::debug!(node = %peer.short(), "serving a session; waiting for streams");
        loop {
            let mut stream = match session.connection.accept().await {
                Ok(stream) => stream,
                Err(e) => {
                    // Logged, because `break` here was silent and a session that ended is
                    // indistinguishable from one still waiting when nothing says which.
                    tracing::debug!(node = %peer.short(), error = %e, "session ended");
                    break;
                }
            };
            tracing::debug!(node = %peer.short(), "stream accepted");
            let cluster = self.clone();
            let who = who.clone();
            tokio::spawn(async move {
                if let Err(e) = cluster.serve_stream(&who, &mut stream).await {
                    tracing::debug!(node = %peer.short(), error = %e, "stream ended");
                }
            });
        }
        // Every way out of the loop, including the one [`Self::disconnect`] causes: the entry
        // is this task's to remove, so a session that ended on its own leaves nothing behind.
        if inbound {
            self.inbound.lock().await.remove(&token);
        }
    }

    async fn serve_stream(
        &self,
        who: &offload_proto::handshake::Peer,
        stream: &mut Stream,
    ) -> Result<(), TransportError> {
        let peer = who.node;
        let message: ClusterMessage = stream.recv().await?;
        // A catch-all rather than an exhaustive match on purpose: this is a log line, not a
        // decision, and the question it exists to answer is whether a request decoded at all.
        let kind = match &message {
            ClusterMessage::Ping { .. } => "ping",
            ClusterMessage::PingReq { .. } => "ping-req",
            ClusterMessage::Leaving { .. } => "leaving",
            _ => "other",
        };
        tracing::debug!(node = %peer.short(), kind, "request decoded");
        let now = self.clock.now();

        if let Some(gossip) = message.gossip() {
            self.learn(gossip, peer, now).await;
        }
        // Anything a peer sends is contact: it is alive, whatever we believed a moment ago.
        self.alive(peer, now);

        match message {
            ClusterMessage::Ping { seq, .. } => {
                stream
                    .send(&ClusterMessage::Ack {
                        seq,
                        gossip: self.gossip(),
                    })
                    .await?;
                self.exchanges
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            ClusterMessage::PingReq { seq, target, .. } => {
                // Somebody cannot reach `target` and we might be able to. Their sequence
                // number comes back either way, so they can match the answer to the question.
                let reachable = self.probe(target, now).await;
                let reply = if reachable {
                    ClusterMessage::Ack {
                        seq,
                        gossip: self.gossip(),
                    }
                } else {
                    ClusterMessage::Nack { seq, target }
                };
                stream.send(&reply).await?;
            }
            ClusterMessage::Leaving { seq, .. } => {
                tracing::info!(node = %peer.short(), "peer is leaving");
                self.declare(peer, NodeStatus::Draining, now);
                self.forget(peer);
                stream
                    .send(&ClusterMessage::Ack {
                        seq,
                        gossip: self.gossip(),
                    })
                    .await?;
            }
            ClusterMessage::WillYouTake { run } => {
                let id = run.id;
                let reply = match self.host().evaluate(&run).await {
                    Ok(offer) => ClusterMessage::Bid {
                        run: id,
                        score: offer.score.0,
                        available: offer.available,
                        terms: offer.terms,
                    },
                    Err(reason) => ClusterMessage::WillNot { run: id, reason },
                };
                stream.send(&reply).await?;
            }
            ClusterMessage::FetchEvents { run, after, limit } => {
                // Answered from the store, so a run that finished an hour ago is as followable
                // as one mid-turn — and so a restarted daemon still has the log its peers are
                // asking about.
                let reply = match self.host().events_since(peer, run, after, limit).await {
                    Ok((events, done)) => ClusterMessage::Events { run, events, done },
                    Err(reason) => ClusterMessage::NoEvents { run, reason },
                };
                stream.send(&reply).await?;
            }
            ClusterMessage::FetchFiles { run, path } => {
                // Read where the checkout is, and only there: a directory is on one machine.
                let reply = match self.host().files(peer, run, &path).await {
                    Ok(view) => ClusterMessage::Files { run, view },
                    Err(reason) => ClusterMessage::NoFiles { run, reason },
                };
                stream.send(&reply).await?;
            }
            ClusterMessage::Deliver { sink, note } => {
                // The peer chose this route from what this node advertises, so what is decided
                // here is only whether it still works — the credential behind it never left.
                let reply = match self.deliverer().deliver(peer, &sink, &note).await {
                    Ok(()) => ClusterMessage::Delivered { sink },
                    Err(reason) => ClusterMessage::Undelivered { sink, reason },
                };
                stream.send(&reply).await?;
            }
            ClusterMessage::EditSpec { run, edit } => {
                // Applied here because this node owns the field, not because it holds the run
                // (ADR-0013). Whether it really is the owner was decided by the node that
                // forwarded it, from the same `arbiter_for` this one would compute — and if
                // that has just changed under us, the worst case is a revision written by a
                // node the fleet no longer asks, which the counter absorbs.
                let reply = match self.host().edit_spec(run, edit).await {
                    Ok((rev, note)) => ClusterMessage::SpecEdited { run, rev, note },
                    Err(reason) => ClusterMessage::EditRefused { run, reason },
                };
                stream.send(&reply).await?;
            }
            ClusterMessage::Cancel { run, by } => {
                // Applied here because the run is here. No epoch check, and unlike `Answer` that
                // is a decision rather than fencing by construction: an operator's cancel is
                // unfenced everywhere in this design, and the terminal state it writes beats
                // anything at the same epoch. What the implementation checks is whether there is
                // anything left to stop — a run that finished while the command was in flight is
                // a race, not a fault, and it says so.
                let reply = match self.host().cancel(run, &by).await {
                    Ok(note) => ClusterMessage::Cancelled { run, note },
                    Err(reason) => ClusterMessage::CancelRefused { run, reason },
                };
                stream.send(&reply).await?;
            }
            ClusterMessage::ContinueBase { run, session } => {
                // Built here because the parent's checkout is here. Read, never written: the
                // capture touches only the index, so a person reading that worktree is not
                // disturbed. Who may ask was settled by the handshake; whether the asker may
                // *submit* is its own door, asked before it forwarded.
                let reply = match self.host().continuation_base(run, session).await {
                    Ok(base) => ClusterMessage::ContinueBaseBuilt {
                        run,
                        base: Box::new(base),
                    },
                    Err(reason) => ClusterMessage::ContinueBaseRefused { run, reason },
                };
                stream.send(&reply).await?;
            }
            ClusterMessage::Checkpoint { run } => {
                // Applied here because the agent is here. Nothing is captured yet — what this
                // sets is a request the pump honours at its next turn boundary, which is the
                // only safe capture point (ADR-0004).
                let reply = match self.host().request_checkpoint(run).await {
                    Ok(()) => ClusterMessage::CheckpointRequested { run },
                    Err(reason) => ClusterMessage::CheckpointRefused { run, reason },
                };
                stream.send(&reply).await?;
            }
            ClusterMessage::Answer {
                run,
                tool_use_id,
                allow,
                by,
            } => {
                // Applied here because the blocked process is here. No epoch check: the identity
                // of a question is the agent's own `tool_use_id`, which exists only while that
                // agent is blocked on that call — so an answer for a leg that has ended matches
                // nothing and is refused by having nowhere to go.
                let reply = match self.host().answer(run, tool_use_id, allow, &by).await {
                    Ok((tool, detail, allowed)) => ClusterMessage::AnswerTaken {
                        run,
                        tool,
                        detail,
                        allowed,
                    },
                    Err(reason) => ClusterMessage::AnswerRefused { run, reason },
                };
                stream.send(&reply).await?;
            }
            ClusterMessage::UseResource { run, service } => {
                // `peer` is what the handshake authenticated. The run id and the service are
                // the asker's claim, and the implementation checks them against what it knows
                // about that run — which is the only place that check can be made, because the
                // holder is the only node that knows what it nominated.
                let mut channel = match self.resources().open(peer, run, &service).await {
                    Ok(channel) => {
                        stream
                            .send(&ClusterMessage::ResourceReady {
                                id: service.to_string(),
                            })
                            .await?;
                        channel
                    }
                    Err(reason) => {
                        tracing::info!(
                            node = %peer.short(),
                            %service,
                            %reason,
                            "refused a peer's run the use of a resource"
                        );
                        stream
                            .send(&ClusterMessage::ResourceRefused { reason })
                            .await?;
                        return Ok(());
                    }
                };
                tracing::info!(
                    node = %peer.short(),
                    run_id = %run,
                    %service,
                    "carrying a peer's run's calls to a resource"
                );
                let ended = pump_resource(stream, channel.as_mut()).await;
                channel.close().await;
                tracing::info!(node = %peer.short(), %service, reason = %ended, "resource closed");
            }
            ClusterMessage::RenewMe { credentials } => {
                // The asker's current papers, taken only for the asker itself (ADR-0069 §2): the
                // handshake's certificate is as old as the connection, which is older than the
                // renewal the asker already holds. `Members::renew` verifies whatever it is given
                // before restating it, so a carried certificate is checked exactly as hard as the
                // handshake's; one for anybody else falls back to what the handshake proved.
                let carried = credentials.filter(|c| c.membership.member == peer);
                let (cert, delegation) = match &carried {
                    Some(c) => (&c.membership, c.delegation.as_ref()),
                    None => (&who.membership, who.delegation.as_ref()),
                };
                // Kept as the peer's current papers once they pass what a handshake checks, so
                // what this node reports about its peers (`offload status`, the re-approval list)
                // is not the certificate the connection opened with — measured: an approver whose
                // list left out the one member in its approval's last month, because that
                // member's handshake certificate had lapsed two renewals earlier.
                if let Some(c) = &carried {
                    if self
                        .members()
                        .admits(&c.membership, c.delegation.as_ref(), now)
                    {
                        self.record_newer_certificate(&c.membership);
                    }
                }
                let reply = match self.members().renew(cert, delegation, now) {
                    Ok(credentials) => {
                        // …and what this node just issued it, which is newer still.
                        self.record_newer_certificate(&credentials.membership);
                        // Stamped with this moment's approval: a person's decision here was acted
                        // on (ADR-0069 §3), which is the line somebody will look for.
                        let issued = &credentials.membership;
                        tracing::info!(
                            node = %peer.short(),
                            reapproved = issued.approved_at() == issued.issued_at,
                            "re-issued a membership certificate"
                        );
                        ClusterMessage::Renewed {
                            credentials: Box::new(credentials),
                        }
                    }
                    Err(reason) => ClusterMessage::NotRenewed { reason },
                };
                stream.send(&reply).await?;
            }
            ClusterMessage::FetchAsks => {
                let asks = self.host().pending_asks().await;
                stream.send(&ClusterMessage::Asks { asks }).await?;
            }
            ClusterMessage::Grant { run } => {
                let id = run.id;
                // Answered only after the run is actually taken: an arbiter that hears
                // "granted" has stopped looking for anywhere else to put it.
                let reply = match self.host().accept(*run).await {
                    Ok(()) => ClusterMessage::Granted { run: id },
                    Err(reason) => ClusterMessage::Declined { run: id, reason },
                };
                stream.send(&reply).await?;
            }
            ClusterMessage::BlobHave { hash } => {
                let held = self.blobs.has(hash).await;
                stream
                    .send(&ClusterMessage::BlobHeld { hash, held })
                    .await?;
            }
            ClusterMessage::BlobRequest { hash } => {
                match self.blobs.get(hash).await {
                    Some(bytes) => {
                        // Header first, then the payload raw. The size is what lets the far
                        // end read exactly this blob and not whatever follows it.
                        stream
                            .send(&ClusterMessage::BlobFound {
                                size: bytes.len() as u64,
                            })
                            .await?;
                        stream.send_bytes(&bytes).await?;
                        tracing::debug!(
                            node = %peer.short(),
                            blob = %hash.short(),
                            bytes = bytes.len(),
                            "served a blob"
                        );
                    }
                    // Not an error: a node that garbage-collected it is behaving correctly,
                    // and the asker moves on to the next peer.
                    None => stream.send(&ClusterMessage::BlobMissing { hash }).await?,
                }
            }
            ClusterMessage::BlobPush { hash, size } => {
                let refusal = if self.blobs.has(hash).await {
                    Some("already held".to_string())
                } else if size > offload_proto::cluster::MAX_BLOB_BYTES {
                    Some(format!("{size} bytes is over the limit"))
                } else if !self.blobs.accepts_push(hash, size) {
                    Some("this node is not taking pushes right now".to_string())
                } else {
                    None
                };

                match refusal {
                    Some(reason) => {
                        stream
                            .send(&ClusterMessage::BlobDeclined { reason })
                            .await?;
                    }
                    None => {
                        stream.send(&ClusterMessage::BlobAccepted).await?;
                        let size = usize::try_from(size).unwrap_or(usize::MAX);
                        let bytes = stream.recv_bytes(size).await?;
                        // Hash what arrived rather than trusting what was announced: a peer
                        // that sends the wrong bytes gets them dropped, not filed under the
                        // hash it claimed (ADR-0016).
                        let received = bytes.len();
                        // The answer goes back only after the bytes are stored: a sender that
                        // heard "accepted" and stopped would report a checkpoint durable while
                        // it was still in flight.
                        let reply = match self.blobs.store(bytes).await {
                            Ok(stored) if stored == hash => {
                                tracing::info!(
                                    node = %peer.short(),
                                    blob = %hash.short(),
                                    bytes = received,
                                    "accepted a replica"
                                );
                                ClusterMessage::BlobStored { hash }
                            }
                            Ok(stored) => {
                                tracing::warn!(
                                    node = %peer.short(),
                                    announced = %hash.short(),
                                    received = %stored.short(),
                                    "pushed blob is not what it claimed to be"
                                );
                                ClusterMessage::BlobDeclined {
                                    reason: format!("those bytes are {}", stored.short()),
                                }
                            }
                            Err(e) => {
                                tracing::warn!(node = %peer.short(), error = %e, "could not store a pushed blob");
                                ClusterMessage::BlobDeclined { reason: e }
                            }
                        };
                        stream.send(&reply).await?;
                    }
                }
            }
            ClusterMessage::Ack { .. }
            | ClusterMessage::Nack { .. }
            | ClusterMessage::BlobFound { .. }
            | ClusterMessage::BlobMissing { .. }
            | ClusterMessage::BlobAccepted
            | ClusterMessage::BlobStored { .. }
            | ClusterMessage::BlobHeld { .. }
            | ClusterMessage::BlobDeclined { .. }
            | ClusterMessage::Bid { .. }
            | ClusterMessage::WillNot { .. }
            | ClusterMessage::Granted { .. }
            | ClusterMessage::SpecEdited { .. }
            | ClusterMessage::EditRefused { .. }
            | ClusterMessage::Events { .. }
            | ClusterMessage::NoEvents { .. }
            | ClusterMessage::Files { .. }
            | ClusterMessage::NoFiles { .. }
            | ClusterMessage::Delivered { .. }
            | ClusterMessage::Undelivered { .. }
            | ClusterMessage::AnswerTaken { .. }
            | ClusterMessage::AnswerRefused { .. }
            | ClusterMessage::Cancelled { .. }
            | ClusterMessage::CancelRefused { .. }
            | ClusterMessage::CheckpointRequested { .. }
            | ClusterMessage::CheckpointRefused { .. }
            | ClusterMessage::ContinueBaseBuilt { .. }
            | ClusterMessage::ContinueBaseRefused { .. }
            | ClusterMessage::Asks { .. }
            | ClusterMessage::Renewed { .. }
            | ClusterMessage::NotRenewed { .. }
            | ClusterMessage::ResourceReady { .. }
            | ClusterMessage::ResourceRefused { .. }
            | ClusterMessage::ResourceData { .. }
            | ClusterMessage::ResourceClosed { .. }
            | ClusterMessage::Declined { .. } => {
                // Answers arrive on the stream that asked; one arriving unprompted is a peer
                // confused about which end of an exchange it is on.
                tracing::debug!(node = %peer.short(), "unsolicited answer");
            }
        }
        // **Not discarded.** quinn resets a stream whose `SendStream` is dropped without being
        // finished, throwing away anything still buffered — so this call is what decides whether
        // a reply that `send` accepted ever reaches the wire, and swallowing its error made a
        // lost answer indistinguishable from one that was never sent.
        if let Err(e) = stream.finish().await {
            tracing::debug!(node = %peer.short(), error = %e, "could not finish the reply stream");
        }
        Ok(())
    }

    /// Fetch a blob from the fleet, verifying it by its hash.
    ///
    /// `from` is where to ask first — usually the node that last held the run — and the rest
    /// of the fleet is asked in turn after that. Availability is not gossiped (ADR-0016), so
    /// asking *is* the discovery: an advertisement would be stale by the time it was used, and
    /// a node that has garbage-collected a blob answers `BlobMissing` in a millisecond.
    pub async fn fetch_blob(
        &self,
        hash: BlobHash,
        from: Option<NodeId>,
    ) -> Result<BlobHash, TransportError> {
        if self.blobs.has(hash).await {
            return Ok(hash);
        }

        // Everybody, in the order most likely to answer — and *not* filtered by status, which
        // is the distinction the doc comment above is making and this list used to contradict.
        // Availability is not gossiped because an advertisement would be stale by the time it
        // was used; a node's liveness is exactly as stale, and asking costs a millisecond
        // either way. The moment a checkpoint is needed is the moment a node has gone quiet, so
        // filtering on `Alive` skipped precisely the peers most likely to hold the bytes: a
        // `Suspect` node is usually reachable — that is the entire reason the state exists — and
        // the only copy in the fleet living on one was unfetchable while it answered every dial.
        //
        // The code already conceded this for `from`, which is asked whatever its status. This is
        // the same rule applied to the rest of the sweep: a live peer is tried first because it
        // is likelier, never because a quiet one is ineligible.
        let mut asked = Vec::new();
        if let Some(first) = from {
            asked.push(first);
        }
        {
            let view = self.locked_view();
            let mut rest: Vec<&NodeView> = view
                .nodes
                .values()
                .filter(|n| n.id != self.node() && Some(n.id) != from)
                .collect();
            rest.sort_by_key(|n| (!n.status.is_available(), n.id));
            asked.extend(rest.iter().map(|n| n.id));
        }

        let mut last = String::from("nobody to ask");
        for peer in asked {
            match self.fetch_from(peer, hash).await {
                Ok(Some(stored)) => return Ok(stored),
                Ok(None) => last = format!("{} does not have it", peer.short()),
                Err(e) => {
                    tracing::debug!(node = %peer.short(), error = %e, "blob fetch failed");
                    last = e.to_string();
                }
            }
        }
        Err(TransportError::Closed {
            peer: hash.short(),
            reason: format!("no peer could supply the blob: {last}"),
        })
    }

    /// `Ok(None)` means the peer answered honestly that it does not have it — which is not a
    /// failure, and must not be confused with one, or a fetch would give up on the first
    /// garbage-collected copy.
    async fn fetch_from(
        &self,
        peer: NodeId,
        hash: BlobHash,
    ) -> Result<Option<BlobHash>, TransportError> {
        let session = self.session(peer).await?;
        let mut stream = session.connection.open().await?;
        stream.send(&ClusterMessage::BlobRequest { hash }).await?;

        match stream.recv::<ClusterMessage>().await? {
            ClusterMessage::BlobFound { size } => {
                if size > offload_proto::cluster::MAX_BLOB_BYTES {
                    return Err(TransportError::Closed {
                        peer: peer.short(),
                        reason: format!("offered {size} bytes, over the limit"),
                    });
                }
                let size = usize::try_from(size).unwrap_or(usize::MAX);
                let bytes = stream.recv_bytes(size).await?;

                let stored = self
                    .blobs
                    .store(bytes)
                    .await
                    .map_err(|e| TransportError::io("storing a fetched blob", &e))?;
                if stored == hash {
                    Ok(Some(stored))
                } else {
                    // The whole reason content addressing is worth having: this is a retry,
                    // not a corrupted checkpoint.
                    Err(TransportError::Closed {
                        peer: peer.short(),
                        reason: format!("sent {} when asked for {}", stored.short(), hash.short()),
                    })
                }
            }
            ClusterMessage::BlobMissing { .. } => Ok(None),
            _ => Err(TransportError::Closed {
                peer: peer.short(),
                reason: "answered a blob request with something else".into(),
            }),
        }
    }

    /// Does a peer hold this blob?
    ///
    /// A `BlobRequest` would answer it too and would drag the bytes across the network to do
    /// so. This asks and reads nothing: it exists because "declined" from a push can mean
    /// *already held*, which is a success, and the sender has to be able to tell.
    pub async fn peer_has_blob(&self, peer: NodeId, hash: BlobHash) -> bool {
        let Ok(session) = self.session(peer).await else {
            return false;
        };
        let Ok(mut stream) = session.connection.open().await else {
            return false;
        };
        if stream
            .send(&ClusterMessage::BlobHave { hash })
            .await
            .is_err()
        {
            return false;
        }
        matches!(
            stream.recv::<ClusterMessage>().await,
            Ok(ClusterMessage::BlobHeld { held: true, .. })
        )
    }

    /// Offer a blob to a peer, so a checkpoint exists somewhere other than here.
    ///
    /// The durability half of ADR-0016. A decline is an answer rather than a failure, and it
    /// comes back **with the peer's sentence** rather than as a `false`: the caller cannot tell
    /// *already held* — a perfectly good outcome — from *over the size limit*, which no amount
    /// of waiting will fix, and it was re-asking the peer to find out.
    pub async fn push_blob(&self, peer: NodeId, hash: BlobHash) -> Result<Pushed, TransportError> {
        let Some(bytes) = self.blobs.get(hash).await else {
            return Err(TransportError::io(
                "pushing a blob",
                &format!("this node does not hold {}", hash.short()),
            ));
        };

        // Asked here, before a session is opened: the cap is the same on every node, so this
        // is a fact about the blob and dialling a peer to be told it wastes a round trip and
        // returns an answer that reads like the peer's opinion.
        let size = bytes.len() as u64;
        let limit = offload_proto::cluster::MAX_BLOB_BYTES;
        if size > limit {
            return Ok(Pushed::TooLarge { size, limit });
        }

        let session = self.session(peer).await?;
        let mut stream = session.connection.open().await?;
        stream
            .send(&ClusterMessage::BlobPush { hash, size })
            .await?;

        match stream.recv::<ClusterMessage>().await? {
            ClusterMessage::BlobAccepted => {
                stream.send_bytes(&bytes).await?;
                // Waited for on purpose: this is what makes the difference between "the peer
                // was willing" and "the peer has it", and only the second one is durability.
                match stream.recv::<ClusterMessage>().await? {
                    ClusterMessage::BlobStored { hash: stored } if stored == hash => {
                        Ok(Pushed::Stored)
                    }
                    ClusterMessage::BlobDeclined { reason } => {
                        tracing::warn!(node = %peer.short(), blob = %hash.short(), %reason, "replica refused after transfer");
                        Ok(Pushed::Declined { reason })
                    }
                    _ => Err(TransportError::Closed {
                        peer: peer.short(),
                        reason: "did not confirm the blob it was sent".into(),
                    }),
                }
            }
            // Not logged here any more, and it used to be at `debug` while the refusal *after*
            // a transfer was at `warn` — exactly backwards. A refusal after the bytes have moved
            // is the transient one (wrong bytes, a retry); this one is where the size cap fires,
            // and being over the cap is permanent. The caller has the sentence now and decides.
            ClusterMessage::BlobDeclined { reason } => Ok(Pushed::Declined { reason }),
            _ => Err(TransportError::Closed {
                peer: peer.short(),
                reason: "answered a blob push with something else".into(),
            }),
        }
    }

    // -- talking ------------------------------------------------------------------------

    /// Send one message and wait for one answer, on a stream of its own.
    ///
    /// The timeout covers **connecting as well as answering**, because a probe period has to
    /// be bounded end to end. Dialling a machine that has vanished is where the time actually
    /// goes — the kernel and QUIC will both wait far longer than a probe interval — and a
    /// period that overruns stops suspicions from ever expiring into conclusions.
    async fn ask(
        &self,
        peer: NodeId,
        message: &ClusterMessage,
        timeout: Millis,
    ) -> Result<ClusterMessage, TransportError> {
        let exchange = async {
            let session = self.session(peer).await?;
            let mut stream = session.connection.open().await?;
            stream.send(message).await?;
            stream.recv().await
        };

        match tokio::time::timeout(Duration::from_millis(timeout.0), exchange).await {
            Ok(result) => result,
            Err(_) => Err(TransportError::Closed {
                peer: peer.short(),
                reason: format!("no answer within {}ms", timeout.0),
            }),
        }
    }

    async fn session(&self, peer: NodeId) -> Result<Arc<Session>, TransportError> {
        if let Some(session) = self.existing_session(peer).await {
            return Ok(session);
        }
        let session = Arc::new(match self.transport.connect(peer).await {
            Ok(session) => session,
            Err(e) => {
                self.note_own_revocation(&e);
                self.note_turnaway(peer, &e);
                return Err(e);
            }
        });
        self.record_name(peer, &session.peer.name);
        self.record_certificate(&session.peer.membership);
        self.connections.lock().await.insert(peer, session.clone());
        // Served as well as used, so the peer can reach this node back over it (see `serve`).
        // Nothing to do if nobody is serving — a cluster built without `serve` answers nothing.
        let _ = self.dialled.send(session.clone());
        Ok(session)
    }

    /// The session [`Self::session`] would use with `peer` right now, without dialling: the open
    /// one this node dialled, else the newest open one the peer dialled.
    ///
    /// **One lookup for every question about "the connection to that peer".** The inbound
    /// fall-back went into `session` alone, and the bid round went on reading the peer's
    /// certificate from the dialled sessions only — so a bid that arrived over the peer's own
    /// connection had "no certificate", was refused and hung up on, and a phone nobody can dial
    /// could never host (session ninety-two).
    pub(crate) async fn existing_session(&self, peer: NodeId) -> Option<Arc<Session>> {
        {
            let mut connections = self.connections.lock().await;
            match connections.get(&peer) {
                Some(session) if !session.connection.is_closed() => return Some(session.clone()),
                // Closed from the far end — a peer that refused our certificate hangs up both
                // ways — and forgotten here, so the next exchange dials rather than failing once
                // on a dead session first.
                Some(_) => {
                    connections.remove(&peer);
                }
                None => {}
            }
        }
        // **The connection the peer opened, before dialling it.** A node that can only dial out —
        // a phone on a carrier, which admits nothing inbound — is reachable over its own
        // connection or not at all. Dialling instead is what had the laptop answer a second
        // node's indirect probe with a `Nack` about a phone it was talking to that very second,
        // and the second node declare the phone dead 1 772 times in one night (Samsung phone,
        // mobile data). The newest such session, since an older one may be on its way out.
        let inbound = self.inbound.lock().await;
        inbound
            .iter()
            .filter(|(_, (node, session))| *node == peer && !session.connection.is_closed())
            .max_by_key(|(token, _)| **token)
            .map(|(_, (_, session))| session.clone())
    }

    /// File a revocation *of this node* that arrived as a refusal on a dial (ADR-0044).
    ///
    /// The path a revoked node's own eviction actually reaches it by, and until this existed it
    /// was the path with no handler. Gossip cannot carry the fact — the revoking node hangs up in
    /// both directions the moment it files it, and refuses every handshake after — so the only
    /// message a revoked node still receives from its fleet is the refusal itself.
    ///
    /// **The refusal is not what is believed; the signature is.** A bare `Refusal::Revoked` is a
    /// peer's claim about this node, and this whole crate is built on a node never believing one
    /// of those about itself. So what is handed on is the `Revocation`, and
    /// [`Members::revoked`] verifies it against the fleet key this node already holds before
    /// writing anything down — the same check that makes a *gossiped* revocation safe to file,
    /// on the same code path.
    ///
    /// The `member` check is the second half of it. A refusal reaches here only from a dial, so
    /// the subject should always be this node; a refusal naming somebody else is either a bug or
    /// a peer trying something, and neither is a reason to file a revocation for a third party
    /// out of a message addressed to us.
    /// Note that a peer would not admit this node to its fleet (ADR-0060).
    ///
    /// [`Self::note_own_revocation`]'s sibling, and the difference between them is the whole
    /// decision. That one is handed a **signature** and acts on it; this one is handed a
    /// sentence and may not. `offload rekey` revokes nobody — it founds a new fleet without
    /// this device — so there is no credential that could make the claim checkable, and a node
    /// that stood down on an uncheckable claim is a node any peer could switch off.
    ///
    /// So it is counted. The half that is this node's own to state is that it dialled, reached
    /// something, and was turned away, which every report used to render as `no answer`: a
    /// rekeyed-out daemon called its former fleet `dead` and went on saying `2 member(s) met`.
    fn note_turnaway(&self, peer: NodeId, error: &TransportError) {
        let TransportError::Refused {
            refusal: refusal @ offload_proto::handshake::Refusal::WrongFleet { .. },
            ..
        } = error
        else {
            return;
        };
        // The peer's own words. They name both fleets, which is the entire diagnosis, and a
        // sentence of ours in their place would be this node resolving a claim it has just
        // decided it cannot resolve.
        self.turnaways.record(peer, &refusal.to_string());
    }

    /// Handshake refusals this node has collected since it started (ADR-0060).
    #[must_use]
    pub fn turnaways(&self) -> Vec<turnaway::Turnaway> {
        self.turnaways.by_peer()
    }

    /// How many, including peers past the breakdown's cap.
    #[must_use]
    pub fn turnaways_total(&self) -> u64 {
        self.turnaways.total()
    }

    fn note_own_revocation(&self, error: &TransportError) {
        let TransportError::Refused {
            refusal: offload_proto::handshake::Refusal::Revoked { proof, .. },
            ..
        } = error
        else {
            return;
        };
        if proof.member != self.node() {
            tracing::warn!(
                named = %proof.member.short(),
                "a peer refused us with a revocation naming somebody else; ignored"
            );
            return;
        }
        if self.members().revoked((**proof).clone()) {
            tracing::error!(
                "this node has been revoked from its own fleet, and a peer proved it with the fleet's own signature"
            );
        }
    }

    /// Forget the session this node *dialled*, so the next exchange re-dials.
    ///
    /// Deliberately not [`Self::disconnect`]: this is what a failed probe does, and a probe
    /// failing says as much about the path as about the peer — which is the whole argument
    /// `probe_indirect` rests on. An inbound session that is still delivering is evidence of
    /// life over a path that works, and throwing it away because *our* dial did not would
    /// discard the better of the two answers.
    async fn drop_connection(&self, peer: NodeId) {
        if let Some(session) = self.connections.lock().await.remove(&peer) {
            session.connection.close("probe failed");
        }
    }

    /// Hang up on a peer **in both directions**, so the next exchange re-handshakes.
    ///
    /// Public because membership can change under a live connection and the handshake is the
    /// only place it is checked: a QUIC session that stays up never presents its certificate
    /// again, so a device revoked an hour ago keeps talking until somebody closes the socket
    /// (ADR-0012 — revocation is immediate and local, or it is not immediate).
    ///
    /// **Both directions, because the dangerous one is the one this node did not open.** For a
    /// session per peer in `connections` there is a caller that dialled it and will dial again;
    /// the revoked device is by definition the one still trying, and its connection is inbound.
    /// Closing only what we dialled refuses a device that is not going to make a connection and
    /// leaves the one it did make open — see [`Self::inbound`] for what that measured.
    pub async fn disconnect(&self, peer: NodeId, reason: &str) {
        if let Some(session) = self.connections.lock().await.remove(&peer) {
            session.connection.close(reason);
        }
        // Closed, not removed: the entry belongs to the task serving it, which drops it when
        // the close ends its `accept`. Removing it here would leave that task holding the only
        // reference to a session nothing can reach a second time.
        for (_, session) in self.inbound.lock().await.values() {
            if session.peer.node == peer {
                session.connection.close(reason);
            }
        }
    }

    // -- state, each behind its own short lock --------------------------------------------

    fn locked_view(&self) -> std::sync::MutexGuard<'_, ClusterView> {
        self.view
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn locked_detector(&self) -> std::sync::MutexGuard<'_, Detector> {
        self.detector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn config(&self) -> DetectorConfig {
        self.locked_detector().config()
    }

    pub(crate) fn next_seq(&self) -> u64 {
        let mut seq = self
            .seq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *seq = seq.wrapping_add(1);
        *seq
    }

    /// What we currently believe, for a peer to merge. Public to `place`, which uses an
    /// ordinary probe to get a submission written down somewhere else.
    pub(crate) fn gossip_snapshot(&self) -> Gossip {
        self.gossip()
    }

    /// What we currently believe, for a peer to merge.
    fn gossip(&self) -> Gossip {
        let view = self.locked_view();
        Gossip {
            nodes: view.nodes.values().cloned().collect(),
            runs: view.runs.values().cloned().collect(),
            progress: view
                .progress
                .iter()
                .map(|(run, progress)| offload_proto::cluster::ProgressReport {
                    run: *run,
                    progress: progress.clone(),
                })
                .collect(),
            // Read outside the view lock's business on purpose: membership is not part of the
            // view, and a fact that outranks a signature has no place being merged by the same
            // rules as a node's own self-report.
            revocations: self.members().revocations(),
            // Own and learned alike, tombstones included: a removal that stopped being sent
            // would be re-learned from the next peer for ever (ADR-0056 §5).
            schedules: self.locked_schedules().values().cloned().collect(),
            models_asked: *self.models_asked.borrow(),
        }
    }

    /// How many exchanges have carried this node's gossip to a peer (see the field).
    #[must_use]
    pub fn exchanges(&self) -> u64 {
        self.exchanges.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether this node holds a revocation of `node`: the one way a device leaves a fleet for
    /// good (ADR-0012). Being absent from the view is not that — a node that has just started has
    /// heard of almost nobody yet.
    #[must_use]
    pub fn is_revoked(&self, node: NodeId) -> bool {
        self.members()
            .revocations()
            .iter()
            .any(|revocation| revocation.covers(node))
    }

    /// Ask the fleet to read its agents' model lists again (ADR-0080): raise the count, which
    /// travels on every probe from here. Returns the new count. The node that asks reads its
    /// own list straight away rather than waiting to hear this back.
    pub fn ask_for_models(&self) -> u64 {
        let mut now = 0;
        self.models_asked.send_modify(|held| {
            *held = held.saturating_add(1);
            now = *held;
        });
        now
    }

    /// The fleet's count of requests to read model lists again, woken whenever it rises after
    /// this node's first exchange; a count heard in that first exchange predates this node's
    /// startup, so it is adopted without waking anybody.
    #[must_use]
    pub fn models_asked(&self) -> tokio::sync::watch::Receiver<u64> {
        self.models_asked.subscribe()
    }

    /// The copy of the schedules that gossips, re-synced from the daemon's store.
    ///
    /// Wholesale rather than merged, for `publish_runs`' reason: the store is the truth, and it
    /// has already applied `Schedule::merge` to everything that arrived. Anything that has
    /// stopped being in the store's list has been forgotten there, and there is exactly one way
    /// for that to happen — which is not removal (that leaves a tombstone) but a database
    /// somebody rebuilt.
    pub fn publish_schedules(&self, schedules: Vec<offload_core::Schedule>) {
        let mut held = self.locked_schedules();
        held.clear();
        for schedule in schedules {
            held.insert(schedule.id, schedule);
        }
    }

    fn locked_schedules(
        &self,
    ) -> std::sync::MutexGuard<
        '_,
        std::collections::BTreeMap<offload_core::ScheduleId, offload_core::Schedule>,
    > {
        self.schedules
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Publish the runs this node holds, so the fleet — and whoever submitted them — can see
    /// what became of them.
    ///
    /// Replaces this node's contribution wholesale rather than merging: the store is the
    /// truth for a run we hold, and anything of ours no longer in the list has either
    /// finished long enough ago to stop being news or moved somewhere else.
    pub fn publish_runs(&self, runs: Vec<offload_core::Run>) {
        let me = self.node();
        let mut view = self.locked_view();
        view.runs.retain(|_, run| run.holder() != Some(me));
        for run in runs {
            view.runs.insert(run.id, run);
        }
    }

    /// Forget runs that finished before `cutoff`, so probes stop carrying them
    /// (`ClusterView::forget_finished_before`). Returns how many went.
    pub fn forget_finished_before(&self, cutoff: Millis) -> usize {
        self.locked_view().forget_finished_before(cutoff)
    }

    /// Publish one decision about a run, leaving every other record alone.
    ///
    /// Not [`Self::publish_runs`] with one element: that replaces this node's *whole*
    /// contribution, which is right for the tick that republishes the store and wrong for an
    /// arbiter's decision about somebody else's run. It would drop every run we hold from our
    /// own view until the next tick — and `running_count` reads that view to answer whether
    /// we are at capacity, so a bid arriving in the gap would be answered as if this node
    /// were idle.
    pub fn publish_run(&self, run: offload_core::Run) {
        self.locked_view().runs.insert(run.id, run);
    }

    /// Publish what this node's own runs have done and cost.
    ///
    /// Ours to state without arbitration — we are the node running the turns — so this writes
    /// straight in rather than merging. The forward-only rule exists to stop *other* nodes
    /// rolling these numbers back; applying it to our own would mean a run that was reopened
    /// and started again could never report a smaller number than the one it superseded.
    pub fn publish_progress(&self, progress: Vec<(offload_core::RunId, RunProgress)>) {
        let mut view = self.locked_view();
        for (run, progress) in progress {
            view.progress.insert(run, progress);
        }
        view.prune_progress();
    }

    /// Merge what a peer believes, and answer any suspicion about *us* by refuting it.
    ///
    /// Returns the runs worth writing down: ones held elsewhere whose record moved forward.
    /// The caller persists them, because a run known only in view memory is gone at the next
    /// restart and the node that submitted it is the one most likely to be shut (ADR-0006).
    fn absorb(&self, gossip: &Gossip, from: NodeId, now: Millis) -> Learned {
        let me = self.node();
        // **Taken before the view is locked**, and applied on the way in rather than repaired
        // afterwards. A `NodeView`'s `name` is the node's own config name, which defaults to the
        // machine's hostname; the *certificate* carries the name the fleet gave the device, it is
        // signed, and ADR-0012 puts it there for `offload nodes` in as many words. `record_name`
        // writes it at the handshake — and then the first merge that raises the peer's
        // incarnation copies the gossiped name straight over it. An incarnation rises whenever
        // capabilities or policy change, which on a laptop is every probe: measured on two
        // daemons, `bravo` for **25 seconds** and `fedora` thereafter, for ever, while the
        // daemon's own startup warning promises that peers show the certificate's name.
        //
        // Correcting `incoming` rather than the entry afterwards keeps this out of `merge_node`,
        // which is `offload-core` and has no certificates to consult, and means no reader ever
        // sees the wrong name for an instant.
        let certified = self.certified_names();
        let mut view = self.locked_view();
        let mut refuted: Option<NodeStatus> = None;
        for node in &gossip.nodes {
            if node.id == me {
                // A peer's copy of *us*: believed about nothing, and its incarnation is still
                // information. `merge_node` owns both answers — refute a wrong liveness claim,
                // and land clear of a number a previous life reached — because this loop used to
                // `continue` here and call `refute()` beside it, which is the first answer and
                // not the second. A `+1` from a node that has just restarted does not escape
                // what its peers remember, so everything it said about itself was dropped.
                if view.merge_node(node.clone(), now) {
                    refuted = Some(node.status);
                }
                continue;
            }
            // A peer this node has never handshaken with — learned by relay — has no certificate
            // here, and its gossiped name is the only one there is. That is the honest answer
            // rather than a gap, and it is why this corrects rather than refuses.
            let mut node = node.clone();
            if let Some(name) = certified.get(&node.id) {
                node.name.clone_from(name);
            }
            view.merge_node(node, now);
        }
        if let Some(claimed) = refuted {
            let incarnation = view.node(&me).map_or(0, |me| me.incarnation);
            // Two different things to be wrong about, and the wording says which: a peer that
            // thinks we are gone, and a peer holding a previous life's facts about us at a number
            // we had already reached. The second is every restart, and it says `Alive` while
            // being wrong, so a message about liveness alone would describe the wrong bug.
            if claimed == NodeStatus::Alive {
                tracing::info!(
                    incarnation,
                    "a peer was holding facts from a previous life of this node; refuting"
                );
            } else {
                tracing::info!(
                    incarnation,
                    claimed = ?claimed,
                    "a peer thought we were gone; refuting"
                );
            }
        }

        let mut learned = Learned::default();
        for run in &gossip.runs {
            // A run we hold is ours to describe: our own store is newer than anything a peer
            // can say about it, and merging their copy would let a stale record undo a turn.
            //
            // Except for the half of it that was never ours. The deadline and the priority
            // belong to the run's *home* node — that is where `offload deadline` is forwarded
            // to — and gossip is the only way an edit can reach the node running the run. So
            // the record is refused and the edit is taken.
            if run.holder() == Some(me) {
                // The peer's record, not the view's copy of ours. What the view holds for a run
                // this node is *running* is whatever the last gossip tick published, and the
                // store has moved on since: a checkpoint recorded at a turn boundary, or the
                // agent finishing. Handing that copy down to be written whole is how an edit
                // arriving a second after a checkpoint erases it — the merge refuses a peer's
                // record here for exactly that reason and then passed one on anyway.
                if view.merge_spec_edit(run) {
                    learned.edits.push(run.clone());
                }
                continue;
            }
            let behind = !run.state.is_terminal()
                && view
                    .runs
                    .get(&run.id)
                    .is_some_and(|known| known.state.is_terminal() && known.epoch >= run.epoch);
            if behind {
                learned.stale.push(run.id);
            }
            if view.merge_run(run.clone(), from) {
                // The record as *settled*, not as it arrived. They differ when the incoming
                // record carried an older spec revision than the one already here: the merge
                // patches it before adopting it, and writing the raw arrival down instead
                // would leave the store a revision behind the view — silently, and for good,
                // since the store is what a restart believes.
                learned.runs.extend(
                    view.runs
                        .get(&run.id)
                        .cloned()
                        .or_else(|| Some(run.clone())),
                );
            }
        }

        for report in &gossip.progress {
            // Same reason, and the more dangerous direction: every node gossips what it knows
            // about runs it does not hold, and the submitting node's copy has zeros in it.
            if view
                .runs
                .get(&report.run)
                .and_then(offload_core::Run::holder)
                == Some(me)
            {
                continue;
            }
            // As settled, never as it arrived: `absorb`'s answer can be a combination of two
            // legs — this leg's position beside the fleet's high-water spend — and handing the
            // store the incoming copy instead would leave it holding a record the view never
            // agreed with, permanently, since the loser never wins a later merge. Same mistake
            // the run half of this function made and the same fix.
            if let Some(settled) = view.merge_progress(report.run, report.progress.clone()) {
                learned.progress.push((report.run, settled));
            }
        }

        // Schedules, and the whole merge rule is `Schedule::merge` (ADR-0056 §5): every field
        // but the tombstone is immutable, so the only question is whether this copy knows
        // something the stored one does not. **Only the changes travel down** — a node that
        // handed the store every schedule in every probe would rewrite each row once a second
        // for the life of the fleet, and the store would then republish them, which is a loop
        // that does nothing but write.
        //
        // Outside the view deliberately: nothing that reads the view has business with a
        // schedule (see the field's own comment), and this needs a different lock.
        drop(view);
        {
            let mut held = self.locked_schedules();
            for schedule in &gossip.schedules {
                let settled = match held.get(&schedule.id) {
                    Some(existing) => existing.merge(schedule),
                    None => schedule.clone(),
                };
                let changed = held.get(&schedule.id) != Some(&settled);
                if changed {
                    held.insert(settled.id, settled.clone());
                    learned.schedules.push(settled);
                }
            }
        }
        // By maximum, the whole of the rule: nobody lowers it, so a smaller count is old news.
        // A rise in the first exchange is adopted quietly (see `heard_any`); every later rise is
        // somebody asking now, and wakes the node's reader.
        let asked = gossip.models_asked;
        let first = !self
            .heard_any
            .swap(true, std::sync::atomic::Ordering::Relaxed);
        self.models_asked.send_if_modified(|held| {
            let raised = asked > *held;
            if raised {
                *held = asked;
            }
            raised && !first
        });
        learned
    }

    /// Merge a peer's gossip and write down anything new about runs elsewhere.
    async fn learn(&self, gossip: &Gossip, from: NodeId, now: Millis) {
        // Membership first, and outside `absorb`: filing a revocation writes a file, which has
        // no business happening under the view lock, and hanging up needs the connection map
        // that lock does not cover. Rare enough to be worth doing plainly — a revocation that
        // is not new returns immediately.
        let members = self.members();
        for revocation in &gossip.revocations {
            let member = revocation.member;
            if members.revoked(revocation.clone()) {
                // "that node", not "this node": the member named is almost always a third
                // party, and the one log line about a revocation should not read as though the
                // daemon writing it were the one thrown out.
                tracing::warn!(
                    node = %member.short(),
                    "heard that a node has been revoked; hanging up on it"
                );
                self.disconnect(member, "revoked").await;
            }
        }

        let learned = self.absorb(gossip, from, now);
        for run in learned.runs {
            self.host().record(&run).await;
        }
        for run in learned.edits {
            self.host().record_spec_edit(&run).await;
        }
        for (run, progress) in learned.progress {
            self.host().record_progress(run, &progress).await;
        }
        for schedule in learned.schedules {
            self.host().record_schedule(&schedule).await;
        }
        for run in learned.stale {
            self.host().stale_copy(run).await;
        }
    }

    fn pick_target(&self, now: Millis) -> Option<NodeId> {
        let view = self.view();
        self.locked_detector().next_target(&view, now)
    }

    fn pick_helpers(&self, target: NodeId) -> Vec<NodeId> {
        let view = self.view();
        self.locked_detector().helpers(&view, target)
    }

    fn expired_suspicions(&self, now: Millis) -> Vec<NodeId> {
        let view = self.view();
        self.locked_detector().expired(&view, now)
    }

    fn suspect(&self, node: NodeId, now: Millis) -> Option<NodeStatus> {
        self.locked_detector().unreachable(node, now)
    }

    fn forget(&self, node: NodeId) {
        self.locked_detector().forget(node);
    }

    /// Record that a node answered: clear any suspicion and put it back in the view.
    fn alive(&self, node: NodeId, now: Millis) {
        let cleared = self.locked_detector().heard_from(node);
        let mut view = self.locked_view();
        if cleared {
            // Back from a conclusion is news; back from a question is not (see `probe_round`).
            if view
                .nodes
                .get(&node)
                .is_some_and(|n| n.status == NodeStatus::Dead)
            {
                tracing::info!(node = %node.short(), "answered: back after being marked dead");
            } else {
                tracing::debug!(node = %node.short(), "answered: no longer suspect");
            }
        }
        if let Some(entry) = view.nodes.get_mut(&node) {
            entry.set_status(NodeStatus::Alive, now);
            entry.last_heard = now;
            entry.heard_here = Some(now);
        }
    }

    /// Remember what a peer's certificate says about it.
    ///
    /// Authenticated rather than gossiped, which is the whole reason it is worth keeping: a
    /// node's grants live on a certificate precisely because its own claims about itself are
    /// not to be believed (ADR-0012), so counting approvers from gossip would count whatever a
    /// device said it was. This is what the handshake proved, and it is therefore only about
    /// peers this node has actually met — which is exactly the caveat `offload status` has to
    /// state when it reports fleet health.
    fn record_certificate(&self, cert: &offload_core::MembershipCert) {
        self.certificates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(cert.member, cert.clone());
    }

    /// [`Self::record_certificate`] for papers that arrived mid-connection, which replace what is
    /// held only when they were issued later: an old certificate carried again must not wind this
    /// node's view of the peer back.
    fn record_newer_certificate(&self, cert: &offload_core::MembershipCert) {
        let mut held = self
            .certificates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held
            .get(&cert.member)
            .is_none_or(|current| current.issued_at < cert.issued_at)
        {
            held.insert(cert.member, cert.clone());
        }
    }

    /// The certificates of every peer this node has handshaken with since it started.
    #[must_use]
    pub fn certificates(&self) -> Vec<offload_core::MembershipCert> {
        self.certificates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    /// Adopt the name a peer's certificate carries.
    ///
    /// Better than gossip and not merely cosmetic: a certificate's name is signed, so it is the
    /// one nobody else could have chosen, and ADR-0012 puts it on the certificate *for*
    /// `offload nodes`. The gossiped name is whatever the node's own config says it is called,
    /// which defaults to the machine's hostname — so two devices can carry the same one.
    ///
    /// This is the first answer and not the whole of it: a merge would overwrite what is written
    /// here, which is why `absorb` corrects an incoming name from [`Self::certified_names`].
    fn record_name(&self, node: NodeId, name: &str) {
        if name.is_empty() {
            return;
        }
        let mut view = self.locked_view();
        if let Some(entry) = view.nodes.get_mut(&node) {
            if entry.name != name {
                entry.name = name.to_string();
            }
        }
    }

    /// The name on the certificate of every peer this node has handshaken with.
    ///
    /// Taken as a map, once, so the lock on the certificates is released before the view's is
    /// taken: `absorb` holds the view for its whole merge loop, and a second lock acquired
    /// inside it is an ordering nobody should have to reason about.
    fn certified_names(&self) -> std::collections::HashMap<NodeId, String> {
        self.certificates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, cert)| !cert.name.is_empty())
            .map(|(node, cert)| (*node, cert.name.clone()))
            .collect()
    }

    /// What this node currently believes about a peer, or `None` for one it has never met.
    fn status_of(&self, node: NodeId) -> Option<NodeStatus> {
        self.locked_view().node(&node).map(|entry| entry.status)
    }

    /// Apply a conclusion of our own to the view. It spreads with the next gossip, because
    /// gossip is whatever we believe.
    fn declare(&self, node: NodeId, status: NodeStatus, now: Millis) {
        let mut view = self.locked_view();
        if let Some(entry) = view.nodes.get_mut(&node) {
            entry.set_status(status, now);
        }
    }
}

/// Carry lines both ways until one end stops.
///
/// Deliberately dumb: everything about *what* is being carried belongs to the agent's protocol,
/// and the moment this function knows a JSON-RPC method name it has become a reimplementation of
/// somebody else's protocol living inside a proxy (ADR-0004's rule, on a different plane).
///
/// Returns why it ended, for the log at both ends. A proxy that stops silently is the hardest
/// kind of failure to look at afterwards: the agent simply reports a tool that did not answer.
async fn pump_resource(stream: &mut Stream, channel: &mut dyn ResourceChannel) -> String {
    loop {
        tokio::select! {
            incoming = stream.recv::<ClusterMessage>() => match incoming {
                Ok(ClusterMessage::ResourceData { line }) => {
                    if let Err(e) = channel.send(line).await {
                        let _ = stream
                            .send(&ClusterMessage::ResourceClosed { reason: e.clone() })
                            .await;
                        return e;
                    }
                }
                Ok(ClusterMessage::ResourceClosed { reason }) => return reason,
                Ok(_) => return "the caller said something that is not a resource message".into(),
                Err(e) => return format!("the caller went away ({e})"),
            },
            outgoing = channel.next() => match outgoing {
                Some(line) => {
                    if let Err(e) = stream.send(&ClusterMessage::ResourceData { line }).await {
                        return format!("could not answer the caller ({e})");
                    }
                }
                // The server exited. Said out loud rather than left for the caller to time out
                // on, because an MCP server that dies is a thing the owner has to fix.
                None => {
                    let _ = stream
                        .send(&ClusterMessage::ResourceClosed {
                            reason: "the resource's own program stopped".into(),
                        })
                        .await;
                    return "the resource's own program stopped".into();
                }
            },
        }
    }
}

#[cfg(test)]
mod streak_tests {
    use super::*;

    /// A slow answer is not a broken connection: only `TIMEOUTS_BEFORE_REDIAL` in a row hang
    /// up, and an answer in between starts the count again (session ninety-two, the phone on LTE).
    #[test]
    fn one_slow_probe_does_not_hang_up_three_in_a_row_do() {
        let a = NodeId::from_bytes([1; 32]);
        let b = NodeId::from_bytes([2; 32]);
        let mut streaks = TimeoutStreaks::default();
        assert!(!streaks.timed_out(a));
        assert!(!streaks.timed_out(a));
        streaks.answered(a);
        assert!(!streaks.timed_out(a), "an answer started the count again");
        assert!(!streaks.timed_out(b), "per peer");
        assert!(!streaks.timed_out(a));
        assert!(streaks.timed_out(a), "the third in a row re-dials");
        assert!(
            !streaks.timed_out(a),
            "and the next streak starts from nothing"
        );
    }
}
