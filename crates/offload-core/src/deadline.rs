//! When a run needs to be done, and the urgency that follows from it.
//!
//! ADR-0013 splits what a single urgency level used to conflate. The one this module is
//! about is **deadline: how long a wait may be**. It is an absolute instant, so it is
//! compared against an injected `now` and nothing here reads a clock.
//!
//! Two properties are the whole reason it is a deadline rather than a level:
//!
//! * **Urgency is derived, so it rises on its own.** [`Slack`] is `f(deadline, now)`. A run
//!   gets more urgent as the morning approaches without anybody editing anything and without
//!   a single message being exchanged — every node computes the same value at the same
//!   instant from state it already has. A stored level would need a revision counter, an
//!   owner, and somebody to remember to bump it.
//! * **An unspecified deadline means the moment of submission**, not "no hurry". That is what
//!   somebody typing `offload run` with nothing further actually means: *as soon as you can*.
//!   It also makes slack equal to negative age, decreasing for as long as the run waits — so
//!   **aging is free and starvation needs no anti-starvation mechanism**. A queue of runs with
//!   no deadlines is ordered by how long each has been waiting, because that is exactly what
//!   its urgency measures.
//!
//! ## The distinction that is easy to lose
//!
//! Slack is routinely negative, and it means two different things depending on where it came
//! from. A run with **no** deadline is late from its second second: that is a perfectly good
//! way to order runs against each other, and a terrible way to decide how long to wait for a
//! laptop that is probably just suspended. So:
//!
//! * **Ordering and pickiness** use [`Slack`] whatever its origin. The most overdue run a node
//!   could take is the one it takes ([`Run::urgency_order`]).
//! * **Patience** — how long to hold a run down before moving it (`policy::grace_for`) — is
//!   shortened only by a deadline somebody actually stated. Cutting the hold-down costs a turn
//!   and risks the migration thrash ADR-0007 exists to avoid, and "as soon as you can" is a
//!   preference, not a time the run must land by.
//!
//! And the boundary from CLAUDE.md, because this is exactly where a scheduling hint turns into
//! a correctness bug: a deadline changes *when we give up*, never *what we are allowed to do*.
//! No deadline, however close, may weaken epoch fencing, skip a checkpoint, snapshot mid-turn,
//! or shorten a lease. When the two are in tension, the run misses its deadline.

use crate::run::Run;
use crate::time::Millis;
use serde::{Deserialize, Serialize};

/// How long a run has left before it is due. Negative when it is late.
///
/// Signed, unlike [`Millis`], because being overdue is the normal case rather than an error:
/// a run with no stated deadline is overdue from the moment after it was submitted, and that
/// is how aging comes for free.
///
/// **Ordering is by urgency**: `Ord` is the natural one on the underlying milliseconds, so
/// sorting ascending puts the run in the most trouble first. That is the direction every
/// caller wants and the reason not to store this as "urgency", which would sort the other way
/// and read the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Slack(pub i64);

impl Slack {
    /// The time between an instant a run is due and the instant it is being asked about.
    ///
    /// Both arguments are unsigned instants; the subtraction is done in whichever direction
    /// keeps it in range, because `Millis` saturates at zero and a saturated difference would
    /// report every late run as being due exactly now.
    #[must_use]
    pub fn between(due: Millis, now: Millis) -> Slack {
        let magnitude = |m: Millis| i64::try_from(m.0).unwrap_or(i64::MAX);
        if due >= now {
            Slack(magnitude(due - now))
        } else {
            Slack(-magnitude(now - due))
        }
    }

    /// Past due. A run at exactly its deadline counts as overdue: the deadline is the instant
    /// it was supposed to be *finished*, and it plainly is not.
    #[must_use]
    pub const fn is_overdue(self) -> bool {
        self.0 <= 0
    }

    /// The time still available, or `None` if there is none.
    ///
    /// The awkward shape is deliberate: every caller that spends slack has to answer what it
    /// does when there is none left, and the tempting reading — "refuse everything, I am
    /// already late" — is exactly wrong. Being picky is how a late run gets later.
    #[must_use]
    pub fn remaining(self) -> Option<Millis> {
        u64::try_from(self.0).ok().filter(|m| *m > 0).map(Millis)
    }
}

