//! Whether anybody is watching, and what that means when a run breaks.
//!
//! ADR-0013's third axis, and the one that is not a field on anything. **Attendance is
//! observed, never declared**: a run is attended while some client is streaming it, and
//! unattended otherwise. Nobody has to predict at submit time whether they will still be at
//! their desk in three hours — a guess that is worth very little and is stale the moment it is
//! made — and the observation changes correctly when somebody walks away.
//!
//! What it decides is **autonomy in failure**, which follows attendance and *not* the deadline:
//!
//! * A run due in ten minutes with nobody watching should resume itself, because waiting for a
//!   human who is not there is how it misses the deadline.
//! * A run due next week that somebody is watching should stop and let them look, because they
//!   are right there and can fix it — and because retrying behind somebody's back is how a run
//!   quietly burns four turns' worth of money on the same broken tool call.
//!
//! An ordinal that bundled "soon" with "watched" got both of those backwards half the time,
//! which is the argument for three axes rather than one urgency level.
//!
//! ## Observed *when*, exactly
//!
//! At the moment the run is written `Failed`, and not later — which is a real distinction and
//! not a shortcut. A client following a run stops following when the stream ends, and a failure
//! ends the stream: sampling attendance a tick after the fact would find nobody watching every
//! single time, and the attended branch would be dead code that looked correct.
//!
//! So the question this answers is *was somebody looking when it broke* — which is also the
//! useful one, because that person has the failure in their terminal already. The escalation
//! has, in a sense, happened; what is left is not to retry behind it.
//!
//! Since it is an observation rather than a stored field, a node that has forgotten it — one
//! that restarted between the failure and the decision — passes `None`, and the answer is to
//! leave the run alone. Escalating is the safe direction: the cost is a run waiting for a
//! human, and the cost of guessing the other way is an agent restarted behind somebody's back.

use crate::run::{Restartability, Run, RunState};
use crate::time::Millis;
use serde::{Deserialize, Serialize};

/// Whether anybody is watching a run. Observed, never declared (ADR-0013).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attendance {
    /// Some client is streaming this run: `offload run --follow`, `offload logs -f`.
    Attended,
    Unattended,
}

impl Attendance {
    /// From a count of streaming clients, which is the only way this is ever known.
    #[must_use]
    pub const fn watching(clients: u32) -> Attendance {
        if clients > 0 {
            Attendance::Attended
        } else {
            Attendance::Unattended
        }
    }

    // There was an `is_attended` here, referenced by nothing at all. Worth a note rather than a
    // silent deletion: it is the predicate this module's whole argument is about, so a reader
    // would reasonably assume the decision turns on it. It does not — `decide_recovery` matches
    // the three-way `Option<Attendance>`, because *cannot tell* is a third answer and a `bool`
    // cannot hold it.
}

impl std::fmt::Display for Attendance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Attendance::Attended => "attended",
            Attendance::Unattended => "unattended",
        })
    }
}

/// How eagerly a node picks a failed run back up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryPolicy {
    /// How many times *this node* may resume one run before a person has to look.
    ///
    /// Counted in memory by the node doing the resuming rather than from `Run::attempts`,
    /// which counts every assignment: a run that migrated three times legitimately and then
    /// crashed once has not been retried three times, and spending its retries on its travels
    /// would leave the case this exists for — an agent that fell over at 02:00 — with none.
    pub max_resumes: u32,
    /// Added per resume already made, so a run that keeps breaking is picked up less often.
    pub backoff_per_resume: Millis,
    /// However urgent the run, never faster than this. What stops a run that fails on start
    /// from becoming a spin loop that bills for it.
    pub min_backoff: Millis,
    pub max_backoff: Millis,
}

impl Default for RecoveryPolicy {
    fn default() -> Self {
        RecoveryPolicy {
            max_resumes: 3,
            backoff_per_resume: Millis::from_secs(30),
            min_backoff: Millis::from_secs(5),
            max_backoff: Millis::from_mins(5),
        }
    }
}

/// What to do about a run whose agent stopped with work left behind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "recovery")]
pub enum Recovery {
    /// Start the agent again from the run's checkpoint. `resume` is which try this is.
    Resume { resume: u32 },
    /// Not yet — the backoff has not elapsed. Carries when, because "why has my run not come
    /// back" is a question with a number for an answer.
    Wait { until: Millis, resume: u32 },
    /// Leave it `Failed` and let a person deal with it.
    ///
    /// Today that means the reason it stopped is the last thing in its event stream and `ps`
    /// shows it; the delivery half — actually interrupting somebody — is ADR-0010 and phase 5.
    Escalate(Escalation),
    /// Give it back to the fleet: this node would have picked it up, and this node is leaving
    /// (ADR-0043).
    ///
    /// **The only outcome here that is not about this node's own agent**, and the one case
    /// `departing` used to end the question at. Every other answer below says the next try is
    /// unlikely to help *anywhere*; this one says the next try is unlikely to help **here**, and
    /// that being the only objection is precisely the run another machine should have.
    ///
    /// Reachable only when `departing`, and it is what `Resume` and `Wait` both become there —
    /// `Wait` included, because a backoff is a promise to try again in thirty seconds and a
    /// departing node has no thirty seconds to promise. A refusal now is not an answer about
    /// later (ADR-0041), and neither is a wait.
    ///
    /// The caller's effect is [`crate::Run::let_go`], not a resume: the run leaves `Failed` for
    /// `Pending` with this node named on it, and whoever arbitrates offers it.
    LetGo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "escalate")]
