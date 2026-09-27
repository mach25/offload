//! The scenarios this crate exists for: a node goes quiet, a path goes bad, a device leaves.
//!
//! All of it over the in-memory transport with the clock in the test's hands. That is the
//! point of both — a partition happens at an exact instant, suspicion expires at an exact
//! instant, and nothing here sleeps hoping the timing works out.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use offload_cluster::blobs::Blobs;
use offload_cluster::{Clock, Cluster, DetectorConfig, Pushed};
use offload_core::fleet::{FleetKey, PROBATION};
use offload_core::{
    Arch, BlobHash, Capabilities, ClusterView, DeviceClass, FleetId, Grant, Issuer, MembershipCert,
    Millis, NodeId, NodeStatus, NodeView, Os, Revocation, Terms, WorkPolicy,
};
use offload_proto::handshake::Credentials;
use offload_transport::memory::MemoryNetwork;
use offload_transport::{Membership, Transport};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const START: Millis = Millis(1_700_000_000_000);

fn signing(seed: u8) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[seed; 32])
}

fn id(seed: u8) -> NodeId {
    NodeId::from_bytes(signing(seed).verifying_key().to_bytes())
}

fn fleet() -> FleetKey {
    FleetKey::derive("abacus zoom yo-yo").expect("derive")
}

/// A node's membership and its clock, both movable by the test.
struct TestNode {
    fleet: FleetId,
    credentials: std::sync::Mutex<Credentials>,
    /// Who this node has been told no longer belongs. A list rather than a set of ids, because
    /// a revocation is a signed fact that travels and keeping the signature is what lets this
    /// node pass it on.
    revocations: std::sync::Mutex<Vec<Revocation>>,
    /// The fleet-signed statement that this node may issue certificates, if it is an approver.
    delegation: std::sync::Mutex<Option<offload_core::Delegation>>,
    /// This node's own key. An approver signs renewals with it, under the delegation above —
    /// never with the fleet key, which is on no device at all.
    signing: ed25519_dalek::SigningKey,
    now: AtomicU64,
}

impl TestNode {
    fn new(key: &FleetKey, seed: u8, name: &str) -> Arc<TestNode> {
        Self::with_terms(key, seed, Terms::founding(key.id(), id(seed), name, START))
    }

    fn with_terms(key: &FleetKey, seed: u8, terms: Terms) -> Arc<TestNode> {
        Arc::new(TestNode {
            fleet: key.id(),
            delegation: std::sync::Mutex::new(None),
            signing: signing(seed),
            credentials: std::sync::Mutex::new(Credentials {
                membership: MembershipCert::issue(key.signing_key(), Issuer::Fleet, terms),
                delegation: None,
            }),
            revocations: std::sync::Mutex::new(Vec::new()),
            now: AtomicU64::new(START.0),
        })
    }

    /// The owner granted something: a new certificate exists on this node, and nothing tells
    /// the connections that are already up — which is the situation the bid check has to heal.
    fn reissue(&self, key: &FleetKey, terms: Terms) {
        *self.credentials.lock().expect("lock") = Credentials {
            membership: MembershipCert::issue(key.signing_key(), Issuer::Fleet, terms),
            delegation: None,
        };
    }

    fn set(&self, now: Millis) {
        self.now.store(now.0, Ordering::SeqCst);
    }

    /// The owner typed `offload revoke` on this device.
    fn revoke(&self, key: &FleetKey, member: NodeId) {
        let at = Millis(self.now.load(Ordering::SeqCst));
        self.revocations
            .lock()
            .expect("lock")
            .push(Revocation::issue(
                key.signing_key(),
                self.fleet,
                member,
                at,
                at.0,
            ));
    }

    /// The fleet key delegated to this node: it may issue certificates from now on.
    fn make_approver(&self, key: &FleetKey) {
        let me = self.credentials.lock().expect("lock").membership.member;
        *self.delegation.lock().expect("lock") = Some(offload_core::Delegation::issue(
            key.signing_key(),
            self.fleet,
            me,
            START,
            Millis(365 * 24 * 60 * 60 * 1_000),
            1,
        ));
    }

    fn expires_at(&self) -> Millis {
        self.credentials.lock().expect("lock").membership.expires_at
    }

    fn take_up(&self, credentials: Credentials) {
        *self.credentials.lock().expect("lock") = credentials;
    }

    fn knows_revoked(&self, member: NodeId) -> bool {
        self.revocations
            .lock()
            .expect("lock")
            .iter()
            .any(|r| r.covers(member))
    }
}

impl Membership for TestNode {
    fn fleet(&self) -> FleetId {
        self.fleet
    }
    fn credentials(&self) -> Credentials {
        self.credentials.lock().expect("lock").clone()
    }
    fn revocations(&self) -> Vec<Revocation> {
        self.revocations.lock().expect("lock").clone()
    }
    fn now(&self) -> Millis {
        Millis(self.now.load(Ordering::SeqCst))
    }
}

impl offload_cluster::Members for TestNode {
    fn revocations(&self) -> Vec<Revocation> {
        self.revocations.lock().expect("lock").clone()
    }

    fn renew(
        &self,
        cert: &MembershipCert,
        _delegation: Option<&offload_core::Delegation>,
        now: Millis,
    ) -> Result<Credentials, String> {
        // The approver's half, as small as it can be while still being the real rule: only a
        // node the fleet key delegated to may re-issue, and what comes back is the same
        // certificate with a fresh clock.
        let Some(delegation) = self.delegation.lock().expect("lock").clone() else {
            return Err("this node is not an approver".into());
        };
        Ok(Credentials {
            membership: MembershipCert::issue(
                &self.signing,
                Issuer::Approver {
                    node: self.credentials.lock().expect("lock").membership.member,
                },
                cert.renewal(now, offload_core::fleet::CERT_LIFETIME),
            ),
            delegation: Some(delegation),
        })
    }

    fn revoked(&self, revocation: Revocation) -> bool {
        if revocation.verify(self.fleet).is_err() {
            return false;
        }
        let mut held = self.revocations.lock().expect("lock");
        if held.iter().any(|r| r.covers(revocation.member)) {
            return false;
        }
        held.push(revocation);
        true
    }
}

impl Clock for TestNode {
    fn now(&self) -> Millis {
        Millis(self.now.load(Ordering::SeqCst))
    }
}

fn node_view(node: NodeId) -> NodeView {
    let mut view = NodeView::new(
        node,
        Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Desktop),
        WorkPolicy::for_class(DeviceClass::Desktop),
        START,
    );
    view.capabilities.cpu_cores = 8;
    view
}

fn config() -> DetectorConfig {
    DetectorConfig {
        probe_interval: Millis(1_000),
        probe_timeout: Millis(200),
        indirect_peers: 3,
        suspect_timeout: Millis(5_000),
    }
}

/// A blob store in a `HashMap`, and a switch for "this device is not taking pushes".
#[derive(Debug, Default)]
struct TestBlobs {
    held: std::sync::Mutex<std::collections::HashMap<BlobHash, Vec<u8>>>,
    refuses: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl Blobs for TestBlobs {
    async fn has(&self, hash: BlobHash) -> bool {
        self.held.lock().expect("lock").contains_key(&hash)
    }
    async fn get(&self, hash: BlobHash) -> Option<Vec<u8>> {
        self.held.lock().expect("lock").get(&hash).cloned()
    }
    async fn store(&self, bytes: Vec<u8>) -> Result<BlobHash, String> {
        let hash = BlobHash::from_bytes(*blake3::hash(&bytes).as_bytes());
        self.held.lock().expect("lock").insert(hash, bytes);
        Ok(hash)
    }
    fn accepts_push(&self, _hash: BlobHash, _size: u64) -> bool {
        !self.refuses.load(Ordering::SeqCst)
    }
}

/// One node, wired up and listening.
struct Member {
    cluster: Arc<Cluster>,
    membership: Arc<TestNode>,
    blobs: Arc<TestBlobs>,
}

impl Member {
    fn status_of(&self, other: NodeId) -> Option<NodeStatus> {
        self.cluster.view().node(&other).map(|n| n.status)
    }

    fn set_clock(&self, now: Millis) {
        self.membership.set(now);
    }
}

fn join(net: &MemoryNetwork, key: &FleetKey, seed: u8, name: &str) -> Member {
    join_as(net, key, seed, name, None)
}

/// A member whose **config** name differs from the one on its certificate.
///
/// The ordinary case on a real fleet and one no other helper here can make: `join` names the
/// certificate and the view entry alike, so every test in this file has a node with one name and
/// none of them could see the merge lose the signed one. `offload init --name alpha` sets the
/// certificate; `[node] name` is a separate optional field that defaults to the machine's
/// hostname, so the two agree only if somebody wrote the name down twice.
fn join_named(
    net: &MemoryNetwork,
    key: &FleetKey,
    seed: u8,
    certified: &str,
    configured: &str,
) -> Member {
    let node = id(seed);
    let membership = TestNode::new(key, seed, certified);
    let transport: Arc<dyn Transport> =
        Arc::new(net.join(node, configured, membership.clone() as Arc<dyn Membership>));
    let blobs = Arc::new(TestBlobs::default());
    let cluster = Cluster::new(
        transport,
        node_view(node).named(configured),
        config(),
        membership.clone() as Arc<dyn Clock>,
        blobs.clone() as Arc<dyn Blobs>,
    );
    cluster.members_via(membership.clone() as Arc<dyn offload_cluster::Members>);
    tokio::spawn(cluster.clone().serve());
    Member {
        cluster,
        membership,
        blobs,
    }
}

/// A member whose certificate says something other than "founder": pass the terms the fleet
/// actually issued. `None` founds, which is what every node in the older tests is.
fn join_as(
    net: &MemoryNetwork,
    key: &FleetKey,
    seed: u8,
    name: &str,
    terms: Option<Terms>,
) -> Member {
    let node = id(seed);
    let membership = match terms {
        Some(terms) => TestNode::with_terms(key, seed, terms),
        None => TestNode::new(key, seed, name),
    };
    let transport: Arc<dyn Transport> =
        Arc::new(net.join(node, name, membership.clone() as Arc<dyn Membership>));
    let blobs = Arc::new(TestBlobs::default());
    let cluster = Cluster::new(
        transport,
        // Named, the way the daemon names its own view entry from config: a node's own name is
        // what it signs answers to a peer with (ADR-0017's `by`).
        node_view(node).named(name),
        config(),
        membership.clone() as Arc<dyn Clock>,
        blobs.clone() as Arc<dyn Blobs>,
    );
    cluster.members_via(membership.clone() as Arc<dyn offload_cluster::Members>);
    tokio::spawn(cluster.clone().serve());
    Member {
        cluster,
        membership,
        blobs,
    }
}

/// Point two nodes at each other, the way a seed list or mDNS would: an id, and a guess at
/// the rest.
fn introduce(a: &Member, b: &Member) {
    let (caps, policy) = (
        unknown_capabilities(),
        WorkPolicy::for_class(DeviceClass::Desktop),
    );
    a.cluster
        .introduce(b.cluster.node(), caps.clone(), policy.clone());
    b.cluster.introduce(a.cluster.node(), caps, policy);
}

fn unknown_capabilities() -> Capabilities {
    Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Desktop)
}

fn advance(members: &[&Member], to: Millis) {
    for member in members {
        member.set_clock(to);
    }
}

#[tokio::test]
async fn a_probe_teaches_both_ends_about_the_fleet() {
    // The base case, and the reason gossip rides on probes: one exchange and each side knows
    // everything the other did.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    let c = join(&net, &key, 3, "phone");

    // a knows b, b knows c, and nobody has introduced a to c.
    introduce(&a, &b);
    introduce(&b, &c);
    assert!(a.cluster.view().node(&c.cluster.node()).is_none());

    a.cluster.probe_round(START).await;

    // a learned about c from b's gossip, without ever being told.
    assert_eq!(a.status_of(c.cluster.node()), Some(NodeStatus::Alive));
    assert_eq!(b.status_of(a.cluster.node()), Some(NodeStatus::Alive));
}

/// A peer is shown by the name on its **certificate**, and a gossip round does not take it back.
///
/// Two names for one device, and which is right depends on who reads it: the config's is local
/// and defaults to the machine's *hostname*, the certificate's is what the fleet gave the device
/// and is signed. `main::report_membership` says exactly that at startup and warns when they
/// differ; `Cluster::record_name` writes the signed one at the handshake; ADR-0012 puts a name
/// on the certificate "for `offload nodes`".
///
/// And then the first merge that raised the peer's incarnation copied the gossiped name straight
/// over it — `merge_node` takes a peer's facts wholesale above its own incarnation, and an
/// incarnation rises whenever capabilities or policy change, which on a laptop is every probe.
/// Measured on two daemons with neither naming itself in `node.toml`, so both were `fedora`:
/// `offload nodes` showed the certified `bravo` for **25 seconds** and the hostname for ever
/// after.
///
/// No test could see it, because every helper in this file names the certificate and the view
/// entry alike — which is why `join_named` exists.
#[tokio::test]
async fn a_peer_is_shown_by_the_name_on_its_certificate_and_gossip_does_not_take_it_back() {
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join_named(&net, &key, 1, "alpha", "fedora");
    let b = join_named(&net, &key, 2, "bravo", "fedora");
    introduce(&a, &b);

    a.cluster.probe_round(START).await;
    let named = |m: &Member, peer| {
        m.cluster
            .view()
            .node(&peer)
            .map(offload_core::NodeView::display_name)
    };
    assert_eq!(named(&a, b.cluster.node()).as_deref(), Some("bravo"));
    assert_eq!(named(&b, a.cluster.node()).as_deref(), Some("alpha"));

    // The peer raises its own incarnation and gossips again — which is what every probe does on a
    // machine whose cpu load moves (`set_capabilities` bumps it). Before the fix this is the round
    // that replaced `bravo` with `fedora`, and it needed nothing else to go wrong.
    let mut changed = unknown_capabilities();
    changed.cpu_cores = 16;
    assert!(b
        .cluster
        .set_capabilities(changed, WorkPolicy::for_class(DeviceClass::Desktop)));
    let later = START + Millis(1_000);
    advance(&[&a, &b], later);
    a.cluster.probe_round(later).await;
    b.cluster.probe_round(later).await;

    assert_eq!(
        named(&a, b.cluster.node()).as_deref(),
        Some("bravo"),
        "a signed name must not be displaced by the peer's own claim about itself"
    );
    assert_eq!(named(&b, a.cluster.node()).as_deref(), Some("alpha"));
}

