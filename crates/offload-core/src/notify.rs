//! What is worth interrupting a person for, and which route it may take (ADR-0010).
//!
//! The delivery plane's pure half. Two planes are kept apart on purpose: the execution plane
//! answers *who does the work*, and this one answers *who hears about it*. A device that cannot
//! host an agent at all is a full member of the fleet if it can carry a message.
//!
//! Three rules from the ADR are load-bearing here, and each is a variant or a signature rather
//! than a comment:
//!
//! * **A notification is derived from the event log, never emitted as a side effect.** The log
//!   is durable and sequenced, so a notification survives the node that would have sent it
//!   going away mid-turn. [`notable`] is the whole projection, and it is a pure function of one
//!   log entry — which is what makes "did this get sent" answerable from the store rather than
//!   from whatever an adapter remembers.
//! * **It is a small typed subset**, not the stream. Nobody wants a push per tool call, and
//!   deciding what matters *at the source* is what keeps every sink dumb and interchangeable.
//!   The first set of variants will be wrong; keeping it small and typed is what makes being
//!   wrong cheap.
//! * **A route is a capability** (ADR-0011), so choosing one is the same constraint-match-plus-
//!   policy decision as choosing where a run goes. There is no second scheduler here, and
//!   [`Undeliverable`] exists so a sink that will not carry something says why.
//!
//! What this module does not do: any I/O, any templating, and any deduplication. The dedup
//! identity is [`Notification::key`] and the memory of it belongs to the store.

use crate::capability::{Capability, Role, Service};
use crate::event::{LogEvent, LogKind, Waiting};
use crate::fleet_event::{Enrolment, FleetEvent, FleetLogEvent};
use crate::id::RunId;
use crate::time::Millis;
use serde::{Deserialize, Serialize};

/// One thing a person would want to be told, derived from one entry in a run's event log.
///
/// Carries the log sequence it came from, because that is half of the at-least-once identity
/// (ADR-0010: deduplicated on `(run, seq, sink)`) and because it is how a delivery is traced
/// back to the fact that caused it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    /// What this is about. Almost always a run; occasionally the fleet itself (ADR-0012).
    pub subject: Subject,
    /// The event log sequence this was projected from. Node-local, like the log itself, and
    /// only comparable within one [`Subject::topic`] — the two logs count separately.
    pub seq: u64,
    pub at: Millis,
    pub notice: Notice,
}

/// What a notification is about.
///
/// Added when membership news needed carrying and the plane could only address runs. It is an
/// enum rather than an `Option<RunId>` for the reason every other decision here is typed: the
/// dedup key, the outbox and the cursor all have to say *which log* a sequence number is in,
/// and two logs sharing an integer with nothing to distinguish them is one migration away from
/// a fleet notification suppressing a run's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "about")]
pub enum Subject {
    Run {
        run: RunId,
    },
    /// The fleet this node belongs to. Not a node: an enrolment is news about the membership,
    /// and the device it happened on is a detail inside the notice.
    Fleet,
}

impl Subject {
    #[must_use]
    pub const fn run(run: RunId) -> Subject {
        Subject::Run { run }
    }

    #[must_use]
    pub const fn as_run(&self) -> Option<RunId> {
        match self {
            Subject::Run { run } => Some(*run),
            Subject::Fleet => None,
        }
    }

    /// Which log this sequence number belongs to.
    #[must_use]
    pub const fn topic(&self) -> Topic {
        match self {
            Subject::Run { .. } => Topic::Run,
            Subject::Fleet => Topic::Fleet,
        }
    }
}

/// Which of this node's two logs a sequence number counts in.
///
/// A type rather than a `&str` because it is half of a primary key in the outbox and half of a
/// cursor's identity, and the two logs number independently: a bare integer that could mean
/// either is one careless query away from a fleet alarm marking a run's notification sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Run,
    Fleet,
}

impl Topic {
    /// The stored form, so it is an interface: it is the `topic` column.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Topic::Run => "run",
            Topic::Fleet => "fleet",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Topic> {
        match name {
            "run" => Some(Topic::Run),
            "fleet" => Some(Topic::Fleet),
            _ => None,
        }
    }
}

impl std::fmt::Display for Topic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The subset. Deliberately short.
///
/// The test for admitting a variant is whether somebody would want their evening interrupted
/// by it — not whether it is interesting. A turn boundary is interesting; a run that failed
/// three hours into the night is worth a phone buzzing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "notice")]
pub enum Notice {
    /// It worked. The one piece of good news, and it is here because "is it done" is the
    /// question that otherwise gets answered by checking all evening.
    Finished {
        turns: u32,
        cost_micro_usd: u64,
        /// Denials are on the good-news variant on purpose: a run that finished having been
        /// blocked from half of what it wanted did *not* do what was asked, and nothing else
        /// about it looks wrong (ADR-0008).
        denials: u32,
        /// Which tier finished, so [`Notice::summary`] does not describe a nominated program
        /// in an agent's units. Carried from the event; see `LogKind::Finished`'s own field.
        #[serde(default)]
        work: crate::WorkKind,
    },
    /// It stopped and will not finish on its own. Whether it waits for a person is
    /// [`crate::recovery`]'s decision and is not re-decided here.
    Failed { reason: String },
    /// It is not going to make a deadline somebody stated (ADR-0013). The only notice about
    /// something that has *not* happened, which is exactly why it exists: everything else in
    /// this list announces itself by occurring.
    Overdue { by: Millis, waiting: Waiting },
    /// The agent is blocked on a question only a person can answer (ADR-0017).
    ///
    /// The only notice that is *asked* rather than told, and the only one with a deadline of its
    /// own: the run is mid-turn while it waits, so nobody answering is itself an answer. That is
    /// why it carries the identity of the request — an answer is for one tool call — and how long
    /// there is to give one, because "approve this" with no clock is a promise this plane cannot
    /// keep.
    NeedsDecision {
        tool: String,
        detail: String,
        tool_use_id: String,
        /// How long there is to answer, from when the question was asked.
        ///
        /// The field this variant's own doc comment described for two phases without having it.
        /// A duration rather than an instant, like [`Notice::Overdue`]'s `by` and for the same
        /// reason: this crate has no clock, and the reader of a notification wants "you have five
        /// minutes", not a timestamp to subtract.
        within: Millis,
    },
    /// Nobody decided, and the run has stopped waiting (ADR-0017's third answer).
    ///
    /// The only notice that *closes* another one. Every other notice here is a fact — finished,
    /// failed, overdue — and a question is an **open item** on somebody's phone: it is the one
    /// piece of news that creates an obligation the plane then has to discharge.
    ///
    /// `Allowed` and `Denied` are still silent, which is the carve-out `notable` always had and
    /// which is right for them: whoever acted knows. It was wrong for this one, on the stated
    /// grounds that "what a run did without permission is what its result reports" — a claim about
    /// the result being *delivered*, and `Notices::Problems` throws the result away. That is a
    /// rule's default, and a rule is exactly the run that asks with nobody there.
    Undecided {
        tool: String,
        /// Why nobody decided, in the words the log recorded: the wait ran out, or the agent
        /// stopped waiting. Two different facts and the person can act on the difference.
        why: String,
    },
    /// A device is a member of this fleet (ADR-0012 mitigation 4).
    ///
    /// The compensating control for there being no per-join approval, which makes it the one
    /// notice here that is about *security* rather than about work. It has to arrive while
    /// revoking is still ahead of an attacker rather than behind — the fifteen minutes
    /// probation buys are the fifteen minutes this is supposed to fill.
    Enrolled {
        node: String,
        name: String,
        grants: String,
        /// Whether this node watched it happen or merely met the result. Different sentences,
        /// because they have different things to do about them.
        first_contact: bool,
    },
    /// The fleet passphrase was used somewhere (ADR-0012).
    ///
    /// Worth interrupting somebody for precisely because it should be rare: once an approver
    /// exists, the root secret is break-glass, and the whole value of a break-glass credential
    /// is that every use is signal. A fleet where this is noise is a fleet with no approver.
    PassphraseUsed { what: String, name: String },
}

