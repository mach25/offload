//! Negotiated placement: nodes bid for work rather than being told to take it.
//!
//! The reason to do it this way is information, not elegance. A node knows things about
//! itself that gossip cannot carry accurately — whether the repo is already cloned, how hot
//! it is right now, what its load looks like this second, whether its battery just dropped.
//! A central scheduler can only ever work from a stale summary. So the decision is made
//! where the information is, and arbitration only picks a winner among self-assessments.
//!
//! See `docs/adr/0006-negotiated-placement.md`.

use crate::capability::Stability;
use crate::capacity::{NodeLoad, Occupancy, PRESSURE_RETRY};
use crate::id::{NodeId, RunId};
use crate::policy::Refusal;
use crate::repo::RepoReach;
use crate::run::{Epoch, Run};
use crate::time::Millis;
use crate::version;
use crate::view::{ClusterView, NodeView};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Score(pub i64);

/// When a bidding node could actually start the run.
///
/// A node at its concurrency cap is not refusing: it is capable, willing, and busy. Declining
/// with "come back later" leaves **nobody committed** — the run stays `Pending` and hopes,
/// which is the outcome ADR-0014 exists to forbid and the one the overnight case dies of. So
/// a busy node offers anyway, and takes the grant into `Assigned` until its own work frees up
/// (ADR-0006, "accepting without starting").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "available")]
pub enum Availability {
    /// The agent starts as soon as the grant is confirmed. The common case.
    Now,
    /// Committed, but queued behind work already on this node.
    ///
    /// `behind` is a count and **not an ETA**, because a count is what this node actually
    /// knows: how long an agent turn takes is model time, not machine time, and a fabricated
    /// "~20m" would be the number everybody quotes back. Capacity as a budget with real
    /// estimates is ADR-0013, and it can replace this with something better without any node
    /// having learned to lie in the meantime.
    WhenFree { behind: u32 },
    /// Committed, and held by the **account's** rate limit until a stated instant (ADR-0029).
    ///
    /// An instant rather than a count, which is the opposite of [`Availability::WhenFree`] and
    /// for the same reason: a count is what a node knows about its own queue, and here the agent
    /// has said in as many words when the limit lifts. Inventing a count would be the fabrication
    /// that comment refuses; refusing to say the time we were *told* would be the other one.
    ///
    /// Not [`Availability::WhenFree`] with a number attached, because the two send somebody to
    /// different places: one empties when work here finishes, and the other empties when a clock
    /// somewhere else says so and is unaffected by anything this fleet does.
    NotBefore { at: crate::time::Millis },
}

impl Availability {
    #[must_use]
    pub fn is_now(self) -> bool {
        self == Availability::Now
    }
}

impl std::fmt::Display for Availability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Availability::Now => f.write_str("starting now"),
            // "the run ahead of it" rather than "its current run": since the account cap, the
            // work a run waits behind is not always on the node that is waiting, and a message
            // that says "its" about a peer's run sends somebody to the wrong machine.
            Availability::WhenFree { behind: 1 } => {
                f.write_str("starting when the run ahead of it finishes")
            }
            // No instant in the sentence, deliberately. This crate has no clock, so the only
            // thing it could print is a unix millisecond count — and a reader who wants "in 43
            // minutes" is served by the CLI, which has one. A raw number here would be the
            // fabricated ETA `WhenFree` refuses, spelled in a unit nobody reads.
            Availability::NotBefore { .. } => {
                f.write_str("starting when the account's rate limit lifts")
            }
            Availability::WhenFree { behind } => {
                write!(
                    f,
                    "starting when one of the {behind} runs ahead of it finishes"
                )
            }
        }
    }
}

/// What a node says about itself when asked to take a run: how keen, and whether it could
/// start now. A [`Bid`] is this plus the identity of who is offering and for what.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub score: Score,
    pub available: Availability,
    /// What `score` was summed from, so the arbiter can say *why* a preferred node lost
    /// (ADR-0063 §7) from the numbers that node read rather than a second copy of them. `None`
    /// from anything that did not compute one — a test double, a peer's refusal.
    #[serde(default)]
    pub terms: Option<ScoreTerms>,
}

/// A node's offer to host a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bid {
    pub run: RunId,
    pub node: NodeId,
    /// The epoch the bid was computed against. A bid for a stale epoch is ignored — it was
    /// made about a situation that no longer exists.
    pub epoch: Epoch,
    pub score: Score,
    /// Whether this node could start the run now, or is committing to it for later.
    #[serde(default = "now_available")]
    pub available: Availability,
    pub submitted_at: Millis,
    /// See [`Offer::terms`].
    #[serde(default)]
    pub terms: Option<ScoreTerms>,
}

fn now_available() -> Availability {
    Availability::Now
}

impl Bid {
    #[must_use]
    pub fn offer(&self) -> Offer {
        Offer {
            score: self.score,
            available: self.available,
            terms: self.terms,
        }
    }
}

/// Facts a node knows about itself that are too volatile or too local to gossip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalFacts {
    /// Repo already cloned here. Usually the single biggest real-world difference.
    pub workspace_warm: bool,
    /// Whether this node can obtain the repo at all, and if not, why not.
    ///
    /// Always `Obtainable` for a clone URL — that is what `Portability::Fleetwide` means. For a
    /// repo named by local path it is a fact about *this machine*, and it is the difference
    /// between a run that cannot be placed and a run that is placed and then fails at 03:00
    /// when the laptop holding the path is shut.
    ///
    /// Three-valued rather than a `bool` because the refusal it produces is read by somebody
    /// standing at one particular machine: *not here* and *here and not a repository* are two
    /// different pieces of advice, and only the node that looked can tell them apart.
    pub repo_reach: RepoReach,
    /// Already holds the checkpoint blobs, so resuming needs no transfer.
    pub checkpoint_local: bool,
    /// How busy the machine is, as a percentage of its cores. `None` where the platform will
    /// not say — which is not the same as idle, and the rules that read it skip rather than
    /// assume. A node reporting a plausible zero wins a bid it should lose.
    pub cpu_load_percent: Option<u8>,
    /// How hot the device says it is (ADR-0068). `None` where nothing says, like the load.
    pub thermal: Option<crate::capacity::Thermal>,
    /// What the *rest of this device* has promised: other `offloadd` instances, one per fleet
    /// (ADR-0013's broker). Added to what this node holds, because capacity belongs to the
    /// machine and the other instance's runs are invisible to this fleet's view.
    ///
    /// Here rather than in the view for the reason every `LocalFacts` field is: it is measured
    /// locally and gossiped nowhere. A fleet learns the *number* its device has left, never the
    /// other fleet's runs — which is the whole of what the ledger is permitted to carry.
    ///
    /// Zero on a device with one fleet, or one whose ledger could not be read, which is exactly
    /// the behaviour there was before it existed.
    pub device_committed: crate::capacity::Occupancy,
    /// When this account's rate limit lifts, if the agent has said it is blocking (ADR-0029).
    ///
    /// **Already a decision, not a raw observation.** The caller has a clock and this crate does
    /// not, so what arrives here is "still blocking, until this instant" — `None` covers both
    /// "never seen one" and "the one we saw has lifted", which are the same thing for every
    /// reader.
    ///
    /// Here rather than in the view for every other `LocalFacts` field's reason, and one of its
    /// own: an account's position is a *moving* value, and ADR-0013's rule about attendance
    /// applies — gossiping one hands a reader a number from before the silence and calls it
    /// current. What travels safely is only the half with a stated reset, and even that is
    /// deliberately not built yet (ADR-0029's consequences).
    pub account_limited_until: Option<crate::time::Millis>,
}

impl Default for LocalFacts {
    /// A cold node with a repo it can fetch.
    ///
    /// `repo_reach` defaults to `Obtainable`, because the common case is a clone URL and a
    /// default of anything else would make every node refuse every run — loudly, but for the
    /// wrong reason. The daemon sets it from the actual repo before it bids; the default is
    /// what tests and a fleetwide repo both want.
    fn default() -> Self {
        LocalFacts {
            workspace_warm: false,
            repo_reach: RepoReach::Obtainable,
            checkpoint_local: false,
            cpu_load_percent: None,
            thermal: None,
            // Nothing promised elsewhere — a device with one fleet on it, which is also what
            // every test wants unless it says otherwise.
            device_committed: Occupancy::default(),
            // Nothing has told us otherwise, which is what a node that has never run a turn
            // knows and what every test wants unless it says so.
            account_limited_until: None,
        }
    }
}

