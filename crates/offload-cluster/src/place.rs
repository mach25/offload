//! One bid round: ask the fleet, pick a winner, hand the run over.
//!
//! ADR-0006 in the form the transport makes possible. Nodes decide *locally* whether they
//! want a run — owner policy, then eligibility, then a self-score using facts gossip cannot
//! carry — and the arbiter only picks among self-assessments. Two departures from the ADR's
//! letter, both because there is now a transport that can address a peer:
//!
//! * **The arbiter asks; nodes do not broadcast.** The score-proportional delay existed to
//!   keep a broadcast from becoming a storm. Asking directly costs the same messages, needs
//!   no delay, and lets the round be bounded by the asker rather than by a timer everybody
//!   has to agree on.
//! * **Only the arbiter needs the bids.** The ADR broadcasts so that any node could compute
//!   the same winner. Arbiter failover does not need that: the successor runs its own round
//!   rather than finishing somebody else's, and a round is cheap. `winner()` is deterministic
//!   and lives in core, so the bids can travel again if that ever stops being true.
//!
//! What is *not* a departure: a grant is an offer, and the granted node may still decline
//! (step 6). A declined grant never strands the run — the next-best bid is tried, and a node
//! that declined is not reconsidered in this round.

use crate::Cluster;
use async_trait::async_trait;
use offload_core::{Availability, Bid, Grant, NodeId, Offer, Run, LEASE};
use offload_proto::cluster::ClusterMessage;

/// What this node can say about hosting a run.
///
/// Implemented by the daemon, because everything it answers with — the workspace being warm,
/// the load right now, the battery, whether the repo is even obtainable here — is knowable
/// only where the run would actually go.
#[async_trait]
pub trait Host: Send + Sync + 'static {
    /// Would this node take the run, how keenly, and could it start now? `Err` is a sentence
    /// for a human, not a code.
    async fn evaluate(&self, run: &Run) -> Result<Offer, String>;

    /// Take it. The run arrives already assigned to this node at a fresh epoch, so this is
    /// the point of no return: after `Ok`, the fleet believes the run is here.
    async fn accept(&self, run: Run) -> Result<(), String>;

    /// This node, arbitrating, has granted the run — possibly to itself.
    ///
    /// Called **once per grant**, from the confirmed branch of the round and from the local
    /// branch, which is the whole reason it is not [`Host::record`]. Those are two facts and one
    /// hook used to carry both: "I made this decision, under this epoch" happens once, while "I
    /// have a record of a run living elsewhere" happens on **every gossip merge** that moves the
    /// record. Sharing them made an audit row that is supposed to be the durable evidence of one
    /// arbiter spending a token twice appear three times for one ordinary run — and never at all
    /// when the arbiter granted to itself, because that path returns before `record` is reached.
    /// Over-reported where it must be exact and missing in the commonest case, from one hook.
    async fn granted(&self, _run: &Run) {}

    /// Remember a run that lives somewhere else.
    ///
    /// Two callers, and the second is the reason this cannot also mean "I granted it": the
    /// arbiter calls it once a peer has taken the run, because a record held only in view memory
    /// is gone at the next restart and the node that submitted a run is exactly the one about to
    /// be shut (ADR-0006) — and `Cluster::learn` calls it for **every** record a gossip merge
    /// moved, which is how a node learns about work it has nothing to do with.
    async fn record(&self, _run: &Run) {}

    /// A peer is still describing `run` as live, and this node's copy says it finished. Tell it
    /// again: the peer was away when the news went round and will not hear it any other way,
    /// since a finished run stops being gossiped once it has been told (session ninety-four).
    async fn stale_copy(&self, _run: offload_core::RunId) {}

    /// Write down a schedule a gossip merge changed (ADR-0019 §3, ADR-0056).
    ///
    /// [`Host::record`]'s sibling, and the same argument: a schedule held only in the cluster's
    /// gossip copy is gone at the next restart, and the whole reason a schedule is gossiped at
    /// all is that it must outlive the device that created it.
    ///
    /// Called only for what *changed* — a schedule this node had not heard of, or a tombstone
    /// for one it had — so it is not a per-probe write. The merge rule is the domain's
    /// (`Schedule::merge`) and the store applies it again on the way in, which is deliberate
    /// belt and braces: this hook and the store's own writer are reached from two directions.
    async fn record_schedule(&self, _schedule: &offload_core::Schedule) {}

    /// Take an operator's edit to a run **this node is holding**.
    ///
    /// The other half of [`Host::record`], and deliberately not the same call. A peer's record
    /// of a run held here is refused — our own copy is newer about the state, the epoch, the
    /// lease and the checkpoint — while the deadline and the priority on it were never ours
    /// (`SpecEdit`). So what crosses is the edit, and what applies it is a read-modify-write on
    /// the node's own copy: writing the record whole would undo whatever the run did between
    /// the last gossip tick and now.
    async fn record_spec_edit(&self, _run: &Run) {}

    /// Remember what a run elsewhere has done and cost.
    ///
    /// Separate from [`Host::record`] because the two arrive independently: a run's record
    /// moves when a decision is taken about it, and its numbers move on every turn it runs.
    async fn record_progress(
        &self,
        _run: offload_core::RunId,
        _progress: &offload_core::RunProgress,
    ) {
    }

    /// This run's event log, from after `after`, and whether this node has more to come.
    ///
    /// Asked by a peer whose operator is following a run that lives here. `peer` is passed
    /// because being asked *is* the observation that somebody is watching (ADR-0013): attendance
    /// is only ever knowable where the stream is served, and a remote follower is a peer that
    /// keeps asking.
    async fn events_since(
        &self,
        _peer: NodeId,
        _run: offload_core::RunId,
        _after: u64,
        _limit: u32,
    ) -> Result<(Vec<offload_proto::cluster::SeqEvent>, bool), String> {
        Err("this node keeps no run logs".into())
    }

    /// What is at `path` in a run's workspace here, read-only (ADR-0075).
    async fn files(
        &self,
        _peer: NodeId,
        _run: offload_core::RunId,
        _path: &str,
    ) -> Result<offload_proto::cluster::FilesView, String> {
        Err("this node keeps no workspaces".into())
    }

    /// Change the editable part of a run this node *owns* — its deadline or its priority — and
    /// say what that means for the run.
    ///
    /// Not about hosting: the owner is the run's home node (ADR-0013), which is usually not
    /// the node running it. It is here because this trait is where the cluster reaches the
    /// daemon's store, and a second trait for one method would be ceremony.
    async fn edit_spec(
        &self,
        _run: offload_core::RunId,
        _edit: offload_core::SpecEdit,
    ) -> Result<(u32, String), String> {
        Err("this node does not keep run records".into())
    }

    /// Stop a run that is *here*, because somebody typed `offload cancel` somewhere else.
    ///
    /// On this trait for the same reason [`Host::answer`] is: a run is on exactly one machine,
    /// and the node that can stop it is that one. What comes back is a sentence rather than a
    /// bool — a mid-turn agent and a commitment that never started are both "cancelled", and
    /// only one of them cost anything.
    async fn cancel(&self, _run: offload_core::RunId, _by: &str) -> Result<String, String> {
        Err("this node hosts no runs, so nothing here can be stopped".into())
    }

    /// Build the base for a continuation of a finished run whose last leg ran *here*, because
    /// somebody typed `offload continue` on another node (ADR-0064 §2).
    ///
    /// On this trait for [`Host::cancel`]'s reason, about a directory rather than a process: the
    /// parent's worktree is on one machine, and a capture of it can only be taken there.
    async fn continuation_base(
        &self,
        _run: offload_core::RunId,
        _session: bool,
    ) -> Result<offload_core::ContinuationBase, String> {
        Err("this node hosts no runs, so no run's workspace is here".into())
    }

    /// Capture a run that is *here* at its next turn boundary and hand it back to the pool.
    ///
    /// [`Host::cancel`]'s sibling with the record half removed: what is being asked for is a
    /// pause in a process, at a moment only that process reaches, so there is no version of this
    /// that a node not running the agent could answer.
    async fn request_checkpoint(&self, _run: offload_core::RunId) -> Result<(), String> {
        Err("this node hosts no runs, so nothing here reaches a turn boundary".into())
    }

    /// Answer a question an agent *here* is blocked on (ADR-0017).
    ///
    /// On this trait rather than a registration of its own, unlike delivery: the node that can
    /// answer is by definition the node running the agent, so there is no interesting device
    /// that implements this and hosts nothing. Returns what was decided, said back, because an
    /// agent can be blocked on several calls at once.
    async fn answer(
        &self,
        _run: offload_core::RunId,
        _tool_use_id: Option<String>,
        _allow: bool,
        _by: &str,
    ) -> Result<(String, String, bool), String> {
        Err("this node runs no agents, so nothing here is waiting".into())
    }

    /// What this node's agents are waiting for, right now.
    ///
    /// Asked rather than gossiped: a blocked process stops being blocked the moment somebody
    /// answers, and a remembered copy shown as current is the same wrong answer a replayed bid
    /// would be.
    async fn pending_asks(&self) -> Vec<offload_core::PendingAsk> {
        Vec::new()
    }
}

