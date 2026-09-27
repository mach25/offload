//! The control protocol between `offload` (CLI) and `offloadd` (daemon).
//!
//! Newline-delimited JSON over a unix socket: one request per line, one or more responses
//! per line. Deliberately not the cluster protocol — that is `offload-proto` in phase 3
//! and has entirely different constraints (versioning, untrusted-ish peers, wire size).
//! This one is local, same-user, and optimized for being debuggable with `nc`.

use offload_core::PermissionMode;
/// What `Request::Files` answers with, shared with the peer that read it (ADR-0075).
pub use offload_proto::cluster::{FileEntry, FilesContent, FilesSource, FilesView};
use serde::{Deserialize, Serialize};

/// Sent by the CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// What is this node, and what is it doing?
    Status,
    Submit(SubmitRequest),
    /// Submit a **task**: the cheap tier, a program this fleet's owner nominated (ADR-0019).
    ///
    /// A second request rather than optional fields on [`SubmitRequest`], for the reason the
    /// domain has a `Work` enum: a submission carrying a prompt *and* a service, with a rule
    /// somewhere about which wins, is one message that is two messages. Two requests make the
    /// impossible combinations unsendable rather than merely refused.
    SubmitTask(SubmitTaskRequest),
    /// Take an archive workspace into this node's blob store, and answer with its digest
    /// (ADR-0061 §1).
    ///
    /// A **path**, not the bytes. The control socket is a unix socket and the submitting agent
    /// is on this machine, so the file is already reachable — and an archive is up to
    /// `MAX_BLOB_BYTES`, which is not a thing to base64 through a JSON line. It is the same
    /// reasoning `--repo <path>` already relies on.
    ///
    /// Separate from [`Request::Submit`] rather than a field on it, because the two fail
    /// differently and at different times: a 400 MB file that cannot be read is not a rejected
    /// submission, and a submission refused by the fleet should not silently leave a blob
    /// behind. The digest comes back so the caller spells the repo itself.
    StoreArchive {
        /// Absolute or relative to the daemon's working directory.
        path: String,
    },
    /// An agent, blocked mid-tool-call, is asking whether it may do something (ADR-0017).
    ///
    /// Sent by the permission hook rather than by a person, which is why it carries the epoch:
    /// the hook belongs to one *leg* of a run, and a question from an agent whose run has since
    /// been reassigned must not be answered as if it were current.
    ///
    /// The connection is held open until there is an answer — it is the only request in this
    /// protocol that waits on a human, and the wait is bounded by
    /// `Run::approval_patience`.
    Ask {
        run: String,
        epoch: u64,
        /// The agent's own id for this tool call, which is what an answer is addressed to.
        tool_use_id: String,
        tool: String,
        /// One line describing the call, for whoever has to decide.
        detail: String,
    },
    /// Answer a question an agent is blocked on.
    ///
    /// `tool_use_id` may be omitted when the run is waiting on exactly one thing, and is
    /// *required* when it is waiting on several: an agent can block on parallel calls, and
    /// approving the wrong one of three on somebody's behalf is not a convenience.
    Answer {
        run: String,
        #[serde(default)]
        tool_use_id: Option<String>,
        allow: bool,
    },
    /// What is waiting for a person right now.
    Asks,
    /// "Connect me to the resource my run was granted, wherever it is." (ADR-0011)
    ///
    /// Sent by `offloadd use-resource`, which the agent spawns as an ordinary MCP server. The
    /// connection then carries the agent's own protocol in both directions as
    /// [`Request::ResourceLine`] and [`Response::ResourceLine`], one message per line, until the
    /// agent exits and its side of the pipe closes.
    ///
    /// Wrapped rather than switched to a raw byte pipe, because this protocol is one JSON object
    /// per line and stays debuggable with `nc` — the double encoding costs nothing on control
    /// traffic, and a connection that changes shape halfway through is a thing every future
    /// reader has to know about.
    UseResource {
        run: String,
        service: String,
    },
    /// One line of the agent's protocol, on a connection that has already said `UseResource`.
    ResourceLine {
        line: String,
    },
    /// All runs this node knows about.
    List,
    /// Every node this one knows about, and how it is doing.
    Nodes,
    /// Read this node's agent model list again, and ask the fleet to do the same (ADR-0080).
    RefreshModels,
    /// What this node decided about runs, and what it was refused (`offload_core::audit`).
    ///
    /// Local by design, like `FleetHistory`: it is a record of what *this machine* did, so there
    /// is nothing to canvass and nothing to gossip. Asked of each node in turn when the question
    /// is about the fleet.
    Audit {
        /// One run, or all of them.
        #[serde(default)]
        run: Option<String>,
        limit: u32,
    },
    /// What this node has witnessed happening to its fleet (ADR-0012 mitigation 4).
    ///
    /// A record of observations rather than the fleet's memory, which is the honest framing: the
    /// machine that issued an invitation saw an enrolment, and every other machine saw a member
    /// it had never met. Both are true where they were written.
    FleetHistory {
        limit: u32,
    },
    /// Hand every run here to somebody else and stop taking new ones.
    Drain,
    /// Replay a run's event log, optionally following it live.
    Logs {
        run: String,
        follow: bool,
    },
    Cancel {
        run: String,
    },
    /// Discard a finished run's worktree. Refused while the run is still active.
    Remove {
        run: String,
    },
    /// Capture the run at its next turn boundary and hand it back to the pool.
    ///
    /// Not "checkpoint right now": mid-turn is not a safe capture point (ADR-0004), so
    /// this is a request the holder honours when it next reaches one.
    Checkpoint {
        run: String,
    },
    /// Restart a checkpointed run from where it stopped.
    Resume {
        run: String,
        /// What to tell the agent on the way back in. `None` uses the adapter's
        /// continue-where-you-left-off prompt.
        prompt: Option<String>,
    },
    /// What is at `path` in a run's workspace, read-only (ADR-0075): here if the checkout or the
    /// branch is here, else from the node that ran it. `""` is the root.
    Files {
        run: String,
        #[serde(default)]
        path: String,
    },
    /// Continue a finished run with a new one (ADR-0064). Answered like a submission.
    Continue(Box<ContinueRequest>),
    /// Why this run is where it is: who arbitrates it, what they make of it, and what every
    /// node says about taking it.
    ///
    /// Runs a fresh bid round with nothing granted, so it costs the bid window and one
    /// message per peer. Deliberately a request rather than a local read: the answers are
    /// about now, and the ones from the round that placed the run are hours stale.
    Explain {
        run: String,
    },
    /// Change when a run is due, or put it back to as soon as possible.
    ///
    /// The only thing about a submitted run that can be changed (ADR-0013), and it is
    /// forwarded to the node that owns the field rather than applied wherever it was typed.
    SetDeadline {
        run: String,
        /// Unix milliseconds. `None` means *as soon as you can*, which is what a run with no
        /// deadline has always meant.
        deadline: Option<u64>,
    },
    /// Change which runs this one goes ahead of when a machine has room for only some of them.
    ///
    /// The second and last editable field (ADR-0013). Absolute rather than a delta: a delta has
    /// to be added to something, and the only value available where the command is typed may be
    /// a gossip tick old.
    SetPriority {
        run: String,
        priority: i32,
    },
    /// This node's routes to a human, whether they work, and what they have carried
    /// (ADR-0010).
    ///
    /// Local, and deliberately so: a sink's credentials never leave the device that holds them,
    /// so what a peer could tell you about its own routes is what it advertises as a capability
    /// — which is `offload nodes`' business. This answers "why did my phone not buzz".
    Sinks {
        /// Send a test notification through each usable route rather than only describing them.
        ///
        /// The only way to check the half this node cannot probe: it can see that a script
        /// exists and never that it reaches anybody. Deliberately not written to the outbox — a
        /// test is not a run event, and it must not be able to mark a real notification sent.
        test: bool,
    },
    /// Write a standing instruction: when this service's trigger fires here, submit this run
    /// (ADR-0020).
    ///
    /// Local, like `Sinks` and for the same reason turned around: a trigger is a program *this*
    /// node's owner nominated, so the rule that binds it to work belongs on the machine the
    /// program is on. Nothing about it is gossiped, and nothing about it travels.
    Watch {
        service: String,
        rule: RuleRequest,
    },
    /// What this node will do when one of its triggers fires, and what it has done so far.
    Rules,
    /// Forget one standing instruction.
    Unwatch {
        rule: String,
    },
    /// What this node is watching, and whether the watchers are up (ADR-0020).
    Triggers,
    /// Create a standing instruction on a clock (ADR-0019 §3, ADR-0056).
    ///
    /// **Not** local, unlike `Watch` next door, and the difference is the whole reason a
    /// schedule is the larger mechanism: a trigger is a program this node's owner nominated, so
    /// a rule bound to it belongs on that machine — while a clock is everywhere, and a schedule
    /// that died with the laptop that created it would be `cron` with extra steps.
    ///
    /// Carries a submission rather than a spec: the daemon builds it here, where a submission
    /// becomes a spec anyway, so the creating node's model, permission mode and baseline
    /// allowlist are the ones written down (ADR-0056 §3).
    Every(Box<EverySpec>),
    /// Every schedule this node knows: its own and the fleet's.
    Schedules,
    /// Take a schedule away, everywhere.
    Unschedule {
        schedule: String,
    },
}

/// What to run, and how often (ADR-0056).
///
/// One of `agent` or `task`, which is the `Work` split arriving at the control protocol: a
/// message carrying both, with a rule about which wins, is one message that is two.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EverySpec {
    /// The period, in milliseconds. At least `offload_core::MIN_EVERY`.
    pub every_ms: u64,
    /// How far into the period the tick falls, in milliseconds — `24h` with `3h` is 03:00 UTC.
    ///
    /// **UTC**, and the CLI says so where somebody types it. A schedule is a fleet-wide fact and
    /// the fleet's nodes are not necessarily in one timezone, so the only clock they can all
    /// agree about is the one with no rules in it (ADR-0056 §1).
    #[serde(default)]
    pub offset_ms: u64,
    /// One line for `offload schedules`, so a person can tell two of them apart.
    #[serde(default)]
    pub note: String,
    /// An agent run, every tick.
    #[serde(default)]
    pub agent: Option<SubmitRequest>,
    /// …or the cheap tier, which is what ADR-0019's scenario is about.
    #[serde(default)]
    pub task: Option<SubmitTaskRequest>,
}