/// Why a node declined to bid. Kept structured so `offload explain` can print the actual
/// reason rather than "no eligible nodes".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoBid {
    /// The owner's policy said no. Distinct from incapability — the phone *could*.
    Refused(Refusal),
    /// Not now, and this node is the right one shortly. **Not a refusal**, and not a
    /// commitment either.
    ///
    /// The distinction is which queue the node is behind. Being *full* is this node's own
    /// queue: it accepted that work, it knows how deep it is, and it empties by itself — so a
    /// full node commits and takes the grant into `Assigned` (ADR-0006). Being under *pressure*
    /// is a condition of the machine caused by work this node never accepted — the owner's
    /// build, another fleet's daemon — and it can neither promise when that drains nor make it
    /// drain. Committing there would park a run behind something nobody is managing.
    ///
    /// So it defers instead, and only when the run can afford the wait: `retry_after` against
    /// the run's slack, which is why this needed a deadline to exist before it could mean
    /// anything. A run with nothing left to spend is told to look elsewhere *now* rather than
    /// wait for this node — being picky is how a late run gets later (ADR-0013).
    Busy {
        retry_after: Millis,
        reason: Refusal,
    },
    /// The run's constraints are not met. Carries the failing clauses.
    Ineligible(Vec<String>),
    /// Resuming this checkpoint needs an agent at least as new as the one that wrote it.
    AgentTooOld { have: String, need: String },
    /// Already holds this run; nothing to bid on.
    AlreadyHolder,
    /// This node cannot get the repository. For a local-path repo that is most nodes, and
    /// saying so at placement is the whole point (ADR-0003's portability note).
    ///
    /// Carries the `RepoReach` the node **measured**, not the `Portability` derivable from the
    /// repo string: the two causes of an unobtainable local path are one `NodeLocal` and two
    /// different sentences, and only the node that looked at the filesystem knows which.
    RepoUnavailable { repo: String, reach: RepoReach },
    /// The workspace archive is larger than **this node's owner** will take (ADR-0061 §4).
    ///
    /// Deliberately distinct from the fleet-wide cap, which is refused before a run exists at
    /// all. The two send somebody to opposite places: this one is somebody's policy, so another
    /// node may well take it and the fix is to ask elsewhere or change the setting; the
    /// structural one is every node's, so there is no elsewhere and the archive has to be
    /// smaller. Collapsing them would be `RepoUnavailable`'s own defect — one sentence for two
    /// causes — which session seventy-five had to unpick.
    ArchiveTooLarge { bytes: u64, limit: u64 },
    /// Held out for a preferred node that is not this one, until a stated time (ADR-0063 §4).
    ///
    /// Its own variant rather than the `Ineligible` the hold's clause would otherwise produce,
    /// which read `ineligible: is node 2a3b5d5d (this is 29be31be)` — two ids, and nothing to
    /// say the cause was a hold that ends by itself. `left` is how long, from `now`.
    Held { left: Millis },
}

impl std::fmt::Display for NoBid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoBid::Refused(refusal) => write!(f, "{refusal}"),
            NoBid::Busy {
                retry_after,
                reason,
            } => write!(f, "busy — {reason}; worth asking again in {retry_after}"),
            NoBid::Ineligible(failures) => write!(f, "ineligible: {}", failures.join("; ")),
            NoBid::AgentTooOld { have, need } => {
                write!(f, "agent {have} is older than the checkpoint's {need}")
            }
            NoBid::AlreadyHolder => f.write_str("already holds this run"),
            NoBid::Held { left } => write!(
                f,
                "held for its preferred node for another {left}, and this is not it"
            ),
            NoBid::ArchiveTooLarge { bytes, limit } => write!(
                f,
                "its workspace archive is {bytes} bytes and this node's owner takes at most \
                 {limit} — another node may still take it"
            ),
            NoBid::RepoUnavailable {
                repo,
                reach: RepoReach::NotHere,
            } => write!(f, "{repo} is a path on another machine"),
            NoBid::RepoUnavailable {
                repo,
                reach: RepoReach::NotARepo,
            } => write!(f, "{repo} is here, but it is not a git repository"),
            // Not reachable from `bid`, which only builds this variant for a reach that is
            // not obtainable — but a variant that can be constructed has to render.
            NoBid::RepoUnavailable { repo, .. } => write!(f, "cannot obtain {repo}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BidWeights {
    pub stability: i64,
    pub workspace_warm: i64,
    pub checkpoint_local: i64,
    /// Scored per *doubling* of cores/memory, not per unit. An agent run is bound by model
    /// latency far more than by CPU, so a 32-core box is worth somewhat more than an
    /// 8-core one — not four times more. Linear weighting here drowns out every signal
    /// that actually predicts a good placement.
    pub per_core_doubling: i64,
    pub per_gib_doubling: i64,
    pub load_penalty: i64,
    pub battery_penalty: i64,
    pub metered_penalty: i64,
    /// Charged when the run currently has a healthy holder elsewhere: moving work that is
    /// fine where it is should need a clearly better offer.
    pub migration_penalty: i64,
    /// Charged per run already active on the same agent account anywhere in the fleet.
    pub account_pressure: i64,
    /// Added when the bidder satisfies the run's [`RunSpec::prefer`](crate::RunSpec::prefer)
    /// (ADR-0063 §2). Set to outweigh any *one* reason the fleet has to go elsewhere and yield to
    /// two: a preference that always won would be a pin, and a pin is `--require`.
    ///
    /// A constant of the protocol and not configuration, because it is the term the *submitter*
    /// asked for, so two nodes weighing it differently is ROADMAP question 7 at its sharpest.
    pub preferred: i64,
    /// Added per resource the run was granted that the bidder itself holds (ADR-0063 §5). The
    /// signal ADR-0011's proxy stopped being a constraint: a run beside its context reads it
    /// locally. `checkpoint_local`'s weight, because it is the same kind of fact.
    pub resource_local: i64,
}

// There is deliberately no bid *delay* here, and there used to be two fields and a function for
// one. ADR-0006 step 3 had nodes broadcast bids after a score-proportional delay so the keenest
// spoke first and the negotiation usually cost one message; the implementation amendment on that
// ADR replaced it before it ever shipped, because a transport that addresses peers directly lets
// the **arbiter ask and nodes answer** — same messages, no delay, and a round the asker can bound,
// which is what ADR-0014 needs when somebody is standing at the keyboard.
//
// What was left was `bid_delay`, `max_bid_delay` and `score_ceiling`: called by nothing but their
// own test, and carrying a doc comment that described the delay in the present tense. Dead code
// that asserts a mechanism is worse than dead code, because a reader learns something false about
// the protocol — and ADR-0006's consequences still cited the delay as what mitigates a bid storm,
// which is a mitigation by a mechanism that is not there.
//
// If bids ever travel again — `winner()` stays deterministic precisely so they can — a delay is a
// decision to make then, with its reasoning fresh, not a field to rediscover.

impl Default for BidWeights {
    fn default() -> Self {
        BidWeights {
            stability: 10,
            // The single biggest real-world difference, and the one only the node itself
            // knows about — worth more than a machine twice the size.
            workspace_warm: 40,
            checkpoint_local: 20,
            per_core_doubling: 6,
            per_gib_doubling: 4,
            load_penalty: 30,
            battery_penalty: 40,
            metered_penalty: 20,
            migration_penalty: 30,
            account_pressure: 5,
            preferred: 60,
            resource_local: 20,
        }
    }
}

