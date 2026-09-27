//! `offload explain <run>`: the reasons, assembled into sentences.
//!
//! Nothing here decides anything. Every answer already exists as a structured value —
//! `Supervision`, `HoldReason`, `NoBid`, `Availability` — because a decision that discarded
//! its reasoning cannot answer the question this command exists for. What was missing was a
//! place that asks all of them about one run and says the answers out loud.
//!
//! Two things it deliberately does *not* do. It does not replay the round that placed the
//! run: bids describe a second and are thrown away with good reason, so what is shown is what
//! the fleet would say now, labelled as such. And it does not grant anything — `WillYouTake`
//! is side-effect free, which is what makes it safe to ask about a run that is running
//! perfectly well somewhere else.

use crate::api::{Explanation, NodeOpinion};
use offload_core::{
    Bystanding, ClusterView, Escalation, Millis, NodeId, Offering, ReassignDecision,
    ReassignPolicy, Run, RunState, Supervision,
};

/// What the caller went and found out, because it is I/O and this module is not.
///
/// A struct rather than four more parameters, and not only for the argument count: two of them are
/// `Option<String>` and mean opposite things — one is why the fleet was *not asked*, the other is
/// why a run this node holds has *not started* — so positionally they are a silent swap.
#[derive(Debug, Default)]
pub struct Observed {
    /// How many clients are streaming the run here, or `None` where this node cannot tell.
    pub watchers: Option<u32>,
    /// Every node's answer to the canvass. Empty when nobody was asked.
    pub opinions: Vec<NodeOpinion>,
    /// Why the fleet was not asked: a finished run, or no mesh.
    pub not_canvassed: Option<String>,
    /// Why a run this node holds has not started, asked of the gate that would start it.
    pub held_back: Option<String>,
    /// What this node remembers about retrying it, for a run that **failed** here.
    ///
    /// Both halves matter and neither could be re-derived: the attendance the decision was taken
    /// on (sampling it now finds nobody watching every time — ADR-0013), and the decision itself.
    pub recovery: Option<crate::supervisor::RecoveryState>,
    /// How many turns the run has taken, from this node's own copy of its numbers.
    ///
    /// Passed in for `watchers`' reason: it lives beside the record rather than on it, so the
    /// caller holding the store is the one that can state it. What reads it is the same
    /// `decide_recovery` the tick calls — this line and that decision have to come out of one
    /// function, or `offload explain` is a second copy of the rule confidently disagreeing with
    /// what the node will actually do.
    pub turns: u32,
    /// What is true of this *node*, which the recovery decision reads and an explanation
    /// therefore has to (ADR-0034, ADR-0044).
    ///
    /// Here for `turns`' reason, one field on: a drained node will not start an agent, and a
    /// revoked one may not be one, so an explanation that does not know tells somebody their run
    /// is about to be picked up again by a machine that has stopped doing that.
    pub node: NodeStanding,
    /// What this run is stopped waiting for a person to answer.
    ///
    /// Passed in because finding out is I/O and this module does none: on the holder it is a
    /// registry read, and anywhere else it is a canvass of the fleet — the one `offload asks`
    /// already runs, since a pending question is a fact the holder can be *asked* for rather
    /// than arithmetic only it could do. That distinction is the whole difference between this
    /// field and [`Self::held_back`], which stays local; they read alike and are not alike.
    pub waiting: Vec<offload_core::PendingAsk>,
    /// The sentence `offload resume` would be refused with here, from the door itself.
    ///
    /// Not derived from [`Self::node`], though every input is in there: the door composes the
    /// three questions in an order it has reasons for, and re-composing them here would be the
    /// second copy of the rule this module's own header disowns. It arrives already said.
    pub resume_refusal: Option<String>,
}

/// The facts about the machine rather than the run that decide whether it will retry.
///
/// One value rather than three adjacent parameters, for the reason
/// [`offload_core::Circumstances`] is one value: `recovery_note(&state, run, turns, true, false,
/// now)` is a call site nobody can read, and the booleans are lined up by eye at exactly the
/// place where getting them the wrong way round says the opposite of the truth.
///
/// This report and the tick's decision have to come out of one function, so every field the tick
/// reads is a field here (ADR-0049 added the third).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeStanding {
    pub departing: bool,
    pub revoked: bool,
    /// The owner's standing answer, asked of the *live* capabilities — the same question
    /// `offload status`'s `accepting` line answers, so the two cannot disagree (ADR-0048).
    pub hosting: offload_core::Hosting,
}

impl Default for NodeStanding {
    fn default() -> Self {
        NodeStanding {
            departing: false,
            revoked: false,
            hosting: offload_core::Hosting::Allowed,
        }
    }
}

/// Build the explanation for `run` from what this node can see, without asking anybody.
#[must_use]
pub fn explain(
    view: &ClusterView,
    run: &Run,
    now: Millis,
    policy: &ReassignPolicy,
    observed: Observed,
) -> Explanation {
    let Observed {
        watchers,
        opinions,
        not_canvassed,
        held_back,
        waiting,
        recovery,
        turns,
        node,
        resume_refusal,
    } = observed;
    let arbiter = view.arbiter_for(run);
    let parked = offload_core::parked(run);
    Explanation {
        run: run.id.to_string(),
        state: run.state.name().to_string(),
        state_detail: state_detail(run, now, parked),
        parked,
        kind: run.spec.work.kind(),
        work: run.spec.work.summary(),
        epoch: run.epoch.0,
        due: due(run, now),
        demand: run.spec.demand.to_string(),
        // Straight off the spec, like `demand`: it needs no clock and no view, and the copy
        // every node holds is settled by `spec_rev` against the home node (ADR-0005).
        priority: run.spec.priority,
        attendance: attendance(view, run, watchers, recovery, !waiting.is_empty()),
        holder: run.holder().map(|n| name_of(view, n)),
        arbiter: arbiter.map(|n| name_of(view, n)),
        arbiter_is_local: arbiter == Some(view.local),
        verdict: verdict(view, run, now, policy),
        checkpoint: checkpoint(view, run),
        opinions,
        not_canvassed,
        held_back,
        waiting,
        recovery: recovery.map(|state| recovery_note(&state, run, turns, node, view.alone(), now)),
        // Only where this run makes `offload resume` the next thing somebody would type. The
        // refusal is about the node and is true whatever the run is doing, but set against a run
        // that is running perfectly well it answers a question nobody asked. The gate is a
        // predicate beside the door's own rather than a state list written out again here:
        // `Pending` (parked by `offload checkpoint`) and `Failed` (interrupted) are the two
        // states, and a **task** is neither tier — it is restarted, never resumed (ADR-0058), so
        // a node-level refusal here would stand in front of a permanent run-level one and name a
        // fleet grant as the way past it. `state_detail` above is where the stored sentence
        // naming the command appears.
        resume_refusal: crate::supervisor::resumable_by_hand(run)
            .then_some(resume_refusal)
            .flatten(),
    }
}

