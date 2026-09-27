//! A run's event log: what the agent did, as the fleet sees it.
//!
//! Not logging. This is the run's *output* — the agent's text, its tool calls, its turn
//! boundaries, what a checkpoint captured — and it is data: it is written to the store, streamed
//! to whoever is following, and (since a follower may be on another machine) carried between
//! nodes. That last one is why it lives here rather than in the daemon's control protocol where
//! it started: `offload-proto` cannot depend on `offload-node`, and an event that crossed the
//! wire as untyped JSON would be exactly the "encoding error at runtime" trap this codebase has
//! already been bitten by once.
//!
//! Two things are deliberately *not* here: appending (that is the store's) and the clock
//! ([`LogEvent::at`] takes the instant, because this crate has none).

use serde::{Deserialize, Serialize};

/// `#[serde(default)]` for a `bool` that defaults to `true`, since `Default::default()` is
/// `false`. See [`LogKind::CaptureFailed::run_continues`].
const fn yes() -> bool {
    true
}

/// One line of a run's event log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEvent {
    pub at_unix_ms: u64,
    #[serde(flatten)]
    pub kind: LogKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum LogKind {
    Submitted {
        repo: String,
        prompt: String,
        permission: String,
    },
    WorkspaceReady {
        path: String,
        branch: String,
        base: String,
    },
    /// A workspace that arrived as bytes was unpacked here (ADR-0061 §1–§2).
    ///
    /// The acquisition report, and it is in the **run's** log rather than only in this node's,
    /// because the question it answers is asked from anywhere and often later: what travelled?
    /// A cloned repository needs no such line — the ref says what is in it, and anybody can go
    /// and look. An archive is a *subset an agent chose*, nobody else has a copy of the
    /// selection, and the failure this design has is a run dying for want of a file that is
    /// sitting on the operator's disk. Without this the two are indistinguishable.
    ///
    /// Written once per node per run (§3): a node that already held the archive moved no bytes
    /// and logs nothing, because a line claiming an acquisition that did not happen is worse
    /// than no line.
    WorkspaceAcquired {
        /// The archive's digest, short — what `offload status` and the refusals name it by.
        archive: String,
        /// Files, bytes and the top-level entries, as
        /// `offload_workspace::Acquired` renders them.
        summary: String,
    },
    AgentStarted {
        model: String,
        version: String,
        session: String,
    },
    /// This run continues a finished one, and here is what it was given (ADR-0064 §6).
    ///
    /// The continuation's first row after `Submitted`, on whichever node starts it. `missing`
    /// says in words what was *not* handed over — no closing message, no transcript — because a
    /// continuation that did not know what came before must not read like one that did.
    Continued {
        parent: String,
        mode: crate::run::ContinueMode,
        /// What the workspace was built from, as the capture summarised it.
        base: String,
        /// The parent transcript's size, when one was handed over.
        #[serde(default)]
        transcript_bytes: Option<u64>,
        #[serde(default)]
        missing: Vec<String>,
    },
    /// A task began: the nominated program is running (ADR-0019).
    ///
    /// Its own variant rather than an `AgentStarted` with the model and version left blank.
    /// That is what the first walk of the task tier actually printed — `agent claude-code ,
    /// model` — a log line claiming an agent for a run that has none, with two empty fields
    /// where the claim should have been. "Don't over-claim" applies to a log line as much as
    /// to a probe, and a run's log is where somebody goes to find out what happened.
    TaskStarted {
        /// What the owner declared, which is all the submitter ever named.
        service: String,
    },
    Text {
        text: String,
    },
    ToolUse {
        name: String,
    },
    /// A checkpointable moment (ADR-0004).
    TurnBoundary {
        turn: u32,
    },
    /// State was captured durably at a turn boundary.
    Checkpointed {
        turn: u32,
        /// What went into it, including anything the untracked policy left behind.
        summary: String,
        /// Whether the holder also gave the run up. A released checkpoint ends this
        /// holder's output; a routine one is just a marker mid-stream.
        released: bool,
    },
    /// The run started again from a checkpoint.
    Resumed {
        /// The turn count the checkpoint was taken at.
        from_turn: u32,
        session: String,
        /// How the workspace came back: adopted in place, or rebuilt from blobs.
        workspace: String,
    },
    RateLimit {
        kind: String,
        status: String,
        /// When the limit lifts, if the agent said. Unix milliseconds.
        ///
        /// The agent has always sent this and the log used to drop it, which was fine while
        /// nothing judged a rate limit against anything. It is the input [`LogKind::Overdue`]
        /// needs: a limit that lifts is a wait, and whether a wait matters is entirely a
        /// question of when it ends.
        #[serde(default)]
        resets_at_unix_ms: Option<u64>,
    },
    /// This run is not going to make a deadline somebody stated, and nothing else was going
    /// to notice.
    ///
    /// The one event here that is about the *absence* of progress, which is why it exists at
    /// all: every other line in this log is something that happened. ADR-0013 ends two of its
    /// failure rows here — a run nobody will take, and a run whose rate limit lifts too late —
    /// and both were previously silent, either retried for ever or recorded as a fact with no
    /// consequence.
    ///
    /// Three things it deliberately is not:
    ///
    /// * **Not terminal.** The run is still pending or still running, and a deadline changes
    ///   when we give up, never what happens to the work (ADR-0013). Anything that treated
    ///   this as the end of a run would be cancelling work on a schedule, which is the one
    ///   thing that boundary forbids.
    /// * **Not repeated.** Said once per deadline, by the node that established it. An alarm
    ///   that fires every thirty seconds until somebody acts is an alarm people filter.
    /// * **Not a notification, yet.** It is a typed line in the run's own log, which is the
    ///   substrate ADR-0010's delivery plane fans out from when it exists. Until then the
    ///   answer to "who sees this" is whoever reads the log — honest, and better than a
    ///   `WARN` on a machine nobody is logged into.
    Overdue {
        /// How far past due, once the wait it is stuck behind ends.
        by_ms: u64,
        waiting: Waiting,
    },
    Finished {
        success: bool,
        turns: u32,
        denials: u32,
        cost_micro_usd: u64,
        /// Which tier finished, because the three numbers above are an *agent's* and a task
        /// has none of them.
        ///
        /// A field with a `#[serde(default)]` of `Agent` rather than a second variant, for
        /// [`LogKind::CaptureFailed::run_continues`]'s reason: a defaulted field decodes on a
        /// node that has never heard of it, and a new variant fails the whole log page (v8,
        /// v9, v11). The default is not a guess — a row written before the task tier was
        /// written by a binary in which an agent was the only thing a run could be.
        ///
        /// Carried here rather than looked up from the spec because [`crate::notify::notable`]
        /// is a pure function of one event: the notification it projects is delivered to a
        /// phone, and *"finished after 0 turn(s), $0.0000"* about a shell script is what
        /// having no tier here produced.
        #[serde(default)]
        work: crate::run::WorkKind,
        /// The agent's own closing message — the `result` of its last `result` event — verbatim,
        /// cut at [`CLOSING_MESSAGE_CAP`] by [`closing_message`] (ADR-0064, amendment check 2).
        ///
        /// Kept because a continuation is handed it, and it was being dropped: the supervisor had
        /// it and wrote only the numbers. `None` for a task, for a row written before this, and
        /// for an agent that closed with nothing — three cases a reader treats the same, as *no
        /// closing message*, which is why one `Option` is enough. Defaulted for `work`'s reason.
        #[serde(default)]
        result: Option<String>,
    },
    /// An operator stopped it, and from where.
    ///
    /// `by` for [`LogKind::Answered`]'s reason: `offload cancel` is answered wherever it is
    /// typed and acted on wherever the run is, so a log that said only "cancelled" would be a
    /// log of a decision with no record of who made it. It is the *node's* name, which is as
    /// much as the machine that acts can honestly know — the handshake settled that the peer may
    /// say this at all, and nothing on the wire carries a person.
    Cancelled {
        by: String,
    },
    Failed {
        reason: String,
    },
    /// A checkpoint could not be taken, and the run carried on.
    ///
    /// Its own variant because `Failed` is the *run's* state, and reusing it for an operation
    /// that happens inside a healthy run was wrong in three ways at once — which is what makes
    /// this worth a variant rather than a wording change. Anything reading the log reads `Failed`
    /// as the end: [`LogEvent::is_terminal`] said so, which hung up every follower mid-run and
    /// with them the attendance that decides whether a failure resumes itself; and the
    /// notification projection dutifully told somebody their run had failed, minutes before
    /// telling them it had finished.
    ///
    /// A failed capture is not news and not terminal: the agent is unharmed, nothing has been
    /// handed over, and the next turn boundary tries again. It is in the log because a run
    /// checkpointing nothing all night is worth being able to see afterwards.
    CaptureFailed {
        turn: u32,
        reason: String,
        /// Is there another boundary to try at?
        ///
        /// **The sentence above is true of the caller this variant was written for and false of
        /// the one ADR-0039 added.** A turn limit captures at the boundary it stops on, and that
        /// boundary is the last one there will ever be — so the log said "the run continues; the
        /// next turn boundary tries again" one line above the line saying the run had ended, and
        /// the run somebody capped was left with no checkpoint and a report claiming a retry
        /// that could not happen.
        ///
        /// A field rather than a variant because the capture failed the same way in both cases;
        /// what differs is what happens next, which only the caller knows. Named after the
        /// assertion the renderer makes, so the two cannot drift apart silently — the previous
        /// version of that assertion was a constant string with nothing to check it against.
        ///
        /// `#[serde(default)]` to `true`, which is what every row written before this field
        /// meant on the path that wrote almost all of them. An older node decodes the field's
        /// absence the same way and prints the sentence it always printed, so no wire bump —
        /// the same shape as ADR-0040's `RunProgress::tokens`, and unlike a new *variant*, which
        /// fails a whole log page on a node that does not know it (v8, v9, v11).
        #[serde(default = "yes")]
        run_continues: bool,
    },
    /// The agent wants to do something the run does not already permit, and is **blocked**
    /// waiting for an answer (ADR-0017).
    ///
    /// The one line in this log that somebody is expected to *act* on rather than read, which is
    /// why it is here rather than only in the daemon's own logging: it is what the delivery plane
    /// fans out, and a question nobody was shown is a run that quietly did less than it was
    /// asked.
    ///
    /// Not terminal, and not a denial. It is the state of a wait — the answer arrives as
    /// [`LogKind::Answered`], including when the answer is "nobody said anything in time".
    Asked {
        /// The agent's tool, as the agent names it: `Bash`, `WebFetch`.
        tool: String,
        /// What it wants to do, in one line, for a person reading it on a phone.
        detail: String,
        /// The agent's own `tool_use_id`. The identity of *this* request, which is what makes an
        /// answer un-replayable: an approval is for one tool call and cannot be spent twice.
        tool_use_id: String,
        /// How long there is to answer, as a duration from when this was written.
        ///
        /// In the *log* because that is what leaves the node: the delivery plane fans this out to
        /// whichever device can reach a person, and `Notice::NeedsDecision` said in its own doc
        /// comment that it carries "how long there is to give one, because `approve this` with no
        /// clock is a promise this plane cannot keep" — while having no such field. The number was
        /// in scope at this call site the whole time and went only to `tracing`, on the one machine
        /// nobody is logged into.
        #[serde(default)]
        within_ms: u64,
    },
    /// This run has asked as many questions as it was given, and will not ask another
    /// (ADR-0017).
    ///
    /// Said **once**, for [`LogKind::Overdue`]'s reason: an alarm that repeats is an alarm people
    /// filter, and everything after this point is ordinary agent behaviour rather than news.
    ///
    /// Not a failure and not terminal. The run carries on with its calls decided by the agent's
    /// own rules, which is what every run did before this channel existed — so this line is in the
    /// log for the same reason denials are counted at all (ADR-0008): a run that quietly did less
    /// than it was asked has to be something a person can see afterwards.
    AskBudgetSpent {
        /// How many were put to somebody. The budget, since running out is what this event is.
        asked: u32,
    },
    /// What was decided, and by whom.
    ///
    /// Deliberately **not** notable (`offload_core::notify`): telling somebody what they just did
    /// is the fastest way to teach them to ignore notifications, and a timed-out question is news
    /// about the *run*, which its result already carries.
    Answered {
        tool_use_id: String,
        answer: Answer,
        /// Who answered, for the log. An operator, or the daemon saying nobody did.
        by: String,
        /// The tool the question was about, repeated from [`LogKind::Asked`].
        ///
        /// Denormalised on purpose: the projection sees one event at a time, and a notification
        /// saying a decision window closed is useless without saying which decision. The
        /// alternative is a notice that names a `tool_use_id` at somebody on a phone.
        #[serde(default)]
        tool: String,
    },
}

