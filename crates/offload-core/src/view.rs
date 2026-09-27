//! The replicated cluster view: every node's picture of who exists, what they can do, and
//! who is running what.
//!
//! There is no central registry. Each node keeps a full view and merges gossip into it.
//! What makes that tractable is **fact ownership** — every field has exactly one
//! authoritative writer, so merging never needs a human-chosen tiebreak:
//!
//! | Fact                                   | Owner                | Arbitrated by  |
//! |----------------------------------------|----------------------|----------------|
//! | a node's capabilities, policy, workload| that node            | `incarnation`  |
//! | a node's liveness                      | its peers, refutable | `incarnation`  |
//! | a run's assignment and state           | the run's arbiter    | `epoch`        |
//! | a run's turns, cost and denials        | the run's holder     | forward only   |
//! | a run's deadline and priority           | the run's home node  | `spec_rev`     |
//!
//! See `docs/adr/0005-cluster-view.md`.

use crate::capability::Capabilities;
use crate::id::{NodeId, RunId};
use crate::policy::{NodeObservation, WorkPolicy};
use crate::progress::RunProgress;
use crate::run::{Epoch, Run, RunState};
use crate::time::Millis;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Alive,
    /// Unreachable, but not confirmed gone. Deliberately distinct from `Dead`: this is a
    /// suspicion held by peers, and the node can refute it.
    Suspect,
    Dead,
    /// Announced its own departure. Believed immediately — a node knows when it is leaving.
    Draining,
    /// Departed cleanly and is not expected back.
    Departed,
}

impl NodeStatus {
    /// Can this node be given new work?
    #[must_use]
    pub fn is_available(self) -> bool {
        self == NodeStatus::Alive
    }

    /// Has it stopped being a useful place for a run to be?
    #[must_use]
    pub fn is_gone(self) -> bool {
        matches!(
            self,
            NodeStatus::Dead | NodeStatus::Draining | NodeStatus::Departed
        )
    }
}

/// One node, as seen from here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeView {
    pub id: NodeId,
    /// What to call it in `offload nodes`. Cosmetic, and gossiped, so it is what the node
    /// *says* it is called. The authoritative name is the one on its membership certificate,
    /// which is signed precisely so a peer cannot relabel a device (ADR-0012) — the cluster
    /// overwrites this with that name for any node it has actually talked to.
    #[serde(default)]
    pub name: String,
    pub status: NodeStatus,
    /// Bumped by the node itself. Higher wins for anything the node owns, and is how a
    /// node refutes a `Suspect` assertion about it.
    pub incarnation: u64,
    pub capabilities: Capabilities,
    pub policy: WorkPolicy,
    /// What this node says it is running. The ground truth that lets any node rebuild the
    /// run registry by observation rather than by recovering a log.
    pub running: BTreeSet<RunId>,
    /// When any news of this node last changed its entry here, first-hand or relayed. Still sent,
    /// because a v37 node requires the field; **nothing reports it**. [`Self::heard_here`] is the
    /// observation `offload nodes` shows.
    pub last_heard: Millis,
    /// When **this** node last heard from that one first-hand — it answered, or sent something —
    /// or `None` if it never has. **Does not travel**, for the reason `absences` below does not:
    /// an observation cannot be relayed. `offload nodes` printed `last_heard`, which a relayed
    /// incarnation reset to now, so the laptop showed the Mac `dead` and seen `3s` ago in one row:
    /// dead by its own detector, "seen" through the tablet's gossip (session ninety-four).
    #[serde(skip)]
    pub heard_here: Option<Millis>,
    /// Absence history, feeding the drop-off hold-down policy (ADR-0007).
    ///
    /// **Does not travel** (ADR-0031), and it used to. It is an *observation* — what **this** node
    /// has seen of that peer's comings and goings — and an observation cannot be relayed, which is
    /// the rule `fleet_events`, attendance and orphan status all already follow. Gossiped, it had
    /// no owner and gave two different answers: a peer learned *indirectly* inherited the
    /// relayer's history of it wholesale (`merge_node`'s insert path takes `incoming` entire),
    /// while one learned directly started at zero — because a node's report about *itself* always
    /// carries zeros, `set_status` refusing to believe a peer about its own liveness. So the only
    /// values that ever crossed the wire were third-party hearsay.
    ///
    /// What it is instead is **durable**, per node, in `node_observations` — which is where the
    /// value actually needs to survive: a restart is precisely when a peer has just been absent.
    #[serde(skip)]
    pub absences: u32,
    #[serde(skip)]
    pub typical_absence: Option<Millis>,
    #[serde(skip)]
    absent_since: Option<Millis>,
    /// Where this node can be dialled, as it states it (ADR-0076): `ip:port`, the addresses mDNS
    /// would advertise for its bound socket. **Owned by the node itself** and carried in its own
    /// record, so it is replaced only by a newer incarnation, like its capabilities.
    ///
    /// Because mDNS does not always cross: the laptop never received the Mac's announcements and
    /// knew it only from gossip, with no address, so it could reach the Mac only over a connection
    /// the Mac had opened, and a laptop restart lost it until the Mac dialled again (session
    /// ninety-two). A dial to one of these is checked at the handshake like any other, so a stale
    /// or wrong address costs a failed dial and nothing else.
    #[serde(default)]
    pub addresses: Vec<String>,
    /// This node asks to be left mostly alone (ADR-0078): a phone on battery with its screen off.
    /// Peers probe it at most once a minute and give it longer before calling it dead, and it
    /// probes them as rarely. **Owned by the node itself**, like `addresses`: only it knows its
    /// battery and screen, and a peer must never decide that a node may be ignored.
    ///
    /// Because a phone probed about once a second never reached deep sleep: the phone was awake
    /// 9 h 48 min of 10 h 55 min overnight and lost half its charge (session ninety-two).
    #[serde(default)]
    pub quiet: bool,
}

impl NodeView {
    #[must_use]
    pub fn new(id: NodeId, capabilities: Capabilities, policy: WorkPolicy, now: Millis) -> Self {
        NodeView {
            id,
            name: String::new(),
            status: NodeStatus::Alive,
            incarnation: 0,
            capabilities,
            policy,
            running: BTreeSet::new(),
            last_heard: now,
            heard_here: None,
            absences: 0,
            typical_absence: None,
            absent_since: None,
            addresses: Vec::new(),
            quiet: false,
        }
    }

    /// Name it, for display. Builder-style because a name is optional everywhere the view is
    /// constructed in a test.
    #[must_use]
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Something to print. Falls back to the short id, which is what a node discovered but
    /// never spoken to has.
    #[must_use]
    pub fn display_name(&self) -> String {
        if self.name.is_empty() {
            self.id.short()
        } else {
            self.name.clone()
        }
    }

    #[must_use]
    pub fn observation(&self) -> NodeObservation {
        NodeObservation {
            status: self.status,
            typical_absence: self.typical_absence,
            observed_absences: self.absences,
        }
    }

    /// Seed what this node had learned about a peer before it restarted (ADR-0031).
    ///
    /// Applied **only where nothing has been learned in this incarnation**, which is what makes it
    /// safe to call on every tick and what makes "apply on first contact" need no bookkeeping of
    /// its own: once a value is in, the guard stops it being overwritten by the same stale row for
    /// ever, and a live observation always wins over a remembered one.
    ///
    /// Returns whether it seeded anything.
    pub fn seed_absence_history(&mut self, absences: u32, typical: Option<Millis>) -> bool {
        if self.absences > 0 || self.typical_absence.is_some() {
            return false;
        }
        if absences == 0 && typical.is_none() {
            return false;
        }
        self.absences = absences;
        self.typical_absence = typical;
        true
    }

    /// Record a status change, maintaining the absence statistics the hold-down policy
    /// depends on. A node whose absences are consistently short earns patience.
    ///
    /// **The one place the average is computed.** `Store::record_return` was a second copy of this
    /// arithmetic, with a doc comment naming the hazard — "both places must agree, or a restart
    /// would visibly change how patient the policy is" — and neither had a caller. Deleted: the
    /// store persists what this produces rather than re-deriving it (ADR-0031).
    pub fn set_status(&mut self, status: NodeStatus, now: Millis) {
        if self.status == status {
            return;
        }
        let was_present = self.status == NodeStatus::Alive;
        let now_present = status == NodeStatus::Alive;

        if was_present && !now_present {
            self.absent_since = Some(now);
        } else if !was_present && now_present {
            if let Some(since) = self.absent_since.take() {
                let duration = now.saturating_sub(since);
                self.absences += 1;
                // Exponential moving average, 1/4 weight on the newest sample. Cheap, and
                // it forgets an unusual outage after a handful of normal ones.
                self.typical_absence = Some(match self.typical_absence {
                    None => duration,
                    Some(prev) => Millis((prev.0 * 3 + duration.0) / 4),
                });
            }
        }
        self.status = status;
    }
}

/// Every node's full picture of the cluster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterView {
    pub local: NodeId,
    pub nodes: BTreeMap<NodeId, NodeView>,
    pub runs: BTreeMap<RunId, Run>,
    /// What each run has done and cost, kept beside the runs rather than inside them.
    ///
    /// A separate map because it is a separate fact with a separate owner: the record says
    /// where a run *is* and this says how far it has got, and they are merged by different
    /// rules (see [`crate::progress`]). A node may know one without the other in either
    /// direction — progress for a run whose record has not arrived, or a record from a node
    /// that has never run a turn of it.
    #[serde(default)]
    pub progress: BTreeMap<RunId, RunProgress>,
}

impl ClusterView {
    #[must_use]
    pub fn new(local: NodeId) -> Self {
        ClusterView {
            local,
            nodes: BTreeMap::new(),
            runs: BTreeMap::new(),
            progress: BTreeMap::new(),
        }
    }

    pub fn upsert_node(&mut self, view: NodeView) {
        self.nodes.insert(view.id, view);
    }