/// What this node will do about a failed run, and what it already did.
///
/// The line `offload explain` had no answer for. A failed run sits there looking identical whether
/// it is thirty seconds from being picked up, out of retries, or deliberately left for the person
/// who was watching when it broke — and ADR-0013's third axis is what decides between them.
///
/// `resumes` is said whenever it is non-zero, decision or not, because "it has been picked up
/// twice already" is most of the answer to "why does it keep breaking".
fn recovery_note(
    state: &crate::supervisor::RecoveryState,
    run: &Run,
    turns: u32,
    node: NodeStanding,
    // Whether this fleet has a second device (`ClusterView::alone`). A parameter rather than a
    // field on `NodeStanding`, and the line is worth drawing: that struct holds the facts this
    // function *cannot see* — a drain, a revocation, the owner's policy — while this one the
    // view holds, and the caller has the view.
    alone: bool,
    now: Millis,
) -> String {
    let tried = match state.resumes {
        0 => String::new(),
        1 => " (picked up once already)".to_string(),
        n => format!(" (picked up {n} times already)"),
    };
    match state.decided {
        // **Only while the run is still the failure the decision was about.** The memory is
        // deliberately kept — ADR-0013 wants the escalation answerable for rather than deleted —
        // but a remembered decision is not the same thing as the current answer, and this arm
        // printed it as one. A run that has since been picked back up somewhere else is not
        // failed any more, so the sentence below it is false while every other line in the same
        // report says so: measured on two daemons, `offload explain` on the node that hosted the
        // first leg said *its holder is answering and its lease is current*, *checkpoint turn 16,
        // copied to fedora*, and *nobody could continue it* — in one output, and it never
        // self-corrected, because nothing re-examines a decision once `decided` is set.
        //
        // The undecided path below has always asked: `decide_recovery` returns `None` for a run
        // that is not `Failed`, which is the *nothing to decide — it is not failed* arm. So this
        // is the same question, asked on the branch that was skipping it.
        Some(reason) if matches!(run.state, RunState::Failed { .. }) => {
            format!("left for a person: {reason}{tried}")
        }
        // No decision yet — or one about a failure that is over — so the live answer is whatever
        // the policy says now, and the number somebody wants is *when*, which only a clock can
        // give.
        _ => match offload_core::decide_recovery(
            run,
            now,
            &offload_core::RecoveryPolicy::default(),
            offload_core::Circumstances {
                attendance: state.observed,
                standing: offload_core::Standing::Ours,
                departing: node.departing,
                revoked: node.revoked,
                hosting: node.hosting,
                // Originating in the view, unlike the three above it, and on a node with no mesh
                // that view is `local_view` — itself, alone — so this is the same function's
                // answer either way.
                alone,
                turns,
                resumes: state.resumes,
            },
        ) {
            Some(offload_core::Recovery::Wait { until, resume }) => format!(
                "picking it up again in {} (try {resume}){tried}",
                until.saturating_sub(now)
            ),
            Some(offload_core::Recovery::Resume { resume }) => {
                format!("picking it up again now (try {resume}){tried}")
            }
            // Reachable only in the window between a decision being due and the tick taking it.
            Some(offload_core::Recovery::Escalate(reason)) => {
                format!("about to be left for a person: {reason}{tried}")
            }
            // Same window, other outcome (ADR-0043). Said as a hand-over rather than as a
            // resume, because it is one: the next thing that happens to this run is a bid round
            // somewhere else, and the node that runs it will not be this one.
            // …and it has two causes now, which the sentence has to keep straight: a node that
            // is leaving and a node whose owner will not have it host anything are different
            // things to be told, and only one of them is fixed by waiting (ADR-0049).
            Some(offload_core::Recovery::LetGo) => {
                let why = if node.departing {
                    "this node is leaving"
                } else {
                    "this node's owner does not allow it to host runs"
                };
                format!("{why}; about to hand it back to the fleet to carry on with{tried}")
            }
            None => format!("nothing to decide — it is not failed{tried}"),
        },
    }
}

/// Whether anybody is watching, and whether this node is in a position to know.
///
/// Observed rather than declared (ADR-0013), which means it is observable in exactly one place:
/// the node serving the stream, which is the node running the agent. Explaining a run from
/// anywhere else has to say so rather than report "unattended" — the difference between *nobody
/// is watching* and *I would not see it if they were* is the whole reason attendance decides
/// anything.
///
/// **For a run that has failed here, the remembered observation wins over a fresh sample**, and
/// that is not a nicety. A failure ends the stream, so sampling afterwards finds nobody watching
/// *every single time* — ADR-0013's module docs say so in as many words, as the reason attendance
/// is taken at the moment of failure. This line printed the fresh sample: measured on one daemon,
/// `attendance unattended — nobody is streaming it` about a run whose own log said it was being
/// left alone **because somebody was watching**. Two answers to one question, one second apart,
/// and the operator reads the wrong one.
fn attendance(
    view: &ClusterView,
    run: &Run,
    watchers: Option<u32>,
    recovery: Option<crate::supervisor::RecoveryState>,
    // Whether the run is stopped on a question. A `bool` rather than the asks themselves: this
    // line never prints one, it only has to stop claiming the opposite of one.
    blocked: bool,
) -> String {
    if run.state.is_terminal() {
        // Never failed here at all — it finished, or it is somebody else's run.
        let Some(state) = recovery else {
            return "it is over; nobody is streaming it".to_string();
        };
        // Failed here, and this node has forgotten who was watching: a restart since. The
        // same answer `Escalation::AttendanceUnknown` gives, for the same reason.
        let Some(observed) = state.observed else {
            return "this node cannot tell who was watching when it failed".to_string();
        };
        // *Whether* it is what decided, rather than asserting it. Attendance was the only thing
        // that could decide for as long as this line existed, so it said so unconditionally —
        // and `Escalation::TurnLimitReached` is the first reason that outranks it, which made
        // this line state a cause the decision beside it contradicts. Measured on a capped run:
        // `attended when it failed — which is what decided`, one line above `left for a person:
        // it reached its turn limit of 1`.
        //
        // Listed by the escalations that *are* about attendance rather than by the ones that
        // are not, so a reason added later has to be added here deliberately to make this claim,
        // instead of inheriting it by being unlisted. `None` is a decision not yet taken, where
        // attendance is still what it turns on.
        let attendance_decided = matches!(
            state.decided,
            None | Some(Escalation::SomebodyWasWatching) | Some(Escalation::AttendanceUnknown)
        );
        return if attendance_decided {
            format!("{observed} when it failed — which is what decided")
        } else {
            // The reason is on the `recovery` line directly below, so this one stops at the
            // observation rather than repeating it or guessing at it.
            format!("{observed} when it failed")
        };
    }
    attendance_now(view, run, watchers, blocked)
}