#[tokio::test]
async fn a_partitioned_node_is_suspected_and_then_confirmed_dead() {
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    a.cluster.probe_round(START).await;
    assert_eq!(a.status_of(b.cluster.node()), Some(NodeStatus::Alive));

    net.partition(a.cluster.node(), b.cluster.node());

    // Unreachable, with nobody else to ask: suspicion, not a conclusion.
    let t1 = START + Millis(1_000);
    advance(&[&a, &b], t1);
    a.cluster.probe_round(t1).await;
    assert_eq!(a.status_of(b.cluster.node()), Some(NodeStatus::Suspect));

    // Still only suspicion a moment before the timeout: `Suspect` exists to be argued with,
    // and shortening that window is how a suspended laptop becomes a migration.
    let t2 = t1 + Millis(4_999);
    advance(&[&a, &b], t2);
    a.cluster.probe_round(t2).await;
    assert_eq!(a.status_of(b.cluster.node()), Some(NodeStatus::Suspect));

    let t3 = t1 + Millis(5_000);
    advance(&[&a, &b], t3);
    a.cluster.probe_round(t3).await;
    assert_eq!(a.status_of(b.cluster.node()), Some(NodeStatus::Dead));
}

/// **A node nobody can dial is kept alive over the connection it opened.** A phone on mobile
/// data sits behind a carrier firewall that admits nothing inbound, and reaches the fleet only by
/// dialling out to its seed. Measured overnight on a Samsung phone, the laptop it had dialled, and a
/// second home node: the second node could not dial the phone, and its indirect probe through the
/// laptop failed too, because the laptop *dialled* the phone to answer it instead of using the
/// connection the phone had opened. So it declared the phone dead 1 772 times; the laptop adopted
/// each claim and the phone refuted each one (incarnation 3 790 by morning), orphaning whatever it
/// held every time.
#[tokio::test]
async fn a_node_nobody_can_dial_is_kept_alive_over_the_connection_it_opened() {
    let net = MemoryNetwork::new();
    let key = fleet();
    let laptop = join(&net, &key, 1, "laptop");
    let bravo = join(&net, &key, 2, "bravo");
    let phone = join(&net, &key, 3, "phone");
    introduce(&laptop, &bravo);
    introduce(&laptop, &phone);
    introduce(&bravo, &phone);
    net.firewall(phone.cluster.node());
    let p = phone.cluster.node();

    // The phone dials out to the laptop — the one connection it will ever have — and nobody
    // else hears from it directly for the rest of the test.
    for _ in 0..3 {
        phone.cluster.probe_round(START).await;
    }

    // Well past the suspect timeout, with the laptop and bravo both probing and nothing from
    // the phone itself.
    let mut now = START;
    for _ in 0..8 {
        now = now + Millis(1_000);
        advance(&[&laptop, &bravo, &phone], now);
        for _ in 0..3 {
            laptop.cluster.probe_round(now).await;
            bravo.cluster.probe_round(now).await;
        }
    }
    assert_eq!(
        laptop.status_of(p),
        Some(NodeStatus::Alive),
        "the laptop reaches the phone over the phone's own connection"
    );
    assert_eq!(
        bravo.status_of(p),
        Some(NodeStatus::Alive),
        "bravo cannot dial the phone, and the laptop can vouch for it"
    );
}

#[tokio::test]
async fn a_third_node_rescues_a_peer_from_one_bad_path() {
    // The reason for the indirect probe: a laptop on a flaky access point is unreachable from
    // here and perfectly fine from the desktop. Concluding from one path is how a working
    // node gets its runs migrated away.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    let c = join(&net, &key, 3, "phone");
    introduce(&a, &b);
    introduce(&a, &c);
    introduce(&b, &c);

    net.partition(a.cluster.node(), b.cluster.node());

    let t1 = START + Millis(1_000);
    advance(&[&a, &b, &c], t1);
    // Probe until the rotation picks b; the point is what happens then, not when.
    for _ in 0..4 {
        a.cluster.probe_round(t1).await;
    }

    assert_eq!(
        a.status_of(b.cluster.node()),
        Some(NodeStatus::Alive),
        "c could reach b, so b is not suspect"
    );
}

#[tokio::test]
async fn a_node_that_comes_back_is_alive_again_and_its_absence_is_on_the_record() {
    // Absence history is what the hold-down policy reads (ADR-0007): a node that always
    // comes back in ninety seconds has earned patience the next time it goes quiet.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    net.partition(a.cluster.node(), b.cluster.node());
    let t1 = START + Millis(1_000);
    advance(&[&a, &b], t1);
    a.cluster.probe_round(t1).await;
    assert_eq!(a.status_of(b.cluster.node()), Some(NodeStatus::Suspect));

    net.heal(a.cluster.node(), b.cluster.node());
    let t2 = t1 + Millis(2_000);
    advance(&[&a, &b], t2);
    a.cluster.probe_round(t2).await;

    let view = a.cluster.view();
    let b_view = view.node(&b.cluster.node()).expect("known");
    assert_eq!(b_view.status, NodeStatus::Alive);
    assert_eq!(b_view.absences, 1);
    assert_eq!(b_view.typical_absence, Some(Millis(2_000)));
}

#[tokio::test]
async fn a_node_refutes_a_suspicion_the_moment_it_hears_of_it() {
    // The other half of `Suspect` being refutable: a is wrong about b, and b says so with a
    // higher incarnation rather than by arguing.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    net.partition(a.cluster.node(), b.cluster.node());
    let t1 = START + Millis(1_000);
    advance(&[&a, &b], t1);
    a.cluster.probe_round(t1).await;
    assert_eq!(a.status_of(b.cluster.node()), Some(NodeStatus::Suspect));

    // The link comes back and a probes b, carrying its own stale suspicion of b along with
    // it — which is how the subject of a suspicion normally hears about it.
    net.heal(a.cluster.node(), b.cluster.node());
    let t2 = t1 + Millis(1_000);
    advance(&[&a, &b], t2);
    a.cluster.probe_round(t2).await;

    assert_eq!(
        b.cluster
            .view()
            .node(&b.cluster.node())
            .expect("itself")
            .incarnation,
        2,
        "b started at 1 and bumped it on hearing the suspicion"
    );
    assert_eq!(a.status_of(b.cluster.node()), Some(NodeStatus::Alive));
}

#[tokio::test]
async fn a_departure_is_believed_immediately_rather_than_detected() {
    // A node knows when it is leaving, so a shutdown should cost nobody a detection timeout.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);
    a.cluster.probe_round(START).await;

    a.cluster.announce_departure().await;

    assert_eq!(b.status_of(a.cluster.node()), Some(NodeStatus::Draining));
    // And nobody wastes probes on it afterwards.
    let t1 = START + Millis(1_000);
    advance(&[&a, &b], t1);
    b.cluster.probe_round(t1).await;
    assert_eq!(b.status_of(a.cluster.node()), Some(NodeStatus::Draining));
}

#[tokio::test]
async fn a_node_that_announced_its_departure_can_come_back() {
    // The other half of the test above, and it was missing: `probeable` excludes `Draining` and
    // `Departed` for a good reason, and *nothing revised it*. So an impolite death healed — a
    // `Dead` node is re-probed for ever, because that is how one comes back — while a node that
    // politely said it was leaving was gone from that peer's view until the peer itself
    // restarted. Measured on two daemons: 40 successful handshakes over four minutes, the peer
    // still `draining` and last heard `7m` ago, and never probed or gossiped to again.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);
    a.cluster.probe_round(START).await;
    a.cluster.announce_departure().await;
    assert_eq!(b.status_of(a.cluster.node()), Some(NodeStatus::Draining));

    // It comes back and dials: a completed handshake is first-hand contact, and it outranks what
    // b remembers about a departure a *previous* incarnation announced. Nothing else can heal it
    // — b will not probe a departed node, and a restarted node's gossip is behind the incarnation
    // its own previous life reached.
    let t1 = START + Millis(1_000);
    advance(&[&a, &b], t1);
    introduce(&a, &b);
    assert_eq!(
        b.status_of(a.cluster.node()),
        Some(NodeStatus::Alive),
        "a peer that just answered a handshake is here, whatever it announced before"
    );
    // And it is probed again, which is what makes the recovery stick.
    let t2 = t1 + Millis(1_000);
    advance(&[&a, &b], t2);
    b.cluster.probe_round(t2).await;
    assert_eq!(b.status_of(a.cluster.node()), Some(NodeStatus::Alive));
}

#[tokio::test]
async fn a_guess_about_a_peer_does_not_survive_meeting_it() {
    // Discovery supplies an id and whatever it could guess about the rest. If a placeholder
    // could outlive first contact, the fleet would place runs on capabilities nobody ever
    // reported — which is the failure mode `Capabilities` exists to prevent.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    assert_ne!(
        a.cluster
            .view()
            .node(&b.cluster.node())
            .expect("placeholder")
            .capabilities
            .cpu_cores,
        8,
        "a guess, and not b's real one"
    );

    a.cluster.probe_round(START).await;

    let view = a.cluster.view();
    let b_view = view.node(&b.cluster.node()).expect("known");
    assert_eq!(b_view.capabilities.cpu_cores, 8, "b's own report");
    assert_eq!(b_view.incarnation, 1);
}

#[tokio::test]
async fn what_a_node_is_running_reaches_the_rest_of_the_fleet() {
    // Phase 4 places work by asking who is busy, and this is where that answer comes from.
    // The incarnation bump is the load-bearing part: peers merge a node's self-reported facts
    // on it, so a workload that changed without one would be discarded as stale.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);
    a.cluster.probe_round(START).await;

    let run = offload_core::RunId::from_bytes([9; 16]);
    assert!(a.cluster.set_running([run].into_iter().collect()));
    // Saying the same thing again is not news, and would burn an incarnation per second.
    assert!(!a.cluster.set_running([run].into_iter().collect()));

    let t1 = START + Millis(1_000);
    advance(&[&a, &b], t1);
    a.cluster.probe_round(t1).await;

    let view = b.cluster.view();
    let a_view = view.node(&a.cluster.node()).expect("known");
    assert!(a_view.running.contains(&run));
    assert_eq!(a_view.incarnation, 2, "one bump for the workload");
}

/// An answer that reached this node from a peer: which run, which call, and who said so.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Answered {
    run: offload_core::RunId,
    tool_use_id: Option<String>,
    allow: bool,
    by: String,
}

/// A node that answers bids however the test wants, and records what it was granted.
#[derive(Debug)]
struct TestHost {
    /// `None` means "I will not take it", with the reason.
    bid: std::sync::Mutex<Result<i64, String>>,
    accepted: std::sync::Mutex<Vec<offload_core::RunId>>,
    /// Every grant this node was *offered*, whether or not it took it, with the epoch it came
    /// under. An epoch is the fencing token for one grant, so two attempts in one round must
    /// not share one — a node whose answer was lost is running under the first.
    offered: std::sync::Mutex<Vec<(offload_core::RunId, offload_core::Epoch)>>,
    recorded: std::sync::Mutex<Vec<offload_core::Run>>,
    /// Edits to runs this node is *holding*, which arrive as their own callback: a peer's
    /// record of such a run is refused, and the two fields on it the home node owns are not.
    edited: std::sync::Mutex<Vec<offload_core::Run>>,
    /// What this node was told about runs it does not hold — the durable half of learning
    /// somebody else's numbers.
    learned: std::sync::Mutex<Vec<(offload_core::RunId, offload_core::RunProgress)>>,
    /// Bid happily, then refuse the grant — the case ADR-0006 step 6 exists for.
    decline_grant: std::sync::atomic::AtomicBool,
    /// Whether this node could start now, or is committing to a run it has no room for.
    available: std::sync::Mutex<offload_core::Availability>,
    /// Deadline changes forwarded here because this node owns the field.
    deadlines: std::sync::Mutex<Vec<(offload_core::RunId, offload_core::SpecEdit)>>,
    refuse_deadline: std::sync::atomic::AtomicBool,
    /// A run's log, as this node would have written it, plus who has asked for it.
    log: std::sync::Mutex<Vec<offload_core::LogEvent>>,
    pulled_by: std::sync::Mutex<Vec<NodeId>>,
    /// Questions this node's agents are blocked on, and the answers that arrived (ADR-0017).
    waiting: std::sync::Mutex<Vec<offload_core::PendingAsk>>,
    answers: std::sync::Mutex<Vec<Answered>>,
    /// Cancels forwarded here because the run is on this node, with the name of the machine
    /// they were typed at.
    cancelled: std::sync::Mutex<Vec<(offload_core::RunId, String)>>,
    /// Answer a cancel with "there is nothing here to stop" — the race where the run finished
    /// while the command was crossing the network.
    refuse_cancel: std::sync::atomic::AtomicBool,
    /// Checkpoint requests forwarded here because the agent is on this node.
    checkpointing: std::sync::Mutex<Vec<offload_core::RunId>>,
    /// Runs a peer sent as live that this node knows finished.
    stale: std::sync::Mutex<Vec<offload_core::RunId>>,
}

