//! Joining the mesh: the daemon's half of phase 3.
//!
//! Assembles what the other crates decided — a QUIC transport keyed by this node's own
//! keypair (ADR-0015), a membership view read from `fleet.json` (ADR-0012), and the SWIM loop
//! that keeps a picture of who is out there (ADR-0005) — and starts them.
//!
//! **A node with no fleet does not join anything, and that is not a degraded mode.** Phases 1
//! and 2 are a single machine doing useful work, and they stay that way: no fleet means no
//! transport, no listener, and no behaviour change at all.

use crate::config::Config;
use crate::fleet::FleetState;
use crate::identity::NodeIdentity;
use offload_cluster::blobs::Blobs;
use offload_cluster::{Clock, Cluster, SystemClock};
use offload_core::{
    BlobHash, Capabilities, FleetId, Grant, Millis, NodeId, NodeView, Revocation, WorkPolicy,
};
use offload_proto::handshake::Credentials;
use offload_transport::discovery::{self, Advertisement};
use offload_transport::quic::{QuicTransport, Resolver, StaticPeers};
use offload_transport::{Membership, Transport};
use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// This node's membership, as the transport sees it.
///
/// Behind a lock because none of it is static: certificates are renewed on contact and
/// revocations arrive by gossip, and a transport holding a snapshot from startup would keep
/// talking to a device revoked an hour ago.
///
/// That sentence was a description of an intention for two phases. Nothing wrote to the lock,
/// so `offload grant host-runs` and `offload revoke` — both of which rewrite `fleet.json` and
/// need no daemon, by ADR-0012's design — reached a running daemon only when it was next
/// restarted. The grant direction was merely confusing; the revoke direction made this ADR's
/// "revocation is immediate and local" false on every machine that had a daemon up, which is
/// all of them. [`NodeMembership::reload`] is what makes the comment true.
pub struct NodeMembership {
    state: Arc<RwLock<FleetState>>,
    /// Where the file this mirrors lives. Held rather than derived, because the daemon's
    /// state directory is relocatable and resolving it twice is how two paths disagree.
    path: PathBuf,
    /// This node's own key, for the one credential it may sign itself: a renewal for a peer,
    /// when this node is an approver (ADR-0012). Never the fleet key, which is not on any
    /// device — an approver issues *under* a delegation and can therefore be revoked, which is
    /// the whole difference between an issuer and a root.
    signing: ed25519_dalek::SigningKey,
}

impl std::fmt::Debug for NodeMembership {
    /// By hand, and only the public parts: a derived one would put this node's secret key in
    /// whatever log line printed it, which is how a secret escapes without anybody deciding to
    /// let it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeMembership")
            .field("fleet", &self.read().fleet.short())
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// What changed when fleet state was re-read, for a caller that has to act on it.
///
/// Empty is the ordinary answer and is not reported: a fleet's membership changes a handful
/// of times in its life, and a tick that logs "nothing changed" once a second is a log nobody
/// reads.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FleetChange {
    /// Members revoked since the last read. The connections to these are no longer allowed to
    /// exist — refusing the *next* handshake is not enough, because a QUIC session that stays
    /// up is a session that never handshakes again.
    pub revoked: Vec<NodeId>,
    // There was a `grants: Option<BTreeSet<Grant>>` here — this node's own grants, if they were
    // not what they were — and it was the `ourselves` edge below a second time. It compared the
    // file with the copy in memory, so it missed both changes that do not arrive as somebody
    // else's write: `NodeMembership::adopt` reloads and drops the change, and **probation
    // lifting is a change in time, not in the file** — the certificate is byte-for-byte the one
    // loaded fifteen minutes earlier, `effective_grants` simply starts answering differently.
    // A node that started with `grants=submit,deliver` and began hosting at probation's end
    // never said so. What a node may do is a standing condition, asked as one:
    // [`NodeMembership::grants_changed`].
    // There was an `ourselves: bool` here — this node is the one that was revoked — and it was
    // an **edge** for a fact that latches, which is why it never fired. `NodeMembership::file`
    // reloads to refresh its own copy and drops the change it gets back, so a revocation
    // arriving from a peer moved the in-memory state and left the next `reload` with nothing to
    // report. Whoever read first ate the only announcement. Being revoked is not a transition
    // to notice once; it is a standing condition, and it is asked as one:
    // [`NodeMembership::revoked_here`].
    /// The fleet itself is a different fleet: `offload rekey` re-founded it here (ADR-0012).
    ///
    /// Every existing connection is to a peer in a fleet this node has just left, and every
    /// one of them will refuse the next handshake — so they are dropped rather than left to
    /// fail one probe at a time.
    pub refounded: bool,
}

impl FleetChange {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.revoked.is_empty() && !self.refounded
    }
}

impl NodeMembership {
    #[must_use]
    pub fn new(
        state: FleetState,
        path: PathBuf,
        signing: ed25519_dalek::SigningKey,
    ) -> Arc<NodeMembership> {
        Arc::new(NodeMembership {
            state: Arc::new(RwLock::new(state)),
            path,
            signing,
        })
    }

    /// Take up a certificate a peer re-issued for this node, and write it down.
    ///
    /// Through the file rather than the copy in memory, for [`NodeMembership::file`]'s reason:
    /// the CLI writes this file with no daemon involved, so a saved snapshot would undo a grant
    /// typed a moment ago.
    pub fn adopt(
        &self,
        credentials: offload_proto::handshake::Credentials,
        now: Millis,
    ) -> Result<crate::fleet::Adopted, crate::fleet::FleetError> {
        let mut on_disk = crate::fleet::load_from(&self.path)?.unwrap_or_else(|| self.snapshot());
        let outcome = on_disk.adopt(credentials, now)?;
        if !outcome.took_it() {
            return Ok(outcome);
        }
        crate::fleet::save_to(&self.path, &on_disk)?;
        self.reload()?;
        Ok(outcome)
    }

    /// Re-read `fleet.json` if it has changed, and say what that changed here.
    ///
    /// The file is the fact and this is a mirror of it, which is the right way round: every
    /// membership command works with no daemon (ADR-0012), so a daemon that expected to be
    /// told would be a daemon that has to be running for the fleet's front door to work.
    ///
    /// A read that fails leaves the in-memory copy alone and is reported. The alternative —
    /// treating an unreadable file as an empty one — would drop this node out of its own
    /// fleet over a transient error, which is the wrong direction for every failure here.
    ///
    /// The whole file is parsed each time rather than compared by timestamp first. A `stat`
    /// would be cheaper and is wrong in the one case that matters: two writes inside a
    /// filesystem's timestamp resolution — `offload join` and then `offload grant`, which is
    /// how a device is set up — leave the second one invisible until something else happens to
    /// touch the file. Two kilobytes of JSON a second is not worth a silent staleness bug.
    pub fn reload(&self) -> Result<FleetChange, crate::fleet::FleetError> {
        let Some(fresh) = crate::fleet::load_from(&self.path)? else {
            // The file is gone. Not a departure: `offload revoke` and friends write through a
            // rename, so an absent file is somebody moving state around, and forgetting our
            // own certificate over it would be a node that cannot rejoin without the
            // passphrase.
            return Ok(FleetChange::default());
        };

        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *state == fresh {
            return Ok(FleetChange::default());
        }
        let before: BTreeSet<NodeId> = state.revocations.iter().map(|r| r.member).collect();
        let change = FleetChange {
            refounded: fresh.fleet != state.fleet,
            revoked: fresh
                .revocations
                .iter()
                .map(|r| r.member)
                .filter(|member| !before.contains(member))
                .collect(),
        };
        *state = fresh;
        Ok(change)
    }

    /// File a revocation heard from a peer, persisting it if it is new.
    ///
    /// Written back through a fresh read of the file rather than from the copy in memory: the
    /// CLI writes this file too, with no daemon and no lock, so a daemon that saved its own
    /// snapshot would undo a `grant` typed a moment earlier. Additive by construction — the
    /// disk's certificate is kept and only revocations are added — which leaves a window of a
    /// few microseconds between the read and the rename, and nothing worse.
    pub fn file(&self, revocation: Revocation) -> Result<bool, crate::fleet::FleetError> {
        {
            let state = self.read();
            revocation.verify(state.fleet)?;
            if state.is_revoked(revocation.member) {
                return Ok(false);
            }
        }
        let mut on_disk = crate::fleet::load_from(&self.path)?.unwrap_or_else(|| self.snapshot());
        on_disk.file(revocation)?;
        crate::fleet::save_to(&self.path, &on_disk)?;
        // Read it back the ordinary way, so the in-memory copy is refreshed and the change is
        // reported through exactly one path.
        self.reload()?;
        Ok(true)
    }

    /// Has this node been revoked from its own fleet, right now (ADR-0044)?
    ///
    /// A standing question rather than the change it used to be reported as. Revocation is
    /// monotonic — nothing here un-revokes a device, and `offload rekey` founds a *different*
    /// fleet rather than undoing one — so the honest shape is a condition anybody may ask at any
    /// time, and asking it costs a lock and a list walk that is empty on almost every fleet.
    ///
    /// The old shape was an edge on [`FleetChange`], and an edge is consumed by whoever observes
    /// it first: [`NodeMembership::file`] reloads to refresh its own copy and discards the
    /// change, which is exactly the path a revocation heard from anybody else arrives on. So the
    /// one announcement was eaten by the mechanism that wrote the fact down.
    #[must_use]
    pub fn revoked_here(&self) -> bool {
        let state = self.read();
        state.is_revoked(state.membership.member)
    }

    /// What this node may do at `now`, if that is not `announced` — which is then updated to it.
    ///
    /// Compared against what was last *said* rather than against the previous read of the file,
    /// for [`FleetChange`]'s reason: the grants a node holds change without the file changing
    /// (probation lifts at a time, not at a write) and change through paths that consume the
    /// file's edge themselves ([`NodeMembership::adopt`]). The caller owns `announced`, so a
    /// second caller asking cannot eat the first one's answer.
    pub fn grants_changed(
        &self,
        announced: &mut BTreeSet<Grant>,
        now: Millis,
    ) -> Option<BTreeSet<Grant>> {
        let current = self.read().grants(now);
        if current == *announced {
            return None;
        }
        announced.clone_from(&current);
        Some(current)
    }

    /// A copy of what this node currently believes about its fleet.
    #[must_use]
    pub fn snapshot(&self) -> FleetState {
        self.read().clone()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, FleetState> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The fleet's side of the same state, for the gossip loop (ADR-0012).
///
/// Two traits over one type rather than one trait doing both jobs: `Membership` is what the
/// *transport* needs at a handshake, and this is what the *cluster* needs to spread a fact. They
/// live in different crates for the reason the crates are split at all, and merging them would
/// put the failure detector's dependency on the transport's.
impl offload_cluster::Members for NodeMembership {
    fn revocations(&self) -> Vec<Revocation> {
        self.read().revocations.clone()
    }

    fn renew(
        &self,
        cert: &offload_core::MembershipCert,
        delegation: Option<&offload_core::Delegation>,
        now: Millis,
    ) -> Result<offload_proto::handshake::Credentials, String> {
        let state = self.read();
        let me = state.membership.member;
        let refused = match state.renew_for(&self.signing, me, cert, delegation, now) {
            Ok(credentials) => return Ok(credentials),
            Err(reason) => reason,
        };
        // A re-approval this approver can only sign in hardware (ADR-0069 §4): filed for a person,
        // served once they confirm. The member asks again on its own schedule, so nothing waits.
        let Some((unsigned, mine)) = state.hardware_reapproval_for(me, cert, delegation, now)
        else {
            return Err(refused);
        };
        let dir = self.path.parent().unwrap_or(std::path::Path::new("."));
        let id = format!("reapprove-{}", cert.member.short());
        let waiting = || {
            format!(
                "the re-approval of {} waits for a person to confirm it on {}",
                cert.name, state.membership.name
            )
        };
        match crate::approval_key::answered(dir, &id) {
            crate::approval_key::Answered::Signed(signed) => {
                signed
                    .verify(state.fleet, Some(&mine), now)
                    .map_err(|e| format!("the signed re-approval does not verify: {e}"))?;
                Ok(offload_proto::handshake::Credentials {
                    membership: *signed,
                    delegation: Some(mine),
                })
            }
            crate::approval_key::Answered::Refused(reason) => Err(format!(
                "the re-approval of {} was refused on {}: {reason}",
                cert.name, state.membership.name
            )),
            crate::approval_key::Answered::Waiting => Err(waiting()),
            crate::approval_key::Answered::Absent => {
                crate::approval_key::file_request(dir, &id, "reapprove", &unsigned)?;
                tracing::info!(member = %cert.member.short(), "a re-approval is waiting for a person to confirm it here");
                Err(waiting())
            }
        }
    }

    fn admits(
        &self,
        cert: &offload_core::MembershipCert,
        delegation: Option<&offload_core::Delegation>,
        now: Millis,
    ) -> bool {
        let state = self.read();
        cert.verify(state.fleet, delegation, now).is_ok()
            && !state.is_revoked(cert.member)
            && !cert
                .renewer()
                .is_some_and(|renewer| state.is_revoked(renewer))
    }

    fn revoked(&self, revocation: Revocation) -> bool {
        match self.file(revocation) {
            Ok(new) => new,
            Err(e) => {
                // A forged one is the interesting case and the reason this is a warning rather
                // than a shrug: a peer that could assert a revocation could evict any device it
                // liked, so an unverifiable one says who tried.
                tracing::warn!(error = %e, "could not file a revocation heard from a peer");
                false
            }
        }
    }
}

impl Membership for NodeMembership {
    fn fleet(&self) -> FleetId {
        self.read().fleet
    }

    fn credentials(&self) -> Credentials {
        let state = self.read();
        Credentials {
            membership: state.membership.clone(),
            delegation: state.chain.clone(),
        }
    }

    fn revocations(&self) -> Vec<Revocation> {
        self.read().revocations.clone()
    }

    fn now(&self) -> Millis {
        SystemClock.now()
    }
}

/// The node's blob store, as the mesh sees it.
///
/// Every method hops onto a blocking thread: `offload-store` is synchronous by decision
/// (ADR-0009 — wrapping SQLite in `async` would hide the cost rather than remove it), and a
/// megabyte written on the reactor is a stall in the failure detector.
#[derive(Debug)]
pub struct StoreBlobs {
    store: offload_store::Store,
    /// What the owner permits. A device perfectly able to hold a checkpoint may be quite right
    /// to refuse one — replication is bytes, and bytes cost money on a phone.
    policy: WorkPolicy,
    metered: offload_core::Metered,
    power: offload_core::PowerSource,
}

/// Will this device hold somebody else's bytes right now?
///
/// Deliberately *not* the full admission check: holding a replica is not hosting a run, and a
/// busy node is still a fine place to keep a copy. What is checked is what replication
/// actually costs — somebody else's bytes over somebody's metered link, and disk on a device
/// that is nearly flat.
///
/// A pure function rather than a method because it is the whole of the decision, and because
/// the one bug it has had was invisible from outside: a plugged-in laptop whose battery
/// reported 0% refused every replica, so every checkpoint stayed `here only` and ADR-0016's
/// durability was quietly not happening.
#[must_use]
fn accepts_replica(
    policy: &WorkPolicy,
    power: &offload_core::PowerSource,
    metered: offload_core::Metered,
) -> bool {
    // `known_metered`, not "not known unmetered". Refusing a replica claims this link costs
    // money, and the claim carries the burden (ADR-0045 §4) — refusing on `Unknown` would stop
    // every machine with no NetworkManager from holding anybody's checkpoint, which is ADR-0016's
    // durability quietly not happening. That is the failure this function's own doc comment
    // already records once, from the battery side.
    if metered.known_metered() && !policy.allow_metered {
        return false;
    }
    // Ignored on mains, exactly as `WorkPolicy::min_battery_percent` says and
    // `WorkPolicy::admits` does: the floor is about running out of charge, and the charge is
    // going up.
    if power.on_mains() {
        return true;
    }
    !matches!(
        (power.battery_percent(), policy.min_battery_percent),
        (Some(have), Some(floor)) if have < floor
    )
}

impl StoreBlobs {
    #[must_use]
    pub fn new(
        store: offload_store::Store,
        policy: WorkPolicy,
        capabilities: &Capabilities,
    ) -> Arc<StoreBlobs> {
        Arc::new(StoreBlobs {
            store,
            policy,
            metered: capabilities.metered_network,
            power: capabilities.power,
        })
    }
}

#[async_trait::async_trait]
impl Blobs for StoreBlobs {
    async fn has(&self, hash: BlobHash) -> bool {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.has_blob(hash))
            .await
            .unwrap_or(false)
    }

    async fn get(&self, hash: BlobHash) -> Option<Vec<u8>> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.get_blob(hash).ok())
            .await
            .ok()
            .flatten()
    }

    async fn store(&self, bytes: Vec<u8>) -> Result<BlobHash, String> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.put_blob(&bytes).map_err(|e| e.to_string()))
            .await
            .map_err(|e| e.to_string())?
    }

    fn accepts_push(&self, _hash: BlobHash, _size: u64) -> bool {
        accepts_replica(&self.policy, &self.power, self.metered)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MeshError {
    #[error("cluster listen address {address} is not usable: {reason}")]
    BadAddress { address: String, reason: String },
    #[error("could not start the transport: {0}")]
    Transport(#[from] offload_transport::TransportError),
}

/// This node's answer to "would you take this run", and what happens when it does.
///
/// Everything it decides with is local by construction (ADR-0006): the workspace being warm,
/// whether the repo can be obtained here at all, what the agent's account is already doing.
/// A central scheduler would have to be told all of it, badly and late.
pub struct NodeHost {
    supervisor: crate::Supervisor,
    /// Where `fleet.json` lives, re-read per bid: a grant made a minute ago should count, and
    /// a probation that has lapsed should too.
    state_dir: std::path::PathBuf,
    /// Weak, because the cluster holds this host: an `Arc` both ways is a fleet that never
    /// frees its own mesh.
    cluster: std::sync::Weak<Cluster>,
    weights: offload_core::BidWeights,
}

impl std::fmt::Debug for NodeHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeHost").finish_non_exhaustive()
    }
}