/// A member that answers "not me" to everything.
///
/// The right answer for a node that hosts nothing — a phone whose only capability is
/// delivering notifications is a full member of the fleet and will never take a run.
#[derive(Debug)]
pub struct NoHost;

#[async_trait]
impl Host for NoHost {
    async fn evaluate(&self, _run: &Run) -> Result<Offer, String> {
        Err("this node does not host runs".into())
    }

    async fn accept(&self, _run: Run) -> Result<(), String> {
        Err("this node does not host runs".into())
    }
}

/// A node that will not take a run, and why. What `offload run` prints when nobody will.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub node: NodeId,
    pub name: String,
    pub reason: String,
}

/// What one node says about a run when nothing is being granted.
///
/// A round discards these the moment it has a winner, which is why `offload explain` asks
/// again rather than reading a record of the round that placed the run. That is not a
/// shortcut: a bid describes *this second* — a node that was full at submission is not full an
/// hour later, and a laptop that refused on battery is on mains now — so the answer worth
/// showing somebody is the one the fleet would give today. It is bounded and side-effect free:
/// `WillYouTake` grants nothing, and a node that does not answer costs the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opinion {
    pub node: NodeId,
    pub name: String,
    pub verdict: Verdict,
}

/// The three things a node can say, kept apart because they are three different facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// It would take it, this keenly, and could start then.
    Bids(Offer),
    /// It would not, and why. Already a sentence — `NoBid`'s, usually.
    WillNot(String),
    /// It did not answer inside the window. **Not** a refusal: a node mid-restart is neither
    /// bidding nor declining, and reporting silence as "no" would put words in its mouth.
    Silent,
    /// The question never reached it: this node could not get a connection, and the transport
    /// said why — it knows no address for the peer, or the dial timed out. Kept apart from
    /// `Silent` because the reason is the useful half: measured on three daemons, a bystander
    /// printed `charlie  no answer` about the healthy node holding the run, while the arbiter
    /// one screen over had its bid — the bystander had never had an address to dial. Not a
    /// refusal either, for the reason `Silent` is not one.
    Unreachable(String),
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The preference is said per bidder and from the terms that bidder summed (ADR-0063
            // §7): "why did it not go to the desktop" is answered on this line or nowhere.
            Verdict::Bids(offer) => {
                write!(f, "bid {}, {}", offer.score.0, offer.available)?;
                if let Some(terms) = offer.terms.filter(offload_core::ScoreTerms::is_preferred) {
                    write!(f, " — preferred, +{}", terms.preferred)?;
                }
                Ok(())
            }
            Verdict::WillNot(reason) => f.write_str(reason),
            Verdict::Silent => f.write_str("no answer"),
            Verdict::Unreachable(reason) => write!(f, "not reachable from this node: {reason}"),
        }
    }
}

