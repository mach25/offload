//! The run state machine, including epoch fencing.
//!
//! Every transition that could result in work happening is guarded by `(holder, epoch)`.
//! Epoch is the fencing token: it increases on every assignment, and a node presenting a
//! stale epoch is refused. This is what makes it safe to reassign a run whose holder we only
//! *believe* is gone — the belief can be wrong without causing two agents to run.
//!
//! Note [`RunState::Orphaned`]: losing contact with a holder is deliberately not the same
//! event as deciding to move the run. See `docs/adr/0007-node-dropoff.md`.

use crate::capability::AgentKind;
use crate::id::{BlobHash, NodeId, RunId};
use crate::time::Millis;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Fencing token. Monotonic per run; bumped on every assignment.
///
/// `Default` is `Epoch(0)`, the number a run carries before anybody has been granted it — which
/// is also what a stored record written before [`RunProgress`](crate::RunProgress) carried one
/// decodes as, and is therefore the answer that makes an unstamped record lose to a stamped one.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Epoch(pub u64);

impl Epoch {
    pub const INITIAL: Epoch = Epoch(0);

    #[must_use]
    pub const fn next(self) -> Epoch {
        Epoch(self.0 + 1)
    }
}

impl std::fmt::Display for Epoch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "e{}", self.0)
    }
}

/// A node's time-bounded right to hold a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub node: NodeId,
    pub epoch: Epoch,
    pub expires_at: Millis,
}

impl Lease {
    #[must_use]
    pub fn is_expired(&self, now: Millis) -> bool {
        now >= self.expires_at
    }
}

/// How long a node holds a run before it has to say so again.
///
/// **One number, for every way a run can be taken up.** There were two: a run granted by a bid
/// round leased for a minute, and a run submitted locally leased for an hour — the latter set
/// when there was no arbiter to answer to and left behind when phase 4 gave it one. Nothing
/// was broken by that (the failure detector is what notices a dead holder, and a long lease
/// errs toward a stalled run rather than a duplicated one), but two runs on one machine
/// answering "how long may I hold this" differently is the kind of difference nobody
/// remembers when reasoning about fencing at three in the morning.
///
/// The value is a compromise between two costs that pull in opposite directions. Too short and
/// a healthy holder whose heartbeat is momentarily late has its run declared `Orphaned` by its
/// arbiter; too long and a run whose holder vanished waits that much longer for anyone to be
/// allowed to notice. A minute, renewed every few seconds, leaves an order of magnitude of
/// slack for the first and costs the second nothing that the hold-down does not already spend.
///
/// Anything handing out a lease has to arrange for its heartbeat in the same breath — the
/// whole of the reason this is a *lease* and not a flag.
pub const LEASE: Millis = Millis(60_000);

/// What we are allowed to do when a run has to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Restartability {
    /// Move the transcript and workspace; resume mid-conversation. Default for agent runs.
    Resumable,
    /// Safe to re-run from the original prompt on a clean workspace. No state moves.
    Idempotent,
    /// Cannot move. Fails if its holder goes away.
    Pinned,
}

/// How much autonomy the agent has. Conservative by default — this is a real security
/// control, not metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    #[default]
    Ask,
    AcceptEdits,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    /// Clone URL or local path. Resolved by the holding node, which may already have it.
    pub repo: String,
    /// How big the workspace archive is, when `repo` names one (ADR-0061 §5).
    ///
    /// Carried in the spec so a node can bid **against** it: the alternative is discovering the
    /// size by fetching, which is the expensive step the whole clause exists to put a number in
    /// front of. `None` for every ordinary repository, and for an archive submitted by something
    /// that did not know to say — in which case the node cannot refuse on size and finds out at
    /// acquisition, which is correct and merely slower.
    ///
    /// Not a wire version: the `archive:` spelling is its own gate. A node too old to know the
    /// form parses it as a path, finds nothing of that name, and declines to bid — so this field
    /// only ever travels between nodes that both understand it.
    #[serde(default)]
    pub archive_bytes: Option<u64>,
    /// Ref to start from. `None` means the repo's default branch.
    pub git_ref: Option<String>,
    /// Branch the agent works on. `None` means one is minted from the run id.
    pub branch: Option<String>,
}

/// What a run actually *does*, and the axis everything else in a spec is shared across.
///
/// **A tagged enum, never optional fields beside each other.** The kind of work is declared by
/// the submission and is never inferred from a missing repository — an `Option<WorkspaceSpec>`
/// read as "then it is not an agent run" is inference wearing a type, and one field that is two
/// facts is the mistake this project has now fixed three times: `LogKind::Failed` borrowed for a
/// failed capture, one signal carrying both "stop the agent" and "the run is over", and a run's
/// *position* sharing a merge rule with its *spend* (ADR-0005's second amendment). An enum makes
/// the impossible combinations impossible rather than merely unwritten (ADR-0019 §2).
///
/// Each variant holds a named struct rather than spelling its fields inline. That is the ADR's
/// decision with the ergonomics it did not need to specify: the agent half is eight fields that
/// travel together everywhere below `offload-core`, and passing `&AgentWork` is what stops them
/// being eight arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Work {
    /// The expensive tier: a model, a prompt and a workspace.
    Agent(AgentWork),
    /// The cheap middle tier: a program the node's owner nominated (ADR-0019 §1).
    ///
    /// **It runs.** `[[tasks]]` nominates one, `offload run --task <service>` submits it,
    /// `bid::evaluate` bids for it like any other run, and `Supervisor::drive_task` executes
    /// it. What is still owed is a *schedule* to fire one (ADR-0019 §3) and the demand axis on
    /// the owner's gates (§4).
    ///
    /// Three paths are agent-only and refuse with a reason rather than a default —
    /// `prepare_start`, `resume` and `drive`, which are the two ends of a conversation and the
    /// loop between them — and that is `SubmitError::UnsupportedWork`. A task reaches none of
    /// them: `start_run` dispatches to `drive_task` above all three, and there is no transcript
    /// to resume from.
    Task(TaskWork),
}

/// An agent run: what to run, on what, and how much of itself it may spend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentWork {
    pub agent: AgentKind,
    pub model: Option<String>,
    pub prompt: String,
    pub workspace: WorkspaceSpec,
    pub permission_mode: PermissionMode,
    /// Tools this run may use without prompting, on top of `permission_mode`.
    ///
    /// The point of the pairing: `AcceptEdits` plus `Bash(cargo test:*)` lets an agent
    /// write code and run the tests, without the unrestricted execution `Full` grants.
    pub allow: crate::ToolAllowlist,
    /// The most turns this run may take, over its whole life rather than over one leg of it.
    ///
    /// A budget on the *work*, and the only bound in a spec that stops an agent that is
    /// making progress. Everything else here either places a run or decides what it may
    /// reach; this decides when it has had enough — which is the bound worth having when the
    /// thing being bounded is a model deciding for itself how many turns the task deserves.
    ///
    /// **Counted cumulatively, which is what makes it a budget and not a per-leg allowance.**
    /// Turns are already offset across legs by the supervisor so a resumed agent's turn 1 is
    /// the run's turn 7, and this reads that number: a run that migrates four times still gets
    /// the turns its submitter agreed to, rather than a fresh allowance from every node it
    /// lands on. Exactly [`crate::RunProgress::asks`]'s argument, applied to the other budget.
    ///
    /// **`None` is no limit**, which is what every run has had until now and stays the default:
    /// an agent stops when it is done, and most runs should.
    ///
    /// [`NonZeroU32`](std::num::NonZeroU32) rather than a `u32` that callers must remember to
    /// check, for `WorkPolicy::allowed_agents`' reason: zero reads as a limit being set while
    /// meaning "take no turns at all", which is a run not worth submitting said in a way
    /// nothing else here understands. The type refuses it at deserialize, so no path can
    /// forget to — including a rule stored months ago and fired tonight.
    #[serde(default)]
    pub max_turns: Option<std::num::NonZeroU32>,
    /// May this run stop and ask a person for permission, and how often (ADR-0017)?
    ///
    /// [`AskPolicy::Never`] is what has always happened: a tool call the run does not already
    /// permit is denied headless, recorded as a denial, and the run carries on having done less
    /// than it was asked.
    ///
    /// [`AskPolicy::UpTo`] says the opposite is preferable — *stop and ask me* — and it is opt-in
    /// because it costs something real: the question is asked mid-turn, so the run is stalled,
    /// undrainable and holding a lease while it waits (see [`Run::approval_patience`]). A separate
    /// axis from [`RunSpec::permission_mode`] because it answers a different question: the mode
    /// says what needs permission, and this says what happens when something does — which is also
    /// why the *number* matters, since the two modes gate wildly different amounts of a run.
    ///
    /// On the spec, and travels with the run, so a migrated run can still ask. The count of what
    /// it has already spent travels beside it, on [`RunProgress`](crate::RunProgress).
    #[serde(default)]
    pub ask: crate::ask::AskPolicy,
}