/// The three ways a permission question ends (ADR-0017).
///
/// Three rather than two, because *nobody answered* is not a denial and recording it as one would
/// be a lie in a log that other things read. Nothing was granted — and what happens next is the
/// agent's own rules, which for a gated command is a refusal and for a harmless one is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "answer")]
pub enum Answer {
    Allowed,
    Denied,
    /// The wait ran out. See [`crate::Run::approval_patience`] for why there is a wait at all.
    Unanswered,
}

impl std::fmt::Display for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Answer::Allowed => f.write_str("allowed"),
            Answer::Denied => f.write_str("denied"),
            Answer::Unanswered => f.write_str("left to the agent's own rules — nobody answered"),
        }
    }
}

/// What a run is waiting for, when the wait is longer than the run has.
///
/// Typed rather than a sentence because ADR-0010's notifications are "the small typed subset
/// of run events worth interrupting a person for", and a delivery plane that has to parse
/// English to decide what to do with something is not one. The two variants are the two rows
/// ADR-0013 left open, and they are genuinely different situations: one is the fleet having no
/// room, which somebody can change by turning a machine on, and one is an account being spent,
/// which they cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "waiting", rename_all = "snake_case")]
pub enum Waiting {
    /// Offered to the fleet and refused by everybody, with no end in sight.
    ///
    /// The count is how many nodes answered and would not take it — the number that makes the
    /// difference between "the fleet is full" and "there is nobody out there", which are the
    /// same silence and two different problems.
    Placement { refused_by: u32 },
    /// The agent's account is rate limited, and the reset is later than the run is due.
    ///
    /// The reset instant is optional because the agent's own report is: it can say it is being
    /// held up without saying until when. That is a wait with no known end rather than a
    /// missing field, and it is judged the way the other one is — against now, which means it
    /// is announced only for a run that is *already* past due. See [`crate::Run::prospect_at`].
    RateLimit {
        kind: String,
        resets_at_unix_ms: Option<u64>,
    },
}

