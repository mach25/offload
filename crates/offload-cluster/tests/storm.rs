//! The same claims as `offload-core/tests/churn.rs`, made through the code that actually runs.
//!
//! `churn.rs` drives a fleet of `ClusterView`s: `merge_run` and `supervise` are pure functions,
//! so a partition is a fact about which deliveries a loop performs. It says so in its own header,
//! and it names the limit that leaves — *"a node holding a run means its own view says so"*. This
//! file is that limit closed. The nodes here are real `Cluster`s: they encode gossip and decode
//! it, handshake, run the real SWIM detector, and place runs through the real bid round, over a
//! transport that carries bytes. What is generated is the churn.
//!
//! **Nothing here sleeps.** The clock is an argument (`Cluster::probe_round(now)`) and the network
//! is `MemoryNetwork`, so a partition happens at an exact instant and heals at another. That is
//! the same arrangement `mesh.rs` uses; what is new is that the sequence is not arranged by hand.
//!
//! **The honest limits, before the properties so they are not mistaken for oversights.**
//!
//! * *A node that is down here is a node that is unreachable, not one that crashed.* This layer
//!   cannot tell the difference and ADR-0007 insists it must not try — `Suspect` before `Dead`,
//!   and neither one moves a run on its own. Losing in-memory state across a restart is a real
//!   thing that happens, and `churn.rs` models it; a `Cluster` cannot be restarted in place
//!   without rebuilding it, and pretending an isolate-and-rejoin is a crash would be modelling
//!   the wrong failure with the right name.
//! * *A persistent partition still grants a run twice, and that is not a bug here.* ADR-0002's
//!   amendment says so: without quorum, two arbiting sides that cannot hear each other will each
//!   grant, and what fencing buys is that exactly one survives the moment the records meet. So
//!   the strong properties below are checked at the **fixpoint** — healed, and gossiped until
//!   nothing changes — and the per-step invariants are deliberately weak.
//! * *No agents, again.* A host here accepts or refuses; there is no process. What that costs is
//!   covered by `offload-node`'s own tests, which drive real ones.

// Same exemption and same reason as `churn.rs`: panicking is how a property reports a
// counterexample, and this file is nothing but properties.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use offload_cluster::blobs::Blobs;
use offload_cluster::{Clock, Cluster, DetectorConfig, Host};
use offload_core::fleet::FleetKey;
use offload_core::run::{Checkpoint, Epoch};
use offload_core::{
    AgentWork, Arch, Availability, BlobHash, Capabilities, Constraint, DeviceClass, FleetId,
    MembershipCert, Millis, NodeId, NodeView, Offer, Os, PermissionMode, Restartability,
    Revocation, Run, RunId, RunProgress, RunSpec, Score, Terms, ToolAllowlist, Work, WorkPolicy,
    WorkspaceSpec,
};
use offload_proto::handshake::Credentials;
use offload_transport::memory::MemoryNetwork;
use offload_transport::{Membership, Transport};
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const START: Millis = Millis(1_700_000_000_000);
const RUN: RunId = RunId::from_bytes([7; 16]);
/// Short enough that a round costs nothing when a peer is unreachable — the memory transport
/// refuses a cut connection immediately rather than hanging, so this bounds only real silence.
const WINDOW: Millis = Millis(200);

fn signing(seed: u8) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[seed; 32])
}

fn id(seed: u8) -> NodeId {
    NodeId::from_bytes(signing(seed).verifying_key().to_bytes())
}

fn fleet_key() -> FleetKey {
    FleetKey::derive("abacus zoom yo-yo").expect("derive")
}

/// Membership and the clock, which is all this file needs from either.
///
/// Deliberately not shared with `mesh.rs`'s richer harness: that one models renewal, revocation
/// and approvers because those are what it tests. Everything here is a founder that never
/// changes, and a smaller fixture is one less thing to read when a property fails.
struct Card {
    fleet: FleetId,
    credentials: Credentials,
    now: AtomicU64,
}

impl Card {
    fn new(key: &FleetKey, seed: u8, name: &str) -> Arc<Card> {
        Arc::new(Card {
            fleet: key.id(),
            credentials: Credentials {
                membership: MembershipCert::issue(
                    key.signing_key(),
                    offload_core::Issuer::Fleet,
                    Terms::founding(key.id(), id(seed), name, START),
                ),
                delegation: None,
            },
            now: AtomicU64::new(START.0),
        })
    }

    fn set(&self, now: Millis) {
        self.now.store(now.0, Ordering::SeqCst);
    }
}

impl Membership for Card {
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
        Millis(self.now.load(Ordering::SeqCst))
    }
}

impl Clock for Card {
    fn now(&self) -> Millis {
        Millis(self.now.load(Ordering::SeqCst))
    }
}