impl Notification {
    /// The at-least-once identity: `(run, seq, sink)`, with the sink supplied by the caller.
    ///
    /// A tuple rather than a hash, because the point is that it is *reconstructible*: a node
    /// that restarts mid-delivery recomputes the same key from the same log entry, which is
    /// what makes "have I already sent this" a question the store can answer instead of a
    /// guess. Duplicates are a nuisance and a silence is the failure this plane exists to
    /// prevent, so the asymmetry decides the trade.
    #[must_use]
    pub fn key(&self, sink: &str) -> (Subject, u64, String) {
        (self.subject, self.seq, sink.to_string())
    }

    /// A short, stable name for the kind of news this is.
    ///
    /// Written out rather than derived from `Debug`, for `LogEvent::kind_name`'s reason and one
    /// more: this string is handed to somebody's notification script, so it is an interface. A
    /// variant rename must not silently change what a `case` statement in a shell script sees.
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match self.notice {
            Notice::Finished { .. } => "finished",
            Notice::Failed { .. } => "failed",
            Notice::Overdue { .. } => "overdue",
            Notice::NeedsDecision { .. } => "asked",
            Notice::Undecided { .. } => "answered",
            Notice::Enrolled { .. } => "enrolled",
            Notice::PassphraseUsed { .. } => "passphrase",
        }
    }

    /// A few words, for whatever a sink puts in a title bar.
    #[must_use]
    pub fn title(&self) -> String {
        let what = match &self.notice {
            Notice::Finished { .. } => "finished",
            Notice::Failed { .. } => "failed",
            Notice::Overdue { .. } => "will miss its deadline",
            Notice::NeedsDecision { .. } => "needs a decision",
            Notice::Undecided { .. } => "went ahead undecided",
            // Not "run …": these are about the fleet, and putting a run id nobody can look up
            // in front of a security alarm is the fastest way to have it skimmed past.
            Notice::Enrolled {
                first_contact: false,
                ..
            } => return "a device joined your fleet".to_string(),
            Notice::Enrolled {
                first_contact: true,
                ..
            } => return "a device you have not seen before is in your fleet".to_string(),
            Notice::PassphraseUsed { .. } => return "the fleet passphrase was used".to_string(),
        };
        // Twelve characters of a run id is a clock rather than an identity (they collide
        // within a millisecond), so this names a run for a person reading one line — never for
        // anything that then looks it up.
        match self.subject.as_run() {
            Some(run) => format!("run {} {what}", run.short()),
            None => what.to_string(),
        }
    }

    /// One line a person can act on without opening anything.
    #[must_use]
    pub fn summary(&self) -> String {
        match &self.notice {
            // A task's whole result is that it exited zero — no turns, no bill, and nothing it
            // could have been denied (ADR-0019 §2). First, and it says the one thing true of
            // both tiers: this read `finished after 0 turn(s), $0.0000` on a phone, about a
            // shell script.
            Notice::Finished {
                work: crate::WorkKind::Task,
                ..
            } => "finished".to_string(),
            Notice::Finished {
                turns,
                cost_micro_usd,
                denials: 0,
                work: crate::WorkKind::Agent,
            } => format!(
                "finished after {turns} turn(s), ${:.4}",
                *cost_micro_usd as f64 / 1_000_000.0
            ),
            Notice::Finished {
                turns,
                cost_micro_usd,
                denials,
                work: crate::WorkKind::Agent,
            } => format!(
                "finished after {turns} turn(s), ${:.4} — but {denials} permission request(s) \
                 were denied, so it did less than it was asked",
                *cost_micro_usd as f64 / 1_000_000.0
            ),
            Notice::Failed { reason } => format!("failed: {reason}"),
            Notice::Overdue { by, waiting } => {
                format!("will not make its deadline — {by} past it, and {waiting}")
            }
            Notice::NeedsDecision {
                tool,
                detail,
                within,
                ..
            } => {
                format!("wants to use {tool}: {detail} — answer within {within}")
            }
            Notice::Undecided { tool, why } => {
                format!("{why}, so {tool} was left to the agent's own rules")
            }
            Notice::Enrolled {
                node,
                name,
                grants,
                first_contact,
            } => format!(
                "{name} {}, granted {grants}. If that was not you: offload revoke {node}",
                if *first_contact {
                    "has appeared in your fleet"
                } else {
                    "was enrolled"
                }
            ),
            Notice::PassphraseUsed { what, name } => format!(
                "the fleet passphrase was used on {name} to {what} — it should be rare enough \
                 that you remember doing it"
            ),
        }
    }
}