impl std::fmt::Display for Waiting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Waiting::Placement { refused_by: 0 } => {
                f.write_str("no node answered an offer to take it")
            }
            Waiting::Placement { refused_by: 1 } => f.write_str(
                "the one node that answered \
                 would not take it",
            ),
            Waiting::Placement { refused_by } => {
                write!(f, "all {refused_by} nodes that answered refused it")
            }
            Waiting::RateLimit {
                resets_at_unix_ms: Some(_),
                kind,
            } => write!(
                f,
                "its account's {kind} rate limit lifts after the deadline"
            ),
            Waiting::RateLimit {
                resets_at_unix_ms: None,
                kind,
            } => write!(
                f,
                "its account's {kind} rate limit has no stated reset, and the deadline has gone"
            ),
        }
    }
}

/// Is this rate-limit status the agent being *stopped*, or reporting where it stands?
///
/// It emits one of these per turn either way, so the distinction decides whether a deadline is
/// judged at all — and judging the informational ones would announce a missed deadline for
/// every overdue run in the fleet that merely got told how much quota it has left.
///
/// A prefix test rather than an equality one, because `allowed_warning` is the agent saying it
/// is *approaching* a limit, which is still not being held up by one. Anything the agent
/// invents later that does not begin with `allowed` is treated as a hold-up, which is the safe
/// direction: over-reporting a delay costs one line in a log, and missing one costs the run.
#[must_use]
pub fn rate_limit_blocks(status: &str) -> bool {
    !status.starts_with("allowed")
}