/// The live sample, which is the right answer for a run that is still going.
fn attendance_now(view: &ClusterView, run: &Run, watchers: Option<u32>, blocked: bool) -> String {
    match (watchers, run.holder()) {
        (Some(0), _) => "unattended — nobody is streaming it".to_string(),
        (Some(1), _) => "attended — one client is streaming it".to_string(),
        (Some(n), _) => format!("attended — {n} clients are streaming it"),
        // Held elsewhere, and *what is happening there* decides the sentence. This arm said
        // "it is running there" for every state with a holder, and two of them are not running:
        // an `Assigned` run is one this node's own state line calls *accepted but not started*,
        // one line above — two lines of one screen contradicting each other — and an `Orphaned`
        // one is held by a node that is out of contact, where claiming it is running is the
        // confident wrong answer `Run::holder`'s own doc warns about (it answers `Some(last)`
        // there, which is *who was holding it* rather than who is).
        (None, Some(holder)) if holder != view.local => match run.state {
            RunState::Assigned { .. } => format!(
                "nothing is streaming it: {} has taken it and not started it",
                name_of(view, holder)
            ),
            RunState::Orphaned { .. } => format!(
                "only {} could tell, and it is out of contact",
                name_of(view, holder)
            ),
            // …and the third state in the same family, reachable only since the `waiting` line
            // started being canvassed from off the holder. A run stopped mid-tool-call for a
            // person (ADR-0017) is `Running` and making no progress by design, so "it is running
            // there" was the justification clause sitting one line above *stopped for an answer*
            // on one screen. The state line above still says `running`, which is the honest
            // word for the process; this line is about whether anybody is watching, and it has
            // no business restating a claim the line below it qualifies.
            _ if blocked => format!(
                "only {} can tell; it is stopped there, waiting for an answer",
                name_of(view, holder)
            ),
            _ => format!(
                "only {} can tell; it is running there",
                name_of(view, holder)
            ),
        },
        (None, _) => "not running here, so nobody is streaming it from here".to_string(),
    }
}

/// A node's name, falling back to its id: discovery gives us a key before it gives us a name,
/// and an empty column would read as a bug rather than as a node nobody has met.
fn name_of(view: &ClusterView, node: NodeId) -> String {
    view.node(&node)
        .map_or_else(|| node.short(), offload_core::NodeView::display_name)
}

/// What the state carries, which is usually the number somebody is actually asking about.
fn state_detail(run: &Run, now: Millis, parked: bool) -> String {
    match &run.state {
        // Parked by `offload checkpoint`: it is waiting for a person, and "waiting for a node"
        // was the sentence two lines above `arbiter`'s *"waits for `offload resume` rather than
        // for a bid round"* — one screen, two answers to what it is waiting for (session ninety).
        RunState::Pending { since, .. } if parked => format!(
            "parked {} ago, waiting for `offload resume` rather than for a node",
            now.saturating_sub(*since)
        ),
        // Two different waits, and until ADR-0042 the state could not tell them apart. A run a
        // person parked waits for that person; a run nobody is coming back for waits for a bid
        // round that is actually coming. The first sentence was printed for both.
        //
        // **What the record can support, which is not intent.** `let_go_by` is set by
        // `Run::release`, and that has a second caller nobody printing this had in mind: a bid
        // round giving its token back when a grant is declined or never confirmed
        // (`Cluster::hand_over`). So the node named here may never have held the run at all —
        // and in the unconfirmed case it may be the one node that *is* running it, since
        // ADR-0006 treats silence as a decline in order to *act*, not because anybody observed
        // one. "let it go" states an intention this node did not witness; "last assigned to" is
        // the fact, true of a drain and of a give-back alike, and it still separates the two
        // waits — which is all ADR-0042 asked of it.
        RunState::Pending {
            since,
            let_go_by: Some(by),
        } => format!(
            "waiting for a node since {} ago; last assigned to {}",
            now.saturating_sub(*since),
            by.short()
        ),
        RunState::Pending {
            since,
            let_go_by: None,
        } => {
            format!(
                "waiting for a node since {} ago",
                now.saturating_sub(*since)
            )
        }
        RunState::Assigned { lease } => format!(
            "accepted but not started; lease has {} left",
            lease.expires_at.saturating_sub(now)
        ),
        RunState::Running { started_at, lease } => format!(
            "started {} ago; lease has {} left",
            now.saturating_sub(*started_at),
            lease.expires_at.saturating_sub(now)
        ),
        RunState::Checkpointing { requested_at, .. } => format!(
            "finishing its turn before checkpointing, asked {} ago",
            now.saturating_sub(*requested_at)
        ),
        // An orphan grants no authority and is not a decision (ADR-0007), so the useful
        // number is how long the silence has lasted, not what the lease used to say.
        RunState::Orphaned { since, .. } => format!(
            "its holder has been out of contact {}",
            now.saturating_sub(*since)
        ),
        RunState::Completed { at } => format!("finished {} ago", now.saturating_sub(*at)),
        RunState::Failed { at, reason } => {
            format!("failed {} ago: {reason}", now.saturating_sub(*at))
        }
        RunState::Cancelled { at } => format!("cancelled {} ago", now.saturating_sub(*at)),
    }
}

/// When it is due, and how much of that is left.
///
/// The number `explain` exists to show here is **slack**, not the deadline: behaviour driven
/// by it changes with no event to point at — a run that was content to wait becomes one that
/// moves at the first sign of trouble, and the only thing that happened is that time passed.
/// A log has nothing to show for that, so this does.
///
/// A run with no stated deadline gets the same answer in its own terms: due as soon as
/// somebody can, and its urgency is how long it has been waiting.
///
/// **A finished run is asked a different question.** Slack is a countdown, and reading it against
/// `now` had `explain` print `due overdue by 31.2s` two lines under `state completed — finished
/// 1m25s ago`, about a run that had completed thirty seconds *inside* the deadline it was given —
/// with the number growing for as long as the record was kept. The no-deadline half was the same
/// mistake more quietly: `waiting 1m58s`, still counting, about work that was over. `RunSummary`
/// has had the filter this lacked from the beginning, so `ps` and `explain` — one daemon, one
/// field — gave different answers about the same run. What a person wants once a run is over is
/// whether the deadline was met, which is [`Run::prospect_at`] asked at the instant it ended.
fn due(run: &Run, now: Millis) -> String {
    if let Some(ended) = run.state.finished_at() {
        return match run.prospect_at(ended) {
            offload_core::Prospect::NoStatedDeadline => "no deadline was stated for it".to_string(),
            met_or_missed => format!("it ended {met_or_missed}"),
        };
    }
    let age = now.saturating_sub(run.created_at);
    match (run.spec.deadline, run.holder()) {
        (Some(_), _) => format!("{}", run.slack(now)),
        // The age is still the urgency (`due_at` is `created_at`), but a node has taken it, so
        // "as soon as a node can take it … waiting 46.0s" contradicted the `state running` line
        // four lines above it. Measured on three daemons, on the holder and two bystanders.
        (None, Some(_)) => format!("no deadline given — submitted {age} ago"),
        (None, None) => format!("as soon as a node can take it — no deadline given, waiting {age}"),
    }
}