/// A node that takes work and remembers what it took.
///
/// The grants are the point: a `Placement::Accepted` is a promise that one machine and no other
/// is to run this, and the epoch on the record is the token that promise is made under. Every
/// one is kept, so a property can ask afterwards whether the same token was ever handed out
/// twice — which is the whole of what fencing claims.
#[derive(Debug, Default)]
struct Taker {
    score: i64,
    accepted: Mutex<Vec<(RunId, Epoch)>>,
    /// Grants to refuse before accepting again.
    ///
    /// Without this the simulation never reaches a round's **second** attempt, because the first
    /// winner always says yes — and the second attempt is where the fencing token is spent twice
    /// if it is spent wrongly. A node declining after it bid is not exotic: ADR-0006 step 6 is
    /// exactly that, a node that got busy between answering and being granted.
    declines: std::sync::atomic::AtomicUsize,
    /// Grants to **take** while telling the arbiter they were not taken.
    ///
    /// ADR-0006's silence-is-a-decline makes this the ordinary case rather than an exotic one: an
    /// answer that does not come back is indistinguishable from a refusal, and the node on the
    /// other end is running the work under the epoch it was handed. It is the *only* shape in
    /// which one arbiter can leave two live grants, so a simulation without it cannot see the
    /// bug — which is how the first version of this file passed with the fix reverted.
    swallows: std::sync::atomic::AtomicUsize,
    /// Runs this node was told about but does not hold.
    recorded: Mutex<Vec<Run>>,
    /// Every grant this node *reported making*, as its own arbiter, via `Host::granted`.
    ///
    /// Distinct from `accepted`, which is what a node was granted. This is the other end — the
    /// decision, from the machine that made it — and the reason it is recorded separately is the
    /// bug it exists to guard: the report used to be written from `Host::record`, which is also
    /// the hook `Cluster::learn` calls for **every** gossip merge, so one ordinary run produced
    /// three reports of a grant at one epoch. That is the audit log's own signature for an
    /// arbiter spending a fencing token twice, said about a run where nothing went wrong.
    reported: Mutex<Vec<(RunId, Epoch, NodeId)>>,
    /// This leg's own turn counter, and the last numbers it published.
    ///
    /// A leg does not start counting from one: a resumed agent is a new process and numbers its
    /// turns from one, which `accumulate_turns` offsets by the checkpoint it resumed from, so the
    /// *run* continues its count rather than restarting it. Modelled here for the same reason it
    /// exists there — without the offset an ordinary migration would look like a run going
    /// backwards, and the property below would be reporting the harness.
    ///
    /// Two legs of a **forked** run therefore start from the same number and diverge, which is
    /// the whole of what this models.
    turns: Mutex<u32>,
    published: Mutex<Option<RunProgress>>,
    /// Weak for `NodeHost`'s reason — the cluster holds this host, and an `Arc` both ways is a
    /// fleet that never frees itself. Set after construction for the same reason: the host
    /// answers for the cluster that asks.
    cluster: std::sync::OnceLock<std::sync::Weak<Cluster>>,
}

impl Taker {
    fn new(score: i64) -> Arc<Taker> {
        Arc::new(Taker {
            score,
            ..Taker::default()
        })
    }

    fn grants(&self) -> Vec<(RunId, Epoch)> {
        self.accepted.lock().expect("lock").clone()
    }

    /// What this node says it granted, and to whom.
    fn reported(&self) -> Vec<(RunId, Epoch, NodeId)> {
        self.reported.lock().expect("lock").clone()
    }

    /// Refuse the next grant, the way a node that filled up between bidding and being asked
    /// does.
    fn decline_next(&self) {
        self.declines.fetch_add(1, Ordering::SeqCst);
    }

    /// Take the next grant and let the answer go missing.
    fn swallow_next(&self) {
        self.swallows.fetch_add(1, Ordering::SeqCst);
    }

    fn take_one(counter: &std::sync::atomic::AtomicUsize) -> bool {
        counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
    }

    fn publish(&self, run: Run) {
        if let Some(cluster) = self.cluster.get().and_then(std::sync::Weak::upgrade) {
            cluster.publish_run(run);
        }
    }

    /// The last numbers this leg published, or `None` if it never did a turn.
    fn published(&self) -> Option<RunProgress> {
        self.published.lock().expect("lock").clone()
    }
}

#[async_trait::async_trait]
impl Host for Taker {
    async fn evaluate(&self, _run: &Run) -> Result<Offer, String> {
        Ok(Offer {
            score: Score(self.score),
            available: Availability::Now,
            terms: None,
        })
    }

    async fn accept(&self, run: Run) -> Result<(), String> {
        if Taker::take_one(&self.declines) {
            return Err("busy since bidding".into());
        }
        // Where this leg's turns start: the checkpoint the record it was granted carries. Not
        // the run's gossiped *progress*, and the difference is the point — a resuming agent is
        // handed a transcript, so what it continues from is the position that travelled on the
        // record and was fenced, never the high-water mark somebody gossiped.
        *self.turns.lock().expect("lock") = run.checkpoint.as_ref().map_or(0, |c| c.turns);
        // Taken, and the arbiter is about to be told otherwise. The order matters and is the
        // whole point: this node is now running under `run.epoch`, whatever the round does next.
        if Taker::take_one(&self.swallows) {
            self.accepted
                .lock()
                .expect("lock")
                .push((run.id, run.epoch));
            self.publish(run);
            return Err("the answer never came back".into());
        }
        self.accepted
            .lock()
            .expect("lock")
            .push((run.id, run.epoch));
        // Straight into the view, not at the next tick — which is what `Mesh::accept` does and
        // why: `evaluate` answers the next round from the view, so a node whose view has not
        // caught up with what it just accepted bids as though it were idle. Without this the
        // holder is the one node in the fleet that does not know it holds the run, and a single
        // `Place` is enough to show it.
        self.publish(run);
        Ok(())
    }

    async fn granted(&self, run: &Run) {
        // Called on the node whose `place` ran, once per grant. Recorded rather than counted so a
        // failure names the run and the epoch it was reported at twice.
        if let Some(holder) = run.holder() {
            self.reported
                .lock()
                .expect("lock")
                .push((run.id, run.epoch, holder));
        }
    }

    async fn record(&self, run: &Run) {
        // Remembered and **not** published, which is what `Mesh::record` does: this is a run
        // placed somewhere else, so the view learns about it by merging gossip, under the epoch
        // rules. Publishing it here writes straight into the view with no epoch check — the
        // right thing for a decision this node just made and quite wrong for somebody else's,
        // and it made a node's view appear to walk its epoch backwards.
        self.recorded.lock().expect("lock").push(run.clone());
    }
}

/// A node's blob store, which is the checkpoint half of the fleet (ADR-0016).
#[derive(Debug, Default)]
struct Bag {
    held: Mutex<BTreeMap<BlobHash, Vec<u8>>>,
}

#[async_trait::async_trait]
impl Blobs for Bag {
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
}

/// The bytes every `Store` event writes, so every node's copy has one hash.
const CHECKPOINT: &[u8] = b"turn nineteen";