pub enum Escalation {
    /// Somebody was watching when it broke, so it is theirs. They have the failure in front of
    /// them already; restarting the agent underneath them is the thing not to do.
    SomebodyWasWatching,
    /// This node does not know whether anybody was watching — it has restarted since. Leaving
    /// the run alone is the safe direction, because the other one restarts an agent behind
    /// somebody's back.
    AttendanceUnknown,
    /// No checkpoint, or no session in it: there is no conversation to continue, so a resume
    /// would silently start a *different* run from the same prompt.
    NothingToResumeFrom,
    /// A run that cannot be re-run from its spec and has nothing to resume: `Pinned` says one
    /// holder for life, and `Run::assign` refuses a second attempt.
    CannotBeRestarted,
    /// It has been picked up this many times already and keeps failing. The next try is not
    /// the one that works.
    TooManyResumes { resumes: u32 },
    /// It has spent the turns its submitter gave it (`RunSpec::max_turns`).
    ///
    /// The one escalation here that is not about *whether a retry would work*. Every other
    /// reason on this list says the next try is unlikely to help; this one says the run is not
    /// ours to give another turn to, because a person already said how many it gets. Resuming
    /// it would be the machinery that exists to undo failures undoing an operator's own
    /// instruction — silently, unattended, and at a turn's cost each time it came round.
    ///
    /// Which is why it is checked here at all rather than only where the limit is reached:
    /// a run stopped by its budget is written `Failed`, and `Failed` is precisely the input
    /// this function turns back into a running agent.
    TurnLimitReached { limit: u32 },
    /// A machine-started run whose rule is on some other machine (ADR-0030).
    ///
    /// Not a refusal to help and not a missing feature: this node cannot answer the question a
    /// retry turns on. An occurrence is superseded by *the next event*, and the only node that
    /// sees those is the one holding the rule — so retrying here is acting on information this
    /// node does not have, three turns of an agent at a time.
    NotOursToRetry,
    /// This node is leaving, and the only copy of the conversation is on it (ADR-0043).
    ///
    /// The one reason a departing node keeps a run it would otherwise have handed to the fleet,
    /// and it is the honest half of what [`Recovery::LetGo`] promises: offering a run whose
    /// transcript exists nowhere else is offering work the winner cannot start. The bidder would
    /// take it, fail to fetch the blobs, and leave the run `Pending` behind an unreachable
    /// checkpoint — which is a worse place than `Failed`, because `Failed` at least says so.
    ///
    /// **`Restartability::Idempotent` never lands here**: that work is re-runnable from its
    /// spec, so there is nothing to fetch and nothing to lose.
    ///
    /// This is what became of `NodeIsDeparting` (ADR-0034 §1). That variant answered every
    /// departing case with a fact about the *node*, which was right while there was one answer
    /// and became a report naming the wrong cause the moment there were three: a run that had
    /// spent its turn limit was told the drain was what stopped it. Every other departing case
    /// now says the run's own reason, and the departing node still starts nothing — which was
    /// the whole of what the variant guarded.
    NoCopyElsewhere,
    /// This node is the only one there is, so there is nobody to hand the run to (ADR-0043's
    /// premise, made explicit).
    ///
    /// **Not** [`Escalation::NoCopyElsewhere`], and the difference is the whole reason this is a
    /// variant: that one is about the *run* — its conversation is on one disk — and this one is
    /// about the *fleet*. Telling somebody with one laptop that their task's conversation exists
    /// nowhere else would be a true sentence about a task that has no conversation, and a
    /// confident wrong answer about why nothing happened.
    ///
    /// Reachable for an `Idempotent` run on a node with no peers, which is a fleet of one — the
    /// mode most of this project runs in. What it replaces is `Recovery::LetGo` into a pool with
    /// no members: a `Pending` run nothing would ever offer again, which is strictly worse than
    /// the `Failed` it came from because `Failed` carries its own reason and a person can act on
    /// it.
    NobodyElseInTheFleet,
    /// The same absence of a fleet, on a node whose *owner* will not have it host (ADR-0049).
    ///
    /// The fourth corner of the 2×2 the caller computes in one match, and it exists for
    /// [`Escalation::OwnerWillNotHost`]'s reason: a laptop under its battery floor is not
    /// leaving, and telling its owner that it is would be the wrong cause said confidently. The
    /// difference matters to what they do about it — one waits for a charger, the other for a
    /// second device.
    OwnerWillNotHostAndAlone,
    /// This node's owner will not have it host runs, and the only copy of the conversation is
    /// here (ADR-0049).
    ///
    /// The policy twin of [`Escalation::NoCopyElsewhere`], and a separate variant rather than a
    /// reworded one for the reason ADR-0043 replaced `NodeIsDeparting`: an escalation is a
    /// sentence somebody reads, and "this node is leaving" about a laptop that is sitting right
    /// there with 15% of a battery is the wrong cause confidently stated. The two states differ
    /// in what the operator does next — wait for the node to come back, or plug it in.
    ///
    /// Reached only when the fleet could not have started it anyway. Where somebody else holds a
    /// copy, the answer is [`Recovery::LetGo`] and this is never said.
    OwnerWillNotHost,
    /// This node has been revoked from its fleet, so it may not start an agent for anything
    /// (ADR-0044).
    ///
    /// The only escalation here that is about *permission* rather than about whether a retry
    /// would work or whose retry it is. Every other answer on this list would be as true on the
    /// next machine; this one is true of nowhere but here, and the run is almost certainly
    /// already running somewhere else — the fleet evicted this node and moved it.
    ///
    /// Which is why it does not become [`Recovery::LetGo`] the way a drain's does. Letting go is
    /// an offer to the fleet, and a revoked node has no fleet: nothing it writes gossips, no
    /// checkpoint of its replicates, and the run it would be offering is one the fleet has
    /// already picked up under a higher epoch. There is nobody to tell and nothing to say.
    NodeIsRevoked,
}

impl std::fmt::Display for Escalation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Escalation::SomebodyWasWatching => {
                f.write_str("somebody was watching it when it failed")
            }
            Escalation::AttendanceUnknown => {
                f.write_str("this node cannot tell whether anybody was watching")
            }
            Escalation::NothingToResumeFrom => {
                f.write_str("it has no checkpointed conversation to continue")
            }
            Escalation::CannotBeRestarted => f.write_str("it is pinned to one holder for life"),
            Escalation::TooManyResumes { resumes } => {
                write!(f, "it has already been picked up {resumes} times")
            }
            Escalation::TurnLimitReached { limit } => {
                write!(f, "it reached its turn limit of {limit}")
            }
            Escalation::NotOursToRetry => {
                f.write_str(
                    "a rule on another machine started it, and only that machine can tell whether a newer event has superseded it",
                )
            }
            Escalation::NoCopyElsewhere => {
                f.write_str(
                    "this node is leaving and its conversation exists nowhere else, so nobody could continue it",
                )
            }
            Escalation::NobodyElseInTheFleet => f.write_str(
                "this node is leaving and there is no other device in the fleet to take it",
            ),
            Escalation::OwnerWillNotHostAndAlone => f.write_str(
                "this node's owner does not allow it to host runs, and there is no other device in the fleet to take it",
            ),
            Escalation::OwnerWillNotHost => f.write_str(
                "this node's owner does not allow it to host runs, and its conversation exists nowhere else",
            ),
            Escalation::NodeIsRevoked => f.write_str(
                "this node has been revoked from its fleet, so it will not start an agent for it",
            ),
        }
    }
}

/// Whether this node is in a position to decide about retrying a run at all (ADR-0030).
///
/// Separate from every other input here because it is not about the *run* — it is about this
/// node's standing to answer. A machine-started run is superseded by the next event its rule
/// sees, and a node that does not hold that rule sees none of them: it can tell the run failed,
/// and cannot tell whether retrying it is the right thing or three turns of an agent working on
/// something that stopped mattering forty seconds ago.
///
/// A type rather than a `bool` for the reason `Prospect` is one: a caller matching on it cannot
/// skip the case by accident, and the variant carries the sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// A run somebody submitted, or an occurrence of a rule this node holds. Ours to decide.
    Ours,
    /// An occurrence of a rule on another machine.
    MachineStartedElsewhere,
}

/// Whether this node's *owner* will have it host runs at all right now (ADR-0049).
///
/// The third fact here about the node rather than the run, and the one that arrives from a
/// direction the other two do not: `departing` and `revoked` are latches something set, and this
/// is a standing instruction re-answered from what the machine currently is —
/// `WorkPolicy::permits` over the last probe, so a battery that drains or a link that becomes
/// metered changes it under a running daemon (ADR-0048).
///
/// A type rather than a `bool` for [`Standing`]'s reason, and one more: it is the fourth boolean
/// this struct would have carried, and the struct exists because a boolean among small integers
/// is a call site nobody can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hosting {
    /// The owner's policy allows an agent to start here.
    Allowed,
    /// It does not: `accept = "never"`, under the battery floor, on a metered link, or an agent
    /// the owner did not allow. Which of those it is belongs in the caller's log, not here — the
    /// decision is the same for all four, and `offload status` is where the sentence lives.
    RefusedByOwner,
}