impl Opinion {
    /// Best-first ordering, so a printed round reads top-down as who would get the run.
    ///
    /// The same precedence `winner()` uses — able to start now, then score, then lowest id —
    /// followed by the nodes that declined and then the ones that said nothing. Deterministic,
    /// because a list that reshuffles between two runs of the same command looks like the
    /// fleet changed its mind.
    fn rank(&self) -> (u8, std::cmp::Reverse<(bool, i64)>, NodeId) {
        let (class, keys) = match &self.verdict {
            Verdict::Bids(offer) => (0, (offer.available.is_now(), offer.score.0)),
            Verdict::WillNot(_) => (1, (false, 0)),
            Verdict::Silent | Verdict::Unreachable(_) => (2, (false, 0)),
        };
        (class, std::cmp::Reverse(keys), self.node)
    }
}

/// How a round ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// A node took it. This node itself is the common case, and the one that needs no
    /// network at all.
    ///
    /// `starting` is the difference between "it is running on the desktop" and "the desktop
    /// has committed to it and begins when its own work finishes" — both are acceptance
    /// (ADR-0006), and telling somebody the first when it is the second is how they close the
    /// laptop expecting output by morning.
    Accepted {
        node: NodeId,
        name: String,
        starting: Availability,
        /// What every node said in this round, the winner included — so the submitter can be
        /// told why a node they *preferred* was not the one that took it (ADR-0063 §7), from
        /// the offers the round actually compared. A node whose grant failed is recorded as
        /// having declined, with the reason it gave.
        opinions: Vec<Opinion>,
    },
    /// Nobody would, and here is every reason (ADR-0014).
    ///
    /// `spent` is the run **as this round left it** — `Pending`, at an epoch past every grant the
    /// round handed out — and it is `None` only when the round spent nothing, which is a round
    /// with no bidders. It is on the outcome because the caller's copy is from *before* the
    /// round, and a caller that writes that copy down un-spends tokens this node has already
    /// issued. See the block at the end of [`Cluster::place`] for why that is the one thing an
    /// epoch exists to make impossible.
    Refused {
        refusals: Vec<Refusal>,
        spent: Option<Box<Run>>,
    },
}

impl Placement {
    #[must_use]
    pub fn accepted_by(&self) -> Option<NodeId> {
        match self {
            Placement::Accepted { node, .. } => Some(*node),
            Placement::Refused { .. } => None,
        }
    }
}

/// Puts a run's placement rendezvous back when the round holding it goes away, however it goes.
///
/// A `Drop` rather than a line at the end of [`Cluster::place`], because a round can end without
/// reaching that line: a submission is awaited by the control socket while somebody stands at the
/// keyboard (ADR-0014), and an operator that hangs up drops the future mid-round. A map entry left
/// behind by every abandoned submission is small and unbounded, which is the shape of leak that is
/// never noticed on a laptop and is noticed on the machine that has been up for a month.
struct RoundCleanup<'a> {
    cluster: &'a Cluster,
    run: offload_core::RunId,
}

impl Drop for RoundCleanup<'_> {
    fn drop(&mut self) {
        self.cluster.round_done(self.run);
    }
}

impl Cluster {
    /// The rendezvous for this run's placement round on this node, creating it if this is the
    /// first arrival (ADR-0050).
    ///
    /// Handed out under the synchronous map lock and cleaned up under the same one, so the
    /// count that decides whether the entry may go is taken where no other arrival can be
    /// half-way through cloning it. A caller holds the `Arc` for as long as its round lasts.
    fn round(&self, run: offload_core::RunId) -> std::sync::Arc<crate::Round> {
        let mut rounds = self
            .rounds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        rounds.entry(run).or_default().clone()
    }

    /// Drop this run's rendezvous once the last round holding it has gone. See [`RoundCleanup`],
    /// which is the only caller.
    ///
    /// The answer a round leaves behind is for whoever is *already waiting on it*, and for
    /// nobody else: a stale outcome returned to a round minutes later would be this node
    /// reporting a decision it did not take, about a fleet that has since changed. Two strong
    /// references — the map's and this caller's — means nobody is waiting.
    fn round_done(&self, run: offload_core::RunId) {
        let mut rounds = self
            .rounds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if rounds
            .get(&run)
            .is_some_and(|round| std::sync::Arc::strong_count(round) == 2)
        {
            rounds.remove(&run);
        }
    }

    /// Run one bounded bid round for `run` and hand it to whoever wins.
    ///
    /// `mine` is this node's own bid — computed by the caller, since only the caller can see
    /// its own local facts. `None` means this node is not offering, with the reason recorded
    /// like any other refusal.
    ///
    /// Bounded on purpose: `offload run` waits for this while somebody is standing at the
    /// keyboard (ADR-0014), so a node that has gone quiet costs the round its timeout and
    /// nothing more.
    pub async fn place(
        &self,
        run: &Run,
        mine: Result<Offer, String>,
        window: offload_core::Millis,
    ) -> Placement {
        // One round per run on this node, for the whole of the round (ADR-0050). An epoch is
        // monotonic per *arbiter*, so two rounds here for one run read `run.epoch` from two
        // copies that both say *N* and both grant *N+1* — and every later `fence`, `holds` and
        // `describes` check waves both legs through, because neither is stale. Nothing above
        // this arranges them: a drain runs a round for the run it just released while the
        // supervise tick, seeing the same run unheld, runs one too.
        let round = self.round(run.id);
        // Unconditional, so a round that is *cancelled* — a submission whose operator hung up
        // mid-round — cleans up after itself too. Declared before the lock is taken and after
        // the `Arc` is held, which is the drop order this needs: the outcome, then the
        // rendezvous, then the reference the count is taken against.
        let _cleanup = RoundCleanup {
            cluster: self,
            run: run.id,
        };
        let mut decided = round.lock().await;

        // Somebody else's round for this run finished while this call waited. Running a second
        // one would ask the same fleet the same question about a copy of the run that round has
        // already moved past, so this stands down and reports what it decided. The run *was*
        // placed by this node, a moment ago, and that is the whole of what any caller here needs
        // to know — including the drain, which counts a handover it did not itself perform.
        if let Some(already) = decided.as_ref() {
            tracing::debug!(
                run_id = %run.id,
                "a round for this run was already under way here; standing down"
            );
            return already.clone();
        }

        let outcome = self.round_of(run, mine, window).await;
        *decided = Some(outcome.clone());
        outcome
    }