/// What this node's supervision pass makes of the run, in a sentence.
///
/// The same `offload_core::supervise` the loop runs once a second, asked one extra time for a
/// human. That is the point: an explanation computed by different code from the decision it
/// describes is a second implementation that will disagree with the first, quietly, on the day
/// it matters.
fn verdict(view: &ClusterView, run: &Run, now: Millis, policy: &ReassignPolicy) -> String {
    match offload_core::supervise(view, run, now, policy) {
        // **Terminal is three outcomes and the one that reopens was getting the other two's
        // sentence.** `supervise` is the drop-off loop and answers `Terminal` for all three, which
        // is correct — that loop is about holders going quiet and has nothing left to do here.
        // What was wrong is the *claim*: a `Failed` run is the one terminal state ADR-0013 picks
        // back up, so "nothing is supervising it" was printed one line above `recovery picking it
        // up again in 29.0s (try 1)` on the node that failed it — measured on one daemon, twice,
        // once for an agent run and once for a task. Two adjacent lines of one screen
        // contradicting each other.
        //
        // The replacement says only what this loop decided and then names *where* the other
        // answer is taken. A machine rather than a line, deliberately: the `recovery` line is
        // printed only on the node that failed the run, so a sentence pointing at it would be
        // pointing at nothing on every other node — which is the trap the `held back` sentence
        // fell into one report over.
        Supervision::Bystander(Bystanding::Terminal) if matches!(run.state, RunState::Failed { .. }) => {
            "this loop is done with it; whether a failed run comes back is decided on the node it failed on".into()
        }
        Supervision::Bystander(Bystanding::Terminal) => {
            "it is over; nothing is supervising it".into()
        }
        // **The arm that offers nothing, and it used to describe what the arm below does.** It
        // said "waiting for a bid round", which is `Supervision::Place` — this one is the
        // `else`, reached precisely when no round is coming. The sentence was inherited from
        // `Bystanding::Unheld`'s own doc comment, written when nothing auto-placed a pending run
        // and "waiting for a bid round" meant waiting for a person to hold one; ADR-0014's
        // queued runs gave those words to the other branch and left these behind.
        //
        // Worth two sentences rather than one because they end differently. A checkpointed run
        // is parked and resumable, and the thing that moves it is a person. One that never ran
        // was refused by everybody at submission and not queued, so nothing offers it again.
        //
        // There used to be a third arm here, for a run a drain had released and was re-offering
        // from a debt in memory: `supervise` could not see that, so the report was handed the
        // fact on the side. It is gone because the fact is on the record now (ADR-0042) — the
        // decision sees it, this reads the decision, and a report and the thing it describes
        // being the same function again is the whole point of the arrangement.
        Supervision::Bystander(Bystanding::Unheld) if run.checkpoint.is_some() => {
            // One line, deliberately: written as a `\`-continued literal, `cargo fmt` collapses
            // it and keeps the indentation, which prints the sentence with a hole in the middle
            // of it. That is what `offload-node/tests/messages.rs` catches, and it caught this.
            "nobody holds it: it is parked with its checkpoint, and waits for `offload resume` rather than for a bid round".into()
        }
        Supervision::Bystander(Bystanding::Unheld) => {
            "nobody holds it, and it was not queued — nothing offers it to the fleet again on its own".into()
        }
        // The one verdict with a second sentence, because the first one is a promise about
        // *later* and later can already have gone. Said from the same core function the
        // announcement in the supervision loop uses (`Run::prospect_at`), so the two cannot
        // drift into disagreeing about whether a run is in trouble.
        //
        // **Two reasons reach it, and the reason is carried rather than guessed** (ADR-0042).
        // When this arm said "it was queued" unconditionally it was right about the only run
        // that could get here; a run a node let go now gets here too, and telling its operator
        // it was queued would be the confidently-wrong report `docs/pitfalls/reports-and-cli.md`
        // is about — the run was not queued, it was *running*, on a machine that has since left.
        Supervision::Place(offering) => {
            let head = match offering {
                Offering::Queued => {
                    "nobody holds it, and it was queued — this node offers it to the fleet again \
                     until somebody takes it"
                        .to_string()
                }
                // Named, not characterised: see `state_detail` for why this does not say
                // what the node meant by it.
                Offering::LetGo { by } => format!(
                    "nobody holds it: it was last assigned to {}, and nobody parked it, so this \
                     node offers it to the fleet until somebody takes it",
                    name_of(view, by)
                ),
            };
            match run.prospect_at(now).missed_by() {
                None => head,
                Some(late) => format!(
                    "{head}, and it is {late} past the deadline it was given, which changes \
                     nothing about that"
                ),
            }
        }
        Supervision::Bystander(Bystanding::HeldHere) => {
            "this node is holding it, and this loop is about other nodes going quiet".into()
        }
        Supervision::Bystander(Bystanding::ArbitratedBy(Some(node))) => format!(
            "{} arbitrates it; every other node watches",
            name_of(view, node)
        ),
        // Not a failure and not "nobody can have it", and not only a node's first second of
        // gossip either, which is what this comment used to say. Measured on three daemons: home
        // `kill -9`'d, a bystander restarted — its view holds nobody, and it says this for as
        // long as it meets nobody. It healed the moment that node met a peer that remembered the
        // home, as dead, and the successor rule picked. So the sentence says how it ends, the
        // way `offload schedules`' `Steward::Nobody` does for the same `steward_of` answer.
        Supervision::Bystander(Bystanding::ArbitratedBy(None)) => format!(
            "nobody arbitrates it from here: this node has not met its home node {} since it \
             started, so the successor rule has nobody to pick — that changes once it hears from \
             that node, or from any peer that remembers it",
            run.home.short()
        ),
        Supervision::Bystander(Bystanding::HolderPresent) => {
            "its holder is answering and its lease is current".into()
        }
        Supervision::Orphan => {
            "its holder is out of contact; this node is about to mark the run orphaned".into()
        }
        Supervision::Decided(ReassignDecision::NoAction) => "nothing to decide".into(),
        Supervision::Decided(ReassignDecision::Hold { until, reason }) => format!(
            "holding it for up to {} more: {reason}",
            until.saturating_sub(now)
        ),
        Supervision::Decided(ReassignDecision::Reassign { reason }) => {
            format!("moving it: {reason}")
        }
        Supervision::Decided(ReassignDecision::Abandon { reason }) => {
            format!("giving up on it: {reason}")
        }
    }
}