/// Where a run will stand at some instant: still in time, or past a deadline somebody set.
///
/// The question [`Slack`] answers, asked about a moment that something has just made
/// unavoidable — a rate-limit reset, or simply now, for a wait with no end in sight. It exists
/// because two of ADR-0013's failure rows end at the same fork: *wait, or tell somebody*. A
/// delay that fits inside a run's slack is nobody's business; one that does not is the run
/// quietly not happening, and nothing else in this system is going to notice on its behalf.
///
/// **The distinction that carries all the weight is [`Prospect::NoStatedDeadline`]**, which is
/// this trap's fourth appearance (`grace_for`, `review_commitment`, and now here): an
/// unspecified deadline means the moment of submission, so *every* run is past due within a
/// second of being submitted. A rule that skipped this check would announce a missed deadline
/// for every ordinary run in the fleet, which is how a useful alarm becomes noise nobody reads.
/// ADR-0013's own wording is the test: a run that cannot make its deadline says so; one without
/// stays pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "prospect")]
pub enum Prospect {
    /// Nobody stated a deadline, so there is nothing to miss and nothing to announce.
    NoStatedDeadline,
    /// There is still time when the wait ends. Deliberately not "it will make it": nothing
    /// here knows how long an agent turn takes, and a promise is not what the caller needs —
    /// it needs to know whether to bother a person.
    TimeLeft { spare: Millis },
    /// The deadline has gone, or will have by the time the wait ends.
    Missed { by: Millis },
}

impl Prospect {
    /// How far past due, for the one caller that acts on it. `None` for the two cases that
    /// are not a miss, so the announcement path cannot accidentally treat them as one.
    #[must_use]
    pub const fn missed_by(self) -> Option<Millis> {
        match self {
            Prospect::Missed { by } => Some(by),
            _ => None,
        }
    }
}

impl std::fmt::Display for Prospect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Prospect::NoStatedDeadline => f.write_str("no deadline was stated for it"),
            Prospect::TimeLeft { spare } => write!(f, "{spare} inside its deadline"),
            Prospect::Missed { by } => write!(f, "{by} past its deadline"),
        }
    }
}

impl std::fmt::Display for Slack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.remaining() {
            Some(left) => write!(f, "{left} left"),
            None => {
                let late = u64::try_from(-self.0).unwrap_or(0);
                write!(f, "overdue by {}", Millis(late))
            }
        }
    }
}

impl Run {
    /// When this run is due.
    ///
    /// `None` on the spec means the moment it was submitted, which is neither a bogus
    /// timestamp nor "no deadline" — it is *as soon as you can*, and it is what makes a
    /// waiting run get more urgent without an aging mechanism. The `Option` survives on the
    /// spec so a display can say `asap` rather than printing a time in the past.
    #[must_use]
    pub fn due_at(&self) -> Millis {
        self.spec.deadline.unwrap_or(self.created_at)
    }

    /// How much time this run has left, which is routinely negative. See [`Slack`].
    #[must_use]
    pub fn slack(&self, now: Millis) -> Slack {
        Slack::between(self.due_at(), now)
    }

    /// Where this run stands at `at` — the instant a delay it has just run into will end.
    ///
    /// One function for two of ADR-0013's failure rows, because they are one question asked
    /// about different waits. A rate limit says when it lifts, so `at` is that instant. A
    /// fleet that refused the run says nothing about when it will change its mind, so `at` is
    /// `now`: with no end to wait for, the only thing that can be established is whether the
    /// deadline has *already* gone. Passing `now` is therefore not a degenerate case, it is
    /// the honest one — and it keeps this a pure comparison rather than a forecast.
    ///
    /// See [`Prospect`] for why a run with no stated deadline is answered separately rather
    /// than reported as overdue, which by the rules of this module it always is.
    #[must_use]
    pub fn prospect_at(&self, at: Millis) -> Prospect {
        if self.spec.deadline.is_none() {
            return Prospect::NoStatedDeadline;
        }
        let slack = Slack::between(self.due_at(), at);
        match slack.remaining() {
            Some(spare) => Prospect::TimeLeft { spare },
            None => Prospect::Missed {
                by: Millis(u64::try_from(-slack.0).unwrap_or(0)),
            },
        }
    }

    /// How long to wait for a person to answer a permission question (ADR-0017).
    ///
    /// The wait happens **mid-turn**: the agent is blocked inside a tool call, so nothing can be
    /// checkpointed (ADR-0004), nothing can be drained, and a lease is being renewed for a
    /// machine that is doing nothing. That is what makes this bounded rather than patient — and
    /// it is the opposite of a queued notification, which is durable and rightly waits for hours
    /// (ADR-0010's *away is not gone*).
    ///
    /// A *stated* deadline shortens it and nothing else does. An unspecified deadline means the
    /// moment of submission (see [`Run::due_at`]), so reading slack unconditionally would give
    /// every ordinary run zero patience and deny every question the instant it was asked — the
    /// same trap as everywhere else in this module, and the one place where it would look exactly
    /// like the feature being broken. Floored, so a run that is already overdue still gets long
    /// enough for somebody holding a phone to answer.
    #[must_use]
    pub fn approval_patience(&self, now: Millis, default: Millis, floor: Millis) -> Millis {
        // Three cases, and the first two are the ones it is fatal to collapse: *no deadline* is
        // the ordinary run, which gets the full default, while *past its deadline* is a run whose
        // slack has run out and gets the floor. `Slack::remaining` reports `None` for both.
        if self.spec.deadline.is_none() {
            return default.max(floor);
        }
        match self.slack(now).remaining() {
            Some(spare) => spare.min(default).max(floor),
            None => floor,
        }
    }