fn checkpoint_hash() -> BlobHash {
    BlobHash::from_bytes(*blake3::hash(CHECKPOINT).as_bytes())
}

/// What a capture writes onto the record at a turn boundary.
///
/// The same bytes on every node, as everywhere else in this file: what diverges between two legs
/// of a forked run is the **turn** its checkpoint is at, and giving each leg its own blob would
/// only be modelling a hash comparison nothing here makes.
fn checkpoint_at(turns: u32, now: Millis) -> Checkpoint {
    Checkpoint {
        session_id: None,
        transcript: checkpoint_hash(),
        bundle: None,
        patch: None,
        base_commit: "0000000".into(),
        turns,
        taken_at: now,
        agent_version: "2.1.238".into(),
        replicas: Default::default(),
    }
}

/// And what it publishes beside the record: where the run has got to, and whose checkout that is.
///
/// The author is in the worktree summary because that is what a summary *is* — "2 modified, 1
/// new" about one machine's checkout — and a string is how the fleet carries it. Two legs of one
/// run produce two different sentences about two different directories, which is the whole
/// difficulty: only one of them is a description of the run.
///
/// Stamped with the leg, which is what `Supervisor::update_stats` does for every number the
/// daemon writes. Not decoration: without it these records are the *unstamped* kind — an older
/// build's row — and the merge falls back to comparing them forward-only, so a harness that
/// skipped this would be exercising the rule this file exists to test out of existence.
fn progress_at(turns: u32, by: NodeId, epoch: Epoch, now: Millis) -> RunProgress {
    RunProgress {
        turns,
        workspace: format!("{turns} modified on {}", by.short()),
        at: now,
        by: Some(by),
        epoch,
        ..RunProgress::default()
    }
}

fn detector() -> DetectorConfig {
    DetectorConfig {
        probe_interval: Millis(1_000),
        probe_timeout: Millis(500),
        suspect_timeout: Millis(5_000),
        indirect_peers: 3,
    }
}