/// The whole projection from the event log: is this entry worth telling somebody about?
///
/// `None` for almost everything, which is the point. Note what is deliberately absent:
///
/// * **`Checkpointed { released: true }`**, which ends a holder's stream and looks terminal.
///   It is a run being handed on mid-conversation — the machinery working, not news.
/// * **`Cancelled`**, because a person cancelled it. Telling them what they just did is the
///   fastest way to teach somebody to ignore notifications.
/// * **`RateLimit`**, which the agent emits per turn. The one case worth raising is a limit
///   that outlasts the deadline, and that already arrives as [`LogKind::Overdue`].
/// * **`Answered { Allowed | Denied }`**, for `Cancelled`'s reason: whoever acted knows.
///
/// And what is deliberately *no longer* absent, because the reasoning above does not stretch to
/// it: **`Answered { Unanswered }`**. This list used to exclude the whole kind, on the grounds
/// that "what a run did without permission is what its result reports" — which is a claim about
/// the result being **delivered**, and [`Notices::Problems`] discards exactly the result. That is
/// a *rule's* default (ADR-0026 §3), and a rule is precisely the run that asks with nobody there:
/// measured on a rule with a working push route, the phone got `needs a decision` and then
/// nothing, for ever. A question is the one notice that is an **open item** rather than a fact,
/// and the plane that opened it is the only thing that can close it.
#[must_use]
pub fn notable(run: RunId, seq: u64, event: &LogEvent) -> Option<Notification> {
    let notice = match &event.kind {
        LogKind::Finished {
            success: true,
            turns,
            denials,
            cost_micro_usd,
            work,
            ..
        } => Notice::Finished {
            turns: *turns,
            cost_micro_usd: *cost_micro_usd,
            denials: *denials,
            work: *work,
        },
        // An unsuccessful `Finished` and a `Failed` are the same news arriving by two routes:
        // the agent reporting a failure, and something else failing the run. A sink does not
        // care which, and a person cares even less.
        LogKind::Finished { success: false, .. } => Notice::Failed {
            reason: "the agent reported failure".to_string(),
        },
        LogKind::Failed { reason } => Notice::Failed {
            reason: reason.clone(),
        },
        LogKind::Overdue { by_ms, waiting } => Notice::Overdue {
            by: Millis(*by_ms),
            waiting: waiting.clone(),
        },
        LogKind::Asked {
            tool,
            detail,
            tool_use_id,
            within_ms,
        } => Notice::NeedsDecision {
            tool: tool.clone(),
            detail: detail.clone(),
            tool_use_id: tool_use_id.clone(),
            within: Millis(*within_ms),
        },
        // The third answer only. `Allowed` and `Denied` stay silent because whoever acted knows;
        // nobody acted on this one, and the person who was interrupted is owed the end of it.
        LogKind::Answered {
            answer: crate::Answer::Unanswered,
            by,
            tool,
            ..
        } => Notice::Undecided {
            tool: tool.clone(),
            why: by.clone(),
        },
        _ => return None,
    };
    Some(Notification {
        subject: Subject::run(run),
        seq,
        at: Millis(event.at_unix_ms),
        notice,
    })
}

/// The same projection for the fleet's own log (ADR-0012 mitigation 4).
///
/// A second function rather than a second arm, because the two logs are two logs: they have
/// separate sequences, separate cursors, and a filter that scanned one for the other's kinds
/// would find nothing and say nothing. What they share is everything downstream of here.
///
/// Every fleet event is notable — which is the difference between this log and a run's. A run
/// emits a hundred entries and four of them are news; this log is *only* written when something
/// has happened that the owner is supposed to check.
#[must_use]
pub fn notable_fleet(seq: u64, event: &FleetLogEvent) -> Option<Notification> {
    let notice = match &event.kind {
        FleetEvent::Enrolled {
            node,
            name,
            grants,
            how,
        } => Notice::Enrolled {
            node: node.to_string(),
            name: name.clone(),
            grants: grants
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            first_contact: *how == Enrolment::Met,
        },
        FleetEvent::PassphraseUsed { what, name, .. } => Notice::PassphraseUsed {
            what: what.clone(),
            name: name.clone(),
        },
    };
    Some(Notification {
        subject: Subject::Fleet,
        seq,
        at: Millis(event.at_unix_ms),
        notice,
    })
}

/// The fleet log kinds [`notable_fleet`] can project. Every one of them, today — kept as a list
/// anyway so the scan and the projection have the same shape as the run log's, and so that a
/// kind added for the record rather than for the alarm has somewhere to not be.
pub const NOTABLE_FLEET_KINDS: &[&str] = &["enrolled", "passphrase_used"];

/// The event log kinds [`notable`] can project, for a store that wants to scan for them.
///
/// Here rather than in the store because it must not drift from `notable`: a query that
/// filters on a list this module does not own is a query that silently stops finding a kind
/// somebody added. The names are [`LogEvent::kind_name`]'s, which is the column's contents.
pub const NOTABLE_KINDS: &[&str] = &["finished", "failed", "overdue", "asked", "answered"];

/// The notices a **rule** may be bound to (ADR-0057 §1, §7).
///
/// [`Notification::kind_name`]'s strings, and the same set as [`NOTABLE_KINDS`] today because
/// every notice about a run projects from a log kind of the same name. Named separately anyway,
/// because the two are answers to different questions and only one of them is a *scan bound*:
/// this is the list a person may type, and the day a notice projects from two kinds — as
/// `Notice::Failed` already does, from `finished` and from `failed` — the two stop being equal.
///
/// **Fleet notices are deliberately absent.** `enrolled` and `passphrase_used` are about
/// membership rather than about a run, and firing work at them is a security response with a
/// harder failure mode than a missed notification (ADR-0057 §7). A rule naming one is refused
/// with the reason rather than written and never fired.
pub const RULE_NOTICE_KINDS: &[&str] = &["finished", "failed", "overdue", "asked", "answered"];

/// The one notice that is good news.
///
/// A predicate over the projected [`Notice`] rather than a name, because a *name* was the bug:
/// [`Notices::admits`] used to be asked about the event log's `kind` column, and
/// `LogKind::Finished { success: false }` — an agent reporting its own failure, which is the
/// commonest failure there is — carries the kind name `finished` and projects to
/// [`Notice::Failed`]. So `Problems`, whose entire promise is that a failure gets through,
/// silently discarded every one of them. Measured on a rule firing every three seconds with an
/// agent that fails every time: **22 firings, 35 `finished`-kind events, 0 notifications**, on a
/// node whose `offload when` had printed "you will hear about a failure".
///
/// The two namespaces overlap in three of four kinds, which is what made the mistake invisible:
/// only `finished` means different things in the two, and it is the one this filter turns on.
#[must_use]
pub const fn is_good_news(notice: &Notice) -> bool {
    matches!(notice, Notice::Finished { .. })
}