/// What has been captured, and whether it would survive its holder (ADR-0016).
///
/// The durability half is a question about a run that still has somewhere to go, so a run that
/// has *stopped* is not judged on it — `ps` makes the same distinction for the same reason.
/// "On one machine only" about work that is already done reads as an alarm about nothing.
///
/// **"Somewhere to go" is `may_resume_later`, not `is_terminal`.** A `Failed` run is terminal and
/// reopens (ADR-0013), so it is the one run whose checkpoint decides whether the work survives
/// its machine — and it was the one this went quiet about, printing a bare turn number where
/// `here only` belonged. The other direction is a wording fix in the same sentence: past a
/// decision to stop, the blob collector reclaims those bytes, so calling what is left a
/// checkpoint would promise something to restore from.
fn checkpoint(view: &ClusterView, run: &Run) -> Option<String> {
    let Some(checkpoint) = run.resumable_checkpoint() else {
        let stale = run.checkpoint.as_ref()?;
        return Some(format!("last captured at turn {}", stale.turns));
    };
    let holder = run.holder();
    let durable = checkpoint.is_durable(holder);
    let elsewhere: Vec<String> = checkpoint
        .replicas
        .iter()
        .filter(|n| Some(**n) != holder)
        .map(|n| name_of(view, *n))
        .collect();

    Some(if durable {
        format!(
            "turn {}, copied to {}",
            checkpoint.turns,
            elsewhere.join(", ")
        )
    } else {
        // The state worth showing somebody: the work is real and it lives on one machine.
        format!("turn {}, on one machine only", checkpoint.turns)
    })
}