impl TestHost {
    fn answering(bid: Result<i64, String>) -> Arc<TestHost> {
        Arc::new(TestHost {
            bid: std::sync::Mutex::new(bid),
            accepted: std::sync::Mutex::new(Vec::new()),
            offered: std::sync::Mutex::new(Vec::new()),
            recorded: std::sync::Mutex::new(Vec::new()),
            edited: std::sync::Mutex::new(Vec::new()),
            learned: std::sync::Mutex::new(Vec::new()),
            decline_grant: std::sync::atomic::AtomicBool::new(false),
            available: std::sync::Mutex::new(offload_core::Availability::Now),
            deadlines: std::sync::Mutex::new(Vec::new()),
            refuse_deadline: std::sync::atomic::AtomicBool::new(false),
            log: std::sync::Mutex::new(Vec::new()),
            pulled_by: std::sync::Mutex::new(Vec::new()),
            waiting: std::sync::Mutex::new(Vec::new()),
            answers: std::sync::Mutex::new(Vec::new()),
            cancelled: std::sync::Mutex::new(Vec::new()),
            refuse_cancel: std::sync::atomic::AtomicBool::new(false),
            checkpointing: std::sync::Mutex::new(Vec::new()),
            stale: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn bidding(score: i64) -> Arc<TestHost> {
        TestHost::answering(Ok(score))
    }

    fn refusing(reason: &str) -> Arc<TestHost> {
        TestHost::answering(Err(reason.to_string()))
    }
}

#[async_trait::async_trait]
impl offload_cluster::Host for TestHost {
    async fn evaluate(&self, _run: &offload_core::Run) -> Result<offload_core::Offer, String> {
        let available = *self.available.lock().expect("lock");
        self.bid
            .lock()
            .expect("lock")
            .clone()
            .map(|score| offload_core::Offer {
                score: offload_core::Score(score),
                available,
                terms: None,
            })
    }

    async fn accept(&self, run: offload_core::Run) -> Result<(), String> {
        self.offered.lock().expect("lock").push((run.id, run.epoch));
        if self.decline_grant.load(Ordering::SeqCst) {
            return Err("busy since bidding".into());
        }
        self.accepted.lock().expect("lock").push(run.id);
        Ok(())
    }

    async fn record(&self, run: &offload_core::Run) {
        self.recorded.lock().expect("lock").push(run.clone());
    }

    async fn stale_copy(&self, run: offload_core::RunId) {
        self.stale.lock().expect("lock").push(run);
    }

    async fn record_spec_edit(&self, run: &offload_core::Run) {
        self.edited.lock().expect("lock").push(run.clone());
    }

    async fn record_progress(
        &self,
        run: offload_core::RunId,
        progress: &offload_core::RunProgress,
    ) {
        self.learned
            .lock()
            .expect("lock")
            .push((run, progress.clone()));
    }

    async fn events_since(
        &self,
        peer: NodeId,
        _run: offload_core::RunId,
        after: u64,
        limit: u32,
    ) -> Result<(Vec<offload_proto::cluster::SeqEvent>, bool), String> {
        self.pulled_by.lock().expect("lock").push(peer);
        let log = self.log.lock().expect("lock");
        let page: Vec<offload_proto::cluster::SeqEvent> = log
            .iter()
            .enumerate()
            .map(|(i, event)| offload_proto::cluster::SeqEvent {
                seq: i as u64 + 1,
                event: event.clone(),
            })
            .filter(|se| se.seq > after)
            .take(limit as usize)
            .collect();
        let done = page.last().is_some_and(|se| se.event.is_terminal());
        Ok((page, done))
    }

    async fn edit_spec(
        &self,
        run: offload_core::RunId,
        edit: offload_core::SpecEdit,
    ) -> Result<(u32, String), String> {
        if self.refuse_deadline.load(Ordering::SeqCst) {
            return Err("it has already completed".into());
        }
        let mut edits = self.deadlines.lock().expect("lock");
        edits.push((run, edit));
        Ok((
            u32::try_from(edits.len()).unwrap_or(u32::MAX),
            "noted".into(),
        ))
    }

    async fn answer(
        &self,
        run: offload_core::RunId,
        tool_use_id: Option<String>,
        allow: bool,
        by: &str,
    ) -> Result<(String, String, bool), String> {
        let mut waiting = self.waiting.lock().expect("lock");
        let found = waiting
            .iter()
            .position(|ask| {
                ask.run == run && tool_use_id.as_ref().is_none_or(|id| &ask.tool_use_id == id)
            })
            .ok_or_else(|| "nothing here is waiting for that".to_string())?;
        let ask = waiting.remove(found);
        self.answers.lock().expect("lock").push(Answered {
            run,
            tool_use_id,
            allow,
            by: by.to_string(),
        });
        Ok((ask.tool, ask.detail, allow))
    }

    async fn cancel(&self, run: offload_core::RunId, by: &str) -> Result<String, String> {
        if self.refuse_cancel.load(Ordering::SeqCst) {
            return Err("it is already completed".into());
        }
        self.cancelled
            .lock()
            .expect("lock")
            .push((run, by.to_string()));
        Ok("its agent was stopped".into())
    }

    async fn request_checkpoint(&self, run: offload_core::RunId) -> Result<(), String> {
        self.checkpointing.lock().expect("lock").push(run);
        Ok(())
    }

    async fn pending_asks(&self) -> Vec<offload_core::PendingAsk> {
        self.waiting.lock().expect("lock").clone()
    }
}

/// The run ids a host was told to write down, in order.
fn recorded_ids(host: &Arc<TestHost>) -> Vec<offload_core::RunId> {
    host.recorded
        .lock()
        .expect("lock")
        .iter()
        .map(|run| run.id)
        .collect()
}

fn a_run(submitter: NodeId) -> offload_core::Run {
    use offload_core::{
        AgentKind, AgentWork, Constraint, PermissionMode, Restartability, RunId, RunSpec,
        ToolAllowlist, Work, WorkspaceSpec,
    };
    offload_core::Run::new(
        RunId::from_bytes([7; 16]),
        RunSpec {
            work: Work::Agent(AgentWork {
                agent: AgentKind::ClaudeCode,
                model: None,
                prompt: "add tests for the parser".into(),
                workspace: WorkspaceSpec {
                    repo: "https://example.com/me/api.git".into(),
                    archive_bytes: None,
                    git_ref: None,
                    branch: None,
                },
                permission_mode: PermissionMode::AcceptEdits,
                allow: ToolAllowlist::default(),
                max_turns: None,
                ask: offload_core::AskPolicy::Never,
            }),
            constraint: Constraint::Always,
            restartability: Restartability::Resumable,
            priority: 0,
            queue: false,
            deadline: None,
            demand: Default::default(),
            notify: Default::default(),
            notices: Default::default(),
            resources: Vec::new(),
            prefer: offload_core::Constraint::Always,
            hold_until: None,
            parent: None,
        },
        submitter,
        START,
    )
}

const WINDOW: Millis = Millis(500);

/// This node's own answer in a round, for the common case of "yes, and I can start now".
fn offer(score: i64) -> offload_core::Offer {
    offload_core::Offer {
        score: offload_core::Score(score),
        available: offload_core::Availability::Now,
        terms: None,
    }
}

#[tokio::test]
async fn the_keenest_node_takes_the_run() {
    // ADR-0006's whole point: the decision is made where the information is, and the arbiter
    // only picks among self-assessments.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let host_b = TestHost::bidding(90);
    b.cluster.hosts_runs(host_b.clone());
    let host_a = TestHost::bidding(10);
    a.cluster.hosts_runs(host_a.clone());

    let run = a_run(a.cluster.node());
    let placed = a.cluster.place(&run, Ok(offer(10)), WINDOW).await;

    assert_eq!(placed.accepted_by(), Some(b.cluster.node()));
    assert_eq!(host_b.accepted.lock().expect("lock").as_slice(), [run.id]);
    assert!(host_a.accepted.lock().expect("lock").is_empty());
    // The submitting node remembers where it went, because a record held only in memory is
    // gone at the next restart — and that node is the one about to be shut.
    assert_eq!(recorded_ids(&host_a), [run.id]);
}

#[tokio::test]
async fn two_rounds_for_one_run_on_one_node_are_one_round() {
    // ADR-0050. An epoch is monotonic per *arbiter*, and an arbiter is a node — so two rounds
    // running at once on one node for one run read `run.epoch` from two copies that both say
    // *N*, both grant *N+1*, and no fence downstream can tell the two legs apart, because
    // neither of them is stale.
    //
    // Found on two daemons rather than here: a drain's own round and the supervise tick's
    // round for the same released run, 1.4ms apart, two `granted to <bravo> at epoch 3` rows
    // in `offload audit`, and two concurrent `git worktree add`. What stopped it being two
    // agents was git's branch-name collision, and the leg that collided wrote `Failed` first —
    // so the healthy leg was fenced out by the loser and a clean migration ended `failed`.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let host_b = TestHost::bidding(90);
    b.cluster.hosts_runs(host_b.clone());
    a.cluster.hosts_runs(TestHost::bidding(10));

    let run = a_run(a.cluster.node());
    let (first, second) = tokio::join!(
        a.cluster.place(&run, Ok(offer(10)), WINDOW),
        a.cluster.place(&run, Ok(offer(10)), WINDOW),
    );

    // One grant, spending one token, put to the winner once.
    let offered = host_b.offered.lock().expect("lock").clone();
    assert_eq!(offered.len(), 1, "one round, one grant: {offered:?}");
    assert_eq!(host_b.accepted.lock().expect("lock").as_slice(), [run.id]);

    // And the call that stood down is told what the round that ran decided, rather than a
    // shrug it would have to interpret. The drain is the caller this matters to: it counts a
    // handover it did not itself perform, and telling it "refused" would make `offload drain`
    // report that nobody took a run that had just left.
    assert_eq!(first.accepted_by(), Some(b.cluster.node()));
    assert_eq!(second.accepted_by(), Some(b.cluster.node()));
}

#[tokio::test]
async fn the_submitting_node_keeps_a_run_it_wants_most() {
    // The common case, and the one that needs no network at all: the workspace is warm here.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let host_a = TestHost::bidding(0);
    a.cluster.hosts_runs(host_a.clone());
    b.cluster.hosts_runs(TestHost::bidding(5));

    let run = a_run(a.cluster.node());
    let placed = a.cluster.place(&run, Ok(offer(99)), WINDOW).await;

    assert_eq!(placed.accepted_by(), Some(a.cluster.node()));
    assert_eq!(host_a.accepted.lock().expect("lock").as_slice(), [run.id]);
}

#[tokio::test]
async fn nobody_taking_it_produces_every_reason_rather_than_a_shrug() {
    // ADR-0014: a submission is accepted by a node or refused to your face, and the refusal
    // has to say which node said what — "no eligible nodes" sends somebody guessing.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    let c = join(&net, &key, 3, "phone");
    introduce(&a, &b);
    introduce(&a, &c);

    b.cluster
        .hosts_runs(TestHost::refusing("busy, free in ~40m"));
    c.cluster
        .hosts_runs(TestHost::refusing("battery 22%, policy floor is 40%"));

    let run = a_run(a.cluster.node());
    let placed = a
        .cluster
        .place(&run, Err("no agent installed".into()), WINDOW)
        .await;

    let offload_cluster::Placement::Refused { refusals, spent } = placed else {
        panic!("somebody took it");
    };
    let reasons: Vec<&str> = refusals.iter().map(|r| r.reason.as_str()).collect();
    assert!(reasons.contains(&"no agent installed"));
    assert!(reasons.contains(&"busy, free in ~40m"));
    assert!(reasons.contains(&"battery 22%, policy floor is 40%"));
    // Nothing was granted, so nothing was spent, and the caller's own copy is still the truth
    // about this run. `None` rather than a copy of what came in: a round that handed nothing out
    // has no record to hand back, and inventing one would put a run in the view that the
    // operator is about to be told nobody took.
    assert!(
        spent.is_none(),
        "a round with no bidders spent no tokens and has nothing to hand back"
    );
    assert!(
        !a.cluster.view().runs.contains_key(&run.id),
        "…and published nothing either"
    );
}

#[tokio::test]
async fn a_node_that_declines_its_grant_does_not_strand_the_run() {
    // A bid describes a moment and the grant arrives later. ADR-0006 step 6: the arbiter
    // tries the next-best bid, and the decliner is not reconsidered in this round.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    let c = join(&net, &key, 3, "server");
    introduce(&a, &b);
    introduce(&a, &c);

    let keen_but_gone = TestHost::bidding(90);
    keen_but_gone.decline_grant.store(true, Ordering::SeqCst);
    b.cluster.hosts_runs(keen_but_gone.clone());
    let host_c = TestHost::bidding(50);
    c.cluster.hosts_runs(host_c.clone());
    a.cluster.hosts_runs(TestHost::refusing("no agent"));

    let run = a_run(a.cluster.node());
    let placed = a.cluster.place(&run, Err("no agent".into()), WINDOW).await;

    assert_eq!(placed.accepted_by(), Some(c.cluster.node()));
    assert_eq!(host_c.accepted.lock().expect("lock").as_slice(), [run.id]);
}

#[tokio::test]
async fn a_round_that_failed_does_not_hand_its_tokens_out_again() {
    // The test below, one level out — and the level the fix stopped at. Threading one record
    // through a round stops an epoch being spent twice *inside* it; nothing carried that across
    // the boundary. `place` works on a local copy and publishes only a **confirmed** grant, so a
    // round in which nobody confirmed left this node's view at the epoch it started from, and
    // the next round began counting from there and handed the same numbers out again.
    //
    // Which matters for exactly the reason the within-a-round version does: silence is a decline
    // (ADR-0006), so a node that took the grant and lost the answer is running under a token the
    // arbiter is about to give to somebody else. Two grants at one epoch from one arbiter is the
    // thing the epoch exists to make impossible — `fence` cannot order them, so neither agent
    // ever learns it lost.
    //
    // Found by `tests/storm.rs`, which generated a round where three nodes swallowed their
    // grants in turn and the arbiter then re-offered from a view that had never moved.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let host_b = TestHost::bidding(90);
    host_b.decline_grant.store(true, Ordering::SeqCst);
    b.cluster.hosts_runs(host_b.clone());
    let host_a = TestHost::refusing("no agent");
    a.cluster.hosts_runs(host_a.clone());

    let run = a_run(a.cluster.node());
    let first = a.cluster.place(&run, Err("no agent".into()), WINDOW).await;
    let offload_cluster::Placement::Refused { spent, .. } = first else {
        panic!("the only bidder declined, so the round failed");
    };

    let spent_epochs: Vec<offload_core::run::Epoch> = host_b
        .offered
        .lock()
        .expect("lock")
        .iter()
        .map(|(_, epoch)| *epoch)
        .collect();
    assert!(!spent_epochs.is_empty(), "b was offered the run");

    // What the arbiter re-offers from is its own record, which is what `supervise` does when it
    // comes back to a run nobody took — so the record is where the spent tokens have to be, and
    // its absence is the bug rather than a reason to stop the test.
    let again = a
        .cluster
        .view()
        .runs
        .get(&run.id)
        .cloned()
        .unwrap_or_else(|| run.clone());
    assert!(
        again.epoch > run.epoch,
        "the arbiter went back to a record at epoch {:?} after spending {spent_epochs:?}",
        again.epoch
    );
    // **And the outcome carries it too, which is the half publishing alone does not buy.** The
    // caller holds a copy of the run from *before* the round; `server::place`'s queue branch
    // wrote that copy to the store and back into this view, at the epoch the round had already
    // spent — undoing the publish above from one line up the stack. So the record leaves the
    // round on the decision as well as in the view, and the two have to agree, or the caller and
    // the arbiter are reading two answers.
    assert_eq!(
        spent.map(|spent| *spent),
        Some(again.clone()),
        "a refused round that spent tokens hands back the record it published"
    );
    // **And it is written down, not merely published.** A `ClusterView` is memory: an arbiter
    // that published the spent epoch and did not record it came back from a restart at the epoch
    // it had started from, and the next round handed the same numbers out again — the bug this
    // test is named for, reached through the one door the publish does not cover.
    assert_eq!(
        host_a
            .recorded
            .lock()
            .expect("lock")
            .last()
            .map(|run| run.epoch),
        Some(again.epoch),
        "the arbiter recorded the tokens it spent, so a restart does not un-spend them"
    );
    host_b.decline_grant.store(false, Ordering::SeqCst);
    let second = a
        .cluster
        .place(&again, Err("no agent".into()), WINDOW)
        .await;
    assert_eq!(second.accepted_by(), Some(b.cluster.node()));

    let taken = host_b
        .offered
        .lock()
        .expect("lock")
        .last()
        .copied()
        .expect("b took it the second time")
        .1;
    assert!(
        spent_epochs.iter().all(|e| taken > *e),
        "the second round handed out {taken:?}, which the first round had already spent: \
         {spent_epochs:?}"
    );
}

#[tokio::test]
async fn a_grant_nobody_confirmed_does_not_hand_the_next_node_the_same_token() {
    // The same round as the test above, asked about the fencing token rather than about who
    // ended up with the run. `hand_over` used to re-clone the *original* record for each
    // attempt, so every node in one round was granted the run at the same epoch — and a
    // decline is not proof the grant never arrived. ADR-0006 says silence is a decline, so the
    // ordinary case is a node that took the grant, started the agent, and whose `Granted` was
    // lost: it is running under exactly the epoch the next node is then given, and `fence`
    // calls neither of them stale.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    let c = join(&net, &key, 3, "server");
    introduce(&a, &b);
    introduce(&a, &c);

    let host_b = TestHost::bidding(90);
    host_b.decline_grant.store(true, Ordering::SeqCst);
    b.cluster.hosts_runs(host_b.clone());
    let host_c = TestHost::bidding(50);
    c.cluster.hosts_runs(host_c.clone());
    a.cluster.hosts_runs(TestHost::refusing("no agent"));

    let run = a_run(a.cluster.node());
    let placed = a.cluster.place(&run, Err("no agent".into()), WINDOW).await;
    assert_eq!(placed.accepted_by(), Some(c.cluster.node()));

    let refused = *host_b
        .offered
        .lock()
        .expect("lock")
        .first()
        .expect("b was offered it");
    let taken = *host_c
        .offered
        .lock()
        .expect("lock")
        .first()
        .expect("c took it");
    assert_eq!(refused.0, run.id);
    assert!(
        taken.1 > refused.1,
        "both nodes were granted the run under {refused:?} and {taken:?} — one token, two \
         holders, and nothing able to tell them apart"
    );
}

#[tokio::test]
async fn a_bid_from_a_node_the_fleet_never_granted_host_runs_is_refused() {
    // ADR-0012's other half: grants live on the certificate so a device cannot promote itself
    // by editing its own config — and the place that has to be true is the bid exchange,
    // because an honest node never bids without the grant. This one does.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join_as(
        &net,
        &key,
        2,
        "phone",
        Some(Terms::joining(key.id(), id(2), "phone", START)),
    );
    introduce(&a, &b);

    let liar = TestHost::bidding(99);
    b.cluster.hosts_runs(liar.clone());

    let run = a_run(a.cluster.node());
    let placed = a.cluster.place(&run, Err("no agent".into()), WINDOW).await;

    let offload_cluster::Placement::Refused { refusals, .. } = placed else {
        panic!("a node the fleet never granted host-runs took a run");
    };
    assert!(liar.accepted.lock().expect("lock").is_empty());
    assert!(
        refusals
            .iter()
            .any(|r| r.reason.contains("does not grant host-runs")),
        "the refusal should name the missing grant: {refusals:?}"
    );
}

#[tokio::test]
async fn probation_holds_a_bid_back_and_lifts_without_anybody_redialling() {
    // The certificate travels with the session and is asked at each decision, so a grant that
    // is merely dormant starts working the moment probation lapses — no re-handshake, no
    // restart, nothing gossiped (ADR-0012).
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let mut terms = Terms::joining(key.id(), id(2), "desktop", START);
    terms.grants.insert(Grant::HostRuns);
    let b = join_as(&net, &key, 2, "desktop", Some(terms));
    introduce(&a, &b);

    let host_b = TestHost::bidding(99);
    b.cluster.hosts_runs(host_b.clone());

    let run = a_run(a.cluster.node());
    let placed = a.cluster.place(&run, Err("no agent".into()), WINDOW).await;
    let offload_cluster::Placement::Refused { refusals, .. } = placed else {
        panic!("a node still on probation took a run");
    };
    assert!(
        refusals.iter().any(|r| r.reason.contains("probation")),
        "the refusal should say the grant is dormant, and for how long: {refusals:?}"
    );

    advance(&[&a, &b], START + PROBATION);
    let placed = a.cluster.place(&run, Err("no agent".into()), WINDOW).await;
    assert_eq!(placed.accepted_by(), Some(b.cluster.node()));
    assert_eq!(host_b.accepted.lock().expect("lock").as_slice(), [run.id]);
}

#[tokio::test]
async fn a_grant_issued_mid_connection_reaches_the_next_round() {
    // The staleness that runs the other way: `offload grant host-runs` mints a new
    // certificate, and the live connection still carries the old one. Refusing the bid also
    // drops that connection, so the next round re-handshakes and believes the new papers —
    // within the arbiter's own retry, rather than whenever the link happens to break.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join_as(
        &net,
        &key,
        2,
        "desktop",
        Some(Terms::joining(key.id(), id(2), "desktop", START)),
    );
    introduce(&a, &b);

    let host_b = TestHost::bidding(99);
    b.cluster.hosts_runs(host_b.clone());

    let run = a_run(a.cluster.node());
    let placed = a.cluster.place(&run, Err("no agent".into()), WINDOW).await;
    assert_eq!(placed.accepted_by(), None, "the old certificate says no");

    // The owner grants it: a deliberate act on the desktop, invite-style, so no probation.
    let mut granted = Terms::joining(key.id(), id(2), "desktop", START);
    granted.grants.insert(Grant::HostRuns);
    granted.probation = false;
    b.membership.reissue(&key, granted);

    let placed = a.cluster.place(&run, Err("no agent".into()), WINDOW).await;
    assert_eq!(placed.accepted_by(), Some(b.cluster.node()));
    assert_eq!(host_b.accepted.lock().expect("lock").as_slice(), [run.id]);
}

/// …and the same when it was the **peer** that dialled. Since the arbiter reuses a peer's own
/// connection rather than dialling it (the phone behind a carrier firewall), refusing the bid has
/// to hang up that connection too, or the next round asks the old certificate again for as long as
/// the peer keeps its connection up.
#[tokio::test]
async fn a_grant_issued_mid_connection_reaches_the_next_round_when_the_peer_dialled() {
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join_as(
        &net,
        &key,
        2,
        "desktop",
        Some(Terms::joining(key.id(), id(2), "desktop", START)),
    );
    introduce(&a, &b);
    // The desktop dials the laptop, so the laptop's only session with it is inbound.
    b.cluster.probe_round(START).await;

    let host_b = TestHost::bidding(99);
    b.cluster.hosts_runs(host_b.clone());

    // Nobody can dial the desktop, so every round has to go over a connection it opened.
    net.firewall(b.cluster.node());

    let run = a_run(a.cluster.node());
    let placed = a.cluster.place(&run, Err("no agent".into()), WINDOW).await;
    // The reason, not only the outcome: this passed for months on a refusal that said the
    // connection had closed — the certificate lookup that missed inbound sessions.
    let offload_cluster::Placement::Refused { refusals, .. } = placed else {
        panic!("the old certificate should say no");
    };
    assert!(
        refusals.iter().any(|r| r.reason.contains("host-runs")),
        "refused for the old certificate: {refusals:?}"
    );

    let mut granted = Terms::joining(key.id(), id(2), "desktop", START);
    granted.grants.insert(Grant::HostRuns);
    granted.probation = false;
    b.membership.reissue(&key, granted);
    // The refusal hung up both ways; the desktop dials back, as its next probe would.
    b.cluster.probe_round(START).await;

    let placed = a.cluster.place(&run, Err("no agent".into()), WINDOW).await;
    assert_eq!(placed.accepted_by(), Some(b.cluster.node()), "{placed:?}");
    assert_eq!(host_b.accepted.lock().expect("lock").as_slice(), [run.id]);
}

/// A host nobody can dial wins a round over the connection it opened. The bid arrived on that
/// session, and its certificate was then looked up only among the sessions this node dialled —
/// found nowhere, refused as "connection closed before its certificate could be checked", and
/// hung up on. On a LAN the next round dials out and it looks like a flake; a phone on a carrier
/// firewall could never host (session ninety-two, the iOS Simulator after a peer restart).
#[tokio::test]
async fn a_host_nobody_can_dial_wins_a_round_over_its_own_connection() {
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "phone");
    introduce(&a, &b);
    net.firewall(b.cluster.node());
    b.cluster.probe_round(START).await;