impl NodeHost {
    #[must_use]
    pub fn new(
        supervisor: crate::Supervisor,
        cluster: &Arc<Cluster>,
        state_dir: std::path::PathBuf,
    ) -> Arc<NodeHost> {
        Arc::new(NodeHost {
            supervisor,
            state_dir,
            cluster: Arc::downgrade(cluster),
            weights: offload_core::BidWeights::default(),
        })
    }

    /// Has the fleet granted this node the right to run agents at all?
    ///
    /// Checked in the bid rather than at submission: a device may perfectly well ask the
    /// fleet for work it is not allowed to do itself — ADR-0012 grants `{Submit, Deliver}` at
    /// the door precisely so a phone can — and this is the node saying "not me" for the one
    /// reason that is about neither capacity nor capability.
    fn host_runs_refusal(&self) -> Option<String> {
        let state = crate::fleet::load(&self.state_dir).ok().flatten()?;
        let now = SystemClock.now();
        if state.grants(now).contains(&offload_core::Grant::HostRuns) {
            return None;
        }
        Some(match state.membership.probation_until(now) {
            Some(until) => format!(
                "host-runs granted but on probation for {}",
                offload_core::fleet::dormant_for(until.saturating_sub(now))
            ),
            None => "not granted host-runs by the fleet".to_string(),
        })
    }

    /// What only this machine knows, gathered off the reactor because every field is a
    /// filesystem question.
    async fn local_facts(
        &self,
        run: &offload_core::Run,
        caps: &offload_core::Capabilities,
    ) -> offload_core::LocalFacts {
        let workspaces = self.supervisor.workspaces().clone();
        let source =
            offload_core::RepoSource::parse(run.spec.workspace().map_or("", |w| w.repo.as_str()));
        let blobs = run
            .checkpoint
            .as_ref()
            .map(offload_core::Checkpoint::blobs)
            .unwrap_or_default();
        let supervisor = self.supervisor.clone();
        let (os, cores) = (caps.os.clone(), caps.cpu_cores);
        let state_dir = self.state_dir.clone();

        tokio::task::spawn_blocking(move || offload_core::LocalFacts {
            thermal: crate::deliver::host_thermal(&state_dir),
            workspace_warm: workspaces.is_warm(&source),
            repo_reach: workspaces.reach(&source),
            checkpoint_local: !blobs.is_empty() && blobs.iter().all(|b| supervisor.holds_blob(*b)),
            // Measured now, and `None` where the platform will not say — a plausible-looking
            // zero would win bids this machine should lose (ADR-0013).
            cpu_load_percent: offload_probe::cpu_load_percent(&os, cores),
            // What the machine's other fleets have promised. Read here so the *bid* already
            // knows — otherwise this node offers to start now and then holds the run the moment
            // it arrives, which is a promise broken between two lines of output.
            device_committed: supervisor.device_elsewhere(),
            // And what the *account* has left, which is the one ceiling in this list that no
            // machine can fix (ADR-0029). Read here so the bid already knows, for
            // `device_committed`'s reason one line up: a node that offers to start now and then
            // holds the run until 09:00 has broken a promise between two lines of output.
            account_limited_until: supervisor.account_limited_until(),
        })
        .await
        .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl offload_cluster::Host for NodeHost {
    async fn evaluate(&self, run: &offload_core::Run) -> Result<offload_core::Offer, String> {
        // First of all, and before anything is read: this node is leaving, and a bid is a
        // promise to still be here. Not a capability question and not a policy one, which is why
        // it is neither of the two checks below it.
        if self.supervisor.is_draining() {
            return Err("draining".into());
        }
        if let Some(reason) = self.host_runs_refusal() {
            return Err(reason);
        }
        let cluster = self.cluster.upgrade().ok_or("this node is shutting down")?;
        let view = cluster.view();
        let me = view
            .node(&cluster.node())
            .cloned()
            .ok_or("this node is not in its own view")?;
        let facts = self.local_facts(run, &me.capabilities).await;

        offload_core::bid::evaluate(run, &me, &facts, &view, &self.weights, SystemClock.now())
            .map(|bid| bid.offer())
            // `NoBid` is already a sentence — "battery 22%, policy floor is 40%" — and it is
            // the sentence `offload run` prints when nobody will take the work (ADR-0014).
            .map_err(|no| no.to_string())
    }

    /// Take the run. Start it now if there is room, and hold it if there is not.
    ///
    /// The second half is ADR-0006's accepting-without-starting, and the whole of it on this
    /// side: `Assigned` already means "granted, not started", so a commitment is that state
    /// lasting rather than any new machinery. What makes it a commitment rather than a
    /// promise is that it carries the epoch and the lease — the fleet can see who holds the
    /// run, and if this node dies the ordinary orphan path takes it back.
    async fn accept(&self, run: offload_core::Run) -> Result<(), String> {
        // Before anything, like the bid: a grant can arrive after the bid that earned it — a
        // round this node answered a moment before the drain began, or a peer acting on what it
        // last heard. Refusing here is what makes the bid's promise true rather than merely
        // usually true, and `place` treats a declined grant as a node to re-offer past
        // (ADR-0006 step 6).
        if self.supervisor.is_draining() {
            return Err("draining".into());
        }
        let cluster = self.cluster.upgrade().ok_or("this node is shutting down")?;
        // The cap this node *gossiped*, which is the one it bid against — read from the view
        // for exactly the reason `evaluate` does, so that what we offered and what we admit
        // cannot disagree.
        let room = {
            let view = cluster.view();
            let capacity = view
                .node(&cluster.node())
                .map(|me| offload_core::Capacity::of(&me.policy))
                .ok_or("this node is not in its own view")?;
            // The account's ceiling comes from the same view for the same reason: it is a fold
            // over what the nodes on that account gossiped, so accepting and bidding are
            // answering from one set of numbers.
            let account = run
                .spec
                .agent_kind()
                .and_then(|kind| view.account_use(&cluster.node(), kind));
            // The rate limit is *not* read from the view, which is the split `AccountUse`
            // already draws for the same reason: a moving value is not gossiped (ADR-0029), and
            // admitting on a peer's stale copy of it would be the confident wrong answer that
            // rule exists to refuse. `Supervisor::room` overlays this node's own.
            offload_core::Room::new(capacity, account, None)
        };

        let taken = run.clone();
        self.supervisor
            .take_run(run, room)
            .await
            .map_err(|e| e.to_string())?;

        // Count it against ourselves *now*, not at the next gossip tick. `evaluate` answers
        // the next bid from the view, so a node whose view has not caught up with what it just
        // accepted bids as though it were idle — which is how six submissions in two seconds
        // all landed on one machine while the other one sat empty. The tick that republishes
        // the store is what corrects this if anything here is wrong; it is not what makes it
        // right.
        cluster.publish_run(taken);
        Ok(())
    }

    /// The audit row for a grant this node made. See [`Host::granted`] for why it is not in
    /// `record`: the epoch is the point (`offload_core::audit`), since two of these at one number
    /// is one arbiter having spent a fencing token twice — so a row written on the gossip path
    /// instead said exactly that about an ordinary run, once per merge.
    async fn granted(&self, run: &offload_core::Run) {
        if let Some(to) = run.holder() {
            self.supervisor.note_granted(run.id, to, run.epoch);
        }
    }

    async fn record_schedule(&self, schedule: &offload_core::Schedule) {
        // Straight to the store, which applies `Schedule::merge` again on the way in. Belt and
        // braces on purpose: this hook and `offload schedule` are two writers, and the rule that
        // settles two copies belongs in one place rather than at each of them.
        if let Err(e) = self.supervisor.store().merge_schedule(schedule) {
            tracing::warn!(
                schedule = %schedule.id,
                error = %e,
                "could not write down a schedule the fleet gossiped"
            );
        }
    }

    async fn record(&self, run: &offload_core::Run) {
        if let Err(e) = self.supervisor.record_run(run) {
            tracing::warn!(run_id = %run.id, error = %e, "could not record a run placed elsewhere");
        }
    }

    async fn stale_copy(&self, run: offload_core::RunId) {
        self.supervisor.retell(run);
    }

    async fn record_spec_edit(&self, run: &offload_core::Run) {
        if let Err(e) = self.supervisor.apply_spec_edit(run) {
            tracing::warn!(run_id = %run.id, error = %e, "could not take an edit to a run here");
        }
    }

    async fn record_progress(
        &self,
        run: offload_core::RunId,
        progress: &offload_core::RunProgress,
    ) {
        self.supervisor.record_progress(run, progress);
    }

    /// Apply a deadline change forwarded here because this node owns the field.
    ///
    /// Published straight away for the same reason an acceptance is: the next decision about
    /// this run — a bid round, a hold-down — reads the view, and one that has not caught up
    /// with the change would take the decision the operator just asked us not to.
    async fn events_since(
        &self,
        peer: NodeId,
        run: offload_core::RunId,
        after: u64,
        limit: u32,
    ) -> Result<(Vec<offload_proto::cluster::SeqEvent>, bool), String> {
        // Being asked is the observation, so it is noted before the answer is even read: a peer
        // that asks and then loses the reply is still a peer with somebody watching.
        self.supervisor.note_remote_watcher(run, peer);
        let supervisor = self.supervisor.clone();
        // The store is synchronous by decision (ADR-0009), and a log can be thousands of rows.
        tokio::task::spawn_blocking(move || supervisor.events_after(run, after, limit))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }

    async fn files(
        &self,
        _peer: NodeId,
        run: offload_core::RunId,
        path: &str,
    ) -> Result<offload_proto::cluster::FilesView, String> {
        self.supervisor.peek(run, path).await
    }

    async fn edit_spec(
        &self,
        run: offload_core::RunId,
        edit: offload_core::SpecEdit,
    ) -> Result<(u32, String), String> {
        let (run, note) = self
            .supervisor
            .edit_spec(run, edit)
            .map_err(|e| e.to_string())?;
        let rev = run.spec_rev;
        if let Some(cluster) = self.cluster.upgrade() {
            cluster.publish_run(run);
        }
        Ok((rev, note))
    }

    /// A peer forwarding a cancel for a run that is here.
    ///
    /// Nothing is checked here that is not checked for a local cancel, and the check that matters
    /// is in `cancel_run`: a record naming another node is refused, so a peer that forwarded to
    /// the wrong machine gets a sentence rather than a terminal state written on a run somebody
    /// else is running. Who may ask at all was settled by the handshake.
    ///
    /// Published straight away, like an accepted grant: the operator is waiting for this answer,
    /// and the node that typed the command learns what happened from gossip.
    async fn cancel(&self, run: offload_core::RunId, by: &str) -> Result<String, String> {
        let note = self
            .supervisor
            .cancel_run(run, by)
            .await
            .map_err(|e| e.to_string())?;
        if let (Some(cluster), Some(cancelled)) = (self.cluster.upgrade(), self.supervisor.run(run))
        {
            cluster.publish_run(cancelled);
        }
        Ok(note)
    }

    /// A peer building a continuation of a run whose last leg ran here (ADR-0064 §2).
    ///
    /// The same `continuation_base` a continuation typed here calls, so the checks are the same
    /// ones: the parent finished, it is not a task, its last leg ended **in this node's own
    /// log**, and its checkout is still here. A peer that routed on a stale `RunProgress::by`
    /// gets the sentence that says so rather than a capture of an older leg's checkout.
    async fn continuation_base(
        &self,
        run: offload_core::RunId,
        session: bool,
    ) -> Result<offload_core::ContinuationBase, String> {
        let mode = if session {
            offload_core::ContinueMode::Session
        } else {
            offload_core::ContinueMode::Handoff
        };
        // Worded with this node's fleet name, and handed back as the bare reason: the asker
        // prints it under its own `run … cannot be continued`, at a keyboard where "this node"
        // is the other machine.
        let me = self.supervisor.fleet_name();
        match self
            .supervisor
            .continuation_base(run, mode, Some(&me))
            .await
        {
            Ok(base) => Ok(base),
            Err(crate::supervisor::SubmitError::Refused { reason, .. }) => Err(reason),
            Err(e) => Err(format!("{me} could not build it: {e}")),
        }
    }

    /// A peer forwarding a checkpoint request for a run whose agent is here.
    ///
    /// Nothing is checked here that is not checked locally, and the check is the same one:
    /// `request_checkpoint` refuses a run this node is not running, which is what a peer that
    /// forwarded to a stale holder gets back. Nothing is published — the request changes the
    /// run's state to `Checkpointing`, and the gossip tick carries that like any other
    /// transition; the capture itself is minutes away.
    ///
    /// [`GivenUp::Parked`], because the only thing that forwards one of these is `offload
    /// checkpoint` typed at a device that is not the holder — a person, at a keyboard, who means
    /// to come back for the run (ADR-0042). A departing node drains its *own* runs and has no
    /// reason to ask this of anybody else's.
    async fn request_checkpoint(&self, run: offload_core::RunId) -> Result<(), String> {
        self.supervisor
            .request_checkpoint(run, offload_core::GivenUp::Parked)
            .map_err(|e| e.to_string())
    }

    /// A peer forwarding an answer to a question one of this node's agents is blocked on
    /// (ADR-0017).
    ///
    /// Nothing is checked here that is not checked for a local answer: the question's identity is
    /// the agent's own `tool_use_id`, so an answer that matches nothing is refused by the
    /// registry, and who may answer at all was settled by the handshake. What the peer adds is
    /// `by`, its own name, which goes into the run's log — so the record says the desktop's run
    /// was approved from the phone.
    async fn answer(
        &self,
        run: offload_core::RunId,
        tool_use_id: Option<String>,
        allow: bool,
        by: &str,
    ) -> Result<(String, String, bool), String> {
        let answered = self
            .supervisor
            .answer(
                run,
                tool_use_id.as_deref(),
                allow,
                &format!("an operator on {by}"),
            )
            .map_err(|e| e.to_string())?;
        Ok((answered.tool, answered.detail, answered.allowed))
    }

    async fn pending_asks(&self) -> Vec<offload_core::PendingAsk> {
        self.supervisor.asks()
    }
}

/// The mesh, as the supervisor sees it: somewhere to put a copy of a checkpoint.
///
/// The choice of peer is `offload_core::bid::replica_for` — the same view every node has, so
/// every node would pick the same replica, and preferring a plausible successor means the
/// common migration needs no fetch at all (ADR-0016).
#[async_trait::async_trait]
impl crate::supervisor::Peers for Mesh {
    async fn replicate(
        &self,
        run: offload_core::RunId,
        blobs: Vec<BlobHash>,
    ) -> crate::supervisor::Replicated {
        use crate::supervisor::Replicated;
        let view = self.cluster.view();
        let held = view.runs.get(&run).cloned();
        let peer = match &held {
            Some(run) => offload_core::bid::replica_for(run, &view, self.cluster.node()),
            // A run this node holds but has not gossiped yet: fall back to the most durable
            // available peer rather than skipping replication entirely.
            None => view
                .nodes
                .values()
                .filter(|n| n.id != self.cluster.node() && n.status.is_available())
                .max_by_key(|n| (n.capabilities.stability, std::cmp::Reverse(n.id)))
                .map(|n| n.id),
        };
        let Some(peer) = peer else {
            return Replicated::NoPeer;
        };

        // All or nothing: a peer holding two of three blobs cannot materialise the run, so
        // recording it as a replica would make `is_durable` a lie.
        for blob in blobs {
            match self.cluster.push_blob(peer, blob).await {
                Ok(offload_cluster::Pushed::Stored) => {}
                Ok(outcome) => {
                    let permanent = outcome.is_permanent();
                    // `already held` is a decline and a perfectly good outcome, so it is still
                    // re-checked — but only when it could be that. An over-sized blob was never
                    // offered to anybody, so asking the peer whether it has it is a round trip
                    // whose answer is already known.
                    if !permanent && self.cluster.peer_has_blob(peer, blob).await {
                        continue;
                    }
                    return Replicated::Refused {
                        peer,
                        why: outcome.why(),
                        permanent,
                    };
                }
                Err(e) => {
                    return Replicated::Refused {
                        peer,
                        why: e.to_string(),
                        permanent: false,
                    };
                }
            }
        }
        Replicated::To(peer)
    }

    async fn fetch(&self, blob: BlobHash) -> Result<(), String> {
        // No hint about who has it: the run's last holder is the obvious guess and is also
        // the node most likely to have just gone away, which is why this is asked rather
        // than looked up (ADR-0016).
        self.cluster
            .fetch_blob(blob, None)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn resources_elsewhere(&self) -> crate::resource::Reachable {
        crate::resource::Reachable::in_view(&self.cluster.view(), self.cluster.node())
    }
}

/// Everything the daemon needs to keep hold of once the mesh is up.
pub struct Mesh {
    pub cluster: Arc<Cluster>,
    transport: Arc<QuicTransport>,
    /// This node's live view of who belongs to the fleet, kept so the daemon can re-read it
    /// when the file changes underneath — see [`NodeMembership::reload`].
    membership: Arc<NodeMembership>,
    /// Members already checked against the fleet log since this daemon started. Not a cache of
    /// the log — a memo that the question has been asked, so the answer costs one query per
    /// device per start rather than one per second.
    noted: std::sync::Mutex<std::collections::HashSet<NodeId>>,
    /// When this node last asked the fleet to re-issue its certificate. See
    /// [`Mesh::renew_if_due`]: the window is a week and the loop is a second, so without this
    /// a fleet that cannot renew would spend the whole week asking.
    renewed: std::sync::Mutex<Option<Instant>>,
    /// When each run was last offered to the fleet after its holder went away.
    ///
    /// A refused reassignment is a fleet with nowhere to put the work, which does not change
    /// in a second — and re-running a bid round per orphaned run per tick would fill the logs
    /// and the network with the same answer.
    ///
    /// It rate-limits the *other* offer too — a run nobody holds, queued or let go — which is
    /// what keeps one node from running two rounds for one run in one tick (ADR-0042).
    retried: std::sync::Mutex<HashMap<offload_core::RunId, Instant>>,
    /// When each peer was last dialled at the addresses it gossips (ADR-0076), so a peer that
    /// is truly gone costs one attempt a minute rather than one a tick.
    gossip_dialled: std::sync::Mutex<HashMap<NodeId, Instant>>,
}

/// The delivery plane's two halves, as the mesh sees them (ADR-0010).
///
/// One type implementing both directions because they are the same fact from two ends: which
/// routes the fleet advertises, and what this node does when somebody uses one of ours. It holds
/// nothing but what it needs — the sinks come from config, so there is no state to keep in step.
pub struct Delivery {
    sinks: Arc<crate::deliver::Sinks>,
    /// Weak for `NodeHost`'s reason: the cluster holds this, and an `Arc` both ways is a fleet
    /// that never frees its own mesh.
    cluster: std::sync::Weak<Cluster>,
    window: offload_core::Millis,
}

impl std::fmt::Debug for Delivery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Delivery").finish_non_exhaustive()
    }
}

impl Delivery {
    #[must_use]
    pub fn new(
        sinks: Arc<crate::deliver::Sinks>,
        cluster: &Arc<Cluster>,
        window: offload_core::Millis,
    ) -> Self {
        Delivery {
            sinks,
            cluster: Arc::downgrade(cluster),
            window,
        }
    }
}

/// Asked by a peer to carry something. This is the phone's whole job in the fleet.
#[async_trait::async_trait]
impl offload_cluster::Deliverer for Delivery {
    async fn deliver(
        &self,
        from: offload_core::NodeId,
        sink: &str,
        note: &offload_core::Notification,
    ) -> Result<(), String> {
        // Named by the capability this node advertises, which is `sink:<id>`; accept either
        // spelling, because the id that travelled is the one the asker read off our gossip.
        let id = crate::deliver::sink_id_of(sink);
        let Some(route) = self.sinks.get(id) else {
            // Loud on this side too: the asker will report it, and the person who can fix it is
            // sitting at *this* machine.
            tracing::warn!(
                sink = %id,
                from = %from.short(),
                "a peer asked for a delivery route this node does not have"
            );
            return Err(format!("this node has no route called {id}"));
        };
        route.deliver(note).await.inspect(|()| {
            tracing::info!(
                about = %note.subject.topic(),
                sink = %id,
                from = %from.short(),
                notice = %note.title(),
                "carried a notification for a peer"
            );
        })
    }
}

/// The other direction: which routes the fleet offers, and how to use one.
#[async_trait::async_trait]
impl crate::deliver::Fleet for Delivery {
    fn remote_routes(&self) -> Vec<crate::deliver::Route> {
        let Some(cluster) = self.cluster.upgrade() else {
            return Vec::new();
        };
        // Derived in `deliver`, where the reporting half derives it too: what a peer offers and
        // what is wrong with it are one question, and answering it twice is how the pass came to
        // call a route gone that `offload sinks` was listing.
        crate::deliver::routes_in(&cluster.view())
    }