    /// Merge a gossiped [`NodeView`]. Returns whether anything changed.
    ///
    /// Two different rules run here, because a `NodeView` carries two different kinds of
    /// fact (ADR-0005):
    ///
    /// * **What the node says about itself** — capabilities, policy, what it is running —
    ///   moves only on a higher `incarnation`. Nobody else may edit those.
    /// * **What peers say about its liveness** moves on a higher incarnation *or*, at equal
    ///   incarnation, on stronger evidence: [`liveness_rank`]. Without that second half a
    ///   `Suspect` could never spread, since suspecting a node does not change its
    ///   incarnation — the node is not participating in the claim at all.
    ///
    /// Locally observed history — absence statistics, when *we* last heard from it — is never
    /// taken from a peer. It is our observation, and a node refuting a suspicion must not be
    /// able to erase our record of how often it does that.
    pub fn merge_node(&mut self, incoming: NodeView, now: Millis) -> bool {
        if incoming.id == self.local {
            // Our own entry is ours. A peer that thinks we are suspect is answered by bumping
            // our incarnation and saying so (`refute`), never by believing it about ourselves.
            //
            // **But the peer's *number* is information.** At equal incarnation a *worse* claim
            // wins — `liveness_rank` puts `Alive` lowest, deliberately, so suspicion beats
            // silence — which means the only way to correct a peer that is wrong about us is to
            // outrank it. The daemon did that for a peer that thought we were gone, beside its
            // merge loop, and could not do it for a peer that remembered a number: two answers to
            // one question, in two places, with the second one missing. Both are here now, and
            // neither is us believing anything about ourselves:
            let me = self.nodes.get(&self.local);
            let mine = me.map_or(0, |me| me.incarnation);
            // It is holding facts about us that are not ours — a *previous life's* capabilities or
            // policy. This is the case that needs no partition and no suspicion, and it is every
            // restart: an incarnation lives in memory, so a node begins again at 0, and if the
            // peer's remembered copy is *also* at 0 the merge below copies nothing, because it
            // requires strictly greater. Measured on two daemons: a mailbox nominated on a node
            // that restarted stayed invisible to the fleet for 37 seconds and became visible only
            // when an unrelated cpu-load change bumped the counter — and on a device whose
            // capabilities do not churn, never.
            let stale_facts = me.is_some_and(|me| {
                incoming.capabilities != me.capabilities
                    || incoming.policy != me.policy
                    || incoming.addresses != me.addresses
                    || incoming.quiet != me.quiet
            });
            // Or it remembers a life ahead of ours, which is the same thing with the numbers
            // further apart.
            let remembers_more = incoming.incarnation > mine;
            // Or it thinks we are gone, which is the textbook case and the one the daemon
            // already answered. Deliberately at *any* incarnation, including one below ours,
            // because that is the behaviour that shipped: `refute_above` with a lower floor is
            // exactly the old `refute()`, so this adds cases rather than changing one.
            let thinks_we_are_gone =
                liveness_rank(incoming.status) > liveness_rank(NodeStatus::Alive);
            if thinks_we_are_gone || remembers_more || (incoming.incarnation == mine && stale_facts)
            {
                self.refute_above(incoming.incarnation, now);
                return true;
            }
            // A peer echoing our own number back at us is the ordinary case and must change
            // nothing: bumping on every gossip tick would make our entry churn for ever.
            return false;
        }
        let Some(existing) = self.nodes.get_mut(&incoming.id) else {
            self.nodes.insert(incoming.id, incoming);
            return true;
        };

        // Decided before anything is written: adopting the incarnation first would make the
        // liveness comparison below see two equal incarnations and quietly stop refutations
        // from taking effect.
        let liveness = liveness_wins(&incoming, existing);

        let mut changed = false;
        if incoming.incarnation > existing.incarnation {
            existing.incarnation = incoming.incarnation;
            if !incoming.name.is_empty() {
                existing.name = incoming.name.clone();
            }
            existing.capabilities = incoming.capabilities.clone();
            existing.policy = incoming.policy.clone();
            existing.running = incoming.running.clone();
            existing.addresses = incoming.addresses.clone();
            existing.quiet = incoming.quiet;
            existing.last_heard = now;
            changed = true;
        }
        if liveness {
            // Through `set_status` so the absence statistics the hold-down policy reads stay
            // maintained whether a status change came from a probe or from gossip.
            existing.set_status(incoming.status, now);
            changed = true;
        }
        changed
    }

    /// Bump our own incarnation and assert that we are alive.
    ///
    /// The answer to a peer suspecting us: the node is the owner of its own liveness in the
    /// one direction that matters, and this is how that ownership is exercised (ADR-0005).
    /// Returns the new incarnation, which the caller gossips.
    ///
    /// Reached through [`Self::merge_node`], which is now the one owner of the rule — the
    /// daemon's `absorb` used to skip its own entry and call this beside the loop instead, which
    /// covered a peer that thought we were *gone* and not a peer that remembered a *number*.
    pub fn refute(&mut self, now: Millis) -> u64 {
        self.refute_above(0, now)
    }

    /// Refute, and land clear of a number a peer is still holding.
    ///
    /// The floor is what makes this work after a **restart**, and it is the half a plain
    /// refutation cannot do: `+1` from a node whose incarnation began again at nothing does not
    /// escape the number its peers remember. A node's incarnation lives in memory, and
    /// `merge_node` copies a peer's facts only when the incoming incarnation is *greater* — so
    /// everything a restarted node says about itself is dropped: its capabilities, its policy,
    /// its running set. Measured on two daemons: a mailbox nominated on a node that then
    /// restarted stayed invisible to the fleet for **100 seconds**, and became visible only
    /// because that laptop's cpu load kept changing and each change bumped the incarnation by
    /// one. On a quiet device — a phone, the thing most likely to restart and least likely to
    /// churn — the window has no bound at all.
    fn refute_above(&mut self, floor: u64, now: Millis) -> u64 {
        let local = self.local;
        match self.nodes.get_mut(&local) {
            Some(me) => {
                me.incarnation = me.incarnation.max(floor).saturating_add(1);
                me.set_status(NodeStatus::Alive, now);
                me.last_heard = now;
                me.heard_here = Some(now);
                me.incarnation
            }
            None => 0,
        }
    }

    /// Merge a gossiped [`Run`]. Ordered by `(epoch, progress)`: epoch is monotonic by
    /// construction, and within one epoch a run only ever moves forward.
    pub fn merge_run(&mut self, mut incoming: Run, from: NodeId) -> bool {
        let arbiter = self.arbiter_for(&incoming);
        let local = self.local;
        let Some(existing) = self.runs.get_mut(&incoming.id) else {
            // **A node does not learn of its own run from somebody else.** The sibling of the
            // rule one function up — a node is the owner of its own liveness, and it is the
            // owner of its own runs' *existence* too. A record naming this node as `home` was
            // created here, so if it is not here now it was deleted here (ADR-0021 prunes a
            // rule's spent occurrences), and a peer's copy is a memory rather than news.
            //
            // Without this, ADR-0021 has a hole with no partition in it: a laptop that saw an
            // occurrence while it was `Running` and then closed for an hour comes back still
            // holding that record, the home node has since pruned it, and the home node takes
            // it as a run it has never heard of — held by itself, at an epoch nothing can
            // contradict, with no agent behind it. `heartbeat` then renews that lease for ever,
            // `ps` reports a run that finished last night as running, and a restart offers it to
            // somebody as resumable. That is the "a run left `Running` with no agent behind it,
            // its lease renewed by the very loop that erased it" shape, reached from outside.
            //
            // Safe because the ordering is not a race: `hand_over` records a grant locally
            // before it publishes it, and a run kept here is saved by `take_run` before the
            // arbiter confirms — so by the time any peer can echo one of our runs back, our own
            // copy exists. The residual is stated in ADR-0021 §7: the stale peer keeps its copy,
            // and nobody who matters accepts it.
            if incoming.home == local {
                return false;
            }
            self.runs.insert(incoming.id, incoming);
            return true;
        };

        // The editable part of the spec is settled before the record is, and separately from
        // it — see [`settle_spec`].
        let spec_moved = settle_spec(existing, &mut incoming);

        // A higher epoch is a decision that has already been made. Nothing at a lower one has
        // anything to say — except the spec edit it may have been carrying, which is why that
        // is settled above rather than here.
        match incoming.epoch.cmp(&existing.epoch) {
            std::cmp::Ordering::Greater => {
                *existing = incoming;
                return true;
            }
            std::cmp::Ordering::Less => return spec_moved,
            std::cmp::Ordering::Equal => {}
        }

        // At equal epoch, *who is speaking* decides — which is the part a rank comparison
        // alone gets wrong. `Orphaned` is not a lesser state than `Running`; it is the
        // arbiter's observation that the holder has gone quiet, and a third node relaying its
        // own stale copy of `Running` must not undo it. That bug keeps a run pinned to a
        // machine that no longer exists, for as long as anybody remembers it working.
        let accept = match (&existing.state, &incoming.state) {
            // Finished is finished. Nothing at the same epoch reopens it.
            (state, _) if state.is_terminal() => false,
            (_, state) if state.is_terminal() => true,
            // Two live grants at one epoch: decided by the token, never by who spoke last.
            //
            // This is the one case the who-is-speaking rules below cannot settle, because both
            // speakers are entitled. Each record's holder is *its* holder, so `from_holder` is
            // true on both sides and whichever arrived most recently won — which is not a
            // decision, it is a coin the fleet flips again every gossip tick. Neither agent
            // ever learns it lost, and two agents on one repository, both committing, is the
            // failure this design exists to prevent.
            //
            // ADR-0002 accepts that a partition can produce two arbiters and says fencing makes
            // the second writer a *rejected* one. That is only true if the two grants can be
            // ordered, and two arbiters both bump `next()` from the same number — so the epoch
            // orders grants made by one arbiter and says nothing about grants made by two. The
            // tiebreak is the lowest holder id, which is the same one the bid round uses and is
            // computable by every node from the record alone, offline, with no message. Which
            // grant wins is arbitrary; that every node picks the *same* one is the point,
            // because it is what makes the loser's record name somebody else and its own
            // `fence` refuse it.
            (a, b) => match (a.lease(), b.lease()) {
                (Some(ours), Some(theirs)) if ours.node != theirs.node => theirs.node < ours.node,
                _ => Self::speaker_decides(existing, &incoming, from, arbiter, local),
            },
        };

        if accept && *existing != incoming {
            *existing = incoming;
            true
        } else {
            spec_moved
        }
    }

    /// Settle the half of a spec this node does not own, and nothing else.
    ///
    /// For the run a node is **holding**. Its own record is newer than anything a peer can say
    /// about it — a stale copy would undo a turn — and that is a statement about the state, the
    /// epoch, the lease and the checkpoint, all of which the holder owns. The deadline and the
    /// priority it does not own: they belong to the run's home node, which is where an
    /// `offload deadline` is forwarded to, and gossip is the only way they can travel back.
    /// Discarding a peer's whole record therefore discards the one field on it that was never
    /// ours to have an opinion about.
    ///
    /// So the two halves separate here: `merge_run` for the record, this for the edit.
    pub fn merge_spec_edit(&mut self, incoming: &Run) -> bool {
        let Some(existing) = self.runs.get_mut(&incoming.id) else {
            return false;
        };
        // Cloned because the rule is written once and settles both directions; the copy is
        // discarded, and only the fields the home node owns are taken from it.
        let mut incoming = incoming.clone();
        settle_spec(existing, &mut incoming)
    }