/// A task: a program the owner nominated, addressed by service (ADR-0019 §1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskWork {
    /// *Which* nominated program, by service — never a command line.
    ///
    /// The submitter says `watch-api`; the node's own config says what that runs. Third
    /// application of the rule sinks and resources already follow: the outbound access is the
    /// **owner's** grant, so a submitter cannot name a program the owner did not nominate. By
    /// service rather than by capability id for [`crate::Audience`]'s reason — an id is a node's
    /// own name for one of its own things, so naming one would pin the run to a device.
    pub service: crate::Service,
    /// Arguments appended to what the owner nominated. Never the program itself.
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunSpec {
    /// What this run does. Everything else here is shared across both kinds of work.
    pub work: Work,
    /// What the run needs from a node. Combined with the node's own policy at bid time.
    pub constraint: crate::constraint::Constraint,
    pub restartability: Restartability,
    /// Higher wins when nodes have capacity for only one. Purely advisory.
    pub priority: i32,
    /// May the fleet leave this run pending and place it later?
    ///
    /// `false` — the default, and ADR-0014's whole point — means a submission is accepted by
    /// somebody while the operator is still at the keyboard, or refused to their face with
    /// every node's reason. There is no third outcome, because "filed away, good luck" is the
    /// babysitting this project exists to remove.
    ///
    /// `true` is the deliberate opt-in: *I know nobody can take it now; leave it pending and
    /// let somebody pick it up when things change.* Legitimate — the desktop is off and will
    /// be on in the morning — and it is precisely the case where the operator has chosen to
    /// check back, which is why it has to be asked for rather than given by default.
    ///
    /// It is on the spec rather than the submission because it has to survive: the node that
    /// eventually places the run may not be the one that took the request, and a run nobody
    /// remembers was queueable is a run nobody retries.
    #[serde(default)]
    pub queue: bool,
    /// When this run needs to be done. **`None` means the moment it was submitted**, not "no
    /// hurry" — see [`Run::due_at`] and [`crate::deadline`].
    ///
    /// The only mutable field in a spec that is otherwise immutable (ADR-0013): changing your
    /// mind about when you need something is ordinary, and the run that was fine overnight is
    /// the one blocking a demo. Mutability is why it needs an owner and a revision counter of
    /// its own, which is [`Run::deadline_rev`].
    #[serde(default)]
    pub deadline: Option<Millis>,
    /// What hosting this is expected to cost a device (ADR-0013). `Normal` unless somebody
    /// says otherwise, which is the honest default for an agent run: what it will actually do
    /// is the model's decision, not the submitter's.
    ///
    /// A *hint*, and never a requirement — hardware a run cannot work without belongs in
    /// [`RunSpec::constraint`], which fails loudly and names itself. Getting demand wrong means
    /// the run lands somewhere suboptimal; getting a constraint wrong means it cannot run.
    #[serde(default)]
    pub demand: crate::capacity::Demand,
    /// Who this run's news is for (ADR-0010). `Everyone` unless somebody says otherwise, which
    /// is the honest default for a fleet whose devices all belong to one person.
    ///
    /// Here rather than in the submission for [`RunSpec::queue`]'s reason: the node that ends up
    /// delivering the news is usually not the node that took the request, so an audience that did
    /// not travel with the run would be an audience a migration silently widened. And *not* in
    /// [`SpecEdit`] — two editable fields share one counter on purpose, and a third belongs to an
    /// ADR rather than to whoever needs it first.
    #[serde(default)]
    pub notify: crate::notify::Audience,
    /// *Which* of this run's news is worth interrupting somebody for (ADR-0026).
    ///
    /// [`RunSpec::notify`]'s missing other half. That one selects routes and this one selects
    /// kinds, and with only the first there were exactly two settings for a standing instruction:
    /// tell me every three seconds that nothing happened, or tell me nothing including the
    /// failures. Beside `notify` rather than folded into it because they compose — "reach me by
    /// push, and only when something is wrong" is one sentence with two answers in it.
    ///
    /// Here for `notify`'s reason, verbatim: the node that delivers is usually not the node that
    /// took the request, so a selection that did not travel would be one a migration silently
    /// widened. Not in [`SpecEdit`], for the same reason as `notify`.
    #[serde(default)]
    pub notices: crate::notify::Notices,
    /// Resources this run has been granted, by service (ADR-0011).
    ///
    /// The only role that hands a run something, so it is the only one that has to be granted
    /// rather than implied by placement: a node holding a mailbox does not mean every run that
    /// lands there may read mail. Layered exactly like [`RunSpec::allow`] and read at the same
    /// two moments — placement, and the moment the agent is spawned.
    ///
    /// By **service**, never by capability id, for [`crate::Audience`]'s reason: an id is a
    /// node's own name for one of its own things, so naming one would pin the run to a device.
    /// "Reach email" is a grant; "the desktop's `mail-mcp` script" is a machine.
    ///
    /// Empty is the overwhelmingly common case and means what it has always meant: the run
    /// reaches nothing beyond its own worktree.
    #[serde(default)]
    pub resources: Vec<crate::Service>,
    /// Where this run would rather be, **scored and never filtered** (ADR-0063 §1).
    ///
    /// The same tree and the same `explain` as [`RunSpec::constraint`], read by the other half
    /// of a bid: a node that satisfies it adds `BidWeights::preferred` to its score, and one that
    /// does not adds nothing and is not refused. `Always` — the default and every stored run's —
    /// adds nothing to anybody, so a run that states no preference scores exactly as before.
    ///
    /// Usually a [`Constraint::Node`](crate::Constraint::Node), resolved when the run was
    /// submitted: a preference for *here* has to go on meaning the machine it was typed at.
    #[serde(default)]
    pub prefer: crate::constraint::Constraint,
    /// Until when [`RunSpec::prefer`] is a **requirement** rather than a preference (ADR-0063 §4).
    ///
    /// Only ever a time somebody stated — an implied one is the unspecified-deadline trap — and
    /// only with `queue`, because a run held for an absent desktop has nobody to bid at
    /// submission. [`RunSpec::eligibility`] is the one reading of it; placement and every report
    /// ask that, never this field.
    #[serde(default)]
    pub hold_until: Option<Millis>,
    /// The finished run this one continues, and what it was handed (ADR-0064 §1).
    ///
    /// Immutable, set at submission and owned by the submitter the way `Origin` is, so it gossips
    /// cleanly. `None` for every ordinary run and every row written before continuations existed.
    #[serde(default)]
    pub parent: Option<Continuation>,
}

/// What a continuation carries about the run it continues (ADR-0064).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Continuation {
    /// The parent, whole — a continuation names exactly one run, and a prefix is a display.
    pub run: RunId,
    pub mode: ContinueMode,
    /// The parent's transcript, as the agent wrote it, stored as a blob and materialised at
    /// `.offload/parent/transcript.jsonl` wherever this run starts (§3). Naming it here is what
    /// keeps it past the collector — `referenced_blobs` asks every run for it — and only for the
    /// runs somebody chose to continue. `None` when neither the parent's checkpoint nor the
    /// agent's own file could be found on the parent's node, which the first log row says.
    #[serde(default)]
    pub transcript: Option<crate::id::BlobHash>,
    /// What the parent was asked, verbatim — the first of the two strings a handoff moves.
    #[serde(default)]
    pub parent_prompt: String,
    /// The parent's own closing words, verbatim and capped (`LogKind::Finished::result`), or
    /// `None` when it closed with none — which the continuation's first log row says in words,
    /// wherever it runs, because that is the thin case §6 insists be loud.
    ///
    /// Here rather than folded into the prompt, so that the spec's prompt stays *what the
    /// continuer typed* — `offload ps` and the `submitted` row print it — and the handoff heading
    /// is composed at spawn from these two fields by one function.
    #[serde(default)]
    pub closing_message: Option<String>,
}

/// What only the parent's own node can produce for a continuation: the parent's work, captured
/// as a starting checkpoint, and the two things it left behind (ADR-0064 §2).
///
/// Built where the parent's last leg ran, because that is where its worktree is, and sent back
/// to the node the continuation was typed at, which makes the run itself — so that its home,
/// and a `--prefer here`, are the machine the person is at rather than the one that happened to
/// hold the parent's checkout. The blobs it names are on the building node when it answers; the
/// asker fetches them before the run exists, so no collector anywhere can take them from under a
/// run that names them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationBase {
    /// At `turns: 0`, which is how [`Run::continuation_base`] recognises it.
    pub checkpoint: Checkpoint,
    /// [`Continuation::transcript`].
    pub transcript: Option<crate::id::BlobHash>,
    /// [`Continuation::closing_message`].
    pub closing_message: Option<String>,
    /// What the capture held, in the words the building node's log used — for the asker's log.
    pub summary: String,
}

impl ContinuationBase {
    /// Every blob the base names, so the asker can fetch each before the run exists.
    #[must_use]
    pub fn blobs(&self) -> Vec<crate::id::BlobHash> {
        let mut blobs = vec![self.checkpoint.transcript];
        blobs.extend(self.checkpoint.bundle);
        blobs.extend(self.checkpoint.patch);
        blobs.extend(self.transcript);
        blobs.sort_unstable();
        blobs.dedup();
        blobs
    }
}

/// How much of the parent's conversation a continuation starts with (ADR-0064 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinueMode {
    /// A fresh session, handed the parent's prompt and closing message, with its transcript as a
    /// file to read if it needs to. The default: an empty context, portable across agent
    /// versions and accounts.
    Handoff,
    /// The parent's conversation resumed and forked — the reasoning as live context.
    Session,
}

impl std::fmt::Display for ContinueMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ContinueMode::Handoff => "handoff",
            ContinueMode::Session => "session",
        })
    }
}

/// Where a continuation's parent transcript is put in its worktree, relative to the root.
pub const PARENT_TRANSCRIPT: &str = ".offload/parent/transcript.jsonl";

/// Accepts both the shape written today and the flat one written before the `Work` split.
///
/// **Not politeness — a node's own stored state.** A whole `Run` is one JSON blob in
/// `runs.run_json`, and a rule keeps a whole `RunSpec` too, so a node that upgrades reads back
/// rows written by the version before it. Without this, every run and every rule already on
/// disk becomes `StoreError::Decode` and the daemon comes up having lost them. The wire needs
/// no such thing: `MIN_VERSION` tracks `VERSION`, so nodes at different wire versions refuse
/// each other at the handshake rather than exchanging a spec neither can read.
///
/// The two shapes are told apart by the presence of `work`, which is the only field the old
/// form never had. Nothing here guesses: a document with neither `work` nor `agent` is an error
/// in both dialects and is reported as one.
impl<'de> Deserialize<'de> for RunSpec {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            // The current shape.
            work: Option<Work>,
            // The pre-split shape, flat. Every one of these is optional *here* and required by
            // `AgentWork`, so a half-written old document fails rather than being filled in.
            agent: Option<AgentKind>,
            #[serde(default)]
            model: Option<String>,
            prompt: Option<String>,
            workspace: Option<WorkspaceSpec>,
            permission_mode: Option<PermissionMode>,
            #[serde(default)]
            allow: crate::ToolAllowlist,
            #[serde(default)]
            max_turns: Option<std::num::NonZeroU32>,
            #[serde(default)]
            ask: crate::ask::AskPolicy,

            constraint: crate::constraint::Constraint,
            restartability: Restartability,
            priority: i32,
            #[serde(default)]
            queue: bool,
            #[serde(default)]
            deadline: Option<Millis>,
            #[serde(default)]
            demand: crate::capacity::Demand,
            #[serde(default)]
            notify: crate::notify::Audience,
            #[serde(default)]
            notices: crate::notify::Notices,
            #[serde(default)]
            resources: Vec<crate::Service>,
            #[serde(default)]
            prefer: crate::constraint::Constraint,
            #[serde(default)]
            hold_until: Option<Millis>,
            #[serde(default)]
            parent: Option<Continuation>,
        }

        let wire = Wire::deserialize(deserializer)?;
        let work = match wire.work {
            Some(work) => work,
            None => Work::Agent(AgentWork {
                agent: wire
                    .agent
                    .ok_or_else(|| serde::de::Error::missing_field("work"))?,
                model: wire.model,
                prompt: wire
                    .prompt
                    .ok_or_else(|| serde::de::Error::missing_field("prompt"))?,
                workspace: wire
                    .workspace
                    .ok_or_else(|| serde::de::Error::missing_field("workspace"))?,
                permission_mode: wire
                    .permission_mode
                    .ok_or_else(|| serde::de::Error::missing_field("permission_mode"))?,
                allow: wire.allow,
                max_turns: wire.max_turns,
                ask: wire.ask,
            }),
        };
        Ok(RunSpec {
            work,
            constraint: wire.constraint,
            restartability: wire.restartability,
            priority: wire.priority,
            queue: wire.queue,
            deadline: wire.deadline,
            demand: wire.demand,
            notify: wire.notify,
            notices: wire.notices,
            resources: wire.resources,
            prefer: wire.prefer,
            hold_until: wire.hold_until,
            parent: wire.parent,
        })
    }
}

/// Which tier of work a record is about, as a value something can carry.
///
/// Beside [`Work`] rather than inside it because it is the part of the answer that outlives the
/// work: a run's `Finished` event and the notification projected from it are read long after
/// the spec they came from, and both were saying `finished after 0 turn(s), $0.0000` about a
/// shell script. A tier is what tells a reader whether the numbers next to it mean anything.
///
/// [`Default`] is `Agent`, and it is not a guess: a row written before the task tier existed
/// was written by a binary in which an agent was the only thing a run could be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    #[default]
    Agent,
    Task,
}

impl WorkKind {
    /// The word for it, for a column or a sentence.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            WorkKind::Agent => "agent",
            WorkKind::Task => "task",
        }
    }
}

impl std::fmt::Display for WorkKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `pad`, not `write_str`: a `Display` written with `write_str` silently ignores the
        // width in `{:<5}`, and `offload ps` prints this **in a column**. Measured on a daemon
        // with one row of each tier — `task` is a character shorter than `agent`, so every
        // numeric column to its right sat one place left on task rows and the table stopped
        // lining up the moment this stopped being a `String`.
        f.pad(self.name())
    }
}

impl Work {
    /// The agent half, or `None` for a task.
    #[must_use]
    pub fn agent(&self) -> Option<&AgentWork> {
        match self {
            Work::Agent(agent) => Some(agent),
            Work::Task(_) => None,
        }
    }

    /// The task half, or `None` for an agent run.
    #[must_use]
    pub fn task(&self) -> Option<&TaskWork> {
        match self {
            Work::Task(task) => Some(task),
            Work::Agent(_) => None,
        }
    }