    /// The round itself, with the exclusion already taken. See [`Cluster::place`].
    async fn round_of(
        &self,
        run: &Run,
        mine: Result<Offer, String>,
        window: offload_core::Millis,
    ) -> Placement {
        let mut bids: Vec<Bid> = Vec::new();
        let mut refusals: Vec<Refusal> = Vec::new();
        let mut opinions = self.canvass(run, mine, window).await;

        for opinion in opinions.clone() {
            match opinion.verdict {
                Verdict::Bids(offer) => bids.push(bid_of(run, opinion.node, offer)),
                Verdict::WillNot(reason) => refusals.push(Refusal {
                    node: opinion.node,
                    name: opinion.name,
                    reason,
                }),
                // A node that said nothing is not a reason nobody took the run, so it is not
                // printed as one. It is worth showing when the question is "what does the
                // fleet think" — which is `canvass`'s other caller, not this one.
                Verdict::Silent | Verdict::Unreachable(_) => {}
            }
        }

        // Able to start first, then highest score, ties by lowest id — deterministic, so this
        // would produce the same answer on any node that saw the same bids (ADR-0006).
        //
        // One `Run` threaded through every attempt, rather than a fresh clone of the original
        // per attempt. An epoch is the fencing token for **one grant**, and re-cloning gave
        // each attempt in the round the same one: a node whose `Granted` never came back — and
        // ADR-0006 says silence is a decline — held the run at the very epoch the next node was
        // then granted it at. Two holders, one token, and `fence` cannot tell them apart
        // because neither is stale. `hand_over` gives the token back on failure, at a number
        // the refused node can no longer act under.
        let mut offered = run.clone();
        while let Some(winner) = offload_core::bid::winner(&bids).cloned() {
            let node = winner.node;
            match self.hand_over(&mut offered, node, window).await {
                Ok(()) => {
                    return Placement::Accepted {
                        node,
                        name: self.name_of(node),
                        starting: winner.available,
                        opinions,
                    }
                }
                Err(reason) => {
                    if let Some(opinion) = opinions.iter_mut().find(|o| o.node == node) {
                        opinion.verdict = Verdict::WillNot(reason.clone());
                    }
                    // A declined grant must never strand the run, and a node that declined is
                    // not reconsidered in this round — otherwise arbiter and node ping-pong,
                    // since the decliner still has the highest score.
                    refusals.push(Refusal {
                        node,
                        name: self.name_of(node),
                        reason,
                    });
                    bids.retain(|b| b.node != node);
                }
            }
        }

        // Every token this round spent, remembered — even though nothing was confirmed, and
        // *because* nothing was confirmed.
        //
        // ADR-0006's silence-is-a-decline makes "took it, and the answer never came back" the
        // ordinary outcome rather than an exotic one, so a refused round is not evidence that the
        // grants it made did not land. The rule that follows was written down when the
        // within-a-round version of this was fixed — a grant spends a token whether or not it is
        // confirmed — and it stopped at the edge of the round: `offered` is a local copy, only a
        // *confirmed* grant is published, so a round that failed wholesale left this node's view
        // at the epoch it started from and the next round handed the same numbers out again. To
        // nodes that were, in the swallowed case, already running under them.
        //
        // Two grants at one epoch from one arbiter is precisely what the epoch exists to make
        // impossible: `fence` cannot order them, so neither agent is ever told it lost, and
        // `merge_run`'s equal-epoch tiebreak is left holding a decision it was only ever meant to
        // be the backstop for.
        //
        // What is published is what this node knows: nobody confirmed, so the run is `Pending`,
        // at an epoch past everything handed out. A node that did take one of those grants is
        // then behind the fleet and stops — the stalled-rather-than-duplicated trade every other
        // fence here makes. Only when the epoch actually moved: a round with no bidders spent
        // nothing, and inventing a record for a submission the operator is about to be told was
        // refused would leave it in the view with nobody holding it.
        //
        // **Remembered means written down**, which is the half the view alone does not buy. A
        // `ClusterView` is in memory and is rebuilt from the store at startup, so a fix that
        // published and did not record survived exactly as long as the process: an arbiter that
        // spent three tokens in a refused round and was then restarted came back at the epoch it
        // had started from and handed the same numbers out again — which is the paragraph above,
        // reached through the one door it did not cover. Recorded before it is published, the
        // same order and for the same reason `hand_over` records a confirmed grant.
        //
        // And handed back on the outcome, because the caller holds a copy of the run from
        // *before* the round: `server::place`'s queue branch wrote that copy to the store and
        // back into this view at the epoch this round had already spent, undoing both halves
        // from one line up the stack.
        let spent = if offered.epoch > run.epoch {
            self.host().record(&offered).await;
            self.publish_run(offered.clone());
            Some(Box::new(offered))
        } else {
            None
        };

        Placement::Refused { refusals, spent }
    }