    /// At one epoch and one grant, who is entitled to be believed.
    fn speaker_decides(
        existing: &Run,
        incoming: &Run,
        from: NodeId,
        arbiter: Option<NodeId>,
        local: NodeId,
    ) -> bool {
        let from_arbiter = Some(from) == arbiter;
        let from_holder = Some(from) == incoming.holder();
        match (&existing.state, &incoming.state) {
            // Only the holder may say it is still there — that is the reclaim path, and it is
            // the cheapest possible outcome of a device dropping off (ADR-0007).
            (RunState::Orphaned { .. }, _) => from_holder || from_arbiter,
            // And only the arbiter may say it has gone — never about a run *this node is
            // holding*, which is [`Self::merge_node`]'s rule about our own liveness, arriving
            // late at the one place it matters more.
            //
            // A holder that took a peer's word for its own absence lost the argument it is
            // uniquely qualified to win. It has no lease afterwards, because `Orphaned` returns
            // none by design, so it cannot renew — and when its agent *finishes*, `complete` is
            // fenced out and the work is not recorded. All from two missed probes, which is a
            // Wi-Fi handover. The arbiter is not wrong to have said it; it is simply asking the
            // wrong node to agree, and the answer to being suspected is to say so (the reclaim
            // path above, from the other end).
            (held, RunState::Orphaned { .. }) => {
                from_arbiter && !held.lease().is_some_and(|lease| lease.node == local)
            }
            // Otherwise the owners of the record — the holder running it and the arbiter
            // deciding about it — are believed even when the *state* has not changed. Most of
            // what a run does happens inside `Running`: turns, and the checkpoints that make
            // them survivable. Requiring the state to advance means none of that ever
            // travels, and every ungraceful migration starts the conversation again.
            _ if from_holder || from_arbiter => true,
            _ => run_order(incoming) > run_order(existing),
        }
    }

    /// Merge a gossiped [`RunProgress`]. Returns the record **as settled**, if it moved.
    ///
    /// The rule is in [`RunProgress::absorb`], and it is half `merge_run`'s and half not, which
    /// is the distinction worth carrying: where a run *is* describes a decision and is settled by
    /// the epoch exactly as the record is, while what it has *spent* describes work that
    /// happened and only ever grows. An epoch bump does not reset a run's bill.
    ///
    /// As settled rather than as it arrived, for `absorb`'s reason one layer up: a merge that
    /// combines two records has an answer that is neither of them, and handing the caller the
    /// incoming copy to write down would leave the store holding a record the view never agreed
    /// with — permanently, since the loser never wins a later merge.
    pub fn merge_progress(&mut self, run: RunId, incoming: RunProgress) -> Option<RunProgress> {
        match self.progress.get_mut(&run) {
            Some(existing) => existing.absorb(&incoming).then(|| existing.clone()),
            // Nothing to fold into, so the first thing a node hears about a run *is* the answer.
            None => {
                self.progress.insert(run, incoming.clone());
                Some(incoming)
            }
        }
    }

    /// Forget the numbers for runs nobody is talking about any more.
    ///
    /// Progress rides along with the run records, and those age out — live runs plus what
    /// finished recently. Without this the map is the one thing in the view that only ever
    /// grows, on a daemon that is meant to run for months.
    pub fn prune_progress(&mut self) {
        self.progress.retain(|id, _| self.runs.contains_key(id));
    }

    /// Forget runs that finished before `cutoff`, and their numbers. Returns how many went.
    ///
    /// **What makes the view hold "live runs plus what finished recently", which it did not.** A
    /// node publishes only those (`Supervisor::gossipable_runs`), but the view kept every record
    /// it had ever been sent: republishing drops only runs this node holds, and a finished run
    /// is held by nobody. So every probe carried the fleet's whole history, about 190 records on
    /// the phone's walk fleet, and its daemon spent a quarter of a core idle encoding and decoding
    /// it (measured, session ninety-two). Every node keeps its own stored copy, which is what
    /// `ps`, `logs` and `explain` read, so forgetting here loses nothing but the re-sending.
    pub fn forget_finished_before(&mut self, cutoff: Millis) -> usize {
        let before = self.runs.len();
        self.runs
            .retain(|_, run| run.state.finished_at().is_none_or(|at| at >= cutoff));
        self.prune_progress();
        before - self.runs.len()
    }

    pub fn node(&self, id: &NodeId) -> Option<&NodeView> {
        self.nodes.get(id)
    }

    pub fn alive(&self) -> impl Iterator<Item = &NodeView> {
        self.nodes.values().filter(|n| n.status.is_available())
    }

    /// Runs this view believes are held by `node`, from the run records themselves rather
    /// than from the node's self-report.
    pub fn runs_held_by<'a>(&'a self, node: &'a NodeId) -> impl Iterator<Item = &'a Run> {
        self.runs
            .values()
            .filter(move |r| r.holder().as_ref() == Some(node))
    }

    #[must_use]
    pub fn running_count(&self, node: &NodeId) -> u32 {
        self.occupancy(node).runs
    }

    /// What this node has on it: how many runs, and what their demands add up to.
    ///
    /// Both from the same filter, deliberately. Two functions walking the same runs with the
    /// same predicate is a predicate that gets edited once, and a node whose count and shares
    /// describe different sets of runs would refuse work for a reason nobody could reconstruct.
    ///
    /// `Assigned` counts, because a commitment fills a slot for the purpose of accepting *more*
    /// work (ADR-0006) — which is the question this occupancy is for. The other question, what
    /// to *start* next, counts running agents and is asked locally where that is knowable.
    #[must_use]
    pub fn occupancy(&self, node: &NodeId) -> crate::capacity::Occupancy {
        let held = self.runs_held_by(node).filter(|r| {
            matches!(
                r.state,
                RunState::Running { .. } | RunState::Assigned { .. }
            )
        });
        let mut occupancy = crate::capacity::Occupancy::default();
        for run in held {
            occupancy.runs = occupancy.runs.saturating_add(1);
            occupancy.shares = occupancy.shares.saturating_add(run.spec.demand.shares());
        }
        occupancy
    }

    #[must_use]
    pub fn running_count_for_agent(&self, node: &NodeId, agent: &crate::AgentKind) -> u32 {
        self.runs_held_by(node)
            .filter(|r| r.spec.agent().is_some_and(|work| &work.agent == agent))
            .filter(|r| {
                matches!(
                    r.state,
                    RunState::Running { .. } | RunState::Assigned { .. }
                )
            })
            .count()
            .try_into()
            .unwrap_or(u32::MAX)
    }

    /// What one agent account has running across the fleet, and the ceiling it must stay under.
    ///
    /// `None` when the node is not signed into a comparable account for this agent, or when
    /// nobody on that account claims a limit — an unstated ceiling is not a ceiling of zero.
    ///
    /// The limit is the **lowest** any node on the account claims. A node cannot be allowed to
    /// raise the fleet's ceiling by claiming more for itself, which a maximum or an average
    /// would both let it do; and if two nodes disagree about an account's rate limit, the
    /// cautious one is the one to believe. Each node still owns its own claim, so this needs no
    /// new entry in ADR-0005's ownership table — it is a fold over a field that already has an
    /// owner, not a new gossiped fact.
    #[must_use]
    pub fn account_use(
        &self,
        node: &NodeId,
        agent: &crate::AgentKind,
    ) -> Option<crate::capacity::AccountUse> {
        let account = self
            .node(node)?
            .capabilities
            .agent(agent)
            .filter(|c| c.authenticated)
            .and_then(|c| c.identity.clone())?;

        let on_account: Vec<&NodeId> = self
            .nodes
            .iter()
            .filter(|(_, view)| {
                view.capabilities
                    .agent(agent)
                    .filter(|c| c.authenticated)
                    .and_then(|c| c.identity.as_ref())
                    == Some(&account)
            })
            .map(|(id, _)| id)
            .collect();

        let limit = on_account
            .iter()
            .filter_map(|id| self.node(id))
            .filter_map(|view| view.policy.max_concurrent_account)
            .min()?;

        // Everybody on the account *except* this node. What this node has is a question its own
        // store answers exactly, and the caller supplies it: see `AccountUse::elsewhere` for the
        // two bugs that came of asking the view about ourselves.
        let elsewhere = on_account
            .iter()
            .filter(|id| ***id != *node)
            .map(|id| self.running_count_for_agent(id, agent))
            .fold(0u32, u32::saturating_add);

        Some(crate::capacity::AccountUse {
            account,
            elsewhere,
            limit,
        })
    }

    /// Who arbitrates bids for this run.
    ///
    /// Its home node, until that node is *gone*. Otherwise the lowest-id available node,
    /// which every node computes identically from the same view — no election round-trip.
    /// Arbitration is per-run, so it spreads across the fleet instead of funnelling through
    /// one coordinator. See ADR-0006.
    ///
    /// **A suspected home node keeps arbitrating**, which is where this differs from every
    /// other "can this node be given work" question in the codebase. `Suspect` is one peer's
    /// guess and the node can refute it (ADR-0007), so failing over on suspicion means the
    /// home node — which still believes itself alive, because it is — and the successor both
    /// arbitrate the same run at once. Two arbiters run two bid rounds and grant the same run
    /// twice; the epochs then fence one of the agents, but only after both have started. A
    /// few extra seconds of waiting for `Suspect` to resolve is the cheaper mistake.
    /// A home node this view has never heard of returns `None` rather than failing over:
    /// "not met yet" and "watched it die" look the same from here, and the first is the
    /// ordinary state of a node in its first second of gossip. Preferring a stalled run to a
    /// twice-arbitrated one is the same trade the epoch fencing makes everywhere else.
    #[must_use]
    pub fn arbiter_for(&self, run: &Run) -> Option<NodeId> {
        self.steward_of(run.home)
    }

    /// Is this node the only one there is?
    ///
    /// Every node this one has **met**, whatever it is doing now — a peer that is asleep,
    /// suspected or even dead counts, because a device in a bag comes back and a run waiting for
    /// it is exactly what handing one over is for (ADR-0043). What this answers is the narrower
    /// question: has this fleet ever had a second member?
    ///
    /// One definition, because three callers ask it — the recovery tick, a departure, and
    /// `offload explain` — and a second copy would be a fleet of one that two of them disagreed
    /// about. On a node with no mesh the caller has `local_view`, which holds exactly this node,
    /// so the answer comes out of the same function rather than out of an `Option<Cluster>`.
    #[must_use]
    pub fn alone(&self) -> bool {
        self.nodes.keys().all(|id| *id == self.local)
    }