fn a_run(home: NodeId) -> Run {
    Run::new(
        RUN,
        RunSpec {
            work: Work::Agent(AgentWork {
                agent: offload_core::AgentKind::ClaudeCode,
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
            demand: offload_core::Demand::Normal,
            notify: offload_core::Audience::default(),
            notices: Default::default(),
            resources: Vec::new(),
            prefer: offload_core::Constraint::Always,
            hold_until: None,
            parent: None,
        },
        home,
        START,
    )
}

/// One node of the simulated fleet.
struct Member {
    cluster: Arc<Cluster>,
    card: Arc<Card>,
    host: Arc<Taker>,
    blobs: Arc<Bag>,
}

/// The fleet, the wire between its members, and the clock they all read.
struct Storm {
    net: MemoryNetwork,
    members: Vec<Member>,
    now: Millis,
    /// Unordered index pairs that cannot reach each other, kept beside the network so `heal_all`
    /// knows what to undo. The network itself is the authority; this is the record.
    cut: BTreeSet<(usize, usize)>,
    /// The highest epoch each node has ever had in its view for the run. A view that walks one
    /// back has forgotten a decision, which is the invariant checked after every single step.
    high: BTreeMap<NodeId, Epoch>,
    /// Grants made to a node the arbiter could not reach at the time. Should always be empty —
    /// see the property. Collected rather than asserted inline so a counterexample shrinks to
    /// the script that caused it rather than to the step that noticed.
    unreachable_grants: Vec<(NodeId, NodeId)>,
    /// Every grant that was made, and by whom: `(arbiter, holder, run, epoch)`.
    ///
    /// Attributed by diffing the hosts' own records across a round, because the arbiter is not
    /// on the record it hands out — putting it there is open question #6 and costs a wire
    /// version. Exact here regardless: the driver runs one round at a time.
    granted: Vec<(NodeId, NodeId, RunId, Epoch)>,
}

impl Storm {
    fn new(size: usize) -> Storm {
        let key = fleet_key();
        let net = MemoryNetwork::new();
        let members: Vec<Member> = (0..size)
            .map(|i| {
                let seed = u8::try_from(i + 1).expect("small fleet");
                let node = id(seed);
                let name = format!("n{i}");
                let card = Card::new(&key, seed, &name);
                let transport: Arc<dyn Transport> =
                    Arc::new(net.join(node, name.clone(), card.clone() as Arc<dyn Membership>));
                let mut view = NodeView::new(
                    node,
                    Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Desktop),
                    WorkPolicy::for_class(DeviceClass::Desktop),
                    START,
                );
                view.capabilities.cpu_cores = 8;
                let blobs = Arc::new(Bag::default());
                let cluster = Cluster::new(
                    transport,
                    view.named(&name),
                    detector(),
                    card.clone() as Arc<dyn Clock>,
                    blobs.clone() as Arc<dyn Blobs>,
                );
                // Scores differ per node so the winner of a round is decided rather than tied,
                // which keeps a counterexample about churn rather than about tiebreaks.
                let host = Taker::new(i64::try_from(i).expect("small fleet") * 10);
                host.cluster
                    .set(Arc::downgrade(&cluster))
                    .expect("set once");
                cluster.hosts_runs(host.clone());
                tokio::spawn(cluster.clone().serve());
                Member {
                    cluster,
                    card,
                    host,
                    blobs,
                }
            })
            .collect();

        // Everybody knows of everybody, the way a seed list would leave them.
        for a in &members {
            for b in &members {
                if a.cluster.node() != b.cluster.node() {
                    a.cluster.introduce(
                        b.cluster.node(),
                        Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Unknown),
                        WorkPolicy::for_class(DeviceClass::Desktop),
                    );
                }
            }
        }

        // The run exists once, submitted at member 0, the way a `RunId` is minted once by the
        // daemon that took the operator's command. Publishing it here rather than letting
        // `place` conjure one per arbiter is not tidiness: two nodes independently creating a
        // record under the same id is a thing that cannot happen, and simulating it made a
        // node's view appear to walk its epoch backwards — an invariant violation that was
        // entirely the simulation's own.
        members[0]
            .cluster
            .publish_run(a_run(members[0].cluster.node()));

        Storm {
            net,
            members,
            now: START,
            cut: BTreeSet::new(),
            high: BTreeMap::new(),
            unreachable_grants: Vec::new(),
            granted: Vec::new(),
        }
    }

    fn node(&self, i: usize) -> NodeId {
        self.members[i % self.members.len()].cluster.node()
    }

    fn at(&self, i: usize) -> &Member {
        &self.members[i % self.members.len()]
    }

    /// Move every node's clock together. Skew is a separate axis and this file does not vary it:
    /// what it would exercise is `Millis` arithmetic, which `offload-core`'s properties already
    /// drive, and it would make every failure here ambiguous between the two.
    fn advance(&mut self, secs: u64) {
        self.now = Millis(self.now.0 + secs * 1_000);
        for m in &self.members {
            m.card.set(self.now);
        }
    }

    async fn tick(&mut self, i: usize) {
        let now = self.now;
        self.at(i).cluster.probe_round(now).await;
    }

    fn cut(&mut self, a: usize, b: usize) {
        let (a, b) = (a % self.members.len(), b % self.members.len());
        if a == b {
            return;
        }
        let key = if a < b { (a, b) } else { (b, a) };
        self.net.partition(self.node(a), self.node(b));
        self.cut.insert(key);
    }

    fn heal(&mut self, a: usize, b: usize) {
        let (a, b) = (a % self.members.len(), b % self.members.len());
        if a == b {
            return;
        }
        let key = if a < b { (a, b) } else { (b, a) };
        self.net.heal(self.node(a), self.node(b));
        self.cut.remove(&key);
    }

    /// Take a node off the network entirely: unreachable, not crashed. See the header.
    fn isolate(&mut self, i: usize) {
        for j in 0..self.members.len() {
            if j != i % self.members.len() {
                self.cut(i, j);
            }
        }
    }

    fn heal_all(&mut self) {
        for (a, b) in std::mem::take(&mut self.cut) {
            self.net.heal(self.node(a), self.node(b));
        }
    }

    /// One real bid round, arbitrated by node `i`.
    ///
    /// The run offered is whatever this node currently believes about it, so a second round on a
    /// node that already knows it was granted elsewhere is exactly what a re-offer looks like in
    /// the field — including the epoch it carries, which is the fencing token under test.
    async fn place(&mut self, i: usize) {
        let me = self.node(i);
        let view = self.at(i).cluster.view();
        // Only the run's **arbiter** may place it, and that is not a convenience — it is
        // `arbiter_for`, the rule the daemon follows, and the reason the epoch means anything.
        // An epoch is monotonic *per arbiter*: it is `next()` computed from the arbiter's own
        // record, so a second node placing the same run is a second counter, and every guarantee
        // built on the number stops holding. Letting any node place here produced exactly that —
        // a node granted the run at epoch 1 while another already held it at epoch 3, and the
        // recipient's own view walked backwards to take it. Real, and reachable only by breaking
        // the rule the simulation is supposed to be modelling.
        let Some(run) = view.runs.get(&RUN).cloned() else {
            return;
        };
        if view.arbiter_for(&run) != Some(me) || run.state.is_terminal() {
            return;
        }
        drop(view);
        // This node's own answer, which `place` takes as an argument rather than asking for:
        // the daemon computes it from local facts a peer cannot see. Here the host is the whole
        // of those facts.
        let mine = self.at(i).host.evaluate(&run).await;
        let before = self.all_grants();
        let _ = self.at(i).cluster.place(&run, mine, WINDOW).await;
        for (holder, run, epoch) in self.all_grants() {
            if !before.contains(&(holder, run, epoch)) {
                if !self.can_reach(i, holder) {
                    self.unreachable_grants.push((me, holder));
                }
                self.granted.push((me, holder, run, epoch));
            }
        }
    }

    /// Could the arbiter at index `i` reach `holder` when the round ran? Itself always.
    fn can_reach(&self, i: usize, holder: NodeId) -> bool {
        let i = i % self.members.len();
        let Some(j) = (0..self.members.len()).find(|j| self.node(*j) == holder) else {
            return false;
        };
        if i == j {
            return true;
        }
        let key = if i < j { (i, j) } else { (j, i) };
        !self.cut.contains(&key)
    }

    /// A turn boundary on node `i`, if the run is that node's to work on.
    ///
    /// Everything a real boundary does that leaves the machine, and nothing else: the capture
    /// onto the record (the shipped cadence is one per boundary — `CheckpointConfig::every_turns`
    /// — so position reaches the *record* as well as the numbers), the turn count
    /// (`record_event`'s `stats.turns = turn`) and the worktree summary beside it, refreshed at
    /// every boundary rather than at every capture.
    ///
    /// Guarded by this node's own view naming it holder, which is `Supervisor::describes` — the
    /// question the writer asks now rather than each caller. So nothing below can be blamed on a
    /// leg that had already lost and went on typing, which is the known window the last session
    /// left alone deliberately. During a partition **both** legs pass this guard, because each
    /// one's view says the run is its own, and each one is right.
    async fn turn(&mut self, i: usize) {
        let now = self.now;
        let me = self.node(i);
        let Some(mut run) = self.at(i).cluster.view().runs.get(&RUN).cloned() else {
            return;
        };
        if run.holder() != Some(me) {
            return;
        }
        let epoch = run.epoch;
        let turns = {
            let mut count = self.at(i).host.turns.lock().expect("lock");
            *count += 1;
            *count
        };
        let _ = self.at(i).blobs.store(CHECKPOINT.to_vec()).await;
        // Fenced, exactly as the daemon's capture is: a leg whose record has moved past it is
        // refused here and has nothing to say.
        if run
            .record_checkpoint(me, epoch, checkpoint_at(turns, now))
            .is_err()
        {
            return;
        }
        let numbers = progress_at(turns, me, epoch, now);
        self.at(i).cluster.publish_run(run);
        self.at(i)
            .cluster
            .publish_progress(vec![(RUN, numbers.clone())]);
        *self.at(i).host.published.lock().expect("lock") = Some(numbers);
    }

    /// Write the checkpoint on node `i`, as a turn boundary does.
    async fn store(&mut self, i: usize) {
        let _ = self.at(i).blobs.store(CHECKPOINT.to_vec()).await;
    }

    /// Replicate it, the way `Mesh::replicate` does after a capture (ADR-0016).
    async fn push(&mut self, i: usize, j: usize) {
        let to = self.node(j);
        if to == self.node(i) {
            return;
        }
        let _ = self.at(i).cluster.push_blob(to, checkpoint_hash()).await;
    }

    /// Try to get it, the way a node restoring a migrated run does. `None` for the hint, so
    /// what is under test is the sweep rather than the one peer it is told about.
    async fn fetch(&mut self, i: usize) -> bool {
        self.at(i)
            .cluster
            .fetch_blob(checkpoint_hash(), None)
            .await
            .is_ok()
    }

    /// Which nodes hold the checkpoint right now.
    async fn holders_of_blob(&self) -> Vec<usize> {
        let hash = checkpoint_hash();
        let mut out = Vec::new();
        for (i, m) in self.members.iter().enumerate() {
            if m.blobs.has(hash).await {
                out.push(i);
            }
        }
        out
    }

    /// Could node `i` reach node `j` right now?
    fn reaches(&self, i: usize, j: usize) -> bool {
        if i == j {
            return true;
        }
        let key = if i < j { (i, j) } else { (j, i) };
        !self.cut.contains(&key)
    }

    /// What every node currently says about who holds the run, and under what epoch.
    fn answers(&self) -> Vec<(NodeId, Option<NodeId>, Epoch)> {
        self.members
            .iter()
            .map(|m| {
                let view = m.cluster.view();
                let run = view.runs.get(&RUN);
                (
                    m.cluster.node(),
                    run.and_then(offload_core::Run::holder),
                    run.map_or(Epoch(0), |r| r.epoch),
                )
            })
            .collect()
    }

    /// Gossip until nothing changes, or give up and say so.
    ///
    /// "Nothing changes" has to mean nothing changes for a **whole rotation**, not for one pass,
    /// and that distinction is the whole of what makes this function honest. SWIM probes one peer
    /// per period, so a node's opinion of any particular peer is revisited once every
    /// `peers` rounds — which means two consecutive identical passes are the *ordinary* state of
    /// a fleet mid-recovery rather than evidence of a fixpoint. Written the eager way first, this
    /// declared a fleet settled at round 1 while a node it had marked `Dead` came back at round 4
    /// and everybody agreed by round 5.
    ///
    /// That is `churn.rs`'s lesson arriving by a different route — its first version stopped as
    /// soon as the run record stopped moving and reported a fleet still arguing about liveness as
    /// converged. Here the records had settled and the *membership* had not, which is the same
    /// error with the halves swapped.
    async fn settle(&mut self) -> bool {
        // Quiet for long enough that every node has had a turn at probing even the peers it has
        // concluded dead, which it does one round in `DEAD_EVERY` (session ninety-two). Two nodes
        // that marked each other dead during a partition heal when either visits the other, and a
        // window shorter than that called a fleet at rest while both still said `Dead`.
        let dead_every = usize::try_from(offload_cluster::DEAD_EVERY).unwrap_or(30);
        let quiet_enough = self.members.len() + 1 + dead_every;
        let mut quiet = 0;
        for _ in 0..(80 + 2 * dead_every) {
            let before = (self.answers(), self.statuses());
            for i in 0..self.members.len() {
                self.tick(i).await;
            }
            if (self.answers(), self.statuses()) == before {
                quiet += 1;
                if quiet >= quiet_enough {
                    return true;
                }
            } else {
                quiet = 0;
            }
        }
        false
    }

    fn statuses(&self) -> Vec<Vec<offload_core::NodeStatus>> {
        self.members
            .iter()
            .map(|m| {
                let view = m.cluster.view();
                let mut out: Vec<_> = view.nodes.values().map(|n| n.status).collect();
                out.sort_by_key(|s| format!("{s:?}"));
                out
            })
            .collect()
    }

    /// The last numbers the node named published, if it ever did a turn.
    fn published_by(&self, node: NodeId) -> Option<RunProgress> {
        self.members
            .iter()
            .find(|m| m.cluster.node() == node)
            .and_then(|m| m.host.published())
    }

    /// What every node currently says about where the run has got to.
    fn positions(&self) -> Vec<(NodeId, Option<(u32, String)>)> {
        self.members
            .iter()
            .map(|m| {
                let view = m.cluster.view();
                let said = view
                    .progress
                    .get(&RUN)
                    .map(|p| (p.turns, p.workspace.clone()));
                (m.cluster.node(), said)
            })
            .collect()
    }

    /// Every grant any node has accepted, across the whole simulation.
    fn all_grants(&self) -> Vec<(NodeId, RunId, Epoch)> {
        self.members
            .iter()
            .flat_map(|m| {
                let node = m.cluster.node();
                m.host.grants().into_iter().map(move |(r, e)| (node, r, e))
            })
            .collect()
    }

    /// The invariant that has to hold at every instant, partition or no partition.
    ///
    /// A view that walks an epoch backwards has forgotten a decision that was already made, and
    /// everything downstream — fencing, reassignment, the merge itself — is built on the epoch
    /// only ever going up. `churn.rs` checks this over pure merges; here it is checked over
    /// records that have been encoded, sent, decoded and merged.
    fn check_epochs(&mut self) -> Result<(), TestCaseError> {
        for (node, _, epoch) in self.answers() {
            let seen = self.high.entry(node).or_insert(Epoch(0));
            prop_assert!(
                epoch >= *seen,
                "{} walked its epoch back from {} to {}",
                node.short(),
                seen.0,
                epoch.0
            );
            *seen = epoch;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum Ev {
    Tick(u8),
    Advance(u8),
    Cut(u8, u8),
    Heal(u8, u8),
    Isolate(u8),
    Place(u8),
    /// Arm a node to refuse its next grant. What makes a round's second attempt reachable at
    /// all, and with it the only path on which an epoch can be spent twice by one arbiter.
    Decline(u8),
    /// Arm a node to take its next grant and lose the answer — ADR-0006's silence-is-a-decline,
    /// which is the case a fencing token has to survive.
    Swallow(u8),
    /// Do a turn: capture, count it, and say where the run has got to.
    Turn(u8),
    /// Write the checkpoint here, as a turn boundary does.
    Store(u8),
    /// Replicate it to another node (ADR-0016).
    Push(u8, u8),
}

fn ev() -> impl Strategy<Value = Ev> {
    prop_oneof![
        4 => (0u8..4).prop_map(Ev::Tick),
        2 => (1u8..8).prop_map(Ev::Advance),
        2 => ((0u8..4), (0u8..4)).prop_map(|(a, b)| Ev::Cut(a, b)),
        2 => ((0u8..4), (0u8..4)).prop_map(|(a, b)| Ev::Heal(a, b)),
        1 => (0u8..4).prop_map(Ev::Isolate),
        3 => (0u8..4).prop_map(Ev::Place),
        2 => (0u8..4).prop_map(Ev::Decline),
        2 => (0u8..4).prop_map(Ev::Swallow),
        3 => (0u8..4).prop_map(Ev::Turn),
        2 => (0u8..4).prop_map(Ev::Store),
        2 => ((0u8..4), (0u8..4)).prop_map(|(a, b)| Ev::Push(a, b)),
    ]
}

/// Run a generated script, checking the per-step invariant as it goes.
async fn run_storm(size: usize, events: &[Ev]) -> Result<Storm, TestCaseError> {
    let mut storm = Storm::new(size);
    for e in events {
        match *e {
            Ev::Tick(i) => storm.tick(i as usize).await,
            Ev::Advance(s) => storm.advance(u64::from(s)),
            Ev::Cut(a, b) => storm.cut(a as usize, b as usize),
            Ev::Heal(a, b) => storm.heal(a as usize, b as usize),
            Ev::Isolate(i) => storm.isolate(i as usize),
            Ev::Place(i) => storm.place(i as usize).await,
            Ev::Decline(i) => storm.at(i as usize).host.decline_next(),
            Ev::Swallow(i) => storm.at(i as usize).host.swallow_next(),
            Ev::Turn(i) => storm.turn(i as usize).await,
            Ev::Store(i) => storm.store(i as usize).await,
            Ev::Push(a, b) => storm.push(a as usize, b as usize).await,
        }
        storm.check_epochs()?;
    }
    Ok(storm)
}

/// Drive a script and then let the fleet talk itself out.
async fn run_and_settle(size: usize, events: &[Ev]) -> Result<Storm, TestCaseError> {
    let mut storm = run_storm(size, events).await?;
    storm.heal_all();
    // Far enough past every timeout that suspicion has either expired or been refuted: what is
    // being asked is what the fleet agrees on at rest, not what it believes mid-detection.
    storm.advance(60);
    prop_assert!(
        storm.settle().await,
        "the fleet never stopped changing its mind"
    );
    Ok(storm)
}

fn sim<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime")
        .block_on(f)
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Each case builds a fleet of real clusters and runs bid rounds over them, which is
        // orders of magnitude more expensive than `churn.rs`'s pure merges. The default 256
        // would make `cargo test` unpleasant; `PROPTEST_CASES` raises it for a real search.
        cases: 24,
        max_shrink_iters: 2_000,
        ..ProptestConfig::default()
    })]

    /// The claim ADR-0002 makes about fencing, checked against the grants that were actually
    /// handed out rather than against the records left behind.
    ///
    /// An epoch is the fencing token for **one** grant. `hand_over` spends one per attempt for
    /// exactly this reason — a node whose answer was lost still ran under the token it was given,
    /// so re-using it for the next node in the same round is two live grants at one epoch. That
    /// bug shipped, and the churn properties found it at the merge; this finds it at the source,
    /// where the grant is made.
    ///
    /// Scoped to **one arbiter's** grants, because two arbiters producing one epoch is the
    /// partition case the ADR's amendment accepts and the settling property below is what covers
    /// it.
    #[test]
    fn one_arbiter_never_spends_an_epoch_twice(events in prop::collection::vec(ev(), 1..24)) {
        let storm = sim(run_storm(4, &events))?;
        let mut seen: BTreeMap<(NodeId, RunId, Epoch), NodeId> = BTreeMap::new();
        for (arbiter, holder, run, epoch) in storm.granted {
            if let Some(first) = seen.insert((arbiter, run, epoch), holder) {
                prop_assert_eq!(
                    first, holder,
                    "{} granted run {} at epoch {} to {} and to {}",
                    arbiter.short(), run.short(), epoch.0, first.short(), holder.short()
                );
            }
        }
    }

    /// Once the network is whole and the gossip stops, the fleet holds one answer.
    ///
    /// The same sentence as `churn.rs`'s, one layer down: these records were serialised, sent
    /// through a handshake, decoded and merged, so this also says the wire round-trips whatever
    /// the merge rules depend on. An encoding that dropped the lease would pass every unit test
    /// in `offload-proto` and fail here.
    #[test]
    fn a_settled_fleet_agrees_who_holds_the_run(events in prop::collection::vec(ev(), 1..24)) {
        let storm = sim(run_and_settle(4, &events))?;
        let answers = storm.answers();
        let (_, holder, epoch) = answers[0];
        for (node, h, e) in &answers {
            prop_assert_eq!(
                (*h, *e), (holder, epoch),
                "{} says {:?} at epoch {}, {} says {:?} at epoch {}",
                node.short(), h.map(|n| n.short()), e.0,
                answers[0].0.short(), holder.map(|n| n.short()), epoch.0
            );
        }
    }

    /// A grant only ever goes to a node that answered this round.
    ///
    /// The rule behind `offload explain` re-asking the fleet instead of replaying stored bids: a
    /// bid describes one second. An arbiter that picked a winner from its *view* — which is a
    /// gossip tick old and full of nodes that were alive a moment ago — would hand the run to a
    /// machine that is not there, and the run would sit `Assigned` until the lease ran out. The
    /// answers are the only current thing in the round, and this says the winner came from them.
    #[test]
    fn a_run_is_only_granted_to_a_node_that_answered(events in prop::collection::vec(ev(), 1..24)) {
        let storm = sim(run_storm(4, &events))?;
        prop_assert!(
            storm.unreachable_grants.is_empty(),
            "granted to a node the arbiter could not reach: {:?}",
            storm.unreachable_grants
                .iter()
                .map(|(a, h)| format!("{} -> {}", a.short(), h.short()))
                .collect::<Vec<_>>()
        );
    }

    /// Once the network is whole, the fleet agrees about who is alive.
    ///
    /// The other half of "settled", and the half `churn.rs` had to learn: its first version
    /// watched only the run record and reported a fleet still arguing about liveness as
    /// converged. Here it is the real SWIM detector rather than a model of one — suspicion has to
    /// travel, be refuted by the subject, and expire — so what this says is that a partition
    /// heals in the *membership* as well as in the records, which is what makes every other
    /// decision here recover.
    ///
    /// `Alive` specifically, not merely agreement: a fleet that unanimously believes a node
    /// present and reachable is dead has agreed on something false, and would never offer it
    /// work again.
    #[test]
    fn a_healed_fleet_agrees_everybody_is_alive(events in prop::collection::vec(ev(), 1..24)) {
        let storm = sim(run_and_settle(4, &events))?;
        for m in &storm.members {
            let view = m.cluster.view();
            for peer in view.nodes.values() {
                prop_assert_eq!(
                    peer.status,
                    offload_core::NodeStatus::Alive,
                    "{} still says {} is {:?} after the partition healed",
                    m.cluster.node().short(),
                    peer.id.short(),
                    peer.status
                );
            }
        }
    }

    /// A checkpoint any reachable node holds can be fetched.
    ///
    /// ADR-0016's whole claim, and the one place a filter can quietly break it. `fetch_blob`
    /// asked only peers it marked `Alive` — while its own doc comment argues that availability
    /// must not be gossiped *because* an advertisement is stale by the time it is used, and asking
    /// costs a millisecond. A node's liveness is exactly as stale, and the moment a checkpoint is
    /// needed is the moment somebody has gone quiet: filtering skipped precisely the peers most
    /// likely to hold the bytes. `Suspect` is not unreachable — it exists to be argued with — so
    /// the only copy in the fleet could sit on a node that answered every dial and never be asked.
    ///
    /// Needs a **deeper search than the default** to find, and that is worth stating rather than
    /// discovering: the window is cut, advance, tick until suspicion, heal, ask — five specific
    /// steps in order — so 24 cases pass with the fix reverted and 600 shrink it to
    /// `[Isolate(1), Store(1), Tick(3), Heal(..)]`. The shrunk seed is checked in beside this
    /// file, so it is re-run first every time; `mesh.rs` has the same case arranged by hand,
    /// which is what makes the failure legible.
    #[test]
    fn a_checkpoint_any_reachable_node_holds_can_be_fetched(
        events in prop::collection::vec(ev(), 1..24)
    ) {
        let outcome = sim(async {
            let mut storm = run_storm(4, &events).await?;
            let holders = storm.holders_of_blob().await;
            let mut checked = Vec::new();
            for i in 0..4 {
                let reachable_copy = holders.iter().any(|h| storm.reaches(i, *h));
                if !reachable_copy {
                    continue;
                }
                checked.push((i, storm.fetch(i).await));
            }
            Ok::<_, TestCaseError>(checked)
        })?;
        for (node, got) in outcome {
            prop_assert!(
                got,
                "node {node} could reach a copy of the checkpoint and did not get it"
            );
        }
    }


    /// An arbiter reports each grant it makes exactly once, and reports only grants it made.
    ///
    /// The audit log's central claim rests on this: *two* `Granted` rows at one epoch is one
    /// arbiter having spent a fencing token twice, which is the failure the whole epoch scheme
    /// exists to prevent. So a spurious row is not noise — it is a false report of the worst thing
    /// that can happen here, and somebody reading it would go looking for a double execution that
    /// never occurred.
    ///
    /// It was spurious. The row was written from `Host::record`, whose doc comment describes only
    /// the arbiter's post-grant persistence — and `Cluster::learn` calls that same hook for every
    /// record a gossip merge moved, so an ordinary two-node run produced three of them. The other
    /// half of one hook carrying two facts: a grant to *this* node returns before `record` is
    /// reached, so on a fleet of one no grant was ever recorded at all. Measured on two daemons
    /// both ways round.
    ///
    /// Reverted, it needs a script of four steps in order — a grant, a record change, and two
    /// ticks to deliver it — so 24 cases pass and 300 shrink it to
    /// `[Place(0), Turn(3), Tick(3), Tick(0)]`, which is the same shape the two daemons showed.
    /// The seed is checked in beside this file.
    ///
    /// A report is a **subset** of what was accepted rather than equal to it, and that is
    /// ADR-0006's silence-is-a-decline rather than a gap: a node that took a grant and whose
    /// answer was lost is running under an epoch its arbiter believes it declined, so the arbiter
    /// has nothing to report and the taker has a grant. That asymmetry is the thing `Swallow`
    /// exists to generate.
    #[test]
    fn an_arbiter_reports_each_grant_once(events in prop::collection::vec(ev(), 1..24)) {
        let storm = sim(run_storm(4, &events))?;
        let accepted: BTreeSet<(NodeId, RunId, Epoch)> = storm.all_grants().into_iter().collect();

        for m in &storm.members {
            let me = m.cluster.node();
            let mut seen: BTreeSet<(RunId, Epoch)> = BTreeSet::new();
            for (run, epoch, holder) in m.host.reported() {
                prop_assert!(
                    seen.insert((run, epoch)),
                    "{} reported granting run {} at epoch {} more than once",
                    me.short(), run.short(), epoch.0
                );
                prop_assert!(
                    accepted.contains(&(holder, run, epoch)),
                    "{} reported granting run {} at epoch {} to {}, which never took it",
                    me.short(), run.short(), epoch.0, holder.short()
                );
            }
        }
    }

    /// And exactly one machine is left believing the work is its own.
    ///
    /// The property the whole design exists for, and the one a merge-level test can only
    /// approximate: two agents on one repository, both committing, is worse than no agent. Here
    /// it is asked of the nodes that were *granted* the run — a node that accepted at an epoch
    /// the fleet has since moved past has been fenced out, and must not still be named holder.
    #[test]
    fn at_most_one_node_is_left_holding_it(events in prop::collection::vec(ev(), 1..24)) {
        let storm = sim(run_and_settle(4, &events))?;
        let believers: Vec<NodeId> = storm
            .members
            .iter()
            .filter(|m| {
                m.cluster.view().runs.get(&RUN).and_then(offload_core::Run::holder)
                    == Some(m.cluster.node())
            })
            .map(|m| m.cluster.node())
            .collect();
        prop_assert!(
            believers.len() <= 1,
            "{} nodes each believe they hold the run: {:?}",
            believers.len(),
            believers.iter().map(offload_core::NodeId::short).collect::<Vec<_>>()
        );
    }
    /// The run's numbers describe the leg the fleet settled on.
    ///
    /// `RunProgress` is arbitrated **forward only** and its own doc comment says why that is
    /// enough: the numbers "have a single author by construction — only the node running the
    /// turns produces them, everybody else relays them verbatim, and a relay that has fallen
    /// behind is behind rather than wrong". A partition is precisely the case where the
    /// construction does not hold. Two legs run, each one legitimately the holder in its own
    /// view, and both are authors.
    ///
    /// So this asks the question the record merge already answers, of the numbers beside it: the
    /// fleet decided which leg survived, and the position it reports has to be that leg's. The
    /// position specifically — the turn count and the worktree summary — because those name
    /// *where* the surviving branch is, and a high-water mark from a branch that lost is not a
    /// smaller version of that answer, it is a different run. Spend is deliberately not asked
    /// about: money left the account on both legs and no harness here produces any.
    ///
    /// Scoped to a holder that has actually done a turn. A run granted and not yet started has
    /// no position of its own, and the numbers it inherits are the last ones somebody wrote.
    ///
    /// **Needs a deep search**, and that is worth stating rather than discovering: the shrunk
    /// script is five steps in order — a decline, a swallowed grant, the round that produces two
    /// legs, and then a turn on each — so with the merge rule reverted 24 cases pass and 1200
    /// fail after 1124 successes, in twelve minutes. Both counterexamples are checked in beside
    /// this file and re-run first. The hand-arranged case below is the same failure in six lines,
    /// and is what actually guards the rule in a default `cargo test`.
    #[test]
    fn the_position_reported_is_the_surviving_leg_s(events in prop::collection::vec(ev(), 1..24)) {
        let storm = sim(run_and_settle(4, &events))?;
        if let Some(holder) = storm.answers()[0].1 {
            if let Some(leg) = storm.published_by(holder) {
                let mine = (leg.turns, leg.workspace.clone());
                for (node, said) in storm.positions() {
                    if let Some(said) = said {
                        prop_assert_eq!(
                            &said, &mine,
                            "{} says the run is at turn {} in `{}`, while {} — which the fleet \
                             agrees holds it — last said turn {} in `{}`",
                            node.short(), said.0, said.1,
                            holder.short(), mine.0, mine.1
                        );
                    }
                }
            }
        }
    }
}