/// Decide whether this node wants a run, and how badly.
pub fn evaluate(
    run: &Run,
    me: &NodeView,
    local: &LocalFacts,
    view: &ClusterView,
    weights: &BidWeights,
    now: Millis,
) -> Result<Bid, NoBid> {
    // The agent half, or `None` for a task — asked once, here, so that each check below says
    // for itself whether it has anything to ask about. **Three do not**, and they are exactly
    // the three a task has no referent for: the repository, the account's rate limit, and the
    // transcript's agent version. Everything else in this function is about a *run* — the
    // owner's standing answer, the demand budget, observed pressure, the constraint tree — and
    // applies to both tiers unchanged.
    //
    // There was a `NoBid::UnsupportedWork` here, written when nothing could construct a
    // `Work::Task` and honest about that in its own comment — and it outlived the *yet*: with
    // `--task` submitting and the supervisor running one, this was the last door still shut, so
    // the cheap tier ran on a node with `cluster.enabled = false` and was refused at the
    // keyboard everywhere else, which is every default node. **The variant is gone rather than
    // kept for form**: nothing constructs it, `NoBid` never crosses the wire (only its
    // `to_string()` does), and a refusal this binary cannot give is not a refusal.
    let agent = run.spec.work.agent();

    if run.holder() == Some(me.id) {
        return Err(NoBid::AlreadyHolder);
    }

    let caps = &me.capabilities;

    // Before anything else about this node: can it even get the code? A repo named by local
    // path exists on exactly the machines that hold that path, and the run has to be refused
    // here rather than accepted and discovered at resume — which happens, by construction, at
    // the hour nobody is watching.
    //
    // Asked only of work that names a repository. `LocalFacts::repo_reach` is answered for
    // a task too — `reach("")` — and the answer is `NotHere`, so hoisting this above the
    // `if let` would refuse every task on every node for a repo it never asked for. A field
    // that has no meaning for this row must not be read as though it did.
    //
    // The reason travels with the refusal rather than being re-derived from the repo string:
    // `Portability` cannot tell *nothing of that name here* from *here, and not a repository*,
    // and the second was reported as the first by the very machine the path was on.
    if let Some(work) = agent {
        if !local.repo_reach.is_obtainable() {
            return Err(NoBid::RepoUnavailable {
                repo: work.workspace.repo.clone(),
                reach: local.repo_reach,
            });
        }
        // The owner's own ceiling on a workspace that arrives as bytes (ADR-0061 §4). Asked
        // beside the repo's reachability because it is the same kind of fact — *can this node
        // have the workspace at all* — and asked only when there is a size to ask about: an
        // archive submitted without one is refused by nothing here and discovered at
        // acquisition, which is slower and still correct.
        //
        // The fleet-wide cap is **not** checked here. It is refused before the run exists
        // (`StoreArchive`), so a run carrying an over-cap archive is not a thing that reaches a
        // bid round; and if one ever did, saying *this node's owner takes at most …* about a
        // limit nobody set would name the wrong cause.
        if let (Some(bytes), Some(limit)) =
            (work.workspace.archive_bytes, me.policy.max_archive_bytes)
        {
            if bytes > limit {
                return Err(NoBid::ArchiveTooLarge { bytes, limit });
            }
        }
    }

    // Owner policy first: it is the cheapest check and the most likely reason a phone
    // declines. Reporting it separately from ineligibility is the whole point.
    //
    // Being *full* is the one refusal that is not a no. It is this node's own queue, it
    // empties by itself, and the alternative — declining so the run stays pending and hopes —
    // is what the overnight case dies of. So a full node offers anyway and says when
    // (ADR-0006). Every other refusal here is a condition of the device, not a queue: a phone
    // on battery has not promised to be plugged in later, and a node that pretended otherwise
    // would be committing on behalf of its owner.
    let held_here = view.occupancy(&me.id);
    let load = NodeLoad {
        // This node's own runs plus the rest of the device's. A bid that ignored the ledger
        // would say "starting now" and then be held on arrival, which is the shape of wrong
        // answer this project spends most of its comments avoiding.
        held: Occupancy {
            runs: held_here.runs.saturating_add(local.device_committed.runs),
            shares: held_here
                .shares
                .saturating_add(local.device_committed.shares),
        },
        // Zero for a task, and nothing reads it: `admits` asks this only inside the arm that
        // has an agent to ask about, because a count of runs on an agent is not a number a
        // task has a smaller version of.
        held_for_agent: agent.map_or(0, |work| view.running_count_for_agent(&me.id, &work.agent)),
        cpu_percent: local.cpu_load_percent,
        thermal: local.thermal,
    };
    // Summed from the view rather than from this node, because the thing being counted belongs
    // to the account and not to the machine. Best-effort by construction; see `AccountUse`.
    let account = agent.and_then(|work| view.account_use(&me.id, &work.agent));
    let busy = match me.policy.admits(
        caps,
        run.spec.work.tier(),
        run.spec.demand,
        &load,
        account.as_ref(),
    ) {
        Ok(()) => None,
        Err(Refusal::AtCapacity { running, .. }) => Some(running),
        Err(Refusal::AgentAtCapacity(_)) => Some(load.held_for_agent),
        // Full by the *budget* is the same kind of full: this node accepted the runs that
        // spent it, so it knows the queue and commits like any other busy node. What it
        // reports as `behind` is runs rather than shares, because that is what the operator
        // is waiting for — "behind 8 shares" answers a question nobody asked.
        Err(Refusal::BudgetFull { .. }) => Some(load.held.runs),
        // An account at its ceiling is a queue that empties too — the runs ahead of it are this
        // fleet's own, on this account, and they finish. So it commits like any other full node
        // rather than refusing, and what steers a fresh run towards a node on a *different*
        // account is scoring (`BidWeights::account_pressure`), not this refusal.
        Err(refusal @ Refusal::AccountAtCapacity { running, .. }) => {
            // Except a ceiling of zero, which is "never" rather than "not yet": nothing is
            // going to finish and make room, so committing would be a promise nobody can keep.
            if running == 0 {
                return Err(NoBid::Refused(refusal));
            }
            Some(running)
        }
        // Pressure is not a queue this node owns, so it does not commit — it defers, if the
        // run has slack to spend, and otherwise sends it elsewhere. See `NoBid::Busy`.
        Err(pressure @ (Refusal::UnderPressure { .. } | Refusal::TooHot { .. })) => {
            return Err(match run.slack(now).remaining() {
                Some(left) if left >= PRESSURE_RETRY => NoBid::Busy {
                    retry_after: PRESSURE_RETRY,
                    reason: pressure,
                },
                _ => NoBid::Refused(pressure),
            })
        }
        Err(other) => return Err(NoBid::Refused(other)),
    };

    // The requirement, asked of *this* node by id — the only way a `Constraint::Node` clause
    // can be answered at all.
    let named = |id: NodeId| {
        view.node(&id)
            .map_or_else(|| id.short(), NodeView::display_name)
    };
    let explain = run.spec.constraint.explain_on_naming(me.id, caps, &named);
    if !explain.satisfied {
        return Err(NoBid::Ineligible(explain.failures()));
    }
    // …and then the requirement as it stands at `now`, which is the same tree with the
    // preference ANDed in while a stated hold lasts (ADR-0063 §4). Through `eligibility`, the one
    // reading every other placement reader uses; asked second so that a node which fails the
    // requirement itself says *that*, and a node failing only the hold says it is a hold.
    if !run.spec.eligibility(now).matches_on(me.id, caps) {
        return Err(NoBid::Held {
            left: run
                .spec
                .hold_until
                .map_or(Millis(0), |until| until.saturating_sub(now)),
        });
    }

    // A transcript written by a newer agent may not be readable by an older one, so this
    // is checked before placement rather than discovered at resume. See ADR-0003.
    // Nothing to ask for a task: it has no transcript, so ADR-0019 §2 keeps it out of
    // `Checkpointing` altogether and there is no captured agent version to be older than.
    if let (Some(cp), Some(work)) = (&run.checkpoint, agent) {
        if let Some(details) = caps.agent_details(&work.agent) {
            if !version::at_least(&details.version, &cp.agent_version) {
                return Err(NoBid::AgentTooOld {
                    have: details.version.clone(),
                    need: cp.agent_version.clone(),
                });
            }
        }
    }

    let terms = score_terms(run, me, local, view, weights);
    Ok(Bid {
        run: run.id,
        node: me.id,
        epoch: run.epoch,
        score: terms.total(),
        terms: Some(terms),
        // The rate limit is the *account's*, and a task does not spend one — so a node blocked
        // until 09:00 can still run a shell script now, and saying `NotBefore` about one would
        // park the cheap tier behind the expensive tier's bill. Asked of the work rather than
        // of the machine, for the reason the three checks above are.
        available: match (
            busy,
            agent
                .and(local.account_limited_until)
                .filter(|at| *at > now),
        ) {
            // The rate limit first, when both hold. A queue empties when work here finishes and
            // a rate limit does not care what happens here at all, so the later of the two is
            // always the limit — and telling somebody about the count would send them to watch a
            // slot that will free long before the run can use it (ADR-0029).
            (_, Some(at)) => Availability::NotBefore { at },
            (None, None) => Availability::Now,
            (Some(behind), None) => Availability::WhenFree { behind },
        },
        submitted_at: now,
    })
}

/// Where to keep a second copy of this run's checkpoint.
///
/// ADR-0016: a checkpoint that exists only on the node that died cannot be migrated from, so
/// capture pushes to somebody. Whom is a placement question with a different shape from
/// bidding — nobody is committing to anything, and the cost is bytes rather than an agent —
/// so it is decided here rather than by running a round.
///
/// Two tiers, in order:
///
/// 1. **A node that could plausibly take the run over**: alive, and eligible by the run's own
///    constraints. Choosing the likely successor means the common migration needs no fetch at
///    all, which is the whole reason to prefer it.
/// 2. **Anybody else that is alive.** Weaker — the run cannot move there — but a copy that
///    cannot host the run still survives the holder, and durability is the thing being bought.
///    The ADR does not say this; it also does not accept losing the only copy because the
///    fleet happens to be a laptop and a phone.
///
/// Stability breaks ties, then the node id, so every node computes the same answer from the
/// same view. `None` means there is nobody: a fleet of one, which is honest and has to be
/// reported rather than hidden.
#[must_use]
pub fn replica_for(run: &Run, view: &ClusterView, holder: NodeId) -> Option<NodeId> {
    let candidates = || {
        view.nodes
            .values()
            .filter(|n| n.id != holder && n.status.is_available())
    };

    let best = |mut nodes: Vec<&NodeView>| -> Option<NodeId> {
        // Descending stability, then ascending id: deterministic, and it prefers the machine
        // most likely to still be there tomorrow.
        nodes.sort_by_key(|n| (std::cmp::Reverse(n.capabilities.stability), n.id));
        nodes.first().map(|n| n.id)
    };

    let eligible: Vec<&NodeView> = candidates()
        // The requirement without any hold: this is *who could plausibly take over*, and a copy
        // on the laptop is still worth having for a run held for the desktop, whose hold may
        // have lapsed by the time anybody needs it.
        .filter(|n| run.spec.constraint.matches_on(n.id, &n.capabilities))
        .collect();
    best(eligible).or_else(|| best(candidates().collect()))
}

/// How much this node wants the run. Higher is keener. The sum of [`score_terms`].
#[must_use]
pub fn score(
    run: &Run,
    me: &NodeView,
    local: &LocalFacts,
    view: &ClusterView,
    weights: &BidWeights,
) -> Score {
    score_terms(run, me, local, view, weights).total()
}

/// A score, term by term — what [`score`] adds up, kept so that a report can say *which* terms
/// decided a round from the numbers the round read (ADR-0063 §7) rather than from a second copy
/// of this arithmetic.
///
/// Signed as they are applied: a penalty is negative here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScoreTerms {
    pub stability: i64,
    pub workspace_warm: i64,
    pub checkpoint_local: i64,
    pub hardware: i64,
    pub load: i64,
    pub battery: i64,
    pub metered: i64,
    pub migration: i64,
    pub account_pressure: i64,
    /// `weights.preferred` when this node satisfied the run's preference, else zero.
    pub preferred: i64,
    /// `weights.resource_local` per granted resource held here.
    pub resource_local: i64,
}

impl ScoreTerms {
    #[must_use]
    pub fn total(&self) -> Score {
        Score(
            self.stability
                + self.workspace_warm
                + self.checkpoint_local
                + self.hardware
                + self.load
                + self.battery
                + self.metered
                + self.migration
                + self.account_pressure
                + self.preferred
                + self.resource_local,
        )
    }

    /// Did this node satisfy the run's stated preference?
    #[must_use]
    pub fn is_preferred(&self) -> bool {
        self.preferred > 0
    }

    /// The named terms, non-zero only, for a sentence that says what moved a score.
    #[must_use]
    pub fn named(&self) -> Vec<(&'static str, i64)> {
        [
            ("stability", self.stability),
            ("warm workspace", self.workspace_warm),
            ("checkpoint here", self.checkpoint_local),
            ("hardware", self.hardware),
            ("load", self.load),
            ("battery", self.battery),
            ("metered link", self.metered),
            ("already held elsewhere", self.migration),
            ("account in use", self.account_pressure),
            ("preferred", self.preferred),
            ("resources here", self.resource_local),
        ]
        .into_iter()
        .filter(|(_, v)| *v != 0)
        .collect()
    }
}