/// One schedule, as reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleReport {
    pub id: String,
    /// The node that created it, by name where this node knows it.
    pub home: String,
    /// Which node fires it right now (`ClusterView::steward_of`), as this node can see it.
    ///
    /// Said rather than left to be worked out, because it is the answer to the question a
    /// person asks of a schedule that is not firing — and computing it in the CLI would be a
    /// second copy of a successor rule.
    #[serde(default)]
    pub steward: Steward,
    /// `every 15m`, worded by the daemon.
    pub every: String,
    /// What it fires, in one line (`Work::summary`), and which tier.
    pub kind: offload_core::WorkKind,
    pub work: String,
    pub note: String,
    /// When the next tick falls, as a duration from now.
    pub next: String,
    /// The last tick **this node** fired, as a duration ago. `None` where it has fired none —
    /// which is every schedule on a node that is not its steward, and is not a fault.
    pub last_fired: Option<String>,
    /// Set on a schedule somebody has removed. It is still here because a tombstone is what
    /// travels; a listing that hid them would be hiding the mechanism from the one person who
    /// might wonder why a removed schedule is still in a database.
    pub removed: bool,
}

/// Who fires a schedule, from where this node is standing.
///
/// **Three-valued, because `ClusterView::steward_of` is.** The successor rule answers *this
/// node*, *that node*, or **nobody** — and the third is not a quiet variant of the second. A
/// node that cannot see a schedule's home chooses no successor, on purpose: a daemon that has
/// just started and met no peers must not begin firing the whole fleet's schedules. So nothing
/// fires it, anywhere this node can see.
///
/// It was a `bool`, and the third answer rendered as *"fired elsewhere"* — a report that names
/// another machine for work no machine is doing. Measured on two daemons: bravo restarted while
/// alpha was off, held a live copy of alpha's schedule, and said `fired elsewhere` with a
/// countdown beneath it across two whole tick periods while firing nothing. The honest half of
/// the same fact was one line above it, where `home` had fallen back to a bare node id because
/// that node is not in the view either.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "steward", rename_all = "snake_case")]
pub enum Steward {
    /// This node fires it.
    Here,
    /// Another node does, named where this node knows the name. Not necessarily the home: a
    /// home that is `gone` hands the tick to the lowest-id node that is alive.
    Elsewhere { node: String },
    /// Nobody this node can see. The default, because a daemon too old to send the field is
    /// also one whose answer cannot be assumed to be good news.
    #[default]
    Nobody,
}

/// A submission plus the one field a standing instruction cannot store as submitted.
///
/// `SubmitRequest::deadline` is absolute unix milliseconds, resolved where the command was typed.
/// That is right for `offload run` and wrong for a rule in a way that would never announce
/// itself: a rule written this morning would fire runs whose deadline passed hours ago, for ever.
/// So a rule holds the *duration* and it is resolved at the firing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleRequest {
    /// What the rule fires: an agent run, or a task (ADR-0019).
    ///
    /// One of the two, never both, which is `Request::SubmitTask`'s argument arriving at the
    /// command that stores a submission instead of making one.
    pub work: RuleWork,
    /// What *fires* it: a trigger this node watches, or a notice (ADR-0057).
    ///
    /// The name it is bound to travels in `Request::Watch::service` either way, because in both
    /// cases it is a name the daemon looks rules up by. This says which namespace that name is
    /// in — a service or a `Notification::kind_name` — and it has to be said rather than
    /// guessed, since `Service::Other` accepts any string and a trigger whose service is called
    /// `failed` is a legal thing to nominate.
    #[serde(default)]
    pub fired_by: FiredBy,
    #[serde(default)]
    pub deadline_secs: Option<u64>,
}

/// What fires a rule (ADR-0057 §1).
///
/// `Trigger` is the default in every sense: it is what a rule written before this existed is,
/// what an older CLI's request decodes to, and the only thing ADR-0020 had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FiredBy {
    /// One line of a nominated program's stdout (ADR-0020).
    #[default]
    Trigger,
    /// A notice this node projected from a run's event log (ADR-0057).
    Notice,
}

/// The two things a trigger can start.
///
/// Named here rather than in `trigger` because it is part of the control protocol as well as of
/// what a rule stores — and it is the same shape in both, deliberately: the daemon turns one
/// into the other by appending the event and stamping the origin, and a second shape in between
/// would be a place for the two to disagree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tier", rename_all = "snake_case")]
pub enum RuleWork {
    Agent(SubmitRequest),
    Task(SubmitTaskRequest),
}

/// One standing instruction, as reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleReport {
    pub id: String,
    pub service: String,
    /// What fires it: a trigger this node watches, or a notice (ADR-0057).
    ///
    /// Here for the reason it is on `Response::Watching`, met a second time: the listing printed
    /// *"nothing on this node watches `failed` — it cannot fire"* under an escalation rule that
    /// had just fired, because `trigger_present` is the wrong question about a rule the delivery
    /// plane fires. Two reports, one wrong sentence, and the same fix — a report describes a
    /// mechanism, so it has to know which one.
    #[serde(default)]
    pub fired_by: FiredBy,
    /// Which tier a firing starts (`Work::kind`).
    ///
    /// `offload ps`'s column, on the other command that describes work — and here for the same
    /// reason: `work` below holds a prompt on one row and a service on the next, and a column
    /// that means two things depending on the row is the report shape this project keeps having
    /// to fix. `#[serde(default)]` is `Agent`, which every rule written before the task tier
    /// was.
    #[serde(default)]
    pub kind: offload_core::WorkKind,
    /// What a firing runs, in one line: the prompt as written — without the event appended,
    /// which has not happened yet — or the task's service and arguments.
    pub work: String,
    /// The repository a fired run acts on, or empty for a tier that has none.
    pub repo: String,
    pub fired: u64,
    /// Events that arrived while this rule's last run was still going, and were discarded
    /// (ADR-0020 §3). Never folded into `fired`: a rule reporting "fired 3, dropped 412" is
    /// saying its runs are slower than its trigger, and that is the only symptom that has.
    pub dropped: u64,
    pub last_run: Option<String>,
    /// Occurrence records this rule still has here (ADR-0021).
    ///
    /// A rule that fires all night and succeeds settles at one. A number that only grows is the
    /// symptom every residual ADR-0021 accepts shares — a rule whose runs keep failing, a route
    /// that has been away for days, a peer nobody removed — and none of them has any other
    /// visible sign, which is the delivery plane's own rule applied to the thing beside it.
    #[serde(default)]
    pub kept: u64,
    /// Which of this rule's news will interrupt somebody (ADR-0026).
    ///
    /// Printed because the default here is *not* `offload run`'s: a rule is quiet about a firing
    /// that went fine. A rule written months ago whose author has forgotten which way it was set
    /// is exactly the case where "no notification" and "no failure" look identical.
    #[serde(default)]
    pub notices: String,
    pub last_fired_unix_ms: Option<u64>,
    /// Whether an occurrence is going **right now**, asked of the store at the moment of the
    /// report rather than remembered.
    ///
    /// The stored reason beside it is history — "the last event was dropped because X was still
    /// running" is true of the moment it was written and says nothing about this one. Printed
    /// with no tense, that reads as current state: measured, a rule whose run had *failed* an
    /// hour earlier still reported `still running 01a03ad8ef23`, one line under `last fired
    /// 01a03ad8ef23`, naming one run twice and saying two different things about it. Same family
    /// as `offload explain` counting a finished run down towards its deadline, and the same fix:
    /// ask the question at the instant it is answered.
    #[serde(default)]
    pub in_flight: Option<String>,
    /// Why the last event produced no run, as a sentence about *then*.
    pub last_error: Option<String>,
    /// Whether any of this node's triggers actually offers this service right now. A rule for a
    /// trigger the owner has since removed from their config is listed rather than deleted:
    /// removing somebody's standing instruction on the strength of a config edit is the wrong
    /// direction to be confident in.
    pub trigger_present: bool,
}

/// One watcher, as reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerReport {
    pub id: String,
    pub service: String,
    /// Shown because this is the owner reading about their own machine — it is never gossiped.
    pub command: String,
    pub description: String,
    pub watching: bool,
    pub events: u64,
    pub restarts: u64,
    pub last_error: Option<String>,
    /// How many rules are bound to this trigger's service. A watcher firing into nothing is the
    /// commonest reason "my trigger does not work" turns out to be true and uninteresting.
    pub rules: u64,
}

/// One line of the fleet's history, already worded by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetHistoryEntry {
    pub at_unix_ms: u64,
    pub kind: String,
    /// The device it is about.
    pub node: String,
    pub summary: String,
}