/// Does this notice go to a route on `route_node`, for a run submitted on `home` (ADR-0073)?
///
/// **Good news goes to the device the run was submitted from; problems go everywhere.** Once every
/// phone and tablet with the app was a route, `Audience::Everyone` meant that every run anybody
/// started buzzed every device when it finished. "It finished" matters to whoever asked for it.
/// "It failed" and "it needs a decision" need somebody, wherever they are. The owner chose this.
///
/// `route_node` is `None` when the pass cannot say which node a route is on. That is told: a
/// duplicate is a nuisance, and a silence is the failure this plane exists to prevent. A route
/// to one of this node's own rules is not a route to a person, and the caller does not ask.
#[must_use]
pub fn reaches(notice: &Notice, route_node: Option<crate::NodeId>, home: crate::NodeId) -> bool {
    !is_good_news(notice) || route_node.is_none_or(|node| node == home)
}

/// Which of a run's notices are worth interrupting somebody for.
///
/// **The axis [`Audience`] is not.** An audience selects *routes* — reach me by push, not by mail
/// — and it is asked per capability for that reason. This selects *kinds*, and the two were
/// conflated by having only one of them: a run could say who to tell and never what was worth
/// telling, so the only way to stop being told was `Audience::Nobody`, which stops the failures
/// too.
///
/// Measured on a rule firing every three seconds, which is the case that has no right answer
/// without this: the default delivered **11 notifications in 40 seconds**, every one of them
/// "finished after 2 turn(s), $0.0005"; `--notify nobody` on the same rule failing every firing
/// delivered **0 of 11 failures**. A watcher's whole value is that it is quiet until something
/// happens, and neither setting could say so.
///
/// Deliberately **two** values and not a set of kinds. The question a person actually has is
/// "tell me when it needs me", and a checklist of five notice kinds is a configuration surface
/// for a decision nobody wants to make twice. The line falls in one place — a run finishing is
/// the only notice here that is not a request for attention.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notices {
    /// Everything the projection produces. Right for a run somebody submitted and walked away
    /// from: "is it done" is the question the delivery plane exists to answer without them
    /// checking all evening.
    #[default]
    Everything,
    /// Everything except the good news.
    ///
    /// What a standing instruction wants, and why it is not `Audience::Nobody`: a rule that fires
    /// all night should say nothing while it works and reach somebody the moment it does not.
    /// `overdue` and `asked` are problems too — an unanswered question blocks the run mid-turn
    /// until its patience runs out, which is the last thing to be quiet about.
    Problems,
}

impl Notices {
    /// Is this news worth interrupting somebody for?
    ///
    /// Takes the projected [`Notice`] and not a kind *name*. It used to take the name, on the
    /// stated grounds that the scan filling the outbox works from the event log's denormalised
    /// `kind` column and never decodes the payload — and that was the hole: `success` is *in* the
    /// payload, so the column cannot tell an agent reporting failure from a run that went fine.
    /// Both are `LogKind::Finished`, and one of them is the news a watcher exists for. See
    /// [`is_good_news`] for the measurement.
    ///
    /// The column still bounds the scan ([`NOTABLE_KINDS`]), which is all it was ever good for:
    /// it is a cheap superset, and the decision is taken on the same value the sender will
    /// re-derive, so the filter and the message cannot disagree about one event.
    #[must_use]
    pub fn admits(&self, notice: &Notice) -> bool {
        match self {
            Notices::Everything => true,
            Notices::Problems => !is_good_news(notice),
        }
    }
}

impl std::fmt::Display for Notices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Notices::Everything => "everything",
            Notices::Problems => "problems",
        })
    }
}

impl std::str::FromStr for Notices {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "everything" | "all" => Ok(Notices::Everything),
            "problems" | "problem" => Ok(Notices::Problems),
            other => Err(format!(
                "unknown notice selection `{other}` — expected `everything` or `problems`"
            )),
        }
    }
}

/// Which routes a run's news is for (ADR-0010's audience).
///
/// The gap this closes: every authenticated route in the fleet carried every notification, so a
/// fleet with three toast sinks gave three toasts. That is a configuration somebody chose, and it
/// is still not the same as a run saying *reach me by push*.
///
/// **Not a [`crate::Constraint`], and the difference is the whole reason this type exists.** A
/// constraint chooses a *node*, and a node holding both a push route and a mailbox satisfies
/// `HasService { push }` — after which the obvious implementation tells that person twice, once
/// by each route. An audience chooses *routes*, so it is asked about one capability at a time.
///
/// **Named by service, never by route id.** A route id is a node's own name for one of its sinks
/// (`toast`, on the phone), so naming one would pin a run's news to a device — the exact thing
/// the plane exists to stop mattering. "Push" is a promise about reaching somebody; "the phone's
/// `toast` script" is a promise about a machine being awake.
///
/// **It lives on the spec** for `crate::RunSpec::queue`'s reason: the node that delivers the news
/// is often not the node that took the request, and an audience only the submitting process
/// remembers is an audience a migrated run has lost. It is deliberately *not* editable — see
/// [`crate::SpecEdit`], which is two fields and one counter on purpose.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "audience")]
pub enum Audience {
    /// Every route that works, wherever it is. The default, and the right answer for a fleet
    /// whose devices all belong to one person: anything else would have somebody describe their
    /// routes twice, once on the device that holds one and once on every run.
    #[default]
    Everyone,
    /// Only routes offering this service.
    ///
    /// A **struct** variant rather than `Service(Service)`, for the reason written out on
    /// [`crate::SpecEdit`]: an internally-tagged enum wrapping another tagged one is a runtime
    /// encoding question rather than a compile-time one, and this type crosses the wire.
    Service { service: Service },
    /// Nobody. Somebody watching a run in a terminal does not need their phone to buzz about it,
    /// and the honest way to say so is per run rather than by turning a route off for everything
    /// else as well.
    ///
    /// It decides who is *interrupted*, never what is recorded: the event is in the log either
    /// way, which is what keeps this from being a way to lose news.
    Nobody,
}

impl Audience {
    /// May a route offering this service carry the run's news?
    ///
    /// The only question this type answers, and it is asked per route rather than per node.
    /// Whether that route *works* is [`deliverability`]'s separate answer — a run asking for push
    /// and a phone whose credential has expired are two different silences with two different
    /// fixes.
    ///
    /// `None` is a route that never said what it is. It carries what nobody routed and cannot be
    /// asked for by name, which is the auth flag's rule in another place: what a node cannot
    /// establish, it does not claim.
    #[must_use]
    pub fn admits(&self, service: Option<&Service>) -> bool {
        match (self, service) {
            (Audience::Everyone, _) => true,
            (Audience::Nobody, _) => false,
            (Audience::Service { .. }, None) => false,
            (Audience::Service { service: wanted }, Some(service)) => wanted == service,
        }
    }