    /// Ask the fleet what it makes of this run, and grant nothing.
    ///
    /// One round, same questions, same window as [`Cluster::place`] — which is built on this —
    /// but every answer is kept, including the silences. That is what `offload explain` needs:
    /// "why is my run still pending" and "why did nobody move it" are the same question asked
    /// at the two ends of a run's life, and both are answered per node or not at all.
    pub async fn canvass(
        &self,
        run: &Run,
        mine: Result<Offer, String>,
        window: offload_core::Millis,
    ) -> Vec<Opinion> {
        let me = self.node();
        let mut opinions = vec![Opinion {
            node: me,
            name: self.name_of(me),
            verdict: match mine {
                Ok(offer) => Verdict::Bids(offer),
                Err(reason) => Verdict::WillNot(reason),
            },
        }];

        for peer in self.candidates() {
            opinions.push(Opinion {
                node: peer,
                name: self.name_of(peer),
                verdict: self.ask_one(peer, run, window).await,
            });
        }

        opinions.sort_by_key(Opinion::rank);
        opinions
    }

    /// Ask the node that is running a run for its event log, from after `after`.
    ///
    /// One page per call, polled by the caller: see `ClusterMessage::FetchEvents` for why a poll
    /// rather than a stream held open. Returns the page and whether that holder is finished, so
    /// the caller knows when to stop asking rather than guessing from an empty page — a run
    /// between turns is quiet for minutes and is not over.
    pub async fn fetch_events(
        &self,
        holder: NodeId,
        run: offload_core::RunId,
        after: u64,
        limit: u32,
        window: offload_core::Millis,
    ) -> Result<(Vec<offload_proto::cluster::SeqEvent>, bool), String> {
        let ask = ClusterMessage::FetchEvents { run, after, limit };
        match self.request(holder, &ask, window).await {
            Ok(ClusterMessage::Events { events, done, .. }) => Ok((events, done)),
            Ok(ClusterMessage::NoEvents { reason, .. }) => Err(reason),
            Ok(_) => Err("answered a log request with something else".into()),
            Err(e) => Err(format!("{} did not answer ({e})", self.name_of(holder))),
        }
    }

    /// Ask the node that ran a run what is at `path` in its workspace (ADR-0075).
    pub async fn fetch_files(
        &self,
        node: NodeId,
        run: offload_core::RunId,
        path: &str,
        window: offload_core::Millis,
    ) -> Result<offload_proto::cluster::FilesView, String> {
        let ask = ClusterMessage::FetchFiles {
            run,
            path: path.to_string(),
        };
        match self.request(node, &ask, window).await {
            Ok(ClusterMessage::Files { view, .. }) => Ok(view),
            Ok(ClusterMessage::NoFiles { reason, .. }) => Err(reason),
            Ok(_) => Err("answered a workspace request with something else".into()),
            Err(e) => Err(format!("{} did not answer ({e})", self.name_of(node))),
        }
    }

    /// Ask the node that owns a run's editable fields to change one of them.
    ///
    /// Forwarded rather than applied where it was typed, which is the whole of what makes
    /// `spec_rev` work (ADR-0013): one owner counting its own writes is a total order, and
    /// two nodes editing locally are two records at revision 1 undoing each other for ever.
    ///
    /// A refusal comes back as a sentence, and so does silence — the operator is standing
    /// there, and "the node that owns this run is not answering" is a fact they can act on,
    /// unlike a change that was accepted by nobody.
    pub async fn edit_spec_at(
        &self,
        owner: NodeId,
        run: offload_core::RunId,
        edit: offload_core::SpecEdit,
        window: offload_core::Millis,
    ) -> Result<String, String> {
        let ask = ClusterMessage::EditSpec { run, edit };
        match self.request(owner, &ask, window).await {
            Ok(ClusterMessage::SpecEdited { note, .. }) => Ok(note),
            Ok(ClusterMessage::EditRefused { reason, .. }) => Err(reason),
            Ok(_) => Err("answered a spec edit with something else".into()),
            Err(e) => Err(format!(
                "{}, which owns this run's deadline and priority, did not answer ({e})",
                self.name_of(owner)
            )),
        }
    }

    /// Forward a cancel to the node the run is on.
    ///
    /// The sibling of [`Cluster::answer_at`] and [`Cluster::edit_spec_at`], and the target is
    /// chosen by the caller for a reason those two make plain: a run being *held* is on its
    /// holder, and a run nobody holds is a record, which belongs to the node that arbitrates it.
    /// Both are the same command — "stop this" — and neither is the machine the operator
    /// happens to be typing at.
    ///
    /// Silence is a sentence rather than a code, for `answer_at`'s reason: somebody is standing
    /// there, and "the machine running your run is not answering" is a fact they can act on —
    /// including by going to that machine.
    pub async fn cancel_at(
        &self,
        node: NodeId,
        run: offload_core::RunId,
        window: offload_core::Millis,
    ) -> Result<String, String> {
        let ask = ClusterMessage::Cancel {
            run,
            // The node it was typed at, for the run's log on the machine that acts on it.
            by: self.name_of(self.node()),
        };
        match self.request(node, &ask, window).await {
            Ok(ClusterMessage::Cancelled { note, .. }) => Ok(note),
            Ok(ClusterMessage::CancelRefused { reason, .. }) => Err(reason),
            Ok(_) => Err("answered a cancel with something else".into()),
            Err(e) => Err(format!(
                "{}, which has this run, did not answer ({e})",
                self.name_of(node)
            )),
        }
    }