/// The same fork, arranged by hand, with the leg that lost getting further.
///
/// The property above is the net; this is the case worth reading, and it is the damaging half.
/// `Swallow(3)` is ADR-0006's silence-is-a-decline: node 3 takes the grant, runs under the epoch
/// it was handed, and its answer never comes back, so the arbiter spends that token and grants the
/// next bidder at the next one. Two legs, no partition, and the fleet has already decided which
/// one survives — the higher epoch, everywhere.
///
/// The lost leg then does three turns to the survivor's one, because it started first. Under the
/// forward-only rule that used to settle these numbers, its position is simply the larger one and
/// wins the whole fleet: `offload ps` says turn 4 for a run that is on turn 1, beside a worktree
/// summary describing a checkout on a machine that is not running it — and the survivor can never
/// correct either, because for the next three turns everything it says is smaller and refused.
///
/// A plain test rather than a generated one for `mesh.rs`'s reason: the property has to *find* the
/// script, and needing "swallow, place, turn on the loser three times" in that order makes it a
/// deep search for something that reads in six lines.
#[test]
fn a_lost_leg_that_got_further_does_not_become_the_run_s_position() {
    sim(async {
        let mut storm = Storm::new(4);
        storm.at(3).host.swallow_next();
        storm.place(0).await;
        storm.advance(1);
        storm.turn(2).await;
        for _ in 0..3 {
            storm.advance(1);
            storm.turn(3).await;
        }
        storm.heal_all();
        storm.advance(60);
        assert!(
            storm.settle().await,
            "the fleet never stopped changing its mind"
        );

        let holder = storm.answers()[0].1.expect("somebody holds it");
        let leg = storm.published_by(holder).expect("and has done a turn");
        assert_eq!(leg.turns, 1, "the surviving leg has done one turn");
        for (node, said) in storm.positions() {
            assert_eq!(
                said,
                Some((leg.turns, leg.workspace.clone())),
                "{} reports a position the run is not at",
                node.short()
            );
        }
    });
}