    fn me(&self) -> Option<offload_core::NodeId> {
        self.cluster.upgrade().map(|cluster| cluster.node())
    }

    fn standing(&self, node: offload_core::NodeId) -> crate::deliver::Standing {
        use crate::deliver::Standing;
        let Some(cluster) = self.cluster.upgrade() else {
            return Standing::Unheard;
        };
        if cluster.is_revoked(node) {
            Standing::Revoked
        } else if cluster.view().node(&node).is_some() {
            Standing::Listed
        } else {
            Standing::Unheard
        }
    }

    async fn send(
        &self,
        node: offload_core::NodeId,
        sink: &str,
        note: &offload_core::Notification,
    ) -> Result<(), crate::deliver::SendFailure> {
        use crate::deliver::SendFailure;
        let Some(cluster) = self.cluster.upgrade() else {
            // Not the peer's doing and not a delivery attempt: the news waits for the next start.
            return Err(SendFailure::Unanswered("this node is shutting down".into()));
        };
        cluster
            .deliver_via(node, sink, note, self.window)
            .await
            .map_err(|e| match e {
                offload_cluster::DeliverError::Unanswered(why) => SendFailure::Unanswered(why),
                offload_cluster::DeliverError::Refused(why) => SendFailure::Refused(why),
            })
    }
}

/// How many nodes other than `me` look able to start this run right now.
///
/// An estimate, and the honest one available: it is read from gossip, so it is a tick old, and
/// a node that has just taken something else will decline. Capacity is written down the instant
/// it changes (that is why the six-submissions-in-two-seconds race was fixed at the source), so
/// this is stale rather than fictional.
///
/// It asks for **room**, not merely eligibility, because the decision it feeds is whether
/// giving a run back would achieve anything: handing a late commitment from this node's queue
/// to another node's queue costs a bid round, an epoch and an attempt, and buys nothing.
/// How many *other* nodes look able to start this run right now.
///
/// `barred` is the peers this node **positively knows** the fleet has not permitted to host —
/// see [`Mesh::peers_barred_from_hosting`], which is where that is read and why it is a set
/// rather than a fourth filter here.
///
/// The other three gates are liveness, the run's constraint against the node's *capabilities*,
/// and room by its *policy* — which is every gate `NodeView` carries, and which is exactly the
/// vocabulary's own split: capabilities are ability and a work policy is what the owner permits.
/// The fleet's `host-runs` grant is a third thing and lives on a certificate, so it was the one
/// gate this estimate could not reach and therefore did not apply. A member that joined and was
/// never granted it — the default, deliberately: joining a fleet is not permission to run agents
/// on its repositories — counted as ready for every late commitment in the fleet.
fn ready_elsewhere(
    view: &offload_core::ClusterView,
    me: NodeId,
    run: &offload_core::Run,
    barred: &std::collections::BTreeSet<NodeId>,
    now: offload_core::Millis,
) -> usize {
    // What the bid round will ask, hold included (ADR-0063 §4): a run held for the desktop is
    // not *ready elsewhere* on the laptop, and counting it would hand it back for a round the
    // laptop then wins again.
    let eligible = run.spec.eligibility(now);
    view.alive()
        .filter(|n| n.id != me)
        .filter(|n| !barred.contains(&n.id))
        .filter(|n| eligible.matches_on(n.id, &n.capabilities))
        .filter(|n| {
            offload_core::Capacity::of(&n.policy)
                .room_for(view.occupancy(&n.id), run.spec.demand)
                .is_ok()
        })
        .count()
}

/// How long to wait before offering an orphaned run to the fleet again.
const REASSIGN_RETRY: Duration = Duration::from_secs(30);

impl std::fmt::Debug for Mesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mesh")
            .field("node", &self.cluster.node().short())
            .field("listening", &self.transport.local_addr().ok())
            .finish()
    }
}

/// What a drain did: how many runs went somewhere else, how many ended on their own, and how
/// many are still here.
///
/// Three numbers rather than one because they are three different sentences to an operator
/// standing at a laptop they are about to close, and because none of them has another honest
/// source — see [`Mesh::drain`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Drained {
    pub moved: usize,
    /// Runs this node could not hand over: mid-turn at the deadline, or refused by every peer.
    pub left: usize,
    /// Runs that finished by themselves while this pass waited for their turn to end.
    ///
    /// Its own number because it is the *good* outcome and it was being reported as the worst
    /// one: [`wait_for_checkpoint`] answered `None` for three different reasons and the caller
    /// read all three as "still mid-turn at the drain deadline", so a run that had completed
    /// thirty milliseconds earlier was counted as a run left behind. Measured twice in one walk,
    /// including on the shutdown path. The same shape as `offload logs` falling back for a value
    /// that is `None` for two reasons: a single answer is right for at most one of them.
    pub finished: usize,
    /// Runs still mid-turn at the deadline, handed over at their next turn boundary.
    ///
    /// Its own number for the reason [`Self::finished`] has one, one outcome further on: `left`
    /// meant two things that read the same and end differently — a run nobody would take, which
    /// is final, and a run that had not finished its turn yet, which is not. The second is the
    /// only one the CLI's "`offload ps` and `offload nodes` say which" advice cannot help with,
    /// because what it is waiting for is a turn boundary that has not happened.
    ///
    /// **It counted the refused ones too until ADR-0042, and stopped.** Both were debts this
    /// daemon owed, so one sentence covered them; now only this one is. A released run is the
    /// fleet's — see [`Self::pooled`] — and this one is still running here, on an agent only
    /// this process can carry to its boundary.
    ///
    /// Only ever non-zero for a departure the daemon outlives ([`Lifespan::StaysUp`]): it is a
    /// promise about something this node does *later*, and a process on its way out cannot make
    /// one.
    pub later: usize,
    /// Runs released into the pool that nobody would take yet.
    ///
    /// Neither moved nor left behind nor waiting on anything here. The release records which
    /// node let the run go (ADR-0042), so its arbiter offers it again on the backoff — including
    /// after this daemon has stopped, which is what makes it a different sentence from
    /// [`Self::later`] rather than a share of it.
    ///
    /// Non-zero for **either** [`Lifespan`], and that is the point: a promise this node keeps
    /// needs this node, and a fact on the record does not.
    pub pooled: usize,
    /// Failed runs given back to the fleet on the way out (ADR-0043).
    ///
    /// Not a share of any number above it, because it is not counted from the same list: every
    /// other field here describes a run this node *held*, and a failed run is terminal and holds
    /// nothing. It is the drain's own pass over them ([`Supervisor::hand_back_failed_runs`]),
    /// taken before the handover pass that cannot see them.
    ///
    /// Like [`Self::pooled`] and unlike [`Self::later`], it needs no promise from this daemon:
    /// the run is `Pending` with a node's name on it, and whoever arbitrates offers it.
    pub handed_back: usize,
    /// Runs still going here that have **no turn boundary to wait for** — tasks (ADR-0019 §2).
    ///
    /// Its own number for the reason [`Self::finished`] and [`Self::later`] have theirs, and the
    /// clearest case of it yet: a running task was counted in [`Self::later`], which promises a
    /// handover *at its next turn boundary*, and a nominated program has none. Before that it was
    /// **waited for**, the whole of `drain_deadline_secs`, which is 300 seconds by default.
    ///
    /// Nothing is owed for one. On [`Lifespan::StaysUp`] the node stays and the program ends by
    /// itself; on [`Lifespan::Exits`] main stops it, it fails, and ADR-0043's pass hands it back
    /// to be re-run from its spec — a task's handover needs neither a boundary nor a checkpoint,
    /// which is exactly why this is not a share of any number above it.
    pub no_boundary: usize,
}

/// Does this daemon outlive the departure it is performing?
///
/// The two callers of [`depart`] differ in exactly one way that a run can feel, and it decides
/// what a run left mid-turn at the deadline is promised. An operator's `offload drain` stops the
/// node accepting and leaves it running, so the turn finishes, the boundary arrives, and this
/// node is still there to hand the run over. A signal is the process going away: main stops
/// every agent the drain could not move as soon as it returns, so there is no later boundary and
/// nothing to promise — those runs are the orphan path's (ADR-0007).
///
/// A parameter rather than a read of [`Report::silent`], which is the other thing that differs
/// between the two: "nobody is listening" and "this process is about to stop existing" are two
/// facts, and inferring one from the other is how the departure flag ended up in the wrong place
/// twice (ADR-0035 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifespan {
    /// `offload drain`: stops accepting, stays up, and can still keep a promise.
    StaysUp,
    /// A signal: the drain is the last thing this process does.
    Exits,
}

/// Where a drain says what it is doing, while it is doing it (ADR-0035).
///
/// A drain is the one operation here that can take minutes, and until this existed it was also
/// the one that could not report progress: the request blocked and the CLI printed nothing until
/// the pass returned. What made that worth fixing rather than tolerating is the case where the
/// wait is *avoidable* — a run blocked on an unanswered question — where the person who could
/// end it instantly is the person staring at the blank terminal.
///
/// Silent for the caller that has nobody to tell: a `SIGTERM` shutdown drains too, and there
/// the daemon's log is the only reader there has ever been.
pub struct Report(Option<tokio::sync::mpsc::Sender<crate::api::DrainStep>>);

impl Report {
    /// Send each step to a client that is reading them.
    #[must_use]
    pub fn to(steps: tokio::sync::mpsc::Sender<crate::api::DrainStep>) -> Self {
        Report(Some(steps))
    }

    /// Say nothing to anybody: the shutdown path, and every test that only wants the counts.
    #[must_use]
    pub fn silent() -> Self {
        Report(None)
    }

    /// Say one thing, and never wait to say it.
    ///
    /// `try_send`, so a client that has stopped reading cannot hold up a departure — the pass is
    /// the point and the commentary is not. A dropped step is a line missing from a report; a
    /// blocked pass is a laptop that will not close.
    fn say(&self, step: crate::api::DrainStep) {
        if let Some(steps) = &self.0 {
            let _ = steps.try_send(step);
        }
    }
}

impl std::fmt::Debug for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Report")
            .field("listening", &self.0.is_some())
            .finish()
    }
}

/// This node is leaving: record that first, then hand over whatever there is to hand over.
///
/// **One function because "this node is leaving" is one fact with two ways to be told** — an
/// operator typing `offload drain`, and a signal — and the flag that records it has now been in
/// the wrong place twice. It began at the top of [`Mesh::drain`], which the no-mesh branch skips
/// entirely, so `offload drain` on a fleet of one set nothing at all (ADR-0034). Moving it to
/// `Request::Drain` fixed that caller and silently broke the other one: measured, `SIGTERM` at
/// 15:55:41, `draining runs=1` a millisecond later, and at 15:55:44 — three seconds into the
/// shutdown drain — `spawning claude code` for a run submitted after the daemon had been told to
/// stop, with `offload status` saying `accepting yes` throughout. A node in the act of leaving
/// took new work and then died with it.
///
/// So the owner is neither the pass nor either caller: it is the departure, which is this.
/// `mesh` is an `Option` for exactly the reason the flag was missed the first time — a fleet of
/// one is a departure too, and stop-accepting is the whole of what a drain can do there.
/// How long a departing node may spend pushing the last copy of a checkpoint (ADR-0054).
///
/// A whole-pass budget, not a per-run one. Thirty seconds against the measured cost — 1.3–47.3 ms
/// for blobs up to 11.5 MB over loopback, and about 244 MB/s — is room for far more than any
/// personal fleet holds, while still being a number a person waiting at a laptop lid can live
/// with. It is deliberately not configurable: the knob somebody would reach for is
/// `drain_deadline_secs`, which bounds the wait for *turn boundaries*, and two timeouts that
/// sound alike is how ADR-0035's residual happened.
const LAST_COPY_BUDGET: Duration = Duration::from_secs(30);

pub async fn depart(
    supervisor: &crate::Supervisor,
    mesh: Option<&Mesh>,
    deadline: Duration,
    window: Millis,
    lifespan: Lifespan,
    report: &Report,
) -> Drained {
    supervisor.stop_accepting();
    report.say(crate::api::DrainStep::StoppedAccepting);

    // Before the handover pass, and deliberately not inside it (ADR-0043). A failed run is
    // terminal, so `Mesh::drain` filters it out of `held` and has never seen one — and a node
    // whose only run has failed holds nothing at all, which makes that pass return on its first
    // line. The recovery tick would get there a tick later on the `offload drain` door; on the
    // signal door there is no later, because `main` stops everything the moment this returns.
    //
    // Placed here for the reason `stop_accepting` is: `depart` is the one way in, and a fact
    // hoisted into "the caller" has as many owners as that function has callers. It runs on the
    // no-fleet arm too — nobody will take the run today, but the record outlives this process
    // and a peer that appears tomorrow is offered it.
    // **Before the failed-run pass, because that pass reads `is_durable` and this is what can
    // still change the answer** (ADR-0054). `fleet_could_start` decides between handing a run to
    // the fleet and stranding it with `NoCopyElsewhere`, and until this line the fact it reads was
    // one nothing on the way out had ever tried to improve — a checkpoint taken while no peer was
    // reachable stays `here only` for ever, and a failed run has no next capture to fix it.
    //
    // Every run, not only the failed ones: a `Pending` run released by `offload checkpoint` has
    // no next turn either, and a peer that wins it later fetches from a node that has gone.
    //
    // The budget is spent here rather than added to the drain's, so a departure cannot be made
    // slower than its deadline by a slow peer. What it costs when the fleet is unreachable is one
    // timeout, once.
    let copies = supervisor.push_last_copies(LAST_COPY_BUDGET).await;
    if copies.worth_saying() {
        report.say(crate::api::DrainStep::PushedLastCopies {
            pushed: copies.pushed,
            stranded: copies.refused + copies.unattempted,
        });
    }

    // **Whether there is anybody to hand them to**, and on the no-fleet arm there is not:
    // `LetGo` leaves a run `Pending` for whoever arbitrates to offer, and a node with no peers
    // offers it to nobody — including to itself when it comes back a second later. Measured: a
    // task that exits 2, a `SIGTERM`, a restart, and a `pending` row at epoch 2 that nothing
    // would ever pick up, jamming its rule for good (`rule_run_in_flight` reads non-terminal as
    // in-flight). The comment above about a peer that appears tomorrow is right for a fleet that
    // has ever had two members, which is the question `ClusterView::alone` asks.
    let alone = mesh.is_none_or(|mesh| mesh.cluster.view().alone());
    let given_back = supervisor.hand_back_failed_runs(alone).await;
    let handed_back = given_back.len();
    if handed_back > 0 {
        tracing::info!(runs = handed_back, "gave failed runs back to the fleet");
    }
    // Into the view, so the fact leaves with the goodbye. **Measured, and the reason this line
    // exists**: without it a `SIGTERM`ed node wrote `Pending` to its own store and exited, and
    // every peer went on holding the run as `Failed` until that daemon came back — which is the
    // stranding this whole decision is about, moved from the record to the network. The
    // periodic republish is a tick away and there is no next tick here; `announce_departure`,
    // one step after this returns, carries `Gossip::runs` and is already waited for.
    if let Some(mesh) = mesh {
        for run in given_back {
            mesh.cluster.publish_run(run);
        }
    }

    match mesh {
        Some(mesh) => Drained {
            handed_back,
            ..mesh
                .drain(supervisor, deadline, window, lifespan, report)
                .await
        },
        // No fleet, so nothing was offered anywhere and every run this node holds is staying —
        // which is worth saying rather than reporting as an idle node. `later` and `pooled` are
        // zero for the same reason `moved` is: there is nobody to hand anything to, now or at
        // any boundary, and no arbiter but this node to keep asking.
        None => {
            // Split here too, for the reason `Supervisor::held_no_boundary` exists: a running
            // task has no boundary whether or not there is a fleet, and counting it in `left` on
            // this arm alone would give one fact two spellings. `left`'s own sentence — *"it
            // stays here with its checkpoint"* — is false for a task twice over, since it has
            // none and never will.
            let no_boundary = supervisor.held_no_boundary();
            Drained {
                moved: 0,
                left: supervisor.held_count().saturating_sub(no_boundary) as usize,
                finished: 0,
                later: 0,
                pooled: 0,
                handed_back,
                no_boundary: no_boundary as usize,
            }
        }
    }
}