impl LogEvent {
    /// Stamped by the caller, because this crate has no clock (ADR-0001). The daemon has one
    /// `now()` and everything it writes goes through it; a type that read the clock itself would
    /// be a second source of time in the one crate that is meant to have none.
    #[must_use]
    pub fn at(now: crate::time::Millis, kind: LogKind) -> Self {
        LogEvent {
            at_unix_ms: now.0,
            kind,
        }
    }

    /// A short, stable name for this event, used as the store's `kind` column.
    ///
    /// Written out rather than derived from `Debug`: this value goes in a database and is
    /// queried by hand, so it must not change when a variant is renamed or reordered.
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            LogKind::Submitted { .. } => "submitted",
            LogKind::WorkspaceReady { .. } => "workspace_ready",
            LogKind::WorkspaceAcquired { .. } => "workspace_acquired",
            LogKind::AgentStarted { .. } => "agent_started",
            LogKind::Continued { .. } => "continued",
            LogKind::TaskStarted { .. } => "task_started",
            LogKind::Text { .. } => "text",
            LogKind::ToolUse { .. } => "tool_use",
            LogKind::TurnBoundary { .. } => "turn_boundary",
            LogKind::Checkpointed { .. } => "checkpointed",
            LogKind::Resumed { .. } => "resumed",
            LogKind::RateLimit { .. } => "rate_limit",
            LogKind::Overdue { .. } => "overdue",
            LogKind::Finished { .. } => "finished",
            LogKind::Cancelled { .. } => "cancelled",
            LogKind::Failed { .. } => "failed",
            LogKind::CaptureFailed { .. } => "capture_failed",
            LogKind::Asked { .. } => "asked",
            LogKind::AskBudgetSpent { .. } => "ask_budget_spent",
            LogKind::Answered { .. } => "answered",
        }
    }

    /// Is this the last event this holder will produce?
    ///
    /// Not the same as "the run is over": a released checkpoint ends the agent process
    /// while leaving the run pending for somebody to pick up. A follower has to stop on it
    /// regardless, or `logs -f` waits forever on a stream that has nothing left to say.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.kind,
            LogKind::Finished { .. }
                | LogKind::Cancelled { .. }
                | LogKind::Failed { .. }
                | LogKind::Checkpointed { released: true, .. }
        )
    }
}