    /// The same successor rule, for anything else that has a home node and needs one node to
    /// act on it.
    ///
    /// A **schedule** is the second such thing (ADR-0019 §3, ADR-0056 §4): exactly one node
    /// fires it, and the answer is its home while that node is available, else the lowest-id
    /// available node. Factored rather than restated, because a second copy of a successor rule
    /// is two answers to *who acts* — and every paragraph above applies here unchanged, `Suspect`
    /// and never-met included.
    ///
    /// It is [`Self::arbiter_for`] with the `Run` taken out, and that is all it is.
    #[must_use]
    pub fn steward_of(&self, home: NodeId) -> Option<NodeId> {
        match self.node(&home) {
            Some(node) if !node.status.is_gone() => Some(home),
            Some(_) => self.alive().map(|n| n.id).min(),
            None => None,
        }
    }

    #[must_use]
    pub fn is_local_arbiter(&self, run: &Run) -> bool {
        self.arbiter_for(run) == Some(self.local)
    }

    // There was an `orphan_candidates` here — "the entry point for the drop-off policy" — with no
    // reference anywhere, tests included. The entry point is `policy::supervise`, which is asked
    // about **every** run rather than a pre-filtered set, on purpose: it decides about a healthy
    // run too (whether to leave it alone), and `is_local_arbiter` is one of the answers it gives
    // rather than a filter applied before it is consulted. A dead helper claiming to be the way in
    // is worse than no helper, because the next reader believes it.
}

/// How strong a liveness claim is, at equal incarnation.
///
/// Suspicion beats silence, confirmation beats suspicion, and a node's own announcement that
/// it is leaving beats a peer's guess about why it went quiet — a node knows when it is
/// leaving, and `Draining` reached us because it said so.
///
/// **`Dead` is not terminal here, and that is a deliberate departure from textbook SWIM.** A
/// higher incarnation from the node itself brings it back, because in this fleet the common
/// case behind "unreachable" is a laptop in a bag, and a device that comes back should not
/// have to be re-enrolled to be believed (ADR-0007: unreachable is not dead, and dead is not
/// a decision).
#[must_use]
pub fn liveness_rank(status: NodeStatus) -> u8 {
    match status {
        NodeStatus::Alive => 0,
        NodeStatus::Suspect => 1,
        NodeStatus::Dead => 2,
        NodeStatus::Draining => 3,
        NodeStatus::Departed => 4,
    }
}

fn liveness_wins(incoming: &NodeView, existing: &NodeView) -> bool {
    match incoming.incarnation.cmp(&existing.incarnation) {
        // Fresher word about that node, whichever direction it points.
        std::cmp::Ordering::Greater => incoming.status != existing.status,
        std::cmp::Ordering::Equal => {
            liveness_rank(incoming.status) > liveness_rank(existing.status)
        }
        std::cmp::Ordering::Less => false,
    }
}

/// Total order on a run's progress, for merge. Within one epoch a run only advances, and
/// releasing a run bumps the epoch, so this never has to move backwards.
fn run_order(run: &Run) -> (Epoch, u8) {
    let rank = match run.state {
        RunState::Pending { .. } => 0,
        // A peer's suspicion, weaker than the holder's own claim to be running.
        RunState::Orphaned { .. } => 1,
        RunState::Assigned { .. } => 2,
        RunState::Running { .. } => 3,
        RunState::Checkpointing { .. } => 4,
        RunState::Completed { .. } | RunState::Failed { .. } | RunState::Cancelled { .. } => 5,
    };
    (run.epoch, rank)
}