/// What the node asking knows — about the run, and about itself.
///
/// A struct rather than five more positional parameters, and the reason is the shape of the
/// mistake it prevents: `departing` is a `bool` that arrived among four small integers, so
/// `decide_recovery(&run, attendance, standing, false, 0, 0, now, &policy)` is a call nobody can
/// read and anybody can write wrong. Every field is named at every call site now, which matters
/// most for the two that must agree — the recovery tick and `offload explain`, which answer the
/// same question and would otherwise be two argument lists somebody has to line up by eye.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Circumstances {
    /// What was observed **when the run failed**, not now (see the module docs). `None` means
    /// this node has forgotten — it has restarted since — which is not the same as nobody.
    pub attendance: Option<Attendance>,
    /// Whether this run is this node's to retry at all (ADR-0030).
    pub standing: Standing,
    /// Whether this node is leaving (ADR-0034). The only field here about the node rather than
    /// the run, and the one no answer below may overrule: a drained machine is the wrong place
    /// to start an agent however resumable the run is.
    ///
    /// It no longer ends the question, though. A run this node would have picked up is a run
    /// the fleet can pick up, so on the way out `Resume` and `Wait` both become
    /// [`Recovery::LetGo`] and every other answer is the run's own (ADR-0043).
    pub departing: bool,
    /// Whether this node has been revoked from its fleet (ADR-0044). The second field here about
    /// the node rather than the run, and the one that outranks every other answer including
    /// `departing`: a drained node is the wrong place to start an agent, and a revoked one is
    /// not permitted to be one.
    pub revoked: bool,
    /// Whether the owner's policy allows this node to host anything at all (ADR-0049).
    ///
    /// Below `revoked` and below `departing`, and it behaves like the second rather than the
    /// first: a node that will not host is still a node the fleet can hear, so a run it would
    /// have resumed becomes [`Recovery::LetGo`] exactly as a departing node's does. What differs
    /// is the sentence when nobody else holds a copy — a laptop under its battery floor is not
    /// going anywhere, and telling its owner the node is *leaving* would be the wrong cause
    /// stated confidently.
    pub hosting: Hosting,
    /// Whether this node knows of **any other node at all** — the premise handing a run to the
    /// fleet rests on.
    ///
    /// `true` on a fleet of one, and that is the case this exists for: `Recovery::LetGo` leaves
    /// the run `Pending` for whoever arbitrates to offer, and on a node with no peers there is
    /// nobody to offer it to and nobody to offer it *by*. Measured — a task that exits 2, a
    /// `SIGTERM`, a restart — and the run came back `Pending` at epoch 2 and stayed there: a row
    /// in `offload ps` for ever, and, because `rule_run_in_flight` reads *non-terminal* as
    /// in-flight, **a rule jammed permanently** on a plane whose whole job is to be loud.
    ///
    /// Any node this one has *met*, not only the ones answering now: a peer in a bag is a peer
    /// that comes back, and a `Pending` run waiting for it is exactly what ADR-0043 wants. What
    /// this rules out is the fleet that has never had a second member.
    pub alone: bool,
    /// How many turns the run has taken over its whole life.
    pub turns: u32,
    /// How many times this node has already picked this run up.
    pub resumes: u32,
}

/// Should this node pick a failed run back up, or leave it for a person?
///
/// `None` means the question does not apply — the run is not `Failed` — which is the one answer
/// here that is not a decision and therefore carries no reason.
///
/// `attendance` is what was observed **when the run failed** (see the module docs on why not
/// now), and `None` there means this node has forgotten. `resumes` is how many times this node
/// has already picked this run up.
///
/// `turns` is how many turns the run has taken over its whole life, which the caller passes in
/// because it lives beside the record rather than on it ([`RunProgress`](crate::RunProgress)):
/// the node with both is the node entitled to ask.
///
/// `departing` is the one input here that is about the **node** and not the run: a drained node
/// must not start an agent, and this is the path where `stop_accepting` never reached. It is
/// applied *around* the run's own verdict rather than instead of it — the run that would have
/// been resumed here is handed to the fleet instead (ADR-0043), and every other run keeps the
/// reason that is true of it wherever it goes.
///
/// The deadline is deliberately *not* what decides whether to resume — only how long to wait
/// before doing it. A passed deadline never stops the work: ADR-0013 is explicit that missing
/// one produces a notification and never a cancellation, and a run that gave up on itself
/// because it was late would be a far worse surprise than a late one.
#[must_use]
pub fn decide_recovery(
    run: &Run,
    now: Millis,
    policy: &RecoveryPolicy,
    seen: Circumstances,
) -> Option<Recovery> {
    let RunState::Failed { at, .. } = run.state else {
        return None;
    };

    // What the *run* says, with this node's own circumstances taken out of it: would a retry
    // work at all, and is it ours to make. Split out because `departing` is the one input that
    // is not about the run, and answering it by ending the question was right only while there
    // was one thing to say (ADR-0043).
    // Above everything, `departing` included, and for a reason one step stronger than the one
    // that put `departing` above the rest. Every other input here asks whether a retry would
    // *work* and `departing` asks whether this is the machine to try it on; this asks whether
    // this machine is allowed to be a machine at all. A revoked node's runs have already been
    // moved by the fleet that evicted it, so the agent this would start is the second one on the
    // repository — the failure this project puts first (ADR-0044).
    if seen.revoked {
        return Some(Recovery::Escalate(Escalation::NodeIsRevoked));
    }

    let verdict = retryable(run, at, now, policy, seen);
    // Two reasons this machine may not be the one to try, and they behave identically here: a
    // node on its way out, and a node whose owner will not have it host anything (ADR-0049).
    // Both leave the fleet able to hear about the run, which is what separates them from
    // `revoked` above; both must start nothing here, which is what separates them from every
    // answer `retryable` gives.
    //
    // **`departing` is asked first where they differ**, because a node that is leaving is
    // leaving whatever else is true of it — the same precedence `GivenUp::LetGo` has over
    // `Parked` for the same reason.
    // **Two sentences per reason, not one**, because there are two ways a handover can be off and
    // this node's own reason has to be true of both. It is a 2×2 — leaving or refusing to host,
    // crossed with no copy of the conversation or no second device — and computing it in one
    // match is what stops the second column borrowing the first column's words: an idempotent
    // task has no conversation to be "nowhere else", and a laptop under its battery floor is not
    // "leaving". `NodeIsDeparting` was deleted for exactly that (ADR-0034 §1).
    let not_here = match (seen.departing, seen.hosting) {
        (true, _) => Some((
            Escalation::NoCopyElsewhere,
            Escalation::NobodyElseInTheFleet,
        )),
        (false, Hosting::RefusedByOwner) => Some((
            Escalation::OwnerWillNotHost,
            Escalation::OwnerWillNotHostAndAlone,
        )),
        (false, Hosting::Allowed) => None,
    };
    let Some((no_copy, no_peers)) = not_here else {
        return Some(verdict);
    };

    // **A node that will not host still starts nothing**, which is the whole of what ADR-0034 §1
    // bought and the reason this match is written as it is: `Resume` and `Wait` are the only two
    // answers that would spawn an agent, and neither one leaves here. `stop_accepting` refuses
    // the bid and the grant, and `WorkPolicy::permits` refuses the two doors a person types at
    // (ADR-0046, ADR-0047); this path asks none of them, and it went on spawning agents on a
    // machine whose own `offload status` said `accepting no` — measured for the drain in session
    // thirty-four and for the owner's policy in session forty-nine, sixty seconds after the node
    // had begun refusing everything else.
    //
    // What it no longer does is *stop the question*. "This node will not start an agent" and
    // "nobody should" are two facts, and both of these are moments they come apart: the run is
    // resumable, unattended, inside its budget, and the only objection is about the machine.
    Some(match verdict {
        Recovery::Resume { .. } | Recovery::Wait { .. } => match fleet_could_start(run, seen.alone)
        {
            Handover::Possible => Recovery::LetGo,
            // Two reasons a handover is not on, and they are different sentences: the run's
            // conversation is here only, or there is no *fleet* to hand it to. Saying the first
            // about the second would be this project's commonest mistake — a report naming a
            // cause confidently — and it is the one `NodeIsDeparting` was deleted for.
            Handover::NoCopy => Recovery::Escalate(no_copy),
            Handover::NobodyElse => Recovery::Escalate(no_peers),
        },
        // Every other answer is about the run rather than about this node, so it is exactly as
        // true of the next machine: a run that spent its turn limit has spent it everywhere, and
        // a run somebody was watching is theirs wherever it goes. Passing it through rather than
        // overwriting it with a fact about the drain — or about the battery — is what stops this
        // report naming a cause it no longer has.
        escalated => escalated,
    })
}