    /// One line describing what this work *is*, for a summary row.
    ///
    /// The agent tier's answer is its prompt, which is what `offload ps` has always shown. The
    /// task tier's is the service and its arguments, because that is the whole of what a task
    /// is — there is no prompt to show and an empty column would read as a missing one. When
    /// the task tier lands, `offload ps` wants a **kind** column beside this rather than a
    /// cleverer string in it; see `docs/ROADMAP.md`'s phase 8.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Work::Agent(agent) => agent.prompt.clone(),
            Work::Task(task) if task.args.is_empty() => task.service.to_string(),
            Work::Task(task) => format!("{} {}", task.service, task.args.join(" ")),
        }
    }

    /// Which tier of the owner's policy this work is asked about under (ADR-0019).
    ///
    /// The one place the mapping is made, so no caller has to decide what a task means to a
    /// gate: `WorkPolicy` asks three questions that only an agent run can answer, and this is
    /// what tells it which it may ask.
    #[must_use]
    pub fn tier(&self) -> crate::policy::Tier<'_> {
        match self {
            Work::Agent(agent) => crate::policy::Tier::Agent(&agent.agent),
            Work::Task(_) => crate::policy::Tier::Task,
        }
    }

    /// Which of the two tiers this is, for a report, a log field or a refusal.
    #[must_use]
    pub fn kind(&self) -> WorkKind {
        match self {
            Work::Agent(_) => WorkKind::Agent,
            Work::Task(_) => WorkKind::Task,
        }
    }
}

/// Why a submitted spec was refused before anything was placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpecProblem {
    #[error(
        "a task cannot be `resumable`: resuming means resuming a conversation, and a task has \
         no transcript. Use `idempotent` (safe to run again from the start) or `pinned` (does \
         not move)."
    )]
    TaskCannotResume,
    /// ADR-0063 §4: holding out for a preferred node means nobody may be willing at submission,
    /// and ADR-0014's answer to nobody is a refusal to the operator's face.
    #[error("a run held for its preferred node has to be allowed to wait — add `--queue`")]
    HoldWithoutQueue,
    #[error("`--hold` holds out for a preferred node, and this run prefers none — add `--prefer`")]
    HoldWithoutPreference,
    /// Waiting past the point somebody said waiting was pointless.
    #[error(
        "the hold ends after the run's deadline, so the fleet would be asked to wait for the \
         preferred node past the moment the work was needed — move one of them"
    )]
    HoldPastDeadline,
    /// An agent run naming no workspace at all (ADR-0072). It used to be accepted and fail on
    /// whichever node took it, at `git clone ''`, a minute later and somewhere else.
    #[error(
        "an agent run needs a workspace: a repository, an archive (`--archive`), or an empty \
         one (`--scratch`)"
    )]
    NoWorkspace,
    /// A start point in a repository that has no history to have one in.
    #[error("a scratch workspace starts empty, so it has no branch or commit to start from — drop `--ref`")]
    ScratchWithRef,
}

impl RunSpec {
    /// The agent half of this spec, or `None` if it is a task.
    ///
    /// Most callers below `offload-core` are agent-only paths that cannot proceed without one,
    /// and they say so by asking here and failing with a reason rather than by reading fields
    /// that a task legitimately does not have.
    #[must_use]
    pub fn agent(&self) -> Option<&AgentWork> {
        self.work.agent()
    }

    /// The agent half, mutably — the one part of a spec a leg may add to (its allowlist).
    #[must_use]
    pub fn agent_mut(&mut self) -> Option<&mut AgentWork> {
        match &mut self.work {
            Work::Agent(agent) => Some(agent),
            Work::Task(_) => None,
        }
    }

    /// The task half of this spec, or `None` if it is an agent run.
    #[must_use]
    pub fn task(&self) -> Option<&TaskWork> {
        self.work.task()
    }

    /// Which agent this run needs, or `None` for work that needs none.
    ///
    /// Read by the counting questions — how many runs on this node share an agent, what an
    /// account is already carrying — where the honest answer for a task is that it shares
    /// nothing, because it has no account and no rate limit to share (ADR-0019 §2). That is a
    /// real zero rather than an unknown one, which is why those callers may take `0` for
    /// `None` without breaking the rule that unknown is not none.
    #[must_use]
    pub fn agent_kind(&self) -> Option<&AgentKind> {
        self.agent().map(|work| &work.agent)
    }

    /// The workspace this run acts on — `None` for a task, which has none at all.
    ///
    /// A task has no worktree rather than an empty one (ADR-0019 §2), which is why this is an
    /// `Option` at the *accessor* and not a field on the spec: the absence is a consequence of
    /// the kind of work, not a thing a submission may leave out.
    #[must_use]
    pub fn workspace(&self) -> Option<&WorkspaceSpec> {
        self.agent().map(|work| &work.workspace)
    }

    /// Is this spec self-consistent, and if not, why not (ADR-0019 §2)?
    ///
    /// One rule so far, and it is refused at *submission* rather than discovered at migration.
    /// [`Restartability::Resumable`] means "resume mid-conversation", and a task has no
    /// transcript for that to refer to — so a task that claimed it would be placed, moved, and
    /// only then found to be unmovable, which is the mistake open question #4 made about
    /// `Portability::NodeLocal`. A reason rather than a bool, because every user-facing
    /// question here is *why did that not happen*.
    pub fn check(&self) -> Result<(), SpecProblem> {
        if matches!(self.work, Work::Task(_)) && self.restartability == Restartability::Resumable {
            return Err(SpecProblem::TaskCannotResume);
        }
        if let Some(workspace) = self.workspace() {
            if workspace.repo.trim().is_empty() {
                return Err(SpecProblem::NoWorkspace);
            }
            if workspace.git_ref.is_some()
                && crate::RepoSource::parse(&workspace.repo) == crate::RepoSource::Scratch
            {
                return Err(SpecProblem::ScratchWithRef);
            }
        }
        if let Some(until) = self.hold_until {
            if self.prefer == crate::constraint::Constraint::Always {
                return Err(SpecProblem::HoldWithoutPreference);
            }
            if !self.queue {
                return Err(SpecProblem::HoldWithoutQueue);
            }
            if self.deadline.is_some_and(|deadline| until > deadline) {
                return Err(SpecProblem::HoldPastDeadline);
            }
        }
        Ok(())
    }

    /// What a node has to satisfy to be **eligible** for this run at `now` (ADR-0063 §4).
    ///
    /// The requirement, with the preference ANDed in while a stated hold lasts and not after. A
    /// pure function of `(spec, now)` — no stored state, no clock — so every node and every
    /// report computes the same answer, and the hold ends at the same instant everywhere without
    /// anybody saying so. Placement and every report read this and never `constraint` directly.
    #[must_use]
    pub fn eligibility(&self, now: Millis) -> std::borrow::Cow<'_, crate::constraint::Constraint> {
        if self.holding(now) {
            std::borrow::Cow::Owned(self.constraint.clone().and(self.prefer.clone()))
        } else {
            std::borrow::Cow::Borrowed(&self.constraint)
        }
    }

    /// Is the preference still a requirement at `now`? The one reading of `hold_until`.
    #[must_use]
    pub fn holding(&self, now: Millis) -> bool {
        self.hold_until.is_some_and(|until| now < until)
            && self.prefer != crate::constraint::Constraint::Always
    }

    /// Has this run spent the turns it was given, having taken `turns` of them?
    ///
    /// `Some(limit)` is the whole of the enforcement, and it carries the cap because every
    /// caller has to *say* it: "left for a person" and "it reached its turn limit" are the same
    /// decision, and a report computed from a second copy of this arithmetic is the confidently
    /// wrong kind. `None` means either no limit was set or there are turns left.
    ///
    /// **`>=`, not `>`.** `turns` is turns *completed*, so a run that has finished its tenth
    /// turn under a limit of ten has spent the budget and may not open an eleventh. Asked at
    /// the turn boundary, which is the only place a run can be stopped without losing a turn's
    /// work (ADR-0004) — and asked again before anything *starts* an agent, because a limit
    /// checked only where it is reached is one that the next resume walks straight past.
    /// **Always `None` for a task**, which has no turns to spend and so no budget to exhaust
    /// (ADR-0019 §2). That is absence rather than a limit of zero: zero would read as *may take
    /// no turns at all*, which is a different sentence and one nothing here means.
    #[must_use]
    pub fn turn_limit_reached(&self, turns: u32) -> Option<u32> {
        self.agent()?
            .max_turns
            .map(std::num::NonZeroU32::get)
            .filter(|&limit| turns >= limit)
    }
}

/// The parts of a submitted run an operator may still change (ADR-0013).
///
/// Two, and deliberately not three. Changing your mind about *when* you need something is
/// ordinary — the run that was fine overnight is the one blocking a demo — and so is deciding
/// that it should go before the others. Everything else about a run is what was asked for, and a
/// third mutable field deserves an ADR of its own rather than following these two's lead: one
/// counter with an owner is manageable, and the risk is that it establishes a pattern and the
/// next field arrives with neither.
///
/// Both are *scheduling* inputs, and neither is a promise. Neither may weaken fencing, skip a
/// checkpoint or shorten a lease, and neither stops a run that is already going: there is no
/// preemption anywhere in this design (ADR-0004 — mid-turn is not a safe checkpoint), so the
/// most either one can do is decide what happens next.
/// **Struct variants, not newtypes.** `Deadline(Option<Millis>)` compiles and then fails at
/// runtime with a bare encoding error: serde's internally-tagged representation cannot encode a
/// newtype variant wrapping an `Option` (the same family of trap as one wrapping a sequence).
/// Found by a round-trip test over a real exchange rather than by the compiler, which is the
/// only way this kind is ever found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "edit")]
pub enum SpecEdit {
    /// When it is due. `None` is *as soon as you can*, which is what an unset deadline has
    /// always meant and not "no hurry".
    Deadline { at: Option<Millis> },
    /// Who yields when two runs want the same machine at the same moment.
    ///
    /// Absolute rather than a delta, though `+10` reads naturally and parses to the same thing.
    /// A delta would have to be added to *something*, and the only value available where the
    /// command is typed is a copy that may be a gossip tick old — so `+10` twice would sometimes
    /// mean 20 and sometimes 10, which is a worse answer than not offering it.
    Priority { to: i32 },
}

impl std::fmt::Display for SpecEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecEdit::Deadline { at: None } => f.write_str("due as soon as a node can take it"),
            SpecEdit::Deadline { at: Some(due) } => write!(f, "due at {}", due.0),
            SpecEdit::Priority { to } => write!(f, "priority {to}"),
        }
    }
}

/// Everything needed to resume a run somewhere else. Content-addressed; see ADR-0003.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// The agent's own session identifier, for `--resume`.
    pub session_id: Option<String>,
    pub transcript: BlobHash,
    /// Git bundle of the run's branch. `None` if nothing was committed yet.
    pub bundle: Option<BlobHash>,
    /// Uncommitted changes. The valuable, fragile part.
    pub patch: Option<BlobHash>,
    /// The commit the run branched from. The bundle and the patch are both meaningless
    /// without it: a receiving node has to check this out before it has anywhere to apply
    /// them, and it cannot be re-derived from the blobs.
    pub base_commit: String,
    pub turns: u32,
    pub taken_at: Millis,
    /// Guards against resuming on an agent too old to read this transcript.
    pub agent_version: String,
    /// Nodes known to hold every blob of this checkpoint.
    ///
    /// A checkpoint that exists only on the node that took it cannot be migrated from
    /// (ADR-0016), so this is what makes "durable" a fact rather than a hope. Filled in as
    /// replicas confirm; empty means the run survives only as long as its holder does, and
    /// that is worth saying out loud rather than discovering at 03:00.
    #[serde(default)]
    pub replicas: BTreeSet<NodeId>,
}

impl Checkpoint {
    /// Everything a receiving node has to have before it can materialise this.
    ///
    /// `base_commit` is deliberately not in here: it is a commit id, not a blob, and it comes
    /// from the repository rather than from the store.
    #[must_use]
    pub fn blobs(&self) -> Vec<BlobHash> {
        std::iter::once(self.transcript)
            .chain(self.bundle)
            .chain(self.patch)
            .collect()
    }