impl Mesh {
    /// Where peers should be told to find this node.
    pub fn address(&self) -> Result<SocketAddr, offload_transport::TransportError> {
        self.transport.local_addr()
    }

    /// Datagrams this machine's kernel would not send, per destination.
    ///
    /// From the transport, because that is the only layer that can see it: quinn discards a
    /// send error and the symptom reaches every layer above as a *peer* that did not answer.
    /// Empty on a healthy machine, which is why the report prints nothing then.
    #[must_use]
    pub fn send_refusals(&self) -> Vec<offload_transport::sends::SendRefusal> {
        self.transport.send_refusals()
    }

    /// …and the total, which includes destinations past the breakdown's cap.
    #[must_use]
    pub fn sends_refused(&self) -> u64 {
        self.transport.sends_refused()
    }

    /// Peers that would not admit this node to their fleet (ADR-0060).
    ///
    /// The sibling of [`Self::send_refusals`] one layer up, and the sibling of the same
    /// silence: that one is this machine never speaking, this one is this machine speaking and
    /// being turned away. Both reach every report above as a *peer that did not answer*.
    #[must_use]
    pub fn turnaways(&self) -> Vec<offload_cluster::turnaway::Turnaway> {
        self.cluster.turnaways()
    }

    /// …and the total, which includes peers past the breakdown's cap.
    #[must_use]
    pub fn turnaways_total(&self) -> u64 {
        self.cluster.turnaways_total()
    }

    /// This node's live fleet state, for whoever needs to read or add to it.
    #[must_use]
    pub fn membership(&self) -> &Arc<NodeMembership> {
        &self.membership
    }

    /// Ask the fleet to re-issue this node's certificate, if it is running out.
    ///
    /// ADR-0012's backstop, which is load-bearing in two directions at once: it is what makes a
    /// revocation that never arrives eventually take effect, and what makes a device that has
    /// been out of contact for a month stop being a member. Nothing implemented it for two
    /// phases, so a fleet's real behaviour after thirty days was that *every* device fell out
    /// simultaneously and the passphrase was the only way back on each of them.
    ///
    /// Asked, never volunteered: the node whose certificate is lapsing is the one that knows
    /// and the only one that suffers, and an approver offering renewals would renew nothing for
    /// the laptop that has been shut for three weeks, which is precisely the device that needs
    /// it. Peers are tried in turn because most members are not approvers and being told so is
    /// not a failure.
    ///
    /// Attempts are spaced out because the window is a week and this loop runs every second: a
    /// node in a fleet with no approver left would otherwise ask everybody, constantly, right
    /// up to the moment it expired.
    pub async fn renew_if_due(&self, window: Millis) -> bool {
        const RETRY: Duration = RENEW_RETRY;

        let now = SystemClock.now();
        let before = self.membership.snapshot().membership;
        // Asked for a re-approval too (ADR-0069 §3): a member in its approval's last month keeps
        // asking, because the answer depends on a person having run `offload reapprove` on an
        // approver, which may happen at any point in that month.
        let renewal_due = before.is_due_for_renewal(now);
        if !renewal_due && !before.is_due_for_reapproval(now) {
            return false;
        }
        {
            let mut last = self
                .renewed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if last.is_some_and(|at| at.elapsed() < RETRY) {
                return false;
            }
            *last = Some(Instant::now());
        }

        let me = self.cluster.node();
        let peers: Vec<NodeId> = self
            .cluster
            .view()
            .nodes
            .values()
            .filter(|node| node.id != me && node.status != offload_core::NodeStatus::Dead)
            .map(|node| node.id)
            .collect();
        if peers.is_empty() {
            tracing::warn!(
                "this node's membership certificate is running out and there is nobody to ask \
                 — `offload join --passphrase` once it lapses"
            );
            return false;
        }

        // Every answer says why, and the summary below used to discard all of them.
        let mut refusals: Vec<String> = Vec::new();
        for peer in peers {
            let held = self.membership.snapshot();
            let current = offload_proto::handshake::Credentials {
                membership: held.membership,
                delegation: held.chain,
            };
            match self.cluster.renew_with(peer, window, current).await {
                Ok(credentials) => match self.membership.adopt(credentials, now) {
                    Ok(crate::fleet::Adopted::Taken) => {
                        // `by`, not `approver`: since ADR-0069 any member holding `Renew` renews,
                        // and the field named every one of them an approver.
                        let taken = self.membership.snapshot().membership;
                        tracing::info!(
                            by = %peer.short(),
                            as_renewer = taken.renewer().is_some(),
                            reapproved = taken.approved_at() > before.approved_at(),
                            "membership certificate renewed"
                        );
                        return true;
                    }
                    // Not an improvement, or not ours. Either way this peer is not the answer
                    // and the next one might be.
                    Ok(crate::fleet::Adopted::NoBetter) => {
                        refusals.push(format!("{}: offered no better a certificate", peer.short()));
                        tracing::debug!(
                            node = %peer.short(),
                            "offered a certificate that was no better than the one held"
                        );
                    }
                    // A renewal restates the grants it descends from, so an approver answering
                    // `RenewMe` with a narrower certificate is a peer doing something it was
                    // never asked to do. At `warn`, named, and not taken up: this is the one
                    // arm that arrives over the network with nobody having pasted anything.
                    Ok(outcome @ crate::fleet::Adopted::Narrower { .. }) => {
                        refusals.push(format!(
                            "{}: offered one that drops {}",
                            peer.short(),
                            outcome.lost()
                        ));
                        tracing::warn!(
                            node = %peer.short(),
                            lost = %outcome.lost(),
                            "an approver offered a certificate that takes grants away; not taken up"
                        );
                    }
                    Err(e) => {
                        refusals.push(format!("{}: {e}", peer.short()));
                        tracing::warn!(
                            node = %peer.short(),
                            error = %e,
                            "could not take up a renewed certificate"
                        );
                    }
                },
                Err(reason) => {
                    refusals.push(format!("{}: {reason}", peer.short()));
                    tracing::debug!(node = %peer.short(), %reason, "did not renew");
                }
            }
        }
        // Only a re-approval was being asked for, and nobody has decided to give one yet. That is
        // the ordinary state for most of the approval's last month, so it is not worth a warning
        // every fifteen minutes; `offload status` and `offload fleet` say it to the person who can
        // act on it, which is the report that matters.
        if !renewal_due {
            tracing::debug!(
                refused = %refusals.join("; "),
                "this node's approval runs out within a month and no approver has re-approved it yet"
            );
            return false;
        }
        // The approval's year is the cause when it has run out or is about to, and no renewal
        // fixes that: every peer refuses to restate an approval a person has not renewed. The
        // certificate sentence below, printed here instead, sent a lapsed approver looking for
        // a second approver every twenty seconds (the ADR-0069 §3 walk, session ninety-two).
        if before.is_due_for_reapproval(now) || now >= before.approval_expires_at() {
            tracing::warn!(
                refused = %refusals.join("; "),
                "this node's approval has run out or is about to, and a renewal cannot restate \
                 it — it needs a person: `offload reapprove` for this node on an approver, or \
                 the passphrase"
            );
            return false;
        }
        // Carries the reasons it just collected, rather than naming one cause it never measured.
        // The old line said "a fleet with no approver needs `offload grant approve` on a device
        // that has one" whatever had happened, and on the fleet this warning fires for most it
        // was **false**: `peers` excludes `me` — deliberately, since a node that could re-sign
        // its own membership would never fall out of a fleet it had been removed from — so the
        // sole approver in a fleet can never renew itself, and the device printing "a fleet with
        // no approver" is the approver. Measured in session seventy-eight on both nodes of a
        // two-device fleet: on the founder, whose own `offload fleet` said `approver this node
        // may enrol others` one command away; and on the joiner, where the approver was reachable
        // and had answered, and the answer was refused one line above (`lost=host-runs`).
        if self
            .membership
            .snapshot()
            .membership
            .granted(Grant::Approve, now)
        {
            tracing::warn!(
                refused = %refusals.join("; "),
                "this node's membership certificate is running out and no peer would renew it. \
                 This node is itself an approver and cannot renew its own certificate, so a \
                 second device needs `offload grant approve`"
            );
        } else {
            tracing::warn!(
                refused = %refusals.join("; "),
                "this node's membership certificate is running out and no peer would renew it — \
                 if no device in this fleet holds `approve`, one needs `offload grant approve`"
            );
        }
        false
    }