    /// Ask the node that ran a finished run's last leg for a continuation's base, and fetch the
    /// blobs it names from there (ADR-0064 §2).
    ///
    /// [`Cluster::cancel_at`]'s sibling, with the target chosen the way `offload logs` chooses
    /// one: the leg that wrote the run's last position is the leg whose checkout is left. The
    /// blobs are fetched **before** this returns, so the base the caller makes a run from is one
    /// this node already holds — the building node's collector owes it nothing, since no run
    /// there names those blobs until gossip says so.
    ///
    /// `window` bounds the answer, which is a capture and not a lookup: a bundle of the parent's
    /// branch, so it is given longer than a bid round.
    pub async fn continuation_base_at(
        &self,
        node: NodeId,
        run: offload_core::RunId,
        session: bool,
        window: offload_core::Millis,
    ) -> Result<offload_core::ContinuationBase, String> {
        let ask = ClusterMessage::ContinueBase { run, session };
        let base = match self.request(node, &ask, window).await {
            Ok(ClusterMessage::ContinueBaseBuilt { base, .. }) => *base,
            // Worded by the building node about itself, by name, so it reads right here.
            Ok(ClusterMessage::ContinueBaseRefused { reason, .. }) => return Err(reason),
            Ok(_) => return Err("answered a continuation with something else".into()),
            Err(e) => {
                return Err(format!(
                    "{}, which ran its last leg and has its workspace, did not answer ({e})",
                    self.name_of(node)
                ))
            }
        };
        for blob in base.blobs() {
            self.fetch_blob(blob, Some(node)).await.map_err(|e| {
                format!(
                    "{} built the base, and fetching it failed ({e})",
                    self.name_of(node)
                )
            })?;
        }
        Ok(base)
    }

    /// Ask the holder to checkpoint and give the run up.
    ///
    /// [`Cluster::cancel_at`]'s sibling, and the target is never in doubt: only the holder has an
    /// agent, and only an agent reaches a turn boundary. What comes back is an acknowledgement
    /// rather than a result — the capture happens whenever the current turn ends, which may be
    /// minutes away, and claiming otherwise is the one thing this command must not do (ADR-0004).
    pub async fn checkpoint_at(
        &self,
        holder: NodeId,
        run: offload_core::RunId,
        window: offload_core::Millis,
    ) -> Result<(), String> {
        let ask = ClusterMessage::Checkpoint { run };
        match self.request(holder, &ask, window).await {
            Ok(ClusterMessage::CheckpointRequested { .. }) => Ok(()),
            Ok(ClusterMessage::CheckpointRefused { reason, .. }) => Err(reason),
            Ok(_) => Err("answered a checkpoint request with something else".into()),
            Err(e) => Err(format!(
                "{}, which is running this run, did not answer ({e})",
                self.name_of(holder)
            )),
        }
    }

    /// Forward an answer to the node whose agent is blocked (ADR-0017).
    ///
    /// The mirror of [`Cluster::edit_spec_at`], and for a stricter reason: an edit could in
    /// principle be applied anywhere and reconciled, while a blocked *process* exists on exactly
    /// one machine. An answer typed anywhere else has to travel or it does nothing.
    ///
    /// Silence is a sentence rather than a code, because somebody is standing there — and "the
    /// machine running your agent is not answering" is a fact they can act on.
    pub async fn answer_at(
        &self,
        holder: NodeId,
        run: offload_core::RunId,
        tool_use_id: Option<String>,
        allow: bool,
        window: offload_core::Millis,
    ) -> Result<(String, String, bool), String> {
        let ask = ClusterMessage::Answer {
            run,
            tool_use_id,
            allow,
            // The node it was typed at, for the run's log on the machine that acts on it.
            by: self.name_of(self.node()),
        };
        match self.request(holder, &ask, window).await {
            Ok(ClusterMessage::AnswerTaken {
                tool,
                detail,
                allowed,
                ..
            }) => Ok((tool, detail, allowed)),
            Ok(ClusterMessage::AnswerRefused { reason, .. }) => Err(reason),
            Ok(_) => Err("answered an approval with something else".into()),
            Err(e) => Err(format!(
                "{}, which is running this agent, did not answer ({e})",
                self.name_of(holder)
            )),
        }
    }

    /// Ask every peer what its agents are waiting for.
    ///
    /// One bounded round, like `canvass`: current by construction, and a node that is asleep
    /// simply contributes nothing rather than contributing something stale. Peers that do not
    /// answer are skipped in silence — a question this node cannot see is one it cannot answer
    /// either, and an error row per sleeping phone would bury the questions that are real.
    pub async fn canvass_asks(
        &self,
        window: offload_core::Millis,
    ) -> Vec<offload_core::PendingAsk> {
        let mut out = Vec::new();
        for peer in self.candidates() {
            if let Ok(ClusterMessage::Asks { asks }) =
                self.request(peer, &ClusterMessage::FetchAsks, window).await
            {
                out.extend(asks.into_iter().map(|ask| offload_core::PendingAsk {
                    // Stamped by the collector: the holder does not need to name itself, and
                    // this is the only place that knows who was asked or what it is called.
                    node: Some(peer),
                    node_name: Some(self.name_of(peer)),
                    ..ask
                }));
            }
        }
        out
    }