/// The most of a closing message a log row keeps, in bytes (ADR-0064 §3).
pub const CLOSING_MESSAGE_CAP: usize = 16 * 1024;

/// A closing message as a log row keeps it: whole when it fits, else cut at a character boundary
/// under [`CLOSING_MESSAGE_CAP`] with a line saying so — a cut nobody is told about reads as the
/// agent having stopped mid-sentence. Empty is `None`, since an empty closing message and none at
/// all are one fact to every reader.
#[must_use]
pub fn closing_message(text: Option<String>) -> Option<String> {
    let text = text?;
    if text.trim().is_empty() {
        return None;
    }
    if text.len() <= CLOSING_MESSAGE_CAP {
        return Some(text);
    }
    let mut end = CLOSING_MESSAGE_CAP;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(format!(
        "{}\n[cut: the closing message was {} bytes, and {} are kept]",
        &text[..end],
        text.len(),
        end
    ))
}

#[cfg(test)]
mod tests {

    /// ADR-0064 §3: a closing message is kept whole under the cap, cut on a character boundary
    /// with a line saying so above it, and an empty one is none at all.
    #[test]
    fn a_closing_message_is_kept_whole_cut_loudly_or_absent() {
        assert_eq!(closing_message(None), None);
        assert_eq!(closing_message(Some("  \n".into())), None);
        assert_eq!(closing_message(Some("DONE".into())), Some("DONE".into()));
        // A multi-byte character straddling the cap must not panic the slice.
        let long = format!(
            "{}é{}",
            "a".repeat(CLOSING_MESSAGE_CAP - 1),
            "b".repeat(100)
        );
        let kept = closing_message(Some(long.clone())).expect("kept");
        assert!(kept.starts_with(&"a".repeat(CLOSING_MESSAGE_CAP - 1)));
        assert!(kept.contains("[cut: the closing message was"), "{kept}");
        assert!(kept.len() < long.len() + 100);
    }