/// Turn a canvass into what the CLI prints, marking whoever would win a round held now.
///
/// The winner is computed by `offload_core::bid::winner` — the same function the round itself
/// uses — so the mark cannot say one thing while a real submission does another.
#[must_use]
pub fn opinions(run: &Run, canvassed: Vec<offload_cluster::Opinion>) -> Vec<NodeOpinion> {
    let bids: Vec<offload_core::Bid> = canvassed
        .iter()
        .filter_map(|o| match &o.verdict {
            offload_cluster::Verdict::Bids(offer) => Some(offload_core::Bid {
                run: run.id,
                node: o.node,
                epoch: run.epoch,
                score: offer.score,
                available: offer.available,
                submitted_at: Millis::ZERO,
                terms: None,
            }),
            _ => None,
        })
        .collect();
    let winner = offload_core::bid::winner(&bids).map(|b| b.node);

    canvassed
        .into_iter()
        .map(|o| NodeOpinion {
            verdict: o.verdict.to_string(),
            bidding: matches!(o.verdict, offload_cluster::Verdict::Bids(_)),
            would_win: winner == Some(o.node),
            node: o.name,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use offload_core::{
        AgentKind, AgentWork, Arch, Availability, Capabilities, Constraint, DeviceClass, NodeView,
        Offer, Os, PermissionMode, Restartability, RunId, RunSpec, Score, ToolAllowlist, Work,
        WorkPolicy, WorkspaceSpec,
    };

    fn node(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    fn a_run(home: NodeId) -> Run {
        Run::new(
            RunId::from_bytes([1; 16]),
            RunSpec {
                work: Work::Agent(AgentWork {
                    agent: AgentKind::ClaudeCode,
                    model: None,
                    prompt: "add tests for the parser".into(),
                    workspace: WorkspaceSpec {
                        repo: "/r".into(),
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
            home,
            Millis(0),
        )
    }

    fn node_view(id: u8) -> NodeView {
        NodeView::new(
            node(id),
            Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Desktop),
            WorkPolicy::for_class(DeviceClass::Desktop),
            Millis(0),
        )
        .named(format!("node-{id}"))
    }

    /// A fleet of `ids`, seen from `local`, every member alive and named.
    fn view(local: u8, ids: &[u8]) -> ClusterView {
        let mut view = ClusterView::new(node(local));
        for id in ids {
            view.upsert_node(node_view(*id));
        }
        view
    }

    #[test]
    fn a_pending_run_is_not_promised_a_round_that_nothing_will_hold() {
        // The top debugging question, and the two answers people confuse: a run nobody holds is
        // waiting to be *placed*, which no amount of hold-down patience will resolve.
        //
        // **This test used to assert the promise instead of the distinction**, requiring the
        // words "bid round" — which is what `Supervision::Place` does, on the arm reached only
        // when no round is coming. So the guard held the wrong sentence in place: an operator
        // was told the fleet was about to take a run that nothing would offer it. What is
        // asserted now is what the comment above already said — placement, not patience — plus
        // the half that was missing, which is that no round is claimed.
        let view = view(1, &[1, 2]);
        let run = a_run(node(1));
        let explanation = explain(
            &view,
            &run,
            Millis(30_000),
            &ReassignPolicy::default(),
            Observed::default(),
        );

        assert_eq!(explanation.state, "pending");
        assert!(
            explanation.state_detail.contains("30.0s"),
            "{explanation:?}"
        );
        assert_eq!(explanation.holder, None);
        assert_eq!(explanation.arbiter.as_deref(), Some("node-1"));
        assert!(explanation.arbiter_is_local);
        assert!(
            !explanation.verdict.contains("waiting for a bid round"),
            "a round nothing will hold must not be promised: {explanation:?}"
        );
        assert!(
            explanation.verdict.contains("not queued"),
            "{explanation:?}"
        );
    }

    #[test]
    fn a_parked_run_is_told_what_actually_moves_it() {
        // The seam a drain exposed: a checkpointed run sitting `Pending` is one a person parked,
        // and the thing that moves it is that person typing `offload resume` — never a round.
        // Both cases reach `Bystanding::Unheld` and they end differently, so they get different
        // sentences; with one sentence between them, the run with a checkpoint got the advice
        // meant for a run that had never started.
        let view = view(1, &[1, 2]);
        let mut run = a_run(node(1));
        let epoch = run
            .assign(node(1), Millis(0), Millis(60_000))
            .expect("assign");
        run.started(node(1), epoch, Millis(0)).expect("start");
        run.checkpointed(
            node(1),
            epoch,
            offload_core::Checkpoint {
                session_id: None,
                transcript: offload_core::BlobHash::from_bytes([1; 32]),
                bundle: None,
                patch: None,
                base_commit: "0f0f".into(),
                turns: 4,
                taken_at: Millis(1_000),
                agent_version: "2.11.0".into(),
                replicas: std::collections::BTreeSet::from([node(2)]),
            },
            offload_core::GivenUp::Parked,
            Millis(1_000),
        )
        .expect("checkpointed");

        let explanation = explain(
            &view,
            &run,
            Millis(30_000),
            &ReassignPolicy::default(),
            Observed::default(),
        );

        assert_eq!(explanation.state, "pending");
        assert_eq!(explanation.holder, None);
        assert!(
            explanation.verdict.contains("offload resume"),
            "a parked run's way forward is a person: {explanation:?}"
        );
        assert!(
            !explanation.verdict.contains("waiting for a bid round"),
            "{explanation:?}"
        );
    }

    #[test]
    fn a_run_a_node_let_go_is_not_described_as_one_a_person_parked() {
        // The same run record as the test above but for one field, and the opposite advice.
        // Until ADR-0042 the difference was not in the record at all: a drain released the run
        // and could not place it, and the only thing that knew was a debt in the departing
        // node's memory. `supervise` could not see it, so the *report* inferred from the row —
        // and what is on the row is a checkpointed `Pending` run, which is what `offload
        // checkpoint` leaves too.
        //
        // Measured before any of it existed: `offload drain` with the peer down said `1 run(s)
        // still here`, and `offload explain` said the run "is parked with its checkpoint, and
        // waits for `offload resume` rather than for a bid round" — sending somebody after a run
        // nobody had parked. The fact is on the release now, so this sentence comes from the
        // verdict rather than from a second channel, and there is no longer a way to have one
        // without the other.
        let view = view(1, &[1, 2]);
        let mut run = a_run(node(1));
        let epoch = run
            .assign(node(1), Millis(0), Millis(60_000))
            .expect("assign");
        run.started(node(1), epoch, Millis(0)).expect("start");
        run.checkpointed(
            node(1),
            epoch,
            offload_core::Checkpoint {
                session_id: None,
                transcript: offload_core::BlobHash::from_bytes([1; 32]),
                bundle: None,
                patch: None,
                base_commit: "0f0f".into(),
                turns: 4,
                taken_at: Millis(1_000),
                agent_version: "2.11.0".into(),
                replicas: std::collections::BTreeSet::from([node(2)]),
            },
            offload_core::GivenUp::LetGo,
            Millis(1_000),
        )
        .expect("checkpointed");

        let explanation = explain(
            &view,
            &run,
            Millis(30_000),
            &ReassignPolicy::default(),
            Observed::default(),
        );

        assert!(
            !explanation.verdict.contains("offload resume"),
            "nobody parked this, and nobody has to unpark it: {explanation:?}"
        );
        assert!(
            explanation.verdict.contains("nobody parked it"),
            "it has to say why nobody holds it: {explanation:?}"
        );
        assert!(
            explanation.verdict.contains("until somebody takes it"),
            "and that a round is coming: {explanation:?}"
        );
        // The state line is the other half of the same mistake: two different waits printed with
        // one sentence. It names the node, because "which machine dropped this" is the next
        // question and the record is the only thing that can answer it — and it names it
        // *only*, because the record cannot say what the node meant. A give-back from a bid
        // round writes this field about a node that never held the run (`Run::release`).
        assert!(
            explanation.state_detail.contains("last assigned to"),
            "{explanation:?}"
        );
        assert!(
            !explanation.state_detail.contains("let it go")
                && !explanation.verdict.contains("let it go"),
            "neither line may claim an intention this node did not witness: {explanation:?}"
        );
    }

    #[test]
    fn a_finished_run_is_told_whether_it_met_its_deadline_rather_than_counted_down() {
        // Measured on a real daemon: a run given `--deadline 1m` completed 30 seconds inside it,
        // and `offload explain` said
        //
        //     state       completed — finished 1m25s ago
        //     due         overdue by 31.2s
        //
        // two lines apart, about a run that made its deadline comfortably — and the number kept
        // growing for as long as the row was kept. `ps` gets this right, from the same daemon
        // and the same field, because `RunSummary` filters the deadline on `!is_terminal()`.
        // A deadline stops being a countdown the moment there is nothing left to run.
        let view = view(1, &[1, 2]);
        let mut run = a_run(node(1));
        run.spec.deadline = Some(Millis(60_000));
        let epoch = run
            .assign(node(1), Millis(0), Millis(60_000))
            .expect("assign");
        run.started(node(1), epoch, Millis(0)).expect("start");
        run.complete(node(1), epoch, Millis(30_000)).expect("done");

        // Asked long after the deadline has gone by, which is when somebody actually reads this.
        let explanation = explain(
            &view,
            &run,
            Millis(600_000),
            &ReassignPolicy::default(),
            Observed::default(),
        );
        assert_eq!(explanation.state, "completed");
        assert!(
            !explanation.due.contains("overdue"),
            "a run that finished inside its deadline was reported late: {}",
            explanation.due
        );
        assert!(
            explanation.due.contains("inside its deadline"),
            "and it should say it was met: {}",
            explanation.due
        );

        // The same mistake more quietly, on the commoner run: no deadline was ever stated, and
        // `explain` reported it as still waiting, with the wait growing for ever.
        let mut asap = a_run(node(1));
        let epoch = asap
            .assign(node(1), Millis(0), Millis(60_000))
            .expect("assign");
        asap.started(node(1), epoch, Millis(0)).expect("start");
        asap.complete(node(1), epoch, Millis(30_000)).expect("done");
        let explanation = explain(
            &view,
            &asap,
            Millis(600_000),
            &ReassignPolicy::default(),
            Observed::default(),
        );
        assert!(
            !explanation.due.contains("waiting"),
            "a finished run was still counting up how long it had been waiting: {}",
            explanation.due
        );
    }

    #[test]
    fn a_run_arbitrated_elsewhere_names_the_node_that_decides() {
        // "Why did nobody move my run" is answered on the node you happen to be typing at,
        // which is usually not the one that would move it.
        let view = view(2, &[1, 2]);
        let mut run = a_run(node(1));
        let epoch = run
            .assign(node(2), Millis(0), Millis(60_000))
            .expect("assign");
        run.started(node(2), epoch, Millis(0)).expect("start");

        let explanation = explain(
            &view,
            &run,
            Millis(1_000),
            &ReassignPolicy::default(),
            Observed::default(),
        );
        assert_eq!(explanation.holder.as_deref(), Some("node-2"));
        assert_eq!(explanation.arbiter.as_deref(), Some("node-1"));
        assert!(!explanation.arbiter_is_local);
        // Taken, so not waiting for anybody to take it.
        assert!(!explanation.due.contains("waiting"), "{}", explanation.due);
        assert!(
            explanation.due.contains("submitted 1.0s ago"),
            "{}",
            explanation.due
        );
        assert!(explanation.verdict.contains("node-1"), "{explanation:?}");
    }

    #[test]
    fn a_run_whose_home_this_node_has_not_met_says_how_that_ends() {
        // Reached on three daemons by restarting a bystander after the home was `kill -9`'d: a
        // standing state for as long as the node meets nobody, not a first second of gossip.
        let view = view(2, &[2, 3]);
        let mut run = a_run(node(1));
        let epoch = run
            .assign(node(3), Millis(0), Millis(60_000))
            .expect("assign");
        run.started(node(3), epoch, Millis(0)).expect("start");

        let explanation = explain(
            &view,
            &run,
            Millis(1_000),
            &ReassignPolicy::default(),
            Observed::default(),
        );
        assert_eq!(explanation.arbiter, None);
        assert!(
            explanation.verdict.contains("since it started"),
            "{explanation:?}"
        );
        assert!(
            explanation.verdict.contains("any peer that remembers it"),
            "{explanation:?}"
        );
    }

    #[test]
    fn the_hold_down_explains_itself_with_the_numbers_it_decided_on() {
        // A held-down orphan is the case where the reason is the whole answer: the run has
        // not moved *yet*, and how long it will wait is a computed value, not a constant.
        let mut view = view(1, &[1, 2]);
        let mut run = a_run(node(1));
        let epoch = run
            .assign(node(2), Millis(0), Millis(60_000))
            .expect("assign");
        run.started(node(2), epoch, Millis(0)).expect("start");
        run.orphan(Millis(10_000)).expect("orphan");
        let mut holder = node_view(2);
        holder.set_status(offload_core::NodeStatus::Dead, Millis(10_000));
        view.upsert_node(holder);

        let explanation = explain(
            &view,
            &run,
            Millis(12_000),
            &ReassignPolicy::default(),
            Observed::default(),
        );
        assert_eq!(explanation.state, "orphaned");
        assert!(
            explanation.verdict.starts_with("holding it"),
            "{explanation:?}"
        );
        assert!(
            explanation.verdict.contains("out of contact"),
            "{explanation:?}"
        );
    }

    #[test]
    fn the_node_that_would_win_is_marked_by_the_same_rule_that_would_pick_it() {
        // A mark computed by different code from the round itself is a mark that will
        // eventually point at the wrong node, in front of somebody trying to trust it.
        let run = a_run(node(1));
        let canvassed = vec![
            offload_cluster::Opinion {
                node: node(1),
                name: "laptop".into(),
                verdict: offload_cluster::Verdict::Bids(Offer {
                    score: Score(90),
                    available: Availability::WhenFree { behind: 1 },
                    terms: None,
                }),
            },
            offload_cluster::Opinion {
                node: node(2),
                name: "desktop".into(),
                verdict: offload_cluster::Verdict::Bids(Offer {
                    score: Score(40),
                    available: Availability::Now,
                    terms: None,
                }),
            },
            offload_cluster::Opinion {
                node: node(3),
                name: "phone".into(),
                verdict: offload_cluster::Verdict::WillNot("battery 22%".into()),
            },
            offload_cluster::Opinion {
                node: node(4),
                name: "vm".into(),
                verdict: offload_cluster::Verdict::Silent,
            },
            offload_cluster::Opinion {
                node: node(5),
                name: "mac".into(),
                verdict: offload_cluster::Verdict::Unreachable(
                    "no known address — not discovered yet".into(),
                ),
            },
        ];

        let opinions = opinions(&run, canvassed);
        // Able to start now outranks a keener node that cannot, exactly as `winner` says.
        assert_eq!(opinions[1].node, "desktop");
        assert!(opinions[1].would_win);
        assert_eq!(opinions.iter().filter(|o| o.would_win).count(), 1);
        assert_eq!(
            opinions[0].verdict,
            "bid 90, starting when the run ahead of it finishes"
        );
        assert!(!opinions[2].bidding);
        assert_eq!(opinions[3].verdict, "no answer");
        // Never asked is not silence: the peer may be perfectly healthy and simply out of this
        // node's reach, which is a different thing to go and fix.
        assert_eq!(
            opinions[4].verdict,
            "not reachable from this node: no known address — not discovered yet"
        );
        assert!(!opinions[4].bidding);
    }

    #[test]
    fn a_checkpoint_on_one_machine_says_so() {
        // "here only" is the difference between closing the laptop costing nothing and
        // costing an afternoon (ADR-0016), so it is never silence.
        let mut run = a_run(node(1));
        let epoch = run
            .assign(node(2), Millis(0), Millis(60_000))
            .expect("assign");
        run.started(node(2), epoch, Millis(0)).expect("start");
        run.record_checkpoint(
            node(2),
            epoch,
            offload_core::Checkpoint {
                session_id: None,
                transcript: offload_core::BlobHash::from_bytes([1; 32]),
                bundle: None,
                patch: None,
                base_commit: "0f0f".into(),
                turns: 8,
                taken_at: Millis(0),
                agent_version: "2.11.0".into(),
                replicas: std::collections::BTreeSet::from([node(2)]),
            },
        )
        .expect("record");

        assert_eq!(
            checkpoint(&view(1, &[1, 2]), &run).as_deref(),
            Some("turn 8, on one machine only"),
            "a copy on the holder is not a copy"
        );
    }

    #[test]
    fn a_failed_run_is_the_one_that_most_needs_the_durability_line() {
        // The state whose checkpoint decides whether the work survives its machine: `Failed` is
        // terminal and it *reopens* (ADR-0013), so a copy nowhere else is exactly the alarm
        // ADR-0016 exists to raise — and `is_terminal` was the test, so this printed a bare turn
        // number and said nothing at all. Both directions, because the fix that says "here only"
        // about everything is no better than the silence.
        let mut run = a_run(node(1));
        let epoch = run
            .assign(node(2), Millis(0), Millis(60_000))
            .expect("assign");
        run.started(node(2), epoch, Millis(0)).expect("start");
        let cp =
            |replicas: std::collections::BTreeSet<offload_core::NodeId>| offload_core::Checkpoint {
                session_id: None,
                transcript: offload_core::BlobHash::from_bytes([1; 32]),
                bundle: None,
                patch: None,
                base_commit: "0f0f".into(),
                turns: 8,
                taken_at: Millis(0),
                agent_version: "2.11.0".into(),
                replicas,
            };

        let mut alone = run.clone();
        alone
            .record_checkpoint(node(2), epoch, cp(std::collections::BTreeSet::new()))
            .expect("record");
        alone
            .fail(node(2), epoch, "the agent died", Millis(1))
            .expect("fail");
        assert_eq!(
            checkpoint(&view(1, &[1, 2]), &alone).as_deref(),
            Some("turn 8, on one machine only"),
            "no holder to discount is not the same as no copy anywhere"
        );

        let mut copied = run.clone();
        copied
            .record_checkpoint(
                node(2),
                epoch,
                cp(std::collections::BTreeSet::from([node(1)])),
            )
            .expect("record");
        copied
            .fail(node(2), epoch, "the agent died", Millis(1))
            .expect("fail");
        assert_eq!(
            checkpoint(&view(1, &[1, 2]), &copied).as_deref(),
            Some("turn 8, copied to node-1")
        );

        // …and the other half of the same predicate: past a decision to stop, the collector
        // reclaims those bytes, so calling what is left a checkpoint would promise a restore.
        run.record_checkpoint(
            node(2),
            epoch,
            cp(std::collections::BTreeSet::from([node(1)])),
        )
        .expect("record");
        run.complete(node(2), epoch, Millis(1)).expect("complete");
        assert_eq!(
            checkpoint(&view(1, &[1, 2]), &run).as_deref(),
            Some("last captured at turn 8")
        );
    }

    #[test]
    fn a_remembered_escalation_stops_being_the_answer_once_the_run_is_running_again() {
        // Measured on two daemons. Alpha hosted the first leg, failed it while it was the only
        // node, and its recovery tick escalated. The run then moved to bravo, which ran it and
        // kept it alive; alpha's `offload explain` went on saying **nobody could continue it**
        // in the same output as *its holder is answering and its lease is current* and
        // *checkpoint turn 16, copied to fedora*. It never self-corrected, because the whole
        // point of `decided` is that it is remembered.
        //
        // The memory is right and stays. What was wrong is printing it as the *current* answer:
        // the undecided branch beside it has always asked `decide_recovery`, whose first act is
        // to return `None` for a run that is not `Failed`.
        let home = node(1);
        let mut run = a_run(home);
        let epoch = run
            .assign(home, Millis(1_000), offload_core::LEASE)
            .expect("assign");
        run.started(home, epoch, Millis(1_100)).expect("start");
        run.fail(home, epoch, "its agent stopped", Millis(1_200))
            .expect("fail");

        let state = crate::supervisor::RecoveryState {
            observed: Some(offload_core::Attendance::Unattended),
            resumes: 1,
            decided: Some(offload_core::Escalation::NoCopyElsewhere),
        };

        // Still failed: the decision is the answer, and this is the sentence it was written for.
        let while_failed = recovery_note(
            &state,
            &run,
            4,
            NodeStanding::default(),
            false,
            Millis(2_000),
        );
        assert!(
            while_failed.starts_with("left for a person:"),
            "a failed run's remembered decision is still the answer: {while_failed}"
        );

        // Picked back up — by a peer's own tick, or by a person elsewhere. Same remembered
        // decision, and it must no longer be presented as what is true now.
        run.reopen(Millis(2_100)).expect("reopen");
        let after = recovery_note(
            &state,
            &run,
            4,
            NodeStanding::default(),
            false,
            Millis(2_200),
        );
        assert!(
            !after.contains("left for a person"),
            "a run that is not failed any more is not one nobody could continue: {after}"
        );
        assert!(
            after.starts_with("nothing to decide — it is not failed"),
            "and the honest answer is the one the undecided branch already gives: {after}"
        );
        assert!(
            after.contains("picked up once already"),
            "the history is still worth saying: {after}"
        );
    }

    #[test]
    fn a_failed_run_is_not_told_that_nothing_is_supervising_it() {
        // Measured on one daemon, twice — once for an agent run and once for a task. `offload
        // explain` on the node that failed the run printed, on two adjacent lines:
        //
        //     it is over; nothing is supervising it
        //     recovery    picking it up again in 29.0s (try 1)
        //
        // `supervise` is right: the drop-off loop has nothing left to do with any terminal run.
        // The sentence over-claimed past what that loop decided, and `Failed` is the one of the
        // three that reopens (ADR-0013). The other two keep the old words, which are true of
        // them, so this asserts both directions.
        let home = node(1);
        let policy = ReassignPolicy::default();
        let view = view(1, &[1, 2]);

        let mut failed = a_run(home);
        let epoch = failed
            .assign(home, Millis(1_000), offload_core::LEASE)
            .expect("assign");
        failed.started(home, epoch, Millis(1_100)).expect("start");
        failed
            .fail(home, epoch, "its agent stopped", Millis(1_200))
            .expect("fail");
        let said = verdict(&view, &failed, Millis(1_300), &policy);
        assert!(
            !said.contains("nothing is supervising it"),
            "a run the recovery tick is about to pick up is not one nothing is supervising: \
             {said}"
        );
        assert!(
            said.contains("the node it failed on"),
            "and the reader is told which machine holds the other half of the answer, because \
             the `recovery` line is printed on that node alone: {said}"
        );

        let mut completed = a_run(home);
        let epoch = completed
            .assign(home, Millis(1_000), offload_core::LEASE)
            .expect("assign");
        completed
            .started(home, epoch, Millis(1_100))
            .expect("start");
        completed
            .complete(home, epoch, Millis(1_200))
            .expect("complete");
        assert_eq!(
            verdict(&view, &completed, Millis(1_300), &policy),
            "it is over; nothing is supervising it",
            "a run that finished really is over, and that sentence was always right about it"
        );
    }

    #[test]
    fn a_run_stopped_for_an_answer_is_not_described_as_running_there() {
        // The third state this arm has over-claimed about, and the first one that is *not*
        // visible in `run.state`: a run stopped mid-tool-call for a person (ADR-0017) is
        // `Running`. It only became reachable when `waiting` started being canvassed from off
        // the holder — before that the peer's screen said "it is running there" and mentioned no
        // question at all, which was a different defect and is the one this closes the other
        // half of. Both directions, since an arm that hedges about every run says nothing.
        let home = node(1);
        let mut run = a_run(home);
        let epoch = run
            .assign(node(2), Millis(1_000), offload_core::LEASE)
            .expect("assign");
        run.started(node(2), epoch, Millis(1_100)).expect("start");
        let view = view(1, &[1, 2]);

        assert_eq!(
            attendance_now(&view, &run, None, false),
            "only node-2 can tell; it is running there",
            "an ordinary run held elsewhere is running there, and saying so is the point"
        );
        let blocked = attendance_now(&view, &run, None, true);
        assert!(
            !blocked.contains("running there"),
            "a run the line below calls `stopped for an answer` is not one this line calls \
             running: {blocked}"
        );
        assert!(
            blocked.contains("only node-2 can tell"),
            "and the half that was always right — who could answer — stays: {blocked}"
        );
    }
}