    /// Does this checkpoint survive its holder disappearing?
    ///
    /// The holder itself does not count, which is the entire point: a copy on the machine
    /// that might be in a bag is not a copy.
    ///
    /// **`None` is a real answer and not a missing one.** A run that reopens — the `Failed` one
    /// whose checkpoint decides whether the work survives its machine — has *no* holder, because
    /// the lease goes with the terminal transition; so does a run released by `offload
    /// checkpoint`. There is nothing to discount there, and `replicas` never contains the node
    /// that *took* the checkpoint (it is filled in as peers confirm), so the question is simply
    /// whether anybody else has it. Both callers used to invent a holder instead — one by
    /// substituting the local node, which answers about a peer's copy on a peer, and one by
    /// treating no holder as no copy, which reports three replicas as `on one machine only`.
    #[must_use]
    pub fn is_durable(&self, holder: Option<NodeId>) -> bool {
        self.replicas.iter().any(|node| Some(*node) != holder)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum RunState {
    /// Awaiting a holder. Nodes bid from here.
    Pending {
        since: Millis,
        /// The node that let this run go, when that is why nobody is holding it (ADR-0042).
        ///
        /// **`Pending` was two facts with one spelling.** A person parks a run with `offload
        /// checkpoint` and means to come back for it; a node *lets one go* on its way out — a
        /// drain, a shutdown, a commitment it can no longer keep — and means the fleet to carry
        /// on with it. Both leave the same state with the same checkpoint beside it, so
        /// [`crate::supervise`] read the second as the first and passed a stranded run by
        /// (ADR-0041 measured it; ADR-0042 is what this field is).
        ///
        /// **Inside the state on purpose**, which is the whole of its merge story: the state is
        /// the holder's to write and rides on whichever record wins `merge_run`, so this needs no
        /// owner and no arbiter of its own (ADR-0005). It also cannot go stale — every way out of
        /// `Pending` replaces the state, and every way *in* states this afresh.
        ///
        /// `None` is "not a node's doing, or nobody said": a fresh submission, a reopened
        /// failure, a run a person parked, and a record from a build that predates the field.
        /// That is the conservative reading and today's behaviour — it leaves the run for a
        /// person, which is the safe way to be wrong about who is coming back for it.
        #[serde(default)]
        let_go_by: Option<NodeId>,
    },
    /// Granted to a node; not yet confirmed started.
    Assigned {
        lease: Lease,
    },
    Running {
        lease: Lease,
        started_at: Millis,
    },
    /// Asked to checkpoint; finishing the current turn.
    Checkpointing {
        lease: Lease,
        requested_at: Millis,
    },
    /// Holder is out of contact. **Not yet a decision to move.** A returning holder can
    /// reclaim this; the hold-down policy decides when to give up on it instead.
    Orphaned {
        last: Lease,
        since: Millis,
    },
    Completed {
        at: Millis,
    },
    Failed {
        at: Millis,
        reason: String,
    },
    Cancelled {
        at: Millis,
    },
}

impl RunState {
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            RunState::Pending { .. } => "pending",
            RunState::Assigned { .. } => "assigned",
            RunState::Running { .. } => "running",
            RunState::Checkpointing { .. } => "checkpointing",
            RunState::Orphaned { .. } => "orphaned",
            RunState::Completed { .. } => "completed",
            RunState::Failed { .. } => "failed",
            RunState::Cancelled { .. } => "cancelled",
        }
    }

    /// When a terminal state was reached, for anything that ages out finished runs.
    #[must_use]
    pub fn finished_at(&self) -> Option<Millis> {
        match self {
            RunState::Completed { at }
            | RunState::Failed { at, .. }
            | RunState::Cancelled { at } => Some(*at),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunState::Completed { .. } | RunState::Failed { .. } | RunState::Cancelled { .. }
        )
    }

    /// Stopped by accident rather than by decision — the one terminal state that can be
    /// reopened, and the only one anything is allowed to pick back up on its own (ADR-0013).
    #[must_use]
    pub fn is_failed(&self) -> bool {
        matches!(self, RunState::Failed { .. })
    }

    /// Could an agent still be started from a checkpoint of a run in this state?
    ///
    /// A checkpoint has exactly one purpose — start the agent again — and the states that
    /// answer `false` here are the ones [`crate::run::Run`]'s consumers refuse by name:
    /// `Completed` and `Cancelled` are *decisions*, and re-opening one would restart work
    /// somebody deliberately stopped. `Failed` is the exception, and it is the reason this is
    /// not `is_terminal()`: it is the one terminal state that reopens, so its checkpoint is the
    /// one that matters most.
    ///
    /// **The arms are written out.** A new state has to make this decision rather than inherit
    /// it, because the two wrong answers are not symmetrical: answering `false` for something
    /// resumable deletes the only copy of a conversation, and the compiler is the only thing
    /// that will ask.
    #[must_use]
    pub fn may_resume_later(&self) -> bool {
        match self {
            RunState::Pending { .. }
            | RunState::Assigned { .. }
            | RunState::Running { .. }
            | RunState::Checkpointing { .. }
            | RunState::Orphaned { .. }
            | RunState::Failed { .. } => true,
            RunState::Completed { .. } | RunState::Cancelled { .. } => false,
        }
    }

    /// The lease under which work is currently permitted, if any.
    #[must_use]
    pub fn lease(&self) -> Option<&Lease> {
        match self {
            RunState::Assigned { lease }
            | RunState::Running { lease, .. }
            | RunState::Checkpointing { lease, .. } => Some(lease),
            // Orphaned deliberately does not return a live lease: the last holder has no
            // current right to act, only a right to *reclaim*.
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub spec: RunSpec,
    pub state: RunState,
    /// Highest epoch ever issued for this run. Never decreases.
    pub epoch: Epoch,
    /// Latest durable checkpoint. What a migration or a reclaim-failure resumes from.
    pub checkpoint: Option<Checkpoint>,
    /// Assignments so far. Drives backoff, so a run cannot ping-pong across the fleet.
    pub attempts: u32,
    /// The node that accepted the submission. Arbitrates bids for this run; see ADR-0006.
    pub home: NodeId,
    pub created_at: Millis,
    /// How many times an operator has edited the editable part of this spec — see
    /// [`SpecEdit`], which is the whole of it.
    ///
    /// Everything else about a spec is fixed at submit time, so "whichever record wins carries
    /// the spec" holds for all of it. These two fields are the exception, and mutability is why
    /// they need an owner and an arbitration rule of their own (ADR-0005): **the home node owns
    /// them and bumps this**, and a record carrying a higher revision wins them regardless of
    /// which record wins the run. Without that the holder — which is also believed about a run
    /// it is running — would gossip the deadline it started with and undo the edit every second.
    ///
    /// **One counter for both fields, not one each.** They have the same owner by design, and an
    /// owner serialises its own writes: a record at revision N carries the spec as it stood
    /// after N edits, whichever fields those touched. Two counters would say the same thing
    /// twice and leave a second merge rule to get wrong.
    ///
    /// It is a counter rather than a timestamp because the two writers are the same node
    /// writing twice, not two nodes disagreeing; an operator issuing the change at another
    /// node has it forwarded to the owner rather than applied locally.
    #[serde(default)]
    pub spec_rev: u32,
    /// What kind of thing started this run (ADR-0024).
    ///
    /// `home` says *which node* asked; this says *what asked*. Set once when the run is created
    /// and changed by nothing — no transition, no operator command, no merge — which puts it in
    /// the same class as `id`, `home` and `created_at`: it rides inside whichever record
    /// `merge_run` settles on and is identical in every copy, so there is no owner to name and no
    /// merge rule to get wrong.
    ///
    /// **Not attendance.** Whether anybody is *watching* is observed and never declared
    /// (ADR-0013), and gossiping it would hand a decision a value from before the silence. This
    /// is the other thing entirely: known at submission, immutable, and stated by the only node
    /// that could know. What reads it wants the inference "nobody will come back for this
    /// output", which is ADR-0024's decision rather than something the field claims.
    ///
    /// **It decides only what may be thrown away.** It must not reach bidding, placement,
    /// capacity or the delivery plane: a triggered run is an *ordinary* run, which is ADR-0020's
    /// central claim and the reason it changed no existing type.
    #[serde(default)]
    pub origin: Origin,
}

/// What kind of thing started a run (ADR-0024).
///
/// The default is `Operator` in every sense — the serde default, what an older build's record
/// decodes to, and the answer whenever anything is unsure. It is the value that *keeps* things,
/// which is what makes a build that has never heard of this field behave exactly as it does
/// today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// A person asked for this, and is expected to read what came of it.
    #[default]
    Operator,
    /// A rule fired it (ADR-0020). No person asked and none is waiting, which is the whole
    /// meaning of unattended and the premise `Supervisor::cleanup` was built on the other side
    /// of.
    Rule,
}

/// Why a holder is giving a run up — the difference between a run a person parked and one a
/// node let go (ADR-0042).
///
/// Both reach `Pending` with the same checkpoint beside them, and they are opposite instructions
/// to everything downstream: one waits for `offload resume`, the other wants a bid round. The
/// distinction is knowable only where the checkpoint was *asked for*, which is why it travels
/// from there rather than being worked out at the transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GivenUp {
    /// A person asked for it, with `offload checkpoint`, and is expected to come back for the
    /// run. Picking it up on their behalf is the "we decided for you" this project refuses
    /// (ADR-0014).
    Parked,
    /// The node stopped intending to run it — a drain, a shutdown, a commitment it cannot keep.
    /// Nobody is coming back for it, so the fleet should carry on with it.
    LetGo,
}