    /// A row written before the field existed decodes as having no closing message.
    #[test]
    fn a_finished_row_written_before_the_closing_message_was_kept_has_none() {
        let old =
            r#"{"event":"finished","success":true,"turns":3,"denials":0,"cost_micro_usd":120}"#;
        let kind: LogKind = serde_json::from_str(old).expect("an old row decodes");
        assert!(
            matches!(kind, LogKind::Finished { result: None, .. }),
            "{kind:?}"
        );
    }
    use super::*;
    use crate::time::Millis;

    /// Both directions, over the encoding the wire actually uses.
    ///
    /// This codebase has shipped two bugs where an internally-tagged enum compiled and then
    /// failed at *runtime* — a variant wrapping a sequence, and a newtype variant wrapping an
    /// `Option`. `Overdue` is a struct variant holding another tagged enum, which is fine, and
    /// the way to know that is to round-trip it rather than to reason about it.
    #[test]
    fn the_new_events_survive_a_round_trip() {
        let events = [
            LogEvent::at(
                Millis(1),
                LogKind::Overdue {
                    by_ms: 3_600_000,
                    waiting: Waiting::Placement { refused_by: 4 },
                },
            ),
            LogEvent::at(
                Millis(2),
                LogKind::Overdue {
                    by_ms: 60_000,
                    waiting: Waiting::RateLimit {
                        kind: "five_hour".into(),
                        resets_at_unix_ms: Some(1_785_006_000_000),
                    },
                },
            ),
            LogEvent::at(
                Millis(3),
                LogKind::RateLimit {
                    kind: "five_hour".into(),
                    status: "allowed".into(),
                    resets_at_unix_ms: None,
                },
            ),
        ];
        for event in events {
            let json = serde_json::to_string(&event).expect("encode");
            let back: LogEvent = serde_json::from_str(&json).expect("decode");
            assert_eq!(back, event, "{json}");
        }
    }

    /// A log written before the reset instant existed still reads, which is the whole reason
    /// that field is an `Option` with a `default` rather than a `u64` meaning "epoch".
    #[test]
    fn a_rate_limit_logged_by_an_older_build_still_decodes() {
        let json = r#"{"at_unix_ms":1,"event":"rate_limit","kind":"five_hour","status":"allowed"}"#;
        let back: LogEvent = serde_json::from_str(json).expect("decode");
        assert!(matches!(
            back.kind,
            LogKind::RateLimit {
                resets_at_unix_ms: None,
                ..
            }
        ));
    }