/// Submit a task: which nominated program, and the shared half of any run's spec.
///
/// Notice what is **not** here, and could not be added without giving a submitter the fleet:
/// no command, no arguments to a shell, no URL. `service` names what the owner declared and
/// this node's own config says what that runs (ADR-0019 §1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitTaskRequest {
    /// Which nominated program, by service.
    pub service: offload_core::Service,
    /// What kind of thing is asking (ADR-0024). Absent is `Operator`, which is what a person at
    /// a keyboard means and what an older CLI's request decodes to.
    ///
    /// The agent request has carried this since ADR-0024 and this one did not, which was
    /// invisible while nothing machine-started could be a task: a rule firing one would have
    /// produced an occurrence recorded as an **operator's**, so nothing would ever reclaim its
    /// record and `offload cleanup`'s premise — somebody will read the worktree — would be
    /// applied to a run nobody typed. Set by the daemon at the firing
    /// (`RuleSpec::into_submission`), never trusted from what was stored.
    #[serde(default)]
    pub origin: offload_core::Origin,
    /// Arguments appended to the owner's own. Never the program.
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub queue: bool,
    #[serde(default)]
    pub deadline: Option<u64>,
    #[serde(default)]
    pub demand: offload_core::Demand,
    #[serde(default)]
    pub notify: offload_core::Audience,
    #[serde(default)]
    pub notices: offload_core::Notices,
    /// What a node must have to host this (ADR-0063 §6), ANDed with what the daemon adds itself
    /// — `agent_ready`, or the task tier's clause. `here` and `node=` arrive unresolved and are
    /// looked up by the daemon that takes the submission, which is the one that knows the ids.
    #[serde(default)]
    pub require: offload_core::Wanted,
    /// Where it would rather be: scored, never filtered (ADR-0063 §1).
    #[serde(default)]
    pub prefer: offload_core::Wanted,
    /// Until when the preference is a requirement, in unix milliseconds (ADR-0063 §4). Absolute
    /// for `deadline`'s reason.
    #[serde(default)]
    pub hold_until: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitRequest {
    /// Repo path or clone URL.
    pub repo: String,
    pub prompt: String,
    pub model: Option<String>,
    /// `None` means the node's configured default (ADR-0008: `AcceptEdits`).
    pub permission: Option<PermissionMode>,
    /// Ref to branch from. `None` means the repo's default branch.
    pub git_ref: Option<String>,
    /// Extra tool grants for this run, layered onto the node's and the repo's.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Leave the run pending if nobody will take it now, instead of refusing (ADR-0014).
    #[serde(default)]
    pub queue: bool,
    /// When this run needs to be done, in unix milliseconds. `None` means as soon as you can,
    /// which is what an unset deadline has always meant (ADR-0013).
    ///
    /// Absolute rather than a duration from now, because it is resolved where it was typed:
    /// the daemon adding an offset to its own clock would answer a slightly different question
    /// from the one asked, and a run that is forwarded gets it added twice.
    #[serde(default)]
    pub deadline: Option<u64>,
    /// What hosting this is expected to cost a device (ADR-0013). Absent means `Normal`, which
    /// is what an agent run is until somebody knows better.
    #[serde(default)]
    pub demand: offload_core::Demand,
    /// Which routes this run's news is for (ADR-0010). Absent means every route the fleet has,
    /// which is what a fleet of one person's devices wants.
    #[serde(default)]
    pub notify: offload_core::Audience,
    /// Which of this run's news is worth interrupting somebody for (ADR-0026). `Everything` for
    /// a submission, because somebody typed it and walked away; `offload when` chooses
    /// `Problems`, because a standing instruction's value is that it is quiet until something
    /// happens.
    #[serde(default)]
    pub notices: offload_core::Notices,
    /// May this run stop and ask a person for permission rather than being denied, and how
    /// many times (ADR-0017)?
    #[serde(default)]
    pub ask: offload_core::AskPolicy,
    /// The most turns this run may take before it is stopped. `None` is no limit, which is
    /// what every run had before this existed and is still the default.
    ///
    /// `NonZeroU32`, so a limit of zero is refused here — by serde, on the way in, rather than
    /// by a check in `build` that a second submission path could forget.
    #[serde(default)]
    pub max_turns: Option<std::num::NonZeroU32>,
    /// Resources this run is granted the use of, by service (ADR-0011). Empty is nearly every
    /// run, and means it reaches nothing beyond its own worktree.
    #[serde(default)]
    pub resources: Vec<offload_core::Service>,
    /// What a node must have to host this (ADR-0063 §6), ANDed with what the daemon adds itself
    /// — `agent_ready`, or the task tier's clause. `here` and `node=` arrive unresolved and are
    /// looked up by the daemon that takes the submission, which is the one that knows the ids.
    #[serde(default)]
    pub require: offload_core::Wanted,
    /// Where it would rather be: scored, never filtered (ADR-0063 §1).
    #[serde(default)]
    pub prefer: offload_core::Wanted,
    /// Until when the preference is a requirement, in unix milliseconds (ADR-0063 §4). Absolute
    /// for `deadline`'s reason.
    #[serde(default)]
    pub hold_until: Option<u64>,
    /// What kind of thing is asking (ADR-0024). Absent is `Operator`, which is what every
    /// submission typed at a keyboard means and what an older CLI's request decodes to.
    ///
    /// Set by exactly one caller — a rule firing (`RuleSpec::into_request`) — and carried through
    /// `submit_run` onto the `Run`, because the whole point is that the fact reaches nodes that
    /// have never heard of the rule.
    #[serde(default)]
    pub origin: offload_core::Origin,
}

/// `offload continue`: which finished run, what to do next, and how much of it to start with.
///
/// Everything else about the new run is the parent's (ADR-0064 §4), so this carries only what
/// the continuer may change. The deadline and a hold are deliberately the continuer's alone —
/// the parent's were about the parent — and so are the budgets, which count afresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinueRequest {
    /// The parent, by id or prefix.
    pub run: String,
    pub prompt: String,
    pub mode: offload_core::ContinueMode,
    #[serde(default)]
    pub queue: bool,
    #[serde(default)]
    pub deadline: Option<u64>,
    /// Added to the parent's grants, never replacing them.
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub max_turns: Option<std::num::NonZeroU32>,
    #[serde(default)]
    pub ask: offload_core::AskPolicy,
    /// `None` keeps the parent's, which the ADR's inheritance table says it does.
    #[serde(default)]
    pub require: Option<offload_core::Wanted>,
    #[serde(default)]
    pub prefer: Option<offload_core::Wanted>,
    #[serde(default)]
    pub hold_until: Option<u64>,
}

/// One thing a drain has done or is waiting on, as it happens (ADR-0035).
///
/// Deliberately three steps and not a running log of everything: what an operator standing at a
/// laptop needs is the fact that took effect the moment they asked (they can close the lid), the
/// wait they are now in, and the one thing they can do something about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum DrainStep {
    /// This node has stopped taking work.
    ///
    /// First and unconditionally, because it is the one thing a drain always does and the *only*
    /// thing it can do on a fleet of one (ADR-0034) — and it is true before anything is
    /// attempted, so saying it afterwards is saying it minutes late.
    StoppedAccepting,
    /// Waiting for agents to reach a turn boundary, and how long that wait is bounded by.
    Waiting { runs: u32, up_to_ms: u64 },
    /// The last copy of a checkpoint nobody else had has been pushed to a peer (ADR-0054).
    ///
    /// Said only when there was something at risk, because a drain that reports every pass it
    /// makes is a drain nobody reads. `stranded` is the runs whose only copy is still only here —
    /// the fleet would not take one, or the budget ran out — and it is the number that decides
    /// whether closing the lid loses work.
    PushedLastCopies { pushed: u32, stranded: u32 },
    /// …and one of them is blocked on a question only a person can answer (ADR-0017).
    ///
    /// The whole reason this stream exists. A run blocked mid-tool-call reaches no turn boundary,
    /// so the drain waits out the question's patience for nothing, and the answer is one command
    /// away — carried here with the `tool_use_id` an answer is addressed to, because that is what
    /// makes the sentence actionable rather than merely informative.
    Blocked {
        /// The run id **whole**, because the CLI prints it into an `offload approve` line and an
        /// instruction is not a column. It carried `RunId::short` — twelve characters, which for
        /// a scheduled occurrence name the tick rather than the run (ADR-0056).
        run: String,
        tool: String,
        detail: String,
        tool_use_id: String,
        /// How long is left before nobody-answering decides it.
        within_ms: u64,
        /// …and how long is left before **this drain** stops waiting, which is a different
        /// deadline with a different consequence.
        ///
        /// The question expiring *decides* it, and the agent then takes its turn and reaches a
        /// boundary, so the run is handed over normally. The drain's deadline expiring first
        /// decides nothing: the run is left mid-turn with the question still open. So the
        /// smaller of the two is the one an operator is actually racing, and reporting only
        /// `within_ms` told somebody ten seconds from losing the run that they had four
        /// minutes. `#[serde(default)]` because it was added after `within_ms`; zero from an
        /// older daemon reads as "unknown", and the CLI falls back to the sentence it had.
        #[serde(default)]
        drain_ends_in_ms: u64,
    },
}