    /// The order to take runs in when there is capacity for some of them but not all.
    ///
    /// Ascending: the most urgent sorts first. Slack leads and priority breaks ties, which is
    /// ADR-0013's split — *how long can this wait* and *who goes first* are two questions, and
    /// answering both with one number is what an urgency level got wrong. Because slack grows
    /// without bound as a run waits, any run eventually outranks any priority: **priority is a
    /// head start, not a veto**, which is why it can stay an advisory integer with no fairness
    /// machinery of its own.
    ///
    /// Ordering is rough by design and frequently irrelevant — nodes bid independently and
    /// there is no queue to serialise. This is what a *single* node does with the runs it is
    /// already holding, which is the only place an order exists at all.
    #[must_use]
    pub fn urgency_order(&self, now: Millis) -> (Slack, std::cmp::Reverse<i32>) {
        (self.slack(now), std::cmp::Reverse(self.spec.priority))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::AgentKind;
    use crate::capacity::Demand;
    use crate::constraint::Constraint;
    use crate::id::{NodeId, RunId};
    use crate::run::{AgentWork, Work};
    use crate::run::{PermissionMode, Restartability, RunSpec, WorkspaceSpec};

    fn run(created_at: Millis, deadline: Option<Millis>, priority: i32) -> Run {
        let spec = RunSpec {
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
            priority,
            queue: false,
            deadline,
            demand: Demand::Normal,
            notify: Default::default(),
            notices: Default::default(),
            resources: Vec::new(),
            prefer: crate::Constraint::Always,
            hold_until: None,
            parent: None,
        };
        Run::new(
            RunId::from_bytes([1; 16]),
            spec,
            NodeId::from_bytes([9; 32]),
            created_at,
        )
    }

    #[test]
    fn patience_for_a_question_is_bounded_and_a_stated_deadline_is_what_shortens_it() {
        let default = Millis::from_mins(5);
        let floor = Millis::from_secs(30);
        let now = Millis::from_mins(60);

        // No stated deadline: the full default. The trap, and the one place it would look
        // exactly like the feature not working — an unspecified deadline means the moment of
        // submission, so reading slack here would deny every question the instant it was asked.
        let asap = run(Millis::from_mins(0), None, 0);
        assert!(
            asap.slack(now).is_overdue(),
            "it is an hour past 'as soon as you can'"
        );
        assert_eq!(asap.approval_patience(now, default, floor), default);

        // A deadline with plenty of room: still the default, because patience is about the run
        // being stalled mid-turn and not about how much time it has.
        let tomorrow = run(Millis::from_mins(0), Some(Millis::from_mins(600)), 0);
        assert_eq!(tomorrow.approval_patience(now, default, floor), default);

        // A deadline two minutes out: two minutes is all the waiting it can pay for.
        let soon = run(Millis::from_mins(0), Some(Millis::from_mins(62)), 0);
        assert_eq!(
            soon.approval_patience(now, default, floor),
            Millis::from_mins(2)
        );

        // Already past it: the floor rather than zero, because somebody reaching for a phone
        // should still be able to answer, and denying instantly is not what a missed deadline
        // decides (ADR-0013).
        let late = run(Millis::from_mins(0), Some(Millis::from_mins(30)), 0);
        assert_eq!(late.approval_patience(now, default, floor), floor);
    }

    #[test]
    fn a_run_with_no_deadline_gets_more_urgent_by_waiting() {
        // The property that makes aging free: nothing bumps anything, and the run submitted
        // an hour ago is more urgent than the one submitted a minute ago, on every node, with
        // no message exchanged.
        let old = run(Millis::from_mins(0), None, 0);
        let recent = run(Millis::from_mins(59), None, 0);
        let now = Millis::from_mins(60);

        assert!(old.slack(now) < recent.slack(now));
        assert!(old.slack(now).is_overdue());
        assert_eq!(old.slack(now), Slack(-3_600_000));
    }

    #[test]
    fn urgency_rises_on_its_own_as_the_deadline_approaches() {
        let overnight = run(Millis::from_mins(0), Some(Millis::from_mins(540)), 0);
        assert_eq!(
            overnight.slack(Millis::from_mins(60)),
            Slack(480 * 60 * 1_000)
        );
        assert!(!overnight.slack(Millis::from_mins(539)).is_overdue());
        assert!(overnight.slack(Millis::from_mins(540)).is_overdue());
    }

    #[test]
    fn the_most_overdue_run_goes_first_and_priority_only_breaks_ties() {
        // Two runs a node is holding and one free slot. The old one wins even though the new
        // one was submitted at a higher priority: priority is a head start, not a veto, and a
        // run that could be outranked for ever is the starvation this ordering exists to
        // prevent.
        let now = Millis::from_mins(60);
        let waiting = run(Millis::from_mins(0), None, 0);
        let eager = run(Millis::from_mins(59), None, 10);
        assert!(waiting.urgency_order(now) < eager.urgency_order(now));

        // Same submission instant, so nothing separates them but the number that exists to.
        let a = run(Millis::from_mins(10), None, 0);
        let b = run(Millis::from_mins(10), None, 5);
        assert!(b.urgency_order(now) < a.urgency_order(now));
    }

    #[test]
    fn a_run_with_no_stated_deadline_can_never_miss_one() {
        // The whole reason `Prospect` has three variants rather than two. This run is overdue
        // by an hour on every other measure in this module, and it is not a run anybody should
        // be told about: "as soon as you can" is a preference, not a time it must land by.
        let asap = run(Millis::from_mins(0), None, 0);
        let now = Millis::from_mins(60);
        assert!(asap.slack(now).is_overdue());
        assert_eq!(asap.prospect_at(now), Prospect::NoStatedDeadline);
        assert_eq!(asap.prospect_at(Millis::from_mins(600)).missed_by(), None);
    }

    #[test]
    fn a_wait_that_ends_before_the_deadline_is_nobody_s_business() {
        // Rate limited until 08:00, due at 09:00: an hour to spare, and a person who was told
        // about this would rightly wonder what they were supposed to do with it.
        let overnight = run(Millis::from_mins(0), Some(Millis::from_mins(540)), 0);
        assert_eq!(
            overnight.prospect_at(Millis::from_mins(480)),
            Prospect::TimeLeft {
                spare: Millis::from_mins(60)
            }
        );
    }

    #[test]
    fn a_wait_that_ends_after_the_deadline_is_a_miss_before_it_happens() {
        // The point of asking about a future instant rather than about now: at 07:00 this run
        // is an hour inside its deadline and in no trouble at all, and the reset it is waiting
        // for is two hours the wrong side of it. Nothing is gained by waiting to find out.
        let due_at_eight = run(Millis::from_mins(0), Some(Millis::from_mins(480)), 0);
        let now = Millis::from_mins(420);
        assert!(!due_at_eight.slack(now).is_overdue());
        assert_eq!(
            due_at_eight.prospect_at(Millis::from_mins(600)),
            Prospect::Missed {
                by: Millis::from_mins(120)
            }
        );
    }

    #[test]
    fn a_wait_with_no_end_is_judged_against_now() {
        // A fleet that refused the run says nothing about when it will change its mind, so the
        // only thing that can be established is whether the deadline has already gone.
        let due_at_eight = run(Millis::from_mins(0), Some(Millis::from_mins(480)), 0);
        assert!(matches!(
            due_at_eight.prospect_at(Millis::from_mins(479)),
            Prospect::TimeLeft { .. }
        ));
        assert_eq!(
            due_at_eight.prospect_at(Millis::from_mins(481)).missed_by(),
            Some(Millis::from_mins(1))
        );
    }

    #[test]
    fn a_prospect_reads_as_a_sentence() {
        assert_eq!(
            Prospect::NoStatedDeadline.to_string(),
            "no deadline was stated for it"
        );
        assert_eq!(
            Prospect::TimeLeft {
                spare: Millis::from_mins(90)
            }
            .to_string(),
            "1h30m inside its deadline"
        );
        assert_eq!(
            Prospect::Missed {
                by: Millis::from_secs(90)
            }
            .to_string(),
            "1m30s past its deadline"
        );
    }

    #[test]
    fn slack_reads_as_a_sentence_in_both_directions() {
        assert_eq!(Slack(90_000).to_string(), "1m30s left");
        assert_eq!(Slack(-90_000).to_string(), "overdue by 1m30s");
        assert_eq!(Slack(0).to_string(), "overdue by 0ms");
    }

    #[test]
    fn there_is_no_such_thing_as_negative_time_remaining() {
        assert_eq!(Slack(1_000).remaining(), Some(Millis(1_000)));
        assert_eq!(Slack(0).remaining(), None);
        assert_eq!(Slack(-1).remaining(), None);
    }
}