    let host_b = TestHost::bidding(99);
    b.cluster.hosts_runs(host_b.clone());

    let run = a_run(a.cluster.node());
    let placed = a.cluster.place(&run, Err("no agent".into()), WINDOW).await;
    assert_eq!(placed.accepted_by(), Some(b.cluster.node()));
    assert_eq!(host_b.accepted.lock().expect("lock").as_slice(), [run.id]);
}

#[tokio::test]
async fn a_node_that_has_gone_quiet_costs_the_round_its_window_and_nothing_more() {
    // Somebody is standing at the keyboard while this happens. A partitioned node must not
    // turn a submission into a wait.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);
    b.cluster.hosts_runs(TestHost::bidding(90));
    let host_a = TestHost::bidding(1);
    a.cluster.hosts_runs(host_a.clone());

    net.partition(a.cluster.node(), b.cluster.node());

    let run = a_run(a.cluster.node());
    let placed = a.cluster.place(&run, Ok(offer(1)), WINDOW).await;

    assert_eq!(
        placed.accepted_by(),
        Some(a.cluster.node()),
        "the only node that answered"
    );
    assert_eq!(host_a.accepted.lock().expect("lock").as_slice(), [run.id]);
}

#[tokio::test]
async fn a_submission_this_node_keeps_is_written_down_somewhere_else_first() {
    // The overnight case at the one moment it is fragile. A run placed on a peer is already on
    // a second machine; a run this node kept is on exactly one, and the next thing that
    // happens to a laptop at 23:00 is that it closes. No new message does this: a probe
    // carries the whole view and a peer writes down what it learns *before* it answers, so the
    // `Ack` is the receipt.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let host_b = TestHost::bidding(1);
    b.cluster.hosts_runs(host_b.clone());
    a.cluster.hosts_runs(TestHost::bidding(1));

    let mut run = a_run(a.cluster.node());
    let epoch = run
        .assign(a.cluster.node(), START, Millis(60_000))
        .expect("assign");
    run.started(a.cluster.node(), epoch, START).expect("start");

    let copy = a.cluster.confirm_record(&run, WINDOW).await;
    assert_eq!(copy, Some(b.cluster.node()));
    // Written down on the peer, not merely believed: a record held in view memory is gone at
    // the next restart, which is the same event as the laptop closing.
    assert_eq!(recorded_ids(&host_b), [run.id]);

    // And a fleet with nobody else awake says so rather than pretending.
    net.partition(a.cluster.node(), b.cluster.node());
    let alone = a.cluster.confirm_record(&run, WINDOW).await;
    assert_eq!(alone, None);
}

#[tokio::test]
async fn a_grant_counts_against_the_winner_before_it_has_gossiped() {
    // Six submissions in two seconds all landed on one node while the other sat idle: every
    // round after the first was reading a view taken before the previous grant, because the
    // arbiter waited for the winner to gossip its own new workload. Being at capacity is
    // precisely the fact a bid round exists to act on, so the arbiter writes down what it
    // just did.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    b.cluster.hosts_runs(TestHost::bidding(90));
    a.cluster.hosts_runs(TestHost::bidding(10));

    let run = a_run(a.cluster.node());
    let placed = a.cluster.place(&run, Ok(offer(10)), WINDOW).await;
    assert_eq!(placed.accepted_by(), Some(b.cluster.node()));

    // The arbiter's own view knows immediately, without a probe round in between.
    let view = a.cluster.view();
    assert_eq!(
        view.running_count(&b.cluster.node()),
        1,
        "the next round has to see that the winner just took one"
    );
    assert_eq!(
        view.runs.get(&run.id).and_then(offload_core::Run::holder),
        Some(b.cluster.node())
    );
}

#[tokio::test]
async fn a_canvass_keeps_every_answer_including_the_silence_and_grants_nothing() {
    // What `offload explain` is: the same round a submission runs, with nobody granted
    // anything. The three kinds of answer stay distinguishable — a bid, a reason, and a node
    // that said nothing — because "would not" and "did not answer" are different facts and
    // reporting the second as the first puts words in a node's mouth.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    let c = join(&net, &key, 3, "phone");
    let d = join(&net, &key, 4, "server");
    introduce(&a, &b);
    introduce(&a, &c);
    introduce(&a, &d);

    let desktop = TestHost::bidding(90);
    b.cluster.hosts_runs(desktop.clone());
    c.cluster
        .hosts_runs(TestHost::refusing("battery 22%, policy floor is 40%"));
    let server = TestHost::bidding(50);
    d.cluster.hosts_runs(server.clone());
    net.partition(a.cluster.node(), d.cluster.node());

    let run = a_run(a.cluster.node());
    let opinions = a.cluster.canvass(&run, Ok(offer(10)), WINDOW).await;

    let said: Vec<(NodeId, String)> = opinions
        .iter()
        .map(|o| (o.node, o.verdict.to_string()))
        .collect();
    assert_eq!(
        said,
        vec![
            // Best first, by the same precedence `winner()` uses, then the refusals, then
            // whoever did not answer.
            (b.cluster.node(), "bid 90, starting now".to_string()),
            (a.cluster.node(), "bid 10, starting now".to_string()),
            (
                c.cluster.node(),
                "battery 22%, policy floor is 40%".to_string()
            ),
            // Never asked, and the transport's reason carried: a connection this node could
            // not make is a different thing to go and fix from a peer that went quiet on one.
            (
                d.cluster.node(),
                "not reachable from this node: partitioned".to_string()
            ),
        ]
    );
    // A *peer* met but not yet gossiped about is shown by its id rather than by an empty
    // column: `offload nodes` makes the same choice for the same reason. This node knows its
    // own name, because it named itself.
    assert!(opinions
        .iter()
        .filter(|o| o.node != a.cluster.node())
        .all(|o| !o.name.is_empty() && o.name == o.node.short()));
    assert_eq!(
        opinions
            .iter()
            .find(|o| o.node == a.cluster.node())
            .map(|o| o.name.as_str()),
        Some("laptop")
    );

    // Asking is not placing: the run is still where it was, and nobody has been granted it.
    assert!(desktop.accepted.lock().expect("lock").is_empty());
    assert!(server.accepted.lock().expect("lock").is_empty());
    assert_eq!(run.holder(), None);
}

#[tokio::test]
async fn what_became_of_a_run_reaches_the_node_that_submitted_it() {
    // The rough edge this closes: a run placed on a peer used to sit at `assigned` for ever
    // in the submitting node's `ps`, because the record travelled with the grant and nothing
    // updated it afterwards.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let host_a = TestHost::bidding(1);
    a.cluster.hosts_runs(host_a.clone());
    b.cluster.hosts_runs(TestHost::bidding(1));

    // b holds the run and finishes it.
    let mut run = a_run(a.cluster.node());
    let epoch = run
        .assign(b.cluster.node(), START, Millis(60_000))
        .expect("assign");
    // The arbiter has its own grant before anybody can gossip it back — `hand_over` records
    // and publishes before it returns — which is what `merge_run` relies on when it refuses to
    // learn of this node's own run from a peer (ADR-0021 §7). Conjuring the run on b alone
    // would be modelling a step the daemon has no code path for.
    a.cluster.publish_run(run.clone());
    run.started(b.cluster.node(), epoch, START).expect("start");
    b.cluster.publish_runs(vec![run.clone()]);

    a.cluster.probe_round(START).await;

    let seen = a
        .cluster
        .view()
        .runs
        .get(&run.id)
        .cloned()
        .expect("learned");
    assert_eq!(seen.state.name(), "running");
    // And it is written down, not merely believed: a record held in memory is gone at the
    // next restart, and the submitting node is the one most likely to be shut.
    assert_eq!(recorded_ids(&host_a), [run.id]);

    let mut finished = run.clone();
    finished
        .complete(b.cluster.node(), epoch, START + Millis(1_000))
        .expect("complete");
    b.cluster.publish_runs(vec![finished.clone()]);

    let t1 = START + Millis(1_000);
    advance(&[&a, &b], t1);
    a.cluster.probe_round(t1).await;

    assert_eq!(
        a.cluster
            .view()
            .runs
            .get(&run.id)
            .expect("still known")
            .state
            .name(),
        "completed"
    );
}