impl GivenUp {
    /// The node to record on the state, given who is giving the run up.
    ///
    /// A method rather than a `match` at the one call site, because the next transition that
    /// releases a run should get this from here rather than reinvent the mapping.
    #[must_use]
    pub fn by(self, node: NodeId) -> Option<NodeId> {
        match self {
            GivenUp::Parked => None,
            GivenUp::LetGo => Some(node),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransitionError {
    #[error("cannot {action} a run that is {state}")]
    WrongState {
        action: &'static str,
        state: &'static str,
    },
    #[error("stale epoch: run is at {current}, caller presented {presented}")]
    StaleEpoch { current: Epoch, presented: Epoch },
    #[error("node {presented} is not the holder of this run ({holder})")]
    NotHolder { holder: String, presented: String },
    #[error("run has already finished ({state})")]
    Terminal { state: &'static str },
    /// Asked of work that has no turn boundary to do it at.
    ///
    /// A task is a nominated program (ADR-0019 §2): no agent, no conversation, no transcript —
    /// so there is no boundary to capture at and nothing a capture would be *of*. ADR-0058 is
    /// the other half of the same fact: a failed task is **restarted from its spec**, never
    /// resumed, so even a checkpoint that could be taken would have nothing to be resumed from.
    ///
    /// Its own variant rather than a `WrongState`, because the state is not what is wrong: the
    /// run was `Running`, which is exactly when a checkpoint request is legal. What is wrong is
    /// the kind of work, and a sentence naming the state would send somebody to wait for a
    /// moment that is never coming.
    #[error("cannot {action} {kind} work: it has no turn boundary to do it at (ADR-0019 §2)")]
    NoBoundary {
        action: &'static str,
        kind: &'static str,
    },
}

impl Run {
    #[must_use]
    pub fn new(id: RunId, spec: RunSpec, home: NodeId, now: Millis) -> Self {
        Run {
            id,
            spec,
            state: RunState::Pending {
                since: now,
                let_go_by: None,
            },
            epoch: Epoch::INITIAL,
            checkpoint: None,
            attempts: 0,
            home,
            created_at: now,
            spec_rev: 0,
            origin: Origin::Operator,
        }
    }

    /// Say that a rule started this, at the moment it is built.
    ///
    /// A builder rather than an argument to [`Run::new`], which has sixty call sites and would
    /// gain a parameter every one of them answers the same way. The one caller that does not is
    /// the trigger's, and it is the one that has to remember.
    #[must_use]
    pub fn started_by(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }

    /// Change the editable part of this run's spec, as its owner. Returns the new revision.
    ///
    /// Nothing else about the run moves, and nothing about it moves *now*: there is no
    /// preemption, no way to make an agent think faster, and no run is stopped to let another
    /// one past. What an edit changes is later decisions — where the run lands if it is
    /// re-placed, which of this node's commitments starts next, how patiently its next failure
    /// is handled. Claiming otherwise would be the most tempting lie in this design.
    pub fn edit(&mut self, edit: SpecEdit) -> u32 {
        match edit {
            SpecEdit::Deadline { at } => self.spec.deadline = at,
            SpecEdit::Priority { to } => self.spec.priority = to,
        }
        self.spec_rev += 1;
        self.spec_rev
    }

    /// The node that let this run go, if that is why nobody is holding it (ADR-0042).
    ///
    /// `None` for every other reason a run has no holder, including a run a person parked —
    /// see [`RunState::Pending`]'s field, where the conservative reading is written down.
    #[must_use]
    pub fn let_go_by(&self) -> Option<NodeId> {
        match self.state {
            RunState::Pending { let_go_by, .. } => let_go_by,
            _ => None,
        }
    }

    /// Grant the run to `node`, bumping the epoch. Legal from `Pending` and from
    /// `Orphaned` (the hold-down policy having decided to give up on the old holder).
    pub fn assign(
        &mut self,
        node: NodeId,
        now: Millis,
        lease_ttl: Millis,
    ) -> Result<Epoch, TransitionError> {
        self.reject_if_terminal()?;
        match self.state {
            RunState::Pending { .. } | RunState::Orphaned { .. } => {
                if self.spec.restartability == Restartability::Pinned && self.attempts > 0 {
                    // A pinned run has exactly one holder for life.
                    return Err(TransitionError::WrongState {
                        action: "reassign",
                        state: "pinned",
                    });
                }
                self.epoch = self.epoch.next();
                self.attempts += 1;
                self.state = RunState::Assigned {
                    lease: Lease {
                        node,
                        epoch: self.epoch,
                        expires_at: now + lease_ttl,
                    },
                };
                Ok(self.epoch)
            }
            _ => Err(TransitionError::WrongState {
                action: "assign",
                state: self.state.name(),
            }),
        }
    }

    /// Holder confirms the agent is up.
    pub fn started(
        &mut self,
        node: NodeId,
        epoch: Epoch,
        now: Millis,
    ) -> Result<(), TransitionError> {
        self.fence(node, epoch)?;
        match self.state {
            RunState::Assigned { lease } => {
                self.state = RunState::Running {
                    lease,
                    started_at: now,
                };
                Ok(())
            }
            _ => Err(TransitionError::WrongState {
                action: "start",
                state: self.state.name(),
            }),
        }
    }

    /// Heartbeat. Extends the lease.
    pub fn renew(
        &mut self,
        node: NodeId,
        epoch: Epoch,
        now: Millis,
        lease_ttl: Millis,
    ) -> Result<(), TransitionError> {
        self.fence(node, epoch)?;
        let expires_at = now + lease_ttl;
        match &mut self.state {
            RunState::Assigned { lease }
            | RunState::Running { lease, .. }
            | RunState::Checkpointing { lease, .. } => {
                lease.expires_at = expires_at;
                Ok(())
            }
            _ => Err(TransitionError::WrongState {
                action: "renew",
                state: self.state.name(),
            }),
        }
    }

    /// Contact with the holder has been lost. This records the fact; it does **not**
    /// decide anything. `Orphaned` is where the hold-down policy gets to think.
    pub fn orphan(&mut self, now: Millis) -> Result<(), TransitionError> {
        self.reject_if_terminal()?;
        match self.state.lease() {
            Some(&lease) => {
                self.state = RunState::Orphaned {
                    last: lease,
                    since: now,
                };
                Ok(())
            }
            None => Err(TransitionError::WrongState {
                action: "orphan",
                state: self.state.name(),
            }),
        }
    }

    /// The original holder came back before we gave up on it. No migration, no epoch bump,
    /// no lost work — the cheapest possible outcome of a device dropping off.
    pub fn reclaim(
        &mut self,
        node: NodeId,
        epoch: Epoch,
        now: Millis,
        lease_ttl: Millis,
    ) -> Result<(), TransitionError> {
        match self.state {
            RunState::Orphaned { last, .. } => {
                if last.node != node {
                    return Err(TransitionError::NotHolder {
                        holder: last.node.short(),
                        presented: node.short(),
                    });
                }
                if epoch != self.epoch {
                    return Err(TransitionError::StaleEpoch {
                        current: self.epoch,
                        presented: epoch,
                    });
                }
                self.state = RunState::Running {
                    lease: Lease {
                        node,
                        epoch,
                        expires_at: now + lease_ttl,
                    },
                    started_at: now,
                };
                Ok(())
            }
            _ => Err(TransitionError::WrongState {
                action: "reclaim",
                state: self.state.name(),
            }),
        }
    }

    /// Ask the holder to checkpoint at its next turn boundary (ADR-0004).
    ///
    /// **Idempotent from `Checkpointing`**, because a second request is the same request and
    /// there is exactly one boundary coming for both. It used to be refused, which was harmless
    /// while a request was only ever "somebody wants a checkpoint" and stopped being so when it
    /// began to carry *who* (ADR-0042): a drain reaching a run a person had already parked was
    /// turned away here, so the run released as parked and no node would offer it — the drained
    /// machine's operator having said, in as many words, that this node is leaving.
    ///
    /// `requested_at` keeps the *first* asking. It is what `offload explain` counts from, and
    /// the honest answer to "how long has this been waiting for its turn to end" is the whole
    /// wait rather than the part since the last person mentioned it.
    pub fn request_checkpoint(&mut self, now: Millis) -> Result<(), TransitionError> {
        self.reject_if_terminal()?;
        // **Asked before the state, because the state is not what is wrong.** A running task is
        // `Running`, which is precisely when this is legal — and it has no agent, no conversation
        // and no boundary, so the request can never be honoured. It was accepted: the flag
        // `Supervisor::request_checkpoint` arms is guarded on *something is running here*
        // (`cancel.is_some()`), and `start_run` registers that channel one branch **above** the
        // split on `Work::Task`, so a task passes a guard written for an agent. Measured: the run
        // went to `checkpointing`, `offload explain` said *"finishing its turn before
        // checkpointing"* about a `/bin/sleep`, the operator was told to watch for a capture with
        // `offload logs -f`, and nothing was ever taken. The costlier half is the drain, which
        // arms the same flag and then waits `drain_deadline_secs` — **300 seconds by default** —
        // for a boundary that cannot arrive.
        //
        // Here rather than at either caller, for the reason `Supervisor::register` holds the
        // second-agent check: two callers guarding is two things to remember, and the third will
        // not. This is the **fourth** agent-only path — `prepare_start`, `resume` and `drive` are
        // the other three — and the only one that was never given a door.
        if self.spec.work.kind() != WorkKind::Agent {
            return Err(TransitionError::NoBoundary {
                action: "checkpoint",
                kind: self.spec.work.kind().name(),
            });
        }
        match self.state {
            RunState::Running { lease, .. } => {
                self.state = RunState::Checkpointing {
                    lease,
                    requested_at: now,
                };
                Ok(())
            }
            RunState::Checkpointing { .. } => Ok(()),
            _ => Err(TransitionError::WrongState {
                action: "checkpoint",
                state: self.state.name(),
            }),
        }
    }

    /// Holder has captured and uploaded a checkpoint and released the run. Back to the
    /// pool for anyone to bid on, including the node that just released it.
    ///
    /// `given_up` is the whole of ADR-0042: the same capture, at the same boundary, writing the
    /// same blobs, means one thing when a person asked for it and another when the node did.
    /// It is a parameter rather than something inferred here because the only thing that knows
    /// is whoever armed the request, several minutes and one turn earlier.
    pub fn checkpointed(
        &mut self,
        node: NodeId,
        epoch: Epoch,
        checkpoint: Checkpoint,
        given_up: GivenUp,
        now: Millis,
    ) -> Result<(), TransitionError> {
        self.fence(node, epoch)?;
        self.reject_if_unplaceable_once_released()?;
        self.checkpoint = Some(checkpoint);
        // Bump the epoch on release, for two reasons: it immediately invalidates the
        // departing holder's right to act, and it keeps `(epoch, progress)` monotonic so
        // gossiped run states merge unambiguously (see `view::merge_run`).
        self.epoch = self.epoch.next();
        self.state = RunState::Pending {
            since: now,
            let_go_by: given_up.by(node),
        };
        Ok(())
    }

    /// Give back a run this node accepted and never started.
    ///
    /// The counterpart of [`Self::checkpointed`] for a commitment rather than for work: a node
    /// that took a run into `Assigned` to start later (ADR-0006) and then finds it cannot —
    /// it is draining, or its own estimate has slipped — hands it back to the pool instead of
    /// sitting on it. There is nothing to capture, because nothing ran.
    ///
    /// The epoch bumps for the same reason it does on release: it ends the departing holder's
    /// right to act on the run at the moment it stops intending to.
    pub fn release(
        &mut self,
        node: NodeId,
        epoch: Epoch,
        now: Millis,
    ) -> Result<(), TransitionError> {
        self.fence(node, epoch)?;
        match self.state {
            RunState::Assigned { .. } => {
                self.reject_if_unplaceable_once_released()?;
                self.epoch = self.epoch.next();
                // Never an operator's doing: there is no command that gives back a
                // commitment, so the run is the fleet's again rather than a person's, and
                // something has to be able to see that (ADR-0042).
                //
                // **`let_go_by` is who it was assigned to, not who decided.** Two of the three
                // callers are the holder changing its mind about work nobody parked — a drain,
                // and a node that cannot make the start it promised (ADR-0006). The third is
                // the *arbiter*: `Cluster::hand_over` gives its token back when a grant is
                // declined or never confirmed, and the node named then may never have held the
                // run — may, in the unconfirmed case, be running it. All three want the same
                // decision out of the field (offer it again, ADR-0042) and only one of them
                // knows an intention, so nothing downstream may read one out of it. See
                // `explain::state_detail`, which said "let it go" about all three until a walk
                // put the sentence next to `did not confirm` about the same node.
                self.state = RunState::Pending {
                    since: now,
                    let_go_by: Some(node),
                };
                Ok(())
            }
            // Deliberately not `Running`: an agent that has started has state to capture, and
            // dropping it here would be the mid-turn snapshot ADR-0004 forbids.
            _ => Err(TransitionError::WrongState {
                action: "release",
                state: self.state.name(),
            }),
        }
    }

    /// Record a checkpoint without giving up the run — the routine per-turn case.
    pub fn record_checkpoint(
        &mut self,
        node: NodeId,
        epoch: Epoch,
        checkpoint: Checkpoint,
    ) -> Result<(), TransitionError> {
        self.fence(node, epoch)?;
        self.checkpoint = Some(checkpoint);
        Ok(())
    }

    pub fn complete(
        &mut self,
        node: NodeId,
        epoch: Epoch,
        now: Millis,
    ) -> Result<(), TransitionError> {
        self.fence(node, epoch)?;
        self.state = RunState::Completed { at: now };
        Ok(())
    }

    pub fn fail(
        &mut self,
        node: NodeId,
        epoch: Epoch,
        reason: impl Into<String>,
        now: Millis,
    ) -> Result<(), TransitionError> {
        self.fence(node, epoch)?;
        self.state = RunState::Failed {
            at: now,
            reason: reason.into(),
        };
        Ok(())
    }

    /// Operator action, so it is not fenced — cancelling always wins.
    pub fn cancel(&mut self, now: Millis) -> Result<(), TransitionError> {
        self.reject_if_terminal()?;
        self.state = RunState::Cancelled { at: now };
        Ok(())
    }

    /// Give up on a run that can never be placed again.
    pub fn abandon(
        &mut self,
        reason: impl Into<String>,
        now: Millis,
    ) -> Result<(), TransitionError> {
        self.reject_if_terminal()?;
        self.state = RunState::Failed {
            at: now,
            reason: reason.into(),
        };
        Ok(())
    }

    /// Put a failed run back in the pool so it can be picked up again.
    ///
    /// Operator action, so it is unfenced like [`Run::cancel`] — and legal **only** from
    /// `Failed`. `Completed` and `Cancelled` are decisions somebody made; `Failed` is an
    /// accident, and an accident that happened to a run holding a checkpoint has real work
    /// behind it. A daemon that died mid-run leaves exactly this state, and refusing to
    /// reopen it would throw away the conversation and the uncommitted edits.
    ///
    /// The epoch is bumped: whatever was holding the run when it failed must not be able
    /// to act on it after somebody else picks it up.
    pub fn reopen(&mut self, now: Millis) -> Result<(), TransitionError> {
        match self.state {
            RunState::Failed { .. } => {
                self.reject_if_unplaceable_once_released()?;
                self.epoch = self.epoch.next();
                // Nobody let this go: it broke, and a person or the recovery tick is picking it
                // back up here and now. Stating it rather than inheriting whatever the last
                // `Pending` said is the point of putting the fact inside the state.
                self.state = RunState::Pending {
                    since: now,
                    let_go_by: None,
                };
                Ok(())
            }
            _ => Err(TransitionError::WrongState {
                action: "reopen",
                state: self.state.name(),
            }),
        }
    }

    /// Hand a failed run back to the fleet on the way out (ADR-0043).
    ///
    /// The other half of [`Self::reopen`], and the difference is who is expected to run it
    /// next. `reopen` is a failed run picked back up *here* — by a person typing `offload
    /// resume`, or by the recovery tick that ADR-0013 lets act on their behalf — so nobody let
    /// it go and `let_go_by` is `None`. This is the node that last ran it saying it will not be
    /// the one to try again, which is exactly the fact [`crate::supervise`] needs before it will
    /// offer a run to anybody (ADR-0042).
    ///
    /// **Unfenced, like `reopen`, and for the same reason**: a `Failed` run holds no lease, so
    /// there is nothing to fence against — the terminal transition took it. The epoch bump is
    /// what ends the last leg's right to act, and it is why this must be run inside
    /// `Store::update_run` rather than on a caller's copy.
    pub fn let_go(&mut self, node: NodeId, now: Millis) -> Result<(), TransitionError> {
        match self.state {
            RunState::Failed { .. } => {
                self.reject_if_unplaceable_once_released()?;
                self.epoch = self.epoch.next();
                self.state = RunState::Pending {
                    since: now,
                    let_go_by: Some(node),
                };
                Ok(())
            }
            _ => Err(TransitionError::WrongState {
                action: "let go",
                state: self.state.name(),
            }),
        }
    }

    /// The guard. Every side-effecting transition goes through here.
    pub fn fence(&self, node: NodeId, epoch: Epoch) -> Result<(), TransitionError> {
        if self.state.is_terminal() {
            return Err(TransitionError::Terminal {
                state: self.state.name(),
            });
        }
        let Some(lease) = self.state.lease() else {
            return Err(TransitionError::WrongState {
                action: "act on",
                state: self.state.name(),
            });
        };
        if epoch < self.epoch {
            return Err(TransitionError::StaleEpoch {
                current: self.epoch,
                presented: epoch,
            });
        }
        if lease.node != node {
            return Err(TransitionError::NotHolder {
                holder: lease.node.short(),
                presented: node.short(),
            });
        }
        Ok(())
    }

    #[must_use]
    pub fn holder(&self) -> Option<NodeId> {
        self.state.lease().map(|l| l.node).or(match self.state {
            RunState::Orphaned { last, .. } => Some(last.node),
            _ => None,
        })
    }

    /// The checkpoint anything could still start an agent from — `None` once nothing can.
    ///
    /// The field says what the last capture *was*; this says whether it is still a checkpoint.
    /// Past a decision to stop ([`RunState::may_resume_later`]) the bytes it names are
    /// unreachable by every path there is: `Supervisor::resume` refuses `Completed` and
    /// `Cancelled` by name, and nothing else fetches a checkpoint at all.
    ///
    /// Asked by three callers, and each of them was reading `is_terminal` — which is right for a
    /// completed run and **wrong for a failed one**, the very run whose checkpoint decides
    /// whether the work survives its machine. The blob collector kept the bytes for ever
    /// (measured: 472 KB a run, and a transcript is the whole conversation so far, so it grows
    /// with the run), while `ps` and `offload explain` went quiet about durability at exactly the
    /// moment it mattered.
    #[must_use]
    pub fn resumable_checkpoint(&self) -> Option<&Checkpoint> {
        self.checkpoint
            .as_ref()
            .filter(|_| self.state.may_resume_later())
    }

    /// The checkpoint this run *starts from*, when it is a continuation that has not yet taken
    /// one of its own (ADR-0064 §2): the parent's work, captured at submission, at turn zero.
    ///
    /// Told apart by the turn count because a real capture is only ever taken at a turn
    /// boundary, so it is at turn one or later, and a continuation's own first capture replaces
    /// the base exactly as any capture replaces the one before. From then on it is an ordinary
    /// run with an ordinary checkpoint, and a migration resumes its *own* conversation.
    #[must_use]
    pub fn continuation_base(&self) -> Option<&Checkpoint> {
        self.spec.parent.as_ref()?;
        self.checkpoint.as_ref().filter(|c| c.turns == 0)
    }

    #[must_use]
    pub fn lease_expired(&self, now: Millis) -> bool {
        self.state.lease().is_some_and(|l| l.is_expired(now))
    }

    fn reject_if_terminal(&self) -> Result<(), TransitionError> {
        if self.state.is_terminal() {
            return Err(TransitionError::Terminal {
                state: self.state.name(),
            });
        }
        Ok(())
    }

    /// Refuse to put a run back in a pool that cannot place it again.
    ///
    /// `Pending` has three entrances — a released checkpoint, a handed-back commitment, and an
    /// operator reopening a failure — and one exit, [`Self::assign`], which refuses a pinned
    /// run a second holder. So for a pinned run every one of those entrances is a trap: the
    /// run leaves a state somebody can see, arrives somewhere nothing will ever take it from,
    /// and `ps` says `pending` for ever. Nothing fails, which is what makes it worth a guard
    /// rather than a comment.
    ///
    /// The rule was already written down — `KeepReason::PinnedHere` says releasing a pinned run
    /// "would not re-place it, it would make it unplaceable" — and honoured in exactly the one
    /// place it was written. Here it is enforced where `Pending` is entered, so a fourth caller
    /// cannot forget it.
    ///
    /// What a pinned run does instead is what its own documentation says: it stays where it is,
    /// and it fails if its holder goes away. `decide_reassignment` abandons it after
    /// `pinned_timeout` and `decide_recovery` declines to resume it
    /// (`Escalation::CannotBeRestarted`), so refusing here loses nothing — it only makes the
    /// refusal loud on the paths that used to swallow it.
    fn reject_if_unplaceable_once_released(&self) -> Result<(), TransitionError> {
        if self.spec.restartability == Restartability::Pinned && self.attempts > 0 {
            return Err(TransitionError::WrongState {
                action: "return to the pool",
                state: "pinned",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::AgentKind;
    use crate::constraint::Constraint;

    const TTL: Millis = Millis(10_000);

    fn node(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    fn checkpoint() -> Checkpoint {
        Checkpoint {
            session_id: Some("sess-1".into()),
            transcript: BlobHash::from_bytes([7; 32]),
            bundle: None,
            patch: None,
            base_commit: "0f0f0f0f".into(),
            turns: 3,
            taken_at: Millis(200),
            agent_version: "2.10.0".into(),
            replicas: BTreeSet::new(),
        }
    }

    #[test]
    fn a_turn_limit_is_reached_at_the_limit_and_not_after_it() {
        let mut run = run();
        run.spec.agent_mut().expect("an agent run").max_turns = std::num::NonZeroU32::new(3);

        // `>=`, and the boundary is the whole point: `turns` counts turns *completed*, so a run
        // that has finished its third turn under a limit of three has spent the budget. A `>`
        // here would let every run take one more turn than it was given — the off-by-one that
        // is invisible in a log and shows up on a bill.
        assert_eq!(
            run.spec.turn_limit_reached(2),
            None,
            "two of three, room left"
        );
        assert_eq!(
            run.spec.turn_limit_reached(3),
            Some(3),
            "the third finished it"
        );
        // Over, which a run reaches by being resumed at a limit lowered nowhere: it still
        // answers, because the caller needs a sentence rather than a surprise.
        assert_eq!(run.spec.turn_limit_reached(9), Some(3));

        // And no limit is no limit, however many turns have gone by. This is every run that has
        // ever been submitted, and it must keep behaving exactly as it did.
        run.spec.agent_mut().expect("an agent run").max_turns = None;
        assert_eq!(run.spec.turn_limit_reached(0), None);
        assert_eq!(run.spec.turn_limit_reached(10_000), None);
    }

    #[test]
    fn a_turn_limit_of_zero_is_refused_by_the_type() {
        // `WorkPolicy::allowed_agents`' rule, at the other end of the same argument: a value
        // that reads as a restriction while meaning "do nothing at all" is refused where it
        // arrives rather than interpreted later. The CLI refuses it too, with a sentence — but
        // the CLI is not the only way in, and a rule stored months ago is fired by a daemon
        // that never saw that parser.
        let spec = run().spec;
        let mut encoded = serde_json::to_value(&spec).expect("encode");
        encoded["work"]["Agent"]["max_turns"] = serde_json::json!(0);
        let err = serde_json::from_value::<RunSpec>(encoded).expect_err("zero must not decode");
        assert!(
            err.to_string().contains("nonzero"),
            "and it should say why: {err}"
        );

        // While the value that means *no limit* stays absent-or-null, which is what every
        // record written before this field did anything decodes as.
        let mut encoded = serde_json::to_value(&spec).expect("encode");
        encoded["work"]["Agent"]["max_turns"] = serde_json::Value::Null;
        assert_eq!(
            serde_json::from_value::<RunSpec>(encoded)
                .expect("null decodes")
                .agent()
                .and_then(|w| w.max_turns),
            None
        );
    }

    #[test]
    fn a_checkpoint_is_refused_for_work_that_has_no_boundary() {
        // The guard is on the **kind**, not the state, and the state is why it was missing: a
        // running task is `Running`, which is exactly when a checkpoint request is legal. It was
        // accepted — `Supervisor::request_checkpoint`'s only door asks whether *something* is
        // running here (`cancel.is_some()`), and `start_run` registers that channel one branch
        // above the split on `Work::Task`, so a task walks through a guard written for an agent.
        //
        // Measured on a daemon: `offload checkpoint <a running task>` answered "checkpoint
        // requested — it will be taken at the next turn boundary", `offload ps` then read
        // `checkpointing`, and `offload explain` said "finishing its turn before checkpointing"
        // about a `/bin/sleep`. Nothing was ever captured. The costlier half is the drain, which
        // arms the same flag and waited `drain_deadline_secs` — 300s by default — for it.
        let mut task = run();
        task.spec = RunSpec {
            work: Work::Task(TaskWork {
                service: crate::Service::Other("slow".into()),
                args: Vec::new(),
            }),
            ..task.spec
        };
        let holder = node(2);
        let e = task.assign(holder, Millis(0), TTL).expect("assign");
        task.started(holder, e, Millis(1)).expect("start");
        assert!(
            matches!(task.state, RunState::Running { .. }),
            "the premise: it is running, which is when this is legal for an agent"
        );

        let refused = task
            .request_checkpoint(Millis(2))
            .expect_err("a task has no turn boundary to capture at");
        assert!(
            matches!(
                refused,
                TransitionError::NoBoundary {
                    action: "checkpoint",
                    kind: "task"
                }
            ),
            "the refusal has to name the kind, not the state: {refused}"
        );
        assert!(
            matches!(task.state, RunState::Running { .. }),
            "a refused request must not have the side effects of the honoured one — it went to \
             `checkpointing`, which is what put `finishing its turn` on an operator's screen"
        );

        // And the control: the same call on the same state, for work that does have a boundary.
        let mut agent = run();
        let e = agent.assign(holder, Millis(0), TTL).expect("assign");
        agent.started(holder, e, Millis(1)).expect("start");
        agent
            .request_checkpoint(Millis(2))
            .expect("an agent run still checkpoints");
        assert!(matches!(agent.state, RunState::Checkpointing { .. }));
    }

    fn a_task(restartability: Restartability) -> RunSpec {
        RunSpec {
            work: Work::Task(TaskWork {
                service: crate::Service::Other("watch-api".into()),
                args: vec!["--once".into()],
            }),
            restartability,
            ..run().spec
        }
    }

    #[test]
    fn a_spec_written_before_the_work_split_still_decodes() {
        // **The reason the compatibility deserializer exists.** A whole `Run` is one JSON blob
        // in `runs.run_json`, and a rule keeps a whole `RunSpec`, so the first thing a node
        // does after this change is read back rows the version before it wrote. If this test
        // goes red, upgrading a daemon loses every run and every rule already on disk.
        //
        // The document below is the pre-split shape spelled out rather than generated: a
        // fixture built by re-serialising today's type would test nothing, because it would
        // change shape in lockstep with the code it is meant to pin.
        let old = serde_json::json!({
            "agent": "claude_code",
            "model": "claude-haiku-4-5-20251001",
            "prompt": "add tests for the parser",
            "workspace": { "repo": "/repo", "git_ref": null, "branch": "offload/run" },
            "constraint": "always",
            "restartability": "resumable",
            "permission_mode": "accept_edits",
            "max_turns": 12,
            "priority": 3,
            "queue": true,
            "deadline": 1_700_000_000_000_u64
        });
        let spec: RunSpec = serde_json::from_value(old).expect("the old shape must still decode");
        let work = spec.agent().expect("an agent run");
        assert_eq!(work.prompt, "add tests for the parser");
        assert_eq!(work.workspace.repo, "/repo");
        assert_eq!(work.max_turns.map(std::num::NonZeroU32::get), Some(12));
        assert_eq!(work.permission_mode, PermissionMode::AcceptEdits);
        // …and the shared half came across too, including the fields serde defaulted.
        assert_eq!(spec.priority, 3);
        assert!(spec.queue);
        assert_eq!(spec.deadline, Some(Millis(1_700_000_000_000)));
        assert_eq!(spec.restartability, Restartability::Resumable);
    }

    /// A spec a v29 node wrote to its own disk, before `prefer` and `hold_until` existed, reads
    /// back as a run that prefers nothing and holds for nothing — which is what it meant.
    ///
    /// The literal is **what the v29 build actually serialised**, captured from that tree in
    /// session eighty-nine rather than typed from memory, per `storage-and-encoding`'s rule.
    #[test]
    fn a_spec_written_before_preferences_existed_prefers_nothing() {
        let v29 = r#"{"work":{"Agent":{"agent":"claude_code","model":null,"prompt":"fix the parser","workspace":{"repo":"/repo","archive_bytes":null,"git_ref":null,"branch":null},"permission_mode":"ask","allow":[],"max_turns":null,"ask":"never"}},"constraint":"always","restartability":"resumable","priority":0,"queue":false,"deadline":null,"demand":"normal","notify":{"audience":"everyone"},"notices":"everything","resources":[]}"#;
        let spec: RunSpec = serde_json::from_str(v29).expect("a v29 row must still decode");
        assert_eq!(spec.prefer, crate::Constraint::Always);
        assert_eq!(spec.hold_until, None);
        assert!(!spec.holding(Millis(0)));
        assert_eq!(*spec.eligibility(Millis(0)), spec.constraint);
    }

    /// ADR-0072: an empty repository was accepted and failed at `git clone ''` on another node.
    #[test]
    fn an_agent_run_names_a_workspace_and_a_scratch_one_names_no_ref() {
        let with = |repo: &str, git_ref: Option<&str>| {
            let mut spec = run().spec;
            if let Work::Agent(work) = &mut spec.work {
                work.workspace.repo = repo.into();
                work.workspace.git_ref = git_ref.map(Into::into);
            }
            spec.check()
        };
        assert_eq!(with("", None), Err(SpecProblem::NoWorkspace));
        assert_eq!(with("  ", None), Err(SpecProblem::NoWorkspace));
        assert_eq!(with(crate::SCRATCH, None), Ok(()));
        assert_eq!(
            with(crate::SCRATCH, Some("main")),
            Err(SpecProblem::ScratchWithRef)
        );
        assert_eq!(with("/repo", Some("main")), Ok(()));
    }

    /// ADR-0063 §4's three refusals, each with the reason it exists.
    #[test]
    fn a_hold_needs_a_preference_a_queue_and_to_end_before_the_deadline() {
        let base = || RunSpec {
            prefer: crate::Constraint::Node(NodeId::from_bytes([2; 32])),
            hold_until: Some(Millis(10_000)),
            queue: true,
            ..run().spec
        };
        assert_eq!(base().check(), Ok(()));
        assert_eq!(
            RunSpec {
                prefer: crate::Constraint::Always,
                ..base()
            }
            .check(),
            Err(SpecProblem::HoldWithoutPreference)
        );
        assert_eq!(
            RunSpec {
                queue: false,
                ..base()
            }
            .check(),
            Err(SpecProblem::HoldWithoutQueue)
        );
        assert_eq!(
            RunSpec {
                deadline: Some(Millis(9_000)),
                ..base()
            }
            .check(),
            Err(SpecProblem::HoldPastDeadline)
        );
        // The hold is the eligibility while it lasts, and not after.
        let held = base();
        assert!(held.eligibility(Millis(9_999)).names_a_node());
        assert!(!held.eligibility(Millis(10_000)).names_a_node());
    }

    /// A continuation starts from its base checkpoint only until it takes one of its own: turn
    /// zero is the parent's work, and any real capture — always at turn one or later — replaces
    /// it, so a migrated continuation resumes its *own* conversation.
    #[test]
    fn a_continuation_starts_from_its_base_until_it_has_a_checkpoint_of_its_own() {
        let checkpoint = |turns| Checkpoint {
            session_id: None,
            transcript: crate::BlobHash::from_bytes([1; 32]),
            bundle: None,
            patch: None,
            base_commit: "0f0f".into(),
            turns,
            taken_at: Millis(0),
            agent_version: "0".into(),
            replicas: std::collections::BTreeSet::new(),
        };
        let mut r = run();
        r.checkpoint = Some(checkpoint(0));
        assert!(
            r.continuation_base().is_none(),
            "not a continuation: an ordinary checkpoint"
        );
        r.spec.parent = Some(Continuation {
            run: RunId::from_bytes([9; 16]),
            mode: ContinueMode::Handoff,
            transcript: None,
            parent_prompt: "p".into(),
            closing_message: None,
        });
        assert!(r.continuation_base().is_some());
        r.checkpoint = Some(checkpoint(1));
        assert!(
            r.continuation_base().is_none(),
            "its own first capture replaced the base"
        );
        // …and the field round-trips, both ways.
        let json = serde_json::to_string(&r.spec).expect("encode");
        assert_eq!(
            serde_json::from_str::<RunSpec>(&json).expect("decode"),
            r.spec
        );
    }

    #[test]
    fn a_spec_round_trips_through_the_new_shape_in_both_directions() {
        // `storage-and-encoding`'s rule: round-trip *both* ways. An enum that serialises and
        // does not come back is the shape of bug that only shows up on the next restart.
        let spec = run().spec;
        let json = serde_json::to_string(&spec).expect("encode");
        assert!(
            json.contains("\"work\""),
            "the new shape is what gets written: {json}"
        );
        assert_eq!(
            serde_json::from_str::<RunSpec>(&json).expect("decode"),
            spec
        );

        let task = a_task(Restartability::Idempotent);
        let json = serde_json::to_string(&task).expect("encode");
        let back: RunSpec = serde_json::from_str(&json).expect("decode");
        assert_eq!(back, task);
        assert!(back.agent().is_none(), "a task has no agent half");
        assert!(back.workspace().is_none(), "and no workspace at all");
        assert_eq!(back.agent_kind(), None);
    }

    #[test]
    fn a_document_that_is_neither_shape_is_an_error_rather_than_a_guess() {
        // The compatibility path must not invent an agent run out of a spec that named no work
        // at all. Missing `work` *and* missing `agent` is an error in both dialects.
        let neither = serde_json::json!({
            "constraint": "always",
            "restartability": "resumable",
            "priority": 0
        });
        let err = serde_json::from_value::<RunSpec>(neither).expect_err("must not be guessed at");
        assert!(
            err.to_string().contains("work"),
            "and it should name the field it wanted: {err}"
        );
    }

    #[test]
    fn a_work_kind_pads_to_a_column_width() {
        // `offload ps` prints this in a fixed-width column, and a `Display` written with
        // `write_str` ignores the width silently — `task` is one character shorter than
        // `agent`, so every column to its right shifts on task rows. Checkable, so checked.
        assert_eq!(format!("{:<5}", WorkKind::Task).len(), 5);
        assert_eq!(format!("{:<5}", WorkKind::Agent).len(), 5);
        assert_eq!(WorkKind::Task.to_string(), "task");
        assert_eq!(WorkKind::default(), WorkKind::Agent);
    }

    #[test]
    fn a_task_may_not_be_resumable_and_is_refused_at_submission() {
        // ADR-0019 §2: "resume" means resume a conversation, and a task has no transcript.
        // Refused where it is submitted rather than discovered at migration — by then the run
        // has already been placed and moved, which is the mistake open question #4 made about
        // `Portability::NodeLocal`.
        assert_eq!(
            a_task(Restartability::Resumable).check(),
            Err(SpecProblem::TaskCannotResume)
        );
        // The two that do mean something for a task are allowed, and an agent run is allowed at
        // every setting — the rule is about the pairing, not about either half alone.
        assert!(a_task(Restartability::Idempotent).check().is_ok());
        assert!(a_task(Restartability::Pinned).check().is_ok());
        assert!(run().spec.check().is_ok());
    }

    #[test]
    fn a_task_has_no_turn_budget_rather_than_a_spent_one() {
        // Absence, not zero: zero would read as "may take no turns at all", which is a
        // different sentence and one nothing here means (ADR-0019 §2).
        let task = a_task(Restartability::Idempotent);
        assert_eq!(task.turn_limit_reached(0), None);
        assert_eq!(task.turn_limit_reached(u32::MAX), None);
    }

    #[test]
    fn a_summary_row_describes_a_task_by_what_it_runs() {
        // `offload ps` shows a prompt for an agent run; a task has none, and an empty column
        // would read as a missing value rather than as an inapplicable one.
        assert_eq!(run().spec.work.summary(), "fix the parser");
        assert_eq!(
            a_task(Restartability::Idempotent).work.summary(),
            "watch-api --once"
        );
        assert_eq!(run().spec.work.kind(), WorkKind::Agent);
        assert_eq!(
            a_task(Restartability::Idempotent).work.kind(),
            WorkKind::Task
        );
    }

    fn run() -> Run {
        Run::new(
            RunId::from_bytes([1; 16]),
            RunSpec {
                work: Work::Agent(AgentWork {
                    agent: AgentKind::ClaudeCode,
                    model: None,
                    prompt: "fix the parser".into(),
                    workspace: WorkspaceSpec {
                        repo: "/repo".into(),
                        archive_bytes: None,
                        git_ref: None,
                        branch: None,
                    },
                    permission_mode: PermissionMode::Ask,
                    allow: crate::ToolAllowlist::default(),
                    max_turns: None,
                    ask: crate::ask::AskPolicy::Never,
                }),
                constraint: Constraint::Always,
                restartability: Restartability::Resumable,
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
            node(9),
            Millis(0),
        )
    }

    #[test]
    fn happy_path() {
        let mut r = run();
        let e = r.assign(node(1), Millis(0), TTL).expect("assign");
        r.started(node(1), e, Millis(10)).expect("start");
        r.renew(node(1), e, Millis(5_000), TTL).expect("renew");
        r.complete(node(1), e, Millis(6_000)).expect("complete");
        assert!(r.state.is_terminal());
    }

    #[test]
    fn stale_epoch_is_refused() {
        // The core safety property: a node that was reassigned away cannot keep acting.
        let mut r = run();
        let old = r.assign(node(1), Millis(0), TTL).expect("assign");
        r.started(node(1), old, Millis(1)).expect("start");
        r.orphan(Millis(20_000)).expect("orphan");
        let new = r.assign(node(2), Millis(20_000), TTL).expect("reassign");
        assert!(new > old);

        let err = r.complete(node(1), old, Millis(21_000)).unwrap_err();
        assert_eq!(
            err,
            TransitionError::StaleEpoch {
                current: new,
                presented: old
            }
        );
    }

    #[test]
    fn a_returning_holder_reclaims_without_migrating() {
        let mut r = run();
        let e = r.assign(node(1), Millis(0), TTL).expect("assign");
        r.started(node(1), e, Millis(1)).expect("start");
        r.orphan(Millis(11_000)).expect("orphan");

        r.reclaim(node(1), e, Millis(12_000), TTL).expect("reclaim");
        assert_eq!(r.epoch, e, "reclaim must not bump the epoch");
        assert_eq!(r.attempts, 1, "reclaim is not a new attempt");
        assert!(matches!(r.state, RunState::Running { .. }));
    }

    /// `Origin` crosses the wire, and a build that has never heard of it keeps everything.
    ///
    /// The gossip body is JSON and this field is what a node uses to decide whether a record and
    /// a checkout may be **thrown away** (ADR-0024), so both directions matter and they matter
    /// asymmetrically: a value that fails to survive is a peer that keeps too much, and a *record
    /// without the field* that decoded as anything but `Operator` would be an older node's run
    /// being reclaimed by a newer one. Round-tripped both ways for the reason `Response::Runs`
    /// exists as a struct variant — testing one direction is how that one shipped.
    #[test]
    fn origin_survives_the_wire_and_its_absence_means_an_operator() {
        let mut fired = run();
        fired.origin = Origin::Rule;
        let line = serde_json::to_string(&fired).expect("encode");
        assert_eq!(
            serde_json::from_str::<Run>(&line).expect("decode"),
            fired,
            "a rule's run stays one"
        );

        let mut without = serde_json::to_value(&fired).expect("value");
        without
            .as_object_mut()
            .expect("object")
            .remove("origin")
            .expect("there was one");
        let older: Run = serde_json::from_value(without).expect("a record from before the field");
        assert_eq!(
            older.origin,
            Origin::Operator,
            "the value that keeps things is the one an older record decodes to"
        );
    }

    #[test]
    fn a_different_node_cannot_reclaim() {
        let mut r = run();
        let e = r.assign(node(1), Millis(0), TTL).expect("assign");
        r.orphan(Millis(11_000)).expect("orphan");
        assert!(matches!(
            r.reclaim(node(2), e, Millis(12_000), TTL),
            Err(TransitionError::NotHolder { .. })
        ));
    }

    #[test]
    fn orphaned_grants_no_authority_to_act() {
        // The old holder must not be able to complete a run we may be about to move.
        let mut r = run();
        let e = r.assign(node(1), Millis(0), TTL).expect("assign");
        r.started(node(1), e, Millis(1)).expect("start");
        r.orphan(Millis(11_000)).expect("orphan");
        assert!(r.state.lease().is_none());
        assert!(r.complete(node(1), e, Millis(12_000)).is_err());
    }

    #[test]
    fn checkpoint_returns_the_run_to_the_pool() {
        let mut r = run();
        let e = r.assign(node(1), Millis(0), TTL).expect("assign");
        r.started(node(1), e, Millis(1)).expect("start");
        r.request_checkpoint(Millis(100)).expect("request");

        r.checkpointed(node(1), e, checkpoint(), GivenUp::Parked, Millis(200))
            .expect("checkpointed");

        assert!(matches!(r.state, RunState::Pending { .. }));
        assert!(r.checkpoint.is_some());
        // A person asked, so nobody let it go — the fleet leaves it for them (ADR-0042).
        assert_eq!(r.let_go_by(), None);

        // And it can then be picked up elsewhere, at a higher epoch.
        let e2 = r.assign(node(2), Millis(300), TTL).expect("reassign");
        assert!(e2 > e);
    }

    #[test]
    fn a_failed_run_can_be_reopened_and_keeps_its_checkpoint() {
        // The crash-recovery path: the daemon died mid-run, so the run is `Failed` with a
        // checkpoint behind it. Refusing to reopen that would discard the conversation and
        // the uncommitted edits it holds.
        let mut r = run();
        let e = r.assign(node(1), Millis(0), TTL).expect("assign");
        r.started(node(1), e, Millis(1)).expect("start");
        r.record_checkpoint(node(1), e, checkpoint())
            .expect("record");
        r.abandon("daemon restarted", Millis(100)).expect("abandon");

        r.reopen(Millis(200)).expect("reopen");
        assert!(matches!(r.state, RunState::Pending { .. }));
        assert!(r.checkpoint.is_some(), "the work is still there to resume");
        assert!(r.epoch > e, "the dead holder cannot act on it any more");
        assert!(r.assign(node(2), Millis(300), TTL).is_ok());
    }

    #[test]
    fn a_node_on_its_way_out_hands_a_failed_run_to_the_fleet_rather_than_a_person() {
        // ADR-0043, and the whole of the difference from `reopen`: the same state, the same
        // checkpoint, and a `let_go_by` that decides whether `supervise` offers the run or
        // passes it by as parked (ADR-0042).
        let mut r = run();
        let e = r.assign(node(1), Millis(0), TTL).expect("assign");
        r.started(node(1), e, Millis(1)).expect("start");
        r.record_checkpoint(node(1), e, checkpoint())
            .expect("record");
        r.fail(node(1), e, "the agent stopped", Millis(100))
            .expect("fail");

        r.let_go(node(1), Millis(200)).expect("let go");
        assert_eq!(r.let_go_by(), Some(node(1)));
        assert!(r.checkpoint.is_some(), "the conversation travels with it");
        assert!(r.epoch > e, "the leg that failed cannot act on it any more");
        assert!(r.assign(node(2), Millis(300), TTL).is_ok());

        // And the same run picked back up *here* says nobody let it go, because nobody did.
        let mut here = run();
        let e = here.assign(node(1), Millis(0), TTL).expect("assign");
        here.started(node(1), e, Millis(1)).expect("start");
        here.fail(node(1), e, "the agent stopped", Millis(100))
            .expect("fail");
        here.reopen(Millis(200)).expect("reopen");
        assert_eq!(here.let_go_by(), None);
    }

    #[test]
    fn only_a_failed_run_can_be_let_go() {
        // The same boundary `reopen` has, for the same reason: `Completed` and `Cancelled` are
        // decisions somebody made, and handing one to the fleet would restart work that was
        // deliberately stopped. A `Running` run is the drain's other path — it is checkpointed
        // at its next boundary and released there (ADR-0041), never dropped mid-turn.
        let mut completed = run();
        let e = completed.assign(node(1), Millis(0), TTL).expect("assign");
        completed.complete(node(1), e, Millis(1)).expect("complete");
        assert!(completed.let_go(node(1), Millis(2)).is_err());

        let mut cancelled = run();
        cancelled.assign(node(1), Millis(0), TTL).expect("assign");
        cancelled.cancel(Millis(1)).expect("cancel");
        assert!(cancelled.let_go(node(1), Millis(2)).is_err());

        let mut running = run();
        running.assign(node(1), Millis(0), TTL).expect("assign");
        assert!(running.let_go(node(1), Millis(1)).is_err());
    }

    #[test]
    fn only_failure_is_reopenable() {
        // Completed and cancelled are decisions, not accidents. Reopening either would
        // restart work somebody deliberately stopped.
        let mut completed = run();
        let e = completed.assign(node(1), Millis(0), TTL).expect("assign");
        completed.complete(node(1), e, Millis(1)).expect("complete");
        assert!(completed.reopen(Millis(2)).is_err());

        let mut cancelled = run();
        cancelled.assign(node(1), Millis(0), TTL).expect("assign");
        cancelled.cancel(Millis(1)).expect("cancel");
        assert!(cancelled.reopen(Millis(2)).is_err());

        let mut running = run();
        running.assign(node(1), Millis(0), TTL).expect("assign");
        assert!(running.reopen(Millis(1)).is_err());
    }

    #[test]
    fn pinned_runs_are_never_reassigned() {
        let mut r = run();
        r.spec.restartability = Restartability::Pinned;
        let e = r.assign(node(1), Millis(0), TTL).expect("assign");
        r.started(node(1), e, Millis(1)).expect("start");
        r.orphan(Millis(11_000)).expect("orphan");
        assert!(r.assign(node(2), Millis(12_000), TTL).is_err());
        // But its own holder may still come back.
        assert!(r.reclaim(node(1), e, Millis(12_000), TTL).is_ok());
    }

    /// Found by `tests/properties.rs`, from a two-act sequence: assign, then checkpoint.
    ///
    /// `Pending` has three entrances and one exit, and the exit refuses a pinned run. So a
    /// pinned run that reached the pool by any of the three was **stuck for ever** — not
    /// failed, not cancelled, not holdable by the hold-down, which only ever looks at
    /// `Orphaned`. `ps` said `pending`, the arbiter re-offered it, every grant was refused by
    /// `assign`, and nothing anywhere reported a problem. The rule was already written on
    /// `KeepReason::PinnedHere` and honoured only there.
    #[test]
    fn a_pinned_run_is_never_returned_to_a_pool_that_cannot_place_it() {
        let started = || {
            let mut r = run();
            r.spec.restartability = Restartability::Pinned;
            let e = r.assign(node(1), Millis(0), TTL).expect("assign");
            (r, e)
        };

        // Releasing a checkpoint: the drain path, and the one the property test found.
        let (mut r, e) = started();
        r.started(node(1), e, Millis(1)).expect("start");
        assert!(r
            .checkpointed(node(1), e, checkpoint(), GivenUp::Parked, Millis(2_000))
            .is_err());
        // …and the capture that goes with keeping it is still allowed.
        assert!(r.record_checkpoint(node(1), e, checkpoint()).is_ok());

        // Handing back a commitment: what a draining node does to a run it never started.
        let (mut r, e) = started();
        assert!(r.release(node(1), e, Millis(2_000)).is_err());

        // And an operator reopening it. A failed pinned run has no future either way, and
        // `failed` with a reason on it is the honest place to say so.
        let (mut r, e) = started();
        r.fail(node(1), e, "died", Millis(2_000)).expect("fail");
        assert!(r.reopen(Millis(3_000)).is_err());
        assert!(r.state.is_failed());
    }

    /// The one state that reopens keeps its checkpoint; the two that are decisions do not.
    ///
    /// Read alongside `Supervisor::resume`, which refuses `Completed` and `Cancelled` by name.
    /// Three callers asked this question as `is_terminal` and all three were wrong about `Failed`
    /// — the collector kept bytes nothing could reach, and `ps` and `offload explain` went quiet
    /// about durability at exactly the moment it decided whether the work survives its machine.
    #[test]
    fn a_checkpoint_stops_being_one_when_the_run_stopped_by_decision() {
        for (name, resumable) in [("completed", false), ("cancelled", false), ("failed", true)] {
            let mut r = run();
            let e = r.assign(node(1), Millis(0), TTL).expect("assign");
            r.started(node(1), e, Millis(1)).expect("start");
            r.record_checkpoint(node(1), e, checkpoint()).expect("cp");
            assert!(
                r.resumable_checkpoint().is_some(),
                "{name}: mid-run, a checkpoint is what a migration carries"
            );

            match name {
                "completed" => r.complete(node(1), e, Millis(2)).expect("complete"),
                "cancelled" => r.cancel(Millis(2)).expect("cancel"),
                _ => r.fail(node(1), e, "died", Millis(2)).expect("fail"),
            }

            assert_eq!(r.state.may_resume_later(), resumable, "{name}");
            assert_eq!(r.resumable_checkpoint().is_some(), resumable, "{name}");
            assert!(
                r.checkpoint.is_some(),
                "{name}: the field still records what the last capture was"
            );
        }
    }

    #[test]
    fn terminal_runs_reject_everything() {
        let mut r = run();
        let e = r.assign(node(1), Millis(0), TTL).expect("assign");
        r.started(node(1), e, Millis(1)).expect("start");
        r.complete(node(1), e, Millis(2)).expect("complete");

        assert!(r.renew(node(1), e, Millis(3), TTL).is_err());
        assert!(r.orphan(Millis(3)).is_err());
        assert!(r.cancel(Millis(3)).is_err());
    }
}