/// Settle the editable half of a spec between two copies of one run — the deadline and the
/// priority, which move together under one revision counter (`SpecEdit`).
///
/// Everything else about a spec is fixed at submit time, so "whichever record wins carries the
/// spec" holds for all of it — except these, where the loser can be the only one that has heard
/// about the edit. The home node owns them and counts its own writes; a higher revision
/// therefore wins them outright, in either direction, and as a pair: a record at revision N
/// carries the spec as it stood after N edits, whichever fields those touched.
///
/// A free function because the view is not the only place two copies of a run meet: a node's
/// store holds the copy that a resume, an ordering decision and a hold-down all read, and an
/// edit that settled in the view alone is an edit the machine running the run never acted on
/// (`Supervisor::apply_spec_edit`).
///
/// Returns whether `existing` moved. Both directions, which is why `incoming` is mutable: a
/// caller may go on to adopt that record whole, and it must carry the spec as settled rather
/// than as it arrived.
pub fn settle_spec(existing: &mut Run, incoming: &mut Run) -> bool {
    match incoming.spec_rev.cmp(&existing.spec_rev) {
        std::cmp::Ordering::Greater => {
            existing.spec.deadline = incoming.spec.deadline;
            existing.spec.priority = incoming.spec.priority;
            existing.spec_rev = incoming.spec_rev;
            true
        }
        std::cmp::Ordering::Less => {
            incoming.spec.deadline = existing.spec.deadline;
            incoming.spec.priority = existing.spec.priority;
            incoming.spec_rev = existing.spec_rev;
            false
        }
        std::cmp::Ordering::Equal => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{Arch, DeviceClass, Os};
    use crate::run::{AgentWork, Work};

    fn node_id(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    fn checkpoint_at(turns: u32) -> crate::Checkpoint {
        crate::Checkpoint {
            session_id: Some("s".into()),
            transcript: crate::BlobHash::from_bytes([9; 32]),
            bundle: None,
            patch: None,
            base_commit: "0f0f".into(),
            turns,
            taken_at: Millis(turns.into()),
            agent_version: "2.11.0".into(),
            replicas: std::collections::BTreeSet::new(),
        }
    }

    /// A run whose home node is `home`, for the merge tests.
    /// Put a run into a view the way the daemon does — directly, not by learning it.
    ///
    /// A node never learns of its own run from a peer, which `merge_run` now relies on
    /// (ADR-0021 §7): `hand_over` records a grant before it publishes one, `take_run` saves a
    /// run kept here before the arbiter confirms it, and the gossip tick republishes this
    /// node's store straight into the view. A fixture that introduced the *home* node's own run
    /// by gossip was modelling a step the daemon has no code path for, which is `storm.rs`'s
    /// lesson one layer down: a simulation that breaks the rule it is modelling reports the
    /// product as broken.
    fn seed(cv: &mut ClusterView, run: &crate::Run) {
        cv.runs.insert(run.id, run.clone());
    }

    /// When this node last heard from a peer is its own observation, and gossip cannot move it: a
    /// node learned by relay has never been heard from here, and a relayed newer incarnation of a
    /// node that is dead here does not make it "seen". The laptop showed the Mac `dead` and seen
    /// `3s` ago in one row, because merging the tablet's copy reset the time (session ninety-four).
    #[test]
    fn gossip_does_not_count_as_having_heard_from_a_node() {
        let mut cv = ClusterView::new(node_id(1));
        let caps = Capabilities::empty(Os::MacOs, Arch::Aarch64, DeviceClass::Desktop);
        let policy = WorkPolicy::for_class(DeviceClass::Desktop);
        let mut mac = NodeView::new(node_id(2), caps, policy, Millis(0));
        mac.incarnation = 3;
        // Even if a relayer's copy claims a time, it does not travel: skipped on the wire.
        mac.heard_here = Some(Millis(5));
        let wire: NodeView =
            serde_json::from_str(&serde_json::to_string(&mac).expect("encode")).expect("decode");
        assert_eq!(wire.heard_here, None, "an observation is not on the wire");

        cv.merge_node(wire.clone(), Millis(10));
        assert_eq!(
            cv.nodes[&node_id(2)].heard_here,
            None,
            "known by relay: never heard here"
        );

        // Heard from first-hand at 20, then concluded dead here.
        if let Some(entry) = cv.nodes.get_mut(&node_id(2)) {
            entry.heard_here = Some(Millis(20));
            entry.status = NodeStatus::Dead;
        }
        let newer = NodeView {
            incarnation: 4,
            ..wire
        };
        assert!(
            cv.merge_node(newer, Millis(60)),
            "its newer facts are still taken"
        );
        assert_eq!(cv.nodes[&node_id(2)].incarnation, 4);
        assert_eq!(
            cv.nodes[&node_id(2)].heard_here,
            Some(Millis(20)),
            "but relayed news is not having heard from it"
        );
    }

    /// ADR-0076: a node's addresses are its own, and travel in its record: a newer incarnation
    /// brings them, and a copy at the same incarnation (an old node relaying it without them)
    /// cannot erase them, which is why the field came with a version bump.
    #[test]
    fn a_nodes_addresses_come_with_its_newer_record() {
        let mut cv = ClusterView::new(node_id(1));
        let caps = Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Desktop);
        let policy = WorkPolicy::for_class(DeviceClass::Desktop);
        let mut peer = NodeView::new(node_id(2), caps, policy, Millis(0));
        peer.addresses = vec!["192.0.2.51:7433".into()];
        peer.incarnation = 3;
        cv.merge_node(peer.clone(), Millis(1));
        assert_eq!(cv.nodes[&node_id(2)].addresses, peer.addresses);

        let relayed = NodeView {
            addresses: Vec::new(),
            ..peer.clone()
        };
        cv.merge_node(relayed, Millis(2));
        assert_eq!(
            cv.nodes[&node_id(2)].addresses,
            peer.addresses,
            "same incarnation: kept"
        );

        let moved = NodeView {
            addresses: vec!["10.0.0.9:7433".into()],
            incarnation: 4,
            ..peer
        };
        cv.merge_node(moved, Millis(3));
        assert_eq!(
            cv.nodes[&node_id(2)].addresses,
            vec!["10.0.0.9:7433".to_string()]
        );
    }

    /// The view holds live runs plus what finished inside the tail, and nothing older. It held
    /// everything it was ever sent, and every probe carried the fleet's history (the phone at a
    /// quarter of a core idle, session ninety-two).
    #[test]
    fn a_run_finished_before_the_tail_is_forgotten_with_its_numbers() {
        let mut cv = ClusterView::new(node_id(1));
        let with_id = |b: u8, state: Option<crate::RunState>| {
            let mut run = a_run(node_id(1));
            run.id = RunId::from_bytes([b; 16]);
            if let Some(state) = state {
                run.state = state;
            }
            run
        };
        let live = with_id(1, None); // pending, as a new run is
        let recent = with_id(2, Some(crate::RunState::Completed { at: Millis(9_000) }));
        let old = with_id(
            3,
            Some(crate::RunState::Failed {
                at: Millis(1_000),
                reason: "x".into(),
            }),
        );
        for run in [&live, &recent, &old] {
            seed(&mut cv, run);
            cv.merge_progress(
                run.id,
                RunProgress {
                    turns: 1,
                    ..RunProgress::default()
                },
            );
        }

        assert_eq!(cv.forget_finished_before(Millis(5_000)), 1);
        assert!(
            cv.runs.contains_key(&live.id),
            "a live run stays whatever its age"
        );
        assert!(
            cv.runs.contains_key(&recent.id),
            "one that finished inside the tail stays"
        );
        assert!(
            !cv.runs.contains_key(&old.id),
            "one that finished before it goes"
        );
        assert!(
            !cv.progress.contains_key(&old.id),
            "and its numbers with it"
        );
        assert!(cv.progress.contains_key(&live.id));
        assert_eq!(
            cv.forget_finished_before(Millis(5_000)),
            0,
            "nothing more to forget"
        );
    }

    fn a_run(home: NodeId) -> crate::Run {
        crate::Run::new(
            RunId::from_bytes([1; 16]),
            crate::RunSpec {
                work: Work::Agent(AgentWork {
                    agent: crate::AgentKind::ClaudeCode,
                    model: None,
                    prompt: "p".into(),
                    workspace: crate::WorkspaceSpec {
                        repo: "/r".into(),
                        archive_bytes: None,
                        git_ref: None,
                        branch: None,
                    },
                    permission_mode: crate::PermissionMode::Ask,
                    allow: crate::ToolAllowlist::default(),
                    max_turns: None,
                    ask: crate::AskPolicy::Never,
                }),
                constraint: crate::Constraint::Always,
                restartability: crate::Restartability::Resumable,
                priority: 0,
                queue: false,
                deadline: None,
                demand: crate::Demand::Normal,
                notify: Default::default(),
                notices: Default::default(),
                resources: Vec::new(),
                prefer: crate::Constraint::Always,
                hold_until: None,
                parent: None,
            },
            home,
            Millis(0),
        )
    }

    fn view_of(b: u8) -> NodeView {
        NodeView::new(
            node_id(b),
            Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Desktop),
            WorkPolicy::for_class(DeviceClass::Desktop),
            Millis(0),
        )
    }

    /// The three answers, kept apart — because a caller that treats this as a `bool` gets the
    /// third one wrong in the direction of confidence. `offload schedules` did exactly that:
    /// `steward_of(home) == Some(me)` is false both when another node fires it and when
    /// **nothing does**, and the report said *"fired elsewhere"* for both.
    #[test]
    fn the_steward_is_the_home_while_the_home_is_here() {
        let mut cv = ClusterView::new(node_id(1));
        cv.upsert_node(view_of(1));
        cv.upsert_node(view_of(2));
        assert_eq!(cv.steward_of(node_id(2)), Some(node_id(2)));
    }

    #[test]
    fn a_home_that_is_gone_hands_the_tick_to_the_lowest_id_that_is_alive() {
        let mut cv = ClusterView::new(node_id(1));
        cv.upsert_node(view_of(1));
        cv.upsert_node(view_of(3));
        let mut home = view_of(2);
        home.status = NodeStatus::Dead;
        cv.upsert_node(home);
        assert_eq!(cv.steward_of(node_id(2)), Some(node_id(1)));
    }

    /// **The one that was rendered as somebody else's job.** A node that cannot see the home at
    /// all picks no successor — deliberately, since a daemon that has just started and met
    /// nobody must not begin firing the whole fleet's schedules. Nothing fires it, and that is
    /// a different sentence from "another machine has it".
    #[test]
    fn a_home_this_node_has_never_met_has_no_steward() {
        let mut cv = ClusterView::new(node_id(1));
        cv.upsert_node(view_of(1));
        assert_eq!(cv.steward_of(node_id(2)), None);
        assert_ne!(cv.steward_of(node_id(2)), Some(node_id(1)));
    }

    #[test]
    fn a_long_outage_decays_toward_the_normal_case() {
        // Moved here from `offload-store`, where it was testing a *second* copy of this average
        // (ADR-0031). One implementation, so one test: a 1/4-weighted EMA has a half-life of about
        // 2.4 samples, so a thirty-minute outlier takes roughly a dozen normal absences to stop
        // mattering. Asserting the property — monotonic decay toward the recent value — rather
        // than a guessed threshold is what makes it meaningful.
        let mut n = NodeView::new(
            NodeId::from_bytes([9; 32]),
            Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Laptop),
            WorkPolicy::for_class(DeviceClass::Laptop),
            Millis(0),
        );
        let mut clock = Millis(0);
        let away_for = |n: &mut NodeView, clock: &mut Millis, away: Millis| {
            n.set_status(NodeStatus::Suspect, *clock);
            *clock = *clock + away;
            n.set_status(NodeStatus::Alive, *clock);
        };

        away_for(&mut n, &mut clock, Millis::from_mins(30));
        assert_eq!(
            n.typical_absence,
            Some(Millis::from_mins(30)),
            "the first sample is the value"
        );

        let mut previous = n.typical_absence.expect("some");
        for _ in 0..12 {
            away_for(&mut n, &mut clock, Millis::from_secs(60));
            let current = n.typical_absence.expect("some");
            assert!(
                current < previous,
                "must decay, went {previous} -> {current}"
            );
            previous = current;
        }
        assert_eq!(n.absences, 13, "and every return is counted");
    }

    #[test]
    fn a_peers_absence_history_does_not_travel() {
        // ADR-0031. It is an observation — what *this* node saw of that peer — and an observation
        // cannot be relayed, which is the rule `fleet_events`, attendance and orphan status all
        // already follow. Gossiped, it had no owner and gave two answers: a peer learned
        // indirectly inherited the relayer's history wholesale, and one learned directly started
        // at zero, because a node's report about itself always carries zeros.
        let mut n = NodeView::new(
            NodeId::from_bytes([4; 32]),
            Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Laptop),
            WorkPolicy::for_class(DeviceClass::Laptop),
            Millis(0),
        );
        n.set_status(NodeStatus::Suspect, Millis(1_000));
        n.set_status(NodeStatus::Alive, Millis(4_000));
        assert_eq!(n.absences, 1);
        assert_eq!(n.typical_absence, Some(Millis(3_000)));

        let crossed: NodeView =
            serde_json::from_str(&serde_json::to_string(&n).expect("encode")).expect("decode");
        assert_eq!(crossed.absences, 0, "hearsay does not cross the wire");
        assert_eq!(crossed.typical_absence, None);
        // …and everything the peer *does* own still does, or this would be a different bug.
        assert_eq!(crossed.id, n.id);
        assert_eq!(crossed.status, n.status);
        assert_eq!(crossed.incarnation, n.incarnation);
    }

    #[test]
    fn a_remembered_history_seeds_a_peer_but_never_overwrites_a_live_one() {
        // The restart case, and the guard that makes it safe to apply on every tick: once
        // anything has been learned in this incarnation, a remembered row must not win.
        let fresh = || {
            NodeView::new(
                NodeId::from_bytes([5; 32]),
                Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Laptop),
                WorkPolicy::for_class(DeviceClass::Laptop),
                Millis(0),
            )
        };

        let mut n = fresh();
        assert!(n.seed_absence_history(3, Some(Millis::from_secs(90))));
        assert_eq!(n.absences, 3);
        assert_eq!(n.typical_absence, Some(Millis::from_secs(90)));
        // Idempotent, which is why no caller has to remember whether it has seeded this peer.
        assert!(!n.seed_absence_history(9, Some(Millis::from_secs(1))));
        assert_eq!(n.typical_absence, Some(Millis::from_secs(90)));

        // A live observation always wins over a remembered one.
        let mut live = fresh();
        live.set_status(NodeStatus::Suspect, Millis(0));
        live.set_status(NodeStatus::Alive, Millis(2_000));
        assert!(!live.seed_absence_history(3, Some(Millis::from_secs(90))));
        assert_eq!(live.typical_absence, Some(Millis(2_000)));

        // And an empty row seeds nothing rather than counting as "learned".
        let mut empty = fresh();
        assert!(!empty.seed_absence_history(0, None));
        assert!(empty.seed_absence_history(1, Some(Millis(5))));
    }

    #[test]
    fn higher_incarnation_wins_for_self_owned_facts() {
        let mut cv = ClusterView::new(node_id(1));
        cv.upsert_node(view_of(2));

        let mut newer = view_of(2);
        newer.incarnation = 5;
        newer.capabilities.cpu_cores = 32;
        assert!(cv.merge_node(newer, Millis(10)));
        assert_eq!(
            cv.node(&node_id(2)).expect("node").capabilities.cpu_cores,
            32
        );

        let mut older = view_of(2);
        older.incarnation = 1;
        older.capabilities.cpu_cores = 2;
        assert!(!cv.merge_node(older, Millis(20)));
        assert_eq!(
            cv.node(&node_id(2)).expect("node").capabilities.cpu_cores,
            32
        );
    }

    #[test]
    fn locally_observed_absence_history_survives_a_refutation() {
        let mut cv = ClusterView::new(node_id(1));
        cv.upsert_node(view_of(2));
        {
            let n = cv.nodes.get_mut(&node_id(2)).expect("node");
            n.set_status(NodeStatus::Suspect, Millis(1_000));
            n.set_status(NodeStatus::Alive, Millis(3_000));
        }

        let mut refutation = view_of(2);
        refutation.incarnation = 9;
        cv.merge_node(refutation, Millis(4_000));

        let n = cv.node(&node_id(2)).expect("node");
        assert_eq!(n.absences, 1);
        assert_eq!(n.typical_absence, Some(Millis(2_000)));
    }

    #[test]
    fn absence_average_smooths_towards_recent_samples() {
        let mut n = view_of(2);
        for (out, back) in [(0, 1_000), (2_000, 3_000), (4_000, 5_000)] {
            n.set_status(NodeStatus::Suspect, Millis(out));
            n.set_status(NodeStatus::Alive, Millis(back));
        }
        assert_eq!(n.absences, 3);
        assert_eq!(n.typical_absence, Some(Millis(1_000)));
    }

    #[test]
    fn arbitration_falls_over_to_a_deterministic_successor() {
        let mut cv = ClusterView::new(node_id(1));
        cv.upsert_node(view_of(1));
        cv.upsert_node(view_of(3));
        let mut home = view_of(7);
        cv.upsert_node(home.clone());

        let run = crate::Run::new(
            RunId::from_bytes([1; 16]),
            crate::RunSpec {
                work: Work::Agent(AgentWork {
                    agent: crate::AgentKind::ClaudeCode,
                    model: None,
                    prompt: "p".into(),
                    workspace: crate::WorkspaceSpec {
                        repo: "/r".into(),
                        archive_bytes: None,
                        git_ref: None,
                        branch: None,
                    },
                    permission_mode: crate::PermissionMode::Ask,
                    allow: crate::ToolAllowlist::default(),
                    max_turns: None,
                    ask: crate::AskPolicy::Never,
                }),
                constraint: crate::Constraint::Always,
                restartability: crate::Restartability::Resumable,
                priority: 0,
                queue: false,
                deadline: None,
                demand: crate::Demand::Normal,
                notify: Default::default(),
                notices: Default::default(),
                resources: Vec::new(),
                prefer: crate::Constraint::Always,
                hold_until: None,
                parent: None,
            },
            node_id(7),
            Millis(0),
        );

        assert_eq!(cv.arbiter_for(&run), Some(node_id(7)));

        // A suspicion does not move arbitration. The home node can refute it, and while it is
        // arguing it still believes it arbitrates — so failing over here would mean two
        // arbiters, two bid rounds, and one run granted twice.
        let mut suspected = home.clone();
        suspected.set_status(NodeStatus::Suspect, Millis(5));
        suspected.incarnation = 1;
        cv.merge_node(suspected, Millis(5));
        assert_eq!(cv.arbiter_for(&run), Some(node_id(7)));

        home.set_status(NodeStatus::Dead, Millis(10));
        home.incarnation = 2;
        cv.merge_node(home, Millis(0));

        // Every node computes the same successor from the same view.
        assert_eq!(cv.arbiter_for(&run), Some(node_id(1)));
        assert!(cv.is_local_arbiter(&run));
    }

    #[test]
    fn a_home_node_we_have_never_met_is_not_a_home_node_that_died() {
        // A node in its first second of gossip knows itself and little else. If "unknown"
        // failed over the same way "dead" does, it would appoint itself arbiter for every run
        // it heard about before it heard about the node that submitted them.
        let mut cv = ClusterView::new(node_id(1));
        cv.upsert_node(view_of(1));
        let run = a_run(node_id(7));

        assert_eq!(cv.arbiter_for(&run), None);
        assert!(!cv.is_local_arbiter(&run));

        cv.upsert_node(view_of(7));
        assert_eq!(cv.arbiter_for(&run), Some(node_id(7)));
    }

    #[test]
    fn a_relayed_copy_cannot_undo_an_orphan() {
        // Found by killing a node for real. `Orphaned` is not a lesser state than `Running`;
        // it is the arbiter's observation that the holder went quiet. A third node relaying
        // its own stale `Running` used to win the merge on progress rank alone — which pinned
        // the run to a machine that no longer existed, for as long as anybody remembered it
        // working.
        let arbiter = node_id(1);
        let holder = node_id(2);
        let bystander = node_id(3);

        let mut cv = ClusterView::new(arbiter);
        for id in [arbiter, holder, bystander] {
            let mut view = view_of(0);
            view.id = id;
            cv.upsert_node(view);
        }

        let mut run = a_run(arbiter);
        let epoch = run
            .assign(holder, Millis(0), Millis(60_000))
            .expect("assign");
        run.started(holder, epoch, Millis(0)).expect("start");
        seed(&mut cv, &run);

        let mut orphaned = run.clone();
        orphaned.orphan(Millis(1_000)).expect("orphan");
        assert!(
            cv.merge_run(orphaned, arbiter),
            "the arbiter's decision lands"
        );

        // The bystander still remembers it running, and says so on every probe.
        assert!(!cv.merge_run(run.clone(), bystander));
        assert_eq!(
            cv.runs.get(&run.id).expect("run").state.name(),
            "orphaned",
            "a peer relaying a stale copy is not the holder claiming to be alive"
        );

        // The holder itself is a different matter: that is the reclaim path, and it costs
        // nothing (ADR-0007).
        assert!(cv.merge_run(run.clone(), holder));
        assert_eq!(cv.runs.get(&run.id).expect("run").state.name(), "running");
    }

    #[test]
    fn a_checkpoint_travels_while_the_state_stays_the_same() {
        // Found by killing a node for real: everything a run *does* happens inside `Running`,
        // so a merge that required the state to advance dropped every checkpoint. The
        // reassignment then handed the next node a run with no conversation to continue, and
        // it started again from the prompt — three seconds after a checkpoint had been
        // replicated to it.
        let arbiter = node_id(1);
        let holder = node_id(2);
        let mut cv = ClusterView::new(arbiter);
        for id in [arbiter, holder] {
            let mut view = view_of(0);
            view.id = id;
            cv.upsert_node(view);
        }

        let mut run = a_run(arbiter);
        let epoch = run
            .assign(holder, Millis(0), Millis(60_000))
            .expect("assign");
        run.started(holder, epoch, Millis(0)).expect("start");
        seed(&mut cv, &run);

        let mut progressed = run.clone();
        progressed
            .record_checkpoint(holder, epoch, checkpoint_at(6))
            .expect("checkpoint");
        assert!(
            cv.merge_run(progressed, holder),
            "the holder's own progress"
        );
        assert_eq!(
            cv.runs
                .get(&run.id)
                .expect("run")
                .checkpoint
                .as_ref()
                .expect("checkpoint")
                .turns,
            6
        );

        // A bystander's copy of the older record still loses.
        assert!(!cv.merge_run(run, node_id(3)));
    }

    #[test]
    fn only_the_arbiter_may_declare_a_run_orphaned() {
        // Otherwise any node that briefly lost contact could take a run away from a holder
        // that is answering everybody else perfectly well.
        let arbiter = node_id(1);
        let holder = node_id(2);
        let bystander = node_id(3);

        let mut cv = ClusterView::new(bystander);
        for id in [arbiter, holder, bystander] {
            let mut view = view_of(0);
            view.id = id;
            cv.upsert_node(view);
        }

        let mut run = a_run(arbiter);
        let epoch = run
            .assign(holder, Millis(0), Millis(60_000))
            .expect("assign");
        run.started(holder, epoch, Millis(0)).expect("start");
        cv.merge_run(run.clone(), holder);

        let mut orphaned = run.clone();
        orphaned.orphan(Millis(1_000)).expect("orphan");
        assert!(!cv.merge_run(orphaned.clone(), bystander));
        assert_eq!(cv.runs.get(&run.id).expect("run").state.name(), "running");

        assert!(cv.merge_run(orphaned, arbiter));
        assert_eq!(cv.runs.get(&run.id).expect("run").state.name(), "orphaned");
    }

    /// The holder is the one party that does not take the arbiter's word about it.
    ///
    /// Found by the churn simulation, and it costs the run everything. Two missed probes — a
    /// Wi-Fi handover — and the arbiter suspects a perfectly healthy holder and gossips
    /// `Orphaned`. The holder used to *accept that about itself*, which leaves it with no lease
    /// (`Orphaned` returns none by design), so it can no longer renew, and when its agent
    /// finishes `complete` is fenced out and the work is not recorded anywhere. The run had
    /// nothing wrong with it and neither did the node.
    ///
    /// `merge_node` has had this rule from the start, in as many words — "our own entry is
    /// ours… never by believing it about ourselves" — and answers a suspicion by refuting it.
    /// This is the same sentence about the run record, which is where it decides whether work
    /// counts. The refutation is the reclaim path from the other end: the holder keeps its
    /// `Running`, gossips it, and the arbiter takes it back.
    #[test]
    fn a_holder_does_not_believe_a_peer_about_its_own_absence() {
        let arbiter = node_id(1);
        let holder = node_id(2);

        // The view belongs to the *holder*, which is the whole point.
        let mut cv = ClusterView::new(holder);
        for id in [arbiter, holder] {
            let mut view = view_of(0);
            view.id = id;
            cv.upsert_node(view);
        }

        let mut run = a_run(arbiter);
        let epoch = run
            .assign(holder, Millis(0), Millis(60_000))
            .expect("assign");
        run.started(holder, epoch, Millis(0)).expect("start");
        cv.merge_run(run.clone(), holder);

        let mut orphaned = run.clone();
        orphaned.orphan(Millis(1_000)).expect("orphan");
        assert!(
            !cv.merge_run(orphaned.clone(), arbiter),
            "the holder took its arbiter's word for its own absence"
        );

        let mine = cv.runs.get(&run.id).expect("run").clone();
        assert_eq!(mine.state.name(), "running");
        // The two things being orphaned here would have cost: the lease it renews by, and the
        // right to say the agent finished.
        assert!(mine.state.lease().is_some(), "it kept no lease to renew");
        let mut mine = mine;
        assert!(
            mine.complete(holder, epoch, Millis(2_000)).is_ok(),
            "a healthy run could not report that it had finished"
        );

        // A node that is *not* holding it still believes the arbiter, which is the rule this
        // one narrows rather than replaces.
        let mut bystander = ClusterView::new(node_id(3));
        for id in [arbiter, holder, node_id(3)] {
            let mut view = view_of(0);
            view.id = id;
            bystander.upsert_node(view);
        }
        bystander.merge_run(run, holder);
        assert!(bystander.merge_run(orphaned, arbiter));
        assert_eq!(
            bystander.runs.values().next().expect("run").state.name(),
            "orphaned"
        );
    }

    #[test]
    fn an_edited_deadline_outlives_the_holder_still_gossiping_the_old_one() {
        // The reason a mutable spec field needs a rule of its own. The holder is believed
        // about a run it is running — that is what makes turns and checkpoints travel — so it
        // would put its copy of the deadline back every second, and `offload deadline` would
        // appear to work and then quietly undo itself.
        let home = node_id(1);
        let holder = node_id(2);
        let mut cv = ClusterView::new(home);
        for id in [home, holder] {
            let mut view = view_of(0);
            view.id = id;
            cv.upsert_node(view);
        }

        let mut run = a_run(home);
        let epoch = run
            .assign(holder, Millis(0), Millis(60_000))
            .expect("assign");
        run.started(holder, epoch, Millis(0)).expect("start");
        let stale = run.clone();

        run.edit(crate::SpecEdit::Deadline {
            at: Some(Millis::from_mins(30)),
        });
        // Applied at home rather than merged: an edit is written to the owner's own row
        // (`Host::record_spec_edit`), and gossip is how the *holder* hears about it.
        seed(&mut cv, &run);

        assert!(
            !cv.merge_run(stale.clone(), holder),
            "the holder's record is behind on the one field it does not own"
        );
        let held = cv.runs.get(&stale.id).expect("run");
        assert_eq!(held.spec.deadline, Some(Millis::from_mins(30)));
        assert_eq!(held.spec_rev, 1);
        assert_eq!(held.state.name(), "running", "and it lost nothing else");
    }

    #[test]
    fn the_node_holding_a_run_takes_the_edit_and_nothing_else() {
        // What `merge_spec_edit` is for. The holder must not take a peer's word about the
        // record — a stale copy would undo a turn — and must take it about the deadline, which
        // belongs to the run's home node and travels no other way. So the same arrival is
        // refused as a record and accepted as an edit.
        let home = node_id(1);
        let holder = node_id(2);
        let mut cv = ClusterView::new(holder);
        for id in [home, holder] {
            let mut view = view_of(0);
            view.id = id;
            cv.upsert_node(view);
        }

        let mut run = a_run(home);
        let epoch = run
            .assign(holder, Millis(0), Millis(60_000))
            .expect("assign");
        let before_it_started = run.clone();
        run.started(holder, epoch, Millis(0)).expect("start");
        cv.runs.insert(run.id, run.clone());

        // What the home node gossips: a turn behind on the record, a revision ahead on the
        // spec. Both at once, because that is what one gossip tick looks like.
        let mut theirs = before_it_started;
        theirs.edit(crate::SpecEdit::Priority { to: 7 });

        assert!(cv.merge_spec_edit(&theirs));
        let ours = cv.runs.get(&run.id).expect("run");
        assert_eq!(ours.spec.priority, 7, "the edit arrived");
        assert_eq!(ours.spec_rev, 1);
        assert_eq!(
            ours.state.name(),
            "running",
            "and took nothing else with it"
        );
        assert_eq!(ours.holder(), Some(holder));

        // Idempotent, which matters because the home node re-gossips it once a second: a
        // second arrival of the same revision is not news, and news is what gets written down.
        assert!(!cv.merge_spec_edit(&theirs));
        // And an older revision never wins, in this direction either.
        let mut older = theirs.clone();
        older.spec_rev = 0;
        older.spec.priority = 0;
        assert!(!cv.merge_spec_edit(&older));
        assert_eq!(cv.runs.get(&run.id).expect("run").spec.priority, 7);
    }

    #[test]
    fn two_edits_travel_as_one_revision_of_the_spec() {
        // One counter for both editable fields (`SpecEdit`), which only works because they have
        // the same owner: a record at revision N carries the spec as it stood after N edits,
        // whichever fields those touched. A record that has heard about the deadline but not the
        // priority does not exist, so nothing has to merge one.
        let home = node_id(1);
        let holder = node_id(2);
        let mut cv = ClusterView::new(home);
        for id in [home, holder] {
            let mut view = view_of(0);
            view.id = id;
            cv.upsert_node(view);
        }

        let mut run = a_run(home);
        let epoch = run
            .assign(holder, Millis(0), Millis(60_000))
            .expect("assign");
        run.started(holder, epoch, Millis(0)).expect("start");
        let stale = run.clone();

        assert_eq!(
            run.edit(crate::SpecEdit::Deadline {
                at: Some(Millis(1_000))
            }),
            1
        );
        assert_eq!(run.edit(crate::SpecEdit::Priority { to: 10 }), 2);
        seed(&mut cv, &run);

        // The holder re-asserts the spec it started with, once a second, for ever.
        assert!(!cv.merge_run(stale.clone(), holder));
        let held = cv.runs.get(&stale.id).expect("run");
        assert_eq!(held.spec.deadline, Some(Millis(1_000)));
        assert_eq!(held.spec.priority, 10);
        assert_eq!(held.spec_rev, 2);
    }

    #[test]
    fn a_node_learning_the_new_deadline_second_hand_keeps_it() {
        // A bystander relays what it has, so the edit has to survive arriving *on* a record
        // that loses — and has to survive arriving on one that wins, without dragging an old
        // deadline along with it.
        let home = node_id(1);
        let holder = node_id(2);
        let bystander = node_id(3);
        let mut cv = ClusterView::new(bystander);
        for id in [home, holder, bystander] {
            let mut view = view_of(0);
            view.id = id;
            cv.upsert_node(view);
        }

        let mut run = a_run(home);
        let epoch = run
            .assign(holder, Millis(0), Millis(60_000))
            .expect("assign");
        run.started(holder, epoch, Millis(0)).expect("start");
        cv.merge_run(run.clone(), holder);

        // Only the edit reaches us, on an otherwise identical record.
        let mut edited = run.clone();
        edited.edit(crate::SpecEdit::Deadline {
            at: Some(Millis::from_mins(30)),
        });
        assert!(cv.merge_run(edited, home), "an edit is a change");
        assert_eq!(
            cv.runs.get(&run.id).expect("run").spec.deadline,
            Some(Millis::from_mins(30))
        );

        // Then the holder moves the run on, carrying the deadline it still believes in.
        let mut finished = run;
        finished
            .complete(holder, epoch, Millis(5_000))
            .expect("complete");
        assert!(cv.merge_run(finished.clone(), holder));
        let held = cv.runs.get(&finished.id).expect("run");
        assert_eq!(held.state.name(), "completed");
        assert_eq!(held.spec.deadline, Some(Millis::from_mins(30)));

        // And the consequence for whoever persists this: **the settled record is not the
        // record that arrived**. A caller that writes the arrival down instead leaves its store
        // a revision behind its view, which nothing heals — the store is what a restart
        // believes, and a copy that loses every merge never teaches anybody anything.
        assert_ne!(
            *held, finished,
            "the arrival lost its spec to the merge; it is not what to write down"
        );
    }

    #[test]
    fn a_finished_run_is_not_reopened_by_anybody_at_the_same_epoch() {
        let arbiter = node_id(1);
        let holder = node_id(2);
        let mut cv = ClusterView::new(arbiter);
        for id in [arbiter, holder] {
            let mut view = view_of(0);
            view.id = id;
            cv.upsert_node(view);
        }

        let mut run = a_run(arbiter);
        let epoch = run
            .assign(holder, Millis(0), Millis(60_000))
            .expect("assign");
        run.started(holder, epoch, Millis(0)).expect("start");
        let running = run.clone();
        run.complete(holder, epoch, Millis(5_000))
            .expect("complete");
        seed(&mut cv, &run);

        assert!(!cv.merge_run(running, holder));
        assert_eq!(
            cv.runs.values().next().expect("run").state.name(),
            "completed"
        );
    }

    /// A node does not learn of its own run from somebody else (ADR-0021 §7).
    ///
    /// The hole ADR-0021 opened, and it needs no partition — only a lid. A laptop sees an
    /// occurrence while it is `Running` and closes; the rule's node finishes it, and five quiet
    /// minutes later prunes the record; the laptop opens and gossips the copy it still holds.
    /// Taken as news, that is a run the home node has never heard of, held by *itself*, at an
    /// epoch nothing can contradict and with no agent behind it: `heartbeat` renews the lease
    /// for ever, `ps` reports last night's finished work as running, and a restart offers it to
    /// somebody as resumable. The sibling rule one function up already says a node must not
    /// believe a peer about its own absence; this says the same about its own runs existing.
    #[test]
    fn a_pruned_record_is_not_taught_back_by_a_peer_that_was_away() {
        let home = node_id(1);
        let holder = node_id(2);
        let mut cv = ClusterView::new(home);
        for id in [home, holder] {
            let mut view = view_of(0);
            view.id = id;
            cv.upsert_node(view);
        }

        let mut run = a_run(home);
        let epoch = run
            .assign(holder, Millis(0), Millis(60_000))
            .expect("assign");
        run.started(holder, epoch, Millis(0)).expect("start");

        assert!(
            !cv.merge_run(run.clone(), holder),
            "our own run, which we do not have, is a memory of somebody else's rather than news"
        );
        assert!(cv.runs.is_empty(), "and nothing was written down");

        // The rule is about *this node's* runs and nothing else: a peer's run this node has
        // never seen is exactly what gossip is for, and refusing it would leave a node that
        // joined mid-flight unable to learn the fleet's work at all.
        let mut theirs = a_run(holder);
        theirs.id = RunId::from_bytes([2; 16]);
        assert!(cv.merge_run(theirs, holder), "a peer's run is still news");
        assert_eq!(cv.runs.len(), 1);
    }

    /// Two arbiters, one epoch, two holders — and every node has to pick the same one.
    ///
    /// Reachable without a partition: a node that concludes the home node dead grants the run,
    /// then hears the refutation. Both grants were made from the same epoch, so `next()` gave
    /// them the same number and `fence` calls neither stale. Both records claim to come from
    /// their own holder, so before this the answer was "whoever we heard from last" — a coin
    /// re-flipped every gossip tick, with two agents running on one repository throughout and
    /// neither one ever told.
    #[test]
    fn two_grants_at_one_epoch_settle_the_same_way_on_every_node() {
        let home = node_id(1);
        let x = node_id(2);
        let y = node_id(3);
        let mut base = ClusterView::new(home);
        for id in [home, x, y, node_id(4)] {
            let mut view = view_of(0);
            view.id = id;
            base.upsert_node(view);
        }

        let mut run = a_run(home);
        let e = run.assign(x, Millis(0), Millis(60_000)).expect("assign");
        run.started(x, e, Millis(0)).expect("start");
        run.orphan(Millis(1_000)).expect("orphan");
        seed(&mut base, &run);

        // The home node grants it to x; a successor that thought home was gone grants it to y.
        // Same base epoch, so the same number.
        let to_x = {
            let mut r = run.clone();
            let e = r.assign(x, Millis(2_000), Millis(60_000)).expect("x");
            r.started(x, e, Millis(2_000)).expect("start x");
            r
        };
        let to_y = {
            let mut r = run.clone();
            let e = r.assign(y, Millis(2_000), Millis(60_000)).expect("y");
            r.started(y, e, Millis(2_000)).expect("start y");
            r
        };
        assert_eq!(to_x.epoch, to_y.epoch, "two arbiters, one token");

        // Whichever order they arrive, and whoever relays them, the same grant survives — the
        // lowest holder id, which every node can compute from the record with no message.
        for (first, second, from_first, from_second) in [
            (&to_x, &to_y, x, y),
            (&to_y, &to_x, y, x),
            (&to_x, &to_y, node_id(4), node_id(4)),
            (&to_y, &to_x, node_id(4), node_id(4)),
        ] {
            let mut cv = base.clone();
            cv.merge_run(first.clone(), from_first);
            cv.merge_run(second.clone(), from_second);
            assert_eq!(
                cv.runs.values().next().expect("run").holder(),
                Some(x),
                "the fleet disagreed about which grant won"
            );
        }
    }

    #[test]
    fn a_suspicion_spreads_without_the_subject_taking_part() {
        // The bug this rule exists to prevent: suspecting a node does not change that node's
        // incarnation, because the node is not involved in the claim. Arbitrating liveness on
        // incarnation alone means `Suspect` never leaves the node that noticed.
        let mut cv = ClusterView::new(node_id(1));
        cv.upsert_node(view_of(2));

        let mut suspicious = view_of(2);
        suspicious.status = NodeStatus::Suspect;
        assert!(cv.merge_node(suspicious, Millis(1_000)));
        assert_eq!(
            cv.node(&node_id(2)).expect("node").status,
            NodeStatus::Suspect
        );
    }

    #[test]
    fn stronger_evidence_wins_at_equal_incarnation_and_weaker_does_not() {
        let mut cv = ClusterView::new(node_id(1));
        cv.upsert_node(view_of(2));

        let mut dead = view_of(2);
        dead.status = NodeStatus::Dead;
        assert!(cv.merge_node(dead, Millis(1_000)));

        // A peer that has not heard the bad news does not undo it: at equal incarnation
        // `Alive` is the weakest claim there is, and only the node itself can refute.
        let alive = view_of(2);
        assert!(!cv.merge_node(alive, Millis(2_000)));
        assert_eq!(cv.node(&node_id(2)).expect("node").status, NodeStatus::Dead);
    }

    #[test]
    fn a_node_that_comes_back_is_believed_over_a_peers_confirmation() {
        // A departure from textbook SWIM, on purpose: `Dead` is terminal there, and here the
        // usual cause of "unreachable" is a laptop in a bag (ADR-0007).
        let mut cv = ClusterView::new(node_id(1));
        cv.upsert_node(view_of(2));
        let mut dead = view_of(2);
        dead.status = NodeStatus::Dead;
        cv.merge_node(dead, Millis(1_000));

        let mut back = view_of(2);
        back.incarnation = 1;
        assert!(cv.merge_node(back, Millis(5_000)));
        assert_eq!(
            cv.node(&node_id(2)).expect("node").status,
            NodeStatus::Alive
        );

        // And the absence is on its record, which is what the hold-down policy reads.
        assert_eq!(cv.node(&node_id(2)).expect("node").absences, 1);
    }

    #[test]
    fn nobody_gets_to_tell_us_what_we_are() {
        // Believing a peer's suspicion about ourselves would mean a node could be talked out
        // of its own liveness. The answer is a refutation, not a merge — and this test used to
        // assert the *absence* of the refutation its own sentence names: `incarnation, 0`, which
        // is a node that will never be believed again, because at equal incarnation a worse claim
        // wins and above a peer's number we never went.
        let mut cv = ClusterView::new(node_id(1));
        cv.upsert_node(view_of(1));

        let mut about_us = view_of(1);
        about_us.status = NodeStatus::Dead;
        about_us.incarnation = 99;
        assert!(cv.merge_node(about_us, Millis(1_000)), "it is answered");

        let me = cv.node(&node_id(1)).expect("node");
        assert_eq!(me.status, NodeStatus::Alive, "and not believed");
        assert!(
            me.incarnation > 99,
            "the answer has to outrank what the peer is holding, or it cannot travel: {}",
            me.incarnation
        );

        // A peer echoing our own number back is the ordinary case and changes nothing — bumping
        // on every gossip tick would make our own entry churn for ever.
        let settled = me.incarnation;
        let mut echo = view_of(1);
        echo.incarnation = settled;
        assert!(!cv.merge_node(echo, Millis(2_000)));
        assert_eq!(
            cv.node(&node_id(1)).expect("node").incarnation,
            settled,
            "agreement is not something to argue with"
        );

        // And the case that needs no suspicion at all: a peer holding a *previous life's* facts
        // at the same number. Every restart looks like this — an incarnation lives in memory, so
        // a node begins again at 0 — and the merge copies nothing at equal incarnations, so what
        // the fleet knows about a restarted node's capabilities is whatever it knew before.
        let mut stale = view_of(1);
        stale.incarnation = settled;
        stale.capabilities.cpu_cores += 7;
        assert!(
            cv.merge_node(stale, Millis(3_000)),
            "a peer holding facts that are not ours has to be outranked"
        );
        assert!(cv.node(&node_id(1)).expect("node").incarnation > settled);
        assert_eq!(
            cv.node(&node_id(1)).expect("node").capabilities.cpu_cores,
            view_of(1).capabilities.cpu_cores,
            "and its facts are still not adopted"
        );
    }

    #[test]
    fn refuting_outranks_the_suspicion_it_answers() {
        let mut cv = ClusterView::new(node_id(1));
        cv.upsert_node(view_of(1));
        assert_eq!(cv.refute(Millis(1_000)), 1);

        // What a peer holding the suspicion does with the refutation.
        let mut theirs = ClusterView::new(node_id(2));
        let mut suspected = view_of(1);
        suspected.status = NodeStatus::Suspect;
        theirs.upsert_node(suspected);

        let refutation = cv.node(&node_id(1)).expect("node").clone();
        assert!(theirs.merge_node(refutation, Millis(2_000)));
        assert_eq!(
            theirs.node(&node_id(1)).expect("node").status,
            NodeStatus::Alive
        );
    }

    #[test]
    fn a_departure_outranks_a_guess_about_why_a_node_went_quiet() {
        // `Draining` reached us because the node said so; `Suspect` is a peer guessing.
        let mut cv = ClusterView::new(node_id(1));
        cv.upsert_node(view_of(2));
        let mut suspect = view_of(2);
        suspect.status = NodeStatus::Suspect;
        cv.merge_node(suspect, Millis(1_000));

        let mut draining = view_of(2);
        draining.status = NodeStatus::Draining;
        assert!(cv.merge_node(draining, Millis(1_100)));
        assert_eq!(
            cv.node(&node_id(2)).expect("node").status,
            NodeStatus::Draining
        );
    }

    /// A node signed into `account`, claiming `limit` runs for it (or claiming nothing).
    fn on_account(b: u8, account: &str, limit: Option<u32>) -> NodeView {
        let mut caps = Capabilities::empty(
            crate::capability::Os::Linux,
            crate::capability::Arch::X86_64,
            crate::capability::DeviceClass::Desktop,
        );
        caps.add(
            crate::capability::Capability::agent(
                crate::AgentKind::ClaudeCode,
                crate::capability::AgentDetails {
                    version: "2.10.0".into(),
                    models: Vec::new(),
                    max_concurrent: 4,
                },
                true,
            )
            .with_identity(Some(crate::capability::AccountId(account.into()))),
        );
        let mut policy = WorkPolicy::for_class(crate::capability::DeviceClass::Desktop);
        policy.max_concurrent_account = limit;
        NodeView::new(node_id(b), caps, policy, Millis(0))
    }

    /// One run of `id`, held and running on `holder`.
    fn running_on(view: &mut ClusterView, id: u8, holder: NodeId) {
        let mut run = a_run(holder);
        run.id = RunId::from_bytes([id; 16]);
        let epoch = run
            .assign(holder, Millis(0), Millis::from_secs(60))
            .expect("assign");
        run.started(holder, epoch, Millis(0)).expect("start");
        view.runs.insert(run.id, run);
    }

    #[test]
    fn an_accounts_runs_are_counted_across_every_node_that_shares_it() {
        // The fact the whole cap rests on: two machines, one login, one rate limit. Counting
        // per node would see one run each and conclude the account is idle.
        let mut view = ClusterView::new(node_id(1));
        view.upsert_node(on_account(1, "acct:one", Some(3)));
        view.upsert_node(on_account(2, "acct:one", Some(3)));
        view.upsert_node(on_account(3, "acct:other", Some(3)));
        running_on(&mut view, 10, node_id(1));
        running_on(&mut view, 11, node_id(2));
        running_on(&mut view, 12, node_id(3));

        // One each here and on the peer: what node 1 is told is the *peer's*, because its own is
        // a question its own store answers exactly (`AccountUse::elsewhere`).
        let use_of_one = view.account_use(&node_id(1), &crate::AgentKind::ClaudeCode);
        assert_eq!(use_of_one.map(|u| u.elsewhere), Some(1));

        // The third node is on its own account, so nothing on the shared one counts for it.
        let other = view.account_use(&node_id(3), &crate::AgentKind::ClaudeCode);
        assert_eq!(other.map(|u| u.elsewhere), Some(0));
    }

    #[test]
    fn the_lowest_claimed_ceiling_is_the_one_that_binds() {
        // A node must not be able to raise the fleet's limit by claiming more for itself, which
        // is what a maximum or an average would let it do. And where two nodes disagree about
        // an account's rate limit, the cautious one is the one to believe.
        let mut view = ClusterView::new(node_id(1));
        view.upsert_node(on_account(1, "acct:one", Some(9)));
        view.upsert_node(on_account(2, "acct:one", Some(2)));

        assert_eq!(
            view.account_use(&node_id(1), &crate::AgentKind::ClaudeCode)
                .map(|u| u.limit),
            Some(2)
        );
    }

    #[test]
    fn an_account_nobody_put_a_ceiling_on_has_none() {
        // `None` rather than a number, so `admits` skips the rule instead of enforcing a zero.
        let mut view = ClusterView::new(node_id(1));
        view.upsert_node(on_account(1, "acct:one", None));
        view.upsert_node(on_account(2, "acct:one", None));

        assert_eq!(
            view.account_use(&node_id(1), &crate::AgentKind::ClaudeCode),
            None
        );
    }

    #[test]
    fn a_node_with_no_comparable_account_is_capped_by_nothing() {
        // An unauthenticated agent, or one whose account could not be fingerprinted, has no
        // account to share — and must not be lumped in with somebody else's.
        let mut view = ClusterView::new(node_id(1));
        let mut caps = Capabilities::empty(
            crate::capability::Os::Linux,
            crate::capability::Arch::X86_64,
            crate::capability::DeviceClass::Desktop,
        );
        caps.add(crate::capability::Capability::agent(
            crate::AgentKind::ClaudeCode,
            crate::capability::AgentDetails {
                version: "2.10.0".into(),
                models: Vec::new(),
                max_concurrent: 4,
            },
            true,
        ));
        let mut policy = WorkPolicy::for_class(crate::capability::DeviceClass::Desktop);
        policy.max_concurrent_account = Some(2);
        view.upsert_node(NodeView::new(node_id(1), caps, policy, Millis(0)));

        assert_eq!(
            view.account_use(&node_id(1), &crate::AgentKind::ClaudeCode),
            None
        );
    }
}