/// Sent by the daemon. A request may produce several of these before `Done`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Response {
    /// Boxed because it is much the largest thing this enum carries, and every other variant
    /// would otherwise pay for it — `Response` is the reply to every control-socket request.
    Status(Box<NodeStatus>),
    /// An archive is in this node's blob store, under this digest (ADR-0061 §1).
    ///
    /// Carries the size because the caller is about to submit a run against it and the number
    /// is the one thing about an archive worth saying out loud — §4's cap is the constraint the
    /// submitting agent is working to.
    ArchiveStored {
        hash: String,
        bytes: u64,
    },
    Submitted {
        run: String,
        /// Which node took it, when that was decided by a bid round. `None` on a node with no
        /// mesh, where "here" is the only answer there has ever been.
        #[serde(default)]
        node: Option<String>,
        /// When it starts, if that is not "now" — a node may accept a run it has no room for
        /// yet and hold it (ADR-0006). Reporting acceptance without saying so would tell
        /// somebody their work is under way when it is queued.
        #[serde(default)]
        waiting: Option<String>,
        /// The submission exists on this machine and nowhere else.
        ///
        /// Only true when this node kept the run and no peer would take a copy of the record.
        /// It is the difference between closing the laptop costing nothing and losing the work
        /// before it started, so it is said out loud rather than left to `ps`.
        #[serde(default)]
        here_only: bool,
        /// Nobody would take it and the operator asked for it to be left pending anyway
        /// (ADR-0014). Carries every node's reason: "queued" on its own is the shrug the
        /// whole decision exists to prevent, and the reasons are what somebody checks back
        /// against in the morning.
        #[serde(default)]
        queued: Option<String>,
        /// What the run asked to be told through, and whether the fleet can honour it
        /// (ADR-0010's audience). `None` for the default audience, which needs no words.
        ///
        /// Answered while the person is still at the keyboard, for ADR-0014's reason: a run
        /// asking for a route no device in this fleet offers is a silence somebody would
        /// otherwise discover by not being told at two in the morning. It is a *note* and never
        /// a refusal — the work is still worth doing, and a run whose news went nowhere is a
        /// finished run.
        #[serde(default)]
        audience: Option<String>,
        /// What `--ask` will amount to on this fleet, when that is not what it says on the tin.
        ///
        /// `None` when the run did not ask, or when there is somebody to ask. The flag changes
        /// what the *run* does when it is blocked — not merely who hears about it — so a fleet
        /// that can reach nobody turns it into the behaviour of not having passed it, and that is
        /// answerable while the operator is still at the keyboard (ADR-0014).
        #[serde(default)]
        ask: Option<String>,
        /// The run's preference was not what decided where it went, and why (ADR-0063 §7).
        /// `None` when it was met, or when there was none.
        #[serde(default)]
        preference: Option<String>,
        /// For a continuation, the parent's other continuations this node knows, whole — the
        /// question after *continue the last one* is *which one was last* (ADR-0064 §6).
        #[serde(default)]
        siblings: Vec<String>,
    },
    /// A struct variant, not `Runs(Vec<_>)`: serde's internally-tagged representation
    /// cannot encode a newtype variant wrapping a sequence, and fails at runtime rather
    /// than at compile time.
    Runs {
        runs: Vec<RunSummary>,
        /// Why `offload resume`, typed here, would be refused — when a listed run is one
        /// somebody would type it at.
        ///
        /// A fact about the *node*, travelling with a listing of *runs*, because one of the
        /// sentences in that listing names a command this node answers. `recover` writes
        /// `resumable from turn N with 'offload resume <id>'` onto a run at startup; that is
        /// right about the run and can be wrong about the machine, since a drained node, a
        /// revoked one, or one whose owner said `accept = "never"` refuses that command
        /// (ADR-0047) while the record goes on naming it. The record must keep saying what it
        /// says — it is a permanent note about why a run stopped, and baking a live fact into
        /// one is the trap that has cost this project three sessions — so the live half
        /// arrives beside it instead.
        ///
        /// Both halves of the question are answered **here** rather than in the CLI: the
        /// sentence is [`crate::server::hosting_refusal`]'s, so this listing and the door
        /// cannot disagree, and the relevance gate is `supervisor::resumable_by_hand` — the
        /// states `refuse_unresumable` admits a run by, **and** the tier: a task is restarted
        /// and never resumed (ADR-0058), so this footnote about the machine has no business
        /// standing in front of a refusal about the run. A client that decided either for
        /// itself would be the second copy of the rule that every entry in
        /// `docs/pitfalls/reports-and-cli.md` is about.
        ///
        /// `hosting_refusal` rather than `offload status`'s `accepting` line: the two differ by
        /// the queue, which empties by itself and which the door reports separately.
        #[serde(default)]
        resume_refusal: Option<String>,
    },
    /// What a drain is doing, while it is doing it.
    ///
    /// A drain is the only request here that can take *minutes*: it waits for every agent to
    /// reach a turn boundary (ADR-0004), and one blocked mid-turn on a question reaches none
    /// until somebody answers. Blocking until the pass returned meant `offload drain` printed
    /// nothing at all for up to five minutes while the person who could release it instantly was
    /// the person who had just typed it (ADR-0035).
    Draining(DrainStep),
    /// How a drain went: what moved, what finished on its own, and what could not.
    Drained {
        moved: u32,
        left: u32,
        /// Runs that finished by themselves while the drain waited for their turn to end.
        ///
        /// Neither moved nor left behind, and `wait_for_checkpoint`'s own comment calls it "the
        /// nicest possible outcome" — while the caller counted it in `left` and logged it as
        /// `still mid-turn at the drain deadline`, which is the sentence that keeps a lid open.
        #[serde(default)]
        finished: u32,
        /// Runs still mid-turn at the deadline, which this node will hand over at their next
        /// turn boundary.
        ///
        /// Split out of `left` for the reason `finished` was: a count that means two things is
        /// described by a sentence right for at most one of them. `left` still means final — a
        /// run nobody would take — and this one means not yet, which is the case the CLI's
        /// "`offload ps` and `offload nodes` say which" advice cannot answer, because what it
        /// waits on is a turn boundary that has not happened.
        ///
        /// **Mid-turn only, since ADR-0042.** It used to count refused-after-release too, which
        /// was right while both waited on this daemon. They no longer wait on the same thing —
        /// see `pooled` — and the difference is the question somebody typing `offload drain`
        /// is actually asking, which is whether the lid can be closed. This half still says no:
        /// the agent is running here, and the boundary it is being handed over at is one only
        /// this process can reach.
        #[serde(default)]
        later: u32,
        /// Runs this node released into the pool that nobody would take **yet**.
        ///
        /// The other half of what `later` used to be, and it is a different sentence now that
        /// the record can carry the fact: a released run says which node let it go (ADR-0042),
        /// so whoever arbitrates it keeps offering it whether or not this daemon is still
        /// running. Nothing further is owed by this machine, which is what an operator standing
        /// at a laptop wants to know — and it is the outcome that used to be reported as `left`
        /// and left the run stopped for good.
        #[serde(default)]
        pooled: u32,
        /// Failed runs handed back to the fleet on the way out (ADR-0043).
        ///
        /// Not a share of anything above it. Every other number here counts a run this node
        /// *held*, and a failed run holds nothing — it is terminal, so the handover pass has
        /// never seen one. Until this existed a drain acted on these runs and said nothing at
        /// all about them, which is the eighth thing `offload drain` has been silent or wrong
        /// about; the fix and the sentence arrived together this time.
        #[serde(default)]
        handed_back: u32,
        /// There was no fleet to offer anything to.
        ///
        /// Without it the CLI cannot tell "every peer refused it" from "there are no peers", and
        /// it printed the first about a node that has none — advising `offload nodes` on a
        /// machine whose fleet is itself. The daemon's own arm knows which case it is and the
        /// distinction was dropped between two `u32`s.
        #[serde(default)]
        no_fleet: bool,
        /// Runs still going here with **no turn boundary to wait for** — tasks (ADR-0019 §2).
        ///
        /// Split out of `later`, which promises a handover at a run's next turn boundary and is
        /// false in both halves for a nominated program. The drain used to *wait* for that
        /// boundary — `drain_deadline_secs`, 300 seconds by default — and then make the promise.
        #[serde(default)]
        no_boundary: u32,
    },
    /// What an agent's blocked hook is told (ADR-0017).
    ///
    /// Three outcomes rather than two, because *nobody answered* is not a denial: nothing was
    /// granted and the agent's own rules decide, which is exactly what happens on every run in
    /// the fleet today.
    Decision {
        answer: offload_core::Answer,
        /// What to tell the agent, in the words the person used where there were any.
        reason: String,
    },
    /// A question was answered, and which one it was.
    ///
    /// Said back rather than acknowledged silently: an agent can be blocked on more than one
    /// call, and "approved" without saying what would be the wrong kind of reassuring.
    Answered {
        tool: String,
        detail: String,
        allowed: bool,
    },
    /// A struct variant for the same reason as `Runs`.
    ///
    /// Includes questions on *other* nodes, asked for the moment somebody looked (ADR-0017).
    /// Not gossiped: a blocked process stops being blocked the instant somebody answers, so a
    /// remembered copy is a question that has already been dealt with, shown as current — the
    /// same confident wrong answer a replayed bid would be.
    Asks {
        asks: Vec<offload_core::PendingAsk>,
    },
    /// A struct variant for the same reason as `Runs`.
    Sinks {
        sinks: Vec<crate::deliver::SinkReport>,
        /// Routes other nodes advertise, which this node can use for its own runs' news
        /// (ADR-0010). Read from gossiped capabilities rather than asked for: a peer's sink is a
        /// capability like any other, and what it *is* travels while how it works never does.
        #[serde(default)]
        fleet: Vec<crate::deliver::FleetRoute>,
    },
    /// A struct variant for the same reason as `Runs`.
    Rules {
        rules: Vec<RuleReport>,
    },
    /// A struct variant for the same reason as `Runs`.
    Triggers {
        triggers: Vec<TriggerReport>,
    },
    /// A struct variant for the same reason as `Runs`.
    Schedules {
        schedules: Vec<ScheduleReport>,
    },
    /// A schedule was taken away. Its id, in full, because a prefix was what was typed.
    Unscheduled {
        schedule: String,
        /// How long ago it had *already* been removed, when this call removed nothing.
        ///
        /// `None` is the ordinary case: this call wrote the tombstone. `Some` is a second
        /// `offload unschedule` on the same schedule, which is not a failure — the schedule is
        /// gone, which is what was wanted — and is not what the command used to say it was.
        /// Worded by the daemon, for `Scheduled::next`'s reason: the clock is here.
        ///
        /// `#[serde(default)]`, like every other field added to this socket: the CLI and the
        /// daemon ship together, and a daemon too old to send it renders the ordinary sentence
        /// rather than a wrong one.
        #[serde(default)]
        already: Option<String>,
    },
    /// A schedule was created. Its id, so it can be taken away again.
    Scheduled {
        schedule: String,
        /// When its first tick falls, worded by the daemon — because the answer to *did that
        /// work* is when something will happen, and a person who typed a period is owed the
        /// instant it lands on.
        next: String,
        /// Where its occurrences' news goes, when that is worth saying (ADR-0010).
        ///
        /// The three notes below are [`Self::Watching`]'s, from the same functions, and they
        /// were missing here for the reason the comment beside `Request::Every` gives about
        /// its neighbour: `offload when` gained them when somebody walked a rule, and nobody
        /// had walked a schedule. Every argument for warning a rule is **stronger** for a
        /// schedule, which does not die with the device it was typed on.
        ///
        /// Spelled exactly as [`Self::Watching`] spells them, with no `serde` defaults, because
        /// the point is that the two cannot drift: the CLI and the daemon ship together, and a
        /// fourth "the daemon did not say" state here would be a distinction `Watching` does
        /// not make about the same three facts.
        ///
        /// There is deliberately no `ask` note beside them, which `Watching` has: `offload
        /// every` exposes no `--ask`, so the policy is always `Never` and the note would be a
        /// field that is always absent. Add it with the flag, not before.
        audience: Option<String>,
        /// Whether anything in this fleet could tell a person about an occurrence.
        reach: crate::deliver::Reach,
        /// Why a firing will be refused rather than run: a task nobody nominates, or a
        /// resource nobody offers. `None` when every precondition is met.
        resources: Option<String>,
    },
    /// A rule was written. Its id, so it can be forgotten again.
    Watching {
        rule: String,
        /// Whether a trigger for that service is nominated on this node right now. A rule for a
        /// service nothing here watches is written and *said* to be inert, rather than refused:
        /// the config may be about to gain one, and refusing would make the order of two commands
        /// matter.
        trigger_present: bool,
        /// What fires it (ADR-0057), so the report can say the right sentence about the right
        /// mechanism: a trigger's watcher may be missing and a notice's plane cannot be.
        #[serde(default)]
        fired_by: FiredBy,
        /// What the rule asked to be told through, and whether the fleet can honour it — the
        /// same note `Submitted` carries, from the same function.
        ///
        /// It was on the submission and on nothing else for two phases, which is the wrong way
        /// round: a run is answered for while its author is at the keyboard, and a rule is
        /// written once and then fires unattended for months. `offload run --notify push` on a
        /// fleet with no push route said so; `offload when --notify push` printed a promise.
        #[serde(default)]
        audience: Option<String>,
        /// What `--ask` will amount to on this fleet, when that is not what it says on the tin.
        /// `Submitted`'s field, for `Submitted`'s reason, on the command that needed it more.
        #[serde(default)]
        ask: Option<String>,
        /// Whether this rule's news can get anywhere — and if not, which silence it is.
        ///
        /// Not derivable from `audience` above: that is `None` for the default audience, so
        /// `Audience::Everyone` on a fleet with no sinks at all is the one case a note computed
        /// for `offload run` is deliberately silent about. It is also the plainest possible
        /// invocation of `offload when`.
        reach: crate::deliver::Reach,
        /// What `--use` will amount to on this fleet, when nothing in it offers the service.
        ///
        /// The third precondition, and the one whose absence was worst, because on the *other*
        /// command it is not a note at all: `offload run --use email` is **refused** where no node
        /// offers a mailbox, so a rule with the same flag is written, fires, and has every
        /// occurrence refused. Measured: 13 firings, 0 runs, thirty seconds — and the keyboard
        /// said `Nothing else to do: the next event fires it` (ADR-0036).
        #[serde(default)]
        resources: Option<String>,
    },
    /// The answer to [`Request::Files`].
    Files {
        view: offload_proto::cluster::FilesView,
    },
    /// A struct variant for the same reason as `Runs`.
    Nodes {
        nodes: Vec<crate::mesh::NodeSummary>,
        /// This node's own id, so the caller can mark which row is the machine it is talking
        /// to. `None` when there is no mesh at all — a different answer from "the mesh has one
        /// member", and the CLI says so.
        local: Option<String>,
    },
    /// What this node has witnessed happening to its fleet, newest first.
    FleetHistory {
        entries: Vec<FleetHistoryEntry>,
    },
    /// A struct variant for the same reason as `Runs`.
    Audit {
        entries: Vec<offload_core::AuditEntry>,
        /// What this node calls the nodes its rows mention, so the CLI prints `bravo` where the
        /// row holds an id. Empty from an older daemon, which renders short ids as before.
        #[serde(default)]
        names: Vec<(offload_core::NodeId, String)>,
    },
    /// The resource is open: what follows is the agent's own protocol.
    ResourceOpen {
        /// Where it turned out to be, for the proxy's own log. Not shown to the agent.
        node: String,
    },
    /// One line of the agent's protocol, coming back.
    ResourceLine {
        line: String,
    },
    Event(LogEvent),
    /// Boxed because it is much the largest thing this enum carries, and every other
    /// response would otherwise be as big as an explanation.
    Explanation(Box<Explanation>),
    /// A run's deadline or priority was changed, with what that does to *this* run.
    ///
    /// The caveat is the point. On a pending run the change re-opens placement or reorders what
    /// is offered first; on a running one it changes nothing anybody can see until something
    /// goes wrong, and a command that let somebody believe otherwise would be the most tempting
    /// lie in this design.
    SpecEdited {
        run: String,
        /// What the field says now: "1h59m left", "priority 10".
        summary: String,
        note: String,
    },
    /// A run was stopped, and where.
    ///
    /// `note` rather than a bare acknowledgement, for the same reason [`Response::Answered`]
    /// says back what it decided: "cancelled" covers an agent taken down mid-turn and a
    /// commitment that had not started, and only one of those cost money. `node` is `None` when
    /// the run was here, which needs no words.
    Cancelled {
        run: String,
        #[serde(default)]
        node: Option<String>,
        note: String,
    },
    /// A checkpoint was asked for, and of which machine.
    ///
    /// Deliberately not "checkpointed": the agent may be minutes into a turn, and mid-turn is not
    /// a safe capture point (ADR-0004). `node` is `None` when the run is here.
    CheckpointRequested {
        run: String,
        #[serde(default)]
        node: Option<String>,
    },
    /// A standing instruction was forgotten.
    Forgotten {
        rule: String,
        /// Occurrence records left behind, after the spent ones were pruned (ADR-0021 §4).
        ///
        /// Said rather than left to be discovered: what survives a prune is a failed occurrence
        /// or one whose news has not gone out, and after the rule is gone there is nothing left
        /// that would ever mention them again.
        #[serde(default)]
        kept: u64,
    },
    /// A run's checkout was torn down here.
    ///
    /// `discarded` is what that checkout held that nothing else has a copy of, in the words
    /// `offload ps`'s WORKSPACE column uses. `None` — the ordinary case — is a checkout that
    /// held nothing uncommitted. Said rather than left out, because the command's own sentence
    /// is about what *survives*, and a person tearing down a `failed` run's worktree is one
    /// keystroke from `offload resume` and has no other way to find out what went with it.
    Removed {
        run: String,
        #[serde(default)]
        discarded: Option<String>,
    },
    /// The answer to [`Request::RefreshModels`]: this node is reading its list again, and
    /// `fleet` says whether the request is travelling to others, which it cannot without a mesh.
    ModelsAsked {
        fleet: bool,
    },
    /// End of a multi-part response.
    Done,
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeStatus {
    pub node_id: String,
    pub name: String,
    pub device_class: String,
    pub agent: Option<String>,
    pub agent_authenticated: bool,
    /// The program this node would spawn for a run, as its owner wrote it.
    ///
    /// Shown when there is no agent, for the reason a resource's missing program is named: this
    /// is the setting that decides which binary was asked, so "none installed" without it is a
    /// sentence about a machine that may well have an agent somewhere else on it.
    #[serde(default)]
    pub agent_binary: String,
    /// How many runs of that agent this node will have going at once, if the agent's own ceiling
    /// is lower than the owner's.
    ///
    /// Two numbers because they are two limits and either can bind: the pair `runs 2/4` was the
    /// owner's, printed on a machine refusing its third run against an agent ceiling of 2 that
    /// nothing in the config could raise. `None` when the owner's is the one that binds, which is
    /// the ordinary case and the one worth saying nothing about.
    #[serde(default)]
    pub agent_max_concurrent: Option<u32>,
    /// The largest workspace archive this node will take, in bytes (ADR-0061 §4).
    ///
    /// Here so it can be read **before** an archive is built. An agent deciding which 12 MB of a
    /// 6.5 GB directory matter is working to a budget, and learning the budget from a refusal
    /// costs it the minutes and the disk it spent packing the wrong thing — which is §4's *say
    /// the cap before the expensive step*, and the reason this is a field rather than a sentence
    /// in `--help`. A number in a doc string drifts from the constant; this one is the constant.
    ///
    /// Fleet-wide today, because `MAX_BLOB_BYTES` is the protocol's and every node enforces the
    /// same one. When §4's `max_archive_bytes` lands it *tightens* this per node, and the
    /// question the field answers — what will this node take — does not change.
    #[serde(default)]
    pub max_archive_bytes: u64,
    /// Which account this node's runs spend on, as the fingerprint every node compares
    /// (`acct:…`), and where that answer came from (ADR-0028).
    ///
    /// Two facts, because either can be the surprising one. The account is what a bill and a rate
    /// limit are actually about, and until an owner could nominate a state directory there was
    /// nothing to ask — so nothing printed it, on a machine that may well have two logins on it.
    /// The directory is the setting that decided, named for `agent_binary`'s reason: a report
    /// about the wrong login is indistinguishable from a report about the right one.
    #[serde(default)]
    pub agent_account: Option<String>,
    /// When the account's rate limit lifts, if it is holding new runs (ADR-0029).
    ///
    /// The number that answers "the machine is idle and my run has not started", which is
    /// otherwise unanswerable: it is not this device's capacity, not its policy, and not
    /// anything an operator can raise.
    #[serde(default)]
    pub agent_limited_until_unix_ms: Option<u64>,
    #[serde(default)]
    pub agent_state_dir: String,
    pub state_dir: String,
    pub running: u32,
    /// What each run held here is doing, one line each (ADR-0079), for a host that shows it.
    #[serde(default)]
    pub holding: Vec<String>,
    pub max_concurrent: u32,
    /// Shares committed here, and the budget they are spent against (ADR-0013). Two runs is
    /// two runs; whether that is half the machine or all of it is this pair.
    #[serde(default)]
    pub committed_shares: u32,
    #[serde(default)]
    pub budget: u32,
    /// What the machine is actually doing, as a percentage of its cores. `None` where the
    /// platform will not say — which is not the same as idle, and is displayed as such.
    #[serde(default)]
    pub cpu_percent: Option<u8>,
    /// The device's thermal status as its host reports it (ADR-0068), spelled as `Thermal`
    /// displays it. `None` where no host says, which is printed as nothing rather than "none".
    #[serde(default)]
    pub thermal: Option<String>,
    /// Whether this node is keeping its machine from idle sleep, and why (ADR-0077). `None` on a
    /// platform where it cannot, which is printed as nothing.
    #[serde(default)]
    pub sleep: Option<String>,
    /// The approval key the host holds in secure hardware, if it made one (ADR-0069 §4): where
    /// it is, as the host verified it, and whether this node's delegation names it. This device's
    /// own report about itself, printed here and acted on by no peer.
    #[serde(default)]
    pub approval_key: Option<String>,
    /// Why this node would refuse an ordinary run right now, if it would. `None` means it
    /// accepts one.
    pub refusal: Option<String>,
    /// …and the same question asked about **light** work, when the owner has given it a
    /// different answer (ADR-0019 §4). `None` on every device whose policy says nothing about
    /// light work, which is the common case and needs no second line.
    ///
    /// Two fields rather than one sentence because they are two answers, and the pair is the
    /// whole point: measured on a daemon whose owner had written `accept = "never"` with
    /// `[policy.light] accept = "always"`, `offload status` said `accepting no — node is not
    /// accepting work` one command after that node had run a task to completion. A line that
    /// describes the machine has to describe what the machine does.
    #[serde(default)]
    pub light_refusal: Option<LightAnswer>,
    /// Runs stopped waiting for a person to answer a permission question (ADR-0017).
    ///
    /// Here because a blocked run looks like a working one from every other angle: `ps` says
    /// `running`, the lease is being renewed, and the agent is doing nothing at all. A count is
    /// the cheapest place to notice, and `offload asks` is what it points at.
    #[serde(default)]
    pub asks: u32,
    /// Run records held here, and how many are other machines' work (ADR-0025).
    ///
    /// Here for [`Self::asks`]'s reason, one layer down: a store that only grows has no other
    /// symptom, and the machine it grows fastest on is the one nobody logs into. `offload rules`
    /// prints what a rule is keeping and `offload sinks` prints the queue behind a silence — a
    /// node with neither has had nothing to print at all.
    #[serde(default)]
    pub records: u64,
    /// Of [`Self::records`], finished runs this node neither submitted nor ran.
    #[serde(default)]
    pub records_learned: u64,
    /// …and how many of those failed. The only unbounded supply of them, because ADR-0021 §2
    /// keeps every failure and ADR-0024's travelling bit prunes only the completed ones.
    #[serde(default)]
    pub records_learned_failed: u64,
    /// Checkouts on this disk that no run record here accounts for (ADR-0023's residual).
    ///
    /// The one place a directory holding somebody's uncommitted work can end up with nothing
    /// pointing at it: the sweep keeps a checkout whose run it has never heard of, correctly, and
    /// until now said nothing. `worktrees_dir` comes with it, because the useful next command is
    /// `du -sh` on a path.
    #[serde(default)]
    pub stray_checkouts: u64,
    #[serde(default)]
    pub worktrees_dir: String,
    /// What this node offers a run the use of (ADR-0011), and whether each one works.
    ///
    /// Shown for the reason a delivery route is: it is nominated rather than probed, so the only
    /// evidence the owner has that they configured it correctly is a line saying so — and a
    /// resource whose program is missing is advertised anyway, which is the state most worth
    /// being able to see.
    #[serde(default)]
    pub resources: Vec<ResourceStatus>,
    /// What this node can be asked to *run* as a task (ADR-0019 §1), and whether each one works.
    ///
    /// Exactly [`Self::resources`]'s argument, and it was missing for the tier that is a **run**.
    /// A resource whose program is missing had a line on this screen; a task whose program is
    /// missing had none anywhere — not here, not in `offload probe`'s clause, nowhere but one
    /// `WARN` at startup on a machine nobody is logged into. Measured: four nominated kinds with
    /// a broken program, and the task was the only one no report mentioned, while it was the only
    /// one a bid round would place work on.
    #[serde(default)]
    pub tasks: Vec<TaskStatus>,
    /// How the fleet is doing (ADR-0012 mitigation 6). `None` on a node that belongs to none,
    /// which is an ordinary state and not a fleet in poor health.
    #[serde(default)]
    pub fleet: Option<crate::fleet::FleetHealth>,
    /// Datagrams this machine's own kernel refused to send, since this daemon started
    /// (ADR-0059). Empty on every healthy node, and on one with no mesh at all.
    ///
    /// Here because it is the only fact in the system that separates *the peer said nothing*
    /// from *this machine never spoke*, and every other report conflates them: quinn discards a
    /// UDP send error, so the failure detector says `no answer within 500ms` about a node that
    /// was never sent to. It is a count and not a decision — a send error really is non-fatal
    /// and nothing here tears a connection down over one.
    #[serde(default)]
    pub sends_refused: Vec<SendRefusal>,
    /// Of those, the number counted past [`SendRefusal`]'s cap on remembered destinations.
    ///
    /// Both numbers because they can disagree, and the total is the honest one: a node dialling
    /// a long seed list could refuse more than the breakdown can name.
    #[serde(default)]
    pub sends_refused_total: u64,
    /// Peers that refused this node's certificate at a dial, and what they said (ADR-0060).
    ///
    /// [`Self::sends_refused`]'s sibling and the other half of the same silence: that one is
    /// this machine never speaking, this one is this machine speaking and being turned away.
    /// Both reach the failure detector as a peer that did not answer, and a rekeyed-out node
    /// therefore reported its former fleet as `dead` while saying its own fleet was healthy.
    ///
    /// Counted, never acted on: `offload rekey` revokes nobody, so there is no signature that
    /// could make the claim checkable, and a node must not believe a peer about itself.
    #[serde(default)]
    pub turnaways: Vec<Turnaway>,
    /// Of those, the number counted past [`Turnaway`]'s cap on remembered peers.
    #[serde(default)]
    pub turnaways_total: u64,
}