/// Could anybody *else* start this run from what exists outside this node?
///
/// Asked only on the way out, and it is the difference between handing a run to the fleet and
/// throwing it at one. The bidder that wins a run whose transcript is on the departing machine
/// alone fetches nothing and starts nothing, and leaves the run `Pending` behind a checkpoint
/// that may never be reachable again — which is strictly worse than the `Failed` it came from,
/// because `Failed` carries its own reason and this does not.
///
/// The holder is `None` for a failed run — the lease went with the terminal transition — so
/// [`crate::Checkpoint::is_durable`] is asking the plain question here: does anybody else have it.
fn fleet_could_start(run: &Run, alone: bool) -> Handover {
    // A conversation somebody else already holds. `is_durable` is the plain question here — the
    // holder is `None` for a failed run — and a non-empty replica set is itself a second node,
    // so this arm needs no `alone` of its own.
    if run
        .checkpoint
        .as_ref()
        .is_some_and(|cp| cp.is_durable(run.holder()))
    {
        return Handover::Possible;
    }
    // Re-runnable from its spec, so there is nothing to fetch — **and somebody has to be there
    // to read the spec.** That second half was missing, and this function's own name is what
    // says so: *could anybody else start this run*. On a fleet of one the answer was `true`, so
    // a departing node released the run into a pool with no members and nothing ever offered it
    // again, including the node that came back a second later.
    if run.spec.restartability == Restartability::Idempotent {
        return if alone {
            Handover::NobodyElse
        } else {
            Handover::Possible
        };
    }
    Handover::NoCopy
}

/// Whether a departing node can hand a run on, and if not, which of the two reasons it is.
///
/// Three answers rather than a `bool` because the two negatives are different sentences to the
/// person reading `offload explain`, and this project has paid for collapsing a pair of those
/// more than once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handover {
    /// Somebody else could pick it up: the conversation exists elsewhere, or the work is
    /// re-runnable from its spec and there is a fleet to re-run it.
    Possible,
    /// The conversation is on this machine only, so a bidder would fetch nothing.
    NoCopy,
    /// There is no other node. Nothing to do with the run.
    NobodyElse,
}

/// The run's own answer: would picking this up work, and is it ours to pick up.
///
/// Everything here asks about the run. The one input that is not — whether this node is on its
/// way out — is applied by the caller, so there is exactly one place that can say `Resume` and
/// exactly one place that decides a departing node may not hear it.
fn retryable(
    run: &Run,
    at: Millis,
    now: Millis,
    policy: &RecoveryPolicy,
    seen: Circumstances,
) -> Recovery {
    let Circumstances {
        attendance,
        standing,
        departing: _,
        revoked: _,
        hosting: _,
        // Not the run's business: whether anybody else exists decides where a run *goes*, and
        // everything here asks whether picking it up would work at all. The caller applies it,
        // for `departing`'s reason two fields up.
        alone: _,
        turns,
        resumes,
    } = seen;
    let escalate = Recovery::Escalate;

    // First, and before attendance: a run this node has no standing to retry is one whose other
    // answers are beside the point. Unattended is *always* true of an occurrence — that is what a
    // rule firing into the night means — so every check below would wave it through.
    if standing == Standing::MachineStartedElsewhere {
        return escalate(Escalation::NotOursToRetry);
    }
    // Beside standing, and above attendance, because it is the same *kind* of answer: a fact
    // that makes every question below it beside the point. Unattended is the branch that
    // resumes, and a run that has spent its budget is overwhelmingly likely to be unattended —
    // that is what a limit is for — so a check placed after attendance would be one the case it
    // exists for walks straight past.
    if let Some(limit) = run.spec.turn_limit_reached(turns) {
        return escalate(Escalation::TurnLimitReached { limit });
    }

    match attendance {
        None => return escalate(Escalation::AttendanceUnknown),
        Some(Attendance::Attended) => return escalate(Escalation::SomebodyWasWatching),
        Some(Attendance::Unattended) => {}
    }
    if run.spec.restartability == Restartability::Pinned {
        return escalate(Escalation::CannotBeRestarted);
    }
    // `Idempotent` work needs no transcript — it is re-runnable from its spec — but nothing
    // else here is: resuming without a session id would start a *different* conversation from
    // the same prompt and call it the same run.
    let resumable = run.spec.restartability == Restartability::Idempotent
        || run
            .checkpoint
            .as_ref()
            .is_some_and(|cp| cp.session_id.is_some());
    if !resumable {
        return escalate(Escalation::NothingToResumeFrom);
    }
    if resumes >= policy.max_resumes {
        return escalate(Escalation::TooManyResumes { resumes });
    }

    let until = at + backoff(run, at, resumes, policy);
    if now >= until {
        Recovery::Resume {
            resume: resumes + 1,
        }
    } else {
        Recovery::Wait {
            until,
            resume: resumes + 1,
        }
    }
}