/// [`score`], term by term.
#[must_use]
pub fn score_terms(
    run: &Run,
    me: &NodeView,
    local: &LocalFacts,
    view: &ClusterView,
    weights: &BidWeights,
) -> ScoreTerms {
    let caps = &me.capabilities;
    let mut terms = ScoreTerms {
        stability: weights.stability
            * match caps.stability {
                Stability::Ephemeral => 0,
                Stability::Transient => 1,
                Stability::Stable => 2,
            },
        ..ScoreTerms::default()
    };

    if local.workspace_warm {
        terms.workspace_warm = weights.workspace_warm;
    }
    if local.checkpoint_local {
        terms.checkpoint_local = weights.checkpoint_local;
    }

    terms.hardware = weights.per_core_doubling * i64::from(caps.cpu_cores.max(1).ilog2())
        + weights.per_gib_doubling * i64::from(memory_doublings(caps.memory_mb));
    if let Some(load) = local.cpu_load_percent {
        terms.load = -(weights.load_penalty * i64::from(load.min(100)) / 100);
    }

    // Don't cook someone's phone: the penalty grows as the battery drains.
    if let Some(percent) = caps.power.battery_percent() {
        if !caps.power.on_mains() {
            terms.battery = -(weights.battery_penalty * i64::from(100 - percent.min(100)) / 100);
        }
    }
    // Only a link somebody could actually say costs money. Scoring a node down because nobody
    // asked would spend the penalty on every device in the fleet (ADR-0045 §4).
    if caps.metered_network.known_metered() {
        terms.metered = -weights.metered_penalty;
    }

    // Moving a run that is fine where it is should require a clearly better offer.
    if let Some(holder) = run.holder() {
        if holder != me.id && view.node(&holder).is_some_and(|n| n.status.is_available()) {
            terms.migration = -weights.migration_penalty;
        }
    }

    // Nodes sharing an agent account share its rate limit, so spread load across accounts.
    // Skipped entirely for work with no agent: there is no account, so there is no rate limit
    // to spread and no penalty to apply — a zero here would be a score, and absence is not one.
    if let Some(account) = run
        .spec
        .agent()
        .and_then(|work| caps.agent(&work.agent))
        .and_then(|a| a.identity.as_ref())
    {
        let pressure: i64 = view
            .nodes
            .values()
            .filter(|n| {
                run.spec
                    .agent()
                    .and_then(|work| n.capabilities.agent(&work.agent))
                    .and_then(|a| a.identity.as_ref())
                    == Some(account)
            })
            .map(|n| i64::from(view.running_count(&n.id)))
            .sum();
        terms.account_pressure = -(weights.account_pressure * pressure);
    }

    // ADR-0063 §1: binary, and asked of this node by id — a node matching two of three clauses
    // scores as one matching none, because partial credit is a sum the submitter never wrote.
    // `Always` adds nothing to anybody rather than the weight to everybody: the two rank the
    // same and only one of them reads true in `explain`.
    if run.spec.prefer != crate::constraint::Constraint::Always
        && run.spec.prefer.matches_on(me.id, caps)
    {
        terms.preferred = weights.preferred;
    }

    // ADR-0063 §5: what `CanUse` asks, per grant, about this node — the same predicate the
    // proxy's holder answers, so a resource that cannot be used here scores nothing here.
    let held = run
        .spec
        .resources
        .iter()
        .filter(|service| {
            crate::constraint::Constraint::CanUse {
                service: (*service).clone(),
            }
            .matches(caps)
        })
        .count();
    terms.resource_local = weights.resource_local * i64::try_from(held).unwrap_or(i64::MAX / 2);

    terms
}

/// How many times a machine's memory doubles from 1 GiB, to the **nearest** doubling.
///
/// It was `ilog2` of whole GiB, which floors twice: a Linux "16 GiB" machine reports ~15.5 GiB
/// usable (15 862 MB on the laptop this was measured on), becomes 15, and scores three doublings —
/// the same as an 8 GiB Mac, which reports its memory whole. ADR-0063 §2's walk found it. Nearest
/// in log space: `k + 1` once memory reaches `√2 · 2^k` GiB, decided in integers (squares) so
/// every node computes the same number for the same machine.
#[must_use]
pub fn memory_doublings(memory_mb: u64) -> u32 {
    let gib_floor = (memory_mb / 1024).max(1);
    let k = gib_floor.ilog2();
    let base_mb = u128::from(1024u64) << k;
    let mb = u128::from(memory_mb);
    if mb * mb >= 2 * base_mb * base_mb {
        k + 1
    } else {
        k
    }
}

/// Pick the winning bid: **anybody who can start now**, then highest score, ties broken by
/// lowest node id.
///
/// Availability outranks score outright rather than being worth some number of points. A node
/// that can start now is doing the run *today* and a keener one is doing it after whatever it
/// is already busy with — no score difference between two machines on the same LAN is worth
/// that, and expressing it as a bonus would mean picking a number that is wrong for some
/// fleet. Score decides among nodes that are equally able to begin, which is what it is good
/// at (ADR-0006).
///
/// Deterministic, so every node that has seen the same bids agrees on the winner without
/// exchanging another message.
#[must_use]
pub fn winner(bids: &[Bid]) -> Option<&Bid> {
    bids.iter()
        .filter(|b| !bids.iter().any(|o| better(o, b)))
        .min_by_key(|b| b.node)
}