    /// Is this the default — tell whoever can be told?
    ///
    /// **Not what keeps the submission quiet about the ordinary case**, which this said and which
    /// was untrue: that decision is `deliver::audience_note`, which matches the variant and
    /// returns `None` for `Everyone`. The reasoning was right — a sentence printed on every
    /// submission is one nobody reads on the submission where it matters — and it belongs where
    /// the decision is. Kept because it reads well in an assertion, which is its only caller.
    #[must_use]
    pub fn is_everyone(&self) -> bool {
        matches!(self, Audience::Everyone)
    }
}

impl std::fmt::Display for Audience {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Audience::Everyone => f.write_str("everyone"),
            Audience::Service { service } => write!(f, "{service}"),
            Audience::Nobody => f.write_str("nobody"),
        }
    }
}

/// The inverse of the `Display`, because [`Service`]'s exists for the same reason: a type with
/// one direction gets a second hand-written parser somewhere else eventually, and then two
/// spellings of one audience.
///
/// `all` and `none` are accepted beside `everyone` and `nobody` because they are what somebody
/// types. An unrecognised name becomes [`Service::Other`] rather than an error — the escape hatch
/// [`Service`] already has — so the guard against a typo is not the parser: it is being told at
/// submission time that no route in this fleet offers what was asked for.
impl std::str::FromStr for Audience {
    type Err = crate::capability::UnknownService;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_ascii_lowercase().as_str() {
            "everyone" | "all" | "any" => Ok(Audience::Everyone),
            "nobody" | "none" | "off" => Ok(Audience::Nobody),
            other => other.parse().map(|service| Audience::Service { service }),
        }
    }
}

/// Why a sink will not carry a notification.
///
/// Separate from "nothing to deliver": a fleet with a push route that has lost its credential
/// is in a different situation from one with no route at all, and the two have different fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "undeliverable")]
pub enum Undeliverable {
    /// The capability exists but is not a route to a human. Same service, different role
    /// (ADR-0011) — an inbox a run *reads* is not an inbox the fleet may write to.
    NotASink,
    /// The device that offers it says it cannot use it. The rule the agent probe already
    /// follows, and the reason it matters more here: a node that claims a route it does not
    /// have wins the routing decision and then drops the message, and silence is the one
    /// failure this plane cannot detect on its own.
    ///
    /// **What it must not say is *why*.** `Capability::authenticated` is one bit with a
    /// different meaning per role — a credential for a sink, a program on disk for an
    /// `Execute` — and even within `Role::Sink` the owner's script may be missing, unreadable
    /// or refusing. `offload sinks` printed *"alpha says its credential does not work"* about a
    /// `[[sinks]]` entry naming a program that is not there, while alpha's own table one command
    /// away said *"not found on this device"*. Core holds the bit and not the machine; the
    /// measured reason travels in `Capability::description` (ADR-0019's amendment made the
    /// nominated kinds put it there) and is the fleet listing's to print.
    Unauthenticated,
}

impl std::fmt::Display for Undeliverable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Undeliverable::NotASink => f.write_str("it is not a route to a human"),
            // Worded as a **predicate of the offering device**, because that is how it is read:
            // the delivery pass wraps it as `{node} says {this}; waiting for it`, and the first
            // cut of this fix named the device inside it too — *"alpha says the device offering
            // it says it cannot use it"*. A `Display` that will be embedded has to be written
            // for the sentence it lands in.
            Undeliverable::Unauthenticated => {
                f.write_str("it cannot use that route, so it would drop the message")
            }
        }
    }
}