/// One peer that would not let this node in (ADR-0060).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turnaway {
    /// Who refused — an id rather than an address, unlike [`SendRefusal`]'s: a handshake
    /// refusal comes back from something that presented a certificate.
    pub peer: String,
    pub refused: u64,
    /// The peer's own words, unreworded. They name both fleets, which is the diagnosis, and
    /// this node is deliberately not resolving which of them is right.
    pub last: String,
}

/// One destination this node's kernel would not send to (ADR-0059).
///
/// The address rather than a node id, deliberately: the dials that fail this way include seeds
/// nobody has identified yet, and the address is what the next command an operator types takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendRefusal {
    pub destination: String,
    pub refused: u64,
    /// The last thing the kernel said, in its own words — `Operation not permitted`,
    /// `No route to host`. Never reworded: the errno is the thing worth searching for.
    pub last: String,
}

/// What this node would do with light work, when that differs from everything else.
///
/// A three-state answer rather than an `Option<String>`, because "no second answer" and "yes to
/// light work" are different facts and only one of them is a sentence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "light", rename_all = "snake_case")]
pub enum LightAnswer {
    /// It takes light work, whatever it says about the rest.
    Accepted,
    /// It refuses light work too, and why — which may be a different reason.
    Refused { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceStatus {
    /// The service a run names to be granted it.
    pub service: String,
    /// This node's own name for it, which is also what the agent sees the tools prefixed with.
    pub id: String,
    pub access: String,
    /// How it is invoked. This node's own view only, for the reason a sink's is: the owner is
    /// reading about their own machine, and a peer never learns this.
    #[serde(default)]
    pub command: String,
    /// Whether the program is there. Never assumed (the probe's rule).
    pub usable: bool,
    pub description: String,
}

/// One `[[tasks]]` entry, as its owner's own machine reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStatus {
    /// The service a run names with `--task`. Not the block's `id`, which is the commonest
    /// confusion about this tier and is why both are printed.
    pub service: String,
    /// This node's own name for the entry, so the owner can find the block in their config.
    pub id: String,
    /// How it is invoked. This node's own view only, for the reason a resource's is: the owner
    /// is reading about their own machine, and a peer never learns this (ADR-0019 §1).
    pub command: String,
    /// Whether the program is there — the same bit the bid round places on
    /// (`Constraint::ServiceAuthenticated`), read from the capability rather than measured a
    /// second time here.
    pub usable: bool,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: String,
    pub state: String,
    /// Which tier of work this is (`Work::kind`).
    ///
    /// Beside the summary rather than inferred from it, because the summary is a *sentence about
    /// the work* and there is nothing in "webhook --url x" that says it is not a prompt. A
    /// column of its own is what stops the busiest column in `offload ps` from meaning two
    /// different things depending on the row, and it is what makes the empty numeric columns
    /// beside it legible: a task has no turns rather than zero turns.
    ///
    /// The **type** rather than the word, so a reader compares against a variant instead of
    /// spelling `"task"` in a second place. `#[serde(default)]` is `Agent`, which is not a
    /// guess: a daemon that does not send this predates the tier, so every run it has is an
    /// agent run.
    #[serde(default)]
    pub kind: offload_core::WorkKind,
    /// What this run *is*, in one line, truncated for display (`Work::summary`).
    ///
    /// A prompt for an agent run and the service with its arguments for a task. Named `work`
    /// rather than `prompt` since the task tier landed: it was the label as much as the value
    /// that was wrong, and a field called `prompt` invites the next report to print that word
    /// over a shell script's name.
    pub work: String,
    /// The repository this run acts on, or empty for work that has none.
    pub repo: String,
    /// The branch its worktree is on, or empty for work that has no workspace.
    ///
    /// Empty rather than the name a branch *would* have: `run_branch` is a pure function of the
    /// run id and answers for every run, task included, which is how a run that never checks
    /// anything out came to report a branch nobody had created.
    pub branch: String,
    pub turns: u32,
    /// What the conversation has consumed, in tokens (ADR-0040).
    ///
    /// Beside `cost_micro_usd` rather than instead of it, and the pairing is the point: the cost
    /// is the agent's own figure and is absent for any run that did not reach a `result` — a
    /// checkpoint, a drain, a cancel, a turn limit. These come from the transcript, so they are
    /// there for all of those. Tokens rather than a second opinion about dollars, for ADR-0040
    /// §1's reason.
    #[serde(default)]
    pub tokens: offload_core::TokenUse,
    /// Tokens spent by legs whose conversation did not survive — a partition that ran the run
    /// twice (ADR-0067). Zero along any ordinary chain of legs.
    #[serde(default)]
    pub lost_tokens: u64,
    /// The turn limit this run was submitted with, if any (`RunSpec::max_turns`).
    ///
    /// Beside the count rather than folded into a rendered string, so the caller decides how to
    /// show it — and shown at all because a limit nobody can see is indistinguishable from one
    /// that is not being applied, which is the state this field spent seven phases in.
    #[serde(default)]
    pub max_turns: Option<std::num::NonZeroU32>,
    /// Denied permission requests. Surfaced because a silently hobbled run that looks
    /// like a bad model is the worst outcome of ADR-0008's default.
    pub denials: u32,
    pub workspace: String,
    pub cost_micro_usd: u64,
    pub started_at_unix: u64,
    pub error: Option<String>,
    /// Whether this run's checkpoint exists anywhere but here.
    ///
    /// `None` means there is no checkpoint yet. `Some(false)` is the state worth showing a
    /// human: the work is real and it lives on one machine (ADR-0016).
    #[serde(default)]
    pub checkpoint_durable: Option<bool>,
    /// How long this run has left, for a run whose operator said when it is due.
    ///
    /// Slack rather than a timestamp, because slack is the thing that changes on its own and
    /// the thing every decision reads (ADR-0013). `None` for a run with no stated deadline and
    /// for one that has finished.
    #[serde(default)]
    pub due: Option<String>,
    /// For a run held out for its preferred node, which node and for how long (ADR-0063 §7) —
    /// from `RunSpec::holding`, the function placement reads, so the line appears exactly while
    /// the hold is keeping other nodes off it.
    #[serde(default)]
    pub held: Option<String>,
    /// The run this one continues, whole (ADR-0064 §6), for a line that names it.
    #[serde(default)]
    pub continues: Option<String>,
    /// Whether `offload resume` is the next thing somebody would type at this run.
    ///
    /// The state — `Pending` from a checkpoint, `Failed` from an interruption — and the tier,
    /// since a failed **task** is restarted from its spec and never resumed (ADR-0058). Not a
    /// promise that a resume would succeed beyond that: the run may have no session in its
    /// checkpoint, may have spent its turn limit, and the node may have no room. Those are the
    /// door's to say, once, to somebody who typed the command. The tier is here and not left to
    /// the door for the same reason the state is — this flag gates *advice*, and advice pointing
    /// at a command that refuses every run of its kind is not a caveat, it is the wrong command.
    ///
    /// Here so that a reader can pair it with [`Response::Runs::resume_refusal`] **against the
    /// rows it is actually showing**. That is the whole reason it is per run rather than one
    /// flag on the listing: `offload ps` hides finished runs unless asked, so a node-level gate
    /// would print advice about a run that is not on screen. Computed by
    /// `supervisor::resumable_by_hand`, which is the door's own state predicate plus the tier.
    #[serde(default)]
    pub resumable: bool,
    /// Who started it: a person, or a rule or schedule (ADR-0024). For a list deciding what it
    /// may leave out, which is the one thing `Origin` decides: the app hides a rule's successful
    /// tasks, which arrive every minute or so, and never a task somebody started by hand.
    #[serde(default)]
    pub origin: offload_core::Origin,
    /// Which node has it, or last worked on it: the holder while there is one, otherwise the leg
    /// its numbers came from. `None` when nobody has taken it. So a person can see who did the
    /// work, which they asked for from the phone (session ninety-four).
    #[serde(default)]
    pub host: Option<String>,
    /// The model the run asked for, as `--model` takes it; `None` for the agent's own default.
    /// What was asked, not what the agent resolved it to: that is in the run's log.
    #[serde(default)]
    pub model: Option<String>,
}

/// Why a run is where it is — the answer to "why is this still pending" and to "why did
/// nobody move it", which are the same question asked at the two ends of a run's life.
///
/// Every field is a sentence rather than a code, and they are composed here rather than in the
/// CLI for the same reason `NodeSummary` is: the daemon has the view, the names and the
/// policy, and a client that re-derived any of it would be a second implementation of the
/// rules that decide where work goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    pub run: String,
    /// The state machine's own word for it: `pending`, `running`, `orphaned`, …
    pub state: String,
    /// Fleshed out with what the state carries — how long it has been pending, how long its
    /// holder has been out of contact — because the number is the whole question.
    pub state_detail: String,
    /// Parked by `offload checkpoint`: nothing moves it but `offload resume`, so no round is
    /// coming and the canvass is about where a resume *could* go. Defaulted for an older daemon.
    #[serde(default)]
    pub parked: bool,
    /// Which tier of work this is (`Work::kind`).
    ///
    /// Its own line rather than a qualifier on `work`, for the reason `offload ps` gained a
    /// column: the summary is a sentence about the work, and nothing in it says which kind of
    /// thing it describes. This is the label that used to read `prompt` over a shell script's
    /// name.
    #[serde(default)]
    pub kind: offload_core::WorkKind,
    /// What this run *is*, in one line (`Work::summary`) — a prompt, or a service and its
    /// arguments.
    pub work: String,
    pub epoch: u64,
    /// When it is due, said as slack rather than as a time.
    ///
    /// Always present, because every run has an answer: one with no stated deadline is due
    /// *as soon as you can* and this says so, with how long it has been waiting — which is
    /// exactly its urgency (ADR-0013). Slack rather than a timestamp because behaviour here
    /// changes with **no event to point at**: a run that was content to wait becomes one that
    /// migrates aggressively, and the only thing that happened is that time passed.
    #[serde(default)]
    pub due: String,
    /// What this run says it costs a device to host (ADR-0013). Worth showing next to the
    /// refusals: "budget full" on three nodes reads as a broken fleet until you can see that
    /// the run declared itself heavy.
    #[serde(default)]
    pub demand: String,
    /// The other half of the pair that orders runs waiting for a slot (`urgency_order`).
    ///
    /// `due` above is the slack that leads; this is the number that breaks its ties, and it was
    /// shown **nowhere** — `offload priority` echoed the value it had just set and no command
    /// could be asked for it afterwards. The one report that already says a run is *held back*
    /// was therefore missing half of what decides which held run goes next.
    ///
    /// `0` is the default and printing it on every run is noise, so the renderer omits it; a run
    /// somebody has actually prioritised is the case this exists for.
    #[serde(default)]
    pub priority: i32,
    /// Whether anybody is watching, which decides what happens if the run breaks (ADR-0013).
    ///
    /// A sentence rather than a flag, because the interesting case is the third one: a node
    /// explaining a run it does not hold cannot see the stream, and saying "unattended" there
    /// would be a guess dressed as an observation.
    #[serde(default)]
    pub attendance: String,
    /// The node holding it, by name. `None` for a run nobody holds, which is the state most
    /// worth explaining.
    pub holder: Option<String>,
    /// Who decides where this run goes. `None` when this node has never met its home node and
    /// therefore refuses to guess (see `ClusterView::arbiter_for`).
    pub arbiter: Option<String>,
    pub arbiter_is_local: bool,
    /// What the arbiter's supervision pass makes of the run right now.
    pub verdict: String,
    /// What is captured, and whether it exists anywhere but on the holder (ADR-0016).
    pub checkpoint: Option<String>,
    /// Every node's answer, best first. Empty when nobody was asked.
    pub opinions: Vec<NodeOpinion>,
    /// Why the fleet was not asked, when it was not: a finished run, or no mesh to ask.
    pub not_canvassed: Option<String>,
    /// Why a run this node **holds** has not started yet.
    ///
    /// The answer this command exists for, in the one state where it was missing entirely: a run
    /// in `Assigned` got the lease's remaining time and a canvass line saying "already holds this
    /// run", which is true and is not the question. Only the holder can say it — the arithmetic is
    /// this machine's occupancy and its own account's rate limit, neither of which is gossiped —
    /// so it is `None` when explaining somebody else's run, which is honest rather than empty.
    #[serde(default)]
    pub held_back: Option<String>,
    /// What this node will do about a **failed** run, and what it already did.
    ///
    /// `failed` looks identical whether the run is thirty seconds from being picked up, out of
    /// retries, or deliberately left for the person who was watching when it broke — and
    /// ADR-0013's third axis is what decides between them. `None` where this node has no opinion:
    /// the run did not fail here, or it failed while an earlier incarnation was running.
    #[serde(default)]
    pub recovery: Option<String>,
    /// What this run is stopped waiting for a person to answer (ADR-0017).
    ///
    /// The one way a run is `running` and making no progress *by design*, which makes it the
    /// question this command is most often typed for — and the one state it could not see. A
    /// blocked run answered nothing here at all: `running — started 9.0s ago`, an attendance
    /// line, and a canvass saying every node is busy or refused.
    ///
    /// Read from the same registry `offload asks` reads, on the holder, so the two cannot say
    /// different things about one question — and **canvassed from the fleet anywhere else**,
    /// through the round `offload asks` already runs. It used to be empty off the holder, for
    /// [`Self::held_back`]'s reason carried one step further, and that step does not hold: a
    /// blocked process exists on one machine, but a *question* is something that machine can be
    /// asked for. Measured on two daemons: `offload explain` on the peer showed no sign of the
    /// block while `offload asks` on the same socket printed the tool, the clock and the
    /// `offload approve` line — which travels, so the reader of the silent screen could have
    /// answered it from where they stood.
    #[serde(default)]
    pub waiting: Vec<offload_core::PendingAsk>,
    /// Why `offload resume`, typed here, would be refused — regardless of the run.
    ///
    /// [`Self::recovery`] answers what this node will do *unasked*, and is not this: a run left
    /// for a person because nobody could be seen watching is one a person is invited to pick up,
    /// and on a drained, revoked or policy-refusing node that invitation is declined. Both lines
    /// are needed and neither implies the other.
    ///
    /// `None` on a node that would host, which is the ordinary state. The sentence is
    /// [`crate::server::hosting_refusal`]'s, so this report and the door cannot disagree.
    #[serde(default)]
    pub resume_refusal: Option<String>,
}