fn better(a: &Bid, b: &Bid) -> bool {
    (a.available.is_now(), a.score, std::cmp::Reverse(a.node))
        > (b.available.is_now(), b.score, std::cmp::Reverse(b.node))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Machines score as the size they are sold as, which is what the doubling is for.
    #[test]
    fn memory_rounds_to_the_nearest_doubling() {
        assert_eq!(
            memory_doublings(15_862),
            4,
            "a Linux 16 GiB laptop is a 16 GiB machine"
        );
        assert_eq!(memory_doublings(8_192), 3, "an 8 GiB Mac");
        assert_eq!(memory_doublings(7_295), 3, "the phone's 7.1 GiB is an 8");
        assert_eq!(memory_doublings(63_400), 6, "a Linux 64 GiB desktop");
        assert_eq!(
            memory_doublings(2_474),
            1,
            "the emulator's 2.4 GiB is nearer 2 than 4"
        );
        assert_eq!(memory_doublings(512), 0, "under a GiB is none");
        // The boundary: √2 · 8 GiB ≈ 11.31 GiB.
        assert_eq!(memory_doublings(11_500), 3);
        assert_eq!(memory_doublings(11_600), 4);
    }
    use crate::capability::{
        AgentDetails, AgentKind, Arch, Capabilities, Capability, DeviceClass, Os, PowerSource,
    };
    use crate::capacity::Demand;
    use crate::constraint::Constraint;
    use crate::id::BlobHash;
    use crate::policy::WorkPolicy;
    use crate::run::{AgentWork, PermissionMode, Restartability, RunSpec, Work, WorkspaceSpec};
    use std::collections::BTreeSet;

    fn node_id(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    fn caps_for(class: DeviceClass, cores: u32, mem_gb: u64) -> Capabilities {
        let (os, arch) = match class {
            DeviceClass::Phone => (Os::Android, Arch::Aarch64),
            _ => (Os::Linux, Arch::X86_64),
        };
        let mut caps = Capabilities::empty(os, arch, class);
        caps.cpu_cores = cores;
        caps.memory_mb = mem_gb * 1024;
        caps.power = PowerSource::Ac;
        caps.add(claude(AgentKind::ClaudeCode, "2.10.0"));
        caps
    }

    fn claude(kind: AgentKind, version: &str) -> Capability {
        Capability::agent(
            kind,
            AgentDetails {
                version: version.into(),
                models: vec![crate::capability::Model::named("claude-opus-5")],
                max_concurrent: 4,
            },
            true,
        )
    }

    fn checkpoint_for(_holder: NodeId) -> crate::run::Checkpoint {
        crate::run::Checkpoint {
            session_id: None,
            transcript: BlobHash::from_bytes([1; 32]),
            bundle: None,
            patch: None,
            base_commit: "0f0f".into(),
            turns: 1,
            taken_at: Millis(0),
            agent_version: "2.11.0".into(),
            replicas: BTreeSet::new(),
        }
    }

    fn node_view(b: u8, class: DeviceClass, cores: u32, mem_gb: u64) -> NodeView {
        NodeView::new(
            node_id(b),
            caps_for(class, cores, mem_gb),
            WorkPolicy::for_class(class),
            Millis(0),
        )
    }

    fn a_run() -> Run {
        Run::new(
            RunId::from_bytes([1; 16]),
            RunSpec {
                work: Work::Agent(AgentWork {
                    agent: AgentKind::ClaudeCode,
                    model: None,
                    prompt: "p".into(),
                    workspace: WorkspaceSpec {
                        repo: "/r".into(),
                        archive_bytes: None,
                        git_ref: None,
                        branch: None,
                    },
                    permission_mode: PermissionMode::Ask,
                    allow: crate::ToolAllowlist::default(),
                    max_turns: None,
                    ask: crate::AskPolicy::Never,
                }),
                constraint: Constraint::agent_ready(AgentKind::ClaudeCode, None),
                restartability: Restartability::Resumable,
                priority: 0,
                queue: false,
                deadline: None,
                demand: Demand::Normal,
                notify: Default::default(),
                notices: Default::default(),
                resources: Vec::new(),
                prefer: crate::Constraint::Always,
                hold_until: None,
                parent: None,
            },
            node_id(9),
            Millis(0),
        )
    }

    fn view_with(nodes: Vec<NodeView>) -> ClusterView {
        let mut cv = ClusterView::new(node_id(1));
        for n in nodes {
            cv.upsert_node(n);
        }
        cv
    }

    /// A task on a node that nominates its service, with nothing agent-shaped about it.
    fn a_task() -> Run {
        let mut run = a_run();
        run.spec.work = Work::Task(crate::TaskWork {
            service: crate::Service::Other("watch-api".into()),
            args: vec!["--once".into()],
        });
        run.spec.restartability = Restartability::Idempotent;
        run.spec.constraint = Constraint::HasService {
            service: crate::Service::Other("watch-api".into()),
            role: Some(crate::Role::Execute),
        };
        run
    }

    /// The capability an owner's `[[tasks]]` entry advertises (ADR-0019 §1).
    fn nominates_watch_api(view: &mut NodeView) {
        view.capabilities.add(Capability::new(
            "task:watch",
            crate::Service::Other("watch-api".into()),
            [crate::Role::Execute],
        ));
    }

    #[test]
    fn a_task_is_bid_for_by_a_node_that_nominates_its_service() {
        // **The door that was still shut.** `--task` submits, the supervisor runs one, and
        // `evaluate` refused every task at its first line — so the cheap tier ran on a node
        // with `cluster.enabled = false` and was refused at the keyboard on every other,
        // which is every default node. Measured on one daemon before the fix: `walker  this
        // node cannot host task work`.
        let weights = BidWeights::default();
        let mut desktop = node_view(3, DeviceClass::Desktop, 32, 64);
        nominates_watch_api(&mut desktop);
        let view = view_with(vec![desktop.clone()]);

        let bid = evaluate(
            &a_task(),
            &desktop,
            &LocalFacts::default(),
            &view,
            &weights,
            Millis(0),
        )
        .expect("a nominated task is work this node can do");
        assert_eq!(bid.available, Availability::Now);

        // …and a node that nominates nothing says which clause it failed, rather than
        // refusing the whole tier.
        let bare = node_view(4, DeviceClass::Desktop, 32, 64);
        assert!(matches!(
            evaluate(
                &a_task(),
                &bare,
                &LocalFacts::default(),
                &view,
                &weights,
                Millis(0)
            ),
            Err(NoBid::Ineligible(_))
        ));
    }

    #[test]
    fn a_task_is_not_refused_for_a_repository_it_never_named() {
        // `LocalFacts::repo_reach` is answered for a task too — `reach("")` — and the answer
        // is `NotHere`. Reading it for work that names no repository refuses every task
        // on every node, for a repo nobody asked about.
        let weights = BidWeights::default();
        let mut desktop = node_view(3, DeviceClass::Desktop, 32, 64);
        nominates_watch_api(&mut desktop);
        let view = view_with(vec![desktop.clone()]);
        let no_repo = LocalFacts {
            repo_reach: RepoReach::NotHere,
            ..LocalFacts::default()
        };

        assert!(evaluate(&a_task(), &desktop, &no_repo, &view, &weights, Millis(0)).is_ok());
        // The same facts, for work that *does* name one, still refuse.
        assert!(matches!(
            evaluate(&a_run(), &desktop, &no_repo, &view, &weights, Millis(0)),
            Err(NoBid::RepoUnavailable { .. })
        ));
    }

    #[test]
    fn an_agents_rate_limit_does_not_delay_a_task() {
        // A task spends no tokens (ADR-0019 §2), so an account blocked until 09:00 says
        // nothing about whether a shell script can run now. Answering `NotBefore` here would
        // park the cheap tier behind the expensive tier's bill.
        let weights = BidWeights::default();
        let mut desktop = node_view(3, DeviceClass::Desktop, 32, 64);
        nominates_watch_api(&mut desktop);
        let view = view_with(vec![desktop.clone()]);
        let limited = LocalFacts {
            account_limited_until: Some(Millis::from_mins(60)),
            ..LocalFacts::default()
        };

        let bid = evaluate(&a_task(), &desktop, &limited, &view, &weights, Millis(0))
            .expect("a task has no account to be limited on");
        assert_eq!(bid.available, Availability::Now);

        // The same limit, on work that does spend it.
        let agent = evaluate(&a_run(), &desktop, &limited, &view, &weights, Millis(0))
            .expect("an agent run is still placeable, just not yet");
        assert_eq!(
            agent.available,
            Availability::NotBefore {
                at: Millis::from_mins(60)
            }
        );
    }

    #[test]
    fn a_phone_on_battery_bids_for_the_light_watcher_and_refuses_the_rest() {
        // ADR-0019's scenario, in the round rather than in the policy: the owner has said they
        // trust light work on battery, and the bid — which is what actually decides where work
        // goes — has to read that. It does because `admits` takes the run's demand, so nothing
        // in this file needed to know about the light form at all.
        let weights = BidWeights::default();
        let mut phone = node_view(5, DeviceClass::Phone, 8, 8);
        nominates_watch_api(&mut phone);
        phone.capabilities.power = PowerSource::Battery {
            percent: 55,
            charging: false,
        };
        phone.policy = crate::WorkPolicy {
            accept: crate::AcceptWork::WhenCharging,
            min_battery_percent: Some(40),
            light: crate::LightWork {
                accept: Some(crate::AcceptWork::Always),
                ..crate::LightWork::default()
            },
            ..crate::WorkPolicy::for_class(DeviceClass::Phone)
        };
        let view = view_with(vec![phone.clone()]);

        let mut watcher = a_task();
        watcher.spec.demand = Demand::Light;
        assert!(
            evaluate(
                &watcher,
                &phone,
                &LocalFacts::default(),
                &view,
                &weights,
                Millis(0)
            )
            .is_ok(),
            "the light watcher is the work this device was enrolled for"
        );

        // …and the same device, the same second, still says no to everything heavier.
        assert_eq!(
            evaluate(
                &a_task(),
                &phone,
                &LocalFacts::default(),
                &view,
                &weights,
                Millis(0)
            ),
            Err(NoBid::Refused(Refusal::NotCharging))
        );
    }

    #[test]
    fn the_owners_standing_no_applies_to_a_task_as_much_as_to_an_agent() {
        // What is *not* skipped. `accept`, the battery floor and the demand budget are facts
        // about the device, and a phone that will not work on battery will not run a shell
        // script on battery either — skipping those with the agent clauses would have made
        // the cheap tier a way around the owner's own answer.
        let weights = BidWeights::default();
        let mut phone = node_view(5, DeviceClass::Phone, 8, 8);
        nominates_watch_api(&mut phone);
        phone.capabilities.power = PowerSource::Battery {
            percent: 20,
            charging: false,
        };
        phone.policy = WorkPolicy {
            accept: crate::AcceptWork::WhenCharging,
            ..WorkPolicy::for_class(DeviceClass::Phone)
        };
        let view = view_with(vec![phone.clone()]);

        assert_eq!(
            evaluate(
                &a_task(),
                &phone,
                &LocalFacts::default(),
                &view,
                &weights,
                Millis(0)
            ),
            Err(NoBid::Refused(Refusal::NotCharging))
        );
    }

    #[test]
    fn a_pressured_node_defers_when_the_run_can_afford_to_wait_and_declines_when_it_cannot() {
        // The two halves of ADR-0013's judgeable deferral, on one node with one load average.
        // What differs is the run: a deferral is worth taking if the run still lands before it
        // is due, and if there is nothing left to spend the answer has to be "look elsewhere
        // now" — being picky is how a late run gets later.
        let weights = BidWeights::default();
        let desktop = node_view(3, DeviceClass::Desktop, 32, 64);
        let view = view_with(vec![desktop.clone()]);
        let thrashing = LocalFacts {
            cpu_load_percent: Some(95),
            ..LocalFacts::default()
        };

        let mut overnight = a_run();
        overnight.spec.deadline = Some(Millis::from_mins(540));
        assert_eq!(
            evaluate(&overnight, &desktop, &thrashing, &view, &weights, Millis(0)),
            Err(NoBid::Busy {
                retry_after: PRESSURE_RETRY,
                reason: Refusal::UnderPressure {
                    load_percent: 95,
                    budget_free: 100,
                },
            })
        );

        // No deadline means the moment of submission, so this run is already overdue — and
        // that is exactly the run that must not be told to wait a minute for this machine.
        let asap = a_run();
        assert!(matches!(
            evaluate(&asap, &desktop, &thrashing, &view, &weights, Millis(1)),
            Err(NoBid::Refused(Refusal::UnderPressure { .. }))
        ));
    }

    #[test]
    fn a_node_full_by_budget_still_commits_rather_than_deferring() {
        // The line between the two: the budget was spent by runs *this* node accepted, so it
        // owns that queue and knows it empties — a commitment, like any other busy node
        // (ADR-0006). Pressure is somebody else's work on the same machine, and that is the
        // only thing this node refuses to promise about.
        let weights = BidWeights::default();
        let laptop = node_view(2, DeviceClass::Laptop, 8, 16);
        let mut heavy = a_run();
        heavy.spec.demand = Demand::Heavy;
        heavy
            .assign(node_id(2), Millis(0), Millis(60_000))
            .expect("assign");

        let mut view = view_with(vec![laptop.clone()]);
        view.merge_run(heavy, node_id(2));

        let bid = evaluate(
            &a_run(),
            &laptop,
            &LocalFacts::default(),
            &view,
            &weights,
            Millis(0),
        )
        .expect("a busy node bids");
        assert_eq!(bid.available, Availability::WhenFree { behind: 1 });
    }

    /// ADR-0063 §2's table, as arithmetic on `BidWeights::default()`: the laptop somebody is
    /// sat at, preferred, against a desktop twice its size on every axis. A preference outweighs
    /// any single reason to go elsewhere and yields to two. **Arithmetic, not a walk** — the ADR
    /// says so, and the walk is still owed; if it moves the numbers, this test moves with them.
    #[test]
    fn a_preference_beats_one_reason_to_go_elsewhere_and_yields_to_two() {
        let weights = BidWeights::default();
        let mut laptop = node_view(2, DeviceClass::Laptop, 8, 16);
        let desktop = node_view(3, DeviceClass::Desktop, 32, 64);
        let view = view_with(vec![laptop.clone(), desktop.clone()]);
        let mut run = a_run();
        run.spec.prefer = Constraint::Node(laptop.id);

        let terms =
            |node: &NodeView, local: &LocalFacts| score_terms(&run, node, local, &view, &weights);
        let idle = LocalFacts::default();
        let there = terms(&desktop, &idle);
        assert!(!there.is_preferred());
        assert_eq!(
            there.total(),
            Score(74),
            "stability 20, cores 30, memory 24"
        );

        let here = terms(&laptop, &idle);
        assert!(here.is_preferred());
        assert_eq!(here.total(), Score(44 + 60));
        assert!(
            here.total() > there.total(),
            "on mains, the preference wins"
        );

        // One reason to go elsewhere: on battery at half charge.
        laptop.capabilities.power = PowerSource::Battery {
            percent: 50,
            charging: false,
        };
        assert!(
            terms(&laptop, &idle).total() > there.total(),
            "…and still wins"
        );

        // Two: on battery *and* flat out.
        let busy = LocalFacts {
            cpu_load_percent: Some(100),
            ..LocalFacts::default()
        };
        assert!(
            terms(&laptop, &busy).total() < there.total(),
            "a preference that always won would be a pin"
        );
    }

    /// ADR-0063 §1: a node that does not satisfy the preference still bids — it is scored down
    /// relative to the one that does, and never refused.
    #[test]
    fn a_preference_is_scored_and_never_refuses_anybody() {
        let weights = BidWeights::default();
        let laptop = node_view(2, DeviceClass::Laptop, 8, 16);
        let desktop = node_view(3, DeviceClass::Desktop, 32, 64);
        let view = view_with(vec![laptop.clone(), desktop.clone()]);
        let mut run = a_run();
        run.spec.prefer = Constraint::Os(crate::capability::Os::MacOs);

        let bid = evaluate(
            &run,
            &desktop,
            &LocalFacts::default(),
            &view,
            &weights,
            Millis(0),
        )
        .expect("a node that is not a Mac still bids");
        assert_eq!(
            bid.score,
            score(&run, &desktop, &LocalFacts::default(), &view, &weights),
            "and scores exactly as it would with no preference at all"
        );
        run.spec.prefer = Constraint::Always;
        assert_eq!(
            bid.score,
            score(&run, &desktop, &LocalFacts::default(), &view, &weights)
        );
    }

    /// ADR-0063 §4: until the stated hold, the preference is a requirement — the desktop is
    /// ineligible for a run held for the laptop, and names the clause — and after it, it is a
    /// preference again, with nothing having to happen at the instant.
    #[test]
    fn a_hold_is_a_requirement_until_its_stated_time_and_a_preference_after() {
        let weights = BidWeights::default();
        let laptop = node_view(2, DeviceClass::Laptop, 8, 16);
        let desktop = node_view(3, DeviceClass::Desktop, 32, 64);
        let view = view_with(vec![laptop.clone(), desktop.clone()]);
        let mut run = a_run();
        run.spec.prefer = Constraint::Node(laptop.id);
        run.spec.queue = true;
        run.spec.hold_until = Some(Millis(10_000));
        run.spec
            .check()
            .expect("a stated hold, queued, with a preference");

        let held = evaluate(
            &run,
            &desktop,
            &LocalFacts::default(),
            &view,
            &weights,
            Millis(9_999),
        );
        assert_eq!(
            held,
            Err(NoBid::Held { left: Millis(1) }),
            "the desktop is held off while the hold lasts, and says it is a hold"
        );
        assert!(
            evaluate(
                &run,
                &laptop,
                &LocalFacts::default(),
                &view,
                &weights,
                Millis(9_999)
            )
            .is_ok(),
            "and the preferred node is exactly as eligible as before"
        );
        assert!(
            evaluate(
                &run,
                &desktop,
                &LocalFacts::default(),
                &view,
                &weights,
                Millis(10_000)
            )
            .is_ok(),
            "at the stated time the hold is over"
        );
    }

    /// ADR-0063 §5: a node holding a resource the run was granted is worth `resource_local`
    /// more per grant — and only for a resource that is usable there.
    #[test]
    fn a_granted_resource_held_by_the_bidder_is_scored() {
        let weights = BidWeights::default();
        let mut holder = node_view(2, DeviceClass::Desktop, 8, 16);
        let other = node_view(3, DeviceClass::Desktop, 8, 16);
        let mut mailbox = Capability::new(
            "mail",
            crate::Service::Email,
            [crate::Role::Resource {
                access: crate::capability::Access::Read,
            }],
        );
        mailbox.authenticated = true;
        holder.capabilities.add(mailbox);
        let view = view_with(vec![holder.clone(), other.clone()]);
        let mut run = a_run();
        run.spec.resources = vec![crate::Service::Email];

        let local = LocalFacts::default();
        let there = score_terms(&run, &holder, &local, &view, &weights);
        let elsewhere = score_terms(&run, &other, &local, &view, &weights);
        assert_eq!(there.resource_local, 20);
        assert_eq!(elsewhere.resource_local, 0);
        assert_eq!(there.total().0 - elsewhere.total().0, 20);
    }

    #[test]
    fn a_warm_workspace_beats_a_bigger_cold_machine() {
        // The reason bidding exists: only the node itself knows the repo is already there.
        let weights = BidWeights::default();
        let run = a_run();
        let laptop = node_view(2, DeviceClass::Laptop, 8, 16);
        let desktop = node_view(3, DeviceClass::Desktop, 32, 64);
        let view = view_with(vec![laptop.clone(), desktop.clone()]);

        let warm = LocalFacts {
            workspace_warm: true,
            ..LocalFacts::default()
        };
        let cold = LocalFacts::default();

        let laptop_bid = evaluate(&run, &laptop, &warm, &view, &weights, Millis(0)).expect("bid");
        let desktop_bid = evaluate(&run, &desktop, &cold, &view, &weights, Millis(0)).expect("bid");

        assert!(
            laptop_bid.score > desktop_bid.score,
            "warm {:?} should beat cold {:?}",
            laptop_bid.score,
            desktop_bid.score
        );
    }

    #[test]
    fn a_phone_can_win_a_run_it_is_suited_to() {
        // Phones are workers. A charging phone with a warm workspace is a legitimate host.
        let weights = BidWeights::default();
        let run = a_run();
        let mut phone = node_view(2, DeviceClass::Phone, 8, 8);
        phone.capabilities.power = PowerSource::Battery {
            percent: 90,
            charging: true,
        };
        let view = view_with(vec![phone.clone()]);

        let bid = evaluate(
            &run,
            &phone,
            &LocalFacts {
                workspace_warm: true,
                ..LocalFacts::default()
            },
            &view,
            &weights,
            Millis(0),
        )
        .expect("phone should be able to bid");

        assert_eq!(winner(std::slice::from_ref(&bid)), Some(&bid));
    }

    #[test]
    fn a_phone_on_battery_declines_for_a_reportable_reason() {
        let weights = BidWeights::default();
        let run = a_run();
        let mut phone = node_view(2, DeviceClass::Phone, 8, 8);
        phone.capabilities.power = PowerSource::Battery {
            percent: 20,
            charging: false,
        };
        let view = view_with(vec![phone.clone()]);

        match evaluate(
            &run,
            &phone,
            &LocalFacts::default(),
            &view,
            &weights,
            Millis(0),
        ) {
            Err(NoBid::Refused(Refusal::NotCharging)) => {}
            other => panic!("expected a policy refusal, got {other:?}"),
        }
    }

    #[test]
    fn ineligibility_names_the_failing_clause() {
        let weights = BidWeights::default();
        let mut run = a_run();
        run.spec.constraint = Constraint::all([Constraint::MinCores(64)]);
        let laptop = node_view(2, DeviceClass::Laptop, 8, 16);
        let view = view_with(vec![laptop.clone()]);

        match evaluate(
            &run,
            &laptop,
            &LocalFacts::default(),
            &view,
            &weights,
            Millis(0),
        ) {
            Err(NoBid::Ineligible(failures)) => {
                assert_eq!(failures.len(), 1);
                assert!(failures[0].contains("cores >= 64"), "{failures:?}");
            }
            other => panic!("expected ineligible, got {other:?}"),
        }
    }

    #[test]
    fn a_node_too_old_to_read_the_checkpoint_does_not_bid() {
        let weights = BidWeights::default();
        let mut run = a_run();
        run.checkpoint = Some(crate::run::Checkpoint {
            session_id: None,
            transcript: crate::BlobHash::from_bytes([0; 32]),
            bundle: None,
            patch: None,
            base_commit: "0f0f0f0f".into(),
            turns: 2,
            taken_at: Millis(0),
            agent_version: "2.11.0".into(),
            replicas: BTreeSet::new(),
        });

        let mut old = node_view(2, DeviceClass::Desktop, 16, 32);
        old.capabilities.add(claude(AgentKind::ClaudeCode, "2.9.0"));
        let view = view_with(vec![old.clone()]);

        assert!(matches!(
            evaluate(
                &run,
                &old,
                &LocalFacts::default(),
                &view,
                &weights,
                Millis(0)
            ),
            Err(NoBid::AgentTooOld { .. })
        ));
    }

    #[test]
    fn ties_break_deterministically_by_node_id() {
        // Every node must reach the same winner without another round trip.
        let bids = vec![
            Bid {
                run: RunId::from_bytes([1; 16]),
                node: node_id(5),
                epoch: Epoch(1),
                score: Score(50),
                available: Availability::Now,
                submitted_at: Millis(0),
                terms: None,
            },
            Bid {
                run: RunId::from_bytes([1; 16]),
                node: node_id(2),
                epoch: Epoch(1),
                score: Score(50),
                available: Availability::Now,
                submitted_at: Millis(0),
                terms: None,
            },
        ];
        assert_eq!(winner(&bids).map(|b| b.node), Some(node_id(2)));

        let reversed: Vec<Bid> = bids.iter().rev().cloned().collect();
        assert_eq!(winner(&reversed).map(|b| b.node), Some(node_id(2)));
    }

    #[test]
    fn highest_score_wins_regardless_of_id() {
        let bids = vec![
            Bid {
                run: RunId::from_bytes([1; 16]),
                node: node_id(2),
                epoch: Epoch(1),
                score: Score(10),
                available: Availability::Now,
                submitted_at: Millis(0),
                terms: None,
            },
            Bid {
                run: RunId::from_bytes([1; 16]),
                node: node_id(9),
                epoch: Epoch(1),
                score: Score(90),
                available: Availability::Now,
                submitted_at: Millis(0),
                terms: None,
            },
        ];
        assert_eq!(winner(&bids).map(|b| b.node), Some(node_id(9)));
    }

    #[test]
    fn no_bids_no_winner() {
        assert_eq!(winner(&[]), None);
    }

    /// A view where `busy` holds `count` runs, so the policy's capacity check has something
    /// to count. Runs are held, not merely known: `running_count` reads the run records.
    fn view_with_load(nodes: Vec<NodeView>, busy: NodeId, count: u8) -> ClusterView {
        let mut cv = view_with(nodes);
        for i in 0..count {
            let mut run = a_run();
            run.id = RunId::from_bytes([100 + i; 16]);
            let epoch = run.assign(busy, Millis(0), Millis(60_000)).expect("assign");
            run.started(busy, epoch, Millis(0)).expect("start");
            cv.runs.insert(run.id, run);
        }
        cv
    }

    #[test]
    fn a_full_node_offers_anyway_and_says_when() {
        // ADR-0006's overnight case. Declining because the node is full leaves nobody
        // committed and the run pending — "filed away, good luck" — which is exactly what
        // ADR-0014 says must never be an outcome. A queue empties by itself, so being full is
        // the one refusal that is really a "later".
        let weights = BidWeights::default();
        let run = a_run();
        let desktop = node_view(3, DeviceClass::Desktop, 32, 64);
        // The desktop policy takes four at a time; give it four.
        let view = view_with_load(vec![desktop.clone()], desktop.id, 4);

        let bid = evaluate(
            &run,
            &desktop,
            &LocalFacts::default(),
            &view,
            &weights,
            Millis(0),
        )
        .expect("a full node still offers");
        assert_eq!(bid.available, Availability::WhenFree { behind: 4 });
        assert_eq!(
            bid.available.to_string(),
            "starting when one of the 4 runs ahead of it finishes"
        );
    }

    #[test]
    fn being_on_battery_is_not_a_queue() {
        // The line between the two: a full node's queue empties on its own, so committing to
        // it is honest. A phone on battery has not promised anybody it will be plugged in
        // later, and a node that offered on its owner's behalf would be deciding for them.
        let weights = BidWeights::default();
        let run = a_run();
        let mut phone = node_view(2, DeviceClass::Phone, 8, 8);
        phone.capabilities.power = PowerSource::Battery {
            percent: 90,
            charging: false,
        };
        let view = view_with(vec![phone.clone()]);

        assert!(matches!(
            evaluate(
                &run,
                &phone,
                &LocalFacts::default(),
                &view,
                &weights,
                Millis(0)
            ),
            Err(NoBid::Refused(Refusal::NotCharging))
        ));
    }

    #[test]
    fn a_node_that_can_start_now_beats_a_keener_one_that_cannot() {
        // Availability outranks score outright rather than being worth some number of points:
        // starting today beats starting after whatever that machine is already doing, and no
        // score gap between two boxes on one LAN is worth more than that.
        let bids = vec![
            Bid {
                run: RunId::from_bytes([1; 16]),
                node: node_id(2),
                epoch: Epoch(1),
                score: Score(500),
                available: Availability::WhenFree { behind: 2 },
                submitted_at: Millis(0),
                terms: None,
            },
            Bid {
                run: RunId::from_bytes([1; 16]),
                node: node_id(9),
                epoch: Epoch(1),
                score: Score(1),
                available: Availability::Now,
                submitted_at: Millis(0),
                terms: None,
            },
        ];
        assert_eq!(winner(&bids).map(|b| b.node), Some(node_id(9)));

        // Among nodes that are equally able to begin, score decides as it always did.
        let both_busy: Vec<Bid> = bids
            .iter()
            .cloned()
            .map(|mut b| {
                b.available = Availability::WhenFree { behind: 1 };
                b
            })
            .collect();
        assert_eq!(winner(&both_busy).map(|b| b.node), Some(node_id(2)));
    }

    #[test]
    fn an_archive_larger_than_the_owner_allows_is_refused_by_that_node_alone() {
        // ADR-0061 §4's knob. The refusal has to be distinguishable from the fleet-wide cap,
        // because the two send somebody to opposite places: this one is one owner's setting, so
        // another node may take the run and the sentence says so; the structural one is every
        // node's, and is refused before a run exists at all.
        let mut run = a_run();
        let work = run.spec.agent_mut().expect("an agent run");
        work.workspace.repo = format!("archive:{}", "ab".repeat(32));
        work.workspace.archive_bytes = Some(40 * 1024 * 1024);

        let mut me = node_view(2, DeviceClass::Desktop, 16, 32);
        me.policy.max_archive_bytes = Some(10 * 1024 * 1024);
        let view = view_with(vec![me.clone()]);
        let facts = LocalFacts {
            repo_reach: RepoReach::Obtainable,
            ..LocalFacts::default()
        };

        let refusal = evaluate(&run, &me, &facts, &view, &BidWeights::default(), Millis(0))
            .expect_err("over this owner's ceiling");
        assert!(
            matches!(refusal, NoBid::ArchiveTooLarge { .. }),
            "{refusal:?}"
        );
        assert!(
            refusal
                .to_string()
                .contains("another node may still take it"),
            "it is one owner's answer, not the fleet's: {refusal}"
        );

        // Raise the ceiling and the same node bids.
        me.policy.max_archive_bytes = Some(50 * 1024 * 1024);
        let view = view_with(vec![me.clone()]);
        assert!(evaluate(&run, &me, &facts, &view, &BidWeights::default(), Millis(0)).is_ok());

        // And an owner with no opinion refuses nothing: `None` is the fleet's own limit, which
        // was already enforced before this run could exist.
        me.policy.max_archive_bytes = None;
        let view = view_with(vec![me.clone()]);
        assert!(evaluate(&run, &me, &facts, &view, &BidWeights::default(), Millis(0)).is_ok());
    }

    #[test]
    fn an_archive_whose_size_nobody_stated_is_not_refused_on_size() {
        // `archive_bytes` is `None` for anything that did not know to say. Refusing would turn
        // a missing field into a policy decision; the node finds out at acquisition instead,
        // which is slower and still correct.
        let mut run = a_run();
        let work = run.spec.agent_mut().expect("an agent run");
        work.workspace.repo = format!("archive:{}", "ab".repeat(32));
        work.workspace.archive_bytes = None;

        let mut me = node_view(2, DeviceClass::Desktop, 16, 32);
        me.policy.max_archive_bytes = Some(1);
        let view = view_with(vec![me.clone()]);
        let facts = LocalFacts {
            repo_reach: RepoReach::Obtainable,
            ..LocalFacts::default()
        };
        assert!(evaluate(&run, &me, &facts, &view, &BidWeights::default(), Millis(0)).is_ok());
    }

    #[test]
    fn a_repo_named_by_local_path_is_refused_where_that_path_is_not() {
        // The failure this exists to prevent lands at the worst possible hour: submit from a
        // laptop, close the laptop, and a run whose repo lived only there is placed on a
        // desktop that cannot clone it — at 03:00, with nobody watching (roadmap #4).
        let mut run = a_run();
        run.spec.agent_mut().expect("an agent run").workspace.repo = "/home/owner/dev/api".into();
        let me = node_view(2, DeviceClass::Desktop, 16, 32);
        let view = view_with(vec![me.clone()]);

        let cold = LocalFacts {
            repo_reach: RepoReach::NotHere,
            ..LocalFacts::default()
        };
        let refusal = evaluate(&run, &me, &cold, &view, &BidWeights::default(), Millis(0))
            .expect_err("cannot get the repo");
        assert!(matches!(
            refusal,
            NoBid::RepoUnavailable {
                reach: RepoReach::NotHere,
                ..
            }
        ));
        // It names the actual problem: "ineligible" would send somebody to look at
        // capabilities, which are fine.
        assert!(refusal.to_string().contains("another machine"));

        // …and the *other* way a local path is unobtainable gets the other sentence. One
        // `Portability::NodeLocal` covers both, which is why the refusal used to tell somebody
        // standing in front of the directory that it was on another machine.
        let here_but_not_a_repo = LocalFacts {
            repo_reach: RepoReach::NotARepo,
            ..LocalFacts::default()
        };
        let refusal = evaluate(
            &run,
            &me,
            &here_but_not_a_repo,
            &view,
            &BidWeights::default(),
            Millis(0),
        )
        .expect_err("a directory is not a repository");
        assert_eq!(
            refusal.to_string(),
            "/home/owner/dev/api is here, but it is not a git repository"
        );

        // The machine that has it bids normally.
        assert!(evaluate(
            &run,
            &me,
            &LocalFacts::default(),
            &view,
            &BidWeights::default(),
            Millis(0)
        )
        .is_ok());
    }

    #[test]
    fn a_checkpoint_is_replicated_to_the_node_most_likely_to_still_be_there() {
        // Choosing the plausible successor is what makes the common migration need no fetch:
        // the copy is already where the run would go.
        let run = a_run();
        let holder = node_view(1, DeviceClass::Laptop, 8, 16);
        let desktop = node_view(2, DeviceClass::Desktop, 16, 32);
        let phone = node_view(3, DeviceClass::Phone, 8, 8);
        let view = view_with(vec![holder.clone(), phone, desktop.clone()]);

        assert_eq!(replica_for(&run, &view, holder.id), Some(desktop.id));
    }

    #[test]
    fn a_node_that_could_not_run_it_is_still_better_than_no_copy_at_all() {
        // Second tier, and a small extension of ADR-0016: the run cannot move to a node that
        // fails its constraints, but a copy there still survives the holder — and losing the
        // only copy because the fleet is a laptop and a phone is not an acceptable answer.
        let mut run = a_run();
        run.spec.constraint = Constraint::MinCores(64);

        let holder = node_view(1, DeviceClass::Laptop, 8, 16);
        let phone = node_view(3, DeviceClass::Phone, 8, 8);
        let view = view_with(vec![holder.clone(), phone.clone()]);

        assert_eq!(replica_for(&run, &view, holder.id), Some(phone.id));
    }

    #[test]
    fn a_node_that_has_gone_quiet_is_not_a_replica() {
        let run = a_run();
        let holder = node_view(1, DeviceClass::Laptop, 8, 16);
        let mut desktop = node_view(2, DeviceClass::Desktop, 16, 32);
        desktop.set_status(crate::view::NodeStatus::Suspect, Millis(0));
        let view = view_with(vec![holder.clone(), desktop]);

        assert_eq!(replica_for(&run, &view, holder.id), None, "a fleet of one");
    }

    #[test]
    fn a_checkpoint_is_not_durable_on_the_machine_that_might_be_in_a_bag() {
        let holder = node_id(1);
        let mut checkpoint = checkpoint_for(holder);
        assert!(!checkpoint.is_durable(Some(holder)), "no copies anywhere");

        checkpoint.replicas.insert(holder);
        assert!(
            !checkpoint.is_durable(Some(holder)),
            "a copy on the holder is not a copy"
        );

        checkpoint.replicas.insert(node_id(2));
        assert!(checkpoint.is_durable(Some(holder)));
    }

    #[test]
    fn a_checkpoint_names_every_blob_a_receiver_needs() {
        // Miss one and the migration fails at materialise time, on the far node, after the
        // handoff — which is the worst place to find out.
        let mut checkpoint = checkpoint_for(node_id(1));
        assert_eq!(checkpoint.blobs(), vec![checkpoint.transcript]);

        checkpoint.bundle = Some(BlobHash::from_bytes([2; 32]));
        checkpoint.patch = Some(BlobHash::from_bytes([3; 32]));
        assert_eq!(
            checkpoint.blobs(),
            vec![
                checkpoint.transcript,
                BlobHash::from_bytes([2; 32]),
                BlobHash::from_bytes([3; 32])
            ]
        );
    }

    /// A node on a named account, claiming `limit` concurrent runs for it.
    fn sharing_account(b: u8, account: &str, limit: Option<u32>) -> NodeView {
        let mut view = node_view(b, DeviceClass::Desktop, 8, 32);
        view.capabilities.add(
            claude(AgentKind::ClaudeCode, "2.10.0")
                .with_identity(Some(crate::capability::AccountId(account.into()))),
        );
        view.policy.max_concurrent_account = limit;
        view
    }

    #[test]
    fn a_node_whose_account_is_spent_commits_rather_than_refusing() {
        // This desktop is idle: no runs, eight cores, nothing wrong with it. What is spent is
        // the login it shares with the machine actually doing the work — and an account's queue
        // empties by itself, so this is the *full* case rather than the pressured one. Refusing
        // instead would leave nobody holding the run, which is what ADR-0014 exists to stop.
        let me = sharing_account(1, "acct:one", Some(1));
        let peer = sharing_account(2, "acct:one", Some(1));
        let view = view_with_load(vec![me.clone(), peer], node_id(2), 1);

        let bid = evaluate(
            &a_run(),
            &me,
            &LocalFacts::default(),
            &view,
            &BidWeights::default(),
            Millis(1),
        )
        .expect("a full account still bids");
        assert_eq!(bid.available, Availability::WhenFree { behind: 1 });
    }

    #[test]
    fn a_rate_limited_account_commits_and_says_when_it_can_start() {
        // ADR-0029. The machine is idle and nothing about it is wrong; what is spent is the
        // account's quota, and the agent said when it lifts. So this is the *full* case with a
        // clock instead of a count — it commits, because refusing would leave nobody holding the
        // run (ADR-0014), and it says the instant rather than a queue position.
        let me = sharing_account(1, "acct:one", None);
        let view = view_with_load(vec![me.clone()], node_id(1), 0);
        let limited = LocalFacts {
            account_limited_until: Some(Millis(9_000)),
            ..LocalFacts::default()
        };

        let bid = evaluate(
            &a_run(),
            &me,
            &limited,
            &view,
            &BidWeights::default(),
            Millis(1_000),
        )
        .expect("a rate-limited account still bids");
        assert_eq!(bid.available, Availability::NotBefore { at: Millis(9_000) });
        assert!(
            !bid.available.is_now(),
            "so `winner()` prefers any node that can start now — the whole point of saying it"
        );

        // A limit that has already lifted is not a limit. `None` and `Some(past)` have to mean
        // the same thing, or a node holds work for ever on the last thing it was told.
        let lifted = LocalFacts {
            account_limited_until: Some(Millis(500)),
            ..LocalFacts::default()
        };
        let bid = evaluate(
            &a_run(),
            &me,
            &lifted,
            &view,
            &BidWeights::default(),
            Millis(1_000),
        )
        .expect("bid");
        assert_eq!(bid.available, Availability::Now);
    }

    #[test]
    fn a_rate_limit_outranks_a_queue_when_both_hold() {
        // Both are true and only one is worth saying: a slot here frees when this node's own work
        // finishes, and the limit does not care what happens here at all — so the limit is always
        // the later of the two, and reporting the count would send somebody to watch a slot that
        // frees long before the run can use it.
        let me = sharing_account(1, "acct:one", Some(1));
        let peer = sharing_account(2, "acct:one", Some(1));
        let view = view_with_load(vec![me.clone(), peer], node_id(2), 1);
        let limited = LocalFacts {
            account_limited_until: Some(Millis(9_000)),
            ..LocalFacts::default()
        };

        let bid = evaluate(
            &a_run(),
            &me,
            &limited,
            &view,
            &BidWeights::default(),
            Millis(1_000),
        )
        .expect("bid");
        assert_eq!(bid.available, Availability::NotBefore { at: Millis(9_000) });
    }

    #[test]
    fn an_account_ceiling_of_zero_is_a_refusal_and_not_a_queue() {
        // "Never" rather than "not yet": nothing is going to finish and make room, so a
        // commitment here would be a promise no node can keep.
        let me = sharing_account(1, "acct:one", Some(0));
        let view = view_with(vec![me.clone()]);

        assert!(matches!(
            evaluate(
                &a_run(),
                &me,
                &LocalFacts::default(),
                &view,
                &BidWeights::default(),
                Millis(1),
            ),
            Err(NoBid::Refused(Refusal::AccountAtCapacity { .. }))
        ));
    }

    #[test]
    fn a_node_on_its_own_account_is_unaffected_by_somebody_elses_ceiling() {
        // The counting has to be per account, not per fleet: a second login is a second rate
        // limit, and this is the case that makes a two-account fleet worth having.
        let me = sharing_account(1, "acct:mine", Some(1));
        let busy = sharing_account(2, "acct:theirs", Some(1));
        let view = view_with_load(vec![me.clone(), busy], node_id(2), 1);

        let bid = evaluate(
            &a_run(),
            &me,
            &LocalFacts::default(),
            &view,
            &BidWeights::default(),
            Millis(1),
        )
        .expect("a different account is a different limit");
        assert_eq!(bid.available, Availability::Now);
    }

    #[test]
    fn a_device_another_fleet_is_using_bids_as_the_busy_machine_it_is() {
        // Two `offloadd` instances, one laptop, one fleet each (ADR-0012) — so this node's view
        // shows nothing running and the machine is nonetheless full. Without the ledger in the
        // bid, this node offers to start now and then holds the run the instant it arrives:
        // an accurate outcome reached through an inaccurate promise.
        let me = node_view(1, DeviceClass::Laptop, 8, 32);
        let view = view_with(vec![me.clone()]);
        let facts = LocalFacts {
            device_committed: Occupancy::new(2, 8),
            ..LocalFacts::default()
        };

        let bid = evaluate(
            &a_run(),
            &me,
            &facts,
            &view,
            &BidWeights::default(),
            Millis(1),
        )
        .expect("a device busy with another fleet still commits");
        assert_eq!(bid.available, Availability::WhenFree { behind: 2 });
    }
}