    /// Make sure somebody else has this run's record before we call the submission accepted.
    ///
    /// The overnight case, at the only moment it is fragile: a run this node accepted *itself*
    /// exists on exactly one machine, and the next thing that happens is somebody closing the
    /// laptop. A run placed on a peer needs none of this — the peer has it, which is what
    /// taking it means — so this is the one-node-fleet-of-two gap that would otherwise lose a
    /// submission between `offload run` returning and the lid closing.
    ///
    /// Deliberately **not** a new message. An ordinary probe already carries this node's whole
    /// view, and a peer merges and writes down what it learns *before* it answers — so an
    /// `Ack` is a receipt, and the mechanism that spreads run records is the mechanism that
    /// confirms one. Adding a "remember this" message would be a second way to do the same
    /// thing, and a protocol version to pay for it.
    ///
    /// Returns the node that has a copy, or `None` for a fleet with nobody else awake — which
    /// is reported rather than hidden, because "the only copy is on this machine" is exactly
    /// what somebody about to shut it needs to know (ADR-0016 makes the same choice for
    /// checkpoints).
    pub async fn confirm_record(&self, run: &Run, window: offload_core::Millis) -> Option<NodeId> {
        self.publish_run(run.clone());
        let gossip_seq = self.next_seq();

        for peer in self.candidates() {
            let ping = ClusterMessage::Ping {
                seq: gossip_seq,
                gossip: self.gossip_snapshot(),
            };
            match self.request(peer, &ping, window).await {
                Ok(ClusterMessage::Ack { .. }) => return Some(peer),
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!(node = %peer.short(), error = %e, "would not take a copy");
                }
            }
        }
        None
    }

    /// Everybody worth asking: alive, and not us.
    fn candidates(&self) -> Vec<NodeId> {
        let me = self.node();
        self.view()
            .nodes
            .values()
            .filter(|n| n.id != me && n.status.is_available())
            .map(|n| n.id)
            .collect()
    }

    pub(crate) fn name_of(&self, node: NodeId) -> String {
        self.view()
            .node(&node)
            .map_or_else(|| node.short(), offload_core::NodeView::display_name)
    }

    /// `Ok(Some(score))` is a bid, `Ok(None)` is a node that did not answer in time, and
    /// `Err` is a refusal with a reason worth printing.
    async fn ask_one(&self, peer: NodeId, run: &Run, window: offload_core::Millis) -> Verdict {
        let ask = ClusterMessage::WillYouTake {
            run: Box::new(run.clone()),
        };
        match self.request(peer, &ask, window).await {
            Ok(ClusterMessage::Bid {
                score,
                available,
                terms,
                ..
            }) => {
                // The bidder said yes; its certificate has the final say (ADR-0012).
                if let Some(objection) = self.hosting_objection(peer).await {
                    return Verdict::WillNot(objection);
                }
                Verdict::Bids(Offer {
                    score: offload_core::Score(score),
                    available,
                    terms,
                })
            }
            Ok(ClusterMessage::WillNot { reason, .. }) => Verdict::WillNot(reason),
            Ok(_) => Verdict::WillNot("answered a bid request with something else".into()),
            // Never dialled: the transport has its own sentence for why, and it is about this
            // node's reach rather than the peer's health.
            Err(offload_transport::TransportError::Unreachable { reason, .. }) => {
                tracing::debug!(node = %peer.short(), %reason, "no bid: not reachable");
                Verdict::Unreachable(reason)
            }
            Err(e) => {
                // Silence is not a refusal with a reason: the node may be mid-restart. It is
                // reported as a non-answer so the round moves on rather than waiting.
                tracing::debug!(node = %peer.short(), error = %e, "no bid");
                Verdict::Silent
            }
        }
    }

    /// The certificate a peer presented on the connection this node still holds to it.
    ///
    /// `None` is **unknown**, not *no*: there is no live connection, so nothing to read. Every
    /// caller has to say what it does with that, and they do not agree — see
    /// [`Self::peer_hosts_runs`].
    async fn peer_certificate(&self, peer: NodeId) -> Option<offload_core::MembershipCert> {
        self.existing_session(peer)
            .await
            .map(|session| session.peer.membership.clone())
    }

    /// Is `peer` permitted by the fleet to host runs, as far as this node can tell?
    ///
    /// The same question [`Self::hosting_objection`] asks, without the sentence and without the
    /// side effect — it is asked here by an *estimate* rather than by an enforcement, so it must
    /// not drop a connection.
    ///
    /// **`None` means unknown and must not be read as `false`.** The only reason to ask is
    /// `review_commitment`: whether anywhere else could start a run this node has committed to
    /// and cannot start yet. Its own docs settle which way to be wrong — "being wrong there
    /// costs a bid round; being unwilling to estimate at all costs the run its night" — so a
    /// peer this node has no connection to stays counted, and the only nodes this ever removes
    /// are the ones whose certificates are *here* and say no. That keeps the change strictly
    /// subtractive: it can turn a doomed round into no round, and can never turn a possible
    /// handover into a run left on a busy machine.
    pub async fn peer_hosts_runs(&self, peer: NodeId) -> Option<bool> {
        let now = self.clock_now();
        self.peer_certificate(peer)
            .await
            .map(|cert| permits_hosting(&cert, now))
    }

    /// The other half of what a signed certificate is for (ADR-0012): the handshake admits a
    /// member, and this refuses the member's *bid* when the fleet never said it may host. An
    /// honest node's own bid path already checks its own grants, so an objection here means the
    /// bidder is misconfigured, out of date, or promoting itself — and the arbiter is where the
    /// fleet's say is enforced, because the bidder has had its chance to be honest.
    ///
    /// The certificate is asked at *this* moment, not the handshake's: probation ends while a
    /// connection stays up, and a check against admission-time grants would refuse a
    /// now-legitimate host until something happened to redial. Staleness also runs the other
    /// way — a grant issued a minute ago lives on a certificate this connection has never
    /// seen — so an objection drops the connection too: the next round re-handshakes, and a
    /// renewed certificate is picked up within the arbiter's own retry rather than never.
    async fn hosting_objection(&self, peer: NodeId) -> Option<String> {
        let now = self.clock_now();
        let membership = self.peer_certificate(peer).await;
        let on_probation = membership
            .as_ref()
            .is_some_and(|cert| cert.probation_until(now).is_some());

        let objection = match membership {
            // The connection the bid arrived on is already gone, so there is nothing left to
            // check the claim against. Prefer a run stalled over a run misplaced.
            None => Some(
                "bid, but its connection closed before its certificate could be checked"
                    .to_string(),
            ),
            Some(ref cert) if permits_hosting(cert, now) => None,
            Some(cert) if cert.is_expired(now) => {
                Some("bid, but its membership certificate has expired".to_string())
            }
            Some(cert) => Some(match cert.probation_until(now) {
                Some(until) => format!(
                    "bid, but its host-runs grant is dormant for another {}m of probation",
                    (until.0.saturating_sub(now.0)).div_ceil(60_000)
                ),
                None => "bid, but its certificate does not grant host-runs".to_string(),
            }),
        };

        // Refusing a bid also ends the session that carried the certificate, so the next round
        // re-handshakes and reads the peer's papers as they are *then* — a grant issued or a
        // certificate renewed mid-connection. **Both directions** for that, because the session
        // this node uses may be one the peer dialled (a phone behind a carrier firewall is
        // reachable no other way, see `Cluster::session`), and dropping only what this node
        // dialled left the old certificate answering every round. Probation is the exception:
        // the certificate is already the right one and lifts by time alone, so nothing is hung up
        // on a peer that may have no other way back in.
        if objection.is_some() {
            if on_probation {
                self.drop_connection(peer).await;
            } else {
                self.disconnect(peer, "refused its bid on its certificate")
                    .await;
            }
        }
        objection
    }

    /// Grant the run to `node` and wait for it to confirm.
    ///
    /// Takes the run by reference *mutably* because a grant spends a fencing token whether or
    /// not it is confirmed. A node that received the grant and whose answer was lost is
    /// running the agent under the epoch this attempt issued, so the next node in the round
    /// must not be handed the same one — `Run::release` gives it back at a number that fences
    /// the unreachable one out. Which is the same sentence `release` already carried for a
    /// commitment somebody hands back: the bump "ends the departing holder's right to act on
    /// the run at the moment it stops intending to", and an unconfirmed grant is exactly that
    /// moment seen from the arbiter's end.
    async fn hand_over(
        &self,
        run: &mut Run,
        node: NodeId,
        window: offload_core::Millis,
    ) -> Result<(), String> {
        let epoch = run
            .assign(node, self.clock_now(), LEASE)
            .map_err(|e| e.to_string())?;

        // Anything below here that does not return `Ok` has to give the token back, so the
        // failure paths funnel through one place rather than each remembering.
        let outcome = self.offer(run.clone(), node, window).await;
        if outcome.is_err() {
            let now = self.clock_now();
            if let Err(e) = run.release(node, epoch, now) {
                // A pinned run has one grant for life, so there is no next attempt for a
                // token to matter to — `assign` refuses the next node anyway.
                tracing::debug!(run_id = %run.id, error = %e, "grant not given back");
            }
        }
        outcome
    }

    /// Put the grant to the node and read its answer.
    async fn offer(
        &self,
        granted: Run,
        node: NodeId,
        window: offload_core::Millis,
    ) -> Result<(), String> {
        if node == self.node() {
            tracing::debug!(run_id = %granted.id, epoch = granted.epoch.0, "taking it here");
            // Granting to yourself is still arbitrating, and on a fleet of one it is the only
            // shape a grant ever has — this branch returning early is why the audit log had no
            // record of a grant on a single-node fleet at all.
            //
            // Reported only once the run is actually taken, which is the *remote* path's rule and
            // has to be this one's too. `accept` can refuse — ADR-0006 step 6 is a node re-running
            // its own checks and finding itself busy since it bid — and a report written before
            // asking says this node gave the run to somebody who never took it. Written the eager
            // way first, and `an_arbiter_reports_each_grant_once` found it inside the hour:
            // `[Swallow(1), Decline(0), …, Place(0)]`, a node declining its own grant.
            let taken = self.host().accept(granted.clone()).await;
            if taken.is_ok() {
                self.host().granted(&granted).await;
            }
            return taken;
        }

        let grant = ClusterMessage::Grant {
            run: Box::new(granted.clone()),
        };
        match self.request(node, &grant, window).await {
            Ok(ClusterMessage::Granted { .. }) => {
                // The decision, once, before the record. See [`Host::granted`].
                self.host().granted(&granted).await;
                // Ours to remember even though it is not ours to run: `offload ps` on the
                // machine somebody submitted from should still know where the work went.
                self.host().record(&granted).await;
                // And ours to *count*. Capacity is read from the view, which otherwise learns
                // that this node is now busy only when it next gossips — up to a probe
                // interval away. Six submissions in two seconds all went to the same node
                // because every round after the first was still looking at a picture taken
                // before the previous grant, and being at capacity is precisely the fact a
                // bid round is supposed to act on.
                self.publish_run(granted);
                Ok(())
            }
            Ok(ClusterMessage::Declined { reason, .. }) => Err(reason),
            Ok(_) => Err("answered a grant with something else".into()),
            // Silence is a decline (ADR-0006): a node that went away between bidding and
            // being granted must not cost the run a full lease expiry.
            Err(e) => Err(format!("did not confirm: {e}")),
        }
    }
}

fn bid_of(run: &Run, node: NodeId, offer: Offer) -> Bid {
    Bid {
        run: run.id,
        node,
        epoch: run.epoch,
        score: offer.score,
        available: offer.available,
        submitted_at: offload_core::Millis(0),
        terms: offer.terms,
    }
}

/// The one test of a certificate's right to host runs.
///
/// Two callers with different jobs — the arbiter's objection to a bid, which *enforces*, and
/// [`Cluster::peer_hosts_runs`], which *estimates* — and they must not be able to disagree. They
/// were not two until now: the estimate did not exist, and `mesh::ready_elsewhere` counted every
/// alive node with room, which is **ability** where hosting needs **permission**. Written once,
/// because a second copy of a rule is how a report ends up confidently wrong about what the
/// decision beside it will do.
///
/// Asked at the moment of the question rather than at the handshake's, for the reason
/// [`Cluster::hosting_objection`] gives at length: probation ends while a connection stays up.
fn permits_hosting(cert: &offload_core::MembershipCert, now: offload_core::Millis) -> bool {
    !cert.is_expired(now) && cert.granted(Grant::HostRuns, now)
}