/// One node's answer to "would you take this run".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeOpinion {
    pub node: String,
    /// "bid 84, starting now", "battery 22%, policy floor is 40%", "no answer".
    pub verdict: String,
    /// Whether this is a bid at all — the difference between a node that would take the run
    /// and one that explained why it would not.
    pub bidding: bool,
    /// Whether this node would win a round held right now. At most one is true, and none are
    /// when nobody bid.
    pub would_win: bool,
}

/// A run's event log lives in `offload-core` now, because a follower may be on another machine
/// and the cluster protocol therefore has to carry these (see `offload_core::event`). Re-exported
/// so this module stays the one place the CLI's protocol is described.
pub use offload_core::{LogEvent, LogKind};

/// The daemon's stamp for a new event. The clock lives here, not in the type (ADR-0001).
#[must_use]
pub fn logged_now(kind: LogKind) -> LogEvent {
    LogEvent::at(
        offload_core::Millis(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        ),
        kind,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        for req in [
            Request::Status,
            Request::List,
            Request::Logs {
                run: "abc".into(),
                follow: true,
            },
            Request::Cancel { run: "abc".into() },
            Request::Explain { run: "abc".into() },
            Request::SetDeadline {
                run: "abc".into(),
                deadline: Some(1_764_000_000_000),
            },
            Request::SetDeadline {
                run: "abc".into(),
                deadline: None,
            },
            Request::Checkpoint { run: "abc".into() },
            Request::Resume {
                run: "abc".into(),
                prompt: None,
            },
            Request::Resume {
                run: "abc".into(),
                prompt: Some("now finish the tests".into()),
            },
            Request::Ask {
                run: "abc".into(),
                epoch: 3,
                tool_use_id: "toolu_01ABC".into(),
                tool: "Bash".into(),
                detail: "cargo build --release".into(),
            },
            Request::Answer {
                run: "abc".into(),
                tool_use_id: Some("toolu_01ABC".into()),
                allow: true,
            },
            Request::Answer {
                run: "abc".into(),
                tool_use_id: None,
                allow: false,
            },
            Request::Asks,
            Request::Submit(SubmitRequest {
                max_turns: None,
                demand: offload_core::Demand::Heavy,
                repo: "/w/repo".into(),
                prompt: "do the thing".into(),
                model: Some("claude-opus-5".into()),
                permission: Some(PermissionMode::Full),
                git_ref: None,
                allow: vec!["Bash(cargo test:*)".into()],
                queue: true,
                deadline: Some(1_764_000_000_000),
                // A tagged enum inside a tagged enum inside a tagged enum, which is exactly
                // the shape that has failed at runtime here before.
                notify: offload_core::Audience::Service {
                    service: offload_core::Service::Push,
                },
                notices: offload_core::Notices::Problems,
                ask: offload_core::AskPolicy::UpTo { questions: 3 },
                resources: Vec::new(),
                origin: offload_core::Origin::Rule,
                require: offload_core::Wanted::default(),
                prefer: offload_core::Wanted::default(),
                hold_until: None,
            }),
        ] {
            let line = serde_json::to_string(&req).expect("encode");
            assert!(!line.contains('\n'), "protocol is line-delimited: {line}");
            assert_eq!(serde_json::from_str::<Request>(&line).expect("decode"), req);
        }
    }

    #[test]
    fn every_response_variant_round_trips() {
        // Request round-tripping was tested from the start and Response was not, so a
        // variant that serde could not encode shipped and only surfaced when `ps` was
        // run against a live daemon. Internally-tagged enums reject newtype variants
        // wrapping sequences — at runtime, with a bare "failed to encode".
        let responses = vec![
            Response::Status(Box::new(NodeStatus {
                node_id: "n".into(),
                name: "laptop".into(),
                device_class: "laptop".into(),
                agent: Some("2.1.220".into()),
                agent_authenticated: true,
                agent_binary: "claude".into(),
                max_archive_bytes: offload_proto::cluster::MAX_BLOB_BYTES,
                agent_max_concurrent: Some(2),
                agent_account: Some("acct:0123456789abcdef".into()),
                agent_limited_until_unix_ms: None,
                agent_state_dir: "/home/x/.claude".into(),
                state_dir: "/s".into(),
                running: 1,
                holding: vec!["add tests for the parser".into()],
                max_concurrent: 2,
                committed_shares: 4,
                budget: 8,
                cpu_percent: Some(12),
                thermal: None,
                sleep: None,
                approval_key: None,
                refusal: None,
                light_refusal: Some(LightAnswer::Accepted),
                asks: 1,
                records: 12,
                records_learned: 7,
                records_learned_failed: 3,
                stray_checkouts: 1,
                worktrees_dir: "/s/worktrees".into(),
                resources: vec![ResourceStatus {
                    service: "email".into(),
                    id: "mail".into(),
                    access: "read".into(),
                    command: "/usr/bin/mail-mcp".into(),
                    usable: true,
                    description: String::new(),
                }],
                tasks: vec![TaskStatus {
                    service: "nightly".into(),
                    id: "nightly".into(),
                    command: "/usr/local/bin/nightly".into(),
                    usable: false,
                    description: "posts the nightly report".into(),
                }],
                fleet: Some(crate::fleet::FleetHealth {
                    fleet: "ab12cd34".into(),
                    members_met: 2,
                    approvers_met: 1,
                    cert_days: 29,
                    revoked_here: false,
                    expiring: vec!["phone in 5 days".into()],
                    reapproval_due: vec![crate::fleet::ApprovalDue {
                        node: offload_core::NodeId::from_bytes([7; 32]),
                        name: "vps-17".into(),
                        days: 12,
                        decided: true,
                    }],
                    passphrase_days: Some(3),
                    revoked: 0,
                    notes: vec!["only one approver".into()],
                }),
                sends_refused: vec![SendRefusal {
                    destination: "192.0.2.240:9000".into(),
                    refused: 7,
                    last: "Operation not permitted (os error 1)".into(),
                }],
                sends_refused_total: 9,
                turnaways: vec![Turnaway {
                    peer: "cf869d9847d6".into(),
                    refused: 3,
                    last: "that certificate is for fleet bf36a64a, this is fleet 433de62b".into(),
                }],
                turnaways_total: 3,
            })),
            Response::Submitted {
                run: "r".into(),
                node: Some("desktop".into()),
                waiting: Some("starting when the run ahead of it finishes".into()),
                here_only: false,
                queued: None,
                audience: Some("news goes to push: phone/toast".into()),
                ask: Some("nothing in this fleet can reach a person".into()),
                preference: Some(
                    "preferred desktop, which did not answer — placed on laptop".into(),
                ),
                siblings: vec!["01a0d4a35ab072d09b79e6bd1676aa75".into()],
            },
            Response::Decision {
                answer: offload_core::Answer::Unanswered,
                reason: "nobody answered in 5m0s".into(),
            },
            Response::Answered {
                tool: "Bash".into(),
                detail: "cargo build".into(),
                allowed: true,
            },
            Response::Asks {
                asks: vec![offload_core::PendingAsk {
                    run: offload_core::RunId::from_bytes([2; 16]),
                    node: Some(offload_core::NodeId::from_bytes([3; 32])),
                    node_name: Some("desktop".into()),
                    tool_use_id: "toolu_1".into(),
                    tool: "Bash".into(),
                    detail: "cargo build".into(),
                    waiting: offload_core::Millis(12_000),
                    left: offload_core::Millis(288_000),
                }],
            },
            Response::Runs {
                runs: vec![RunSummary {
                    tokens: offload_core::TokenUse::default(),
                    lost_tokens: 0,
                    max_turns: None,
                    id: "r".into(),
                    state: "running".into(),
                    kind: offload_core::WorkKind::Agent,
                    work: "p".into(),
                    repo: "/r".into(),
                    branch: "b".into(),
                    turns: 2,
                    denials: 0,
                    workspace: "clean".into(),
                    cost_micro_usd: 100,
                    started_at_unix: 0,
                    error: None,
                    checkpoint_durable: Some(false),
                    due: Some("1h30m left".into()),
                    held: None,
                    continues: None,
                    resumable: false,
                    origin: offload_core::Origin::Operator,
                    host: Some("laptop".into()),
                    model: Some("haiku".into()),
                }],
                resume_refusal: Some("this node is draining".into()),
            },
            Response::Event(crate::api::logged_now(LogKind::TurnBoundary { turn: 1 })),
            Response::Explanation(Box::new(Explanation {
                run: "r".into(),
                state: "orphaned".into(),
                state_detail: "its holder has been out of contact 12.0s".into(),
                parked: false,
                kind: offload_core::WorkKind::Agent,
                work: "add tests for the parser".into(),
                epoch: 3,
                due: "2h14m left".into(),
                demand: "heavy".into(),
                priority: 10,
                attendance: "attended — one client is streaming it".into(),
                holder: Some("laptop".into()),
                arbiter: Some("desktop".into()),
                arbiter_is_local: true,
                verdict: "holding it for up to 33.0s more: …".into(),
                checkpoint: Some("turn 8, on one machine only".into()),
                opinions: vec![NodeOpinion {
                    node: "desktop".into(),
                    verdict: "bid 84, starting now".into(),
                    bidding: true,
                    would_win: true,
                }],
                not_canvassed: None,
                held_back: Some("the account's rate limit is holding new runs".into()),
                waiting: vec![offload_core::PendingAsk {
                    run: offload_core::RunId::from_bytes([7; 16]),
                    node: None,
                    node_name: None,
                    tool_use_id: "toolu_01".into(),
                    tool: "Bash".into(),
                    detail: "rm -rf build".into(),
                    waiting: offload_core::Millis(12_000),
                    left: offload_core::Millis(288_000),
                }],
                recovery: Some("picking it up again in 24.0s (try 2)".into()),
                resume_refusal: Some("this node is draining".into()),
            })),
            Response::SpecEdited {
                run: "r".into(),
                summary: "2h left".into(),
                note: "it is pending, so it is offered again".into(),
            },
            Response::Done,
            Response::Error {
                message: "e".into(),
            },
        ];

        for response in responses {
            let line = serde_json::to_string(&response)
                .unwrap_or_else(|e| panic!("cannot encode {response:?}: {e}"));
            assert!(!line.contains('\n'), "protocol is line-delimited: {line}");
            assert_eq!(
                serde_json::from_str::<Response>(&line).expect("decode"),
                response
            );
        }
    }

    #[test]
    fn log_events_round_trip_and_flatten_cleanly() {
        let ev = crate::api::logged_now(LogKind::TurnBoundary { turn: 3 });
        let line = serde_json::to_string(&ev).expect("encode");
        // Flattened, so the log reads as one object per line rather than a nested wrapper.
        assert!(line.contains(r#""event":"turn_boundary""#), "{line}");
        assert!(line.contains(r#""turn":3"#), "{line}");
        assert_eq!(serde_json::from_str::<LogEvent>(&line).expect("decode"), ev);
    }

    #[test]
    fn kind_names_match_the_serialised_tag() {
        // The `kind` column is queried by hand (`where kind = 'turn_boundary'`), so it
        // must agree with what the JSON says or the two views of the log disagree.
        for kind in [
            LogKind::Text { text: "x".into() },
            LogKind::TurnBoundary { turn: 1 },
            LogKind::Cancelled {
                by: "laptop".into(),
            },
            LogKind::Failed { reason: "r".into() },
        ] {
            let event = crate::api::logged_now(kind);
            let json = serde_json::to_value(&event).expect("encode");
            assert_eq!(
                json.get("event").and_then(serde_json::Value::as_str),
                Some(event.kind_name())
            );
        }
    }

    #[test]
    fn a_released_checkpoint_ends_the_stream_but_a_routine_one_does_not() {
        // `logs -f` stops on a terminal event. A released checkpoint kills the agent
        // process, so a follower that kept waiting would hang on a dead stream; a per-turn
        // checkpoint is just a marker and the run keeps talking.
        assert!(crate::api::logged_now(LogKind::Checkpointed {
            turn: 3,
            summary: "1 KiB patch".into(),
            released: true,
        })
        .is_terminal());
        assert!(!crate::api::logged_now(LogKind::Checkpointed {
            turn: 3,
            summary: "1 KiB patch".into(),
            released: false,
        })
        .is_terminal());
    }

    #[test]
    fn terminal_events_are_recognised() {
        assert!(crate::api::logged_now(LogKind::Cancelled {
            by: "laptop".into()
        })
        .is_terminal());
        assert!(crate::api::logged_now(LogKind::Failed {
            reason: "boom".into()
        })
        .is_terminal());
        assert!(crate::api::logged_now(LogKind::Finished {
            success: true,
            turns: 1,
            denials: 0,
            cost_micro_usd: 0,
            work: offload_core::WorkKind::Agent,
            result: None,
        })
        .is_terminal());
        assert!(!crate::api::logged_now(LogKind::TurnBoundary { turn: 1 }).is_terminal());
    }

    #[test]
    fn multiline_agent_text_stays_on_one_protocol_line() {
        // Agent output routinely contains newlines. If encoding leaked one, the reader
        // would resynchronise on a partial object and the log would corrupt from there.
        let ev = crate::api::logged_now(LogKind::Text {
            text: "line one\nline two\n".into(),
        });
        let line = serde_json::to_string(&ev).expect("encode");
        assert!(!line.contains('\n'));
        assert_eq!(serde_json::from_str::<LogEvent>(&line).expect("decode"), ev);
    }
}