#[tokio::test]
async fn a_runs_turns_and_cost_reach_the_node_that_submitted_it() {
    // `offload ps` on the submitting node had the state right and said "0 turns, $0" —
    // honest about what it had seen and useless as an answer, because the numbers only ever
    // existed on the machine running the agent.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let host_a = TestHost::bidding(1);
    a.cluster.hosts_runs(host_a.clone());
    b.cluster.hosts_runs(TestHost::bidding(1));

    let mut run = a_run(a.cluster.node());
    let epoch = run
        .assign(b.cluster.node(), START, Millis(60_000))
        .expect("assign");
    run.started(b.cluster.node(), epoch, START).expect("start");
    b.cluster.publish_runs(vec![run.clone()]);
    b.cluster.publish_progress(vec![(
        run.id,
        offload_core::RunProgress {
            turns: 8,
            cost_micro_usd: 83_000,
            workspace: "2 modified".into(),
            at: START,
            by: Some(b.cluster.node()),
            epoch,
            ..offload_core::RunProgress::default()
        },
    )]);

    a.cluster.probe_round(START).await;

    let seen = a
        .cluster
        .view()
        .progress
        .get(&run.id)
        .cloned()
        .expect("learned the numbers");
    assert_eq!(seen.turns, 8);
    assert_eq!(seen.cost_micro_usd, 83_000);
    assert_eq!(seen.workspace, "2 modified");
    // And written down, for the same reason the record is: the submitting node is the one
    // most likely to be shut, and a number held only in view memory does not survive that.
    assert_eq!(
        host_a.learned.lock().expect("lock").len(),
        1,
        "learned once, and only once"
    );

    // Now the direction that would be dangerous. `a` gossips what it knows about runs it does
    // not hold, and its own copy started as zeros; under last-writer-wins the holder's `ps`
    // would flicker between the truth and nothing. Two separate guards stop it — the holder
    // never merges anything about a run it holds, and the forward-only rule (tested in
    // `offload_core::progress`) catches the same claim reaching a third node.
    a.cluster.publish_progress(vec![(
        run.id,
        offload_core::RunProgress {
            workspace: "elsewhere".into(),
            // Written later than the holder's, and still worth nothing: the tiebreak only
            // settles records that describe the same amount of work.
            at: START + Millis(5_000),
            ..offload_core::RunProgress::default()
        },
    )]);
    let t1 = START + Millis(1_000);
    advance(&[&a, &b], t1);
    b.cluster.probe_round(t1).await;

    assert_eq!(
        b.cluster
            .view()
            .progress
            .get(&run.id)
            .expect("still ours")
            .turns,
        8,
        "a node that never ran a turn cannot reset the count"
    );
}

#[tokio::test]
async fn a_deadline_the_operator_moved_reaches_the_node_running_the_run() {
    // The other half of the rule below it, and the half that was missing. "Our store is newer
    // than anything a peer can say" is true of the record — the state, the epoch, the
    // checkpoint — and false of the one part of a run this node does *not* own: the deadline
    // and the priority belong to the run's home node (ADR-0013), which is the node the
    // operator's `offload deadline` is forwarded to. Skipping a peer's whole record because we
    // hold the run threw that away, so a run being held for a slot on the desktop went on
    // being ordered by the deadline it was submitted with.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);
    let host_a = TestHost::bidding(1);
    a.cluster.hosts_runs(host_a.clone());
    b.cluster.hosts_runs(TestHost::bidding(1));

    // Submitted on b, running on a: b owns the editable spec, a owns the record.
    let mut mine = a_run(b.cluster.node());
    let epoch = mine
        .assign(a.cluster.node(), START, Millis(60_000))
        .expect("assign");
    mine.started(a.cluster.node(), epoch, START).expect("start");
    a.cluster.publish_runs(vec![mine.clone()]);

    // `offload deadline <run> 1h`, typed anywhere and forwarded to the owner.
    let mut edited = mine.clone();
    let due = START + Millis(3_600_000);
    assert_eq!(
        edited.edit(offload_core::SpecEdit::Deadline { at: Some(due) }),
        1
    );
    b.cluster.publish_runs(vec![edited]);

    a.cluster.probe_round(START).await;

    let view = a.cluster.view();
    let ours = view.runs.get(&mine.id).expect("ours");
    assert_eq!(ours.spec.deadline, Some(due), "the edit arrived");
    assert_eq!(ours.spec_rev, 1);
    assert_eq!(
        ours.state.name(),
        "running",
        "and it did not cost us the state we own"
    );
    assert_eq!(
        ours.holder(),
        Some(a.cluster.node()),
        "nor the lease we are running under"
    );

    // And it is written down, because the view is memory: a deadline that survives only until
    // the next restart is one the operator will find missing at 03:00.
    //
    // As an *edit*, though, and not as a record — which is the whole of the second half of this
    // rule. What the view holds for a run this node is running is whatever the last gossip tick
    // published, and the store has moved on since: handing that copy down to be written whole
    // would undo a checkpoint taken in between, which is a worse loss than the one this branch
    // exists to prevent. So the peer's record crosses and the node applies the edit to its own
    // copy (`Supervisor::apply_spec_edit`).
    assert!(
        host_a.recorded.lock().expect("lock").is_empty(),
        "a peer's record of a run we hold is not ours to write down"
    );
    let edited = host_a.edited.lock().expect("lock");
    let handed = edited.last().expect("the edit was handed down");
    assert_eq!(handed.spec.deadline, Some(due));
    assert_eq!(handed.spec_rev, 1, "and the revision that settles it");
}

#[tokio::test]
async fn a_peer_cannot_talk_us_out_of_what_we_know_about_our_own_run() {
    // Our store is newer than anything a peer can say about a run we hold. Merging their copy
    // would let a stale record undo a turn — silently, and only under load.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);
    let host_a = TestHost::bidding(1);
    a.cluster.hosts_runs(host_a.clone());
    b.cluster.hosts_runs(TestHost::bidding(1));

    // a holds the run, and is two states ahead of what b remembers.
    let mut mine = a_run(a.cluster.node());
    let epoch = mine
        .assign(a.cluster.node(), START, Millis(60_000))
        .expect("assign");
    let stale = mine.clone();
    mine.started(a.cluster.node(), epoch, START).expect("start");
    a.cluster.publish_runs(vec![mine.clone()]);
    b.cluster.publish_runs(vec![stale]);

    a.cluster.probe_round(START).await;

    assert_eq!(
        a.cluster
            .view()
            .runs
            .get(&mine.id)
            .expect("ours")
            .state
            .name(),
        "running",
        "a peer's older copy did not roll it back"
    );
    assert!(
        host_a.recorded.lock().expect("lock").is_empty(),
        "and nothing was written down about a run we already hold"
    );
}

#[tokio::test]
async fn a_stale_relayed_copy_cannot_resurrect_a_dead_holders_claim() {
    // Found by killing a node for real: a third node relaying its own remembered `Running`
    // undid the arbiter's `Orphaned` every second, pinning the run to a machine that no
    // longer existed. Who is speaking decides, not which state looks further along.
    let net = MemoryNetwork::new();
    let key = fleet();
    let arbiter = join(&net, &key, 1, "laptop");
    let bystander = join(&net, &key, 2, "desktop");
    introduce(&arbiter, &bystander);
    arbiter.cluster.hosts_runs(TestHost::bidding(1));
    bystander.cluster.hosts_runs(TestHost::bidding(1));

    // A third node holds the run and then vanishes; the bystander remembers it running.
    let holder = id(9);
    let mut run = a_run(arbiter.cluster.node());
    let epoch = run.assign(holder, START, Millis(60_000)).expect("assign");
    run.started(holder, epoch, START).expect("start");
    bystander.cluster.publish_runs(vec![run.clone()]);

    let mut orphaned = run.clone();
    orphaned.orphan(START + Millis(1_000)).expect("orphan");
    arbiter.cluster.publish_runs(vec![orphaned]);

    let t1 = START + Millis(1_000);
    advance(&[&arbiter, &bystander], t1);
    arbiter.cluster.probe_round(t1).await;

    assert_eq!(
        arbiter
            .cluster
            .view()
            .runs
            .get(&run.id)
            .expect("known")
            .state
            .name(),
        "orphaned",
        "the bystander's stale copy did not undo the arbiter's decision"
    );
}

#[tokio::test]
async fn a_holder_that_comes_back_takes_its_run_with_it() {
    // ADR-0007's cheapest outcome, and the common one: a laptop suspends for ten seconds, the
    // arbiter notices and marks the run `Orphaned` — an observation, not a decision — and the
    // holder's next word puts it straight back. No migration, no epoch bump, no lost turn.
    let net = MemoryNetwork::new();
    let key = fleet();
    let arbiter = join(&net, &key, 1, "laptop");
    let holder = join(&net, &key, 2, "desktop");
    introduce(&arbiter, &holder);
    arbiter.cluster.hosts_runs(TestHost::bidding(1));
    holder.cluster.hosts_runs(TestHost::bidding(1));

    let mut run = a_run(arbiter.cluster.node());
    let epoch = run
        .assign(holder.cluster.node(), START, Millis(60_000))
        .expect("assign");
    run.started(holder.cluster.node(), epoch, START)
        .expect("start");
    holder.cluster.publish_runs(vec![run.clone()]);
    arbiter.cluster.probe_round(START).await;

    // The holder goes quiet and the arbiter says so.
    let mut orphaned = run.clone();
    orphaned.orphan(START + Millis(1_000)).expect("orphan");
    arbiter.cluster.publish_runs(vec![orphaned]);
    assert_eq!(
        arbiter
            .cluster
            .view()
            .runs
            .get(&run.id)
            .expect("known")
            .state
            .name(),
        "orphaned"
    );

    // It comes back, still running the same turn it was on.
    let t1 = START + Millis(2_000);
    advance(&[&arbiter, &holder], t1);
    arbiter.cluster.probe_round(t1).await;

    let reclaimed = arbiter
        .cluster
        .view()
        .runs
        .get(&run.id)
        .cloned()
        .expect("run");
    assert_eq!(reclaimed.state.name(), "running");
    assert_eq!(reclaimed.epoch, epoch, "reclaiming costs no epoch");
}

#[tokio::test]
async fn a_run_a_node_let_go_is_offered_by_its_arbiter_and_by_nobody_else() {
    // ADR-0042, at the level that decides whether it works: the fact has to survive gossip, and
    // exactly one node has to act on it.
    //
    // It used to be neither. A drain releases a run and offers it once; a refusal at that
    // instant left the run `Pending` with a checkpoint and no holder — which is what `offload
    // checkpoint` leaves, so `supervise` read it as parked by a person and passed it by. What
    // ADR-0041 added was a debt in the departing node's *memory*, which fixed the case it could
    // reach and could not reach the two that matter: a daemon restarted before the fleet has
    // room, and a `SIGTERM`, which is what closing a laptop looks like.
    //
    // The release states which node let the run go, so this is what has to be true of it: the
    // holder writes it, the record carries it, and the run's arbiter — not the node that let it
    // go — is the one that holds the round. Two nodes offering one run is how it gets granted
    // twice at one epoch, which `place` calls the thing the epoch exists to make impossible.
    let net = MemoryNetwork::new();
    let key = fleet();
    let home = join(&net, &key, 1, "laptop");
    let holder = join(&net, &key, 2, "desktop");
    let bystander = join(&net, &key, 3, "mini");
    let members = [&home, &holder, &bystander];
    for (i, a) in members.iter().enumerate() {
        for b in &members[i + 1..] {
            introduce(a, b);
        }
    }

    // Submitted on the laptop, which is where the record is born: a node does not learn of its
    // own run from a peer (`merge_run`), so staging this any other way tests a fleet that cannot
    // exist.
    let mut run = a_run(home.cluster.node());
    home.cluster.publish_runs(vec![run.clone()]);
    for _ in 0..4 {
        for member in members {
            member.cluster.probe_round(START).await;
        }
    }

    // Running on the desktop — and then the desktop drains.
    let epoch = run
        .assign(holder.cluster.node(), START, Millis(60_000))
        .expect("assign");
    run.started(holder.cluster.node(), epoch, START)
        .expect("start");
    run.request_checkpoint(START).expect("the drain asks");
    run.checkpointed(
        holder.cluster.node(),
        epoch,
        offload_core::Checkpoint {
            session_id: Some("11111111-2222-3333-4444-555555555555".into()),
            transcript: offload_core::BlobHash::from_bytes([7; 32]),
            bundle: None,
            patch: None,
            base_commit: "0f0f0f0f".into(),
            turns: 4,
            taken_at: START,
            agent_version: "2.1.238".into(),
            replicas: std::collections::BTreeSet::new(),
        },
        offload_core::GivenUp::LetGo,
        START,
    )
    .expect("released at its boundary");
    holder.cluster.publish_runs(vec![run.clone()]);

    for _ in 0..8 {
        for member in members {
            member.cluster.probe_round(START).await;
        }
    }

    let policy = offload_core::ReassignPolicy::default();
    let seen = |m: &Member| m.cluster.view().runs.get(&run.id).cloned().expect("known");

    // It survived the round trip. Worth asserting on its own: the field rides inside the run's
    // *state*, so a merge that settled on somebody's older copy would erase it silently — and
    // the run would go back to being one nothing offers, with nothing to notice.
    for member in members {
        assert_eq!(
            seen(member).let_go_by(),
            Some(holder.cluster.node()),
            "every node hears which machine let the run go"
        );
    }

    assert_eq!(
        offload_core::supervise(&home.cluster.view(), &seen(&home), START, &policy),
        offload_core::Supervision::Place(offload_core::Offering::LetGo {
            by: holder.cluster.node()
        }),
        "the arbiter offers it, and knows why it is offering it"
    );
    for watcher in [&holder, &bystander] {
        assert_eq!(
            offload_core::supervise(&watcher.cluster.view(), &seen(watcher), START, &policy),
            offload_core::Supervision::Bystander(offload_core::Bystanding::ArbitratedBy(Some(
                home.cluster.node()
            ))),
            "and nobody else does — including the node that let it go"
        );
    }

    // The other half of the pair, from the same record with one field changed: a run a person
    // parked is left alone by everybody. This is the sentence the old code got wrong about both.
    let mut parked = seen(&home);
    parked.state = offload_core::RunState::Pending {
        since: START,
        let_go_by: None,
    };
    assert_eq!(
        offload_core::supervise(&home.cluster.view(), &parked, START, &policy),
        offload_core::Supervision::Bystander(offload_core::Bystanding::Unheld),
        "and a run somebody parked waits for them, which is the case that was right all along"
    );
}

