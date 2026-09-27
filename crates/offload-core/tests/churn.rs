//! Several nodes, each with its own picture of the fleet, driven by churn nobody arranged.
//!
//! `tests/properties.rs` drives one `Run` and one `ClusterView`. This drives a *fleet*: N nodes
//! that each hold their own view, gossip to each other, run their own supervision pass, and
//! crash and come back — which is where the interesting failures live, because every one of
//! them is a disagreement rather than a bug in a function. It is the other half of the phase-0
//! roadmap item and the first half of phase 6's "no run lost under churn".
//!
//! **What makes this possible is that none of it needs a network.** `supervise` is a pure
//! function of `(view, run, now)` and `merge_run` is a pure function of the two records and who
//! spoke, so a partition is a fact about which deliveries this loop performs rather than
//! something to provoke with a firewall. ADR-0001's arrangement, collected on.
//!
//! **The honest limits, written down before the properties so they are not mistaken for
//! oversights.**
//!
//! * *Merging is not commutative, by design.* At equal epoch the rules ask **who is speaking** —
//!   only the holder may refute an orphan, only the arbiter may declare one — so delivering the
//!   holder's record and then the arbiter's lands somewhere different from the reverse. Both are
//!   correct: one is a reclaim and the other is a decision. So the properties below are about
//!   what the fleet *agrees on* once the gossip stops, never about the answer being predictable
//!   from the set of records alone.
//! * *A partition that persists is not survivable, and that is not a bug here.* Two arbitrating
//!   sides that cannot hear each other will grant the run twice, and no fencing changes that
//!   without quorum (ADR-0002, and its 2026-08-21 amendment). What fencing buys is that the
//!   moment the records meet, exactly one grant survives — which is the property this file
//!   checks, at the fixpoint rather than at every instant.
//! * *No agents.* A node "holding" a run means its own view says so. That is the right level for
//!   a crate with no I/O in it, and it is the input every real decision is made from.

// Same exemption, same reason as `no_clock.rs` and `properties.rs`: panicking is how a property
// reports a counterexample, and this file is nothing but properties.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use offload_core::capability::{Arch, Capabilities, DeviceClass, Os};
use offload_core::constraint::Constraint;
use offload_core::id::{NodeId, RunId};
use offload_core::policy::{supervise, ReassignDecision, ReassignPolicy, Supervision, WorkPolicy};
use offload_core::run::{
    PermissionMode, Restartability, Run, RunSpec, RunState, WorkspaceSpec, LEASE,
};
use offload_core::time::Millis;
use offload_core::view::{ClusterView, NodeStatus, NodeView};
use offload_core::AskPolicy;
use offload_core::{AgentWork, Work};
use proptest::prelude::*;
use std::collections::BTreeMap;

const RUN: RunId = RunId::from_bytes([7; 16]);

fn node(b: u8) -> NodeId {
    NodeId::from_bytes([b; 32])
}

fn spec() -> RunSpec {
    RunSpec {
        work: Work::Agent(AgentWork {
            agent: offload_core::capability::AgentKind::ClaudeCode,
            model: None,
            prompt: "p".into(),
            workspace: WorkspaceSpec {
                repo: "https://example.invalid/r.git".into(),
                archive_bytes: None,
                git_ref: None,
                branch: None,
            },
            permission_mode: PermissionMode::AcceptEdits,
            allow: Default::default(),
            max_turns: None,
            ask: AskPolicy::Never,
        }),
        // `Always`, so every alive node is an eligible alternative. Eligibility is
        // `properties.rs`'s subject; this file is about who ends up believing what.
        constraint: Constraint::Always,
        restartability: Restartability::Resumable,
        priority: 0,
        queue: true,
        deadline: None,
        demand: offload_core::capacity::Demand::Normal,
        notify: Default::default(),
        notices: Default::default(),
        resources: Vec::new(),
        prefer: offload_core::Constraint::Always,
        hold_until: None,
        parent: None,
    }
}

struct SimNode {
    view: ClusterView,
    /// Whether the daemon is running. A node that is down neither gossips nor supervises, and
    /// its peers do not know that until something notices.
    up: bool,
}