    /// Write down any member this node has met and never recorded (ADR-0012 mitigation 4).
    ///
    /// The other half of announcing an enrolment, and the half that reaches somebody. The
    /// machine that issued an invitation records it, but that machine may hold no route to a
    /// person and may be the very machine an attacker used. Every *other* node in the fleet
    /// meets the device eventually and says so — which turns "an enrolment nobody performed"
    /// into an alarm on whichever device is holding a push credential.
    ///
    /// Once per device per node, because the log is a record of what this node witnessed and
    /// not the fleet's memory. A device that was invited *here* is already recorded, so it does
    /// not announce itself twice.
    pub fn note_new_members(&self, store: &offload_store::Store) {
        for cert in self.cluster.certificates() {
            {
                // Asked once per device per daemon rather than once a second: this loop runs on
                // the reactor, `offload-store` is synchronous by decision (ADR-0009), and a
                // peer that has been in the fleet for a year should cost nothing to keep not
                // announcing.
                let mut noted = self
                    .noted
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !noted.insert(cert.member) {
                    continue;
                }
            }
            match store.knows_member(&cert.member) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "could not check the fleet log");
                    return;
                }
            }
            let event = offload_core::FleetLogEvent::new(
                SystemClock.now().0,
                offload_core::FleetEvent::Enrolled {
                    node: cert.member,
                    name: cert.name.clone(),
                    grants: cert.grants.clone(),
                    how: offload_core::Enrolment::Met,
                },
            );
            match store.append_fleet_event(&event) {
                Ok(_) => tracing::info!(
                    node = %cert.member.short(),
                    name = %cert.name,
                    "a member this node had never met is in the fleet"
                ),
                Err(e) => tracing::warn!(error = %e, "could not record a new member"),
            }
        }
    }

    /// Re-read `fleet.json`, and hang up on anybody it has just revoked.
    ///
    /// The hang-up is the half that is easy to leave out and the half that matters: membership
    /// is checked at the handshake, and a live QUIC session never handshakes again. Refusing
    /// the next connection to a device that is not going to make one is not a revocation.
    pub async fn refresh_membership(&self) -> Result<FleetChange, crate::fleet::FleetError> {
        let change = self.membership.reload()?;
        for peer in &change.revoked {
            self.cluster.disconnect(*peer, "revoked").await;
        }
        if change.refounded {
            // Everything this node is connected to belongs to the fleet it has just left. Each
            // of those sessions is authenticated against a key that no longer means anything
            // here, and none of them will survive a re-handshake — so they go now rather than
            // one failed probe at a time.
            let me = self.cluster.node();
            for peer in self.cluster.view().nodes.keys() {
                if *peer != me {
                    self.cluster
                        .disconnect(*peer, "the fleet was re-founded")
                        .await;
                }
            }
        }
        Ok(change)
    }

    /// Dial the seed list and put whoever answers into the view.
    ///
    /// Seeds are addresses, not keys: whoever answers has to present a certificate for this
    /// fleet, and their key is learned from the handshake. A seed that is down, or belongs to
    /// somebody else's fleet, is logged and skipped — bootstrapping is not the place to be
    /// fussy, because the fleet converges from any one working introduction.
    pub async fn bootstrap(&self, seeds: &[String], policy: &WorkPolicy) -> bool {
        let mut met = false;
        for seed in seeds {
            for address in resolve(seed).await {
                match self.transport.discover(address).await {
                    Ok(session) => {
                        tracing::info!(
                            node = %session.peer.node,
                            name = %session.peer.name,
                            %address,
                            "met a seed"
                        );
                        // A placeholder at incarnation 0: the peer's own gossip replaces it at
                        // the first exchange.
                        self.cluster.introduce(
                            session.peer.node,
                            unknown_capabilities(),
                            policy.clone(),
                        );
                        met = true;
                        // One working introduction per seed is the whole job — the fleet
                        // converges from any of them, and a name with both an A and an AAAA
                        // record is one seed, not two.
                        break;
                    }
                    Err(e) => tracing::warn!(%address, error = %e, "could not reach seed"),
                }
            }
        }
        met
    }

    /// Is there anybody worth talking to?
    ///
    /// What decides whether the seed list is dialled again (ADR-0037 §2). `is_gone` is the
    /// existing predicate and its wording is the right one here — "has it stopped being a useful
    /// place for a run to be" — because the case this exists for is a peer whose *address*
    /// changed: it goes `Alive`, `Suspect`, `Dead`, and by the time the fleet has concluded it is
    /// gone, re-dialling the seed is exactly the right response.
    #[must_use]
    pub fn has_a_live_peer(&self) -> bool {
        any_live_peer(&self.cluster.view(), self.cluster.node())
    }

    /// Announce this node on the LAN and dial whoever announces back.
    ///
    /// Runs until the process ends. Discovery only ever *proposes* a peer: the dial that
    /// follows is what decides, because an advertisement is a claim about a key and the
    /// handshake is the thing that checks it.
    pub async fn discover(
        self: Arc<Self>,
        name: String,
        policy: WorkPolicy,
    ) -> Result<(), offload_transport::TransportError> {
        let bound = self.address()?;
        let node = self.cluster.node();

        if discovery::Advertisable::of(bound) == discovery::Advertisable::Loopback {
            // Not a failure and not something to work around: a loopback-bound node cannot be
            // reached from the LAN and cannot reach it, so announcing would only publish this
            // machine's *other* addresses and send peers somewhere nothing is listening. Said
            // once, with the way out in it, because two nodes on one machine is a supported and
            // common setup — it is how every multi-node test in this repository is run.
            tracing::info!(
                %bound,
                "bound to loopback, so not announcing on the LAN; peers reach this node through \
                 `seeds` instead"
            );
            return Ok(());
        }

        // Held for as long as this task lives: dropping it withdraws the advertisement, which
        // is what stops peers dialling a machine that has gone.
        let _advertisement: Advertisement = discovery::advertise(node, bound, &name)?;
        let browser = discovery::browse(node)?;
        tracing::info!(%bound, "advertising on the LAN");

        let mut tried: HashMap<NodeId, Instant> = HashMap::new();
        while let Some(found) = browser.next().await {
            // Skipped only when it is already alive here. "Known" was the test, and a node known
            // only from a peer's gossip, marked dead there, has no address here: every
            // announcement it made was thrown away, and it came back only if it dialled in. After
            // a restart the laptop never found the Mac it could see on the LAN (session
            // ninety-two). Dead or suspect is exactly when the address it just announced matters.
            let alive = self
                .cluster
                .view()
                .nodes
                .get(&found.node)
                .is_some_and(|n| n.status == offload_core::NodeStatus::Alive);
            if alive {
                continue;
            }
            // A node from another fleet advertises on the same multicast group and is refused
            // at the handshake — correctly, and every time. The cooldown is what stops that
            // correct refusal from becoming a dial every few seconds for as long as both
            // machines are on the same wifi.
            let recent = tried
                .get(&found.node)
                .is_some_and(|at| at.elapsed() < RETRY_COOLDOWN);
            if recent {
                continue;
            }
            tried.insert(found.node, Instant::now());

            // Every address it advertised, in turn: a laptop announces its wifi and its dock,
            // and only one of them reaches it from here.
            for address in &found.addresses {
                match self.transport.discover(*address).await {
                    Ok(session) => {
                        tracing::info!(
                            node = %session.peer.node.short(),
                            name = %session.peer.name,
                            %address,
                            "found a peer on the LAN"
                        );
                        self.cluster.introduce(
                            session.peer.node,
                            unknown_capabilities(),
                            policy.clone(),
                        );
                        break;
                    }
                    Err(e) => tracing::debug!(
                        node = %found.node.short(),
                        %address,
                        error = %e,
                        "discovered node would not talk to us there"
                    ),
                }
            }
        }
        Ok(())
    }

    /// State where this node can be dialled, and dial peers at the addresses they state
    /// (ADR-0076). Runs until the process ends, whether or not mDNS is on: it is for the peers
    /// mDNS does not reach, a laptop that never hears a Mac's announcements among them.
    pub async fn gossip_addresses(self: Arc<Self>, policy: WorkPolicy) {
        /// How often to re-state this node's addresses: interfaces come and go on a laptop.
        const RESTATE_EVERY: u32 = 3;
        /// How long before a peer is dialled at its gossiped addresses again.
        const REDIAL: Duration = Duration::from_secs(60);
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        let mut n: u32 = 0;
        loop {
            tick.tick().await;
            if n % RESTATE_EVERY == 0 {
                if let Ok(bound) = self.address() {
                    let addresses: Vec<String> =
                        offload_transport::discovery::dialable_addresses(bound)
                            .into_iter()
                            .map(|a| a.to_string())
                            .collect();
                    if self.cluster.set_addresses(addresses.clone()) {
                        tracing::debug!(?addresses, "where this node can be dialled has changed");
                    }
                }
            }
            n = n.wrapping_add(1);

            // Not while quiet (ADR-0078): dialling a sleeping peer every minute wakes this phone as
            // surely as being probed does. It dials again once the screen is on or it is charging.
            let me_quiet = self
                .cluster
                .view()
                .nodes
                .get(&self.cluster.node())
                .is_some_and(|me| me.quiet);
            if me_quiet {
                continue;
            }

            // Peers this node cannot currently hear from, that say where they can be dialled.
            let me = self.cluster.node();
            let wanted: Vec<(NodeId, Vec<String>)> = self
                .cluster
                .view()
                .nodes
                .values()
                .filter(|v| v.id != me && !v.addresses.is_empty())
                .filter(|v| {
                    matches!(
                        v.status,
                        offload_core::NodeStatus::Dead | offload_core::NodeStatus::Suspect
                    )
                })
                .map(|v| (v.id, v.addresses.clone()))
                .collect();
            for (node, addresses) in wanted {
                {
                    let mut dialled = self
                        .gossip_dialled
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if dialled.get(&node).is_some_and(|at| at.elapsed() < REDIAL) {
                        continue;
                    }
                    dialled.insert(node, Instant::now());
                }
                for address in addresses
                    .iter()
                    .filter_map(|a| a.parse::<SocketAddr>().ok())
                {
                    match self.transport.discover(address).await {
                        Ok(session) if session.peer.node == node => {
                            tracing::info!(
                                node = %node.short(),
                                name = %session.peer.name,
                                %address,
                                "reached a peer at an address it gossips"
                            );
                            self.cluster
                                .introduce(node, unknown_capabilities(), policy.clone());
                            break;
                        }
                        // Somebody else is there now: the handshake said who, and it is not the
                        // node this address was stated for. Nothing to believe.
                        Ok(session) => tracing::debug!(
                            node = %node.short(),
                            %address,
                            answered = %session.peer.node.short(),
                            "a gossiped address now belongs to another node"
                        ),
                        Err(e) => tracing::debug!(
                            node = %node.short(),
                            %address,
                            error = %e,
                            "could not reach a peer at an address it gossips"
                        ),
                    }
                }
            }
        }
    }

    /// Hand every run this node is holding to somebody else, and stop accepting new ones.
    ///
    /// The graceful half of migration, and the shape of the whole product claim: close the
    /// laptop, the agent keeps working on the desktop. Each run is checkpointed at its next
    /// *turn boundary* — never mid-turn, because an agent halfway through a tool call has
    /// state in the tool rather than the transcript (ADR-0004) — and then offered to the
    /// fleet exactly like a fresh submission.
    ///
    /// Returns how many runs found a new home, and how many are **still here**. Anything that
    /// did not move is left with its lease running: an ungraceful departure is the ordinary
    /// orphan path (ADR-0007), and pretending otherwise by force-killing the agent would lose the
    /// turn this waited for.
    ///
    /// **Both numbers are counted here because this is the only thing that knows them.** The
    /// caller used to ask `Supervisor::held_count` for the second one, which is a *capacity*
    /// question — how many runs is this node holding — and a run this drain has just released is
    /// deliberately not held: a checkpoint that captured with `release = true` leaves the run
    /// `Pending` with no holder. So the case that matters most reported zero. Walked: a run at
    /// turn 3 on a fleet whose only peer cannot host, drained; it checkpointed at turn 7, was
    /// released, was refused by everybody, and `offload drain` printed **`nothing to hand
    /// over`** — the same words it prints on an idle laptop — while the daemon's own log said
    /// `nobody would take it; it stays here`. The agent had been stopped and the person about to
    /// close the lid was told there was nothing to do. Third time this command has said something
    /// untrue about what it did.
    ///
    /// Every run ends this pass in exactly one of three ways — handed over, still mid-turn at the
    /// deadline, or refused by every other node — so counting them as they go is exact, where
    /// asking a question afterwards is a different question asked at a different instant.
    /// Private, so [`depart`] is the only way in: the departure and the handover pass are two
    /// facts, and every time they have been reachable separately one caller has taken the flag
    /// and the other has not.
    async fn drain(
        &self,
        supervisor: &crate::Supervisor,
        deadline: Duration,
        window: Millis,
        lifespan: Lifespan,
        report: &Report,
    ) -> Drained {
        // The departure itself is recorded by [`depart`], which is the only way in here — before
        // it decides whether there is a handover pass to run at all, because a fleet of one is
        // leaving too. It used to be the first line of this function, which fixed the
        // early-return case one level too low, and then the first line of `Request::Drain`,
        // which fixed one of this function's two callers and left the signal path accepting work
        // for the whole of its shutdown. Two wrong homes for one fact; `depart` is the third.

        let held: Vec<offload_core::Run> = supervisor
            .gossipable_runs()
            .into_iter()
            .filter(|run| run.holder() == Some(self.cluster.node()) && !run.state.is_terminal())
            .collect();

        if held.is_empty() {
            return Drained::default();
        }
        tracing::info!(runs = held.len(), "draining");

        // A run this node accepted and never started (ADR-0006) has no turn to finish and
        // nothing to capture — it is a commitment, and a departing node's commitment is worth
        // nothing to anybody. Hand it straight back to the pool rather than making the drain
        // wait out a checkpoint deadline for an agent that was never spawned.
        let unstarted = supervisor.release_unstarted(&held);
        let mut out = Drained::default();
        for run in &unstarted {
            match self
                .cluster
                .place(run, Err("draining".into()), window)
                .await
            {
                offload_cluster::Placement::Accepted { name, starting, .. } => {
                    tracing::info!(run_id = %run.id, %name, %starting, "handed over unstarted");
                    out.moved += 1;
                }
                offload_cluster::Placement::Refused { refusals, .. } => {
                    // It is `Pending` here now, which is honest: this node has stopped
                    // intending to run it, and says so rather than leaving a commitment behind
                    // that nothing will honour. And the record says *who* stopped intending it
                    // (ADR-0042), so a refusal now is not an answer about later: whoever
                    // arbitrates the run offers it again on the backoff, including after this
                    // daemon has gone. Nothing to remember here.
                    tracing::warn!(
                        run_id = %run.id,
                        refusals = refusals.len(),
                        "nobody would take the run we were holding for later; \
                         its arbiter keeps offering it"
                    );
                    out.pooled += 1;
                }
            }
        }

        let released: std::collections::BTreeSet<offload_core::RunId> =
            unstarted.iter().map(|run| run.id).collect();
        // **Split by whether a boundary is coming, not only by whether the run is ours.** A
        // running task is held here and started, so it landed in `mine` and was waited on for the
        // full `drain_deadline_secs` — 300 seconds by default — for a turn boundary a nominated
        // program does not have. Measured with the deadline cut to 20s: `offload drain` took the
        // whole 20s on one `/bin/sleep`, then promised to hand it over *"at its next turn
        // boundary"*. `Run::request_checkpoint` refuses it at the record now, so this pass would
        // merely log the refusal and wait anyway — the wait is the cost, and it is here.
        //
        // Nothing is owed for these. On `Lifespan::StaysUp` the node stays and the program runs
        // to its own end; on `Exits` main stops it as it already does, it fails, and ADR-0043's
        // pass hands it back to be re-run from its spec — which is the whole of what a task's
        // handover is (ADR-0058), and needs no boundary and no checkpoint.
        let (with_boundary, no_boundary): (Vec<&offload_core::Run>, Vec<&offload_core::Run>) = held
            .iter()
            .filter(|run| !released.contains(&run.id))
            .partition(|run| run.spec.work.kind() == offload_core::WorkKind::Agent);
        let mine: Vec<offload_core::RunId> = with_boundary.iter().map(|run| run.id).collect();
        for run in &no_boundary {
            tracing::info!(
                run_id = %run.id,
                "still running here and not waited for: a task has no turn boundary"
            );
        }
        out.no_boundary = no_boundary.len();

        // Ask them all to stop at their next boundary before waiting on any of them: turns
        // run in parallel, and a serial wait would make a drain take as long as the sum.
        for run in &mine {
            if let Err(e) = supervisor.request_checkpoint(*run, offload_core::GivenUp::LetGo) {
                tracing::debug!(run_id = %run, error = %e, "cannot checkpoint that one");
            }
        }

        let until = Instant::now() + deadline;
        // Said before the first wait rather than per run: turns run in parallel and the
        // checkpoints were all requested above, so what an operator is now in is one wait
        // bounded by one deadline.
        report.say(crate::api::DrainStep::Waiting {
            runs: u32::try_from(mine.len()).unwrap_or(u32::MAX),
            up_to_ms: u64::try_from(deadline.as_millis()).unwrap_or(u64::MAX),
        });
        let mut wait = Wait {
            supervisor,
            report,
            runs: mine.clone(),
            said: std::collections::BTreeSet::new(),
        };
        for run_id in mine {
            let run = match wait.boundary(run_id, until).await {
                Boundary::Reached(run) => *run,
                // The one outcome that is neither moved nor left behind, and it was being
                // counted and logged as the worst one.
                Boundary::Finished => {
                    tracing::info!(run_id = %run_id, "it finished while we waited");
                    out.finished += 1;
                    continue;
                }
                // **Not the end of this run's story, which is why it is no longer counted with
                // the runs nobody would take.** The checkpoint this pass requested is a standing
                // instruction until it is honoured, so the run goes on to finish its turn,
                // capture, and release itself `Pending` — minutes after this line. Whether that
                // is a handover or an abandonment is decided by whether this daemon is still
                // here when it happens.
                Boundary::StillMidTurn if lifespan == Lifespan::StaysUp => {
                    // Nothing is remembered, and nothing needs to be: the checkpoint this pass
                    // requested is a standing instruction until it is honoured, and the release
                    // it produces states that a node let the run go (ADR-0042). Whoever
                    // arbitrates the run — this node, usually, since it is generally home to
                    // what it is running — picks it up from the record on the next tick.
                    tracing::info!(
                        run_id = %run_id,
                        "still mid-turn at the drain deadline; handing it over at its next boundary"
                    );
                    out.later += 1;
                    continue;
                }
                // The process is going away, so there is no next boundary to wait for: main
                // stops every agent this pass could not move as soon as it returns, and the run
                // is the orphan path's from there (ADR-0007).
                Boundary::StillMidTurn => {
                    tracing::warn!(
                        run_id = %run_id,
                        "still mid-turn at the drain deadline; leaving it to the lease"
                    );
                    out.left += 1;
                    continue;
                }
                // Unknown is not good news: the record went away under us, so this says what it
                // saw rather than borrowing either of the sentences above.
                Boundary::Gone => {
                    tracing::warn!(
                        run_id = %run_id,
                        "its record went away while we waited for its turn to end"
                    );
                    out.left += 1;
                    continue;
                }
            };

            // This node is leaving, so it does not bid for its own run.
            match self
                .cluster
                .place(&run, Err("draining".into()), window)
                .await
            {
                offload_cluster::Placement::Accepted {
                    node,
                    name,
                    starting,
                    ..
                } if node != self.cluster.node() => {
                    tracing::info!(run_id = %run_id, %name, %starting, "handed over");
                    out.moved += 1;
                }
                // Accepted by *this* node, which a draining node does not bid for — so if it
                // happens the run has not gone anywhere and is still here.
                offload_cluster::Placement::Accepted { .. } => out.left += 1,
                offload_cluster::Placement::Refused { refusals, .. } => {
                    // **Not "it stays here", which is what this said and is not what happens.**
                    // The run was released before it was offered, so a refusal leaves it
                    // `Pending` in the pool with nobody holding it — and until ADR-0042 with
                    // nothing that would ever offer it again, which is the worse outcome the
                    // run got for reaching its boundary *inside* the deadline. The release
                    // states who let it go, so a refusal now is a fact about this instant and
                    // the run's arbiter keeps asking. That holds whichever [`Lifespan`] this is:
                    // the fact is on the record rather than in this process.
                    tracing::warn!(
                        run_id = %run_id,
                        refusals = refusals.len(),
                        "nobody would take it right now; its arbiter keeps offering it"
                    );
                    out.pooled += 1;
                }
            }
        }
        out
    }

    /// One pass of the ungraceful path: notice a holder that has gone, wait the right amount
    /// of time, then move the run or give up on it.
    ///
    /// Three states, deliberately distinct (ADR-0007). A lease that expired or a holder that
    /// has gone quiet makes a run **`Orphaned`** — which is an observation and *not* a
    /// decision: it grants no authority, and a returning holder reclaims it at the same epoch
    /// for free. What turns it into a migration is the hold-down, which is computed per case
    /// from how flaky that node has been, whether the run has a checkpoint to lose, and
    /// whether there is anywhere else to put it.
    ///
    /// Only the run's arbiter acts, so every node can run this loop and exactly one of them
    /// does anything — including when the node that *submitted* the run is the one that went
    /// away, which is the case the deterministic successor in `arbiter_for` exists for.
    ///
    /// The decision itself is [`offload_core::supervise`], a pure function of (view, run,
    /// now). What is left here is the two effects it can ask for: write the observation down
    /// and gossip it, or run a bid round.
    pub async fn supervise(&self, supervisor: &crate::Supervisor, window: Millis) {
        let now = SystemClock.now();
        let view = self.cluster.view();
        let policy = offload_core::ReassignPolicy::default();

        // Most urgent first, which matters for exactly one of the effects below: a node that
        // arbitrates several placeable runs and can place only some of them should offer the one
        // in the most trouble. Unsorted, that was `RunId` order — a UUIDv7, so *roughly*
        // submission order, which looks deliberate and is not (ADR-0013: slack leads, priority
        // breaks its ties). Everything else in the loop is per-run and does not care.
        let mut runs: Vec<&offload_core::Run> = view.runs.values().collect();
        runs.sort_by_key(|run| run.urgency_order(now));

        for run in runs {
            // Every node runs this loop; `offload_core::supervise` is what makes exactly one
            // of them act, and it is where the failover lives — a run whose home node has
            // gone is arbitrated by a deterministic successor rather than by nobody.
            match offload_core::supervise(&view, run, now, &policy) {
                offload_core::Supervision::Bystander(_) => {}

                // Nobody holds it and somebody should. Two ways in, carried on the decision
                // rather than worked out again here (ADR-0042): a run somebody queued
                // (ADR-0014) and nobody has taken, and a run a node let go on its way out.
                // Offering the first again is the whole of what `--queue` promised — "later" is
                // not a state a run reaches by itself — and offering the second is the promise
                // this product is named for. Rate-limited by the same backoff a reassignment
                // uses, because a fleet that had no room a second ago still has none.
                //
                // **Exactly one *node* offers it, and that is not the whole property.** The
                // debt a drain used to keep in memory made the departing node offer its own run
                // while the run's arbiter watched — right only because the arbiter could not see
                // the fact and did nothing. The fact is on the record now, so exactly one node
                // acts on it: whoever arbitrates.
                //
                // What this comment used to claim, and what cost a walk to find out it did not:
                // one node is not one *pass*. A draining node runs this loop **and** `drain`'s own
                // round for the same released run, and two rounds on one node grant the same epoch
                // twice — the one thing `place` says the epoch exists to make impossible. The
                // exclusion for that is per run inside `Cluster::place` (ADR-0050), not here,
                // because it is the round that spends the token and this is only one of its
                // callers.
                offload_core::Supervision::Place(offering) => {
                    if !self.may_retry(run.id) {
                        continue;
                    }
                    // A draining node's own `evaluate` answers `draining`, which is what the
                    // drain's own pass passed by hand: a node on its way out does not bid for a
                    // run it is trying to be rid of.
                    let mine = self.cluster.host().evaluate(run).await;
                    match self.cluster.place(run, mine, window).await {
                        offload_cluster::Placement::Accepted { name, starting, .. } => {
                            tracing::info!(
                                run_id = %run.id, %name, %starting, ?offering,
                                "an unheld run was placed"
                            );
                        }
                        offload_cluster::Placement::Refused { refusals, .. } => {
                            tracing::debug!(
                                run_id = %run.id,
                                refusals = refusals.len(),
                                ?offering,
                                "still nobody for it"
                            );
                            // ADR-0013's other reporting row. A queued run is offered again
                            // every thirty seconds for as long as it takes, which is the whole
                            // of what `--queue` promised — and for a run with a deadline
                            // somebody stated, "for as long as it takes" quietly stopped being
                            // what they asked for at some point in the night. Said once, and
                            // said *after* a refused round rather than from the run merely
                            // being pending and late: this is a claim that the fleet was asked,
                            // and between two rounds a placeable run is pending too.
                            //
                            // Nothing else changes. The run keeps being offered, because a
                            // deadline decides when we give up and not what happens to the
                            // work; giving up on it here would be cancelling somebody's run on
                            // a timer they set as a scheduling hint.
                            if let Some(by) = run.prospect_at(now).missed_by() {
                                let refused_by = u32::try_from(refusals.len()).unwrap_or(u32::MAX);
                                supervisor.note_overdue(
                                    run,
                                    by,
                                    offload_core::Waiting::Placement { refused_by },
                                );
                            }
                        }
                    }
                }

                // Losing contact is an observation. Nothing has been decided yet, which is
                // why this is a state of its own and not `Pending`.
                offload_core::Supervision::Orphan => {
                    let mut orphaned = run.clone();
                    if orphaned.orphan(now).is_ok() {
                        tracing::info!(
                            run_id = %run.id,
                            node = %run.holder().map(|n| n.short()).unwrap_or_default(),
                            "holder out of contact; run is orphaned"
                        );
                        self.publish(supervisor, orphaned);
                    }
                }

                offload_core::Supervision::Decided(decision) => match decision {
                    offload_core::ReassignDecision::Hold { reason, .. } => {
                        tracing::debug!(run_id = %run.id, ?reason, "holding");
                    }
                    offload_core::ReassignDecision::NoAction => {}
                    offload_core::ReassignDecision::Abandon { reason } => {
                        let mut abandoned = run.clone();
                        if abandoned.abandon(reason.clone(), now).is_ok() {
                            tracing::warn!(run_id = %run.id, %reason, "abandoned");
                            self.publish(supervisor, abandoned);
                        }
                    }
                    offload_core::ReassignDecision::Reassign { reason } => {
                        if !self.may_retry(run.id) {
                            continue;
                        }
                        tracing::info!(run_id = %run.id, ?reason, "reassigning");
                        // The same round a submission runs, from a different starting state.
                        // The old holder is not a candidate: it is not available, so nobody
                        // asks it.
                        let mine = self.cluster.host().evaluate(run).await;
                        match self.cluster.place(run, mine, window).await {
                            offload_cluster::Placement::Accepted { name, starting, .. } => {
                                tracing::info!(run_id = %run.id, %name, %starting, "reassigned");
                            }
                            offload_cluster::Placement::Refused { refusals, .. } => {
                                tracing::warn!(
                                    run_id = %run.id,
                                    refusals = refusals.len(),
                                    "nobody will take it; it stays orphaned"
                                );
                            }
                        }
                    }
                },
            }
        }
    }

    /// Give back the commitments this node can no longer honour in time.
    ///
    /// The other half of accepting-without-starting (ADR-0006), and the half that needed a
    /// deadline before it could exist. A full node takes work rather than declining it, which
    /// is right — but the promise can go stale: the run is due, there is still no room here,
    /// and some other machine could start it now. Sitting on it then is exactly the
    /// babysitting this project exists to remove, and nothing looks wrong while it happens.
    ///
    /// Three things this is careful about:
    ///
    /// * **It offers before it lets go.** A run is never left `Pending` in nobody's hands: if
    ///   the round comes back refused, this node takes its own commitment back rather than
    ///   filing the run away (ADR-0014). The cost of being wrong is a bid round.
    /// * **Only a stated deadline moves it.** `review_commitment` is where that lives, and the
    ///   reason is in its docs: with an unspecified deadline every held run is overdue within a
    ///   second, so the rule would bounce every commitment in the fleet from queue to queue.
    /// * **Rate-limited by the same backoff a reassignment uses.** A run that is late stays
    ///   late, so this question has the same answer for the next thirty seconds.
    pub async fn hand_back_late_commitments(
        &self,
        supervisor: &crate::Supervisor,
        capacity: offload_core::Capacity,
        window: Millis,
    ) {
        let now = SystemClock.now();
        let view = self.cluster.view();
        let me = self.cluster.node();
        // Once for the pass, not once per run: it takes the connection table's lock and the
        // answer does not depend on the run.
        let barred = self.peers_barred_from_hosting(&view, me).await;

        for run in supervisor.held_unstarted() {
            let ready = ready_elsewhere(&view, me, &run, &barred, now);
            match offload_core::review_commitment(&run, ready, now) {
                offload_core::Commitment::Keep(reason) => {
                    tracing::trace!(run_id = %run.id, %reason, "keeping the commitment");
                }
                offload_core::Commitment::GiveBack { overdue_by } => {
                    if !self.may_retry(run.id) {
                        continue;
                    }
                    let Some(released) = supervisor
                        .release_unstarted(std::slice::from_ref(&run))
                        .into_iter()
                        .next()
                    else {
                        continue;
                    };
                    tracing::info!(
                        run_id = %run.id,
                        %overdue_by,
                        ready,
                        "due and still not started here; offering it to the fleet"
                    );
                    let mine = Err(format!(
                        "committed to it here but cannot start it yet, and it is overdue by \
                         {overdue_by}"
                    ));
                    match self.cluster.place(&released, mine, window).await {
                        offload_cluster::Placement::Accepted { name, starting, .. } => {
                            tracing::info!(
                                run_id = %run.id, %name, %starting,
                                "a late commitment was handed over"
                            );
                        }
                        offload_cluster::Placement::Refused { refusals, .. } => {
                            // Nobody after all — the view was a tick old, or every bidder went
                            // busy in between. Take the promise back rather than leaving the
                            // run pending with nobody holding it.
                            tracing::info!(
                                run_id = %run.id,
                                refusals = refusals.len(),
                                "nobody else would take it; keeping the commitment here"
                            );
                            if let Err(e) = supervisor.take_run(released, capacity).await {
                                tracing::warn!(
                                    run_id = %run.id, error = %e,
                                    "could not take back the run we offered"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// The peers this node knows the fleet has not permitted to host runs.
    ///
    /// Read from the certificate each peer presented on the connection this node still holds —
    /// the same source, and now the same predicate, the arbiter uses to refuse a bid it does not
    /// believe (`Cluster::hosting_objection`). Nothing is gossiped for this and nothing needs to
    /// be: the fact was already here, one function further on than the estimate was looking.
    ///
    /// **Only nodes it can positively rule out.** `peer_hosts_runs` answers `None` where there is
    /// no live connection, and unknown stays *counted* — which is the opposite of the usual rule
    /// here and is the right way round for this one question. `review_commitment` says which way
    /// to be wrong: "being wrong there costs a bid round; being unwilling to estimate at all
    /// costs the run its night." So this can only ever turn a doomed round into no round, and can
    /// never turn a possible handover into a run left on a busy machine.
    async fn peers_barred_from_hosting(
        &self,
        view: &offload_core::ClusterView,
        me: NodeId,
    ) -> std::collections::BTreeSet<NodeId> {
        let mut barred = std::collections::BTreeSet::new();
        for node in view.alive() {
            if node.id != me && self.cluster.peer_hosts_runs(node.id).await == Some(false) {
                barred.insert(node.id);
            }
        }
        barred
    }

    /// Has this run waited long enough for another attempt? Records the attempt if so.
    fn may_retry(&self, run: offload_core::RunId) -> bool {
        let mut retried = self
            .retried
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match retried.get(&run) {
            Some(at) if at.elapsed() < REASSIGN_RETRY => false,
            _ => {
                retried.insert(run, Instant::now());
                true
            }
        }
    }

    /// Write a run down and tell the fleet, in that order: the store is what survives a
    /// restart, and gossip is what makes the decision visible before then.
    fn publish(&self, supervisor: &crate::Supervisor, run: offload_core::Run) {
        if let Err(e) = supervisor.record_run(&run) {
            tracing::warn!(run_id = %run.id, error = %e, "could not record a run decision");
        }
        self.cluster.publish_run(run);
    }

    pub fn shutdown(&self) {
        self.transport.shutdown();
    }
}

/// What became of a run the drain was waiting on.
///
/// Four outcomes, because the function this replaced answered `None` for three of them and its
/// caller reported one: a run that *finished* while the drain waited — which the code's own
/// comment called "the nicest possible outcome" — was counted as a run left behind and logged as
/// `still mid-turn at the drain deadline`. Measured twice in one walk, once on the shutdown path,
/// and the sentence it produces is the one that keeps somebody's lid open for nothing.
enum Boundary {
    /// Checkpointed and released: ready to be offered to the fleet.
    ///
    /// Boxed because a `Run` is much larger than the three sentences beside it, and an enum is
    /// as big as its widest arm.
    Reached(Box<offload_core::Run>),
    /// It ended by itself while we waited. Nothing to hand over and nothing left behind.
    Finished,
    /// Still mid-turn when the deadline came. Left with its lease (ADR-0004).
    StillMidTurn,
    /// The record stopped existing under us. Not good news, and not the same news as either
    /// of the two above.
    Gone,
}

/// The drain's wait, and the running commentary that goes with it.
///
/// It holds the whole list of runs still being waited on rather than one at a time, because the
/// waits are serial and the *questions* are not: with one run blocked for five minutes, a
/// question on the second run would otherwise not be mentioned until the first wait was over.
struct Wait<'a> {
    supervisor: &'a crate::Supervisor,
    report: &'a Report,
    runs: Vec<offload_core::RunId>,
    /// Questions already reported, so polling every 250ms says each one once. Keyed on the
    /// `tool_use_id` for the same reason an answer is addressed to it: an agent can be blocked on
    /// several calls at once, and a retried hook re-registers the same one.
    said: std::collections::BTreeSet<(offload_core::RunId, String)>,
}

impl Wait<'_> {
    /// Wait for one run to reach a state a migration can move: checkpointed and released.
    ///
    /// Polled rather than notified, for the same reason the rest of this daemon polls: it cannot
    /// miss a transition, and a drain is a rare, human-initiated event where a second of latency
    /// is invisible.
    async fn boundary(&mut self, run_id: offload_core::RunId, until: Instant) -> Boundary {
        loop {
            let Some(run) = self.supervisor.run(run_id) else {
                return Boundary::Gone;
            };
            let ready = matches!(run.state, offload_core::RunState::Pending { .. })
                && run.checkpoint.is_some();
            if ready {
                return Boundary::Reached(Box::new(run));
            }
            if run.state.is_terminal() {
                return Boundary::Finished;
            }
            if Instant::now() >= until {
                return Boundary::StillMidTurn;
            }
            self.questions(until);
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Say, once each, what the runs this pass is waiting on are blocked on.
    ///
    /// The whole reason a drain reports anything at all (ADR-0035). A run stopped mid-tool-call
    /// on `--ask` reaches no turn boundary until somebody answers or its patience runs out — both
    /// of which are five minutes by default — so the drain waits the whole window out while the
    /// one person who could end it instantly is the person who typed `offload drain` and is
    /// looking at nothing.
    ///
    /// **Both clocks, because two different things end this wait and they end it differently.**
    /// The question's own patience running out *decides* it (ADR-0017), after which the agent
    /// takes its turn, reaches a boundary and the run is handed over normally. The drain's
    /// deadline running out first does not decide anything — it leaves the run mid-turn with the
    /// question still open and still answerable. Reporting only the first is what this did, and
    /// it is the report that reads a clock the decision does not: an operator ten seconds from
    /// having the run abandoned under them was told `or 4m50s from now`, which is true about the
    /// question and wrong about what they are waiting for (ADR-0035's residual).
    fn questions(&mut self, until: Instant) {
        let drain_left = until
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        for ask in self.supervisor.asks() {
            if !self.runs.contains(&ask.run) {
                continue;
            }
            let key = (ask.run, ask.tool_use_id.clone());
            if !self.said.insert(key) {
                continue;
            }
            // In the log as well as down the wire: the shutdown path has no client to tell, and
            // "why did stopping this daemon take five minutes" is the same question there.
            let drain_ends_in =
                offload_core::Millis(u64::try_from(drain_left.as_millis()).unwrap_or(u64::MAX));
            tracing::info!(
                run_id = %ask.run,
                tool = %ask.tool,
                within = %ask.left,
                drain_ends_in = %drain_ends_in,
                "waiting on a run that is blocked for an answer; answering it releases the drain"
            );
            self.report.say(crate::api::DrainStep::Blocked {
                // Whole, not `short()`. The CLI prints this straight into an `offload approve`
                // line, and an instruction is not a column: there is nothing for it to line up
                // with and nothing an abbreviation buys, while twelve characters of a derived
                // occurrence id name a *tick* rather than a run (ADR-0056).
                run: ask.run.to_string(),
                tool: ask.tool,
                detail: ask.detail,
                tool_use_id: ask.tool_use_id,
                within_ms: ask.left.0,
                drain_ends_in_ms: drain_ends_in.0,
            });
        }
    }
}

/// Is anybody in this view worth talking to, other than us?
///
/// A pure function over the view for the usual reason: what it decides — whether the seed list is
/// dialled again — is a *policy* question, and the version that reached into a live cluster could
/// only be exercised by standing one up.
fn any_live_peer(view: &offload_core::ClusterView, me: NodeId) -> bool {
    view.nodes
        .values()
        .any(|node| node.id != me && !node.status.is_gone())
}

/// Turn one seed into addresses to try — **every time it is dialled**, not once at startup.
///
/// A seed used to be `seed.parse::<SocketAddr>()`, which refused a name outright: `seed is not a
/// host:port address`. That made dynamic DNS unconfigurable, and a changing home address is the
/// ordinary case for the fleet this exists for (ADR-0037). `lookup_host` takes both — a literal
/// costs no query — and resolving on every attempt is the point rather than an implementation
/// detail: the name is the stable thing, and what it points at is what changed.
///
/// Addresses stay inside the transport layer either way (ADR-0015 §1); this hands them straight
/// to `discover` and nothing above learns one.
async fn resolve(seed: &str) -> Vec<SocketAddr> {
    match tokio::net::lookup_host(seed).await {
        Ok(addresses) => {
            let addresses: Vec<SocketAddr> = addresses.collect();
            if addresses.is_empty() {
                tracing::warn!(seed = %seed, "seed resolved to no addresses");
            }
            addresses
        }
        // Not an error to abort on: a seed that does not resolve *yet* is the whole reason the
        // seed list is dialled again. A daemon that starts before the network is up is ordinary.
        Err(e) => {
            tracing::warn!(seed = %seed, error = %e, "cannot resolve seed");
            Vec::new()
        }
    }
}

/// How long before re-dialling a node that would not talk to us. Long enough that a foreign
/// fleet on the same wifi is a handful of refused handshakes an hour rather than per minute.
/// How long a node whose certificate or approval is due waits between rounds of asking
/// ([`Mesh::renew_if_due`]). Public so `offload reapprove` states it rather than restating it.
pub const RENEW_RETRY: Duration = Duration::from_secs(15 * 60);

const RETRY_COOLDOWN: Duration = Duration::from_secs(300);

/// What is known about a device nobody has gossiped about yet: nothing.
///
/// Deliberately not a guess. A placeholder that claimed cores would win bids it should lose,
/// and `Capabilities::empty` is the honest form of "we have only met the key".
fn unknown_capabilities() -> Capabilities {
    Capabilities::empty(
        offload_core::Os::Linux,
        offload_core::Arch::X86_64,
        offload_core::DeviceClass::Unknown,
    )
}

/// Start the mesh, or explain why there isn't one.
///
/// Returns `None` for the ordinary reasons a node is alone — no fleet, or `enabled = false` —
/// after saying so. An error is for a mesh that was asked for and could not be built.
pub fn start(
    config: &Config,
    identity: &NodeIdentity,
    capabilities: &Capabilities,
    policy: WorkPolicy,
    fleet: Option<FleetState>,
    store: offload_store::Store,
) -> Result<Option<Mesh>, MeshError> {
    if !config.cluster.enabled {
        tracing::info!("cluster disabled by config; this node is a fleet of one");
        return Ok(None);
    }
    let Some(state) = fleet else {
        tracing::info!(
            "not a member of a fleet, so there is nobody to talk to — `offload init` or \
             `offload join --passphrase`"
        );
        return Ok(None);
    };
    if let Err(e) = state.check(identity.id(), SystemClock.now()) {
        // Refusing to listen is the honest response: every peer would refuse us anyway, and
        // an endpoint nobody can use is worse than a message saying why.
        tracing::warn!(error = %e, "membership is not usable; not joining the mesh");
        return Ok(None);
    }

    let listen: SocketAddr =
        config
            .cluster
            .listen
            .parse()
            .map_err(|e: std::net::AddrParseError| MeshError::BadAddress {
                address: config.cluster.listen.clone(),
                reason: e.to_string(),
            })?;

    let membership = NodeMembership::new(
        state,
        crate::fleet::path(&config.state_dir),
        identity.signing_key().clone(),
    );
    let resolver: Arc<dyn Resolver> = Arc::new(StaticPeers::new());
    let transport = Arc::new(QuicTransport::bind(
        listen,
        identity.signing_key(),
        &config.name,
        membership.clone() as Arc<dyn Membership>,
        resolver,
    )?);

    let policy_for_blobs = policy.clone();
    let local = NodeView::new(
        identity.id(),
        capabilities.clone(),
        policy,
        SystemClock.now(),
    )
    // **The certificate's name, not the config's.** This entry is what the node gossips about
    // itself and what `Cluster::name_of(self.node())` answers — the `by` on a forwarded cancel or
    // answer, and every peer-facing sentence that names this machine. `config.name` defaults to
    // the hostname, so two devices can carry it; the certificate's is the one the fleet gave this
    // device and is signed (ADR-0012). `offload status` still shows the configured one, which is
    // the local half of that split and is what `report_membership` warns about at startup.
    .named(crate::fleet::display_name(&config.state_dir, &config.name));

    let cluster = Cluster::new(
        transport.clone() as Arc<dyn Transport>,
        local,
        config.cluster.detector(),
        Arc::new(SystemClock),
        StoreBlobs::new(store, policy_for_blobs, capabilities) as Arc<dyn Blobs>,
    );

    Ok(Some(Mesh {
        cluster,
        transport,
        membership,
        noted: std::sync::Mutex::new(std::collections::HashSet::new()),
        renewed: std::sync::Mutex::new(None),
        retried: std::sync::Mutex::new(HashMap::new()),
        gossip_dialled: std::sync::Mutex::new(HashMap::new()),
    }))
}

/// A one-line summary of a peer, for `offload nodes`.
#[must_use]
pub fn summarize(view: &NodeView, now: Millis) -> NodeSummary {
    let kind = offload_core::AgentKind::ClaudeCode;
    let agent = view.capabilities.agent(&kind).map(|capability| {
        let version = capability
            .details
            .agent()
            .map_or("?", |details| details.version.as_str());
        format!(
            "claude-code {version}{}",
            if capability.authenticated {
                ""
            } else {
                " (no auth)"
            }
        )
    });
    NodeSummary {
        id: view.id,
        name: view.display_name(),
        status: view.status,
        device_class: format!("{:?}", view.capabilities.device_class),
        cpu_cores: view.capabilities.cpu_cores,
        memory_mb: view.capabilities.memory_mb,
        agent,
        running: u32::try_from(view.running.len()).unwrap_or(u32::MAX),
        // First-hand only: relayed news is not having heard from it (see `NodeView::heard_here`).
        last_heard_ms: view.heard_here.map(|at| now.saturating_sub(at).0),
        absences: view.absences,
        // The programs it would run as a task (ADR-0019): nominated and runnable, which is the
        // `ServiceAuthenticated` a task's constraint asks of a bidder. Not the agent, which is
        // an executor too and has its own column.
        may_host: None,
        // What its agent says it offers (ADR-0080), and only while it can use them: the list is
        // already empty for a logged-out agent, and this says so again rather than trusting it.
        models: view
            .capabilities
            .agent(&kind)
            .filter(|capability| capability.authenticated)
            .and_then(|capability| capability.details.agent())
            .map(|details| details.models.clone())
            .unwrap_or_default(),
        programs: {
            let mut programs: Vec<String> = view
                .capabilities
                .playing(offload_core::Role::Execute)
                .filter(|c| c.authenticated && c.details.agent().is_none())
                .map(|c| c.service.to_string())
                .collect();
            programs.sort();
            programs.dedup();
            programs
        },
    }
}

/// What `offload nodes` prints, decided here so the CLI does no interpreting.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NodeSummary {
    pub id: NodeId,
    pub name: String,
    pub status: offload_core::NodeStatus,
    pub device_class: String,
    pub cpu_cores: u32,
    pub memory_mb: u64,
    pub agent: Option<String>,
    pub running: u32,
    /// Since this node last heard from it first-hand; `None` if it never has.
    #[serde(default)]
    pub last_heard_ms: Option<u64>,
    pub absences: u32,
    /// The programs this node offers to run as tasks. Empty from an older daemon.
    #[serde(default)]
    pub programs: Vec<String>,
    /// Whether the fleet lets it host runs (`host-runs`), from the certificate it presented,
    /// which is what a bid is checked against. `None` when no certificate of its has been seen
    /// here: unknown, which is not the same as no.
    #[serde(default)]
    pub may_host: Option<bool>,
    /// The models its agent says it offers, in the agent's order (ADR-0080). Empty from an older
    /// daemon, and from one whose agent is logged out or could not be asked.
    #[serde(default)]
    pub models: Vec<offload_core::Model>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0069 §4's re-approval through a hardware key, through the daemon's own renewal entry:
    /// the first ask files a request and waits, a person's signature (a software key here, where
    /// the host would sign in the TEE) turns the next ask into a re-approval that starts a new
    /// year, and a refusal stays a refusal rather than prompting again.
    #[test]
    fn a_hardware_reapproval_is_filed_for_a_person_and_served_once_signed() {
        use offload_cluster::Members as _;
        use p256::ecdsa::signature::Signer as _;
        const DAY: u64 = 24 * 60 * 60 * 1_000;
        let dir = std::env::temp_dir().join(format!("offload-hw-reapprove-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");

        let key = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let signing = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let me = NodeId::from_bytes(signing.verifying_key().to_bytes());
        let mut scalar = [0u8; 32];
        scalar[0] = 1;
        scalar[31] = 5;
        let hw = p256::ecdsa::SigningKey::from_bytes(&scalar.into()).expect("hw key");
        let issuer_key = offload_core::fleet::IssuerKey::p256_from_sec1(
            hw.verifying_key().to_encoded_point(true).as_bytes(),
        )
        .expect("point");
        let t0 = Millis(1_700_000_000_000);
        let mut founder =
            crate::fleet::FleetState::found_with_key(&key, me, "tablet", t0, issuer_key);

        let host_signing = ed25519_dalek::SigningKey::from_bytes(&[2; 32]);
        let host = NodeId::from_bytes(host_signing.verifying_key().to_bytes());
        let approved = offload_core::MembershipCert::issue(
            key.signing_key(),
            offload_core::fleet::Issuer::Fleet,
            offload_core::fleet::Terms {
                probation: false,
                ..offload_core::fleet::Terms::joining(key.id(), host, "vps-17", t0)
            },
        );
        let day = |d: u64| Millis(t0.0 + d * DAY);
        let host_cert = offload_core::MembershipCert::issue(
            key.signing_key(),
            offload_core::fleet::Issuer::Fleet,
            approved.renewal(day(330), offload_core::fleet::CERT_LIFETIME),
        );
        founder.membership = offload_core::MembershipCert::issue(
            key.signing_key(),
            offload_core::fleet::Issuer::Fleet,
            founder
                .membership
                .renewal(day(330), offload_core::fleet::CERT_LIFETIME),
        );
        founder
            .decide_reapproval(me, host, "vps-17", day(336))
            .expect("a hardware approver records a decision");
        let membership = NodeMembership::new(founder, dir.join("fleet.json"), signing);

        let waiting = membership
            .renew(&host_cert, None, day(336))
            .expect_err("nobody has confirmed yet");
        assert!(waiting.contains("waits for a person"), "{waiting}");
        let request = dir
            .join("sign-requests")
            .join(format!("reapprove-{}.json", host.short()));
        let filed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&request).expect("filed")).expect("json");
        assert_eq!(filed["purpose"], "reapprove");
        // Asked again, still waiting, and still one request — not a second prompt.
        assert!(membership.renew(&host_cert, None, day(336)).is_err());
        assert_eq!(
            std::fs::read_dir(dir.join("sign-requests"))
                .expect("dir")
                .count(),
            1
        );

        // The host's half: the person confirmed, the key signed the certificate in the request.
        let unsigned: offload_core::MembershipCert =
            serde_json::from_value(filed["certificate"].clone()).expect("cert");
        let signature: p256::ecdsa::Signature = hw.sign(&unsigned.signing_bytes());
        std::fs::write(
            request.with_extension("sig"),
            hex::encode(signature.to_der().as_bytes()),
        )
        .expect("answer");

        let credentials = membership
            .renew(&host_cert, None, day(337))
            .expect("served once signed");
        assert_eq!(credentials.membership.approved_at(), day(336));
        // Half a day past the first approval's year, and inside the re-approval's month (day 336
        // to day 366): it verifies, where the approval it replaced no longer would.
        let past_the_year = Millis(day(365).0 + DAY / 2);
        assert_eq!(
            credentials
                .membership
                .verify(key.id(), credentials.delegation.as_ref(), past_the_year),
            Ok(())
        );

        // A refusal is kept: the member is told, and nobody is asked again.
        std::fs::remove_file(request.with_extension("sig")).expect("rm sig");
        std::fs::write(request.with_extension("refused"), "cancelled").expect("refuse");
        let refused = membership
            .renew(&host_cert, None, day(338))
            .expect_err("refused");
        assert!(
            refused.contains("was refused on tablet: cancelled"),
            "{refused}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
    use offload_core::fleet::FleetKey;

    use offload_core::{Arch, DeviceClass, NodeStatus, Os};

    const NOW: Millis = Millis(1_700_000_000_000);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "offload-membership-{tag}-{}-{:?}",
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

    fn a_node(seed: u8) -> NodeId {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        NodeId::from_bytes(signing.verifying_key().to_bytes())
    }

    /// A founded fleet on disk, and a live mirror of it — the arrangement the daemon has.
    fn founded(tag: &str) -> (Scratch, FleetKey, NodeId, Arc<NodeMembership>) {
        let scratch = Scratch::new(tag);
        let key = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let me = a_node(1);
        let state = FleetState::found(&key, me, "desktop", NOW);
        let path = crate::fleet::path(&scratch.0);
        crate::fleet::save_to(&path, &state).expect("save");
        let membership =
            NodeMembership::new(state, path, ed25519_dalek::SigningKey::from_bytes(&[1; 32]));
        (scratch, key, me, membership)
    }

    #[test]
    fn a_grant_typed_at_the_cli_reaches_a_running_daemon() {
        // The half of session eleven's "a grant needs no restart" that was not true: peers
        // re-ask a live connection for its grants, but this node kept presenting the
        // certificate it was holding when it started.
        let (scratch, key, me, membership) = founded("grant");
        let mut state = crate::fleet::require(&scratch.0).expect("load");
        state.membership.grants.remove(&Grant::HostRuns);
        crate::fleet::save_to(&crate::fleet::path(&scratch.0), &state).expect("save");
        membership.reload().expect("reload");
        assert!(!membership.snapshot().grants(NOW).contains(&Grant::HostRuns));
        let mut announced = membership.snapshot().grants(NOW);

        let mut state = crate::fleet::require(&scratch.0).expect("load");
        state
            .add_grant(&key, Grant::HostRuns, NOW, None)
            .expect("grant");
        crate::fleet::save_to(&crate::fleet::path(&scratch.0), &state).expect("save");

        let change = membership.reload().expect("reload");
        assert!(!membership.revoked_here());
        // A grant that was taken away and given back is re-probated (ADR-0012: probation follows
        // the grant), so it is in force at probation's end. This test used to pass at the same
        // instant only because `reload` read the wall clock, which is years past `NOW`.
        let lifted = NOW + offload_core::fleet::PROBATION;
        assert_eq!(membership.grants_changed(&mut announced, NOW), None);
        assert_eq!(
            membership
                .grants_changed(&mut announced, lifted)
                .map(|g| g.contains(&Grant::HostRuns)),
            Some(true)
        );
        assert_eq!(membership.grants_changed(&mut announced, lifted), None);
        assert!(change.revoked.is_empty());
        // And the credentials the transport presents are the new ones, which is the whole
        // point: a certificate nobody shows is a grant nobody honours.
        assert_eq!(membership.credentials().membership.member, me);
        assert!(membership
            .credentials()
            .membership
            .grants
            .contains(&Grant::HostRuns));
    }

    #[test]
    fn probation_lifting_is_a_change_in_what_the_node_may_do_though_the_file_is_unchanged() {
        // The edge `FleetChange::grants` used to be could not see this: the certificate loaded at
        // startup is the certificate fifteen minutes later, and only the clock moved. A daemon
        // that began hosting at probation's end printed `grants=submit,deliver` at startup and
        // nothing after it.
        let scratch = Scratch::new("probation");
        let key = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let me = a_node(2);
        let mut state = FleetState::join(&key, me, "phone", NOW);
        state
            .add_grant(&key, Grant::HostRuns, NOW, None)
            .expect("grant");
        let path = crate::fleet::path(&scratch.0);
        crate::fleet::save_to(&path, &state).expect("save");
        let membership =
            NodeMembership::new(state, path, ed25519_dalek::SigningKey::from_bytes(&[2; 32]));

        let mut announced = membership.snapshot().grants(NOW);
        assert!(!announced.contains(&Grant::HostRuns), "probating");
        assert_eq!(membership.grants_changed(&mut announced, NOW), None);
        assert!(
            membership.reload().expect("reload").is_empty(),
            "the file did not move"
        );

        let lifted = NOW + offload_core::fleet::PROBATION;
        assert_eq!(
            membership
                .grants_changed(&mut announced, lifted)
                .map(|g| g.contains(&Grant::HostRuns)),
            Some(true)
        );
        assert_eq!(
            membership.grants_changed(&mut announced, lifted),
            None,
            "said once"
        );
    }

    #[test]
    fn a_revocation_typed_at_the_cli_names_who_to_hang_up_on() {
        let (scratch, key, _me, membership) = founded("revoke");
        let stranger = a_node(9);

        let mut state = crate::fleet::require(&scratch.0).expect("load");
        state.revoke(&key, stranger, NOW).expect("revoke");
        crate::fleet::save_to(&crate::fleet::path(&scratch.0), &state).expect("save");

        let change = membership.reload().expect("reload");
        assert_eq!(change.revoked, vec![stranger]);
        assert!(!membership.revoked_here());
        // Reported once. A tick that re-announced every revocation it already knew would bury
        // the one that just happened.
        assert!(membership.reload().expect("reload").is_empty());
    }

    #[test]
    fn being_revoked_is_a_standing_condition_and_not_an_announcement() {
        let (scratch, key, me, membership) = founded("self");
        assert!(!membership.revoked_here());

        let mut state = crate::fleet::require(&scratch.0).expect("load");
        state.revoke(&key, me, NOW).expect("revoke");
        crate::fleet::save_to(&crate::fleet::path(&scratch.0), &state).expect("save");

        let change = membership.reload().expect("reload");
        assert_eq!(change.revoked, vec![me]);
        assert!(membership.revoked_here());
        // The half that used to be a `FleetChange` field, and the reason it could not work: a
        // second read reports nothing, because the change has already been seen. A caller that
        // polls — which is every caller here — has to be able to ask again and get the same
        // answer, or the one tick that missed it misses it for good.
        assert!(membership.reload().expect("reload").is_empty());
        assert!(membership.revoked_here());
    }

    #[test]
    fn filing_a_revocation_of_ourselves_does_not_consume_the_fact() {
        // The exact path that ate the old edge: `file` is what a revocation heard from a peer
        // arrives on (`Members::revoked`), and it reloads to refresh its own copy. That reload
        // is the one that saw the change, and it threw it away — so the daemon's own membership
        // tick, a second later, was told nothing had happened.
        let (scratch, key, me, membership) = founded("filed");
        let state = crate::fleet::require(&scratch.0).expect("load");
        let mut issuing = state.clone();
        issuing.revoke(&key, me, NOW).expect("revoke");
        let revocation = issuing.revocations.first().cloned().expect("one");

        assert!(membership.file(revocation).expect("file"));
        assert!(membership.revoked_here());
        assert!(membership.reload().expect("reload").is_empty());
        assert!(membership.revoked_here(), "still true a tick later");
    }

    #[test]
    fn a_revocation_heard_from_a_peer_is_written_down_without_undoing_a_local_grant() {
        // Two writers, one file, no lock: the CLI writes `fleet.json` with no daemon involved
        // (ADR-0012), so a daemon that saved its own in-memory snapshot would silently undo
        // whatever was typed a moment earlier.
        let (scratch, key, _me, membership) = founded("merge");
        let stranger = a_node(9);

        // Typed at the keyboard while the daemon holds an older copy.
        let mut typed = crate::fleet::require(&scratch.0).expect("load");
        typed.membership.name = "renamed-at-the-keyboard".into();
        crate::fleet::save_to(&crate::fleet::path(&scratch.0), &typed).expect("save");

        // Heard from a peer at the same moment.
        let revocation = offload_core::Revocation::issue(
            key.signing_key(),
            membership.snapshot().fleet,
            stranger,
            NOW,
            NOW.0,
        );
        assert!(membership.file(revocation).expect("file"));

        let on_disk = crate::fleet::require(&scratch.0).expect("load");
        assert!(on_disk.is_revoked(stranger), "the revocation was kept");
        assert_eq!(
            on_disk.membership.name, "renamed-at-the-keyboard",
            "and so was the edit the daemon never saw"
        );
    }

    #[test]
    fn a_revocation_already_known_is_not_written_again() {
        let (scratch, key, _me, membership) = founded("dup");
        let stranger = a_node(9);
        let revocation = offload_core::Revocation::issue(
            key.signing_key(),
            membership.snapshot().fleet,
            stranger,
            NOW,
            NOW.0,
        );
        assert!(membership.file(revocation.clone()).expect("file"));
        assert!(!membership.file(revocation).expect("file"));
        assert_eq!(
            crate::fleet::require(&scratch.0)
                .expect("load")
                .revocations
                .len(),
            1
        );
    }

    #[test]
    fn a_revocation_signed_by_somebody_else_is_refused() {
        // The fleet key is the only thing that can revoke. A peer that could would be a peer
        // that can evict any device it likes by asserting it.
        let (_scratch, _key, _me, membership) = founded("forged");
        let impostor = FleetKey::derive("zebra puppy abacus").expect("derive");
        let forged = offload_core::Revocation::issue(
            impostor.signing_key(),
            membership.snapshot().fleet,
            a_node(9),
            NOW,
            NOW.0,
        );
        assert!(membership.file(forged).is_err());
    }

    /// A seed may be a name, and it is resolved on every attempt.
    ///
    /// It used to be `seed.parse::<SocketAddr>()`, which refused a name outright — so dynamic DNS,
    /// the standard answer to a home address that changes, could not be configured at all
    /// (ADR-0037). Both forms now go through one path, and a literal costs no query.
    #[tokio::test]
    async fn a_seed_is_an_address_or_a_name() {
        assert_eq!(
            resolve("127.0.0.1:7433").await,
            vec!["127.0.0.1:7433".parse::<SocketAddr>().expect("literal")],
            "a literal resolves to itself"
        );
        let named = resolve("localhost:7433").await;
        assert!(
            named.iter().any(|a| a.ip().is_loopback()),
            "a name resolves: {named:?}"
        );
        assert_eq!(
            named.iter().map(SocketAddr::port).collect::<Vec<_>>(),
            vec![7433; named.len()],
            "and keeps the port it was given"
        );
        // A name that does not resolve is empty rather than fatal: a daemon that starts before
        // the network is up is ordinary, and dialling again is the whole point of the loop above.
        assert!(resolve("no-such-host.invalid:7433").await.is_empty());
    }

    /// What decides whether the seed list is dialled again.
    #[test]
    fn a_fleet_with_nobody_left_to_talk_to_says_so() {
        let me = NodeId::from_bytes([1; 32]);
        let mut fleet = offload_core::ClusterView::new(me);

        // Ourselves only: a fleet of one has nobody to lose contact with, and dialling a seed is
        // how it stops being a fleet of one.
        let mut mine = view();
        mine.id = me;
        fleet.nodes.insert(me, mine);
        assert!(
            !any_live_peer(&fleet, me),
            "we do not count as our own peer"
        );

        let peer = NodeId::from_bytes([2; 32]);
        fleet.nodes.insert(peer, view());
        assert!(any_live_peer(&fleet, me));

        // The case this exists for: the one peer we knew has an address that no longer works, so
        // it goes Alive, Suspect, Dead — and by the time the fleet has concluded it, re-dialling
        // the seed is exactly the right response.
        for gone in [NodeStatus::Dead, NodeStatus::Departed, NodeStatus::Draining] {
            if let Some(entry) = fleet.nodes.get_mut(&peer) {
                entry.status = gone;
            }
            assert!(!any_live_peer(&fleet, me), "{gone:?} is nobody to talk to");
        }
        // Suspect is not gone: the state exists to be argued with (ADR-0007), and a peer that
        // refutes costs us nothing.
        if let Some(entry) = fleet.nodes.get_mut(&peer) {
            entry.status = NodeStatus::Suspect;
        }
        assert!(any_live_peer(&fleet, me));
    }

    fn view() -> NodeView {
        NodeView::new(
            NodeId::from_bytes([2; 32]),
            Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Laptop),
            WorkPolicy::for_class(DeviceClass::Laptop),
            Millis(1_000),
        )
    }

    #[test]
    fn a_node_nobody_has_spoken_to_is_shown_by_its_id() {
        // Discovery gives an id before it gives a name, and printing an empty column would
        // read as a bug in the fleet rather than a node that has not been met.
        let summary = summarize(&view(), Millis(1_000));
        assert_eq!(summary.name, NodeId::from_bytes([2; 32]).short());
        assert_eq!(summary.status, NodeStatus::Alive);
        // …and never heard from, rather than heard from "0 ms ago" (session ninety-four).
        assert_eq!(summary.last_heard_ms, None);
    }

    #[test]
    fn only_a_node_with_room_counts_as_somewhere_else_to_send_a_late_run() {
        // What decides whether giving a commitment back achieves anything. A fleet full of
        // eligible-but-busy machines is not somewhere else to send the run — it is the same
        // queue with an extra bid round, an epoch and an attempt spent on getting there.
        let me = NodeId::from_bytes([1; 32]);
        let free = NodeId::from_bytes([2; 32]);
        let full = NodeId::from_bytes([3; 32]);

        let mut view = offload_core::ClusterView::new(me);
        for (id, class) in [
            (me, DeviceClass::Desktop),
            (free, DeviceClass::Desktop),
            // A two-run laptop, whose whole budget is what one heavy run spends.
            (full, DeviceClass::Laptop),
        ] {
            view.upsert_node(NodeView::new(
                id,
                Capabilities::empty(Os::Linux, Arch::X86_64, class),
                WorkPolicy::for_class(class),
                Millis(0),
            ));
        }

        let mut run = offload_core::Run::new(
            offload_core::RunId::from_bytes([7; 16]),
            spec(),
            me,
            Millis(0),
        );
        let none = std::collections::BTreeSet::new();
        assert_eq!(
            ready_elsewhere(&view, me, &run, &none, offload_core::Millis(0)),
            2,
            "both, and not myself"
        );

        // …and a peer the fleet has not permitted to host is not one of them, however much room
        // it has. It used to be: this estimate reads `capabilities` and `policy`, which is
        // ability and the owner's permission, and hosting needs the fleet's — a grant that lives
        // on a certificate rather than in the view. A member that joined and was never granted
        // `host-runs` therefore made every late commitment look re-placeable, and the round it
        // bought could only ever come back refused.
        let barred: std::collections::BTreeSet<_> = std::iter::once(free).collect();
        assert_eq!(
            ready_elsewhere(&view, me, &run, &barred, offload_core::Millis(0)),
            1,
            "a node that may not host cannot be somewhere this run could go"
        );

        // Fill one of them with a heavy run: its count still has room and its budget does not.
        let mut hog = offload_core::Run::new(
            offload_core::RunId::from_bytes([8; 16]),
            offload_core::RunSpec {
                demand: offload_core::Demand::Heavy,
                ..spec()
            },
            full,
            Millis(0),
        );
        hog.assign(full, Millis(0), offload_core::LEASE)
            .expect("assign");
        view.merge_run(hog, full);
        run.spec.demand = offload_core::Demand::Heavy;
        assert_eq!(
            ready_elsewhere(&view, me, &run, &none, offload_core::Millis(0)),
            1,
            "the laptop has a free slot by the count and none of its budget left"
        );
    }

    fn spec() -> offload_core::RunSpec {
        offload_core::RunSpec {
            work: offload_core::Work::Agent(offload_core::AgentWork {
                agent: offload_core::AgentKind::ClaudeCode,
                model: None,
                prompt: "p".into(),
                workspace: offload_core::WorkspaceSpec {
                    repo: "/r".into(),
                    archive_bytes: None,
                    git_ref: None,
                    branch: None,
                },
                permission_mode: offload_core::PermissionMode::AcceptEdits,
                allow: offload_core::ToolAllowlist::default(),
                max_turns: None,
                ask: offload_core::AskPolicy::Never,
            }),
            constraint: offload_core::Constraint::Always,
            restartability: offload_core::Restartability::Resumable,
            priority: 0,
            queue: false,
            deadline: None,
            demand: offload_core::Demand::Normal,
            notify: Default::default(),
            notices: Default::default(),
            resources: Vec::new(),
            prefer: offload_core::Constraint::Always,
            hold_until: None,
            parent: None,
        }
    }

    #[test]
    fn a_plugged_in_laptop_holds_replicas_whatever_its_battery_says() {
        // Found on real daemons: this machine's probe reports `battery 0%, charging`, so the
        // laptop policy's 20% floor refused every push and each checkpoint stayed on the one
        // node that made it. Nothing failed — the warning is a `WARN` at most — which is
        // exactly why it survived until a migration needed the copy.
        let policy = WorkPolicy::for_class(DeviceClass::Laptop);
        let charging = offload_core::PowerSource::Battery {
            percent: 0,
            charging: true,
        };
        assert!(accepts_replica(
            &policy,
            &charging,
            offload_core::Metered::No
        ));

        // On battery, the floor is the whole point and still applies.
        let flat = offload_core::PowerSource::Battery {
            percent: 3,
            charging: false,
        };
        assert!(!accepts_replica(&policy, &flat, offload_core::Metered::No));
        assert!(accepts_replica(
            &policy,
            &offload_core::PowerSource::Ac,
            offload_core::Metered::No
        ));

        // Metered is about the bytes, not the charge: refused even on mains.
        assert!(!accepts_replica(
            &policy,
            &offload_core::PowerSource::Ac,
            offload_core::Metered::Yes
        ));

        // And a link nobody could ask about is not a link known to cost money, so the replica is
        // held (ADR-0045 §4). Refusing here would be this test's own finding one field over —
        // every checkpoint staying `here only` on a machine with nothing wrong with it, for a
        // reason no report would name.
        assert!(accepts_replica(
            &policy,
            &offload_core::PowerSource::Ac,
            offload_core::Metered::Unknown
        ));
    }

    #[test]
    fn silence_is_reported_as_a_duration_rather_than_a_timestamp() {
        // "last heard 4s ago" is the question being asked; a unix millisecond is not.
        let mut heard = view().named("laptop");
        heard.heard_here = Some(Millis(1_000));
        let summary = summarize(&heard, Millis(5_000));
        assert_eq!(summary.name, "laptop");
        assert_eq!(summary.last_heard_ms, Some(4_000));
    }

    #[tokio::test]
    async fn a_drained_node_bids_on_nothing_and_takes_nothing() {
        // `drain`'s doc comment has said "and stop accepting new ones" since it was written, and
        // nothing implemented it — so `offload drain` on an idle laptop did nothing at all, and
        // the node went on winning rounds. Measured: a submission one second later was accepted
        // by the machine that had just been told to stop, which could send the run it had handed
        // to the desktop straight back.
        let scratch = Scratch::new("drain");
        let store = offload_store::Store::open_memory().expect("store");
        let config = Config {
            state_dir: scratch.0.clone(),
            ..Config::default()
        };
        let supervisor =
            crate::Supervisor::new(Arc::new(config), NodeId::from_bytes([1; 32]), store)
                .with_private_ledger();
        let host = NodeHost {
            supervisor: supervisor.clone(),
            state_dir: scratch.0.clone(),
            // Deliberately dangling: what is under test is that a drained node answers before it
            // consults anything, which is the ordering the refusal depends on being cheap.
            cluster: std::sync::Weak::new(),
            weights: offload_core::BidWeights::default(),
        };
        let run = a_run();

        // Before the drain the answer is about this machine — a dangling cluster, here — and
        // decidedly not "draining".
        let before = offload_cluster::Host::evaluate(&host, &run)
            .await
            .expect_err("no cluster to bid into");
        assert!(!before.contains("draining"), "{before}");

        supervisor.stop_accepting();
        assert_eq!(
            offload_cluster::Host::evaluate(&host, &run)
                .await
                .expect_err("a drained node bids on nothing"),
            "draining"
        );
        assert_eq!(
            offload_cluster::Host::accept(&host, run)
                .await
                .expect_err("and takes nothing, including a grant already in flight"),
            "draining"
        );
    }

    /// The next step a drain said, or a failure rather than a hang.
    ///
    /// `recv().await` with the sender still alive waits for ever, so a step that is never sent
    /// would make the guard against it hang instead of going red — which is the one failure mode
    /// a test must not have.
    async fn said(
        heard: &mut tokio::sync::mpsc::Receiver<crate::api::DrainStep>,
    ) -> crate::api::DrainStep {
        tokio::time::timeout(Duration::from_secs(5), heard.recv())
            .await
            .expect("a drain has to say what it is doing")
            .expect("the reporting channel is still open")
    }

    /// A supervisor with its own store, for the tests about what a drain reads.
    fn a_supervisor(scratch: &Scratch, store: offload_store::Store) -> crate::Supervisor {
        let config = Config {
            state_dir: scratch.0.clone(),
            ..Config::default()
        };
        crate::Supervisor::new(Arc::new(config), NodeId::from_bytes([1; 32]), store)
            .with_private_ledger()
    }

    /// A run this node holds and has started, saved where the drain will read it.
    fn a_started_run(
        store: &offload_store::Store,
        ask: offload_core::AskPolicy,
    ) -> offload_core::Run {
        let mut run = a_run();
        run.spec.agent_mut().expect("an agent run").ask = ask;
        let node = NodeId::from_bytes([1; 32]);
        let now = SystemClock.now();
        let epoch = run
            .assign(node, now, offload_core::LEASE)
            .expect("assign to ourselves");
        run.started(node, epoch, now).expect("start");
        store.save_run(&run).expect("save");
        run
    }

    #[test]
    fn a_release_says_who_let_the_run_go() {
        // What replaced the drain's in-memory debt, and the reason the two tests that were here
        // are gone with it (ADR-0042). The debt existed because `Pending` plus a checkpoint is
        // two facts with one spelling: a run a person parked with `offload checkpoint`, which
        // waits for them, and a run a node let go on its way out, which the fleet should carry
        // on with. `offload_core::supervise` read the second as the first and passed it by, so a
        // departing node had to remember its own unfinished business — node-local, in memory,
        // and therefore invisible to the run's arbiter and gone at the next restart.
        //
        // Measured before, on two daemons: `offload drain` with the peer down said `nothing
        // could be handed over; 1 run(s) still here`, `offload explain` described the run as
        // *"parked with its checkpoint, waits for `offload resume`"* — about a run nobody had
        // parked — and both daemons restarted idle, accepting, holding the checkpoint, with the
        // run still `pending` for as long as anybody watched.
        //
        // The release states it now, so this is the seam worth pinning here: the same capture,
        // at the same boundary, writing the same blobs, leaves two different runs behind.
        let store = offload_store::Store::open_memory().expect("store");
        let node = NodeId::from_bytes([1; 32]);
        let capture = || offload_core::Checkpoint {
            session_id: Some("11111111-2222-3333-4444-555555555555".into()),
            transcript: offload_core::BlobHash::from_bytes([3; 32]),
            bundle: None,
            patch: None,
            base_commit: "0f0f0f0f".into(),
            turns: 3,
            taken_at: SystemClock.now(),
            agent_version: "2.1.238".into(),
            replicas: std::collections::BTreeSet::new(),
        };

        let mut parked = a_started_run(&store, offload_core::AskPolicy::Never);
        let epoch = parked.epoch;
        parked
            .checkpointed(
                node,
                epoch,
                capture(),
                offload_core::GivenUp::Parked,
                SystemClock.now(),
            )
            .expect("parked");
        assert_eq!(
            parked.let_go_by(),
            None,
            "a person asked, so the run waits for them"
        );

        let mut let_go = a_started_run(&store, offload_core::AskPolicy::Never);
        let epoch = let_go.epoch;
        let_go
            .checkpointed(
                node,
                epoch,
                capture(),
                offload_core::GivenUp::LetGo,
                SystemClock.now(),
            )
            .expect("let go");
        assert_eq!(
            let_go.let_go_by(),
            Some(node),
            "the drain's own request leaves a run the fleet is entitled to place"
        );

        // And the commitment path, which has only ever been a node changing its mind: there is
        // no operator command that hands one back, so it does not take a parameter. It used to
        // strand a run too, one branch over and for the same reason.
        let mut committed = a_run();
        let epoch = committed
            .assign(node, SystemClock.now(), offload_core::LEASE)
            .expect("assign");
        committed
            .release(node, epoch, SystemClock.now())
            .expect("release");
        assert_eq!(committed.let_go_by(), Some(node));
    }

    #[tokio::test]
    async fn a_departure_is_recorded_even_where_there_is_nothing_to_hand_over() {
        // ADR-0034's fix put `stop_accepting` in the operator's *handler*, which fixed the fleet
        // of one and silently broke the other caller: a `SIGTERM`ed daemon went on bidding and
        // starting work for the whole of its shutdown drain. Measured — `accepting yes` three
        // seconds after the signal, and `spawning claude code` for a run submitted after the
        // daemon had been told to stop. `depart` is the one way in, which is what makes the two
        // paths agree; `Mesh::drain` is private so nothing can take the other one.
        let scratch = Scratch::new("depart");
        let store = offload_store::Store::open_memory().expect("store");
        let sup = a_supervisor(&scratch, store.clone());
        assert!(!sup.is_draining(), "nothing has asked it to leave yet");

        let (steps, mut heard) = tokio::sync::mpsc::channel(4);
        let drained = depart(
            &sup,
            None,
            Duration::from_secs(1),
            Millis(50),
            Lifespan::StaysUp,
            &Report::to(steps),
        )
        .await;

        assert!(sup.is_draining(), "a fleet of one is a departure too");
        assert_eq!(drained, Drained::default(), "nothing was held");
        // And it is said *first*, before anything is attempted: on a fleet of one it is the whole
        // report, and it was being printed after a pass that can take five minutes.
        assert_eq!(
            said(&mut heard).await,
            crate::api::DrainStep::StoppedAccepting
        );
    }

    #[tokio::test]
    async fn the_four_ways_a_wait_for_a_turn_boundary_ends_are_four_answers() {
        // The finding: this answered `None` for three of them and the caller reported one, so a
        // run that *finished* while the drain waited — "the nicest possible outcome", in the
        // code's own words — was counted as a run left behind and logged as `still mid-turn at
        // the drain deadline`. Measured twice in one walk, once on the shutdown path.
        let scratch = Scratch::new("boundary");
        let store = offload_store::Store::open_memory().expect("store");
        let sup = a_supervisor(&scratch, store.clone());
        let report = Report::silent();
        let mut wait = Wait {
            supervisor: &sup,
            report: &report,
            runs: Vec::new(),
            said: std::collections::BTreeSet::new(),
        };
        let plenty = Instant::now() + Duration::from_secs(60);
        let gone_by = Instant::now();

        // Nothing there at all.
        assert!(matches!(
            wait.boundary(offload_core::RunId::from_bytes([9; 16]), plenty)
                .await,
            Boundary::Gone
        ));

        let mut run = a_started_run(&store, offload_core::AskPolicy::Never);
        // Mid-turn, and the deadline has passed: left with its lease (ADR-0004).
        assert!(matches!(
            wait.boundary(run.id, gone_by).await,
            Boundary::StillMidTurn
        ));

        // It ended by itself. Neither moved nor left behind.
        let node = NodeId::from_bytes([1; 32]);
        run.complete(node, run.epoch, SystemClock.now())
            .expect("complete");
        store.save_run(&run).expect("save");
        assert!(matches!(
            wait.boundary(run.id, plenty).await,
            Boundary::Finished
        ));

        // And the one the drain is actually waiting for: checkpointed and handed back.
        run.state = offload_core::RunState::Pending {
            since: SystemClock.now(),
            let_go_by: Some(NodeId::from_bytes([1; 32])),
        };
        run.checkpoint = Some(offload_core::Checkpoint {
            session_id: Some("11111111-2222-3333-4444-555555555555".into()),
            transcript: offload_core::BlobHash::from_bytes([3; 32]),
            bundle: None,
            patch: None,
            base_commit: "0f0f0f0f".into(),
            turns: 3,
            taken_at: SystemClock.now(),
            agent_version: "2.1.238".into(),
            replicas: std::collections::BTreeSet::new(),
        });
        store.save_run(&run).expect("save");
        assert!(matches!(
            wait.boundary(run.id, plenty).await,
            Boundary::Reached(_)
        ));
    }

    #[tokio::test]
    async fn a_drain_says_what_the_run_it_is_waiting_on_is_blocked_on() {
        // ADR-0035. Both clocks are five minutes, so a run stopped mid-tool-call on `--ask`
        // holds the drain for the whole window — and the one person who could end it instantly
        // is the person who typed `offload drain` and saw nothing at all. Measured: forty
        // seconds of blank terminal, with `offload asks` in the next window naming the question.
        let scratch = Scratch::new("blocked");
        let store = offload_store::Store::open_memory().expect("store");
        let sup = a_supervisor(&scratch, store.clone());
        let run = a_started_run(&store, offload_core::AskPolicy::UpTo { questions: 5 });
        let _waiter = sup
            .ask(run.id, run.epoch, "toolu_1", "Bash", "rm -rf /", true)
            .expect("a run that asked, with somebody to ask");

        let (steps, mut heard) = tokio::sync::mpsc::channel(4);
        let report = Report::to(steps);
        let mut wait = Wait {
            supervisor: &sup,
            report: &report,
            runs: vec![run.id],
            said: std::collections::BTreeSet::new(),
        };
        // A drain with a long way to run, so the question is the nearer of the two clocks —
        // the case the sentence below has always been right about.
        let until = Instant::now() + Duration::from_secs(600);
        wait.questions(until);
        let crate::api::DrainStep::Blocked {
            run: named,
            tool,
            tool_use_id,
            within_ms,
            drain_ends_in_ms,
            ..
        } = said(&mut heard).await
        else {
            panic!("the drain has to say what it is waiting behind");
        };
        // The **whole** id. The CLI prints this into an `offload approve` line, which is an
        // instruction rather than a column: twelve characters of a scheduled occurrence's id
        // name the tick and not the run, and the command then refuses the line it was handed.
        assert_eq!(named, run.id.to_string());
        assert_eq!(tool, "Bash");
        // The `tool_use_id` is what makes the line actionable rather than merely informative:
        // `offload approve` addresses one tool call.
        assert_eq!(tool_use_id, "toolu_1");
        assert!(
            within_ms > 0,
            "a question with no clock is the promise this plane cannot keep"
        );

        // **Both clocks, and this one is the nearer.** Reporting only the question's patience is
        // what ADR-0035 left as a residual: the two deadlines end this wait differently — the
        // question expiring decides it and the run is handed over normally, the drain expiring
        // first leaves the run mid-turn with the question still open — so the smaller is the one
        // an operator is racing.
        assert!(
            drain_ends_in_ms > within_ms,
            "the staging: a ten-minute drain against a five-minute question"
        );

        // Said once. This is polled four times a second for up to five minutes.
        wait.questions(until);
        assert!(heard.try_recv().is_err(), "one question, one sentence");
    }

    fn a_run() -> offload_core::Run {
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
                demand: offload_core::Demand::Normal,
                notify: offload_core::Audience::default(),
                notices: Default::default(),
                resources: Vec::new(),
                prefer: offload_core::Constraint::Always,
                hold_until: None,
                parent: None,
            },
            NodeId::from_bytes([1; 32]),
            NOW,
        )
    }
}