#[tokio::test]
async fn a_run_outlives_the_node_that_submitted_it() {
    // Arbiter failover, the case the deterministic-successor rule exists for. A run's home
    // node is the one somebody typed the command on — a laptop — and it is therefore the most
    // likely thing in the fleet to be shut. If arbitration died with it, the run's holder
    // could go quiet and *nobody* would notice: no orphan, no hold-down, no migration, until
    // a human looked. That is the babysitting this project exists to remove.
    let net = MemoryNetwork::new();
    let key = fleet();
    let home = join(&net, &key, 1, "laptop");
    let holder = join(&net, &key, 2, "desktop");
    let x = join(&net, &key, 3, "mini");
    let y = join(&net, &key, 4, "vm");
    let members = [&home, &holder, &x, &y];
    for (i, a) in members.iter().enumerate() {
        for b in &members[i + 1..] {
            introduce(a, b);
        }
    }

    // The run: submitted on `home`, running on `holder`, and gossiped by the holder because
    // that is who owns the record of a run in flight.
    let mut run = a_run(home.cluster.node());
    let epoch = run
        .assign(holder.cluster.node(), START, Millis(60_000))
        .expect("assign");
    run.started(holder.cluster.node(), epoch, START)
        .expect("start");
    holder.cluster.publish_runs(vec![run.clone()]);

    for _ in 0..8 {
        for member in members {
            member.cluster.probe_round(START).await;
        }
    }
    for survivor in [&x, &y] {
        assert!(
            survivor.cluster.view().runs.contains_key(&run.id),
            "everybody hears about a run in flight"
        );
    }

    // Both the laptop and the machine it placed the run on go away at the same instant —
    // which is what closing a lid on a Wi-Fi network looks like from the other side.
    for gone in [&home, &holder] {
        for survivor in [&x, &y] {
            net.partition(gone.cluster.node(), survivor.cluster.node());
        }
    }

    let mut now = START;
    for _ in 0..40 {
        now = now + Millis(1_000);
        advance(&members, now);
        x.cluster.probe_round(now).await;
        y.cluster.probe_round(now).await;
        let dead = |m: &Member| {
            [home.cluster.node(), holder.cluster.node()]
                .iter()
                .all(|n| m.status_of(*n) == Some(NodeStatus::Dead))
        };
        if dead(&x) && dead(&y) {
            break;
        }
    }
    for survivor in [&x, &y] {
        assert!(
            survivor.status_of(home.cluster.node()) == Some(NodeStatus::Dead)
                && survivor.status_of(holder.cluster.node()) == Some(NodeStatus::Dead),
            "both survivors concluded the same thing before anything acts on it"
        );
    }

    // Whoever wins the successor rule is whoever wins it — the property under test is that
    // exactly one node takes it on, and that both of them agree which.
    let (successor, bystander) = if x.cluster.node() < y.cluster.node() {
        (&x, &y)
    } else {
        (&y, &x)
    };
    let policy = offload_core::ReassignPolicy::default();
    let seen = |m: &Member| m.cluster.view().runs.get(&run.id).cloned().expect("known");

    assert_eq!(
        offload_core::supervise(&successor.cluster.view(), &seen(successor), now, &policy),
        offload_core::Supervision::Orphan,
        "somebody picks up arbitration for a run whose home node has gone"
    );
    assert_eq!(
        offload_core::supervise(&bystander.cluster.view(), &seen(bystander), now, &policy),
        offload_core::Supervision::Bystander(offload_core::Bystanding::ArbitratedBy(Some(
            successor.cluster.node()
        ))),
        "and exactly one node does, computed rather than elected"
    );

    // The other half: an orphan from a *successor* has to be believed. `merge_run` asks who
    // is speaking, and the answer now depends on a view that says the home node is gone —
    // so the bystander has to reach the same conclusion independently or the decision is
    // rejected every second and the run never moves.
    let mut orphaned = seen(successor);
    orphaned.orphan(now).expect("orphan");
    successor.cluster.publish_runs(vec![orphaned]);

    now = now + Millis(1_000);
    advance(&members, now);
    successor.cluster.probe_round(now).await;
    bystander.cluster.probe_round(now).await;

    assert_eq!(
        seen(bystander).state.name(),
        "orphaned",
        "the successor's decision is accepted by a peer that agrees who the successor is"
    );
}

#[tokio::test]
async fn a_free_node_beats_a_keener_busy_one_and_a_busy_fleet_still_commits() {
    // ADR-0006's accepting-without-starting, from the arbiter's side. Two properties, and the
    // second is the one the overnight case needs: given a choice, work goes where it can
    // start; given no choice, somebody still *takes* it rather than leaving it pending with a
    // collection of reasons.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    // The desktop wants it far more, and cannot start it.
    let host_b = TestHost::bidding(900);
    *host_b.available.lock().expect("lock") = offload_core::Availability::WhenFree { behind: 2 };
    b.cluster.hosts_runs(host_b.clone());
    let host_a = TestHost::bidding(1);
    a.cluster.hosts_runs(host_a.clone());

    let run = a_run(a.cluster.node());
    let placed = a.cluster.place(&run, Ok(offer(1)), WINDOW).await;
    assert_eq!(
        placed.accepted_by(),
        Some(a.cluster.node()),
        "starting today beats wanting it more"
    );
    assert!(host_b.accepted.lock().expect("lock").is_empty());

    // Now nobody is free. The keenest busy node takes it, and says so: acceptance that does
    // not admit to being a queue is how somebody closes their laptop expecting output.
    let run = a_run(a.cluster.node());
    let placed = a
        .cluster
        .place(
            &run,
            Ok(offload_core::Offer {
                score: offload_core::Score(1),
                available: offload_core::Availability::WhenFree { behind: 1 },
                terms: None,
            }),
            WINDOW,
        )
        .await;
    let offload_cluster::Placement::Accepted {
        node,
        name,
        starting,
        ..
    } = placed
    else {
        panic!("somebody commits: {placed:?}");
    };
    assert_eq!(
        (node, name.as_str(), starting),
        (
            b.cluster.node(),
            "desktop",
            offload_core::Availability::WhenFree { behind: 2 }
        ),
        "somebody commits, and the answer says when rather than implying now"
    );
    assert_eq!(host_b.accepted.lock().expect("lock").as_slice(), [run.id]);
}

#[tokio::test]
async fn deciding_about_somebody_elses_run_does_not_forget_our_own() {
    // `publish_runs` replaces this node's whole contribution, which is right once a tick and
    // wrong for a single arbiter's decision: it would drop the run we are *running* out of
    // our own view, and `running_count` reads that view to decide whether we are at capacity.
    // A bid arriving in the gap would be answered as if this machine were idle.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let mut ours = a_run(a.cluster.node());
    let epoch = ours
        .assign(a.cluster.node(), START, Millis(60_000))
        .expect("assign");
    ours.started(a.cluster.node(), epoch, START).expect("start");
    a.cluster.publish_runs(vec![ours.clone()]);
    assert_eq!(a.cluster.view().running_count(&a.cluster.node()), 1);

    // Meanwhile a run held by a third node, which we arbitrate and have just orphaned.
    let mut theirs = a_run(a.cluster.node());
    theirs.id = offload_core::RunId::from_bytes([8; 16]);
    let epoch = theirs.assign(id(9), START, Millis(60_000)).expect("assign");
    theirs.started(id(9), epoch, START).expect("start");
    theirs.orphan(START + Millis(1_000)).expect("orphan");
    a.cluster.publish_run(theirs.clone());

    assert_eq!(
        a.cluster.view().running_count(&a.cluster.node()),
        1,
        "the run this node is holding is still in its own view"
    );
    assert_eq!(
        a.cluster
            .view()
            .runs
            .get(&theirs.id)
            .expect("known")
            .state
            .name(),
        "orphaned"
    );
}

#[tokio::test]
async fn a_blob_is_fetched_from_whoever_has_it() {
    // Migration is "fetch these hashes and materialise the worktree" (ADR-0003). This is the
    // fetching part, and the reason availability is not gossiped: asking is one round trip and
    // is never stale (ADR-0016).
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let hash = b
        .blobs
        .store(b"a transcript, and some uncommitted work".to_vec())
        .await
        .expect("store");

    assert!(!a.blobs.has(hash).await, "a has never seen it");
    assert_eq!(a.cluster.fetch_blob(hash, None).await.expect("fetch"), hash);
    assert!(a.blobs.has(hash).await);
}

#[tokio::test]
async fn a_blob_nobody_has_fails_with_a_reason_rather_than_hanging() {
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let missing = BlobHash::from_bytes([9; 32]);
    let error = a
        .cluster
        .fetch_blob(missing, None)
        .await
        .expect_err("nobody has it");
    assert!(error.to_string().contains("does not have it"));
}

#[tokio::test]
async fn a_peer_that_sends_the_wrong_bytes_gets_a_retry_and_not_a_poisoned_checkpoint() {
    // The property content addressing buys, and the reason a blob may be fetched from anyone:
    // the hash is the authority, so the fetch path needs no trust decision beyond membership.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "liar");
    introduce(&a, &b);

    // b holds something, but a asks for a hash b will answer with the wrong content for:
    // simulate by storing bytes under b and asking for a hash that is not theirs.
    let real = b
        .blobs
        .store(b"the real thing".to_vec())
        .await
        .expect("store");
    let wrong = BlobHash::from_bytes([3; 32]);
    b.blobs
        .held
        .lock()
        .expect("lock")
        .insert(wrong, b"not the real thing".to_vec());

    let error = a
        .cluster
        .fetch_blob(wrong, None)
        .await
        .expect_err("the bytes do not hash to what was asked for");
    assert!(error.to_string().contains("when asked for"));
    assert!(
        !a.blobs.has(wrong).await,
        "nothing was filed under that hash"
    );
    assert!(!a.blobs.has(real).await);
}

#[tokio::test]
async fn an_answer_typed_on_one_machine_reaches_the_agent_blocked_on_another() {
    // ADR-0017's fleet half, and the reason it exists: the question reaches the phone in your
    // pocket, and answering it there has to *do* something to the desktop.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "phone");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let host_b = TestHost::bidding(50);
    b.cluster.hosts_runs(host_b.clone());

    let run = a_run(b.cluster.node()).id;
    host_b
        .waiting
        .lock()
        .expect("lock")
        .push(offload_core::PendingAsk {
            run,
            node: None,
            node_name: None,
            tool_use_id: "toolu_01ABC".into(),
            tool: "Bash".into(),
            detail: "cargo build --release".into(),
            waiting: Millis(12_000),
            left: Millis(288_000),
        });

    // The phone can see what the desktop is stopped on, asked for at the moment it looked
    // rather than gossiped — a blocked process stops being blocked when somebody answers.
    let seen = a.cluster.canvass_asks(Millis(500)).await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].tool_use_id, "toolu_01ABC");
    assert_eq!(
        seen[0].node,
        Some(b.cluster.node()),
        "stamped by the collector, because the holder does not name itself"
    );
    assert_eq!(seen[0].where_it_waits(), "desktop");

    // And answering it on the phone unblocks the agent on the desktop.
    let (tool, detail, allowed) = a
        .cluster
        .answer_at(b.cluster.node(), run, None, true, Millis(500))
        .await
        .expect("the holder took the answer");
    assert_eq!(tool, "Bash");
    assert_eq!(detail, "cargo build --release");
    assert!(allowed);

    let answers = host_b.answers.lock().expect("lock");
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].run, run);
    assert!(answers[0].allow);
    assert_eq!(
        answers[0].by, "phone",
        "the run's log says which machine it was approved from"
    );
    assert!(
        host_b.waiting.lock().expect("lock").is_empty(),
        "an answered question stops being offered to anybody"
    );
}

#[tokio::test]
async fn a_cancel_typed_on_one_machine_stops_the_run_on_another() {
    // The last operator command that acted on the machine it was typed at. It read the local
    // process table, so a run on the desktop came back as "run ab12… is not running" — a false
    // sentence about a run that was working and spending money, with nothing forwarded and
    // nowhere for the operator to go.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "phone");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let host_b = TestHost::bidding(50);
    b.cluster.hosts_runs(host_b.clone());

    let run = a_run(b.cluster.node()).id;
    let note = a
        .cluster
        .cancel_at(b.cluster.node(), run, Millis(500))
        .await
        .expect("the holder stopped it");
    assert_eq!(
        note, "its agent was stopped",
        "what was stopped is the answer, because a mid-turn agent cost money and a commitment did not"
    );

    let acted = host_b.cancelled.lock().expect("lock");
    assert_eq!(acted.len(), 1);
    assert_eq!(acted[0].0, run);
    assert_eq!(
        acted[0].1, "phone",
        "the run's log says which machine it was cancelled from"
    );
}

#[tokio::test]
async fn a_checkpoint_asked_for_on_one_machine_reaches_the_agent_on_another() {
    // The sibling of the cancel above, and the last command that acted only where it was typed:
    // it answered "it is not running here, so there is no turn boundary coming", which is true
    // about the machine you are at and silent about the one the run is on.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "phone");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let host_b = TestHost::bidding(50);
    b.cluster.hosts_runs(host_b.clone());

    let run = a_run(b.cluster.node()).id;
    a.cluster
        .checkpoint_at(b.cluster.node(), run, Millis(500))
        .await
        .expect("the holder took the request");
    assert_eq!(
        *host_b.checkpointing.lock().expect("lock"),
        vec![run],
        "the machine running the agent is the one that was asked"
    );

    // And a node that hosts nothing says so rather than accepting a request it can never honour:
    // a turn boundary is a moment in a process, and there is no process here.
    let phone = a.cluster.node();
    let error = b
        .cluster
        .checkpoint_at(phone, a_run(phone).id, Millis(500))
        .await
        .expect_err("nothing here reaches a turn boundary");
    assert!(error.contains("turn boundary"), "{error}");
}

#[tokio::test]
async fn a_cancel_that_arrives_after_the_run_finished_comes_back_as_a_sentence() {
    // Most of a second passes while a forwarded cancel is in flight, which makes "it finished
    // first" ordinary rather than exceptional — and a refusal that says why is the difference
    // between a race and an apparent fault.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "phone");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let host_b = TestHost::bidding(50);
    host_b.refuse_cancel.store(true, Ordering::SeqCst);
    b.cluster.hosts_runs(host_b.clone());

    let error = a
        .cluster
        .cancel_at(b.cluster.node(), a_run(b.cluster.node()).id, Millis(500))
        .await
        .expect_err("nothing left to stop");
    assert!(error.contains("already completed"), "{error}");

    // And a node that hosts nothing at all says so in the same shape, rather than accepting a
    // cancel it cannot act on.
    let host_a = a.cluster.node();
    let error = b
        .cluster
        .cancel_at(host_a, a_run(host_a).id, Millis(500))
        .await
        .expect_err("the phone hosts nothing");
    assert!(error.contains("hosts no runs"), "{error}");
}