/// Whether this capability may carry notifications at all.
///
/// Not "should this one go here" — that is a `Constraint` match against the notification's
/// audience, and there is nothing to match on until a notification can name one. This is the
/// gate that has to hold whatever routing is built on top: a sink that cannot deliver must not
/// be chosen, and the reason has to survive to the operator.
pub fn deliverability(cap: &Capability) -> Result<(), Undeliverable> {
    if !cap.roles.contains(&Role::Sink) {
        return Err(Undeliverable::NotASink);
    }
    if !cap.authenticated {
        return Err(Undeliverable::Unauthenticated);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// A rule may name every notice about a **run** and no notice about the fleet (ADR-0057 §7).
    ///
    /// A `const` list is a second copy of the enum, so it needs something to hold it in step:
    /// this walks one of every variant and checks that its own `kind_name` is on the right side
    /// of the line. A variant added without a decision about it fails here rather than becoming
    /// a name nobody can bind to, or — worse — one that fires work at a membership alarm.
    ///
    /// Deliberately **not** compared against `NOTABLE_FLEET_KINDS`: that list holds *fleet event*
    /// kinds, which is a different namespace from a notice's own name and overlaps it by
    /// coincidence (`passphrase_used` the event projects to `passphrase` the notice). Asserting
    /// across the two would be the same conflation `Notices::admits` was fixed for.
    #[test]
    fn every_notice_is_classified_and_only_a_runs_may_fire_a_rule() {
        use super::*;
        let one_of_each = [
            Notice::Finished {
                turns: 1,
                cost_micro_usd: 0,
                denials: 0,
                work: crate::WorkKind::Agent,
            },
            Notice::Failed { reason: "x".into() },
            Notice::Overdue {
                by: crate::Millis(1),
                waiting: Waiting::Placement { refused_by: 1 },
            },
            Notice::NeedsDecision {
                tool: "Bash".into(),
                detail: "cargo test".into(),
                tool_use_id: "t".into(),
                within: crate::Millis(1),
            },
            Notice::Undecided {
                tool: "Bash".into(),
                why: "nobody answered".into(),
            },
            Notice::Enrolled {
                node: "abc".into(),
                name: "laptop".into(),
                grants: "submit".into(),
                first_contact: false,
            },
            Notice::PassphraseUsed {
                what: "an invitation".into(),
                name: "laptop".into(),
            },
        ];

        for notice in one_of_each {
            let about_the_fleet = matches!(
                notice,
                Notice::Enrolled { .. } | Notice::PassphraseUsed { .. }
            );
            let note = Notification {
                subject: Subject::Run {
                    run: crate::RunId::from_bytes([1; 16]),
                },
                seq: 1,
                at: crate::Millis(0),
                notice,
            };
            let name = note.kind_name();
            assert_eq!(
                RULE_NOTICE_KINDS.contains(&name),
                !about_the_fleet,
                "`{name}` is on the wrong side of the line a rule may bind to"
            );
        }
    }

    use super::*;
    use crate::capability::Service;

    fn run() -> RunId {
        RunId::from_bytes([7; 16])
    }

    fn event(kind: LogKind) -> LogEvent {
        LogEvent::at(Millis(1_700_000_000_000), kind)
    }

    #[test]
    fn the_subset_is_the_point() {
        // Everything a run says all day, and the three things worth a phone buzzing.
        let noise = [
            LogKind::Submitted {
                repo: "/r".into(),
                prompt: "p".into(),
                permission: "acceptEdits".into(),
            },
            LogKind::Text {
                text: "hello".into(),
            },
            LogKind::ToolUse {
                name: "Bash".into(),
            },
            LogKind::TurnBoundary { turn: 3 },
            LogKind::RateLimit {
                kind: "five_hour".into(),
                status: "rejected".into(),
                resets_at_unix_ms: None,
            },
            // Terminal for a follower and *not* news: the run is being handed on.
            LogKind::Checkpointed {
                turn: 3,
                summary: "s".into(),
                released: true,
            },
            // They did it themselves.
            LogKind::Cancelled { by: "phone".into() },
            // And this one they *answered* themselves, so telling them is teaching them to
            // ignore the next one. The question is the news; the answer is not.
            LogKind::Answered {
                tool_use_id: "toolu_1".into(),
                answer: crate::event::Answer::Allowed,
                by: "owner".into(),
                tool: "Bash".into(),
            },
        ];
        for kind in noise {
            assert_eq!(notable(run(), 1, &event(kind.clone())), None, "{kind:?}");
        }
    }

    #[test]
    fn success_failure_and_a_missed_deadline_all_get_through() {
        let finished = notable(
            run(),
            9,
            &event(LogKind::Finished {
                success: true,
                turns: 12,
                denials: 0,
                cost_micro_usd: 321_000,
                work: crate::WorkKind::Agent,
                result: None,
            }),
        )
        .expect("news");
        assert_eq!(finished.seq, 9);
        assert_eq!(finished.summary(), "finished after 12 turn(s), $0.3210");

        // A task's is the same news in the units a task has, which is none of them. The three
        // numbers are an agent's, and this line goes to somebody's phone: it read `finished
        // after 0 turn(s), $0.0000` about a shell script, which is what a tier-blind sentence
        // says the moment a second tier exists (ADR-0019 §2).
        let task = notable(
            run(),
            9,
            &event(LogKind::Finished {
                success: true,
                turns: 0,
                denials: 0,
                cost_micro_usd: 0,
                work: crate::WorkKind::Task,
                result: None,
            }),
        )
        .expect("news");
        assert_eq!(task.summary(), "finished");
        assert_eq!(
            task.kind_name(),
            "finished",
            "the same kind, so the audience and dedup rules are unchanged"
        );

        // …and a row written before the field existed decodes as the agent it was written by.
        let old: LogKind = serde_json::from_value(serde_json::json!({
            "event": "finished",
            "success": true,
            "turns": 3,
            "denials": 0,
            "cost_micro_usd": 100,
        }))
        .expect("a pre-tier row still decodes");
        assert!(matches!(
            old,
            LogKind::Finished {
                work: crate::WorkKind::Agent,
                ..
            }
        ));

        // An unsuccessful result and a failure are the same news by two routes.
        let unsuccessful = notable(
            run(),
            10,
            &event(LogKind::Finished {
                success: false,
                turns: 1,
                denials: 0,
                cost_micro_usd: 0,
                work: crate::WorkKind::Agent,
                result: None,
            }),
        )
        .expect("news");
        assert!(matches!(unsuccessful.notice, Notice::Failed { .. }));

        let overdue = notable(
            run(),
            11,
            &event(LogKind::Overdue {
                by_ms: 3_600_000,
                waiting: Waiting::Placement { refused_by: 2 },
            }),
        )
        .expect("news");
        assert_eq!(
            overdue.summary(),
            "will not make its deadline — 1h0m past it, and all 2 nodes that answered refused it"
        );
    }

    #[test]
    fn a_question_is_news_and_says_what_is_waiting_on_the_answer() {
        // The only notice that is asked rather than told (ADR-0017). It has to name the tool and
        // what it wants, because "your run needs a decision" is not something anybody can answer
        // from a lock screen.
        let asked = notable(
            run(),
            12,
            &event(LogKind::Asked {
                tool: "Bash".into(),
                detail: "cargo build --release".into(),
                within_ms: 300_000,
                tool_use_id: "toolu_01ABC".into(),
            }),
        )
        .expect("news");
        assert_eq!(asked.kind_name(), "asked");
        assert!(
            asked.title().ends_with("needs a decision"),
            "{}",
            asked.title()
        );
        // With the clock on it. This variant's doc comment claimed for two phases that it carries
        // "how long there is to give one, because `approve this` with no clock is a promise this
        // plane cannot keep", and the field did not exist — so the summary said "it is waiting for
        // an answer", which is a promise with no clock. `Notice::Overdue` one arm above renders a
        // duration and always has.
        assert_eq!(
            asked.summary(),
            "wants to use Bash: cargo build --release — answer within 5m0s"
        );
        // And the identity of the request travels with it, because an answer is for one tool
        // call and a route that dropped it could not tell two questions apart.
        assert!(matches!(
            &asked.notice,
            Notice::NeedsDecision { tool_use_id, .. } if tool_use_id == "toolu_01ABC"
        ));
    }

    /// A question is the one notice that is an open item, so something has to close it.
    ///
    /// `notable` excluded the whole `Answered` kind, on the stated grounds that "what a run did
    /// without permission is what its result reports" — a claim about the result being
    /// *delivered*. `Notices::Problems` discards the result, and that is a **rule's** default,
    /// which makes a rule the run that asks unattended and is never answered for. Measured on a
    /// rule with a working push route: two firings, two `asked` notifications, and nothing else
    /// ever.
    #[test]
    fn nobody_deciding_is_the_one_answer_worth_telling_somebody_about() {
        let ran_out = notable(
            run(),
            13,
            &event(LogKind::Answered {
                tool_use_id: "toolu_01ABC".into(),
                answer: crate::event::Answer::Unanswered,
                by: "nobody answered within 5m0s".into(),
                tool: "Bash".into(),
            }),
        )
        .expect("the end of a question is news");
        assert_eq!(
            ran_out.summary(),
            "nobody answered within 5m0s, so Bash was left to the agent's own rules"
        );
        // And a watcher hears it: `Problems` discards only the good news, so any notice that is
        // not `Finished` gets through without this having to be enumerated anywhere.
        assert!(Notices::Problems.admits(&ran_out.notice));

        // The two a person *gave* stay silent — the carve-out that was always right, for the
        // reason that was always right: whoever acted knows.
        for answer in [crate::event::Answer::Allowed, crate::event::Answer::Denied] {
            assert_eq!(
                notable(
                    run(),
                    14,
                    &event(LogKind::Answered {
                        tool_use_id: "toolu_01ABC".into(),
                        answer,
                        by: "owner".into(),
                        tool: "Bash".into(),
                    })
                ),
                None,
                "{answer:?}"
            );
        }

        // The scan's kind filter has to admit it, or the projection is never reached. `answered`
        // names three outcomes of which one is news — the same shape as `finished` naming two,
        // which is the bug that made this filter a list of names bounding a scan rather than a
        // decision (ADR-0026).
        assert!(NOTABLE_KINDS.contains(&"answered"));
    }

    #[test]
    fn a_finished_run_that_was_blocked_says_so() {
        // Denials belong on the good-news variant precisely because nothing else about the run
        // looks wrong: it completed, and it did less than it was asked (ADR-0008).
        let note = notable(
            run(),
            1,
            &event(LogKind::Finished {
                success: true,
                turns: 4,
                denials: 3,
                cost_micro_usd: 0,
                work: crate::WorkKind::Agent,
                result: None,
            }),
        )
        .expect("news");
        assert!(note
            .summary()
            .contains("3 permission request(s) were denied"));
    }

    #[test]
    fn every_notable_kind_is_in_the_list_the_store_scans_for() {
        // The list and the projection have to agree, and nothing but a test makes them: a
        // query filtering on a stale list silently stops finding the kind somebody added.
        for kind in [
            LogKind::Finished {
                success: true,
                turns: 1,
                denials: 0,
                cost_micro_usd: 0,
                work: crate::WorkKind::Agent,
                result: None,
            },
            LogKind::Failed { reason: "r".into() },
            LogKind::Overdue {
                by_ms: 1,
                waiting: Waiting::Placement { refused_by: 0 },
            },
            LogKind::Asked {
                tool: "Bash".into(),
                detail: "cargo build --release".into(),
                within_ms: 300_000,
                tool_use_id: "toolu_1".into(),
            },
        ] {
            let event = event(kind);
            assert!(notable(run(), 1, &event).is_some());
            assert!(
                NOTABLE_KINDS.contains(&event.kind_name()),
                "{} projects but is not scanned for",
                event.kind_name()
            );
        }
    }

    /// A watcher is quiet about exactly one thing, asked of every log kind that projects.
    ///
    /// Driven from `LogKind` through [`notable`] rather than from a list of names, because a
    /// list of names is what was wrong: `Problems` was asked about the event log's `kind`
    /// column, where an agent reporting failure and a run going fine are both `finished`. The
    /// row that matters is `Finished { success: false }` — the commonest failure there is, and
    /// the one the old test's list did not contain.
    #[test]
    fn a_watcher_is_quiet_about_exactly_one_projected_notice() {
        let quiet_about = |kind: LogKind| {
            let event = event(kind);
            let note = notable(run(), 1, &event).expect("this kind projects");
            assert!(
                Notices::Everything.admits(&note.notice),
                "{}: `everything` admits everything",
                note.kind_name()
            );
            (!Notices::Problems.admits(&note.notice), note.kind_name())
        };

        assert_eq!(
            quiet_about(LogKind::Finished {
                success: true,
                turns: 2,
                denials: 0,
                cost_micro_usd: 500,
                work: crate::WorkKind::Agent,
                result: None,
            }),
            (true, "finished"),
        );
        // The same variant, the same `kind` column, the opposite answer.
        assert_eq!(
            quiet_about(LogKind::Finished {
                success: false,
                turns: 2,
                denials: 0,
                cost_micro_usd: 500,
                work: crate::WorkKind::Agent,
                result: None,
            }),
            (false, "failed"),
        );
        assert_eq!(
            quiet_about(LogKind::Failed { reason: "r".into() }),
            (false, "failed"),
        );
        assert_eq!(
            quiet_about(LogKind::Overdue {
                by_ms: 1,
                waiting: Waiting::Placement { refused_by: 0 },
            }),
            (false, "overdue"),
        );
        assert_eq!(
            quiet_about(LogKind::Asked {
                tool: "Bash".into(),
                detail: "rm -rf /".into(),
                within_ms: 300_000,
                tool_use_id: "toolu_1".into(),
            }),
            (false, "asked"),
        );
    }

    #[test]
    fn a_notice_selection_round_trips_through_what_somebody_types() {
        // The `Display`/`FromStr` pair exists for `Service`'s reason: a type with one direction
        // grows a hand-written parser elsewhere, and then two spellings of one setting.
        for value in [Notices::Everything, Notices::Problems] {
            assert_eq!(value.to_string().parse::<Notices>(), Ok(value));
        }
        assert_eq!("all".parse::<Notices>(), Ok(Notices::Everything));
        assert!("sometimes".parse::<Notices>().is_err());
    }

    #[test]
    fn the_dedup_key_is_reconstructible_rather_than_remembered() {
        // The whole reason it is a tuple of durable facts: a node that died mid-delivery
        // recomputes it from the same log entry and asks the store, instead of guessing.
        let event = event(LogKind::Failed { reason: "x".into() });
        let first = notable(run(), 42, &event).expect("news");
        let again = notable(run(), 42, &event).expect("news");
        assert_eq!(first.key("phone"), again.key("phone"));
        assert_ne!(first.key("phone"), first.key("laptop"));
    }

    #[test]
    fn an_audience_picks_routes_rather_than_nodes() {
        // The distinction the type exists for: a phone holding a push route *and* a mailbox
        // would satisfy a node-level `HasService { push }` and then be told twice.
        let push = Audience::Service {
            service: Service::Push,
        };
        assert!(push.admits(Some(&Service::Push)));
        assert!(!push.admits(Some(&Service::Email)));

        // The default reaches everything, including a route whose owner never said what it is.
        assert!(Audience::Everyone.admits(Some(&Service::Email)));
        assert!(Audience::Everyone.admits(None));

        // A route that cannot say what it is cannot be asked for by name — the auth flag's rule
        // in another place: what a node cannot establish, it does not claim.
        assert!(!push.admits(None));

        // And a run somebody is watching interrupts nobody, however good the route.
        assert!(!Audience::Nobody.admits(Some(&Service::Push)));
        assert!(!Audience::Nobody.admits(None));
    }

    #[test]
    fn an_audience_survives_the_wire_and_the_terminal() {
        // A tagged enum wrapping a tagged enum, which is the shape that has failed at runtime
        // in this codebase before — hence the struct variant, and hence this test.
        for audience in [
            Audience::Everyone,
            Audience::Nobody,
            Audience::Service {
                service: Service::Push,
            },
            Audience::Service {
                service: Service::Chat("team".into()),
            },
        ] {
            let json = serde_json::to_string(&audience).expect("encode");
            assert_eq!(
                serde_json::from_str::<Audience>(&json).expect("decode"),
                audience,
                "{json}"
            );
            // And through what a person types, which is the other direction it travels in.
            assert_eq!(
                audience.to_string().parse::<Audience>().expect("parse"),
                audience
            );
        }

        // The spellings somebody actually types, all meaning the two ends of the range.
        for text in ["all", "any", "everyone", " Everyone "] {
            assert_eq!(text.parse::<Audience>().expect("parse"), Audience::Everyone);
        }
        for text in ["none", "off", "nobody"] {
            assert_eq!(text.parse::<Audience>().expect("parse"), Audience::Nobody);
        }
        // An unknown name is a service this build has not heard of rather than an error — and
        // what saves somebody from a typo is being told at submission time that no route in the
        // fleet offers it, not the parser.
        assert_eq!(
            "psuh".parse::<Audience>().expect("parse"),
            Audience::Service {
                service: Service::Other("psuh".into())
            }
        );
        // An agent is a service and not a route to anybody, and this parser is `Service`'s: it
        // says what the words mean, not whether they make sense as an audience. Nothing can
        // honour this one, and the CLI refuses it before it gets this far.
        assert_eq!(
            "agent:claude-code".parse::<Audience>().expect("parse"),
            Audience::Service {
                service: Service::Agent(crate::capability::AgentKind::ClaudeCode)
            }
        );
        // The one spelling with no meaning at all.
        assert!("".parse::<Audience>().is_err());
    }

    #[test]
    fn a_missing_audience_on_the_wire_means_tell_everybody() {
        // A v9 record has no audience, and the direction it defaults in matters: a silence is
        // the failure this plane exists to prevent, so the absence has to widen rather than
        // narrow. Also what `#[serde(default)]` on the spec relies on.
        #[derive(Deserialize)]
        struct Spec {
            #[serde(default)]
            notify: Audience,
            // The same question for the kinds half (ADR-0026), and the same answer: a record
            // written before it existed must widen to telling somebody, never narrow.
            #[serde(default)]
            notices: Notices,
        }
        let spec: Spec = serde_json::from_str("{}").expect("decode");
        assert_eq!(spec.notify, Audience::Everyone);
        assert!(spec.notify.is_everyone());
        assert_eq!(spec.notices, Notices::Everything);
        assert!(
            spec.notices.admits(&Notice::Finished {
                turns: 1,
                cost_micro_usd: 0,
                denials: 0,
                work: crate::WorkKind::Agent,
            }),
            "the absence tells somebody"
        );
    }

    #[test]
    fn a_sink_that_cannot_deliver_says_which_kind_of_cannot() {
        let mut cap = Capability::new("push:phone", Service::Push, [Role::Sink]);
        assert_eq!(
            deliverability(&cap),
            Err(Undeliverable::Unauthenticated),
            "nothing is authenticated until it is verified"
        );
        cap.authenticated = true;
        assert_eq!(deliverability(&cap), Ok(()));

        // Same service, different role: an inbox a run reads is not one the fleet may write to.
        let mut resource = Capability::new(
            "email:me",
            Service::Email,
            [Role::Resource {
                access: crate::capability::Access::Read,
            }],
        );
        resource.authenticated = true;
        assert_eq!(deliverability(&resource), Err(Undeliverable::NotASink));
    }

    #[test]
    fn an_enrolment_names_the_device_and_the_way_out() {
        // ADR-0012 mitigation 4: this is the compensating control for there being no per-join
        // approval, so the one thing it must contain is what to do about it. A notification
        // saying "a device joined" and nothing else is an alarm somebody cannot act on from a
        // phone at midnight.
        let event = FleetLogEvent::new(
            1_700_000_000_000,
            FleetEvent::Enrolled {
                node: crate::NodeId::from_bytes([7; 32]),
                name: "laptop-2".into(),
                grants: crate::default_grants(),
                how: Enrolment::Passphrase,
            },
        );
        let note = notable_fleet(3, &event).expect("every fleet event is notable");
        assert_eq!(note.subject, Subject::Fleet);
        assert_eq!(note.subject.as_run(), None);
        assert_eq!(note.kind_name(), "enrolled");
        assert!(note.summary().contains("laptop-2"));
        assert!(
            note.summary().contains("offload revoke"),
            "{}",
            note.summary()
        );
        assert!(note.title().contains("joined"));
    }

    #[test]
    fn a_device_met_rather_than_enrolled_says_so() {
        // Two sentences about one event, each true where it was written. The machine that
        // issued the invitation watched it happen; every other machine met the result, and
        // "a device you have not seen before is in your fleet" is the one worth reading twice.
        let met = FleetLogEvent::new(
            1_700_000_000_000,
            FleetEvent::Enrolled {
                node: crate::NodeId::from_bytes([7; 32]),
                name: "laptop-2".into(),
                grants: crate::default_grants(),
                how: Enrolment::Met,
            },
        );
        let note = notable_fleet(1, &met).expect("notable");
        assert!(
            note.title().contains("have not seen before"),
            "{}",
            note.title()
        );
    }

    #[test]
    fn the_dedup_key_distinguishes_the_two_logs() {
        // Both logs number from one, so a key that carried only the sequence would let the
        // first fleet event mark the first run's notification as already sent.
        let run = crate::RunId::from_bytes([1; 16]);
        let a = Notification {
            subject: Subject::run(run),
            seq: 1,
            at: Millis(0),
            notice: Notice::Failed { reason: "x".into() },
        };
        let b = Notification {
            subject: Subject::Fleet,
            ..a.clone()
        };
        assert_ne!(a.key("phone"), b.key("phone"));
        assert_eq!(a.subject.topic().name(), "run");
        assert_eq!(b.subject.topic().name(), "fleet");
    }

    #[test]
    fn a_use_of_the_passphrase_names_the_machine_it_was_typed_on() {
        // The alarm is only useful if it says *where*: "the passphrase was used" is a fact
        // somebody can neither confirm nor act on, and the whole posture rests on its being
        // rare enough that they remember doing it.
        let event = FleetLogEvent::new(
            1_700_000_000_000,
            FleetEvent::PassphraseUsed {
                what: "revoke ab12cd34".into(),
                node: crate::NodeId::from_bytes([1; 32]),
                name: "desktop".into(),
            },
        );
        let note = notable_fleet(2, &event).expect("notable");
        assert_eq!(note.kind_name(), "passphrase");
        assert!(note.summary().contains("desktop"));
        assert!(note.summary().contains("revoke ab12cd34"));
    }
}