/// How long to leave a failed run alone before picking it up again.
///
/// Grows with the number of resumes already made, and never waits past the moment the run is
/// due — **only when a deadline was stated**, for the third time in this codebase and the same
/// reason as `grace_for` and `review_commitment`: an unspecified deadline means the moment of
/// submission, so every failed run is already past due and a rule that read it would collapse
/// every backoff to the floor and retry a broken agent as fast as the floor allows.
///
/// Measured from the failure rather than from `now`, so the answer does not drift while a tick
/// is late: two nodes asking a second apart get the same instant.
///
/// Never below `min_backoff`, however pressing. A deadline changes when we give up, never what
/// we may do, and "resume instantly, for ever" is a bill rather than a policy.
fn backoff(run: &Run, failed_at: Millis, resumes: u32, policy: &RecoveryPolicy) -> Millis {
    let grown = policy
        .backoff_per_resume
        .0
        .saturating_mul(u64::from(resumes) + 1);
    let mut wait = Millis(grown.min(policy.max_backoff.0));
    if run.spec.deadline.is_some() {
        wait = wait.min(run.due_at().saturating_sub(failed_at));
    }
    wait.max(policy.min_backoff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::AgentKind;
    use crate::capacity::Demand;
    use crate::constraint::Constraint;
    use crate::id::{BlobHash, NodeId, RunId};
    use crate::run::{AgentWork, Work};
    use crate::run::{Checkpoint, PermissionMode, RunSpec, WorkspaceSpec};
    use std::collections::BTreeSet;

    fn failed_run(at: Millis) -> Run {
        let mut run = Run::new(
            RunId::from_bytes([3; 16]),
            RunSpec {
                work: Work::Agent(AgentWork {
                    agent: AgentKind::ClaudeCode,
                    model: None,
                    prompt: "p".into(),
                    workspace: WorkspaceSpec {
                        repo: "/repo".into(),
                        archive_bytes: None,
                        git_ref: None,
                        branch: None,
                    },
                    permission_mode: PermissionMode::AcceptEdits,
                    allow: crate::ToolAllowlist::default(),
                    max_turns: None,
                    ask: crate::AskPolicy::Never,
                }),
                constraint: Constraint::Always,
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
            NodeId::from_bytes([9; 32]),
            Millis(0),
        );
        let node = NodeId::from_bytes([1; 32]);
        let epoch = run.assign(node, Millis(0), Millis(60_000)).expect("assign");
        run.started(node, epoch, Millis(0)).expect("start");
        run.checkpoint = Some(Checkpoint {
            session_id: Some("s-1".into()),
            transcript: BlobHash::from_bytes([1; 32]),
            bundle: None,
            patch: None,
            base_commit: "0f0f".into(),
            turns: 4,
            taken_at: Millis(0),
            agent_version: "2.10.0".into(),
            replicas: BTreeSet::new(),
        });
        run.fail(node, epoch, "agent reported failure", at)
            .expect("fail");
        run
    }

    #[test]
    fn a_run_that_spent_its_turn_limit_is_never_picked_back_up() {
        // The half of this limit that is easy to leave out, and the one that makes the other
        // half worth having. Reaching the limit writes the run `Failed` — and `Failed` is the
        // input this function turns back into a running agent, so a cap enforced only at the
        // turn boundary is one the recovery tick undoes a backoff later, unattended, for as
        // many resumes as the policy allows.
        let policy = RecoveryPolicy::default();
        let mut run = failed_run(Millis(0));
        run.spec.agent_mut().expect("an agent run").max_turns = std::num::NonZeroU32::new(4);
        let after = Millis(0) + policy.backoff_per_resume + Millis(1);

        // Under the limit, everything else says resume — which is what makes the assertion
        // below about the limit rather than about some other refusal.
        assert!(
            matches!(
                decide_recovery(
                    &run,
                    after,
                    &policy,
                    Circumstances {
                        attendance: Some(Attendance::Unattended),
                        standing: Standing::Ours,
                        departing: false,
                        revoked: false,
                        hosting: Hosting::Allowed,
                        alone: false,
                        turns: 3,
                        resumes: 0,
                    },
                ),
                Some(Recovery::Resume { .. })
            ),
            "three turns of four still has one left"
        );

        // At it, and over it.
        for turns in [4, 5, 99] {
            assert_eq!(
                decide_recovery(
                    &run,
                    after,
                    &policy,
                    Circumstances {
                        attendance: Some(Attendance::Unattended),
                        standing: Standing::Ours,
                        departing: false,
                        revoked: false,
                        hosting: Hosting::Allowed,
                        alone: false,
                        turns,
                        resumes: 0,
                    },
                ),
                Some(Recovery::Escalate(Escalation::TurnLimitReached {
                    limit: 4
                })),
                "a run that has taken {turns} of its 4 turns was picked back up"
            );
        }

        // **Above attendance, deliberately**, and this is the ordering that carries the weight.
        // `Unattended` is the branch that resumes, and a run that quietly spent its budget
        // overnight is unattended by construction — that is the case the limit exists for — so
        // a check placed after attendance would be waved past every time it mattered. Proven
        // rather than assumed, the way `NotOursToRetry` is next door: neither of the other two
        // attendance answers gets to speak first either, since each would report a different
        // reason for the same run and "somebody was watching" is not why this one stops.
        for attendance in [Some(Attendance::Attended), None] {
            assert_eq!(
                decide_recovery(
                    &run,
                    after,
                    &policy,
                    Circumstances {
                        attendance,
                        standing: Standing::Ours,
                        departing: false,
                        revoked: false,
                        hosting: Hosting::Allowed,
                        alone: false,
                        turns: 4,
                        resumes: 0,
                    },
                ),
                Some(Recovery::Escalate(Escalation::TurnLimitReached {
                    limit: 4
                })),
            );
        }

        // And a run with no limit is untouched by any of this, which is every run submitted
        // before the flag existed.
        run.spec.agent_mut().expect("an agent run").max_turns = None;
        assert!(matches!(
            decide_recovery(
                &run,
                after,
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Unattended),
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 10_000,
                    resumes: 0,
                },
            ),
            Some(Recovery::Resume { .. })
        ));
    }

    #[test]
    fn an_occurrence_of_somebody_elses_rule_is_not_this_nodes_to_retry() {
        // ADR-0030. Measured on two daemons: a rule on alpha, every occurrence placed on beta and
        // failing there — **29 firings, 50 agent launches on beta**, bursts of five resumes in one
        // second, while alpha reported 0 events dropped because its own bookkeeping was perfect.
        // ADR-0027's withdrawal is a no-op there: the watch is on beta and the rule is on alpha.
        let policy = RecoveryPolicy::default();
        let run = failed_run(Millis(0));
        let after = Millis(0) + policy.backoff_per_resume + Millis(1);

        // Ours, and every other answer says resume: unattended, resumable, budget untouched.
        assert!(matches!(
            decide_recovery(
                &run,
                after,
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Unattended),
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 0,
                },
            ),
            Some(Recovery::Resume { .. })
        ));

        // Somebody else's, and it is left alone with the reason said.
        assert_eq!(
            decide_recovery(
                &run,
                after,
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Unattended),
                    standing: Standing::MachineStartedElsewhere,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 0,
                },
            ),
            Some(Recovery::Escalate(Escalation::NotOursToRetry)),
        );

        // **Before** attendance, deliberately. An occurrence is unattended by construction — that
        // is what a rule firing into the night means — so a check placed after this one would be
        // waved through every time, and the two answers below prove the order rather than assume
        // it: neither "somebody was watching" nor "cannot tell" gets to speak first.
        for attendance in [Some(Attendance::Attended), None] {
            assert_eq!(
                decide_recovery(
                    &run,
                    after,
                    &policy,
                    Circumstances {
                        attendance,
                        standing: Standing::MachineStartedElsewhere,
                        departing: false,
                        revoked: false,
                        hosting: Hosting::Allowed,
                        alone: false,
                        turns: 0,
                        resumes: 0,
                    },
                ),
                Some(Recovery::Escalate(Escalation::NotOursToRetry)),
            );
        }

        // And it still answers `None` for a run that has not failed: standing decides who may
        // retry, never whether there is anything to retry.
        let mut running = failed_run(Millis(0));
        running.state = RunState::Pending {
            since: Millis(0),
            let_go_by: None,
        };
        assert_eq!(
            decide_recovery(
                &running,
                after,
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Unattended),
                    standing: Standing::MachineStartedElsewhere,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 0,
                },
            ),
            None,
        );
    }

    #[test]
    fn nobody_watching_means_the_run_picks_itself_back_up() {
        // The case this exists for: 02:00, the agent fell over, and waiting for a human is how
        // the run misses a deadline nobody is awake to care about yet.
        let policy = RecoveryPolicy::default();
        let run = failed_run(Millis::from_mins(10));
        let after = Millis::from_mins(11);

        assert_eq!(
            decide_recovery(
                &run,
                after,
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Unattended),
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 0,
                },
            ),
            Some(Recovery::Resume { resume: 1 })
        );
    }

    /// The run every other answer in this function says "resume" to: unattended, ours, well
    /// inside its retry budget and past its backoff.
    fn most_resumable() -> Circumstances {
        Circumstances {
            attendance: Some(Attendance::Unattended),
            standing: Standing::Ours,
            departing: false,
            revoked: false,
            hosting: Hosting::Allowed,
            alone: false,
            turns: 0,
            resumes: 0,
        }
    }

    #[test]
    fn a_departing_node_starts_no_agent_however_resumable_the_run_is() {
        // **The path `stop_accepting` never reached.** A drain refuses bids and refuses grants;
        // auto-resume asks neither, so a drained node went on spawning agents for its own failed
        // runs. Measured on two daemons: `offload drain` returned at 11:42:50, the run failed at
        // 11:43:23, and at 11:43:54 the same node logged `nobody was watching; resuming it` and
        // `spawning claude code` — while its own `offload status` said `accepting no — drained`
        // and a peer holding a replica of the checkpoint sat idle. Fourth path this flag has
        // been missing from (ADR-0034 §1).
        //
        // The property, which ADR-0043 changed the *answer* under and must not have changed:
        // whatever else is true, a departing node never hears `Resume` or `Wait`.
        let policy = RecoveryPolicy::default();
        let mut run = failed_run(Millis::from_mins(10));
        run.checkpoint
            .as_mut()
            .expect("checkpoint")
            .replicas
            .insert(NodeId::from_bytes([7; 32]));
        let after = Millis::from_mins(11);

        assert_eq!(
            decide_recovery(&run, after, &policy, most_resumable()),
            Some(Recovery::Resume { resume: 1 }),
            "the control: staying, this run resumes"
        );

        // Every shape of failed run there is, on a node that is leaving. Exhaustive by
        // construction rather than by three hand-picked cases, because the thing being asserted
        // is that nothing falls through: `departing` is applied *around* the run's own verdict
        // now instead of instead of it, so the guarantee is a property of one `match` arm and a
        // property is what should check it.
        let mut pinned = run.clone();
        pinned.spec.restartability = Restartability::Pinned;
        let mut capped = run.clone();
        capped.spec.agent_mut().expect("an agent run").max_turns = std::num::NonZeroU32::new(1);
        let mut alone = run.clone();
        alone
            .checkpoint
            .as_mut()
            .expect("checkpoint")
            .replicas
            .clear();
        let mut bare = run.clone();
        bare.checkpoint = None;

        for candidate in [&run, &pinned, &capped, &alone, &bare] {
            for attendance in [
                None,
                Some(Attendance::Attended),
                Some(Attendance::Unattended),
            ] {
                for standing in [Standing::Ours, Standing::MachineStartedElsewhere] {
                    for resumes in [0, policy.max_resumes] {
                        for turns in [0, 4] {
                            for now in [Millis::from_mins(10), after] {
                                let decided = decide_recovery(
                                    candidate,
                                    now,
                                    &policy,
                                    Circumstances {
                                        attendance,
                                        standing,
                                        departing: true,
                                        revoked: false,
                                        hosting: Hosting::Allowed,
                                        alone: false,
                                        turns,
                                        resumes,
                                    },
                                );
                                assert!(
                                    !matches!(
                                        decided,
                                        Some(Recovery::Resume { .. } | Recovery::Wait { .. })
                                    ),
                                    "a departing node was told to start an agent: {decided:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_departing_node_with_no_peers_keeps_the_run_rather_than_handing_it_to_nobody() {
        // **`fleet_could_start` was answering half its own question.** Its name asks whether
        // anybody *else* could start the run, and for an `Idempotent` one it said yes
        // unconditionally — the work is re-runnable from its spec, so nothing has to be fetched.
        // What that leaves out is that somebody has to be there to re-run it.
        //
        // Measured on one daemon with `cluster.enabled = false`: a task that exits 2, a
        // `SIGTERM`, a restart, and the run came back `Pending` at epoch 2 with `last assigned
        // to` set — released into a pool with no members. Nothing ever offered it again,
        // including the node that came back a second later, because offering is the cluster's
        // job. It sat in `offload ps` for ever and, since `rule_run_in_flight` reads
        // *non-terminal* as in-flight, it **jammed its rule permanently**.
        let policy = RecoveryPolicy::default();
        let mut task = failed_run(Millis::from_mins(10));
        task.spec.restartability = Restartability::Idempotent;
        // A task has no conversation, which is the whole reason the idempotent arm exists.
        task.checkpoint = None;
        let after = Millis::from_mins(11);
        let leaving = Circumstances {
            departing: true,
            ..most_resumable()
        };

        assert_eq!(
            decide_recovery(&task, after, &policy, leaving),
            Some(Recovery::LetGo),
            "the control: with a fleet to hand it to, a departing node hands it over (ADR-0043)"
        );
        assert_eq!(
            decide_recovery(
                &task,
                after,
                &policy,
                Circumstances {
                    alone: true,
                    ..leaving
                }
            ),
            Some(Recovery::Escalate(Escalation::NobodyElseInTheFleet)),
            "and with nobody to hand it to it stays `Failed`, which is a state a person can act \
             on — `Pending` in an empty pool is not"
        );

        // The sentence is the *fleet's*, not the run's, and this is why it is a variant of its
        // own: telling somebody with one laptop that their task's conversation exists nowhere
        // else would be true of a task that has no conversation and a confident wrong answer
        // about why nothing happened.
        // A checkpoint with no replicas, which is what `failed_run` builds: there *is* a
        // conversation and it is on one disk. A resumable run with none at all escalates
        // earlier still, with `NothingToResumeFrom` — three reasons, three sentences.
        let resumable = failed_run(Millis::from_mins(10));
        assert_eq!(resumable.spec.restartability, Restartability::Resumable);
        assert_eq!(
            decide_recovery(
                &resumable,
                after,
                &policy,
                Circumstances {
                    alone: true,
                    ..leaving
                }
            ),
            Some(Recovery::Escalate(Escalation::NoCopyElsewhere)),
            "a resumable run with nothing captured has its own reason, and it is not this one"
        );

        // The sentence a person reads, checked here because this is where the decision is made:
        // it is about the fleet, and it must not borrow the run's.
        assert_eq!(
            Escalation::NobodyElseInTheFleet.to_string(),
            "this node is leaving and there is no other device in the fleet to take it"
        );
        assert!(
            !Escalation::NobodyElseInTheFleet
                .to_string()
                .contains("conversation"),
            "a task has no conversation, so its reason must not mention one"
        );

        // …and being alone changes nothing about a node that is *staying*: it resumes its own
        // failed run, which is the whole of autonomy in failure on a fleet of one (ADR-0013).
        assert_eq!(
            decide_recovery(
                &task,
                after,
                &policy,
                Circumstances {
                    alone: true,
                    ..most_resumable()
                }
            ),
            Some(Recovery::Resume { resume: 1 }),
            "the fleet's size is only ever a question about handing work *over*"
        );
    }

    #[test]
    fn a_node_its_owner_will_not_let_host_starts_no_agent_either() {
        // The seventh door on this gate and the last one that spawns an agent (ADR-0049).
        // Measured on one daemon whose link was turned metered underneath it: `offload status`
        // saying `accepting no — network is metered and policy disallows it`, both doors a
        // person types at refusing, and sixty seconds later `nobody was watching; resuming it`
        // and `spawning claude code`, on to turn 14.
        //
        // Swept in the same shape as the departing one above, and for the same reason: the
        // guarantee is a property of one `match` arm, so a property is what should check it.
        let policy = RecoveryPolicy::default();
        let mut run = failed_run(Millis::from_mins(10));
        run.checkpoint
            .as_mut()
            .expect("checkpoint")
            .replicas
            .insert(NodeId::from_bytes([7; 32]));
        let after = Millis::from_mins(11);

        assert_eq!(
            decide_recovery(&run, after, &policy, most_resumable()),
            Some(Recovery::Resume { resume: 1 }),
            "the control: allowed to host, this run resumes"
        );

        let mut pinned = run.clone();
        pinned.spec.restartability = Restartability::Pinned;
        let mut capped = run.clone();
        capped.spec.agent_mut().expect("an agent run").max_turns = std::num::NonZeroU32::new(1);
        let mut alone = run.clone();
        alone
            .checkpoint
            .as_mut()
            .expect("checkpoint")
            .replicas
            .clear();
        let mut bare = run.clone();
        bare.checkpoint = None;

        for candidate in [&run, &pinned, &capped, &alone, &bare] {
            for attendance in [
                None,
                Some(Attendance::Attended),
                Some(Attendance::Unattended),
            ] {
                for standing in [Standing::Ours, Standing::MachineStartedElsewhere] {
                    for resumes in [0, policy.max_resumes] {
                        for turns in [0, 4] {
                            for now in [Millis::from_mins(10), after] {
                                let decided = decide_recovery(
                                    candidate,
                                    now,
                                    &policy,
                                    Circumstances {
                                        attendance,
                                        standing,
                                        departing: false,
                                        revoked: false,
                                        hosting: Hosting::RefusedByOwner,
                                        alone: false,
                                        turns,
                                        resumes,
                                    },
                                );
                                assert!(
                                    !matches!(
                                        decided,
                                        Some(Recovery::Resume { .. } | Recovery::Wait { .. })
                                    ),
                                    "a node that may not host was told to start an agent: \
                                     {decided:?}"
                                );
                                // …and it never claims to be leaving, which is the other half:
                                // a laptop under its battery floor is sitting right there.
                                assert_ne!(
                                    decided,
                                    Some(Recovery::Escalate(Escalation::NoCopyElsewhere)),
                                    "the wrong cause, confidently stated"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_node_that_will_not_host_hands_the_run_over_where_it_can_and_says_why_where_it_cannot() {
        // The two outcomes either side of `fleet_could_start`, and the reason the policy case
        // gets its own escalation rather than borrowing the drain's: what the operator does next
        // is different — wait for the node to come back, or plug it in.
        let policy = RecoveryPolicy::default();
        let mut replicated = failed_run(Millis::from_mins(10));
        replicated
            .checkpoint
            .as_mut()
            .expect("checkpoint")
            .replicas
            .insert(NodeId::from_bytes([7; 32]));
        let after = Millis::from_mins(11);
        let refused = Circumstances {
            hosting: Hosting::RefusedByOwner,
            ..most_resumable()
        };

        assert_eq!(
            decide_recovery(&replicated, after, &policy, refused),
            Some(Recovery::LetGo),
            "somebody else holds the conversation, so the fleet can carry on with it"
        );

        // The same run with the copy taken away: a fleet of one, or a checkpoint that never got
        // out. Offering it would leave it `Pending` behind a checkpoint nobody can fetch, which
        // is worse than `Failed` — `Failed` at least says why (ADR-0043's rule, met here from
        // the other cause).
        let mut here_only = replicated.clone();
        here_only
            .checkpoint
            .as_mut()
            .expect("checkpoint")
            .replicas
            .clear();
        assert_eq!(
            decide_recovery(&here_only, after, &policy, refused),
            Some(Recovery::Escalate(Escalation::OwnerWillNotHost)),
        );

        // The fourth corner, and the reason the caller computes a *pair* of sentences rather
        // than one: an idempotent task on a refusing node has no conversation to be nowhere
        // else, so `OwnerWillNotHost` — "no other device holds a copy" — would name a cause that
        // cannot apply to it. What is actually wrong is that there is no other device at all,
        // and the two are fixed by different actions: plug this one in, or add a second.
        let mut task = here_only.clone();
        task.spec.restartability = Restartability::Idempotent;
        task.checkpoint = None;
        assert_eq!(
            decide_recovery(&task, after, &policy, refused),
            Some(Recovery::LetGo),
            "the control: re-runnable work goes to a fleet that has somebody in it"
        );
        assert_eq!(
            decide_recovery(
                &task,
                after,
                &policy,
                Circumstances {
                    alone: true,
                    ..refused
                }
            ),
            Some(Recovery::Escalate(Escalation::OwnerWillNotHostAndAlone)),
        );

        // And when both are true, the departing sentence wins: the node is leaving whatever
        // else its owner said, which is the precedence `GivenUp::LetGo` has over `Parked`.
        assert_eq!(
            decide_recovery(
                &here_only,
                after,
                &policy,
                Circumstances {
                    departing: true,
                    ..refused
                }
            ),
            Some(Recovery::Escalate(Escalation::NoCopyElsewhere)),
        );

        // …and revoked outranks both, still: there is nobody to hand it to.
        assert_eq!(
            decide_recovery(
                &replicated,
                after,
                &policy,
                Circumstances {
                    revoked: true,
                    departing: true,
                    ..refused
                }
            ),
            Some(Recovery::Escalate(Escalation::NodeIsRevoked)),
        );
    }

    #[test]
    fn a_revoked_node_neither_starts_an_agent_nor_offers_the_run_to_anybody() {
        // ADR-0044, and the sweep is the same shape as the departing one above because the
        // guarantee is the same *kind* of guarantee — a property of one arm, not a claim about
        // line order. The answer is stronger, though: a drained node hands the run to the fleet,
        // and a revoked node has no fleet to hand it to. `LetGo` here would mean writing
        // `Pending` into a store nothing gossips out of, about a run the fleet has already taken
        // back under a higher epoch.
        let policy = RecoveryPolicy::default();
        let mut run = failed_run(Millis::from_mins(10));
        run.checkpoint
            .as_mut()
            .expect("checkpoint")
            .replicas
            .insert(NodeId::from_bytes([7; 32]));
        let after = Millis::from_mins(11);

        assert_eq!(
            decide_recovery(&run, after, &policy, most_resumable()),
            Some(Recovery::Resume { resume: 1 }),
            "the control: still a member, this run resumes"
        );

        let mut pinned = run.clone();
        pinned.spec.restartability = Restartability::Pinned;
        let mut capped = run.clone();
        capped.spec.agent_mut().expect("an agent run").max_turns = std::num::NonZeroU32::new(1);
        let mut idempotent = run.clone();
        idempotent.spec.restartability = Restartability::Idempotent;
        let mut alone = run.clone();
        alone
            .checkpoint
            .as_mut()
            .expect("checkpoint")
            .replicas
            .clear();
        let mut bare = run.clone();
        bare.checkpoint = None;

        for candidate in [&run, &pinned, &capped, &idempotent, &alone, &bare] {
            for attendance in [
                None,
                Some(Attendance::Attended),
                Some(Attendance::Unattended),
            ] {
                for standing in [Standing::Ours, Standing::MachineStartedElsewhere] {
                    // Both, because `revoked` has to outrank `departing` as well as everything
                    // the run says: a revoked node that is also draining must not reach `LetGo`.
                    for departing in [false, true] {
                        for resumes in [0, policy.max_resumes] {
                            for turns in [0, 4] {
                                for now in [Millis::from_mins(10), after] {
                                    assert_eq!(
                                        decide_recovery(
                                            candidate,
                                            now,
                                            &policy,
                                            Circumstances {
                                                attendance,
                                                standing,
                                                departing,
                                                revoked: true,
                                                hosting: Hosting::Allowed,
                                                alone: false,
                                                turns,
                                                resumes,
                                            },
                                        ),
                                        Some(Recovery::Escalate(Escalation::NodeIsRevoked)),
                                        "a revoked node decided something else about a run"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_departing_node_hands_back_the_run_it_would_have_picked_up() {
        // ADR-0043. "This node will not start an agent" and "nobody should" are two facts, and
        // a drain is where they come apart: the run below is resumable, unattended, inside its
        // budget, and the only objection to it is a machine that is going away. It used to be
        // left `Failed` in front of a fleet that could have carried on with it — and left there
        // for good, because `supervise` reads failed as terminal and nobody bids on it.
        let policy = RecoveryPolicy::default();
        let mut run = failed_run(Millis::from_mins(10));
        run.checkpoint
            .as_mut()
            .expect("checkpoint")
            .replicas
            .insert(NodeId::from_bytes([7; 32]));
        let after = Millis::from_mins(11);

        assert_eq!(
            decide_recovery(
                &run,
                after,
                &policy,
                Circumstances {
                    departing: true,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    ..most_resumable()
                },
            ),
            Some(Recovery::LetGo),
        );

        // And *before* the backoff has elapsed, which is the case worth writing down: a backoff
        // is a promise to try again in thirty seconds, and a departing node has no thirty
        // seconds to promise. Staying, this is `Wait`.
        let soon = Millis::from_mins(10) + policy.min_backoff.saturating_sub(Millis(1));
        assert!(matches!(
            decide_recovery(&run, soon, &policy, most_resumable()),
            Some(Recovery::Wait { .. })
        ));
        assert_eq!(
            decide_recovery(
                &run,
                soon,
                &policy,
                Circumstances {
                    departing: true,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    ..most_resumable()
                },
            ),
            Some(Recovery::LetGo),
            "a wait this node will not be here for is not a reason to keep the run"
        );
    }

    #[test]
    fn a_run_whose_only_copy_is_here_is_not_offered_to_a_fleet_that_cannot_start_it() {
        // The honest half of the hand-back. `replicas` is empty, so the transcript exists on
        // this node alone: a bidder would win the run, fetch nothing, and leave it `Pending`
        // behind a checkpoint that may never be reachable again — strictly worse than the
        // `Failed` it came from, which at least carries its own reason.
        let policy = RecoveryPolicy::default();
        let run = failed_run(Millis::from_mins(10));
        assert!(run
            .checkpoint
            .as_ref()
            .expect("checkpoint")
            .replicas
            .is_empty());

        assert_eq!(
            decide_recovery(
                &run,
                Millis::from_mins(11),
                &policy,
                Circumstances {
                    departing: true,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    ..most_resumable()
                },
            ),
            Some(Recovery::Escalate(Escalation::NoCopyElsewhere)),
        );

        // Unless there is nothing to fetch in the first place: idempotent work is re-runnable
        // from its spec, so the fleet needs no copy of anything.
        let mut idempotent = run.clone();
        idempotent.spec.restartability = Restartability::Idempotent;
        idempotent.checkpoint = None;
        assert_eq!(
            decide_recovery(
                &idempotent,
                Millis::from_mins(11),
                &policy,
                Circumstances {
                    departing: true,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    ..most_resumable()
                },
            ),
            Some(Recovery::LetGo),
        );
    }

    #[test]
    fn a_departing_node_reports_the_runs_own_reason_rather_than_the_drain() {
        // What `Escalation::NodeIsDeparting` used to do to every one of these, and the reason
        // it went: a report that names a cause has to be told when it stops being the cause.
        // The drain is not why a capped run will never run again, and telling somebody it is
        // sends them to restart the daemon.
        let policy = RecoveryPolicy::default();
        let now = Millis::from_mins(11);
        let base = failed_run(Millis::from_mins(10));
        let departing = Circumstances {
            departing: true,
            revoked: false,
            hosting: Hosting::Allowed,
            ..most_resumable()
        };

        let mut capped = base.clone();
        capped.spec.agent_mut().expect("an agent run").max_turns = std::num::NonZeroU32::new(1);
        assert_eq!(
            decide_recovery(
                &capped,
                now,
                &policy,
                Circumstances {
                    turns: 4,
                    ..departing
                }
            ),
            Some(Recovery::Escalate(Escalation::TurnLimitReached {
                limit: 1
            })),
        );

        assert_eq!(
            decide_recovery(
                &base,
                now,
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Attended),
                    ..departing
                },
            ),
            Some(Recovery::Escalate(Escalation::SomebodyWasWatching)),
        );

        assert_eq!(
            decide_recovery(
                &base,
                now,
                &policy,
                Circumstances {
                    standing: Standing::MachineStartedElsewhere,
                    ..departing
                },
            ),
            Some(Recovery::Escalate(Escalation::NotOursToRetry)),
        );

        let mut pinned = base.clone();
        pinned.spec.restartability = Restartability::Pinned;
        assert_eq!(
            decide_recovery(&pinned, now, &policy, departing),
            Some(Recovery::Escalate(Escalation::CannotBeRestarted)),
        );
    }

    #[test]
    fn somebody_watching_keeps_the_failure_rather_than_retrying_behind_them() {
        // They have the reason in their terminal already, and a fresh agent started underneath
        // them is four turns of money spent on the same broken tool call.
        let policy = RecoveryPolicy::default();
        let run = failed_run(Millis::from_mins(10));
        assert_eq!(
            decide_recovery(
                &run,
                Millis::from_mins(30),
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Attended),
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 0,
                },
            ),
            Some(Recovery::Escalate(Escalation::SomebodyWasWatching))
        );
        // Due next week and watched still escalates: autonomy follows attendance, not urgency.
        let mut relaxed = run.clone();
        relaxed.spec.deadline = Some(Millis::from_mins(10_000));
        assert!(matches!(
            decide_recovery(
                &relaxed,
                Millis::from_mins(30),
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Attended),
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 0,
                },
            ),
            Some(Recovery::Escalate(Escalation::SomebodyWasWatching))
        ));
    }

    #[test]
    fn a_node_that_has_forgotten_who_was_watching_leaves_the_run_alone() {
        // A daemon that restarted between the failure and this decision knows nothing, and
        // guessing "nobody" restarts an agent behind somebody's back.
        let policy = RecoveryPolicy::default();
        let run = failed_run(Millis::from_mins(10));
        assert_eq!(
            decide_recovery(
                &run,
                Millis::from_mins(30),
                &policy,
                Circumstances {
                    attendance: None,
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 0,
                },
            ),
            Some(Recovery::Escalate(Escalation::AttendanceUnknown))
        );
    }

    #[test]
    fn the_backoff_grows_and_a_stated_deadline_can_shorten_it_but_not_past_the_floor() {
        let policy = RecoveryPolicy::default();
        let run = failed_run(Millis::from_mins(10));
        let just_after = Millis::from_mins(10) + Millis(1);

        // First resume waits 30s, the third 90s: a run that keeps breaking is picked up less
        // often, which is the whole of what `resumes` buys.
        assert_eq!(
            decide_recovery(
                &run,
                just_after,
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Unattended),
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 0,
                },
            ),
            Some(Recovery::Wait {
                until: Millis::from_mins(10) + Millis::from_secs(30),
                resume: 1
            })
        );
        assert_eq!(
            decide_recovery(
                &run,
                just_after,
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Unattended),
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 2,
                },
            ),
            Some(Recovery::Wait {
                until: Millis::from_mins(10) + Millis::from_secs(90),
                resume: 3
            })
        );

        // A run due in ten seconds does not wait thirty for its retry...
        let mut pressing = run.clone();
        pressing.spec.deadline = Some(Millis::from_mins(10) + Millis::from_secs(10));
        assert_eq!(
            decide_recovery(
                &pressing,
                just_after,
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Unattended),
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 0,
                },
            ),
            Some(Recovery::Wait {
                until: Millis::from_mins(10) + Millis(10_000),
                resume: 1
            })
        );

        // ...and an overdue one still waits the floor rather than spinning. `min_backoff` is
        // the line between "as soon as you can" and a billing loop.
        let mut late = run.clone();
        late.spec.deadline = Some(Millis(0));
        assert_eq!(
            decide_recovery(
                &late,
                just_after,
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Unattended),
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 0,
                },
            ),
            Some(Recovery::Wait {
                until: Millis::from_mins(10) + policy.min_backoff,
                resume: 1
            })
        );
    }

    #[test]
    fn a_run_with_no_stated_deadline_gets_the_full_backoff_however_old_it_is() {
        // The trap, in its third disguise. This run's slack is negative by an hour because
        // nobody said when it was due; reading that as urgency would retry a broken agent at
        // the floor for ever.
        let policy = RecoveryPolicy::default();
        let run = failed_run(Millis::from_mins(60));
        assert_eq!(run.spec.deadline, None);
        assert_eq!(
            decide_recovery(
                &run,
                Millis::from_mins(60) + Millis(1),
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Unattended),
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 0,
                },
            ),
            Some(Recovery::Wait {
                until: Millis::from_mins(60) + policy.backoff_per_resume,
                resume: 1
            })
        );
    }

    #[test]
    fn a_run_that_keeps_failing_ends_up_in_front_of_a_person() {
        let policy = RecoveryPolicy::default();
        let run = failed_run(Millis::from_mins(10));
        assert_eq!(
            decide_recovery(
                &run,
                Millis::from_mins(30),
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Unattended),
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: policy.max_resumes,
                },
            ),
            Some(Recovery::Escalate(Escalation::TooManyResumes {
                resumes: 3
            }))
        );
    }

    #[test]
    fn there_is_nothing_to_resume_without_a_session_and_nothing_to_decide_unless_it_failed() {
        let policy = RecoveryPolicy::default();
        let mut run = failed_run(Millis::from_mins(10));
        let now = Millis::from_mins(30);

        run.checkpoint = None;
        assert_eq!(
            decide_recovery(
                &run,
                now,
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Unattended),
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 0,
                },
            ),
            Some(Recovery::Escalate(Escalation::NothingToResumeFrom))
        );
        // Except for work that is re-runnable from its spec, which never needed a transcript.
        let mut watcher = run.clone();
        watcher.spec.restartability = Restartability::Idempotent;
        assert_eq!(
            decide_recovery(
                &watcher,
                now,
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Unattended),
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 0,
                },
            ),
            Some(Recovery::Resume { resume: 1 })
        );

        // And a run that has not failed is not a question this answers.
        let mut fine = failed_run(Millis::from_mins(10));
        fine.reopen(now).expect("reopen");
        assert_eq!(
            decide_recovery(
                &fine,
                now,
                &policy,
                Circumstances {
                    attendance: Some(Attendance::Unattended),
                    standing: Standing::Ours,
                    departing: false,
                    revoked: false,
                    hosting: Hosting::Allowed,
                    alone: false,
                    turns: 0,
                    resumes: 0,
                },
            ),
            None
        );
    }
}