#[tokio::test]
async fn an_answer_to_a_question_that_has_gone_is_refused_rather_than_applied() {
    // The race the whole addressing scheme exists for: patience ran out, or the run moved and
    // its new leg asked its own question with its own id. Either way the old answer matches
    // nothing — fencing by construction rather than by a check somebody has to remember.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "phone");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    b.cluster.hosts_runs(TestHost::bidding(50));

    let run = a_run(b.cluster.node()).id;
    let error = a
        .cluster
        .answer_at(
            b.cluster.node(),
            run,
            Some("toolu_from_a_previous_leg".into()),
            true,
            Millis(500),
        )
        .await
        .expect_err("nothing is waiting for that");
    assert!(error.contains("nothing here is waiting"), "{error}");

    // And a fleet where nobody is waiting says so rather than inventing a question.
    assert!(a.cluster.canvass_asks(Millis(500)).await.is_empty());
}

#[tokio::test]
async fn a_checkpoint_is_pushed_to_a_peer_so_it_survives_the_node_that_made_it() {
    // ADR-0016's durability half: a checkpoint that exists only on the node that died cannot
    // be migrated from, and no amount of discovery finds a copy nobody made.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let hash = a.blobs.store(b"turn four".to_vec()).await.expect("store");
    assert_eq!(
        a.cluster
            .push_blob(b.cluster.node(), hash)
            .await
            .expect("push"),
        Pushed::Stored
    );
    assert!(b.blobs.has(hash).await);

    // Pushing it again is declined rather than re-sent: the bytes are already there — and the
    // decline says so, which is what tells a caller this is the good outcome rather than a peer
    // that will not have it.
    let again = a
        .cluster
        .push_blob(b.cluster.node(), hash)
        .await
        .expect("push again");
    assert_eq!(
        again,
        Pushed::Declined {
            reason: "already held".into()
        }
    );
    assert!(!again.is_permanent());
}

#[tokio::test]
async fn a_blob_is_fetched_from_a_peer_the_asker_merely_suspects() {
    // The failure ADR-0016 exists to prevent, arrived at through the filter that was supposed to
    // help. `fetch_blob` asked only peers this node marks `Alive` — and the moment a checkpoint
    // is *needed* is the moment a node has gone quiet, which is when everybody's view is full of
    // suspicion. A `Suspect` peer is very often perfectly reachable: that is the entire reason
    // the state exists, "long enough for the subject to hear the suspicion and refute it".
    //
    // `fetch_blob`'s own doc comment makes the argument against the filter: availability is not
    // gossiped, so *asking is the discovery*, and a node that does not have the bytes says so in
    // a millisecond. A status is exactly as stale as the advertisement that reasoning rejects.
    // The code already conceded the point for the `from` hint, which is asked whatever its
    // status; it was every other peer that got skipped.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    let c = join(&net, &key, 3, "server");
    introduce(&a, &b);
    introduce(&a, &c);

    // Only c has the checkpoint. b is the node the run was last on, so it is the hint — and it
    // does not have the bytes, which is the ordinary case after a reassignment.
    let hash = c
        .blobs
        .store(b"turn nineteen".to_vec())
        .await
        .expect("store");

    a.cluster.probe_round(START).await;
    a.cluster.probe_round(START).await;

    // Cut from *everyone*, so the indirect probe through b fails too — otherwise b answers for c
    // and a is quite right to keep calling it alive, which is SWIM doing its job.
    net.partition(a.cluster.node(), c.cluster.node());
    net.partition(b.cluster.node(), c.cluster.node());
    let t1 = START + Millis(1_000);
    advance(&[&a, &b, &c], t1);
    // The rotation visits one peer per period, so keep going until it has been c's turn.
    for _ in 0..4 {
        a.cluster.probe_round(t1).await;
    }
    assert_eq!(
        a.status_of(c.cluster.node()),
        Some(NodeStatus::Suspect),
        "the setup needs a to be suspicious of c"
    );

    // And the wobble is over — a has simply not re-probed yet, which is a whole rotation away.
    net.heal(a.cluster.node(), c.cluster.node());

    let got = a
        .cluster
        .fetch_blob(hash, Some(b.cluster.node()))
        .await
        .expect("the only copy in the fleet is on a node that answers");
    assert_eq!(got, hash);
    assert!(a.blobs.has(hash).await);
}

#[tokio::test]
async fn a_device_that_will_not_take_pushes_declines_rather_than_dropping_them() {
    // A phone on mobile data is perfectly able to hold a checkpoint and quite right to refuse
    // one. The sender learns that, rather than believing the run is durable.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "phone");
    introduce(&a, &b);
    b.blobs.refuses.store(true, Ordering::SeqCst);

    let hash = a.blobs.store(b"turn four".to_vec()).await.expect("store");
    let refused = a
        .cluster
        .push_blob(b.cluster.node(), hash)
        .await
        .expect("declined, not failed");
    assert_eq!(
        refused,
        Pushed::Declined {
            reason: "this node is not taking pushes right now".into()
        }
    );
    // A policy refusal is not permanent: the phone comes off mobile data and takes it later.
    assert!(!refused.is_permanent());
    assert!(!b.blobs.has(hash).await);
}

#[tokio::test]
async fn a_blob_over_the_protocol_cap_is_refused_here_rather_than_by_the_peer() {
    // The cap is the same on every node, so this is a fact about the blob and not about the
    // peer — which makes "try another peer" and "try again later" both wrong, and makes the
    // answer knowable without dialling anybody. Measured in session seventy-eight on two
    // daemons with the cap lowered under a 256 MiB bundle: the peer was alive, meshed and
    // willing, the push was refused for size, and the only record of why was a `debug` line
    // inside this crate while the operator was told "nobody would take a copy".
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let limit = offload_proto::cluster::MAX_BLOB_BYTES;
    let size = usize::try_from(limit).expect("cap fits") + 1;
    let hash = a.blobs.store(vec![7u8; size]).await.expect("store");

    let outcome = a
        .cluster
        .push_blob(b.cluster.node(), hash)
        .await
        .expect("a refusal is an answer, not a transport failure");
    assert_eq!(
        outcome,
        Pushed::TooLarge {
            size: size as u64,
            limit
        }
    );
    assert!(outcome.is_permanent());
    assert!(
        outcome
            .why()
            .contains("no node in this fleet can take a copy"),
        "{}",
        outcome.why()
    );
    assert!(!b.blobs.has(hash).await, "nothing was offered to the peer");
}

#[tokio::test]
async fn a_blob_already_held_needs_no_network_at_all() {
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);
    let hash = a
        .blobs
        .store(b"already here".to_vec())
        .await
        .expect("store");

    // Partitioned from everybody, and the fetch still succeeds.
    net.partition(a.cluster.node(), b.cluster.node());
    assert_eq!(a.cluster.fetch_blob(hash, None).await.expect("fetch"), hash);
}

#[tokio::test]
async fn a_view_of_a_fleet_of_one_is_just_itself() {
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");

    a.cluster.probe_round(START).await;

    let view: ClusterView = a.cluster.view();
    assert_eq!(view.nodes.len(), 1);
    assert_eq!(view.local, a.cluster.node());
}

#[tokio::test]
async fn a_run_can_be_followed_from_a_node_that_is_not_running_it() {
    // The promise on the front of the README, from the operator's end: submit from the laptop,
    // watch it move to the desktop, and keep seeing output. The log lives with whoever is
    // running the agent, so following it from anywhere else means asking them — and being asked
    // is also how that node learns somebody is watching, which is the only way attendance is
    // ever knowable (ADR-0013).
    let net = MemoryNetwork::new();
    let key = fleet();
    let watcher = join(&net, &key, 1, "laptop");
    let runner = join(&net, &key, 2, "desktop");
    introduce(&watcher, &runner);

    let host = TestHost::bidding(50);
    runner.cluster.hosts_runs(host.clone());
    let run = a_run(watcher.cluster.node());

    // Two turns in, and not finished: the follower has to be told there is more to come rather
    // than concluding from a short page that the run is over.
    {
        let mut log = host.log.lock().expect("lock");
        log.push(offload_core::LogEvent::at(
            Millis(1),
            offload_core::LogKind::Text {
                text: "thinking".into(),
            },
        ));
        log.push(offload_core::LogEvent::at(
            Millis(2),
            offload_core::LogKind::TurnBoundary { turn: 1 },
        ));
    }

    let (page, done) = watcher
        .cluster
        .fetch_events(runner.cluster.node(), run.id, 0, 128, WINDOW)
        .await
        .expect("the holder answered");
    assert_eq!(page.len(), 2);
    assert!(!done, "a quiet run is not a finished one");
    assert_eq!(
        host.pulled_by.lock().expect("lock").as_slice(),
        [watcher.cluster.node()],
        "and the holder knows who is watching"
    );

    // Asking again from where the last page ended gets only what is new — which is what makes a
    // poll cheap enough to run once a second per follower.
    host.log
        .lock()
        .expect("lock")
        .push(offload_core::LogEvent::at(
            Millis(3),
            offload_core::LogKind::Finished {
                success: true,
                turns: 1,
                denials: 0,
                cost_micro_usd: 10,
                work: offload_core::WorkKind::Agent,
                result: None,
            },
        ));
    let (tail, done) = watcher
        .cluster
        .fetch_events(runner.cluster.node(), run.id, page[1].seq, 128, WINDOW)
        .await
        .expect("answered");
    assert_eq!(tail.len(), 1);
    assert!(
        done,
        "and the end of the log says so, so nothing waits for more"
    );
}

#[tokio::test]
async fn a_node_that_keeps_no_logs_says_so_rather_than_looking_empty() {
    // "I have no log for that run" and "that run has produced nothing yet" are different facts,
    // and a follower that cannot tell them apart waits for ever on a machine that was never
    // going to answer.
    let net = MemoryNetwork::new();
    let key = fleet();
    let watcher = join(&net, &key, 1, "laptop");
    let quiet = join(&net, &key, 2, "phone");
    introduce(&watcher, &quiet);
    quiet.cluster.hosts_runs(Arc::new(offload_cluster::NoHost));

    let run = a_run(watcher.cluster.node());
    assert_eq!(
        watcher
            .cluster
            .fetch_events(quiet.cluster.node(), run.id, 0, 128, WINDOW)
            .await,
        Err("this node keeps no run logs".into())
    );
}

#[tokio::test]
async fn a_deadline_change_is_carried_to_the_node_that_owns_it() {
    // ADR-0013's ownership rule, exercised over a real exchange: an operator types the change
    // at whichever machine is in front of them, and it is applied at the run's home node. The
    // alternative — apply locally, gossip, let the merge sort it out — is two records at
    // revision 1 undoing each other for as long as both nodes are up.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);

    let owner = TestHost::bidding(50);
    b.cluster.hosts_runs(owner.clone());

    let run = a_run(b.cluster.node());
    let due = Millis(1_764_000_000_000);
    let note = a
        .cluster
        .edit_spec_at(
            b.cluster.node(),
            run.id,
            offload_core::SpecEdit::Deadline { at: Some(due) },
            WINDOW,
        )
        .await
        .expect("the owner answered");

    assert_eq!(note, "noted");
    assert_eq!(
        owner.deadlines.lock().expect("lock").as_slice(),
        [(run.id, offload_core::SpecEdit::Deadline { at: Some(due) })]
    );

    // A refusal comes back as a sentence rather than as a revision nobody would honour: a
    // finished run has no decision left for a deadline to influence.
    owner.refuse_deadline.store(true, Ordering::SeqCst);
    assert_eq!(
        a.cluster
            .edit_spec_at(
                b.cluster.node(),
                run.id,
                offload_core::SpecEdit::Deadline { at: None },
                WINDOW,
            )
            .await,
        Err("it has already completed".into())
    );
}

#[tokio::test]
async fn an_owner_that_cannot_be_reached_is_said_out_loud() {
    // The operator is standing there, so "the node that owns this is not answering" is a fact
    // they can act on. Applying it here instead would be the convergence bug in a nicer coat.
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    introduce(&a, &b);
    b.cluster.hosts_runs(TestHost::bidding(50));
    net.partition(a.cluster.node(), b.cluster.node());

    let run = a_run(b.cluster.node());
    let refused = a
        .cluster
        .edit_spec_at(
            b.cluster.node(),
            run.id,
            offload_core::SpecEdit::Deadline { at: None },
            WINDOW,
        )
        .await
        .expect_err("nobody answered");
    // Named where the view has a name for it, and by id where it has only met a key.
    assert!(
        refused.contains(&b.cluster.node().short()),
        "it has to say which node: {refused}"
    );
    assert!(refused.contains("did not answer"), "{refused}");
}

#[tokio::test]
async fn a_revocation_reaches_a_node_that_never_heard_the_command() {
    // ADR-0012: membership is verifiable offline, so nothing about *belonging* has to travel —
    // and revocation is the one exception, because it is the absence of a signature and no
    // certificate can carry it. Typed on the desktop, believed on the laptop.
    let net = MemoryNetwork::new();
    let key = fleet();
    let desktop = join(&net, &key, 1, "desktop");
    let laptop = join(&net, &key, 2, "laptop");
    introduce(&desktop, &laptop);

    desktop.membership.revoke(&key, id(3));
    assert!(!laptop.membership.knows_revoked(id(3)));

    desktop.cluster.probe_round(START).await;

    assert!(
        laptop.membership.knows_revoked(id(3)),
        "a node that was not at the keyboard has to learn it from somebody"
    );
    // And it is now this node's fact to pass on, rather than something it merely heard: the
    // signature travels, so the third machine learns it from either of the first two.
    assert_eq!(
        offload_cluster::Members::revocations(laptop.membership.as_ref()).len(),
        1
    );
}

#[tokio::test]
async fn a_revocation_signed_by_anybody_else_is_not_believed() {
    // The fleet key is the only thing that can revoke. A peer that could would be a peer that
    // can evict every other device by asserting it — and gossip is exactly the path an attacker
    // who got admitted would use.
    let net = MemoryNetwork::new();
    let key = fleet();
    let desktop = join(&net, &key, 1, "desktop");
    let laptop = join(&net, &key, 2, "laptop");
    introduce(&desktop, &laptop);

    // A member of this fleet, signing with a key that is not the fleet's.
    let impostor = FleetKey::derive("zebra puppy abacus").expect("derive");
    desktop
        .membership
        .revocations
        .lock()
        .expect("lock")
        .push(Revocation::issue(
            impostor.signing_key(),
            key.id(),
            id(2),
            START,
            START.0,
        ));

    desktop.cluster.probe_round(START).await;

    assert!(
        !laptop.membership.knows_revoked(id(2)),
        "an unverifiable revocation is not a revocation"
    );
}