    /// The bug this variant was carved out of, as two assertions.
    ///
    /// A failed capture used to be logged as `LogKind::Failed`, and everything downstream
    /// believed it: `is_terminal` ended every follower's stream mid-run — taking the attendance
    /// that decides whether a failure resumes itself with it — and the notification projection
    /// told somebody their run had failed a minute before telling them it had finished.
    #[test]
    fn a_failed_capture_is_neither_the_end_of_the_run_nor_news() {
        let capture = LogEvent::at(
            Millis(1),
            LogKind::CaptureFailed {
                turn: 3,
                reason: "no transcript found".into(),
                run_continues: true,
            },
        );
        assert!(!capture.is_terminal());
        assert_eq!(capture.kind_name(), "capture_failed");
        assert!(!crate::notify::NOTABLE_KINDS.contains(&capture.kind_name()));
        assert!(crate::notify::notable(crate::RunId::from_bytes([1; 16]), 1, &capture).is_none());

        // A capture that failed at the boundary a turn limit stopped on is still not the run's
        // own failure — the run ends `Failed` with the limit as its reason, written separately
        // and by `fail`. What changes is only the sentence under it, and this holds the two
        // apart: `run_continues` must not leak into terminality or into what is worth telling
        // somebody about, which is what borrowing `LogKind::Failed` for this did.
        let last = LogEvent::at(
            Millis(1),
            LogKind::CaptureFailed {
                turn: 3,
                reason: "the agent's transcript holds no conversation yet at turn 3".into(),
                run_continues: false,
            },
        );
        assert!(!last.is_terminal());
        assert_eq!(last.kind_name(), "capture_failed");
        assert!(crate::notify::notable(crate::RunId::from_bytes([1; 16]), 1, &last).is_none());

        // And the state it was borrowing still means what it always did.
        let failed = LogEvent::at(
            Millis(1),
            LogKind::Failed {
                reason: "the agent stopped".into(),
            },
        );
        assert!(failed.is_terminal());
        assert!(crate::notify::notable(crate::RunId::from_bytes([1; 16]), 1, &failed).is_some());
    }

    /// The claim that let `run_continues` be added without a wire bump, as a test rather than a
    /// paragraph: a row written before the field decodes as the sentence it was printed with.
    ///
    /// This is the whole compatibility story. The event log is `event_json` in SQLite and a log
    /// page over the wire, so both hold rows written by older builds — and the default has to be
    /// `true`, not `Default::default()`, or every capture failure ever recorded would come back
    /// claiming the run had ended.
    #[test]
    fn a_capture_failure_recorded_before_the_field_still_says_the_run_continued() {
        let old =
            r#"{"at_unix_ms":1,"event":"capture_failed","turn":3,"reason":"no transcript found"}"#;
        let decoded: LogEvent = serde_json::from_str(old).expect("an old row still decodes");
        assert_eq!(
            decoded.kind,
            LogKind::CaptureFailed {
                turn: 3,
                reason: "no transcript found".into(),
                run_continues: true,
            }
        );
    }

    /// Not terminal, and the test is here because the consequence of getting it wrong is
    /// invisible: a follower would hang up on it and a person watching a run would be told it
    /// had ended, when what happened is that it is going to be late.
    #[test]
    fn being_late_does_not_end_a_run() {
        assert!(!LogEvent::at(
            Millis(1),
            LogKind::Overdue {
                by_ms: 1,
                waiting: Waiting::Placement { refused_by: 1 },
            }
        )
        .is_terminal());
    }

    #[test]
    fn only_a_status_that_is_not_allowed_counts_as_being_held_up() {
        // Per turn, either way — so this is what stops an ordinary run's quota report from
        // announcing a missed deadline.
        assert!(!rate_limit_blocks("allowed"));
        assert!(!rate_limit_blocks("allowed_warning"));
        assert!(rate_limit_blocks("rejected"));
        assert!(rate_limit_blocks(""));
    }
}