/// A fleet, its members' separate opinions, and the clock they all read from an argument.
struct Sim {
    now: Millis,
    members: Vec<NodeId>,
    nodes: BTreeMap<NodeId, SimNode>,
    policy: ReassignPolicy,
    /// The highest epoch each node has ever had for the run. Nothing may walk this back.
    watermark: BTreeMap<NodeId, offload_core::run::Epoch>,
}

impl Sim {
    /// Every member knows every member, and the run is already running on `members[1]` —
    /// something for the churn to threaten, rather than a `Pending` run with nothing at stake.
    fn new(members: &[u8]) -> Sim {
        let members: Vec<NodeId> = members.iter().map(|b| node(*b)).collect();
        let home = members[0];
        let holder = members[1 % members.len()];

        let mut run = Run::new(RUN, spec(), home, Millis(0));
        let epoch = run.assign(holder, Millis(0), LEASE).expect("assign");
        run.started(holder, epoch, Millis(0)).expect("start");

        let mut nodes = BTreeMap::new();
        for id in &members {
            let mut view = ClusterView::new(*id);
            for other in &members {
                view.upsert_node(NodeView::new(
                    *other,
                    Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Desktop),
                    WorkPolicy::for_class(DeviceClass::Desktop),
                    Millis(0),
                ));
            }
            view.runs.insert(RUN, run.clone());
            nodes.insert(*id, SimNode { view, up: true });
        }
        let watermark = members.iter().map(|id| (*id, run.epoch)).collect();
        Sim {
            now: Millis(0),
            members,
            nodes,
            policy: ReassignPolicy::default(),
            watermark,
        }
    }

    fn who(&self, i: u8) -> NodeId {
        self.members[usize::from(i) % self.members.len()]
    }

    fn record(&self, id: NodeId) -> Run {
        self.nodes[&id].view.runs[&RUN].clone()
    }

    fn save(&mut self, id: NodeId, run: Run) {
        self.nodes
            .get_mut(&id)
            .expect("member")
            .view
            .runs
            .insert(RUN, run);
    }

    /// Up nodes whose own record says they are holding the run right now.
    ///
    /// The operative definition of "an agent is running here": a node starts one because its own
    /// view told it to, so this is exactly the set the double-execution question is asked about.
    fn holders(&self) -> Vec<NodeId> {
        self.members
            .iter()
            .copied()
            .filter(|id| self.nodes[id].up)
            .filter(|id| {
                self.record(*id)
                    .state
                    .lease()
                    .is_some_and(|l| l.node == *id)
            })
            .collect()
    }

    fn tick(&mut self, secs: u64) {
        self.now = self.now + Millis::from_secs(secs.clamp(1, 120));
    }

    fn crash(&mut self, id: NodeId) {
        self.nodes.get_mut(&id).expect("member").up = false;
    }

    fn restart(&mut self, id: NodeId) {
        self.nodes.get_mut(&id).expect("member").up = true;
    }

    /// The failure detector, as an event rather than as a component.
    ///
    /// A node that is down is escalated `Alive` → `Suspect` → `Dead` one notice at a time,
    /// because ADR-0007's whole point is that those are two different facts. A node that is
    /// actually *up* can be suspected too — that is the wrong guess the design has to tolerate,
    /// and it is how two arbiters come to exist without any partition at all.
    fn notice(&mut self, observer: NodeId, subject: NodeId) {
        if observer == subject || !self.nodes[&observer].up {
            return;
        }
        let now = self.now;
        let Some(entry) = self
            .nodes
            .get_mut(&observer)
            .expect("member")
            .view
            .nodes
            .get_mut(&subject)
        else {
            return;
        };
        let next = match entry.status {
            NodeStatus::Alive => NodeStatus::Suspect,
            _ => NodeStatus::Dead,
        };
        entry.set_status(next, now);
    }

    /// One gossip delivery: `from`'s whole view to `to`, which is what a probe carries.
    fn gossip(&mut self, from: NodeId, to: NodeId) {
        if from == to || !self.nodes[&from].up || !self.nodes[&to].up {
            return;
        }
        let now = self.now;
        let peers: Vec<NodeView> = self.nodes[&from].view.nodes.values().cloned().collect();
        let record = self.record(from);

        // A node that hears a suspicion about *itself* refutes it, which is the one direction a
        // node owns its own liveness in (ADR-0005). Without it a wrong guess is permanent and the
        // sim would only ever model correct detection.
        //
        // **Done by `merge_node` now, and not here.** This block used to do it by hand, which
        // made the sim model a mechanism the daemon did not have: `refute` had no production
        // caller for two phases, so these properties passed while the real fleet could not
        // correct a peer that was wrong about it. Removing it is what proves the fix — the same
        // reason the sim honours `arbiter_for` rather than letting any node place a run.

        let view = &mut self.nodes.get_mut(&to).expect("member").view;
        for peer in peers {
            view.merge_node(peer, now);
        }
        view.merge_run(record, from);
    }

    /// One node's supervision pass, and the effects it is entitled to.
    ///
    /// The same three outcomes the daemon acts on, and nothing else: `supervise` is where the
    /// deciding happens, and a sim that second-guessed it would be testing itself.
    fn supervise(&mut self, id: NodeId) {
        if !self.nodes[&id].up {
            return;
        }
        let run = self.record(id);
        let now = self.now;
        let decision = supervise(&self.nodes[&id].view, &run, now, &self.policy);
        match decision {
            Supervision::Orphan => {
                let mut run = run;
                if run.orphan(now).is_ok() {
                    self.save(id, run);
                }
            }
            Supervision::Place(_) | Supervision::Decided(ReassignDecision::Reassign { .. }) => {
                self.grant(id);
            }
            Supervision::Decided(ReassignDecision::Abandon { reason }) => {
                let mut run = run;
                if run.abandon(reason, now).is_ok() {
                    self.save(id, run);
                }
            }
            Supervision::Bystander(_) | Supervision::Decided(_) => {}
        }
    }

    /// A bid round, as `Cluster::place` runs one: offer to each candidate in turn, and spend a
    /// fencing token on every offer whether or not it is confirmed.
    ///
    /// Candidates are the alive nodes in *this arbiter's* view — which is the whole point, since
    /// two arbiters with different views are two different rounds. A candidate that is actually
    /// down does not answer, and ADR-0006 says silence is a decline: the token goes back at a
    /// number the unreachable node can no longer act under, and the next candidate gets a fresh
    /// one. A round that finds nobody leaves the arbiter's own record alone, because `place`
    /// works on a clone and only a success is published.
    fn grant(&mut self, arbiter: NodeId) {
        let mut offered = self.record(arbiter);
        let holder = offered.holder();
        let candidates: Vec<NodeId> = self.nodes[&arbiter]
            .view
            .alive()
            .map(|n| n.id)
            .filter(|id| Some(*id) != holder)
            .collect();

        for target in candidates {
            let now = self.now;
            let Ok(epoch) = offered.assign(target, now, LEASE) else {
                return;
            };
            if self.nodes[&target].up {
                self.save(target, offered.clone());
                self.save(arbiter, offered);
                return;
            }
            let _ = offered.release(target, epoch, now);
        }
    }

    /// The holder gets on with it: start if granted, finish if running.
    fn progress(&mut self, id: NodeId, breaks: bool) {
        if !self.nodes[&id].up {
            return;
        }
        let mut run = self.record(id);
        let now = self.now;
        let epoch = run.epoch;
        let ok = match run.state {
            RunState::Assigned { lease } if lease.node == id => run.started(id, epoch, now).is_ok(),
            RunState::Running { lease, .. } if lease.node == id => {
                if breaks {
                    run.fail(id, epoch, "the agent stopped", now).is_ok()
                } else {
                    run.complete(id, epoch, now).is_ok()
                }
            }
            _ => false,
        };
        if ok {
            self.save(id, run);
        }
    }

    /// Deliver gossip between every pair of up nodes until nothing changes.
    ///
    /// Nothing here supervises, on purpose: the arbiter deciding again is a *new* fact, and a
    /// fleet where the arbiter keeps concluding the holder is gone while the holder keeps
    /// refuting is correctly unsettled. What is being asked is narrower and is the thing a
    /// distributed system owes: with no new facts, does everybody end up in the same place.
    ///
    /// Returns false if it never settled, which is a finding in itself — two records that each
    /// refuse the other would sit at no fixpoint at all.
    ///
    /// The fixpoint is over the **whole view**, not over the run record, and the difference is
    /// not bookkeeping. `merge_run`'s equal-epoch rules key on *entitlement* — who is the holder,
    /// who is the arbiter — and `arbiter_for` derives entitlement from liveness, which is the
    /// other half of the view. So two nodes that disagree about whether the home node is dead
    /// disagree about who is entitled, and each rejects the other's record: a genuine standoff
    /// that lasts exactly as long as the liveness disagreement does. Stopping as soon as the run
    /// record stopped moving reports that standoff as settled, which is how this file first
    /// "found" a divergence that was its own impatience.
    fn settle(&mut self) -> bool {
        self.settle_in(false)
    }

    /// What each up node makes of the run: the two fields that are *decisions*.
    ///
    /// Deliberately not the state name. Whether a holder is `orphaned` or still `running` is an
    /// observation, and the entitlement rules make an observation order-dependent on purpose.
    fn answers(&self) -> Vec<(NodeId, Option<NodeId>, offload_core::run::Epoch)> {
        self.members
            .iter()
            .copied()
            .filter(|id| self.nodes[id].up)
            .map(|id| {
                let run = self.record(id);
                (id, run.holder(), run.epoch)
            })
            .collect()
    }

    /// Every node's view, to put back afterwards — the two delivery orders have to start level.
    fn snapshot(&self) -> Vec<(NodeId, ClusterView)> {
        self.members
            .iter()
            .map(|id| (*id, self.nodes[id].view.clone()))
            .collect()
    }

    fn restore(&mut self, snap: &[(NodeId, ClusterView)]) {
        for (id, view) in snap {
            self.nodes.get_mut(id).expect("member").view = view.clone();
        }
    }

    fn settle_in(&mut self, reverse: bool) -> bool {
        let mut up: Vec<NodeId> = self
            .members
            .iter()
            .copied()
            .filter(|id| self.nodes[id].up)
            .collect();
        if reverse {
            up.reverse();
        }
        for _ in 0..64 {
            let before: Vec<ClusterView> =
                up.iter().map(|id| self.nodes[id].view.clone()).collect();
            for from in &up {
                for to in &up {
                    self.gossip(*from, *to);
                }
            }
            let after: Vec<ClusterView> = up.iter().map(|id| self.nodes[id].view.clone()).collect();
            if before == after {
                return true;
            }
        }
        false
    }

    /// What must hold after every single event, whatever the fleet is in the middle of.
    ///
    /// Two things, and they are the ones no in-flight message can excuse. Everything about
    /// *agreement* waits for a fixpoint, because a node cannot be blamed for not yet having
    /// heard; these are about what a node may believe on its own.
    fn check_invariants(&mut self) -> Result<(), TestCaseError> {
        for id in self.members.clone() {
            let run = self.record(id);

            // The fencing token, per node. A view that walked one back would hand a node that
            // had been fenced out its authority again.
            let epoch = run.epoch;
            let mark = self.watermark.entry(id).or_insert(epoch);
            prop_assert!(
                epoch >= *mark,
                "{} walked back from {} to {}",
                id.short(),
                *mark,
                epoch
            );
            *mark = epoch;

            // And nobody who is up believes *it* is the holder that has gone quiet. A node that
            // can be asked is present, by construction, so this is a claim it is uniquely
            // placed to refuse — and the cost of accepting it is everything: `Orphaned` returns
            // no lease, so the node cannot renew and cannot report its own agent finishing.
            //
            // The one real-world case this would wrongly catch is not modelled here: a node
            // that *restarted* and learned the record fresh is correctly told it is the orphaned
            // holder, because its agent really is gone. A restart in this sim keeps the node's
            // store, which is what a real one does too — so within these runs the situation can
            // only arise by believing a peer.
            if self.nodes[&id].up {
                if let RunState::Orphaned { last, .. } = run.state {
                    prop_assert!(
                        last.node != id,
                        "{} is up and believes it is its own run's absent holder",
                        id.short()
                    );
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum Ev {
    Tick(u64),
    Gossip(u8, u8),
    Supervise(u8),
    Progress(u8, bool),
    Crash(u8),
    Restart(u8),
    Notice(u8, u8),
}

fn ev() -> impl Strategy<Value = Ev> {
    prop_oneof![
        3 => (1u64..90).prop_map(Ev::Tick),
        6 => (0u8..5, 0u8..5).prop_map(|(a, b)| Ev::Gossip(a, b)),
        6 => (0u8..5).prop_map(Ev::Supervise),
        3 => (0u8..5, any::<bool>()).prop_map(|(n, b)| Ev::Progress(n, b)),
        2 => (0u8..5).prop_map(Ev::Crash),
        2 => (0u8..5).prop_map(Ev::Restart),
        4 => (0u8..5, 0u8..5).prop_map(|(a, b)| Ev::Notice(a, b)),
    ]
}

fn run_sim(members: &[u8], events: &[Ev]) -> Result<Sim, TestCaseError> {
    let mut sim = Sim::new(members);
    for ev in events {
        match *ev {
            Ev::Tick(s) => sim.tick(s),
            Ev::Gossip(a, b) => {
                let (a, b) = (sim.who(a), sim.who(b));
                sim.gossip(a, b);
            }
            Ev::Supervise(n) => {
                let n = sim.who(n);
                sim.supervise(n);
            }
            Ev::Progress(n, breaks) => {
                let n = sim.who(n);
                sim.progress(n, breaks);
            }
            Ev::Crash(n) => {
                let n = sim.who(n);
                sim.crash(n);
            }
            Ev::Restart(n) => {
                let n = sim.who(n);
                sim.restart(n);
            }
            Ev::Notice(a, b) => {
                let (a, b) = (sim.who(a), sim.who(b));
                sim.notice(a, b);
            }
        }
        sim.check_invariants()?;
    }
    Ok(sim)
}

/// At least three distinct members, built rather than filtered: the liveness property below
/// needs a fleet that still has somewhere to put the run after its holder dies, and rejecting
/// until the generator happens to produce one throws most of the search away.
fn a_fleet_of_three() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(1u8..6, 3..6).prop_map(|v| {
        let mut out: Vec<u8> = Vec::new();
        for b in v {
            if !out.contains(&b) {
                out.push(b);
            }
        }
        let mut spare = 1u8;
        while out.len() < 3 {
            if !out.contains(&spare) {
                out.push(spare);
            }
            spare += 1;
        }
        out
    })
}

fn members() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(1u8..6, 2..5).prop_filter_map("distinct members", |v| {
        let mut out: Vec<u8> = Vec::new();
        for b in v {
            if !out.contains(&b) {
                out.push(b);
            }
        }
        (out.len() >= 2).then_some(out)
    })
}

proptest! {
    /// When the gossip stops, one node holds the run — never two.
    ///
    /// The whole of what epoch fencing is for, asked of a fleet instead of of a function. Two
    /// nodes can each believe they hold it *while a message is still in flight*, which is
    /// unavoidable and is why this is asked at the fixpoint; what must not survive contact is
    /// two grants at one epoch, which is what a wrongly-suspected arbiter produces and what
    /// `merge_run` had no way to order until it was given one.
    #[test]
    fn when_the_gossip_stops_at_most_one_node_thinks_it_holds_the_run(
        members in members(),
        events in prop::collection::vec(ev(), 0..40),
    ) {
        let mut sim = run_sim(&members, &events)?;
        prop_assert!(sim.settle(), "the gossip never settled");
        let holders = sim.holders();
        prop_assert!(
            holders.len() <= 1,
            "{} nodes each think they are running this run: {:?}",
            holders.len(),
            holders.iter().map(|n| n.short()).collect::<Vec<_>>()
        );
    }

    /// A settled fleet never disagrees about *who holds a run*, or at what epoch.
    ///
    /// Agreement, not predictability: which of two entitled speakers wins depends on delivery
    /// order and correctly so — a holder refuting an orphan and an arbiter declaring one are
    /// both right, and the merge asks who is speaking precisely so that both can be. What no
    /// fleet may come to rest on is two different holders or two different epochs, because
    /// those are decisions, and a fleet that disagrees about a decision has two futures.
    ///
    /// **The state name is a weaker claim, and only while the arbiter is there.** An `Orphaned`
    /// record is the arbiter's *observation* that its holder has gone quiet, and nobody else may
    /// relay one (that rule is what stops a third node's stale copy undoing a decision). So an
    /// observation half-delivered at the moment its author crashed stays half-delivered: some
    /// nodes say `orphaned`, the rest say `running`, about the same holder at the same epoch,
    /// and no amount of gossip moves either. That is ADR-0007's own distinction doing its job
    /// rather than failing — *unreachable is not dead, and dead is not a decision*. An orphan
    /// grants no authority to anybody, so the disagreement costs nothing, and it ends the moment
    /// anything *acts*: acting bumps the epoch, and a higher epoch wins outright everywhere.
    /// Which is why this checks the two fields that are decisions unconditionally, and the
    /// observation only when its author is present to state it.
    #[test]
    fn a_settled_fleet_holds_one_answer(
        members in members(),
        events in prop::collection::vec(ev(), 0..40),
    ) {
        let mut sim = run_sim(&members, &events)?;
        prop_assert!(sim.settle(), "the gossip never settled");
        let up: Vec<NodeId> = sim
            .members
            .iter()
            .copied()
            .filter(|id| sim.nodes[id].up)
            .collect();
        // A fleet with nobody left to disagree satisfies this vacuously rather than being an
        // input worth rejecting — churn crashes most of the fleet often enough that rejecting
        // these throws away the runs that got furthest.
        if up.len() < 2 {
            return Ok(());
        }

        let first = sim.record(up[0]);
        // Liveness has settled too, so every up node computes the same arbiter.
        let arbiter = sim.nodes[&up[0]].view.arbiter_for(&first);
        let arbiter_present = arbiter.is_some_and(|a| sim.nodes[&a].up);

        for id in &up[1..] {
            let theirs = sim.record(*id);
            prop_assert_eq!(
                (theirs.epoch, theirs.holder()),
                (first.epoch, first.holder()),
                "{} and {} settled on different decisions",
                up[0].short(),
                id.short()
            );
            if arbiter_present {
                prop_assert_eq!(
                    theirs.state.name(),
                    first.state.name(),
                    "{} says {} and {} says {}, with their arbiter right there",
                    up[0].short(),
                    first.state.name(),
                    id.short(),
                    theirs.state.name()
                );
            } else {
                // With nobody entitled to speak, the only thing they may differ on is whether
                // that holder is still answering.
                let pair = [first.state.name(), theirs.state.name()];
                prop_assert!(
                    first.state.name() == theirs.state.name()
                        || pair.contains(&"orphaned"),
                    "{} and {} differ on more than an observation: {:?}",
                    up[0].short(),
                    id.short(),
                    pair
                );
            }
        }
    }

    /// Two nodes that both believe they arbitrate do not leave two nodes holding the run.
    ///
    /// Aimed rather than stumbled upon, because the general walk above almost never assembles
    /// it: a wrongly-suspected home node needs the successor to have concluded it *dead* while
    /// it is still up and still arbitrating its own runs, and both of them to run a round. It is
    /// three coincidences in a random forty-step walk and a Tuesday afternoon in a real fleet —
    /// ADR-0007's `Suspect` exists exactly because this guess is often wrong.
    ///
    /// Both rounds grant at the same epoch, because both bump `next()` from the same record.
    /// Everything that keeps that safe is in the merge (ADR-0002's amendment), which is why the
    /// assertion is about the fleet after the gossip and not about either round.
    #[test]
    fn two_arbiters_that_both_grant_do_not_leave_two_holders(
        members in a_fleet_of_three(),
        churn in prop::collection::vec(ev(), 0..10),
        reverse in any::<bool>(),
    ) {
        let mut sim = Sim::new(&members);
        let home = sim.members[0];
        let holder = sim.record(home).holder().expect("the run starts held");

        // The holder really has gone: the lease has lapsed and everybody has concluded it dead,
        // so the hold-down will actually reassign rather than wait.
        sim.tick(120);
        for observer in sim.members.clone() {
            sim.notice(observer, holder);
            sim.notice(observer, holder);
            // …and everybody except the home node has concluded the *home* node is dead too.
            // Wrongly: it is up, nobody has told it, and it still arbitrates its own runs. That
            // asymmetry is the whole scenario, and `Suspect` exists because this guess is often
            // wrong (ADR-0007).
            if observer != home {
                sim.notice(observer, home);
                sim.notice(observer, home);
            }
        }

        let arbiters: Vec<NodeId> = sim
            .members
            .iter()
            .copied()
            .filter(|id| sim.nodes[id].up)
            .filter(|id| {
                let run = sim.record(*id);
                sim.nodes[id].view.is_local_arbiter(&run)
            })
            .collect();
        if arbiters.len() < 2 {
            return Ok(());
        }

        // Orphan, wait out the grace, then each arbiter runs its own round.
        for id in arbiters.clone() {
            sim.supervise(id);
        }
        sim.tick(120);
        for id in arbiters.clone() {
            sim.supervise(id);
        }
        sim.check_invariants()?;

        // Two rounds do not always collide — the arbiters may both pick the same node, which is
        // a fine outcome and not this test's subject. What is required is two *different*
        // holders at one epoch.
        // Only the arbiters that actually re-placed it. One of them may be the old holder
        // itself, which `supervise` correctly leaves alone (`HeldHere`) — its record still names
        // the original grant and is not a competing one.
        let mut granted: Vec<(NodeId, offload_core::run::Epoch)> = arbiters
            .iter()
            .filter_map(|id| {
                let run = sim.record(*id);
                run.state
                    .lease()
                    .filter(|l| l.node != holder)
                    .map(|l| (l.node, run.epoch))
            })
            .collect();
        granted.sort();
        granted.dedup();
        if granted.len() < 2 {
            return Ok(());
        }
        prop_assert!(
            granted.windows(2).all(|w| w[0].1 == w[1].1),
            "the premise is two grants at ONE epoch, and it got {granted:?}"
        );

        // Then let the fleet get on with it, in either direction, with whatever else happens.
        for ev in &churn {
            match *ev {
                Ev::Tick(s) => sim.tick(s),
                Ev::Gossip(a, b) => {
                    let (a, b) = (sim.who(a), sim.who(b));
                    sim.gossip(a, b);
                }
                Ev::Notice(a, b) => {
                    let (a, b) = (sim.who(a), sim.who(b));
                    sim.notice(a, b);
                }
                Ev::Progress(n, breaks) => {
                    let n = sim.who(n);
                    sim.progress(n, breaks);
                }
                Ev::Supervise(n) => {
                    let n = sim.who(n);
                    sim.supervise(n);
                }
                Ev::Crash(n) => {
                    let n = sim.who(n);
                    sim.crash(n);
                }
                Ev::Restart(n) => {
                    let n = sim.who(n);
                    sim.restart(n);
                }
            }
            sim.check_invariants()?;
        }
        // Settled twice from the same starting point, in both delivery orders. Convergence
        // alone is not the property and is not where the danger is: with the tie unresolved the
        // fleet *does* converge, on whichever grant each node happened to hear last, and it
        // agrees with itself perfectly while having chosen by coin toss. What has to be true is
        // that the answer is a function of the records — which is what lets every node work out
        // who lost, and therefore what makes the losing holder's own `fence` refuse it.
        let before_settling = sim.snapshot();
        prop_assert!(sim.settle_in(false), "the gossip never settled");
        let forward = sim.answers();

        sim.restore(&before_settling);
        prop_assert!(sim.settle_in(true), "the gossip never settled in reverse");
        let backward = sim.answers();

        prop_assert_eq!(
            &forward,
            &backward,
            "two arbiters granted at one epoch and the fleet picked its winner by arrival order"
        );

        // The other half, and the one a person would notice: whichever grant won, only one
        // node is left thinking it is the one running the agent.
        sim.restore(&before_settling);
        sim.settle_in(reverse);
        let holders = sim.holders();
        prop_assert!(
            holders.len() <= 1,
            "two arbiters, and {} nodes each think they are running the result: {:?}",
            holders.len(),
            holders.iter().map(|n| n.short()).collect::<Vec<_>>()
        );

        // And nobody is left believing a grant that lost. Every up node names one holder.
        let up: Vec<NodeId> = sim
            .members
            .iter()
            .copied()
            .filter(|id| sim.nodes[id].up)
            .collect();
        if let Some(first) = up.first() {
            let theirs = sim.record(*first);
            for id in &up[1..] {
                let mine = sim.record(*id);
                prop_assert_eq!(
                    (mine.epoch, mine.holder()),
                    (theirs.epoch, theirs.holder()),
                    "the fleet is still holding two grants: {} vs {}",
                    first.short(),
                    id.short()
                );
            }
        }
    }

    /// Which grant survives does not depend on the order the gossip was delivered in.
    ///
    /// The other half of the previous property, and the one that is about a *decision* rather
    /// than about an observation. Two nodes granted the same run at one epoch — what a wrongly
    /// suspected arbiter produces — are resolved by the record, so every node reaches the same
    /// answer without a message. Resolve it by arrival order instead and the fleet settles on
    /// whichever grant each node happened to hear last, which is not a decision at all: it is
    /// a coin, and the loser is whoever the network was slower to.
    ///
    /// State names are deliberately *not* compared. Whether a holder is `orphaned` or still
    /// `running` is an observation and the entitlement rules make it order-dependent on purpose
    /// — see the property above. Who holds it, and under which epoch, are decisions.
    #[test]
    fn which_grant_survives_does_not_depend_on_the_delivery_order(
        members in members(),
        events in prop::collection::vec(ev(), 0..40),
    ) {
        let mut sim = run_sim(&members, &events)?;
        let start = sim.snapshot();

        prop_assert!(sim.settle_in(false), "the gossip never settled");
        let forward = sim.answers();

        sim.restore(&start);
        prop_assert!(sim.settle_in(true), "the gossip never settled in reverse");
        let backward = sim.answers();

        prop_assert_eq!(
            &forward,
            &backward,
            "the fleet placed the run differently depending on who gossiped first"
        );
    }

    /// A run whose holder dies is taken by somebody else, or given up on out loud.
    ///
    /// The liveness half, and the reason this project exists: closing the laptop is supposed to
    /// move the work, not lose it. Driven forward rather than asserted at an instant — the
    /// hold-down is *meant* to wait (ADR-0007), so the question is whether waiting ever ends.
    /// A run left `orphaned` for ever with an idle fleet around it is the failure this catches,
    /// and it is invisible to any test that checks one tick.
    #[test]
    fn a_run_outlives_the_machine_it_was_running_on(
        members in a_fleet_of_three(),
        stagger in prop::collection::vec(0u8..5, 0..6),
    ) {
        let mut sim = Sim::new(&members);

        // Some churn first, so this is not always a pristine fleet: the holder may already have
        // been suspected once, and the run may already have moved.
        for i in stagger {
            let n = sim.who(i);
            sim.supervise(n);
            sim.gossip(n, sim.who(i.wrapping_add(1)));
            sim.tick(5);
        }
        sim.settle();

        let Some(holder) = sim.record(sim.members[0]).holder() else {
            return Ok(());
        };
        prop_assume!(sim.nodes[&holder].up);
        sim.crash(holder);

        // Now leave the fleet alone with the problem: notice, decide, gossip, wait. Everything
        // here is what the daemon's own loop does once a second.
        for _ in 0..40 {
            sim.tick(20);
            for observer in sim.members.clone() {
                sim.notice(observer, holder);
            }
            for id in sim.members.clone() {
                sim.supervise(id);
            }
            sim.settle();
            sim.check_invariants()?;

            let run = sim.record(
                *sim.members
                    .iter()
                    .find(|id| sim.nodes[id].up)
                    .expect("somebody is up"),
            );
            let moved = run.holder().is_some_and(|h| h != holder && sim.nodes[&h].up);
            if run.state.is_terminal() || moved {
                return Ok(());
            }
        }

        let survivor = *sim
            .members
            .iter()
            .find(|id| sim.nodes[id].up)
            .expect("somebody is up");
        let run = sim.record(survivor);
        prop_assert!(
            false,
            "13 minutes after its holder died the run is still {} on {:?}, and nobody has \
             either taken it or given up on it",
            run.state.name(),
            run.holder().map(|n| n.short())
        );
    }
}