#[tokio::test]
async fn a_revoked_peer_stops_being_talked_to_rather_than_merely_being_marked() {
    // The half that is easy to leave out. Membership is checked at the handshake, and a live
    // session never handshakes again — so a node that files a revocation and keeps the socket
    // open has recorded a fact and changed nothing at all.
    let net = MemoryNetwork::new();
    let key = fleet();
    let desktop = join(&net, &key, 1, "desktop");
    let laptop = join(&net, &key, 2, "laptop");
    let phone = join(&net, &key, 3, "phone");
    introduce(&desktop, &laptop);
    introduce(&desktop, &phone);

    // A connection that is up and working.
    desktop.cluster.probe_round(START).await;
    assert_eq!(desktop.status_of(id(2)), Some(NodeStatus::Alive));

    // Typed on the phone, which the desktop then hears from.
    phone.membership.revoke(&key, id(2));
    phone.cluster.probe_round(START).await;
    assert!(desktop.membership.knows_revoked(id(2)));

    // And now the desktop cannot reach the laptop at all: the old session was closed, and the
    // handshake that would replace it is refused against the revocation it now holds.
    let mut at = START;
    for _ in 0..8 {
        at = at + Millis(1_000);
        advance(&[&desktop, &laptop, &phone], at);
        desktop.cluster.probe_round(at).await;
    }
    assert_eq!(
        desktop.status_of(id(2)),
        Some(NodeStatus::Dead),
        "a revoked device is unreachable, not merely annotated"
    );
}

#[tokio::test]
async fn a_revoked_peer_that_keeps_dialling_in_is_hung_up_on_too() {
    // The other half of the connection, and the half a hang-up could not reach. `disconnect`
    // walks the map of sessions *this node dialled*; a session the peer opened lives in the task
    // serving it and is in no map at all. So a revoked device that goes on dialling in keeps a
    // channel nothing closes — and every message on it is contact, which is what makes the
    // failure detector answer "no longer suspect" about a device the fleet has just thrown out.
    //
    // The test above this one cannot see it: there, the desktop is the one dialling.
    let net = MemoryNetwork::new();
    let key = fleet();
    let desktop = join(&net, &key, 1, "desktop");
    let laptop = join(&net, &key, 2, "laptop");
    let phone = join(&net, &key, 3, "phone");
    introduce(&desktop, &laptop);
    introduce(&desktop, &phone);

    // The laptop is the one dialling, so the desktop's only session with it is inbound.
    laptop.cluster.probe_round(START).await;
    assert_eq!(desktop.status_of(id(2)), Some(NodeStatus::Alive));

    // Revoked on the phone, which the desktop then hears it from — the gossip path, and the one
    // that calls `disconnect` inside the cluster.
    phone.membership.revoke(&key, id(2));
    phone.cluster.probe_round(START).await;
    assert!(desktop.membership.knows_revoked(id(2)));

    // And the laptop goes on probing, which is what a device that has not heard does.
    let mut at = START;
    for _ in 0..12 {
        at = at + Millis(1_000);
        advance(&[&desktop, &laptop, &phone], at);
        laptop.cluster.probe_round(at).await;
        desktop.cluster.probe_round(at).await;
    }
    assert_eq!(
        desktop.status_of(id(2)),
        Some(NodeStatus::Dead),
        "a revoked device that keeps dialling in is still a revoked device"
    );
}

#[tokio::test]
async fn a_revoked_node_learns_of_its_own_revocation_from_the_refusal() {
    // ADR-0044. The handler for "this node has been revoked" sat on the gossip path, and the
    // gossip path is the one a revocation makes impossible: the revoking node hangs up in both
    // directions and refuses every handshake after, so nothing it knows can reach the subject
    // again. The only message a revoked node still receives from its fleet is the refusal on the
    // dial it keeps making — and that refusal named it and proved nothing.
    let net = MemoryNetwork::new();
    let key = fleet();
    let desktop = join(&net, &key, 1, "desktop");
    let laptop = join(&net, &key, 2, "laptop");
    introduce(&desktop, &laptop);

    // Revoked before the two have ever spoken, which is what isolates the path being tested: a
    // node with a live session learns by gossip like anybody else, and that is the arrangement a
    // revocation destroys. Here there is no session to carry anything, and the laptop's only
    // contact with its fleet is the dial it is refused on.
    desktop.membership.revoke(&key, id(2));
    assert!(!laptop.membership.knows_revoked(id(2)));

    let mut at = START;
    at = at + Millis(1_000);
    advance(&[&desktop, &laptop], at);
    laptop.cluster.probe_round(at).await;

    assert!(
        laptop.membership.knows_revoked(id(2)),
        "the refusal carried the signed revocation, and the subject filed it"
    );
}

#[tokio::test]
async fn a_revocation_signed_by_the_wrong_key_evicts_nobody() {
    // The objection ADR-0044 has to answer: a refusal is a peer's claim about this node, and a
    // node must never believe one of those about itself. What makes it safe to act on is not the
    // refusal but the fleet's signature inside it — checked by `Members::revoked`, which is
    // where `note_own_revocation` sends it and is the same gate a *gossiped* revocation passes.
    // Asserted against that gate directly, because a peer that could forge one is not something
    // the memory transport can be made to be.
    let net = MemoryNetwork::new();
    let key = fleet();
    let impostor = FleetKey::derive("zebra puppy abacus").expect("derive");
    let laptop = join(&net, &key, 2, "laptop");

    let forged = Revocation::issue(impostor.signing_key(), key.id(), id(2), START, 1);
    assert!(
        !offload_cluster::Members::revoked(&*laptop.membership, forged),
        "a revocation signed by the wrong key evicts nobody"
    );
    assert!(!laptop.membership.knows_revoked(id(2)));
}

#[tokio::test]
async fn a_certificate_running_out_is_re_issued_by_an_approver() {
    // ADR-0012's backstop, which nothing implemented for two phases: certificates expire in
    // thirty days and are "renewed on contact". Without this the fleet's behaviour after a
    // month is not that a stale revocation finally bites — it is that every device falls out
    // at once and the passphrase is the only way back on each of them.
    let net = MemoryNetwork::new();
    let key = fleet();
    let desktop = join(&net, &key, 1, "desktop");
    // A laptop whose certificate has a week left on a thirty-day life.
    let month = offload_core::fleet::CERT_LIFETIME;
    let issued = Millis(START.0 - (month.0 - 6 * 24 * 60 * 60 * 1_000));
    let laptop = join_as(
        &net,
        &key,
        2,
        "laptop",
        Some(Terms {
            issued_at: issued,
            ..Terms::joining(key.id(), id(2), "laptop", issued)
        }),
    );
    desktop.membership.make_approver(&key);
    introduce(&desktop, &laptop);

    let before = laptop.membership.expires_at();
    assert!(
        laptop
            .membership
            .credentials()
            .membership
            .is_due_for_renewal(START),
        "the test's premise: it is in the last quarter of its life"
    );

    let credentials = laptop
        .cluster
        .renew_with(id(1), Millis(1_000), laptop.membership.credentials())
        .await
        .expect("renewed");
    // Verified against the fleet key alone, with the approver's delegation beside it — which is
    // what makes an approver an issuer rather than a root: the chain is checkable offline and
    // the delegation behind it can be withdrawn.
    credentials
        .membership
        .verify(key.id(), credentials.delegation.as_ref(), START)
        .expect("the renewed certificate verifies");
    assert!(credentials.membership.expires_at > before);
    assert_eq!(
        credentials.membership.grants,
        laptop.membership.credentials().membership.grants,
        "renewal is a fresh clock and nothing else"
    );
    // The approver keeps what it issued as the laptop's papers, not the handshake's copy: a
    // report built on the handshake's (the re-approval list, `expiring`) described a certificate
    // that had lapsed two renewals earlier, on a connection that never re-handshook.
    let held = desktop
        .cluster
        .certificates()
        .into_iter()
        .find(|cert| cert.member == id(2))
        .expect("the approver holds the laptop's certificate");
    assert_eq!(held.expires_at, credentials.membership.expires_at);
    laptop.membership.take_up(credentials);

    // And the fleet still admits it, on the new papers.
    desktop.cluster.probe_round(START).await;
    assert_eq!(desktop.status_of(id(2)), Some(NodeStatus::Alive));
}

#[tokio::test]
async fn a_host_node_is_renewed_by_an_approver_that_could_not_have_granted_it() {
    // The case the test above does not reach, and the one the rule about what an approver may
    // issue could most easily have broken. A laptop that joined has `{submit, deliver}` and an
    // approver may hand those out all day; a *host* has `host-runs`, which ADR-0012 mitigation 1
    // keeps behind the passphrase — so an approver must be able to keep that certificate alive
    // while being unable to have created it. Get this wrong in the strict direction and every
    // host in the fleet needs the passphrase every thirty days, which is the failure renewal was
    // built to prevent.
    let net = MemoryNetwork::new();
    let key = fleet();
    let desktop = join(&net, &key, 1, "desktop");

    let month = offload_core::fleet::CERT_LIFETIME;
    let issued = Millis(START.0 - (month.0 - 6 * 24 * 60 * 60 * 1_000));
    let mut terms = Terms::joining(key.id(), id(2), "builder", issued);
    terms.grants.insert(Grant::HostRuns);
    terms.probation = false;
    terms.issued_at = issued;
    let builder = join_as(&net, &key, 2, "builder", Some(terms));
    desktop.membership.make_approver(&key);
    introduce(&desktop, &builder);

    let credentials = builder
        .cluster
        .renew_with(id(1), Millis(1_000), builder.membership.credentials())
        .await
        .expect("an approver must be able to renew a host");
    credentials
        .membership
        .verify(key.id(), credentials.delegation.as_ref(), START)
        .expect("the renewed certificate verifies");
    assert!(
        credentials.membership.granted(Grant::HostRuns, START),
        "renewal has to carry the grant that made this node worth having"
    );
    // …and it is a renewal rather than a fresh grant, which is the whole of what makes it
    // acceptable: what it may restate is bounded by a certificate the fleet key signed.
    assert!(
        credentials
            .membership
            .authority
            .as_ref()
            .is_some_and(|a| a.grants.contains(&Grant::HostRuns)),
        "the renewal must carry the authority it is restating"
    );
    builder.membership.take_up(credentials);

    desktop.cluster.probe_round(START).await;
    assert_eq!(desktop.status_of(id(2)), Some(NodeStatus::Alive));
}

#[tokio::test]
async fn a_member_that_is_not_an_approver_says_so_rather_than_signing() {
    // The ordinary answer, and the reason a refusal is a sentence: the asking node tries peers
    // in turn, so "I am not an approver" has to be distinguishable from "your papers are wrong".
    let net = MemoryNetwork::new();
    let key = fleet();
    let desktop = join(&net, &key, 1, "desktop");
    let laptop = join(&net, &key, 2, "laptop");
    introduce(&desktop, &laptop);

    let refusal = laptop
        .cluster
        .renew_with(id(1), Millis(1_000), laptop.membership.credentials())
        .await
        .expect_err("nobody delegated to it");
    assert!(refusal.contains("not an approver"), "{refusal}");
}

/// ADR-0080: a request to read model lists again reaches a node that was not asked, one hop
/// further, and wakes whoever is watching there. The count merges by maximum, so an older, smaller
/// count arriving afterwards does not take it back down. And a count a node hears in its **first**
/// exchange is adopted without waking it: it predates the node, whose startup read answers it.
#[tokio::test]
async fn a_request_to_read_models_again_travels_by_gossip_and_only_rises() {
    // Every member probes a few times: which peer a round picks is not the point here, only
    // that the count gets everywhere by gossip.
    async fn spread(members: &[&Member]) {
        for _ in 0..4 {
            for m in members {
                m.cluster.probe_round(START).await;
            }
        }
    }
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "desktop");
    let c = join(&net, &key, 3, "mac");
    introduce(&a, &b);
    introduce(&b, &c);
    // Everybody has had a first exchange before anybody asks.
    spread(&[&a, &b, &c]).await;
    let mut watching = c.cluster.models_asked();
    assert_eq!(*watching.borrow_and_update(), 0);

    assert_eq!(a.cluster.ask_for_models(), 1);
    assert_eq!(a.cluster.ask_for_models(), 2);
    spread(&[&a, &b, &c]).await;
    assert_eq!(*b.cluster.models_asked().borrow(), 2);
    assert!(
        watching.has_changed().unwrap_or(false),
        "c's watcher was woken"
    );
    assert_eq!(
        *watching.borrow_and_update(),
        2,
        "c heard it by gossip, never asked directly"
    );

    // c asks once more; the older count still circulating does not take it back down.
    assert_eq!(c.cluster.ask_for_models(), 3);
    spread(&[&a, &b, &c]).await;
    for m in [&a, &b, &c] {
        assert_eq!(*m.cluster.models_asked().borrow(), 3);
    }

    // A node that starts now hears the count in its first exchange, and is not woken by it.
    let d = join(&net, &key, 4, "phone");
    introduce(&d, &c);
    let fresh = d.cluster.models_asked();
    spread(&[&d]).await;
    assert_eq!(*fresh.borrow(), 3, "it adopted the fleet's count");
    assert!(
        !fresh.has_changed().unwrap_or(true),
        "without waking its reader"
    );
}

/// A peer that was away still has a finished run as live, and gossips it. The node that knows
/// better is told, through `Host::stale_copy`, so it can publish the finished record again; and
/// its answer to the peer carries that record, which the peer takes (session ninety-four: the
/// tablet kept a cancelled run `pending` because nobody told it twice).
#[tokio::test]
async fn a_peer_with_a_stale_copy_of_a_finished_run_is_noticed_and_put_right() {
    let net = MemoryNetwork::new();
    let key = fleet();
    let a = join(&net, &key, 1, "laptop");
    let b = join(&net, &key, 2, "tablet");
    let host_a = TestHost::bidding(0);
    a.cluster.hosts_runs(host_a.clone());
    b.cluster.hosts_runs(TestHost::bidding(0));
    introduce(&a, &b);

    let pending = a_run(a.cluster.node());
    let mut cancelled = pending.clone();
    cancelled.cancel(START).expect("cancel");
    a.cluster.publish_run(cancelled.clone());
    b.cluster.publish_run(pending.clone());

    // b's round is an exchange with a: a hears the stale copy, and b hears the news.
    b.cluster.probe_round(START).await;
    assert_eq!(
        *host_a.stale.lock().expect("lock"),
        vec![pending.id],
        "the node that knows better was told the peer is behind"
    );
    assert!(
        b.cluster
            .view()
            .runs
            .get(&pending.id)
            .is_some_and(|run| run.state.is_terminal()),
        "and the peer took the finished record from the answer"
    );

    // Up to date now: another exchange is not a stale copy.
    b.cluster.probe_round(START).await;
    assert_eq!(host_a.stale.lock().expect("lock").len(), 1);
}
